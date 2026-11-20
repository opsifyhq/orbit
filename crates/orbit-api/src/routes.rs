use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
use axum::middleware;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::handlers;
use crate::openapi::ApiDoc;
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let cors = build_cors(&state);

    let protected = Router::new()
        .route("/api/v1/docs", get(handlers::docs::index))
        .route("/api/v1/docs/{*path}", get(handlers::docs::get_doc))
        .route("/api/v1/me", get(handlers::account::me))
        .route(
            "/api/v1/organizations/current",
            get(handlers::account::current_organization),
        )
        .route("/api/v1/users", get(handlers::account::list_users))
        .route(
            "/api/v1/users/{user_id}",
            patch(handlers::account::update_user_role),
        )
        .route(
            "/api/v1/api-tokens",
            get(handlers::account::list_api_tokens).post(handlers::account::create_api_token),
        )
        .route(
            "/api/v1/api-tokens/{token_id}",
            delete(handlers::account::revoke_api_token),
        )
        .route(
            "/api/v1/audit-logs",
            get(handlers::account::list_audit_logs),
        )
        .route(
            "/api/v1/agents",
            get(handlers::agents::list_agents).post(handlers::agents::create_agent),
        )
        .route(
            "/api/v1/agents/bulk/archive",
            post(handlers::bulk_lifecycle::bulk_archive_agents),
        )
        .route(
            "/api/v1/agents/bulk/delete",
            post(handlers::bulk_lifecycle::bulk_delete_agents),
        )
        .route(
            "/api/v1/agents/{agent_id}",
            get(handlers::agents::get_agent)
                .patch(handlers::agents::update_agent)
                .delete(handlers::agents::delete_agent),
        )
        .route(
            "/api/v1/agents/{agent_id}/activate",
            post(handlers::agents::activate_agent),
        )
        .route(
            "/api/v1/agents/{agent_id}/pause",
            post(handlers::agents::pause_agent),
        )
        .route(
            "/api/v1/agents/{agent_id}/versions",
            get(handlers::agents::list_versions),
        )
        .route(
            "/api/v1/agents/{agent_id}/export",
            get(handlers::agents::export_agent),
        )
        .route(
            "/api/v1/agents/{agent_id}/runtime",
            get(handlers::runtime::get).post(handlers::runtime::start),
        )
        .route(
            "/api/v1/agents/{agent_id}/runtime/stop",
            post(handlers::runtime::stop),
        )
        .route(
            "/api/v1/agents/{agent_id}/executions",
            get(handlers::executions::list_agent_executions)
                .post(handlers::executions::execute_agent),
        )
        .route(
            "/api/v1/agent-instances",
            get(handlers::agents::list_agent_instances),
        )
        .route(
            "/api/v1/executions",
            get(handlers::executions::list_executions),
        )
        .route(
            "/api/v1/executions/{execution_id}",
            get(handlers::executions::get_execution),
        )
        .route(
            "/api/v1/executions/{execution_id}/events",
            get(handlers::executions::list_execution_events),
        )
        .route(
            "/api/v1/executions/{execution_id}/events/stream",
            get(handlers::executions::stream_execution_events),
        )
        .route(
            "/api/v1/executions/{execution_id}/cancel",
            post(handlers::executions::cancel_execution),
        )
        .route(
            "/api/v1/executions/{execution_id}/rerun",
            post(handlers::executions::rerun_execution),
        )
        .route(
            "/api/v1/agent-teams",
            get(handlers::teams::list_teams).post(handlers::teams::create_team),
        )
        .route(
            "/api/v1/agent-teams/{team_id}",
            get(handlers::teams::get_team)
                .patch(handlers::teams::update_team)
                .delete(handlers::teams::delete_team),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/runs",
            post(handlers::teams::create_run),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/queue",
            get(handlers::team_queue::list).post(handlers::team_queue::enqueue),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/queue/{item_id}/run",
            post(handlers::team_queue::run_item),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/queue/{item_id}/retry",
            post(handlers::team_queue::retry),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/queue/{item_id}/requeue",
            post(handlers::team_queue::requeue),
        )
        .route(
            "/api/v1/agent-teams/{team_id}/queue/{item_id}/cancel",
            post(handlers::team_queue::cancel),
        )
        .route("/api/v1/team-runs", get(handlers::teams::list_runs))
        .route("/api/v1/team-runs/{run_id}", get(handlers::teams::get_run))
        .route("/api/v1/harnesses", get(handlers::catalog::list_harnesses))
        .route("/api/v1/workers", get(handlers::catalog::list_workers))
        .route(
            "/api/v1/skills",
            get(handlers::skills::list_skills).post(handlers::skills::create_skill),
        )
        .route(
            "/api/v1/skills/bulk/delete",
            post(handlers::bulk_lifecycle::bulk_delete_skills),
        )
        .route(
            "/api/v1/skills/{skill_id}",
            get(handlers::skills::get_skill)
                .patch(handlers::skills::update_skill)
                .delete(handlers::skills::delete_skill),
        )
        .route(
            "/api/v1/worker-profiles",
            get(handlers::worker_profiles::list_profiles)
                .post(handlers::worker_profiles::create_profile),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}",
            get(handlers::worker_profiles::get_profile)
                .patch(handlers::worker_profiles::update_profile)
                .delete(handlers::worker_profiles::delete_profile),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/enable",
            post(handlers::worker_profiles::enable_profile),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/disable",
            post(handlers::worker_profiles::disable_profile),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/bootstrap",
            get(handlers::worker_profiles::bootstrap),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/token",
            post(handlers::worker_profiles::rotate_token),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/launch",
            post(handlers::worker_launches::launch_profile),
        )
        .route(
            "/api/v1/worker-launches",
            get(handlers::worker_launches::list_launches),
        )
        .route(
            "/api/v1/worker-launches/{launch_id}",
            get(handlers::worker_launches::get_launch)
                .delete(handlers::worker_launches::delete_launch),
        )
        .route(
            "/api/v1/worker-launches/{launch_id}/archive",
            post(handlers::worker_launches::archive_launch),
        )
        .route(
            "/api/v1/worker-launches/{launch_id}/stop",
            post(handlers::worker_launches::stop_launch),
        )
        .route(
            "/api/v1/worker-launches/{launch_id}/logs",
            get(handlers::worker_launches::launch_logs),
        )
        .route(
            "/api/v1/worker-tokens",
            get(handlers::worker_profiles::list_tokens)
                .post(handlers::worker_profiles::create_token),
        )
        .route(
            "/api/v1/worker-tokens/{token_id}",
            delete(handlers::worker_profiles::revoke_token),
        )
        .route(
            "/api/v1/providers",
            get(handlers::catalog::list_providers).post(handlers::catalog::create_provider),
        )
        .route(
            "/api/v1/providers/{provider_id}",
            patch(handlers::catalog::update_provider).delete(handlers::catalog::delete_provider),
        )
        .route(
            "/api/v1/providers/probe",
            post(handlers::catalog::probe_unsaved),
        )
        .route(
            "/api/v1/providers/{provider_id}/test",
            post(handlers::catalog::probe_provider),
        )
        .route(
            "/api/v1/connectors",
            get(handlers::catalog::list_connectors),
        )
        .route(
            "/api/v1/connections",
            get(handlers::catalog::list_connections).post(handlers::catalog::create_connection),
        )
        .route(
            "/api/v1/connections/{connection_id}",
            patch(handlers::catalog::update_connection)
                .delete(handlers::catalog::delete_connection),
        )
        .route(
            "/api/v1/triggers",
            get(handlers::triggers::list_triggers).post(handlers::triggers::create_trigger),
        )
        .route(
            "/api/v1/triggers/{trigger_id}",
            patch(handlers::triggers::update_trigger).delete(handlers::triggers::delete_trigger),
        )
        .route(
            "/api/v1/approvals",
            get(handlers::approvals::list_approvals),
        )
        .route(
            "/api/v1/approvals/{approval_id}/approve",
            post(handlers::approvals::approve),
        )
        .route(
            "/api/v1/approvals/{approval_id}/reject",
            post(handlers::approvals::reject),
        )
        .route(
            "/api/v1/dashboard/summary",
            get(handlers::dashboard::dashboard_summary),
        )
        .route(
            "/api/v1/settings/harness",
            get(handlers::settings::get_harness).put(handlers::settings::update_harness),
        )
        .route(
            "/api/v1/settings/approvals",
            get(handlers::settings::get_approvals).put(handlers::settings::update_approvals),
        )
        .route(
            "/api/v1/settings/access",
            get(handlers::settings::get_access).put(handlers::settings::update_access),
        )
        .route(
            "/api/v1/settings/oidc",
            get(handlers::settings::get_oidc).put(handlers::settings::update_oidc),
        )
        .route("/api/v1/license", get(handlers::license::overview))
        .route(
            "/api/v1/import/opencode",
            post(handlers::import::import_markdown),
        )
        .route(
            "/api/v1/import",
            post(handlers::import_portable::import_portable),
        )
        .route(
            "/api/v1/export/{resource}/{resource_id}",
            get(handlers::export::export_resource),
        )
        .route(
            "/api/v1/sharing/{resource}/bulk",
            axum::routing::patch(handlers::sharing::bulk_share),
        )
        .route(
            "/api/v1/sharing/{resource}/{resource_id}",
            get(handlers::sharing::get).patch(handlers::sharing::share),
        )
        .route(
            "/api/v1/teams",
            get(handlers::access_teams::list).post(handlers::access_teams::create),
        )
        .route(
            "/api/v1/teams/{team_id}",
            patch(handlers::access_teams::update).delete(handlers::access_teams::delete),
        )
        .route(
            "/api/v1/teams/{team_id}/members",
            get(handlers::access_teams::list_members).post(handlers::access_teams::add_member),
        )
        .route(
            "/api/v1/teams/{team_id}/members/{user_id}",
            delete(handlers::access_teams::remove_member),
        )
        .route(
            "/api/v1/outputs",
            get(handlers::outputs::list).post(handlers::outputs::create),
        )
        .route(
            "/api/v1/outputs/{output_id}",
            patch(handlers::outputs::update).delete(handlers::outputs::delete),
        )
        .route(
            "/api/v1/outputs/{output_id}/test",
            post(handlers::outputs::test_output),
        )
        .route(
            "/api/v1/env-mappings",
            get(handlers::env_mappings::list).post(handlers::env_mappings::create),
        )
        .route(
            "/api/v1/env-mappings/{mapping_id}",
            patch(handlers::env_mappings::update).delete(handlers::env_mappings::delete),
        )
        .route(
            "/api/v1/worker-profiles/{profile_id}/env-mappings",
            get(handlers::env_mappings::profile_mappings),
        )
        .route(
            "/api/v1/roles",
            get(handlers::roles::list).post(handlers::roles::create),
        )
        .route(
            "/api/v1/roles/{role_id}",
            patch(handlers::roles::update).delete(handlers::roles::delete),
        )
        .route(
            "/api/v1/tool-policies",
            get(handlers::tool_policies::list).post(handlers::tool_policies::create),
        )
        .route(
            "/api/v1/tool-policies/{policy_id}",
            patch(handlers::tool_policies::update).delete(handlers::tool_policies::delete),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    let worker = Router::new()
        .route("/api/v1/worker/session", get(handlers::worker_api::session))
        .route(
            "/api/v1/worker/heartbeat",
            post(handlers::worker_api::heartbeat),
        )
        .route(
            "/api/v1/worker/heartbeat/{worker_id}",
            delete(handlers::worker_api::offline),
        )
        .route("/api/v1/worker/claim", post(handlers::worker_api::claim))
        .route(
            "/api/v1/worker/executions/{execution_id}/context",
            get(handlers::worker_api::context),
        )
        .route(
            "/api/v1/worker/executions/{execution_id}/running",
            post(handlers::worker_api::running),
        )
        .route(
            "/api/v1/worker/executions/{execution_id}/events",
            post(handlers::worker_api::event),
        )
        .route(
            "/api/v1/worker/executions/{execution_id}/complete",
            post(handlers::worker_api::complete),
        )
        .route(
            "/api/v1/worker/memory/list",
            post(handlers::worker_api::list_memory),
        )
        .route(
            "/api/v1/worker/memory/store",
            post(handlers::worker_api::store_memory),
        )
        .route("/api/v1/worker/env", get(handlers::worker_api::resolve_env))
        .route(
            "/api/v1/worker/executions/{execution_id}/cancel",
            get(handlers::worker_api::cancel_requested),
        )
        .route(
            "/api/v1/worker/approvals",
            post(handlers::worker_api::create_approval),
        )
        .route(
            "/api/v1/worker/approvals/{approval_id}/wait",
            get(handlers::worker_api::await_approval),
        )
        .route("/api/v1/worker/audit", post(handlers::worker_api::audit))
        .route(
            "/api/v1/worker/connections/{connection_id}",
            get(handlers::worker_api::connection),
        );

    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/readyz", get(handlers::health::readyz))
        .route("/metrics", get(handlers::health::metrics))
        .route("/api/v1/meta", get(handlers::health::meta))
        .route("/api/v1/auth/oidc/config", get(handlers::auth_oidc::config))
        .route("/api/v1/auth/oidc/token", post(handlers::auth_oidc::token))
        .merge(SwaggerUi::new("/docs").url("/api/v1/openapi.json", ApiDoc::openapi()));

    Router::new()
        .merge(public)
        .merge(worker)
        .merge(protected)
        .fallback(crate::error::not_found_fallback)
        // Inside `instrument` so the request id is available to the envelope.
        .layer(middleware::from_fn(crate::error::envelope_rejections))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::request_id::instrument,
        ))
        .layer(DefaultBodyLimit::max(state.max_body_bytes))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

fn build_cors(state: &AppState) -> CorsLayer {
    let origins = &state.auth_cors_origins();
    if origins.iter().any(|origin| origin == "*") {
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(Any);
    }
    let parsed: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::IF_MATCH,
            header::HeaderName::from_static("idempotency-key"),
            header::HeaderName::from_static("last-event-id"),
            header::HeaderName::from_static("x-request-id"),
        ])
        .expose_headers([header::HeaderName::from_static("x-request-id")])
}
