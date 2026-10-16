//! session leases: lease listing and gates, cross-clone leases, prune and force cleanup, shelve/phase-1 status flips and stale-lease recovery (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

// trace:TASK-714
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs_forge::TS)]
pub(crate) struct SessionLease {
    /// 12-char hex id derived from the time-ordered uuid v7, for short refs.
    pub(crate) id: String,
    /// Raw scope string the user passed to `--owns`.
    pub(crate) scope: String,
    /// Resolved scope slug, used for branch + dir naming.
    pub(crate) slug: String,
    /// Owner email/user (best-effort from git config).
    pub(crate) owner: String,
    /// Worktree path (canonicalized).
    pub(crate) worktree_path: std::path::PathBuf,
    /// Branch the worktree is on.
    pub(crate) branch: String,
    /// ISO-8601 UTC.
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    /// Hostname when started.
    pub(crate) hostname: String,
    /// Role active in the calling shell at session-start time, if any.
    /// Lets `aida session leases` / `aida session show` display role per
    /// session, and gives subsequent role-aware tooling inside the worktree
    /// the right starting persona. Optional for back-compat with older
    /// leases written before STORY-65. trace:STORY-65 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    /// PID of the shell that ran `aida session start`. Used by
    /// `aida session end` to resolve the right lease when the user runs
    /// `end` from the same parent shell that ran `start` (cwd is outside
    /// the worktree). Optional for back-compat. trace:STORY-73 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) creator_pid: Option<u32>,
    /// Kernel start identity paired with `creator_pid`. Missing on legacy
    /// leases, which deliberately retain the historical PID-only behavior.
    // trace:TASK-1284 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) creator_pid_start_time: Option<String>,
    /// BUG-741: PID of the currently hosted agent child for process-backed
    /// launches such as headless `codex exec`. Unlike `creator_pid`, this is
    /// the worker process itself: alive => Live, dead => Stale.
    /// BUG-752: harness-worktree (Agent-tool subagent) leases stamp the
    /// parent claude harness pid here — the subagent executes inside that
    /// process, so its lifetime bounds the work and the cwd-based worktree
    /// probe can never see it.
    // trace:BUG-741 | ai:codex
    // trace:BUG-752 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_pid: Option<u32>,
    /// Kernel start identity paired with `active_pid`; prevents a recycled PID
    /// from keeping a dead process-backed session live.
    // trace:TASK-1284 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_pid_start_time: Option<String>,
    /// Parent project's `target/` dir, captured so the session shell can
    /// share its cargo build cache with the parent worktree (avoids a full
    /// rebuild on first `cargo build` inside the session). `None` when the
    /// project doesn't have a `target/` (e.g., non-Rust project, or fresh
    /// checkout that's never been built). Sourced via `.aida/session-env.sh`.
    /// trace:STORY-52 | ai:claude
    #[serde(default)]
    pub(crate) cargo_target_dir: Option<std::path::PathBuf>,
    /// Parent project root (the worktree where `aida session start` ran).
    /// Recorded so cross-worktree views like `aida session list` can walk
    /// the parent's Claude Code session directory in addition to the
    /// session worktree's own. `None` for leases written before STORY-58.
    /// trace:STORY-58 | ai:claude
    #[serde(default)]
    pub(crate) parent_project_root: Option<std::path::PathBuf>,
    /// STORY-71: for PR/MR review sessions (--owns PR-N / MR-N), the
    /// PR head commit SHA captured via `gh pr view` / `glab mr view` at
    /// session-start time. `None` when the scope isn't a PR/MR or when
    /// the forge CLI wasn't available. Surfaced by `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_head_sha: Option<String>,
    /// STORY-71: PR base commit SHA at session-start time (companion to
    /// pr_head_sha). Lets the reviewer recompute the diff range later
    /// without round-tripping to the forge.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_base_sha: Option<String>,
    /// STORY-71: PR base ref name (e.g. `main`). Mostly informational —
    /// for reporting in `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_base_ref: Option<String>,
    /// BUG-237: zen-intent token for a standalone `aida queue work --zen`
    /// session — the per-invocation UUID the `--zen` dispatch minted into
    /// `AIDA_ZEN_TOKEN`. Its presence is what corroborates a standalone zen
    /// session: `aida zen status` trusts an inherited `AIDA_ZEN=1` only when
    /// the lease covering the worktree carries this token (or a live
    /// orchestrator run owns the session). `None` for non-`--zen` sessions —
    /// the dispatch scrubs `AIDA_ZEN_TOKEN` unless `--zen` was genuinely
    /// passed — so a leaked `AIDA_ZEN=1` cannot silently enable zen mode.
    /// trace:BUG-237 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zen_intent_token: Option<String>,
    /// TASK-358: when the orchestrator's `--escalate-blocks` path parks a
    /// punted spec for a human, it stamps this timestamp on the implementer's
    /// lease. The marker has two readers: `aida edit --status` (out of
    /// `NeedsAttention`) cleans up any lease carrying it for the triaged spec
    /// — STORY-306 deliberately left the worktree alive for the advisor's
    /// resume path, but a never-resumed escalation otherwise leaks; and
    /// `aida session prune --escalations` lists the same set for explicit
    /// bulk cleanup. Absent for interactive user sessions and for the
    /// advisor-resume path, so neither is touched.
    /// trace:TASK-358 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) escalated_to_human: Option<chrono::DateTime<chrono::Utc>>,
    /// STORY-248: when this session was started via `aida queue work
    /// --stack` or `--base BRANCH`, the branch we forked from. `None` for
    /// the default case (forked from `origin/main`). Paired with
    /// `parent_branch_sha`; both are also reflected into `.aida/stacks.json`
    /// for the auto-rebase cascade.
    /// trace:STORY-248 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_branch: Option<String>,
    /// STORY-248: HEAD SHA of `parent_branch` at fork time. Critical for
    /// the cascade: the project squash-merges + deletes branches, so a
    /// later `git rebase origin/main` would replay the parent's
    /// pre-squash commits. The cascade uses
    /// `git rebase --onto origin/main <parent_branch_sha> <branch>` to
    /// skip them. `None` when `parent_branch` is `None`.
    /// trace:STORY-248 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_branch_sha: Option<String>,
    /// BUG-511: marks a lease minted by the human-review verb
    /// (`aida review <spec>`). A review lease is an advisory lock with NO
    /// worktree (`worktree_path` is empty, the TASK-474 convention) — its
    /// liveness signal is the `aida review` process itself (`creator_pid`):
    /// alive → the spec is in flight (being reviewed); dead → the lease is
    /// stale and always safe to reap (no worktree, no uncommitted work).
    /// trace:BUG-511 | ai:claude
    #[serde(default)]
    pub(crate) review_verb: bool,
    /// TASK-957: marks a lease minted by `aida claim <spec>` — a spec-scoped
    /// ADVISORY claim that makes advisor-fanned work (an Agent-tool subagent,
    /// which otherwise takes only a generic `harness-worktree` lease) visible
    /// to BUG-637's duplicate-dispatch gates. Like a review lease it has no
    /// worktree of its own (`worktree_path` may be empty when no `--worktree`
    /// was given), so its liveness signal is the claiming process recorded in
    /// `creator_pid`: alive → the claim still holds; dead → stale and ignored
    /// by every gate (no crash-deadlock). The scope IS the spec id, so the
    /// claim matches `live_spec_claim_by_other` / `lease_owning_spec` /
    /// `spec_scoped_lease` exactly like an AIDA-launched spec lease.
    // trace:TASK-957 | ai:claude
    #[serde(default)]
    pub(crate) claim_verb: bool,
    /// BUG-778: stamped when this worktree was handed to a HUMAN by
    /// `aida worktree enter|add` — the verbs that take the implementer lease
    /// and then deliberately launch NO agent (the operator starts one
    /// themselves). Its presence is the "was an agent ever expected here?"
    /// provenance `aida ps` needs: without it, the seconds between `enter` and
    /// the operator's launch look exactly like a crashed agent, and the row
    /// read "process dead — resume/rebrief: `aida queue work <spec>`" — a hint
    /// that would start a SECOND session competing with the hand-worked one.
    /// Refreshed on every re-enter (the worktree is being handed over again).
    /// Absent on orchestrator-spawned leases, so their crash detection is
    /// unchanged.
    // trace:BUG-778 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manual_enter_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TASK-1518: stamped by the drain's SIGTERM handler on every lease the
    /// stopped wave created (`creator_pid` match) as it releases the drain
    /// lock — an INTERRUPTED session, not an abandoned one. `aida ps` reads
    /// it: a stamped lease with no live process and a clean tree classifies
    /// `stopped` (worktree intact, resume as normal) instead of a dead agent.
    /// Written as a generic TOML key by `drain_signal`, so a lease this
    /// binary does not otherwise model keeps its other keys.
    // trace:TASK-1518 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interrupted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TASK-1518: why the lease was interrupted (`sigterm`). Informational;
    /// the classifier keys on `interrupted_at`.
    // trace:TASK-1518 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interrupted_reason: Option<String>,
    /// The driver this lease's session works for (`relay:<run>`, ...), read
    /// from `AIDA_RELAY_RUN` / `AIDA_DRIVER` when the lease was minted. Lets
    /// a lease whose own process is gone read as driven while its driver is
    /// live. Optional; older leases parse without it.
    // trace:STORY-1653 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) driver: Option<String>,
}

pub(crate) fn leases_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("sessions")
}

/// Max display width for the @SCOPE statusline segment (matches @SPEC budget).
/// trace:STORY-55 | ai:claude
pub(crate) const SCOPE_LABEL_MAX: usize = 12;

/// Max display width for the `sess:` statusline label (scope + batch
/// suffix). Slightly wider than SCOPE_LABEL_MAX so common `epic-N#batchM`
/// shapes don't truncate the scope when the suffix is the new signal.
/// Shared by the standalone `sess:` segment and the `[sess:]` anchor
/// annotation folded into `@SPEC`. trace:TASK-60 trace:TASK-282 | ai:claude
pub(crate) const SESS_LABEL_MAX: usize = 20;

pub(crate) fn lease_path(project_root: &std::path::Path, id: &str) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.toml", id))
}

pub(crate) fn session_harness_branch(
    cwd: &std::path::Path,
    override_branch: Option<&str>,
) -> Result<String> {
    if let Some(branch) = override_branch.filter(|s| !s.trim().is_empty()) {
        return Ok(branch.to_string());
    }
    current_branch_at(cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "could not derive branch for harness worktree `{}`",
            cwd.display()
        )
    })
}

pub(crate) fn session_harness_worktree_register(
    agent_id: &str,
    cwd: &str,
    agent_type: Option<&str>,
    branch: Option<&str>,
    scope: Option<&str>,
) -> Result<()> {
    let cwd = std::path::PathBuf::from(cwd);
    // The lease branch later reaches `git log` / `git diff` / `git cherry`;
    // never store one git would read as an option. trace:BUG-1622 | ai:claude
    if let Some(b) = branch {
        git_arg_guard::reject_option_like("--branch", b)?;
    }
    let branch = session_harness_branch(&cwd, branch)?;
    let payload = worktree_lease::SubagentPayload {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.map(|s| s.to_string()),
        cwd: cwd.clone(),
    };
    let mut spec = worktree_lease::lease_spec_for_start(&payload, &branch);
    if let Some(scope) = scope.filter(|s| !s.trim().is_empty()) {
        spec.scope = scope.to_string();
    }

    let project_root = find_main_worktree_root()?;

    // BUG-1772 AC1: A SubagentStart whose cwd is the project root has no
    // dedicated worktree of its own. A "worktree lease" here would track nothing
    // and would erroneously absorb the liveness of any agent process in the root.
    // So we do not write a lease at all.
    // trace:BUG-1772 | ai:antigravity
    let cwd_canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let root_canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());
    if cwd_canonical == root_canonical {
        return Ok(());
    }

    let id = worktree_lease::lease_id_from_agent_id(&spec.agent_id);
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let active_pid = process_probe::nearest_claude_ancestor_pid(std::process::id());
    let lease = SessionLease {
        id: id.clone(),
        scope: spec.scope.clone(),
        slug: slugify(&spec.scope),
        owner,
        worktree_path: spec
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| spec.worktree_path.clone()),
        branch: spec.branch.clone(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        role: spec.agent_type.clone(),
        creator_pid: None,
        creator_pid_start_time: None,
        // BUG-752: an Agent-tool subagent runs INSIDE the parent claude
        // harness process (cwd = parent project root, never the isolation
        // worktree), so the cwd-based worktree probe can never see it — a
        // pid-less harness lease read dormant/"dead process, salvageable"
        // while the agent was actively working. This register command runs
        // as a hook child of that same harness process: walk our own
        // ancestry and stamp the nearest live claude pid as the lease's
        // liveness signal — the agent-tool analog of BUG-741's headless
        // codex child-pid stamp. `None` when no claude ancestor is found
        // (e.g. a detached hook runner); the classifier then treats
        // liveness as unknown rather than dead. trace:BUG-752 | ai:claude
        active_pid,
        active_pid_start_time: active_pid.and_then(process_probe::process_start_identity),
        cargo_target_dir: None,
        parent_project_root: Some(
            project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.clone()),
        ),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: aida_core::driver_registry::driver_from_env(), // trace:STORY-1653 | ai:claude
    };

    std::fs::create_dir_all(leases_dir(&project_root))?;
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(
        &lease_path(&project_root, &id),
        toml::to_string_pretty(&lease)?,
    )?;
    // BUG-1918: durable record of who is authoring this worktree's work.
    // trace:BUG-1918 | ai:claude
    review_authority::record_authoring_lease(&project_root, &lease);
    println!(
        "registered harness worktree lease {} scope:{} branch:{}",
        id, lease.scope, lease.branch
    );
    // BUG-1656: a subagent dispatched INTO a worktree an existing spec lease
    // owns adopts that lease — its harness pid becomes the spec lease's live
    // worker, so `aida ps` / `aida awaiting` stop calling the spec dead.
    // trace:BUG-1656 | ai:claude
    if let Some(adopted) =
        adopt_spec_lease_for_subagent(&project_root, &lease.worktree_path, active_pid)
    {
        println!(
            "adopted spec lease {adopted} for this subagent (its worktree is this subagent's cwd)"
        );
        // BUG-1918: the adopting subagent now authors that spec's work.
        // trace:BUG-1918 | ai:claude
        if let Some(spec_lease) = list_leases(&project_root)
            .into_iter()
            .find(|l| l.id == adopted)
        {
            review_authority::record_authoring_lease(&project_root, &spec_lease);
        }
    }
    // BUG-754: a spec-scoped harness lease means a fanned-out implementer is
    // now working that spec — flip Approved → In Progress at lease-take (the
    // same coherence bump `aida session start` performs per BUG-379) so
    // `aida queue list` / `aida list` never show the same spec as both a
    // pickable Approved row and in-flight/leased.
    // trace:BUG-754 | ai:claude
    if bump_spec_in_progress_at_lease_take(&project_root, &lease.scope) {
        println!(
            "bumped {} Approved → In Progress (spec-scoped lease taken)",
            lease.scope
        );
    }
    Ok(())
}

/// BUG-1656 (2): is `lease_scope`/`lease_worktree` a spec-scoped lease whose
/// worktree is exactly the subagent's `cwd`? Pure so the adoption rule is
/// unit-testable without a lease dir. Generic harness leases and non-spec
/// scopes are never adopted; neither is a lease for a different worktree.
// trace:BUG-1656 | ai:claude
pub(crate) fn subagent_adopts_lease(
    lease_scope: &str,
    lease_worktree: &std::path::Path,
    cwd: &std::path::Path,
) -> bool {
    if lease_scope.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
        return false;
    }
    if worktree_lease::spec_id_from_branch(lease_scope).is_none() {
        return false;
    }
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    !lease_worktree.as_os_str().is_empty() && canon(lease_worktree) == canon(cwd)
}

/// BUG-1656 (2): give an Agent-tool subagent the spec lease of the worktree it
/// was dispatched into. Walks `.aida/sessions/`, finds the spec-scoped lease
/// whose `worktree_path` is `cwd`, and — unless that lease already records a
/// LIVE `active_pid` — stamps `harness_pid` as its `active_pid` (plus the
/// kernel start identity and an `adopted_by_subagent_at` marker). Patched as
/// generic TOML key inserts (the `manual_enter_at` pattern) so keys this
/// binary does not model survive. Returns the adopted lease id. Best-effort:
/// unreadable leases are skipped, never rewritten.
// trace:BUG-1656 | ai:claude
pub(crate) fn adopt_spec_lease_for_subagent(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    harness_pid: Option<u32>,
) -> Option<String> {
    let pid = harness_pid?;
    let entries = std::fs::read_dir(leases_dir(project_root)).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut value) = toml::from_str::<toml::Value>(&body) else {
            continue;
        };
        let Some(table) = value.as_table_mut() else {
            continue;
        };
        let scope = table
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let worktree = table
            .get("worktree_path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        if !subagent_adopts_lease(&scope, &worktree, cwd) {
            continue;
        }
        let already_live = table
            .get("active_pid")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
            .is_some_and(process_probe::pid_is_alive);
        if already_live {
            continue;
        }
        table.insert(
            "active_pid".to_string(),
            toml::Value::Integer(i64::from(pid)),
        );
        match process_probe::process_start_identity(pid) {
            Some(start) => {
                table.insert(
                    "active_pid_start_time".to_string(),
                    toml::Value::String(start),
                );
            }
            None => {
                table.remove("active_pid_start_time");
            }
        }
        table.insert(
            "adopted_by_subagent_at".to_string(),
            toml::Value::String(chrono::Utc::now().to_rfc3339()),
        );
        let id = table
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        let Ok(content) = toml::to_string_pretty(&value) else {
            continue;
        };
        if write_atomic(&path, &content).is_ok() {
            return Some(id);
        }
    }
    None
}

// BUG-754: does this harness-lease scope name a spec (vs the generic
// `harness-worktree` scope or a non-spec label)? Returns the canonical
// SPEC-ID when it does. Pure, so the fanout-pickup bump gate is
// unit-testable without a store fixture.
// trace:BUG-754 | ai:claude
pub(crate) fn harness_lease_spec_scope(scope: &str) -> Option<String> {
    if scope.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
        return None;
    }
    worktree_lease::spec_id_from_branch(scope)
}

// BUG-754: at lease-take for a spec-scoped harness lease, bump the spec
// Approved → In Progress — the same coherence flip `aida session start`
// performs after saving its lease (BUG-379) — so a harness-fanout
// implementer's spec never reads as a pickable Approved row while an active
// lease reports it in flight. Targeted single-spec write (the BUG-634 rule:
// resolve the one spec via the cache-backed backend and rewrite only its
// YAML); this runs from the SubagentStart hook's small time budget, so a
// full-store load/save is not acceptable here. Idempotent: any status other
// than Approved is left alone — the lifecycle owns those transitions (drain
// completion still lands Done via `aida queue done`, a shelve still lands
// NeedsAttention, and `aida doctor` restores In Progress-without-lease back
// to Approved). Failure is non-fatal: the doctor's spec-status-drift heal
// ("Approved but lease active → bump to In Progress") is the backstop.
// trace:BUG-754 | ai:claude
pub(crate) fn bump_spec_in_progress_at_lease_take(
    project_root: &std::path::Path,
    scope: &str,
) -> bool {
    let Some(spec_id) = harness_lease_spec_scope(scope) else {
        return false;
    };
    // BUG-1944: one shared targeted bump (BUG-634/1637/1638 rules live there).
    // trace:BUG-1944 | ai:claude
    crate::live_work::bump_spec_in_progress(
        project_root,
        &spec_id,
        aida_core::conflict::LEASE_TAKE_AUTHOR,
    )
}

// BUG-754: fanout-pickup lease-take status-bump tests.
// trace:BUG-754 | ai:claude
#[cfg(test)]
#[path = "tests/harness_lease_bump_tests.rs"]
mod harness_lease_bump_tests;

pub(crate) fn session_harness_worktree_release(agent_id: &str) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let payload = worktree_lease::SubagentPayload {
        agent_id: agent_id.to_string(),
        agent_type: None,
        cwd: std::path::PathBuf::new(),
    };
    let id = worktree_lease::lease_id_for_stop(&payload);
    let path = lease_path(&project_root, &id);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("released harness worktree lease {}", id);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Lease-occupancy primitive: find a lease that already occupies `worktree` and
/// is still live. Pure over the lease set + an injected liveness predicate: the
/// caller supplies PID-liveness / dormancy (via `creator_pid` + the same logic
/// `classify_for_auto_release` uses), so a dead or auto-released lease's worktree
/// is correctly seen as free. Path comparison is exact — the caller canonicalizes
/// `worktree` to match the canonicalized `lease.worktree_path`.
///
/// NOTE: BUG-416's shipped fix is the attribution-aware `aida add` hint, which
/// gates on *agent* occupancy (`agent_registry::live_agents_covering_cwd`), not
/// lease occupancy — so this primitive is currently unused. Retained as the
/// natural building block for lease-hygiene work that needs "is this worktree
/// still claimed?" (e.g. BUG-362 pr-ship lease cleanup). trace:TASK-607 | ai:claude
#[allow(dead_code)] // unused since BUG-416 chose agent-occupancy; kept for lease-hygiene reuse
pub(crate) fn worktree_occupant<'a>(
    worktree: &std::path::Path,
    leases: &'a [SessionLease],
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<&'a SessionLease> {
    leases
        .iter()
        .find(|l| l.worktree_path == worktree && is_live(l))
}

pub(crate) fn list_leases(project_root: &std::path::Path) -> Vec<SessionLease> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // trace:BUG-1814 | ai:antigravity
            if !file_name.ends_with(".toml")
                || file_name.ends_with(".manifest.toml")
                || file_name.ends_with(".activity.toml")
            {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(lease) = toml::from_str::<SessionLease>(&content) {
                    out.push(lease);
                }
            }
        }
    }
    out.sort_by_key(|l| l.started_at);
    out
}

/// Every persisted spec-scoped lease blocks agent dispatch, including stale ones.
/// Keep default listing aligned with that conservative ownership check.
// trace:BUG-2002 | ai:codex
pub(crate) fn lease_blocks_agent_dispatch(lease: &SessionLease) -> bool {
    !lease.scope.is_empty()
        && !lease
            .scope
            .eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE)
}

// trace:BUG-2002 | ai:codex
pub(crate) fn lease_visible_in_default_listing(lease: &SessionLease, state: LeaseState) -> bool {
    !matches!(state, LeaseState::Stale) || lease_blocks_agent_dispatch(lease)
}

// The actual listing and its tests share the default/--all selection boundary.
// trace:BUG-2002 | ai:codex
type ClassifiedLease = (SessionLease, LeaseState, Option<u32>);

// trace:BUG-2002 | ai:codex
pub(crate) fn select_lease_listing(
    classified: Vec<ClassifiedLease>,
    all: bool,
) -> (Vec<ClassifiedLease>, Vec<ClassifiedLease>) {
    if all {
        (classified, Vec::new())
    } else {
        classified
            .into_iter()
            .partition(|(lease, state, _)| lease_visible_in_default_listing(lease, *state))
    }
}

/// BUG-574: pure decision — does THIS clone already hold a lease for `scope`?
/// Returns the existing lease (the first by start time, matching `list_leases`'
/// sort) so `aida session start` can treat a re-run as benign idempotent
/// re-entry (exit 0, report the live session) rather than a hard error.
/// Case-insensitive on the raw scope string, mirroring the old guard.
/// trace:BUG-574 | ai:claude
pub(crate) fn existing_lease_for_scope<'a>(
    leases: &'a [SessionLease],
    scope: &str,
) -> Option<&'a SessionLease> {
    // BUG-2143: an advisory lease (`aida claim` / `aida review`) is never
    // re-entered, whatever path it recorded, and neither is a blank path —
    // "re-entering" one printed `cd ` with a blank or bogus path as success and
    // created nothing. Callers pass leases pre-filtered by
    // `worktree_gc::reenterable_leases`, which also drops MCP claims and any
    // path that is not a live linked worktree.
    // trace:BUG-2143 | ai:claude
    leases.iter().find(|l| {
        l.scope.eq_ignore_ascii_case(scope)
            && !l.claim_verb
            && !l.review_verb
            && !l.worktree_path.to_string_lossy().trim().is_empty()
    })
}

/// BUG-483: pure decision — does any lease OTHER than the one being ended
/// share `worktree_path`? `aida agent new` can land two sessions in one
/// worktree (BUG-416), and `session end` force-removes the worktree
/// unconditionally — which would strand the peer (TASK-0396 cross-worktree
/// fingerprint hazard + BUG-108 `(deleted)` cwd). We filter the ending lease
/// out by id and compare canonicalized paths so symlink chains don't mask a
/// match. Conservative: returns the first peer found so the caller can SKIP
/// the removal and name the peer. trace:BUG-483 | ai:claude
pub(crate) fn peer_lease_sharing_worktree<'a>(
    leases: &'a [SessionLease],
    ending_id: &str,
    worktree_path: &std::path::Path,
) -> Option<&'a SessionLease> {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target_canon = canon(worktree_path);
    leases.iter().find(|l| {
        l.id != ending_id
            && !l.worktree_path.as_os_str().is_empty()
            && canon(&l.worktree_path) == target_canon
    })
}

/// TASK-670: the set of requirement UUIDs that sit in *some* role queue
/// (across every user's queue file), for the leading queued work-routing glyph on
/// `aida list`. Cheap by design — one `read_dir` over
/// `.aida-store/registry/queues/` + a YAML parse per file, no Storage::load().
/// Work-routing is a project-global axis (not the current shell's queue), so we
/// union across all queue files. Returns an empty set when the dir is absent.
/// trace:TASK-670 | ai:claude
pub(crate) fn all_queued_requirement_ids(project_root: &std::path::Path) -> HashSet<Uuid> {
    let mut out = HashSet::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) {
            for item in items {
                out.insert(item.requirement_id);
            }
        }
    }
    out
}

/// The queue-insertion timestamp per requirement — requirement UUID → the
/// EARLIEST `added_at` across every user's queue file. The substrate fact the
/// burndown's ready-set ordering is anchored on: "when did this spec enter the
/// queue", as opposed to the accident of how its spec id happens to spell.
///
/// Same cheap `read_dir` + per-file YAML parse as [`all_queued_requirement_ids`]
/// (no `Storage::load()`), and the same project-global union: a spec routed into
/// two role queues carries the earlier of the two stamps, so re-routing work
/// never silently moves it to the back of the wave. Returns an empty map when
/// the queue dir is absent.
// trace:TASK-1175 | ai:claude
pub(crate) fn all_queued_added_at(
    project_root: &std::path::Path,
) -> std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> {
    let mut out: std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> =
        std::collections::HashMap::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) {
            for item in items {
                out.entry(item.requirement_id)
                    .and_modify(|at| {
                        if item.added_at < *at {
                            *at = item.added_at;
                        }
                    })
                    .or_insert(item.added_at);
            }
        }
    }
    out
}

/// BUG-527: queue memberships for a single spec, across every user's queue
/// file, for the `Queued:` line on `aida show <ID>` (and the MCP
/// `show_requirement` mirror). Each membership is `(for_role, rank)`, where
/// `for_role` is the canonical routing role (`None` = unrouted / general
/// queue) and `rank` is the 1-based position the operator sees on
/// `aida queue list` — the index within that role's queue ordered by raw
/// on-disk position, NOT the sparse `entry.position` value (STORY-72's
/// `max_position + 1000` scheme means the raw value is not a human rank).
/// Reuses the same cheap `read_dir` + per-file YAML parse as
/// [`all_queued_requirement_ids`] (no `Storage::load()`); queue membership
/// is a project-global axis so we union across all queue files. Returns an
/// empty vec when the dir is absent or the spec sits in no queue (the
/// caller omits the line in that case). trace:BUG-527
pub(crate) fn queue_memberships_for(
    project_root: &std::path::Path,
    requirement_id: &Uuid,
) -> Vec<(Option<String>, usize)> {
    let mut out: Vec<(Option<String>, usize)> = Vec::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(dir_entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for dirent in dir_entries.flatten() {
        let p = dirent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) else {
            continue;
        };
        // Rank is per-role within this user's queue, ordered by raw position
        // (the same ordering `aida queue list` groups + numbers by).
        items.sort_by_key(|e| e.position);
        let mut rank_by_role: std::collections::HashMap<Option<String>, usize> =
            std::collections::HashMap::new();
        for item in &items {
            let role = item.for_role.as_deref().map(canonical_role_name);
            let rank = rank_by_role.entry(role.clone()).or_insert(0);
            *rank += 1;
            if &item.requirement_id == requirement_id {
                out.push((role, *rank));
            }
        }
    }
    // Stable presentation order: routed roles alphabetically, unrouted last,
    // then by rank.
    out.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    out
}

/// BUG-527: render the `(for_role, rank)` memberships from
/// [`queue_memberships_for`] as the value half of the `Queued:` line, e.g.
/// `implementer (pos 2)` or `implementer (pos 2), reviewer (pos 1)`.
/// Unrouted entries render as `general (pos N)`. Returns `None` when there
/// are no memberships so the caller can omit the line entirely.
/// trace:BUG-527
pub(crate) fn format_queue_membership(memberships: &[(Option<String>, usize)]) -> Option<String> {
    if memberships.is_empty() {
        return None;
    }
    let parts: Vec<String> = memberships
        .iter()
        .map(|(role, rank)| {
            let label = role.as_deref().unwrap_or("general");
            format!("{} (pos {})", label, rank)
        })
        .collect();
    Some(parts.join(", "))
}

/// TASK-670: the set of scope strings held by *live* session leases — the input
/// for the in-flight work-routing glyph. "Live" means a running claude was
/// found inside the lease's worktree (the same probe `aida session leases` uses
/// for its `live` state); dormant / stale / reaped leases are excluded so a
/// dead session never shows the in-flight marker (cf. STORY-496). Scopes are lowercased for a
/// case-insensitive match against a row's spec/agreed id. trace:TASK-670 | ai:claude
pub(crate) fn in_flight_lease_scopes(project_root: &std::path::Path) -> HashSet<String> {
    in_flight_lease_role_map(project_root).into_keys().collect()
}

// TASK-1267: `--express` is a one-release compatibility alias for ordinary
// mode=drain intake. It deliberately emits no batch:express or lifecycle tag.
// The default fasttrack filing convention is unchanged.
// trace:TASK-1267 | ai:codex
pub(crate) fn fasttrack_lane_filing(
    express: bool,
) -> (Option<String>, Option<String>, Option<String>) {
    if express {
        (None, None, Some("drain".to_string()))
    } else {
        (
            Some("fasttrack".to_string()),
            Some("lifecycle:no-review".to_string()),
            None,
        )
    }
}

// The eight lane stages a fasttrack/express item can occupy, in roughly
// request-to-ship order. This is a *projection* — there is no new store; the
// stage is derived from the spec's existing status, queue membership, active
// lease, punt ledger, and merged (Completed) state.
// trace:TASK-905 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FasttrackStage {
    /// Draft — filed, not yet dispositioned.
    Requested,
    /// Approved — blessed but not yet on a queue.
    Accepted,
    /// Approved/Planned and sitting in a role queue, awaiting a drain owner.
    Queued,
    /// InProgress, or a live lease holds the spec (a drain is working it).
    Running,
    /// NeedsAttention with no punt record — parked, but not a recorded punt.
    Blocked,
    /// NeedsAttention with a punt record — left the lane loudly (the punt-out
    /// invariant: non-trivial discovery parks + records a punt).
    Punted,
    /// Done or Completed — work finished (Completed == merged to main).
    Shipped,
    /// Rejected — declined; will not be done.
    Rejected,
}

impl FasttrackStage {
    /// Lowercase slug used in CLI output and the `--json` `stage` field.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            FasttrackStage::Requested => "requested",
            FasttrackStage::Accepted => "accepted",
            FasttrackStage::Queued => "queued",
            FasttrackStage::Running => "running",
            FasttrackStage::Blocked => "blocked",
            FasttrackStage::Punted => "punted",
            FasttrackStage::Shipped => "shipped",
            FasttrackStage::Rejected => "rejected",
        }
    }
}

/// Pure stage projection for a single lane item. Kept side-effect-free (takes
/// already-derived booleans, returns the stage) so it is unit-testable without
/// a store, a queue dir, or a lease probe.
///
/// `status_str` is the cache's status string (the `Debug` form of
/// `RequirementStatus` — `Draft`, `Approved`, `Planned`, `InProgress`, `Done`,
/// `Completed`, `Rejected`, `NeedsAttention`); matching is case-insensitive so a
/// `custom_status` row degrades gracefully (unknown → its closest status, else
/// `requested`). `is_queued` = in some role queue; `is_running` = a live lease
/// holds it; `has_punt` = an (unresolved) punt record names it.
///
/// Mapping (per the fasttrack-lane design's status→stage table):
///   Draft            → requested
///   Approved         → accepted, or queued (in a queue), or running (live lease)
///   Planned          → queued, or running (live lease)
///   InProgress       → running
///   NeedsAttention   → punted (has a punt record) else blocked
///   Done | Completed → shipped
///   Rejected         → rejected
// trace:TASK-905 | ai:claude — see docs/plans/2026-06-26-task-0438-fasttrack-lane.md
pub(crate) fn project_fasttrack_stage(
    status_str: &str,
    is_queued: bool,
    is_running: bool,
    has_punt: bool,
) -> FasttrackStage {
    match status_str.to_ascii_lowercase().as_str() {
        "draft" => FasttrackStage::Requested,
        "approved" => {
            if is_running {
                FasttrackStage::Running
            } else if is_queued {
                FasttrackStage::Queued
            } else {
                FasttrackStage::Accepted
            }
        }
        "planned" => {
            if is_running {
                FasttrackStage::Running
            } else {
                FasttrackStage::Queued
            }
        }
        "inprogress" => FasttrackStage::Running,
        "needsattention" => {
            if has_punt {
                FasttrackStage::Punted
            } else {
                FasttrackStage::Blocked
            }
        }
        "done" | "completed" => FasttrackStage::Shipped,
        "rejected" => FasttrackStage::Rejected,
        // Unknown / custom status: a live lease or queue membership still tells
        // us where it is; otherwise treat it as the entry stage.
        _ => {
            if is_running {
                FasttrackStage::Running
            } else if is_queued {
                FasttrackStage::Queued
            } else {
                FasttrackStage::Requested
            }
        }
    }
}

/// `aida fasttrack status` — the lane stage projection.
///
/// Cache-fast: it reuses `backend.list_summaries` (the same projection
/// `aida list` reads) for the current fasttrack and legacy express buckets, and
/// the cheap queue-dir scan + live-lease probe + punt-ledger read the list view
/// already uses for its routing glyphs — NOT the full-store `aida status` scan.
// trace:TASK-905 | ai:claude
pub(crate) fn handle_fasttrack_status(
    store_path: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    json: bool,
) -> Result<()> {
    // Each lane bucket is one tag filter. The cache's tag filter is AND-only, so
    // we run one query per bucket (with archive/defer set to Both so a
    // parked/archived lane item still surfaces in its own status surface) and
    // tag each row with the bucket it came from. A row carrying BOTH tags is
    // kept once, under fasttrack (the more-trivial tier) — fasttrack is listed
    // first so the de-dup HashSet skips the express copy.
    let mut rows: Vec<(&'static str, aida_core::RequirementSummary)> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    for (bucket, tag) in [
        ("fasttrack", "batch:fasttrack"),
        // trace:TASK-1267 | ai:codex — read-only legacy compatibility.
        ("express", "batch:express"),
    ] {
        let filter = aida_core::ListFilter {
            tags: vec![tag.to_string()],
            archive: aida_core::ArchiveFilter::Both,
            defer: aida_core::DeferFilter::Both,
            ..Default::default()
        };
        for s in backend.list_summaries(&filter)? {
            if seen.insert(s.id) {
                rows.push((bucket, s));
            }
        }
    }

    // Routing signals — the same cheap reads `aida list` uses for its flow
    // glyphs. project root = parent of the `.aida-store` worktree.
    let routing_root = store_path.parent().map(|p| p.to_path_buf());
    let queued_ids: HashSet<Uuid> = routing_root
        .as_deref()
        .map(all_queued_requirement_ids)
        .unwrap_or_default();
    let in_flight_scopes: HashSet<String> = routing_root
        .as_deref()
        .map(in_flight_lease_scopes)
        .unwrap_or_default();
    // Specs named by an unresolved punt record (distinguishes punted vs blocked
    // within NeedsAttention). `read_ledger` is a single JSONL read.
    let punted_specs: HashSet<String> = routing_root
        .as_deref()
        .map(|root| {
            punt::read_ledger(root)
                .into_iter()
                .map(|r| r.spec.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let stage_of = |bucket: &str, r: &aida_core::RequirementSummary| -> FasttrackStage {
        let is_queued = queued_ids.contains(&r.id);
        let is_running = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()));
        let has_punt = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| punted_specs.contains(&id.to_ascii_lowercase()));
        let _ = bucket;
        project_fasttrack_stage(&r.status, is_queued, is_running, has_punt)
    };

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(bucket, r)| {
                let stage = stage_of(bucket, r);
                serde_json::json!({
                    "spec_id": r.spec_id.as_deref().or(r.agreed_id.as_deref()),
                    "title": r.title,
                    "tier": bucket,
                    "status": r.status,
                    "stage": stage.slug(),
                })
            })
            .collect();
        println!("{}", crate::cache_output::json_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("{}", "No fasttrack-lane items.".yellow());
        return Ok(());
    }

    // Order rows by lane progression so the surface reads request → ship.
    let stage_rank = |s: FasttrackStage| -> u8 {
        match s {
            FasttrackStage::Requested => 0,
            FasttrackStage::Accepted => 1,
            FasttrackStage::Queued => 2,
            FasttrackStage::Running => 3,
            FasttrackStage::Blocked => 4,
            FasttrackStage::Punted => 5,
            FasttrackStage::Shipped => 6,
            FasttrackStage::Rejected => 7,
        }
    };
    let mut ordered: Vec<(&'static str, &aida_core::RequirementSummary, FasttrackStage)> = rows
        .iter()
        .map(|(bucket, r)| (*bucket, r, stage_of(bucket, r)))
        .collect();
    ordered.sort_by_key(|(_, _, stage)| stage_rank(*stage));

    println!("{}  {} item(s)", "Fasttrack lane".bold(), ordered.len());
    println!(
        "{}",
        "requested → accepted → queued → running → blocked → punted → shipped → rejected".dimmed()
    );
    println!("{}", "─".repeat(72));
    for (bucket, r, stage) in &ordered {
        let id = r
            .spec_id
            .as_deref()
            .or(r.agreed_id.as_deref())
            .unwrap_or("-");
        let stage_disp = match stage {
            FasttrackStage::Requested => stage.slug().yellow(),
            FasttrackStage::Accepted => stage.slug().blue(),
            FasttrackStage::Queued => stage.slug().cyan(),
            FasttrackStage::Running => stage.slug().magenta(),
            FasttrackStage::Blocked | FasttrackStage::Punted => stage.slug().red().bold(),
            FasttrackStage::Shipped => stage.slug().green(),
            FasttrackStage::Rejected => stage.slug().red(),
        };
        println!("{:<10} {:<11} [{}] {}", id, stage_disp, bucket, r.title);
    }
    Ok(())
}

/// BUG-511: lease-state classification that understands review-verb
/// advisory leases. A review lease has no worktree, so the standard
/// worktree/claude/age matrix would always call it stale; its real
/// liveness signal is the `aida review` process recorded in
/// `creator_pid`. Session leases fall through to [`classify_lease_state`]
/// unchanged. trace:BUG-511 | ai:claude
pub(crate) fn lease_state_for(
    l: &SessionLease,
    live_sessions: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> LeaseState {
    // BUG-511 review leases AND TASK-957 claim leases are advisory locks when
    // they have no worktree of their own — the standard worktree/claude/age
    // matrix would always call them stale. Their real liveness signal is the
    // process that minted them, recorded in `creator_pid`: alive → Live,
    // dead/absent → Stale (never Dormant; an advisory lock's lifetime is
    // exactly its process's lifetime). BUG-882 lets reviewer worktree sessions
    // carry review_verb too; those must still use normal worktree liveness.
    // trace:BUG-511 trace:TASK-957 | ai:claude
    // trace:BUG-882 | ai:codex
    if (l.review_verb || l.claim_verb) && l.worktree_path.as_os_str().is_empty() {
        let alive = l
            .creator_pid
            .map(|pid| {
                process_probe::process_identity_is_alive(pid, l.creator_pid_start_time.as_deref())
            })
            .unwrap_or(false);
        return if alive {
            LeaseState::Live
        } else {
            LeaseState::Stale
        };
    }
    if let Some(pid) = l.active_pid {
        return if process_probe::process_identity_is_alive(pid, l.active_pid_start_time.as_deref())
        {
            LeaseState::Live
        } else {
            LeaseState::Stale
        };
    }
    let worktree_exists = l.worktree_path.exists();
    let has_live_claude = live_sessions
        .iter()
        .any(|s| !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path)));
    let age_hours = now.signed_duration_since(l.started_at).num_hours();
    classify_lease_state(worktree_exists, has_live_claude, age_hours)
}

/// BUG-98: cheap lease count for the `aida session list` footer hint.
/// Users repeatedly reach for `session list` looking for "active
/// sessions" when they really want `session leases` (the scoped-lease
/// view). Exposing the count here lets the historical-view command
/// nudge them at the right moment. Tries the current project root first
/// and silently falls through to 0 if we're not inside a project.
/// trace:BUG-98 | ai:claude
pub(crate) fn active_lease_count_for_cwd() -> usize {
    match find_project_root() {
        Ok(root) => list_leases(&root).len(),
        Err(_) => 0,
    }
}

/// Find the active session lease whose worktree contains `cwd`, if any.
/// Used by statusline + enforcement to identify which session "owns" the
/// shell the user is operating from.
/// trace:STORY-55 | ai:claude
pub(crate) fn active_lease_for_cwd(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<SessionLease> {
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    list_leases(project_root)
        .into_iter()
        .find(|l| lease_covers_cwd(l, &canon))
}

/// Whether `lease` covers `canon_cwd` — true iff the lease records a real
/// worktree path AND `canon_cwd` is exactly that path or a descendant of it.
///
/// TASK-474: leases with an empty `worktree_path` are advisory locks with
/// no session context (e.g. an MCP `claim_task` lease that didn't pass a
/// `worktree_path` argument — see `aida-cli/src/mcp.rs::tool_claim_task`).
/// `Path::starts_with` treats every path as starting with the empty path,
/// so an empty-worktree lease would otherwise match every cwd and misroute
/// "this session owns scope X" hints in `aida add` to unrelated shells.
/// trace:TASK-474 | ai:claude
pub(crate) fn lease_covers_cwd(lease: &SessionLease, canon_cwd: &std::path::Path) -> bool {
    if lease.worktree_path.as_os_str().is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let lease_key = windows_path_key(&lease.worktree_path.to_string_lossy());
        let cwd_key = windows_path_key(&canon_cwd.to_string_lossy());
        return cwd_key == lease_key
            || cwd_key
                .strip_prefix(&lease_key)
                .is_some_and(|tail| tail.starts_with('/'));
    }
    #[cfg(not(windows))]
    {
        canon_cwd == lease.worktree_path || canon_cwd.starts_with(&lease.worktree_path)
    }
}

/// Normalize Windows spellings used by `Path::display()` and `canonicalize()`
/// before comparing a recorded lease path with the current worktree path.
/// The casing fold is Unicode-aware (`str::to_lowercase`, not
/// `to_ascii_lowercase`) because NTFS path matching is case-insensitive for
/// non-ASCII names too — a lease recorded under `Åsa` must match a cwd
/// canonicalized as `åsa`.
// trace:BUG-1794 | ai:codex
// trace:BUG-1794 | ai:claude
#[cfg(windows)]
pub(crate) fn windows_path_key(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let path = path
        .strip_prefix("//?/unc/")
        .map(|unc| format!("//{unc}"))
        .or_else(|| path.strip_prefix("//?/").map(str::to_string))
        .unwrap_or(path);
    path.trim_end_matches('/').to_string()
}

#[cfg(test)]
#[path = "tests/lease_covers_cwd_tests.rs"]
mod lease_covers_cwd_tests;

/// STORY-58: from inside a session worktree, return the parent project's
/// root path so cross-worktree views (currently just `aida session list`)
/// can also surface sessions launched from the parent. `None` when:
///   - cwd isn't covered by an active lease (not in a session), OR
///   - the lease was written before STORY-58 (no parent recorded), OR
///   - we can't locate any project root to read leases from.
///     trace:STORY-58 | ai:claude
pub(crate) fn parent_project_root_for_session(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    // The lease dir is `<root>/.aida/sessions/`. Inside a session worktree
    // that dir is a symlink back to the parent project's, so `list_leases`
    // returns the same set either way. Walk up from cwd looking for any
    // ancestor that has `.aida/sessions/`, then ask for its lease set.
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut probe = canon.as_path();
    loop {
        // BUG-1598: never adopt a temp root itself as the project root —
        // see `aida_core::store_locate` for the shared rationale.
        // trace:BUG-1598 | ai:claude
        if aida_core::store_locate::is_system_temp_dir(probe) {
            return None;
        }
        if probe.join(".aida").join("sessions").is_dir() {
            let lease = active_lease_for_cwd(probe, &canon)?;
            return lease.parent_project_root;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return None,
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionActivityLog {
    #[serde(default)]
    pub(crate) entries: Vec<SessionActivityEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionActivityEntry {
    /// Role active when this entry was recorded — needed at session-end to
    /// know which project-level role file to fold the entry back into.
    pub(crate) role: String,
    pub(crate) spec_id: String,
    pub(crate) action: String,
    pub(crate) at: chrono::DateTime<chrono::Utc>,
}

/// Hard cap on session activity entries — keeps the file small for long
/// sessions. Older-than-cap entries fall off the back (so the newest cap
/// entries are kept, matching the project-level role's stream behavior).
pub(crate) const SESSION_ACTIVITY_MAX: usize = 200;

pub(crate) fn session_activity_path(
    project_root: &std::path::Path,
    id: &str,
) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.activity.toml", id))
}

pub(crate) fn load_session_activity(
    project_root: &std::path::Path,
    id: &str,
) -> SessionActivityLog {
    let path = session_activity_path(project_root, id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionActivityLog::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub(crate) fn save_session_activity(
    project_root: &std::path::Path,
    id: &str,
    log: &SessionActivityLog,
) -> Result<()> {
    let path = session_activity_path(project_root, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(log)?;
    // BUG-228: atomic write — guards the session activity log against the
    // same torn-concurrent-write corruption as the project-level role file.
    write_atomic(&path, &content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Append an activity entry to the given session's log. Dedupes against
/// the most recent entry the same way project-level activity does:
/// consecutive (role, spec_id, action) collapse into one entry whose
/// timestamp ticks forward.
/// trace:STORY-56 | ai:claude
pub(crate) fn append_session_activity(
    project_root: &std::path::Path,
    session_id: &str,
    role: &str,
    spec_id: &str,
    action: &str,
) -> Result<()> {
    let mut log = load_session_activity(project_root, session_id);
    let entry = SessionActivityEntry {
        role: role.to_string(),
        spec_id: spec_id.to_string(),
        action: action.to_string(),
        at: chrono::Utc::now(),
    };
    // BUG-65: LRU-by-(role, spec_id, action). Drop any prior entry with
    // the same key, then insert at the front — interleaved actions across
    // specs no longer accumulate stale duplicates.
    // trace:BUG-65 | ai:claude
    log.entries.retain(|prev| {
        !(prev.role == entry.role && prev.spec_id == entry.spec_id && prev.action == entry.action)
    });
    log.entries.insert(0, entry);
    log.entries.truncate(SESSION_ACTIVITY_MAX);
    save_session_activity(project_root, session_id, &log)
}

/// BUG-112: the most-recent spec a session lease touched, read from its
/// session activity log. `entries` are newest-first, so the head entry's
/// `spec_id` is the session's current focus. `None` when the log is empty
/// or absent. Used by `aida session list`'s RECENT FOCUS column.
/// trace:BUG-112 | ai:claude
pub(crate) fn session_log_recent_spec(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Option<String> {
    load_session_activity(project_root, lease_id)
        .entries
        .into_iter()
        .next()
        .map(|e| e.spec_id)
}

/// STORY-57: queue routing filter. Returns true if `entry`'s scope/session
/// routing tags are compatible with the consumer side (the shell calling
/// `queue list` / `queue next`).
///
/// Rules:
///   - `for_scope` set on the entry must match the consumer's lease scope
///     (case-insensitive). No lease + scope-tagged entry → filtered out
///     (the entry is targeted at a session that isn't this shell).
///   - `for_session` set on the entry must match the consumer lease's id
///     (8+ char prefix on either side, since a user might have typed a
///     short prefix).
///   - Either field absent on the entry = unrouted on that axis = visible
///     to all consumers.
///
/// Bypass with `bypass=true` (used for `--all` / explicit `--scope any`).
/// trace:STORY-57 | ai:claude
/// TASK-44: walk `req`'s relationships for a Child edge whose target is
/// an Epic-typed requirement and return that Epic's display id
/// (`agreed_id` when merged-trunk-canonical, else `spec_id`). A `Child`
/// relationship semantically means "this req IS a child of target" —
/// the inverse of `Parent`. Used by `aida queue list` to auto-derive
/// the `[@EPIC-XX*]` cluster chip for entries that weren't explicitly
/// queued with `--scope`. Returns None when:
///   - the req has no Child relationship, OR
///   - the target id doesn't resolve to a requirement in `store`, OR
///   - the target's type isn't Epic.
///     Pure — no I/O — so the decision rules can be unit-tested without
///     fixtures. trace:TASK-44 | ai:claude
///     TASK-91: detect a synthetic review-story title `Review PR-<n>:` filed
///     by STORY-90's auto-queue and return a display tuple where PR-N is the
///     prominent id and the canonical STORY-NNN is the parenthetical
///     secondary. Returns `None` when the title doesn't match the pattern;
///     callers fall back to the unchanged (display_id, title) pair.
///
/// Rendering convention (Option A from TASK-91):
///   STORY-108  Review PR-15: EPIC-23 wrap-up      →  display_id stays STORY-108
///                                                    title stays "Review PR-15: EPIC-23 wrap-up"
///   becomes:
///   PR-15 (STORY-108)  EPIC-23 wrap-up            →  prominent PR-N, dual id, trimmed title
///
/// PR-N is the conceptual handle the user reaches for ("I need to review
/// PR-15"); STORY-NNN stays visible for direct `aida edit` operations.
/// trace:TASK-91 | ai:claude
pub(crate) fn format_review_story_display(
    display_id: &str,
    title: &str,
) -> Option<(String, String)> {
    // BUG-91: match the `Review PR-` prefix case-insensitively so a
    // hand-typed `review pr-15: foo` renders consistently with how
    // `review_title_matches` (also case-insensitive) routes it. Slice the
    // ORIGINAL title (not a lowercased copy) so the after-colon title keeps
    // its case.
    const PREFIX: &str = "Review PR-";
    // BUG-525: `str::get(..n)` returns None when `n` is out of bounds OR not a
    // char boundary, so a non-ASCII title (e.g. an em-dash straddling byte 10)
    // no longer panics the way a raw `title[..PREFIX.len()]` byte-slice did.
    let Some(head) = title.get(..PREFIX.len()) else {
        return None;
    };
    if !head.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let rest = &title[PREFIX.len()..];
    let (pr_n, after) = rest.split_once(':')?;
    if pr_n.is_empty() || !pr_n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let combined = format!("PR-{} ({})", pr_n, display_id);
    let trimmed = after.trim_start().to_string();
    Some((combined, trimmed))
}

pub(crate) fn derive_parent_epic_label(
    req: &aida_core::Requirement,
    store: &RequirementsStore,
) -> Option<String> {
    req.relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Child)
        .find_map(|r| {
            let parent = store
                .requirements
                .iter()
                .find(|p| p.id == r.target_id && p.req_type == RequirementType::Epic)?;
            parent
                .agreed_id
                .as_deref()
                .or(parent.spec_id.as_deref())
                .map(String::from)
        })
}

/// Render the header line for a `--tree` group (`aida list --tree`).
///
/// A real parent-backed group (`parent` resolves to the requirement the label
/// groups on) carries the parent's own title + status badge at the parent
/// level, with the child count trailing as metadata — so a real scoped parent
/// reads as a requirement, not as a synthetic count-only bucket. The synthetic
/// `Unscoped` bucket and an unresolvable label both fall back to the plain
/// `<label> (N items)` form, keeping synthetic buckets distinguishable from
/// real parents.
// trace:TASK-0439 | ai:claude
pub(crate) fn tree_group_header(
    key: &str,
    unscoped_key: &str,
    count: usize,
    parent: Option<&aida_core::Requirement>,
    store: &aida_core::RequirementsStore,
) -> String {
    let plural = if count == 1 { "" } else { "s" };
    if key == unscoped_key {
        format!("{} ({} item{})", "Unscoped".cyan().bold(), count, plural)
    } else if let Some(parent) = parent {
        // BUG-658: an EPIC's status is a read-only rollup of its children
        // (BUG-626) — never the raw stored YAML status, which can drift in both
        // directions (a childless epic reading In Progress, an actively-shipping
        // epic reading Draft). The tree header used `parent.status` directly, so
        // the same epic could read one status as a group HEADER here and another
        // as a row / via `aida show` / `aida why`. Route through
        // `effective_display_status` — the same `derive_epic_status` rollup every
        // other surface uses — so the header agrees with the rest of the view.
        // trace:BUG-658 | ai:claude
        let effective = effective_display_status(store, parent);
        format!(
            "{}  {}  [{}]  ({} item{})",
            key.cyan().bold(),
            parent.title,
            // cache_key() ("InProgress") matches the child rows' status form
            // (summary.status), so both spell the same status identically in
            // one view rather than mixing "InProgress" / "In Progress".
            status_display::status_badge(effective.cache_key()),
            count,
            plural,
        )
    } else {
        format!("{} ({} item{})", key.cyan().bold(), count, plural)
    }
}

pub(crate) fn entry_scope_session_match(
    entry: &aida_core::QueueEntry,
    self_lease: Option<&SessionLease>,
    bypass: bool,
) -> bool {
    if bypass {
        return true;
    }
    // BUG-89: When the viewer has no active lease, do NOT apply the implicit
    // scope/session filter. The implicit "match my scope/session" filter
    // only makes sense WHEN you're in a session — without one, the user is
    // asking from the global vantage point (e.g., the project root), so all
    // routed items should be visible. STORY-57's filter is opt-in by being
    // inside a scoped session, not opt-out by sitting outside one. Users
    // who want strict scope filtering from outside can pass `--scope X`
    // explicitly. trace:BUG-89 | ai:claude
    let Some(lease) = self_lease else {
        return true;
    };
    if let Some(want_scope) = entry.for_scope.as_deref() {
        if !lease.scope.eq_ignore_ascii_case(want_scope) {
            return false;
        }
    }
    if let Some(want_sess) = entry.for_session.as_deref() {
        let n = want_sess.len().min(lease.id.len());
        if n < 4 {
            // Too short to be safely matched — bail out as no-match
            // rather than risk a false positive.
            return false;
        }
        if !lease.id[..n].eq_ignore_ascii_case(&want_sess[..n]) {
            return false;
        }
    }
    true
}

/// Lease-enforcement mode for cross-session writes.
/// Configured via `[session] enforcement = "warn"|"block"|"off"` in
/// `.aida/config.toml`; default is `Warn`.
/// trace:STORY-48 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEnforcement {
    Off,
    Warn,
    Block,
}

impl SessionEnforcement {
    pub(crate) fn from_config_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "none" => SessionEnforcement::Off,
            "block" | "strict" => SessionEnforcement::Block,
            _ => SessionEnforcement::Warn,
        }
    }
}

/// Read `[session].enforcement` from `<project_root>/.aida/config.toml`.
/// Falls back to `Warn` on any parse/IO failure — enforcement should
/// never break the host command.
/// trace:STORY-48 | ai:claude
pub(crate) fn session_enforcement(project_root: &std::path::Path) -> SessionEnforcement {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionEnforcement::Warn;
    };
    let Ok(parsed) = content.parse::<toml::Value>() else {
        return SessionEnforcement::Warn;
    };
    parsed
        .get("session")
        .and_then(|s| s.get("enforcement"))
        .and_then(|v| v.as_str())
        .map(SessionEnforcement::from_config_str)
        .unwrap_or(SessionEnforcement::Warn)
}

/// Returns the lease that "owns" `target_uuid` — i.e. the spec is itself
/// the scope, OR is a descendant of the scope's spec via Parent
/// relationships. Excludes `self_lease` (the caller's own session) so a
/// session can freely edit specs in its own scope. Returns `None` for
/// path-glob / free-form scopes that we can't resolve to a spec id.
/// trace:STORY-48 | ai:claude
/// Build a minimal store containing the target plus its ancestor chain
/// (following `Child` edges — the climb-toward-root direction), resolved via
/// TARGETED per-uuid lookups rather than a full-store `backend.load()` (a scan
/// of every YAML object on the edit write path). `lease_owning_spec` only walks
/// `Child` edges from the target and reads the spec_id/agreed_id of the
/// reachable ancestors, so a store holding exactly that closure is behaviorally
/// identical to the full store for lease enforcement. The targeted
/// `get_requirement` is cache-backed; a missing ancestor is simply skipped (an
/// absent ancestor can't own a lease scope anyway).
// trace:BUG-634 | ai:claude
pub(crate) fn collect_ancestor_store(
    backend: &aida_core::CachedGitBackend,
    target: &Requirement,
) -> RequirementsStore {
    let mut visited: HashSet<Uuid> = HashSet::new();
    visited.insert(target.id);
    let mut requirements: Vec<Requirement> = vec![target.clone()];
    let mut frontier: Vec<Requirement> = vec![target.clone()];
    while let Some(curr) = frontier.pop() {
        for rel in &curr.relationships {
            if rel.rel_type == RelationshipType::Child && visited.insert(rel.target_id) {
                if let Ok(Some(parent)) = backend.get_requirement(&rel.target_id) {
                    requirements.push(parent.clone());
                    frontier.push(parent);
                }
            }
        }
    }
    RequirementsStore {
        requirements,
        ..Default::default()
    }
}

/// BUG-637: a closure that reports whether a session lease is currently LIVE,
/// reusing STORY-694's [`lease_state_for`] (pid liveness + worktree existence +
/// age, with the review-verb special-case). Built once over a pre-probed live
/// session set so the caller pays for the (relatively expensive) process probe a
/// single time and the per-lease check is cheap. A spec-scoped CLAIM only gates
/// work while its holder is live — a dead/stale claim is ignored by every gate
/// (no crash-deadlock).
// trace:BUG-637 | ai:claude
// BUG-1205: what to do when a PR's source branch is held by another lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseConflictDecision {
    /// A human is at the keyboard — offer the four options.
    Prompt,
    /// Headless and the lease's session is gone — end it and continue.
    AutoEnd,
    /// Headless and the lease is LIVE — refuse with a typed message.
    Refuse,
    /// BUG-2150: the holder is not a reviewer (the implementer working the
    /// PR). A review checks out its own `pr-N` branch from the forge's PR
    /// head ref, so it coexists with that lease — no prompt, no refusal, and
    /// never ending the implementer's session.
    Coexist,
}

// BUG-1205: pure decision for the lease-held pre-flight in `session_start`.
// trace:BUG-1205 | ai:claude
pub(crate) fn lease_conflict_decision(headless: bool, lease_live: bool) -> LeaseConflictDecision {
    match (headless, lease_live) {
        (false, _) => LeaseConflictDecision::Prompt,
        (true, false) => LeaseConflictDecision::AutoEnd,
        (true, true) => LeaseConflictDecision::Refuse,
    }
}

/// BUG-2150: the decision for a review whose PR source branch is held by
/// another lease. A review lease is a different kind from an implementer
/// claim, so an explicit reviewer pickup (`pickup_role` = reviewer) coexists
/// with a LIVE non-reviewer holder. Every other case — a non-reviewer pickup,
/// a holder that is itself a reviewer, or a dead holder (ended as before) —
/// keeps the BUG-1205 prompt / auto-end / refuse handling.
// trace:BUG-2150 | ai:claude
pub(crate) fn review_source_branch_conflict_decision(
    headless: bool,
    lease_live: bool,
    holder_role: Option<&str>,
    pickup_role: Option<&str>,
) -> LeaseConflictDecision {
    let is_reviewer = |r: Option<&str>| {
        r.is_some_and(|r| crate::canonical_role_name(r.trim()).eq_ignore_ascii_case("reviewer"))
    };
    if lease_live && is_reviewer(pickup_role) && !is_reviewer(holder_role) {
        LeaseConflictDecision::Coexist
    } else {
        lease_conflict_decision(headless, lease_live)
    }
}

pub(crate) fn lease_is_live<'a>(
    live_sessions: &'a [process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> impl Fn(&SessionLease) -> bool + 'a {
    move |l: &SessionLease| matches!(lease_state_for(l, live_sessions, now), LeaseState::Live)
}

/// BUG-637: the pre-pickup claim gate. Returns the LIVE spec-scoped lease held by
/// a DIFFERENT session for any of `spec_ids`, if one exists — the signal that
/// another agent is already working this spec and a fresh pickup would duplicate
/// it (the BUG-634 incident). Pure over the lease set + an injected liveness
/// predicate so it is unit-testable without a live process or a lease dir:
///
/// - skips the caller's own lease (`self_lease`) — resuming your own work is fine;
/// - skips non-live (crashed / stale / dormant) claims — no crash-deadlock;
/// - matches a lease whose raw `--owns` scope equals one of `spec_ids`
///   (case-insensitive), i.e. the AIDA-launched spec-scoped happy path. Generic
///   `harness-worktree` advisor fan-out scopes never match, so they don't gate
///   (making THOSE spec-claimable is the documented follow-on).
// trace:BUG-637 | ai:claude
pub(crate) fn live_spec_claim_by_other<'a>(
    leases: &'a [SessionLease],
    self_lease: Option<&SessionLease>,
    spec_ids: &[&str],
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<&'a SessionLease> {
    leases.iter().find(|l| {
        if let Some(self_l) = self_lease {
            if l.id == self_l.id {
                return false;
            }
        }
        if !is_live(l) {
            return false;
        }
        spec_ids
            .iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
    })
}

/// What the requeue lease gate found for a spec.
// trace:STORY-1429 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequeueLeaseCheck {
    /// No other session holds a live lease on the spec.
    Clear,
    /// Another session holds a live lease: requeueing would reset work a
    /// running session owns.
    HeldByOther {
        session: String,
        started: String,
        worktree: String,
    },
    /// The lease state could not be established. Missing evidence is not good
    /// evidence, so this refuses exactly like a live lease.
    Unknown(String),
}

impl RequeueLeaseCheck {
    /// The refusal text for `id`, or `None` when the gate is clear. It names
    /// the holder and the command that releases it. `--force` does not
    /// override it.
    pub(crate) fn refusal(&self, id: &str) -> Option<String> {
        match self {
            RequeueLeaseCheck::Clear => None,
            RequeueLeaseCheck::HeldByOther {
                session,
                started,
                worktree,
            } => Some(format!(
                "{id} is claimed by live session {session} (started {started}, worktree \
                 {worktree}); requeueing it would reset work that session owns. If that \
                 session is abandoned, end it with `aida session end {session}` and requeue \
                 again. --force does not override a live claim."
            )),
            RequeueLeaseCheck::Unknown(why) => Some(format!(
                "cannot tell whether another session is working on {id} ({why}); refusing to \
                 requeue it. Fix the session leases (`aida session leases`) and requeue again."
            )),
        }
    }
}

/// Whether `name` is a lease file the requeue gate reads: a session lease
/// `<id>.toml` (no dot in the stem, the same rule `lease_ids_in` (BUG-114)
/// uses), or an MCP `claim_task` claim `mcp-claim.<spec>.toml`. That excludes
/// the `<id>.activity.toml` / `<id>.manifest.toml` companions and the
/// `write_atomic` staging files. An MCP claim parses as a `SessionLease` and
/// the BUG-637 pickup gate evaluates it, so the requeue gate does too: a claim
/// with a real worktree and a live session is Live and refuses, while a dead or
/// worktree-less claim comes out Stale through the same liveness check.
// trace:STORY-1429 | ai:claude
pub(crate) fn is_session_lease_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".toml") else {
        return false;
    };
    let stem = stem.strip_prefix("mcp-claim.").unwrap_or(stem);
    !stem.is_empty() && !stem.contains('.')
}

/// What one lease file told the requeue gate.
// trace:STORY-1429 | ai:claude
#[derive(Debug)]
pub(crate) enum LeaseFileRead {
    Lease(Box<SessionLease>),
    /// Unparseable, but it visibly names a different scope: cannot be a
    /// claim on the spec being requeued.
    OtherScope,
    /// Unreadable or unparseable, and it could name the spec.
    Unknown(String),
}

/// Read one lease file for the requeue gate. A read or parse failure is
/// retried once (a writer may be mid-write); a file still unparseable after
/// the retry is `Unknown` only when it could name one of `spec_ids`: its text
/// mentions one, or no `scope` is visible in it at all.
// trace:STORY-1429 | ai:claude
pub(crate) fn read_lease_file_for_gate(
    label: &str,
    read: impl Fn() -> std::io::Result<String>,
    spec_ids: &[&str],
) -> LeaseFileRead {
    let attempt = || -> Result<SessionLease, (Option<String>, String)> {
        let content =
            read().map_err(|e| (None, format!("the lease {label} is unreadable: {e}")))?;
        toml::from_str::<SessionLease>(&content).map_err(|e| {
            (
                Some(content),
                format!("the lease {label} does not parse: {e}"),
            )
        })
    };
    let (content, why) = match attempt() {
        Ok(lease) => return LeaseFileRead::Lease(Box::new(lease)),
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            match attempt() {
                Ok(lease) => return LeaseFileRead::Lease(Box::new(lease)),
                Err(e) => e,
            }
        }
    };
    let Some(content) = content else {
        return LeaseFileRead::Unknown(why);
    };
    let lower = content.to_ascii_lowercase();
    let mentions = spec_ids
        .iter()
        .any(|id| !id.is_empty() && lower.contains(&id.to_ascii_lowercase()));
    if mentions {
        return LeaseFileRead::Unknown(why);
    }
    // Only a COMPLETE quoted scope counts: a value cut off mid-write could
    // still be the spec's id.
    let names_a_scope = content.lines().any(|l| {
        let l = l.trim_start();
        l.strip_prefix("scope")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::trim)
            .is_some_and(|v| v.len() > 2 && v.starts_with('"') && v.ends_with('"'))
    });
    if names_a_scope {
        LeaseFileRead::OtherScope
    } else {
        LeaseFileRead::Unknown(why)
    }
}

/// Lease listing for the requeue gate. Reads only real lease files (see
/// [`is_session_lease_file`]). An unreadable lease directory is an error; a
/// missing one is a definite "no leases". A lease that stays unparseable after
/// one retry is an error only when it could name one of `spec_ids`.
// trace:STORY-1429 | ai:claude
pub(crate) fn list_leases_strict(
    project_root: &std::path::Path,
    spec_ids: &[&str],
) -> Result<Vec<SessionLease>, String> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| format!("the lease directory {} is unreadable: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_session_lease_file(name) {
            continue;
        }
        let label = p.display().to_string();
        match read_lease_file_for_gate(&label, || std::fs::read_to_string(&p), spec_ids) {
            LeaseFileRead::Lease(lease) => out.push(*lease),
            LeaseFileRead::OtherScope => {}
            LeaseFileRead::Unknown(why) => {
                // A lease removed between the listing and the read is gone,
                // not unknown.
                if !p.exists() {
                    continue;
                }
                return Err(why);
            }
        }
    }
    out.sort_by_key(|l| l.started_at);
    Ok(out)
}

/// The requeue lease gate, pure over a lease listing and a FALLIBLE liveness
/// probe. The BUG-637 predicate cannot say "unknown", so the fail-closed
/// handling lives here: a listing error, or a probe error on a lease that
/// claims one of `spec_ids`, is [`RequeueLeaseCheck::Unknown`]. Dead or stale
/// claims do not block (pickup reclaims them). The caller's own lease never
/// blocks. Only leases in this clone are visible.
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_gate_from(
    listed: Result<Vec<SessionLease>, String>,
    self_lease: Option<&SessionLease>,
    spec_ids: &[&str],
    is_live: impl Fn(&SessionLease) -> Result<bool, String>,
) -> RequeueLeaseCheck {
    let leases = match listed {
        Ok(leases) => leases,
        Err(why) => return RequeueLeaseCheck::Unknown(why),
    };
    for l in &leases {
        if self_lease.is_some_and(|s| s.id == l.id) {
            continue;
        }
        if !spec_ids
            .iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
        {
            continue;
        }
        match is_live(l) {
            Ok(true) => {
                return RequeueLeaseCheck::HeldByOther {
                    session: l.id.chars().take(8).collect(),
                    started: l
                        .started_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string(),
                    worktree: l.worktree_path.display().to_string(),
                }
            }
            Ok(false) => {}
            Err(why) => {
                return RequeueLeaseCheck::Unknown(format!(
                    "the liveness of session {} could not be checked: {why}",
                    l.id.chars().take(8).collect::<String>()
                ))
            }
        }
    }
    RequeueLeaseCheck::Clear
}

/// Production liveness for the requeue gate. On Linux the process table is
/// the evidence; when `/proc` cannot be read, liveness is unknown rather than
/// "dead".
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_liveness(
    l: &SessionLease,
    live_sessions: &std::cell::OnceCell<Vec<process_probe::LiveSession>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    if std::fs::read_dir("/proc").is_err() {
        return Err("the process table (/proc) is not readable".to_string());
    }
    let live = live_sessions.get_or_init(process_probe::probe_live_claude_sessions);
    Ok(matches!(lease_state_for(l, live, now), LeaseState::Live))
}

/// Run the requeue lease gate for `spec_ids` against the leases under
/// `project_root`.
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_gate(
    project_root: &std::path::Path,
    spec_ids: &[&str],
) -> RequeueLeaseCheck {
    let listed = list_leases_strict(project_root, spec_ids);
    let self_lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    let now = chrono::Utc::now();
    let live = std::cell::OnceCell::new();
    requeue_lease_gate_from(listed, self_lease.as_ref(), spec_ids, |l| {
        requeue_lease_liveness(l, &live, now)
    })
}

pub(crate) fn lease_owning_spec(
    leases: &[SessionLease],
    self_lease: Option<&SessionLease>,
    target_uuid: Uuid,
    target_spec_id: Option<&str>,
    store: &RequirementsStore,
    // BUG-637: liveness predicate. A spec-scoped claim only blocks an outbound
    // mutation while its holder is genuinely LIVE — a crashed/stale agent's
    // lease must NOT permanently lock its spec (no crash-deadlock). Callers
    // pass `lease_is_live(&live_sessions, now)`; the unit tests (which exercise
    // ancestry/scope matching, not liveness) pass `|_| true`.
    // trace:BUG-637 | ai:claude
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<SessionLease> {
    if leases.is_empty() {
        return None;
    }

    // Walk to collect ancestors (incl. target itself). AIDA stores
    // hierarchy with `rel_type: Child` on the descendant pointing at the
    // ancestor (display: "X is child of Y"), so the climb-toward-root edge
    // is `Child`, NOT `Parent`. Visit each uuid at most once; pathological
    // cycles are bounded by the visited set.
    let mut ancestors: HashSet<Uuid> = HashSet::new();
    ancestors.insert(target_uuid);
    let mut frontier = vec![target_uuid];
    while let Some(curr) = frontier.pop() {
        if let Some(req) = store.requirements.iter().find(|r| r.id == curr) {
            for rel in &req.relationships {
                if rel.rel_type == RelationshipType::Child && ancestors.insert(rel.target_id) {
                    frontier.push(rel.target_id);
                }
            }
        }
    }

    // Pre-compute the lower-cased spec_ids / agreed_ids of the ancestor
    // set for fast string-match against scope strings.
    let ancestor_ids: HashSet<String> = ancestors
        .iter()
        .filter_map(|id| store.requirements.iter().find(|r| r.id == *id))
        .flat_map(|r| {
            [r.spec_id.as_deref(), r.agreed_id.as_deref()]
                .into_iter()
                .flatten()
                .map(|s| s.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();

    for lease in leases {
        if let Some(self_l) = self_lease {
            if lease.id == self_l.id {
                continue;
            }
        }
        // BUG-637: a dead/stale claim doesn't own the scope anymore — skip it so a
        // crashed agent can't permanently block edits to its spec.
        // trace:BUG-637 | ai:claude
        if !is_live(lease) {
            continue;
        }
        let scope_lc = lease.scope.to_ascii_lowercase();
        // Direct id-form match (handles SPEC-ID + agreed-id forms).
        if ancestor_ids.contains(&scope_lc) {
            return Some(lease.clone());
        }
        // The provided target_spec_id might equal the scope verbatim even
        // when ancestor walking doesn't pick it up (e.g. agreed_id stripped).
        if let Some(tid) = target_spec_id {
            if tid.eq_ignore_ascii_case(&lease.scope) {
                return Some(lease.clone());
            }
        }
        // Path globs / free-form tags: no ancestry to walk → don't enforce.
    }
    None
}

/// Enforce a session lease for an outbound mutation on `target`. Returns
/// `Err` only when the active enforcement mode is `Block` and another
/// session owns the scope; in `Warn` mode it prints a warning and returns
/// `Ok(())` so the operation proceeds. `force_block` (e.g. `--strict` on
/// `aida edit`) escalates Warn → Block for this single call.
///
/// BUG-637: `force_skip` (e.g. `--force` on `aida edit`) is the operator
/// override — it short-circuits the whole gate so a deliberate
/// edit/reject of a live-claimed spec is always possible. The claim check is
/// now LIVENESS-aware ([`lease_is_live`]): a dead/stale claim no longer owns the
/// scope, so a crashed agent can't permanently block edits to its spec (the
/// reject-while-working fix without the crash-deadlock).
// trace:STORY-48 trace:BUG-637 | ai:claude
pub(crate) fn enforce_session_lease(
    project_root: &std::path::Path,
    target: &Requirement,
    store: &RequirementsStore,
    operation: &str,
    force_block: bool,
    force_skip: bool,
) -> Result<()> {
    // BUG-637: operator override — `--force` always wins, no probe needed.
    if force_skip {
        return Ok(());
    }
    let leases = list_leases(project_root);
    if leases.is_empty() {
        return Ok(());
    }
    let self_lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    // BUG-637: probe live sessions once, then gate the claim on liveness.
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let owner = lease_owning_spec(
        &leases,
        self_lease.as_ref(),
        target.id,
        target.spec_id.as_deref(),
        store,
        lease_is_live(&live, now),
    );
    let Some(owner) = owner else { return Ok(()) };

    let mut mode = session_enforcement(project_root);
    if force_block {
        mode = SessionEnforcement::Block;
    }
    let target_label = target.spec_id.as_deref().unwrap_or("?");
    let owner_id_short: String = owner.id.chars().take(8).collect();
    match mode {
        SessionEnforcement::Off => Ok(()),
        SessionEnforcement::Warn => {
            eprintln!(
                "{} {} on {} touches scope owned by session {} ({})",
                "Warning:".yellow().bold(),
                operation,
                target_label.cyan(),
                owner_id_short.yellow(),
                owner.scope.cyan(),
            );
            eprintln!("  worktree: {}", owner.worktree_path.display());
            eprintln!(
                "  ({} or set `[session] enforcement = \"off\"` in .aida/config.toml to silence)",
                "pass --strict to convert into a hard block".dimmed()
            );
            Ok(())
        }
        SessionEnforcement::Block => {
            anyhow::bail!(
                "{} on {} blocked: scope owned by session {} ({}, worktree: {}). \
                 End that session first or set `[session] enforcement = \"warn\"` to downgrade.",
                operation,
                target_label,
                owner_id_short,
                owner.scope,
                owner.worktree_path.display()
            );
        }
    }
}

/// BUG-677: `LeaseState` + `classify_lease_state` moved to `aida_core::liveness`
/// (re-exported at the crate root, see the top-of-file `pub(crate) use`). The
/// pure enum + classifier is now shared with `aida-tui`; only the CLI glyph
/// rendering stays here, as an extension trait (glyph() reaches into the CLI
/// glyph palette, which aida-core has no business knowing). `label()` moved with
/// the enum (pure strings).
// trace:BUG-677 | ai:claude
pub(crate) trait LeaseStateGlyph {
    fn glyph(self) -> &'static str;
}

impl LeaseStateGlyph for LeaseState {
    fn glyph(self) -> &'static str {
        match self {
            LeaseState::Live => "●",
            LeaseState::Dormant => crate::glyph(crate::glyphs::Glyph::InFlight),
            LeaseState::Stale => crate::glyph(crate::glyphs::Glyph::Warning),
        }
    }
}

/// TASK-56: find the Claude Code session id matching a worktree path,
/// derived from the newest `*.jsonl` file at
/// `~/.claude/projects/<encoded-cwd>/`. Returns None when the project
/// dir doesn't exist or has no jsonl files. trace:TASK-56 | ai:claude
pub(crate) fn cc_session_id_for_worktree(worktree: &std::path::Path) -> Option<String> {
    let dir = session::claude_project_dir(worktree).ok()?;
    if !dir.exists() {
        return None;
    }
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        match &newest {
            Some((best_mtime, _)) if mtime <= *best_mtime => {}
            _ => newest = Some((mtime, stem)),
        }
    }
    newest.map(|(_, s)| s)
}

/// STORY-637: render the cross-clone lease section — claims on the shared
/// `aida-store` registry held by OTHER clones (a different clone path than
/// ours). Returns the number of foreign claims shown. Distinct from the
/// local-lease table above it. trace:STORY-637 | ai:claude
///
/// BUG-1764: stale claims are now EXCLUDED by default. This section applied no
/// expiry predicate of any kind — its only filter was `clone_path != ours`, and
/// while it computed `age` from `heartbeat_at` for the display column it never
/// used it as a predicate, so a foreign claim rendered as active forever once
/// written. `show_all` (`--all`) brings them back with the staleness reason,
/// mirroring the contract the LOCAL lease table above already has for `--all`.
// trace:BUG-1764 | ai:claude
pub(crate) fn print_cross_clone_leases(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    show_all: bool,
) -> usize {
    let store_root = project_root.join(".aida-store");
    // Two clones sharing one store inherit the same node id, so discriminate
    // "ours" vs "foreign" by the canonical clone path (the project root).
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    // BUG-1764: split before rendering. `partition_claims` is keyed on `host`,
    // not `clone_path`, so a foreign-CLONE claim on THIS host is evaluated
    // against the local process table rather than excused as unreachable.
    let (live, stale) = coordination::partition_claims(foreign, now, &coordination::hostname());
    let shown: Vec<(&coordination::Claim, Option<&str>)> = if show_all {
        live.iter()
            .map(|c| (c, None))
            .chain(stale.iter().map(|(c, r)| (c, Some(r.as_str()))))
            .collect()
    } else {
        live.iter().map(|c| (c, None)).collect()
    };
    if shown.is_empty() {
        // Nothing live, but say so rather than leaving the operator to wonder
        // why a lease they can see on disk never appears.
        if !stale.is_empty() {
            println!();
            println!(
                "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
                "·".dimmed(),
                stale.len(),
                if stale.len() == 1 { "" } else { "s" },
                "aida session leases --all".cyan(),
                "aida session leases --prune-stale".cyan()
            );
        }
        return 0;
    }
    println!();
    println!("{}", "Cross-clone leases (other clones)".bold());
    println!();
    if show_all {
        println!(
            "{:<20} {:<10} {:<14} {:<14} {:<10} state",
            "scope", "host", "node", "agent", "age"
        );
    } else {
        println!(
            "{:<20} {:<10} {:<14} {:<14} age",
            "scope", "host", "node", "agent"
        );
    }
    println!("{}", "─".repeat(if show_all { 86 } else { 72 }));
    for (c, stale_reason) in &shown {
        let age = chrono::DateTime::parse_from_rfc3339(&c.heartbeat_at)
            .ok()
            .map(|t| {
                let secs = now
                    .signed_duration_since(t.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(0);
                format!("{secs}s")
            })
            .unwrap_or_else(|| "?".to_string());
        if show_all {
            let state = match stale_reason {
                Some(reason) => format!("{} {}", "⚠ stale".yellow(), reason.dimmed()),
                None => format!("{}", "● live".green()),
            };
            println!(
                "{:<20} {:<10} {:<14} {:<14} {:<10} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age,
                state
            );
        } else {
            println!(
                "{:<20} {:<10} {:<14} {:<14} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age
            );
        }
        println!("{}{}", " ".repeat(2), c.clone_path.dimmed());
    }
    if !show_all && !stale.is_empty() {
        println!(
            "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
            "·".dimmed(),
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan(),
            "aida session leases --prune-stale".cyan()
        );
    }
    shown.len()
}

/// TASK-345: build the `drain` cross-reference for a lease whose scope is the
/// active drain's current member, or `None` when this lease is not the drain's
/// active member (or no drain is live). Lets `aida session leases --json`
/// correlate a lease to the orchestrator drain/run driving it — `run_uuid`
/// (the per-spec orchestration id), the current phase, the drain mode/batch,
/// the launching command, the orchestrator pid, and its start time.
// trace:TASK-345 | ai:claude
pub(crate) fn lease_drain_xref(
    scope: &str,
    drain: Option<&drain_state::DrainState>,
) -> Option<serde_json::Value> {
    let d = drain?;
    // Only the drain's *current* member is an orchestrator phase child.
    if d.current.as_deref() != Some(scope) {
        return None;
    }
    Some(serde_json::json!({
        "run_uuid": (!d.run_uuid.is_empty()).then(|| d.run_uuid.clone()),
        "member": d.current.clone(),
        "phase": d.current_phase.clone(),
        "mode": d.mode.clone(),
        "batch": d.batch.clone(),
        "command": d.command.clone(),
        "orchestrator_pid": d.orchestrator_pid,
        "started_at": d.started_at.clone(),
    }))
}

/// TASK-345: assemble the `aida session leases --json` rows. Pure over the
/// classified leases + optional live drain snapshot so the JSON shape is
/// unit-testable. Each row mirrors the human columns (id/scope/branch/role/
/// worktree/state/pid) and adds the `drain` cross-reference (null for leases
/// the orchestrator isn't currently driving).
// trace:TASK-345 | ai:claude
pub(crate) fn leases_json_rows(
    shown: &[(SessionLease, LeaseState, Option<u32>)],
    drain: Option<&drain_state::DrainState>,
) -> Vec<serde_json::Value> {
    shown
        .iter()
        .map(|(l, state, pid)| {
            serde_json::json!({
                "id": l.id,
                "scope": l.scope,
                "branch": l.branch,
                "role": l.role,
                "worktree": l.worktree_path.display().to_string(),
                "state": state.label(),
                "pid": pid,
                "drain": lease_drain_xref(&l.scope, drain),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/task345_leases_json_tests.rs"]
mod task345_leases_json_tests;

#[cfg(test)]
#[path = "tests/bug_1772_harness_worktree_lease_tests.rs"]
mod bug_1772_harness_worktree_lease_tests;

/// BUG-1764 clause 4: `aida session leases --prune-stale` — release every
/// CROSS-CLONE claim the staleness predicate reports dead.
///
/// Before this there was no supported way to clear one. `aida session end
/// --spec BUG-1689` answers `No lease found for spec ...` because a foreign
/// claim has no local lease to resolve; `aida session reap` requires spec
/// Done/Completed + branch merged + process exited; and
/// `coordination::release_claim` refuses a claim whose `clone_path` is not ours
/// by design. The only remedy was deleting
/// `.aida-store/coordination/leases/*.toml` by hand.
///
/// Opt-in and never automatic: releasing another clone's claim is a cross-clone
/// mutation. It can only ever touch a claim the SHARED predicate reports stale,
/// so there is no `--force` to add.
// trace:BUG-1764 | ai:claude
pub(crate) fn session_leases_prune_stale(yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_root = project_root.join(".aida-store");
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    let (_live, stale) =
        coordination::partition_claims(foreign, chrono::Utc::now(), &coordination::hostname());

    // BUG-1772 AC2: `aida session leases --prune-stale` also learns to reap
    // immortal pid-less harness-worktree leases.
    // trace:BUG-1772 | ai:antigravity
    let local_leases = list_leases(&project_root);
    let mut stale_local_harness_leases = vec![];
    let harness_threshold = chrono::Utc::now() - chrono::Duration::hours(24);
    for lease in local_leases {
        if lease.scope == worktree_lease::HARNESS_WORKTREE_SCOPE
            && lease.active_pid.is_none()
            && lease.creator_pid.is_none()
            && lease.started_at < harness_threshold
        {
            stale_local_harness_leases.push(lease);
        }
    }

    if stale.is_empty() && stale_local_harness_leases.is_empty() {
        println!("No stale leases to release.");
        return Ok(());
    }
    if !stale.is_empty() {
        println!(
            "{} stale cross-clone lease{} to release:",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        for (c, reason) in &stale {
            println!(
                "  {} {} {}",
                truncate(&c.scope, 20).yellow(),
                format!("({})", c.host).dimmed(),
                reason.dimmed()
            );
        }
    }
    if !stale_local_harness_leases.is_empty() {
        println!(
            "{} stale pid-less local harness-worktree lease{} to release:",
            stale_local_harness_leases.len(),
            if stale_local_harness_leases.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for l in &stale_local_harness_leases {
            println!("  {} {}", l.id.yellow(), "age > 24h".dimmed());
        }
    }

    // Same posture as `aida session end`: a non-terminal stdin must not be
    // able to release a peer's claims without saying so explicitly.
    if !yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            anyhow::bail!(
                "refusing to prune non-interactively without --yes (releasing another clone's \
                 claim is a cross-clone mutation)"
            );
        }
        let total = stale.len() + stale_local_harness_leases.len();
        if !confirm(&format!("Release {} stale lease(s)? [y/N] ", total)) {
            println!("Aborted.");
            return Ok(());
        }
    }
    let mut released = 0usize;
    for (c, reason) in &stale {
        match coordination::release_stale_foreign_claim(
            &store_root,
            &c.scope,
            &c.clone_path,
            &c.heartbeat_at,
        ) {
            Ok(true) => {
                released += 1;
                println!(
                    "  {} released cross-clone {} — {}",
                    "✓".green(),
                    c.scope,
                    reason
                );
            }
            // Refreshed, reclaimed, or already gone between the listing and the
            // delete — the verdict no longer describes the record on disk.
            Ok(false) => println!(
                "  {} skipped cross-clone {} — changed on the store since it was listed",
                "·".dimmed(),
                c.scope
            ),
            Err(e) => eprintln!(
                "  {} could not release cross-clone {}: {e}",
                "Warning:".yellow().bold(),
                c.scope
            ),
        }
    }
    let mut local_released = 0usize;
    for l in &stale_local_harness_leases {
        let p = lease_path(&project_root, &l.id);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                eprintln!(
                    "  {} could not remove {}: {e}",
                    "Warning:".yellow().bold(),
                    l.id
                );
            } else {
                local_released += 1;
                println!("  {} released local harness lease {}", "✓".green(), l.id);
            }
        }
    }

    println!();
    println!(
        "Released {} of {} stale cross-clone lease claim(s), and {} of {} stale local harness lease(s).",
        released,
        stale.len(),
        local_released,
        stale_local_harness_leases.len()
    );
    Ok(())
}

/// BUG-2010: classify a list of session leases with live agent overlay.
// trace:BUG-2010 | ai:antigravity
pub(crate) fn classify_session_leases(
    project_root: &std::path::Path,
    leases: &[SessionLease],
) -> Vec<(SessionLease, LeaseState, Option<u32>)> {
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let live_agents = agent_registry::load_live_entries(project_root);
    leases
        .iter()
        .map(|l| {
            let live_in_worktree = live.iter().find(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            // BUG-511: review-verb leases classify by creator PID, not worktree.
            let mut state = lease_state_for(l, &live, now);
            let mut pid = l
                .active_pid
                .filter(|p| process_probe::pid_is_alive(*p))
                .or_else(|| live_in_worktree.map(|s| s.pid))
                .or(if l.review_verb || l.claim_verb {
                    l.creator_pid
                } else {
                    None
                });
            // BUG-2010: a live registered agent (Codex, Antigravity, Claude) keeps the lease live
            // even if its recorded pid was dead or transient.
            // trace:BUG-2010 | ai:antigravity
            if !matches!(state, LeaseState::Live) {
                if let Some(agent) =
                    agent_registry::live_agent_for_lease_or_spec(&live_agents, Some(l), None)
                {
                    state = LeaseState::Live;
                    if pid.is_none() {
                        pid = Some(agent.pid);
                    }
                }
            }
            (l.clone(), state, pid)
        })
        .collect()
}

pub(crate) fn session_leases(verbose: bool, all: bool, json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        // TASK-345: JSON consumers get an empty array, not the human hints.
        // trace:TASK-345 | ai:claude
        if json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!([]))?);
            return Ok(());
        }
        // STORY-637: even with no LOCAL leases, a peer clone may hold a
        // cross-clone lease — surface it before the "no sessions" hint so a
        // refused `session start` has a place to point.
        let foreign = print_cross_clone_leases(&project_root, chrono::Utc::now(), all);
        if foreign == 0 {
            println!("(no active sessions)");
        }
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        // BUG-98: explicit pointer at the historical-conversations view so
        // users who landed here looking for `list` know where it lives.
        eprintln!();
        eprintln!(
            "{}",
            "(for the historical list of Claude Code conversations in this project, run `aida session conversations`)"
                .dimmed()
        );
        return Ok(());
    }

    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let root = find_main_worktree_root()?;
    let classified = classify_session_leases(&root, &leases);

    let (shown, hidden_stale) = select_lease_listing(classified, all);

    // STORY-301: cross-reference the active `--auto-complete` drain. A lease
    // whose scope is the drain's current member is an orchestrator phase
    // child — annotate it so `aida session leases` shows which session the
    // orchestrator is driving, and at which phase. trace:STORY-301 | ai:claude
    // TASK-345: retain the full live DrainState so the `--json` path can emit
    // the drain cross-reference (run uuid / mode / orchestrator pid) per lease,
    // not just the phase string the human view needs. trace:TASK-345 | ai:claude
    let drain_active: Option<drain_state::DrainState> = match find_main_worktree_root()
        .map(|r| drain_state::probe(&r))
        .unwrap_or(drain_state::DrainStatus::None)
    {
        drain_state::DrainStatus::Active(s) => Some(s),
        _ => None,
    };
    let drain_current: Option<(String, Option<String>)> = drain_active
        .as_ref()
        .and_then(|s| s.current.clone().map(|c| (c, s.current_phase.clone())));

    // TASK-345: machine-readable lease list. Each row carries a `drain`
    // cross-reference so a consumer can correlate a lease to the orchestrator
    // drain/run driving it. Emitted before any human rendering.
    // trace:TASK-345 | ai:claude
    if json {
        let rows = leases_json_rows(&shown, drain_active.as_ref());
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }

    println!("{}", "Active session leases".bold());
    println!();
    // STORY-65: role column shows the persona inherited at session start.
    // BUG-2002: always show state, since stale spec leases block dispatch.
    // TASK-56: adds
    // `pid` + `cc-session` columns when --verbose is set.
    // trace:STORY-65, TASK-55, TASK-56 | ai:claude
    let mut header = format!(
        "{:<14} {:<20} {:<18} {:<14}",
        "id", "scope", "branch", "role"
    );
    header.push_str(&format!(" {:<10}", "state"));
    if verbose {
        header.push_str(&format!(" {:<10} {:<20}", "pid", "cc-session"));
    }
    header.push_str(" worktree");
    println!("{}", header);
    println!("{}", "─".repeat(header.len().max(80)));
    // BUG-312: HLC-derived UUIDs put the timestamp at the start, so two
    // leases created in the same generation window share the historical
    // 8-char prefix. Disambiguate against the FULL active set (not just
    // `shown`) — `aida session end` resolves over the unfiltered set,
    // so the rendered id has to be unique against everything an end
    // call might match. trace:BUG-312 | ai:claude
    let all_ids: Vec<&str> = leases.iter().map(|l| l.id.as_str()).collect();
    for (l, state, pid) in &shown {
        let prefix_len = unique_prefix_len(&l.id, &all_ids, 8);
        let mut row = format!(
            "{:<14} {:<20} {:<18} {:<14}",
            (&l.id[..prefix_len]).yellow(),
            truncate(&l.scope, 20),
            truncate(&l.branch, 18),
            truncate(l.role.as_deref().unwrap_or("-"), 14),
        );
        let label = format!("{} {}", state.glyph(), state.label());
        let colored = match state {
            LeaseState::Live => label.green(),
            LeaseState::Dormant => label.cyan(),
            LeaseState::Stale => label.yellow(),
        };
        row.push_str(&format!(" {:<10}", colored));
        if verbose {
            let pid_col = pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            let cc = cc_session_id_for_worktree(&l.worktree_path)
                .map(|s| s.chars().take(20).collect::<String>())
                .unwrap_or_else(|| "-".into());
            row.push_str(&format!(" {:<10} {:<20}", pid_col, cc));
        }
        row.push_str(&format!(" {}", l.worktree_path.display()));
        println!("{}", row);
        // STORY-301: annotate the lease the orchestrator is currently driving.
        if let Some((cur, phase)) = &drain_current {
            if &l.scope == cur {
                let phase_txt = phase.as_deref().unwrap_or("starting");
                println!(
                    "{}",
                    format!("{}◆ orchestrator drain — phase {phase_txt}", " ".repeat(15)).cyan()
                );
            }
        }
    }
    if !hidden_stale.is_empty() {
        println!();
        println!(
            "{}",
            format!(
                "({} stale lease{} hidden — pass --all to show)",
                hidden_stale.len(),
                if hidden_stale.len() == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }

    // STORY-69: --verbose warns about leaked claudes — processes whose
    // cwd is `(deleted)` (worktree was removed without ending claude
    // first, the BUG-61 signature). Note: live-in-lease claude PIDs
    // now appear inline in the `pid` column for each lease row
    // (TASK-56), so the previous "Live claude processes in lease
    // worktrees" sub-section was redundant and was removed.
    // trace:STORY-69 | ai:claude
    if verbose {
        let leaked: Vec<_> = live.iter().filter(|s| s.stale_cwd).collect();
        if !leaked.is_empty() {
            println!();
            println!("{}", "Leaked claude processes:".bold());
            for s in &leaked {
                println!(
                    "  {} pid {} cwd {} {} — `kill {}` to clean up",
                    "WARN".yellow().bold(),
                    s.pid.to_string().yellow(),
                    s.cwd.display(),
                    "(deleted)".dimmed(),
                    s.pid
                );
            }
        }
    }

    // STORY-637: surface cross-clone lease claims held by OTHER clones —
    // distinct from the local leases above. trace:STORY-637 | ai:claude
    print_cross_clone_leases(&project_root, now, all);

    println!();
    println!(
        "End one with: {} {}",
        "aida session end".cyan(),
        "<id>".dimmed()
    );
    Ok(())
}

/// One `.jsonl` file old enough to prune. Captured up-front so the
/// confirmation list and the deletion loop see the same set.
/// trace:STORY-60 | ai:claude
pub(crate) struct PruneCandidate {
    pub(crate) path: std::path::PathBuf,
    pub(crate) size: u64,
    pub(crate) age_seconds: u64,
}

/// Format a byte count with a human-readable unit (KB/MB/GB). One
/// decimal place so a 1.5 MB file doesn't read as "1 MB" or "1500000 B".
/// trace:STORY-60 | ai:claude
pub(crate) fn humanize_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{} B", bytes)
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Compute the new queue position for `aida queue move X --after Y`.
/// `anchor` is Y's current position; `successor` is the position of the
/// next entry strictly after Y, or `None` when Y is at the bottom.
///
/// Returns a position that sorts strictly between `anchor` and the next
/// entry. Midpoint math when there's room; +1 fallback when adjacent
/// (collision risk acknowledged — safe in practice because positions are
/// initially gapped by 1000); +1000 when Y is at the bottom. All math is
/// saturating so a corrupt-state queue (every item at `i64::MAX` from a
/// pre-fix queue_add, see git_backend.rs) doesn't panic on overflow —
/// the callsite is expected to surface a friendlier hint instead.
/// trace:STORY-72 | ai:claude
pub(crate) fn position_after(anchor: i64, successor: Option<i64>) -> i64 {
    match successor {
        Some(np) if np > anchor.saturating_add(1) => {
            anchor.saturating_add((np.saturating_sub(anchor)) / 2)
        }
        Some(_) => anchor.saturating_add(1),
        None => anchor.saturating_add(1000),
    }
}

/// Compute the queue order after `aida queue move X --to N` — moving
/// `moved_id` to the 1-indexed absolute slot `requested`.
///
/// `entries` is the queue in display order (position-sorted) and must
/// include `moved_id`. The moved id is dropped, then re-inserted at the
/// clamped slot, so `--to 3` lands the item at the third slot of the
/// *final* queue regardless of where it started. `requested` is clamped
/// to `1..=entries.len()`: an out-of-range N moves the item to the
/// nearest end rather than erroring (the callsite surfaces a hint). The
/// caller renumbers the returned id list with the standard 1000-gap.
/// trace:TASK-280 | ai:claude
pub(crate) fn move_to_absolute_position(
    entries: &[uuid::Uuid],
    moved_id: uuid::Uuid,
    requested: usize,
) -> (Vec<uuid::Uuid>, usize) {
    let queue_len = entries.len();
    let mut order: Vec<uuid::Uuid> = entries.iter().copied().filter(|&e| e != moved_id).collect();
    let slot = requested.clamp(1, queue_len.max(1));
    let insert_idx = (slot - 1).min(order.len());
    order.insert(insert_idx, moved_id);
    (order, slot)
}

pub(crate) fn humanize_age_secs(secs: u64) -> String {
    if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs < 30 * 86_400 {
        format!("{}w", secs / (7 * 86_400))
    } else if secs < 365 * 86_400 {
        format!("{}mo", secs / (30 * 86_400))
    } else {
        format!("{}y", secs / (365 * 86_400))
    }
}

/// `aida session prune` — walk Claude Code's per-project session
/// directories under `~/.claude/projects/<encoded>/` and delete `.jsonl`
/// files whose mtime is older than `days`. Skips any project dir
/// corresponding to an active session lease so a long-running session
/// doesn't self-delete from a forgotten cron-like usage. Logs each
/// deletion to `<project>/.aida/session-prune.log` for auditing.
/// trace:STORY-60 | ai:claude
/// Walk `~/.claude/projects/*` and find project dirs whose recorded cwd
/// no longer exists on disk. Each surviving jsonl encodes its cwd in
/// every event; we read one jsonl per project dir, extract the cwd
/// from the first event that has one, and check the filesystem. No
/// jsonls in the dir → treat as orphan (the dir is content-less).
/// Falls back to the lossy dir-name decode (replace `-` with `/`) when
/// the jsonl lacks a `"cwd":` field. trace:TASK-70 | ai:claude
/// TASK-358: mechanically tear down a lease + its worktree without the
/// interactive ceremony of [`session_end`] — no CI probe, no live-claude
/// refusal, no dirty-tree refusal, no prompts. Only callers that already
/// know the lease is safe to remove (e.g. the orchestrator's
/// `--escalate-blocks` marker [`SessionLease::escalated_to_human`]) should
/// reach this. Mirrors `session_end`'s interior writes — strip runtime
/// symlinks → unlink lease/manifest/activity log → `git worktree remove
/// --force` — and warns rather than errors on partial failure so a sweep
/// that hits one stuck worktree still tries the rest. Returns `true` when
/// the worktree was removed cleanly.
/// trace:TASK-358 | ai:claude
pub(crate) fn force_cleanup_lease(project_root: &std::path::Path, lease: &SessionLease) -> bool {
    // BUG-511: advisory leases (review-verb / MCP claims that recorded no
    // worktree) have nothing on disk beyond the lease file itself — and an
    // empty `worktree_path` would make the symlink-strip and `git worktree
    // remove` legs below resolve relative to CWD. Remove the lease file
    // (after the same activity aggregation) and stop.
    if lease.worktree_path.as_os_str().is_empty() {
        aggregate_session_activity_into_roles(project_root, &lease.id);
        // trace:BUG-2139 | ai:claude
        crate::session::revoke_lease_child_grant(&lease_path(project_root, &lease.id));
        let _ = std::fs::remove_file(lease_path(project_root, &lease.id));
        let activity = session_activity_path(project_root, &lease.id);
        if activity.exists() {
            let _ = std::fs::remove_file(&activity);
        }
        return true;
    }

    // Every caller of this function is automatic or bulk (doctor's lease
    // heal, pickup and review stale-lease release, drain-retry predecessor
    // release, escalated-worktree cleanup), so it uses the strict check:
    // a tree a live process is using, or one the scan could not verify, keeps
    // BOTH its lease and its worktree. Checked before anything is touched, so
    // the lease still marks the scope as occupied and the caller's
    // re-classification sees it as live instead of looping.
    // trace:BUG-1947 | ai:claude
    if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(&lease.worktree_path) {
        eprintln!(
            "  {} kept session {} and its worktree {}: {}",
            "Warning:".yellow().bold(),
            &lease.id[..lease.id.len().min(8)],
            lease.worktree_path.display(),
            in_use.summary(),
        );
        return false;
    }

    // Snapshot the lease's authoritative on-disk path before we strip the
    // worktree's symlinks (which can break the symlink chain). Mirrors
    // BUG-56's pattern in `session_end`.
    let lease_file_via_symlink = lease_path(project_root, &lease.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Aggregate the session's activity log into project-level role activity
    // before we delete the log — same ordering as `session_end`.
    aggregate_session_activity_into_roles(project_root, &lease.id);
    let activity_file = session_activity_path(project_root, &lease.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Strip the runtime symlinks `session_start` installed so `git
    // worktree remove` doesn't refuse on untracked files (BUG-52).
    let store_link = lease.worktree_path.join(".aida-store");
    if store_link.is_symlink() {
        let _ = std::fs::remove_file(&store_link);
    }
    let aida_dir = lease.worktree_path.join(".aida");
    for runtime in &[
        "sessions",
        "roles",
        "cache.db",
        "cache.db-shm",
        "cache.db-wal",
        "pgdata",
    ] {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Lease file first — once gone the session no longer counts as active.
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    // BUG-2139: a headless child's grant ends with its lease.
    // trace:BUG-2139 | ai:claude
    crate::session::revoke_lease_child_grant(lease_target);
    match std::fs::remove_file(lease_target) {
        Ok(_) | Err(_) => {}
    }

    // Companion files — quiet on missing.
    let manifest_target = session_manifest::manifest_path(project_root, &lease.id);
    if manifest_target.exists() {
        let _ = std::fs::remove_file(&manifest_target);
    }
    if activity_target.exists() {
        let _ = std::fs::remove_file(&activity_target);
    }

    // `git worktree remove --force` — `--force` because the dirty-tree
    // refusal in `session_end` is gated by interactive prompts; an
    // escalation cleanup never had a chance to commit, so we just remove.
    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "worktree",
            "remove",
            "--force",
            "--", // trace:BUG-1622 | ai:claude
            lease.worktree_path.to_str().unwrap_or_default(),
        ])
        .output();
    let removed = matches!(&res, Ok(o) if o.status.success());
    if !removed {
        eprintln!(
            "  {} could not remove worktree {} — remove manually with \
             `git worktree remove --force {}`",
            "Warning:".yellow().bold(),
            lease.worktree_path.display(),
            lease.worktree_path.display(),
        );
    }
    removed
}

/// TASK-358: on a triage that takes a spec out of `NeedsAttention`, find any
/// orchestrator-escalated lease for it and clean up the lingering worktree.
/// The lease's `escalated_to_human` marker is the load-bearing gate: only
/// leases set by `--escalate-blocks` are touched, so an interactive user
/// session on the same spec stays put, and the advisor-resume path
/// (which never sets the marker) preserves its worktree as STORY-306
/// requires. Best-effort: a missing project root or zero matching leases
/// is a quiet no-op. trace:TASK-358 | ai:claude
pub(crate) fn cleanup_escalated_leases_for_spec(project_root: &std::path::Path, spec_id: &str) {
    let leases = list_leases(project_root);
    let mut cleaned = 0usize;
    for lease in leases {
        if lease.escalated_to_human.is_none() {
            continue;
        }
        if !lease.scope.eq_ignore_ascii_case(spec_id) {
            continue;
        }
        eprintln!(
            "  {} cleaning up escalated worktree for {}: {}",
            "→".dimmed(),
            spec_id.cyan(),
            lease.worktree_path.display().to_string().dimmed(),
        );
        // A tree still in use keeps its lease (force_cleanup_lease says why);
        // count only leases that were actually released. trace:BUG-1947 | ai:claude
        force_cleanup_lease(project_root, &lease);
        if !lease_path(project_root, &lease.id).exists() {
            cleaned += 1;
        }
    }
    if cleaned > 0 {
        eprintln!(
            "  {} {} escalated worktree{} cleaned up",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            cleaned,
            if cleaned == 1 { "" } else { "s" },
        );
    }
}

/// TASK-1311: the per-row requeue affordance under a NeedsAttention spec in
/// `aida findings list`. It shows the one command that returns the spec to
/// flight and the status it lands in, and puts an open decision first.
/// STORY-1429: rendered from the same `requeue_preview` the interactive
/// `aida rework` loop shows, so the hint and the keystroke cannot disagree.
// trace:TASK-1311 trace:STORY-1429 | ai:claude
pub(crate) fn print_requeue_hint_row(
    r: &aida_core::Requirement,
    did: &str,
    store: Option<&aida_core::RequirementsStore>,
    storage: &Storage,
) {
    let route = queue_cmd::current_queue_route(storage, r.id);
    let preview = requeue::requeue_preview(
        r,
        store,
        route.as_deref(),
        requeue::caller_may_clear_escalation(),
    );
    println!(
        "  {:<20} {:<14} {}",
        "",
        "",
        format!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::SubArrow),
            requeue::requeue_hint(did, &preview)
        )
        .dimmed()
    );
}

/// EPIC-28: park a spec in `NeedsAttention` with a structured
/// [`aida_core::FailureReason`] so a batch drain can continue past a phase
/// failure rather than halting. Called from
/// `RealPhaseDriver::shelve_on_failure` (which the orchestrator's
/// `finish_failure` invokes for every shelvable phase failure).
///
/// Returns the populated `FailureReason` on success (the orchestrator
/// stamps it on `OrchestrationResult::shelved_reason`); returns `Ok(None)`
/// when the spec cannot be safely shelved (e.g. it is already in a
/// terminal status). Best-effort: ledger-write failures are logged but
/// never undo the status flip, mirroring `aida punt`.
/// trace:EPIC-28 | ai:claude
pub(crate) fn shelve_spec_on_failure(
    project_root: &std::path::Path,
    spec: &str,
    phase_slug: &str,
    phase_index: u8,
    kind_slug: &str,
    detail: &str,
    recovery_hint: &str,
) -> anyhow::Result<Option<aida_core::FailureReason>> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        // No distributed store at this root → nothing to update. The
        // orchestrator only runs in git-canonical projects today; this is
        // defensive for the legacy centralized-SQLite layout where
        // shelving is not supported.
        return Ok(None);
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    // trace:TASK-1468 | ai:claude
    let Some(mut req) = backend.get_requirement_unambiguous(spec)? else {
        // Spec not in the store — orchestrator was driving a stale id.
        // Best-effort: log and skip rather than crash the failure path.
        eprintln!(
            "  {} could not load spec {} for shelving — store has no matching id",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            spec,
        );
        return Ok(None);
    };
    let display_id = req.display_id();

    let now = chrono::Utc::now();
    let fr = aida_core::FailureReason {
        phase: phase_slug.to_string(),
        phase_index,
        kind: kind_slug.to_string(),
        detail: detail.to_string(),
        recovery_hint: if recovery_hint.is_empty() {
            None
        } else {
            Some(recovery_hint.to_string())
        },
        shelved_by: std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|r| !r.is_empty()),
        shelved_at: now,
    };

    // Honor the STORY-332 transition guard. The common case is
    // `InProgress → NeedsAttention` (orchestrator drove the spec through
    // phase 1 pickup, which set InProgress). Edge cases where the spec
    // never reached InProgress (a phase-0 setup failure) still record the
    // FailureReason so the operator sees it, but skip the status flip so
    // we don't fight the guard.
    shelve_status_flip(&mut req);
    req.failure_reason = Some(fr.clone());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    // Best-effort ledger append + role activity record. A failure here
    // must not undo the spec update, which is the load-bearing part.
    if let Err(e) = punt::append_failure_to_ledger(project_root, &display_id, &fr) {
        eprintln!(
            "  {} could not write shelving record to .aida/punts.jsonl: {e}",
            "Note:".dimmed()
        );
    }
    record_role_activity(&display_id, "shelve");

    Ok(Some(fr))
}

/// The shelve's status flip: `NeedsAttention` when the STORY-332 transition
/// guard allows it (from In Progress or Done), otherwise no change, so a
/// terminal spec is never moved. Returns whether the status changed.
///
/// BUG-1632: the shelve is an automated write, so the flip is recorded in the
/// status history under [`aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR`].
/// The BUG-1625 merge guard then keeps a terminal status another clone reached
/// meanwhile instead of letting this newer NeedsAttention win.
// trace:EPIC-28 trace:BUG-1632 | ai:claude
pub(crate) fn shelve_status_flip(req: &mut aida_core::Requirement) -> bool {
    if aida_core::forbidden_attention_transition(
        &req.status,
        &aida_core::RequirementStatus::NeedsAttention,
    )
    .is_some()
    {
        return false;
    }
    let from = req.status.clone();
    req.status = aida_core::RequirementStatus::NeedsAttention;
    aida_core::conflict::record_status_transition(
        req,
        aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR,
        &from,
    );
    req.status != from
}

/// BUG-1637: the PR-open Done assertion's status write. Automated, and only
/// ever In Progress -> Done, so it never leaves a terminal status. Recorded
/// under [`aida_core::conflict::PR_OPEN_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn pr_open_done_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::InProgress) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Done,
        aida_core::conflict::PR_OPEN_AUTHOR,
    );
    true
}

/// BUG-1637: the Approved -> In Progress coherence bump shared by the
/// SubagentStart lease-take hook ([`aida_core::conflict::LEASE_TAKE_AUTHOR`])
/// and `aida session start` ([`aida_core::conflict::SESSION_START_AUTHOR`]).
/// Automated; any other source status (terminal included) is left alone.
/// Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn approved_to_in_progress_bump(req: &mut Requirement, author: &str) -> bool {
    if !matches!(req.status, RequirementStatus::Approved) {
        return false;
    }
    aida_core::conflict::set_status_recorded(req, RequirementStatus::InProgress, author);
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the zen autopilot's Draft -> Approved write. It applies only to
/// a Draft (the status the approve-gate judged and the autopilot audit records
/// as the prior state), so it never leaves a terminal status. Recorded under
/// [`aida_core::conflict::ZEN_APPROVE_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn zen_approve_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::Draft) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Approved,
        aida_core::conflict::ZEN_APPROVE_AUTHOR,
    );
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the orchestrator's phase-1 pre-spawn bump (BUG-369). Re-checks
/// the source status on the copy read inside the atomic write, so a spec a
/// concurrent writer moved to Rejected or Completed is not bumped, and records
/// the move under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`].
/// Returns whether it moved.
// trace:BUG-369 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_bump(
    r: &mut Requirement,
    target: &RequirementStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if auto_complete_phase1_target_status(&r.status).as_ref() != Some(target) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        r,
        target.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    r.modified_at = now;
    true
}

/// BUG-1637: TASK-133's restore of the pre-bump status after a phase-1
/// failure. Automated: it never leaves a terminal status (a spec that reached
/// one meanwhile is no longer the spec phase 1 bumped) and records the move
/// under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`]. Returns whether
/// it applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_restore(req: &mut Requirement, prior: &RequirementStatus) -> bool {
    if aida_core::conflict::is_terminal_status(&req.status) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        prior.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    true
}

/// BUG-1637: the whole TASK-133 restore write, run inside the atomic write:
/// restore the pre-bump status and clear the spurious shelve FailureReason, or
/// change nothing when [`phase1_status_restore`] refuses. Returns whether it
/// applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn apply_phase1_restore(r: &mut Requirement, prior: &RequirementStatus) -> bool {
    if !phase1_status_restore(r, prior) {
        return false;
    }
    // The shelve that ran on the failure path stamped a FailureReason; with no
    // lease and no work behind it that finding is spurious — clear it.
    r.failure_reason = None;
    r.modified_at = chrono::Utc::now();
    true
}

// BUG-1638: race-test seam for the self-reading status writers
// (`ensure_spec_done_after_pr`, `bump_spec_in_progress_at_lease_take`,
// `restore_phase1_status_on_lease_failure`). Each reads the spec, then writes
// it through `update_spec_atomically`; a test arms a one-shot hook that runs
// between the read and the write, so it can make a concurrent change there and
// pin that the write re-checks the copy read under the store lock. Compiled
// out of non-test builds. trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) type StatusWriteRaceHook = Box<dyn FnOnce(&std::path::Path)>;

#[cfg(test)]
thread_local! {
    pub(crate) static STATUS_WRITE_RACE_HOOK: std::cell::RefCell<Option<StatusWriteRaceHook>> =
        const { std::cell::RefCell::new(None) };
}

/// BUG-1638: arm the one-shot hook [`status_write_race_seam`] runs on this
/// thread. The hook receives the store root the writer resolved.
// trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) fn inject_status_write_race(hook: impl FnOnce(&std::path::Path) + 'static) {
    STATUS_WRITE_RACE_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}

/// BUG-1638: called by each self-reading status writer between its read and
/// its atomic write. A no-op outside tests.
// trace:BUG-1638 | ai:claude
pub(crate) fn status_write_race_seam(store_root: &std::path::Path) {
    #[cfg(test)]
    {
        let hook = STATUS_WRITE_RACE_HOOK.with(|h| h.borrow_mut().take());
        if let Some(hook) = hook {
            hook(store_root);
        }
    }
    #[cfg(not(test))]
    let _ = store_root;
}

/// BUG-1638: what [`restore_phase1_status_on_lease_failure`] did, so the
/// orchestrator reports a restore only when one happened.
// trace:BUG-1638 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase1RestoreOutcome {
    /// The pre-bump status was written back.
    Restored,
    /// Nothing was written; the reason says why.
    Refused(String),
}

/// BUG-1638: the orchestrator's line for a phase-1 restore outcome. It says
/// "status restored" only for [`Phase1RestoreOutcome::Restored`]. BUG-1647:
/// statuses read as the CLI shows them ("Needs Attention"), the same form the
/// refusal reason uses.
// trace:BUG-1638 trace:BUG-1647 | ai:claude
pub(crate) fn phase1_restore_outcome_message(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
) -> String {
    match outcome {
        Phase1RestoreOutcome::Restored => format!(
            "phase-1 startup failed before acquiring a lease — status restored to {prior} \
             (re-queueable); no work was stranded"
        ),
        Phase1RestoreOutcome::Refused(reason) => format!(
            "phase-1 startup failed before acquiring a lease — {display_id}'s status was not \
             restored to {prior}: {reason}"
        ),
    }
}

/// BUG-1651: the stderr line the orchestrator prints for a phase-1 restore
/// outcome, or `None` under `--json`: both the restore and the refusal are
/// suppressed so JSON output stays machine-readable (BUG-1647).
// trace:BUG-1647 trace:BUG-1651 | ai:claude
pub(crate) fn phase1_restore_report_line(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
    json: bool,
) -> Option<String> {
    if json {
        return None;
    }
    let glyph = match outcome {
        Phase1RestoreOutcome::Restored => "↩".cyan(),
        Phase1RestoreOutcome::Refused(_) => crate::glyph(crate::glyphs::Glyph::Info).cyan(),
    };
    Some(format!(
        "  {glyph} {}",
        phase1_restore_outcome_message(outcome, display_id, prior)
    ))
}

/// TASK-133: undo the orchestrator parent's pre-spawn phase-1 status bump.
///
/// `prepare_auto_complete_phase1_status` flips a spec Approved/Planned/Draft →
/// InProgress *before* spawning the implementer child (BUG-369). When phase 1
/// then fails without the child ever acquiring a lease, no work happened — but
/// the spec is now InProgress and, because `finish_failure` shelves shelvable
/// phase failures, typically NeedsAttention with a `failure_reason`. That
/// strands a spec the operator should be able to re-queue: there is nothing to
/// triage, only a transient spawn / contention error.
///
/// This restores the captured pre-bump status and clears the (spurious)
/// `failure_reason` so the spec drops out of `aida findings list` and is
/// cleanly pickable again. Best-effort, like the shelve it compensates: a
/// store-write error is logged but never aborts the drain epilogue. Writes
/// through `CachedGitBackend` so the read-projection (`aida list`) reflects
/// the reset immediately rather than waiting for a stale-detection rebuild.
/// trace:TASK-133 | ai:claude
/// BUG-479: does ANY child-side evidence of real work exist for `spec`?
/// Pure decision over the three probed signals — a live/stale lease scoped to
/// the spec, an on-disk worktree for it, or unmerged commits on its branch.
/// Any one being present means the implementer child DID acquire a lease and do
/// work before exiting non-zero (e.g. a post-commit `/aida-pr` failure or a
/// headless session aborting after committing), so the parent's "no lease ⇒ no
/// work" assumption is wrong and the spec must stay shelved for triage rather
/// than be reset. trace:BUG-479 | ai:claude
pub(crate) fn child_side_work_exists(
    has_lease: bool,
    has_worktree: bool,
    has_unmerged_commits: bool,
) -> bool {
    has_lease || has_worktree || has_unmerged_commits
}

/// BUG-479: probe the real world for child-side work on `spec`, returning
/// `(has_lease, has_worktree, has_unmerged_commits)`.
///
/// - `has_lease`: a session lease whose scope matches the spec (case-insensitive
///   — the same scope-matching `aida session start`'s double-claim guard and the
///   implementer-prompt lease lookups use).
/// - `has_worktree`: such a lease records a worktree path that still exists on
///   disk (the worktree the child created when it acquired the lease).
/// - `has_unmerged_commits`: such a lease's branch has commits not yet on the
///   default branch (`git rev-list --count <default>..<branch>` > 0).
///
/// Conservative: a git probe that can't run leaves its signal `false`, but the
/// lease/worktree signals already cover the dominant case, and the caller treats
/// ANY of the three as "leave it shelved". trace:BUG-479 | ai:claude
pub(crate) fn probe_child_side_work_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> (bool, bool, bool) {
    let matching: Vec<SessionLease> = list_leases(project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(spec))
        .collect();
    let has_lease = !matching.is_empty();
    let has_worktree = matching.iter().any(|l| l.worktree_path.exists());

    let has_unmerged_commits = match detect_default_branch_ref(project_root) {
        Some(default_ref) => matching
            .iter()
            .any(|l| branch_has_unmerged_commits(project_root, &default_ref, &l.branch)),
        None => false,
    };

    (has_lease, has_worktree, has_unmerged_commits)
}

/// BUG-479 (extracted for BUG-1716): does `branch` carry commits not yet on
/// `default_ref` (`git rev-list --count <default>..<branch>` > 0)?
/// Conservative: an empty/default branch name or a git probe that can't run
/// reads as `false`.
// trace:BUG-479 trace:BUG-1716 | ai:claude
pub(crate) fn branch_has_unmerged_commits(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    if branch.is_empty() || branch == default_ref {
        return false;
    }
    let range = format!("{default_ref}..{branch}");
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// BUG-1716: the session-lease evidence feeding the `Missing`-log arm of
/// [`empty_launch_decision`]. `None` — no lease exists for THIS session (the
/// child never claimed one), which routes the failure to the BUG-1629/
/// BUG-1769 lost-child recovery, not the empty-launch lane. `Some(has
/// commits)` — the child claimed its lease (the TASK-204 shape), and the
/// bool says whether its branch carries unmerged commits (real work the
/// empty-launch release must never orphan). Keyed to THIS session's lease
/// via the minted `--session-id`, never a scope-wide sweep — a sibling
/// attempt's PR branch must not veto classifying this launch.
// trace:BUG-1716 | ai:claude
pub(crate) fn orchestrated_session_lease_evidence(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> Option<bool> {
    let (_, branch, _, _) = find_orchestrated_lease(project_root, claude_session_id)?;
    let has_commits = detect_default_branch_ref(project_root)
        .map(|default_ref| branch_has_unmerged_commits(project_root, &default_ref, &branch))
        .unwrap_or(false);
    Some(has_commits)
}

pub(crate) fn restore_phase1_status_on_lease_failure(
    project_root: &std::path::Path,
    spec: &str,
    prior: &aida_core::RequirementStatus,
) -> anyhow::Result<Phase1RestoreOutcome> {
    // BUG-479: before resetting, probe child-side reality. The caller gates on
    // the PARENT-side `implementer_lease` field, which is only set after a CLEAN
    // child exit — so a child that acquired a lease + worktree + committed and
    // THEN exited non-zero (post-commit `/aida-pr` failure, aborted headless
    // session) reaches here with `implementer_lease == None` even though real
    // work is on disk. Resetting would clear the shelve + restore status while
    // the orphaned lease/worktree/commits remain, so the next pickup collides
    // (BUG-436/BUG-438 class). If ANY child-side work exists, leave it shelved
    // for triage. Conservative: any doubt → don't restore. trace:BUG-479 | ai:claude
    let (has_lease, has_worktree, has_unmerged_commits) =
        probe_child_side_work_for_spec(project_root, spec);
    // BUG-1638: every early exit is a refusal the caller reports, never a
    // silent Ok that reads as "restored". trace:BUG-1638 | ai:claude
    if child_side_work_exists(has_lease, has_worktree, has_unmerged_commits) {
        return Ok(Phase1RestoreOutcome::Refused(format!(
            "a lease, worktree or commits exist for {spec}, so it stays shelved for triage \
             (run `aida findings list`)"
        )));
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Phase1RestoreOutcome::Refused(
            "no AIDA store was found for this project".to_string(),
        ));
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let gone = || Phase1RestoreOutcome::Refused(format!("{spec} no longer exists"));
    // trace:TASK-1468 | ai:claude
    let Some(req) = backend.get_requirement_unambiguous(spec)? else {
        return Ok(gone());
    };
    // BUG-1638: the race-test seam between the read and the write.
    // trace:BUG-1638 | ai:claude
    status_write_race_seam(&store_path);
    // BUG-1637: the restore and its terminal-status refusal run on the copy
    // read under the store lock; a refused restore writes nothing.
    // trace:BUG-1637 | ai:claude
    let mut applied = false;
    let mut seen = req.status.clone();
    let written = backend.update_spec_atomically(&req, |r| {
        seen = r.status.clone();
        applied = apply_phase1_restore(r, prior);
    })?;
    Ok(match (written, applied) {
        (None, _) => gone(),
        (Some(_), true) => Phase1RestoreOutcome::Restored,
        (Some(_), false) => Phase1RestoreOutcome::Refused(format!(
            "it is now {seen}, a final status, so it was left as is"
        )),
    })
}

/// TASK-358 tests — the `--escalate-blocks` worktree cleanup.
///
/// The wiring exercised here:
///   1. `mark_lease_escalated_to_human` stamps the timestamp on the lease
///      file and round-trips through TOML.
///   2. `cleanup_escalated_leases_for_spec` only touches leases that
///      (a) carry the marker AND (b) match the spec by scope. A bare
///      interactive lease on the same spec is ignored (preserves user work),
///      and an escalated lease for an unrelated spec is ignored (the triage
///      hook is per-spec).
///
/// trace:BUG-511 | ai:claude
#[cfg(test)]
#[path = "tests/bug_511_review_lease_tests.rs"]
mod bug_511_review_lease_tests;

// The per-spec liveness verdict (`aida status <spec>`) and the `aida why`
// STALLED surfacing. trace:STORY-694 trace:BUG-623 | ai:claude
#[cfg(test)]
#[path = "tests/story_694_spec_liveness_tests.rs"]
mod story_694_spec_liveness_tests;

// BUG-637: the spec-scoped CLAIM gates — pre-pickup refuse (the BUG-634
// duplicate-fanout fix) + pre-edit liveness (the EPIC-54-reject fix). Both gates
// are pure over the lease set plus an injected liveness predicate, so the matrix
// is testable without a live process or a lease dir.
// trace:BUG-637 | ai:claude
#[cfg(test)]
#[path = "tests/bug_637_spec_claim_tests.rs"]
mod bug_637_spec_claim_tests;

/// TASK-957: `aida claim` / `aida unclaim` — the advisor-fan-out claim that
/// closes the BUG-637 gap. A claim is a spec-scoped advisory lease
/// (`claim_verb = true`, no worktree, pid-based liveness) that the BUG-637 gates
/// pick up exactly like an AIDA-launched lease.
// trace:TASK-957 | ai:claude
#[cfg(test)]
#[path = "tests/task_957_claim_tests.rs"]
mod task_957_claim_tests;

// The global running-work table (`aida ps`). trace:STORY-696 | ai:claude
#[cfg(test)]
#[path = "tests/story_696_ps_tests.rs"]
mod story_696_ps_tests;

// `held_prs` derived from the verdict corpus, so a refusal recorded outside
// `aida review record` is still surfaced. trace:BUG-1773 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1773_corpus_held_prs_tests.rs"]
mod bug_1773_corpus_held_prs_tests;

// The merge chokepoint and the marker/label mirrors derived from the same
// verdict corpus, so a refusal from any producer holds the PR.
// trace:BUG-1774 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1774_corpus_merge_gate_tests.rs"]
mod bug_1774_corpus_merge_gate_tests;

// `aida ps` flags a live seat whose mail identity would fall back to the
// shell user. trace:TASK-1451 | ai:claude
#[cfg(test)]
#[path = "tests/task_1451_mail_identity_ps_tests.rs"]
mod task_1451_mail_identity_ps_tests;

// The pending-approval marker's priority over the heuristic classifier, plus
// `aida ps` / `aida awaiting` end-to-end rendering of a Blocked seat.
// trace:TASK-1454 | ai:claude
#[cfg(test)]
#[path = "tests/task_1454_pending_approval_tests.rs"]
mod task_1454_pending_approval_tests;

// The orphaned-In-Progress detection → `aida awaiting` mapping.
// trace:BUG-1523 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1523_orphaned_in_progress_mapping_tests.rs"]
mod bug_1523_orphaned_in_progress_mapping_tests;

// trace:BUG-1656 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1656_subagent_liveness_tests.rs"]
mod bug_1656_subagent_liveness_tests;

// trace:BUG-1680 | ai:antigravity
#[cfg(test)]
#[path = "tests/bug_1680_salvage_main_tests.rs"]
mod bug_1680_salvage_main_tests;

// trace:BUG-1941 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1941_ps_tests.rs"]
mod bug_1941_ps_tests;

// The three `aida ps` reporting defects: landed work advertised as stalled,
// guessed fan-out ownership, and fan-out rows wearing the parent's role.
// trace:BUG-1681 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1681_ps_reporting_tests.rs"]
mod bug_1681_ps_reporting_tests;

// BUG-1740: `possibly_subagent` must not be corroborated by the reporting
// session, and a harness lease pinned to the project root is not a fan-out.
// trace:BUG-1740 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1740_fanout_self_corroboration_tests.rs"]
mod bug_1740_fanout_self_corroboration_tests;

/// trace:TASK-358 | ai:claude
#[cfg(test)]
#[path = "tests/task_358_escalation_cleanup_tests.rs"]
mod task_358_escalation_cleanup_tests;

/// Resolved flag values after expanding the `aida queue work --drain` alias
/// (TASK-578). `--drain` is pure discoverability sugar: it sets the same
/// internal state that `--auto-complete --no-human=both --max <queue-size>`
/// already produced. Explicit flags always win, so `--drain
/// --no-human=reviewer-only` keeps `reviewer-only`,
/// `--drain --auto-complete=through-ci` keeps the early stop, and an explicit
/// `--max N` caps the drain instead of using the queue size.
/// trace:TASK-578 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainResolution {
    pub(crate) auto_complete: Option<String>,
    pub(crate) no_human: Option<String>,
    pub(crate) max: Option<usize>,
}

/// Expand the `--drain` alias into the underlying auto-complete / no-human / max
/// state. When `drain` is false this is an identity map over the caller's flags.
/// When `drain` is true the unset fields take the drain defaults — full
/// auto-complete, fully-headless (`both`), and a `--max` of the drivable queue
/// size (falling back to 99 when the size is unknown / zero, per the spec). An
/// explicitly-supplied flag is never overwritten. `queue_size` is the number of
/// drivable queued items the caller has already resolved (0 ⇒ unknown). The
/// function is pure so the expansion can be unit-tested without a live queue.
/// trace:TASK-578 | ai:claude
pub(crate) fn resolve_drain_alias(
    drain: bool,
    auto_complete: Option<&str>,
    no_human: Option<&str>,
    max: Option<usize>,
    queue_size: usize,
) -> DrainResolution {
    if !drain {
        return DrainResolution {
            auto_complete: auto_complete.map(str::to_string),
            no_human: no_human.map(str::to_string),
            max,
        };
    }
    DrainResolution {
        auto_complete: Some(auto_complete.unwrap_or("full").to_string()),
        no_human: Some(no_human.unwrap_or("both").to_string()),
        // Explicit --max wins; otherwise bound the drain to the queue size, or
        // fall back to 99 when the size is unknown (the spec's "--max 99").
        max: Some(max.unwrap_or(if queue_size > 0 { queue_size } else { 99 })),
    }
}

/// TASK-84: read `[behavior] permission_mode = "..."` from
/// `.aida/config.toml`. Returns `None` when the file is missing, the
/// section is absent, or the value is empty. We use a hand-rolled
/// section-aware scan (rather than pulling in serde for one optional
/// string) to stay consistent with `read_id_format_settings`.
/// trace:TASK-84 | ai:claude
pub(crate) fn read_behavior_permission_mode(project_dir: &std::path::Path) -> Option<String> {
    let config_path = project_dir.join(".aida").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let mut in_behavior = false;
    for raw in content.lines() {
        // Strip trailing inline comments before any other parsing so a
        // legitimate `permission_mode = "auto"  # default` doesn't end up
        // with the comment glued onto the value. trace:TASK-84 | ai:claude
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_behavior = rest.trim_end_matches(']').trim() == "behavior";
            continue;
        }
        if !in_behavior {
            continue;
        }
        if let Some(rest) = line.strip_prefix("permission_mode") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val.trim().trim_matches('"').trim_matches('\'').to_string();
                if v.is_empty() {
                    return None;
                }
                return Some(v);
            }
        }
    }
    None
}

/// TASK-81: search a lease list for any active lease whose scope matches
/// the requested scope (case-insensitive). Returns the freshest match by
/// `started_at` so the caller has a stable target for `session_end` even
/// if multiple stale leases somehow share a scope. Pure decision helper —
/// kept separate so the unit test in `queue_work_tests` doesn't need to
/// touch disk. trace:TASK-81 | ai:claude
pub(crate) fn find_scope_lease_conflict(
    leases: &[SessionLease],
    scope: &str,
) -> Option<SessionLease> {
    leases
        .iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(scope))
        .max_by_key(|l| l.started_at)
        .cloned()
}

/// BUG-307: gather the three independent liveness signals for `lease` and
/// hand them to [`orchestrator::classify_for_auto_release`]. The classifier
/// is pure; this wrapper is where we touch the process table, lease-file
/// mtime, and `git status --porcelain`. Returns
/// [`orchestrator::AutoReleaseDecision::Live`] when the feature is disabled
/// so callers can branch uniformly. trace:BUG-307 | ai:claude
pub(crate) fn auto_release_decision_for_lease(
    project_root: &std::path::Path,
    lease: &SessionLease,
    config: &orchestrator::OrchestratorConfig,
) -> orchestrator::AutoReleaseDecision {
    // BUG-511: worktree-less review-verb leases are advisory PID locks — no
    // worktree, no uncommitted work to lose — so liveness is the creator
    // process, full stop. Decided BEFORE the config gate and the mtime clock:
    // a dead advisory review lease is always safe to release, a live one
    // always refuses. BUG-882 reviewer worktree leases carry review_verb as
    // metadata but still use the normal worktree cleanup gates below.
    if lease.review_verb && lease.worktree_path.as_os_str().is_empty() {
        let pid_alive = lease
            .creator_pid
            .map(process_probe::pid_is_alive)
            .unwrap_or(false);
        return if pid_alive {
            orchestrator::AutoReleaseDecision::Live
        } else {
            let mtime_age_secs = std::fs::metadata(lease_path(project_root, &lease.id))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            orchestrator::AutoReleaseDecision::SafelyDormant {
                process_dead: true,
                mtime_age_secs,
                worktree_missing: false,
            }
        };
    }

    if !config.auto_release_dormant_leases {
        return orchestrator::AutoReleaseDecision::Live;
    }

    // A worktree that a live process is using (a cwd, an open file, or a
    // program running from it), or one the process scan could not verify, is
    // never dormant, whatever the pid and mtime say: releasing it would tear
    // the tree out from under that process. trace:BUG-1947 | ai:claude
    if aida_core::worktree_holders::check_worktree_removal(&lease.worktree_path).is_err() {
        return orchestrator::AutoReleaseDecision::Live;
    }

    // Signal 1: creator shell still alive? A lease without a recorded
    // `creator_pid` (pre-STORY-73) defaults to "dead" — those leases also
    // pre-date the orchestrator drain and are the canonical leaked-lease
    // case, so the auto-release path catches them.
    // BUG-1111: process-backed headless leases use `active_pid` as the
    // authoritative child liveness signal; `creator_pid` may be the still-live
    // orchestrator that is trying to reclaim the dead child.
    // trace:BUG-1111 | ai:codex
    let pid_alive = lease
        .active_pid
        .or(lease.creator_pid)
        .map(process_probe::pid_is_alive)
        .unwrap_or(false);

    // Signal 2: lease-file mtime age. We use the filesystem mtime rather
    // than `started_at` because future writers (mark_lease_escalated_to_human
    // etc.) bump the mtime — the file's freshness is the authoritative
    // "this lease was touched recently" signal.
    let lease_file = lease_path(project_root, &lease.id);
    let mtime_age_secs = std::fs::metadata(&lease_file)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() as i64)
        // No mtime readable → treat as old (the lease was just listed from
        // disk, so this only fires under genuine fs weirdness; failing
        // "stale" lets the cleanup proceed).
        .unwrap_or(i64::MAX);

    // Signal 3: live `claude` running inside the worktree. Reuses the same
    // probe `aida session leases --all` uses for the ● live glyph so the
    // classifier agrees with what the operator sees in `session leases`.
    let live_claude_in_worktree = process_probe::probe_live_claude_sessions().iter().any(|s| {
        !s.stale_cwd && (s.cwd == lease.worktree_path || s.cwd.starts_with(&lease.worktree_path))
    });

    let worktree_exists = lease.worktree_path.exists();
    let worktree_dirty_count = if worktree_exists {
        worktree_dirty_entries(&lease.worktree_path).len()
    } else {
        0
    };

    orchestrator::classify_for_auto_release(
        pid_alive,
        mtime_age_secs,
        live_claude_in_worktree,
        worktree_exists,
        worktree_dirty_count,
        config.stale_lease_threshold_minutes,
    )
}

/// BUG-777: the recovery picture for one same-scope lease conflict — the
/// verdict plus the evidence a refusal message needs to be *evaluable*
/// ("work there is lost unless committed" is useless if the operator has to
/// go run `git status` in that worktree to find out whether it applies).
// trace:BUG-777 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct StaleLeaseRecoveryReport {
    pub(crate) verdict: StaleLeaseRecovery,
    /// `git status --porcelain` lines in the lease's worktree — empty when the
    /// tree is clean, missing, or the lease is worktree-less (advisory lock).
    pub(crate) dirty: Vec<String>,
    pub(crate) worktree_exists: bool,
    /// Why the worktree is occupied by a live process (or could not be
    /// verified free), when it is. Forces the verdict to `Live`.
    // trace:BUG-1947 | ai:claude
    pub(crate) in_use: Option<String>,
    /// The verdict before the in-use override: what the lease's own owner
    /// liveness and worktree state say. Only non-destructive recovery that
    /// keeps the worktree untouched (the implementer retry reusing the same
    /// tree) may act on it; anything that removes or resets a tree must use
    /// `verdict`.
    // trace:BUG-1947 | ai:claude
    pub(crate) base_verdict: StaleLeaseRecovery,
}

impl StaleLeaseRecoveryReport {
    /// One human clause describing what is (or is not) at risk in the
    /// conflicting lease's worktree — the fix for "work there is lost unless
    /// committed", which the operator previously could not assess.
    // trace:BUG-777 | ai:claude
    pub(crate) fn worktree_state_phrase(&self) -> String {
        if !self.worktree_exists {
            return "worktree GONE (nothing to lose)".to_string();
        }
        if let Some(why) = &self.in_use {
            return format!("worktree {why}"); // trace:BUG-1947 | ai:claude
        }
        if self.dirty.is_empty() {
            return "worktree CLEAN (no uncommitted changes)".to_string();
        }
        format!(
            "worktree DIRTY ({} uncommitted change(s))",
            self.dirty.len()
        )
    }

    /// A short, bounded rendering of the uncommitted entries so a refusal can
    /// name exactly what is unsaved rather than just counting it.
    // trace:BUG-777 | ai:claude
    pub(crate) fn dirty_sample(&self) -> String {
        const MAX: usize = 5;
        let shown: Vec<&str> = self.dirty.iter().take(MAX).map(|s| s.trim()).collect();
        if self.dirty.len() > MAX {
            format!("{}, … (+{} more)", shown.join("; "), self.dirty.len() - MAX)
        } else {
            shown.join("; ")
        }
    }
}

/// BUG-777: gather the recovery picture for a blocking lease.
///
/// Liveness comes from [`lease_state_for`] — the SAME classifier `aida ps` and
/// `aida status <spec>` render as `● live` / `⚠ STALE` — plus the lease's own
/// recorded pids, so this surface can never disagree with what the operator
/// just saw in `aida ps`. The dirty/clean reading is the same
/// `git status --porcelain` probe the BUG-307 auto-release gate uses.
// trace:BUG-777 | ai:claude
pub(crate) fn stale_lease_recovery_for_lease(lease: &SessionLease) -> StaleLeaseRecoveryReport {
    // BUG-1111: `lease_owner_process_gone` treats an `active_pid` vendor child
    // as authoritative over the creator/orchestrator pid.
    // trace:BUG-1111 | ai:codex
    let owner_gone = lease_owner_process_gone(
        lease.active_pid,
        lease.active_pid_start_time.as_deref(),
        lease.creator_pid,
        lease.creator_pid_start_time.as_deref(),
        process_probe::process_identity_is_alive,
    );
    stale_lease_recovery_with_owner(lease, owner_gone)
}

/// BUG-1930: was this lease minted by a phase child of `orchestrator_pid`
/// that has already returned? `aida queue work` creates the phase-1 lease
/// in-process, so its `creator_pid` is the drain orchestrator, which is still
/// alive when it retries. A claude-vendor launch records no `active_pid`, so
/// without this the predecessor of a cleanly exited attempt reads as live on
/// every probe. Only the retry path may ask: it runs after the orchestrator
/// has reaped that child. A recorded start time must match, so a recycled
/// pid never qualifies.
// trace:BUG-1930 | ai:claude
pub(crate) fn lease_minted_by_returned_phase_child(
    lease: &SessionLease,
    orchestrator_pid: u32,
    orchestrator_start_time: Option<&str>,
) -> bool {
    lease.active_pid.is_none()
        && lease.creator_pid == Some(orchestrator_pid)
        && match (
            lease.creator_pid_start_time.as_deref(),
            orchestrator_start_time,
        ) {
            (Some(recorded), Some(current)) => recorded == current,
            _ => true,
        }
}

/// BUG-1930: the recovery picture for a phase-1 retry's predecessor lease.
/// Same as [`stale_lease_recovery_for_lease`], except that a lease this
/// orchestrator minted through a phase child that has since returned counts
/// as owner-gone. A live claude still in the worktree keeps it live.
// trace:BUG-1930 | ai:claude
pub(crate) fn implementer_retry_predecessor_recovery(
    lease: &SessionLease,
    orchestrator_pid: u32,
) -> StaleLeaseRecoveryReport {
    let orchestrator_start = process_probe::process_start_identity(orchestrator_pid);
    if lease_minted_by_returned_phase_child(lease, orchestrator_pid, orchestrator_start.as_deref())
    {
        return stale_lease_recovery_with_owner(lease, Some(true));
    }
    stale_lease_recovery_for_lease(lease)
}

fn stale_lease_recovery_with_owner(
    lease: &SessionLease,
    owner_gone: Option<bool>,
) -> StaleLeaseRecoveryReport {
    let live = process_probe::probe_live_claude_sessions();
    let lease_state = lease_state_for(lease, &live, chrono::Utc::now());
    // A worktree-less advisory lease (review / claim verb, or the TASK-474
    // empty-path MCP claim) has no tree to inspect — treat it as "missing",
    // which is exactly right: there is no uncommitted work to strand.
    let worktree_exists =
        !lease.worktree_path.as_os_str().is_empty() && lease.worktree_path.exists();
    let dirty = if worktree_exists {
        worktree_dirty_entries(&lease.worktree_path)
    } else {
        Vec::new()
    };
    let base_verdict =
        classify_stale_lease_recovery(lease_state, owner_gone, worktree_exists, dirty.len());
    let mut verdict = base_verdict.clone();
    // A tree a live process is using (or one the scan could not verify) is
    // occupied, so it is never recoverable, even when the recorded owner is
    // dead. trace:BUG-1947 | ai:claude
    let in_use = if worktree_exists {
        aida_core::worktree_holders::check_worktree_removal(&lease.worktree_path)
            .err()
            .map(|e| e.summary())
    } else {
        None
    };
    if in_use.is_some() {
        verdict = StaleLeaseRecovery::Live;
    }
    StaleLeaseRecoveryReport {
        verdict,
        dirty,
        worktree_exists,
        in_use,
        base_verdict,
    }
}

/// BUG-906: before a transient phase retry relaunches a child session, reap any
/// predecessor lease on the same scope whose owner process is verifiably gone.
/// Clean/missing worktrees are safe to release; dirty worktrees become a typed,
/// shelvable `cache-locked` failure so the drain parks with the exact worktree
/// that needs human salvage instead of burning the retry on its own leftover
/// lease.
// trace:BUG-906 | ai:codex
pub(crate) fn release_dead_phase_predecessor_leases(
    project_root: &std::path::Path,
    scope: &str,
    phase: auto_complete::Phase,
) -> Result<(), auto_complete::PhaseFailure> {
    let mut remaining = 16usize;
    while remaining > 0 {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            return Ok(());
        };
        let report = stale_lease_recovery_for_lease(&conflict);
        match report.verdict {
            StaleLeaseRecovery::ReclaimableClean { .. } => {
                if !force_cleanup_lease(project_root, &conflict) {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::CacheLocked,
                        format!(
                            "phase {} retry found dead predecessor lease {} on `{scope}`, \
                             but auto-release did not finish; worktree: {}",
                            phase.index(),
                            &conflict.id[..conflict.id.len().min(8)],
                            conflict.worktree_path.display(),
                        ),
                    ));
                }
                eprintln!(
                    "  {} released dead predecessor lease {} on `{scope}` before retrying phase {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    phase.index()
                );
                remaining -= 1;
            }
            StaleLeaseRecovery::StaleDirty { dirty_entries } => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CacheLocked,
                    format!(
                        "phase {} retry found dead predecessor lease {} on `{scope}`, \
                         but its worktree is dirty ({dirty_entries} uncommitted change(s)); \
                         worktree: {}; changes: {}",
                        phase.index(),
                        &conflict.id[..conflict.id.len().min(8)],
                        conflict.worktree_path.display(),
                        report.dirty_sample(),
                    ),
                ));
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness => return Ok(()),
        }
    }
    Err(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::CacheLocked,
        format!(
            "phase {} retry found too many predecessor leases on `{scope}`; \
             release stale leases manually with `aida session leases` / `aida session end`",
            phase.index()
        ),
    ))
}

pub(crate) fn remove_session_lease_record_only(
    project_root: &std::path::Path,
    lease_id: &str,
) -> bool {
    let lease_file = lease_path(project_root, lease_id);
    // trace:BUG-2139 | ai:claude
    crate::session::revoke_lease_child_grant(&lease_file);
    let removed = match std::fs::remove_file(&lease_file) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    let manifest = session_manifest::manifest_path(project_root, lease_id);
    if manifest.exists() {
        let _ = std::fs::remove_file(manifest);
    }
    let activity = session_activity_path(project_root, lease_id);
    if activity.exists() {
        let _ = std::fs::remove_file(activity);
    }
    removed
}

pub(crate) const IMPLEMENTER_RETRY_PREDECESSOR_REPROBES: usize = 3;

pub(crate) const IMPLEMENTER_RETRY_PREDECESSOR_REPROBE_DELAY: std::time::Duration =
    std::time::Duration::from_secs(5);

/// BUG-908: a phase-1 transient retry is continuation, not takeover. Release
/// the dead predecessor lease even when its worktree is dirty, then relaunch
/// the implementer against that same branch/path with force-claim. The dirty
/// tree is the attempt-1 work product; `--steal` is never involved.
// trace:BUG-908 BUG-1078 | ai:codex
// trace:BUG-1930 | ai:claude
pub(crate) fn reclaim_implementer_retry_predecessor(
    project_root: &std::path::Path,
    scope: &str,
) -> Result<Option<(String, std::path::PathBuf, String)>, auto_complete::PhaseFailure> {
    reclaim_implementer_retry_predecessor_with(
        project_root,
        scope,
        IMPLEMENTER_RETRY_PREDECESSOR_REPROBES,
        IMPLEMENTER_RETRY_PREDECESSOR_REPROBE_DELAY,
        |lease| implementer_retry_predecessor_recovery(lease, std::process::id()),
        std::thread::sleep,
    )
}

// trace:BUG-1078 | ai:codex
pub(crate) fn reclaim_implementer_retry_predecessor_with(
    project_root: &std::path::Path,
    scope: &str,
    reprobes: usize,
    reprobe_delay: std::time::Duration,
    mut recovery_for_lease: impl FnMut(&SessionLease) -> StaleLeaseRecoveryReport,
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<Option<(String, std::path::PathBuf, String)>, auto_complete::PhaseFailure> {
    let mut live_or_unknown_seen = 0usize;
    loop {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            return Ok(None);
        };
        let report = recovery_for_lease(&conflict);
        // This retry only drops the lease RECORD and relaunches into the same
        // worktree — nothing is removed or reset — so a live process still in
        // the tree (an orphaned `cargo test` from attempt 1) must not block it.
        // Judge the predecessor by its own owner liveness, not the in-use
        // override. trace:BUG-1947 | ai:claude
        match report.base_verdict {
            StaleLeaseRecovery::ReclaimableClean { .. } | StaleLeaseRecovery::StaleDirty { .. } => {
                if !remove_session_lease_record_only(project_root, &conflict.id) {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LeaseConflict,
                        format!(
                            "phase 1 retry could not release predecessor lease {} on `{scope}`; \
                             worktree: {}",
                            &conflict.id[..conflict.id.len().min(8)],
                            conflict.worktree_path.display(),
                        ),
                    ));
                }
                return Ok(Some((
                    conflict.branch.clone(),
                    conflict.worktree_path.clone(),
                    conflict.id.clone(),
                )));
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness
                if live_or_unknown_seen < reprobes =>
            {
                live_or_unknown_seen += 1;
                sleep(reprobe_delay);
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::LeaseConflict,
                    format!(
                        "phase 1 retry cannot reclaim predecessor lease {} on `{scope}` after \
                         {} liveness re-probe(s); worktree: {}",
                        &conflict.id[..conflict.id.len().min(8)],
                        reprobes,
                        conflict.worktree_path.display(),
                    ),
                ));
            }
        }
    }
}

/// BUG-438: on `--resume-drain`, proactively release the crashed orchestrator's
/// lease(s) on `scope` whose creator process is dead — *independent of the
/// staleness clock*. The time-based auto-release ([`auto_release_decision_for_lease`])
/// keeps a dead-PID lease "Live" while its mtime is still fresh, so a **fast**
/// resume (within `stale_lease_threshold_minutes`) collides with the crashed
/// implementer's lease when the reviewer phase resolves PR→spec. We have already
/// passed the PID-liveness double-drive gate, so the original orchestrator is
/// dead — its child leases are too. We force `threshold = 0` so a dead-PID
/// *clean-worktree* lease releases regardless of mtime, while a **dirty**
/// worktree still classifies `DormantDirty` and is left untouched (no
/// uncommitted work is lost — the operator handles it). trace:BUG-438 | ai:claude
pub(crate) fn release_dead_leases_for_resume(project_root: &std::path::Path, scope: &str) {
    let mut cfg = orchestrator::OrchestratorConfig::load(project_root);
    // Resume is a deliberate recovery — release the crashed lease even if the
    // project disabled the dormant-lease auto-release, and ignore the mtime
    // clock (PID-death is the authoritative signal here).
    cfg.auto_release_dormant_leases = true;
    cfg.stale_lease_threshold_minutes = 0;
    let mut remaining = 16usize; // defense-in-depth bound, mirrors session_start
    while remaining > 0 {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            break;
        };
        match auto_release_decision_for_lease(project_root, &conflict, &cfg) {
            orchestrator::AutoReleaseDecision::SafelyDormant { .. } => {
                eprintln!(
                    "  {} released the crashed drain's lease {} on `{}` (process dead)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    scope
                );
                let _ = force_cleanup_lease(project_root, &conflict);
                remaining -= 1;
            }
            // Live (process somehow still alive — won't happen post-gate) or
            // DormantDirty (uncommitted work) → leave it; the reviewer phase's
            // own conflict path / the operator handles those cases.
            _ => break,
        }
    }
}

/// BUG-307: format a seconds-old duration into the short human form used in
/// the auto-release log line (`"2h"`, `"45m"`, `"30s"`). Distinct from
/// [`humanize_relative`] — that one takes a chrono datetime and appends
/// " ago"; here the caller controls the surrounding text.
pub(crate) fn humanize_secs_short(secs: i64) -> String {
    if secs < 0 {
        return "0s".to_string();
    }
    let secs = secs as u64;
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    format!("{}d", days)
}
