use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical event subjects. Keep these stable: they are part of the deployment contract.
pub mod subjects {
    pub const EXECUTION_REQUESTED: &str = "orbit.execution.requested";
    pub const EXECUTION_DISPATCHED: &str = "orbit.execution.dispatched";
    pub const EXECUTION_UPDATED: &str = "orbit.execution.updated";
    pub const EXECUTION_EVENT: &str = "orbit.execution.event";
    pub const EXECUTION_CANCEL: &str = "orbit.execution.cancel";
    pub const TRIGGER_FIRED: &str = "orbit.trigger.fired";
    pub const CONNECTOR_EVENT: &str = "orbit.connector.event";
    pub const AGENT_EVENT: &str = "orbit.agent.event";
    pub const WORKER_HEARTBEAT: &str = "orbit.worker.heartbeat";
}

/// Audit actions a worker may write through `POST /api/v1/worker/audit`. The API
/// rejects any other name, so a compromised worker cannot write entries that look like
/// user or platform actions. Keep this in sync with the tool broker's `record_audit`
/// calls in `services/worker/src/broker.rs`.
pub const WORKER_AUDIT_ACTIONS: &[&str] = &[
    "tool.call.executed",
    "tool.call.failed",
    "tool.call.approval_requested",
    "tool.call.approved",
    "tool.call.denied",
];

/// Transport-agnostic event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbitEvent {
    pub id: Uuid,
    pub subject: String,
    pub source: String,
    pub time: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_parent: Option<String>,
}

impl OrbitEvent {
    pub fn new(
        subject: impl Into<String>,
        source: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            subject: subject.into(),
            source: source.into(),
            time: Utc::now(),
            org_id: None,
            idempotency_key: None,
            payload,
            trace_parent: None,
        }
    }

    pub fn with_org(mut self, org_id: Uuid) -> Self {
        self.org_id = Some(org_id);
        self
    }

    pub fn with_idempotency(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Subject filter for bus subscriptions. A trailing `*` matches one token, a trailing
/// `>` matches the remainder (NATS semantics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectFilter {
    pattern: String,
}

impl SubjectFilter {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    pub fn all() -> Self {
        Self::new("orbit.>")
    }

    pub fn execution_requested() -> Self {
        Self::new(subjects::EXECUTION_REQUESTED)
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    pub fn matches(&self, subject: &str) -> bool {
        match_tokens(
            &self.pattern.split('.').collect::<Vec<_>>(),
            &subject.split('.').collect::<Vec<_>>(),
        )
    }
}

fn match_tokens(pattern: &[&str], subject: &[&str]) -> bool {
    if pattern.is_empty() {
        return subject.is_empty();
    }
    match pattern[0] {
        ">" => true,
        "*" => !subject.is_empty() && match_tokens(&pattern[1..], &subject[1..]),
        token => {
            !subject.is_empty() && token == subject[0] && match_tokens(&pattern[1..], &subject[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_match_nats_semantics() {
        let filter = SubjectFilter::all();
        assert!(filter.matches("orbit.execution.requested"));
        assert!(!filter.matches("other.execution"));
        let one = SubjectFilter::new("orbit.execution.*");
        assert!(one.matches("orbit.execution.requested"));
        assert!(!one.matches("orbit.execution.event.deep"));
    }

    #[test]
    fn event_roundtrip() {
        let event = OrbitEvent::new(
            subjects::EXECUTION_REQUESTED,
            "orbit-api",
            serde_json::json!({ "executionId": "x" }),
        );
        let bytes = event.encode().unwrap();
        assert_eq!(OrbitEvent::decode(&bytes).unwrap(), event);
    }
}
