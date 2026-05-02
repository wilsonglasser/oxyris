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
        op_name::GIT_LIST_BRANCHES => git::list_branches(from_args(args)?),
        op_name::GIT_LIST_WORKTREES => git::list_worktrees(from_args(args)?),
        op_name::GIT_CREATE_WORKTREE => git::create_worktree(from_args(args)?),
        op_name::GIT_REMOVE_WORKTREE => git::remove_worktree(from_args(args)?),
        op_name::GIT_CHECKPOINT_CAPTURE => git::checkpoint_capture(from_args(args)?),
        op_name::GIT_CHECKPOINT_DIFF => git::checkpoint_diff(from_args(args)?),
        op_name::GIT_CHECKPOINT_REVERT => git::checkpoint_revert(from_args(args)?),
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
