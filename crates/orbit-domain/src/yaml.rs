//! Bounded YAML parsing for user-supplied documents.
//!
//! `serde_yaml`/`unsafe-libyaml` are unmaintained; the workspace uses the maintained
//! `serde_norway` fork under the same dependency name. This module also rejects anchors
//! and aliases before parsing, so a small document cannot expand into a large value.

use serde::de::DeserializeOwned;

/// Larger imports are rejected before the parser sees them.
pub const MAX_YAML_BYTES: usize = 1024 * 1024;

/// Parse a YAML document with the import limits. Errors are safe to return to a user: they
/// do not include the document body.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, String> {
    validate(input)?;
    serde_yaml::from_str(input).map_err(|error| format!("invalid YAML: {error}"))
}

/// Parse a YAML value with the same limits.
pub fn value_from_str(input: &str) -> Result<serde_yaml::Value, String> {
    from_str(input)
}

/// Validate the size and reject anchors/aliases.
pub fn validate(input: &str) -> Result<(), String> {
    if input.len() > MAX_YAML_BYTES {
        return Err(format!(
            "YAML input is {} bytes; the limit is {} bytes",
            input.len(),
            MAX_YAML_BYTES
        ));
    }
    if has_anchor_or_alias(input) {
        return Err("YAML anchors and aliases are not allowed in imports".into());
    }
    Ok(())
}

fn has_anchor_or_alias(input: &str) -> bool {
    let bytes = input.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if !matches!(byte, b'&' | b'*') {
            continue;
        }
        let next = bytes.get(index + 1).copied();
        if !next.is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, b'_' | b'-')) {
            continue;
        }
        let previous = bytes[..index]
            .iter()
            .rev()
            .find(|byte| !byte.is_ascii_whitespace())
            .copied();
        if previous.is_none() || matches!(previous, Some(b'[' | b'{' | b',' | b':' | b'-' | b'?')) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_large_documents_and_aliases() {
        assert!(validate(&"x".repeat(MAX_YAML_BYTES + 1)).is_err());
        assert!(validate("base: &base\n  a: 1\ncopy: *base\n").is_err());
        assert!(validate("task: report *all* results\n").is_ok());
    }

    #[test]
    fn parses_a_normal_document() {
        let value: serde_yaml::Value = value_from_str("name: orbit\nsteps:\n  - one\n").unwrap();
        assert_eq!(value["name"], "orbit");
    }
}
