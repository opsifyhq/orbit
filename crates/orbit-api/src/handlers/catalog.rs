use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use orbit_contracts::{validate_provider_config, HarnessDescriptor, ProviderKind};
use orbit_domain::HarnessKind;
use orbit_security::{new_secret_ref, AuthContext, Permission, SecretValue};

use crate::audit;
use crate::dto::{
    parse_provider_kind, ConnectionView, ConnectorView, CreateConnectionRequest,
    CreateProviderRequest, HarnessHealthView, HarnessView, ProbeProviderRequest,
    ProviderProbeResponse, ProviderView, UpdateConnectionRequest, UpdateProviderRequest,
};
use crate::error::ApiError;
use crate::state::AppState;

// ------------------------------------------------------------------- harnesses

#[utoipa::path(get, path = "/api/v1/harnesses", tag = "harnesses", responses((status = 200, body = [HarnessView])))]
pub async fn list_harnesses(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<HarnessView>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let rows = state.store.list_harnesses(auth.org_id).await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let kind = row
                .kind
                .parse::<HarnessKind>()
                .unwrap_or(HarnessKind::Generic);
            let descriptor = state
                .registry
                .get(kind)
                .map(|adapter| adapter.descriptor())
                .unwrap_or_else(|| fallback_descriptor(kind, &row.name, &row.description));
            HarnessView {
                id: row.id,
                kind,
                name: row.name,
                description: row.description,
                enabled: row.enabled,
                implementation_status: row.implementation_status,
                capabilities: row.capabilities,
                descriptor,
                config: row.config,
                health: HarnessHealthView {
                    status: "unknown".into(),
                    message: Some(
                        "Harness health is reported by workers (Phase 1). Run `orbit-worker` and \
                         check /readyz."
                            .into(),
                    ),
                    reported_at: None,
                },
                updated_at: row.updated_at,
            }
        })
        .collect();
    Ok(Json(items))
}

fn fallback_descriptor(kind: HarnessKind, name: &str, description: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        kind,
        name: name.to_string(),
        description: description.to_string(),
        capabilities: Default::default(),
        implementation_status: kind.implementation_status(),
        config_schema: serde_json::json!({ "type": "object" }),
        docs_url: None,
    }
}

// ------------------------------------------------------------------- providers

#[utoipa::path(get, path = "/api/v1/providers", tag = "providers", responses((status = 200, body = [ProviderView])))]
pub async fn list_providers(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<ProviderView>>, ApiError> {
    auth.require(Permission::ProvidersRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let providers = state
        .store
        .list_providers(auth.org_id, Some(&access.store_access()))
        .await?
        .into_iter()
        .map(ProviderView::from_record)
        .collect();
    Ok(Json(providers))
}

#[utoipa::path(post, path = "/api/v1/providers", tag = "providers", request_body = CreateProviderRequest, responses((status = 201, body = ProviderView)))]
pub async fn create_provider(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateProviderRequest>,
) -> Result<(StatusCode, Json<ProviderView>), ApiError> {
    auth.require(Permission::ProvidersWrite)?;
    let kind = parse_provider_kind(&body.kind).map_err(ApiError::bad_request)?;
    if body.name.trim().is_empty() {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("name", "provider name is required"),
        ]));
    }
    let mut config = body.config.clone();
    sanitize_provider_config(&mut config);
    let report = validate_provider_config(kind, &config);
    if !report.is_valid() {
        return Err(ApiError::validation(report.issues));
    }

    let secret_ref = if let Some(credential) = &body.credential {
        if credential.trim().is_empty() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("credential", "credential must not be empty"),
            ]));
        }
        let reference = new_secret_ref("provider");
        state
            .store
            .put_secret(
                Some(auth.org_id),
                &reference,
                &SecretValue::new(credential.trim()),
                &format!("provider:{}", body.name.trim()),
            )
            .await?;
        Some(reference)
    } else {
        None
    };

    let provider = match state
        .store
        .create_provider(
            auth.org_id,
            body.name.trim(),
            kind.as_str(),
            &config,
            secret_ref.as_deref(),
        )
        .await
    {
        Ok(provider) => provider,
        Err(err) => {
            if let Some(reference) = &secret_ref {
                let _ = state.store.delete_secret(reference).await;
            }
            return Err(err.into());
        }
    };

    state
        .store
        .assign_resource_owner(auth.org_id, "providers", provider.id, auth.user_id)
        .await?;
    audit::record(
        &state,
        &auth,
        "provider.created",
        Some("provider"),
        Some(provider.id),
        serde_json::json!({ "name": provider.name, "kind": provider.kind, "hasCredential": secret_ref.is_some() }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ProviderView::from_record(provider)),
    ))
}

#[utoipa::path(patch, path = "/api/v1/providers/{provider_id}", tag = "providers", request_body = UpdateProviderRequest, responses((status = 200, body = ProviderView)))]
pub async fn update_provider(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(provider_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateProviderRequest>,
) -> Result<Json<ProviderView>, ApiError> {
    auth.require(Permission::ProvidersWrite)?;
    let existing = state.store.get_provider(auth.org_id, provider_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "provider",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    let kind = ProviderKind::parse(&existing.kind)
        .ok_or_else(|| ApiError::internal("provider has an unknown kind"))?;

    let config = match body.config {
        Some(mut config) => {
            sanitize_provider_config(&mut config);
            let report = validate_provider_config(kind, &config);
            if !report.is_valid() {
                return Err(ApiError::validation(report.issues));
            }
            Some(config)
        }
        None => None,
    };

    // A stored credential must never follow an edited endpoint: if the base URL changes,
    // the caller has to supply the credential for the new endpoint in the same request.
    if let (Some(new_config), Some(_)) = (&config, &existing.secret_ref) {
        if body.credential.is_none()
            && normalized_base_url(new_config) != normalized_base_url(&existing.config)
        {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new(
                    "credential",
                    "re-enter the API key when changing the base URL",
                ),
            ]));
        }
    }

    let secret_ref = if let Some(credential) = &body.credential {
        if credential.trim().is_empty() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("credential", "credential must not be empty"),
            ]));
        }
        let reference = existing
            .secret_ref
            .clone()
            .unwrap_or_else(|| new_secret_ref("provider"));
        state
            .store
            .put_secret(
                Some(auth.org_id),
                &reference,
                &SecretValue::new(credential.trim()),
                &format!("provider:{}", existing.name),
            )
            .await?;
        Some(reference)
    } else {
        None
    };

    let provider = state
        .store
        .update_provider(
            auth.org_id,
            provider_id,
            body.name.as_deref().map(str::trim),
            config.as_ref(),
            secret_ref.as_deref(),
            body.enabled,
        )
        .await?;
    audit::record(
        &state,
        &auth,
        "provider.updated",
        Some("provider"),
        Some(provider.id),
        serde_json::json!({ "credentialRotated": secret_ref.is_some() }),
    )
    .await;
    Ok(Json(ProviderView::from_record(provider)))
}

#[utoipa::path(delete, path = "/api/v1/providers/{provider_id}", tag = "providers", responses((status = 204)))]
pub async fn delete_provider(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(provider_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::ProvidersWrite)?;
    let provider = state.store.get_provider(auth.org_id, provider_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "provider",
        provider.owner_user_id,
        provider.owner_team_id,
        &provider.visibility,
    )?;
    state
        .store
        .delete_provider(auth.org_id, provider_id)
        .await?;
    if let Some(reference) = &provider.secret_ref {
        if let Err(err) = state.store.delete_secret(reference).await {
            tracing::warn!(error = %err, "failed to delete provider secret");
        }
    }
    audit::record(
        &state,
        &auth,
        "provider.deleted",
        Some("provider"),
        Some(provider_id),
        serde_json::json!({ "name": provider.name }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Probe an OpenAI-compatible endpoint by listing models.
#[utoipa::path(post, path = "/api/v1/providers/{provider_id}/test", tag = "providers", responses((status = 200, body = ProviderProbeResponse)))]
pub async fn probe_provider(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<ProviderProbeResponse>, ApiError> {
    // Probing sends the stored credential to the provider endpoint and persists the
    // discovered models, so it needs the same rights as editing the provider.
    auth.require(Permission::ProvidersWrite)?;
    let provider = state.store.get_provider(auth.org_id, provider_id).await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "provider",
        provider.owner_user_id,
        provider.owner_team_id,
        &provider.visibility,
    )?;
    let kind = ProviderKind::parse(&provider.kind)
        .ok_or_else(|| ApiError::internal("provider has an unknown kind"))?;
    if kind == ProviderKind::HarnessManaged {
        return Err(ApiError::unsupported(
            "harness-managed providers are authenticated and probed by their harness",
        ));
    }
    let credential = match &provider.secret_ref {
        Some(reference) => state
            .store
            .get_secret_value(reference)
            .await?
            .map(|value| value.expose().to_string()),
        None => None,
    };
    let result = probe_endpoint(kind, &provider.config, credential.as_deref()).await;

    // Persist discovered models so agent/worker selectors can use them without a manual
    // catalog edit. Keep any existing settings/catalog merge semantics.
    if result.ok && !result.models.is_empty() {
        let mut config = provider.config.clone();
        if !config.is_object() {
            config = serde_json::json!({});
        }
        if !config
            .get("settings")
            .is_some_and(|value| value.is_object())
        {
            config["settings"] = serde_json::json!({});
        }
        config["settings"]["models"] = serde_json::json!(result.models);
        if let Err(error) = state
            .store
            .update_provider(auth.org_id, provider_id, None, Some(&config), None, None)
            .await
        {
            tracing::warn!(%error, "failed to persist probed provider models");
        }
    }
    Ok(Json(result))
}

/// Probe an unsaved provider config (create dialog).
#[utoipa::path(post, path = "/api/v1/providers/probe", tag = "providers", request_body = ProbeProviderRequest, responses((status = 200, body = ProviderProbeResponse)))]
pub async fn probe_unsaved(
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<ProbeProviderRequest>,
) -> Result<Json<ProviderProbeResponse>, ApiError> {
    auth.require(Permission::ProvidersWrite)?;
    let kind = ProviderKind::parse(&body.kind)
        .ok_or_else(|| ApiError::bad_request("unknown provider kind"))?;
    if kind == ProviderKind::HarnessManaged {
        return Err(ApiError::unsupported(
            "harness-managed providers are authenticated and probed by their harness",
        ));
    }
    let mut config = body.config;
    sanitize_provider_config(&mut config);
    let report = validate_provider_config(kind, &config);
    if !report.is_valid() {
        return Err(ApiError::validation(report.issues));
    }
    // Never persist or log the credential used for this probe.
    Ok(Json(
        probe_endpoint(kind, &config, body.credential.as_deref()).await,
    ))
}

async fn probe_endpoint(
    kind: ProviderKind,
    config: &serde_json::Value,
    credential: Option<&str>,
) -> ProviderProbeResponse {
    let base_url = match config.get("baseUrl").and_then(|v| v.as_str()) {
        Some(value) if !value.is_empty() => value,
        _ if kind == ProviderKind::Anthropic => "https://api.anthropic.com/v1",
        _ => "",
    };
    if base_url.is_empty() {
        return ProviderProbeResponse {
            ok: false,
            latency_ms: 0,
            models: vec![],
            error: Some("provider config is missing baseUrl".into()),
        };
    }
    let models_endpoint = config
        .pointer("/compat/modelsEndpoint")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !models_endpoint {
        return ProviderProbeResponse {
            ok: false,
            latency_ms: 0,
            models: vec![],
            error: Some("provider declares no /models endpoint".into()),
        };
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    if let Err(error) =
        orbit_security::check_outbound_url(&url, &orbit_security::OutboundPolicy::from_env()).await
    {
        return ProviderProbeResponse {
            ok: false,
            latency_ms: 0,
            models: vec![],
            error: Some(format!("endpoint refused ({})", error.category())),
        };
    }
    let client = orbit_security::guarded_client(std::time::Duration::from_secs(10));
    let mut request = client.get(&url);
    if let Some(credential) = credential {
        request = if kind == ProviderKind::Anthropic {
            request
                .header("x-api-key", credential)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(credential)
        };
    }
    for (name, value) in config
        .get("headers")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        if let Some(value) = value.as_str() {
            request = request.header(name, value);
        }
    }

    let started = std::time::Instant::now();
    let result = request.send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(response) if response.status().is_success() => {
            let models: Vec<String> = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|value| {
                    value.get("data").and_then(|d| d.as_array()).map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                item.get("id")
                                    .and_then(|id| id.as_str())
                                    .map(str::to_string)
                            })
                            .take(200)
                            .collect()
                    })
                })
                .unwrap_or_default();
            ProviderProbeResponse {
                ok: true,
                latency_ms,
                models,
                error: None,
            }
        }
        Ok(response) => ProviderProbeResponse {
            ok: false,
            latency_ms,
            models: vec![],
            error: Some(format!(
                "endpoint returned status {}",
                response.status().as_u16()
            )),
        },
        Err(err) => ProviderProbeResponse {
            ok: false,
            latency_ms,
            models: vec![],
            error: Some(orbit_security::describe_request_error(&err)),
        },
    }
}

fn normalized_base_url(config: &serde_json::Value) -> String {
    config
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn sanitize_provider_config(config: &mut serde_json::Value) {
    if let Some(object) = config.as_object_mut() {
        // Credentials are never accepted inline; strip accidental secret_ref echoes.
        object.remove("secretRef");
        object.remove("apiKey");
        object.remove("credential");
    }
}

// ------------------------------------------------------------------ connectors

#[utoipa::path(get, path = "/api/v1/connectors", tag = "connectors", responses((status = 200, body = [ConnectorView])))]
pub async fn list_connectors(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<ConnectorView>>, ApiError> {
    auth.require(Permission::ConnectionsRead)?;
    let connectors = state.store.list_connectors(auth.org_id).await?;
    Ok(Json(
        connectors.into_iter().map(ConnectorView::from).collect(),
    ))
}

#[utoipa::path(get, path = "/api/v1/connections", tag = "connectors", responses((status = 200, body = [ConnectionView])))]
pub async fn list_connections(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<ConnectionView>>, ApiError> {
    auth.require(Permission::ConnectionsRead)?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    let rows = state
        .store
        .list_connections(auth.org_id, Some(&access.store_access()))
        .await?
        .into_iter()
        .map(ConnectionView::from)
        .collect();
    Ok(Json(rows))
}

/// Config keys that hold an outbound URL the worker or API will call with the connection's
/// credential.
const CONNECTION_URL_FIELDS: [&str; 3] = ["baseUrl", "cloneBaseUrl", "url"];

/// Validate URL-typed connection config fields against the outbound policy
/// (`ORBIT_OUTBOUND_ALLOW_PRIVATE` / `ORBIT_OUTBOUND_ALLOWED_HOSTS`). Private ranges are
/// allowed by default so self-hosted GitLab/GitHub Enterprise keep working; loopback,
/// link-local and cloud-metadata targets are refused with a 422 on `config.<field>`.
async fn validate_connection_urls(config: &serde_json::Value) -> Result<(), ApiError> {
    let policy = orbit_security::OutboundPolicy::from_env();
    let mut issues = Vec::new();
    for field in CONNECTION_URL_FIELDS {
        let Some(value) = config.get(field) else {
            continue;
        };
        let Some(url) = value.as_str().map(str::trim).filter(|url| !url.is_empty()) else {
            if value.is_null() || value.as_str().is_some() {
                continue;
            }
            issues.push(orbit_domain::ValidationIssue::new(
                format!("config.{field}"),
                format!("{field} must be a string"),
            ));
            continue;
        };
        if let Err(error) = orbit_security::check_outbound_url(url, &policy).await {
            issues.push(orbit_domain::ValidationIssue::new(
                format!("config.{field}"),
                error.to_string(),
            ));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
    }
}

#[utoipa::path(post, path = "/api/v1/connections", tag = "connectors", request_body = CreateConnectionRequest, responses((status = 201, body = ConnectionView)))]
pub async fn create_connection(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<ConnectionView>), ApiError> {
    auth.require(Permission::ConnectionsWrite)?;
    if body.name.trim().is_empty() {
        return Err(ApiError::validation(vec![
            orbit_domain::ValidationIssue::new("name", "connection name is required"),
        ]));
    }
    let connector = state
        .store
        .get_connector_by_slug(auth.org_id, &body.connector_slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("connector '{}'", body.connector_slug)))?;
    validate_connection_urls(&body.config).await?;

    let secret_ref = if let Some(credential) = &body.credential {
        if credential.trim().is_empty() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("credential", "credential must not be empty"),
            ]));
        }
        let reference = new_secret_ref("connection");
        state
            .store
            .put_secret(
                Some(auth.org_id),
                &reference,
                &SecretValue::new(credential.trim()),
                &format!("connection:{}", body.name.trim()),
            )
            .await?;
        Some(reference)
    } else {
        None
    };

    let connection = match state
        .store
        .create_connection(
            auth.org_id,
            connector.id,
            body.name.trim(),
            &body.config,
            &body.scopes,
            secret_ref.as_deref(),
            Some(auth.user_id),
        )
        .await
    {
        Ok(connection) => connection,
        Err(err) => {
            if let Some(reference) = &secret_ref {
                let _ = state.store.delete_secret(reference).await;
            }
            return Err(err.into());
        }
    };

    state
        .store
        .assign_resource_owner(auth.org_id, "connections", connection.id, auth.user_id)
        .await?;
    let row = state
        .store
        .get_connection_with_connector(auth.org_id, connection.id)
        .await?;
    audit::record(
        &state,
        &auth,
        "connection.created",
        Some("connection"),
        Some(connection.id),
        serde_json::json!({
            "connector": body.connector_slug,
            "hasCredential": secret_ref.is_some(),
        }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(ConnectionView::from(row))))
}

#[utoipa::path(patch, path = "/api/v1/connections/{connection_id}", tag = "connectors", request_body = UpdateConnectionRequest, responses((status = 200, body = ConnectionView)))]
pub async fn update_connection(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(connection_id): Path<Uuid>,
    crate::extract::ApiJson(body): crate::extract::ApiJson<UpdateConnectionRequest>,
) -> Result<Json<ConnectionView>, ApiError> {
    auth.require(Permission::ConnectionsWrite)?;
    let existing = state
        .store
        .get_connection(auth.org_id, connection_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "connection",
        existing.owner_user_id,
        existing.owner_team_id,
        &existing.visibility,
    )?;
    if let Some(config) = &body.config {
        validate_connection_urls(config).await?;
    }
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut secret_ref: Option<String> = None;
    if let Some(credential) = &body.credential {
        if credential.trim().is_empty() {
            return Err(ApiError::validation(vec![
                orbit_domain::ValidationIssue::new("credential", "credential must not be empty"),
            ]));
        }
        let reference = new_secret_ref("connection");
        state
            .store
            .put_secret(
                Some(auth.org_id),
                &reference,
                &SecretValue::new(credential.trim()),
                &format!("connection:{}", name.unwrap_or(&existing.name)),
            )
            .await?;
        secret_ref = Some(reference);
    }
    let updated = state
        .store
        .update_connection(
            auth.org_id,
            connection_id,
            name,
            body.config.as_ref(),
            body.scopes.as_deref(),
            secret_ref.as_deref(),
        )
        .await?;
    if let Some(reference) = &secret_ref {
        if let Some(old) = &existing.credential_ref {
            if old != reference {
                let _ = state.store.delete_secret(old).await;
            }
        }
    }
    let row = state
        .store
        .get_connection_with_connector(auth.org_id, updated.id)
        .await?;
    audit::record(
        &state,
        &auth,
        "connection.updated",
        Some("connection"),
        Some(updated.id),
        serde_json::json!({ "credentialRotated": secret_ref.is_some() }),
    )
    .await;
    Ok(Json(ConnectionView::from(row)))
}

#[utoipa::path(delete, path = "/api/v1/connections/{connection_id}", tag = "connectors", responses((status = 204)))]
pub async fn delete_connection(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(connection_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    auth.require(Permission::ConnectionsWrite)?;
    let connection = state
        .store
        .get_connection(auth.org_id, connection_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_manage(
        "connection",
        connection.owner_user_id,
        connection.owner_team_id,
        &connection.visibility,
    )?;
    state
        .store
        .delete_connection(auth.org_id, connection_id)
        .await?;
    if let Some(reference) = &connection.credential_ref {
        if let Err(err) = state.store.delete_secret(reference).await {
            tracing::warn!(error = %err, "failed to delete connection secret");
        }
    }
    audit::record(
        &state,
        &auth,
        "connection.deleted",
        Some("connection"),
        Some(connection_id),
        serde_json::json!({ "name": connection.name }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerView {
    pub worker_id: String,
    pub harnesses: Vec<String>,
    pub target: String,
    pub hostname: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_heartbeat_at: chrono::DateTime<chrono::Utc>,
    pub live: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResponse {
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub message: String,
}

#[utoipa::path(post, path = "/api/v1/connections/{connection_id}/test", tag = "connectors", responses((status = 200, body = ConnectionTestResponse)))]
pub async fn test_connection(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(connection_id): Path<Uuid>,
) -> Result<Json<ConnectionTestResponse>, ApiError> {
    auth.require(Permission::ConnectionsRead)?;
    let row = state
        .store
        .get_connection_with_connector(auth.org_id, connection_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_view(
        "connection",
        row.connection.owner_user_id,
        row.connection.owner_team_id,
        &row.connection.visibility,
    )?;
    let credential = match &row.connection.credential_ref {
        Some(reference) => state
            .store
            .get_secret_value(reference)
            .await?
            .map(|value| value.expose().to_string()),
        None => None,
    };
    let started = std::time::Instant::now();
    let (ok, status, message) = probe_connection(
        &row.connector_slug,
        &row.connection.config,
        credential.as_deref(),
    )
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let _ = state
        .store
        .set_connection_check(auth.org_id, connection_id, ok)
        .await;
    Ok(Json(ConnectionTestResponse {
        ok,
        status,
        latency_ms,
        message,
    }))
}

async fn probe_connection(
    slug: &str,
    config: &serde_json::Value,
    credential: Option<&str>,
) -> (bool, Option<u16>, String) {
    let client = orbit_security::guarded_client(std::time::Duration::from_secs(10));
    let (url, request) = match slug {
        "gitlab" | "github" => {
            let base = config
                .get("baseUrl")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim_end_matches('/');
            if base.is_empty() {
                return (false, None, "connection config is missing baseUrl".into());
            }
            let request = client.get(format!("{base}/user"));
            let request = match credential {
                Some(token) if slug == "gitlab" => request.header("PRIVATE-TOKEN", token),
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            (format!("{base}/user"), request)
        }
        "webhook" => {
            let url = config
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if url.is_empty() {
                return (false, None, "connection config is missing url".into());
            }
            (url.to_string(), client.head(url))
        }
        "kubernetes" => {
            let config = match orbit_kubernetes::KubernetesClientConfig::from_connection(
                config,
                credential.map(str::to_string),
            ) {
                Ok(config) => config,
                Err(error) => return (false, None, error),
            };
            return match config.test_connection().await {
                Ok(result) => (
                    true,
                    Some(200),
                    result
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("Kubernetes reachable")
                        .to_string(),
                ),
                Err(error) => (false, None, error),
            };
        }
        other => {
            return (
                false,
                None,
                format!("test connection is not implemented for '{other}'"),
            );
        }
    };
    if let Err(error) =
        orbit_security::check_outbound_url(&url, &orbit_security::OutboundPolicy::from_env()).await
    {
        return (
            false,
            None,
            format!("endpoint refused ({})", error.category()),
        );
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let ok = if slug == "webhook" {
                status.is_success()
                    || status.is_redirection()
                    || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
            } else {
                status.is_success()
            };
            (
                ok,
                Some(status.as_u16()),
                if ok {
                    format!("{url} reachable")
                } else {
                    format!("{url} returned {status}")
                },
            )
        }
        Err(error) => (false, None, orbit_security::describe_request_error(&error)),
    }
}

#[utoipa::path(get, path = "/api/v1/workers", tag = "harnesses", responses((status = 200, body = [WorkerView])))]
pub async fn list_workers(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<Vec<WorkerView>>, ApiError> {
    auth.require(Permission::HarnessesRead)?;
    let now = chrono::Utc::now();
    let mut workers = state.store.list_worker_heartbeats(auth.org_id).await?;
    workers.sort_by_key(|worker| std::cmp::Reverse(worker.last_heartbeat_at));
    Ok(Json(
        workers
            .into_iter()
            .map(|worker| WorkerView {
                live: (now - worker.last_heartbeat_at).num_seconds() < 60,
                worker_id: worker.worker_id,
                harnesses: worker.harnesses,
                target: worker.target,
                hostname: worker.hostname,
                started_at: worker.started_at,
                last_heartbeat_at: worker.last_heartbeat_at,
            })
            .collect(),
    ))
}
