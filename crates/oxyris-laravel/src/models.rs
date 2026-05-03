//! Walk `app/Models/**/*.php`, recover Eloquent model class names, plus
//! `$table` / `$fillable` properties and relation methods (`hasMany`,
//! `belongsTo`, `hasOne`, `belongsToMany`, `morphTo`, `morphMany`).
//!
//! We're strict about "extends Model" — anything else is skipped. That
//! keeps the noise out of the resulting list (helpers, traits, enums in
//! the same dir).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::Parser;

use crate::detect::LaravelProject;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    HasOne,
    HasMany,
    BelongsTo,
    BelongsToMany,
    HasOneThrough,
    HasManyThrough,
    MorphOne,
    MorphMany,
    MorphTo,
    MorphToMany,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRelation {
    pub method: String,
    pub kind: RelationKind,
    /// First argument to the relation call when present (the related
    /// model FQN or short name).
    pub related: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub class: String,
    pub file: String,
    pub line: u32,
    pub table: Option<String>,
    pub fillable: Vec<String>,
    pub relations: Vec<ModelRelation>,
}

pub fn parse_all(project: &LaravelProject) -> Vec<Model> {
    let dir = project.models_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk_php(&dir, &mut |path| match parse_file(path) {
        Ok(Some(model)) => out.push(model),
        Ok(None) => {}
        Err(e) => {
            tracing::debug!(file = %path.display(), error = %e, "laravel: model parse skipped");
        }
    });
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}

fn walk_php(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_php(&path, visit);
        } else if path.extension().and_then(|e| e.to_str()) == Some("php") {
            visit(&path);
        }
    }
}

fn parse_file(path: &Path) -> Result<Option<Model>, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
    parser
        .set_language(&lang)
        .map_err(|e| format!("set_language: {e}"))?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| "parser returned None".to_string())?;
    let bytes = source.as_bytes();
    let root = tree.root_node();

    // Find the first class_declaration that extends a name ending in
    // `Model`. That's our Eloquent model. Anything else, skip.
    let mut class_node: Option<tree_sitter::Node> = None;
    let mut walker = root.walk();
    for child in root.children(&mut walker) {
        if let Some(found) = find_model_class(child, bytes) {
            class_node = Some(found);
            break;
        }
    }
    let Some(class_node) = class_node else {
        return Ok(None);
    };

    let class = class_node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(bytes).ok())
        .unwrap_or("")
        .to_owned();
    if class.is_empty() {
        return Ok(None);
    }
    let line = class_node.start_position().row as u32 + 1;

    let body = class_node.child_by_field_name("body");
    let mut table: Option<String> = None;
    let mut fillable: Vec<String> = Vec::new();
    let mut relations: Vec<ModelRelation> = Vec::new();

    if let Some(body) = body {
        let mut body_walker = body.walk();
        for member in body.children(&mut body_walker) {
            match member.kind() {
                "property_declaration" => {
                    inspect_property(member, bytes, &mut table, &mut fillable);
                }
                "method_declaration" => {
                    if let Some(rel) = extract_relation(member, bytes) {
                        relations.push(rel);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(Some(Model {
        class,
        file: path.to_string_lossy().into_owned(),
        line,
        table,
        fillable,
        relations,
    }))
}

fn find_model_class<'a>(
    node: tree_sitter::Node<'a>,
    bytes: &[u8],
) -> Option<tree_sitter::Node<'a>> {
    if node.kind() == "class_declaration" && extends_model(node, bytes) {
        return Some(node);
    }
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if let Some(found) = find_model_class(child, bytes) {
            return Some(found);
        }
    }
    None
}

fn extends_model(class_node: tree_sitter::Node<'_>, bytes: &[u8]) -> bool {
    let mut walker = class_node.walk();
    for child in class_node.children(&mut walker) {
        if child.kind() == "base_clause"
            && let Ok(text) = child.utf8_text(bytes)
        {
            let text = text.trim().trim_start_matches("extends").trim();
            let last_seg = text.rsplit(['\\', ',', ' ']).next().unwrap_or("");
            if last_seg == "Model" || text == "Model" {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::collapsible_match)]
fn inspect_property(
    node: tree_sitter::Node<'_>,
    bytes: &[u8],
    table: &mut Option<String>,
    fillable: &mut Vec<String>,
) {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if child.kind() != "property_element" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let name = name_node
            .utf8_text(bytes)
            .unwrap_or("")
            .trim_start_matches('$');
        let Some(default) = child.child_by_field_name("default_value") else {
            continue;
        };
        match name {
            "table" => {
                if let Ok(s) = default.utf8_text(bytes) {
                    *table = Some(trim_quotes(s));
                }
            }
            "fillable" => {
                if default.kind() == "array_creation_expression" {
                    let mut arr_walker = default.walk();
                    for elem in default.children(&mut arr_walker) {
                        if elem.kind() == "array_element_initializer" {
                            let mut elem_walker = elem.walk();
                            for inner in elem.children(&mut elem_walker) {
                                if inner.kind() == "string"
                                    && let Ok(s) = inner.utf8_text(bytes)
                                {
                                    fillable.push(trim_quotes(s));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_relation(method: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<ModelRelation> {
    let name_node = method.child_by_field_name("name")?;
    let method_name = name_node.utf8_text(bytes).ok()?.to_owned();
    let line = name_node.start_position().row as u32 + 1;
    let body = method.child_by_field_name("body")?;
    // Look for the first `$this->relationFn(...)` call inside the body.
    let mut found: Option<(RelationKind, Option<String>)> = None;
    walk_for_relation(body, bytes, &mut found);
    let (kind, related) = found?;
    Some(ModelRelation {
        method: method_name,
        kind,
        related,
        line,
    })
}

fn walk_for_relation<'a>(
    node: tree_sitter::Node<'a>,
    bytes: &[u8],
    out: &mut Option<(RelationKind, Option<String>)>,
) {
    if out.is_some() {
        return;
    }
    if node.kind() == "member_call_expression" {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(bytes).ok());
        if let Some(name) = name {
            let kind = match name {
                "hasOne" => Some(RelationKind::HasOne),
                "hasMany" => Some(RelationKind::HasMany),
                "belongsTo" => Some(RelationKind::BelongsTo),
                "belongsToMany" => Some(RelationKind::BelongsToMany),
                "hasOneThrough" => Some(RelationKind::HasOneThrough),
                "hasManyThrough" => Some(RelationKind::HasManyThrough),
                "morphOne" => Some(RelationKind::MorphOne),
                "morphMany" => Some(RelationKind::MorphMany),
                "morphTo" => Some(RelationKind::MorphTo),
                "morphToMany" => Some(RelationKind::MorphToMany),
                _ => None,
            };
            if let Some(kind) = kind {
                let related = first_arg_string(node, bytes);
                *out = Some((kind, related));
                return;
            }
        }
    }
    let mut child_walker = node.walk();
    for child in node.children(&mut child_walker) {
        walk_for_relation(child, bytes, out);
        if out.is_some() {
            return;
        }
    }
}

fn first_arg_string(call_node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let args = call_node.child_by_field_name("arguments")?;
    let mut walker = args.walk();
    for child in args.children(&mut walker) {
        if child.kind() == "argument" {
            let mut arg_walker = child.walk();
            for inner in child.children(&mut arg_walker) {
                match inner.kind() {
                    "class_constant_access_expression" => {
                        // `User::class`
                        if let Ok(text) = inner.utf8_text(bytes) {
                            let trimmed = text.trim_end_matches("::class").trim();
                            return Some(trimmed.to_owned());
                        }
                    }
                    "string" => {
                        if let Ok(text) = inner.utf8_text(bytes) {
                            return Some(trim_quotes(text));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
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
