//! Detection + lifecycle for the per-worktree Docker env contract.
//!
//! Convention: a worktree opts into isolated docker by shipping
//! `.oxyris/compose.yml` at its root. We don't dictate the contents — user
//! defines services, ports, volumes, and references the env vars we inject:
//!
//! - `OXYRIS_WORKTREE_ID`         (full uuid)
//! - `OXYRIS_WORKTREE_SHORT`      (first 8 chars, safe for container names)
//! - `OXYRIS_DOCKER_PROJECT`      (`oxyris_<short>`, used by docker compose -p)
//! - `OXYRIS_PORT_OFFSET`         (hash(id) mod 1000, for port mapping)
//! - `OXYRIS_COMPOSE_FILE`        (absolute path to .oxyris/compose.yml)
//!
//! All compose runs are tagged with `--label oxyris.managed=true` (set in the
//! generated commands) so cleanup on boot can identify orphan stacks left
//! behind by a previous Oxyris session that crashed.

use oxyris_core::{AggregateId, Environment};
use oxyris_ipc::ops::{FsStatArgs, op_name};
use serde::Serialize;

use crate::infra::agent_pool::{AgentError, AgentPool};

const TEMPLATE_RELATIVE: &str = ".oxyris/compose.yml";

#[derive(Debug, Clone, Serialize)]
pub struct EnvTemplate {
    pub has_template: bool,
    pub template_path: Option<String>,
    pub docker_project: String,
    pub port_offset: u16,
}

pub fn docker_project_name(worktree_id: AggregateId) -> String {
    format!("oxyris_{}", short_id(worktree_id))
}

pub fn short_id(worktree_id: AggregateId) -> String {
    worktree_id.to_string().chars().take(8).collect()
}

pub fn port_offset(worktree_id: AggregateId) -> u16 {
    let s = worktree_id.to_string();
    let mut hash: u32 = 0;
    for b in s.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u32);
    }
    (hash % 1000) as u16
}

pub fn env_vars(worktree_id: AggregateId, template_path: Option<&str>) -> Vec<(String, String)> {
    let mut out = vec![
        ("OXYRIS_WORKTREE_ID".into(), worktree_id.to_string()),
        ("OXYRIS_WORKTREE_SHORT".into(), short_id(worktree_id)),
        (
            "OXYRIS_DOCKER_PROJECT".into(),
            docker_project_name(worktree_id),
        ),
        (
            "OXYRIS_PORT_OFFSET".into(),
            port_offset(worktree_id).to_string(),
        ),
    ];
    if let Some(p) = template_path {
        out.push(("OXYRIS_COMPOSE_FILE".into(), p.to_owned()));
    }
    out
}

/// Look for `.oxyris/compose.yml` inside a worktree path. Routes through the
/// agent for WSL projects so we don't cross 9p.
pub async fn detect(
    env: &Environment,
    agent_pool: &AgentPool,
    worktree_id: AggregateId,
    worktree_path: &str,
) -> Result<EnvTemplate, AgentError> {
    let template_path = join(worktree_path, TEMPLATE_RELATIVE, env);
    let exists = match env {
        Environment::Windows => std::path::Path::new(&template_path).is_file(),
        Environment::Wsl { distro } => {
            let value = agent_pool
                .call(
                    distro,
                    op_name::FS_STAT,
                    serde_json::to_value(FsStatArgs {
                        path: template_path.clone(),
                    })?,
                )
                .await?;
            value
                .get("is_file")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        }
    };
    Ok(EnvTemplate {
        has_template: exists,
        template_path: if exists { Some(template_path) } else { None },
        docker_project: docker_project_name(worktree_id),
        port_offset: port_offset(worktree_id),
    })
}

fn join(base: &str, relative: &str, env: &Environment) -> String {
    let sep = match env {
        Environment::Windows => '\\',
        Environment::Wsl { .. } => '/',
    };
    let normalized = if matches!(env, Environment::Wsl { .. }) {
        relative.replace('\\', "/")
    } else {
        relative.replace('/', "\\")
    };
    if base.ends_with(sep) || base.ends_with('/') || base.ends_with('\\') {
        format!("{base}{normalized}")
    } else {
        format!("{base}{sep}{normalized}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_offset_stable() {
        let id =
            AggregateId(uuid::Uuid::parse_str("019dbbee-fb9f-7c40-b6de-d3f6cc3abd09").unwrap());
        assert_eq!(port_offset(id), port_offset(id));
        assert!(port_offset(id) < 1000);
    }

    #[test]
    fn docker_project_name_starts_with_prefix() {
        let id =
            AggregateId(uuid::Uuid::parse_str("019dbbee-fb9f-7c40-b6de-d3f6cc3abd09").unwrap());
        let name = docker_project_name(id);
        assert!(name.starts_with("oxyris_"));
        assert_eq!(name.len(), "oxyris_".len() + 8);
    }
}
