use orbit_domain::UserRole;
use serde::{Deserialize, Serialize};

/// Coarse-grained platform permissions checked in handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    AgentsRead,
    AgentsWrite,
    ExecutionsRead,
    ExecutionsCreate,
    ExecutionsCancel,
    ApprovalsDecide,
    HarnessesRead,
    ProvidersRead,
    ProvidersWrite,
    ConnectionsRead,
    ConnectionsWrite,
    TriggersWrite,
    SettingsRead,
    SettingsWrite,
    MembersRead,
    AuditRead,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AgentsRead => "agents:read",
            Self::AgentsWrite => "agents:write",
            Self::ExecutionsRead => "executions:read",
            Self::ExecutionsCreate => "executions:create",
            Self::ExecutionsCancel => "executions:cancel",
            Self::ApprovalsDecide => "approvals:decide",
            Self::HarnessesRead => "harnesses:read",
            Self::ProvidersRead => "providers:read",
            Self::ProvidersWrite => "providers:write",
            Self::ConnectionsRead => "connections:read",
            Self::ConnectionsWrite => "connections:write",
            Self::TriggersWrite => "triggers:write",
            Self::SettingsRead => "settings:read",
            Self::SettingsWrite => "settings:write",
            Self::MembersRead => "members:read",
            Self::AuditRead => "audit:read",
        }
    }

    pub fn allows(role: UserRole, permission: Permission) -> bool {
        use Permission::*;
        match role {
            UserRole::Viewer => matches!(
                permission,
                AgentsRead
                    | ExecutionsRead
                    | HarnessesRead
                    | ProvidersRead
                    | ConnectionsRead
                    | SettingsRead
                    | MembersRead
            ),
            UserRole::Operator => matches!(
                permission,
                AgentsRead
                    | AgentsWrite
                    | ExecutionsRead
                    | ExecutionsCreate
                    | ExecutionsCancel
                    | HarnessesRead
                    | ProvidersRead
                    | ConnectionsRead
                    | ConnectionsWrite
                    | TriggersWrite
                    | SettingsRead
                    | MembersRead
            ),
            UserRole::Admin => !matches!(permission, SettingsWrite),
            UserRole::Owner => true,
        }
    }

    pub fn all_for(role: UserRole) -> Vec<&'static str> {
        const ALL: [Permission; 16] = [
            Permission::AgentsRead,
            Permission::AgentsWrite,
            Permission::ExecutionsRead,
            Permission::ExecutionsCreate,
            Permission::ExecutionsCancel,
            Permission::ApprovalsDecide,
            Permission::HarnessesRead,
            Permission::ProvidersRead,
            Permission::ProvidersWrite,
            Permission::ConnectionsRead,
            Permission::ConnectionsWrite,
            Permission::TriggersWrite,
            Permission::SettingsRead,
            Permission::SettingsWrite,
            Permission::MembersRead,
            Permission::AuditRead,
        ];
        ALL.into_iter()
            .filter(|p| Permission::allows(role, *p))
            .map(|p| p.as_str())
            .collect()
    }
}

/// Whether a permission set (which may contain the `*` wildcard) grants `scope`.
/// A requested `*` is only granted by a set that itself holds `*`.
pub fn scope_granted(held: &[String], scope: &str) -> bool {
    held.iter().any(|h| h == "*" || h == scope)
}

/// Permissions in `requested` that `held` does not cover (the grant ceiling).
///
/// `held` covers everything when it contains `*` or when it contains every known
/// permission (the Owner's built-in set, which is spelled out rather than `*`). A
/// requested `*` is therefore only covered by one of those two.
pub fn ungranted_permissions(held: &[String], requested: &[String]) -> Vec<String> {
    let all = Permission::all_for(UserRole::Owner);
    let holds_everything =
        held.iter().any(|h| h == "*") || all.iter().all(|p| held.iter().any(|h| h == p));
    if holds_everything {
        return Vec::new();
    }
    requested
        .iter()
        .filter(|scope| scope.as_str() == "*" || !held.iter().any(|h| h == scope.as_str()))
        .cloned()
        .collect()
}

/// Intersect a token's scopes with the owner's current permissions, handling the `*`
/// wildcard on either side. The result never grants more than `held`.
pub fn intersect_scopes(token_scopes: &[String], held: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for scope in token_scopes {
        if scope == "*" {
            // Wildcard token: the owner's current set applies in full.
            for h in held {
                if !out.contains(h) {
                    out.push(h.clone());
                }
            }
        } else if scope_granted(held, scope) && !out.contains(scope) {
            out.push(scope.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewers_cannot_write() {
        assert!(Permission::allows(UserRole::Viewer, Permission::AgentsRead));
        assert!(!Permission::allows(
            UserRole::Viewer,
            Permission::AgentsWrite
        ));
        assert!(Permission::allows(
            UserRole::Operator,
            Permission::ExecutionsCreate
        ));
        assert!(!Permission::allows(
            UserRole::Operator,
            Permission::ProvidersWrite
        ));
        assert!(Permission::allows(
            UserRole::Admin,
            Permission::ProvidersWrite
        ));
        assert!(Permission::allows(
            UserRole::Owner,
            Permission::SettingsWrite
        ));
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn intersection_drops_scopes_the_owner_lost() {
        let held = v(&["agents:read", "settings:read"]);
        let token = v(&["agents:read", "settings:write"]);
        assert_eq!(intersect_scopes(&token, &held), v(&["agents:read"]));
    }

    #[test]
    fn intersection_handles_wildcards_both_ways() {
        let held = v(&["agents:read"]);
        assert_eq!(intersect_scopes(&v(&["*"]), &held), held);
        assert_eq!(
            intersect_scopes(&v(&["agents:read", "audit:read"]), &v(&["*"])),
            v(&["agents:read", "audit:read"])
        );
        assert_eq!(intersect_scopes(&v(&["*"]), &v(&["*"])), v(&["*"]));
        assert!(intersect_scopes(&v(&["audit:read"]), &held).is_empty());
    }

    #[test]
    fn ceiling_compares_real_permission_sets() {
        let held = v(&["settings:write", "agents:read"]);
        assert!(ungranted_permissions(&held, &v(&["agents:read"])).is_empty());
        assert_eq!(
            ungranted_permissions(&held, &v(&["agents:read", "audit:read", "*"])),
            v(&["audit:read", "*"])
        );
        // `*` holders and the Owner's spelled-out full set may grant anything.
        assert!(ungranted_permissions(&v(&["*"]), &v(&["*", "audit:read"])).is_empty());
        let owner: Vec<String> = Permission::all_for(UserRole::Owner)
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(ungranted_permissions(&owner, &v(&["*"])).is_empty());
        // Admin lacks part of the Owner set, so it cannot grant `*`.
        let admin: Vec<String> = Permission::all_for(UserRole::Admin)
            .into_iter()
            .map(str::to_string)
            .collect();
        if admin.len() < owner.len() {
            assert_eq!(ungranted_permissions(&admin, &v(&["*"])), v(&["*"]));
        }
    }

    #[test]
    fn wildcard_scope_requires_wildcard_holder() {
        assert!(!scope_granted(&v(&["agents:read"]), "*"));
        assert!(scope_granted(&v(&["*"]), "*"));
        assert!(scope_granted(&v(&["agents:read"]), "agents:read"));
    }
}
