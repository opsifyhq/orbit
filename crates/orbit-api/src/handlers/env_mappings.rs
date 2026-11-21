//! Reusable environment mappings shared across worker profiles.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use uuid::Uuid;

use orbit_domain::ValidationIssue;
use orbit_security::{AuthContext, Permission};
use orbit_store::EnvMapping;

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvMappingRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mappings: serde_json::Value,
}

fn validated(body: &EnvMappingRequest) -> Result<(String, String, serde_json::Value), ApiError> {
    let name = body.name.trim().to_string();
    let mut issues = Vec::new();
    if name.is_empty() {
        issues.push(ValidationIssue::new("name", "name is required"));
    }
    let array = body.mappings.as_array().cloned().unwrap_or_default();
    if array.is_empty() {
        issues.push(ValidationIssue::new(
            "mappings",
            "at least one mapping is required",
        ));
    }
    for (index, item) in array.iter().enumerate() {
        // References only: an extra key (say `value`) would be stored and returned.
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !orbit_store::INLINE_ENV_MAPPING_KEYS.contains(&key.as_str()) {
                    issues.push(ValidationIssue::new(
                        "mappings",
                        format!(
                            "mapping {index}: unknown field '{key}'; mappings reference a connection or secret and cannot hold a value"
                        ),
                    ));
                }
            }
        }
        let env_var = item.get("envVar").and_then(|v| v.as_str()).unwrap_or("");
        if env_var.trim().is_empty() {
            issues.push(ValidationIssue::new(
                "mappings",
                format!("mapping {index} is missing envVar"),
            ));
        }
        let has_connection = item.get("connectionId").and_then(|v| v.as_str()).is_some();
        let has_secret = item.get("secretKey").and_then(|v| v.as_str()).is_some()
            || item.get("secretRef").and_then(|v| v.as_str()).is_some();
        if !has_connection && !has_secret {
            issues.push(ValidationIssue::new(
                "mappings",
                format!("mapping {index} needs connectionId or secretKey"),
            ));
        }
    }
    if !issues.is_empty() {
        return Err(ApiError::validation(issues));
    }
    Ok((
        name,
        body.description.trim().to_string(),
        body.mappings.clone(),
    ))
}

/// Every secret and connection a mapping routes into worker env must belong to the
/// caller's org and be usable by the caller. Foreign and unknown references are reported
/// identically so the response never confirms that a secret exists elsewhere, and
/// nothing about secret values is ever included.
async fn validate_references(
    state: &AppState,
    auth: &AuthContext,
    mappings: &serde_json::Value,
) -> Result<(), ApiError> {
    let access = crate::access::AccessScope::load(state, auth).await?;
    let mut issues = Vec::new();
    for (index, item) in mappings
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        if let Some(value) = item.get("connectionId").and_then(|v| v.as_str()) {
            let usable = match Uuid::parse_str(value) {
                Ok(id) => match state.store.get_connection(auth.org_id, id).await {
                    Ok(connection) => access
                        .require_use(
                            "connection",
                            connection.owner_user_id,
                            connection.owner_team_id,
                            &connection.visibility,
                        )
                        .is_ok(),
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if !usable {
                issues.push(ValidationIssue::new(
                    "mappings",
                    format!("mapping {index}: connection not found or not usable"),
                ));
            }
        }
        for field in ["secretRef", "secretKey"] {
            let Some(reference) = item.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            let usable = match state.store.secret_owner(auth.org_id, reference).await? {
                Some(owner) if owner.kind != "output" => access
                    .require_use(
                        "secret",
                        owner.owner_user_id,
                        owner.owner_team_id,
                        &owner.visibility,
                    )
                    .is_ok(),
                _ => false,
            };
            if !usable {
                issues.push(ValidationIssue::new(
                    "mappings",
                    format!(
                        "mapping {index}: {field} does not reference a secret you can use in this organization"
                    ),
                ));
            }
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
    }
}

#[utoipa::path(get, path = "/api/v1/env-mappings", tag = "env-mappings", responses((status = 200, body = [EnvMapping])))]
pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<EnvMapping>>, ApiError> {
    auth.require(Permission::SettingsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    Ok(Json(
        state
            .store
            .list_env_mappings(auth.org_id, Some(&access.store_access()))
            .await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/env-mappings", tag = "env-mappings", request_body = EnvMappingRequest, responses((status = 201, body = EnvMapping)))]
pub async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<EnvMappingRequest>,
) -> Result<(StatusCode, Json<EnvMapping>), ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let (name, description, mappings) = validated(&body)?;
    validate_references(&state, &auth, &mappings).await?;
    let mapping = state
        .store
        .create_env_mapping(auth.org_id, &name, &description, &mappings)
        .await?;
    state
        .store
        .assign_resource_owner(auth.org_id, "env_mappings", mapping.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "env_mapping.created",
        Some("env_mapping"),
        Some(mapping.id),
        serde_json::json!({ "name": mapping.name }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(mapping)))
}

#[utoipa::path(patch, path = "/api/v1/env-mappings/{mapping_id}", tag = "env-mappings", request_body = EnvMappingRequest, responses((status = 200, body = EnvMapping)))]
pub async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(mapping_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<EnvMappingRequest>,
) -> Result<Json<EnvMapping>, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let existing = state.store.get_env_mapping(auth.org_id, mapping_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "environment mapping",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    let (name, description, mappings) = validated(&body)?;
    validate_references(&state, &auth, &mappings).await?;
    let mapping = state
        .store
        .update_env_mapping(auth.org_id, mapping_id, &name, &description, &mappings)
        .await?;
    audit::record(
        &state,
        &auth,
        "env_mapping.updated",
        Some("env_mapping"),
        Some(mapping.id),
        serde_json::json!({ "name": mapping.name }),
    )
    .await;
    Ok(Json(mapping))
}

#[utoipa::path(delete, path = "/api/v1/env-mappings/{mapping_id}", tag = "env-mappings", responses((status = 204)))]
pub async fn delete(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(mapping_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::SettingsWrite)?;
    let existing = state.store.get_env_mapping(auth.org_id, mapping_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "environment mapping",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    state
        .store
        .delete_env_mapping(auth.org_id, mapping_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "env_mapping.deleted",
        Some("env_mapping"),
        Some(mapping_id),
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/worker-profiles/{profile_id}/env-mappings", tag = "workers", responses((status = 200, body = [EnvMapping])))]
pub async fn profile_mappings(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(profile_id): Path<Uuid>,
) -> Result<Json<Vec<EnvMapping>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let profile = state
        .store
        .get_worker_profile(auth.org_id, profile_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "worker profile",
        profile.owner_user_id,
        profile.owner_team_id,
        &profile.visibility,
    )?;
    let mut mappings = state
        .store
        .list_worker_profile_env_mappings(profile_id)
        .await?;
    mappings.retain(|mapping| {
        access.can_view(
            mapping.owner_user_id,
            mapping.owner_team_id,
            &mapping.visibility,
        )
    });
    Ok(Json(mappings))
}

/// Every mapping attached to a worker profile must be visible and usable by the caller.
/// Enforced before the profile write so an access change cannot expose a secret through
/// an existing attachment.
pub(crate) async fn validate_profile_mapping_ids(
    state: &AppState,
    auth: &AuthContext,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    let access = crate::access::AccessScope::load(state, auth).await?;
    for id in ids {
        let mapping = state.store.get_env_mapping(auth.org_id, *id).await?;
        access.require_use(
            "environment mapping",
            mapping.owner_user_id,
            mapping.owner_team_id,
            &mapping.visibility,
        )?;
    }
    Ok(())
}
