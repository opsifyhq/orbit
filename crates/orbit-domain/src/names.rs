//! Display names for stored resources (agents, skills, providers, connections, outputs,
//! teams, tokens, …) share one length limit so the API and the web forms agree (#438).

use crate::ValidationIssue;

/// The longest display name, in characters, that any resource accepts.
pub const MAX_NAME_CHARS: usize = 120;

/// A validation issue at `path` when the trimmed `name` is longer than
/// [`MAX_NAME_CHARS`]. Whether a name is required is left to the caller, whose message
/// names the resource.
pub fn name_length_issue(path: &str, name: &str) -> Option<ValidationIssue> {
    (name.trim().chars().count() > MAX_NAME_CHARS).then(|| {
        ValidationIssue::new(
            path,
            format!("name must be at most {MAX_NAME_CHARS} characters"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_limited_in_characters_after_trimming() {
        assert!(name_length_issue("name", &"a".repeat(MAX_NAME_CHARS)).is_none());
        assert!(
            name_length_issue("name", &format!("  {}  ", "a".repeat(MAX_NAME_CHARS))).is_none()
        );
        // Multi-byte characters count once.
        assert!(name_length_issue("name", &"é".repeat(MAX_NAME_CHARS)).is_none());
        let issue = name_length_issue("metadata.name", &"a".repeat(MAX_NAME_CHARS + 1)).unwrap();
        assert_eq!(issue.path, "metadata.name");
        assert_eq!(issue.message, "name must be at most 120 characters");
    }
}
