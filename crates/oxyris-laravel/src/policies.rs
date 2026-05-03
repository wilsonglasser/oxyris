//! Walk `app/Policies/**/*.php`. Each class is reported with the
//! authorization abilities it declares (every public method except
//! magic/`before`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::class_walk;
use crate::detect::LaravelProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub class: String,
    pub file: String,
    pub line: u32,
    /// Inferred from class suffix: `PostPolicy` → `Post`. None when the
    /// class doesn't follow the convention.
    pub model: Option<String>,
    /// Method names that look like authorization abilities. Magic
    /// methods (`__construct`, etc.) and the policy-wide `before` hook
    /// are excluded.
    pub abilities: Vec<String>,
}

pub fn parse_all(project: &LaravelProject) -> Vec<Policy> {
    let dir = project.policies_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    class_walk::walk_php_files(&dir, &mut |path: &Path| {
        class_walk::for_each_class(path, |class, line, node, bytes| {
            let methods = class_walk::class_method_names(node, bytes);
            let abilities: Vec<String> = methods
                .into_iter()
                .filter(|m| !m.starts_with("__") && m != "before")
                .collect();
            let model = class.strip_suffix("Policy").map(str::to_owned);
            out.push(Policy {
                class: class.to_owned(),
                file: path.to_string_lossy().into_owned(),
                line,
                model,
                abilities,
            });
        });
    });
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_policy_abilities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app/Policies/PostPolicy.php");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"<?php
namespace App\Policies;
class PostPolicy {
    public function __construct() {}
    public function before($user, $ability) {}
    public function viewAny($user) {}
    public function view($user, $post) {}
    public function update($user, $post) {}
    public function delete($user, $post) {}
}
"#,
        )
        .unwrap();
        let policies = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].class, "PostPolicy");
        assert_eq!(policies[0].model.as_deref(), Some("Post"));
        assert_eq!(
            policies[0].abilities,
            vec!["viewAny", "view", "update", "delete"]
        );
    }
}
