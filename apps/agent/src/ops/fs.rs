use std::fs;
use std::io::Read;
use std::path::Path;

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use oxyris_ipc::ops::{
    FsContentFileHits, FsContentMatch, FsListDirEntry, FsListDirResult, FsReadBytesResult,
    FsReadResult, FsSearchContentArgs, FsSearchContentResult, FsSearchHit, FsSearchPathsResult,
    FsStatResult, FsWalkArgs, FsWalkEvent, FsWalkResult, FsWriteResult,
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

pub fn write_bytes(path_str: &str, bytes_b64: &str) -> Result<FsWriteResult, OpError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_b64.as_bytes())
        .map_err(|e| OpError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    let path = Path::new(path_str);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, &bytes)?;
    Ok(FsWriteResult {
        path: path_str.to_owned(),
        bytes_written: bytes.len() as u64,
    })
}

pub fn list_dir(path_str: &str, show_hidden: bool) -> Result<FsListDirResult, OpError> {
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(OpError::NotFound(path_str.to_owned()));
    }
    let mut entries = Vec::new();
    for dent in fs::read_dir(path)? {
        let dent = dent?;
        let name = dent.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let ft = match dent.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let md = dent.metadata().ok();
        entries.push(FsListDirEntry {
            name,
            is_dir: ft.is_dir(),
            is_symlink: ft.is_symlink(),
            size: md
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None }),
            modified_secs: md.as_ref().and_then(|m| {
                m.modified().ok().and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64)
                })
            }),
        });
    }
    // Dirs first, then files; both case-insensitive alphabetical.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FsListDirResult {
        path: path_str.to_owned(),
        entries,
    })
}

pub fn create_file(path_str: &str, contents: &str) -> Result<(), OpError> {
    let path = Path::new(path_str);
    if path.exists() {
        return Err(OpError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("already exists: {path_str}"),
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

pub fn create_dir(path_str: &str) -> Result<(), OpError> {
    let path = Path::new(path_str);
    fs::create_dir_all(path)?;
    Ok(())
}

pub fn rename(from_str: &str, to_str: &str) -> Result<(), OpError> {
    let from = Path::new(from_str);
    let to = Path::new(to_str);
    if !from.exists() {
        return Err(OpError::NotFound(from_str.to_owned()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, to)?;
    Ok(())
}

pub fn copy(from_str: &str, to_str: &str) -> Result<(), OpError> {
    let from = Path::new(from_str);
    let to = Path::new(to_str);
    if !from.exists() {
        return Err(OpError::NotFound(from_str.to_owned()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    let md = fs::symlink_metadata(from)?;
    if md.is_dir() {
        copy_dir_recursive(from, to)?;
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), OpError> {
    fs::create_dir_all(to)?;
    for dent in fs::read_dir(from)? {
        let dent = dent?;
        let src = dent.path();
        let dst = to.join(dent.file_name());
        if dent.file_type()?.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

pub fn delete(path_str: &str, recursive: bool) -> Result<(), OpError> {
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(OpError::NotFound(path_str.to_owned()));
    }
    let md = fs::symlink_metadata(path)?;
    if md.is_dir() {
        if recursive {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_dir(path)?;
        }
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn read_bytes(path_str: &str, max_bytes: Option<u64>) -> Result<FsReadBytesResult, OpError> {
    use base64::Engine;
    use std::io::Read;
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(OpError::NotFound(path_str.to_owned()));
    }
    let cap = max_bytes.unwrap_or(16 * 1024 * 1024);
    let mut file = fs::File::open(path)?;
    let total = file.metadata()?.len();
    let mut buf = Vec::with_capacity(cap.min(total) as usize);
    (&mut file).take(cap).read_to_end(&mut buf)?;
    let bytes_read = buf.len() as u64;
    let bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(FsReadBytesResult {
        path: path_str.to_owned(),
        bytes_b64,
        bytes_read,
        truncated: total > bytes_read,
    })
}

pub fn search_paths(
    root_str: &str,
    query: &str,
    limit: u32,
) -> Result<FsSearchPathsResult, OpError> {
    let root = Path::new(root_str);
    if !root.exists() {
        return Err(OpError::NotFound(root_str.to_owned()));
    }
    let q_lower = query.to_lowercase();
    let mut hits: Vec<FsSearchHit> = Vec::new();
    let mut walked = 0u32;
    let mut truncated = false;
    const WALK_CAP: u32 = 20_000;

    for dent in WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .follow_links(false)
        .build()
    {
        let Ok(dent) = dent else { continue };
        walked += 1;
        if walked > WALK_CAP {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Ok(rel) = dent.path().strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !q_lower.is_empty() {
            let hay = rel_str.to_lowercase();
            let Some(_idx) = hay.find(&q_lower) else {
                continue;
            };
            // Score: prefer matches in the basename, then earlier matches.
            let basename = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let in_base = basename.to_lowercase().contains(&q_lower);
            let depth = rel.components().count() as i32;
            let score = if in_base { depth } else { depth + 100 };
            hits.push(FsSearchHit {
                rel_path: rel_str,
                score,
            });
        } else {
            hits.push(FsSearchHit {
                rel_path: rel_str,
                score: rel.components().count() as i32,
            });
        }
    }

    hits.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| a.rel_path.len().cmp(&b.rel_path.len()))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    let cap = limit as usize;
    if hits.len() > cap {
        hits.truncate(cap);
        truncated = true;
    }
    Ok(FsSearchPathsResult { hits, truncated })
}

/// Caps shared by the content searcher. Files bigger than `MAX_FILE_BYTES`
/// or containing NUL bytes (binary) are skipped; lines are truncated to
/// `MAX_LINE_LEN` so a minified bundle can't blow up the payload.
const SEARCH_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const SEARCH_WALK_CAP: u32 = 50_000;
const SEARCH_MAX_LINE_LEN: usize = 500;

/// Build the line matcher from the search flags. Non-regex queries are
/// escaped so metacharacters are literal; `whole_word` wraps in `\b…\b`.
fn build_content_matcher(args: &FsSearchContentArgs) -> Result<regex::Regex, String> {
    if args.query.is_empty() {
        return Err("empty query".to_owned());
    }
    let base = if args.is_regex {
        args.query.clone()
    } else {
        regex::escape(&args.query)
    };
    let pat = if args.whole_word {
        format!(r"\b(?:{base})\b")
    } else {
        base
    };
    regex::RegexBuilder::new(&pat)
        .case_insensitive(!args.case_sensitive)
        .build()
        .map_err(|e| e.to_string())
}

/// Truncate `line` to at most `SEARCH_MAX_LINE_LEN` bytes on a char boundary.
fn cap_line(line: &str) -> String {
    if line.len() <= SEARCH_MAX_LINE_LEN {
        return line.to_owned();
    }
    let mut end = SEARCH_MAX_LINE_LEN;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

pub fn search_content(args: &FsSearchContentArgs) -> Result<FsSearchContentResult, OpError> {
    let root = Path::new(&args.root);
    if !root.exists() {
        return Err(OpError::NotFound(args.root.clone()));
    }
    let re = build_content_matcher(args)
        .map_err(|e| OpError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(false)
        .follow_links(false);
    if let Some(mask) = args.include_glob.as_deref() {
        let mut ob = OverrideBuilder::new(root);
        for pat in mask.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let _ = ob.add(pat);
        }
        if let Ok(ov) = ob.build() {
            builder.overrides(ov);
        }
    }

    let max_results = args.max_results.max(1);
    let mut files: Vec<FsContentFileHits> = Vec::new();
    let mut total = 0u32;
    let mut truncated = false;
    let mut walked = 0u32;

    for dent in builder.build() {
        let Ok(dent) = dent else { continue };
        walked += 1;
        if walked > SEARCH_WALK_CAP {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(md) = dent.metadata()
            && md.len() > SEARCH_MAX_FILE_BYTES
        {
            continue;
        }
        let path = dent.path();
        let Ok(bytes) = fs::read(path) else { continue };
        // Heuristic binary sniff: a NUL byte near the head means skip.
        if bytes.iter().take(8000).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        let mut matches = Vec::new();
        let mut hit_cap = false;
        for (i, line) in text.lines().enumerate() {
            if total >= max_results {
                hit_cap = true;
                break;
            }
            if re.is_match(line) {
                matches.push(FsContentMatch {
                    line: (i as u32) + 1,
                    text: cap_line(line),
                });
                total += 1;
            }
        }
        if !matches.is_empty() {
            files.push(FsContentFileHits {
                rel_path: rel_str,
                matches,
            });
        }
        if hit_cap {
            truncated = true;
            break;
        }
    }

    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(FsSearchContentResult {
        files,
        total_matches: total,
        truncated,
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
