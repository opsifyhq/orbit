//! ORBIT control-plane HTTP API.
//!
//! The API is a thin, well-typed layer over `orbit-store` and the event bus. It never
//! executes agent tasks; executions are queued durably and dispatched by `orbit-worker`.

pub mod access;
pub mod audit;
pub mod auth;
pub mod background;
pub mod bootstrap;
pub mod config;
pub mod connectors;
pub mod dispatcher;
pub mod docs;
pub mod dto;
pub mod error;
pub mod extract;
pub mod handlers;
pub mod images;
pub mod launches;
pub mod metrics;
pub mod openapi;
pub mod output_delivery;
pub mod paging;
pub mod patch;
pub mod preflight;
pub mod rate_limit;
pub mod request_id;
pub mod routes;
pub mod smtp_output;
pub mod state;
pub mod supervisor;
pub mod team_outcomes;
pub mod team_runs;
pub mod team_workflow;
pub mod telemetry;
pub mod work_sources;
pub mod worker_auth;
pub mod worker_tokens;

pub use bootstrap::{initialize, Initialized};
pub use config::ApiConfig;
pub use routes::{build_metrics_router, build_router};
pub use state::{AppState, DevIdentity, MetricsAccess};
