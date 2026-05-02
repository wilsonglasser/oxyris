//! Run the per-language tree-sitter query against a parsed tree to produce a
//! flat list of [`Symbol`]s.

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator, Tree};

use crate::language::Lang;
use crate::{IndexError, Symbol, SymbolKind};

/// Stateful per-language extractor. Holds a [`Parser`] and pre-compiled
/// [`Query`] so we don't pay setup costs per file.
pub struct Extractor {
    pub lang: Lang,
    parser: Parser,
    query: Query,
    name_capture: u32,
    /// (capture_index, kind) for every kind capture name we recognize.
    kind_captures: Vec<(u32, SymbolKind)>,
}

impl Extractor {
    pub fn new(lang: Lang) -> Result<Self, IndexError> {
        let mut parser = Parser::new();
        let ts_lang = lang.tree_sitter_language();
        parser
            .set_language(&ts_lang)
            .map_err(|e| IndexError::Parser(e.to_string()))?;
        let query = lang
            .build_query()
            .map_err(|e| IndexError::Query(e.to_string()))?;

        let mut name_capture: Option<u32> = None;
        let mut kind_captures: Vec<(u32, SymbolKind)> = Vec::new();
        for (idx, name) in query.capture_names().iter().enumerate() {
            let idx = idx as u32;
            if *name == "name" {
                name_capture = Some(idx);
                continue;
            }
            if let Some(kind) = SymbolKind::from_capture(name) {
                kind_captures.push((idx, kind));
            }
        }

        let name_capture = name_capture.ok_or_else(|| {
            IndexError::Query(format!("query for {:?} has no @name capture", lang))
        })?;
        if kind_captures.is_empty() {
            return Err(IndexError::Query(format!(
                "query for {:?} has no kind captures",
                lang
            )));
        }

        Ok(Self {
            lang,
            parser,
            query,
            name_capture,
            kind_captures,
        })
    }

    pub fn parse(&mut self, source: &str) -> Option<Tree> {
        self.parser.parse(source, None)
    }

    /// Extract symbols from already-parsed source. Returns symbols in document
    /// order (sorted by start line, then column).
    pub fn extract(&self, tree: &Tree, source: &str) -> Vec<Symbol> {
        let bytes = source.as_bytes();
        let mut cursor = QueryCursor::new();
        let mut out: Vec<Symbol> = Vec::new();

        let mut matches = cursor.matches(&self.query, tree.root_node(), bytes);
        while let Some(m) = matches.next() {
            let mut name_node = None;
            let mut kind: Option<SymbolKind> = None;
            let mut kind_node = None;
            for cap in m.captures {
                if cap.index == self.name_capture {
                    name_node = Some(cap.node);
                } else if let Some((_, k)) =
                    self.kind_captures.iter().find(|(i, _)| *i == cap.index)
                {
                    kind = Some(*k);
                    kind_node = Some(cap.node);
                }
            }
            let (Some(name_node), Some(kind), Some(outer)) = (name_node, kind, kind_node) else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(bytes) else {
                continue;
            };
            let start = outer.start_position();
            let end = outer.end_position();
            out.push(Symbol {
                name: name.to_owned(),
                kind,
                start_line: start.row as u32 + 1,
                start_col: start.column as u32 + 1,
                end_line: end.row as u32 + 1,
                end_col: end.column as u32 + 1,
            });
        }
        out.sort_by(|a, b| {
            a.start_line
                .cmp(&b.start_line)
                .then(a.start_col.cmp(&b.start_col))
        });
        out
    }
}
