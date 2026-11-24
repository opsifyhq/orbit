use serde::{Deserialize, Serialize};

/// Platform-enforced autonomy levels. The model never decides these; ORBIT does.
///
/// - 1 Observe: read-only capabilities; writes and side effects are denied.
/// - 2 Recommend: analyse and propose; writes and side effects are denied (no approval path).
/// - 3 Prepare: writes/side effects require explicit approval.
/// - 4 Autonomous: allowlisted writes/side effects execute without approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AutonomyLevel(u8);

impl AutonomyLevel {
    pub const OBSERVE: Self = Self(1);
    pub const RECOMMEND: Self = Self(2);
    pub const PREPARE: Self = Self(3);
    pub const AUTONOMOUS: Self = Self(4);

    pub fn from_level(level: u8) -> Option<Self> {
        (1..=4).contains(&level).then_some(Self(level))
    }

    pub fn level(&self) -> u8 {
        self.0
    }

    pub fn label(&self) -> &'static str {
        match self.0 {
            1 => "Observe",
            2 => "Recommend",
            3 => "Prepare",
            _ => "Autonomous",
        }
    }

    pub fn description(&self) -> &'static str {
        match self.0 {
            1 => "Read-only access to connected services; writes are denied.",
            2 => "Analyse and propose actions; writes are denied (raise to Prepare for approval-gated writes).",
            3 => "Prepare writes and side effects; a human must approve.",
            _ => "Execute allowlisted actions within platform policies.",
        }
    }

    /// Could this level ever execute the given capability effect without approval?
    pub fn permits_effect_without_approval(&self, effect: CapabilityEffect) -> bool {
        match effect {
            CapabilityEffect::Read => true,
            CapabilityEffect::Write | CapabilityEffect::ExternalSideEffect => self.0 >= 4,
        }
    }
}

impl Default for AutonomyLevel {
    fn default() -> Self {
        Self::RECOMMEND
    }
}

impl std::fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.0, self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEffect {
    Read,
    Write,
    ExternalSideEffect,
}

impl CapabilityEffect {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::ExternalSideEffect => "external_side_effect",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "external_side_effect" => Some(Self::ExternalSideEffect),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    RequireApproval,
    Deny,
}

impl PolicyDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::RequireApproval => "require_approval",
            Self::Deny => "deny",
        }
    }
}

/// A capability granted to an agent by its platform configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantedCapability {
    pub connection_id: uuid::Uuid,
    pub connector: String,
    pub capability: String,
    pub effect: CapabilityEffect,
}

/// Evaluate a capability invocation against autonomy, approval mode and allowlist.
///
/// `granted` is derived from the agent definition's connector bindings and the
/// connection's capability catalog. An empty allowlist never grants writes.
///
/// Levels 1 (Observe) and 2 (Recommend) **deny** every write or external side
/// effect outright; there is no approval path, regardless of `approval_mode`.
/// Approval-gated writes start at level 3 (Prepare).
pub fn evaluate_capability(
    level: AutonomyLevel,
    approval_mode: crate::ApprovalMode,
    requested: &str,
    effect: CapabilityEffect,
    granted: &[GrantedCapability],
) -> PolicyDecision {
    let allowlisted = granted.iter().any(|g| g.capability == requested);
    if !allowlisted {
        return PolicyDecision::Deny;
    }
    match effect {
        CapabilityEffect::Read => PolicyDecision::Allow,
        CapabilityEffect::Write | CapabilityEffect::ExternalSideEffect
            if level < AutonomyLevel::PREPARE =>
        {
            PolicyDecision::Deny
        }
        CapabilityEffect::Write | CapabilityEffect::ExternalSideEffect => {
            let requires = match approval_mode {
                crate::ApprovalMode::Always => true,
                crate::ApprovalMode::Writes => {
                    effect == CapabilityEffect::ExternalSideEffect
                        || effect == CapabilityEffect::Write
                }
                crate::ApprovalMode::Auto => !level.permits_effect_without_approval(effect),
            };
            if requires {
                PolicyDecision::RequireApproval
            } else {
                PolicyDecision::Allow
            }
        }
    }
}

/// Execution-input key carrying a server-set approval override (#194).
pub const APPROVAL_OVERRIDE_KEY: &str = "approvalOverride";
/// Legacy numeric override that raised the autonomy level. It is ignored everywhere.
pub const LEGACY_AUTONOMY_OVERRIDE_KEY: &str = "autonomyOverride";
/// The only accepted value of [`APPROVAL_OVERRIDE_KEY`].
pub const APPROVAL_OVERRIDE_AUTO: &str = "auto";

/// Apply an execution's approval override to an agent's autonomy spec.
///
/// The override can only switch the approval mode to `Auto`, and only for agents whose own
/// level is already Prepare (3) or higher. It never raises the level, so Observe and
/// Recommend agents keep denying writes. Returns `true` when the override was applied.
pub fn apply_approval_override(spec: &mut crate::AutonomySpec, requested: Option<&str>) -> bool {
    if requested != Some(APPROVAL_OVERRIDE_AUTO) || spec.level < AutonomyLevel::PREPARE.level() {
        return false;
    }
    spec.approval_mode = crate::ApprovalMode::Auto;
    true
}

/// Remove approval/autonomy overrides (including the legacy numeric one) from an execution input.
pub fn strip_approval_overrides(input: &mut serde_json::Value) {
    if let Some(object) = input.as_object_mut() {
        object.remove(APPROVAL_OVERRIDE_KEY);
        object.remove(LEGACY_AUTONOMY_OVERRIDE_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApprovalMode;

    fn grant(capability: &str, effect: CapabilityEffect) -> GrantedCapability {
        GrantedCapability {
            connection_id: uuid::Uuid::now_v7(),
            connector: "gitlab".into(),
            capability: capability.into(),
            effect,
        }
    }

    #[test]
    fn observe_is_read_only() {
        let granted = vec![
            grant("issues.list", CapabilityEffect::Read),
            grant("issues.create", CapabilityEffect::Write),
        ];
        let level = AutonomyLevel::OBSERVE;
        assert_eq!(
            evaluate_capability(
                level,
                ApprovalMode::Auto,
                "issues.list",
                CapabilityEffect::Read,
                &granted
            ),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate_capability(
                level,
                ApprovalMode::Auto,
                "issues.create",
                CapabilityEffect::Write,
                &granted
            ),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn observe_and_recommend_deny_writes_in_every_approval_mode() {
        let granted = vec![
            grant("issues.create", CapabilityEffect::Write),
            grant("messages.send", CapabilityEffect::ExternalSideEffect),
            grant("issues.list", CapabilityEffect::Read),
        ];
        for level in [AutonomyLevel::OBSERVE, AutonomyLevel::RECOMMEND] {
            for mode in [
                ApprovalMode::Auto,
                ApprovalMode::Writes,
                ApprovalMode::Always,
            ] {
                for (cap, effect) in [
                    ("issues.create", CapabilityEffect::Write),
                    ("messages.send", CapabilityEffect::ExternalSideEffect),
                ] {
                    assert_eq!(
                        evaluate_capability(level, mode, cap, effect, &granted),
                        PolicyDecision::Deny,
                        "level {level} mode {mode:?} cap {cap}"
                    );
                }
                assert_eq!(
                    evaluate_capability(
                        level,
                        mode,
                        "issues.list",
                        CapabilityEffect::Read,
                        &granted
                    ),
                    PolicyDecision::Allow
                );
            }
        }
    }

    #[test]
    fn recommend_write_is_denied() {
        let granted = vec![grant("issues.create", CapabilityEffect::Write)];
        assert_eq!(
            evaluate_capability(
                AutonomyLevel::RECOMMEND,
                ApprovalMode::Auto,
                "issues.create",
                CapabilityEffect::Write,
                &granted
            ),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn prepare_write_requires_approval() {
        let granted = vec![grant("issues.create", CapabilityEffect::Write)];
        for mode in [
            ApprovalMode::Auto,
            ApprovalMode::Writes,
            ApprovalMode::Always,
        ] {
            assert_eq!(
                evaluate_capability(
                    AutonomyLevel::PREPARE,
                    mode,
                    "issues.create",
                    CapabilityEffect::Write,
                    &granted
                ),
                PolicyDecision::RequireApproval
            );
        }
    }

    #[test]
    fn unlisted_capabilities_are_denied() {
        let granted = vec![grant("issues.list", CapabilityEffect::Read)];
        assert_eq!(
            evaluate_capability(
                AutonomyLevel::AUTONOMOUS,
                ApprovalMode::Auto,
                "issues.delete",
                CapabilityEffect::Write,
                &granted
            ),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn autonomous_writes_execute() {
        let granted = vec![grant("messages.send", CapabilityEffect::ExternalSideEffect)];
        assert_eq!(
            evaluate_capability(
                AutonomyLevel::AUTONOMOUS,
                ApprovalMode::Auto,
                "messages.send",
                CapabilityEffect::ExternalSideEffect,
                &granted
            ),
            PolicyDecision::Allow
        );
        // ... unless the operator forces approvals.
        assert_eq!(
            evaluate_capability(
                AutonomyLevel::AUTONOMOUS,
                ApprovalMode::Always,
                "messages.send",
                CapabilityEffect::ExternalSideEffect,
                &granted
            ),
            PolicyDecision::RequireApproval
        );
    }

    #[test]
    fn levels_roundtrip() {
        for level in 1..=4 {
            assert_eq!(AutonomyLevel::from_level(level).unwrap().level(), level);
        }
        assert!(AutonomyLevel::from_level(0).is_none());
        assert!(AutonomyLevel::from_level(5).is_none());
    }

    fn spec_at(level: u8, mode: ApprovalMode) -> crate::AutonomySpec {
        crate::AutonomySpec {
            level,
            approval_mode: mode,
            max_executions_per_hour: None,
        }
    }

    #[test]
    fn approval_override_never_raises_level_and_needs_l3() {
        for level in [1u8, 2] {
            let mut spec = spec_at(level, ApprovalMode::Always);
            assert!(!apply_approval_override(&mut spec, Some("auto")));
            assert_eq!(spec, spec_at(level, ApprovalMode::Always));
            // Still denies writes.
            let granted = vec![grant("issues.create", CapabilityEffect::Write)];
            assert_eq!(
                evaluate_capability(
                    AutonomyLevel::from_level(spec.level).unwrap(),
                    spec.approval_mode,
                    "issues.create",
                    CapabilityEffect::Write,
                    &granted
                ),
                PolicyDecision::Deny
            );
        }
        let mut l3 = spec_at(3, ApprovalMode::Always);
        assert!(apply_approval_override(&mut l3, Some("auto")));
        assert_eq!(l3, spec_at(3, ApprovalMode::Auto));
        let mut l4 = spec_at(4, ApprovalMode::Always);
        assert!(apply_approval_override(&mut l4, Some("auto")));
        assert_eq!(l4, spec_at(4, ApprovalMode::Auto));
    }

    #[test]
    fn unknown_override_values_are_ignored() {
        for value in [None, Some("4"), Some("AUTO"), Some("")] {
            let mut spec = spec_at(4, ApprovalMode::Always);
            assert!(!apply_approval_override(&mut spec, value));
            assert_eq!(spec, spec_at(4, ApprovalMode::Always));
        }
    }

    #[test]
    fn strip_removes_both_override_keys() {
        let mut input = serde_json::json!({
            "task": "x", "approvalOverride": "auto", "autonomyOverride": 4
        });
        strip_approval_overrides(&mut input);
        assert_eq!(input, serde_json::json!({ "task": "x" }));
    }

    #[test]
    fn fixed_team_autonomy_replaces_the_agent_level_and_approval() {
        let mut spec = spec_at(2, ApprovalMode::Always);
        let autonomy = TeamAutonomy {
            mode: TeamAutonomyMode::Fixed,
            level: Some(4),
            approval_mode: Some(ApprovalMode::Auto),
            elevations: Vec::new(),
        };
        assert!(apply_team_autonomy(&mut spec, Some(&autonomy)));
        assert_eq!(spec, spec_at(4, ApprovalMode::Auto));
    }

    #[test]
    fn follow_primary_or_an_invalid_level_does_not_change_autonomy() {
        let mut spec = spec_at(2, ApprovalMode::Always);
        let follow = TeamAutonomy {
            mode: TeamAutonomyMode::FollowPrimary,
            level: Some(4),
            approval_mode: Some(ApprovalMode::Auto),
            elevations: Vec::new(),
        };
        assert!(!apply_team_autonomy(&mut spec, Some(&follow)));
        let invalid = TeamAutonomy {
            mode: TeamAutonomyMode::Fixed,
            level: Some(9),
            approval_mode: Some(ApprovalMode::Auto),
            elevations: Vec::new(),
        };
        assert!(!apply_team_autonomy(&mut spec, Some(&invalid)));
        assert_eq!(spec, spec_at(2, ApprovalMode::Always));
    }
}

/// How an Agent Team chooses the autonomy level for its executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TeamAutonomyMode {
    /// Each step uses its own agent version's autonomy; native subagents inherit the
    /// primary step's execution level.
    #[default]
    FollowPrimary,
    /// Every step and native subagent uses the explicit team level. Raising it above a
    /// step agent's own level is authorised when the team is saved.
    Fixed,
}

/// An explicit team-wide autonomy policy. `follow_primary` is the backward-compatible
/// default; `fixed` is snapshotted into a run so later agent/team edits cannot change an
/// in-flight run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamAutonomy {
    #[serde(default)]
    pub mode: TeamAutonomyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<crate::ApprovalMode>,
    /// Server-recorded elevations (#385): for each step agent the team runs less strictly
    /// than its own policy, the agent policy that was in force when someone with manage
    /// access to that agent authorised it. Client-supplied values are discarded on save.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elevations: Vec<TeamAutonomyElevation>,
}

/// One authorised elevation of a fixed team policy over a step agent's own policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamAutonomyElevation {
    pub agent_id: uuid::Uuid,
    /// The agent's own policy when the elevation was authorised.
    pub baseline: AutonomyPolicy,
}

/// The part of an autonomy spec that decides what may run without a human: the level and
/// the approval mode. Ordered by strictness through [`AutonomyPolicy::is_at_least_as_strict_as`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutonomyPolicy {
    pub level: u8,
    #[serde(default)]
    pub approval_mode: crate::ApprovalMode,
}

const ALL_EFFECTS: [CapabilityEffect; 3] = [
    CapabilityEffect::Read,
    CapabilityEffect::Write,
    CapabilityEffect::ExternalSideEffect,
];

fn decision_rank(decision: PolicyDecision) -> u8 {
    match decision {
        PolicyDecision::Allow => 0,
        PolicyDecision::RequireApproval => 1,
        PolicyDecision::Deny => 2,
    }
}

fn approval_mode_rank(mode: crate::ApprovalMode) -> u8 {
    match mode {
        crate::ApprovalMode::Auto => 0,
        crate::ApprovalMode::Writes => 1,
        crate::ApprovalMode::Always => 2,
    }
}

impl AutonomyPolicy {
    pub fn new(level: u8, approval_mode: crate::ApprovalMode) -> Self {
        Self {
            level,
            approval_mode,
        }
    }

    pub fn of(spec: &crate::AutonomySpec) -> Self {
        Self::new(spec.level, spec.approval_mode)
    }

    /// An out-of-range level is treated as Observe, the strictest level.
    fn autonomy_level(&self) -> AutonomyLevel {
        AutonomyLevel::from_level(self.level).unwrap_or(AutonomyLevel::OBSERVE)
    }

    /// What the platform decides for an allowlisted capability with this effect. Uses
    /// [`evaluate_capability`] itself, so the order below can never drift from enforcement.
    pub fn decision(&self, effect: CapabilityEffect) -> PolicyDecision {
        let grant = GrantedCapability {
            connection_id: uuid::Uuid::nil(),
            connector: String::new(),
            capability: "probe".into(),
            effect,
        };
        evaluate_capability(
            self.autonomy_level(),
            self.approval_mode,
            "probe",
            effect,
            std::slice::from_ref(&grant),
        )
    }

    /// `true` when this policy never lets an effect through with less human involvement
    /// than `other` (allow < approval < deny for every effect) and does not run at a higher
    /// level, since the level also decides which native tools a harness offers.
    pub fn is_at_least_as_strict_as(&self, other: &Self) -> bool {
        self.autonomy_level() <= other.autonomy_level()
            && ALL_EFFECTS.iter().all(|effect| {
                decision_rank(self.decision(*effect)) >= decision_rank(other.decision(*effect))
            })
    }

    /// The least strict policy that is at least as strict as both.
    pub fn stricter_of(&self, other: &Self) -> Self {
        let level = self.autonomy_level().min(other.autonomy_level()).level();
        let approval_mode =
            if approval_mode_rank(self.approval_mode) >= approval_mode_rank(other.approval_mode) {
                self.approval_mode
            } else {
                other.approval_mode
            };
        Self::new(level, approval_mode)
    }
}

impl TeamAutonomy {
    /// The policy a fixed team runs `agent` with: the team level, and the team approval
    /// mode or, when the team sets none, the agent's own (see [`apply_team_autonomy`]).
    /// `None` for follow-primary or an invalid level.
    pub fn policy_for(&self, agent: AutonomyPolicy) -> Option<AutonomyPolicy> {
        if self.mode != TeamAutonomyMode::Fixed {
            return None;
        }
        let level = self.level.and_then(AutonomyLevel::from_level)?;
        Some(AutonomyPolicy::new(
            level.level(),
            self.approval_mode.unwrap_or(agent.approval_mode),
        ))
    }

    /// Is running `agent` under this team policy an elevation (less strict than the
    /// agent's own policy, by level or approval mode)?
    pub fn elevates(&self, agent: AutonomyPolicy) -> bool {
        self.policy_for(agent)
            .is_some_and(|team| !team.is_at_least_as_strict_as(&agent))
    }

    /// The team policy without the server-recorded elevations, as sent to a worker.
    pub fn snapshot(&self) -> Self {
        Self {
            elevations: Vec::new(),
            ..self.clone()
        }
    }
}

/// The effective fixed team policy for one step agent at run time (#385).
#[derive(Debug, Clone, PartialEq)]
pub enum TeamAutonomyResolution {
    /// Not a fixed policy (or an invalid level): the agent's own policy applies.
    NotFixed,
    /// The team policy applies as saved: it is not an elevation over the agent's current
    /// policy, or an authorised elevation still holds.
    Applies(TeamAutonomy),
    /// The agent's policy is now stricter than when an elevation was authorised (or no
    /// elevation was ever authorised for it): the step runs with the stricter of the two.
    Clamped {
        autonomy: TeamAutonomy,
        requested: AutonomyPolicy,
        agent: AutonomyPolicy,
    },
}

/// Resolve a fixed team policy against a step agent's **current** policy. A team never
/// runs an agent less strictly than its current policy unless an elevation recorded at
/// save time still holds: the agent has not become stricter than the recorded baseline.
pub fn resolve_team_autonomy(
    autonomy: &TeamAutonomy,
    agent_id: uuid::Uuid,
    agent: AutonomyPolicy,
) -> TeamAutonomyResolution {
    let Some(requested) = autonomy.policy_for(agent) else {
        return TeamAutonomyResolution::NotFixed;
    };
    if requested.is_at_least_as_strict_as(&agent) {
        return TeamAutonomyResolution::Applies(autonomy.snapshot());
    }
    let authorised = autonomy
        .elevations
        .iter()
        .find(|elevation| elevation.agent_id == agent_id)
        .is_some_and(|elevation| elevation.baseline.is_at_least_as_strict_as(&agent));
    if authorised {
        return TeamAutonomyResolution::Applies(autonomy.snapshot());
    }
    let clamped = requested.stricter_of(&agent);
    TeamAutonomyResolution::Clamped {
        autonomy: TeamAutonomy {
            mode: TeamAutonomyMode::Fixed,
            level: Some(clamped.level),
            approval_mode: Some(clamped.approval_mode),
            elevations: Vec::new(),
        },
        requested,
        agent,
    }
}

/// Execution-input key carrying the server-created team autonomy snapshot.
pub const TEAM_AUTONOMY_KEY: &str = "teamAutonomy";

/// Apply a fixed team autonomy policy to an execution's agent spec. Returns `true` when
/// the spec changed. A missing/invalid level never changes anything.
pub fn apply_team_autonomy(
    spec: &mut crate::AutonomySpec,
    autonomy: Option<&TeamAutonomy>,
) -> bool {
    let Some(autonomy) = autonomy else {
        return false;
    };
    if autonomy.mode != TeamAutonomyMode::Fixed {
        return false;
    }
    let Some(level) = autonomy.level.and_then(AutonomyLevel::from_level) else {
        return false;
    };
    spec.level = level.level();
    if let Some(approval_mode) = autonomy.approval_mode {
        spec.approval_mode = approval_mode;
    }
    true
}

#[cfg(test)]
mod team_strictness_tests {
    use super::*;
    use crate::ApprovalMode;

    const MODES: [ApprovalMode; 3] = [
        ApprovalMode::Auto,
        ApprovalMode::Writes,
        ApprovalMode::Always,
    ];

    fn all() -> Vec<AutonomyPolicy> {
        (1..=4)
            .flat_map(|level| {
                MODES
                    .iter()
                    .map(move |mode| AutonomyPolicy::new(level, *mode))
            })
            .collect()
    }

    fn fixed(level: u8, mode: Option<ApprovalMode>) -> TeamAutonomy {
        TeamAutonomy {
            mode: TeamAutonomyMode::Fixed,
            level: Some(level),
            approval_mode: mode,
            elevations: Vec::new(),
        }
    }

    #[test]
    fn l4_auto_is_less_strict_than_l4_always_and_writes() {
        let auto = AutonomyPolicy::new(4, ApprovalMode::Auto);
        for mode in [ApprovalMode::Always, ApprovalMode::Writes] {
            let strict = AutonomyPolicy::new(4, mode);
            assert!(strict.is_at_least_as_strict_as(&auto));
            assert!(!auto.is_at_least_as_strict_as(&strict));
        }
    }

    #[test]
    fn approval_mode_does_not_matter_below_level_four() {
        for level in 1..=3 {
            for a in MODES {
                for b in MODES {
                    assert!(AutonomyPolicy::new(level, a)
                        .is_at_least_as_strict_as(&AutonomyPolicy::new(level, b)));
                }
            }
        }
    }

    #[test]
    fn a_higher_level_is_never_at_least_as_strict() {
        for low in all() {
            for high in all() {
                if high.level > low.level {
                    assert!(!high.is_at_least_as_strict_as(&low), "{high:?} vs {low:?}");
                }
            }
        }
    }

    #[test]
    fn strictness_is_reflexive_and_transitive() {
        let policies = all();
        for a in &policies {
            assert!(a.is_at_least_as_strict_as(a));
            for b in &policies {
                for c in &policies {
                    if a.is_at_least_as_strict_as(b) && b.is_at_least_as_strict_as(c) {
                        assert!(a.is_at_least_as_strict_as(c));
                    }
                }
            }
        }
    }

    #[test]
    fn stricter_of_is_at_least_as_strict_as_both() {
        for a in all() {
            for b in all() {
                let joined = a.stricter_of(&b);
                assert!(
                    joined.is_at_least_as_strict_as(&a),
                    "{a:?} {b:?} -> {joined:?}"
                );
                assert!(
                    joined.is_at_least_as_strict_as(&b),
                    "{a:?} {b:?} -> {joined:?}"
                );
            }
        }
    }

    #[test]
    fn decisions_follow_evaluate_capability() {
        assert_eq!(
            AutonomyPolicy::new(4, ApprovalMode::Auto).decision(CapabilityEffect::Write),
            PolicyDecision::Allow
        );
        assert_eq!(
            AutonomyPolicy::new(4, ApprovalMode::Always).decision(CapabilityEffect::Write),
            PolicyDecision::RequireApproval
        );
        assert_eq!(
            AutonomyPolicy::new(2, ApprovalMode::Auto).decision(CapabilityEffect::Write),
            PolicyDecision::Deny
        );
        assert_eq!(
            AutonomyPolicy::new(1, ApprovalMode::Always).decision(CapabilityEffect::Read),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn team_elevation_covers_approval_mode() {
        let agent = AutonomyPolicy::new(4, ApprovalMode::Always);
        assert!(fixed(4, Some(ApprovalMode::Auto)).elevates(agent));
        assert!(!fixed(4, Some(ApprovalMode::Always)).elevates(agent));
        // No team approval mode keeps the agent's own.
        assert!(!fixed(4, None).elevates(agent));
        assert!(fixed(4, None).elevates(AutonomyPolicy::new(3, ApprovalMode::Always)));
        assert!(!fixed(2, Some(ApprovalMode::Auto)).elevates(agent));
    }

    #[test]
    fn unauthorised_elevation_is_clamped_to_the_agent() {
        let agent_id = uuid::Uuid::now_v7();
        let agent = AutonomyPolicy::new(4, ApprovalMode::Always);
        match resolve_team_autonomy(&fixed(4, Some(ApprovalMode::Auto)), agent_id, agent) {
            TeamAutonomyResolution::Clamped { autonomy, .. } => {
                assert_eq!(autonomy.level, Some(4));
                assert_eq!(autonomy.approval_mode, Some(ApprovalMode::Always));
            }
            other => panic!("expected a clamp, got {other:?}"),
        }
    }

    #[test]
    fn authorised_elevation_holds_until_the_agent_becomes_stricter() {
        let agent_id = uuid::Uuid::now_v7();
        let mut team = fixed(4, Some(ApprovalMode::Auto));
        team.elevations.push(TeamAutonomyElevation {
            agent_id,
            baseline: AutonomyPolicy::new(3, ApprovalMode::Auto),
        });
        // Same as the baseline, or looser: the elevation holds.
        for agent in [
            AutonomyPolicy::new(3, ApprovalMode::Auto),
            AutonomyPolicy::new(3, ApprovalMode::Always),
        ] {
            assert_eq!(
                resolve_team_autonomy(&team, agent_id, agent),
                TeamAutonomyResolution::Applies(team.snapshot())
            );
        }
        // Lowered after the team was saved: the step runs at the agent's level.
        match resolve_team_autonomy(&team, agent_id, AutonomyPolicy::new(1, ApprovalMode::Auto)) {
            TeamAutonomyResolution::Clamped { autonomy, .. } => {
                assert_eq!(autonomy.level, Some(1))
            }
            other => panic!("expected a clamp, got {other:?}"),
        }
        // An elevation recorded for another agent does not count.
        assert!(matches!(
            resolve_team_autonomy(
                &team,
                uuid::Uuid::now_v7(),
                AutonomyPolicy::new(3, ApprovalMode::Auto)
            ),
            TeamAutonomyResolution::Clamped { .. }
        ));
    }

    #[test]
    fn a_non_elevating_policy_applies_and_snapshots_drop_elevations() {
        let agent_id = uuid::Uuid::now_v7();
        let mut team = fixed(2, Some(ApprovalMode::Always));
        team.elevations.push(TeamAutonomyElevation {
            agent_id,
            baseline: AutonomyPolicy::new(4, ApprovalMode::Auto),
        });
        match resolve_team_autonomy(&team, agent_id, AutonomyPolicy::new(4, ApprovalMode::Auto)) {
            TeamAutonomyResolution::Applies(snapshot) => {
                assert!(snapshot.elevations.is_empty());
                assert_eq!(snapshot.level, Some(2));
            }
            other => panic!("expected the team policy, got {other:?}"),
        }
        let follow = TeamAutonomy {
            mode: TeamAutonomyMode::FollowPrimary,
            ..fixed(4, None)
        };
        assert_eq!(
            resolve_team_autonomy(
                &follow,
                agent_id,
                AutonomyPolicy::new(1, ApprovalMode::Auto)
            ),
            TeamAutonomyResolution::NotFixed
        );
    }
}
