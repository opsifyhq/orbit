//! Guards the OpenAPI document against drifting from the routed API.
//!
//! Axum does not expose its route table through a public API, so this test pins the
//! operations and schemas that have historically drifted instead of claiming a full
//! router/spec diff. `ORBIT_OPENAPI_DUMP=<path>` writes the pretty JSON document so the
//! client can be regenerated without starting the API or a database.

use std::path::Path;

use orbit_api::openapi::ApiDoc;
use serde_json::{json, Value};
use utoipa::OpenApi;

fn spec() -> Value {
    serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI document must serialize")
}

fn path<'a>(doc: &'a Value, path_name: &str) -> &'a Value {
    doc["paths"]
        .get(path_name)
        .unwrap_or_else(|| panic!("OpenAPI document is missing path `{path_name}`"))
}

fn assert_method(doc: &Value, path_name: &str, method: &str) {
    assert!(
        path(doc, path_name).get(method).is_some(),
        "OpenAPI document is missing {method} {path_name}"
    );
}

fn assert_schema(doc: &Value, name: &str) {
    assert!(
        doc["components"]["schemas"].get(name).is_some(),
        "OpenAPI document is missing schema `{name}`"
    );
}

#[test]
fn openapi_documents_routed_operations_and_schemas() {
    let doc = spec();

    // Bulk sharing is routed but was historically absent from the spec.
    assert_method(&doc, "/api/v1/sharing/{resource}/bulk", "patch");
    assert_schema(&doc, "BulkShareRequest");
    assert_schema(&doc, "BulkShareResponse");
    assert_schema(&doc, "BulkShareFailure");

    // The docs route is an axum catch-all; the spec must use the same syntax so nested
    // slugs such as `architecture/overview` stay representable.
    assert_method(&doc, "/api/v1/docs/{*path}", "get");
    assert!(doc["paths"].get("/api/v1/docs/{slug}").is_none());
    assert!(doc["paths"].get("/api/v1/docs/assets/{path}").is_none());

    // Public browser OIDC helpers are used by the web app and must be in the contract.
    assert_method(&doc, "/api/v1/auth/oidc/config", "get");
    assert_method(&doc, "/api/v1/auth/oidc/token", "post");
    assert_schema(&doc, "OidcWebConfig");
    assert_schema(&doc, "OidcTokenRequest");
    assert_schema(&doc, "OidcTokenResponse");

    // Worker tokens are issued/revoked outside the profile lifecycle.
    assert_method(&doc, "/api/v1/worker-tokens", "get");
    assert_method(&doc, "/api/v1/worker-tokens", "post");
    assert_method(&doc, "/api/v1/worker-tokens/{token_id}", "delete");
    assert_method(&doc, "/api/v1/worker-profiles/{profile_id}/token", "post");
    for name in [
        "CreateWorkerTokenRequest",
        "IssuedWorkerToken",
        "WorkerTokenView",
    ] {
        assert_schema(&doc, name);
    }

    let kinds = doc["components"]["schemas"]["ProviderKind"]["enum"]
        .as_array()
        .expect("ProviderKind must be an enum schema");
    assert!(kinds.iter().any(|kind| kind == "openai_compatible"));
    assert!(!kinds.iter().any(|kind| kind == "open_ai_compatible"));

    if let Some(destination) = std::env::var_os("ORBIT_OPENAPI_DUMP") {
        let destination = Path::new(&destination);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).expect("cannot create OpenAPI dump directory");
        }
        let json =
            serde_json::to_string_pretty(&doc).expect("OpenAPI document must be pretty JSON");
        std::fs::write(destination, json).expect("cannot write OpenAPI dump");
    }
}

#[test]
fn every_operation_declares_the_shared_error_response() {
    let doc = spec();
    let reference = json!({ "$ref": "#/components/responses/ErrorResponse" });
    assert_eq!(
        doc["components"]["responses"]["ErrorResponse"]["content"]["application/json"]["schema"],
        json!({ "$ref": "#/components/schemas/ErrorEnvelope" })
    );

    let mut operations = 0;
    for (path_name, item) in doc["paths"].as_object().expect("paths").iter() {
        for (method, operation) in item.as_object().expect("path item").iter() {
            if ![
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ]
            .contains(&method.as_str())
            {
                continue;
            }
            operations += 1;
            let responses = operation["responses"]
                .as_object()
                .unwrap_or_else(|| panic!("{method} {path_name} has no responses object"));
            for status in ["400", "401", "403", "404", "409", "422", "429"] {
                assert_eq!(
                    responses.get(status),
                    Some(&reference),
                    "{method} {path_name} must reference ErrorResponse for {status}"
                );
            }
        }
    }
    assert!(operations > 100, "expected the full router in the document");
}

#[test]
fn document_matches_the_observed_wire_shapes() {
    let doc = spec();

    // The output test endpoint returns no content, starting an existing runtime returns the
    // old runtime with 200, and /import accepts a raw body.
    assert!(
        doc["paths"]["/api/v1/outputs/{output_id}/test"]["post"]["responses"]["204"].is_object()
    );
    assert!(
        doc["paths"]["/api/v1/agents/{agent_id}/runtime"]["post"]["responses"]["200"].is_object()
    );
    assert!(doc["paths"]["/api/v1/import"]["post"]["requestBody"].is_object());

    let execute = &doc["paths"]["/api/v1/agents/{agent_id}/executions"]["post"];
    assert!(execute["parameters"]
        .as_array()
        .expect("execution parameters")
        .iter()
        .any(|parameter| parameter["name"] == "Idempotency-Key" && parameter["in"] == "header"));

    let activated_by = &doc["components"]["schemas"]["TeamStep"]["properties"]["activatedBy"];
    assert_eq!(
        activated_by["type"], "array",
        "activatedBy is a list on the wire"
    );
    assert_eq!(
        activated_by["items"],
        json!({ "$ref": "#/components/schemas/TeamStepActivation" })
    );

    let binding = &doc["components"]["schemas"]["ConnectorBinding"]["properties"];
    assert!(
        binding.get("connectionId").is_some(),
        "wire name is connectionId"
    );
    assert!(
        binding.get("connection_id").is_none(),
        "snake_case is ignored"
    );
}

#[test]
fn team_run_and_connector_binding_wire_responses_deserialize() {
    // Shapes copied from real API responses: a team run detail with a workflow step and an
    // agent definition with a connector reference.
    let timestamp = "2026-11-26T10:00:00Z";
    let response = json!({
        "run": {
            "id": "018f6c9e-6c0a-7000-8000-000000000001",
            "orgId": "018f6c9e-6c0a-7000-8000-000000000002",
            "teamId": "018f6c9e-6c0a-7000-8000-000000000003",
            "status": "running",
            "input": { "task": "fix the failing test" },
            "output": null,
            "error": null,
            "requestedBy": "018f6c9e-6c0a-7000-8000-000000000004",
            "createdAt": timestamp,
            "updatedAt": timestamp,
            "finishedAt": null,
            "archivedAt": null,
            "advanceClaimedAt": null
        },
        "steps": [{
            "id": "018f6c9e-6c0a-7000-8000-000000000005",
            "runId": "018f6c9e-6c0a-7000-8000-000000000001",
            "orgId": "018f6c9e-6c0a-7000-8000-000000000002",
            "stepIndex": 1,
            "name": "fix",
            "agentId": "018f6c9e-6c0a-7000-8000-000000000006",
            "agentVersionId": "018f6c9e-6c0a-7000-8000-000000000007",
            "skills": [],
            "status": "queued",
            "executionId": null,
            "input": { "task": "fix the failing test" },
            "output": null,
            "error": null,
            "createdAt": timestamp,
            "updatedAt": timestamp,
            "startedAt": null,
            "finishedAt": null,
            "nodeId": "fix",
            "attempt": 1,
            "outcome": null,
            "activatedBy": [{ "edge": "l1", "step": 0 }]
        }]
    });
    let run: orbit_store::TeamRun =
        serde_json::from_value(response["run"].clone()).expect("team run follows the schema");
    assert_eq!(run.status, "running");
    let steps: Vec<orbit_store::TeamStep> =
        serde_json::from_value(response["steps"].clone()).expect("team step follows the schema");
    assert_eq!(steps[0].activated_by, json!([{ "edge": "l1", "step": 0 }]));

    let connection_id = "018f6c9e-6c0a-7000-8000-000000000008";
    let binding: orbit_domain::ConnectorBinding = serde_json::from_value(json!({
        "connector": "github",
        "connectionId": connection_id,
        "capabilities": ["issues.list"]
    }))
    .expect("connector binding follows the schema");
    assert_eq!(binding.connector, "github");
    assert_eq!(
        binding.connection_id,
        Some(uuid::Uuid::parse_str(connection_id).unwrap())
    );
}
