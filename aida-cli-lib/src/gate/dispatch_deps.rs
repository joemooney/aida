//! The gate-owned facade for everything `gate/dispatch_*` needs from outside
//! `gate/` (TASK-b6b1278890774ef993d2ebc327862470-013).
//!
//! Dispatch authority names no unprotected crate code directly: each external
//! item it uses comes through here. Code whose behaviour feeds a dispatch
//! decision (role-name canonicalisation, the requirement-store lookup, the
//! `/proc` namespace probe) is implemented here, on the protected floor; its
//! historical paths (`kernel`, `lib_part2`, `lib_part3`) delegate to it and
//! `agent_registry` uses the probe from here. What stays outside `gate/` is
//! pinned by name: the data-only [`CiState`] and the drive ledger's
//! [`ActionRow`], [`Outcome`] and [`emit_action`] audit sink, which hands
//! nothing back to the gate.
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

// ── Pinned items outside the gate ───────────────────────────────────────────

/// CI state the flow ship selector matches on. A data-only enum: the gate
/// calls none of its code.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) use crate::forge::CiState;

/// The drive ledger's row, outcome and audit sink. [`emit_action`] returns
/// nothing to the gate, so no edit behind it can change a dispatch decision.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
pub(crate) use crate::mandate_drive::{emit_action, ActionRow, Outcome};
