use std::fs;
use std::io::Read;
use std::path::Path;

use ignore::WalkBuilder;
use oxyris_ipc::ops::{
    FsReadResult, FsStatResult, FsWalkArgs, FsWalkEvent, FsWalkResult, FsWriteResult,
};

use crate::ops::OpError;
use crate::protocol;

pub fn stat(path_str: &str) -> Result<FsStatResult, OpError> {
    let path = Path::new(path_str);
    match fs::symlink_metadata(path) {
        Ok(md) => Ok(FsStatResult {
            path: path_str.to_owned(),
            exists: true,
            is_dir: md.is_dir(),
            is_file: md.is_file(),
            is_symlink: md.file_type().is_symlink(),
            size: if md.is_file() { Some(md.len()) } else { None },
            modified_secs: md.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64)
            }),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FsStatResult {
            path: path_str.to_owned(),
            exists: false,
            is_dir: false,
            is_file: false,
            is_symlink: false,
            size: None,
            modified_secs: None,
        }),
        Err(e) => Err(OpError::Io(e)),
    }
}

const DEFAULT_READ_CAP: u64 = 1_048_576; // 1 MiB safety net.

pub fn read(path_str: &str, max_bytes: Option<u64>) -> Result<FsReadResult, OpError> {
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(OpError::NotFound(path_str.to_owned()));
    }
    let cap = max_bytes.unwrap_or(DEFAULT_READ_CAP);

    let mut file = fs::File::open(path)?;
    let mut buf = Vec::new();
    let take = (&mut file).take(cap);
    let mut limited = take;
    limited.read_to_end(&mut buf)?;
    let bytes_read = buf.len() as u64;

    let metadata = file.metadata()?;
    let truncated = metadata.len() > bytes_read;

    let content = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };

    Ok(FsReadResult {
        path: path_str.to_owned(),
        content,
        bytes_read,
        truncated,
    })
}

pub fn write(path_str: &str, contents: &str) -> Result<FsWriteResult, OpError> {
    let path = Path::new(path_str);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(FsWriteResult {
        path: path_str.to_owned(),
        bytes_written: contents.len() as u64,
    })
}

pub async fn walk(request_id: &str, args: FsWalkArgs) -> Result<FsWalkResult, OpError> {
    let root = Path::new(&args.root);
    if !root.exists() {
        return Err(OpError::NotFound(args.root.clone()));
    }
    let cap = args.max_entries.unwrap_or(u32::MAX);

    // Honor `.gitignore`, `.ignore`, and always-skip standard dirs.
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(false)
        .follow_links(false);
    for pat in &args.ignore {
        // Callers pass bare names like "node_modules" — interpret them as
        // leaf-name matches the walker will skip.
        builder.filter_entry({
            let pat = pat.clone();
            move |entry| entry.file_name().to_string_lossy() != pat.as_str()
        });
    }

    let mut count = 0u32;
    let mut truncated = false;

    for dent in builder.build() {
        let Ok(dent) = dent else { continue };
        if count >= cap {
            truncated = true;
            break;
        }
        let path = dent.path().to_string_lossy().into_owned();
        let is_dir = dent.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let size = dent
            .metadata()
            .ok()
            .and_then(|m| if m.is_file() { Some(m.len()) } else { None });
        protocol::emit_event(
            request_id,
            serde_json::to_value(FsWalkEvent { path, is_dir, size })
                .unwrap_or(serde_json::Value::Null),
        )
        .await;
        count += 1;
    }

    Ok(FsWalkResult { count, truncated })
}
