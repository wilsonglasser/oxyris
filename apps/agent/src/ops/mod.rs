mod fs;
mod git;
pub mod index;
mod system;

use oxyris_ipc::RequestFrame;
use oxyris_ipc::ops::op_name;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OpError {
    #[error("unknown op: {0}")]
    UnknownOp(String),
    #[error("invalid args: {0}")]
    InvalidArgs(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("git: {0}")]
    Git(String),
    #[error("repository has no commits yet")]
    EmptyRepo,
    #[error("watch: {0}")]
    Watch(String),
    #[error("index: {0}")]
    Index(String),
}

impl OpError {
    pub fn code(&self) -> &'static str {
        match self {
            OpError::UnknownOp(_) => "unknown_op",
            OpError::InvalidArgs(_) => "invalid_args",
            OpError::Io(_) => "io",
            OpError::NotFound(_) => "not_found",
            OpError::Git(_) => "git",
            OpError::EmptyRepo => "empty_repo",
            OpError::Watch(_) => "watch",
            OpError::Index(_) => "index",
        }
    }
}

pub use fs::start_watch;

/// Run a synchronous op off the runtime thread. The agent runs on a
/// `current_thread` runtime and dispatches requests concurrently, so a git op
/// that blocks (a fetch over a slow link, a status walk of a big repo) must
/// not pin the thread every other request shares.
async fn blocking<F>(f: F) -> Result<serde_json::Value, OpError>
where
    F: FnOnce() -> Result<serde_json::Value, OpError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| OpError::Io(std::io::Error::other(e)))?
}

pub async fn dispatch(req: RequestFrame) -> Result<serde_json::Value, OpError> {
    let args = req.args;
    let id = req.id;
    match req.op.as_str() {
        op_name::SYSTEM_INFO => {
            let _: oxyris_ipc::ops::SystemInfoArgs = from_args(args)?;
            let result = system::info();
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_STAT => {
            let args: oxyris_ipc::ops::FsStatArgs = from_args(args)?;
            let result = fs::stat(&args.path)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_READ => {
            let args: oxyris_ipc::ops::FsReadArgs = from_args(args)?;
            let result = fs::read(&args.path, args.max_bytes)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_WRITE => {
            let args: oxyris_ipc::ops::FsWriteArgs = from_args(args)?;
            let result = fs::write(&args.path, &args.contents, args.mode)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_WALK => {
            let args: oxyris_ipc::ops::FsWalkArgs = from_args(args)?;
            let result = fs::walk(&id, args).await?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_LIST_DIR => {
            let args: oxyris_ipc::ops::FsListDirArgs = from_args(args)?;
            let result = fs::list_dir(&args.path, args.show_hidden)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_CREATE_FILE => {
            let args: oxyris_ipc::ops::FsCreateFileArgs = from_args(args)?;
            fs::create_file(&args.path, &args.contents)?;
            Ok(serde_json::Value::Null)
        }
        op_name::FS_CREATE_DIR => {
            let args: oxyris_ipc::ops::FsPathArgs = from_args(args)?;
            fs::create_dir(&args.path)?;
            Ok(serde_json::Value::Null)
        }
        op_name::FS_RENAME => {
            let args: oxyris_ipc::ops::FsRenameArgs = from_args(args)?;
            fs::rename(&args.from, &args.to)?;
            Ok(serde_json::Value::Null)
        }
        op_name::FS_COPY => {
            let args: oxyris_ipc::ops::FsCopyArgs = from_args(args)?;
            fs::copy(&args.from, &args.to)?;
            Ok(serde_json::Value::Null)
        }
        op_name::FS_DELETE => {
            let args: oxyris_ipc::ops::FsDeleteArgs = from_args(args)?;
            fs::delete(&args.path, args.recursive)?;
            Ok(serde_json::Value::Null)
        }
        op_name::FS_READ_BYTES => {
            let args: oxyris_ipc::ops::FsReadBytesArgs = from_args(args)?;
            let result = fs::read_bytes(&args.path, args.max_bytes)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_WRITE_BYTES => {
            let args: oxyris_ipc::ops::FsWriteBytesArgs = from_args(args)?;
            let result = fs::write_bytes(&args.path, &args.bytes_b64)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_SEARCH_PATHS => {
            let args: oxyris_ipc::ops::FsSearchPathsArgs = from_args(args)?;
            let result = fs::search_paths(&args.root, &args.query, args.limit)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::FS_SEARCH_CONTENT => {
            let args: oxyris_ipc::ops::FsSearchContentArgs = from_args(args)?;
            let result = fs::search_content(&args)?;
            Ok(serde_json::to_value(result)?)
        }
        // fs.watch is handled out-of-band in main.rs (it must not emit a
        // Result frame); only fs.unwatch flows through normal dispatch.
        op_name::FS_UNWATCH => {
            let args: oxyris_ipc::ops::FsUnwatchArgs = from_args(args)?;
            fs::stop_watch(args)
        }
        op_name::GIT_LIST_BRANCHES => blocking(move || git::list_branches(from_args(args)?)).await,
        op_name::GIT_LIST_WORKTREES => {
            blocking(move || git::list_worktrees(from_args(args)?)).await
        }
        op_name::GIT_CREATE_WORKTREE => {
            blocking(move || git::create_worktree(from_args(args)?)).await
        }
        op_name::GIT_REMOVE_WORKTREE => {
            blocking(move || git::remove_worktree(from_args(args)?)).await
        }
        op_name::GIT_CHECKPOINT_CAPTURE => {
            blocking(move || git::checkpoint_capture(from_args(args)?)).await
        }
        op_name::GIT_CHECKPOINT_DIFF => {
            blocking(move || git::checkpoint_diff(from_args(args)?)).await
        }
        op_name::GIT_CHECKPOINT_REVERT => {
            blocking(move || git::checkpoint_revert(from_args(args)?)).await
        }
        op_name::GIT_STATUS => blocking(move || git::status(from_args(args)?)).await,
        op_name::GIT_DIFF_FILE => blocking(move || git::diff_file(from_args(args)?)).await,
        op_name::GIT_STAGE => blocking(move || git::stage(from_args(args)?)).await,
        op_name::GIT_UNSTAGE => blocking(move || git::unstage(from_args(args)?)).await,
        op_name::GIT_COMMIT => blocking(move || git::commit(from_args(args)?)).await,
        op_name::GIT_CLONE => blocking(move || git::clone(from_args(args)?)).await,
        op_name::GIT_FETCH => blocking(move || git::fetch(from_args(args)?)).await,
        op_name::GIT_PULL => blocking(move || git::pull(from_args(args)?)).await,
        op_name::GIT_PUSH => blocking(move || git::push(from_args(args)?)).await,
        op_name::GIT_CHECKOUT => blocking(move || git::checkout(from_args(args)?)).await,
        op_name::GIT_BRANCH_CREATE => blocking(move || git::branch_create(from_args(args)?)).await,
        op_name::GIT_BRANCH_DELETE => blocking(move || git::branch_delete(from_args(args)?)).await,
        op_name::GIT_BRANCH_LIST_DETAILED => {
            blocking(move || git::branch_list_detailed(from_args(args)?)).await
        }
        op_name::GIT_BRANCH_RENAME => blocking(move || git::branch_rename(from_args(args)?)).await,
        op_name::GIT_BRANCH_DELETE_REMOTE => {
            blocking(move || git::branch_delete_remote(from_args(args)?)).await
        }
        op_name::GIT_CHECKOUT_REMOTE => {
            blocking(move || git::checkout_remote(from_args(args)?)).await
        }
        op_name::GIT_PUSH_DELETE => blocking(move || git::push_delete(from_args(args)?)).await,
        op_name::GIT_MERGE => blocking(move || git::merge(from_args(args)?)).await,
        op_name::GIT_MERGE_ABORT => blocking(move || git::merge_abort(from_args(args)?)).await,
        op_name::GIT_REBASE => blocking(move || git::rebase(from_args(args)?)).await,
        op_name::GIT_REBASE_CONTINUE => {
            blocking(move || git::rebase_continue(from_args(args)?)).await
        }
        op_name::GIT_REBASE_ABORT => blocking(move || git::rebase_abort(from_args(args)?)).await,
        op_name::GIT_LOG => blocking(move || git::log(from_args(args)?)).await,
        op_name::GIT_GET_CONFLICT => blocking(move || git::get_conflict(from_args(args)?)).await,
        op_name::GIT_RESOLVE => blocking(move || git::resolve(from_args(args)?)).await,
        op_name::GIT_APPLY_PATCH => blocking(move || git::apply_patch(from_args(args)?)).await,
        op_name::GIT_STASH_LIST => blocking(move || git::stash_list(from_args(args)?)).await,
        op_name::GIT_STASH_SAVE => blocking(move || git::stash_save(from_args(args)?)).await,
        op_name::GIT_STASH_APPLY => blocking(move || git::stash_apply(from_args(args)?)).await,
        op_name::GIT_STASH_DROP => blocking(move || git::stash_drop(from_args(args)?)).await,
        op_name::GIT_TAG_LIST => blocking(move || git::tag_list(from_args(args)?)).await,
        op_name::GIT_TAG_CREATE => blocking(move || git::tag_create(from_args(args)?)).await,
        op_name::GIT_TAG_DELETE => blocking(move || git::tag_delete(from_args(args)?)).await,
        op_name::GIT_CHERRY_PICK => blocking(move || git::cherry_pick(from_args(args)?)).await,
        op_name::GIT_REVERT => blocking(move || git::revert(from_args(args)?)).await,
        op_name::GIT_DIFF_REVS => blocking(move || git::diff_revs(from_args(args)?)).await,
        op_name::INDEX_ENSURE => {
            let a: oxyris_ipc::ops::IndexRootArgs = from_args(args)?;
            index::ensure(a.root).await
        }
        op_name::INDEX_REBUILD => {
            let a: oxyris_ipc::ops::IndexRootArgs = from_args(args)?;
            index::rebuild(&id, a.root).await
        }
        op_name::INDEX_QUERY_SYMBOL => {
            let a: oxyris_ipc::ops::IndexQuerySymbolArgs = from_args(args)?;
            index::query_symbol(a.root, a.name, a.kind, a.limit).await
        }
        op_name::INDEX_LIST_IN_FILE => {
            let a: oxyris_ipc::ops::IndexListInFileArgs = from_args(args)?;
            index::list_in_file(a.root, a.file).await
        }
        op_name::INDEX_PROJECT_MAP => {
            let a: oxyris_ipc::ops::IndexRootArgs = from_args(args)?;
            index::project_map(a.root).await
        }
        op_name::INDEX_STATS => {
            let a: oxyris_ipc::ops::IndexRootArgs = from_args(args)?;
            index::stats(a.root).await
        }
        other => Err(OpError::UnknownOp(other.to_owned())),
    }
}

fn from_args<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, serde_json::Error> {
    let v = if v.is_null() {
        serde_json::Value::Object(Default::default())
    } else {
        v
    };
    serde_json::from_value(v)
}
