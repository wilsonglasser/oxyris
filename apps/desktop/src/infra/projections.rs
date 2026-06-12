//! Read-model projections — denormalized views of the event log kept in
//! SQLite for fast queries from the UI. Projections are **not** the source of
//! truth: they can be dropped and rebuilt from the event store at any time,
//! so a schema change is just a `drop` + `rebuild`.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use oxyris_core::{AggregateId, Environment, StoredEvent};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::action::{Action, ActionEvent};
use crate::domain::project::{Project, ProjectEvent};
use crate::domain::session::{Session, SessionEvent, SessionState};
use crate::domain::worktree::{Worktree, WorktreeEvent};
use oxyris_core::{Aggregate, replay};

use super::event_store::{EventStore, EventStoreError};

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("event store error: {0}")]
    EventStore(#[from] EventStoreError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectRow {
    pub id: AggregateId,
    pub name: String,
    pub environment: Environment,
    pub root_path: String,
    #[serde(default)]
    pub logo_path: Option<String>,
    /// Free-text workspace/vault label (sidebar grouping); `None` = ungrouped.
    #[serde(default)]
    pub workspace: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub session_count: u32,
    /// Sidebar sort key (drag-to-reorder). Lower = higher in the list. Always
    /// populated on read — projects predating the field are backfilled from
    /// their `created_at` epoch.
    pub sort_order: f64,
}

pub struct Projections {
    conn: Mutex<Connection>,
}

impl Projections {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProjectionError> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, ProjectionError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init(conn: &Connection) -> rusqlite::Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projections_projects (
                id                 TEXT    PRIMARY KEY,
                name               TEXT    NOT NULL,
                environment_kind   TEXT    NOT NULL,
                environment_distro TEXT,
                root_path          TEXT    NOT NULL,
                session_count      INTEGER NOT NULL DEFAULT 0,
                created_at         TEXT    NOT NULL,
                last_activity_at   TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS projections_projects_by_activity
                ON projections_projects (last_activity_at DESC);

            CREATE TABLE IF NOT EXISTS projections_worktrees (
                id         TEXT    PRIMARY KEY,
                project_id TEXT    NOT NULL,
                name       TEXT    NOT NULL,
                branch     TEXT    NOT NULL,
                path       TEXT    NOT NULL,
                is_primary INTEGER NOT NULL DEFAULT 0,
                created_at TEXT    NOT NULL,
                removed_at TEXT
            );
            CREATE INDEX IF NOT EXISTS projections_worktrees_by_project
                ON projections_worktrees (project_id);

            CREATE TABLE IF NOT EXISTS projections_sessions (
                id               TEXT    PRIMARY KEY,
                project_id       TEXT    NOT NULL,
                worktree_id      TEXT,
                provider_id      TEXT    NOT NULL,
                model            TEXT    NOT NULL,
                status           TEXT    NOT NULL,
                turn_count       INTEGER NOT NULL DEFAULT 0,
                state_snapshot   TEXT    NOT NULL,
                created_at       TEXT    NOT NULL,
                last_activity_at TEXT    NOT NULL,
                title            TEXT,
                pinned_at        TEXT
            );
            CREATE INDEX IF NOT EXISTS projections_sessions_by_project
                ON projections_sessions (project_id, last_activity_at DESC);

            CREATE TABLE IF NOT EXISTS projections_actions (
                id                          TEXT    PRIMARY KEY,
                project_id                  TEXT    NOT NULL,
                name                        TEXT    NOT NULL,
                command                     TEXT    NOT NULL,
                keybinding                  TEXT,
                auto_run_on_worktree_create INTEGER NOT NULL DEFAULT 0,
                created_at                  TEXT    NOT NULL,
                updated_at                  TEXT    NOT NULL,
                removed_at                  TEXT
            );
            CREATE INDEX IF NOT EXISTS projections_actions_by_project
                ON projections_actions (project_id);
            "#,
        )?;
        // Idempotent column migrations — SQLite's CREATE TABLE IF NOT EXISTS
        // doesn't touch an existing table, so additive fields have to come
        // through ALTER TABLE. We ignore "duplicate column" errors so the
        // migration is a no-op on the second run.
        for alter in [
            "ALTER TABLE projections_sessions ADD COLUMN title TEXT",
            "ALTER TABLE projections_sessions ADD COLUMN pinned_at TEXT",
            "ALTER TABLE projections_actions ADD COLUMN icon TEXT",
            "ALTER TABLE projections_actions ADD COLUMN action_kind TEXT",
            "ALTER TABLE projections_actions ADD COLUMN show_in_sidebar INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE projections_projects ADD COLUMN logo_path TEXT",
            "ALTER TABLE projections_projects ADD COLUMN workspace TEXT",
            "ALTER TABLE projections_projects ADD COLUMN sort_order REAL",
        ] {
            if let Err(e) = conn.execute(alter, []) {
                let msg = e.to_string().to_ascii_lowercase();
                if !msg.contains("duplicate column name") {
                    return Err(e);
                }
            }
        }
        // Backfill: projects predating the sort_order field get their
        // `created_at` epoch as the initial key, so the existing list order
        // (newest first by activity) is preserved-ish — old rows still sort
        // by creation time, new drag-to-reorder writes override per-project.
        conn.execute(
            "UPDATE projections_projects
                SET sort_order = strftime('%s', created_at) * 1.0
              WHERE sort_order IS NULL",
            [],
        )?;
        Ok(())
    }

    /// Apply one stored event to the matching projection.
    pub fn apply(&self, stored: &StoredEvent) -> Result<(), ProjectionError> {
        match stored.aggregate.as_str() {
            Project::KIND => {
                let event: ProjectEvent = serde_json::from_value(stored.payload.clone())?;
                self.apply_project_event(stored, &event)?;
            }
            Worktree::KIND => {
                let event: WorktreeEvent = serde_json::from_value(stored.payload.clone())?;
                self.apply_worktree_event(stored, &event)?;
            }
            Session::KIND => {
                let event: SessionEvent = serde_json::from_value(stored.payload.clone())?;
                self.apply_session_event(stored, &event)?;
            }
            Action::KIND => {
                let event: ActionEvent = serde_json::from_value(stored.payload.clone())?;
                self.apply_action_event(stored, &event)?;
            }
            _ => {
                // Unknown aggregates are ignored so adding a new one doesn't
                // require updating projections atomically.
            }
        }
        Ok(())
    }

    fn apply_worktree_event(
        &self,
        stored: &StoredEvent,
        event: &WorktreeEvent,
    ) -> Result<(), ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
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
                conn.execute(
                    "INSERT OR REPLACE INTO projections_worktrees
                        (id, project_id, name, branch, path, is_primary,
                         created_at, removed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
                    params![
                        id.to_string(),
                        project_id.to_string(),
                        name,
                        branch,
                        path,
                        if *is_primary { 1 } else { 0 },
                        created_at.to_rfc3339(),
                    ],
                )?;
            }
            WorktreeEvent::WorktreeRemoved { removed_at } => {
                conn.execute(
                    "UPDATE projections_worktrees
                        SET removed_at = ?1
                      WHERE id = ?2",
                    params![removed_at.to_rfc3339(), stored.aggregate_id.to_string(),],
                )?;
            }
        }
        Ok(())
    }

    /// Look up a single worktree by id, regardless of `removed_at`. Returns
    /// `None` when the row was never projected (e.g. the synthetic primary
    /// sentinel) — callers must fall back to project root in that case.
    pub fn get_worktree(
        &self,
        id: oxyris_core::AggregateId,
    ) -> Result<Option<WorktreeRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, branch, path, is_primary, created_at, removed_at
               FROM projections_worktrees
              WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.to_string()])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row_to_worktree(row)?));
        }
        Ok(None)
    }

    pub fn list_worktrees(
        &self,
        project_id: oxyris_core::AggregateId,
        include_removed: bool,
    ) -> Result<Vec<WorktreeRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let sql = if include_removed {
            "SELECT id, project_id, name, branch, path, is_primary, created_at, removed_at
               FROM projections_worktrees
              WHERE project_id = ?1
              ORDER BY is_primary DESC, created_at ASC"
        } else {
            "SELECT id, project_id, name, branch, path, is_primary, created_at, removed_at
               FROM projections_worktrees
              WHERE project_id = ?1 AND removed_at IS NULL
              ORDER BY is_primary DESC, created_at ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![project_id.to_string()], row_to_worktree)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn apply_project_event(
        &self,
        stored: &StoredEvent,
        event: &ProjectEvent,
    ) -> Result<(), ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        match event {
            ProjectEvent::ProjectCreated {
                id,
                name,
                environment,
                root_path,
                workspace,
                created_at,
            } => {
                let (kind, distro) = environment_columns(environment);
                // Initial sort_order = created_at epoch, so a fresh project
                // lands at the chronological end (largest key) until the user
                // drags it.
                let initial_sort = created_at.timestamp() as f64;
                conn.execute(
                    "INSERT OR REPLACE INTO projections_projects
                        (id, name, environment_kind, environment_distro, root_path,
                         session_count, created_at, last_activity_at, workspace,
                         sort_order)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9)",
                    params![
                        id.to_string(),
                        name,
                        kind,
                        distro,
                        root_path,
                        created_at.to_rfc3339(),
                        stored.timestamp.to_rfc3339(),
                        workspace,
                        initial_sort,
                    ],
                )?;
            }
            ProjectEvent::ProjectRenamed { new_name } => {
                conn.execute(
                    "UPDATE projections_projects
                        SET name = ?1, last_activity_at = ?2
                      WHERE id = ?3",
                    params![
                        new_name,
                        stored.timestamp.to_rfc3339(),
                        stored.aggregate_id.to_string(),
                    ],
                )?;
            }
            ProjectEvent::ProjectLogoSet { logo_path } => {
                conn.execute(
                    "UPDATE projections_projects
                        SET logo_path = ?1, last_activity_at = ?2
                      WHERE id = ?3",
                    params![
                        logo_path,
                        stored.timestamp.to_rfc3339(),
                        stored.aggregate_id.to_string(),
                    ],
                )?;
            }
            ProjectEvent::ProjectWorkspaceSet { workspace } => {
                conn.execute(
                    "UPDATE projections_projects
                        SET workspace = ?1, last_activity_at = ?2
                      WHERE id = ?3",
                    params![
                        workspace,
                        stored.timestamp.to_rfc3339(),
                        stored.aggregate_id.to_string(),
                    ],
                )?;
            }
            ProjectEvent::ProjectSortOrderSet { sort_order } => {
                // sort_order changes are user-driven UI state; bump
                // last_activity_at so the project doesn't get demoted to
                // "stale" in views that mix activity with the explicit order.
                conn.execute(
                    "UPDATE projections_projects
                        SET sort_order = ?1, last_activity_at = ?2
                      WHERE id = ?3",
                    params![
                        sort_order,
                        stored.timestamp.to_rfc3339(),
                        stored.aggregate_id.to_string(),
                    ],
                )?;
            }
            ProjectEvent::ProjectDeleted => {
                conn.execute(
                    "DELETE FROM projections_projects WHERE id = ?1",
                    params![stored.aggregate_id.to_string()],
                )?;
            }
        }
        Ok(())
    }

    /// Single-row project lookup by id (indexed PK). Use this on hot paths
    /// (every fs/git command resolves a worktree) instead of `list_projects()`
    /// + `.find()`, which scans and parses every project row each call.
    pub fn get_project(
        &self,
        id: oxyris_core::AggregateId,
    ) -> Result<Option<ProjectRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, environment_kind, environment_distro, root_path,
                    session_count, created_at, last_activity_at, logo_path, workspace,
                    sort_order
               FROM projections_projects
              WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id.to_string()])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(row_to_project(row)?));
        }
        Ok(None)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, environment_kind, environment_distro, root_path,
                    session_count, created_at, last_activity_at, logo_path, workspace,
                    sort_order
               FROM projections_projects
              ORDER BY sort_order ASC, last_activity_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_project)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Drop every projected row and rebuild from the event store. Safe to
    /// call at any time; the event log remains the source of truth.
    /// Only invoked manually (e.g. a future "Rebuild read model" settings
    /// action) — boot no longer calls this because it scales O(total_events)
    /// and the projections already persist across restarts.
    #[allow(dead_code)]
    pub fn rebuild_from(&self, store: &EventStore) -> Result<(), ProjectionError> {
        {
            let conn = self.conn.lock().expect("projections mutex poisoned");
            conn.execute("DELETE FROM projections_projects", [])?;
            conn.execute("DELETE FROM projections_worktrees", [])?;
            conn.execute("DELETE FROM projections_sessions", [])?;
            conn.execute("DELETE FROM projections_actions", [])?;
        }
        for stored in store.load_all(Project::KIND)? {
            self.apply(&stored)?;
        }
        for stored in store.load_all(Worktree::KIND)? {
            self.apply(&stored)?;
        }
        // Sessions need the full event history for the snapshot — reapply
        // every event through the aggregate so each session's `state_snapshot`
        // reflects its terminal state.
        for stored in store.load_all(Session::KIND)? {
            self.apply(&stored)?;
        }
        for stored in store.load_all(Action::KIND)? {
            self.apply(&stored)?;
        }
        Ok(())
    }

    fn apply_session_event(
        &self,
        stored: &StoredEvent,
        event: &SessionEvent,
    ) -> Result<(), ProjectionError> {
        // Sessions keep a full snapshot of their aggregate state under
        // `state_snapshot` (JSON) so the UI can read a turn-rich thread in
        // one query. On every event we re-fold via replay to avoid drift.
        // Delete is a shortcut: the projection drops the row immediately;
        // the event log still holds the full history for audit/replay.
        if matches!(event, SessionEvent::SessionDeleted { .. }) {
            let conn = self.conn.lock().expect("projections mutex poisoned");
            conn.execute(
                "DELETE FROM projections_sessions WHERE id = ?1",
                params![stored.aggregate_id.to_string()],
            )?;
            return Ok(());
        }
        let conn = self.conn.lock().expect("projections mutex poisoned");

        // Load the previous snapshot (if any) and fold this event into it.
        let prev_snapshot: Option<String> = conn
            .query_row(
                "SELECT state_snapshot FROM projections_sessions WHERE id = ?1",
                params![stored.aggregate_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        let mut state = prev_snapshot
            .and_then(|snap| serde_json::from_str::<SessionState>(&snap).ok())
            .unwrap_or_default();
        Session::apply(&mut state, event);

        write_session_row(&conn, &state, stored.timestamp)
    }

    /// Upsert a session's read-model row straight from an already-folded
    /// [`SessionState`]. The supervisor's per-session event pump keeps the
    /// aggregate folded in memory, so on the streaming hot path it hands the
    /// post-event state here — skipping the per-event snapshot read + full
    /// deserialize + re-fold that [`Projections::apply`] does. The written row
    /// is identical to what folding via `apply` would produce.
    pub fn apply_session_state(
        &self,
        state: &SessionState,
        last_activity: DateTime<Utc>,
    ) -> Result<(), ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        write_session_row(&conn, state, last_activity)
    }

    pub fn list_sessions(
        &self,
        project_id: AggregateId,
    ) -> Result<Vec<SessionSummaryRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, project_id, worktree_id, provider_id, model, status,
                    turn_count, created_at, last_activity_at, title, pinned_at
               FROM projections_sessions
              WHERE project_id = ?1
              ORDER BY pinned_at IS NULL, pinned_at DESC, last_activity_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id.to_string()], row_to_session_summary)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Enumerate sessions currently marked `running` — cheap query used by
    /// boot-time reconciliation so we don't have to replay every session's
    /// event history just to find the few that need a fake `SessionStopped`.
    pub fn list_running_sessions(&self) -> Result<Vec<SessionSummaryRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, project_id, worktree_id, provider_id, model, status,
                    turn_count, created_at, last_activity_at, title, pinned_at
               FROM projections_sessions
              WHERE status = 'running'
              ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_session_summary)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_session(
        &self,
        session_id: AggregateId,
    ) -> Result<Option<SessionSnapshot>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let snap: Option<(String, u32, String, String)> = conn
            .query_row(
                "SELECT state_snapshot, turn_count, created_at, last_activity_at
                   FROM projections_sessions WHERE id = ?1",
                params![session_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((snapshot_json, _, _, _)) = snap else {
            return Ok(None);
        };
        let state: SessionState = serde_json::from_str(&snapshot_json)?;
        Ok(state.inner.map(|data| {
            let _ = replay::<Session>; // compile-time link; keeps replay wired.
            SessionSnapshot { data }
        }))
    }

    fn apply_action_event(
        &self,
        stored: &StoredEvent,
        event: &ActionEvent,
    ) -> Result<(), ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
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
                conn.execute(
                    "INSERT OR REPLACE INTO projections_actions
                        (id, project_id, name, command, keybinding,
                         auto_run_on_worktree_create, icon, action_kind,
                         show_in_sidebar, created_at, updated_at, removed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)",
                    params![
                        id.to_string(),
                        project_id.to_string(),
                        name,
                        command,
                        keybinding,
                        if *auto_run_on_worktree_create { 1 } else { 0 },
                        icon,
                        kind,
                        if *show_in_sidebar { 1 } else { 0 },
                        created_at.to_rfc3339(),
                    ],
                )?;
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
                conn.execute(
                    "UPDATE projections_actions
                        SET name = ?1,
                            command = ?2,
                            keybinding = ?3,
                            auto_run_on_worktree_create = ?4,
                            icon = ?5,
                            action_kind = ?6,
                            show_in_sidebar = ?7,
                            updated_at = ?8
                      WHERE id = ?9",
                    params![
                        name,
                        command,
                        keybinding,
                        if *auto_run_on_worktree_create { 1 } else { 0 },
                        icon,
                        kind,
                        if *show_in_sidebar { 1 } else { 0 },
                        updated_at.to_rfc3339(),
                        stored.aggregate_id.to_string(),
                    ],
                )?;
            }
            ActionEvent::ActionRemoved { removed_at } => {
                conn.execute(
                    "UPDATE projections_actions
                        SET removed_at = ?1
                      WHERE id = ?2",
                    params![removed_at.to_rfc3339(), stored.aggregate_id.to_string()],
                )?;
            }
        }
        Ok(())
    }

    pub fn list_actions(&self, project_id: AggregateId) -> Result<Vec<ActionRow>, ProjectionError> {
        let conn = self.conn.lock().expect("projections mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, command, keybinding,
                    auto_run_on_worktree_create,
                    COALESCE(icon, 'Terminal') AS icon,
                    COALESCE(action_kind, 'terminal_command') AS action_kind,
                    COALESCE(show_in_sidebar, 1) AS show_in_sidebar,
                    created_at, updated_at
               FROM projections_actions
              WHERE project_id = ?1 AND removed_at IS NULL
              ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id.to_string()], row_to_action)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionRow {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub name: String,
    pub command: String,
    pub keybinding: Option<String>,
    pub auto_run_on_worktree_create: bool,
    pub icon: String,
    pub kind: String,
    pub show_in_sidebar: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn row_to_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActionRow> {
    let parse_id = |idx: usize| -> rusqlite::Result<AggregateId> {
        let s: String = row.get(idx)?;
        uuid::Uuid::parse_str(&s).map(AggregateId).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let parse_ts = |idx: usize, s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
    };
    let auto_run_flag: i64 = row.get(5)?;
    let icon: String = row.get(6)?;
    let kind: String = row.get(7)?;
    let show_flag: i64 = row.get(8)?;
    let created_text: String = row.get(9)?;
    let updated_text: String = row.get(10)?;
    Ok(ActionRow {
        id: parse_id(0)?,
        project_id: parse_id(1)?,
        name: row.get(2)?,
        command: row.get(3)?,
        keybinding: row.get(4)?,
        auto_run_on_worktree_create: auto_run_flag != 0,
        icon,
        kind,
        show_in_sidebar: show_flag != 0,
        created_at: parse_ts(9, &created_text)?,
        updated_at: parse_ts(10, &updated_text)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeRow {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
    pub removed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryRow {
    pub id: AggregateId,
    pub project_id: AggregateId,
    pub worktree_id: Option<AggregateId>,
    pub provider_id: String,
    pub model: String,
    pub status: String,
    pub turn_count: u32,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub title: Option<String>,
    pub pinned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    #[serde(flatten)]
    pub data: crate::domain::session::SessionData,
}

/// Serialize a folded `SessionState` into the `projections_sessions` row.
/// Shared by the generic per-event `apply` path and the supervisor's
/// folded-state fast path so both write byte-identical rows.
fn write_session_row(
    conn: &Connection,
    state: &SessionState,
    last_activity: DateTime<Utc>,
) -> Result<(), ProjectionError> {
    let snapshot_json = serde_json::to_string(state)?;
    let data = state
        .inner
        .as_ref()
        .expect("session state populated after apply");
    conn.execute(
        "INSERT OR REPLACE INTO projections_sessions
            (id, project_id, worktree_id, provider_id, model, status,
             turn_count, state_snapshot, created_at, last_activity_at,
             title, pinned_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            data.id.to_string(),
            data.project_id.to_string(),
            data.worktree_id.map(|w| w.to_string()),
            data.provider_id,
            data.model,
            serde_json::to_string(&data.status)
                .unwrap_or_default()
                .trim_matches('"')
                .to_owned(),
            data.turns.len() as u32,
            snapshot_json,
            data.created_at.to_rfc3339(),
            last_activity.to_rfc3339(),
            data.title.clone(),
            data.pinned_at.map(|t| t.to_rfc3339()),
        ],
    )?;
    Ok(())
}

fn row_to_session_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummaryRow> {
    let parse_id = |idx: usize| -> rusqlite::Result<AggregateId> {
        let s: String = row.get(idx)?;
        uuid::Uuid::parse_str(&s).map(AggregateId).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let parse_ts = |idx: usize, s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
    };
    let worktree_str: Option<String> = row.get(2)?;
    let worktree_id = match worktree_str {
        Some(s) => Some(uuid::Uuid::parse_str(&s).map(AggregateId).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };
    let created_text: String = row.get(7)?;
    let activity_text: String = row.get(8)?;
    let title: Option<String> = row.get(9)?;
    let pinned_text: Option<String> = row.get(10)?;
    let pinned_at = match pinned_text {
        Some(s) => Some(parse_ts(10, &s)?),
        None => None,
    };
    Ok(SessionSummaryRow {
        id: parse_id(0)?,
        project_id: parse_id(1)?,
        worktree_id,
        provider_id: row.get(3)?,
        model: row.get(4)?,
        status: row.get(5)?,
        turn_count: row.get(6)?,
        created_at: parse_ts(7, &created_text)?,
        last_activity_at: parse_ts(8, &activity_text)?,
        title,
        pinned_at,
    })
}

fn row_to_worktree(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeRow> {
    let parse_id = |idx: usize| -> rusqlite::Result<AggregateId> {
        let s: String = row.get(idx)?;
        uuid::Uuid::parse_str(&s).map(AggregateId).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let parse_ts = |idx: usize, s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
    };
    let created_text: String = row.get(6)?;
    let removed_text: Option<String> = row.get(7)?;
    let removed_at = match removed_text {
        Some(s) => Some(parse_ts(7, &s)?),
        None => None,
    };
    let is_primary: i64 = row.get(5)?;
    Ok(WorktreeRow {
        id: parse_id(0)?,
        project_id: parse_id(1)?,
        name: row.get(2)?,
        branch: row.get(3)?,
        path: row.get(4)?,
        is_primary: is_primary != 0,
        created_at: parse_ts(6, &created_text)?,
        removed_at,
    })
}

fn environment_columns(env: &Environment) -> (&'static str, Option<&str>) {
    match env {
        Environment::Local => ("windows", None),
        Environment::Wsl { distro } => ("wsl", Some(distro.as_str())),
    }
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    let id_str: String = row.get(0)?;
    let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let env_kind: String = row.get(2)?;
    let env_distro: Option<String> = row.get(3)?;
    let environment = match env_kind.as_str() {
        "windows" => Environment::Local,
        "wsl" => Environment::Wsl {
            distro: env_distro.unwrap_or_default(),
        },
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("unknown environment kind {other:?}").into(),
            ));
        }
    };
    let created_text: String = row.get(6)?;
    let activity_text: String = row.get(7)?;
    let parse_ts = |idx, s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
    };

    let logo_path: Option<String> = row.get(8).unwrap_or(None);
    let workspace: Option<String> = row.get(9).unwrap_or(None);
    // Fallback to created_at epoch when the column is NULL (pre-migration
    // rows the backfill missed — defensive, the migration should cover this).
    let sort_order: f64 = row
        .get::<_, Option<f64>>(10)
        .unwrap_or(None)
        .unwrap_or_else(|| {
            DateTime::parse_from_rfc3339(&created_text)
                .map(|dt| dt.timestamp() as f64)
                .unwrap_or(0.0)
        });
    Ok(ProjectRow {
        id: AggregateId(id),
        name: row.get(1)?,
        environment,
        root_path: row.get(4)?,
        logo_path,
        workspace,
        session_count: row.get(5)?,
        created_at: parse_ts(6, &created_text)?,
        last_activity_at: parse_ts(7, &activity_text)?,
        sort_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::project::ProjectEvent;
    use oxyris_core::AggregateId;

    fn fake_stored(agg_id: AggregateId, version: u32, event: ProjectEvent) -> StoredEvent {
        StoredEvent {
            seq: Some(version as i64),
            aggregate: Project::KIND.to_owned(),
            aggregate_id: agg_id,
            version,
            kind: {
                use oxyris_core::DomainEvent;
                event.kind().to_owned()
            },
            payload: serde_json::to_value(&event).unwrap(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn created_event_inserts_row() {
        let p = Projections::open_in_memory().unwrap();
        let id = AggregateId::new();
        p.apply(&fake_stored(
            id,
            1,
            ProjectEvent::ProjectCreated {
                id,
                name: "Oxyris".into(),
                environment: Environment::Local,
                root_path: r"C:\dev\oxyris".into(),
                workspace: None,
                created_at: Utc::now(),
            },
        ))
        .unwrap();
        let rows = p.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Oxyris");
        assert_eq!(rows[0].environment, Environment::Local);
    }

    #[test]
    fn renamed_event_updates_name() {
        let p = Projections::open_in_memory().unwrap();
        let id = AggregateId::new();
        p.apply(&fake_stored(
            id,
            1,
            ProjectEvent::ProjectCreated {
                id,
                name: "Old".into(),
                environment: Environment::Wsl {
                    distro: "Ubuntu".into(),
                },
                root_path: "/home/x/p".into(),
                workspace: None,
                created_at: Utc::now(),
            },
        ))
        .unwrap();
        p.apply(&fake_stored(
            id,
            2,
            ProjectEvent::ProjectRenamed {
                new_name: "New".into(),
            },
        ))
        .unwrap();
        let rows = p.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "New");
        assert_eq!(
            rows[0].environment,
            Environment::Wsl {
                distro: "Ubuntu".into()
            }
        );
    }

    #[test]
    fn deleted_event_removes_row() {
        let p = Projections::open_in_memory().unwrap();
        let id = AggregateId::new();
        p.apply(&fake_stored(
            id,
            1,
            ProjectEvent::ProjectCreated {
                id,
                name: "a".into(),
                environment: Environment::Local,
                root_path: "r".into(),
                workspace: None,
                created_at: Utc::now(),
            },
        ))
        .unwrap();
        p.apply(&fake_stored(id, 2, ProjectEvent::ProjectDeleted))
            .unwrap();
        assert!(p.list_projects().unwrap().is_empty());
    }

    #[test]
    fn rebuild_from_event_store_reproduces_current_state() {
        use crate::domain::project::{Project, ProjectCommand};

        let store = EventStore::open_in_memory().unwrap();
        let projections = Projections::open_in_memory().unwrap();
        let id = AggregateId::new();

        let now = Utc::now();
        let created_events = Project::decide(
            &Default::default(),
            ProjectCommand::Create {
                id,
                name: "Oxyris".into(),
                environment: Environment::Local,
                root_path: "C:\\oxyris".into(),
                workspace: None,
                now,
            },
        )
        .unwrap();
        store.append(Project::KIND, id, 0, &created_events).unwrap();
        store
            .append(
                Project::KIND,
                id,
                1,
                &[ProjectEvent::ProjectRenamed {
                    new_name: "Oxyris Code".into(),
                }],
            )
            .unwrap();

        projections.rebuild_from(&store).unwrap();

        let rows = projections.list_projects().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Oxyris Code");
        assert_eq!(rows[0].id, id);
    }

    fn session_stored(id: AggregateId, version: u32, event: &SessionEvent) -> StoredEvent {
        use oxyris_core::DomainEvent;
        StoredEvent {
            seq: Some(version as i64),
            aggregate: Session::KIND.to_owned(),
            aggregate_id: id,
            version,
            kind: event.kind().to_owned(),
            payload: serde_json::to_value(event).unwrap(),
            timestamp: Utc::now(),
        }
    }

    /// The supervisor's fast path (`apply_session_state`, fed the in-memory
    /// folded state) must produce the same read-model row as the generic
    /// per-event `apply`. Both share `write_session_row`; this guards the two
    /// ways of reaching it from drifting.
    #[test]
    fn folded_state_path_matches_generic_apply() {
        use crate::domain::session::SessionCommand;

        let id = AggregateId::new();
        let project_id = AggregateId::new();
        let now = Utc::now();

        // Build a small event history: start → one turn.
        let mut state = SessionState::default();
        let start = Session::decide(
            &state,
            SessionCommand::Start {
                id,
                project_id,
                worktree_id: None,
                provider_id: "claude".into(),
                model: "opus".into(),
                thinking: Default::default(),
                runtime: Default::default(),
                env_mode: Default::default(),
                kind: Default::default(),
                now,
            },
        )
        .unwrap();
        for e in &start {
            Session::apply(&mut state, e);
        }
        let turn = Session::decide(
            &state,
            SessionCommand::StartTurn {
                turn_id: "turn-1".into(),
                user_text: "hello there friend".into(),
                now,
            },
        )
        .unwrap();
        for e in &turn {
            Session::apply(&mut state, e);
        }
        let all: Vec<SessionEvent> = start.into_iter().chain(turn).collect();

        // Path A: generic per-event apply.
        let pa = Projections::open_in_memory().unwrap();
        for (i, e) in all.iter().enumerate() {
            pa.apply(&session_stored(id, i as u32 + 1, e)).unwrap();
        }
        let snap_a = pa.get_session(id).unwrap().unwrap();

        // Path B: one folded-state upsert.
        let pb = Projections::open_in_memory().unwrap();
        pb.apply_session_state(&state, now).unwrap();
        let snap_b = pb.get_session(id).unwrap().unwrap();

        assert_eq!(snap_a.data, snap_b.data);
        assert_eq!(snap_b.data.turns.len(), 1);
    }
}
