//! Agent Team work-source dispatcher.
//!
//! Runs in the control plane (`orbit-api`) and turns matching GitHub/GitLab issues into
//! durable queue items. A per-team Postgres advisory lock guarantees that two API
//! replicas never dispatch the same issue for the same team at the same time; the
//! `(team_id, external_key)` unique constraint guarantees it is never enqueued twice.

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use orbit_security::AuthContext;
use orbit_store::team_queue::TeamQueueItem;
use orbit_store::AgentTeam;

use crate::handlers::teams::{parse_definition, TeamDefinition};
use crate::state::AppState;
use crate::work_sources::{self, WorkSourceDefinition};

pub async fn dispatch_tick(state: &AppState) -> anyhow::Result<()> {
    let teams = state
        .store
        .list_agent_teams_with_enabled_work_source()
        .await?;
    for team in teams {
        if let Err(error) = dispatch_team(state, team).await {
            tracing::warn!(error = %error, "work source dispatch failed");
        }
    }
    Ok(())
}

/// Take the per-team dispatch lock for the lifetime of the returned transaction.
async fn try_team_lock(
    pool: &sqlx::PgPool,
    team_id: Uuid,
) -> anyhow::Result<Option<sqlx::Transaction<'static, sqlx::Postgres>>> {
    let mut tx = pool.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(advisory_lock_id(team_id))
        .fetch_one(&mut *tx)
        .await?;
    Ok(locked.then_some(tx))
}

async fn dispatch_team(state: &AppState, team: AgentTeam) -> anyhow::Result<()> {
    let definition = parse_definition(&team).map_err(|error| anyhow::anyhow!(error.message))?;
    let Some(source) = definition.work_source.clone() else {
        return Ok(());
    };
    if !source.enabled {
        return Ok(());
    }
    if !poll_due(&team, &source) {
        return Ok(());
    }

    // Transaction-scoped advisory lock: released on commit, rollback or when the
    // transaction is dropped (including a panic or cancelled task), so a failure can never
    // return a pooled connection that still holds the lock.
    let Some(lock) = try_team_lock(state.store.pool(), team.id).await? else {
        tracing::debug!(team_id = %team.id, "work source dispatch skipped: lock held by another replica");
        return Ok(());
    };

    let result = dispatch_team_locked(state, &team, &definition, &source).await;
    if let Err(error) = &result {
        // Record the failure so the next attempt waits for `pollSeconds` instead of
        // retrying the connector on every tick.
        let _ = state
            .store
            .set_team_poll_status(team.org_id, team.id, Some(&error.to_string()))
            .await;
    }
    drop(lock);
    result
}

async fn dispatch_team_locked(
    state: &AppState,
    team: &AgentTeam,
    definition: &TeamDefinition,
    source: &WorkSourceDefinition,
) -> anyhow::Result<()> {
    let existing = state
        .store
        .list_team_queue_external_keys(team.id, 5000)
        .await?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    let connector = WorkSourceConnection::new(state, team.org_id, source).await?;
    let client = connector.client.clone();
    let base_url = connector.base_url.clone();
    let token = connector.token.clone();

    let issues = match source.provider.as_str() {
        "github" => {
            work_sources::list_github_issues(&client, &base_url, token.as_deref(), source).await?
        }
        "gitlab" => {
            work_sources::list_gitlab_issues(&client, &base_url, token.as_deref(), source).await?
        }
        other => anyhow::bail!("unsupported work source provider '{other}'"),
    };

    let mut last_error: Option<String> = None;
    let mut dispatched = 0usize;
    for issue in issues {
        if existing.contains(&issue.key) {
            continue;
        }
        if dispatched >= source.max_items.max(1) as usize {
            break;
        }
        let payload = serde_json::json!({
            "task": issue.title,
            "url": issue.url,
            "provider": source.provider,
            "number": issue.number,
            "labels": issue.labels,
        });
        let repository = issue
            .repository
            .clone()
            .or_else(|| issue.project.clone())
            .or_else(|| source.repository.clone())
            .or_else(|| source.project.clone());
        // Enqueue first, claim second: a database error before the claim leaves the issue
        // untouched (it still matches the source labels and is picked up on the next poll).
        let claimed = enqueue_then_claim(
            &state.store,
            team,
            &issue.key,
            repository.as_deref(),
            &payload,
            source.max_attempts.clamp(1, 10) as i32,
            || async {
                match source.provider.as_str() {
                    "github" => {
                        claim_github(&client, &base_url, token.as_deref(), source, &issue).await
                    }
                    "gitlab" => {
                        claim_gitlab(&client, &base_url, token.as_deref(), source, &issue).await
                    }
                    _ => unreachable!(),
                }
            },
        )
        .await?;
        match claimed {
            EnqueueOutcome::Enqueued => dispatched += 1,
            EnqueueOutcome::AlreadyQueued => {}
            EnqueueOutcome::ClaimFailed(error) => last_error = Some(error),
        }
    }

    if let Some(error) = last_error.as_deref() {
        state
            .store
            .set_team_poll_status(team.org_id, team.id, Some(error))
            .await?;
    } else {
        state
            .store
            .set_team_poll_status(team.org_id, team.id, None)
            .await?;
    }

    if source.auto_run {
        auto_run_pending(state, team, definition, source).await?;
    }
    Ok(())
}

async fn auto_run_pending(
    state: &AppState,
    team: &AgentTeam,
    _definition: &TeamDefinition,
    source: &WorkSourceDefinition,
) -> anyhow::Result<()> {
    let max = source.max_items.clamp(1, 100) as usize;
    let pending = state
        .store
        .list_pending_team_queue(team.org_id, team.id, (max * 2) as i64)
        .await?;
    let auth = crate::bootstrap::system_context(team.org_id);
    let mut started = 0usize;
    for item in pending {
        if started >= max {
            break;
        }
        // `start_queue_item` claims the item atomically and, on failure, records the error
        // and returns the item to a retryable state itself. It enforces executions/day via
        // `start_step`, so dispatcher auto-run cannot bypass the license limit.
        if let Err(error) = start_queue_item(state, &auth, team, &item).await {
            tracing::warn!(
                error = %error,
                queue_item = %item.id,
                "failed to auto-run queue item"
            );
            continue;
        }
        started += 1;
    }
    Ok(())
}

/// Result of [`enqueue_then_claim`].
#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued,
    AlreadyQueued,
    ClaimFailed(String),
}

/// Persist the queue item, then claim the issue at the source (which removes its source
/// labels). The order matters: if persisting fails nothing was claimed, so the issue is
/// not lost. If the claim fails the freshly enqueued item is withdrawn so the next poll
/// retries the whole thing.
pub async fn enqueue_then_claim<F, Fut>(
    store: &orbit_store::Store,
    team: &AgentTeam,
    external_key: &str,
    repository: Option<&str>,
    payload: &Value,
    max_attempts: i32,
    claim: F,
) -> anyhow::Result<EnqueueOutcome>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let Some(item) = store
        .try_enqueue_team_item(
            team.org_id,
            team.id,
            external_key,
            repository,
            payload,
            max_attempts,
        )
        .await?
    else {
        return Ok(EnqueueOutcome::AlreadyQueued);
    };
    if let Err(error) = claim().await {
        if let Err(undo) = store
            .delete_pending_team_queue_item(team.org_id, item.id)
            .await
        {
            tracing::warn!(error = %undo, queue_item = %item.id, "failed to withdraw queue item after claim failure");
        }
        return Ok(EnqueueOutcome::ClaimFailed(error.to_string()));
    }
    Ok(EnqueueOutcome::Enqueued)
}

async fn start_queue_item(
    state: &AppState,
    auth: &AuthContext,
    team: &AgentTeam,
    item: &TeamQueueItem,
) -> Result<crate::handlers::teams::TeamRunDetail, crate::error::ApiError> {
    crate::handlers::team_queue::start_queue_item(state, auth, team, item).await
}

struct WorkSourceConnection {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl WorkSourceConnection {
    async fn new(
        state: &AppState,
        org_id: Uuid,
        source: &WorkSourceDefinition,
    ) -> anyhow::Result<Self> {
        let connection_id = source
            .connection_id
            .ok_or_else(|| anyhow::anyhow!("work source has no connection"))?;
        let connection = state.store.get_connection(org_id, connection_id).await?;
        let token = match &connection.credential_ref {
            Some(reference) => state
                .store
                .get_secret_value(reference)
                .await?
                .map(|secret| secret.expose().to_string()),
            None => None,
        };
        let base_url = match source.provider.as_str() {
            "github" => connection
                .config
                .get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or("https://api.github.com")
                .trim_end_matches('/')
                .to_string(),
            "gitlab" => connection
                .config
                .get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or("https://gitlab.com/api/v4")
                .trim_end_matches('/')
                .to_string(),
            other => anyhow::bail!("unsupported work source provider '{other}'"),
        };
        orbit_security::check_outbound_url(&base_url, &orbit_security::OutboundPolicy::from_env())
            .await
            .map_err(|error| anyhow::anyhow!("work source baseUrl refused: {error}"))?;
        Ok(Self {
            base_url,
            token,
            client: orbit_security::guarded_client(std::time::Duration::from_secs(20)),
        })
    }
}

async fn claim_github(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    source: &WorkSourceDefinition,
    issue: &work_sources::ExternalIssue,
) -> anyhow::Result<()> {
    let repository = issue
        .repository
        .clone()
        .or_else(|| source.repository.clone())
        .ok_or_else(|| anyhow::anyhow!("GitHub issue has no repository"))?;
    let mut labels: Vec<String> = issue
        .labels
        .iter()
        .filter(|label| !source.labels.contains(label) && *label != &source.claim_label)
        .cloned()
        .collect();
    labels.push(source.claim_label.clone());
    labels.sort();
    labels.dedup();
    work_sources::update_github_labels(client, base_url, token, &repository, issue.number, &labels)
        .await?;
    Ok(())
}

async fn claim_gitlab(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
    source: &WorkSourceDefinition,
    issue: &work_sources::ExternalIssue,
) -> anyhow::Result<()> {
    let project = issue
        .project
        .clone()
        .or_else(|| source.project.clone())
        .ok_or_else(|| anyhow::anyhow!("GitLab issue has no project"))?;
    work_sources::update_gitlab_labels(
        client,
        base_url,
        token,
        &project,
        issue.number,
        std::slice::from_ref(&source.claim_label),
        &source.labels,
    )
    .await?;
    Ok(())
}

fn advisory_lock_id(team_id: Uuid) -> i64 {
    let bytes: [u8; 8] = team_id.as_bytes()[..8]
        .try_into()
        .expect("uuid has at least 8 bytes");
    i64::from_be_bytes(bytes)
}

fn poll_due(team: &AgentTeam, source: &WorkSourceDefinition) -> bool {
    match team.last_poll_at {
        None => true,
        Some(last_poll_at) => {
            let seconds = source.poll_seconds.clamp(5, 86_400);
            let due = last_poll_at + chrono::Duration::seconds(seconds);
            due <= Utc::now()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic while the lock is held must not leave it behind on a pooled connection.
    #[tokio::test]
    async fn team_lock_is_released_when_the_holder_panics() {
        let Ok(url) = std::env::var("ORBIT_TEST_DATABASE_URL") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(&url)
            .await
            .unwrap();
        let team_id = Uuid::now_v7();
        let held = try_team_lock(&pool, team_id).await.unwrap();
        assert!(held.is_some());
        assert!(
            try_team_lock(&pool, team_id).await.unwrap().is_none(),
            "second taker must see the lock held"
        );
        drop(held);
        // Holder task panics with the lock taken.
        let task_pool = pool.clone();
        let panicked = tokio::spawn(async move {
            let _lock = try_team_lock(&task_pool, team_id).await.unwrap().unwrap();
            panic!("boom");
        })
        .await;
        assert!(panicked.is_err());
        // Every pooled connection must be able to take the lock again.
        for _ in 0..3 {
            let lock = try_team_lock(&pool, team_id).await.unwrap();
            assert!(lock.is_some(), "lock leaked after panic");
        }
    }

    #[test]
    fn lock_id_is_stable() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap();
        assert_eq!(advisory_lock_id(id), 0);
        let other = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(advisory_lock_id(other), 0, "last 8 bytes are ignored");
    }

    #[test]
    fn poll_due_after_interval() {
        let mut team = AgentTeam {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            owner_user_id: None,
            owner_team_id: None,
            visibility: "org".into(),
            name: "t".into(),
            description: String::new(),
            definition: serde_json::json!({}),
            created_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_poll_at: None,
            last_poll_error: None,
        };
        let source = WorkSourceDefinition {
            poll_seconds: 60,
            ..Default::default()
        };
        assert!(poll_due(&team, &source));
        team.last_poll_at = Some(Utc::now());
        assert!(!poll_due(&team, &source));
        team.last_poll_at = Some(Utc::now() - chrono::Duration::seconds(61));
        assert!(poll_due(&team, &source));
    }
}
