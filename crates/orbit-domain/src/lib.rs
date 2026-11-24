//! ORBIT domain model.
//!
//! This crate is pure: no I/O, no database, no HTTP. It owns the vocabulary shared by
//! the control plane, harness adapters and the generated API contract.

pub mod agent;
pub mod auth_secret;
pub mod autonomy;
pub mod docker_sandbox;
pub mod error;
pub mod execution;
pub mod harness_kind;
pub mod ids;
pub mod pagination;
pub mod schedule;
pub mod tool_policy;
pub mod worker_resources;
pub mod workflow;

pub use agent::*;
pub use auth_secret::{is_dns_label, AuthSecretPolicy, AUTH_SECRETS_ENV, DEFAULT_AUTH_SECRETS};
pub use autonomy::*;
pub use docker_sandbox::{DockerSandbox, HostGateway};
pub use error::*;
pub use execution::*;
pub use harness_kind::*;
pub use ids::*;
pub use pagination::*;
pub use schedule::{next_occurrence, parse_timezone, validate_schedule, ScheduleError};
pub use tool_policy::*;
pub use worker_resources::*;
pub use workflow::*;

/// Definition API version accepted by this build.
pub const AGENT_API_VERSION: &str = "orbit.io/v1";
pub const AGENT_KIND: &str = "Agent";

/// Canonical JSON hash used for agent version checksums.
pub fn canonical_checksum<T: serde::Serialize>(value: &T) -> String {
    let canonical = serde_json::to_vec(value).unwrap_or_default();
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    hex::encode(hasher.finalize())
}
