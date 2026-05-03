//! Parse `routes/*.php` for static `Route::method(...)` calls. Captures
//! HTTP verb, URI, controller/closure, and the chained `->name(...)`
//! when present.
//!
//! Recovery scope: only top-level `Route::get('/foo', ...)`-style calls.
//! Route groups, resource routes, and macros emit zero or partial entries
//! — that's fine because the tool surfaces this as "discoverable static
//! routes", not "every route in the app".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tree_sitter::{Parser, QueryCursor, StreamingIterator};

use crate::detect::LaravelProject;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RouteMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Any,
    /// `Route::match([...], ...)` or `Route::redirect`. Preserved
    /// verbatim so callers can decide what to do.
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub method: RouteMethod,
    /// `'/users/{id}'` from the first arg. URI as written in the source —
    /// we don't substitute placeholders.
    pub uri: String,
    /// Controller@action or `Closure` token, raw from the source.
    pub action: String,
    /// Optional `->name('...')` chain match.
    pub name: Option<String>,
    /// Combined middleware from enclosing group(s) and any chained
    /// `->middleware(...)` on the route call. Source order preserved:
    /// outer group → inner group → chained.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub middleware: Vec<String>,
    /// Source location.
    pub file: String,
    pub line: u32,
}

pub fn parse_all(project: &LaravelProject) -> Vec<Route> {
    let dir = project.routes_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut routes = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return routes,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("php") {
            continue;
        }
        match parse_file(&path) {
            Ok(mut found) => routes.append(&mut found),
            Err(e) => {
                tracing::debug!(file = %path.display(), error = %e, "laravel: route parse skipped");
            }
        }
    }
    routes
}

fn parse_file(path: &PathBuf) -> Result<Vec<Route>, String> {
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

    // Match `Route::method(uri, ...)`. We pull the URI from the first
    // string argument; `action` is fetched manually from the call's
    // arguments node so the query doesn't double-match on optional
    // captures.
    let query_text = r#"
        (scoped_call_expression
            scope: (name) @scope
            name: (name) @method
            arguments: (arguments
                .
                (argument (string) @uri))) @call
    "#;
    let query = tree_sitter::Query::new(&lang, query_text).map_err(|e| format!("query: {e}"))?;
    let scope_idx = capture_index(&query, "scope");
    let method_idx = capture_index(&query, "method");
    let uri_idx = capture_index(&query, "uri");
    let call_idx = capture_index(&query, "call");

    let mut routes = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut scope: Option<&str> = None;
        let mut method: Option<&str> = None;
        let mut uri: Option<&str> = None;
        let mut call_node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let text = cap.node.utf8_text(bytes).unwrap_or("");
            match Some(cap.index) {
                i if i == scope_idx => scope = Some(text),
                i if i == method_idx => method = Some(text),
                i if i == uri_idx => uri = Some(text),
                i if i == call_idx => call_node = Some(cap.node),
                _ => {}
            }
        }
        let (Some(scope), Some(method), Some(uri), Some(call_node)) =
            (scope, method, uri, call_node)
        else {
            continue;
        };
        if scope != "Route" {
            continue;
        }
        // Filter to actual route-defining methods. `prefix`, `name`,
        // `middleware`, `domain`, etc. also match `Route::method(string)`
        // syntactically but are chain modifiers, not routes themselves.
        let lower = method.to_ascii_lowercase();
        let route_method = match lower.as_str() {
            "get" => RouteMethod::Get,
            "post" => RouteMethod::Post,
            "put" => RouteMethod::Put,
            "patch" => RouteMethod::Patch,
            "delete" => RouteMethod::Delete,
            "options" => RouteMethod::Options,
            "any" => RouteMethod::Any,
            "match" | "redirect" | "view" | "fallback" | "resource" | "apiresource" => {
                RouteMethod::Other(lower.clone())
            }
            _ => continue,
        };
        // Pull the second positional argument for `action` if present.
        let action = action_text(call_node, bytes);
        // Chained ->name('foo')? Walk up the parent and look for member
        // access with `name` and a string arg.
        let chained_name = chained_name(call_node, bytes);
        // Chained ->middleware(...) on this route call.
        let chained_mw = chained_middleware(call_node, bytes);
        // Enclosing `Route::prefix('admin')->group(fn() => ...)` chain
        // (or `Route::group(['prefix'=>...,'as'=>...,'middleware'=>...], fn)`)
        // contributes URI/name/middleware propagated to every nested route.
        let group = enclosing_group_modifiers(call_node, bytes);
        let line = call_node.start_position().row as u32 + 1;
        let raw_uri = trim_php_string(uri);
        let trimmed_uri = if group.uri.is_empty() {
            raw_uri
        } else {
            format!("{}{}", group.uri, normalize_uri(&raw_uri))
        };
        let chained_name = match (group.name.is_empty(), chained_name) {
            (true, name) => name,
            (false, Some(n)) => Some(format!("{}{n}", group.name)),
            (false, None) => None,
        };
        let mut combined_mw = group.middleware.clone();
        combined_mw.extend(chained_mw);
        let action_text = action.unwrap_or_default();
        let file = path.to_string_lossy().into_owned();

        // `resource` / `apiResource` expand into a fixed set of REST
        // endpoints. The conventional naming uses singular for path
        // params, derived from the last URI segment.
        if let Some(expanded) = expand_resource(
            method,
            &trimmed_uri,
            &action_text,
            chained_name.as_deref(),
            &combined_mw,
            &file,
            line,
        ) {
            routes.extend(expanded);
            continue;
        }

        routes.push(Route {
            method: route_method,
            uri: trimmed_uri,
            action: action_text,
            name: chained_name,
            middleware: combined_mw,
            file,
            line,
        });
    }
    Ok(routes)
}

/// Returns the 7-route expansion for `Route::resource` or 5-route
/// expansion for `Route::apiResource`. Anything else returns `None`.
///
/// Dot-notation resource names trigger nested expansion:
/// `posts.comments` → `/posts/{post}/comments[/{comment}[...]]` with
/// name base `posts.comments`. Each parent segment contributes its
/// singularized path param.
fn expand_resource(
    method: &str,
    uri: &str,
    action: &str,
    name_chain: Option<&str>,
    middleware: &[String],
    file: &str,
    line: u32,
) -> Option<Vec<Route>> {
    let is_api = match method.to_ascii_lowercase().as_str() {
        "resource" => false,
        "apiresource" => true,
        _ => return None,
    };
    // `apiResource` skips the form-rendering endpoints (create + edit).
    let endpoints: &[(RouteMethod, &str, &str)] = if is_api {
        &[
            (RouteMethod::Get, "", "index"),
            (RouteMethod::Post, "", "store"),
            (RouteMethod::Get, "/{P}", "show"),
            (RouteMethod::Put, "/{P}", "update"),
            (RouteMethod::Delete, "/{P}", "destroy"),
        ]
    } else {
        &[
            (RouteMethod::Get, "", "index"),
            (RouteMethod::Get, "/create", "create"),
            (RouteMethod::Post, "", "store"),
            (RouteMethod::Get, "/{P}", "show"),
            (RouteMethod::Get, "/{P}/edit", "edit"),
            (RouteMethod::Put, "/{P}", "update"),
            (RouteMethod::Delete, "/{P}", "destroy"),
        ]
    };

    let base_uri = uri.trim_end_matches('/');
    let raw_resource = base_uri.rsplit('/').next().unwrap_or("");
    let parts: Vec<&str> = raw_resource.split('.').filter(|p| !p.is_empty()).collect();
    let final_segment = parts.last().copied().unwrap_or("");
    let param = singularize(final_segment);

    // Build the nested URI from all parts. Parents inject `/{singular}` after
    // their segment; only the final segment is bare (the endpoints append
    // `/{P}` themselves where needed).
    let nested_path: String = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i + 1 == parts.len() {
                format!("/{p}")
            } else {
                format!("/{p}/{{{}}}", singularize(p))
            }
        })
        .collect();
    let trunk = base_uri
        .strip_suffix(raw_resource)
        .unwrap_or(base_uri)
        .trim_end_matches('/');
    let nested_base = format!("{trunk}{nested_path}");

    let trunk_dotted = trunk.trim_start_matches('/').replace('/', ".");
    let name_from_uri = if trunk_dotted.is_empty() {
        parts.join(".")
    } else {
        format!("{trunk_dotted}.{}", parts.join("."))
    };
    let name_base = name_chain
        .map(|s| s.trim_end_matches('.').to_owned())
        .unwrap_or(name_from_uri);

    let mut out = Vec::with_capacity(endpoints.len());
    for (m, suffix, action_name) in endpoints {
        let path_suffix = suffix.replace("{P}", &format!("{{{param}}}"));
        let action_qualified = if action.is_empty() {
            String::new()
        } else {
            format!("{action}@{action_name}")
        };
        out.push(Route {
            method: m.clone(),
            uri: format!("{nested_base}{path_suffix}"),
            action: action_qualified,
            name: Some(format!("{name_base}.{action_name}")),
            middleware: middleware.to_vec(),
            file: file.to_owned(),
            line,
        });
    }
    Some(out)
}

#[derive(Default)]
struct GroupModifiers {
    uri: String,
    name: String,
    middleware: Vec<String>,
}

/// Walk up the AST from a `Route::method(...)` call until we find an
/// enclosing `Route::group(...)` (top-level static call) or
/// `->group(...)` (chained). Either form may carry an array first arg
/// `['prefix'=>..., 'as'=>..., 'middleware'=>[...]]`. Chained `->prefix`,
/// `->name`, `->middleware` calls preceding the group also contribute.
///
/// Returns merged URI prefix, name prefix, and middleware list. Empty
/// when no group encloses.
fn enclosing_group_modifiers(start: tree_sitter::Node<'_>, bytes: &[u8]) -> GroupModifiers {
    let mut cur = start.parent();
    while let Some(node) = cur {
        let is_group = matches!(
            node.kind(),
            "member_call_expression" | "scoped_call_expression"
        ) && node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(bytes).ok())
            == Some("group");
        if is_group {
            return collect_group_modifiers(node, bytes);
        }
        cur = node.parent();
    }
    GroupModifiers::default()
}

fn collect_group_modifiers(group_call: tree_sitter::Node<'_>, bytes: &[u8]) -> GroupModifiers {
    let mut uri_segments: Vec<String> = Vec::new();
    let mut name_segments: Vec<String> = Vec::new();
    let mut middleware: Vec<String> = Vec::new();

    // Walk back through the chain of receivers preceding ->group(...).
    // Static `Route::group(...)` has no chain; only the array arg matters.
    let mut node = group_call.child_by_field_name("object");
    let mut chain_uris: Vec<String> = Vec::new();
    let mut chain_names: Vec<String> = Vec::new();
    // Per-call buckets so reversing the outer order doesn't scramble the
    // strings inside a single `middleware(['a','b'])` call.
    let mut chain_mid_buckets: Vec<Vec<String>> = Vec::new();
    while let Some(n) = node {
        let (method_name, next) = match n.kind() {
            "member_call_expression" => {
                let name_text = n
                    .child_by_field_name("name")
                    .and_then(|nm| nm.utf8_text(bytes).ok())
                    .map(|s| s.to_owned());
                (name_text, n.child_by_field_name("object"))
            }
            "scoped_call_expression" => {
                let name_text = n
                    .child_by_field_name("name")
                    .and_then(|nm| nm.utf8_text(bytes).ok())
                    .map(|s| s.to_owned());
                (name_text, None)
            }
            _ => (None, None),
        };
        if let Some(name) = method_name.as_deref() {
            match name {
                "prefix" => {
                    if let Some(s) = first_string_arg(n, bytes) {
                        chain_uris.push(s);
                    }
                }
                "name" => {
                    if let Some(s) = first_string_arg(n, bytes) {
                        chain_names.push(s);
                    }
                }
                "middleware" => {
                    chain_mid_buckets.push(call_string_args(n, bytes));
                }
                _ => {}
            }
        }
        if next.is_none() {
            break;
        }
        node = next;
    }
    chain_uris.reverse();
    chain_names.reverse();
    chain_mid_buckets.reverse();
    uri_segments.extend(chain_uris);
    name_segments.extend(chain_names);
    middleware.extend(chain_mid_buckets.into_iter().flatten());

    // Now the array first arg of group itself: `Route::group(['prefix'=>..],fn)`
    // or `Route::middleware(...)->group(['as'=>..], fn)`.
    let array_args = parse_group_array_arg(group_call, bytes);
    if let Some(p) = array_args.prefix {
        uri_segments.push(p);
    }
    if let Some(n) = array_args.name {
        name_segments.push(n);
    }
    middleware.extend(array_args.middleware);

    let uri = uri_segments
        .iter()
        .map(|s| format!("/{}", s.trim_matches('/')))
        .collect::<String>();
    let name = name_segments.join("");
    GroupModifiers {
        uri,
        name,
        middleware,
    }
}

#[derive(Default)]
struct GroupArrayArgs {
    prefix: Option<String>,
    name: Option<String>,
    middleware: Vec<String>,
}

/// Parse `['prefix'=>'admin', 'as'=>'admin.', 'middleware'=>['auth','x']]`
/// when it's the first positional arg of a group call. Unrecognized keys
/// are ignored.
fn parse_group_array_arg(call_node: tree_sitter::Node<'_>, bytes: &[u8]) -> GroupArrayArgs {
    let mut out = GroupArrayArgs::default();
    let Some(args) = call_node.child_by_field_name("arguments") else {
        return out;
    };
    let mut walker = args.walk();
    let Some(first_arg) = args.children(&mut walker).find(|c| c.kind() == "argument") else {
        return out;
    };
    let mut aw = first_arg.walk();
    let Some(arr) = first_arg
        .named_children(&mut aw)
        .find(|c| c.kind() == "array_creation_expression")
    else {
        return out;
    };
    let mut ew = arr.walk();
    for elem in arr.named_children(&mut ew) {
        if elem.kind() != "array_element_initializer" {
            continue;
        }
        let mut iw = elem.walk();
        let named: Vec<_> = elem.named_children(&mut iw).collect();
        if named.len() < 2 {
            continue;
        }
        let key_node = named[0];
        let value_node = named[1];
        if key_node.kind() != "string" {
            continue;
        }
        let key = trim_php_string(key_node.utf8_text(bytes).unwrap_or(""));
        match key.as_str() {
            "prefix" if value_node.kind() == "string" => {
                out.prefix = Some(trim_php_string(value_node.utf8_text(bytes).unwrap_or("")));
            }
            "as" if value_node.kind() == "string" => {
                out.name = Some(trim_php_string(value_node.utf8_text(bytes).unwrap_or("")));
            }
            "middleware" => {
                out.middleware = collect_strings(value_node, bytes);
            }
            _ => {}
        }
    }
    out
}

/// Pull every string literal out of a call's positional arguments.
/// `->middleware('auth')` → `["auth"]`,
/// `->middleware('auth','throttle')` → `["auth","throttle"]`,
/// `->middleware(['auth','throttle'])` → `["auth","throttle"]`.
fn call_string_args(call_node: tree_sitter::Node<'_>, bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(args) = call_node.child_by_field_name("arguments") else {
        return out;
    };
    let mut walker = args.walk();
    for child in args.children(&mut walker) {
        if child.kind() != "argument" {
            continue;
        }
        let mut iw = child.walk();
        for inner in child.named_children(&mut iw) {
            out.extend(collect_strings(inner, bytes));
        }
    }
    out
}

/// Extract string literals from a value node — either a single `(string)`
/// or an `(array_creation_expression)` whose elements are strings.
fn collect_strings(node: tree_sitter::Node<'_>, bytes: &[u8]) -> Vec<String> {
    if node.kind() == "string" {
        return vec![trim_php_string(node.utf8_text(bytes).unwrap_or(""))];
    }
    if node.kind() == "array_creation_expression" {
        let mut out = Vec::new();
        let mut w = node.walk();
        for elem in node.named_children(&mut w) {
            if elem.kind() != "array_element_initializer" {
                continue;
            }
            let mut iw = elem.walk();
            let named: Vec<_> = elem.named_children(&mut iw).collect();
            // Unkeyed: [val]; keyed: [key, val] — middleware lists are
            // typically unkeyed, but tolerate `['x'=>'auth']` as well.
            let val = match named.len() {
                1 => named[0],
                2 => named[1],
                _ => continue,
            };
            if val.kind() == "string" {
                out.push(trim_php_string(val.utf8_text(bytes).unwrap_or("")));
            }
        }
        return out;
    }
    Vec::new()
}

/// Walk up from a `Route::method(...)` call, collecting every chained
/// `->middleware(...)` along the way. Stops at the closest enclosing
/// closure/program — we don't want to leak group-level middleware here
/// (that's `enclosing_group_modifiers`'s job).
fn chained_middleware(start: tree_sitter::Node<'_>, bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = match start.parent() {
        Some(n) => n,
        None => return out,
    };
    loop {
        if cur.kind() == "member_call_expression"
            && let Some(name) = cur.child_by_field_name("name")
            && name.utf8_text(bytes).ok() == Some("middleware")
        {
            out.extend(call_string_args(cur, bytes));
        }
        cur = match cur.parent() {
            Some(n) => n,
            None => break,
        };
        if matches!(
            cur.kind(),
            "program" | "anonymous_function_creation_expression" | "arrow_function"
        ) {
            break;
        }
    }
    out
}

fn first_string_arg(call_node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let args = call_node.child_by_field_name("arguments")?;
    let mut walker = args.walk();
    for child in args.children(&mut walker) {
        if child.kind() == "argument" {
            let mut w = child.walk();
            for inner in child.children(&mut w) {
                if inner.kind() == "string" {
                    return Some(trim_php_string(inner.utf8_text(bytes).ok()?));
                }
            }
        }
    }
    None
}

/// Strip a leading `/` so URI concatenation doesn't double-slash.
fn normalize_uri(uri: &str) -> String {
    if uri.is_empty() {
        return String::new();
    }
    if uri.starts_with('/') {
        uri.to_owned()
    } else {
        format!("/{uri}")
    }
}

/// Rough-and-ready English singularization for path-param naming. Good
/// enough for `users → user`, `categories → category`, `posts → post`.
fn singularize(plural: &str) -> String {
    if plural.ends_with("ies") && plural.len() > 3 {
        format!("{}y", &plural[..plural.len() - 3])
    } else if plural.ends_with('s') && plural.len() > 1 && !plural.ends_with("ss") {
        plural[..plural.len() - 1].to_owned()
    } else {
        plural.to_owned()
    }
}

/// Pull the second positional argument from `Route::method(uri, action)`
/// as a raw source string. Returns `None` for closures or otherwise
/// non-trivial expressions we'd rather not display verbatim.
fn action_text(call_node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let args = call_node.child_by_field_name("arguments")?;
    let mut walker = args.walk();
    let mut nth = 0;
    for child in args.children(&mut walker) {
        if child.kind() != "argument" {
            continue;
        }
        nth += 1;
        if nth != 2 {
            continue;
        }
        let text = child.utf8_text(bytes).ok()?.trim();
        return Some(text.to_owned());
    }
    None
}

/// Walk up from a `Route::method(...)` call expression, climbing the
/// chain looking for `->name('xxx')`. Returns the literal string when
/// found.
fn chained_name(start: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cur = start.parent()?;
    loop {
        if cur.kind() == "member_call_expression" {
            let name = cur.child_by_field_name("name")?;
            if name.utf8_text(bytes).ok() == Some("name") {
                let args = cur.child_by_field_name("arguments")?;
                let mut walker = args.walk();
                for child in args.children(&mut walker) {
                    if child.kind() == "argument" {
                        let mut arg_walker = child.walk();
                        for inner in child.children(&mut arg_walker) {
                            if inner.kind() == "string" {
                                return Some(trim_php_string(inner.utf8_text(bytes).ok()?));
                            }
                        }
                    }
                }
                return None;
            }
        }
        cur = cur.parent()?;
        if cur.kind() == "program" {
            return None;
        }
    }
}

fn trim_php_string(s: &str) -> String {
    let s = s.trim();
    let mut chars = s.chars();
    let first = chars.clone().next();
    let last = chars.clone().last();
    if matches!(
        (first, last),
        (Some('\''), Some('\'')) | (Some('"'), Some('"'))
    ) {
        // Drop quotes.
        chars.next();
        chars.next_back();
        chars.as_str().to_owned()
    } else {
        s.to_owned()
    }
}

fn capture_index(query: &tree_sitter::Query, name: &str) -> Option<u32> {
    query
        .capture_names()
        .iter()
        .position(|n| *n == name)
        .map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_routes(dir: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(dir.join("routes")).unwrap();
        std::fs::write(dir.join("routes").join("web.php"), contents).unwrap();
    }

    #[test]
    fn parses_basic_routes() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::get('/users', [UserController::class, 'index'])->name('users.index');
Route::post('/users', [UserController::class, 'store']);
Route::delete('/users/{id}', 'UserController@destroy')->name('users.destroy');
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].method, RouteMethod::Get);
        assert_eq!(routes[0].uri, "/users");
        assert_eq!(routes[0].name.as_deref(), Some("users.index"));
        assert_eq!(routes[2].uri, "/users/{id}");
        assert_eq!(routes[2].name.as_deref(), Some("users.destroy"));
    }

    #[test]
    fn expands_resource_routes() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::resource('/users', UserController::class);
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 7);
        let names: Vec<&str> = routes.iter().filter_map(|r| r.name.as_deref()).collect();
        assert!(names.contains(&"users.index"));
        assert!(names.contains(&"users.create"));
        assert!(names.contains(&"users.store"));
        assert!(names.contains(&"users.show"));
        assert!(names.contains(&"users.edit"));
        assert!(names.contains(&"users.update"));
        assert!(names.contains(&"users.destroy"));
        // Param uses singular last-segment.
        assert!(routes.iter().any(|r| r.uri == "/users/{user}"));
        assert!(routes.iter().any(|r| r.uri == "/users/{user}/edit"));
    }

    #[test]
    fn expands_api_resource_skips_create_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::apiResource('/posts', PostController::class);
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 5);
        let names: Vec<&str> = routes.iter().filter_map(|r| r.name.as_deref()).collect();
        assert!(!names.contains(&"posts.create"));
        assert!(!names.contains(&"posts.edit"));
        assert!(names.contains(&"posts.show"));
    }

    #[test]
    fn singularize_handles_y_to_ies() {
        assert_eq!(singularize("categories"), "category");
        assert_eq!(singularize("users"), "user");
        assert_eq!(singularize("posts"), "post");
        assert_eq!(singularize("class"), "class");
    }

    #[test]
    fn applies_prefix_group_modifier() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::prefix('admin')->name('admin.')->group(function () {
    Route::get('/users', [UserController::class, 'index'])->name('users.index');
    Route::post('/users', [UserController::class, 'store'])->name('users.store');
});
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].uri, "/admin/users");
        assert_eq!(routes[0].name.as_deref(), Some("admin.users.index"));
        assert_eq!(routes[1].uri, "/admin/users");
        assert_eq!(routes[1].name.as_deref(), Some("admin.users.store"));
    }

    #[test]
    fn array_syntax_group_modifiers() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::group(['prefix' => 'admin', 'as' => 'admin.', 'middleware' => ['auth', 'verified']], function () {
    Route::get('/users', 'UserController@index')->name('users.index');
    Route::post('/users', 'UserController@store')->name('users.store');
});
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].uri, "/admin/users");
        assert_eq!(routes[0].name.as_deref(), Some("admin.users.index"));
        assert_eq!(routes[0].middleware, vec!["auth", "verified"]);
        assert_eq!(routes[1].uri, "/admin/users");
        assert_eq!(routes[1].middleware, vec!["auth", "verified"]);
    }

    #[test]
    fn array_group_combined_with_chain() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::middleware('web')->prefix('api')->group(['prefix' => 'v1', 'middleware' => 'throttle'], function () {
    Route::get('/users', 'UserController@index');
});
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].uri, "/api/v1/users");
        assert_eq!(routes[0].middleware, vec!["web", "throttle"]);
    }

    #[test]
    fn nested_resource_dot_notation() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::resource('posts.comments', CommentController::class);
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 7);
        assert!(routes.iter().any(|r| r.uri == "/posts/{post}/comments"));
        assert!(
            routes
                .iter()
                .any(|r| r.uri == "/posts/{post}/comments/create")
        );
        assert!(
            routes
                .iter()
                .any(|r| r.uri == "/posts/{post}/comments/{comment}")
        );
        assert!(
            routes
                .iter()
                .any(|r| r.uri == "/posts/{post}/comments/{comment}/edit")
        );
        let names: Vec<&str> = routes.iter().filter_map(|r| r.name.as_deref()).collect();
        assert!(names.contains(&"posts.comments.index"));
        assert!(names.contains(&"posts.comments.show"));
        assert!(names.contains(&"posts.comments.destroy"));
    }

    #[test]
    fn chained_middleware_on_route() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::get('/profile', 'ProfileController@show')->middleware(['auth', 'throttle:60,1'])->name('profile.show');
Route::post('/logout', 'AuthController@logout')->middleware('auth');
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].middleware, vec!["auth", "throttle:60,1"]);
        assert_eq!(routes[0].name.as_deref(), Some("profile.show"));
        assert_eq!(routes[1].middleware, vec!["auth"]);
    }

    #[test]
    fn group_middleware_propagates_to_resource_expansion() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::middleware(['auth', 'verified'])->group(function () {
    Route::resource('/users', UserController::class);
});
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 7);
        for r in &routes {
            assert_eq!(r.middleware, vec!["auth", "verified"]);
        }
    }

    #[test]
    fn nested_prefix_chain_concatenates() {
        let dir = tempfile::tempdir().unwrap();
        write_routes(
            dir.path(),
            r#"<?php
use Illuminate\Support\Facades\Route;
Route::prefix('api')->prefix('v1')->group(function () {
    Route::get('/users', 'UserController@index');
});
"#,
        );
        let routes = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].uri, "/api/v1/users");
    }
}
