//! "Used by" lookups for deletes (#434). Providers, connections and agents are referenced
//! from JSON (agent specs, team definitions, env mappings), so no foreign key stops a delete
//! that leaves the dependant pointing at nothing. Each delete runs these queries in its own
//! transaction and refuses with [`StoreError::InUse`] while anything still refers to the item.

use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{Store, StoreError, StoreResult, IN_USE_LIMIT};

/// What permanently deleting an agent would do: what blocks it, and what goes with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeleteImpact {
    /// Records that still use the agent. While any are listed, the delete is refused.
    pub used_by: Vec<String>,
    /// Records the delete removes or changes with the agent, such as "3 executions" or
    /// "output Slack alerts (deleted)".
    pub removes: Vec<String>,
}

pub(crate) async fn provider_used_by(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    provider_id: Uuid,
) -> StoreResult<Vec<String>> {
    let query = r#"
        WITH target AS (SELECT name FROM providers WHERE org_id = $1 AND id = $2)
        SELECT label FROM (
            SELECT 'agent ' || a.name AS label
            FROM agents a
            JOIN agent_versions v ON v.agent_id = a.id AND v.version = a.current_version
            JOIN target t ON v.spec -> 'model' ->> 'provider' = t.name
            WHERE a.org_id = $1
            UNION
            SELECT 'agent team ' || tm.name
            FROM agent_teams tm, target t
            WHERE tm.org_id = $1
              AND (tm.definition -> 'model' ->> 'provider' = t.name
                   OR EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(tm.definition -> 'steps') = 'array' THEN tm.definition -> 'steps' ELSE '[]'::jsonb END) s WHERE s -> 'model' ->> 'provider' = t.name)
                   OR EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(tm.definition -> 'workflow' -> 'nodes') = 'array' THEN tm.definition -> 'workflow' -> 'nodes' ELSE '[]'::jsonb END) n WHERE n -> 'model' ->> 'provider' = t.name))
            UNION
            SELECT 'worker profile ' || p.name
            FROM worker_profiles p, target t
            WHERE p.org_id = $1 AND p.provider = t.name
        ) used
        ORDER BY label
        LIMIT $3
        "#;
    Ok(sqlx::query_scalar(query)
        .bind(org_id)
        .bind(provider_id)
        .bind(IN_USE_LIMIT)
        .fetch_all(&mut **tx)
        .await?)
}

pub(crate) async fn connection_used_by(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    connection_id: Uuid,
) -> StoreResult<Vec<String>> {
    let query = r#"
        SELECT label FROM (
            SELECT 'agent ' || a.name AS label
            FROM agents a
            JOIN agent_versions v ON v.agent_id = a.id AND v.version = a.current_version
            WHERE a.org_id = $1
              AND EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(v.spec -> 'connectors') = 'array' THEN v.spec -> 'connectors' ELSE '[]'::jsonb END) c WHERE c ->> 'connectionId' = $2::text)
            UNION
            SELECT 'agent team ' || tm.name
            FROM agent_teams tm
            WHERE tm.org_id = $1
              AND (tm.definition -> 'connectionIds' ? $2::text
                   OR tm.definition -> 'workSource' ->> 'connectionId' = $2::text)
            UNION
            SELECT 'environment mapping ' || e.name
            FROM env_mappings e
            WHERE e.org_id = $1
              AND EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(e.mappings) = 'array' THEN e.mappings ELSE '[]'::jsonb END) m WHERE m ->> 'connectionId' = $2::text)
            UNION
            SELECT 'worker profile ' || p.name
            FROM worker_profiles p
            WHERE p.org_id = $1
              AND EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(p.env_mappings) = 'array' THEN p.env_mappings ELSE '[]'::jsonb END) m WHERE m ->> 'connectionId' = $2::text)
        ) used
        ORDER BY label
        LIMIT $3
        "#;
    Ok(sqlx::query_scalar(query)
        .bind(org_id)
        .bind(connection_id)
        .bind(IN_USE_LIMIT)
        .fetch_all(&mut **tx)
        .await?)
}

/// Agent teams (pipeline steps or workflow nodes) and worker profiles scoped to the agent.
/// Team run history is protected separately by the `team_steps` foreign key.
pub(crate) async fn agent_used_by(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    agent_id: Uuid,
) -> StoreResult<Vec<String>> {
    let query = r#"
        SELECT label FROM (
            SELECT 'agent team ' || tm.name AS label
            FROM agent_teams tm
            WHERE tm.org_id = $1
              AND (EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(tm.definition -> 'steps') = 'array' THEN tm.definition -> 'steps' ELSE '[]'::jsonb END) s WHERE s ->> 'agentId' = $2::text)
                   OR EXISTS (SELECT 1 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(tm.definition -> 'workflow' -> 'nodes') = 'array' THEN tm.definition -> 'workflow' -> 'nodes' ELSE '[]'::jsonb END) n WHERE n ->> 'agentId' = $2::text))
            UNION
            SELECT 'worker profile ' || p.name
            FROM worker_profiles p
            WHERE p.org_id = $1 AND $2 = ANY(p.agent_ids)
        ) used
        ORDER BY label
        LIMIT $3
        "#;
    Ok(sqlx::query_scalar(query)
        .bind(org_id)
        .bind(agent_id)
        .bind(IN_USE_LIMIT)
        .fetch_all(&mut **tx)
        .await?)
}

/// What the `ON DELETE CASCADE` keys and the output trigger remove with the agent.
async fn agent_cascade(
    tx: &mut Transaction<'_, Postgres>,
    org_id: Uuid,
    agent_id: Uuid,
) -> StoreResult<Vec<String>> {
    let (executions, triggers, policies, memories, runtimes): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM executions WHERE org_id = $1 AND agent_id = $2),
                (SELECT count(*) FROM triggers WHERE org_id = $1 AND agent_id = $2),
                (SELECT count(*) FROM tool_policies WHERE org_id = $1 AND agent_id = $2),
                (SELECT count(*) FROM agent_memory WHERE agent_id = $2),
                (SELECT count(*) FROM agent_runtimes WHERE agent_id = $2)
            "#,
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_one(&mut **tx)
        .await?;
    let mut removes = Vec::new();
    for (count, singular, plural) in [
        (executions, "execution", "executions"),
        (triggers, "trigger", "triggers"),
        (policies, "tool policy", "tool policies"),
        (memories, "memory entry", "memory entries"),
        (runtimes, "runtime", "runtimes"),
    ] {
        if count > 0 {
            removes.push(format!(
                "{count} {}",
                if count == 1 { singular } else { plural }
            ));
        }
    }
    // Same rule as `orbit_remove_agent_from_outputs` (migration 0062) and the legacy
    // `outputs.agent_id` cascade.
    let outputs: Vec<(String, bool)> = sqlx::query_as(
        r#"
        SELECT name,
               agent_id = $2
               OR (team_id IS NULL AND cardinality(array_remove(agent_ids, $2)) = 0)
        FROM outputs
        WHERE org_id = $1 AND (agent_id = $2 OR $2 = ANY(agent_ids))
        ORDER BY name
        "#,
    )
    .bind(org_id)
    .bind(agent_id)
    .fetch_all(&mut **tx)
    .await?;
    for (name, deleted) in outputs {
        removes.push(if deleted {
            format!("output {name} (deleted)")
        } else {
            format!("output {name} (no longer sent for this agent)")
        });
    }
    Ok(removes)
}

impl Store {
    /// What a permanent delete of the agent would be refused for, and what it would remove.
    pub async fn agent_delete_impact(
        &self,
        org_id: Uuid,
        agent_id: Uuid,
    ) -> StoreResult<AgentDeleteImpact> {
        let mut tx = self.begin().await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM agents WHERE org_id = $1 AND id = $2)",
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(StoreError::NotFound("agent".into()));
        }
        let mut used_by = agent_used_by(&mut tx, org_id, agent_id).await?;
        let team_runs: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT run_id) FROM team_steps WHERE org_id = $1 AND agent_id = $2",
        )
        .bind(org_id)
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await?;
        if team_runs > 0 {
            used_by.push(format!(
                "{team_runs} agent team {}",
                if team_runs == 1 { "run" } else { "runs" }
            ));
        }
        let removes = agent_cascade(&mut tx, org_id, agent_id).await?;
        tx.rollback().await?;
        Ok(AgentDeleteImpact { used_by, removes })
    }

    /// The cascade of a permanent agent delete, read inside the deleting transaction so the
    /// audit entry lists exactly what went.
    pub(crate) async fn agent_cascade_in(
        tx: &mut Transaction<'_, Postgres>,
        org_id: Uuid,
        agent_id: Uuid,
    ) -> StoreResult<Vec<String>> {
        agent_cascade(tx, org_id, agent_id).await
    }
}
