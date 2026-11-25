use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use orbit_contracts::{validate_provider_config, HarnessDescriptor, ProviderKind};
use orbit_domain::{HarnessKind, ImplementationStatus};
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

/// A stored harness row with no adapter in this build's registry. Without an adapter it
/// cannot run, so it is reported as planned; the registry descriptors are the only source
/// of an adapter's real status (AGENTS.md rule 6).
fn fallback_descriptor(kind: HarnessKind, name: &str, description: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        kind,
        name: name.to_string(),
        description: description.to_string(),
        capabilities: Default::default(),
        implementation_status: ImplementationStatus::Planned,
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
            body.enabled.unwrap_or(true),
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
    let result = probe_endpoint(
        kind,
        &provider.config,
        credential.as_deref(),
        &state.outbound,
    )
    .await;
    state.metrics.provider_probe(kind.as_str(), result.ok);

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
    State(state): State<AppState>,
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
    let result = probe_endpoint(kind, &config, body.credential.as_deref(), &state.outbound).await;
    state.metrics.provider_probe(kind.as_str(), result.ok);
    Ok(Json(result))
}

async fn probe_endpoint(
    kind: ProviderKind,
    config: &serde_json::Value,
    credential: Option<&str>,
    policy: &orbit_security::OutboundPolicy,
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
    if let Err(error) = orbit_security::check_outbound_url(&url, policy).await {
        return ProviderProbeResponse {
            ok: false,
            latency_ms: 0,
            models: vec![],
            error: Some(format!("endpoint refused ({})", error.category())),
        };
    }
    let client = orbit_security::guarded_client_with_policy(
        std::time::Duration::from_secs(10),
        policy.clone(),
    );
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

/// Connection config fields that name an endpoint ORBIT sends the credential to.
const CONNECTION_URL_FIELDS: [&str; 4] = ["baseUrl", "cloneBaseUrl", "url", "apiServer"];

/// Validate URL-typed connection config fields against the outbound policy
/// (`ORBIT_OUTBOUND_ALLOW_PRIVATE` / `ORBIT_OUTBOUND_ALLOWED_HOSTS`, read once into
/// `AppState::outbound`) and the Kubernetes namespace fields (`defaultNamespace`,
/// `namespaces`), which are spliced into request paths (#391); errors are 422s on the
/// offending field name. A kubeconfig credential's own `server` is checked when the
/// Kubernetes client connects.
pub(crate) async fn validate_connection_config(
    config: &serde_json::Value,
    policy: &orbit_security::OutboundPolicy,
) -> Result<(), ApiError> {
    let mut issues = Vec::new();
    for field in CONNECTION_URL_FIELDS {
        let Some(value) = config.get(field).and_then(|v| v.as_str()) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if let Err(error) = orbit_security::check_outbound_url(value, policy).await {
            issues.push(orbit_domain::ValidationIssue::new(field, error.to_string()));
        }
    }
    for (field, message) in orbit_kubernetes::config_issues(config) {
        issues.push(orbit_domain::ValidationIssue::new(field, message));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation(issues))
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
    validate_connection_config(&body.config, &state.outbound).await?;

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
        validate_connection_config(config, &state.outbound).await?;
        // A stored credential must never follow an edited endpoint (#321), the same rule
        // providers follow: changing a URL field requires the credential for it.
        if existing.credential_ref.is_some() && body.credential.is_none() {
            if let Some(field) = changed_url_field(&existing.config, config) {
                return Err(ApiError::validation(vec![
                    orbit_domain::ValidationIssue::new(
                        "credential",
                        format!("re-enter the credential when changing {field}"),
                    ),
                ]));
            }
        }
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

/// The first endpoint field whose value differs between two connection configs, ignoring
/// surrounding whitespace and trailing slashes.
fn changed_url_field(
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> Option<&'static str> {
    let normalized = |config: &serde_json::Value, field: &str| {
        config
            .get(field)
            .and_then(|value| value.as_str())
            .map(|value| value.trim().trim_end_matches('/').to_ascii_lowercase())
            .filter(|value| !value.is_empty())
    };
    CONNECTION_URL_FIELDS
        .into_iter()
        .find(|field| normalized(before, field) != normalized(after, field))
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
    /// False when ORBIT has no probe for this connector. Nothing was contacted and
    /// the connection's status is left as it was.
    pub supported: bool,
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
    // A test sends the stored credential upstream and records the result for everyone,
    // so it needs write permission and use of the connection, not just read (#321).
    auth.require(Permission::ConnectionsWrite)?;
    let row = state
        .store
        .get_connection_with_connector(auth.org_id, connection_id)
        .await?;
    let access = crate::access::AccessScope::load(&state, &auth).await?;
    access.require_use(
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
    let Some((ok, status, message)) = probe_connection(
        &row.connector_slug,
        &row.connection.config,
        credential.as_deref(),
        &state.outbound,
    )
    .await
    else {
        // An untestable connector is not a failed one: do not mark it as an error.
        return Ok(Json(ConnectionTestResponse {
            supported: false,
            ok: false,
            status: None,
            latency_ms: 0,
            message: format!(
                "ORBIT cannot test {} connections yet. The credential is checked the first time an agent uses it.",
                row.connector_name
            ),
        }));
    };
    let latency_ms = started.elapsed().as_millis() as u64;
    if let Err(error) = state
        .store
        .set_connection_check(auth.org_id, connection_id, ok)
        .await
    {
        tracing::warn!(%connection_id, %error, "could not record connection test result");
    }
    Ok(Json(ConnectionTestResponse {
        supported: true,
        ok,
        status,
        latency_ms,
        message,
    }))
}

/// Contact the service behind a connection. `None` means this connector has no probe.
async fn probe_connection(
    slug: &str,
    config: &serde_json::Value,
    credential: Option<&str>,
    policy: &orbit_security::OutboundPolicy,
) -> Option<(bool, Option<u16>, String)> {
    Some(match slug {
        // The mock connector talks to nothing; a stored connection is always usable.
        "mock" => (
            true,
            None,
            "Mock connector needs no external service".into(),
        ),
        "gitlab" | "github" | "webhook" | "kubernetes" => {
            probe_supported_connection(slug, config, credential, policy).await
        }
        "s3" => {
            let result = async {
                let (config, credentials) =
                    orbit_s3::S3Config::from_connection(config, credential)?;
                let client = orbit_s3::S3Client::with_policy(config, credentials, policy.clone())?;
                match client.list_buckets().await {
                    Ok(buckets) => Ok(format!(
                        "S3 reachable; {} allowed bucket(s) visible",
                        buckets.len()
                    )),
                    Err(list_error) => {
                        // Many least-privilege credentials can list objects in one bucket
                        // but not enumerate every bucket.
                        let bucket = client
                            .config()
                            .default_bucket
                            .clone()
                            .or_else(|| client.config().allowed_buckets.first().cloned());
                        if let Some(bucket) = bucket {
                            client
                                .list_objects(Some(&bucket), None, None)
                                .await
                                .map(|page| {
                                    format!(
                                        "S3 reachable; {} object(s) visible in {bucket}",
                                        page.objects.len()
                                    )
                                })
                                .map_err(|probe| {
                                    format!("{list_error}; bucket probe failed: {probe}")
                                })
                        } else {
                            Err(list_error.message)
                        }
                    }
                }
            }
            .await;
            match result {
                Ok(message) => (true, Some(200), message),
                Err(message) => (false, None, message),
            }
        }
        _ => return None,
    })
}

async fn probe_supported_connection(
    slug: &str,
    config: &serde_json::Value,
    credential: Option<&str>,
    policy: &orbit_security::OutboundPolicy,
) -> (bool, Option<u16>, String) {
    let client = orbit_security::guarded_client_with_policy(
        std::time::Duration::from_secs(10),
        policy.clone(),
    );
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
            // GitHub requires a User-Agent header; the guarded client does not add one.
            let request = client
                .get(format!("{base}/user"))
                .header("User-Agent", "ORBIT");
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
    if let Err(error) = orbit_security::check_outbound_url(&url, policy).await {
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
