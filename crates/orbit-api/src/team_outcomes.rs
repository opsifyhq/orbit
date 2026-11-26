//! Close-the-loop for Agent Team queue items.
//!
//! When a TeamRun terminates, mirror its outcome back to the source issue through the
//! configured connector: remove the claim label, add a done/failed label and (on success)
//! post a comment with any PR/MR links found in the execution output.

use serde_json::Value;

use orbit_store::team_queue::TeamQueueItem;
use orbit_store::TeamRun;

use crate::handlers::teams::{parse_definition, TeamDefinition};
use crate::state::AppState;
use crate::work_sources::{self, WorkSourceDefinition};

struct WorkSourceConnector {
    source: WorkSourceDefinition,
    /// The connection's config, for its `allowedRepositories` / `allowedProjects` (#384).
    connection_config: Value,
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

/// After this long, give up retrying source-side side effects (labels/comments) and just
/// close the queue item, so a permanently broken connection cannot pin it `running`.
const OUTCOME_RETRY_WINDOW_SECS: i64 = 3600;

/// Apply a terminal run's outcome to the queue item and its source issue.
///
/// Ordering is what makes this retryable: the source-side side effects run first and the
/// queue item is only closed (status leaves `running`) once they succeeded. Any error is
/// returned with the item still `running` and the run terminal, which is exactly the
/// marker `list_team_runs_pending_outcome` selects, so a later tick retries. Side effects
/// are idempotent (labels are recomputed from the issue's current labels, and an issue that
/// already carries the done label is not commented on again).
pub(crate) async fn close_queue_loop(
    state: &AppState,
    run: &TeamRun,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
) -> anyhow::Result<()> {
    close_queue_loop_inner(state, run, status, output, error, true).await
}

/// An operator cancelled the run: mirror the `cancelled` outcome to the source issue
/// (claim label removed, `failedLabel` added) and close the queue item (#393). Errors
/// are logged, not returned: the item then stays `running` next to a terminal run, which
/// is what `reconcile_team_outcomes` retries.
pub(crate) async fn close_cancelled_queue_loop(state: &AppState, run: &TeamRun, reason: &str) {
    if let Err(error) =
        close_queue_loop_inner(state, run, "cancelled", None, Some(reason), false).await
    {
        tracing::warn!(%error, run_id = %run.id, "could not apply the cancelled outcome yet; it is retried");
    }
}

/// Close the queue item of a run ended by the platform without offering a retry: the
/// worker lost the execution, so replaying it would duplicate unknown side effects.
pub(crate) async fn close_queue_loop_without_retry(
    state: &AppState,
    run: &TeamRun,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
) -> anyhow::Result<()> {
    close_queue_loop_inner(state, run, status, output, error, false).await
}

async fn close_queue_loop_inner(
    state: &AppState,
    run: &TeamRun,
    status: &str,
    output: Option<&Value>,
    error: Option<&str>,
    retry_allowed: bool,
) -> anyhow::Result<()> {
    let Some(item) = state.store.get_team_queue_item_by_run(run.id).await? else {
        return Ok(());
    };
    if item.status != "running" {
        return Ok(());
    }
    let expired = run
        .finished_at
        .map(|at| (chrono::Utc::now() - at).num_seconds() > OUTCOME_RETRY_WINDOW_SECS)
        .unwrap_or(false);
    let definition = definition_for_run(state, run).await;
    let source = definition
        .as_ref()
        .and_then(|definition| definition.work_source.clone());
    let connector = match source {
        Some(source) => match connector_for_work_source(state, run, &source).await {
            Ok(connector) => connector,
            // A deleted connection can never come back: skip side effects, close the item.
            Err(error)
                if matches!(
                    error.downcast_ref::<orbit_store::StoreError>(),
                    Some(orbit_store::StoreError::NotFound(_))
                ) =>
            {
                tracing::warn!(%error, queue_item = %item.id, "work source connection is gone; skipping outcome side effects");
                None
            }
            // Lost access is not retried: the credential is not used on the owner's behalf.
            Err(error) if error.is::<crate::work_sources::WorkSourceAccessLost>() => {
                tracing::warn!(%error, queue_item = %item.id, "skipping outcome side effects");
                None
            }
            Err(error) if expired => {
                tracing::warn!(%error, queue_item = %item.id, "giving up on outcome side effects");
                None
            }
            Err(error) => return Err(error),
        },
        None => None,
    };

    let retry = retry_allowed && status == "failed" && item.max_attempts > item.attempts + 1;
    let outcome = match status {
        "succeeded" => "done",
        "failed" if retry => "retry",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => return Ok(()),
    };
    if let Some(connector) = &connector {
        let applied = apply_outcome(
            connector,
            &item,
            outcome,
            if outcome == "done" { output } else { None },
        )
        .await;
        match applied {
            Ok(()) => {}
            // A foreign target never becomes valid: close the item without side effects.
            Err(error) if error.is::<crate::work_sources::WorkSourceTargetRefused>() => {
                tracing::warn!(%error, queue_item = %item.id, "skipping outcome side effects");
            }
            Err(error) if expired => {
                tracing::warn!(%error, queue_item = %item.id, "giving up on outcome side effects");
            }
            Err(error) => return Err(error),
        }
    }
    match status {
        "succeeded" => {
            state
                .store
                .close_team_queue_item_by_run(run.id, "succeeded", None)
                .await?;
        }
        "failed" if retry => {
            state
                .store
                .release_team_queue_item_for_retry(run.id, error)
                .await?;
        }
        "failed" => {
            state
                .store
                .close_team_queue_item_by_run(run.id, "failed", error)
                .await?;
        }
        _ => {
            state
                .store
                .close_team_queue_item_by_run(run.id, "cancelled", error)
                .await?;
        }
    }
    Ok(())
}

async fn definition_for_run(state: &AppState, run: &TeamRun) -> Option<TeamDefinition> {
    if let Some(value) = run.input.get("definition") {
        if let Ok(definition) = serde_json::from_value::<TeamDefinition>(value.clone()) {
            return Some(definition);
        }
    }
    let team = state
        .store
        .get_agent_team(run.org_id, run.team_id)
        .await
        .ok()?;
    parse_definition(&team).ok()
}

async fn connector_for_work_source(
    state: &AppState,
    run: &TeamRun,
    source: &WorkSourceDefinition,
) -> anyhow::Result<Option<WorkSourceConnector>> {
    if !source.enabled {
        return Ok(None);
    }
    let Some(connection_id) = source.connection_id else {
        return Ok(None);
    };
    let team = state.store.get_agent_team(run.org_id, run.team_id).await?;
    let connection =
        crate::work_sources::work_source_connection(state, &team, connection_id).await?;
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
    orbit_security::check_outbound_url(&base_url, &state.outbound)
        .await
        .map_err(|error| anyhow::anyhow!("work source baseUrl refused: {error}"))?;
    Ok(Some(WorkSourceConnector {
        source: source.clone(),
        connection_config: connection.config.clone(),
        base_url,
        token,
        client: orbit_security::guarded_client_with_policy(
            std::time::Duration::from_secs(20),
            state.outbound.clone(),
        ),
    }))
}

async fn apply_outcome(
    connector: &WorkSourceConnector,
    item: &TeamQueueItem,
    outcome: &str,
    output: Option<&Value>,
) -> anyhow::Result<()> {
    let number = parse_number(&item.external_key)?;
    match connector.source.provider.as_str() {
        "github" => {
            let repository = item
                .repository
                .clone()
                .or_else(|| connector.source.repository.clone())
                .ok_or_else(|| anyhow::anyhow!("queue item has no repository"))?;
            ensure_target_allowed(connector, &repository)?;
            let current = current_labels(connector, &repository, number, None).await?;
            // The done label is written last, so its presence means the outcome (comment
            // included) was fully applied by an earlier attempt.
            if outcome == "done" && current.contains(&connector.source.done_label) {
                return Ok(());
            }
            let labels = github_outcome_labels(&connector.source, &current, outcome);
            if outcome == "done" {
                let body = success_comment(output);
                work_sources::comment_github(
                    &connector.client,
                    &connector.base_url,
                    connector.token.as_deref(),
                    &repository,
                    number,
                    &body,
                )
                .await?;
            }
            work_sources::update_github_labels(
                &connector.client,
                &connector.base_url,
                connector.token.as_deref(),
                &repository,
                number,
                &labels,
            )
            .await?;
        }
        "gitlab" => {
            let project = item
                .repository
                .clone()
                .or_else(|| connector.source.project.clone())
                .ok_or_else(|| anyhow::anyhow!("queue item has no GitLab project"))?;
            ensure_target_allowed(connector, &project)?;
            let current = current_labels(connector, &project, number, Some(&project)).await?;
            // The done label is written last, so its presence means the outcome (comment
            // included) was fully applied by an earlier attempt.
            if outcome == "done" && current.contains(&connector.source.done_label) {
                return Ok(());
            }
            let (add, remove) = gitlab_outcome_labels(&connector.source, &current, outcome);
            if outcome == "done" {
                let body = success_comment(output);
                work_sources::comment_gitlab(
                    &connector.client,
                    &connector.base_url,
                    connector.token.as_deref(),
                    &project,
                    number,
                    &body,
                )
                .await?;
            }
            work_sources::update_gitlab_labels(
                &connector.client,
                &connector.base_url,
                connector.token.as_deref(),
                &project,
                number,
                &add,
                &remove,
            )
            .await?;
        }
        other => anyhow::bail!("unsupported work source provider '{other}'"),
    }
    Ok(())
}

/// The outcome writer only touches the work source's own repository or project, within
/// the connection allowlist; the item's stored repository is caller data (#384).
fn ensure_target_allowed(connector: &WorkSourceConnector, target: &str) -> anyhow::Result<()> {
    if crate::work_sources::work_source_allows_target(
        &connector.source,
        &connector.connection_config,
        target,
    ) {
        Ok(())
    } else {
        Err(crate::work_sources::WorkSourceTargetRefused {
            target: target.to_string(),
        }
        .into())
    }
}

async fn current_labels(
    connector: &WorkSourceConnector,
    target: &str,
    number: u64,
    project: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    if connector.source.provider == "github" {
        Ok(work_sources::get_github_issue(
            &connector.client,
            &connector.base_url,
            connector.token.as_deref(),
            target,
            number,
        )
        .await?
        .labels)
    } else {
        let project =
            project.ok_or_else(|| anyhow::anyhow!("GitLab outcome requires a project"))?;
        Ok(work_sources::get_gitlab_issue(
            &connector.client,
            &connector.base_url,
            connector.token.as_deref(),
            project,
            number,
        )
        .await?
        .labels)
    }
}

fn github_outcome_labels(
    source: &WorkSourceDefinition,
    current: &[String],
    outcome: &str,
) -> Vec<String> {
    let mut labels: Vec<String> = current
        .iter()
        .filter(|label| *label != &source.claim_label && !source.labels.contains(label))
        .cloned()
        .collect();
    match outcome {
        "done" => labels.push(source.done_label.clone()),
        "failed" | "cancelled" => labels.push(source.failed_label.clone()),
        "retry" => labels.extend(source.labels.clone()),
        _ => {}
    }
    labels.sort();
    labels.dedup();
    labels
}

fn gitlab_outcome_labels(
    source: &WorkSourceDefinition,
    current: &[String],
    outcome: &str,
) -> (Vec<String>, Vec<String>) {
    let mut add = Vec::new();
    let mut remove = vec![source.claim_label.clone()];
    match outcome {
        "done" => add.push(source.done_label.clone()),
        "failed" | "cancelled" => add.push(source.failed_label.clone()),
        "retry" => {
            remove.extend(source.labels.clone());
            add.extend(source.labels.clone());
        }
        _ => {}
    }
    let _ = current;
    (add, remove)
}

fn parse_number(external_key: &str) -> anyhow::Result<u64> {
    external_key
        .rsplit_once('#')
        .and_then(|(_, number)| number.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("external key has no numeric issue id"))
}

fn success_comment(output: Option<&Value>) -> String {
    let links = collect_links(output);
    let mut body = "ORBIT agent team completed this issue.".to_string();
    if !links.is_empty() {
        body.push_str("\n\nLinked changes:\n");
        for link in links {
            body.push_str(&format!("- {link}\n"));
        }
    }
    body
}

fn collect_links(value: Option<&Value>) -> Vec<String> {
    fn walk(value: &Value, links: &mut Vec<String>) {
        match value {
            Value::String(value) => {
                if (value.contains("pull/") || value.contains("merge_requests"))
                    && (value.starts_with("http://") || value.starts_with("https://"))
                {
                    links.push(value.clone());
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, links);
                }
            }
            Value::Object(map) => {
                for (_key, value) in map {
                    walk(value, links);
                }
            }
            _ => {}
        }
    }
    let mut links = Vec::new();
    if let Some(value) = value {
        walk(value, &mut links);
    }
    links.sort();
    links.dedup();
    links.truncate(10);
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_label_transitions() {
        let source = WorkSourceDefinition {
            labels: vec!["agent-ready".into()],
            ..Default::default()
        };
        let current = vec!["agent-ready".into(), "agent-in-progress".into()];
        let done = github_outcome_labels(&source, &current, "done");
        assert!(done.contains(&"agent-done".into()));
        assert!(!done.contains(&"agent-ready".into()));
        assert!(!done.contains(&"agent-in-progress".into()));

        let failed = github_outcome_labels(&source, &current, "failed");
        assert!(failed.contains(&"agent-failed".into()));
    }

    #[test]
    fn extracts_links_from_output() {
        let output = serde_json::json!({
            "pullRequests": [{ "htmlUrl": "https://github.com/acme/orbit/pull/12" }]
        });
        assert_eq!(
            collect_links(Some(&output)),
            vec!["https://github.com/acme/orbit/pull/12".to_string()]
        );
    }
}
