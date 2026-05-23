//! Boot-time cleanup of orphan Oxyris-managed Docker resources.
//!
//! When the previous Oxyris session crashed mid-flight (or the user removed
//! a worktree externally), the docker stacks tagged `oxyris_<short>` stay
//! up. On startup we enumerate all projects, derive the live set of worktree
//! short ids, and for each environment ask docker which `oxyris_*` compose
//! projects exist. Anything orphaned (project name not in the live set) is
//! torn down: containers are force-removed, plus their volumes and
//! networks (matching by `com.docker.compose.project` label).
//!
//! All operations are best-effort and fully async — failures fall through to
//! tracing warnings, never block the boot path.

use std::collections::HashSet;
use std::process::Stdio;

use oxyris_core::Environment;
use oxyris_procutil::HideConsole;
use serde::Serialize;
use tokio::process::Command;

use crate::infra::env_template;
use crate::infra::projections::Projections;

#[derive(Debug, Default, Clone, Serialize)]
pub struct CleanupReport {
    pub orphan_projects: Vec<String>,
    pub containers_removed: u32,
    pub volumes_removed: u32,
    pub networks_removed: u32,
}

/// Walk every project's environment, find oxyris-tagged compose projects
/// whose worktree id no longer matches anything in the projection, and
/// tear them down. Spawn this as a background task on boot.
pub async fn prune_orphans_for_all(projections: &Projections) -> CleanupReport {
    let projects = match projections.list_projects() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "docker_cleanup: list_projects failed");
            return CleanupReport::default();
        }
    };

    // Build the live set of worktree short ids across all projects.
    let mut live_short_ids: HashSet<String> = HashSet::new();
    for p in &projects {
        let wts = match projections.list_worktrees(p.id, false) {
            Ok(w) => w,
            Err(_) => continue,
        };
        for wt in wts {
            live_short_ids.insert(env_template::short_id(wt.id));
        }
    }

    // Dedupe environments — every project tied to the same Windows host
    // shares one docker daemon, and per-distro WSL is equivalent across
    // projects in that distro.
    let mut envs: Vec<Environment> = Vec::new();
    for p in &projects {
        if !envs.iter().any(|e| same_env(e, &p.environment)) {
            envs.push(p.environment.clone());
        }
    }

    let mut report = CleanupReport::default();
    for env in &envs {
        match prune_orphans_in(env, &live_short_ids).await {
            Ok(part) => merge_report(&mut report, part),
            Err(e) => {
                // Daemon-not-running is expected when Docker Desktop is
                // shut down — demote to debug so the user's normal logs
                // stay quiet. Anything else still escalates.
                let msg = e.to_lowercase();
                if msg.contains("cannot find the file specified")
                    || msg.contains("daemon")
                    || msg.contains("dockerdesktop")
                    || msg.contains("not be found")
                {
                    tracing::debug!(error = %e, ?env, "docker_cleanup: daemon unavailable; skipping");
                } else {
                    tracing::warn!(error = %e, ?env, "docker_cleanup: prune failed");
                }
            }
        }
    }
    if !report.orphan_projects.is_empty() {
        tracing::info!(
            removed = report.containers_removed,
            volumes = report.volumes_removed,
            networks = report.networks_removed,
            projects = ?report.orphan_projects,
            "docker_cleanup: pruned orphan oxyris stacks"
        );
    }
    report
}

async fn prune_orphans_in(
    env: &Environment,
    live_short_ids: &HashSet<String>,
) -> Result<CleanupReport, String> {
    // List every container with a compose-project label, then keep only
    // those starting with `oxyris_`.
    let projects = list_oxyris_compose_projects(env).await?;
    let mut report = CleanupReport::default();
    for project_name in projects {
        let Some(short) = project_name.strip_prefix("oxyris_") else {
            continue;
        };
        if live_short_ids.contains(short) {
            continue;
        }
        report.orphan_projects.push(project_name.clone());
        match tear_down_project(env, &project_name).await {
            Ok((c, v, n)) => {
                report.containers_removed += c;
                report.volumes_removed += v;
                report.networks_removed += n;
            }
            Err(e) => {
                tracing::warn!(project = %project_name, error = %e, "docker_cleanup: tear-down failed");
            }
        }
    }
    Ok(report)
}

async fn list_oxyris_compose_projects(env: &Environment) -> Result<HashSet<String>, String> {
    // `docker compose ls` is the right tool for "what compose projects
    // exist on this host". `docker ps --format` previously tried to
    // template-index `.Labels`, which fails on docker engines where that
    // field is a comma-separated string rather than a map (Docker Desktop
    // on Windows in particular). The compose-ls API returns a stable
    // JSON list, no template gymnastics needed.
    let stdout = run_docker(env, &["compose", "ls", "-a", "--format", "json"]).await?;
    let raw = stdout.trim();
    if raw.is_empty() {
        return Ok(HashSet::new());
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        #[serde(default, rename = "Name")]
        name: String,
    }
    let entries: Vec<Entry> = serde_json::from_str(raw)
        .map_err(|e| format!("docker compose ls returned non-JSON output: {e} (got: {raw:?})"))?;
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            let n = e.name.trim();
            if n.starts_with("oxyris_") {
                Some(n.to_owned())
            } else {
                None
            }
        })
        .collect())
}

async fn tear_down_project(
    env: &Environment,
    project_name: &str,
) -> Result<(u32, u32, u32), String> {
    let label = format!("label=com.docker.compose.project={project_name}");

    // Containers — `rm -f` stops + removes in one call.
    let cids = run_docker(env, &["ps", "-aq", "--filter", &label]).await?;
    let cids: Vec<&str> = cids.split_whitespace().collect();
    let containers = cids.len() as u32;
    if !cids.is_empty() {
        let mut args = vec!["rm", "-f"];
        args.extend(cids.iter().copied());
        let _ = run_docker(env, &args).await;
    }

    // Volumes — only Oxyris-managed ones (compose tags them with the same
    // label). We never touch volumes outside our project name.
    let vids = run_docker(env, &["volume", "ls", "-q", "--filter", &label]).await?;
    let vids: Vec<&str> = vids.split_whitespace().collect();
    let volumes = vids.len() as u32;
    if !vids.is_empty() {
        let mut args = vec!["volume", "rm", "-f"];
        args.extend(vids.iter().copied());
        let _ = run_docker(env, &args).await;
    }

    // Networks — same pattern.
    let nids = run_docker(env, &["network", "ls", "-q", "--filter", &label]).await?;
    let nids: Vec<&str> = nids.split_whitespace().collect();
    let networks = nids.len() as u32;
    if !nids.is_empty() {
        let mut args = vec!["network", "rm"];
        args.extend(nids.iter().copied());
        let _ = run_docker(env, &args).await;
    }

    Ok((containers, volumes, networks))
}

async fn run_docker(env: &Environment, args: &[&str]) -> Result<String, String> {
    let mut cmd = match env {
        Environment::Windows => {
            let mut c = Command::new("docker");
            c.args(args);
            c
        }
        Environment::Wsl { distro } => {
            let mut c = Command::new("wsl.exe");
            c.args(["-d", distro, "--", "docker"]);
            c.args(args);
            c
        }
    };
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.hide_console();
    let out = cmd.output().await.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn same_env(a: &Environment, b: &Environment) -> bool {
    match (a, b) {
        (Environment::Windows, Environment::Windows) => true,
        (Environment::Wsl { distro: x }, Environment::Wsl { distro: y }) => x == y,
        _ => false,
    }
}

fn merge_report(into: &mut CleanupReport, part: CleanupReport) {
    into.orphan_projects.extend(part.orphan_projects);
    into.containers_removed += part.containers_removed;
    into.volumes_removed += part.volumes_removed;
    into.networks_removed += part.networks_removed;
}
