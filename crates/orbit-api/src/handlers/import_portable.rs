//! `POST /api/v1/import`: validated import of portable documents.
//!
//! Accepts a `SKILL.md`, a single YAML/JSON envelope (`apiVersion`/`kind`/`metadata`/
//! `spec`) or a bundle (multi-document YAML or a JSON array). Processing is split:
//!
//! 1. **Plan**: parse, authorize, validate every document, resolve every reference
//!    by name/slug and decide the action (create/update) from the `mode`. Nothing
//!    is written. References to resources that are created earlier in the same
//!    bundle are accepted.
//! 2. **Apply** (skipped for `dryRun=true` or when any item is invalid): items are
//!    written in dependency order through the same handlers the UI uses, so limits,
//!    ownership, validation and per-resource audit entries are identical to a
//!    normal create/update.
//!
//! Credentials are never read from a document: secret-looking fields are dropped
//! with a warning, and credentials are set afterwards on the imported resource.

use std::collections::HashSet;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use orbit_contracts::portable::{
    parse_upload, strip_secrets, AgentTeamSpec, ConnectionSpec, ImportAction, ImportItemReport,
    ImportItemStatus, ImportMode, ImportReport, PortableDocument, PortableKind, ProviderSpec,
    SkillSpec, ToolPolicySpec, TriggerPortableSpec, UnresolvedReference, WorkerProfileSpec,
};
use orbit_contracts::validate_provider_config;
use orbit_domain::{
    AgentDefinition, ToolPolicyAction, ToolPolicyMatcher, ToolPolicyRule, ValidationIssue,
};
use orbit_security::{AuthContext, Permission};

use crate::access::AccessScope;
use crate::audit;
use crate::dto::{
    parse_provider_kind, CreateConnectionRequest, CreateProviderRequest, CreateTriggerRequest,
    UpdateAgentRequest, UpdateConnectionRequest, UpdateProviderRequest, UpdateTriggerRequest,
};
use crate::error::ApiError;
use crate::extract::ApiJson;
use crate::handlers::skills::{CreateSkillRequest, UpdateSkillRequest};
use crate::handlers::teams::{CreateTeamRequest, TeamStepInput};
use crate::handlers::tool_policies::ToolPolicyRequest;
use crate::handlers::worker_profiles::CreateWorkerProfileRequest;
use crate::state::AppState;
use crate::team_runs::TeamModelOverride;
use crate::work_sources::{RepositorySpec, WorkSourceDefinition};

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct PortableImportQuery {
    /// `create` (default: a name conflict is a 409), `update` or `upsert`.
    pub mode: Option<ImportMode>,
    /// Validate and report only; nothing is written.
    pub dry_run: Option<bool>,
}

// ------------------------------------------------------------------ references

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Ref {
    Agent,
    Skill,
    Connection,
    Connector,
    Provider,
    WorkerProfile,
    EnvMapping,
}

impl Ref {
    fn label(self) -> &'static str {
        match self {
            Ref::Agent => "agent",
            Ref::Skill => "skill",
            Ref::Connection => "connection",
            Ref::Connector => "connector",
            Ref::Provider => "provider",
            Ref::WorkerProfile => "worker_profile",
            Ref::EnvMapping => "env_mapping",
        }
    }

    /// Ownership table, when the resource is shareable (visibility applies).
    fn resource(self) -> Option<&'static str> {
        match self {
            Ref::Agent => Some("agents"),
            Ref::Skill => Some("skills"),
            Ref::Connection => Some("connections"),
            Ref::Provider => Some("providers"),
            Ref::WorkerProfile => Some("worker_profiles"),
            Ref::Connector | Ref::EnvMapping => None,
        }
    }
}

struct Resolver<'a> {
    state: &'a AppState,
    org_id: Uuid,
    access: &'a AccessScope,
    /// References satisfied by documents earlier in the same bundle.
    provided: &'a HashSet<(Ref, String)>,
    /// Plan mode: provided references resolve to a nil id. Apply mode: they must exist.
    lenient: bool,
}

impl Resolver<'_> {
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

    async fn lookup(&self, reference: Ref, key: &str) -> Option<Uuid> {
        let store = &self.state.store;
        let found = match reference {
            Ref::Connector => store
                .get_connector_by_slug(self.org_id, key)
                .await
                .ok()
                .flatten()
                .map(|connector| connector.id),
            Ref::Connection => {
                let (connector, name) = key.split_once('/')?;
                store
                    .find_connection_id(self.org_id, connector, name)
                    .await
                    .ok()
                    .flatten()
            }
            Ref::EnvMapping => store
                .find_resource_id_by_key(self.org_id, "env_mappings", key)
                .await
                .ok()
                .flatten(),
            other => store
                .find_resource_id_by_key(self.org_id, other.resource()?, key)
                .await
                .ok()
                .flatten(),
        }?;
        if let Some(resource) = reference.resource() {
            if !self.visible(resource, found).await {
                return None;
            }
        }
        Some(found)
    }

    /// Resolve a reference. Unresolved ones are recorded and a nil id is returned.
    async fn resolve(
        &self,
        reference: Ref,
        key: &str,
        path: &str,
        unresolved: &mut Vec<UnresolvedReference>,
    ) -> Uuid {
        if let Some(id) = self.lookup(reference, key).await {
            return id;
        }
        if !(self.lenient && self.provided.contains(&(reference, key.to_string()))) {
            unresolved.push(UnresolvedReference {
                kind: reference.label().into(),
                reference: key.into(),
                path: path.into(),
            });
        }
        Uuid::nil()
    }
}

// ------------------------------------------------------------------ translation

/// A document translated into the request the matching handler expects.
enum Translated {
    Agent(Box<AgentDefinition>),
    Skill(CreateSkillRequest),
    Connection(CreateConnectionRequest),
    Provider(CreateProviderRequest),
    WorkerProfile(Box<CreateWorkerProfileRequest>),
    Team(Box<CreateTeamRequest>),
    Trigger(CreateTriggerRequest),
    ToolPolicy(ToolPolicyRequest),
}

#[derive(Default)]
struct Findings {
    unresolved: Vec<UnresolvedReference>,
    issues: Vec<ValidationIssue>,
    warnings: Vec<String>,
}

impl Findings {
    fn issue(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ValidationIssue::new(path, message));
    }

    /// Drop credentials from a value and warn about each removal.
    fn strip(&mut self, value: &mut Value, path: &str) {
        let mut removed = Vec::new();
        strip_secrets(value, path, &mut removed);
        for field in removed {
            self.warnings.push(format!(
                "ignored `{field}`: credentials are never imported from files; set them in ORBIT after import"
            ));
        }
    }

    fn spec<T: serde::de::DeserializeOwned>(&mut self, doc: &PortableDocument) -> Option<T> {
        match serde_json::from_value::<T>(doc.spec.clone()) {
            Ok(spec) => Some(spec),
            Err(error) => {
                self.issue("spec", error.to_string());
                None
            }
        }
    }
}

fn nonempty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn replace_connection_ref(object: &mut serde_json::Map<String, Value>, id: Uuid, resolved: bool) {
    object.remove("connection");
    if resolved {
        object.insert("connectionId".into(), Value::String(id.to_string()));
    }
}

async fn translate(
    kind: PortableKind,
    doc: &PortableDocument,
    resolver: &Resolver<'_>,
    found: &mut Findings,
) -> Option<Translated> {
    let name = doc.metadata.name.trim().to_string();
    // Agents, skills and tool policies check the limit in their own validators.
    if matches!(
        kind,
        PortableKind::Provider
            | PortableKind::Connection
            | PortableKind::WorkerProfile
            | PortableKind::AgentTeam
    ) {
        if let Some(issue) = orbit_domain::name_length_issue("metadata.name", &name) {
            found.issues.push(issue);
        }
    }
    match kind {
        PortableKind::Agent => {
            let mut spec = doc.spec.clone();
            if !spec.is_object() {
                found.issue("spec", "agent spec must be a mapping");
                return None;
            }
            // Free text is never scanned for secrets.
            let instructions = spec.as_object_mut().and_then(|o| o.remove("instructions"));
            found.strip(&mut spec, "spec");
            if let (Some(instructions), Some(object)) = (instructions, spec.as_object_mut()) {
                object.insert("instructions".into(), instructions);
            }
            if let Some(bindings) = spec.get_mut("connectors").and_then(Value::as_array_mut) {
                for (index, binding) in bindings.iter_mut().enumerate() {
                    let Some(object) = binding.as_object_mut() else {
                        if let Some(slug) = binding.as_str() {
                            resolver
                                .resolve(
                                    Ref::Connector,
                                    slug,
                                    &format!("spec.connectors[{index}]"),
                                    &mut found.unresolved,
                                )
                                .await;
                        }
                        continue;
                    };
                    if let Some(slug) = object.get("connector").and_then(Value::as_str) {
                        resolver
                            .resolve(
                                Ref::Connector,
                                slug,
                                &format!("spec.connectors[{index}].connector"),
                                &mut found.unresolved,
                            )
                            .await;
                    }
                    if let Some(reference) = object
                        .get("connection")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                    {
                        let before = found.unresolved.len();
                        let id = resolver
                            .resolve(
                                Ref::Connection,
                                &reference,
                                &format!("spec.connectors[{index}].connection"),
                                &mut found.unresolved,
                            )
                            .await;
                        replace_connection_ref(object, id, found.unresolved.len() == before);
                    } else if let Some(id) = object
                        .get("connectionId")
                        .and_then(Value::as_str)
                        .and_then(|raw| raw.parse::<Uuid>().ok())
                    {
                        // Raw ids only make sense inside the installation that wrote them.
                        if resolver
                            .state
                            .store
                            .get_connection(resolver.org_id, id)
                            .await
                            .is_err()
                        {
                            found.unresolved.push(UnresolvedReference {
                                kind: "connection".into(),
                                reference: id.to_string(),
                                path: format!("spec.connectors[{index}].connectionId"),
                            });
                        }
                    }
                }
            }
            if let Some(skills) = spec.get("skills").and_then(Value::as_array) {
                for (index, skill) in skills.iter().enumerate() {
                    if let Some(slug) = skill.as_str() {
                        resolver
                            .resolve(
                                Ref::Skill,
                                slug,
                                &format!("spec.skills[{index}]"),
                                &mut found.unresolved,
                            )
                            .await;
                    }
                }
            }
            if spec.pointer("/model/management").and_then(Value::as_str) == Some("platform") {
                if let Some(provider) = spec.pointer("/model/provider").and_then(Value::as_str) {
                    resolver
                        .resolve(
                            Ref::Provider,
                            provider,
                            "spec.model.provider",
                            &mut found.unresolved,
                        )
                        .await;
                }
            }
            let definition = json!({
                "apiVersion": orbit_domain::AGENT_API_VERSION,
                "kind": orbit_domain::AGENT_KIND,
                "metadata": {
                    "name": name,
                    "description": doc.metadata.description,
                    "labels": doc.metadata.labels,
                },
                "spec": spec,
            });
            match serde_json::from_value::<AgentDefinition>(definition) {
                Ok(definition) => {
                    found.issues.extend(definition.validate().issues);
                    Some(Translated::Agent(Box::new(definition)))
                }
                Err(error) => {
                    found.issue("spec", error.to_string());
                    None
                }
            }
        }
        PortableKind::Skill => {
            let spec: SkillSpec = found.spec(doc)?;
            let files = Value::Object(
                spec.files
                    .iter()
                    .map(|(path, content)| (path.clone(), Value::String(content.clone())))
                    .collect(),
            );
            let request = CreateSkillRequest {
                name: name.clone(),
                slug: spec.slug.trim().to_string(),
                description: doc.metadata.description.trim().to_string(),
                content: spec.content,
                files,
            };
            if let Err(error) = crate::handlers::skills::validate_skill(
                &request.slug,
                &request.name,
                &request.content,
                &request.files,
            ) {
                found.issues.extend(error.details);
            }
            Some(Translated::Skill(request))
        }
        PortableKind::Connection => {
            let spec: ConnectionSpec = found.spec(doc)?;
            let mut config = spec.config.clone();
            found.strip(&mut config, "spec.config");
            resolver
                .resolve(
                    Ref::Connector,
                    &spec.connector,
                    "spec.connector",
                    &mut found.unresolved,
                )
                .await;
            if nonempty(&name).is_none() {
                found.issue("metadata.name", "connection name is required");
            }
            if config.is_null() {
                config = json!({});
            }
            if let Err(error) = crate::handlers::catalog::validate_connection_config(
                &config,
                &resolver.state.outbound,
            )
            .await
            {
                for issue in &error.details {
                    found.issue(issue.path.clone(), issue.message.clone());
                }
            }
            Some(Translated::Connection(CreateConnectionRequest {
                connector_slug: spec.connector,
                name,
                credential: None,
                config,
                scopes: spec.scopes,
            }))
        }
        PortableKind::Provider => {
            let spec: ProviderSpec = found.spec(doc)?;
            let mut config = spec.config.clone();
            found.strip(&mut config, "spec.config");
            if config.is_null() {
                config = json!({});
            }
            match parse_provider_kind(&spec.kind) {
                Ok(provider_kind) => {
                    let report = validate_provider_config(provider_kind, &config);
                    for issue in report.issues {
                        found.issue(format!("spec.config.{}", issue.path), issue.message);
                    }
                }
                Err(message) => found.issue("spec.kind", message),
            }
            if nonempty(&name).is_none() {
                found.issue("metadata.name", "provider name is required");
            }
            Some(Translated::Provider(CreateProviderRequest {
                name,
                kind: spec.kind,
                config,
                credential: None,
                enabled: Some(spec.enabled),
            }))
        }
        PortableKind::WorkerProfile => {
            let spec: WorkerProfileSpec = found.spec(doc)?;
            if nonempty(&name).is_none() {
                found.issue("metadata.name", "worker profile name is required");
            }
            if spec
                .harness_kind
                .parse::<orbit_domain::HarnessKind>()
                .is_err()
            {
                found.issue(
                    "spec.harnessKind",
                    format!("unknown harness '{}'", spec.harness_kind),
                );
            }
            let target = spec.target.clone().unwrap_or_else(|| "docker".into());
            if !matches!(target.as_str(), "local" | "docker" | "kubernetes") {
                found.issue("spec.target", "must be local, docker or kubernetes");
            }
            let driver = spec
                .execution_driver
                .clone()
                .unwrap_or_else(|| "process".into());
            if !matches!(driver.as_str(), "process" | "docker") {
                found.issue("spec.executionDriver", "must be process or docker");
            }
            if let Some(scope) = spec.agent_scope.as_deref() {
                if !matches!(scope, "all" | "selected" | "") {
                    found.issue("spec.agentScope", "must be 'all' or 'selected'");
                }
            }
            if let Err(error) = crate::handlers::worker_profiles::validate_native_tool_permissions(
                &spec.native_tool_permissions,
            ) {
                found.issues.extend(error.details);
            }
            if let Err(error) =
                crate::handlers::worker_profiles::resolve_worker_resources(&spec.resources)
            {
                found.issues.extend(error.details);
            }
            let mut agent_ids = Vec::new();
            for (index, agent) in spec.agents.iter().enumerate() {
                agent_ids.push(
                    resolver
                        .resolve(
                            Ref::Agent,
                            agent,
                            &format!("spec.agents[{index}]"),
                            &mut found.unresolved,
                        )
                        .await,
                );
            }
            for (index, skill) in spec.skills.iter().enumerate() {
                resolver
                    .resolve(
                        Ref::Skill,
                        skill,
                        &format!("spec.skills[{index}]"),
                        &mut found.unresolved,
                    )
                    .await;
            }
            let mut env_mapping_ids = Vec::new();
            for (index, mapping) in spec.env_mappings.iter().enumerate() {
                env_mapping_ids.push(
                    resolver
                        .resolve(
                            Ref::EnvMapping,
                            mapping,
                            &format!("spec.envMappings[{index}]"),
                            &mut found.unresolved,
                        )
                        .await,
                );
            }
            if !spec.env.is_empty() {
                found.warnings.push(
                    "environment variable values are never exported; the variables are created empty and need values"
                        .into(),
                );
            }
            let env: serde_json::Map<String, Value> = spec
                .env
                .keys()
                .map(|key| (key.clone(), Value::String(String::new())))
                .collect();
            Some(Translated::WorkerProfile(Box::new(
                CreateWorkerProfileRequest {
                    name,
                    harness_kind: spec.harness_kind,
                    target,
                    execution_driver: driver,
                    image: spec.image.unwrap_or_default(),
                    image_tag: spec.image_tag.unwrap_or_default(),
                    agent_scope: spec.agent_scope,
                    agent_ids,
                    skill_slugs: spec.skills,
                    provider: spec.provider,
                    model: spec.model,
                    auth_secret_name: None,
                    env: Value::Object(env),
                    env_mappings: Value::Null,
                    env_mapping_ids,
                    resources: spec.resources,
                    allow_web_search: spec.allow_web_search,
                    allow_web_fetch: spec.allow_web_fetch,
                    allow_glab: spec.allow_glab,
                    allow_gh: spec.allow_gh,
                    native_tool_permissions: spec.native_tool_permissions,
                },
            )))
        }
        PortableKind::AgentTeam => {
            let spec: AgentTeamSpec = found.spec(doc)?;
            if nonempty(&name).is_none() {
                found.issue("metadata.name", "team name is required");
            }
            let mode = spec.mode.clone().unwrap_or_else(|| "pipeline".into());
            if !matches!(mode.as_str(), "pipeline" | "native" | "workflow") {
                found.issue("spec.mode", "mode must be pipeline, native or workflow");
            }
            // A workflow names its agents on its steps; resolve them to ids.
            let workflow = match spec.workflow.clone() {
                Some(mut value) => {
                    for (index, node) in value["nodes"]
                        .as_array_mut()
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        let path = format!("spec.workflow.nodes[{index}].agent");
                        let Some(object) = node.as_object_mut() else {
                            continue;
                        };
                        match object.remove("agent") {
                            Some(serde_json::Value::String(agent)) => {
                                let agent_id = resolver
                                    .resolve(Ref::Agent, &agent, &path, &mut found.unresolved)
                                    .await;
                                object.insert("agentId".into(), json!(agent_id));
                            }
                            _ => found.issue(path, "agent name is required"),
                        }
                    }
                    match serde_json::from_value::<orbit_domain::WorkflowDefinition>(value) {
                        Ok(workflow) => Some(workflow),
                        Err(error) => {
                            found.issue("spec.workflow", error.to_string());
                            None
                        }
                    }
                }
                None => None,
            };
            if mode == "workflow" {
                if spec.workflow.is_none() {
                    found.issue("spec.workflow", "a workflow team needs a workflow");
                }
            } else if spec.steps.is_empty() {
                found.issue("spec.steps", "at least one step is required");
            }
            let mut steps = Vec::new();
            let mut seen = HashSet::new();
            for (index, step) in spec.steps.iter().enumerate() {
                if !seen.insert(step.agent.clone()) {
                    found.issue(
                        format!("spec.steps[{index}].agent"),
                        "the same agent cannot appear twice in one team",
                    );
                }
                let agent_id = resolver
                    .resolve(
                        Ref::Agent,
                        &step.agent,
                        &format!("spec.steps[{index}].agent"),
                        &mut found.unresolved,
                    )
                    .await;
                let model = match &step.model {
                    Some(value) => match serde_json::from_value::<TeamModelOverride>(value.clone())
                    {
                        Ok(model) => Some(model),
                        Err(error) => {
                            found.issue(format!("spec.steps[{index}].model"), error.to_string());
                            None
                        }
                    },
                    None => None,
                };
                steps.push(TeamStepInput { agent_id, model });
            }
            let worker_profile_id = match spec.worker_profile.as_deref() {
                Some(profile) => Some(
                    resolver
                        .resolve(
                            Ref::WorkerProfile,
                            profile,
                            "spec.workerProfile",
                            &mut found.unresolved,
                        )
                        .await,
                ),
                None => None,
            };
            let mut connection_ids = Vec::new();
            for (index, connection) in spec.connections.iter().enumerate() {
                connection_ids.push(
                    resolver
                        .resolve(
                            Ref::Connection,
                            connection,
                            &format!("spec.connections[{index}]"),
                            &mut found.unresolved,
                        )
                        .await,
                );
            }
            let mut env_mapping_ids = Vec::new();
            for (index, mapping) in spec.env_mappings.iter().enumerate() {
                env_mapping_ids.push(
                    resolver
                        .resolve(
                            Ref::EnvMapping,
                            mapping,
                            &format!("spec.envMappings[{index}]"),
                            &mut found.unresolved,
                        )
                        .await,
                );
            }
            for (index, skill) in spec.skills.iter().enumerate() {
                resolver
                    .resolve(
                        Ref::Skill,
                        skill,
                        &format!("spec.skills[{index}]"),
                        &mut found.unresolved,
                    )
                    .await;
            }
            let work_source = match spec.work_source.clone() {
                Some(mut value) => {
                    resolve_connection_field(
                        resolver,
                        &mut value,
                        "spec.workSource",
                        &mut found.unresolved,
                    )
                    .await;
                    match serde_json::from_value::<WorkSourceDefinition>(value) {
                        Ok(source) => Some(source),
                        Err(error) => {
                            found.issue("spec.workSource", error.to_string());
                            None
                        }
                    }
                }
                None => None,
            };
            let mut repositories = Vec::new();
            for (index, repository) in spec.repositories.iter().enumerate() {
                let mut value = repository.clone();
                let path = format!("spec.repositories[{index}]");
                resolve_connection_field(resolver, &mut value, &path, &mut found.unresolved).await;
                match serde_json::from_value::<RepositorySpec>(value) {
                    Ok(repository) => repositories.push(repository),
                    Err(error) => found.issue(path, error.to_string()),
                }
            }
            let model = match &spec.model {
                Some(value) => match serde_json::from_value::<TeamModelOverride>(value.clone()) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        found.issue("spec.model", error.to_string());
                        None
                    }
                },
                None => None,
            };
            let autonomy = match spec.autonomy {
                Some(value) => match serde_json::from_value::<orbit_domain::TeamAutonomy>(value) {
                    Ok(autonomy) => Some(autonomy),
                    Err(error) => {
                        found.issue("spec.autonomy", error.to_string());
                        None
                    }
                },
                None => None,
            };
            Some(Translated::Team(Box::new(CreateTeamRequest {
                name,
                description: doc.metadata.description.clone(),
                mode,
                instructions: spec.instructions,
                repository: spec.repository,
                branch: spec.branch,
                model,
                work_source,
                repositories,
                worker_profile_id,
                auto_launch: spec.auto_launch,
                skills: spec.skills,
                harness: spec.harness,
                connection_ids,
                env_mapping_ids,
                timeout_seconds: spec.timeout_seconds,
                autonomy,
                steps,
                workflow,
                expected_updated_at: None,
            })))
        }
        PortableKind::Trigger => {
            let spec: TriggerPortableSpec = found.spec(doc)?;
            let mut config = spec.config.clone();
            found.strip(&mut config, "spec.config");
            if config.is_null() {
                config = json!({});
            }
            let agent_id = resolver
                .resolve(Ref::Agent, &spec.agent, "spec.agent", &mut found.unresolved)
                .await;
            if let Some(harness) = &spec.harness {
                if harness.parse::<orbit_domain::HarnessKind>().is_err() {
                    found.issue("spec.harness", "unknown harness");
                }
            }
            match spec.kind.as_str() {
                "schedule" => {
                    let cron = config.get("cron").and_then(Value::as_str).unwrap_or("");
                    if cron.parse::<croner::Cron>().is_err() {
                        found.issue(
                            "spec.config.cron",
                            "invalid cron expression (5-field cron in UTC)",
                        );
                    }
                }
                "event" => {
                    if config
                        .get("subject")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .is_empty()
                    {
                        found.issue("spec.config.subject", "event subject filter is required");
                    }
                }
                "webhook" | "manual" => {}
                other => found.issue("spec.kind", format!("unknown trigger kind '{other}'")),
            }
            Some(Translated::Trigger(CreateTriggerRequest {
                agent_id,
                kind: spec.kind,
                harness: spec.harness,
                config,
                enabled: Some(spec.enabled),
            }))
        }
        PortableKind::ToolPolicy => {
            let spec: ToolPolicySpec = found.spec(doc)?;
            let agent_id = match spec.agent.as_deref() {
                Some(agent) => Some(
                    resolver
                        .resolve(Ref::Agent, agent, "spec.agent", &mut found.unresolved)
                        .await,
                ),
                None => None,
            };
            let matcher = if spec.matcher.is_null() {
                ToolPolicyMatcher::default()
            } else {
                match serde_json::from_value::<ToolPolicyMatcher>(spec.matcher.clone()) {
                    Ok(matcher) => matcher,
                    Err(error) => {
                        found.issue("spec.matcher", error.to_string());
                        ToolPolicyMatcher::default()
                    }
                }
            };
            match ToolPolicyAction::parse(spec.action.trim()) {
                Some(action) => {
                    let rule = ToolPolicyRule {
                        id: Uuid::nil(),
                        name: name.clone(),
                        priority: spec.priority,
                        enabled: spec.enabled,
                        action,
                        matcher: matcher.clone(),
                        reason: spec.reason.clone(),
                        impact: spec.impact.clone(),
                    };
                    for message in orbit_domain::validate_tool_policy(&rule) {
                        found.issue("spec", message);
                    }
                    if !(-10_000..=10_000).contains(&spec.priority) {
                        found.issue("spec.priority", "priority must be between -10000 and 10000");
                    }
                }
                None => found.issue(
                    "spec.action",
                    "action must be allow, deny or require_approval",
                ),
            }
            Some(Translated::ToolPolicy(ToolPolicyRequest {
                name,
                agent_id,
                priority: spec.priority,
                enabled: spec.enabled,
                action: spec.action,
                matcher,
                reason: spec.reason,
                impact: spec.impact,
            }))
        }
    }
}

/// `connection: <connector>/<name>` -> `connectionId`.
async fn resolve_connection_field(
    resolver: &Resolver<'_>,
    value: &mut Value,
    path: &str,
    unresolved: &mut Vec<UnresolvedReference>,
) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(reference) = object
        .get("connection")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let before = unresolved.len();
    let id = resolver
        .resolve(
            Ref::Connection,
            &reference,
            &format!("{path}.connection"),
            unresolved,
        )
        .await;
    replace_connection_ref(object, id, unresolved.len() == before);
}

// --------------------------------------------------------------------- planning

fn write_permission(kind: PortableKind) -> Permission {
    match kind {
        PortableKind::Agent | PortableKind::Skill => Permission::AgentsWrite,
        PortableKind::Connection => Permission::ConnectionsWrite,
        PortableKind::Provider => Permission::ProvidersWrite,
        PortableKind::Trigger => Permission::TriggersWrite,
        PortableKind::WorkerProfile | PortableKind::AgentTeam | PortableKind::ToolPolicy => {
            Permission::SettingsWrite
        }
    }
}

fn natural_key(kind: PortableKind, doc: &PortableDocument) -> String {
    match kind {
        PortableKind::Skill => doc
            .spec
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or(&doc.metadata.name)
            .to_string(),
        PortableKind::Connection => format!(
            "{}/{}",
            doc.spec
                .get("connector")
                .and_then(Value::as_str)
                .unwrap_or(""),
            doc.metadata.name
        ),
        PortableKind::ToolPolicy => format!(
            "{}/{}",
            doc.spec.get("agent").and_then(Value::as_str).unwrap_or("*"),
            doc.metadata.name
        ),
        PortableKind::Trigger => doc.metadata.name.clone(),
        _ => doc.metadata.name.clone(),
    }
}

fn provided_ref(kind: PortableKind, doc: &PortableDocument) -> Option<(Ref, String)> {
    let key = natural_key(kind, doc);
    match kind {
        PortableKind::Agent => Some((Ref::Agent, key)),
        PortableKind::Skill => Some((Ref::Skill, key)),
        PortableKind::Connection => Some((Ref::Connection, key)),
        PortableKind::Provider => Some((Ref::Provider, key)),
        PortableKind::WorkerProfile => Some((Ref::WorkerProfile, key)),
        _ => None,
    }
}

async fn existing_id(
    state: &AppState,
    org_id: Uuid,
    kind: PortableKind,
    doc: &PortableDocument,
    translated: &Translated,
) -> Result<Option<Uuid>, ApiError> {
    let store = &state.store;
    Ok(match translated {
        Translated::Agent(_)
        | Translated::Provider(_)
        | Translated::WorkerProfile(_)
        | Translated::Team(_) => {
            store
                .find_resource_id_by_key(org_id, kind.resource(), doc.metadata.name.trim())
                .await?
        }
        Translated::Skill(request) => {
            store
                .find_resource_id_by_key(org_id, "skills", &request.slug)
                .await?
        }
        Translated::Connection(request) => {
            store
                .find_connection_id(org_id, &request.connector_slug, &request.name)
                .await?
        }
        Translated::ToolPolicy(request) => {
            if request.agent_id.is_some_and(|id| id.is_nil()) {
                None
            } else {
                store
                    .find_tool_policy_id(org_id, request.agent_id, &request.name)
                    .await?
            }
        }
        Translated::Trigger(request) => {
            if request.agent_id.is_nil() {
                None
            } else {
                store
                    .list_triggers(org_id, Some(request.agent_id), None)
                    .await?
                    .into_iter()
                    .find(|trigger| {
                        trigger.kind == request.kind && trigger.config == request.config
                    })
                    .map(|trigger| trigger.id)
            }
        }
    })
}

struct Planned {
    kind: PortableKind,
    doc: PortableDocument,
    report: ImportItemReport,
    existing: Option<Uuid>,
}

async fn plan_document(
    state: &AppState,
    auth: &AuthContext,
    access: &AccessScope,
    provided: &HashSet<(Ref, String)>,
    mode: ImportMode,
    doc: PortableDocument,
) -> Result<Planned, ApiError> {
    let kind = doc
        .portable_kind()
        .ok_or_else(|| ApiError::bad_request("unsupported kind"))?;
    auth.require(write_permission(kind))?;

    let resolver = Resolver {
        state,
        org_id: auth.org_id,
        access,
        provided,
        lenient: true,
    };
    let mut found = Findings::default();
    let translated = translate(kind, &doc, &resolver, &mut found).await;
    let credentials_required = doc.credentials.as_ref().is_some_and(|c| c.required);
    let mut report = ImportItemReport {
        kind: kind.as_str().into(),
        name: doc.metadata.name.clone(),
        status: ImportItemStatus::Ok,
        action: None,
        id: None,
        unresolved_references: Vec::new(),
        issues: Vec::new(),
        warnings: Vec::new(),
        credentials_required,
    };
    let mut existing = None;
    if let (Some(translated), true) = (&translated, found.issues.is_empty()) {
        existing = existing_id(state, auth.org_id, kind, &doc, translated).await?;
        match (existing, mode) {
            (Some(_), ImportMode::Create) => {
                report.status = ImportItemStatus::Conflict;
                found.issue(
                    "metadata.name",
                    format!(
                        "a {kind} '{}' already exists; use mode=update or mode=upsert to change it",
                        natural_key(kind, &doc)
                    ),
                );
            }
            (Some(id), ImportMode::Update | ImportMode::Upsert) => {
                // Same permission model as editing the resource.
                let (owner_user, owner_team, visibility) = state
                    .store
                    .resource_ownership(auth.org_id, kind.resource(), id)
                    .await?;
                access.require_manage(kind.resource(), owner_user, owner_team, &visibility)?;
                update_checks(state, auth.org_id, translated, id, &mut found).await?;
                report.action = Some(ImportAction::Update);
            }
            (None, ImportMode::Update) => {
                report.status = ImportItemStatus::Invalid;
                found.issue(
                    "metadata.name",
                    format!(
                        "no existing {kind} '{}' to update; use mode=upsert to create it",
                        natural_key(kind, &doc)
                    ),
                );
            }
            (None, ImportMode::Create | ImportMode::Upsert) => {
                report.action = Some(ImportAction::Create);
            }
        }
    }
    if report.status == ImportItemStatus::Ok {
        if !found.issues.is_empty() || translated.is_none() {
            report.status = ImportItemStatus::Invalid;
        } else if !found.unresolved.is_empty() {
            report.status = ImportItemStatus::UnresolvedReferences;
        }
    }
    if report.status != ImportItemStatus::Ok {
        report.action = None;
    }
    if credentials_required {
        found.warnings.push(format!(
            "credentials are not part of the file: set them on the imported {} after import",
            kind.as_str()
        ));
    }
    report.unresolved_references = found.unresolved;
    report.issues = found.issues;
    report.warnings = found.warnings;
    Ok(Planned {
        kind,
        doc,
        report,
        existing,
    })
}

/// Immutable properties must not change through an update.
async fn update_checks(
    state: &AppState,
    org_id: Uuid,
    translated: &Translated,
    id: Uuid,
    found: &mut Findings,
) -> Result<(), ApiError> {
    match translated {
        Translated::Connection(request) => {
            let row = state
                .store
                .get_connection_with_connector(org_id, id)
                .await?;
            if row.connector_slug != request.connector_slug {
                found.issue(
                    "spec.connector",
                    "the connector of a connection cannot change",
                );
            }
        }
        Translated::Provider(request) => {
            let provider = state.store.get_provider(org_id, id).await?;
            if provider.kind != request.kind {
                found.issue("spec.kind", "the kind of a provider cannot change");
            }
        }
        _ => {}
    }
    Ok(())
}

// ----------------------------------------------------------------------- apply

/// Keep credential-shaped keys that exist on the stored config but are (by design)
/// absent from portable documents, so an update never wipes them.
fn preserve_sensitive_keys(existing: &Value, incoming: &mut Value) {
    if let (Some(existing), Some(incoming)) = (existing.as_object(), incoming.as_object_mut()) {
        for (key, value) in existing {
            if orbit_contracts::portable::sensitive_key(key) && !incoming.contains_key(key) {
                incoming.insert(key.clone(), value.clone());
            }
        }
    }
}

async fn apply_item(
    state: &AppState,
    auth: &AuthContext,
    translated: Translated,
    existing: Option<Uuid>,
) -> Result<(Uuid, ImportAction), ApiError> {
    let s = State(state.clone());
    let a = Extension(auth.clone());
    let action = if existing.is_some() {
        ImportAction::Update
    } else {
        ImportAction::Create
    };
    let id = match (translated, existing) {
        (Translated::Agent(definition), None) => {
            let (_, Json(detail)) =
                crate::handlers::agents::create_agent(s, a, ApiJson(*definition)).await?;
            detail.id
        }
        (Translated::Agent(definition), Some(id)) => {
            let _ = crate::handlers::agents::update_agent(
                s,
                a,
                Path(id),
                ApiJson(UpdateAgentRequest {
                    name: None,
                    description: None,
                    labels: None,
                    definition: Some(*definition),
                    // An import replaces whatever is there; it did not start from a known version.
                    expected_version: None,
                }),
            )
            .await?;
            id
        }
        (Translated::Skill(request), None) => {
            let (_, Json(skill)) =
                crate::handlers::skills::create_skill(s, a, ApiJson(request)).await?;
            skill.id
        }
        (Translated::Skill(request), Some(id)) => {
            let _ = crate::handlers::skills::update_skill(
                s,
                a,
                Path(id),
                ApiJson(UpdateSkillRequest {
                    name: Some(request.name),
                    description: Some(request.description),
                    content: Some(request.content),
                    files: Some(request.files),
                }),
            )
            .await?;
            id
        }
        (Translated::Connection(request), None) => {
            let (_, Json(view)) =
                crate::handlers::catalog::create_connection(s, a, ApiJson(request)).await?;
            view.id
        }
        (Translated::Connection(request), Some(id)) => {
            let current = state.store.get_connection(auth.org_id, id).await?;
            let mut config = request.config;
            preserve_sensitive_keys(&current.config, &mut config);
            let _ = crate::handlers::catalog::update_connection(
                s,
                a,
                Path(id),
                ApiJson(UpdateConnectionRequest {
                    name: None,
                    config: Some(config),
                    scopes: Some(request.scopes),
                    credential: None,
                }),
            )
            .await?;
            id
        }
        (Translated::Provider(request), None) => {
            let enabled = request.enabled.unwrap_or(true);
            let (_, Json(view)) =
                crate::handlers::catalog::create_provider(s.clone(), a.clone(), ApiJson(request))
                    .await?;
            if !enabled {
                let _ = crate::handlers::catalog::update_provider(
                    s,
                    a,
                    Path(view.id),
                    ApiJson(UpdateProviderRequest {
                        name: None,
                        config: None,
                        credential: None,
                        enabled: Some(false),
                    }),
                )
                .await?;
            }
            view.id
        }
        (Translated::Provider(request), Some(id)) => {
            let current = state.store.get_provider(auth.org_id, id).await?;
            let mut config = request.config;
            preserve_sensitive_keys(&current.config, &mut config);
            let _ = crate::handlers::catalog::update_provider(
                s,
                a,
                Path(id),
                ApiJson(UpdateProviderRequest {
                    name: None,
                    config: Some(config),
                    credential: None,
                    enabled: request.enabled,
                }),
            )
            .await?;
            id
        }
        (Translated::WorkerProfile(request), None) => {
            let (_, Json(profile)) =
                crate::handlers::worker_profiles::create_profile(s, a, ApiJson(*request)).await?;
            profile.id
        }
        (Translated::WorkerProfile(request), Some(id)) => {
            let current = state.store.get_worker_profile(auth.org_id, id).await?;
            let mut request = *request;
            // Values of existing variables and the auth secret stay as they are:
            // documents only carry names.
            if let (Some(env), Some(old)) = (request.env.as_object_mut(), current.env.as_object()) {
                for (key, value) in env.iter_mut() {
                    if let Some(existing_value) = old.get(key) {
                        *value = existing_value.clone();
                    }
                }
            }
            request.auth_secret_name = current.auth_secret_name.clone();
            request.env_mappings = current.env_mappings.clone();
            let _ =
                crate::handlers::worker_profiles::update_profile(s, a, Path(id), ApiJson(request))
                    .await?;
            id
        }
        (Translated::Team(request), None) => {
            let (_, Json(team)) =
                crate::handlers::teams::create_team(s, a, ApiJson(*request)).await?;
            team.id
        }
        (Translated::Team(request), Some(id)) => {
            // An import replaces the whole team, unlike a PATCH, which merges.
            let _ = crate::handlers::teams::replace_team(&s, &a, id, *request, None).await?;
            id
        }
        (Translated::Trigger(request), None) => {
            let (_, Json(trigger)) =
                crate::handlers::triggers::create_trigger(s, a, ApiJson(request)).await?;
            trigger.id
        }
        (Translated::Trigger(request), Some(id)) => {
            let _ = crate::handlers::triggers::update_trigger(
                s,
                a,
                Path(id),
                ApiJson(UpdateTriggerRequest {
                    enabled: request.enabled,
                }),
            )
            .await?;
            id
        }
        (Translated::ToolPolicy(request), None) => {
            let (_, Json(policy)) =
                crate::handlers::tool_policies::create(s, a, ApiJson(request)).await?;
            policy.id
        }
        (Translated::ToolPolicy(request), Some(id)) => {
            let _ =
                crate::handlers::tool_policies::update(s, a, Path(id), ApiJson(request)).await?;
            id
        }
    };
    Ok((id, action))
}

// ------------------------------------------------------------------------ route

fn extract_upload(headers: &HeaderMap, body: &Bytes) -> Result<(String, String), ApiError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| ApiError::bad_request("the upload must be UTF-8 text"))?;
    let mut filename = headers
        .get("x-orbit-filename")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("json"));
    if is_json {
        // `{ "content": "...", "filename": "..." }` wrapper (used by the web app).
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) {
            if map.get("apiVersion").is_none() {
                if let Some(content) = map.get("content").and_then(Value::as_str) {
                    if let Some(name) = map.get("filename").and_then(Value::as_str) {
                        filename = name.to_string();
                    }
                    return Ok((filename, content.to_string()));
                }
            }
        }
    }
    Ok((filename, text.to_string()))
}

fn report_issues(report: &ImportReport) -> Vec<ValidationIssue> {
    let mut details = Vec::new();
    for item in report
        .items
        .iter()
        .filter(|i| i.status != ImportItemStatus::Ok)
    {
        let prefix = format!("{} '{}'", item.kind, item.name);
        for issue in &item.issues {
            details.push(ValidationIssue::new(
                format!("{prefix}: {}", issue.path),
                issue.message.clone(),
            ));
        }
        for reference in &item.unresolved_references {
            details.push(ValidationIssue::new(
                format!("{prefix}: {}", reference.path),
                format!(
                    "unresolved {} reference '{}'; import it first or create it",
                    reference.kind, reference.reference
                ),
            ));
        }
    }
    details
}

#[utoipa::path(
    post,
    path = "/api/v1/import",
    tag = "import",
    params(PortableImportQuery),
    request_body(content = String, description = "SKILL.md, a YAML/JSON portable document, or a multi-document bundle. May be wrapped as {\"content\", \"filename\"} JSON.", content_type = "application/yaml"),
    responses(
        (status = 200, description = "Dry-run report, or an import that only updated resources", body = ImportReport),
        (status = 201, description = "Imported", body = ImportReport),
        (status = 409, description = "Name conflict in create mode"),
        (status = 422, description = "Validation errors or unresolved references"),
    )
)]
pub async fn import_portable(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<PortableImportQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<ImportReport>), ApiError> {
    let mode = query.mode.unwrap_or_default();
    let dry_run = query.dry_run.unwrap_or(false);
    let (filename, content) = extract_upload(&headers, &body)?;
    let mut parsed = parse_upload(&content, &filename).map_err(ApiError::validation)?;
    // Dependencies first; stable within a kind.
    parsed.documents.sort_by_key(|doc| {
        doc.portable_kind()
            .map(PortableKind::import_rank)
            .unwrap_or(usize::MAX)
    });

    let access = AccessScope::load(&state, &auth).await?;
    // A document may only rely on documents that sort (and are applied) before it.
    let mut provided: HashSet<(Ref, String)> = HashSet::new();
    let mut planned = Vec::new();
    let mut duplicate_keys = HashSet::new();
    for doc in parsed.documents {
        let provides = doc
            .portable_kind()
            .and_then(|kind| provided_ref(kind, &doc));
        let mut item = plan_document(&state, &auth, &access, &provided, mode, doc).await?;
        provided.extend(provides);
        if !duplicate_keys.insert((item.kind, natural_key(item.kind, &item.doc))) {
            item.report.status = ImportItemStatus::Invalid;
            item.report.action = None;
            item.report.issues.push(ValidationIssue::new(
                "metadata.name",
                "duplicate document in the same bundle",
            ));
        }
        planned.push(item);
    }

    let valid = planned
        .iter()
        .all(|item| item.report.status == ImportItemStatus::Ok);
    let mut report = ImportReport {
        dry_run,
        mode,
        valid,
        applied: false,
        items: planned.iter().map(|item| item.report.clone()).collect(),
    };
    if dry_run {
        return Ok((StatusCode::OK, Json(report)));
    }
    if !valid {
        let details = report_issues(&report);
        let conflict = report
            .items
            .iter()
            .any(|item| item.status == ImportItemStatus::Conflict);
        audit::record(
            &state,
            &auth,
            "import.rejected",
            None,
            None,
            json!({
                "filename": filename,
                "mode": mode,
                "documents": report.items.len(),
                "conflicts": conflict,
                "problems": details.len(),
            }),
        )
        .await;
        let mut error = if conflict {
            ApiError::conflict(
                "import would overwrite existing resources; nothing was imported (use mode=update or mode=upsert)",
            )
        } else {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                "the import has validation errors or unresolved references; nothing was imported",
            )
        };
        error.details = details;
        return Err(error);
    }

    // Apply in dependency order. References created earlier in the bundle now exist.
    let no_provided = HashSet::new();
    let mut created_any = false;
    let mut applied = 0usize;
    let total = planned.len();
    for (index, item) in planned.iter().enumerate() {
        let resolver = Resolver {
            state: &state,
            org_id: auth.org_id,
            access: &access,
            provided: &no_provided,
            lenient: false,
        };
        let mut found = Findings::default();
        let outcome = match translate(item.kind, &item.doc, &resolver, &mut found).await {
            Some(translated) if found.issues.is_empty() && found.unresolved.is_empty() => {
                apply_item(&state, &auth, translated, item.existing).await
            }
            _ => Err(ApiError::validation(
                found
                    .issues
                    .into_iter()
                    .chain(found.unresolved.into_iter().map(|reference| {
                        ValidationIssue::new(
                            reference.path,
                            format!("unresolved {} '{}'", reference.kind, reference.reference),
                        )
                    }))
                    .collect(),
            )),
        };
        match outcome {
            Ok((id, action)) => {
                applied += 1;
                created_any |= action == ImportAction::Create;
                report.items[index].id = Some(id);
                report.items[index].action = Some(action);
                audit::record(
                    &state,
                    &auth,
                    "resource.imported",
                    Some(item.kind.resource()),
                    Some(id),
                    json!({
                        "kind": item.kind.as_str(),
                        "name": item.doc.metadata.name,
                        "action": action,
                        "mode": mode,
                        "filename": filename,
                        "credentialsRequired": item.report.credentials_required,
                    }),
                )
                .await;
            }
            Err(mut error) => {
                audit::record(
                    &state,
                    &auth,
                    "import.failed",
                    Some(item.kind.resource()),
                    None,
                    json!({
                        "kind": item.kind.as_str(),
                        "name": item.doc.metadata.name,
                        "filename": filename,
                        "applied": applied,
                        "total": total,
                        "error": error.message,
                    }),
                )
                .await;
                if applied > 0 {
                    error.message = format!(
                        "{} (imported {applied} of {total} document(s) before this failure; re-run with mode=upsert after fixing)",
                        error.message
                    );
                }
                return Err(error);
            }
        }
    }
    report.applied = true;
    audit::record(
        &state,
        &auth,
        "import.completed",
        None,
        None,
        json!({
            "filename": filename,
            "mode": mode,
            "documents": report.items.iter().map(|item| json!({
                "kind": item.kind, "name": item.name, "action": item.action, "id": item.id,
            })).collect::<Vec<_>>(),
        }),
    )
    .await;
    let status = if created_any {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(report)))
}
