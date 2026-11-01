//! The gate-owned facade for everything `gate/dispatch_*` needs from outside
//! `gate/` (TASK-b6b1278890774ef993d2ebc327862470-013).
//!
//! `gate/dispatch_*` names no unprotected crate code directly: each external
//! item it uses comes through here. Code whose behaviour feeds a dispatch
//! decision (role-name canonicalisation, the requirement-store lookup, the
//! `/proc` namespace probe, the drive ledger's row and audit sink) is
//! implemented on the floor, here or in `gate/dispatch_types.rs`. What the
//! facade, and the gate's decision kernels, still invoke outside `gate/` is
//! listed by path in [`FACADE_EXTERNAL_LEAVES`].
// mandate-gate
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude

/// The process start identity a dispatch claim pins (`aida_core`, not crate
/// code).
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) use aida_core::liveness::process_start_identity;

// ── Role names ──────────────────────────────────────────────────────────────

/// The canonical first-class route target for "a human is required" — the
/// escalation-cascade terminus (implementer → advisor → human). `aida queue add
/// --for human` files a spec explicitly into the human-attention set, and
/// `aida list human` unions those explicitly-routed specs with the
/// status/tag-derived membership.
// trace:TASK-747 | ai:claude
pub(crate) const HUMAN_ROUTE: &str = "human";

/// Is this `--for <role>` value the `human` route target? Case-insensitive so
/// `--for Human` and `--for HUMAN` are accepted as the same first-class route.
// trace:TASK-747 | ai:claude
pub(crate) fn is_human_route(raw: &str) -> bool {
    raw.eq_ignore_ascii_case(HUMAN_ROUTE)
}

/// Canonicalize a role name. TASK-586 made `advisor` the canonical
/// identifier; `dialog` (TASK-279's old internal token) is now a
/// deprecated, silently-accepted alias so existing config / shells
/// (`AIDA_SESSION_ROLE=dialog`) / `dialog`-routed queue items / legacy
/// `dialog.toml` role files on not-yet-migrated machines keep resolving.
/// Applied at every role-name boundary: load, list, input resolution,
/// and queue routing.
// trace:TASK-586 | ai:claude
pub(crate) fn canonical_role_name(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else if is_human_route(raw) {
        // SPIKE-57 / TASK-747: `human` is a first-class route target, the
        // escalation-cascade terminus, symmetric with the agent roles. Normalize
        // any casing to the lowercase canonical form so `--for Human` /
        // `--for HUMAN` route identically and surface together in the view.
        HUMAN_ROUTE.to_string()
    } else if raw.eq_ignore_ascii_case("guest") || raw.eq_ignore_ascii_case("requester") {
        // Stakeholder roles are identities, not build-loop seats. Canonicalize
        // casing so the least-privilege gate applies uniformly from env, team
        // roster, and queue target validation. trace:STORY-1110 | ai:codex
        raw.to_ascii_lowercase()
    } else {
        raw.to_string()
    }
}

// ── Requirement store ───────────────────────────────────────────────────────

/// Best-effort load of the requirements store for read-only spec-id /
/// title / status lookups inside session-manifest rendering. Tries
/// distributed git-canonical mode first (the default), then falls back
/// to the legacy YAML/SQLite path. Returns None on any error — caller
/// renders with "(not found)" placeholders so missing data degrades
/// gracefully.
///
/// Pure with respect to `project_root`: BOTH resolution modes read only stores
/// reachable from that root, so this can never return another project's
/// requirements. No process CWD, no `REQ_DB_NAME`, no registry default — see
/// the legacy arm's comment for what went wrong when it did (BUG-1732).
// trace:STORY-98 trace:BUG-1732 | ai:claude
pub(crate) fn load_store_for_lookup(
    project_root: &std::path::Path,
) -> Option<aida_core::RequirementsStore> {
    // Git-canonical: walk up from project_root looking for an .aida/config.toml.
    if let Some(store_path) = aida_core::store_locate::detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(s) = aida_core::DatabaseBackend::load(&backend) {
                return Some(s);
            }
        }
    }
    // Legacy fallback for projects that haven't migrated. BUG-1732: this used
    // `determine_requirements_path(None)`, which probes for `requirements.db` /
    // `requirements.yaml` relative to the PROCESS CWD and honours an ambient
    // `REQ_DB_NAME` — both of which ignore the `project_root` every one of this
    // function's ~70 callers went to the trouble of resolving. On a host where
    // the cwd happens to hold an unrelated legacy store, a project-scoped
    // lookup silently returned that other project's requirements, which is how
    // `aida awaiting` came to report six In-Progress specs from a different
    // project in its orphaned-in-progress channel. BUG-1574's author already
    // documented this hazard and hand-bypassed the fallback for one call site
    // (see `unattended_git_mutation_refusal`); this makes the function itself
    // pure with respect to its argument so no caller has to remember.
    //
    // `resolve_requirements_path_in` is reused rather than reimplemented so the
    // probe ORDER (`requirements.db` before `requirements.yaml`) stays defined
    // in one place. Passing `project_root` as its cwd and `None` for both the
    // `-p` option and `REQ_DB_NAME` leaves no ambient input: on that arm the
    // resolver returns before it ever reads the registry, so the empty registry
    // path below is inert and is passed to make that unreadability explicit
    // rather than to imply the registry participates.
    // trace:BUG-1732 | ai:claude
    aida_core::resolve_requirements_path_in(project_root, None, None, std::path::Path::new(""))
        .ok()
        .map(aida_core::Storage::new)
        .and_then(|s| s.load().ok())
}

// ── /proc namespace probe ───────────────────────────────────────────────────

/// Raw `/proc` facts that say whether this process sits in a PID namespace
/// of its own (a sandboxed shell sees only its own few pids).
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PidNamespaceProbe {
    pub(crate) self_ns: Option<u64>,
    /// `None` when `/proc/1/ns/pid` is unreadable (pid 1 owned by another user).
    pub(crate) pid1_ns: Option<u64>,
    /// Entries on the `NSpid:` line of `/proc/self/status`.
    pub(crate) nspid_levels: usize,
    pub(crate) pid1_comm: Option<String>,
}

impl PidNamespaceProbe {
    pub(crate) fn read() -> Self {
        use std::os::unix::fs::MetadataExt;
        let ns = |path: &str| std::fs::metadata(path).ok().map(|m| m.ino());
        let nspid_levels = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|body| {
                body.lines()
                    .find_map(|l| l.strip_prefix("NSpid:"))
                    .map(|rest| rest.split_whitespace().count())
            })
            .unwrap_or(0);
        Self {
            self_ns: ns("/proc/self/ns/pid"),
            pid1_ns: ns("/proc/1/ns/pid"),
            nspid_levels,
            pid1_comm: std::fs::read_to_string("/proc/1/comm")
                .ok()
                .map(|c| c.trim().to_string()),
        }
    }

    /// Isolated when pid 1 lives in another namespace or the kernel reports
    /// more than one pid level. Only when `/proc/1/ns/pid` is unreadable does
    /// pid 1's name decide: anything but a known init means a sandbox.
    // trace:TASK-2272 | ai:claude
    pub(crate) fn is_isolated(&self) -> bool {
        const INITS: [&str; 10] = [
            "systemd",
            "init",
            "tini",
            "dumb-init",
            "docker-init",
            "catatonit",
            "runit",
            "openrc-init",
            "s6-svscan",
            "busybox",
        ];
        match (self.self_ns, self.pid1_ns) {
            (Some(a), Some(b)) if a != b => return true,
            _ => {}
        }
        if self.nspid_levels > 1 {
            return true;
        }
        if self.pid1_ns.is_some() {
            return false;
        }
        self.pid1_comm
            .as_deref()
            .is_some_and(|comm| !INITS.contains(&comm))
    }
}

// ── Drive ledger ────────────────────────────────────────────────────────────

/// The data the facade hands the gate, defined on the floor.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) use super::dispatch_types::{ActionRow, CiState, Outcome};

/// Ledger one `MandateDriveAction` row. Best-effort.
// trace:STORY-1700 | trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) fn emit_action(root: &std::path::Path, mandate_id: &str, actor: &str, row: &ActionRow) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateDriveAction {
            mandate_id: mandate_id.into(),
            actor: actor.into(),
            lane: row.lane.clone(),
            subject: row.subject.clone(),
            action: row.action.clone(),
            outcome: row.outcome.as_str().into(),
            why: row.why.clone(),
        },
    );
    if actor == "session" {
        event.seat = crate::events::active_seat();
    }
    crate::events::emit(root, &event);
}

// ── Leaves outside the gate ─────────────────────────────────────────────────

/// Every path outside `gate/` that this facade, or a decision-kernel gate
/// module, still reaches, with why it stays there. The orchestrating gate
/// modules' reach is frozen in `gate::orchestrator_leaves` instead. The
/// story_1888 ratchet fails on an unlisted path and on a stale entry, so
/// the list only shrinks unless a human merges a change to this file.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[cfg(test)]
pub(crate) const FACADE_EXTERNAL_LEAVES: &[(&str, &str)] = &[
    // ── this facade ──
    (
        "aida_core::liveness::process_start_identity",
        "sibling-crate process start probe",
    ),
    ("aida_core::RequirementsStore", "sibling-crate store type"),
    (
        "aida_core::store_locate::detect_distributed_store_from",
        "sibling-crate store discovery",
    ),
    ("aida_core::GitBackend::new", "sibling-crate store backend"),
    (
        "aida_core::DatabaseBackend::load",
        "sibling-crate store backend",
    ),
    (
        "aida_core::resolve_requirements_path_in",
        "sibling-crate legacy store path",
    ),
    ("aida_core::Storage::new", "sibling-crate legacy store"),
    ("crate::events::Event::new", "audit event shape; write-only"),
    (
        "crate::events::EventKind::MandateDriveAction",
        "audit event shape; write-only",
    ),
    (
        "crate::events::active_seat",
        "audit event seat label; write-only",
    ),
    ("crate::events::emit", "audit event sink; returns nothing"),
    // ── decision-kernel gate modules (TASK-b6b1278890774ef993d2ebc327862470-024) ──
    // aida_core is not gate-protected, so each sibling path a kernel reaches is
    // listed too (TASK-b6b1278890774ef993d2ebc327862470-041).
    (
        "aida_core::Comment",
        "spec comment shape for the sign-off check; read-only",
    ),
    (
        "aida_core::ExecutionMode",
        "spec execution-mode vocabulary for the mandate exclusions",
    ),
    (
        "aida_core::ExecutionMode::Decide",
        "decide-mode specs stay human",
    ),
    (
        "aida_core::ExecutionMode::Operator",
        "operator-mode specs stay human",
    ),
    (
        "aida_core::Requirement",
        "spec record the mandate classifies; read-only",
    ),
    (
        "aida_core::RequirementStatus::Completed",
        "spec status vocabulary",
    ),
    (
        "aida_core::RequirementType",
        "spec type vocabulary for the mandate exclusions",
    ),
    ("aida_core::Storage", "legacy store for the operator hold"),
    (
        "aida_core::external_tool_output::GH_BRANCH_POLICY_MERGE_REFUSAL",
        "forge refusal text the branch policy matches",
    ),
    (
        "aida_core::external_tool_output::contains_any_case_insensitive",
        "forge refusal text matcher",
    ),
    (
        "aida_core::git_ops::host_git_command",
        "git process builder for gate git reads",
    ),
    (
        "aida_core::lifecycle::CommitAuthority",
        "commit authority the seat resolver returns",
    ),
    (
        "aida_core::lifecycle::CommitAuthority::Grant",
        "commit authority by mandate grant",
    ),
    (
        "aida_core::lifecycle::CommitAuthority::Orchestrator",
        "commit authority by orchestrator seat",
    ),
    (
        "aida_core::lifecycle::register_commit_authority_resolver",
        "installs the gate's commit authority resolver",
    ),
    (
        "aida_core::liveness::pid_is_alive",
        "process liveness for fleet lanes; feeds authority",
    ),
    (
        "aida_core::liveness::process_identity_is_alive",
        "review marker holder liveness; feeds authority",
    ),
    (
        "aida_core::liveness::process_identity_matches_strict",
        "grant holder liveness; feeds authority",
    ),
    (
        "aida_core::liveness::process_start_time",
        "process start time for fleet lanes; feeds authority",
    ),
    (
        "aida_core::store_locate::MissedStore",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreMiss",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreMiss::NoConfig",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreMiss::NoStorePath",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreMiss::StoreMissing",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreMiss::TempRoot",
        "store discovery miss for seat authority",
    ),
    (
        "aida_core::store_locate::StoreOverride",
        "store override for seat authority",
    ),
    (
        "aida_core::store_locate::StoreOverride::Unusable",
        "store override for seat authority",
    ),
    (
        "aida_core::store_locate::StoreOverride::Usable",
        "store override for seat authority",
    ),
    (
        "aida_core::store_locate::locate_distributed_store_from",
        "store discovery for the team roster; feeds authority",
    ),
    (
        "aida_core::team::TeamRoster::load",
        "team roster for seat authority; feeds authority",
    ),
    (
        "aida_core::team::canonical_role",
        "seat name normalisation; feeds authority",
    ),
    ("crate::DoctorFinding", "doctor report row; output only"),
    (
        "crate::coordination::hostname",
        "host label on the review marker",
    ),
    (
        "crate::detect_distributed_store_from",
        "store discovery for grant scoping",
    ),
    ("crate::drain_caps::parse_duration", "grant TTL parser"),
    ("crate::find_project_root", "project root for seat display"),
    ("crate::glyph", "status glyph; output only"),
    ("crate::glyphs::Glyph::Warning", "status glyph; output only"),
    (
        "crate::grant_keyring::InstallError",
        "grant key store error type",
    ),
    (
        "crate::grant_keyring::KeyRead::Held",
        "grant key store read result",
    ),
    ("crate::grant_keyring::available", "grant key store probe"),
    ("crate::grant_keyring::install", "grant key store write"),
    (
        "crate::grant_keyring::install_to_parent",
        "grant key store write",
    ),
    ("crate::grant_keyring::invalidate", "grant key store revoke"),
    ("crate::grant_keyring::read", "grant key store read"),
    (
        "crate::history::git_show_blob",
        "reads the committed gate config blob",
    ),
    ("crate::home_dir", "home-directory lookup (test-hermetic)"),
    (
        "crate::is_aida_repo",
        "solo-review challenge applies to the AIDA repo",
    ),
    ("crate::is_terminal_status_str", "spec status vocabulary"),
    (
        "crate::leases::SessionLease",
        "lease record in the stale-lease refusal text",
    ),
    (
        "crate::main_worktree_root_from",
        "main worktree lookup for the verdict record",
    ),
    (
        "crate::mandate_drive::SESSION_ENV",
        "drive session environment variable name",
    ),
    (
        "crate::orchestrator::OrchestratorContext::Orchestrated",
        "orchestrator context variant",
    ),
    ("crate::orchestrator::detect", "orchestrator context probe"),
    (
        "crate::pr_cmd::pr_ship_hold_root",
        "hold root for the drive tick",
    ),
    (
        "crate::role::current_role_instance_is_companion",
        "companion-session probe",
    ),
    (
        "crate::role::effective_role_with_roster",
        "effective role for seat authority",
    ),
    (
        "crate::store_sync::aida_store_setting",
        "store setting lookup",
    ),
    (
        "crate::test_ambient::stderr_is_terminal",
        "test-only terminal seam",
    ),
    (
        "crate::test_ambient::stdin_is_terminal",
        "test-only terminal seam",
    ),
    (
        "crate::test_ambient::stdout_is_terminal",
        "test-only terminal seam",
    ),
    ("crate::test_home::assert_hermetic", "test-only home guard"),
    (
        "crate::trusted_config::read_trusted_config_toml_local",
        "trusted local config for the solo-review challenge",
    ),
    (
        "crate::worktree_gc::find_main_worktree_root",
        "main worktree lookup for the orchestrator probe",
    ),
];
