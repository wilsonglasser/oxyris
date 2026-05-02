//! Project aggregate — a repo Oxyris knows about. Holds metadata only; the
//! working tree itself lives in whichever `Environment` the project belongs
//! to (Windows or a WSL distro).

use chrono::{DateTime, Utc};
use oxyris_core::{Aggregate, AggregateId, DomainEvent, Environment};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct Project;

/// Hydrated view of one project. `None` means either "not yet created" or
/// "already deleted" — both are rejection cases for non-create commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectState {
    pub inner: Option<ProjectData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectData {
    pub id: AggregateId,
    pub name: String,
    pub environment: Environment,
    pub root_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum ProjectCommand {
    Create {
        id: AggregateId,
        name: String,
        environment: Environment,
        root_path: String,
        now: DateTime<Utc>,
    },
    Rename {
        new_name: String,
    },
    Delete,
}

// Variants keep their `Project*` prefix because the `kind` tag is a global
// discriminator in the shared event log across every aggregate — dropping it
// would collide with other aggregates' `Created`/`Renamed`/`Deleted`.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum ProjectEvent {
    ProjectCreated {
        id: AggregateId,
        name: String,
        environment: Environment,
        root_path: String,
        created_at: DateTime<Utc>,
    },
    ProjectRenamed {
        new_name: String,
    },
    ProjectDeleted,
}

impl DomainEvent for ProjectEvent {
    fn kind(&self) -> &'static str {
        match self {
            Self::ProjectCreated { .. } => "ProjectCreated",
            Self::ProjectRenamed { .. } => "ProjectRenamed",
            Self::ProjectDeleted => "ProjectDeleted",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("project already exists")]
    AlreadyExists,
    #[error("project does not exist")]
    NotFound,
    #[error("project name must not be empty")]
    EmptyName,
    #[error("project name must be 128 characters or fewer")]
    NameTooLong,
    #[error("project root_path must not be empty")]
    EmptyRootPath,
}

const MAX_NAME_LEN: usize = 128;

fn validate_name(name: &str) -> Result<String, ProjectError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProjectError::EmptyName);
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(ProjectError::NameTooLong);
    }
    Ok(trimmed.to_owned())
}

impl Aggregate for Project {
    const KIND: &'static str = "project";
    type Command = ProjectCommand;
    type Event = ProjectEvent;
    type State = ProjectState;
    type Error = ProjectError;

    fn decide(state: &Self::State, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error> {
        match cmd {
            ProjectCommand::Create {
                id,
                name,
                environment,
                root_path,
                now,
            } => {
                if state.inner.is_some() {
                    return Err(ProjectError::AlreadyExists);
                }
                let name = validate_name(&name)?;
                if root_path.trim().is_empty() {
                    return Err(ProjectError::EmptyRootPath);
                }
                Ok(vec![ProjectEvent::ProjectCreated {
                    id,
                    name,
                    environment,
                    root_path,
                    created_at: now,
                }])
            }
            ProjectCommand::Rename { new_name } => {
                let current = state.inner.as_ref().ok_or(ProjectError::NotFound)?;
                let new_name = validate_name(&new_name)?;
                if new_name == current.name {
                    return Ok(vec![]);
                }
                Ok(vec![ProjectEvent::ProjectRenamed { new_name }])
            }
            ProjectCommand::Delete => {
                if state.inner.is_none() {
                    return Err(ProjectError::NotFound);
                }
                Ok(vec![ProjectEvent::ProjectDeleted])
            }
        }
    }

    fn apply(state: &mut Self::State, event: &Self::Event) {
        match event {
            ProjectEvent::ProjectCreated {
                id,
                name,
                environment,
                root_path,
                created_at,
            } => {
                state.inner = Some(ProjectData {
                    id: *id,
                    name: name.clone(),
                    environment: environment.clone(),
                    root_path: root_path.clone(),
                    created_at: *created_at,
                });
            }
            ProjectEvent::ProjectRenamed { new_name } => {
                if let Some(data) = state.inner.as_mut() {
                    data.name = new_name.clone();
                }
            }
            ProjectEvent::ProjectDeleted => {
                state.inner = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxyris_core::replay;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn sample_create() -> ProjectCommand {
        make_create("Oxyris", r"C:\dev\oxyris")
    }

    fn make_create(name: &str, root_path: &str) -> ProjectCommand {
        ProjectCommand::Create {
            id: AggregateId::new(),
            name: name.into(),
            environment: Environment::Windows,
            root_path: root_path.into(),
            now: now(),
        }
    }

    #[test]
    fn create_from_empty_state_produces_created_event() {
        let state = ProjectState::default();
        let events = Project::decide(&state, sample_create()).expect("create ok");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProjectEvent::ProjectCreated { .. }));
    }

    #[test]
    fn create_twice_rejects() {
        let mut state = ProjectState::default();
        let events = Project::decide(&state, sample_create()).unwrap();
        for e in &events {
            Project::apply(&mut state, e);
        }
        assert_eq!(
            Project::decide(&state, sample_create()),
            Err(ProjectError::AlreadyExists)
        );
    }

    #[test]
    fn empty_name_is_rejected() {
        assert_eq!(
            Project::decide(&ProjectState::default(), make_create("   ", "C:\\x")),
            Err(ProjectError::EmptyName)
        );
    }

    #[test]
    fn name_too_long_is_rejected() {
        let long = "a".repeat(MAX_NAME_LEN + 1);
        assert_eq!(
            Project::decide(&ProjectState::default(), make_create(&long, "C:\\x")),
            Err(ProjectError::NameTooLong)
        );
    }

    #[test]
    fn empty_root_path_is_rejected() {
        assert_eq!(
            Project::decide(&ProjectState::default(), make_create("Oxyris", " ")),
            Err(ProjectError::EmptyRootPath)
        );
    }

    #[test]
    fn rename_requires_existing_project() {
        assert_eq!(
            Project::decide(
                &ProjectState::default(),
                ProjectCommand::Rename {
                    new_name: "x".into()
                }
            ),
            Err(ProjectError::NotFound)
        );
    }

    #[test]
    fn rename_to_same_name_is_noop() {
        let mut state = ProjectState::default();
        for e in Project::decide(&state, sample_create()).unwrap() {
            Project::apply(&mut state, &e);
        }
        let events = Project::decide(
            &state,
            ProjectCommand::Rename {
                new_name: "Oxyris".into(),
            },
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn rename_emits_renamed_event_and_applies() {
        let mut state = ProjectState::default();
        for e in Project::decide(&state, sample_create()).unwrap() {
            Project::apply(&mut state, &e);
        }
        let events = Project::decide(
            &state,
            ProjectCommand::Rename {
                new_name: "Oxyris Code".into(),
            },
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        for e in &events {
            Project::apply(&mut state, e);
        }
        assert_eq!(state.inner.unwrap().name, "Oxyris Code");
    }

    #[test]
    fn delete_requires_existing_project() {
        assert_eq!(
            Project::decide(&ProjectState::default(), ProjectCommand::Delete),
            Err(ProjectError::NotFound)
        );
    }

    #[test]
    fn delete_clears_state() {
        let mut state = ProjectState::default();
        for e in Project::decide(&state, sample_create()).unwrap() {
            Project::apply(&mut state, &e);
        }
        for e in Project::decide(&state, ProjectCommand::Delete).unwrap() {
            Project::apply(&mut state, &e);
        }
        assert!(state.inner.is_none());
    }

    #[test]
    fn delete_then_create_is_allowed() {
        let mut state = ProjectState::default();
        for e in Project::decide(&state, sample_create()).unwrap() {
            Project::apply(&mut state, &e);
        }
        for e in Project::decide(&state, ProjectCommand::Delete).unwrap() {
            Project::apply(&mut state, &e);
        }
        let events = Project::decide(&state, sample_create()).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn replay_matches_step_by_step_apply() {
        let mut step = ProjectState::default();
        let mut all_events = Vec::new();

        for cmd in [
            sample_create(),
            ProjectCommand::Rename {
                new_name: "Oxyris Code".into(),
            },
            ProjectCommand::Delete,
            sample_create(),
        ] {
            let evs = Project::decide(&step, cmd).unwrap();
            for e in &evs {
                Project::apply(&mut step, e);
            }
            all_events.extend(evs);
        }

        let replayed = replay::<Project>(&all_events);
        assert_eq!(step, replayed);
    }

    #[test]
    fn event_kind_strings_match_serde_tag() {
        let created = ProjectEvent::ProjectCreated {
            id: AggregateId::new(),
            name: "x".into(),
            environment: Environment::Windows,
            root_path: "p".into(),
            created_at: now(),
        };
        let json = serde_json::to_value(&created).unwrap();
        assert_eq!(json["kind"], "ProjectCreated");
        assert_eq!(created.kind(), "ProjectCreated");
    }
}
