//! Tree-sitter helpers shared by the class-listing modules
//! (observers/policies/jobs). One PHP parser invocation per file, callback
//! invoked for each top-level class.

use std::path::Path;

use tree_sitter::{Node, Parser};

pub(crate) fn walk_php_files(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_php_files(&path, visit);
        } else if path.extension().and_then(|e| e.to_str()) == Some("php") {
            visit(&path);
        }
    }
}

/// Parse `path` and invoke `visitor(class_name, line_1based, class_node,
/// source_bytes)` for every `class_declaration` found anywhere in the
/// file (tree-sitter walks namespaces transparently for us).
pub(crate) fn for_each_class(path: &Path, mut visitor: impl FnMut(&str, u32, Node<'_>, &[u8])) {
    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
    if parser.set_language(&lang).is_err() {
        return;
    }
    let Some(tree) = parser.parse(&source, None) else {
        return;
    };
    let bytes = source.as_bytes();
    visit_classes(tree.root_node(), bytes, &mut visitor);
}

fn visit_classes(
    node: Node<'_>,
    bytes: &[u8],
    visitor: &mut impl FnMut(&str, u32, Node<'_>, &[u8]),
) {
    if node.kind() == "class_declaration"
        && let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = name_node.utf8_text(bytes)
        && !name.is_empty()
    {
        let line = node.start_position().row as u32 + 1;
        visitor(name, line, node, bytes);
    }
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        visit_classes(child, bytes, visitor);
    }
}

/// Returns true when the class's `class_interface_clause` (`implements
/// X, Y\Z`) names `iface`. Comparison is by last namespace segment, so
/// `implements Illuminate\Contracts\Queue\ShouldQueue` matches
/// `iface = "ShouldQueue"`.
pub(crate) fn class_implements(class_node: Node<'_>, bytes: &[u8], iface: &str) -> bool {
    let mut walker = class_node.walk();
    for child in class_node.children(&mut walker) {
        if child.kind() == "class_interface_clause"
            && let Ok(text) = child.utf8_text(bytes)
        {
            for seg in text
                .trim_start_matches("implements")
                .split([',', ' ', '\t', '\n'])
            {
                let last = seg.rsplit('\\').next().unwrap_or("").trim();
                if last == iface {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn class_method_names(class_node: Node<'_>, bytes: &[u8]) -> Vec<String> {
    let Some(body) = class_node.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = body.walk();
    for member in body.children(&mut walker) {
        if member.kind() == "method_declaration"
            && let Some(name) = member.child_by_field_name("name")
            && let Ok(text) = name.utf8_text(bytes)
        {
            out.push(text.to_owned());
        }
    }
    out
}

/// Pull a string-typed property's literal default out of the class body.
/// `protected string $queue = 'high';` → `Some("high")`. Returns None
/// for non-literal defaults.
pub(crate) fn class_string_property(
    class_node: Node<'_>,
    bytes: &[u8],
    name: &str,
) -> Option<String> {
    let body = class_node.child_by_field_name("body")?;
    let mut walker = body.walk();
    for member in body.children(&mut walker) {
        if member.kind() != "property_declaration" {
            continue;
        }
        let mut prop_walker = member.walk();
        for elem in member.children(&mut prop_walker) {
            if elem.kind() != "property_element" {
                continue;
            }
            let Some(name_node) = elem.child_by_field_name("name") else {
                continue;
            };
            let pname = name_node.utf8_text(bytes).unwrap_or("");
            if pname.trim_start_matches('$') != name {
                continue;
            }
            let Some(default) = elem.child_by_field_name("default_value") else {
                continue;
            };
            if default.kind() == "string"
                && let Ok(text) = default.utf8_text(bytes)
            {
                return Some(trim_quotes(text));
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
