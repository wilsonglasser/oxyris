//! Walk `app/Observers/**/*.php`. Each class is reported with the
//! Eloquent event hooks it implements (creating/created/updating/...).
//! No `boot()`-style registration discovery — observers can be registered
//! anywhere; the class list itself is the useful signal.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::class_walk;
use crate::detect::LaravelProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observer {
    pub class: String,
    pub file: String,
    pub line: u32,
    /// Inferred from class suffix: `UserObserver` → `User`. None when
    /// the class doesn't follow the convention.
    pub model: Option<String>,
    /// Eloquent event hooks the observer implements (subset of
    /// `creating, created, updating, updated, saving, saved, deleting,
    /// deleted, restoring, restored, forceDeleted, retrieved`).
    pub events: Vec<String>,
}

const ELOQUENT_EVENTS: &[&str] = &[
    "retrieved",
    "creating",
    "created",
    "updating",
    "updated",
    "saving",
    "saved",
    "deleting",
    "deleted",
    "restoring",
    "restored",
    "forceDeleted",
];

pub fn parse_all(project: &LaravelProject) -> Vec<Observer> {
    let dir = project.observers_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    class_walk::walk_php_files(&dir, &mut |path: &Path| {
        class_walk::for_each_class(path, |class, line, node, bytes| {
            let methods = class_walk::class_method_names(node, bytes);
            let events: Vec<String> = methods
                .into_iter()
                .filter(|m| ELOQUENT_EVENTS.contains(&m.as_str()))
                .collect();
            let model = class.strip_suffix("Observer").map(str::to_owned);
            out.push(Observer {
                class: class.to_owned(),
                file: path.to_string_lossy().into_owned(),
                line,
                model,
                events,
            });
        });
    });
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn lists_observers_with_inferred_model() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "app/Observers/UserObserver.php",
            r#"<?php
namespace App\Observers;
class UserObserver {
    public function created($user) {}
    public function updated($user) {}
    public function deleting($user) {}
    public function helperMethod() {}
}
"#,
        );
        let observers = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(observers.len(), 1);
        assert_eq!(observers[0].class, "UserObserver");
        assert_eq!(observers[0].model.as_deref(), Some("User"));
        assert_eq!(observers[0].events, vec!["created", "updated", "deleting"]);
    }
}
