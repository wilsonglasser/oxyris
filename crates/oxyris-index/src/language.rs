//! Supported languages: detection from file extension and tree-sitter
//! parser/query loading.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Query};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Python,
    Php,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::JavaScript => "javascript",
            Lang::Jsx => "jsx",
            Lang::Python => "python",
            Lang::Php => "php",
        }
    }

    /// Detect a language from the file extension. Returns `None` for files
    /// we don't have a parser for — caller should skip them.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let lang = match ext.as_str() {
            "rs" => Lang::Rust,
            "ts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" => Lang::JavaScript,
            "jsx" => Lang::Jsx,
            "py" | "pyi" => Lang::Python,
            "php" | "phtml" => Lang::Php,
            _ => return None,
        };
        Some(lang)
    }

    pub fn tree_sitter_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript | Lang::Jsx => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        }
    }

    /// Tree-sitter query that captures top-level symbols. Capture names map
    /// 1:1 to [`crate::SymbolKind`]:
    /// `@function`, `@method`, `@class`, `@struct`, `@enum`, `@trait`,
    /// `@interface`, `@type`, `@constant`, `@module`.
    pub fn query_source(self) -> &'static str {
        match self {
            Lang::Rust => include_str!("queries/rust.scm"),
            Lang::TypeScript | Lang::Tsx => include_str!("queries/typescript.scm"),
            Lang::JavaScript | Lang::Jsx => include_str!("queries/javascript.scm"),
            Lang::Python => include_str!("queries/python.scm"),
            Lang::Php => include_str!("queries/php.scm"),
        }
    }

    pub fn build_query(self) -> Result<Query, tree_sitter::QueryError> {
        Query::new(&self.tree_sitter_language(), self.query_source())
    }
}
