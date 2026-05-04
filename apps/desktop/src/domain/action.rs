//! Action aggregate — a user-defined shell command attached to a project.
//!
//! Each action has a friendly name, the command to run, an optional
//! keybinding, and a flag for "run automatically when a new worktree is
//! created". Stored as individual aggregates so edits / deletes are
//! event-sourced like everything else.

use chrono::{DateTime, Utc};
use oxyris_core::{Aggregate, AggregateId, DomainEvent};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct Action;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionState {
    pub inner: Option<ActionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionData {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub name: String,
    pub command: String,
    pub keybinding: Option<String>,
    pub auto_run_on_worktree_create: bool,
    /// Lucide icon name (e.g. "Terminal", "Play", "GitBranch"). Frontend
    /// renders the matching icon component; unknown names fall back to a
    /// generic placeholder.
    #[serde(default = "default_icon")]
    pub icon: String,
    /// Execution mode — `terminal_command` (sends to a terminal pane),
    /// `one_shot` (captures stdout / stderr in a modal), or
    /// `github_workflow` (shells out to `gh workflow run`).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// When false, the action only fires via shortcut / auto-run / list
    /// modal — its icon is hidden from the right sidebar.
    #[serde(default = "default_true")]
    pub show_in_sidebar: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub removed_at: Option<DateTime<Utc>>,
}

fn default_icon() -> String {
    "Terminal".into()
}

fn default_kind() -> String {
    "terminal_command".into()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub enum ActionCommand {
    Register {
        id: AggregateId,
        project_id: AggregateId,
        name: String,
        command: String,
        keybinding: Option<String>,
        auto_run_on_worktree_create: bool,
        icon: String,
        kind: String,
        show_in_sidebar: bool,
        now: DateTime<Utc>,
    },
    Update {
        name: String,
        command: String,
        keybinding: Option<String>,
        auto_run_on_worktree_create: bool,
        icon: String,
        kind: String,
        show_in_sidebar: bool,
        now: DateTime<Utc>,
    },
    Remove {
        now: DateTime<Utc>,
    },
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum ActionEvent {
    ActionRegistered {
        id: AggregateId,
        project_id: AggregateId,
        name: String,
        command: String,
        keybinding: Option<String>,
        auto_run_on_worktree_create: bool,
        #[serde(default = "default_icon")]
        icon: String,
        // Renamed-on-the-wire: the variant tag is `kind`, so the column
        // name on the event has to be different. `action_kind` keeps it
        // parseable in either direction with serde default for old
        // events.
        #[serde(default = "default_kind", rename = "action_kind")]
        kind: String,
        #[serde(default = "default_true")]
        show_in_sidebar: bool,
        created_at: DateTime<Utc>,
    },
    ActionUpdated {
        name: String,
        command: String,
        keybinding: Option<String>,
        auto_run_on_worktree_create: bool,
        #[serde(default = "default_icon")]
        icon: String,
        #[serde(default = "default_kind", rename = "action_kind")]
        kind: String,
        #[serde(default = "default_true")]
        show_in_sidebar: bool,
        updated_at: DateTime<Utc>,
    },
    ActionRemoved {
        removed_at: DateTime<Utc>,
    },
}

impl DomainEvent for ActionEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::ActionRegistered { .. } => "ActionRegistered",
            Self::ActionUpdated { .. } => "ActionUpdated",
            Self::ActionRemoved { .. } => "ActionRemoved",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActionError {
    #[error("action already exists")]
    AlreadyExists,
    #[error("action not found")]
    NotFound,
    #[error("action already removed")]
    AlreadyRemoved,
    #[error("name and command must be non-empty")]
    InvalidFields,
}

impl Aggregate for Action {
    const KIND: &'static str = "action";
    type Command = ActionCommand;
    type Event = ActionEvent;
    type State = ActionState;
    type Error = ActionError;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error> {
        match cmd {
            ActionCommand::Register {
                id,
                project_id,
                name,
                command,
                keybinding,
                auto_run_on_worktree_create,
                icon,
                kind,
                show_in_sidebar,
                now,
            } => {
                if state.inner.is_some() {
                    return Err(ActionError::AlreadyExists);
                }
                if name.trim().is_empty() || command.trim().is_empty() {
                    return Err(ActionError::InvalidFields);
                }
                Ok(vec![ActionEvent::ActionRegistered {
                    id,
                    project_id,
                    name,
                    command,
                    keybinding,
                    auto_run_on_worktree_create,
                    icon,
                    kind,
                    show_in_sidebar,
                    created_at: now,
                }])
            }
            ActionCommand::Update {
                name,
                command,
                keybinding,
                auto_run_on_worktree_create,
                icon,
                kind,
                show_in_sidebar,
                now,
            } => {
                let data = state.inner.as_ref().ok_or(ActionError::NotFound)?;
                if data.removed_at.is_some() {
                    return Err(ActionError::AlreadyRemoved);
                }
                if name.trim().is_empty() || command.trim().is_empty() {
                    return Err(ActionError::InvalidFields);
                }
                Ok(vec![ActionEvent::ActionUpdated {
                    name,
                    command,
                    keybinding,
                    auto_run_on_worktree_create,
                    icon,
                    kind,
                    show_in_sidebar,
                    updated_at: now,
                }])
            }
            ActionCommand::Remove { now } => {
                let data = state.inner.as_ref().ok_or(ActionError::NotFound)?;
                if data.removed_at.is_some() {
                    return Err(ActionError::AlreadyRemoved);
                }
                Ok(vec![ActionEvent::ActionRemoved { removed_at: now }])
            }
        }
    }

    fn apply(state: &mut Self::State, event: &Self::Event) {
        match event {
            ActionEvent::ActionRegistered {
                id,
                project_id,
                name,
                command,
                keybinding,
                auto_run_on_worktree_create,
                icon,
                kind,
                show_in_sidebar,
                created_at,
            } => {
                state.inner = Some(ActionData {
                    id: *id,
                    project_id: *project_id,
                    name: name.clone(),
                    command: command.clone(),
                    keybinding: keybinding.clone(),
                    auto_run_on_worktree_create: *auto_run_on_worktree_create,
                    icon: icon.clone(),
                    kind: kind.clone(),
                    show_in_sidebar: *show_in_sidebar,
                    created_at: *created_at,
                    updated_at: *created_at,
                    removed_at: None,
                });
            }
            ActionEvent::ActionUpdated {
                name,
                command,
                keybinding,
                auto_run_on_worktree_create,
                icon,
                kind,
                show_in_sidebar,
                updated_at,
            } => {
                if let Some(data) = state.inner.as_mut() {
                    data.name = name.clone();
                    data.command = command.clone();
                    data.keybinding = keybinding.clone();
                    data.auto_run_on_worktree_create = *auto_run_on_worktree_create;
                    data.icon = icon.clone();
                    data.kind = kind.clone();
                    data.show_in_sidebar = *show_in_sidebar;
                    data.updated_at = *updated_at;
                }
            }
            ActionEvent::ActionRemoved { removed_at } => {
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

    fn register() -> ActionCommand {
        ActionCommand::Register {
            id: AggregateId::new(),
            project_id: AggregateId::new(),
            name: "Build".into(),
            command: "bun run build".into(),
            keybinding: Some("Ctrl+Shift+B".into()),
            auto_run_on_worktree_create: true,
            icon: "Hammer".into(),
            kind: "terminal_command".into(),
            show_in_sidebar: true,
            now: now(),
        }
    }

    #[test]
    fn register_requires_non_empty_fields() {
        let mut s = ActionState::default();
        for e in Action::decide(&s, register()).unwrap() {
            Action::apply(&mut s, &e);
        }
        assert!(s.inner.is_some());

        let bad = ActionCommand::Register {
            id: AggregateId::new(),
            project_id: AggregateId::new(),
            name: "".into(),
            command: "x".into(),
            keybinding: None,
            auto_run_on_worktree_create: false,
            icon: "Terminal".into(),
            kind: "terminal_command".into(),
            show_in_sidebar: true,
            now: now(),
        };
        assert_eq!(
            Action::decide(&ActionState::default(), bad),
            Err(ActionError::InvalidFields)
        );
    }

    #[test]
    fn double_register_rejected() {
        let mut s = ActionState::default();
        for e in Action::decide(&s, register()).unwrap() {
            Action::apply(&mut s, &e);
        }
        assert_eq!(
            Action::decide(&s, register()),
            Err(ActionError::AlreadyExists)
        );
    }

    #[test]
    fn update_then_remove() {
        let mut s = ActionState::default();
        for e in Action::decide(&s, register()).unwrap() {
            Action::apply(&mut s, &e);
        }
        let update = ActionCommand::Update {
            name: "Test".into(),
            command: "bun test".into(),
            keybinding: None,
            auto_run_on_worktree_create: false,
            icon: "TestTube".into(),
            kind: "terminal_command".into(),
            show_in_sidebar: true,
            now: now(),
        };
        for e in Action::decide(&s, update).unwrap() {
            Action::apply(&mut s, &e);
        }
        assert_eq!(s.inner.as_ref().unwrap().name, "Test");

        for e in Action::decide(&s, ActionCommand::Remove { now: now() }).unwrap() {
            Action::apply(&mut s, &e);
        }
        assert!(s.inner.unwrap().removed_at.is_some());
    }
}
