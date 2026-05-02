//! Worktree aggregate — one git worktree belonging to a project. A session
//! runs inside exactly one worktree so parallel sessions don't race on the
//! same working tree.

use chrono::{DateTime, Utc};
use oxyris_core::{Aggregate, AggregateId, DomainEvent};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct Worktree;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeState {
    pub inner: Option<WorktreeData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeData {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
    pub removed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub enum WorktreeCommand {
    Create {
        id: AggregateId,
        project_id: AggregateId,
        name: String,
        branch: String,
        path: String,
        is_primary: bool,
        now: DateTime<Utc>,
    },
    Remove {
        now: DateTime<Utc>,
    },
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum WorktreeEvent {
    WorktreeCreated {
        id: AggregateId,
        project_id: AggregateId,
        name: String,
        branch: String,
        path: String,
        is_primary: bool,
        created_at: DateTime<Utc>,
    },
    WorktreeRemoved {
        removed_at: DateTime<Utc>,
    },
}

impl DomainEvent for WorktreeEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::WorktreeCreated { .. } => "WorktreeCreated",
            Self::WorktreeRemoved { .. } => "WorktreeRemoved",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorktreeError {
    #[error("worktree already exists")]
    AlreadyExists,
    #[error("worktree not found")]
    NotFound,
    #[error("worktree is already removed")]
    AlreadyRemoved,
    #[error("cannot remove the primary worktree")]
    CannotRemovePrimary,
    #[error("name, branch, and path must be non-empty")]
    InvalidFields,
}

impl Aggregate for Worktree {
    const KIND: &'static str = "worktree";
    type Command = WorktreeCommand;
    type Event = WorktreeEvent;
    type State = WorktreeState;
    type Error = WorktreeError;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error> {
        match cmd {
            WorktreeCommand::Create {
                id,
                project_id,
                name,
                branch,
                path,
                is_primary,
                now,
            } => {
                if state.inner.is_some() {
                    return Err(WorktreeError::AlreadyExists);
                }
                if name.trim().is_empty() || branch.trim().is_empty() || path.trim().is_empty() {
                    return Err(WorktreeError::InvalidFields);
                }
                Ok(vec![WorktreeEvent::WorktreeCreated {
                    id,
                    project_id,
                    name,
                    branch,
                    path,
                    is_primary,
                    created_at: now,
                }])
            }
            WorktreeCommand::Remove { now } => {
                let data = state.inner.as_ref().ok_or(WorktreeError::NotFound)?;
                if data.removed_at.is_some() {
                    return Err(WorktreeError::AlreadyRemoved);
                }
                if data.is_primary {
                    return Err(WorktreeError::CannotRemovePrimary);
                }
                Ok(vec![WorktreeEvent::WorktreeRemoved { removed_at: now }])
            }
        }
    }

    fn apply(state: &mut Self::State, event: &Self::Event) {
        match event {
            WorktreeEvent::WorktreeCreated {
                id,
                project_id,
                name,
                branch,
                path,
                is_primary,
                created_at,
            } => {
                state.inner = Some(WorktreeData {
                    id: *id,
                    project_id: *project_id,
                    name: name.clone(),
                    branch: branch.clone(),
                    path: path.clone(),
                    is_primary: *is_primary,
                    created_at: *created_at,
                    removed_at: None,
                });
            }
            WorktreeEvent::WorktreeRemoved { removed_at } => {
                if let Some(data) = state.inner.as_mut() {
                    data.removed_at = Some(*removed_at);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn create(is_primary: bool) -> WorktreeCommand {
        WorktreeCommand::Create {
            id: AggregateId::new(),
            project_id: AggregateId::new(),
            name: "feature-a".into(),
            branch: "feature-a".into(),
            path: r"C:\dev\proj\.oxyris\worktrees\feature-a".into(),
            is_primary,
            now: now(),
        }
    }

    #[test]
    fn primary_cant_be_removed() {
        let mut state = WorktreeState::default();
        for e in Worktree::decide(&state, create(true)).unwrap() {
            Worktree::apply(&mut state, &e);
        }
        assert_eq!(
            Worktree::decide(&state, WorktreeCommand::Remove { now: now() }),
            Err(WorktreeError::CannotRemovePrimary)
        );
    }

    #[test]
    fn non_primary_remove_sets_removed_at() {
        let mut state = WorktreeState::default();
        for e in Worktree::decide(&state, create(false)).unwrap() {
            Worktree::apply(&mut state, &e);
        }
        for e in Worktree::decide(&state, WorktreeCommand::Remove { now: now() }).unwrap() {
            Worktree::apply(&mut state, &e);
        }
        assert!(state.inner.unwrap().removed_at.is_some());
    }

    #[test]
    fn empty_fields_rejected() {
        let cmd = WorktreeCommand::Create {
            id: AggregateId::new(),
            project_id: AggregateId::new(),
            name: "".into(),
            branch: "feature".into(),
            path: "p".into(),
            is_primary: false,
            now: now(),
        };
        assert_eq!(
            Worktree::decide(&WorktreeState::default(), cmd),
            Err(WorktreeError::InvalidFields)
        );
    }
}
