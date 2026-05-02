//! Tree-sitter based per-worktree symbol index with SQLite persistence.
//!
//! Layout:
//! - [`Lang`] detects the language from a file extension and exposes the
//!   tree-sitter parser + capture query.
//! - [`Extractor`] is a stateful per-language parser+query pair. Reuse one
//!   per language across many files.
//! - [`Index`] is the public entry point: opens a SQLite database under
//!   `<worktree>/.oxyris/index.db`, lets callers upsert files and query
//!   symbols.
//!
//! The index is intentionally a *cache*: drop the file and rebuild from a
//! filesystem walk and you get the same answers.

mod extractor;
pub mod language;
mod storage;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use extractor::Extractor;
pub use language::Lang;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("tree-sitter parser: {0}")]
    Parser(String),
    #[error("tree-sitter query: {0}")]
    Query(String),
    #[error(
        "schema version mismatch: stored={stored} expected={expected}; delete the index db to rebuild"
    )]
    SchemaVersion { stored: String, expected: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Type,
    Constant,
    Module,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Interface => "interface",
            SymbolKind::Type => "type",
            SymbolKind::Constant => "constant",
            SymbolKind::Module => "module",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "interface" => SymbolKind::Interface,
            "type" => SymbolKind::Type,
            "constant" => SymbolKind::Constant,
            "module" => SymbolKind::Module,
            _ => return None,
        })
    }

    pub(crate) fn from_capture(name: &str) -> Option<Self> {
        Self::from_label(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based line numbers (inclusive).
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolHit {
    pub name: String,
    pub kind: SymbolKind,
    pub file: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySummary {
    pub dir: String,
    pub files: u64,
    pub symbols: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMap {
    pub directories: Vec<DirectorySummary>,
    pub total_files: u64,
    pub total_symbols: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub files: u64,
    pub symbols: u64,
}

/// Public handle. Holds the SQLite connection behind a Mutex (cheap — index
/// ops are short) plus a per-language extractor pool that lazily initializes
/// parsers on first use.
pub struct Index {
    conn: Mutex<Connection>,
    extractors: Mutex<HashMap<Lang, Extractor>>,
}

impl Index {
    /// Open or create the index db at `path`. Creates the parent directory
    /// if needed.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let conn = storage::open(path)?;
        Ok(Self {
            conn: Mutex::new(conn),
            extractors: Mutex::new(HashMap::new()),
        })
    }

    /// Open an in-memory index — intended for tests.
    pub fn open_in_memory() -> Result<Self, IndexError> {
        let conn = storage::open_in_memory()?;
        Ok(Self {
            conn: Mutex::new(conn),
            extractors: Mutex::new(HashMap::new()),
        })
    }

    /// Index a single file. `relative_path` is the path stored in the DB
    /// (use forward slashes, relative to the worktree root). `mtime` is a
    /// caller-supplied integer used to skip re-indexing unchanged files.
    /// Returns the number of symbols extracted.
    pub fn index_file(
        &self,
        relative_path: &str,
        lang: Lang,
        mtime: i64,
        source: &str,
    ) -> Result<usize, IndexError> {
        let symbols = self.parse_symbols(lang, source)?;
        let count = symbols.len();
        let mut conn = self.conn.lock().expect("index conn poisoned");
        storage::upsert_file(&mut conn, relative_path, lang, mtime, &symbols)?;
        Ok(count)
    }

    /// Skip the upsert if the stored mtime already matches. Returns true if
    /// the file was re-indexed.
    pub fn index_file_if_changed(
        &self,
        relative_path: &str,
        lang: Lang,
        mtime: i64,
        source: &str,
    ) -> Result<bool, IndexError> {
        {
            let conn = self.conn.lock().expect("index conn poisoned");
            if let Some(existing) = storage::file_mtime(&conn, relative_path)?
                && existing == mtime
            {
                return Ok(false);
            }
        }
        self.index_file(relative_path, lang, mtime, source)?;
        Ok(true)
    }

    pub fn remove_file(&self, relative_path: &str) -> Result<(), IndexError> {
        let conn = self.conn.lock().expect("index conn poisoned");
        storage::remove_file(&conn, relative_path)
    }

    pub fn find_symbol(
        &self,
        name: &str,
        kind: Option<SymbolKind>,
        limit: u32,
    ) -> Result<Vec<SymbolHit>, IndexError> {
        let conn = self.conn.lock().expect("index conn poisoned");
        storage::find_symbol(&conn, name, kind, limit)
    }

    pub fn list_symbols_in_file(&self, relative_path: &str) -> Result<Vec<Symbol>, IndexError> {
        let conn = self.conn.lock().expect("index conn poisoned");
        storage::list_symbols_in_file(&conn, relative_path)
    }

    pub fn project_map(&self) -> Result<ProjectMap, IndexError> {
        let conn = self.conn.lock().expect("index conn poisoned");
        let directories = storage::directory_summary(&conn)?;
        let total_files = storage::count_files(&conn)?;
        let total_symbols = storage::count_symbols(&conn)?;
        Ok(ProjectMap {
            directories,
            total_files,
            total_symbols,
        })
    }

    pub fn stats(&self) -> Result<IndexStats, IndexError> {
        let conn = self.conn.lock().expect("index conn poisoned");
        Ok(IndexStats {
            files: storage::count_files(&conn)?,
            symbols: storage::count_symbols(&conn)?,
        })
    }

    fn parse_symbols(&self, lang: Lang, source: &str) -> Result<Vec<Symbol>, IndexError> {
        let mut extractors = self.extractors.lock().expect("extractors poisoned");
        let extractor = match extractors.get_mut(&lang) {
            Some(e) => e,
            None => {
                extractors.insert(lang, Extractor::new(lang)?);
                extractors
                    .get_mut(&lang)
                    .expect("just inserted extractor missing")
            }
        };
        let Some(tree) = extractor.parse(source) else {
            return Ok(Vec::new());
        };
        Ok(extractor.extract(&tree, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> Index {
        Index::open_in_memory().expect("open in-memory")
    }

    #[test]
    fn detects_languages_by_extension() {
        assert_eq!(Lang::from_path(Path::new("src/main.rs")), Some(Lang::Rust));
        assert_eq!(Lang::from_path(Path::new("App.tsx")), Some(Lang::Tsx));
        assert_eq!(Lang::from_path(Path::new("util.PY")), Some(Lang::Python));
        assert_eq!(Lang::from_path(Path::new("README.md")), None);
    }

    #[test]
    fn indexes_rust_top_level_symbols() {
        let i = idx();
        let src = r#"
pub fn add(a: i32, b: i32) -> i32 { a + b }

pub struct User { pub name: String }

impl User {
    pub fn new(name: String) -> Self { Self { name } }
    fn greeting(&self) -> String { format!("hi {}", self.name) }
}

pub trait Speak { fn say(&self); }

pub enum Color { Red, Green, Blue }

const MAX_USERS: u32 = 100;
"#;
        let n = i.index_file("src/lib.rs", Lang::Rust, 1, src).unwrap();
        assert!(n >= 7, "expected ≥7 symbols, got {n}");

        let hits = i.find_symbol("User", None, 10).unwrap();
        assert!(hits.iter().any(|h| h.kind == SymbolKind::Struct));

        let new_hits = i.find_symbol("new", Some(SymbolKind::Method), 10).unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].file, "src/lib.rs");

        let consts = i.find_symbol("MAX_USERS", None, 10).unwrap();
        assert_eq!(consts[0].kind, SymbolKind::Constant);
    }

    #[test]
    fn indexes_typescript_arrows_and_classes() {
        let i = idx();
        let src = r#"
export const greet = (name: string) => `hi ${name}`;

export function shout(text: string): string { return text.toUpperCase(); }

export class Greeter {
    private name: string;
    constructor(name: string) { this.name = name; }
    greet() { return `hi ${this.name}`; }
}

export interface Friendly { greet(): string; }
export type Name = string;
"#;
        i.index_file("app.ts", Lang::TypeScript, 1, src).unwrap();

        assert!(
            i.find_symbol("greet", Some(SymbolKind::Function), 5)
                .map(|hits| hits.iter().any(|h| h.kind == SymbolKind::Function))
                .unwrap_or(false)
        );
        assert!(!i.find_symbol("Greeter", None, 5).unwrap().is_empty());
        assert!(!i.find_symbol("Friendly", None, 5).unwrap().is_empty());
    }

    #[test]
    fn indexes_python_classes_and_methods() {
        let i = idx();
        let src = r#"
def top_level():
    pass

class Foo:
    def method_a(self):
        pass

    def method_b(self):
        pass

CONST = 42
"#;
        i.index_file("foo.py", Lang::Python, 1, src).unwrap();

        let hits = i.find_symbol("Foo", None, 5).unwrap();
        assert!(hits.iter().any(|h| h.kind == SymbolKind::Class));
        let methods = i.find_symbol("method_a", None, 5).unwrap();
        assert_eq!(methods[0].kind, SymbolKind::Method);
    }

    #[test]
    fn indexes_php_classes() {
        let i = idx();
        let src = r#"<?php
namespace App\Service;

class UserService {
    public function find(int $id): ?User { return null; }
}

interface Repository {
    public function get(int $id): ?object;
}

function helper(): void {}
"#;
        i.index_file("UserService.php", Lang::Php, 1, src).unwrap();

        assert!(!i.find_symbol("UserService", None, 5).unwrap().is_empty());
        assert!(!i.find_symbol("Repository", None, 5).unwrap().is_empty());
        assert!(!i.find_symbol("helper", None, 5).unwrap().is_empty());
    }

    #[test]
    fn skips_reindex_when_mtime_unchanged() {
        let i = idx();
        let src = "fn a() {}\nfn b() {}\n";
        let did = i
            .index_file_if_changed("x.rs", Lang::Rust, 100, src)
            .unwrap();
        assert!(did);
        let did = i
            .index_file_if_changed("x.rs", Lang::Rust, 100, src)
            .unwrap();
        assert!(!did, "same mtime should skip");
        let did = i
            .index_file_if_changed("x.rs", Lang::Rust, 200, src)
            .unwrap();
        assert!(did, "newer mtime should re-index");
    }

    #[test]
    fn remove_file_drops_its_symbols() {
        let i = idx();
        i.index_file("a.rs", Lang::Rust, 1, "fn a() {}").unwrap();
        i.index_file("b.rs", Lang::Rust, 1, "fn b() {}").unwrap();
        assert_eq!(i.stats().unwrap().files, 2);
        i.remove_file("a.rs").unwrap();
        assert_eq!(i.stats().unwrap().files, 1);
        assert!(i.find_symbol("a", None, 5).unwrap().is_empty());
        assert!(!i.find_symbol("b", None, 5).unwrap().is_empty());
    }

    #[test]
    fn project_map_groups_by_top_dir() {
        let i = idx();
        i.index_file("src/foo.rs", Lang::Rust, 1, "fn foo() {}")
            .unwrap();
        i.index_file("src/bar.rs", Lang::Rust, 1, "fn bar() {}")
            .unwrap();
        i.index_file("tests/it.rs", Lang::Rust, 1, "fn it() {}")
            .unwrap();
        let map = i.project_map().unwrap();
        assert_eq!(map.total_files, 3);
        assert!(map.directories.iter().any(|d| d.dir == "src"));
        assert!(map.directories.iter().any(|d| d.dir == "tests"));
    }

    #[test]
    fn case_insensitive_prefix_fallback() {
        let i = idx();
        i.index_file("a.rs", Lang::Rust, 1, "fn UserService() {}")
            .unwrap();
        let hits = i.find_symbol("user", None, 5).unwrap();
        assert!(
            !hits.is_empty(),
            "should fall back to case-insensitive prefix"
        );
    }
}
