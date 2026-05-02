//! Tool handlers. Each takes a JSON arguments object and returns a
//! human-readable text payload. Errors are returned as `Err(String)` and
//! surface to the MCP client as JSON-RPC `-32603` errors.

use oxyris_index::{Index, SymbolKind};
use serde_json::Value;

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
