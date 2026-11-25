//! Issuing worker tokens for worker profiles (bootstrap, rotation and launches).

use crate::error::ApiError;
use crate::state::AppState;
use orbit_security::issue_worker_token;
use orbit_store::WorkerProfile;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedWorkerToken {
    pub token: String,
    pub token_id: Uuid,
    pub profile_id: Option<Uuid>,
    pub harnesses: Vec<String>,
    pub name: String,
}

pub(crate) async fn issue_profile_token(
    state: &AppState,
    org_id: Uuid,
    profile: &WorkerProfile,
    rotate: bool,
) -> Result<IssuedWorkerToken, ApiError> {
    if rotate {
        state
            .store
            .revoke_profile_worker_tokens(org_id, profile.id)
            .await?;
    }
    let issued = issue_worker_token();
    let record = state
        .store
        .create_worker_token(
            org_id,
            Some(profile.id),
            &format!("profile:{}", profile.name),
            &issued.prefix,
            &issued.hash,
            std::slice::from_ref(&profile.harness_kind),
        )
        .await?;
    Ok(IssuedWorkerToken {
        token: issued.token,
        token_id: record.id,
        profile_id: record.profile_id,
        harnesses: record.harnesses,
        name: record.name,
    })
}
