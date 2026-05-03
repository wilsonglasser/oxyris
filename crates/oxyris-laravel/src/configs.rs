//! Parse `config/*.php` files. Each file's name becomes the prefix
//! (`config/app.php` → keys under `app.*`). We capture only top-level
//! array keys — the typical accessor `config('app.name')` only ever needs
//! the file + first-segment name, so deeper nesting isn't worth the
//! parsing cost.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::{Parser, QueryCursor, StreamingIterator};

use crate::detect::LaravelProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// File stem — `app` for `config/app.php`. The accessor key prefix.
    pub name: String,
    pub file: String,
    pub keys: Vec<ConfigKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigKey {
    pub key: String,
    pub line: u32,
}

pub fn parse_all(project: &LaravelProject) -> Vec<ConfigFile> {
    let dir = project.config_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("php") {
            continue;
        }
        match parse_file(&path) {
            Ok(file) => out.push(file),
            Err(e) => {
                tracing::debug!(file = %path.display(), error = %e, "laravel: config parse skipped");
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse_file(path: &Path) -> Result<ConfigFile, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();

    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
    parser
        .set_language(&lang)
        .map_err(|e| format!("set_language: {e}"))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| "parser returned None".to_string())?;
    let bytes = source.as_bytes();

    // Find the `return [...]` at the top level, then capture each
    // top-level `'key' => value` element.
    let query_text = r#"
        (return_statement
            (array_creation_expression
                (array_element_initializer
                    (string) @key))) @return
    "#;
    let query = tree_sitter::Query::new(&lang, query_text).map_err(|e| format!("query: {e}"))?;
    let key_idx = query
        .capture_names()
        .iter()
        .position(|n| *n == "key")
        .map(|i| i as u32);

    let mut keys = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if Some(cap.index) != key_idx {
                continue;
            }
            let raw = cap.node.utf8_text(bytes).unwrap_or("");
            let key = trim_quotes(raw);
            if key.is_empty() {
                continue;
            }
            let line = cap.node.start_position().row as u32 + 1;
            keys.push(ConfigKey { key, line });
        }
    }

    Ok(ConfigFile {
        name: stem,
        file: path.to_string_lossy().into_owned(),
        keys,
    })
}

fn trim_quotes(s: &str) -> String {
    let s = s.trim();
    let mut chars = s.chars();
    let first = chars.clone().next();
    let last = chars.clone().last();
    if matches!(
        (first, last),
        (Some('\''), Some('\'')) | (Some('"'), Some('"'))
    ) {
        chars.next();
        chars.next_back();
        chars.as_str().to_owned()
    } else {
        s.to_owned()
    }
}
