//! Authentication for execution-plane worker requests.

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use orbit_security::{bearer_token, hash_api_token};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct WorkerAuth {
    pub token_id: Uuid,
    pub org_id: Uuid,
    pub profile_id: Option<Uuid>,
    pub name: String,
    pub harnesses: Vec<String>,
}

impl FromRequestParts<AppState> for WorkerAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(
            parts
                .headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
        )
        .ok_or_else(|| ApiError::unauthorized("worker bearer token required"))?;
        if !token.starts_with("orbit_wk_") {
            return Err(ApiError::unauthorized("invalid worker token"));
        }
        let record = state
            .store
            .find_worker_token_by_hash(&hash_api_token(token))
            .await?
            .ok_or_else(|| ApiError::unauthorized("invalid or revoked worker token"))?;
        // Only when the stored time is stale: the store throttles the write too, but this
        // also saves spawning a task and a query on most requests (#447).
        let stale = record
            .last_used_at
            .is_none_or(|used| chrono::Utc::now() - used > chrono::Duration::seconds(60));
        if stale {
            let store = state.store.clone();
            let token_id = record.id;
            tokio::spawn(async move {
                let _ = store.touch_worker_token(token_id).await;
            });
        }
        Ok(Self {
            token_id: record.id,
            org_id: record.org_id,
            profile_id: record.profile_id,
            name: record.name,
            harnesses: record.harnesses,
        })
    }
}
