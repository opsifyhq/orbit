//! Secret-free resource export for sharing outside this installation.
//!
//! `GET /api/v1/export/{resource}/{id}` supports three formats:
//!
//! * `json` (default, legacy): `{ resource, id, exportedAt, data }` with the stored
//!   shape of the resource, recursively redacted.
//! * `yaml`: the portable envelope (`apiVersion`/`kind`/`metadata`/`spec`) defined in
//!   `orbit_contracts::portable`. References use names, never ids.
//! * `markdown`: skills only, as `SKILL.md` (frontmatter + instructions).

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use orbit_contracts::portable::{
    redact, skill_to_markdown, PortableDocument, PortableKind, VALUE_PLACEHOLDER,
};
use orbit_domain::AgentDefinition;
use orbit_security::{AuthContext, Permission};

use crate::access::AccessScope;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
pub struct ExportQuery {
    /// `json` (default), `yaml` (portable envelope) or `markdown` (skills: SKILL.md).
    pub format: Option<String>,
}

/// Read permission required to export a given kind.
pub(crate) fn read_permission(kind: PortableKind) -> Permission {
    match kind {
        PortableKind::Agent | PortableKind::Skill | PortableKind::Trigger => Permission::AgentsRead,
        PortableKind::Connection => Permission::ConnectionsRead,
        PortableKind::Provider => Permission::ProvidersRead,
        PortableKind::WorkerProfile | PortableKind::AgentTeam => Permission::HarnessesRead,
        PortableKind::ToolPolicy => Permission::SettingsRead,
    }
}

fn internal<E: std::fmt::Display>(error: E) -> ApiError {
    ApiError::internal(error.to_string())
}

fn redacted(mut value: Value) -> Value {
    redact(&mut value);
    value
}

fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// How a referenced resource appears in an export.
enum Reference {
    /// The caller can view it: its portable name.
    Named(String),
    /// It exists but the caller cannot view it: only the id, never the name (#325).
    Hidden(Uuid),
}

impl Reference {
    /// The value for a name list (`steps[].agent`, `connections`, …). A hidden
    /// resource is written as its id, which an import reports as unresolved.
    fn portable(self) -> String {
        match self {
            Self::Named(name) => name,
            Self::Hidden(id) => id.to_string(),
        }
    }
}

/// Resolves referenced ids to names, within what the exporting user can view.
struct Names<'a> {
    state: &'a AppState,
    org_id: Uuid,
    access: &'a AccessScope,
}

impl<'a> Names<'a> {
    fn new(state: &'a AppState, org_id: Uuid, access: &'a AccessScope) -> Self {
        Self {
            state,
            org_id,
            access,
        }
    }

    async fn visible(&self, resource: &str, id: Uuid) -> bool {
        match self
            .state
            .store
            .resource_ownership(self.org_id, resource, id)
            .await
        {
            Ok((owner_user, owner_team, visibility)) => {
                self.access.can_view(owner_user, owner_team, &visibility)
            }
            Err(_) => false,
        }
    }

    /// `<connector>/<name>` for a connection id, when it still exists.
    async fn connection(&self, id: Uuid) -> Option<Reference> {
        let row = self
            .state
            .store
            .get_connection_with_connector(self.org_id, id)
            .await
            .ok()?;
        if !self.visible("connections", id).await {
            return Some(Reference::Hidden(id));
        }
        Some(Reference::Named(format!(
            "{}/{}",
            row.connector_slug, row.connection.name
        )))
    }

    async fn agent(&self, id: Uuid) -> Option<Reference> {
        let agent = self.state.store.get_agent(self.org_id, id).await.ok()?;
        if !self.visible("agents", id).await {
            return Some(Reference::Hidden(id));
        }
        Some(Reference::Named(agent.name))
    }

    async fn env_mapping(&self, id: Uuid) -> Option<Reference> {
        let mapping = self
            .state
            .store
            .get_env_mapping(self.org_id, id)
            .await
            .ok()?;
        if !self.visible("env_mappings", id).await {
            return Some(Reference::Hidden(id));
        }
        Some(Reference::Named(mapping.name))
    }

    async fn worker_profile(&self, id: Uuid) -> Option<Reference> {
        let profile = self
            .state
            .store
            .get_worker_profile(self.org_id, id)
            .await
            .ok()?;
        if !self.visible("worker_profiles", id).await {
            return Some(Reference::Hidden(id));
        }
        Some(Reference::Named(profile.name))
    }

    /// Replace `connectionId` with a portable `connection: <connector>/<name>` in a
    /// JSON object (agent binding, work source, repository entry). Unknown ids are
    /// dropped; a connection the caller cannot view keeps its `connectionId` only.
    async fn connection_field(&self, value: &mut Value) {
        let Some(object) = value.as_object_mut() else {
            return;
        };
        let Some(raw) = object.remove("connectionId") else {
            return;
        };
        let Some(id) = raw.as_str().and_then(|id| id.parse::<Uuid>().ok()) else {
            return;
        };
        match self.connection(id).await {
            Some(Reference::Named(reference)) => {
                object.insert("connection".into(), Value::String(reference));
            }
            Some(Reference::Hidden(_)) => {
                object.insert("connectionId".into(), raw);
            }
            None => {}
        }
    }
}

/// Build the portable document for a stored resource. Returns the document and the
/// raw name used for file naming. Referenced resources the caller cannot view are
/// written by id, never by name.
pub(crate) async fn build_document(
    state: &AppState,
    org_id: Uuid,
    access: &AccessScope,
    kind: PortableKind,
    id: Uuid,
) -> Result<PortableDocument, ApiError> {
    let store = &state.store;
    let names = Names::new(state, org_id, access);
    let document = match kind {
        PortableKind::Agent => {
            let agent = store.get_agent(org_id, id).await?;
            let version = store.get_current_version(org_id, &agent).await?;
            let spec: orbit_domain::AgentSpec = super::parse_spec(&version)?;
            let definition = AgentDefinition {
                api_version: orbit_domain::AGENT_API_VERSION.to_string(),
                kind: orbit_domain::AGENT_KIND.to_string(),
                metadata: orbit_domain::AgentMetadata {
                    name: agent.name.clone(),
                    description: agent.description.clone(),
                    labels: serde_json::from_value(agent.labels.clone()).unwrap_or_default(),
                },
                spec,
            };
            let mut spec = serde_json::to_value(&definition.spec).map_err(internal)?;
            // Connection ids are installation-specific: export `connection:
            // <connector>/<name>` instead. Everything credential-shaped (webhook
            // `secretRef`, harness config secrets) is removed by `redact`.
            if let Some(bindings) = spec.get_mut("connectors").and_then(Value::as_array_mut) {
                for binding in bindings {
                    names.connection_field(binding).await;
                }
            }
            let mut doc = PortableDocument::new(PortableKind::Agent, agent.name, redacted(spec));
            doc.metadata.description = agent.description;
            doc.metadata.labels = definition.metadata.labels;
            doc
        }
        PortableKind::Skill => {
            let skill = store.get_skill(org_id, id).await?;
            let mut doc = PortableDocument::new(
                PortableKind::Skill,
                skill.name.clone(),
                json!({
                    "slug": skill.slug,
                    "content": skill.content,
                    "files": string_map(&skill.files),
                }),
            );
            if doc.spec["files"].as_object().is_some_and(|m| m.is_empty()) {
                if let Some(object) = doc.spec.as_object_mut() {
                    object.remove("files");
                }
            }
            doc.metadata.description = skill.description;
            doc
        }
        PortableKind::Connection => {
            let row = store.get_connection_with_connector(org_id, id).await?;
            let has_credential = row.connection.credential_ref.is_some();
            let mut doc = PortableDocument::new(
                PortableKind::Connection,
                row.connection.name.clone(),
                json!({
                    "connector": row.connector_slug,
                    "scopes": row.connection.scopes,
                    "config": redacted(row.connection.config.clone()),
                }),
            );
            doc = doc.with_credentials(has_credential);
            doc
        }
        PortableKind::Provider => {
            let provider = store.get_provider(org_id, id).await?;
            let has_credential = provider.secret_ref.is_some();
            PortableDocument::new(
                PortableKind::Provider,
                provider.name.clone(),
                json!({
                    "kind": provider.kind,
                    "enabled": provider.enabled,
                    "config": redacted(provider.config.clone()),
                }),
            )
            .with_credentials(has_credential)
        }
        PortableKind::WorkerProfile => {
            let profile = store.get_worker_profile(org_id, id).await?;
            let mut agents = Vec::new();
            for agent_id in &profile.agent_ids {
                if let Some(reference) = names.agent(*agent_id).await {
                    agents.push(reference.portable());
                }
            }
            let mut env_mappings = Vec::new();
            for mapping in store.list_worker_profile_env_mappings(profile.id).await? {
                if let Some(reference) = names.env_mapping(mapping.id).await {
                    env_mappings.push(reference.portable());
                }
            }
            let env: BTreeMap<String, String> = profile
                .env
                .as_object()
                .map(|map| {
                    map.keys()
                        .map(|key| (key.clone(), VALUE_PLACEHOLDER.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let needs_credentials = !env.is_empty() || profile.auth_secret_name.is_some();
            let mut spec = redacted(json!({
                "harnessKind": profile.harness_kind,
                "target": profile.target,
                "executionDriver": profile.execution_driver,
                "image": profile.image,
                "imageTag": profile.image_tag,
                "agentScope": profile.agent_scope,
                "agents": agents,
                "skills": profile.skill_slugs,
                "provider": profile.provider,
                "model": profile.model,
                "envMappings": env_mappings,
                "resources": profile.resources,
                "allowWebSearch": profile.allow_web_search,
                "allowWebFetch": profile.allow_web_fetch,
                "allowGlab": profile.allow_glab,
                "allowGh": profile.allow_gh,
                "nativeToolPermissions": profile.native_tool_permissions,
            }));
            // `env` is built after redaction: names are kept, values never leave.
            spec["env"] = serde_json::to_value(env).map_err(internal)?;
            PortableDocument::new(PortableKind::WorkerProfile, profile.name, spec)
                .with_credentials(needs_credentials)
        }
        PortableKind::AgentTeam => {
            let team = store.get_agent_team(org_id, id).await?;
            let definition = super::teams::parse_definition(&team)?;
            let worker_profile = match definition.worker_profile_id {
                Some(profile_id) => names
                    .worker_profile(profile_id)
                    .await
                    .map(Reference::portable),
                None => None,
            };
            let mut connections = Vec::new();
            for connection_id in &definition.connection_ids {
                if let Some(reference) = names.connection(*connection_id).await {
                    connections.push(reference.portable());
                }
            }
            let mut env_mappings = Vec::new();
            for mapping_id in &definition.env_mapping_ids {
                if let Some(reference) = names.env_mapping(*mapping_id).await {
                    env_mappings.push(reference.portable());
                }
            }
            let mut steps = Vec::new();
            for step in &definition.steps {
                if let Some(reference) = names.agent(step.agent_id).await {
                    let mut entry = json!({ "agent": reference.portable() });
                    if let Some(model) = &step.model {
                        entry["model"] = serde_json::to_value(model).map_err(internal)?;
                    }
                    steps.push(entry);
                }
            }
            let mut work_source = match &definition.work_source {
                Some(source) => Some(serde_json::to_value(source).map_err(internal)?),
                None => None,
            };
            if let Some(source) = work_source.as_mut() {
                names.connection_field(source).await;
            }
            let mut repositories = Vec::new();
            for repository in &definition.repositories {
                let mut value = serde_json::to_value(repository).map_err(internal)?;
                names.connection_field(&mut value).await;
                if value.get("connectionId").is_none() {
                    if let Some(object) = value.as_object_mut() {
                        object.retain(|_, v| !v.is_null());
                    }
                }
                repositories.push(value);
            }
            // Workflow steps name their agent, like `steps` does, instead of carrying ids.
            let workflow = match &definition.workflow {
                Some(workflow) => {
                    let mut value = serde_json::to_value(workflow).map_err(internal)?;
                    for (node, source) in value["nodes"]
                        .as_array_mut()
                        .into_iter()
                        .flatten()
                        .zip(&workflow.nodes)
                    {
                        if let Some(object) = node.as_object_mut() {
                            object.remove("agentId");
                            if let Some(reference) = names.agent(source.agent_id).await {
                                object.insert("agent".into(), json!(reference.portable()));
                            }
                        }
                    }
                    Some(value)
                }
                None => None,
            };
            let mut spec = redacted(json!({
                "mode": definition.mode,
                "instructions": definition.instructions,
                "repository": definition.repository,
                "branch": definition.branch,
                "model": definition.model,
                "workSource": work_source,
                "repositories": repositories,
                "workerProfile": worker_profile,
                "autoLaunch": definition.auto_launch,
                "skills": definition.skills,
                "harness": definition.harness,
                "connections": connections,
                "envMappings": env_mappings,
                "timeoutSeconds": definition.timeout_seconds,
                // Elevations are installation-specific authorisations; an import re-authorises.
                "autonomy": definition.autonomy.as_ref().map(orbit_domain::TeamAutonomy::snapshot),
                "steps": steps,
            }));
            if let Some(workflow) = workflow {
                spec["workflow"] = workflow;
            }
            let mut doc = PortableDocument::new(PortableKind::AgentTeam, team.name, spec);
            doc.metadata.description = team.description;
            doc
        }
        PortableKind::Trigger => {
            let trigger = store.get_trigger(org_id, id).await?;
            let agent = names
                .agent(trigger.agent_id)
                .await
                .ok_or_else(|| ApiError::not_found("agent of trigger"))?
                .portable();
            let webhook = trigger.kind == "webhook";
            let mut doc = PortableDocument::new(
                PortableKind::Trigger,
                format!("{agent} {}", trigger.kind),
                json!({
                    "agent": agent,
                    "kind": trigger.kind,
                    "harness": trigger.harness_kind,
                    "enabled": trigger.enabled,
                    "config": redacted(trigger.config.clone()),
                }),
            );
            if webhook {
                doc = doc.with_credentials(true);
            }
            doc
        }
        PortableKind::ToolPolicy => {
            let policy = store.get_tool_policy(org_id, id).await?;
            let agent = match policy.agent_id {
                Some(agent_id) => names.agent(agent_id).await.map(Reference::portable),
                None => None,
            };
            let mut doc = PortableDocument::new(
                PortableKind::ToolPolicy,
                policy.name.clone(),
                redacted(json!({
                    "agent": agent,
                    "priority": policy.priority,
                    "enabled": policy.enabled,
                    "action": policy.action,
                    "matcher": policy.matcher,
                    "reason": policy.reason,
                    "impact": policy.impact,
                })),
            );
            if let Some(object) = doc.spec.as_object_mut() {
                object.retain(|_, v| !v.is_null());
            }
            doc
        }
    };
    Ok(document)
}

fn file_stem(name: &str) -> String {
    let slug = orbit_contracts::portable::slugify(name);
    if slug.is_empty() {
        "export".into()
    } else {
        slug
    }
}

fn attachment(
    mut response: Response,
    content_type: &'static str,
    filename: &str,
) -> Result<Response, ApiError> {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{safe}\"")
            .parse()
            .map_err(|_| ApiError::internal("invalid export filename"))?,
    );
    Ok(response)
}

#[utoipa::path(get, path = "/api/v1/export/{resource}/{resource_id}", tag = "export", params(("resource" = String, Path, description = "agents, skills, connections, providers, worker_profiles, agent_teams, triggers or tool_policies"), ("resource_id" = Uuid, Path), ExportQuery), responses((status = 200, description = "Secret-free resource export (JSON, portable YAML or SKILL.md)")))]
pub async fn export_resource(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((resource, resource_id)): Path<(String, Uuid)>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let kind = PortableKind::from_resource(&resource)
        .ok_or_else(|| ApiError::bad_request(format!("unknown resource '{resource}'")))?;
    auth.require(read_permission(kind))?;
    let (owner_user, owner_team, visibility) = state
        .store
        .resource_ownership(auth.org_id, &resource, resource_id)
        .await?;
    let access = AccessScope::load(&state, &auth).await?;
    access.require_view(&resource, owner_user, owner_team, &visibility)?;

    let format = query.format.as_deref().unwrap_or("json");
    let document = build_document(&state, auth.org_id, &access, kind, resource_id).await?;
    let stem = file_stem(&document.metadata.name);
    match format {
        "yaml" | "yml" => {
            let text = document.to_yaml().map_err(internal)?;
            attachment(
                text.into_response(),
                "application/yaml; charset=utf-8",
                &format!("{stem}.{}.yaml", kind.as_str().to_ascii_lowercase()),
            )
        }
        "markdown" | "md" => {
            if kind != PortableKind::Skill {
                return Err(ApiError::bad_request(
                    "markdown export is only available for skills",
                ));
            }
            let spec = &document.spec;
            let text = skill_to_markdown(
                spec["slug"].as_str().unwrap_or_default(),
                &document.metadata.name,
                &document.metadata.description,
                spec["content"].as_str().unwrap_or_default(),
            );
            let omitted = spec
                .get("files")
                .and_then(Value::as_object)
                .map_or(0, |files| files.len());
            let mut response = attachment(
                text.into_response(),
                "text/markdown; charset=utf-8",
                "SKILL.md",
            )?;
            if omitted > 0 {
                // SKILL.md cannot carry bundled files; the YAML export does.
                response.headers_mut().insert(
                    "x-orbit-omitted-files",
                    HeaderValue::from_str(&omitted.to_string()).map_err(internal)?,
                );
            }
            Ok(response)
        }
        "json" => {
            let data = serde_json::to_value(&document).map_err(internal)?;
            let filename = format!("{stem}-{}.json", &resource_id.to_string()[..8]);
            let response = axum::Json(json!({
                "resource": resource,
                "id": resource_id,
                "exportedAt": chrono::Utc::now(),
                "data": data,
            }))
            .into_response();
            attachment(response, "application/json", &filename)
        }
        other => Err(ApiError::bad_request(format!(
            "unknown format '{other}', expected json, yaml or markdown"
        ))),
    }
}
