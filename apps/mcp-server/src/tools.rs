//! Tool handlers. Each takes a JSON arguments object and returns a
//! human-readable text payload. Errors are returned as `Err(String)` and
//! surface to the MCP client as JSON-RPC `-32603` errors.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxyris_index::{Index, SymbolKind};
use oxyris_lsp::lsp_types::DiagnosticSeverity;
use serde_json::Value;

use crate::laravel_state::LaravelState;
use crate::lsp_backend::LspBackend;

pub fn find_symbol(index: Option<&Index>, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'name'".to_string())?;
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(SymbolKind::from_label);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(50) as u32;

    let Some(index) = index else {
        return Ok(no_index_msg());
    };

    let hits = index
        .find_symbol(name, kind, limit)
        .map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Ok(format!("No symbol found matching '{name}'."));
    }

    let mut out = format!("Found {} match(es) for '{name}':\n", hits.len());
    for hit in hits {
        out.push_str(&format!(
            "  • {} ({}) — {}:{}:{}\n",
            hit.name,
            hit.kind.as_str(),
            hit.file,
            hit.start_line,
            hit.start_col,
        ));
    }
    Ok(out)
}

pub fn list_symbols(index: Option<&Index>, args: &Value) -> Result<String, String> {
    let file = args
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'file'".to_string())?;

    let Some(index) = index else {
        return Ok(no_index_msg());
    };

    let symbols = index
        .list_symbols_in_file(file)
        .map_err(|e| e.to_string())?;
    if symbols.is_empty() {
        return Ok(format!(
            "No indexed symbols in '{file}'. The file may not be indexed yet, or its language is not supported."
        ));
    }

    let mut out = format!("{} symbol(s) in {file}:\n", symbols.len());
    for s in symbols {
        out.push_str(&format!(
            "  • {} ({}) — line {}\n",
            s.name,
            s.kind.as_str(),
            s.start_line
        ));
    }
    Ok(out)
}

pub fn project_map(index: Option<&Index>, _args: &Value) -> Result<String, String> {
    let Some(index) = index else {
        return Ok(no_index_msg());
    };

    let map = index.project_map().map_err(|e| e.to_string())?;
    if map.total_files == 0 {
        return Ok(
            "Project index is empty. Run `index_rebuild` from Oxyris to populate it.".into(),
        );
    }

    let mut out = format!(
        "Project map — {} files, {} symbols:\n",
        map.total_files, map.total_symbols,
    );
    for d in map.directories {
        out.push_str(&format!(
            "  • {}/  ({} files, {} symbols)\n",
            d.dir, d.files, d.symbols
        ));
    }
    Ok(out)
}

fn no_index_msg() -> String {
    "Symbol index database is not available for this worktree (yet).".into()
}

// ────── LSP-backed tools ──────────────────────────────────────────────────

pub async fn lsp_find_references(lsp: &Arc<LspBackend>, args: &Value) -> Result<String, String> {
    let (_, line0, col0, file) = parse_position_args(args)?;
    let include_declaration = args
        .get("include_declaration")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let path = lsp.resolve_path(&file)?;
    let locations = lsp
        .find_references(&path, line0, col0, include_declaration)
        .await?;
    if locations.is_empty() {
        return Ok(format!(
            "No references found for the symbol at {file}:{}:{}.",
            line0 + 1,
            col0 + 1
        ));
    }
    let mut out = format!(
        "Found {} reference(s) for the symbol at {file}:{}:{}:\n",
        locations.len(),
        line0 + 1,
        col0 + 1
    );
    for loc in locations {
        let path_disp = lsp.uri_to_display(&loc.uri.to_string());
        let r = loc.range;
        out.push_str(&format!(
            "  • {}:{}:{}\n",
            path_disp,
            r.start.line + 1,
            r.start.character + 1
        ));
    }
    Ok(out)
}

pub async fn lsp_hover(lsp: &Arc<LspBackend>, args: &Value) -> Result<String, String> {
    let (_, line0, col0, file) = parse_position_args(args)?;
    let path = lsp.resolve_path(&file)?;
    let hover = lsp.hover(&path, line0, col0).await?;
    match hover {
        Some(text) => Ok(format!(
            "Hover at {file}:{}:{}:\n\n{text}",
            line0 + 1,
            col0 + 1
        )),
        None => Ok(format!(
            "No hover info for {file}:{}:{}.",
            line0 + 1,
            col0 + 1
        )),
    }
}

/// Cap on rendered diagnostics. A workspace mid-refactor can report hundreds;
/// the first slice (errors first) is what the agent acts on, and the tail would
/// just evict its context.
const MAX_RENDERED: usize = 60;

pub async fn lsp_diagnostics(lsp: &Arc<LspBackend>, args: &Value) -> Result<String, String> {
    let file = args.get("file").and_then(|v| v.as_str());
    let path = match file {
        Some(f) => Some(lsp.resolve_path(f)?),
        None => None,
    };

    let report = lsp.check(path.as_deref()).await?;

    // Flatten to (display path, diagnostic) so the whole set can be ordered by
    // severity — an error three files away matters more than a hint here.
    let mut rows: Vec<(String, &oxyris_lsp::lsp_types::Diagnostic)> = Vec::new();
    for entry in &report.files {
        let display = match &path {
            // Single-file mode: we already know the caller's spelling of it.
            Some(_) => file.unwrap_or("").to_owned(),
            None => lsp.uri_to_display(&entry.uri),
        };
        for d in &entry.diagnostics {
            rows.push((display.clone(), d));
        }
    }
    rows.sort_by_key(|(f, d)| {
        (
            severity_rank(d),
            f.clone(),
            d.range.start.line,
            d.range.start.character,
        )
    });

    let scope = match file {
        Some(f) => f.to_owned(),
        None => "the workspace".to_owned(),
    };
    if rows.is_empty() {
        let how = if report.checked {
            "`cargo check` finished clean"
        } else {
            "the language server reports nothing"
        };
        return Ok(format!("No diagnostics in {scope} — {how}."));
    }

    let errors = rows.iter().filter(|(_, d)| is_error(d)).count();
    let total = rows.len();
    let mut out = format!("{total} diagnostic(s) in {scope} ({errors} error(s))");
    if !report.checked {
        // Be explicit: without a completed check this is analysis-only, so a
        // type error in a *different* crate may be missing. The agent needs to
        // know it cannot treat a clean-ish read as a green build.
        out.push_str(" — no completed `cargo check`, analysis-only");
    }
    out.push_str(":\n");

    for (display, d) in rows.iter().take(MAX_RENDERED) {
        let sev = match d.severity {
            Some(DiagnosticSeverity::ERROR) => "ERROR",
            Some(DiagnosticSeverity::WARNING) => "WARN",
            Some(DiagnosticSeverity::INFORMATION) => "INFO",
            Some(DiagnosticSeverity::HINT) => "HINT",
            _ => "NOTE",
        };
        let line = d.range.start.line + 1;
        let col = d.range.start.character + 1;
        let source = d.source.as_deref().unwrap_or("lsp");
        out.push_str(&format!(
            "  [{sev}] {display}:{line}:{col} ({source}): {}\n",
            d.message.lines().next().unwrap_or("")
        ));
    }
    if total > MAX_RENDERED {
        out.push_str(&format!(
            "  … {} more (ask for a specific `file` to see them)\n",
            total - MAX_RENDERED
        ));
    }
    Ok(out)
}

fn is_error(d: &oxyris_lsp::lsp_types::Diagnostic) -> bool {
    matches!(d.severity, Some(DiagnosticSeverity::ERROR))
}

/// Sort key: errors, warnings, then everything else.
fn severity_rank(d: &oxyris_lsp::lsp_types::Diagnostic) -> u8 {
    match d.severity {
        Some(DiagnosticSeverity::ERROR) => 0,
        Some(DiagnosticSeverity::WARNING) => 1,
        Some(DiagnosticSeverity::INFORMATION) => 2,
        Some(DiagnosticSeverity::HINT) => 3,
        _ => 2,
    }
}

// ────── Laravel-backed tools ──────────────────────────────────────────────

pub async fn laravel_routes(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let filter = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    let routes: Vec<_> = match filter {
        Some(needle) => {
            let lower = needle.to_lowercase();
            snap.routes
                .iter()
                .filter(|r| {
                    r.name
                        .as_ref()
                        .map(|n| n.to_lowercase().contains(&lower))
                        .unwrap_or(false)
                        || r.uri.to_lowercase().contains(&lower)
                })
                .collect()
        }
        None => snap.routes.iter().collect(),
    };
    if routes.is_empty() {
        return Ok("No routes matched.".into());
    }
    let mut out = format!("{} route(s):\n", routes.len());
    for r in routes {
        let method = match &r.method {
            oxyris_laravel::RouteMethod::Get => "GET",
            oxyris_laravel::RouteMethod::Post => "POST",
            oxyris_laravel::RouteMethod::Put => "PUT",
            oxyris_laravel::RouteMethod::Patch => "PATCH",
            oxyris_laravel::RouteMethod::Delete => "DELETE",
            oxyris_laravel::RouteMethod::Options => "OPTIONS",
            oxyris_laravel::RouteMethod::Any => "ANY",
            oxyris_laravel::RouteMethod::Other(s) => s.as_str(),
        };
        let name_disp = r
            .name
            .as_ref()
            .map(|n| format!(" [{}]", n))
            .unwrap_or_default();
        let mw_disp = if r.middleware.is_empty() {
            String::new()
        } else {
            format!(" {{{}}}", r.middleware.join(","))
        };
        out.push_str(&format!(
            "  {method:<7} {} → {}{name_disp}{mw_disp} ({}:{})\n",
            r.uri,
            r.action,
            short_path(&r.file, workspace),
            r.line
        ));
    }
    Ok(out)
}

pub async fn laravel_configs(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let prefix = args
        .get("prefix")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    let mut out = String::new();
    let mut count = 0;
    for file in &snap.configs {
        if let Some(p) = prefix.as_deref()
            && !file.name.starts_with(p)
        {
            continue;
        }
        out.push_str(&format!("{}.{}\n", file.name, "*"));
        for k in &file.keys {
            if let Some(p) = prefix.as_deref() {
                let full = format!("{}.{}", file.name, k.key);
                if !full.starts_with(p) {
                    continue;
                }
            }
            out.push_str(&format!(
                "  • {}.{} ({}:{})\n",
                file.name,
                k.key,
                short_path(&file.file, workspace),
                k.line
            ));
            count += 1;
        }
    }
    if count == 0 {
        return Ok("No configs matched.".into());
    }
    Ok(format!("{count} config key(s):\n{out}"))
}

pub async fn laravel_models(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let needle = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let models: Vec<_> = match needle.as_deref() {
        Some(n) => snap
            .models
            .iter()
            .filter(|m| m.class.to_lowercase().contains(n))
            .collect(),
        None => snap.models.iter().collect(),
    };
    if models.is_empty() {
        return Ok("No models matched.".into());
    }
    let mut out = format!("{} model(s):\n", models.len());
    for m in models {
        out.push_str(&format!(
            "  • {} ({}:{})\n",
            m.class,
            short_path(&m.file, workspace),
            m.line
        ));
        if let Some(t) = &m.table {
            out.push_str(&format!("      table: {t}\n"));
        }
        if !m.fillable.is_empty() {
            out.push_str(&format!("      fillable: {}\n", m.fillable.join(", ")));
        }
        for r in &m.relations {
            let kind = format!("{:?}", r.kind);
            let related = r
                .related
                .as_ref()
                .map(|s| format!(" → {s}"))
                .unwrap_or_default();
            out.push_str(&format!("      ↳ {}() {kind}{related}\n", r.method));
        }
    }
    Ok(out)
}

pub async fn laravel_blade_components(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let needle = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let views: Vec<_> = match needle.as_deref() {
        Some(n) => snap
            .blades
            .iter()
            .filter(|v| v.name.to_lowercase().contains(n))
            .collect(),
        None => snap.blades.iter().collect(),
    };
    if views.is_empty() {
        return Ok("No blade views matched.".into());
    }
    let mut out = format!("{} blade view(s):\n", views.len());
    for v in views {
        out.push_str(&format!(
            "  • {} ({})\n",
            v.name,
            short_path(&v.file, workspace),
        ));
    }
    Ok(out)
}

pub async fn laravel_observers(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let needle = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let items: Vec<_> = match needle.as_deref() {
        Some(n) => snap
            .observers
            .iter()
            .filter(|o| o.class.to_lowercase().contains(n))
            .collect(),
        None => snap.observers.iter().collect(),
    };
    if items.is_empty() {
        return Ok("No observers matched.".into());
    }
    let mut out = format!("{} observer(s):\n", items.len());
    for o in items {
        let model_disp = o
            .model
            .as_ref()
            .map(|m| format!(" → {m}"))
            .unwrap_or_default();
        let events_disp = if o.events.is_empty() {
            String::new()
        } else {
            format!(" [{}]", o.events.join(","))
        };
        out.push_str(&format!(
            "  • {}{model_disp}{events_disp} ({}:{})\n",
            o.class,
            short_path(&o.file, workspace),
            o.line
        ));
    }
    Ok(out)
}

pub async fn laravel_policies(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let needle = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let items: Vec<_> = match needle.as_deref() {
        Some(n) => snap
            .policies
            .iter()
            .filter(|p| p.class.to_lowercase().contains(n))
            .collect(),
        None => snap.policies.iter().collect(),
    };
    if items.is_empty() {
        return Ok("No policies matched.".into());
    }
    let mut out = format!("{} polic(ies):\n", items.len());
    for p in items {
        let model_disp = p
            .model
            .as_ref()
            .map(|m| format!(" → {m}"))
            .unwrap_or_default();
        let abilities_disp = if p.abilities.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.abilities.join(","))
        };
        out.push_str(&format!(
            "  • {}{model_disp}{abilities_disp} ({}:{})\n",
            p.class,
            short_path(&p.file, workspace),
            p.line
        ));
    }
    Ok(out)
}

pub async fn laravel_jobs(
    laravel: &Arc<LaravelState>,
    workspace: &Path,
    args: &Value,
) -> Result<String, String> {
    let snap = laravel
        .get(workspace)
        .await
        .ok_or_else(|| "this workspace is not a Laravel project".to_string())?;
    let needle = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let items: Vec<_> = match needle.as_deref() {
        Some(n) => snap
            .jobs
            .iter()
            .filter(|j| j.class.to_lowercase().contains(n))
            .collect(),
        None => snap.jobs.iter().collect(),
    };
    if items.is_empty() {
        return Ok("No jobs matched.".into());
    }
    let mut out = format!("{} job(s):\n", items.len());
    for j in items {
        let queue_disp = if j.queueable {
            match j.queue.as_deref() {
                Some(q) => format!(" [queue:{q}]"),
                None => " [queueable]".to_owned(),
            }
        } else {
            " [sync]".to_owned()
        };
        out.push_str(&format!(
            "  • {}{queue_disp} ({}:{})\n",
            j.class,
            short_path(&j.file, workspace),
            j.line
        ));
    }
    Ok(out)
}

fn short_path(file: &str, workspace: &Path) -> String {
    let path = Path::new(file);
    if let Ok(rel) = path.strip_prefix(workspace) {
        rel.to_string_lossy().replace('\\', "/")
    } else {
        file.replace('\\', "/")
    }
}

fn parse_position_args(args: &Value) -> Result<(PathBuf, u32, u32, String), String> {
    let file = args
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'file'".to_string())?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing 'line' (1-based)".to_string())?;
    let column = args
        .get("column")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing 'column' (1-based)".to_string())?;
    if line == 0 || column == 0 {
        return Err("line and column are 1-based; use ≥1".into());
    }
    let line0 = (line - 1) as u32;
    let col0 = (column - 1) as u32;
    Ok((PathBuf::from(file), line0, col0, file.to_owned()))
}
