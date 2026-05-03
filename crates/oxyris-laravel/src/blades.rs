//! Walk `resources/views/**/*.blade.php` and translate paths to Laravel
//! component dot-notation. We don't parse Blade — just enumerate the
//! views the framework would resolve.
//!
//! Naming: `resources/views/admin/users/index.blade.php` → `admin.users.index`.
//! Components in `resources/views/components/` get a leading `components.`
//! to match Blade's component lookup.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::detect::LaravelProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BladeView {
    /// Dot-notation Laravel uses (`view('admin.users.index')`).
    pub name: String,
    /// Absolute filesystem path.
    pub file: String,
}

pub fn list_all(project: &LaravelProject) -> Vec<BladeView> {
    let root = project.views_dir();
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<BladeView>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(base, &path, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".blade.php") {
            continue;
        }
        let stem = &name[..name.len() - ".blade.php".len()];
        let parent = path.parent().unwrap_or(base);
        let rel = parent.strip_prefix(base).unwrap_or(parent);
        let mut parts: Vec<String> = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(String::from))
            .collect();
        parts.push(stem.to_owned());
        let dot_name = parts.join(".");
        if dot_name.is_empty() {
            continue;
        }
        out.push(BladeView {
            name: dot_name,
            file: path.to_string_lossy().into_owned(),
        });
    }
}
