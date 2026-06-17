//! Team RBAC slice 2 (STORY-647): finer-grained op gating, protected
//! specs/tags, and a strict mode — all built on slice-1's `effective_role`
//! (`team::resolve_effective_role` / `team::effective_role_for_user`).
//!
//! **GUARDRAIL, NOT SECURITY.** As with slice 1, the store is a shared git
//! branch — anyone with push access to `aida-store` can edit any YAML directly,
//! so none of this is an access-control boundary. It stops *accidents* (an
//! implementer kicking off a drain, a teammate transitioning a release-tagged
//! spec), encodes team structure, and leaves an audit trail. `--force` always
//! bypasses (the bypass is itself the audit signal). The caveat is surfaced in
//! `aida team set-role` --help, the config policy registry, and the docs.
//!
//! ## What slice 2 adds over slice 1
//!
//! - A small **config-driven permission map**: each [`GatedOp`] resolves to a
//!   minimum role (defaults below), tunable under `[team.permissions]` in
//!   `.aida/config.toml`.
//! - **Protected specs/tags**: `[team] protected_tags = [...]`; editing or
//!   transitioning a spec carrying ANY protected tag requires the configured
//!   role (advisor by default).
//! - A **`[team] strict`** mode: when true, a NON-rostered user gets
//!   least-privilege (default-deny) for gated ops instead of the permissive
//!   env/default fallback, and refusals are NOT bypassable by setting/unsetting
//!   `AIDA_SESSION_ROLE` (the roster is authoritative). `--force` still works.
//!
//! When `strict = false` and no `[team]` config is present, behavior is exactly
//! slice 1 (backward-compatible).
//!
//! The resolution core ([`permits`]) is pure over (op, effective_role, config)
//! so it is directly unit-testable without touching the env or filesystem.
//! trace:STORY-647 | ai:claude

use std::path::Path;

use crate::team::{self, RoleSource};

/// A team-gated operation. Each maps to a minimum role via the permission map.
/// trace:STORY-647 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatedOp {
    /// Status transition into the approved+ pipeline (slice 1's gate).
    StatusTransition,
    /// Editing or transitioning a spec that carries a protected tag.
    ProtectedSpec,
    /// `aida db merge-gate` — assigning agreed short IDs.
    MergeGate,
    /// `aida queue integrate` — merging ready PRs into the default branch.
    Integrate,
    /// Starting an autonomous drain (`aida burndown run` /
    /// `aida queue work --auto-complete`).
    DrainStart,
}

impl GatedOp {
    /// The `[team.permissions]` config key for this op (where the minimum role
    /// is tuned). `None` for ops whose minimum role is configured elsewhere
    /// (protected-spec gating reads `[team] protected_role`, not the map).
    fn config_key(self) -> Option<&'static str> {
        match self {
            GatedOp::StatusTransition => Some("status_transition"),
            GatedOp::MergeGate => Some("merge_gate"),
            GatedOp::Integrate => Some("integrate"),
            GatedOp::DrainStart => Some("drain_start"),
            // Protected-spec gating reads the dedicated `protected_role` knob.
            GatedOp::ProtectedSpec => None,
        }
    }

    /// The built-in default minimum role for this op when unspecified in config.
    /// All current gated ops default to `advisor` (the slice-1 + slice-2 policy).
    fn default_min_role(self) -> &'static str {
        "advisor"
    }
}

/// The resolved `[team]` policy: the permission map (op -> min role), the set of
/// protected tags + their required role, and strict mode. Built from
/// `.aida/config.toml`; an absent file / section yields the all-default policy
/// (which reproduces slice-1 behavior). trace:STORY-647 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct TeamPermissions {
    /// Per-op minimum-role overrides from `[team.permissions]`. A missing entry
    /// falls back to [`GatedOp::default_min_role`].
    perms: std::collections::BTreeMap<String, String>,
    /// Tags that mark a spec "protected" (`[team] protected_tags`). Lowercased
    /// for case-insensitive matching. Empty = no protected specs.
    protected_tags: Vec<String>,
    /// The role required to edit/transition a protected spec
    /// (`[team] protected_role`). Defaults to `advisor`.
    protected_role: Option<String>,
    /// `[team] strict` — when true, non-rostered users get least-privilege and
    /// refusals are roster-authoritative (env can't bypass). Default false.
    pub(crate) strict: bool,
}

impl TeamPermissions {
    /// Load the `[team]` policy from a parsed `.aida/config.toml` value. A
    /// missing `[team]` section yields the all-default policy. Pure over the
    /// parsed value so it can be unit-tested without the filesystem.
    /// trace:STORY-647 | ai:claude
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let Some(team) = cfg.and_then(|c| c.get("team")) else {
            return Self::default();
        };
        let mut perms = std::collections::BTreeMap::new();
        if let Some(map) = team.get("permissions").and_then(|v| v.as_table()) {
            for (k, v) in map {
                if let Some(role) = v.as_str() {
                    let role = role.trim();
                    if !role.is_empty() {
                        perms.insert(k.clone(), super::canonical_role_name(role));
                    }
                }
            }
        }
        let protected_tags = team
            .get("protected_tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let protected_role = team
            .get("protected_role")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(super::canonical_role_name);
        let strict = team
            .get("strict")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            perms,
            protected_tags,
            protected_role,
            strict,
        }
    }

    /// Load the `[team]` policy from the project's `.aida/config.toml`.
    /// Best-effort: an unreadable / unparseable config yields the all-default
    /// (non-strict) policy, so a degraded config never blocks. trace:STORY-647
    pub(crate) fn load(project_root: &Path) -> Self {
        Self::from_config(super::read_project_config_value(project_root).as_ref())
    }

    /// The minimum role required for `op` — the configured override, else the
    /// built-in default. For [`GatedOp::ProtectedSpec`] this is `protected_role`
    /// (default `advisor`).
    pub(crate) fn min_role(&self, op: GatedOp) -> String {
        if op == GatedOp::ProtectedSpec {
            return self
                .protected_role
                .clone()
                .unwrap_or_else(|| "advisor".to_string());
        }
        op.config_key()
            .and_then(|k| self.perms.get(k).cloned())
            .unwrap_or_else(|| op.default_min_role().to_string())
    }

    /// A human-readable rendering of the protected-tag set for `aida config
    /// show` (`(none)` when empty). trace:STORY-647 | ai:claude
    pub(crate) fn protected_tags_display(&self) -> String {
        if self.protected_tags.is_empty() {
            "(none — no protected specs)".to_string()
        } else {
            self.protected_tags.join(", ")
        }
    }

    /// Whether `tags` mark a spec protected under this policy (case-insensitive,
    /// any-match). Empty `protected_tags` => never protected. trace:STORY-647
    pub(crate) fn spec_is_protected<I, S>(&self, tags: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if self.protected_tags.is_empty() {
            return false;
        }
        tags.into_iter().any(|t| {
            let t = t.as_ref().trim().to_ascii_lowercase();
            self.protected_tags.iter().any(|p| *p == t)
        })
    }
}

/// The gated ops shown in `aida config show`'s `[team]` permission-map rows,
/// each paired with its `[team.permissions]` config key. (Protected-spec gating
/// is rendered separately via `protected_role`.) trace:STORY-647 | ai:claude
pub(crate) const POLICY_DISPLAY_OPS: &[(GatedOp, &str)] = &[
    (GatedOp::StatusTransition, "status_transition"),
    (GatedOp::MergeGate, "merge_gate"),
    (GatedOp::Integrate, "integrate"),
    (GatedOp::DrainStart, "drain_start"),
];

/// Whether `effective_role` satisfies the minimum role for `op` under `config`.
///
/// The policy is a simple two-tier ladder: `advisor` outranks every other role.
/// A gated op whose minimum role is `advisor` (every current default) is
/// permitted only to an effective `advisor`; an op tuned to a non-advisor
/// minimum (e.g. `[team.permissions] integrate = "implementer"`) is permitted
/// to anyone (there is no role below implementer in the guardrail model).
///
/// Pure over its inputs — no env, no filesystem — so the op×role×config matrix
/// is directly unit-testable. trace:STORY-647 | ai:claude
pub(crate) fn permits(op: GatedOp, effective_role: &str, config: &TeamPermissions) -> bool {
    let min = config.min_role(op);
    role_satisfies(effective_role, &min)
}

/// Does `have` satisfy a `need` minimum role? `advisor` is the only privileged
/// tier today, so the rule is: if the op needs `advisor`, the caller must be
/// `advisor`; any non-advisor minimum is satisfied by anyone. trace:STORY-647
fn role_satisfies(have: &str, need: &str) -> bool {
    if need == "advisor" {
        have == "advisor"
    } else {
        true
    }
}

/// The effective role to use for a *gated-op* decision, honoring strict mode.
///
/// - Non-strict (slice-1 behavior): roster role → `AIDA_SESSION_ROLE` → default
///   `implementer` (the permissive fallback). Reproduced via
///   [`team::resolve_effective_role`].
/// - **Strict**: the roster is authoritative. A rostered user gets their roster
///   role (env ignored — refusals can't be unset away). A NON-rostered user
///   gets **least-privilege** (`implementer`) regardless of any
///   `AIDA_SESSION_ROLE` — default-deny for gated ops.
///
/// Returns the role plus its source (so the refusal message can name a durable
/// team role). Pure over its inputs. trace:STORY-647 | ai:claude
pub(crate) fn gated_effective_role(
    roster_role: Option<&str>,
    env_role: Option<&str>,
    strict: bool,
) -> (String, RoleSource) {
    if !strict {
        return team::resolve_effective_role(roster_role, env_role);
    }
    match roster_role.map(str::trim).filter(|s| !s.is_empty()) {
        // Rostered: durable role wins, env can't override it (either up or down).
        Some(r) => (super::canonical_role_name(r), RoleSource::Roster),
        // Non-rostered in strict mode: least-privilege, ignore env entirely.
        None => ("implementer".to_string(), RoleSource::Default),
    }
}

/// Resolve the gated-op effective role for the current user against the store,
/// honoring `config.strict`. Best-effort: an unreachable / unreadable store
/// yields an empty roster → in strict mode a non-rostered user (everyone, since
/// the roster couldn't be read) lands least-privilege, EXCEPT we treat an
/// unreachable store as a degraded environment and fall back to non-strict
/// slice-1 behavior so a transient store outage never hard-blocks (point 4:
/// store/config unreachable → never block). trace:STORY-647 | ai:claude
pub(crate) fn gated_effective_role_for_user(
    store_root: Option<&Path>,
    user_id: &str,
    config: &TeamPermissions,
) -> (String, RoleSource) {
    let env_role = std::env::var("AIDA_SESSION_ROLE").ok();
    match store_root {
        Some(root) if root.join("objects").is_dir() => {
            let roster = team::TeamRoster::load(root);
            let roster_role = roster.role_for(user_id).map(str::to_string);
            gated_effective_role(roster_role.as_deref(), env_role.as_deref(), config.strict)
        }
        // Store unreachable → degraded; fall back to non-strict slice-1
        // resolution rather than default-deny everyone. trace:STORY-647
        _ => team::resolve_effective_role(None, env_role.as_deref()),
    }
}

/// A clear, role-naming refusal message for a denied gated op. Names the op, the
/// caller's effective role (and that it's the durable *team* role when sourced
/// from the roster), the required role, and the `--force` audited escape hatch.
/// Reaffirms the guardrail-not-security framing implicitly by offering `--force`.
/// trace:STORY-647 | ai:claude
pub(crate) fn refusal_message(
    op: GatedOp,
    have_role: &str,
    source: RoleSource,
    config: &TeamPermissions,
) -> String {
    let need = config.min_role(op);
    let op_label = match op {
        GatedOp::StatusTransition => "promoting a spec into the approved pipeline",
        GatedOp::ProtectedSpec => "editing or transitioning a protected spec",
        GatedOp::MergeGate => "running the merge gate",
        GatedOp::Integrate => "integrating ready PRs",
        GatedOp::DrainStart => "starting an autonomous drain",
    };
    let role_clause = if source == RoleSource::Roster {
        format!("your team role is `{have_role}`")
    } else {
        format!("your role is `{have_role}`")
    };
    format!(
        "{op_label} needs the `{need}` role — {role_clause}. Ask an advisor, fix it with \
         `aida team set-role`, or re-run with `--force` (a guardrail, not security — the \
         bypass is recorded in history)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_from(toml_str: &str) -> TeamPermissions {
        let v: toml::Value = toml::from_str(toml_str).unwrap();
        TeamPermissions::from_config(Some(&v))
    }

    #[test]
    fn defaults_gate_every_op_on_advisor() {
        let cfg = TeamPermissions::default();
        for op in [
            GatedOp::StatusTransition,
            GatedOp::MergeGate,
            GatedOp::Integrate,
            GatedOp::DrainStart,
            GatedOp::ProtectedSpec,
        ] {
            assert!(permits(op, "advisor", &cfg), "advisor should pass {op:?}");
            assert!(
                !permits(op, "implementer", &cfg),
                "implementer should fail {op:?}"
            );
            assert!(
                !permits(op, "reviewer", &cfg),
                "non-advisor should fail {op:?}"
            );
        }
    }

    #[test]
    fn permission_map_override_loosens_an_op() {
        // Tune integrate down to implementer; the rest stay advisor-only.
        let cfg = cfg_from("[team.permissions]\nintegrate = \"implementer\"\n");
        assert!(permits(GatedOp::Integrate, "implementer", &cfg));
        assert!(permits(GatedOp::Integrate, "advisor", &cfg));
        // Untouched ops still require advisor.
        assert!(!permits(GatedOp::DrainStart, "implementer", &cfg));
        assert!(!permits(GatedOp::MergeGate, "implementer", &cfg));
    }

    #[test]
    fn permission_map_dialog_alias_canonicalizes() {
        // A config that names the deprecated `dialog` token resolves to advisor,
        // so an advisor passes and an implementer is refused.
        let cfg = cfg_from("[team.permissions]\nintegrate = \"dialog\"\n");
        assert_eq!(cfg.min_role(GatedOp::Integrate), "advisor");
        assert!(permits(GatedOp::Integrate, "advisor", &cfg));
        assert!(!permits(GatedOp::Integrate, "implementer", &cfg));
    }

    #[test]
    fn protected_tag_detection_is_case_insensitive_any_match() {
        let cfg = cfg_from("[team]\nprotected_tags = [\"protected\", \"Release\"]\n");
        assert!(cfg.spec_is_protected(["protected"]));
        assert!(cfg.spec_is_protected(["PROTECTED"])); // case-insensitive
        assert!(cfg.spec_is_protected(["release"])); // config-side casing folded
        assert!(cfg.spec_is_protected(["foo", "release", "bar"])); // any-match
        assert!(!cfg.spec_is_protected(["foo", "bar"]));
        assert!(!cfg.spec_is_protected(Vec::<String>::new()));
    }

    #[test]
    fn no_protected_tags_means_never_protected() {
        let cfg = TeamPermissions::default();
        assert!(!cfg.spec_is_protected(["protected", "release"]));
    }

    #[test]
    fn protected_role_defaults_to_advisor_and_is_tunable() {
        let dflt = cfg_from("[team]\nprotected_tags = [\"protected\"]\n");
        assert_eq!(dflt.min_role(GatedOp::ProtectedSpec), "advisor");
        assert!(permits(GatedOp::ProtectedSpec, "advisor", &dflt));
        assert!(!permits(GatedOp::ProtectedSpec, "implementer", &dflt));

        let tuned = cfg_from(
            "[team]\nprotected_tags = [\"protected\"]\nprotected_role = \"implementer\"\n",
        );
        assert_eq!(tuned.min_role(GatedOp::ProtectedSpec), "implementer");
        assert!(permits(GatedOp::ProtectedSpec, "implementer", &tuned));
    }

    #[test]
    fn strict_flag_parses() {
        assert!(!TeamPermissions::default().strict);
        assert!(cfg_from("[team]\nstrict = true\n").strict);
        assert!(!cfg_from("[team]\nstrict = false\n").strict);
        // Section present but no strict key => default false.
        assert!(!cfg_from("[team]\nprotected_tags = [\"x\"]\n").strict);
    }

    #[test]
    fn non_strict_falls_through_to_env_like_slice_1() {
        // Non-rostered user with an advisor env => advisor (permissive fallback).
        let (role, src) = gated_effective_role(None, Some("advisor"), false);
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Env);
        // Nothing set => implementer default.
        let (role, src) = gated_effective_role(None, None, false);
        assert_eq!(role, "implementer");
        assert_eq!(src, RoleSource::Default);
    }

    #[test]
    fn strict_non_rostered_is_least_privilege_and_env_immune() {
        // Strict + non-rostered + advisor env => STILL implementer (env can't
        // grant authority; default-deny). This is the core strict-mode property.
        let (role, src) = gated_effective_role(None, Some("advisor"), true);
        assert_eq!(role, "implementer");
        assert_eq!(src, RoleSource::Default);
        let cfg = cfg_from("[team]\nstrict = true\n");
        assert!(
            !permits(GatedOp::Integrate, &role, &cfg),
            "non-rostered strict user must be refused a gated op"
        );
    }

    #[test]
    fn strict_rostered_advisor_is_authoritative_and_env_immune() {
        // Strict + rostered advisor + implementer env => advisor (roster wins;
        // env can't demote either).
        let (role, src) = gated_effective_role(Some("advisor"), Some("implementer"), true);
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Roster);
        let cfg = cfg_from("[team]\nstrict = true\n");
        assert!(permits(GatedOp::Integrate, &role, &cfg));
    }

    #[test]
    fn strict_rostered_implementer_cannot_env_escalate() {
        // Strict + rostered implementer + advisor env => implementer (roster is
        // authoritative; you can't `AIDA_SESSION_ROLE=advisor` your way past it).
        let (role, src) = gated_effective_role(Some("implementer"), Some("advisor"), true);
        assert_eq!(role, "implementer");
        assert_eq!(src, RoleSource::Roster);
        let cfg = cfg_from("[team]\nstrict = true\n");
        assert!(!permits(GatedOp::Integrate, &role, &cfg));
    }

    #[test]
    fn refusal_message_names_op_role_and_force() {
        let cfg = TeamPermissions::default();
        let msg = refusal_message(GatedOp::Integrate, "implementer", RoleSource::Roster, &cfg);
        assert!(msg.contains("integrating ready PRs"));
        assert!(msg.contains("`advisor`"));
        assert!(msg.contains("team role is `implementer`"));
        assert!(msg.contains("--force"));
    }
}
