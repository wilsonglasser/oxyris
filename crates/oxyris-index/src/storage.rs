//! SQLite persistence for the symbol index. One database per worktree, kept
//! at `<worktree>/.oxyris/index.db`.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::language::Lang;
use crate::{IndexError, Symbol, SymbolHit, SymbolKind};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    path        TEXT PRIMARY KEY,
    lang        TEXT NOT NULL,
    mtime       INTEGER NOT NULL,
    indexed_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    file        TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    start_line  INTEGER NOT NULL,
    start_col   INTEGER NOT NULL,
    end_line    INTEGER NOT NULL,
    end_col     INTEGER NOT NULL,
    FOREIGN KEY (file) REFERENCES files(path) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS symbols_by_name ON symbols(name);
CREATE INDEX IF NOT EXISTS symbols_by_kind ON symbols(kind);
CREATE INDEX IF NOT EXISTS symbols_by_file ON symbols(file);
"#;

pub fn open(path: &Path) -> Result<Connection, IndexError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, IndexError> {
    let conn = Connection::open_in_memory()?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), IndexError> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA_SQL)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    match stored {
        Some(v) if v == SCHEMA_VERSION.to_string() => {}
        Some(other) => {
            return Err(IndexError::SchemaVersion {
                stored: other,
                expected: SCHEMA_VERSION.to_string(),
            });
        }
        None => {
            conn.execute(
                "INSERT INTO schema_meta(key, value) VALUES ('version', ?1)",
                params![SCHEMA_VERSION.to_string()],
            )?;
        }
    }
    Ok(())
}

/// Replace this file's row + all its symbols atomically.
pub fn upsert_file(
    conn: &mut Connection,
    file_path: &str,
    lang: Lang,
    mtime: i64,
    symbols: &[Symbol],
) -> Result<(), IndexError> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM symbols WHERE file = ?1", params![file_path])?;
    tx.execute(
        "INSERT OR REPLACE INTO files(path, lang, mtime, indexed_at) VALUES(?1, ?2, ?3, ?4)",
        params![
            file_path,
            lang.as_str(),
            mtime,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO symbols(file, name, kind, start_line, start_col, end_line, end_col)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for s in symbols {
            stmt.execute(params![
                file_path,
                s.name,
                s.kind.as_str(),
                s.start_line,
                s.start_col,
                s.end_line,
                s.end_col,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn remove_file(conn: &Connection, file_path: &str) -> Result<(), IndexError> {
    conn.execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
    Ok(())
}

pub fn file_mtime(conn: &Connection, file_path: &str) -> Result<Option<i64>, IndexError> {
    let row = conn
        .query_row(
            "SELECT mtime FROM files WHERE path = ?1",
            params![file_path],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(row)
}

pub fn find_symbol(
    conn: &Connection,
    name: &str,
    kind: Option<SymbolKind>,
    limit: u32,
) -> Result<Vec<SymbolHit>, IndexError> {
    let limit = limit.max(1) as i64;
    let mut hits = Vec::new();
    let exact_query = "SELECT name, kind, file, start_line, start_col, end_line, end_col
                       FROM symbols WHERE name = ?1 AND (?2 IS NULL OR kind = ?2)
                       ORDER BY file, start_line LIMIT ?3";
    let mut stmt = conn.prepare(exact_query)?;
    let kind_str = kind.map(|k| k.as_str().to_owned());
    let rows = stmt.query_map(params![name, kind_str, limit], |row| {
        Ok(SymbolHit {
            name: row.get(0)?,
            kind: SymbolKind::from_label(&row.get::<_, String>(1)?).unwrap_or(SymbolKind::Function),
            file: row.get(2)?,
            start_line: row.get(3)?,
            start_col: row.get(4)?,
            end_line: row.get(5)?,
            end_col: row.get(6)?,
        })
    })?;
    for r in rows {
        hits.push(r?);
    }

    // Fall back to case-insensitive prefix if exact found nothing — Claude
    // often guesses casing. We dedupe by (file, start_line) on top of the
    // exact hits (which is already empty here, but future-proof if we
    // combine).
    if hits.is_empty() {
        let prefix_query = "SELECT name, kind, file, start_line, start_col, end_line, end_col
                            FROM symbols WHERE name LIKE ?1 COLLATE NOCASE
                            AND (?2 IS NULL OR kind = ?2)
                            ORDER BY length(name), file, start_line LIMIT ?3";
        let mut stmt = conn.prepare(prefix_query)?;
        let pattern = format!("{}%", name);
        let rows = stmt.query_map(params![pattern, kind_str, limit], |row| {
            Ok(SymbolHit {
                name: row.get(0)?,
                kind: SymbolKind::from_label(&row.get::<_, String>(1)?)
                    .unwrap_or(SymbolKind::Function),
                file: row.get(2)?,
                start_line: row.get(3)?,
                start_col: row.get(4)?,
                end_line: row.get(5)?,
                end_col: row.get(6)?,
            })
        })?;
        for r in rows {
            hits.push(r?);
        }
    }
    Ok(hits)
}

pub fn list_symbols_in_file(conn: &Connection, file_path: &str) -> Result<Vec<Symbol>, IndexError> {
    let mut stmt = conn.prepare(
        "SELECT name, kind, start_line, start_col, end_line, end_col
         FROM symbols WHERE file = ?1 ORDER BY start_line, start_col",
    )?;
    let rows = stmt.query_map(params![file_path], |row| {
        Ok(Symbol {
            name: row.get(0)?,
            kind: SymbolKind::from_label(&row.get::<_, String>(1)?).unwrap_or(SymbolKind::Function),
            start_line: row.get(2)?,
            start_col: row.get(3)?,
            end_line: row.get(4)?,
            end_col: row.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn count_files(conn: &Connection) -> Result<u64, IndexError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    Ok(count as u64)
}

pub fn count_symbols(conn: &Connection) -> Result<u64, IndexError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
    Ok(count as u64)
}

/// Files grouped by top-level directory, with symbol counts. Useful as a
/// project map fed to the LLM at session start.
pub fn directory_summary(conn: &Connection) -> Result<Vec<crate::DirectorySummary>, IndexError> {
    // Group by the first path segment. Paths are stored relative to the
    // worktree root and use forward slashes.
    let mut stmt = conn.prepare(
        "SELECT
             CASE
               WHEN instr(file, '/') > 0 THEN substr(file, 1, instr(file, '/') - 1)
               ELSE '.'
             END AS dir,
             COUNT(DISTINCT file) AS files,
             COUNT(*) AS symbols
         FROM symbols
         GROUP BY dir
         ORDER BY symbols DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(crate::DirectorySummary {
            dir: row.get(0)?,
            files: row.get::<_, i64>(1)? as u64,
            symbols: row.get::<_, i64>(2)? as u64,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
