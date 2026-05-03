mod fs;
mod git;
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
        }
    }
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
            let result = fs::write(&args.path, &args.contents)?;
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
        op_name::FS_SEARCH_PATHS => {
            let args: oxyris_ipc::ops::FsSearchPathsArgs = from_args(args)?;
            let result = fs::search_paths(&args.root, &args.query, args.limit)?;
            Ok(serde_json::to_value(result)?)
        }
        op_name::GIT_LIST_BRANCHES => git::list_branches(from_args(args)?),
        op_name::GIT_LIST_WORKTREES => git::list_worktrees(from_args(args)?),
        op_name::GIT_CREATE_WORKTREE => git::create_worktree(from_args(args)?),
        op_name::GIT_REMOVE_WORKTREE => git::remove_worktree(from_args(args)?),
        op_name::GIT_CHECKPOINT_CAPTURE => git::checkpoint_capture(from_args(args)?),
        op_name::GIT_CHECKPOINT_DIFF => git::checkpoint_diff(from_args(args)?),
        op_name::GIT_CHECKPOINT_REVERT => git::checkpoint_revert(from_args(args)?),
        op_name::GIT_STATUS => git::status(from_args(args)?),
        op_name::GIT_DIFF_FILE => git::diff_file(from_args(args)?),
        op_name::GIT_STAGE => git::stage(from_args(args)?),
        op_name::GIT_UNSTAGE => git::unstage(from_args(args)?),
        op_name::GIT_COMMIT => git::commit(from_args(args)?),
        op_name::GIT_FETCH => git::fetch(from_args(args)?),
        op_name::GIT_PULL => git::pull(from_args(args)?),
        op_name::GIT_PUSH => git::push(from_args(args)?),
        op_name::GIT_CHECKOUT => git::checkout(from_args(args)?),
        op_name::GIT_BRANCH_CREATE => git::branch_create(from_args(args)?),
        op_name::GIT_BRANCH_DELETE => git::branch_delete(from_args(args)?),
        op_name::GIT_LOG => git::log(from_args(args)?),
        op_name::GIT_GET_CONFLICT => git::get_conflict(from_args(args)?),
        op_name::GIT_RESOLVE => git::resolve(from_args(args)?),
        op_name::GIT_APPLY_PATCH => git::apply_patch(from_args(args)?),
        op_name::GIT_STASH_LIST => git::stash_list(from_args(args)?),
        op_name::GIT_STASH_SAVE => git::stash_save(from_args(args)?),
        op_name::GIT_STASH_APPLY => git::stash_apply(from_args(args)?),
        op_name::GIT_STASH_DROP => git::stash_drop(from_args(args)?),
        op_name::GIT_TAG_LIST => git::tag_list(from_args(args)?),
        op_name::GIT_TAG_CREATE => git::tag_create(from_args(args)?),
        op_name::GIT_TAG_DELETE => git::tag_delete(from_args(args)?),
        op_name::GIT_CHERRY_PICK => git::cherry_pick(from_args(args)?),
        op_name::GIT_REVERT => git::revert(from_args(args)?),
        op_name::GIT_DIFF_REVS => git::diff_revs(from_args(args)?),
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
