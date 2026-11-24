//! STORY-2884: the project's default for a spec whose `execution_mode` is
//! unset — the `[gate.unset_mode]` section of `.aida/config.toml`.
//!
//! ```toml
//! [gate.unset_mode]
//! default = "supervised"         # the default: an unset mode holds the merge
//! drain_types = ["bug", "task"]  # with default = "drain": the types it covers
//! ```
//!
//! Only `drain` licenses an unattended merge (BUG-727), and an unset mode is
//! supervised. This setting lets a project treat an unset mode as `drain` for
//! the listed spec types. It never overrides a mode a spec (or its carrier)
//! sets, never applies to a sensitive spec (the classes a mandate excludes or
//! treats as always-human), and is withdrawn for a change that touches a
//! protected gate path ([`withdraw_defaults_for_change`]). Every other
//! merge gate still applies.
//!
//! The protected gate paths are read from the same trusted commit as the
//! setting (its `[mandate.human_gates] protected_paths`), plus the operator
//! layer `~/.aida/config.toml`. When either cannot be read the protection
//! cannot be established, and no spec takes the default.
//!
//! The setting relaxes what a merge waits for, so — like `[landing]` — it is
//! read only from the config on the trusted default branch, never from a
//! feature branch's own copy. A missing, unreadable or malformed value keeps
//! the supervised default; it never fails open.
// trace:STORY-2884 | ai:claude
// mandate-gate

use crate::gate::mandate_gates::{GateLayer, PathSource, ProtectedPaths};
use aida_core::{ExecutionMode, Requirement, RequirementType};
use std::path::Path;

/// What an unset `execution_mode` means for the merge gates.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum UnsetDefault {
    /// An unset mode holds the merge for a human (BUG-727). The default.
    #[default]
    Supervised,
    /// An unset mode on a listed, non-sensitive spec type gates as `drain`.
    Drain,
    /// Only approved, small changes outside strict-tier code.
    // trace:STORY-3038 | ai:codex
    ApprovedSmall,
}

impl UnsetDefault {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Drain => "drain",
            Self::ApprovedSmall => "approved-small",
        }
    }
}

/// The resolved `[gate.unset_mode]` policy.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsetModePolicy {
    pub(crate) default: UnsetDefault,
    safe_paths: Vec<glob::Pattern>,
    pub(crate) configured_default: bool,
    /// The spec types the drain default applies to. Empty: none.
    pub(crate) drain_types: Vec<RequirementType>,
    /// Values that were ignored, each with its reason.
    pub(crate) warnings: Vec<String>,
    /// The protected gate paths a defaulted change may not touch, read from
    /// the same trusted config (and the operator layer). `Err`: they could
    /// not be read, so no spec takes the default (fail closed).
    pub(crate) protected: Result<ProtectedPaths, String>,
}

impl Default for UnsetModePolicy {
    fn default() -> Self {
        Self {
            default: UnsetDefault::default(),
            safe_paths: vec![glob::Pattern::new("docs/cli/*.md").unwrap()],
            configured_default: false,
            drain_types: Vec::new(),
            warnings: Vec::new(),
            protected: Ok(ProtectedPaths::floor()),
        }
    }
}

/// A type name with case, spaces, hyphens and underscores removed, so
/// `change-request`, `Change Request` and `changerequest` all match.
fn type_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn type_named(name: &str) -> Option<RequirementType> {
    let key = type_key(name);
    if key.is_empty() {
        return None;
    }
    RequirementType::ALL
        .into_iter()
        .find(|t| type_key(&t.to_string()) == key)
}

/// The lower-case display name `aida show` uses for a type.
fn type_word(t: &RequirementType) -> String {
    format!("{t:?}").to_ascii_lowercase()
}

impl UnsetModePolicy {
    /// Parse `[gate.unset_mode]` from a config body. Never fails: a bad value
    /// keeps the supervised default (or drops the bad list entry) and records
    /// a warning.
    // trace:STORY-2884 | ai:claude
    fn from_config(config: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(section) = config
            .and_then(|c| c.get("gate"))
            .and_then(|g| g.get("unset_mode"))
        else {
            return policy;
        };
        if !section.is_table() {
            policy.warnings.push(
                "`[gate.unset_mode]` must be a table; an unset execution mode stays supervised"
                    .into(),
            );
            return policy;
        }
        let default = match section.get("default") {
            None => UnsetDefault::Supervised,
            Some(v) => match v.as_str().map(str::trim) {
                Some("supervised") => UnsetDefault::Supervised,
                Some("drain") => UnsetDefault::Drain,
                Some("approved-small") => UnsetDefault::ApprovedSmall,
                Some(other) => {
                    policy.warnings.push(format!(
                        "`[gate.unset_mode] default` is set to `{other}`, which is not a known \
                         value (allowed: supervised, drain, approved-small); an unset execution mode stays \
                         supervised"
                    ));
                    return policy;
                }
                None => {
                    policy.warnings.push(
                        "`[gate.unset_mode] default` must be a string; an unset execution mode \
                         stays supervised"
                            .into(),
                    );
                    return policy;
                }
            },
        };
        policy.configured_default = section.get("default").is_some();
        policy.default = default;
        if default == UnsetDefault::ApprovedSmall {
            if let Some(value) = section.get("drain_safe_paths") {
                let patterns = value.as_array().and_then(|items| {
                    items
                        .iter()
                        .map(|item| {
                            let path = item.as_str()?;
                            if path.contains('\0')
                                || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                            {
                                return None;
                            }
                            glob::Pattern::new(path).ok()
                        })
                        .collect::<Option<Vec<_>>>()
                });
                policy.safe_paths = patterns.unwrap_or_else(|| {
                    policy.warnings.push("invalid drain_safe_paths".into());
                    Vec::new()
                });
            }
        }
        if default != UnsetDefault::Drain {
            return policy;
        }
        let mut types = Vec::new();
        match section.get("drain_types") {
            None => {}
            Some(toml::Value::Array(items)) => {
                for item in items {
                    match item.as_str().and_then(type_named) {
                        Some(t) if !types.contains(&t) => types.push(t),
                        Some(_) => {}
                        None => policy.warnings.push(format!(
                            "`[gate.unset_mode] drain_types` entry {item} is not a spec type; \
                             ignored"
                        )),
                    }
                }
            }
            Some(_) => policy.warnings.push(
                "`[gate.unset_mode] drain_types` must be a list of spec types; no type gets \
                 the drain default"
                    .into(),
            ),
        }
        if types.is_empty() {
            policy.warnings.push(
                "`[gate.unset_mode] default = \"drain\"` lists no `drain_types`; an unset \
                 execution mode stays supervised"
                    .into(),
            );
        }
        policy.drain_types = types;
        policy
    }

    /// The policy of a trusted config body (`None`: no config, the default).
    /// With a drain default, the body's own `[mandate.human_gates]` layer
    /// supplies the committed protected paths; a layer that does not parse
    /// leaves the protection unknown.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn from_body(body: Option<&str>) -> Self {
        match body.map(|b| (b, b.parse::<toml::Value>())) {
            Some((text, Ok(value))) => {
                let mut policy = Self::from_config(Some(&value));
                if policy.drains_any() {
                    policy.protected = crate::gate::mandate_gates::parse_layer(
                        text,
                        "the default branch's config",
                    )
                    .map(|layer| {
                        let mut protected = ProtectedPaths::floor();
                        protected.extend(PathSource::Committed, layer.protected_paths);
                        protected
                    })
                    .map_err(|e| format!("{e:#}"));
                }
                policy
            }
            Some((_, Err(e))) => Self {
                warnings: vec![format!(
                    "the default branch's .aida/config.toml does not parse ({e}); an unset \
                     execution mode stays supervised"
                )],
                ..Self::default()
            },
            None => Self::default(),
        }
    }

    /// The policy of the project at `root`, from the default branch's config
    /// (local refs only, never the worktree copy), with the operator layer's
    /// protected paths. The committed protected paths come from that same
    /// commit, so a default branch other than `main` is read consistently.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn load(root: &Path) -> Self {
        let policy =
            Self::from_body(crate::trusted_config::read_trusted_config_toml_local(root).as_deref());
        if !policy.drains_any() {
            return policy;
        }
        policy.with_operator_layer(crate::gate::mandate_gates::operator_layer())
    }

    /// Add the operator layer's protected paths; a layer that cannot be read
    /// leaves the protection unknown (fail closed).
    // trace:STORY-2884 | ai:claude
    pub(crate) fn with_operator_layer(mut self, layer: anyhow::Result<GateLayer>) -> Self {
        self.protected = match (self.protected, layer) {
            (Ok(mut protected), Ok(layer)) => {
                protected.extend(PathSource::Operator, layer.protected_paths);
                Ok(protected)
            }
            (Err(e), _) => Err(e),
            (Ok(_), Err(e)) => Err(format!("{e:#}")),
        };
        self
    }

    /// Whether the project sets the drain default at all (any listed type).
    pub(crate) fn drains_any(&self) -> bool {
        self.default == UnsetDefault::ApprovedSmall
            || (self.default == UnsetDefault::Drain && !self.drain_types.is_empty())
    }

    /// The effective gating mode of `req`, given `gating` — the mode the
    /// merge gates already read for it (its own, else its carrier's,
    /// BUG-2413). A set mode is returned unchanged; an unset one gets the
    /// project default only for a listed, non-sensitive type.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn resolve(
        &self,
        gating: Option<ExecutionMode>,
        req: &Requirement,
    ) -> EffectiveMode {
        if gating.is_some() {
            return EffectiveMode::from(gating);
        }
        if !self.drains_any() {
            return EffectiveMode::from(None);
        }
        let word = type_word(&req.req_type);
        // trace:STORY-3038 | ai:codex
        let small_policy = self.default == UnsetDefault::ApprovedSmall;
        let withheld = if small_policy
            && (!approved_for_publication(req)
                || !req
                    .tags
                    .iter()
                    .any(|t| t.trim().eq_ignore_ascii_case("small"))
                || req.tags.iter().any(|tag| {
                    matches!(
                        tag.trim().to_ascii_lowercase().as_str(),
                        "strict" | "strict-tier" | "tier:strict"
                    )
                })) {
            Err("the project default requires an approved small standard-tier spec".into())
        } else if !small_policy && !self.drain_types.contains(&req.req_type) {
            Err(format!(
                "the project's drain default does not cover type {word}"
            ))
        } else if let Some(why) = sensitive_class(req) {
            Err(format!("not defaulted, {why}"))
        } else {
            self.protected.as_ref().map_err(|e| {
                format!("not defaulted, the protected gate paths could not be read ({e})")
            })
        };
        match withheld {
            Err(why) => EffectiveMode {
                mode: None,
                source: ModeSource::Unset {
                    withheld: Some(why),
                },
            },
            Ok(protected) => EffectiveMode {
                mode: Some(ExecutionMode::Drain),
                source: ModeSource::ProjectDefault {
                    req_type: word,
                    safe_paths: small_policy.then(|| self.safe_paths.clone()),
                    protected: protected.clone(),
                },
            },
        }
    }
}

/// STORY-2884: the effective-mode line `aida show` prints for a spec with no
/// mode of its own, when the project sets a drain default; `None` otherwise
/// (a set mode prints as before, and without the setting nothing changes).
/// The mode is resolved as the merge gate resolves it: a spec superseded
/// into a carrier gates on the carrier's mode, which the label names.
/// `store` (the requirements, `None` when unreadable) is read only for a
/// superseded spec.
// trace:STORY-2884 | ai:claude
pub(crate) fn show_label(
    root: &Path,
    req: &Requirement,
    store: impl FnOnce() -> Option<Vec<Requirement>>,
) -> Option<String> {
    if req.execution_mode.is_some() {
        return None;
    }
    let policy = UnsetModePolicy::load(root);
    if !policy.drains_any() {
        return None;
    }
    let superseded = req
        .relationships
        .iter()
        .any(|r| r.rel_type == aida_core::RelationshipType::SupersededBy);
    let unset_label = || {
        let mut mode = policy.resolve(None, req);
        if policy.default == UnsetDefault::ApprovedSmall && mode.is_project_default() {
            // Only the spec's own remote work branch supplies approval-time evidence.
            let paths = req.spec_id.as_ref().and_then(|id| {
                if !aida_core::object_store::valid_spec_id_format(id) {
                    return None;
                }
                let base = crate::trusted_config::local_trusted_default_branch_sha(root)?;
                let branch = format!("refs/remotes/origin/{}", id.to_ascii_lowercase());
                let head = crate::trusted_config::rev_parse_commit(root, &branch)?;
                changed_paths(root, &base, &head)
            });
            mode.withdraw_for_change(paths.as_deref());
        }
        mode.label()
    };
    if !superseded {
        return Some(unset_label());
    }
    let Some(requirements) = store() else {
        return Some("unset (supervised: its superseding carrier could not be read)".into());
    };
    match crate::gate::ship::gating_execution_source(&requirements, req) {
        Some((mode, carrier)) => {
            let carrier_id = carrier
                .spec_id
                .clone()
                .unwrap_or_else(|| carrier.id.to_string());
            Some(format!(
                "{} (from its carrier {})",
                policy.resolve(Some(mode), req).label(),
                aida_core::node_alias::display_spec_id(&carrier_id)
            ))
        }
        None => Some(unset_label()),
    }
}

/// Why a spec is sensitive and never takes the project default: the classes
/// a mandate excludes unless minted with `--include-supervised`
/// (keystone, security, authority, containment, ...) or treats as
/// always-human (`operator-only`).
// trace:STORY-2884 | ai:claude
fn sensitive_class(req: &Requirement) -> Option<String> {
    crate::gate::mandate::always_human_class(req)
        .map(|class| format!("{class} spec"))
        .or_else(|| crate::gate::mandate::excluded_class(req))
}

// trace:STORY-3038 | ai:codex
fn approved_for_publication(req: &Requirement) -> bool {
    if req.status != aida_core::RequirementStatus::Done {
        return req.status == aida_core::RequirementStatus::Approved;
    }
    let mut transitions = req.history.iter().rev().flat_map(|entry| {
        entry
            .changes
            .iter()
            .rev()
            .filter(|change| change.field_name == "status")
            .map(move |change| {
                (
                    entry.author.as_str(),
                    change.old_value.as_str(),
                    change.new_value.as_str(),
                )
            })
    });
    match transitions.next() {
        Some((aida_core::conflict::PUBLICATION_DONE_AUTHOR, "Approved", "Done")) => true,
        Some((aida_core::conflict::PUBLICATION_DONE_AUTHOR, "InProgress", "Done")) => matches!(
            transitions.next(),
            Some((
                aida_core::conflict::SESSION_START_AUTHOR,
                "Approved",
                "InProgress"
            ))
        ),
        _ => false,
    }
}

// ADR-75/76 surfaces. Entries match a repository path or its descendants.
// trace:STORY-3038.AC1 | ai:codex
const STRICT_PATHS: &[&str] = &[
    ".githooks",
    ".github/workflows",
    "aida-cli-lib/src/admission_owner.rs",
    "aida-cli-lib/src/advisor_code_gate.rs",
    "aida-cli-lib/src/team_cmd.rs",
    "aida-cli-lib/src/approvals_cmd.rs",
    "aida-cli-lib/src/ci_gate.rs",
    "aida-cli-lib/src/claim.rs",
    "aida-cli-lib/src/claude_trust.rs",
    "aida-cli-lib/src/codex_native_sandbox.rs",
    "aida-cli-lib/src/contained_boundary.rs",
    "aida-cli-lib/src/containment",
    "aida-cli-lib/src/deputy_shell",
    "aida-cli-lib/src/dispatch_flow.rs",
    "aida-cli-lib/src/drain_gate.rs",
    "aida-cli-lib/src/gate",
    "aida-cli-lib/src/grant_keyring.rs",
    "aida-cli-lib/src/identity_gate.rs",
    "aida-cli-lib/src/identity_guard.rs",
    "aida-cli-lib/src/landing_policy.rs",
    "aida-cli-lib/src/lane_job.rs",
    "aida-cli-lib/src/launch",
    "aida-cli-lib/src/launch_permission_profile.rs",
    "aida-cli-lib/src/leases.rs",
    "aida-cli-lib/src/locking_gate.rs",
    "aida-cli-lib/src/mandate.rs",
    "aida-cli-lib/src/mandate_drive.rs",
    "aida-cli-lib/src/mandate_gates.rs",
    "aida-cli-lib/src/mandate_history_index.rs",
    "aida-cli-lib/src/mcp_claim.rs",
    "aida-cli-lib/src/merge_hold.rs",
    "aida-cli-lib/src/merge_hold_cmd.rs",
    "aida-cli-lib/src/merge_lock.rs",
    "aida-cli-lib/src/mirror_guard.rs",
    "aida-cli-lib/src/orchestrator_leases.rs",
    "aida-cli-lib/src/pending_approval.rs",
    "aida-cli-lib/src/permissions.rs",
    "aida-cli-lib/src/pr_claim_surface.rs",
    "aida-cli-lib/src/pr_cmd",
    "aida-cli-lib/src/pr_emergency_land.rs",
    "aida-cli-lib/src/pr_head_guard.rs",
    "aida-cli-lib/src/pr_lease_takeover.rs",
    "aida-cli-lib/src/pr_merge_gate.rs",
    "aida-cli-lib/src/pr_service.rs",
    "aida-cli-lib/src/pr_ship.rs",
    "aida-cli-lib/src/pr_ship_batch.rs",
    "aida-cli-lib/src/pr_train.rs",
    "aida-cli-lib/src/pr_train_check.rs",
    "aida-cli-lib/src/pr_train_verified.rs",
    "aida-cli-lib/src/proc_broker",
    "aida-cli-lib/src/publication_completion.rs",
    "aida-cli-lib/src/queue_role_fallback.rs",
    "aida-cli-lib/src/relay_grant.rs",
    "aida-cli-lib/src/review_blocking.rs",
    "aida-cli-lib/src/review_gates.rs",
    "aida-cli-lib/src/review_implementer_gate.rs",
    "aida-cli-lib/src/reviewer_preflight.rs",
    "aida-cli-lib/src/role.rs",
    "aida-cli-lib/src/role_cmd.rs",
    "aida-cli-lib/src/role_file_helpers.rs",
    "aida-cli-lib/src/role_renew_parent.rs",
    "aida-cli-lib/src/runaway_seats.rs",
    "aida-cli-lib/src/sandbox_cmd.rs",
    "aida-cli-lib/src/sandbox_config_lock.rs",
    "aida-cli-lib/src/scoped_lease.rs",
    "aida-cli-lib/src/scratch_reclaim.rs",
    "aida-cli-lib/src/seat_clone.rs",
    "aida-cli-lib/src/seat_occupancy",
    "aida-cli-lib/src/seat_rotation.rs",
    "aida-cli-lib/src/seat_target_overlay.rs",
    "aida-cli-lib/src/seats.rs",
    "aida-cli-lib/src/session/containment.rs",
    "aida-cli-lib/src/session/lease_lock.rs",
    "aida-cli-lib/src/shift.rs",
    "aida-cli-lib/src/ship.rs",
    "aida-cli-lib/src/ship_from_seat.rs",
    "aida-cli-lib/src/store_mutation_entry.rs",
    "aida-cli-lib/src/supervisor.rs",
    "aida-cli-lib/src/triage_lease.rs",
    "aida-cli-lib/src/trusted_config.rs",
    "aida-cli-lib/src/worktree_lease.rs",
    "aida-cli-lib/src/worktree_reclaim.rs",
    "aida-core/src/allocation_authority.rs",
    "aida-core/src/git_ops/remote_credentials.rs",
    "aida-core/src/liveness.rs",
    "aida-core/src/lock.rs",
    "aida-core/src/mailbox_identity.rs",
    "aida-core/src/mailbox_lifecycle",
    "aida-core/src/proc_identity.rs",
    "aida-core/src/seat_clone_detect.rs",
    "aida-core/src/secret_screen.rs",
    "aida-core/src/transport_policy.rs",
    "aida-core/src/vendors",
    "aida-core/src/worktree_pool.rs",
    "aida-core/templates/hooks",
    "aida-mcp/src",
    "aida-server/src/web_auth.rs",
];

/// Where an effective gating mode came from.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModeSource {
    /// The spec's own mode, or its carrier's.
    Spec,
    /// No mode set; the project's `[gate.unset_mode]` drain default applies.
    /// `protected`: the protected gate paths read with the setting, which a
    /// change must not touch for the default to stand.
    ProjectDefault {
        req_type: String,
        safe_paths: Option<Vec<glob::Pattern>>,
        protected: ProtectedPaths,
    },
    /// No mode set and no default applies. `withheld` says why when the
    /// project has a drain default that did not apply.
    Unset { withheld: Option<String> },
}

/// The mode a merge gate decides on, with its provenance.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveMode {
    pub(crate) mode: Option<ExecutionMode>,
    pub(crate) source: ModeSource,
}

impl From<Option<ExecutionMode>> for EffectiveMode {
    /// A mode as set on the spec (unset stays unset, supervised).
    fn from(mode: Option<ExecutionMode>) -> Self {
        let source = match mode {
            Some(_) => ModeSource::Spec,
            None => ModeSource::Unset { withheld: None },
        };
        Self { mode, source }
    }
}

/// Evidence records keep their old shape: the effective mode alone.
impl serde::Serialize for EffectiveMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.mode.serialize(s)
    }
}

impl EffectiveMode {
    /// The label the refusal text and `aida show` print, e.g. `drive`,
    /// `unset (supervised)`, or `drain (project default for unset bug)`.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn label(&self) -> String {
        match (&self.source, self.mode) {
            (ModeSource::ProjectDefault { req_type, .. }, _) => {
                format!("drain (project default for unset {req_type})")
            }
            (
                ModeSource::Unset {
                    withheld: Some(why),
                },
                _,
            ) => {
                format!("unset (supervised: {why})")
            }
            (_, mode) => crate::gate::ship::supervision_mode_label(mode),
        }
    }

    /// True when this mode holds an unattended merge for a human.
    pub(crate) fn requires_supervision(&self) -> bool {
        crate::gate::ship::merge_requires_supervision(self.mode)
    }

    /// True when the drain comes from the project default, not the spec.
    pub(crate) fn is_project_default(&self) -> bool {
        matches!(self.source, ModeSource::ProjectDefault { .. })
    }

    /// Withdraw a project-default drain when `paths` (the change's files,
    /// `None` when unknown) touch a protected gate path read with the setting.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn withdraw_for_change(&mut self, paths: Option<&[String]>) {
        let ModeSource::ProjectDefault {
            protected,
            safe_paths,
            ..
        } = &self.source
        else {
            return;
        };
        let strict_path = if safe_paths.is_some() {
            paths.and_then(|paths| {
                paths.iter().find(|p| {
                    STRICT_PATHS
                        .iter()
                        .any(|strict| Path::new(p).starts_with(strict))
                })
            })
        } else {
            None
        };
        if let Some(path) = strict_path {
            self.withdraw(format!(
                "not defaulted, the change touches strict-tier path {path}"
            ));
        } else if let Some(why) = protected_path_reason(paths, protected) {
            self.withdraw(why);
        } else if let (Some(patterns), Some(paths)) = (safe_paths, paths) {
            let options = glob::MatchOptions {
                require_literal_separator: true,
                ..glob::MatchOptions::new()
            };
            if paths.is_empty() {
                self.withdraw("not defaulted, the change is empty".into());
            } else if let Some(path) = paths
                .iter()
                .find(|p| !patterns.iter().any(|g| g.matches_with(p, options)))
            {
                self.withdraw(format!("not defaulted, {path} is outside drain_safe_paths"));
            }
        }
    }

    /// Withdraw a project-default drain (back to unset, supervised) with the
    /// reason. A mode the spec sets is never touched.
    // trace:STORY-2884 | ai:claude
    pub(crate) fn withdraw(&mut self, why: String) {
        if self.is_project_default() {
            self.mode = None;
            self.source = ModeSource::Unset {
                withheld: Some(why),
            };
        }
    }
}

/// Why a change's paths withdraw the project default: they touch a protected
/// gate path, or they could not be read. `None`: the default stands.
// trace:STORY-2884 | ai:claude
pub(crate) fn protected_path_reason(
    paths: Option<&[String]>,
    protected: &ProtectedPaths,
) -> Option<String> {
    match paths {
        None => Some("not defaulted, the change's files could not be read".into()),
        Some(paths) => crate::gate::mandate_gates::touches_gates(paths, protected)
            .map(|hit| format!("not defaulted, the change touches {}", hit.describe())),
    }
}

/// Withdraw every project-default drain in `modes` when the change touches a
/// protected gate path (as read with the setting) or its files cannot be
/// read. `paths` (the change's files; `None` when unknown) is called only
/// when some mode is a project default, so a project without the setting
/// pays no git read.
// trace:STORY-2884 | ai:claude
pub(crate) fn withdraw_defaults_for_change<'a>(
    modes: impl IntoIterator<Item = &'a mut EffectiveMode>,
    paths: impl FnOnce() -> Option<Vec<String>>,
) {
    let mut defaulted: Vec<&mut EffectiveMode> = modes
        .into_iter()
        .filter(|m| m.is_project_default())
        .collect();
    if defaulted.is_empty() {
        return;
    }
    let paths = paths();
    for mode in &mut defaulted {
        mode.withdraw_for_change(paths.as_deref());
    }
}

/// STORY-2884: the PR heads whose files a project default was checked
/// against. The default is admitted for those heads only, so ship refuses a
/// preflight (and so merge) head that differs: a push between the check and
/// preflight could otherwise add a protected path the default never saw.
// trace:STORY-2884 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct CheckedHeads(std::cell::RefCell<Vec<String>>);

impl CheckedHeads {
    /// Note that a default was checked against `head`.
    pub(crate) fn record(&self, head: &str) {
        let mut heads = self.0.borrow_mut();
        if !heads.iter().any(|h| h == head) {
            heads.push(head.to_string());
        }
    }

    /// Why the merge must stop: a default was checked at a head other than
    /// `preflight` (or `preflight` is unknown). `None` when no default was
    /// checked, or every check saw `preflight`.
    pub(crate) fn moved_refusal(&self, pr: u64, preflight: Option<&str>) -> Option<String> {
        let heads = self.0.borrow();
        if heads.is_empty() {
            return None;
        }
        let pinned = preflight.map(str::trim).filter(|p| !p.is_empty());
        let moved = heads.iter().find(|h| Some(h.as_str()) != pinned)?;
        Some(format!(
            "PR-{pr} head moved after its unset execution mode took the project's drain \
             default: checked at {}, preflight at {}. Re-run ship so the new head's files \
             are checked.",
            short_sha(moved),
            pinned.map(short_sha).unwrap_or("an unknown head")
        ))
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Files `head` changes against its merge base with `base` (a ref such as
/// `origin/main`). `None` when git cannot say — the caller withdraws the
/// project default.
// trace:STORY-2884 | ai:claude
pub(crate) fn changed_paths(root: &Path, base: &str, head: &str) -> Option<Vec<String>> {
    let (base, head) = (base.trim(), head.trim());
    if base.is_empty() || head.is_empty() || base.starts_with('-') || head.starts_with('-') {
        return None;
    }
    let mut cmd = aida_core::git_ops::host_git_command(root);
    cmd.args(["diff", "--name-only", "-z", "--no-renames"])
        .arg(format!("{base}...{head}"))
        .arg("--");
    // A scoped read deadline (`train --check --timeout`) bounds the read; one
    // it cut off reads as unknown paths, which withdraw the default.
    // trace:STORY-2884 trace:TASK-3792 | ai:claude
    let out = crate::forge_lookup::deadline_output(cmd, std::process::Command::output).ok()?;
    if !out.status.success() {
        return None;
    }
    let names = std::str::from_utf8(&out.stdout).ok()?;
    if !names.is_empty() && !names.ends_with('\0') {
        return None;
    }
    names
        .split_terminator('\0')
        .map(|path| (!path.is_empty()).then(|| path.to_owned()))
        .collect()
}

#[cfg(test)]
#[path = "tests/story_2884_unset_mode_tests.rs"]
mod tests;
