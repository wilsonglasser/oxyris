//! Infrastructure — the non-pure edge of the app: storage, process spawns,
//! path translation, filesystem walks, git. Anything that touches the outside
//! world lives here.

pub mod agent_pool;
pub mod checkpoint;
pub mod docker_cleanup;
pub mod dotenv_render;
pub mod env_template;
pub mod environments;
pub mod event_store;
pub mod git;
pub mod indexing;
pub mod mcp;
pub mod observability;
pub mod path_translator;
pub mod projections;
pub mod provider_discovery;
pub mod pty;
pub mod session_supervisor;
