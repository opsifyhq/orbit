use std::sync::Arc;

use base64::Engine;
use orbit_domain::{
    AgentDefinition, AgentMetadata, AgentSpec, AgentStatus, AutonomySpec, ExecutionMode,
    ExecutionStatus, HarnessKind, HarnessSpec, Instructions, LogLevel, MemorySpec, ModelSpec,
    ResourceLimits, TriggerSource, TriggerSpec, UserRole,
};
use orbit_security::{SecretCipher, SecretValue};
use orbit_store::executions::{EventFilter, NewExecution};
use orbit_store::{Store, WorkerProfile};

async fn test_store() -> Option<Store> {
    let url = std::env::var("ORBIT_TEST_DATABASE_URL").ok()?;
    let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    let cipher = Arc::new(SecretCipher::from_base64_key(&key).unwrap());
    let store = Store::connect(&url, 5)
        .await
        .expect("connect")
        .with_cipher(cipher);
    store.migrate().await.expect("migrate");
    Some(store)
}

async fn seed_org(store: &Store) -> (uuid::Uuid, uuid::Uuid) {
    let slug = format!("test-{}", uuid::Uuid::now_v7());
    let org = store
        .create_organization(&slug, "Test Org", orbit_domain::OrganizationMode::Personal)
        .await
        .expect("org");
    let user = store
        .ensure_user(org.id, "owner@test.local", "Test Owner", UserRole::Owner)
        .await
        .expect("user");
    (org.id, user.id)
}

fn definition(name: &str) -> AgentDefinition {
    AgentDefinition {
        api_version: orbit_domain::AGENT_API_VERSION.into(),
        kind: orbit_domain::AGENT_KIND.into(),
        metadata: AgentMetadata {
            name: name.into(),
            description: "integration test agent".into(),
            labels: Default::default(),
        },
        spec: AgentSpec {
            harness: Some(HarnessSpec {
                kind: HarnessKind::Mock,
                execution_mode: ExecutionMode::OnDemand,
                placement: orbit_domain::PlacementTarget::Auto,
                config: serde_json::json!({}),
            }),
            harness_config: Default::default(),
            model: ModelSpec::default(),
            instructions: Instructions {
                system: "You are a test agent.".into(),
                constraints: vec![],
            },
            connectors: vec![],
            skills: vec![],
            triggers: vec![TriggerSpec::Schedule {
                cron: "*/15 * * * *".into(),
                timezone: None,
                task: None,
            }],
            autonomy: AutonomySpec {
                level: 2,
                approval_mode: Default::default(),
                max_executions_per_hour: Some(10),
            },
            resources: ResourceLimits::default(),
            memory: MemorySpec::default(),
            run_defaults: Default::default(),
        },
    }
}

#[tokio::test]
async fn agent_lifecycle_and_versions() {
    let Some(store) = test_store().await else {
        eprintln!("skipping: ORBIT_TEST_DATABASE_URL not set");
        return;
    };
    let (org, user) = seed_org(&store).await;

    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Lifecycle Agent"))
        .await
        .expect("create agent");
    assert_eq!(version.version, 1);
    assert_eq!(agent.current_version, 1);
    assert_eq!(agent.status, "draft");
    assert_eq!(agent.slug, "lifecycle-agent");

    // Schedule trigger was materialized from the spec.
    let triggers = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].kind, "schedule");
    assert!(triggers[0].next_run_at.is_some());

    // Duplicate version is rejected.
    let err = store
        .add_agent_version(org, agent.id, Some(user), &definition("Lifecycle Agent"))
        .await
        .expect_err("duplicate spec must conflict");
    assert!(matches!(err, orbit_store::StoreError::Conflict(_)));

    // A changed spec creates version 2 and updates metadata.
    let mut changed = definition("Lifecycle Agent v2");
    changed.spec.autonomy.level = 3;
    let (agent, v2) = store
        .add_agent_version(org, agent.id, Some(user), &changed)
        .await
        .expect("new version");
    assert_eq!(v2.version, 2);
    assert_eq!(agent.current_version, 2);
    assert_eq!(agent.name, "Lifecycle Agent v2");

    let (rows, _, total) = store
        .list_agents(org, 10, None, None, Some("Lifecycle"), None)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].spec.is_some());

    // Lifecycle transitions
    let active = store
        .set_agent_status(org, agent.id, AgentStatus::Active)
        .await
        .unwrap();
    assert_eq!(active.status, "active");
    let paused = store
        .set_agent_status(org, agent.id, AgentStatus::Paused)
        .await
        .unwrap();
    assert_eq!(paused.status, "paused");
    assert!(store
        .set_agent_status(org, agent.id, AgentStatus::Paused)
        .await
        .is_err());
    assert_eq!(
        store
            .list_agent_versions(org, agent.id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn execution_lifecycle_and_events() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Execution Agent"))
        .await
        .unwrap();

    let input = serde_json::json!({
        "task": "hello",
        "payload": {},
        "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() }
    });
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: input.clone(),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("idem-1"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    assert_eq!(execution.status, "queued");

    // Idempotent replay returns the same row.
    let replay = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input,
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("idem-1"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    assert_eq!(replay.id, execution.id);

    // Competing claims: only one worker wins.
    let first = store
        .claim_execution(execution.id, "worker-a")
        .await
        .unwrap();
    let second = store
        .claim_execution(execution.id, "worker-b")
        .await
        .unwrap();
    assert!(first.is_some());
    assert!(second.is_none());

    store
        .mark_execution_running(execution.id, Some("session-1"))
        .await
        .unwrap();
    let event = store
        .append_execution_event(
            org,
            execution.id,
            "log",
            orbit_domain::LogLevel::Info,
            "working",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(event.seq, 1);
    let second_event = store
        .append_execution_event(
            org,
            execution.id,
            "output",
            orbit_domain::LogLevel::Info,
            "done",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(second_event.seq, 2);

    let finished = store
        .finish_execution(
            execution.id,
            ExecutionStatus::Succeeded,
            Some(&serde_json::json!({ "ok": true })),
            None,
            Some(&serde_json::json!({ "outputTokens": 3 })),
            Some("session-1"),
        )
        .await
        .unwrap()
        .expect("finished");
    assert_eq!(finished.status, "succeeded");
    assert!(finished.finished_at.is_some());

    // Terminal executions cannot be re-finished or cancelled.
    assert!(store
        .finish_execution(
            execution.id,
            ExecutionStatus::Failed,
            None,
            None,
            None,
            None
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .request_execution_cancel(org, execution.id)
        .await
        .is_err());

    let events = store
        .list_execution_events(org, execution.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);

    let summary = store.dashboard_summary(org, None).await.unwrap();
    assert_eq!(summary.total_agents, 1);
    assert!(summary.executions_24h >= 1);
}

#[tokio::test]
async fn worker_tokens_are_hashed_scoped_and_revocable() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _user) = seed_org(&store).await;
    let issued = orbit_security::issue_worker_token();
    assert!(issued.token.starts_with("orbit_wk_"));
    let record = store
        .create_worker_token(
            org,
            None,
            "test worker",
            &issued.prefix,
            &issued.hash,
            &["mock".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(record.harnesses, vec!["mock".to_string()]);
    let found = store
        .find_worker_token_by_hash(&issued.hash)
        .await
        .unwrap()
        .expect("token should resolve by hash");
    assert_eq!(found.id, record.id);
    store.revoke_worker_token(org, record.id).await.unwrap();
    assert!(store
        .find_worker_token_by_hash(&issued.hash)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn secrets_are_encrypted_and_org_scoped_entities_work() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;

    let reference = "provider/test-secret";
    store
        .put_secret(
            Some(org),
            reference,
            &SecretValue::new("sk-super-secret"),
            "test",
        )
        .await
        .unwrap();
    let value = store.get_secret_value(reference).await.unwrap().unwrap();
    assert_eq!(value.expose(), "sk-super-secret");

    // Raw ciphertext must not contain the plaintext.
    let ciphertext: String =
        sqlx::query_scalar("SELECT ciphertext FROM secrets WHERE reference = $1")
            .bind(reference)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert!(!ciphertext.contains("sk-super-secret"));
    assert!(ciphertext.starts_with("v1:"));

    let provider = store
        .create_provider(
            org,
            "Gateway",
            "openai_compatible",
            &serde_json::json!({ "baseUrl": "http://localhost:8000/v1", "model": "qwen" }),
            Some(reference),
            true,
        )
        .await
        .unwrap();
    assert_eq!(provider.name, "Gateway");
    store
        .update_provider(org, provider.id, Some("Gateway 2"), None, None, Some(false))
        .await
        .unwrap();
    let providers = store.list_providers(org, None).await.unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].name, "Gateway 2");
    assert!(!providers[0].enabled);

    let harness_provider = store
        .create_provider(
            org,
            "Opencode Go",
            "harness_managed",
            &serde_json::json!({
                "harness": "opencode",
                "provider": "opencode-go",
                "model": "deepseek-v4-pro"
            }),
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(harness_provider.kind, "harness_managed");

    // Connections
    let manifest = orbit_contracts::ConnectorManifest {
        slug: format!("test-connector-{}", uuid::Uuid::now_v7()),
        name: "Test".into(),
        version: "0.1.0".into(),
        description: String::new(),
        auth: vec![],
        webhooks: false,
        capabilities: vec![],
        config_schema: serde_json::json!({}),
        docs_url: None,
        builtin: false,
    };
    let connector = store.upsert_connector_manifest(&manifest).await.unwrap();
    let connection = store
        .create_connection(
            org,
            connector.id,
            "Primary",
            &serde_json::json!({}),
            &["notes.list".to_string()],
            Some(reference),
            Some(user),
        )
        .await
        .unwrap();
    let rows = store.list_connections(org, None).await.unwrap();
    assert!(rows
        .iter()
        .any(|row| row.connection.id == connection.id && row.connector_slug == manifest.slug));
    store.delete_connection(org, connection.id).await.unwrap();
    assert!(store.list_connections(org, None).await.unwrap().is_empty());
}

#[tokio::test]
async fn org_scoping_is_enforced() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org_a, user_a) = seed_org(&store).await;
    let (org_b, _) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org_a, Some(user_a), &definition("Scoped Agent"))
        .await
        .unwrap();

    assert!(store.get_agent(org_a, agent.id).await.is_ok());
    assert!(matches!(
        store.get_agent(org_b, agent.id).await,
        Err(orbit_store::StoreError::NotFound(_))
    ));
    assert!(!store
        .list_triggers(org_b, Some(agent.id), None)
        .await
        .unwrap()
        .iter()
        .any(|t| t.agent_id == agent.id));
}

#[tokio::test]
async fn harness_catalog_upsert_is_idempotent() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let descriptor = orbit_harness::HarnessRegistry::builtin()
        .descriptors()
        .into_iter()
        .find(|d| d.kind == HarnessKind::Mock)
        .unwrap();
    store.upsert_harness_descriptor(&descriptor).await.unwrap();
    store.upsert_harness_descriptor(&descriptor).await.unwrap();
    let harnesses = store.list_harnesses(org).await.unwrap();
    assert_eq!(
        harnesses.iter().filter(|r| r.kind == "mock").count(),
        1,
        "upsert must not duplicate built-in harnesses"
    );
}

#[tokio::test]
async fn orphaned_executions_are_failed_and_approvals_expired() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Reaper Agent"))
        .await
        .unwrap();

    let make_execution = |key: &'static str| {
        let store = &store;
        let agent = &agent;
        let version = &version;
        async move {
            store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 30,
                    requested_by: Some(user),
                    idempotency_key: Some(key),
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap()
        }
    };

    let orphan = make_execution("orphan-1").await;
    store
        .claim_execution(orphan.id, "worker-gone")
        .await
        .unwrap();
    store
        .mark_execution_running(orphan.id, Some("s1"))
        .await
        .unwrap();
    let approval = store
        .create_approval(
            org,
            orphan.id,
            serde_json::json!({ "capability": "issues.comment" }),
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE executions SET created_at = now() - interval '1 hour', started_at = now() - interval '1 hour' WHERE id = $1")
        .bind(orphan.id)
        .execute(store.pool())
        .await
        .unwrap();

    // A live worker's execution must not be reaped.
    let healthy = make_execution("healthy-1").await;
    store
        .claim_execution(healthy.id, "worker-alive")
        .await
        .unwrap();
    store
        .mark_execution_running(healthy.id, Some("s2"))
        .await
        .unwrap();
    store
        .upsert_worker_heartbeat(
            org,
            uuid::Uuid::now_v7(),
            "worker-alive",
            &["mock".into()],
            "local",
            Some("test"),
        )
        .await
        .unwrap();

    let reaped = store
        .reap_orphaned_executions_for_org(org, 60)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1, "only the orphan should be reaped");
    assert_eq!(reaped[0].id, orphan.id);
    assert_eq!(reaped[0].status, "failed");
    assert_eq!(reaped[0].error.as_ref().unwrap()["code"], "worker_lost");

    assert_eq!(
        store.get_execution(org, healthy.id).await.unwrap().status,
        "running",
        "live worker execution must survive"
    );
    assert_eq!(
        store.get_approval(org, approval.id).await.unwrap().status,
        "expired"
    );
}

/// #298: a launch claims its execution for the new worker id before the container or Job
/// exists. Until that worker writes its first heartbeat the run is starting, not lost.
#[tokio::test]
async fn launch_claimed_executions_are_not_reaped_before_the_first_heartbeat() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Launch Reaper Agent"))
        .await
        .unwrap();
    let make_execution = || {
        let store = &store;
        let agent = &agent;
        let version = &version;
        async move {
            store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 3600,
                    requested_by: Some(user),
                    idempotency_key: None,
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap()
        }
    };
    let backdate = |id: uuid::Uuid, seconds: i32| {
        let store = &store;
        async move {
            sqlx::query("UPDATE executions SET started_at = now() - make_interval(secs => $2) WHERE id = $1")
                .bind(id)
                .bind(seconds as f64)
                .execute(store.pool())
                .await
                .unwrap();
        }
    };
    let issued = orbit_security::issue_worker_token();
    let token = store
        .create_worker_token(org, None, "launch", &issued.prefix, &issued.hash, &[])
        .await
        .unwrap();

    // Claimed for a launch whose pod is still pulling its image: no heartbeat row yet.
    let starting = make_execution().await;
    let starting_worker = format!("worker-{}", uuid::Uuid::now_v7());
    store
        .claim_execution_for_launch(starting.id, &starting_worker, token.id)
        .await
        .unwrap()
        .expect("claimed");
    backdate(starting.id, 120).await;

    // Same, but the worker reported once and then went silent: it is gone.
    let silent = make_execution().await;
    let silent_worker = format!("worker-{}", uuid::Uuid::now_v7());
    store
        .claim_execution_for_launch(silent.id, &silent_worker, token.id)
        .await
        .unwrap()
        .expect("claimed");
    store
        .upsert_worker_heartbeat(
            org,
            token.id,
            &silent_worker,
            &["mock".into()],
            "docker",
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE worker_heartbeats SET last_heartbeat_at = now() - interval '5 minutes' WHERE worker_id = $1")
        .bind(&silent_worker)
        .execute(store.pool())
        .await
        .unwrap();
    backdate(silent.id, 300).await;

    let reaped = store
        .reap_orphaned_executions_for_org(org, 90)
        .await
        .unwrap();
    let ids: Vec<_> = reaped.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![silent.id], "only the silent worker's run is lost");
    assert_eq!(
        store.get_execution(org, starting.id).await.unwrap().status,
        "dispatched",
        "a run whose worker is still starting must survive"
    );

    // A worker that never appears is ended by the unstarted rule instead.
    sqlx::query("UPDATE executions SET created_at = now() - interval '700 seconds', started_at = now() - interval '700 seconds', updated_at = now() - interval '700 seconds' WHERE id = $1")
        .bind(starting.id)
        .execute(store.pool())
        .await
        .unwrap();
    let unstarted = store
        .reap_unstarted_executions(Some(org), 600, 60)
        .await
        .unwrap();
    assert_eq!(unstarted.len(), 1);
    assert_eq!(unstarted[0].id, starting.id);
    assert_eq!(
        unstarted[0].error.as_ref().unwrap()["code"],
        "worker_did_not_start"
    );
}

#[tokio::test]
async fn unstarted_executions_on_a_live_worker_are_ended() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Unstarted Reaper Agent"))
        .await
        .unwrap();
    store
        .upsert_worker_heartbeat(
            org,
            uuid::Uuid::now_v7(),
            "worker-skewed",
            &["mock".into()],
            "local",
            Some("test"),
        )
        .await
        .unwrap();

    let claimed = |key: &'static str| {
        let store = &store;
        let agent = &agent;
        let version = &version;
        async move {
            let execution = store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 30,
                    requested_by: Some(user),
                    idempotency_key: Some(key),
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap();
            store
                .claim_execution(execution.id, "worker-skewed")
                .await
                .unwrap();
            execution
        }
    };
    let age = |id: uuid::Uuid, interval: &'static str| {
        let store = &store;
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE executions SET created_at = now() - interval '{interval}', \
                 started_at = now() - interval '{interval}', updated_at = now() - interval '{interval}' \
                 WHERE id = $1"
            )))
            .bind(id)
            .execute(store.pool())
            .await
            .unwrap();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE execution_events SET created_at = now() - interval '{interval}' WHERE execution_id = $1"
            )))
            .bind(id)
            .execute(store.pool())
            .await
            .unwrap();
        }
    };

    // Claimed an hour ago, nothing since: the worker never got it.
    let silent = claimed("unstarted-silent").await;
    age(silent.id, "1 hour").await;

    // Claimed an hour ago but still reporting (a long checkout, say): leave it alone.
    let busy = claimed("unstarted-busy").await;
    age(busy.id, "1 hour").await;
    store
        .append_execution_event(
            org,
            busy.id,
            "status",
            LogLevel::Info,
            "checking out repository",
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    // Cancel requested two minutes ago and ignored: end it as cancelled.
    let cancelled = claimed("unstarted-cancelled").await;
    store
        .request_execution_cancel(org, cancelled.id)
        .await
        .unwrap();
    age(cancelled.id, "2 minutes").await;

    // Started runs belong to the heartbeat reaper, not this one.
    let running = claimed("unstarted-running").await;
    store
        .mark_execution_running(running.id, Some("s"))
        .await
        .unwrap();
    age(running.id, "1 hour").await;

    let reaped = store
        .reap_unstarted_executions(Some(org), 600, 60)
        .await
        .unwrap();
    let mut ids: Vec<uuid::Uuid> = reaped.iter().map(|row| row.id).collect();
    ids.sort();
    let mut expected = vec![silent.id, cancelled.id];
    expected.sort();
    assert_eq!(ids, expected, "only the silent and the cancelled run end");

    let silent_now = store.get_execution(org, silent.id).await.unwrap();
    assert_eq!(silent_now.status, "failed");
    assert_eq!(silent_now.error.unwrap()["code"], "worker_did_not_start");
    assert_eq!(
        store.get_execution(org, cancelled.id).await.unwrap().status,
        "cancelled"
    );
    assert_eq!(
        store.get_execution(org, busy.id).await.unwrap().status,
        "dispatched"
    );
    assert_eq!(
        store.get_execution(org, running.id).await.unwrap().status,
        "running"
    );

    // A worker that finally reports in cannot revive an ended run.
    assert!(store
        .mark_execution_running(silent.id, Some("late"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn running_executions_past_timeout_and_grace_are_ended_on_a_live_worker() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Timeout Reaper Agent"))
        .await
        .unwrap();
    store
        .upsert_worker_heartbeat(
            org,
            uuid::Uuid::now_v7(),
            "worker-timeout",
            &["mock".into()],
            "local",
            Some("test"),
        )
        .await
        .unwrap();

    let started = |key: &'static str| {
        let store = &store;
        let agent = &agent;
        async move {
            let execution = store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 30,
                    requested_by: Some(user),
                    idempotency_key: Some(key),
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap();
            store
                .claim_execution(execution.id, "worker-timeout")
                .await
                .unwrap();
            store
                .mark_execution_running(execution.id, Some("s"))
                .await
                .unwrap();
            execution
        }
    };
    let age = |id: uuid::Uuid, interval: &'static str| {
        let store = &store;
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE executions \
                 SET started_at = now() - interval '{interval}', \
                     running_started_at = now() - interval '{interval}' \
                 WHERE id = $1"
            )))
            .bind(id)
            .execute(store.pool())
            .await
            .unwrap();
        }
    };

    // timeout 30s + grace 300s = 330s. Ten minutes old: stuck.
    let stuck = started("timeout-stuck").await;
    age(stuck.id, "10 minutes").await;
    // Past its timeout but still inside the grace window: left alone.
    let in_grace = started("timeout-grace").await;
    age(in_grace.id, "2 minutes").await;
    // Fresh.
    let fresh = started("timeout-fresh").await;

    let reaped = store
        .reap_timed_out_executions(Some(org), 300)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1, "only the stuck run is ended");
    assert_eq!(reaped[0].id, stuck.id);
    assert_eq!(reaped[0].status, "timed_out");
    assert_eq!(
        reaped[0].error.as_ref().unwrap()["code"],
        "execution_timeout"
    );
    for (id, name) in [(in_grace.id, "grace"), (fresh.id, "fresh")] {
        assert_eq!(
            store.get_execution(org, id).await.unwrap().status,
            "running",
            "{name} run must survive"
        );
    }

    // A second sweep (another replica) finds nothing, and a late worker report cannot
    // overwrite the terminal state.
    assert!(store
        .reap_timed_out_executions(Some(org), 300)
        .await
        .unwrap()
        .is_empty());
    let late = store
        .finish_execution(stuck.id, ExecutionStatus::Succeeded, None, None, None, None)
        .await
        .unwrap();
    assert!(late.is_none());
}

#[tokio::test]
async fn timeout_reaper_counts_from_running_not_dispatch() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Timeout From Running Agent"))
        .await
        .unwrap();

    let dispatch_old_just_started = {
        let execution = queued_execution(&store, org, user, &agent, &version).await;
        store
            .claim_execution(execution.id, "worker-dispatch-old")
            .await
            .unwrap();
        store
            .mark_execution_running(execution.id, Some("s"))
            .await
            .unwrap();
        // Simulate a long checkout: dispatched/started long ago, but only just entered running.
        sqlx::query(
            "UPDATE executions \
             SET started_at = now() - interval '15 minutes', \
                 running_started_at = now() \
             WHERE id = $1",
        )
        .bind(execution.id)
        .execute(store.pool())
        .await
        .unwrap();
        execution
    };

    let running_past_deadline = {
        let execution = queued_execution(&store, org, user, &agent, &version).await;
        store
            .claim_execution(execution.id, "worker-running-old")
            .await
            .unwrap();
        store
            .mark_execution_running(execution.id, Some("s"))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE executions \
             SET started_at = now() - interval '20 minutes', \
                 running_started_at = now() - interval '10 minutes' \
             WHERE id = $1",
        )
        .bind(execution.id)
        .execute(store.pool())
        .await
        .unwrap();
        execution
    };

    let reaped = store
        .reap_timed_out_executions(Some(org), 300)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].id, running_past_deadline.id);
    assert_eq!(
        store
            .get_execution(org, dispatch_old_just_started.id)
            .await
            .unwrap()
            .status,
        "running",
        "a run that only just started running must not be timed out for old dispatch time"
    );
}

#[tokio::test]
async fn single_execution_mode_can_take_a_dispatched_execution() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Single Execution Agent"))
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("single-1"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    store
        .claim_execution(execution.id, "parent-worker")
        .await
        .unwrap();
    let taken = store
        .take_execution_for_worker(execution.id, "container-worker")
        .await
        .unwrap()
        .expect("dispatched execution must be takeable");
    assert_eq!(taken.worker_id.as_deref(), Some("container-worker"));
}

#[tokio::test]
async fn skills_profiles_and_agent_scoped_claims() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;

    let skill = store
        .create_skill(
            org,
            "issue-triage",
            "Issue triage",
            "Use when triaging issues",
            "---\nname: issue-triage\ndescription: triage\n---\n# Triage",
            &serde_json::json!({ "references/guide.md": "guide" }),
        )
        .await
        .unwrap();
    assert_eq!(skill.version, 1);
    let updated = store
        .update_skill(org, skill.id, Some("Triage v2"), None, None, None)
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(
        store
            .skills_by_slugs(org, &["issue-triage".into()])
            .await
            .unwrap()
            .len(),
        1
    );

    let (agent_a, version_a) = store
        .create_agent(org, Some(user), &definition("Scoped Agent A"))
        .await
        .unwrap();
    let (agent_b, version_b) = store
        .create_agent(org, Some(user), &definition("Scoped Agent B"))
        .await
        .unwrap();

    let input = serde_json::json!({
        "task": "x", "payload": {},
        "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() }
    });
    let exec_a = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent_a.id,
            agent_version_id: version_a.id,
            agent_name: &agent_a.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: input.clone(),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("scoped-a"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    let _exec_b = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent_b.id,
            agent_version_id: version_b.id,
            agent_name: &agent_b.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input,
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("scoped-b"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();

    let claimed = store
        .claim_next_queued(
            org,
            "scoped-worker",
            &["mock".into()],
            "local",
            &[agent_a.id],
            None,
            None,
        )
        .await
        .unwrap()
        .expect("agent A execution should be claimable");
    assert_eq!(claimed.id, exec_a.id);

    let profile = store
        .create_worker_profile(
            org,
            "Scoped Pi",
            "pi",
            "docker",
            "process",
            "orbit-worker-pi",
            "0.3.0",
            "selected",
            &[agent_a.id],
            &["issue-triage".into()],
            Some("opencode-go"),
            Some("deepseek-v4-pro"),
            Some("orbit-pi-auth"),
            &serde_json::json!({}),
            &serde_json::json!([]),
            &serde_json::json!({}),
            false,
            false,
            false,
            false,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(profile.agent_ids, vec![agent_a.id]);
    assert_eq!(profile.agent_scope, "selected");
    assert_eq!(profile.skill_slugs, vec!["issue-triage"]);
    assert!(
        !store
            .set_worker_profile_enabled(org, profile.id, false)
            .await
            .unwrap()
            .enabled
    );
    store.delete_worker_profile(org, profile.id).await.unwrap();
}

#[tokio::test]
async fn tool_policies_are_scoped_and_ordered() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _user) = seed_org(&store).await;
    let (agent, _version) = store
        .create_agent(org, None, &definition("Tool policy target"))
        .await
        .expect("agent");

    let matcher = serde_json::json!({ "tool": "Bash", "commandRegex": "^rm " });
    let org_rule = store
        .create_tool_policy(
            org,
            None,
            "deny destructive shell",
            10,
            true,
            "deny",
            &matcher,
            Some("destructive command"),
            Some("deletes files"),
        )
        .await
        .expect("org rule");
    let agent_rule = store
        .create_tool_policy(
            org,
            Some(agent.id),
            "ask for writes",
            20,
            true,
            "require_approval",
            &serde_json::json!({ "effect": "write" }),
            None,
            None,
        )
        .await
        .expect("agent rule");

    // Org-level listing does not include agent rules.
    let org_rules = store.list_tool_policies(org, None, None).await.unwrap();
    assert_eq!(org_rules.len(), 1);
    assert_eq!(org_rules[0].id, org_rule.id);

    // Effective rules for the agent are ordered by priority.
    let rules = store
        .effective_tool_policy_rules(org, agent.id)
        .await
        .unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].name, "deny destructive shell");
    assert_eq!(rules[1].id, agent_rule.id);

    // A rule for a different agent is not visible.
    let (other, _) = store
        .create_agent(org, None, &definition("Other agent"))
        .await
        .expect("other agent");
    assert_eq!(
        store
            .effective_tool_policy_rules(org, other.id)
            .await
            .unwrap()
            .len(),
        1
    );

    store.delete_tool_policy(org, org_rule.id).await.unwrap();
    store.delete_tool_policy(org, agent_rule.id).await.unwrap();
    assert!(store
        .effective_tool_policy_rules(org, agent.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn custom_roles_round_trip() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let role = store
        .create_custom_role(
            org,
            "Release manager",
            &["agents:read".to_string(), "executions:create".to_string()],
        )
        .await
        .expect("role");
    assert_eq!(
        store
            .custom_role_permissions(org, "Release manager")
            .await
            .unwrap(),
        Some(vec![
            "agents:read".to_string(),
            "executions:create".to_string()
        ])
    );
    assert!(store
        .custom_role_permissions(org, "missing")
        .await
        .unwrap()
        .is_none());

    // Users can be assigned the custom role by name.
    let updated = store
        .set_user_role_name(org, user, "Release manager")
        .await
        .unwrap();
    assert_eq!(updated.role, "Release manager");

    let updated = store
        .update_custom_role(
            org,
            role.id,
            "Release manager",
            &["agents:read".to_string(), "audit:read".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(updated.permissions.len(), 2);

    store.delete_custom_role(org, role.id).await.unwrap();
    assert!(store.list_custom_roles(org).await.unwrap().is_empty());
}

#[tokio::test]
async fn team_output_enqueue_uses_the_partial_unique_index() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_agent_team(
            org,
            &format!("Team {}", uuid::Uuid::now_v7()),
            "team output",
            &serde_json::json!({ "steps": [] }),
            Some(user),
        )
        .await
        .unwrap();
    let output = store
        .create_output(
            org,
            &format!("team-output-{}", uuid::Uuid::now_v7()),
            "webhook",
            None,
            Some(team.id),
            &["succeeded".to_string()],
            "secret://output/team",
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    let run = store
        .create_team_run(
            org,
            team.id,
            &serde_json::json!({ "task": "t" }),
            Some(user),
        )
        .await
        .unwrap();
    store
        .finish_team_run(
            run.id,
            "succeeded",
            Some(&serde_json::json!({ "ok": true })),
            None,
        )
        .await
        .unwrap();
    assert_eq!(store.enqueue_team_output_deliveries().await.unwrap(), 1);
    // The second pass overlaps the cursor but must dedupe via the partial unique index.
    assert_eq!(store.enqueue_team_output_deliveries().await.unwrap(), 0);
    let deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM output_deliveries WHERE org_id = $1 AND output_id = $2",
    )
    .bind(org)
    .bind(output.id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(deliveries, 1);
}

#[tokio::test]
async fn output_delivery_history_lists_newest_first_with_the_run_task() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_agent_team(
            org,
            &format!("Team {}", uuid::Uuid::now_v7()),
            "history",
            &serde_json::json!({ "steps": [] }),
            Some(user),
        )
        .await
        .unwrap();
    let output = store
        .create_output(
            org,
            &format!("history-{}", uuid::Uuid::now_v7()),
            "webhook",
            None,
            Some(team.id),
            &["succeeded".to_string()],
            "secret://output/history",
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    let quiet = store
        .create_output(
            org,
            &format!("quiet-{}", uuid::Uuid::now_v7()),
            "slack",
            None,
            Some(team.id),
            &["failed".to_string()],
            "secret://output/quiet",
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    for task in ["first", "second"] {
        let run = store
            .create_team_run(
                org,
                team.id,
                &serde_json::json!({ "task": task }),
                Some(user),
            )
            .await
            .unwrap();
        store
            .finish_team_run(
                run.id,
                "succeeded",
                Some(&serde_json::json!({ "ok": true })),
                None,
            )
            .await
            .unwrap();
        store.enqueue_team_output_deliveries().await.unwrap();
        let deliveries: i64 =
            sqlx::query_scalar("SELECT count(*) FROM output_deliveries WHERE output_id = $1")
                .bind(output.id)
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(deliveries, if task == "first" { 1 } else { 2 });
    }
    // Make the order unambiguous and record a failure on the newest delivery, the way
    // `finish_output_delivery` would.
    sqlx::query(
        "UPDATE output_deliveries d SET created_at = now() - interval '1 minute' \
         FROM team_runs t WHERE t.id = d.team_run_id AND d.output_id = $1 AND t.input->>'task' = 'first'",
    )
    .bind(output.id)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE output_deliveries d SET status = 'failed', attempts = 1, response_status = 404, \
         last_error = 'webhook returned HTTP 404' \
         FROM team_runs t WHERE t.id = d.team_run_id AND d.output_id = $1 AND t.input->>'task' = 'second'",
    )
    .bind(output.id)
    .execute(store.pool())
    .await
    .unwrap();

    let history = store
        .list_output_deliveries(org, output.id, 20)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].task.as_deref(), Some("second"));
    assert_eq!(history[0].status, "failed");
    assert_eq!(history[0].response_status, Some(404));
    assert_eq!(
        history[0].last_error.as_deref(),
        Some("webhook returned HTTP 404")
    );
    assert!(history[0].team_run_id.is_some() && history[0].execution_id.is_none());
    assert_eq!(history[1].task.as_deref(), Some("first"));
    assert_eq!(
        store
            .list_output_deliveries(org, output.id, 1)
            .await
            .unwrap()
            .len(),
        1
    );

    // Another organisation sees nothing.
    let (other_org, _) = seed_org(&store).await;
    assert!(store
        .list_output_deliveries(other_org, output.id, 20)
        .await
        .unwrap()
        .is_empty());

    let latest = store.latest_output_deliveries(org).await.unwrap();
    assert_eq!(
        latest.len(),
        1,
        "an output without deliveries has no latest one"
    );
    assert_eq!(latest[0].output_id, output.id);
    assert_eq!(latest[0].status, "failed");
    assert!(latest.iter().all(|delivery| delivery.output_id != quiet.id));
}

#[tokio::test]
async fn delivery_parts_resume_and_pruning_keep_live_rows() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Delivery Reliability Agent"))
        .await
        .unwrap();
    let output = store
        .create_output(
            org,
            "parts-resume",
            "telegram",
            Some(agent.id),
            None,
            &["succeeded".into()],
            "secret://output/parts-resume",
            None,
            Some("-1001234567890"),
            None,
            true,
        )
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "parts", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 300,
            requested_by: Some(user),
            idempotency_key: Some("delivery-parts-1"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    store
        .claim_execution(execution.id, "worker-parts")
        .await
        .unwrap();
    store
        .mark_execution_running(execution.id, None)
        .await
        .unwrap();
    store
        .finish_execution(
            execution.id,
            ExecutionStatus::Succeeded,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(store.enqueue_output_deliveries().await.unwrap(), 1);

    // Claim only this output's row. The shared test database contains deliveries from
    // parallel tests, so the global claim queue cannot identify this job reliably.
    let delivery_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM output_deliveries WHERE output_id = $1")
            .bind(output.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    let claim_token = uuid::Uuid::now_v7();
    sqlx::query(
        "UPDATE output_deliveries SET status = 'sending', attempts = 1, claim_token = $2,          lease_until = now() + interval '120 seconds' WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(claim_token)
    .execute(store.pool())
    .await
    .unwrap();
    let job = orbit_store::OutputDeliveryJob {
        id: delivery_id,
        org_id: org,
        output_id: output.id,
        execution_id: Some(execution.id),
        team_run_id: None,
        attempts: 1,
        claim_token,
        parts_sent: 0,
    };
    assert_eq!(job.parts_sent, 0);
    assert!(store
        .record_output_delivery_part(job.id, job.claim_token)
        .await
        .unwrap());
    assert!(!store
        .record_output_delivery_part(job.id, uuid::Uuid::now_v7())
        .await
        .unwrap());
    assert!(store
        .finish_output_delivery(
            job.id,
            job.claim_token,
            orbit_store::outputs::DeliveryOutcome::Retry,
            None,
            Some("later part failed"),
            job.attempts,
        )
        .await
        .unwrap());

    // The retry reads the counter back from the row.
    let parts_sent: i32 =
        sqlx::query_scalar("SELECT parts_sent FROM output_deliveries WHERE id = $1")
            .bind(job.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(parts_sent, 1);
    let resumed_token = uuid::Uuid::now_v7();
    sqlx::query(
        "UPDATE output_deliveries SET status = 'sending', attempts = 2, claim_token = $2,          lease_until = now() + interval '120 seconds' WHERE id = $1",
    )
    .bind(job.id)
    .bind(resumed_token)
    .execute(store.pool())
    .await
    .unwrap();
    let resumed = orbit_store::OutputDeliveryJob {
        id: job.id,
        org_id: job.org_id,
        output_id: job.output_id,
        execution_id: job.execution_id,
        team_run_id: job.team_run_id,
        attempts: 2,
        claim_token: resumed_token,
        parts_sent,
    };

    // A pending row and a terminal one of the right age: only the latter is pruned.
    let cutoff = chrono::Utc::now() + chrono::Duration::seconds(1);
    store
        .prune_output_deliveries_before(cutoff, 10)
        .await
        .unwrap();
    let claimed_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM output_deliveries WHERE id = $1)")
            .bind(job.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert!(claimed_exists, "a claimed sending row is never pruned");
    assert!(store
        .finish_output_delivery(
            resumed.id,
            resumed.claim_token,
            orbit_store::outputs::DeliveryOutcome::Succeeded,
            None,
            None,
            resumed.attempts,
        )
        .await
        .unwrap());
    // Pruning is installation-wide; the shared test database has other expired rows.
    let pruned = store
        .prune_output_deliveries_before(cutoff, 100_000)
        .await
        .unwrap();
    assert!(pruned >= 1, "our terminal delivery must be pruned");
    let pruned_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM output_deliveries WHERE id = $1)")
            .bind(resumed.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert!(!pruned_exists, "a terminal delivery is pruned");
}

#[tokio::test]
async fn env_mappings_and_outputs_round_trip() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _user) = seed_org(&store).await;

    let mapping = store
        .create_env_mapping(
            org,
            "Shared tokens",
            "reusable",
            &serde_json::json!([
                { "envVar": "EXAMPLE_TOKEN", "connectionId": null, "secretKey": "example" }
            ]),
        )
        .await
        .expect("env mapping");
    assert_eq!(store.list_env_mappings(org, None).await.unwrap().len(), 1);
    assert_eq!(mapping.name, "Shared tokens");
    store.delete_env_mapping(org, mapping.id).await.unwrap();

    let output = store
        .create_output(
            org,
            "Slack release",
            "slack",
            None,
            None,
            &["succeeded".to_string(), "failed".to_string()],
            "secret://output/test",
            Some("release finished"),
            None,
            None,
            true,
        )
        .await
        .expect("output");
    assert_eq!(store.list_outputs(org).await.unwrap().len(), 1);
    assert_eq!(
        store.get_output(org, output.id).await.unwrap().kind,
        "slack"
    );
    store.delete_output(org, output.id).await.unwrap();
}

#[tokio::test]
async fn access_teams_and_ownership_defaults() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;

    let teams = store.list_access_teams(org).await.unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].slug, "default");

    let team = store
        .create_access_team(org, "Payments", "payments", Some(user))
        .await
        .expect("team");
    let member = store
        .upsert_team_member(team.id, user, "lead")
        .await
        .expect("member");
    assert_eq!(member.role, "lead");
    assert_eq!(store.list_team_members(team.id).await.unwrap().len(), 1);

    let updated = store
        .update_access_team(org, team.id, "Payments team", "payments-team")
        .await
        .expect("update");
    assert_eq!(updated.slug, "payments-team");

    let agent = store
        .create_agent(org, Some(user), &definition("Owned agent"))
        .await
        .expect("agent")
        .0;
    let loaded = store.get_agent(org, agent.id).await.unwrap();
    assert_eq!(loaded.visibility, "org");
    assert!(loaded.owner_team_id.is_some());

    store.remove_team_member(team.id, user).await.unwrap();
    store.delete_access_team(org, team.id).await.unwrap();
}

#[tokio::test]
async fn lists_honour_strict_team_isolation() {
    use orbit_store::access::ResourceAccess;

    let Some(store) = test_store().await else {
        return;
    };
    let (org, owner) = seed_org(&store).await;
    let viewer = store
        .create_user(org, "operator@test.local", "Operator", UserRole::Operator)
        .await
        .expect("user");
    let ours = store
        .create_access_team(org, "Ours", "ours", Some(owner))
        .await
        .expect("team");
    let theirs = store
        .create_access_team(org, "Theirs", "theirs", Some(owner))
        .await
        .expect("team");
    store
        .upsert_team_member(ours.id, viewer.id, "member")
        .await
        .unwrap();

    let agent = |name: &'static str, visibility: &'static str, team: Option<uuid::Uuid>| {
        let store = &store;
        async move {
            let created = store
                .create_agent(org, Some(owner), &definition(name))
                .await
                .expect("agent")
                .0;
            store
                .set_resource_visibility(org, "agents", created.id, visibility, team)
                .await
                .expect("visibility");
        }
    };
    agent("Org, other team", "org", Some(theirs.id)).await;
    agent("Org, our team", "org", Some(ours.id)).await;
    agent("Org, no team", "org", None).await;
    agent("Private, other team", "private", Some(theirs.id)).await;

    let names = |access: ResourceAccess| {
        let store = &store;
        async move {
            let (agents, _, total) = store
                .list_agents(org, 50, None, None, None, Some(&access))
                .await
                .expect("list");
            let mut names: Vec<String> = agents.into_iter().map(|agent| agent.agent.name).collect();
            names.sort();
            assert_eq!(total as usize, names.len(), "count matches the page");
            names
        }
    };
    let member = ResourceAccess::new(viewer.id, false, vec![ours.id]);

    assert_eq!(
        names(member.clone()).await,
        ["Org, no team", "Org, other team", "Org, our team"],
        "without isolation, organization visibility is enough"
    );
    assert_eq!(
        names(member.with_team_isolation(true)).await,
        ["Org, no team", "Org, our team"],
        "with isolation, another team's resources stay hidden"
    );
    assert_eq!(
        names(ResourceAccess::new(owner, true, vec![]).with_team_isolation(true))
            .await
            .len(),
        4,
        "admins see everything"
    );
}

#[tokio::test]
async fn team_worker_cannot_claim_another_teams_execution() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team_a = store
        .create_access_team(org, "Team A", "team-a", Some(user))
        .await
        .unwrap();
    let team_b = store
        .create_access_team(org, "Team B", "team-b", Some(user))
        .await
        .unwrap();
    let (agent_a, version_a) = store
        .create_agent(org, Some(user), &definition("Team A Agent"))
        .await
        .unwrap();
    let (agent_b, version_b) = store
        .create_agent(org, Some(user), &definition("Team B Agent"))
        .await
        .unwrap();
    store
        .set_resource_visibility(org, "agents", agent_a.id, "team", Some(team_a.id))
        .await
        .unwrap();
    store
        .set_resource_visibility(org, "agents", agent_b.id, "team", Some(team_b.id))
        .await
        .unwrap();

    let stored_a = store.get_agent(org, agent_a.id).await.unwrap();
    let stored_b = store.get_agent(org, agent_b.id).await.unwrap();
    assert_eq!(stored_a.visibility, "team");
    assert_eq!(stored_a.owner_team_id, Some(team_a.id));
    assert_eq!(stored_b.visibility, "team");
    assert_eq!(stored_b.owner_team_id, Some(team_b.id));
    assert_ne!(team_a.id, team_b.id);

    let input = serde_json::json!({
        "task": "x", "payload": {},
        "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() }
    });
    let exec_b = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent_b.id,
            agent_version_id: version_b.id,
            agent_name: &agent_b.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: input.clone(),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("team-b-claim"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    let exec_a = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent_a.id,
            agent_version_id: version_a.id,
            agent_name: &agent_a.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input,
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("team-a-claim"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();

    assert_eq!(exec_a.agent_id, agent_a.id);
    assert_eq!(exec_b.agent_id, agent_b.id);
    let after_a = store.get_agent(org, agent_a.id).await.unwrap();
    let after_b = store.get_agent(org, agent_b.id).await.unwrap();
    assert_eq!(after_a.visibility, "team");
    assert_eq!(after_b.visibility, "team");

    // A random team must not claim anything.
    let none = store
        .claim_next_queued(
            org,
            "random-worker",
            &["mock".into()],
            "local",
            &[],
            Some(uuid::Uuid::now_v7()),
            None,
        )
        .await
        .unwrap();
    assert!(none.is_none(), "claim ignored the team predicate");

    // B was queued first but is invisible to Team A.
    let claimed = store
        .claim_next_queued(
            org,
            "team-a-worker",
            &["mock".into()],
            "local",
            &[],
            Some(team_a.id),
            None,
        )
        .await
        .unwrap()
        .expect("Team A execution should be claimable");
    assert_eq!(claimed.id, exec_a.id);

    let next = store
        .claim_next_queued(
            org,
            "team-a-worker",
            &["mock".into()],
            "local",
            &[],
            Some(team_a.id),
            None,
        )
        .await
        .unwrap();
    assert!(next.is_none(), "Team A worker claimed Team B execution");

    // The Team B execution is still queued.
    let still_queued = store.get_execution(org, exec_b.id).await.unwrap();
    assert_eq!(still_queued.status, "queued");
}

#[tokio::test]
async fn agent_spec_triggers_are_reconciled() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let mut first = definition("Trigger Reconciler");
    first.spec.triggers = vec![TriggerSpec::Schedule {
        cron: "*/15 * * * *".into(),
        timezone: None,
        task: Some("first task".into()),
    }];
    let (agent, _) = store.create_agent(org, Some(user), &first).await.unwrap();
    let triggers = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].config["cron"], "*/15 * * * *");

    let mut second = definition("Trigger Reconciler");
    second.spec.triggers = vec![TriggerSpec::Schedule {
        cron: "*/5 * * * *".into(),
        timezone: None,
        task: Some("five minute task".into()),
    }];
    store
        .add_agent_version(org, agent.id, Some(user), &second)
        .await
        .unwrap();
    let triggers = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].config["cron"], "*/5 * * * *");
    assert_eq!(triggers[0].config["task"], "five minute task");

    let mut third = definition("Trigger Reconciler");
    third.spec.triggers = vec![];
    store
        .add_agent_version(org, agent.id, Some(user), &third)
        .await
        .unwrap();
    assert!(store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_event_appends_get_dense_unique_seq() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Concurrent Events Agent"))
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "concurrent" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();

    const N: i64 = 50;
    let mut tasks = Vec::new();
    for i in 0..N {
        let store = store.clone();
        let execution_id = execution.id;
        tasks.push(tokio::spawn(async move {
            store
                .append_execution_event(
                    org,
                    execution_id,
                    "log",
                    LogLevel::Info,
                    &format!("event {i}"),
                    &serde_json::json!({ "i": i }),
                )
                .await
        }));
    }
    let mut returned = Vec::new();
    for task in tasks {
        let event = task
            .await
            .unwrap()
            .expect("concurrent append must not fail");
        returned.push(event.seq);
    }
    returned.sort_unstable();
    assert_eq!(returned, (1..=N).collect::<Vec<_>>());

    let stored = store
        .list_execution_events(org, execution.id, 0, 1000)
        .await
        .unwrap();
    let seqs: Vec<i64> = stored.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=N).collect::<Vec<_>>());
}

#[tokio::test]
async fn single_execution_launch_tokens_are_revoked_when_execution_is_terminal() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Launch Token Agent"))
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "t", "payload": {} }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    let profile = store
        .create_worker_profile(
            org,
            "Launch Token Profile",
            "mock",
            "docker",
            "process",
            "orbit-worker",
            "0.1.0",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &serde_json::json!({}),
            &serde_json::json!([]),
            &serde_json::json!({}),
            false,
            false,
            false,
            false,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    let single = orbit_security::issue_worker_token();
    let single_record = store
        .create_worker_token(org, None, "single", &single.prefix, &single.hash, &[])
        .await
        .unwrap();
    let persistent = orbit_security::issue_worker_token();
    let persistent_record = store
        .create_worker_token(
            org,
            None,
            "persistent",
            &persistent.prefix,
            &persistent.hash,
            &[],
        )
        .await
        .unwrap();
    let single_launch = store
        .create_worker_launch(
            org,
            profile.id,
            "single",
            "docker",
            "img:1",
            Some(single_record.id),
            Some(user),
            true,
        )
        .await
        .unwrap();
    store
        .create_worker_launch(
            org,
            profile.id,
            "persistent",
            "docker",
            "img:1",
            Some(persistent_record.id),
            Some(user),
            false,
        )
        .await
        .unwrap();
    store
        .set_worker_launch_single_execution(single_launch.id, execution.id)
        .await
        .unwrap();

    // Execution still queued: neither token is revoked. (The revoke helper is global, so
    // old terminal launches from earlier test runs may also be returned.)
    assert!(!store
        .revoke_finished_single_execution_tokens()
        .await
        .unwrap()
        .contains(&single_record.id));
    assert!(store
        .find_worker_token_by_hash(&single.hash)
        .await
        .unwrap()
        .is_some());

    store
        .claim_execution(execution.id, "single-worker")
        .await
        .unwrap()
        .expect("execution is queued");
    store
        .mark_execution_running(execution.id, None)
        .await
        .unwrap();
    store
        .finish_execution(
            execution.id,
            ExecutionStatus::Succeeded,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let revoked = store
        .revoke_finished_single_execution_tokens()
        .await
        .unwrap();
    assert!(revoked.contains(&single_record.id));
    assert!(!revoked.contains(&persistent_record.id));
    assert!(store
        .find_worker_token_by_hash(&single.hash)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .find_worker_token_by_hash(&persistent.hash)
        .await
        .unwrap()
        .is_some());
}

async fn launch_profile_fixture(store: &Store, org: uuid::Uuid, name: &str) -> WorkerProfile {
    store
        .create_worker_profile(
            org,
            name,
            "pi",
            "docker",
            "process",
            "orbit-worker-pi",
            "0.3.0",
            "all",
            &[],
            &[],
            None,
            None,
            None,
            &serde_json::json!({}),
            &serde_json::json!([]),
            &serde_json::json!({}),
            false,
            false,
            false,
            false,
            &serde_json::json!({}),
        )
        .await
        .unwrap()
}

async fn new_launch(
    store: &Store,
    org: uuid::Uuid,
    profile: &WorkerProfile,
    single: bool,
) -> orbit_store::WorkerLaunch {
    store
        .create_worker_launch(
            org,
            profile.id,
            &profile.name,
            "docker",
            "img:1",
            None,
            None,
            single,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn live_launch_lookup_is_exact_beyond_the_newest_hundred() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let target = launch_profile_fixture(&store, org, "Target").await;
    let other = launch_profile_fixture(&store, org, "Other").await;
    let live = new_launch(&store, org, &target, false).await;
    // 150 newer launches of another profile (single-execution, so they may coexist).
    for _ in 0..150 {
        new_launch(&store, org, &other, true).await;
    }
    assert!(!store
        .list_worker_launches(org, None)
        .await
        .unwrap()
        .iter()
        .any(|l| l.id == live.id));
    let found = store
        .find_live_launch_for_profile(org, target.id)
        .await
        .unwrap()
        .expect("live launch found");
    assert_eq!(found.id, live.id);
    let active = store
        .active_launches_for_profile(org, other.id)
        .await
        .unwrap();
    assert_eq!(active.len(), 150);
}

#[tokio::test]
async fn unique_index_blocks_second_live_long_lived_launch() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let profile = launch_profile_fixture(&store, org, "Unique").await;
    let first = new_launch(&store, org, &profile, false).await;
    let dup = store
        .create_worker_launch(
            org, profile.id, "Unique", "docker", "img:1", None, None, false,
        )
        .await
        .expect_err("second live long-lived launch must be rejected");
    assert!(dup.is_unique_violation());
    // Single-execution launches may coexist with it.
    new_launch(&store, org, &profile, true).await;
    new_launch(&store, org, &profile, true).await;
    // Once the first is stopped a new long-lived launch is allowed again.
    store
        .update_worker_launch(org, first.id, "stopped", None, None, None, false, true)
        .await
        .unwrap();
    new_launch(&store, org, &profile, false).await;
}

#[tokio::test]
async fn aborted_unclaimed_launch_is_not_live_and_token_is_revoked() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let profile = launch_profile_fixture(&store, org, "Abort").await;
    let token_hash = format!("hash-abort-{}", uuid::Uuid::now_v7());
    let token = store
        .create_worker_token(
            org,
            Some(profile.id),
            "t",
            "pfx",
            &token_hash,
            &["pi".into()],
        )
        .await
        .unwrap();
    let launch = store
        .create_worker_launch(
            org,
            profile.id,
            "Abort",
            "docker",
            "img:1",
            Some(token.id),
            None,
            true,
        )
        .await
        .unwrap();
    // Same sequence the handler runs when claim_execution returns None.
    store.revoke_worker_token(org, token.id).await.unwrap();
    let failed = store
        .update_worker_launch(
            org,
            launch.id,
            "failed",
            None,
            None,
            Some("not queued"),
            false,
            true,
        )
        .await
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert!(failed.stopped_at.is_some());
    assert!(store
        .active_launches_for_profile(org, profile.id)
        .await
        .unwrap()
        .is_empty());
    // Revoking again reports NotFound: the token is already revoked.
    assert!(store.revoke_worker_token(org, token.id).await.is_err());
}

#[tokio::test]
async fn terminal_launch_cannot_regress_and_error_is_kept() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let profile = launch_profile_fixture(&store, org, "Guard").await;
    let launch = new_launch(&store, org, &profile, false).await;
    store
        .update_worker_launch(
            org,
            launch.id,
            "starting",
            None,
            None,
            Some("warn"),
            false,
            false,
        )
        .await
        .unwrap();
    // A status-only update must not clear the error.
    let running = store
        .update_worker_launch(
            org,
            launch.id,
            "running",
            Some("rt"),
            None,
            None,
            true,
            false,
        )
        .await
        .unwrap();
    assert_eq!(running.status, "running");
    assert_eq!(running.error.as_deref(), Some("warn"));
    let stopped = store
        .update_worker_launch(org, launch.id, "stopped", None, None, None, false, true)
        .await
        .unwrap();
    assert_eq!(stopped.status, "stopped");
    // A racing poll refresh reporting running must not revive it.
    let after = store
        .update_worker_launch(org, launch.id, "running", None, None, None, true, false)
        .await
        .unwrap();
    assert_eq!(after.status, "stopped");
    assert_eq!(after.stopped_at, stopped.stopped_at);
    // Idempotent terminal re-write is allowed and keeps the first stop time.
    let again = store
        .update_worker_launch(org, launch.id, "stopped", None, None, None, false, true)
        .await
        .unwrap();
    assert_eq!(again.stopped_at, stopped.stopped_at);
    // Missing launches are still NotFound.
    assert!(store
        .update_worker_launch(
            org,
            uuid::Uuid::now_v7(),
            "running",
            None,
            None,
            None,
            false,
            false
        )
        .await
        .is_err());
}

#[tokio::test]
async fn expired_approvals_cannot_be_decided() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Expiry Agent"))
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: Some("expiry-1"),
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();

    let expired = store
        .create_approval(
            org,
            execution.id,
            serde_json::json!({ "action": "x" }),
            Some(chrono::Utc::now() - chrono::Duration::seconds(5)),
        )
        .await
        .unwrap();
    let error = store
        .decide_approval(org, expired.id, "approved", user, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("approval expired"), "{error}");
    let stored = store.get_approval(org, expired.id).await.unwrap();
    assert_eq!(stored.status, "expired");
    assert!(stored.decided_by.is_none());

    // A live approval is still decidable, and pending ones are counted.
    let live = store
        .create_approval(
            org,
            execution.id,
            serde_json::json!({ "action": "y" }),
            Some(chrono::Utc::now() + chrono::Duration::seconds(300)),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .count_pending_approvals(org, execution.id)
            .await
            .unwrap(),
        1
    );
    let decided = store
        .decide_approval(org, live.id, "approved", user, None)
        .await
        .unwrap();
    assert_eq!(decided.status, "approved");
}

#[tokio::test]
async fn terminal_executions_expire_their_pending_approvals() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Approval Reliability Agent"))
        .await
        .unwrap();
    let input = serde_json::json!({
        "task": "gated",
        "payload": {},
        "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() }
    });
    let make_execution = |key: &'static str| NewExecution {
        org_id: org,
        agent_id: agent.id,
        agent_version_id: version.id,
        agent_name: &agent.name,
        harness_kind: HarnessKind::Mock,
        trigger_source: TriggerSource::Manual,
        placement: "auto",
        trigger_id: None,
        input: input.clone(),
        session_ref: None,
        skills: &[],
        timeout_seconds: 300,
        requested_by: Some(user),
        idempotency_key: Some(key),
        team_run_id: None,
        team_step_index: None,
    };

    // finish_execution expires approvals in the same transaction as the terminal state.
    let running = store
        .create_execution(make_execution("approvals-finished"))
        .await
        .unwrap();
    store
        .claim_execution(running.id, "worker-approvals")
        .await
        .unwrap();
    store
        .mark_execution_running(running.id, None)
        .await
        .unwrap();
    let approval = store
        .create_approval(
            org,
            running.id,
            serde_json::json!({ "action": "x" }),
            Some(chrono::Utc::now() + chrono::Duration::seconds(300)),
        )
        .await
        .unwrap();
    assert!(store
        .finish_execution(
            running.id,
            ExecutionStatus::Succeeded,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        store.get_approval(org, approval.id).await.unwrap().status,
        "expired"
    );
    let error = store
        .decide_approval(org, approval.id, "approved", user, None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, orbit_store::StoreError::Conflict(_)),
        "{error}"
    );
    assert!(store
        .get_approval(org, approval.id)
        .await
        .unwrap()
        .decided_by
        .is_none());

    // The user cancel path expires approvals too, even before the worker reports back.
    let queued = store
        .create_execution(make_execution("approvals-cancelled"))
        .await
        .unwrap();
    let approval = store
        .create_approval(
            org,
            queued.id,
            serde_json::json!({ "action": "y" }),
            Some(chrono::Utc::now() + chrono::Duration::seconds(300)),
        )
        .await
        .unwrap();
    let cancelled = store
        .request_execution_cancel(org, queued.id)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(
        store.get_approval(org, approval.id).await.unwrap().status,
        "expired"
    );
    assert!(matches!(
        store
            .decide_approval(org, approval.id, "rejected", user, None)
            .await
            .unwrap_err(),
        orbit_store::StoreError::Conflict(_)
    ));
}

/// Pruning takes a database-wide advisory lock and gives up when another caller holds
/// it (that is how replicas avoid working on the same rows). Tests that prune must
/// therefore not overlap, or one of them deletes nothing.
static AUDIT_PRUNE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn prune_audit_logs_removes_only_rows_older_than_cutoff() {
    let Some(store) = test_store().await else {
        return;
    };
    let _serial = AUDIT_PRUNE_TESTS.lock().await;
    let (org, _) = seed_org(&store).await;
    let append = |action: &'static str| {
        store.append_audit_log(
            org,
            "system",
            None,
            None,
            action,
            None,
            None,
            serde_json::json!({}),
            None,
            None,
        )
    };
    append("test.old").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cutoff = chrono::Utc::now();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    append("test.recent").await.unwrap();

    // A cutoff in the past (retention window not yet elapsed) removes nothing of ours.
    store
        .prune_audit_logs(chrono::Utc::now() - chrono::Duration::days(365))
        .await
        .unwrap();
    let (rows, _) = store.list_audit_logs(org, 10, None, None).await.unwrap();
    assert_eq!(rows.len(), 2);

    let removed = store.prune_audit_logs(cutoff).await.unwrap();
    assert!(removed >= 1);
    let (rows, _) = store.list_audit_logs(org, 10, None, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, "test.recent");
}

fn schedule_config(cron: &str, timezone: Option<&str>) -> serde_json::Value {
    serde_json::json!({ "cron": cron, "timezone": timezone })
}

async fn backdate_next_run(store: &Store, trigger_id: uuid::Uuid) {
    sqlx::query("UPDATE triggers SET next_run_at = now() - interval '1 minute' WHERE id = $1")
        .bind(trigger_id)
        .execute(store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn claimed_schedule_trigger_always_has_next_run() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Claim Atomic"))
        .await
        .unwrap();
    let trigger = store
        .create_trigger(
            org,
            agent.id,
            "schedule",
            None,
            schedule_config("*/5 * * * *", None),
            true,
        )
        .await
        .unwrap();
    backdate_next_run(&store, trigger.id).await;

    // Simulate a crash: claim, then never fire or reschedule. The claim is global and
    // skips rows another transaction has locked, so a parallel test may be the one that
    // claims this row, and its commit may land a moment later: wait for either outcome.
    let mut after = store.get_trigger(org, trigger.id).await.unwrap();
    for _ in 0..40 {
        let _ = store.claim_due_triggers(1000).await.unwrap();
        after = store.get_trigger(org, trigger.id).await.unwrap();
        if after.last_fired_at.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(after.enabled);
    assert!(after.last_fired_at.is_some(), "trigger was not claimed");
    let next = after.next_run_at.expect("next_run_at must never be NULL");
    assert!(next > chrono::Utc::now());
}

#[tokio::test]
async fn stuck_schedule_triggers_are_repaired() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Stuck Repair"))
        .await
        .unwrap();
    let trigger = store
        .create_trigger(
            org,
            agent.id,
            "schedule",
            None,
            schedule_config("0 9 * * *", Some("Europe/Amsterdam")),
            true,
        )
        .await
        .unwrap();
    // The pre-fix crash state.
    sqlx::query("UPDATE triggers SET next_run_at = NULL WHERE id = $1")
        .bind(trigger.id)
        .execute(store.pool())
        .await
        .unwrap();

    store.repair_stuck_schedule_triggers().await.unwrap();

    let after = store.get_trigger(org, trigger.id).await.unwrap();
    assert!(after.next_run_at.unwrap() > chrono::Utc::now());
}

#[tokio::test]
async fn enabling_a_schedule_trigger_recomputes_next_run() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Enable Recompute"))
        .await
        .unwrap();
    let trigger = store
        .create_trigger(
            org,
            agent.id,
            "schedule",
            None,
            schedule_config("0 * * * *", None),
            true,
        )
        .await
        .unwrap();
    store
        .set_trigger_enabled(org, trigger.id, false)
        .await
        .unwrap();
    // Stale (past) value, as left by long disablement.
    backdate_next_run(&store, trigger.id).await;
    let enabled = store
        .set_trigger_enabled(org, trigger.id, true)
        .await
        .unwrap();
    assert!(enabled.next_run_at.unwrap() > chrono::Utc::now());

    // Stuck (NULL) value, as left by the old crash.
    store
        .set_trigger_enabled(org, trigger.id, false)
        .await
        .unwrap();
    sqlx::query("UPDATE triggers SET next_run_at = NULL WHERE id = $1")
        .bind(trigger.id)
        .execute(store.pool())
        .await
        .unwrap();
    let enabled = store
        .set_trigger_enabled(org, trigger.id, true)
        .await
        .unwrap();
    assert!(enabled.next_run_at.unwrap() > chrono::Utc::now());
}

#[tokio::test]
async fn timezone_is_applied_when_scheduling() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Timezone"))
        .await
        .unwrap();
    let utc = store
        .create_trigger(
            org,
            agent.id,
            "schedule",
            None,
            schedule_config("0 9 * * *", None),
            true,
        )
        .await
        .unwrap();
    let ams = store
        .create_trigger(
            org,
            agent.id,
            "schedule",
            None,
            schedule_config("0 9 * * *", Some("Europe/Amsterdam")),
            true,
        )
        .await
        .unwrap();
    use chrono::Timelike;
    assert_eq!(utc.next_run_at.unwrap().hour(), 9);
    let ams_hour = ams.next_run_at.unwrap().hour();
    assert!(ams_hour == 8 || ams_hour == 7, "got {ams_hour}");
}

#[tokio::test]
async fn agent_version_sync_keeps_api_triggers_and_stable_ids() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let spec_trigger = |cron: &str| TriggerSpec::Schedule {
        cron: cron.into(),
        timezone: None,
        task: Some("spec task".into()),
    };
    let mut first = definition("Managed Sync");
    first.spec.triggers = vec![spec_trigger("0 * * * *"), spec_trigger("30 * * * *")];
    let (agent, _) = store.create_agent(org, Some(user), &first).await.unwrap();

    let api_trigger = store
        .create_trigger(
            org,
            agent.id,
            "webhook",
            None,
            serde_json::json!({ "name": "operator" }),
            true,
        )
        .await
        .unwrap();
    let before = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    let unchanged_id = before
        .iter()
        .find(|t| t.config["cron"] == "0 * * * *")
        .unwrap()
        .id;

    // New version: first spec trigger unchanged, second changed.
    let mut second = definition("Managed Sync");
    second.spec.triggers = vec![spec_trigger("0 * * * *"), spec_trigger("45 * * * *")];
    store
        .add_agent_version(org, agent.id, Some(user), &second)
        .await
        .unwrap();

    let after = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    assert_eq!(after.len(), 3);
    assert!(
        after.iter().any(|t| t.id == api_trigger.id),
        "API-created trigger was deleted by sync"
    );
    assert!(
        after.iter().any(|t| t.id == unchanged_id),
        "unchanged spec trigger got a new id"
    );
    assert!(after.iter().any(|t| t.config["cron"] == "45 * * * *"));
    assert!(!after.iter().any(|t| t.config["cron"] == "30 * * * *"));

    // Removing all spec triggers removes only the managed rows.
    let mut third = definition("Managed Sync");
    third.spec.triggers = vec![];
    store
        .add_agent_version(org, agent.id, Some(user), &third)
        .await
        .unwrap();
    let after = store
        .list_triggers(org, Some(agent.id), None)
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, api_trigger.id);
}

// ---------------------------------------------------------------------------
// #144: org-scoped secret resolution (restored, #191)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn secret_resolution_is_org_scoped() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org_a, _) = seed_org(&store).await;
    let (org_b, _) = seed_org(&store).await;
    let reference = format!("connection/{}", uuid::Uuid::now_v7());
    store
        .put_secret(Some(org_a), &reference, &SecretValue::new("a-secret"), "t")
        .await
        .unwrap();
    let own = store
        .get_secret_value_for_org(org_a, &reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(own.expose(), "a-secret");
    assert!(store
        .get_secret_value_for_org(org_b, &reference)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .secret_owner(org_b, &reference)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn stale_worker_heartbeats_are_pruned_after_retention() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let worker_id = format!("stale-retention-{}", uuid::Uuid::now_v7());
    store
        .upsert_worker_heartbeat(
            org,
            uuid::Uuid::now_v7(),
            &worker_id,
            &["mock".into()],
            "local",
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE worker_heartbeats SET last_heartbeat_at = now() - interval '25 hours' WHERE worker_id = $1",
    )
    .bind(&worker_id)
    .execute(store.pool())
    .await
    .unwrap();

    assert!(store.delete_stale_worker_heartbeats(24).await.unwrap() >= 1);
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM worker_heartbeats WHERE worker_id = $1")
            .bind(&worker_id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn agent_can_return_to_an_earlier_configuration() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let original = definition("Revert Agent");
    let (agent, v1) = store
        .create_agent(org, Some(user), &original)
        .await
        .expect("create agent");
    assert_eq!(v1.version, 1);

    let mut changed = original.clone();
    changed.spec.autonomy.level = 3;
    let (_, v2) = store
        .add_agent_version(org, agent.id, Some(user), &changed)
        .await
        .expect("version 2");
    assert_eq!(v2.version, 2);

    // Going back to version 1's configuration appends version 3 with the same spec.
    let (agent, v3) = store
        .add_agent_version(org, agent.id, Some(user), &original)
        .await
        .expect("returning to an earlier configuration is a new version");
    assert_eq!(v3.version, 3);
    assert_eq!(agent.current_version, 3);
    assert_eq!(v3.checksum, v1.checksum);
    assert_eq!(v3.spec, v1.spec);

    // Saving what is already current is still refused.
    let err = store
        .add_agent_version(org, agent.id, Some(user), &original)
        .await
        .expect_err("unchanged spec must conflict");
    assert!(matches!(err, orbit_store::StoreError::Conflict(_)));

    // History keeps every version, in order.
    let versions = store.list_agent_versions(org, agent.id).await.unwrap();
    let numbers: Vec<i32> = versions.iter().map(|version| version.version).collect();
    assert_eq!(numbers.len(), 3);
    assert!(numbers.contains(&1) && numbers.contains(&2) && numbers.contains(&3));
}

#[tokio::test]
async fn execution_event_filters_apply_before_limit() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Event Filter Agent"))
        .await
        .unwrap();
    let execution = store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "filters" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();

    for (kind, message, data) in [
        (
            "tool_call",
            "github.issues_list executed",
            serde_json::json!({ "tool": "github_issues_list", "connector": "github", "effect": "read" }),
        ),
        ("output", "final answer", serde_json::json!({})),
        (
            "error",
            "issues.comment failed",
            serde_json::json!({ "capability": "issues.comment", "effect": "write" }),
        ),
        ("log", "routine", serde_json::json!({})),
    ] {
        store
            .append_execution_event(org, execution.id, kind, LogLevel::Info, message, &data)
            .await
            .unwrap();
    }

    let all = store
        .list_execution_events(org, execution.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 4);

    // A limit of one still finds the matching row instead of the first row.
    let tool = store
        .list_execution_events_filtered(
            org,
            execution.id,
            0,
            1,
            &EventFilter {
                kinds: vec!["tool_call".into()],
                tool: Some("GITHUB".into()),
                q: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(tool.len(), 1);
    assert_eq!(tool[0].seq, all[0].seq);

    let query = store
        .list_execution_events_filtered(
            org,
            execution.id,
            0,
            100,
            &EventFilter {
                kinds: Vec::new(),
                tool: None,
                q: Some("FINAL".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(query.len(), 1);
    assert_eq!(query[0].seq, all[1].seq);

    let capability = store
        .list_execution_events_filtered(
            org,
            execution.id,
            0,
            100,
            &EventFilter {
                kinds: vec!["error".into()],
                tool: Some("issues.comment".into()),
                q: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(capability.len(), 1);
    assert_eq!(capability[0].seq, all[2].seq);

    // Literal `%` is not a wildcard in the text match.
    let wildcard = store
        .list_execution_events_filtered(
            org,
            execution.id,
            0,
            100,
            &EventFilter {
                kinds: Vec::new(),
                tool: None,
                q: Some("%".into()),
            },
        )
        .await
        .unwrap();
    assert!(wildcard.is_empty());
}

// ---------------------------------------------------------------------------
// State-transition guards and resource lifecycle (#215, #216, #218.5)
// ---------------------------------------------------------------------------

async fn queued_execution(
    store: &Store,
    org: uuid::Uuid,
    user: uuid::Uuid,
    agent: &orbit_store::Agent,
    version: &orbit_store::AgentVersion,
) -> orbit_store::ExecutionRecord {
    store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({
                "task": "x", "payload": {},
                "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() }
            }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap()
}

/// Two racing status changes must behave as if they ran one after the other: a
/// transition validated against a stale read never lands on top of the other one.
#[tokio::test]
async fn concurrent_agent_status_changes_are_serialized() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    for round in 0..15 {
        let (agent, _) = store
            .create_agent(org, Some(user), &definition(&format!("Race Agent {round}")))
            .await
            .unwrap();
        store
            .set_agent_status(org, agent.id, AgentStatus::Active)
            .await
            .unwrap();
        // From `active`, pause and archive are both legal, and pausing an archived
        // agent is not: so if only one of them wins, it decides the final status.
        let (archive, pause) = tokio::join!(
            store.set_agent_status(org, agent.id, AgentStatus::Archived),
            store.set_agent_status(org, agent.id, AgentStatus::Paused),
        );
        let final_status = store.get_agent(org, agent.id).await.unwrap().status;
        for error in [&archive, &pause]
            .into_iter()
            .filter_map(|r| r.as_ref().err())
        {
            assert!(
                matches!(
                    error,
                    orbit_store::StoreError::Conflict(_) | orbit_store::StoreError::Invalid(_)
                ),
                "round {round}: {error:?}"
            );
        }
        match (archive.is_ok(), pause.is_ok()) {
            // Paused first, then archived: the only serial order in which both succeed.
            (true, true) => assert_eq!(final_status, "archived", "round {round}"),
            (true, false) => assert_eq!(final_status, "archived", "round {round}"),
            (false, true) => assert_eq!(final_status, "paused", "round {round}"),
            (false, false) => panic!("round {round}: both transitions were refused"),
        }
    }
}

/// ADR 0013: archiving is reversible. Activating an archived agent restores it.
#[tokio::test]
async fn archived_agent_can_be_restored() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Restore Agent"))
        .await
        .unwrap();
    let archived = store
        .set_agent_status(org, agent.id, AgentStatus::Archived)
        .await
        .unwrap();
    assert_eq!(archived.status, "archived");
    assert!(archived.archived_at.is_some());

    let restored = store
        .set_agent_status(org, agent.id, AgentStatus::Active)
        .await
        .unwrap();
    assert_eq!(restored.status, "active");
    assert!(restored.archived_at.is_none(), "restore clears archived_at");

    // Pausing is still only possible from active.
    store
        .set_agent_status(org, agent.id, AgentStatus::Archived)
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_agent_status(org, agent.id, AgentStatus::Paused)
            .await,
        Err(orbit_store::StoreError::Invalid(_))
    ));
}

#[tokio::test]
async fn terminal_team_run_cannot_be_overwritten_and_lease_release_needs_the_token() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_agent_team(org, "Guard Team", "", &serde_json::json!({}), Some(user))
        .await
        .unwrap();
    let run = store
        .create_team_run(org, team.id, &serde_json::json!({}), Some(user))
        .await
        .unwrap();

    // Lease: a stale holder cannot clear the lease a newer holder took over.
    let (_, first) = store
        .claim_team_run_advance(run.id, 60)
        .await
        .unwrap()
        .expect("first claim");
    assert!(store
        .claim_team_run_advance(run.id, 60)
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "UPDATE team_runs SET advance_claimed_at = now() - interval '10 minutes' WHERE id = $1",
    )
    .bind(run.id)
    .execute(store.pool())
    .await
    .unwrap();
    let (_, second) = store
        .claim_team_run_advance(run.id, 60)
        .await
        .unwrap()
        .expect("expired lease can be taken over");
    assert!(!store.release_team_run_advance(run.id, first).await.unwrap());
    assert!(
        store
            .claim_team_run_advance(run.id, 60)
            .await
            .unwrap()
            .is_none(),
        "the new holder's lease must survive the stale release"
    );
    assert!(store
        .release_team_run_advance(run.id, second)
        .await
        .unwrap());

    // A run that ends closes its running steps (#431); a non-terminal update does not.
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Guard Step Agent"))
        .await
        .unwrap();
    store
        .create_team_step(
            run.id,
            org,
            0,
            "first",
            agent.id,
            version.id,
            &[],
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    store
        .update_team_run(run.id, "running", None, None)
        .await
        .unwrap();
    assert_eq!(
        store.list_team_steps(run.id).await.unwrap()[0].status,
        "running"
    );

    // Terminal runs are never overwritten.
    store
        .update_team_run(run.id, "failed", None, Some("boom"))
        .await
        .unwrap();
    let steps = store.list_team_steps(run.id).await.unwrap();
    assert_eq!(steps[0].status, "cancelled");
    assert_eq!(
        steps[0].error.as_deref(),
        Some("stopped because the run ended")
    );
    assert!(steps[0].finished_at.is_some());
    let err = store
        .update_team_run(run.id, "running", None, None)
        .await
        .expect_err("terminal run must not be revived");
    assert!(matches!(err, orbit_store::StoreError::Conflict(_)));
    let err = store
        .update_team_run(run.id, "succeeded", None, None)
        .await
        .expect_err("terminal run must not change outcome");
    assert!(matches!(err, orbit_store::StoreError::Conflict(_)));
    let stored = store.get_team_run(org, run.id).await.unwrap();
    assert_eq!(stored.status, "failed");
    assert_eq!(stored.error.as_deref(), Some("boom"));
}

#[tokio::test]
async fn live_worker_launch_cannot_be_archived() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let profile = launch_profile_fixture(&store, org, "Archive Guard").await;
    let launch = new_launch(&store, org, &profile, false).await;
    let err = store
        .archive_worker_launch(org, launch.id)
        .await
        .expect_err("live launch must not be archived");
    assert!(matches!(err, orbit_store::StoreError::Conflict(_)));
    assert!(store
        .find_live_launch_for_profile(org, profile.id)
        .await
        .unwrap()
        .is_some());
    let err = store
        .archive_worker_launch(org, uuid::Uuid::now_v7())
        .await
        .expect_err("unknown launch");
    assert!(matches!(err, orbit_store::StoreError::NotFound(_)));

    store
        .update_worker_launch(org, launch.id, "stopped", None, None, None, false, true)
        .await
        .unwrap();
    let archived = store.archive_worker_launch(org, launch.id).await.unwrap();
    assert!(archived.archived_at.is_some());
}

#[tokio::test]
async fn launch_claim_binds_the_worker_token_atomically() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Bind Agent"))
        .await
        .unwrap();
    let execution = queued_execution(&store, org, user, &agent, &version).await;
    let profile = launch_profile_fixture(&store, org, "Bind").await;
    let token = store
        .create_worker_token(
            org,
            Some(profile.id),
            "t",
            "pfx",
            &format!("hash-bind-{}", uuid::Uuid::now_v7()),
            &["mock".into()],
        )
        .await
        .unwrap();
    let claimed = store
        .claim_execution_for_launch(execution.id, "worker-x", token.id)
        .await
        .unwrap()
        .expect("queued execution is claimable");
    assert_eq!(claimed.status, "dispatched");
    assert_eq!(
        store.execution_worker_token(execution.id).await.unwrap(),
        Some(token.id)
    );
    assert!(store
        .claim_execution_for_launch(execution.id, "worker-y", token.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn deleting_an_access_team_is_blocked_while_it_owns_team_visible_resources() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_access_team(org, "Doomed", "doomed", Some(user))
        .await
        .unwrap();
    let (agent, _) = store
        .create_agent(org, Some(user), &definition("Team Owned Agent"))
        .await
        .unwrap();
    store
        .set_resource_visibility(org, "agents", agent.id, "team", Some(team.id))
        .await
        .unwrap();
    let err = store
        .delete_access_team(org, team.id)
        .await
        .expect_err("team owns a team-visible agent");
    match err {
        orbit_store::StoreError::Conflict(message) => {
            assert!(message.contains("1 agents"), "{message}")
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    assert!(store.get_access_team(org, team.id).await.is_ok());

    // Once the resource is moved to org visibility the team can go.
    store
        .set_resource_visibility(org, "agents", agent.id, "org", Some(team.id))
        .await
        .unwrap();
    store.delete_access_team(org, team.id).await.unwrap();
    assert!(store.get_access_team(org, team.id).await.is_err());
}

#[tokio::test]
async fn hard_deleting_an_agent_removes_its_history_but_not_team_run_history() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;

    // Agent with executions (+ output webhook secret): hard delete works as documented.
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Hard Delete Agent"))
        .await
        .unwrap();
    let execution = queued_execution(&store, org, user, &agent, &version).await;
    let secret_ref = format!("output/{}", uuid::Uuid::now_v7());
    store
        .put_secret(
            Some(org),
            &secret_ref,
            &SecretValue::new("https://hook.example/x"),
            "t",
        )
        .await
        .unwrap();
    store
        .create_output(
            org,
            &format!("agent-output-{}", uuid::Uuid::now_v7()),
            "webhook",
            Some(agent.id),
            None,
            &["succeeded".to_string()],
            &secret_ref,
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    store.check_delete_agent(org, agent.id).await.unwrap();
    store.delete_agent(org, agent.id).await.unwrap();
    assert!(store.get_agent(org, agent.id).await.is_err());
    assert!(store.get_execution(org, execution.id).await.is_err());
    assert!(store.get_secret_value(&secret_ref).await.unwrap().is_none());

    // Agent referenced by a team run step: refused with a clear conflict, nothing deleted.
    let (blocked, blocked_version) = store
        .create_agent(org, Some(user), &definition("Team Step Agent"))
        .await
        .unwrap();
    let team = store
        .create_agent_team(org, "Step Team", "", &serde_json::json!({}), Some(user))
        .await
        .unwrap();
    let run = store
        .create_team_run(org, team.id, &serde_json::json!({}), Some(user))
        .await
        .unwrap();
    store
        .create_team_step(
            run.id,
            org,
            0,
            "s",
            blocked.id,
            blocked_version.id,
            &[],
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    let err = store
        .delete_agent(org, blocked.id)
        .await
        .expect_err("team step history blocks the delete");
    match err {
        orbit_store::StoreError::Conflict(message) => {
            assert!(message.contains("team runs"), "{message}")
        }
        other => panic!("expected conflict, got {other:?}"),
    }
    assert!(store.get_agent(org, blocked.id).await.is_ok());
}

#[tokio::test]
async fn output_webhook_secret_is_removed_with_its_output_and_on_replacement() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _) = seed_org(&store).await;
    let first = format!("output/{}", uuid::Uuid::now_v7());
    let second = format!("output/{}", uuid::Uuid::now_v7());
    for reference in [&first, &second] {
        store
            .put_secret(
                Some(org),
                reference,
                &SecretValue::new("https://hook.example/y"),
                "t",
            )
            .await
            .unwrap();
    }
    let output = store
        .create_output(
            org,
            &format!("secret-output-{}", uuid::Uuid::now_v7()),
            "webhook",
            None,
            None,
            &["succeeded".to_string()],
            &first,
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    store
        .update_output(
            org,
            output.id,
            &output.name,
            "webhook",
            None,
            None,
            &["succeeded".to_string()],
            Some(&second),
            None,
            None,
            None,
            true,
        )
        .await
        .unwrap();
    assert!(store.get_secret_value(&first).await.unwrap().is_none());
    assert!(store.get_secret_value(&second).await.unwrap().is_some());
    store.delete_output(org, output.id).await.unwrap();
    assert!(store.get_secret_value(&second).await.unwrap().is_none());
}

#[tokio::test]
async fn org_level_worker_profile_does_not_claim_team_private_executions() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_access_team(org, "Claim Team", "claim-team", Some(user))
        .await
        .unwrap();
    let (private_agent, private_version) = store
        .create_agent(org, Some(user), &definition("Claim Private Agent"))
        .await
        .unwrap();
    store
        .set_resource_visibility(org, "agents", private_agent.id, "team", Some(team.id))
        .await
        .unwrap();
    let private_exec = queued_execution(&store, org, user, &private_agent, &private_version).await;

    let none = store
        .claim_next_queued_for_profile(
            org,
            "org-profile-worker",
            &["mock".into()],
            "local",
            &[],
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        none.is_none(),
        "org-level profile claimed a team-private run"
    );
    assert_eq!(
        store
            .get_execution(org, private_exec.id)
            .await
            .unwrap()
            .status,
        "queued"
    );

    // An org-visible agent is claimable by the org-level profile.
    let (open_agent, open_version) = store
        .create_agent(org, Some(user), &definition("Claim Open Agent"))
        .await
        .unwrap();
    store
        .set_resource_visibility(org, "agents", open_agent.id, "org", None)
        .await
        .unwrap();
    let open_exec = queued_execution(&store, org, user, &open_agent, &open_version).await;
    let claimed = store
        .claim_next_queued_for_profile(
            org,
            "org-profile-worker",
            &["mock".into()],
            "local",
            &[],
            None,
            None,
        )
        .await
        .unwrap()
        .expect("org-visible execution is claimable");
    assert_eq!(claimed.id, open_exec.id);

    // The owning team's profile still claims the team-private one.
    let claimed = store
        .claim_next_queued_for_profile(
            org,
            "team-profile-worker",
            &["mock".into()],
            "local",
            &[],
            Some(team.id),
            None,
        )
        .await
        .unwrap()
        .expect("team profile claims its own team's execution");
    assert_eq!(claimed.id, private_exec.id);
}

#[tokio::test]
async fn team_members_must_belong_to_the_teams_org() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (other_org, other_user) = seed_org(&store).await;
    let team = store
        .create_access_team(org, "Members", "members", Some(user))
        .await
        .unwrap();
    store
        .upsert_team_member(team.id, user, "lead")
        .await
        .unwrap();
    let err = store
        .upsert_team_member(team.id, other_user, "member")
        .await
        .expect_err("cross-org user must be refused");
    assert!(matches!(err, orbit_store::StoreError::NotFound(_)));
    let err = store
        .upsert_team_member(team.id, uuid::Uuid::now_v7(), "member")
        .await
        .expect_err("unknown user looks identical");
    assert!(matches!(err, orbit_store::StoreError::NotFound(_)));
    assert_ne!(org, other_org);
    assert_eq!(store.list_team_members(team.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn timeout_counts_from_running_not_from_dispatch() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Timeout From Running Agent"))
        .await
        .unwrap();
    let make = |key: &'static str| {
        let store = &store;
        let agent = &agent;
        async move {
            let execution = store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x", "payload": {}, "provenance": { "source": "test", "trusted": true, "receivedAt": chrono::Utc::now() } }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 300,
                    requested_by: Some(user),
                    idempotency_key: Some(key),
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap();
            store
                .claim_execution(execution.id, "worker-slow-checkout")
                .await
                .unwrap();
            execution
        }
    };
    let set = |id: uuid::Uuid, column: &'static str, interval: &'static str| {
        let store = &store;
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE executions SET {column} = now() - interval '{interval}' WHERE id = $1"
            )))
            .bind(id)
            .execute(store.pool())
            .await
            .unwrap();
        }
    };

    // Dispatched 15 minutes ago (slow checkout), only now reports running.
    let slow = make("slow-checkout").await;
    set(slow.id, "started_at", "15 minutes").await;
    store
        .mark_execution_running(slow.id, Some("s"))
        .await
        .unwrap();
    // Running for longer than timeout (300s) + grace (300s).
    let stuck = make("stuck-running").await;
    set(stuck.id, "started_at", "15 minutes").await;
    store
        .mark_execution_running(stuck.id, Some("s"))
        .await
        .unwrap();
    set(stuck.id, "running_started_at", "11 minutes").await;

    let reaped = store
        .reap_timed_out_executions(Some(org), 300)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1, "{reaped:?}");
    assert_eq!(reaped[0].id, stuck.id);
    assert_eq!(
        store.get_execution(org, slow.id).await.unwrap().status,
        "running"
    );
    // The dispatch time is untouched, so durations shown to users keep their meaning.
    let slow_after = store.get_execution(org, slow.id).await.unwrap();
    assert!(slow_after.started_at.unwrap() < chrono::Utc::now() - chrono::Duration::minutes(14));
}

#[tokio::test]
async fn running_team_runs_are_all_visited_by_keyset_paging() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let team = store
        .create_agent_team(
            org,
            &format!("Team {}", uuid::Uuid::now_v7()),
            "paging",
            &serde_json::json!({ "steps": [] }),
            Some(user),
        )
        .await
        .unwrap();
    let mut expected = std::collections::HashSet::new();
    for _ in 0..60 {
        let run = store
            .create_team_run(
                org,
                team.id,
                &serde_json::json!({ "task": "t" }),
                Some(user),
            )
            .await
            .unwrap();
        expected.insert(run.id);
    }
    let mut seen = std::collections::HashSet::new();
    let mut after = None;
    loop {
        let page = store.list_running_team_runs(after, 50).await.unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.id);
        seen.extend(page.into_iter().map(|run| run.id));
    }
    assert!(expected.is_subset(&seen), "every running run is visited");
}

#[tokio::test]
async fn prune_audit_logs_deletes_in_batches() {
    let Some(store) = test_store().await else {
        return;
    };
    let _serial = AUDIT_PRUNE_TESTS.lock().await;
    let (org, _) = seed_org(&store).await;
    // More than one batch (5000) of old rows plus one recent row.
    sqlx::query(
        "INSERT INTO audit_logs (org_id, action, created_at)
         SELECT $1, 'test.bulk', now() - interval '400 days' + (g * interval '1 second')
         FROM generate_series(1, 5100) g",
    )
    .bind(org)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO audit_logs (org_id, action) VALUES ($1, 'test.keep')")
        .bind(org)
        .execute(store.pool())
        .await
        .unwrap();
    let removed = store
        .prune_audit_logs(chrono::Utc::now() - chrono::Duration::days(365))
        .await
        .unwrap();
    assert!(removed >= 5100, "removed {removed}");
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE org_id = $1")
        .bind(org)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn unparseable_tool_policies_are_enforced_as_deny() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, _user) = seed_org(&store).await;
    let (agent, _version) = store
        .create_agent(org, None, &definition("Corrupt policy target"))
        .await
        .expect("agent");

    // A matcher that no longer deserializes (drifted shape). Unknown actions cannot be
    // stored (CHECK constraint); the parse fallback for them is the same code path.
    let bad_matcher = store
        .create_tool_policy(
            org,
            None,
            "bad-matcher",
            0,
            true,
            "allow",
            &serde_json::json!({ "tool": 5 }),
            None,
            None,
        )
        .await
        .unwrap();
    let rules = store
        .effective_tool_policy_rules(org, agent.id)
        .await
        .unwrap();
    assert_eq!(rules.len(), 1, "unparseable rules must not be dropped");
    for rule in &rules {
        assert_eq!(rule.action, orbit_domain::ToolPolicyAction::Deny);
        assert!(rule.reason.is_some());
    }
    let matcher_rule = rules.iter().find(|r| r.id == bad_matcher.id).unwrap();
    assert!(
        matcher_rule.matcher.is_empty(),
        "unreadable scope covers every call"
    );
}

/// Records what the store reports, keyed by execution id.
#[derive(Default)]
struct RecordingObserver {
    seen: std::sync::Mutex<Vec<(uuid::Uuid, String)>>,
}

impl RecordingObserver {
    fn for_execution(&self, id: uuid::Uuid) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, _)| *seen == id)
            .map(|(_, what)| what.clone())
            .collect()
    }
}

impl orbit_store::observe::ExecutionObserver for RecordingObserver {
    fn created(&self, execution: &orbit_store::ExecutionRecord) {
        self.seen
            .lock()
            .unwrap()
            .push((execution.id, "created".into()));
    }

    fn started(&self, execution: &orbit_store::ExecutionRecord) {
        self.seen
            .lock()
            .unwrap()
            .push((execution.id, "started".into()));
    }

    fn finished(
        &self,
        execution: &orbit_store::ExecutionRecord,
        cause: orbit_store::observe::FinishCause,
    ) {
        self.seen.lock().unwrap().push((
            execution.id,
            format!("finished:{}:{cause:?}", execution.status),
        ));
    }
}

#[tokio::test]
async fn execution_transitions_are_reported_once_including_reaped_runs() {
    let Some(store) = test_store().await else {
        return;
    };
    let observer = Arc::new(RecordingObserver::default());
    let store = store.with_execution_observer(observer.clone());
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Observed Agent"))
        .await
        .unwrap();
    let make_execution = |key: &'static str| {
        let store = &store;
        let agent = &agent;
        let version = &version;
        async move {
            store
                .create_execution(NewExecution {
                    org_id: org,
                    agent_id: agent.id,
                    agent_version_id: version.id,
                    agent_name: &agent.name,
                    harness_kind: HarnessKind::Mock,
                    trigger_source: TriggerSource::Manual,
                    placement: "auto",
                    trigger_id: None,
                    input: serde_json::json!({ "task": "x" }),
                    session_ref: None,
                    skills: &[],
                    timeout_seconds: 30,
                    requested_by: Some(user),
                    idempotency_key: Some(key),
                    team_run_id: None,
                    team_step_index: None,
                })
                .await
                .unwrap()
        }
    };

    // Reported by its worker; a resume after an approval is not a second start.
    let reported = make_execution("observed-reported").await;
    assert_eq!(make_execution("observed-reported").await.id, reported.id);
    store.claim_execution(reported.id, "w1").await.unwrap();
    store
        .mark_execution_running(reported.id, None)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE executions SET status = 'awaiting_approval', \
         running_started_at = now() - interval '5 seconds' WHERE id = $1",
    )
    .bind(reported.id)
    .execute(store.pool())
    .await
    .unwrap();
    assert!(store
        .mark_execution_running(reported.id, None)
        .await
        .unwrap()
        .is_some());
    store
        .finish_execution(
            reported.id,
            ExecutionStatus::Succeeded,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
        .expect("finished");
    assert_eq!(
        observer.for_execution(reported.id),
        ["created", "started", "finished:succeeded:Reported"]
    );

    // Cancelled while queued; cancelling again reports nothing new.
    let cancelled = make_execution("observed-cancelled").await;
    store
        .request_execution_cancel(org, cancelled.id)
        .await
        .unwrap();
    store
        .request_execution_cancel(org, cancelled.id)
        .await
        .unwrap();
    assert_eq!(
        observer.for_execution(cancelled.id),
        ["created", "finished:cancelled:CancelledWhileQueued"]
    );

    // Lost with its worker.
    let lost = make_execution("observed-lost").await;
    store
        .claim_execution(lost.id, "worker-vanished")
        .await
        .unwrap();
    store.mark_execution_running(lost.id, None).await.unwrap();
    sqlx::query("UPDATE executions SET created_at = now() - interval '1 hour', started_at = now() - interval '1 hour' WHERE id = $1")
        .bind(lost.id)
        .execute(store.pool())
        .await
        .unwrap();
    // Taken by a live worker that never started it. The id is unique because a worker
    // row belongs to the token that first reported it, and the database is reused.
    let silent_worker = format!("worker-silent-{}", uuid::Uuid::now_v7());
    let unstarted = make_execution("observed-unstarted").await;
    store
        .claim_execution(unstarted.id, &silent_worker)
        .await
        .unwrap();
    store
        .upsert_worker_heartbeat(
            org,
            uuid::Uuid::now_v7(),
            &silent_worker,
            &["mock".into()],
            "local",
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE executions SET updated_at = now() - interval '1 hour', started_at = now() - interval '1 hour', created_at = now() - interval '1 hour' WHERE id = $1")
        .bind(unstarted.id)
        .execute(store.pool())
        .await
        .unwrap();

    // While both are still open, the snapshot counts them (the database is shared with
    // other tests, so these are lower bounds).
    let snapshot = store.metrics_snapshot(90).await.unwrap();
    let in_flight = |status: &str| {
        snapshot
            .executions_in_flight
            .iter()
            .find(|(name, _)| name == status)
            .map(|(_, count)| *count)
            .unwrap()
    };
    assert_eq!(snapshot.executions_in_flight.len(), 4);
    assert!(in_flight("running") >= 1);
    assert!(in_flight("dispatched") >= 1);
    assert!(snapshot.workers_online >= 1);
    assert_eq!(snapshot.worker_launches.len(), 3);
    let usage: Vec<&str> = snapshot
        .license_usage
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(usage, ["seats", "agents", "workers", "executions_per_day"]);
    assert!(snapshot.license_usage[1].1 >= 1);
    assert!(snapshot.license_usage[3].1 >= 4);

    let reaped = store
        .reap_orphaned_executions_for_org(org, 60)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1);
    let ended = store
        .reap_unstarted_executions(Some(org), 600, 60)
        .await
        .unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        observer.for_execution(lost.id),
        ["created", "started", "finished:failed:WorkerLost"]
    );
    assert_eq!(
        observer.for_execution(unstarted.id),
        ["created", "finished:failed:NeverStarted"]
    );
}

#[tokio::test]
async fn installation_wide_execution_count_uses_created_at_index() {
    let Some(store) = test_store().await else {
        eprintln!("skipping: ORBIT_TEST_DATABASE_URL not set");
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("License Count Agent"))
        .await
        .unwrap();

    let before = store
        .count_all_executions_since_installation(chrono::Utc::now() - chrono::Duration::hours(24))
        .await
        .unwrap();
    store
        .create_execution(NewExecution {
            org_id: org,
            agent_id: agent.id,
            agent_version_id: version.id,
            agent_name: &agent.name,
            harness_kind: HarnessKind::Mock,
            trigger_source: TriggerSource::Manual,
            placement: "auto",
            trigger_id: None,
            input: serde_json::json!({ "task": "license count" }),
            session_ref: None,
            skills: &[],
            timeout_seconds: 30,
            requested_by: Some(user),
            idempotency_key: None,
            team_run_id: None,
            team_step_index: None,
        })
        .await
        .unwrap();
    let after = store
        .count_all_executions_since_installation(chrono::Utc::now() - chrono::Duration::hours(24))
        .await
        .unwrap();
    assert!(
        after > before,
        "the installation-wide count must include the new execution: {before} -> {after}"
    );

    let pool = store.pool();
    let index: Option<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema() \
         AND tablename = 'executions' AND indexname = 'executions_created_at_idx'",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    assert!(
        index.is_some(),
        "migrations must create the executions(created_at) index"
    );

    // A tiny or shared test database may still favour a sequential scan. Disable it for
    // this plan only to prove the index is usable by the exact license-count predicate.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let lines: Vec<String> = sqlx::query_scalar(
        "EXPLAIN SELECT COUNT(*) FROM executions WHERE created_at >= now() - interval '24 hours'",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let plan = lines.join("\n");
    assert!(
        plan.contains("executions_created_at_idx"),
        "expected an index plan, got: {plan}"
    );
    assert!(
        !plan.contains("Seq Scan"),
        "unexpected sequential scan: {plan}"
    );
    tx.rollback().await.unwrap();
}

// ---------------------------------------------------------------------------
// Per-execution liveness and worker boot identity (#342)
// ---------------------------------------------------------------------------

async fn worker_token(
    store: &Store,
    org: uuid::Uuid,
    name: &str,
) -> orbit_store::WorkerTokenRecord {
    let issued = orbit_security::issue_worker_token();
    store
        .create_worker_token(org, None, name, &issued.prefix, &issued.hash, &[])
        .await
        .unwrap()
}

#[tokio::test]
async fn worker_boot_change_fails_executions_from_the_previous_boot() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Boot Change Agent"))
        .await
        .unwrap();
    let token = worker_token(&store, org, "boot-change").await;
    let worker_id = format!("worker-boot-{}", uuid::Uuid::now_v7());
    let execution = queued_execution(&store, org, user, &agent, &version).await;
    store
        .claim_execution_for_launch(execution.id, &worker_id, token.id)
        .await
        .unwrap()
        .unwrap();

    let old_boot = uuid::Uuid::now_v7();
    let new_boot = uuid::Uuid::now_v7();
    let old_started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let touched = store
        .touch_execution_leases(
            org,
            token.id,
            &worker_id,
            Some(old_boot),
            Some(old_started_at),
            &[execution.id],
            60,
        )
        .await
        .unwrap();
    assert_eq!(touched, vec![execution.id]);

    let approval = store
        .create_approval(
            org,
            execution.id,
            serde_json::json!({ "capability": "issues.comment" }),
            None,
        )
        .await
        .unwrap();

    let first = store
        .upsert_worker_heartbeat_with_boot(
            org,
            token.id,
            &worker_id,
            &["mock".into()],
            "local",
            Some("host"),
            Some(old_boot),
            Some(old_started_at),
        )
        .await
        .unwrap();
    assert!(first.accepted);
    assert!(!first.boot_changed);
    assert!(first.failed_executions.is_empty());

    let second = store
        .upsert_worker_heartbeat_with_boot(
            org,
            token.id,
            &worker_id,
            &["mock".into()],
            "local",
            Some("host"),
            Some(new_boot),
            Some(chrono::Utc::now()),
        )
        .await
        .unwrap();
    assert!(second.accepted);
    assert!(second.boot_changed);
    assert_eq!(second.failed_executions.len(), 1);
    let failed = &second.failed_executions[0];
    assert_eq!(failed.id, execution.id);
    let error = failed.error.as_ref().unwrap();
    assert_eq!(error["code"], "worker_lost");
    assert_eq!(
        error["message"],
        "worker restarted; execution outcome is unknown"
    );
    assert_eq!(error["retryable"].as_bool(), Some(true));
    assert!(failed.execution_lease_until.is_none());

    let stored = store.get_execution(org, execution.id).await.unwrap();
    assert_eq!(stored.status, "failed");
    assert!(stored.execution_lease_until.is_none());
    assert_eq!(
        store.get_approval(org, approval.id).await.unwrap().status,
        "expired"
    );
}

/// #343: a worker killed by the runtime cannot report its own failure, so every in-flight
/// execution on it must be findable — not only the ones already `running`.
#[tokio::test]
async fn active_executions_for_worker_covers_every_in_flight_state() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Terminated Agent"))
        .await
        .unwrap();
    let token = worker_token(&store, org, "terminated").await;
    let worker_id = format!("worker-term-{}", uuid::Uuid::now_v7());
    let other_worker = format!("worker-other-{}", uuid::Uuid::now_v7());

    let mut ids = Vec::new();
    for _ in 0..4 {
        let execution = queued_execution(&store, org, user, &agent, &version).await;
        store
            .claim_execution_for_launch(execution.id, &worker_id, token.id)
            .await
            .unwrap()
            .unwrap();
        ids.push(execution.id);
    }
    // ids[0] stays `dispatched` (a single-execution launch claims before the pod starts).
    store.mark_execution_running(ids[1], None).await.unwrap();
    sqlx::query("UPDATE executions SET status = 'awaiting_approval' WHERE id = $1")
        .bind(ids[2])
        .execute(store.pool())
        .await
        .unwrap();
    store
        .finish_execution(ids[3], ExecutionStatus::Failed, None, None, None, None)
        .await
        .unwrap();

    // Still queued, or claimed by a different worker: not this worker's in-flight work.
    let queued = queued_execution(&store, org, user, &agent, &version).await;
    let elsewhere = queued_execution(&store, org, user, &agent, &version).await;
    store
        .claim_execution_for_launch(elsewhere.id, &other_worker, token.id)
        .await
        .unwrap()
        .unwrap();

    let active = store
        .active_executions_for_worker(org, &worker_id)
        .await
        .unwrap();
    let mut found: Vec<uuid::Uuid> = active.iter().map(|execution| execution.id).collect();
    found.sort();
    let mut expected = vec![ids[0], ids[1], ids[2]];
    expected.sort();
    assert_eq!(found, expected);
    assert!(!found.contains(&queued.id));
    assert!(!found.contains(&elsewhere.id));
    assert!(!found.contains(&ids[3]));
    assert!(store
        .active_executions_for_worker(uuid::Uuid::now_v7(), &worker_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn execution_leases_only_touch_listed_active_executions() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Lease Touch Agent"))
        .await
        .unwrap();
    let token = worker_token(&store, org, "lease-touch").await;
    let worker_id = format!("worker-lease-{}", uuid::Uuid::now_v7());

    let active = queued_execution(&store, org, user, &agent, &version).await;
    let ignored = queued_execution(&store, org, user, &agent, &version).await;
    let terminal = queued_execution(&store, org, user, &agent, &version).await;
    for execution in [&active, &ignored, &terminal] {
        store
            .claim_execution_for_launch(execution.id, &worker_id, token.id)
            .await
            .unwrap()
            .unwrap();
    }
    // A terminal execution is never renewed, even when explicitly listed.
    store
        .mark_execution_running(terminal.id, None)
        .await
        .unwrap();
    store
        .finish_execution(
            terminal.id,
            ExecutionStatus::Succeeded,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let boot = uuid::Uuid::now_v7();
    let started_at = chrono::Utc::now();
    let touched = store
        .touch_execution_leases(
            org,
            token.id,
            &worker_id,
            Some(boot),
            Some(started_at),
            &[active.id, terminal.id],
            60,
        )
        .await
        .unwrap();
    assert_eq!(touched, vec![active.id]);

    let renewed = store.get_execution(org, active.id).await.unwrap();
    assert!(renewed.last_execution_heartbeat_at.is_some());
    assert!(renewed.execution_lease_until.is_some());
    assert_eq!(renewed.worker_boot_id, Some(boot));
    assert!(renewed.worker_boot_started_at.is_some());

    for untouched in [&ignored, &terminal] {
        let row = store.get_execution(org, untouched.id).await.unwrap();
        assert!(row.last_execution_heartbeat_at.is_none());
        assert!(row.execution_lease_until.is_none());
        assert!(row.worker_boot_id.is_none());
        assert!(row.worker_boot_started_at.is_none());
    }
}

#[tokio::test]
async fn expired_execution_leases_are_reaped_only_when_the_worker_is_alive() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Lease Reaper Agent"))
        .await
        .unwrap();
    let live_token = worker_token(&store, org, "lease-live").await;
    let quiet_token = worker_token(&store, org, "lease-quiet").await;
    let live_worker = format!("worker-live-{}", uuid::Uuid::now_v7());
    let quiet_worker = format!("worker-quiet-{}", uuid::Uuid::now_v7());

    let live = queued_execution(&store, org, user, &agent, &version).await;
    let quiet = queued_execution(&store, org, user, &agent, &version).await;
    store
        .claim_execution_for_launch(live.id, &live_worker, live_token.id)
        .await
        .unwrap()
        .unwrap();
    store
        .claim_execution_for_launch(quiet.id, &quiet_worker, quiet_token.id)
        .await
        .unwrap()
        .unwrap();

    let approval = store
        .create_approval(
            org,
            live.id,
            serde_json::json!({ "capability": "issues.comment" }),
            None,
        )
        .await
        .unwrap();

    // A negative lease puts both runs in the past; only the live worker's run is reaped.
    store
        .touch_execution_leases(
            org,
            live_token.id,
            &live_worker,
            None,
            None,
            &[live.id],
            -60,
        )
        .await
        .unwrap();
    store
        .touch_execution_leases(
            org,
            quiet_token.id,
            &quiet_worker,
            None,
            None,
            &[quiet.id],
            -60,
        )
        .await
        .unwrap();
    store
        .upsert_worker_heartbeat(
            org,
            live_token.id,
            &live_worker,
            &["mock".into()],
            "local",
            Some("host"),
        )
        .await
        .unwrap();

    let reaped = store.reap_expired_execution_leases(60).await.unwrap();
    let reaped_ids: Vec<uuid::Uuid> = reaped.iter().map(|row| row.id).collect();
    assert!(reaped_ids.contains(&live.id));
    assert!(!reaped_ids.contains(&quiet.id));
    let failed = reaped.iter().find(|row| row.id == live.id).unwrap();
    let error = failed.error.as_ref().unwrap();
    assert_eq!(error["code"], "harness_unresponsive");
    assert_eq!(
        error["message"],
        "worker stopped reporting this execution; outcome is unknown"
    );
    assert_eq!(error["retryable"].as_bool(), Some(true));
    assert!(failed.execution_lease_until.is_none());

    assert_eq!(
        store.get_execution(org, quiet.id).await.unwrap().status,
        "dispatched",
        "a worker that went quiet must be reported as worker_lost instead"
    );
    assert_eq!(
        store.get_approval(org, approval.id).await.unwrap().status,
        "expired"
    );
}

#[tokio::test]
async fn foreign_worker_token_cannot_renew_an_execution_lease() {
    let Some(store) = test_store().await else {
        return;
    };
    let (org, user) = seed_org(&store).await;
    let (agent, version) = store
        .create_agent(org, Some(user), &definition("Lease Ownership Agent"))
        .await
        .unwrap();
    let owner = worker_token(&store, org, "lease-owner").await;
    let foreign = worker_token(&store, org, "lease-foreign").await;
    let worker_id = format!("worker-owned-{}", uuid::Uuid::now_v7());
    let execution = queued_execution(&store, org, user, &agent, &version).await;
    store
        .claim_execution_for_launch(execution.id, &worker_id, owner.id)
        .await
        .unwrap()
        .unwrap();

    let boot = uuid::Uuid::now_v7();
    let started_at = chrono::Utc::now();
    let refused = store
        .touch_execution_leases(
            org,
            foreign.id,
            &worker_id,
            Some(boot),
            Some(started_at),
            &[execution.id],
            60,
        )
        .await
        .unwrap();
    assert!(refused.is_empty());

    let untouched = store.get_execution(org, execution.id).await.unwrap();
    assert!(untouched.last_execution_heartbeat_at.is_none());
    assert!(untouched.execution_lease_until.is_none());
    assert!(untouched.worker_boot_id.is_none());
    assert!(untouched.worker_boot_started_at.is_none());

    let renewed = store
        .touch_execution_leases(
            org,
            owner.id,
            &worker_id,
            Some(boot),
            Some(started_at),
            &[execution.id],
            60,
        )
        .await
        .unwrap();
    assert_eq!(renewed, vec![execution.id]);
}
