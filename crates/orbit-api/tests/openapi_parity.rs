//! Guards the OpenAPI document against drifting from the routed API.
//!
//! Axum does not expose its route table through a public API, so this test pins the
//! operations and schemas that have historically drifted instead of claiming a full
//! router/spec diff. `ORBIT_OPENAPI_DUMP=<path>` writes the pretty JSON document so the
//! client can be regenerated without starting the API or a database.

use std::path::Path;

use orbit_api::openapi::ApiDoc;
use serde_json::Value;
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
