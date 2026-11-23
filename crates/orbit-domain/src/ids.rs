use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type OrganizationId = Uuid;
pub type UserId = Uuid;
pub type AgentId = Uuid;
pub type AgentVersionId = Uuid;
pub type AgentInstanceId = Uuid;
pub type HarnessId = Uuid;
pub type ProviderId = Uuid;
pub type ConnectionId = Uuid;
pub type ConnectorId = Uuid;
pub type TriggerId = Uuid;
pub type ExecutionId = Uuid;
pub type ExecutionEventId = i64;
pub type ApprovalId = Uuid;

/// Generate a sortable, time-ordered identifier (UUIDv7).
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// Slug rules shared by agents, providers and connections.
pub fn is_valid_slug(value: &str) -> bool {
    if value.is_empty() || value.len() > 63 {
        return false;
    }
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for c in value.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    let limited = if trimmed.len() > 48 {
        trimmed[..48].trim_matches('-').to_string()
    } else {
        trimmed
    };
    if limited.is_empty() {
        "agent".to_string()
    } else {
        limited
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OrganizationMode {
    #[default]
    Personal,
    Enterprise,
}

impl OrganizationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Enterprise => "enterprise",
        }
    }
}

impl std::str::FromStr for OrganizationMode {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "personal" => Ok(Self::Personal),
            "enterprise" => Ok(Self::Enterprise),
            other => Err(format!("unknown organization mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Admin,
    Operator,
    #[default]
    Viewer,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Operator => "operator",
            Self::Viewer => "viewer",
        }
    }
}

impl std::str::FromStr for UserRole {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "operator" => Ok(Self::Operator),
            "viewer" => Ok(Self::Viewer),
            other => Err(format!("unknown role: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Social Monitor!"), "social-monitor");
        assert_eq!(slugify("  "), "agent");
        assert!(is_valid_slug(&slugify("GitLab Agent v2")));
    }

    #[test]
    fn ids_are_time_ordered() {
        let a = new_id();
        let b = new_id();
        assert!(a < b, "uuidv7 should be sortable");
    }
}
