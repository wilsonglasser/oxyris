//! Walk `app/Jobs/**/*.php`. Surfaces class name, whether it's queued
//! (`implements ShouldQueue`), and the `$queue` literal when statically
//! set. Dynamic queues set via `onQueue('x')` aren't recovered.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::class_walk;
use crate::detect::LaravelProject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub class: String,
    pub file: String,
    pub line: u32,
    /// True when the class declares `implements ShouldQueue` (anywhere
    /// in the namespace path).
    pub queueable: bool,
    /// Static `protected $queue = '...'` value. None when not declared
    /// or set dynamically.
    pub queue: Option<String>,
}

pub fn parse_all(project: &LaravelProject) -> Vec<Job> {
    let dir = project.jobs_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    class_walk::walk_php_files(&dir, &mut |path: &Path| {
        class_walk::for_each_class(path, |class, line, node, bytes| {
            let queueable = class_walk::class_implements(node, bytes, "ShouldQueue");
            let queue = class_walk::class_string_property(node, bytes, "queue");
            out.push(Job {
                class: class.to_owned(),
                file: path.to_string_lossy().into_owned(),
                line,
                queueable,
                queue,
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
    fn lists_queueable_job_with_queue_property() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app/Jobs/SendEmail.php");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"<?php
namespace App\Jobs;
use Illuminate\Contracts\Queue\ShouldQueue;
class SendEmail implements ShouldQueue {
    public string $queue = 'high';
    public function handle() {}
}
"#,
        )
        .unwrap();
        let jobs = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].class, "SendEmail");
        assert!(jobs[0].queueable);
        assert_eq!(jobs[0].queue.as_deref(), Some("high"));
    }

    #[test]
    fn lists_sync_job_without_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app/Jobs/SyncWork.php");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"<?php
namespace App\Jobs;
class SyncWork {
    public function handle() {}
}
"#,
        )
        .unwrap();
        let jobs = parse_all(&LaravelProject {
            root: dir.path().to_owned(),
        });
        assert_eq!(jobs.len(), 1);
        assert!(!jobs[0].queueable);
        assert!(jobs[0].queue.is_none());
    }
}
