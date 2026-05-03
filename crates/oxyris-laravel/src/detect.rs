//! Detect whether a workspace is a Laravel project. We require both:
//! 1. an `artisan` script at the workspace root, and
//! 2. a `composer.json` whose `require` (or `require-dev`) lists
//!    `laravel/framework`.
//!
//! Either signal alone is too noisy: `artisan` is occasionally present
//! in non-Laravel projects (template clones), and many PHP packages list
//! `laravel/framework` only as a peer dep. Both together is a strong
//! match.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::LaravelError;

/// Resolved Laravel project paths. Holds the workspace root + the
/// canonical Laravel directory layout (which we assume; non-standard
/// projects fall back to "doesn't exist" lookups in each parser).
#[derive(Debug, Clone)]
pub struct LaravelProject {
    pub root: PathBuf,
}

impl LaravelProject {
    pub fn routes_dir(&self) -> PathBuf {
        self.root.join("routes")
    }
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("app").join("Models")
    }
    pub fn views_dir(&self) -> PathBuf {
        self.root.join("resources").join("views")
    }
    pub fn observers_dir(&self) -> PathBuf {
        self.root.join("app").join("Observers")
    }
    pub fn policies_dir(&self) -> PathBuf {
        self.root.join("app").join("Policies")
    }
    pub fn jobs_dir(&self) -> PathBuf {
        self.root.join("app").join("Jobs")
    }
}

pub fn detect(workspace: &Path) -> Result<LaravelProject, LaravelError> {
    if !workspace.join("artisan").exists() {
        return Err(LaravelError::NotLaravel(format!(
            "no `artisan` at {}",
            workspace.display()
        )));
    }
    let composer_path = workspace.join("composer.json");
    if !composer_path.exists() {
        return Err(LaravelError::NotLaravel(
            "no composer.json at workspace root".into(),
        ));
    }
    let raw = std::fs::read_to_string(&composer_path)?;
    let json: Value = serde_json::from_str(&raw).map_err(|e| LaravelError::Parse {
        file: composer_path.to_string_lossy().into_owned(),
        message: e.to_string(),
    })?;
    if !has_laravel_dep(&json) {
        return Err(LaravelError::NotLaravel(
            "composer.json doesn't require laravel/framework".into(),
        ));
    }
    Ok(LaravelProject {
        root: workspace.to_owned(),
    })
}

fn has_laravel_dep(composer: &Value) -> bool {
    for section in ["require", "require-dev"] {
        let Some(deps) = composer.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        if deps.contains_key("laravel/framework") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_when_both_signals_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"require":{"laravel/framework":"^11.0"}}"#,
        )
        .unwrap();
        let project = detect(dir.path()).expect("should detect");
        assert_eq!(project.root, dir.path());
    }

    #[test]
    fn rejects_when_only_artisan_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(dir.path().join("composer.json"), r#"{"name":"foo/bar"}"#).unwrap();
        assert!(matches!(
            detect(dir.path()).unwrap_err(),
            LaravelError::NotLaravel(_)
        ));
    }

    #[test]
    fn rejects_when_no_artisan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"require":{"laravel/framework":"^11.0"}}"#,
        )
        .unwrap();
        assert!(matches!(
            detect(dir.path()).unwrap_err(),
            LaravelError::NotLaravel(_)
        ));
    }

    #[test]
    fn accepts_require_dev() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "x").unwrap();
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"require-dev":{"laravel/framework":"^11.0"}}"#,
        )
        .unwrap();
        assert!(detect(dir.path()).is_ok());
    }
}
