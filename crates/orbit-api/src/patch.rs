//! Shared PATCH semantics: a PATCH body is always partial (#443).
//!
//! Handlers build the JSON representation of the stored resource, overlay the fields the
//! caller sent, then deserialize the result into their normal request DTO. Fields left out
//! keep their stored value; an explicit `null` clears a nullable field. Full replacement
//! is a separate operation (PUT), not PATCH.

use serde_json::Value;

use orbit_domain::ValidationIssue;

use crate::error::ApiError;

/// Overlay `patch` onto `stored`. Unknown keys are rejected so a misspelled field cannot
/// silently become a no-op. `extra_keys` are write-only request fields (credentials) that
/// are never present in a stored representation.
pub fn merge_patch(stored: Value, patch: Value, extra_keys: &[&str]) -> Result<Value, ApiError> {
    let Value::Object(patch) = patch else {
        return Err(ApiError::validation(vec![ValidationIssue::new(
            "body",
            "the request body must be a JSON object",
        )]));
    };
    let Value::Object(mut merged) = stored else {
        return Err(ApiError::internal(
            "stored resource has no object representation",
        ));
    };
    for (key, value) in patch {
        if !merged.contains_key(&key) && !extra_keys.contains(&key.as_str()) {
            return Err(ApiError::validation(vec![ValidationIssue::new(
                &key,
                "unknown field",
            )]));
        }
        merged.insert(key, value);
    }
    Ok(Value::Object(merged))
}

/// Deserialize a merged patch into a request DTO, with a readable field-level error.
pub fn parse_patch<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|error| {
        ApiError::validation(vec![ValidationIssue::new("body", error.to_string())])
    })
}

#[cfg(test)]
mod tests {
    use super::{merge_patch, parse_patch};
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct Request {
        name: String,
        enabled: bool,
    }

    #[test]
    fn one_field_is_enough() {
        let stored = json!({ "name": "old", "enabled": true });
        let merged = merge_patch(stored, json!({ "name": "new" }), &[]).unwrap();
        let request: Request = parse_patch(merged).unwrap();
        assert_eq!(
            request,
            Request {
                name: "new".into(),
                enabled: true
            }
        );
    }

    #[test]
    fn unknown_fields_and_non_objects_are_rejected() {
        let stored = json!({ "name": "old", "enabled": true });
        assert!(merge_patch(stored.clone(), json!({ "typo": true }), &[]).is_err());
        assert!(merge_patch(stored, json!([]), &[]).is_err());
    }

    #[test]
    fn write_only_fields_can_be_supplied_in_a_patch() {
        let stored = json!({ "name": "old" });
        let merged = merge_patch(
            stored,
            json!({ "webhookUrl": "https://example.test" }),
            &["webhookUrl"],
        )
        .unwrap();
        assert_eq!(merged["webhookUrl"], "https://example.test");
    }
}
