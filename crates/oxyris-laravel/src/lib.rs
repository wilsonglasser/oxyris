//! Laravel project introspection. Surfaces routes / configs / Eloquent
//! models / Blade view components as structured data so the MCP layer can
//! expose each as its own Claude tool. Entirely static — we parse files
//! with tree-sitter, never invoke `php artisan`.
//!
//! Detection contract: a Laravel project has both `artisan` at the root
//! and a `composer.json` listing `laravel/framework` as a dep. We don't
//! activate without both signals so plain PHP projects skip this layer.
//!
//! What gets parsed:
//! - `routes/*.php` — `Route::get/post/put/patch/delete/...` calls,
//!   recovering URI + method + handler + name (when chained).
//! - `config/*.php` — top-level array keys (with the file as the prefix).
//! - `app/Models/**/*.php` — class name + `$table` + `$fillable`.
//!   Relation methods are detected by their first-call (`hasMany`,
//!   `belongsTo`, etc.).
//! - `resources/views/**/*.blade.php` — paths only, mapped to component
//!   dot-notation (`resources/views/admin/users/index.blade.php`
//!   → `admin.users.index`).
//!
//! Limitations: dynamic patterns (`Route::resource`, route groups with
//! prefixes, runtime route generation) are best-effort — we report what's
//! statically discoverable.

#![forbid(unsafe_code)]

mod blades;
mod class_walk;
mod configs;
mod detect;
mod jobs;
mod models;
mod observers;
mod policies;
mod routes;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use blades::BladeView;
pub use configs::{ConfigFile, ConfigKey};
pub use detect::{LaravelProject, detect};
pub use jobs::Job;
pub use models::{Model, ModelRelation, RelationKind};
pub use observers::Observer;
pub use policies::Policy;
pub use routes::{Route, RouteMethod};

#[derive(Debug, Error)]
pub enum LaravelError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Laravel project: {0}")]
    NotLaravel(String),
    #[error("parse error in {file}: {message}")]
    Parse { file: String, message: String },
}

/// Aggregated snapshot of every static facet we can recover from a
/// Laravel project. Cheap to recompute (<300 ms on a typical app),
/// callers can re-run on cache miss instead of incremental updates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaravelSnapshot {
    pub routes: Vec<Route>,
    pub configs: Vec<ConfigFile>,
    pub models: Vec<Model>,
    pub blades: Vec<BladeView>,
    pub observers: Vec<Observer>,
    pub policies: Vec<Policy>,
    pub jobs: Vec<Job>,
}

/// Convenience: detect + parse all four facets in one shot. Returns
/// `Err(NotLaravel)` when the workspace doesn't look like Laravel; that's
/// the caller's signal to disable the Laravel tools entirely.
pub fn snapshot(workspace: &std::path::Path) -> Result<LaravelSnapshot, LaravelError> {
    let project = detect(workspace)?;
    Ok(LaravelSnapshot {
        routes: routes::parse_all(&project),
        configs: configs::parse_all(&project),
        models: models::parse_all(&project),
        blades: blades::list_all(&project),
        observers: observers::parse_all(&project),
        policies: policies::parse_all(&project),
        jobs: jobs::parse_all(&project),
    })
}
