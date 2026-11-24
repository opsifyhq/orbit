//! Portable export/import (#80): round trips, redaction, conflict policy, dry run,
//! audit and authorization. Skipped unless `ORBIT_TEST_DATABASE_URL` is set.
//! Every test creates its own organisations, so tests are isolated from each other.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use orbit_api::{
    build_router, config::DEFAULT_ANON_RATE_LIMIT_PER_MINUTE, rate_limit::RateLimiter, AppState,
    DevIdentity,
};
use orbit_domain::UserRole;
use orbit_events::MemoryEventBus;
use orbit_harness::HarnessRegistry;
use orbit_security::{AuthConfig, AuthMode, DevAuthConfig, SecretCipher};
use orbit_store::Store;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET_VALUES: [&str; 6] = [
    "sk-provider-secret-7731",
    "ghp_connectionsecret9921",
    "supersecretenvvalue",
    "whsec_leaked_in_config",
    "hunter2-webhook-secret",
    "orbit-pi-auth-canary-k8s",
];

struct Env {
    router: axum::Router,
    store: Store,
}

struct Org {
    id: Uuid,
    user_id: Uuid,
    token: String,
}

async fn env() -> Option<Env> {
    let url = std::env::var("ORBIT_TEST_DATABASE_URL").ok()?;
    let key = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
    let cipher = Arc::new(SecretCipher::from_base64_key(&key).unwrap());
    let store = Store::connect(&url, 5)
        .await
        .expect("connect")
        .with_cipher(cipher);
    store.migrate().await.expect("migrate");
    for manifest in orbit_api::connectors::builtin_manifests() {
        store.upsert_connector_manifest(&manifest).await.unwrap();
    }
    store
        .upsert_harness_descriptor(
            &HarnessRegistry::builtin()
                .descriptors()
                .into_iter()
                .find(|d| d.kind == orbit_domain::HarnessKind::Mock)
                .unwrap(),
        )
        .await
        .unwrap();
    let default_org = store
        .create_organization(
            &format!("portable-default-{}", Uuid::now_v7()),
            "Portable default",
            orbit_domain::OrganizationMode::Personal,
        )
        .await
        .unwrap();
    let user = store
        .ensure_user(
            default_org.id,
            "portable-default@test.local",
            "Default",
            UserRole::Owner,
        )
        .await
        .unwrap();
    let auth_config = AuthConfig {
        mode: AuthMode::Dev,
        dev: DevAuthConfig {
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            org_slug: default_org.slug.clone(),
            org_name: default_org.name.clone(),
            role: UserRole::Owner,
            allow_anonymous: false,
            dev_token: "test-token".into(),
        },
        oidc: None,
        allow_api_tokens: true,
    };
    let state = AppState {
        store: store.clone(),
        bus: Arc::new(MemoryEventBus::new(128)),
        registry: Arc::new(HarnessRegistry::builtin()),
        auth: Arc::new(auth_config),
        jwt: None,
        default_org_id: default_org.id,
        dev_identity: Some(Arc::new(DevIdentity {
            user_id: user.id,
            org_id: default_org.id,
            email: user.email.clone(),
            display_name: user.display_name.clone(),
            role: UserRole::Owner,
        })),
        environment: "test".into(),
        events_driver: "memory".into(),
        public_base_url: "http://localhost".into(),
        metrics: Arc::new(orbit_api::metrics::Metrics::new()),
        metrics_access: orbit_api::MetricsAccess::default(),
        max_body_bytes: 2 * 1024 * 1024,
        rate_limiter: Arc::new(RateLimiter::new(DEFAULT_ANON_RATE_LIMIT_PER_MINUTE)),
        cors_origins: vec!["*".into()],
        launcher_url: None,
        launcher_token: None,
        worker_api_url: "http://localhost:8080".into(),
        ca_secret_name: None,
        ca_secret_key: "ca-bundle.crt".into(),
        archive_after_days: None,
        output_delivery_retention_days: None,
        license: orbit_security::LicenseHandle::load(orbit_security::LicenseSource {
            token: None,
            file: None,
        }),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    Some(Env {
        router: build_router(state),
        store,
    })
}

async fn new_org(env: &Env, label: &str, role: UserRole, scopes: &[&str]) -> Org {
    let org = env
        .store
        .create_organization(
            &format!("portable-{label}-{}", Uuid::now_v7()),
            label,
            orbit_domain::OrganizationMode::Personal,
        )
        .await
        .unwrap();
    let user = env
        .store
        .ensure_user(org.id, &format!("{label}@test.local"), label, role)
        .await
        .unwrap();
    let issued = orbit_security::issue_api_token();
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    env.store
        .create_api_token(
            org.id,
            user.id,
            label,
            &issued.prefix,
            &issued.hash,
            &scopes,
            None,
        )
        .await
        .unwrap();
    Org {
        id: org.id,
        user_id: user.id,
        token: issued.token,
    }
}

async fn owner_org(env: &Env, label: &str) -> Org {
    new_org(env, label, UserRole::Owner, &[]).await
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    text: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

async fn call(env: &Env, request: Request<Body>) -> Reply {
    let response = env.router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).to_string(),
    }
}

fn get(org: &Org, path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {}", org.token))
        .body(Body::empty())
        .unwrap()
}

fn post_json(org: &Org, path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {}", org.token))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn import_request(org: &Org, query: &str, content: &str, filename: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/v1/import{query}"))
        .header("authorization", format!("Bearer {}", org.token))
        .header("content-type", "application/yaml")
        .header("x-orbit-filename", filename)
        .body(Body::from(content.to_string()))
        .unwrap()
}

async fn create(env: &Env, org: &Org, path: &str, body: Value) -> Value {
    let reply = call(env, post_json(org, path, body)).await;
    assert!(
        reply.status.is_success(),
        "POST {path} -> {}: {}",
        reply.status,
        reply.text
    );
    reply.json()
}

async fn export_yaml(env: &Env, org: &Org, resource: &str, id: &str) -> String {
    let reply = call(
        env,
        get(org, &format!("/api/v1/export/{resource}/{id}?format=yaml")),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    reply.text
}

struct Seeded {
    /// (resource, id)
    ids: Vec<(&'static str, String)>,
}

/// One of every shareable kind, carrying secrets in every place a secret can live.
async fn seed(env: &Env, org: &Org, suffix: &str) -> Seeded {
    let provider = create(
        env,
        org,
        "/api/v1/providers",
        json!({
            "name": format!("Gateway {suffix}"),
            "kind": "openai_compatible",
            "config": { "baseUrl": "http://localhost:9999/v1", "model": "qwen", "maxOutputTokens": 4096 },
            "credential": SECRET_VALUES[0],
        }),
    )
    .await;
    let skill = create(
        env,
        org,
        "/api/v1/skills",
        json!({
            "name": format!("Docs writer {suffix}"),
            "slug": format!("docs-{suffix}"),
            "description": "Writes docs",
            "content": "# Docs\n\nWrite clear docs.\n",
            "files": { "scripts/lint.sh": "#!/bin/sh\necho ok\n", "references/style.md": "Be brief.\n" },
        }),
    )
    .await;
    let connection = create(
        env,
        org,
        "/api/v1/connections",
        json!({
            "connectorSlug": "github",
            "name": format!("gh-{suffix}"),
            "credential": SECRET_VALUES[1],
            "config": { "owner": "acme", "webhookSecret": SECRET_VALUES[3] },
            "scopes": ["repo:read"],
        }),
    )
    .await;
    let agent = create(
        env,
        org,
        "/api/v1/agents",
        json!({
            "apiVersion": "orbit.io/v1",
            "kind": "Agent",
            "metadata": { "name": format!("Docs agent {suffix}"), "description": "d", "labels": { "team": "docs" } },
            "spec": {
                "model": { "management": "harness", "params": { "max_tokens": 1024 } },
                "instructions": { "system": "You are a docs agent." },
                "connectors": [{
                    "connector": "github",
                    "connectionId": connection["id"],
                    "capabilities": []
                }],
                "skills": [format!("docs-{suffix}")],
                "triggers": [{ "type": "webhook", "secret_ref": SECRET_VALUES[4] }],
                "autonomy": { "level": 2 },
                "resources": { "timeoutSeconds": 30, "maxConcurrentExecutions": 1 }
            }
        }),
    )
    .await;
    let profile = create(
        env,
        org,
        "/api/v1/worker-profiles",
        json!({
            "name": format!("profile-{suffix}"),
            "harnessKind": "mock",
            "target": "docker",
            "executionDriver": "process",
            "agentIds": [agent["id"]],
            "skillSlugs": [format!("docs-{suffix}")],
            "provider": format!("Gateway {suffix}"),
            "authSecretName": SECRET_VALUES[5],
            "env": { "API_TOKEN": SECRET_VALUES[2], "LOG_LEVEL": "debug" },
        }),
    )
    .await;
    let team = create(
        env,
        org,
        "/api/v1/agent-teams",
        json!({
            "name": format!("Docs team {suffix}"),
            "description": "pipeline",
            "mode": "pipeline",
            "instructions": "Ship the docs.",
            "steps": [{ "agentId": agent["id"] }],
            "skills": [format!("docs-{suffix}")],
            "workerProfileId": profile["id"],
            "connectionIds": [connection["id"]],
        }),
    )
    .await;
    let trigger = create(
        env,
        org,
        "/api/v1/triggers",
        json!({
            "agentId": agent["id"],
            "kind": "schedule",
            "config": { "cron": "0 9 * * 1" },
        }),
    )
    .await;
    let policy = create(
        env,
        org,
        "/api/v1/tool-policies",
        json!({
            "name": format!("no-rm {suffix}"),
            "priority": 10,
            "action": "deny",
            "matcher": { "tool": "bash", "commandRegex": "rm -rf" },
            "reason": "destructive",
        }),
    )
    .await;
    let id = |value: &Value| value["id"].as_str().unwrap().to_string();
    Seeded {
        ids: vec![
            ("providers", id(&provider)),
            ("skills", id(&skill)),
            ("connections", id(&connection)),
            ("agents", id(&agent)),
            ("worker_profiles", id(&profile)),
            ("agent_teams", id(&team)),
            ("triggers", id(&trigger)),
            ("tool_policies", id(&policy)),
        ],
    }
}

async fn export_bundle(env: &Env, org: &Org, seeded: &Seeded) -> Vec<String> {
    let mut documents = Vec::new();
    for (resource, id) in &seeded.ids {
        documents.push(export_yaml(env, org, resource, id).await);
    }
    documents
}

fn parse_docs(text: &str) -> Vec<Value> {
    use serde::Deserialize;
    serde_yaml::Deserializer::from_str(text)
        .map(|doc| {
            let value = serde_yaml::Value::deserialize(doc).unwrap();
            serde_json::to_value(value).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn exports_for_every_kind_contain_no_secret_values_or_refs() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "export-a").await;
    let seeded = seed(&env, &org, "a").await;
    let documents = export_bundle(&env, &org, &seeded).await;
    assert_eq!(documents.len(), 8);
    for (index, text) in documents.iter().enumerate() {
        let resource = seeded.ids[index].0;
        for secret in SECRET_VALUES {
            assert!(!text.contains(secret), "{resource} export leaks {secret}");
        }
        let lower = text.to_ascii_lowercase();
        for forbidden in [
            "secretref",
            "secret_ref",
            "orbit_sk_",
            "orbit_wk_",
            "credentialref",
        ] {
            assert!(
                !lower.contains(forbidden),
                "{resource} export contains {forbidden}"
            );
        }
        assert!(
            text.contains("apiVersion: orbit.io/v1"),
            "{resource}: {text}"
        );
        // The legacy JSON format is redacted too.
        let (resource, id) = &seeded.ids[index];
        let reply = call(&env, get(&org, &format!("/api/v1/export/{resource}/{id}"))).await;
        assert_eq!(reply.status, StatusCode::OK);
        for secret in SECRET_VALUES {
            assert!(
                !reply.text.contains(secret),
                "{resource} json leaks {secret}"
            );
        }
    }
    // The dedicated agent.yaml route serves the same secret-free document.
    let agent_id = &seeded.ids[3].1;
    let reply = call(
        &env,
        get(&org, &format!("/api/v1/agents/{agent_id}/export")),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    for secret in SECRET_VALUES {
        assert!(!reply.text.contains(secret), "agent.yaml leaks {secret}");
    }
    assert!(!reply.text.contains("connectionId"));
    assert!(
        reply.text.contains("connection: github/gh-a"),
        "{}",
        reply.text
    );
    // Credential-bearing kinds say so; non-secret model params survive.
    let connection = &documents[2];
    assert!(connection.contains("credentials:"), "{connection}");
    assert!(connection.contains("required: true"));
    assert!(documents[0].contains("maxOutputTokens: 4096"));
    assert!(documents[3].contains("max_tokens: 1024"));
    // Env var names are kept, values are placeholders.
    assert!(documents[4].contains("API_TOKEN: <set-on-import>"));
    assert!(documents[4].contains("LOG_LEVEL: <set-on-import>"));
}

#[tokio::test]
async fn round_trip_all_kinds_into_a_fresh_org() {
    let Some(env) = env().await else { return };
    let source = owner_org(&env, "rt-source").await;
    let seeded = seed(&env, &source, "rt").await;
    let documents = export_bundle(&env, &source, &seeded).await;

    // Whole bundle at once, in an arbitrary (reversed) order: dependencies are
    // sorted and references between documents resolve within the bundle.
    let bundle: Vec<&str> = documents.iter().rev().map(String::as_str).collect();
    let bundle = bundle.join("---\n");
    let target = owner_org(&env, "rt-target").await;

    // Dry run first: valid, nothing written.
    let dry = call(
        &env,
        import_request(&target, "?dryRun=true", &bundle, "bundle.yaml"),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = dry.json();
    assert_eq!(report["valid"], true, "{report}");
    assert_eq!(report["applied"], false);
    assert_eq!(report["items"].as_array().unwrap().len(), 8);
    assert!(env
        .store
        .list_skills(target.id, None)
        .await
        .unwrap()
        .is_empty());

    let reply = call(&env, import_request(&target, "", &bundle, "bundle.yaml")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let report = reply.json();
    assert_eq!(report["applied"], true);
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 8);
    assert!(items.iter().all(|item| item["action"] == "create"));

    // Export everything again from the target and compare to the source documents.
    for (index, (resource, _)) in seeded.ids.iter().enumerate() {
        let kind = match *resource {
            "providers" => "Provider",
            "skills" => "Skill",
            "connections" => "Connection",
            "agents" => "Agent",
            "worker_profiles" => "WorkerProfile",
            "agent_teams" => "AgentTeam",
            "triggers" => "Trigger",
            _ => "ToolPolicy",
        };
        let item = items
            .iter()
            .find(|item| item["kind"] == kind)
            .unwrap_or_else(|| panic!("no report item for {kind}"));
        let new_id = item["id"].as_str().expect("id after import");
        let again = export_yaml(&env, &target, resource, new_id).await;
        // `credentials` is a notice about the *source* resource (imports never carry
        // credentials), so it is the one block excluded from the comparison.
        let strip = |text: &str| {
            let mut docs = parse_docs(text);
            for doc in &mut docs {
                doc.as_object_mut().unwrap().remove("credentials");
            }
            docs
        };
        assert_eq!(
            strip(&again),
            strip(&documents[index]),
            "{kind} did not round-trip\n--- original\n{}\n--- imported\n{again}",
            documents[index]
        );
    }

    // Imported resources are owned by the importer and credentials were not carried over.
    let connection_id = items.iter().find(|i| i["kind"] == "Connection").unwrap()["id"]
        .as_str()
        .unwrap();
    let view = call(&env, get(&target, "/api/v1/connections")).await.json();
    let imported = view
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == connection_id)
        .unwrap();
    assert_eq!(imported["hasCredential"], false);
    let (owner, _, _) = env
        .store
        .resource_ownership(target.id, "connections", connection_id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(owner, Some(target.user_id));
}

#[tokio::test]
async fn skill_markdown_export_and_import() {
    let Some(env) = env().await else { return };
    let source = owner_org(&env, "md-source").await;
    let skill = create(
        &env,
        &source,
        "/api/v1/skills",
        json!({
            "name": "Release notes",
            "slug": "release-notes",
            "description": "Drafts release notes",
            "content": "# Release notes\n\nSummarise merged PRs.\n",
            "files": { "references/tone.md": "Friendly." },
        }),
    )
    .await;
    let id = skill["id"].as_str().unwrap();
    let reply = call(
        &env,
        get(
            &source,
            &format!("/api/v1/export/skills/{id}?format=markdown"),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(
        reply.text.starts_with("---\nname: Release notes\n"),
        "{}",
        reply.text
    );
    assert!(reply
        .headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("SKILL.md"));
    assert_eq!(reply.headers.get("x-orbit-omitted-files").unwrap(), "1");

    let target = owner_org(&env, "md-target").await;
    let imported = call(&env, import_request(&target, "", &reply.text, "SKILL.md")).await;
    assert_eq!(imported.status, StatusCode::CREATED, "{}", imported.text);
    let stored = env
        .store
        .get_skill_by_slug(target.id, "release-notes")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.name, "Release notes");
    assert_eq!(stored.description, "Drafts release notes");
    assert_eq!(stored.content, "# Release notes\n\nSummarise merged PRs.\n");
}

const SKILL_DOC: &str = "apiVersion: orbit.io/v1\nkind: Skill\nmetadata:\n  name: Triage\n  description: v1\nspec:\n  slug: triage\n  content: |\n    Triage issues.\n";

#[tokio::test]
async fn conflicts_are_409_without_mode_and_update_requires_opt_in() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "conflict").await;
    let first = call(&env, import_request(&org, "", SKILL_DOC, "triage.yaml")).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text);

    // Default mode never overwrites.
    let changed = SKILL_DOC
        .replace("description: v1", "description: v2")
        .replace("Triage issues.", "Triage faster.");
    let conflict = call(&env, import_request(&org, "", &changed, "triage.yaml")).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.text);
    let body = conflict.json();
    assert_eq!(body["error"]["code"], "conflict");
    assert!(body["error"]["requestId"].is_string());
    assert!(body["error"]["details"][0]["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
    let stored = env
        .store
        .get_skill_by_slug(org.id, "triage")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.description, "v1");

    // The dry run reports the same conflict with 200.
    let dry = call(
        &env,
        import_request(&org, "?dryRun=true", &changed, "triage.yaml"),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK);
    assert_eq!(dry.json()["valid"], false);
    assert_eq!(dry.json()["items"][0]["status"], "conflict");

    // Explicit update.
    let updated = call(
        &env,
        import_request(&org, "?mode=update", &changed, "triage.yaml"),
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.text);
    assert_eq!(updated.json()["items"][0]["action"], "update");
    let stored = env
        .store
        .get_skill_by_slug(org.id, "triage")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.description, "v2");
    assert_eq!(stored.content, "Triage faster.\n");
    assert_eq!(stored.version, 2);

    // update never creates; upsert does.
    let other = SKILL_DOC
        .replace("triage", "other")
        .replace("Triage", "Other");
    let missing = call(&env, import_request(&org, "?mode=update", &other, "o.yaml")).await;
    assert_eq!(
        missing.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        missing.text
    );
    assert!(env
        .store
        .get_skill_by_slug(org.id, "other")
        .await
        .unwrap()
        .is_none());
    let upsert = call(&env, import_request(&org, "?mode=upsert", &other, "o.yaml")).await;
    assert_eq!(upsert.status, StatusCode::CREATED, "{}", upsert.text);
    assert!(env
        .store
        .get_skill_by_slug(org.id, "other")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn unresolved_references_are_reported_and_nothing_is_written() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "unresolved").await;
    let connection = create(
        &env,
        &org,
        "/api/v1/connections",
        json!({
            "connectorSlug": "github",
            "name": "Existing connection",
            "credential": SECRET_VALUES[1],
            "config": { "owner": "acme" }
        }),
    )
    .await;
    let connection_id = connection["id"].as_str().unwrap();
    let agent = format!(
        "apiVersion: orbit.io/v1\nkind: Agent\nmetadata:\n  name: Needs things\nspec:\n  model:\n    management: harness\n  instructions:\n    system: hi\n  connectors:\n    - connector: no-such-connector\n      connectionId: {connection_id}\n    - connector: github\n      connectionId: {connection_id}\n      connection: github/missing-connection\n  skills: [ghost-skill]\n"
    );
    let dry = call(&env, import_request(&org, "?dryRun=true", &agent, "a.yaml")).await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = dry.json();
    assert_eq!(report["valid"], false);
    let item = &report["items"][0];
    assert_eq!(item["status"], "unresolved_references");
    let refs: Vec<(String, String)> = item["unresolvedReferences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["reference"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for expected in [
        ("connector", "no-such-connector"),
        ("connection", "github/missing-connection"),
        ("skill", "ghost-skill"),
    ] {
        assert!(
            refs.contains(&(expected.0.to_string(), expected.1.to_string())),
            "{refs:?}"
        );
    }

    // A real import is rejected as a whole with the error envelope.
    let real = call(&env, import_request(&org, "", &agent, "a.yaml")).await;
    assert_eq!(
        real.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        real.text
    );
    assert_eq!(real.json()["error"]["code"], "validation_error");
    assert!(real.json()["error"]["details"].as_array().unwrap().len() >= 3);
    let agents = call(&env, get(&org, "/api/v1/agents")).await.json();
    assert_eq!(agents["total"], 0);

    // In a bundle that also provides the skill, the skill reference resolves
    // but the missing connector is still reported and nothing is created.
    let bundle = format!("{SKILL_DOC}---\n{}", agent.replace("ghost-skill", "triage"));
    let dry = call(
        &env,
        import_request(&org, "?dryRun=true", &bundle, "b.yaml"),
    )
    .await;
    let report = dry.json();
    assert_eq!(report["items"][0]["kind"], "Skill");
    assert_eq!(report["items"][0]["status"], "ok");
    let agent_refs = report["items"][1]["unresolvedReferences"]
        .as_array()
        .unwrap();
    assert!(agent_refs.iter().all(|r| r["reference"] != "triage"));
    let rejected = call(&env, import_request(&org, "", &bundle, "b.yaml")).await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(env
        .store
        .get_skill_by_slug(org.id, "triage")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn portable_import_rejects_connectors_without_connections() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "connector-required").await;
    let agent = "apiVersion: orbit.io/v1\nkind: Agent\nmetadata:\n  name: Missing connection agent\nspec:\n  instructions:\n    system: hi\n  connectors:\n    - connector: github\n      capabilities: [issues.list]\n";

    let dry = call(&env, import_request(&org, "?dryRun=true", agent, "a.yaml")).await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = dry.json();
    assert_eq!(report["valid"], false);
    let item = &report["items"][0];
    assert_eq!(item["status"], "invalid");
    assert!(item["issues"].as_array().unwrap().iter().any(|issue| {
        issue["path"] == "spec.connectors[0].connectionId"
            && issue["message"].as_str().unwrap().contains("connectionId")
    }));

    let real = call(&env, import_request(&org, "", agent, "a.yaml")).await;
    assert_eq!(
        real.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        real.text
    );
    let body = real.json();
    assert_eq!(body["error"]["code"], "validation_error");
    assert!(body["error"]["requestId"].is_string());
    assert!(body["error"]["details"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["path"]
            .as_str()
            .unwrap()
            .ends_with("spec.connectors[0].connectionId")));
    let agents = call(&env, get(&org, "/api/v1/agents")).await.json();
    assert_eq!(agents["total"], 0);
}

#[tokio::test]
async fn dry_run_makes_no_changes_and_is_not_audited_as_an_import() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "dry").await;
    let dry = call(
        &env,
        import_request(&org, "?dryRun=true", SKILL_DOC, "t.yaml"),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = dry.json();
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["valid"], true);
    assert_eq!(report["applied"], false);
    assert_eq!(report["items"][0]["action"], "create");
    assert!(report["items"][0]["id"].is_null());
    assert!(env
        .store
        .get_skill_by_slug(org.id, "triage")
        .await
        .unwrap()
        .is_none());
    let imported: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE org_id = $1 AND action IN ('resource.imported', 'import.completed')",
    )
    .bind(org.id)
    .fetch_one(env.store.pool())
    .await
    .unwrap();
    assert_eq!(imported, 0);
}

#[tokio::test]
async fn imports_are_audited() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "audit").await;
    let ok = call(&env, import_request(&org, "", SKILL_DOC, "triage.yaml")).await;
    assert_eq!(ok.status, StatusCode::CREATED);
    let conflict = call(&env, import_request(&org, "", SKILL_DOC, "triage.yaml")).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_logs WHERE org_id = $1 ORDER BY created_at")
            .bind(org.id)
            .fetch_all(env.store.pool())
            .await
            .unwrap();
    for expected in [
        "skill.created",
        "resource.imported",
        "import.completed",
        "import.rejected",
    ] {
        assert!(
            actions.iter().any(|a| a == expected),
            "{expected} missing in {actions:?}"
        );
    }
    let (email, metadata): (Option<String>, Value) = sqlx::query_as(
        "SELECT actor_email, metadata FROM audit_logs WHERE org_id = $1 AND action = 'resource.imported'",
    )
    .bind(org.id)
    .fetch_one(env.store.pool())
    .await
    .unwrap();
    assert_eq!(email.as_deref(), Some("audit@test.local"));
    assert_eq!(metadata["kind"], "Skill");
    assert_eq!(metadata["action"], "create");
    assert_eq!(metadata["filename"], "triage.yaml");
}

#[tokio::test]
async fn import_requires_the_permission_to_create_the_resource() {
    let Some(env) = env().await else { return };
    let connection_doc = "apiVersion: orbit.io/v1\nkind: Connection\nmetadata:\n  name: gh\nspec:\n  connector: github\n";

    // A viewer cannot import anything, not even to validate.
    let viewer = new_org(&env, "authz-viewer", UserRole::Viewer, &[]).await;
    for query in ["", "?dryRun=true"] {
        let reply = call(&env, import_request(&viewer, query, SKILL_DOC, "s.yaml")).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text);
        assert_eq!(reply.json()["error"]["code"], "forbidden");
    }
    assert!(env
        .store
        .get_skill_by_slug(viewer.id, "triage")
        .await
        .unwrap()
        .is_none());

    // A token scoped to agents:write may import skills but not connections.
    let scoped = new_org(
        &env,
        "authz-scoped",
        UserRole::Owner,
        &["agents:write", "agents:read"],
    )
    .await;
    let skill = call(&env, import_request(&scoped, "", SKILL_DOC, "s.yaml")).await;
    assert_eq!(skill.status, StatusCode::CREATED, "{}", skill.text);
    let connection = call(&env, import_request(&scoped, "", connection_doc, "c.yaml")).await;
    assert_eq!(
        connection.status,
        StatusCode::FORBIDDEN,
        "{}",
        connection.text
    );

    // Updating requires managing the existing resource: a second operator who does
    // not own a private skill gets a not-found style refusal.
    let owner = owner_org(&env, "authz-owner").await;
    assert_eq!(
        call(&env, import_request(&owner, "", SKILL_DOC, "s.yaml"))
            .await
            .status,
        StatusCode::CREATED
    );
    let skill_id = env
        .store
        .get_skill_by_slug(owner.id, "triage")
        .await
        .unwrap()
        .unwrap()
        .id;
    let other_user = env
        .store
        .ensure_user(
            owner.id,
            "authz-other@test.local",
            "Other",
            UserRole::Operator,
        )
        .await
        .unwrap();
    env.store
        .set_resource_visibility(owner.id, "skills", skill_id, "private", None)
        .await
        .unwrap();
    let issued = orbit_security::issue_api_token();
    env.store
        .create_api_token(
            owner.id,
            other_user.id,
            "other",
            &issued.prefix,
            &issued.hash,
            &[],
            None,
        )
        .await
        .unwrap();
    let other = Org {
        id: owner.id,
        user_id: other_user.id,
        token: issued.token,
    };
    let reply = call(
        &env,
        import_request(
            &other,
            "?mode=update",
            &SKILL_DOC.replace("description: v1", "description: hijack"),
            "s.yaml",
        ),
    )
    .await;
    assert!(
        reply.status == StatusCode::NOT_FOUND || reply.status == StatusCode::FORBIDDEN,
        "{} {}",
        reply.status,
        reply.text
    );
    let stored = env
        .store
        .get_skill_by_slug(owner.id, "triage")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.description, "v1");
}

#[tokio::test]
async fn credentials_in_documents_are_never_imported() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "smuggle").await;
    let connection = "apiVersion: orbit.io/v1\nkind: Connection\nmetadata:\n  name: sneaky\nspec:\n  connector: github\n  config:\n    owner: acme\n    apiKey: ghp_smuggled_token_000\n    webhookUrl: https://hooks.slack.com/services/T/B/X\n";
    let reply = call(&env, import_request(&org, "", connection, "c.yaml")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let report = reply.json();
    let warnings = report["items"][0]["warnings"].to_string();
    assert!(warnings.contains("spec.config.apiKey"), "{warnings}");
    assert!(warnings.contains("spec.config.webhookUrl"), "{warnings}");
    let id: Uuid = report["items"][0]["id"].as_str().unwrap().parse().unwrap();
    let stored = env.store.get_connection(org.id, id).await.unwrap();
    assert_eq!(stored.config, json!({ "owner": "acme" }));
    assert!(stored.credential_ref.is_none());

    // A smuggled webhook secret reference on an agent trigger is dropped as well.
    let agent = "apiVersion: orbit.io/v1\nkind: Agent\nmetadata:\n  name: Hooked\nspec:\n  model:\n    management: harness\n  instructions:\n    system: hi\n  triggers:\n    - type: webhook\n      secret_ref: some-other-secret\n";
    let reply = call(&env, import_request(&org, "", agent, "a.yaml")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let agent_id: Uuid = reply.json()["items"][0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let agent_row = env.store.get_agent(org.id, agent_id).await.unwrap();
    let version = env
        .store
        .get_current_version(org.id, &agent_row)
        .await
        .unwrap();
    assert!(!version.spec.to_string().contains("some-other-secret"));
}

#[tokio::test]
async fn update_keeps_existing_env_values_and_validation_errors_are_reported() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "profile-update").await;
    let profile = create(
        &env,
        &org,
        "/api/v1/worker-profiles",
        json!({
            "name": "keep-env",
            "harnessKind": "mock",
            "env": { "API_TOKEN": SECRET_VALUES[2], "OLD": "x" },
            "authSecretName": SECRET_VALUES[5],
        }),
    )
    .await;
    let id: Uuid = profile["id"].as_str().unwrap().parse().unwrap();
    let doc = "apiVersion: orbit.io/v1\nkind: WorkerProfile\nmetadata:\n  name: keep-env\nspec:\n  harnessKind: mock\n  env:\n    API_TOKEN: <set-on-import>\n    NEW_VAR: <set-on-import>\n  allowGh: true\n";
    let reply = call(&env, import_request(&org, "?mode=update", doc, "p.yaml")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let stored = env.store.get_worker_profile(org.id, id).await.unwrap();
    assert_eq!(
        stored.env["API_TOKEN"], SECRET_VALUES[2],
        "existing value kept"
    );
    assert_eq!(stored.env["NEW_VAR"], "");
    assert!(stored.env.get("OLD").is_none());
    assert_eq!(stored.auth_secret_name.as_deref(), Some(SECRET_VALUES[5]));
    assert!(stored.allow_gh);

    let bad = "apiVersion: orbit.io/v1\nkind: WorkerProfile\nmetadata:\n  name: bad\nspec:\n  harnessKind: nope\n  target: mars\n";
    let dry = call(&env, import_request(&org, "?dryRun=true", bad, "b.yaml")).await;
    let item = &dry.json()["items"][0];
    assert_eq!(item["status"], "invalid");
    assert!(item["issues"].as_array().unwrap().len() >= 2);

    let wrong_kind = "apiVersion: orbit.io/v1\nkind: Banana\nmetadata:\n  name: x\n";
    let reply = call(&env, import_request(&org, "", wrong_kind, "x.yaml")).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.json()["error"]["code"], "validation_error");
}

#[tokio::test]
async fn yaml_aliases_are_rejected_before_parsing() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "alias").await;
    let aliased = "apiVersion: orbit.io/v1\nkind: Skill\nmetadata: &meta\n  name: Alias\nspec:\n  slug: alias\n  content: hello\n";
    let reply = call(&env, import_request(&org, "", aliased, "alias.yaml")).await;
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        reply.text
    );
    assert!(reply.text.contains("aliases"), "{}", reply.text);
}

#[tokio::test]
async fn json_wrapper_and_json_envelope_are_accepted() {
    let Some(env) = env().await else { return };
    let org = owner_org(&env, "json").await;
    let wrapped = call(
        &env,
        post_json(
            &org,
            "/api/v1/import",
            json!({ "content": SKILL_DOC, "filename": "triage.yaml" }),
        ),
    )
    .await;
    assert_eq!(wrapped.status, StatusCode::CREATED, "{}", wrapped.text);

    let envelope = json!({
        "apiVersion": "orbit.io/v1",
        "kind": "Skill",
        "metadata": { "name": "Plain JSON" },
        "spec": { "slug": "plain-json", "content": "hello" }
    });
    let reply = call(&env, post_json(&org, "/api/v1/import", envelope)).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    assert!(env
        .store
        .get_skill_by_slug(org.id, "plain-json")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn re_importing_a_bundle_upserts_every_kind_and_create_mode_conflicts() {
    let Some(env) = env().await else { return };
    let source = owner_org(&env, "upsert-source").await;
    let seeded = seed(&env, &source, "up").await;
    let bundle = export_bundle(&env, &source, &seeded).await.join("---\n");
    let target = owner_org(&env, "upsert-target").await;
    let first = call(&env, import_request(&target, "", &bundle, "b.yaml")).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text);

    let conflict = call(&env, import_request(&target, "", &bundle, "b.yaml")).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.text);
    let details = conflict.json()["error"]["details"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(details, 8, "every document conflicts: {}", conflict.text);

    let again = call(
        &env,
        import_request(&target, "?mode=upsert", &bundle, "b.yaml"),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text);
    let report = again.json();
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 8);
    assert!(
        items.iter().all(|item| item["action"] == "update"),
        "{report}"
    );
    assert!(items.iter().all(|item| item["id"].is_string()));
    // Same ids as the first import: nothing was duplicated.
    let first_ids: Vec<Value> = first.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].clone())
        .collect();
    let second_ids: Vec<Value> = items.iter().map(|item| item["id"].clone()).collect();
    assert_eq!(first_ids, second_ids);
    let agents = call(&env, get(&target, "/api/v1/agents")).await.json();
    assert_eq!(agents["total"], 1);
}

#[tokio::test]
async fn export_does_not_name_resources_the_caller_cannot_view() {
    let Some(env) = env().await else { return };
    let owner = owner_org(&env, "names-owner").await;
    let seeded = seed(&env, &owner, "names").await;
    let id_of = |resource: &str| {
        seeded
            .ids
            .iter()
            .find(|(kind, _)| *kind == resource)
            .map(|(_, id)| id.clone())
            .unwrap()
    };
    let (agent, connection, profile, team, trigger) = (
        id_of("agents"),
        id_of("connections"),
        id_of("worker_profiles"),
        id_of("agent_teams"),
        id_of("triggers"),
    );
    // The team and trigger are shared; the agent, connection and profile they
    // reference stay private to the owner.
    for (resource, id, visibility) in [
        ("agents", &agent, "private"),
        ("connections", &connection, "private"),
        ("worker_profiles", &profile, "private"),
        ("agent_teams", &team, "org"),
        ("triggers", &trigger, "org"),
    ] {
        env.store
            .set_resource_visibility(owner.id, resource, id.parse().unwrap(), visibility, None)
            .await
            .unwrap();
    }
    let operator_user = env
        .store
        .ensure_user(
            owner.id,
            "names-operator@test.local",
            "Operator",
            UserRole::Operator,
        )
        .await
        .unwrap();
    let issued = orbit_security::issue_api_token();
    env.store
        .create_api_token(
            owner.id,
            operator_user.id,
            "operator",
            &issued.prefix,
            &issued.hash,
            &[],
            None,
        )
        .await
        .unwrap();
    let operator = Org {
        id: owner.id,
        user_id: operator_user.id,
        token: issued.token,
    };

    let team_yaml = export_yaml(&env, &operator, "agent_teams", &team).await;
    for hidden in ["Docs agent names", "gh-names", "profile-names"] {
        assert!(!team_yaml.contains(hidden), "{hidden} leaked:\n{team_yaml}");
    }
    for id in [&agent, &connection, &profile] {
        assert!(
            team_yaml.contains(id.as_str()),
            "{id} missing:\n{team_yaml}"
        );
    }
    let trigger_yaml = export_yaml(&env, &operator, "triggers", &trigger).await;
    assert!(!trigger_yaml.contains("Docs agent names"), "{trigger_yaml}");
    assert!(trigger_yaml.contains(agent.as_str()), "{trigger_yaml}");

    // The owner still gets names.
    let owner_yaml = export_yaml(&env, &owner, "agent_teams", &team).await;
    for name in ["Docs agent names", "github/gh-names", "profile-names"] {
        assert!(owner_yaml.contains(name), "{name} missing:\n{owner_yaml}");
    }

    // An id-only reference imports as an unresolved reference, not an error.
    let elsewhere = owner_org(&env, "names-elsewhere").await;
    let dry = call(
        &env,
        import_request(&elsewhere, "?dryRun=true", &team_yaml, "team.yaml"),
    )
    .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    let report = dry.json();
    assert_eq!(report["valid"], false, "{report}");
    let references: Vec<String> = report["items"][0]["unresolvedReferences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["reference"].as_str().unwrap().to_string())
        .collect();
    assert!(references.contains(&agent), "{report}");
}

/// #9: a workflow team exports its graph with agent names and imports it again with the
/// target organization's agent ids.
#[tokio::test]
async fn a_workflow_team_round_trips_with_its_graph() {
    let Some(env) = env().await else { return };
    let source = owner_org(&env, "wf-source").await;
    let seeded = seed(&env, &source, "wf").await;
    let agent_id = &seeded
        .ids
        .iter()
        .find(|(kind, _)| *kind == "agents")
        .unwrap()
        .1;
    let team = create(
        &env,
        &source,
        "/api/v1/agent-teams",
        json!({
            "name": "Gate wf",
            "mode": "workflow",
            "instructions": "Ship it.",
            "workflow": {
                "nodes": [
                    { "id": "test", "name": "Test", "agentId": agent_id, "outcomes": ["pass", "fail"], "maxRuns": 2 },
                    { "id": "fix", "name": "Fix", "agentId": agent_id, "instructions": "Fix it." },
                ],
                "edges": [
                    { "id": "l1", "from": "start", "to": "test", "when": "always" },
                    { "id": "l2", "from": "test", "to": "fix", "when": "outcome", "outcome": "fail" },
                    { "id": "l3", "from": "fix", "to": "test", "when": "success" },
                ],
            },
        }),
    )
    .await;
    let yaml = export_yaml(&env, &source, "agent_teams", team["id"].as_str().unwrap()).await;
    let doc = &parse_docs(&yaml)[0];
    assert_eq!(doc["spec"]["mode"], "workflow");
    let node = &doc["spec"]["workflow"]["nodes"][0];
    assert!(node.get("agentId").is_none(), "ids do not travel: {yaml}");
    assert!(node["agent"].is_string(), "{yaml}");

    // Import the agent and what it needs, then the team, into a fresh organization.
    let target = owner_org(&env, "wf-target").await;
    let mut bundle = export_bundle(&env, &source, &seeded).await;
    bundle.push(yaml);
    let reply = call(
        &env,
        import_request(&target, "", &bundle.join("---\n"), "b.yaml"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    let teams = call(&env, get(&target, "/api/v1/agent-teams")).await.json();
    let imported = teams
        .as_array()
        .unwrap()
        .iter()
        .find(|team| team["name"] == "Gate wf")
        .expect("workflow team imported");
    let agents = call(&env, get(&target, "/api/v1/agents")).await.json();
    let target_agent = agents["items"][0]["id"].clone();
    let workflow = &imported["definition"]["workflow"];
    assert_eq!(workflow["nodes"][0]["agentId"], target_agent);
    assert_eq!(workflow["nodes"][0]["outcomes"], json!(["pass", "fail"]));
    assert_eq!(workflow["nodes"][0]["maxRuns"], 2);
    assert_eq!(workflow["edges"].as_array().unwrap().len(), 3);
    assert_eq!(
        imported["definition"]["steps"],
        json!([{ "agentId": target_agent, "model": null }])
    );
}
