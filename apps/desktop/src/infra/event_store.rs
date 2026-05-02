//! SQLite-backed event store. Append-only, with optimistic concurrency via a
//! UNIQUE constraint on `(aggregate, aggregate_id, version)`.
//!
//! The store is deliberately small — it knows nothing about specific
//! aggregates or event shapes. Callers serialize their typed events into
//! `serde_json::Value` payloads + a string `kind`, and the store persists
//! them alongside the identifying triple.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use oxyris_core::{AggregateId, DomainEvent, StoredEvent};
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, params};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventStoreError {
    #[error(
        "optimistic concurrency conflict on {aggregate}/{aggregate_id}: expected version {expected}"
    )]
    Concurrency {
        aggregate: &'static str,
        aggregate_id: AggregateId,
        expected: u32,
    },
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("projection: {0}")]
    Projection(String),
}

impl EventStoreError {
    pub fn from_projection(e: crate::infra::projections::ProjectionError) -> Self {
        EventStoreError::Projection(e.to_string())
    }
}

pub struct EventStore {
    conn: Mutex<Connection>,
}

impl EventStore {
    /// Open (or create) an event store at `path`. Pass `":memory:"` for tests.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EventStoreError> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, EventStoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init(conn: &Connection) -> rusqlite::Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                seq          INTEGER PRIMARY KEY AUTOINCREMENT,
                aggregate    TEXT    NOT NULL,
                aggregate_id TEXT    NOT NULL,
                version      INTEGER NOT NULL,
                kind         TEXT    NOT NULL,
                payload      TEXT    NOT NULL,
                timestamp    TEXT    NOT NULL,
                UNIQUE (aggregate, aggregate_id, version)
            );
            CREATE INDEX IF NOT EXISTS events_by_aggregate
                ON events (aggregate, aggregate_id, version);
            CREATE INDEX IF NOT EXISTS events_by_seq ON events (seq);
            "#,
        )?;
        Ok(())
    }

    /// Append events for one aggregate instance atomically. `expected_version`
    /// is the version the caller believes the aggregate currently has — the
    /// first appended event gets `expected_version + 1`. If any other writer
    /// has moved ahead the UNIQUE constraint fires and the whole batch is
    /// rolled back, reported as [`EventStoreError::Concurrency`].
    pub fn append<E: DomainEvent>(
        &self,
        aggregate: &'static str,
        aggregate_id: AggregateId,
        expected_version: u32,
        events: &[E],
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock().expect("event store mutex poisoned");
        let tx = conn.transaction()?;
        let mut out = Vec::with_capacity(events.len());
        let now = Utc::now();

        for (i, event) in events.iter().enumerate() {
            let version = expected_version + (i as u32) + 1;
            let payload = serde_json::to_value(event)?;
            let payload_text = serde_json::to_string(&payload)?;
            let timestamp_text = now.to_rfc3339();

            let res = tx.execute(
                "INSERT INTO events (aggregate, aggregate_id, version, kind, payload, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    aggregate,
                    aggregate_id.to_string(),
                    version,
                    event.kind(),
                    payload_text,
                    timestamp_text,
                ],
            );

            match res {
                Ok(_) => {
                    let seq = tx.last_insert_rowid();
                    out.push(StoredEvent {
                        seq: Some(seq),
                        aggregate: aggregate.to_owned(),
                        aggregate_id,
                        version,
                        kind: event.kind().to_owned(),
                        payload,
                        timestamp: now,
                    });
                }
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    return Err(EventStoreError::Concurrency {
                        aggregate,
                        aggregate_id,
                        expected: expected_version,
                    });
                }
                Err(e) => return Err(EventStoreError::Storage(e)),
            }
        }

        tx.commit()?;
        Ok(out)
    }

    /// Load all events for one aggregate instance, ordered by `version`.
    pub fn load(
        &self,
        aggregate: &'static str,
        aggregate_id: AggregateId,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let conn = self.conn.lock().expect("event store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT seq, aggregate, aggregate_id, version, kind, payload, timestamp
             FROM events
             WHERE aggregate = ?1 AND aggregate_id = ?2
             ORDER BY version ASC",
        )?;
        let rows = stmt.query_map(
            params![aggregate, aggregate_id.to_string()],
            row_to_stored_event,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Load every event for a given aggregate *kind*, across all instances,
    /// ordered by insert sequence. Used to rebuild projections.
    pub fn load_all(&self, aggregate: &'static str) -> Result<Vec<StoredEvent>, EventStoreError> {
        let conn = self.conn.lock().expect("event store mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT seq, aggregate, aggregate_id, version, kind, payload, timestamp
             FROM events
             WHERE aggregate = ?1
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![aggregate], row_to_stored_event)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Current version of one aggregate instance (0 when it has no events).
    #[cfg(test)]
    pub fn current_version(
        &self,
        aggregate: &'static str,
        aggregate_id: AggregateId,
    ) -> Result<u32, EventStoreError> {
        let conn = self.conn.lock().expect("event store mutex poisoned");
        let ver: Option<u32> = conn
            .query_row(
                "SELECT MAX(version) FROM events WHERE aggregate = ?1 AND aggregate_id = ?2",
                params![aggregate, aggregate_id.to_string()],
                |row| row.get::<_, Option<u32>>(0),
            )
            .optional()?
            .flatten();
        Ok(ver.unwrap_or(0))
    }
}

fn row_to_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let id_str: String = row.get(2)?;
    let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let payload_text: String = row.get(5)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let ts_text: String = row.get(6)?;
    let timestamp: DateTime<Utc> = DateTime::parse_from_rfc3339(&ts_text)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?
        .with_timezone(&Utc);

    Ok(StoredEvent {
        seq: Some(row.get(0)?),
        aggregate: row.get(1)?,
        aggregate_id: AggregateId(id),
        version: row.get(3)?,
        kind: row.get(4)?,
        payload,
        timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxyris_core::DomainEvent;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "kind")]
    enum FakeEvent {
        Hello { msg: String },
        Goodbye,
    }

    impl DomainEvent for FakeEvent {
        fn kind(&self) -> &'static str {
            match self {
                FakeEvent::Hello { .. } => "Hello",
                FakeEvent::Goodbye => "Goodbye",
            }
        }
    }

    const AGG: &str = "fake";

    #[test]
    fn append_assigns_versions_and_reads_back_in_order() {
        let store = EventStore::open_in_memory().unwrap();
        let id = AggregateId::new();

        let out = store
            .append(
                AGG,
                id,
                0,
                &[
                    FakeEvent::Hello { msg: "a".into() },
                    FakeEvent::Hello { msg: "b".into() },
                ],
            )
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].version, 1);
        assert_eq!(out[1].version, 2);

        let loaded = store.load(AGG, id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].version, 1);
        assert_eq!(loaded[1].version, 2);
        assert_eq!(loaded[0].kind, "Hello");
    }

    #[test]
    fn optimistic_concurrency_rejects_stale_expected_version() {
        let store = EventStore::open_in_memory().unwrap();
        let id = AggregateId::new();

        store
            .append(AGG, id, 0, &[FakeEvent::Hello { msg: "a".into() }])
            .unwrap();

        // Second writer still thinks version is 0 — that slot is taken.
        let err = store.append(AGG, id, 0, &[FakeEvent::Goodbye]).unwrap_err();
        assert!(matches!(err, EventStoreError::Concurrency { .. }));

        // Correct expected version succeeds.
        store.append(AGG, id, 1, &[FakeEvent::Goodbye]).unwrap();
        assert_eq!(store.load(AGG, id).unwrap().len(), 2);
    }

    #[test]
    fn concurrency_conflict_rolls_back_partial_batch() {
        let store = EventStore::open_in_memory().unwrap();
        let id = AggregateId::new();

        // Pre-claim version 2.
        store
            .append(AGG, id, 0, &[FakeEvent::Goodbye, FakeEvent::Goodbye])
            .unwrap();

        // Try to append versions 1..=3, but version 1 is free and 2 is taken
        // — the partial batch must roll back.
        let id2 = AggregateId::new();
        assert_eq!(store.current_version(AGG, id2).unwrap(), 0);

        // Use the *same* aggregate id now — version 1 and 2 already used.
        let err = store
            .append(
                AGG,
                id,
                0,
                &[
                    FakeEvent::Hello { msg: "x".into() },
                    FakeEvent::Hello { msg: "y".into() },
                    FakeEvent::Hello { msg: "z".into() },
                ],
            )
            .unwrap_err();
        assert!(matches!(err, EventStoreError::Concurrency { .. }));

        // No new rows from the failed batch should be visible.
        let loaded = store.load(AGG, id).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(
            loaded.iter().all(|e| matches!(e.kind.as_str(), "Goodbye")),
            "partial batch leaked into the log: {loaded:?}"
        );
    }

    #[test]
    fn load_all_returns_events_across_instances_in_insert_order() {
        let store = EventStore::open_in_memory().unwrap();
        let a = AggregateId::new();
        let b = AggregateId::new();

        store
            .append(AGG, a, 0, &[FakeEvent::Hello { msg: "a1".into() }])
            .unwrap();
        store
            .append(AGG, b, 0, &[FakeEvent::Hello { msg: "b1".into() }])
            .unwrap();
        store.append(AGG, a, 1, &[FakeEvent::Goodbye]).unwrap();

        let all = store.load_all(AGG).unwrap();
        assert_eq!(all.len(), 3);
        // Sequence order is insertion order; aggregate ids alternate as expected.
        assert_eq!(all[0].aggregate_id, a);
        assert_eq!(all[1].aggregate_id, b);
        assert_eq!(all[2].aggregate_id, a);
    }

    #[test]
    fn current_version_reports_the_last_version() {
        let store = EventStore::open_in_memory().unwrap();
        let id = AggregateId::new();
        assert_eq!(store.current_version(AGG, id).unwrap(), 0);
        store
            .append(
                AGG,
                id,
                0,
                &[FakeEvent::Goodbye, FakeEvent::Goodbye, FakeEvent::Goodbye],
            )
            .unwrap();
        assert_eq!(store.current_version(AGG, id).unwrap(), 3);
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.sqlite");
        let id = AggregateId::new();

        {
            let store = EventStore::open(&path).unwrap();
            store
                .append(
                    AGG,
                    id,
                    0,
                    &[FakeEvent::Hello {
                        msg: "persist".into(),
                    }],
                )
                .unwrap();
        }

        let store = EventStore::open(&path).unwrap();
        let loaded = store.load(AGG, id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].kind, "Hello");
    }
}
