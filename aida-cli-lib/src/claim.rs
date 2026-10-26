//! `aida claim` / `aida unclaim` — operator spec-scoped claim leases (STORY-1488 slice 1).
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-957: resolve a `<spec>` argument (SPEC-ID, agreed-id, or UUID) to the
/// matching requirement, loading the store read-only. Shared by `aida claim`
/// and `aida unclaim`.
// trace:TASK-957 | ai:claude
pub(crate) fn resolve_spec_for_claim(spec: &str) -> Result<(std::path::PathBuf, Requirement)> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            project_root.display()
        )
    })?;
    let want = spec.trim();
    let want_uc = want.to_ascii_uppercase();
    let req = store
        .requirements
        .iter()
        .find(|r| {
            // UUID match (the form `aida show` prints), or any id form.
            r.id.to_string().eq_ignore_ascii_case(want)
                || [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|s| s.eq_ignore_ascii_case(&want_uc))
        })
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!("no spec found matching `{spec}` — check the ID with `aida list`.")
        })?;
    Ok((project_root, req))
}

/// The canonical display id (agreed id preferred, else spec id) used as a
/// claim's scope so it matches the BUG-637 gates exactly like an AIDA-launched
/// spec lease.
// trace:TASK-957 | ai:claude
pub(crate) fn claim_scope_id(req: &Requirement) -> String {
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string())
}

/// True iff `lease` is a TASK-957 claim minted by THIS caller for `scope` — same
/// scope (case-insensitive), same advisory-claim kind, and the same creator pid.
/// Used for idempotency (re-claim = refresh) and for `aida unclaim` (remove only
/// the caller's own claim).
// trace:TASK-957 | ai:claude
pub(crate) fn is_own_claim(lease: &SessionLease, scope: &str, my_pid: Option<u32>) -> bool {
    lease.claim_verb && lease.scope.eq_ignore_ascii_case(scope) && lease.creator_pid == my_pid
}

/// BUG-2143: the path an `aida claim --worktree <P>` lease records. A blank
/// `P` records none; anything else is canonicalized when it exists and kept
/// verbatim otherwise. The result is INFORMATIONAL only — a claim lease is
/// advisory and is never re-entered as a worktree, whatever it names.
// trace:BUG-2143 | ai:claude
pub(crate) fn claim_worktree_path(worktree: Option<&str>) -> std::path::PathBuf {
    worktree
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| {
            let p = std::path::PathBuf::from(w);
            p.canonicalize().unwrap_or(p)
        })
        .unwrap_or_default()
}

/// BUG-2143: the leases the BUG-637 pickup gate weighs for `scope` — every
/// lease except the caller's OWN `aida claim` on it, so `aida claim <spec>`
/// then `aida worktree add <spec>` from one shell never self-blocks.
// trace:BUG-2143 | ai:claude
pub(crate) fn without_own_claims(
    leases: Vec<SessionLease>,
    scope: &str,
    my_pid: Option<u32>,
) -> Vec<SessionLease> {
    leases
        .into_iter()
        .filter(|l| !is_own_claim(l, scope, my_pid))
        .collect()
}

/// BUG-2143: the branch a claim lease records — the claimed worktree's own
/// branch, or empty when the claim names no worktree (or names the primary
/// checkout). Never the primary checkout's branch: a worktree-less claim has
/// no branch of its own.
// trace:BUG-2143 | ai:claude
pub(crate) fn claim_lease_branch(
    worktree_path: &std::path::Path,
    project_root: &std::path::Path,
) -> String {
    if worktree_path.as_os_str().is_empty() {
        return String::new();
    }
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    if canon(worktree_path) == canon(project_root) {
        return String::new();
    }
    current_branch_at(worktree_path).unwrap_or_default()
}

/// The role a claim records. Inside a lane worktree whose lease names a role,
/// that role: the lease is the lane's identity, and a `claude --bg` lane's
/// environment is the daemon's (often the advisor's). A lane lease whose role
/// does not validate fails closed: no role, never the inherited seat
/// (TASK-2214). Elsewhere, the seat of the caller's validated grant.
// trace:STORY-1662 | ai:claude
pub(crate) fn claim_role(
    project_root: &std::path::Path,
    cwd: Option<&std::path::Path>,
) -> Option<String> {
    match cwd.and_then(|cwd| crate::lane_env::lane_lease_for_cwd(project_root, cwd)) {
        Some(lease) => match crate::lane_env::lease_role(&lease) {
            Ok(Some(role)) => Some(role),
            Ok(None) => crate::gate::seat_authority::current_seat(project_root),
            Err(e) => {
                eprintln!("warning: {e:#}; the claim records no role");
                None
            }
        },
        None => crate::gate::seat_authority::current_seat(project_root),
    }
}

/// `aida claim <spec>` — record a spec-scoped advisory CLAIM so advisor-fanned
/// work (a Claude Agent-tool subagent, which otherwise takes only a generic
/// `harness-worktree` lease) is visible to the BUG-637 duplicate-dispatch gates.
///
/// Writes a lightweight [`SessionLease`] whose scope IS the spec id, with no
/// worktree of its own — its liveness signal is the claiming process recorded in
/// `creator_pid` (the [`lease_state_for`] advisory-lock path, shared with
/// BUG-511 review leases). Because the scope matches the spec id, the claim is
/// picked up by [`live_spec_claim_by_other`] (pre-pickup),
/// [`lease_owning_spec`] (pre-edit), and [`spec_scoped_lease`]
/// (`aida status <spec>` / `aida ps`) exactly like an AIDA-launched lease.
///
/// Idempotent: re-claiming a spec this caller already claims is a no-op refresh
/// (it rewrites the same lease with a fresh `started_at`).
// trace:TASK-957 | ai:claude
pub(crate) fn handle_claim(spec: &str, worktree: Option<&str>) -> Result<()> {
    let (project_root, req) = resolve_spec_for_claim(spec)?;
    let scope = claim_scope_id(&req);
    let my_pid = creator_shell_pid();

    // Idempotency: if THIS caller already holds a live claim for this scope,
    // refresh it in place rather than minting a second lease. trace:TASK-957
    let leases = list_leases(&project_root);
    let existing = leases.iter().find(|l| is_own_claim(l, &scope, my_pid));

    let worktree_path = claim_worktree_path(worktree);

    // BUG-2143: the branch is the CLAIMED WORKTREE's branch, or none. Reading
    // it off the project root stamped the primary checkout's `main` on every
    // worktree-less claim, which `aida session reap` then protected forever.
    let branch = claim_lease_branch(&worktree_path, &project_root);
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());

    let cwd = std::env::current_dir().ok();
    let role = claim_role(&project_root, cwd.as_deref());
    let id = match existing {
        Some(l) => l.id.clone(),
        None => uuid::Uuid::now_v7().to_string().replace('-', "")[..12].to_string(),
    };
    let refreshed = existing.is_some();

    let lease = SessionLease {
        tree_kind: Default::default(),
        id: id.clone(),
        scope: scope.clone(),
        slug: slugify(&scope),
        owner,
        worktree_path,
        branch,
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        // trace:TASK-1593 | ai:antigravity
        role: role.clone(),
        creator_pid: my_pid,
        creator_pid_start_time: my_pid.and_then(process_probe::process_start_identity),
        active_pid: None,
        active_pid_start_time: None,
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
        claim_verb: true,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: aida_core::driver_registry::driver_from_env(), // trace:STORY-1653 | ai:claude
        child_grant_id: None,
    };

    std::fs::create_dir_all(leases_dir(&project_root))?;
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    let path = lease_path(&project_root, &id);
    let bytes = toml::to_string_pretty(&lease)?;
    aida_core::write_atomic(&path, &bytes)?;
    // trace:BUG-2215 | ai:codex
    finish_claim_write(
        &path,
        bytes.as_bytes(),
        refreshed,
        persist_claim_status(&project_root, &req, &lease.owner),
    )?;
    // BUG-1918: durable record of who claimed this spec, surviving release.
    // trace:BUG-1918 | ai:claude
    crate::gate::review_authority::record_authoring_lease(&project_root, &lease);

    let id_short: String = id.chars().take(8).collect();
    if refreshed {
        println!(
            "{} refreshed claim on {} (session {})",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            scope.cyan().bold(),
            id_short.dimmed(),
        );
    } else {
        println!(
            "{} claimed {} (session {}) — a fresh `aida queue work {}` from another \
             session will now refuse, and `aida edit {}` will warn.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            scope.cyan().bold(),
            id_short.dimmed(),
            scope,
            scope,
        );
        println!(
            "  {}",
            format!("release it with `aida unclaim {}`", scope).dimmed()
        );
    }
    Ok(())
}

/// `aida unclaim <spec>` — remove THIS caller's spec-scoped claim (matched by
/// scope + creator pid), so a fresh `aida queue work <spec>` / `aida edit
/// <spec>` no longer sees it. Removing a claim the caller doesn't hold is a
/// no-op (not an error).
// trace:TASK-957 | ai:claude
pub(crate) fn handle_unclaim(spec: &str) -> Result<()> {
    let (project_root, req) = resolve_spec_for_claim(spec)?;
    let scope = claim_scope_id(&req);
    let my_pid = creator_shell_pid();

    let leases = list_leases(&project_root);
    let mine: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| is_own_claim(l, &scope, my_pid))
        .collect();

    if mine.is_empty() {
        println!(
            "{} no claim by this session on {} — nothing to release.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            scope.cyan(),
        );
        return Ok(());
    }

    let mut removed = 0usize;
    for l in &mine {
        let path = lease_path(&project_root, &l.id);
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
    // Only a confirmed absence of a PR permits returning work to Approved.
    // A failed forge lookup retains In Progress, as does another live lease.
    // trace:BUG-2215 | ai:codex
    let others = list_leases(&project_root);
    let live_sessions = process_probe::probe_live_claude_sessions();
    let held = others.iter().any(|l| {
        l.scope.eq_ignore_ascii_case(&scope)
            && matches!(
                lease_state_for(l, &live_sessions, chrono::Utc::now()),
                LeaseState::Live
            )
    });
    if !held && req.status == RequirementStatus::InProgress {
        let lookup = detect_open_pr_for_spec(&project_root, &scope);
        persist_unclaim_status(&project_root, &req, &lookup)?;
    }
    println!(
        "{} released claim on {} ({} lease{} removed)",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        scope.cyan().bold(),
        removed,
        if removed == 1 { "" } else { "s" },
    );
    Ok(())
}

// A failed invocation must not leave a newly acquired claim behind. Never
// delete a preexisting refresh or a lease changed since our write.
// trace:BUG-2215 | ai:codex
fn finish_claim_write(
    path: &std::path::Path,
    bytes: &[u8],
    refreshed: bool,
    result: Result<bool>,
) -> Result<()> {
    if let Err(error) = result {
        if !refreshed && std::fs::read(path).ok().as_deref() == Some(bytes) {
            std::fs::remove_file(path).with_context(|| {
                format!(
                    "claim status failed ({error:#}); removing new claim {}",
                    path.display()
                )
            })?;
        }
        return Err(error);
    }
    Ok(())
}

// trace:BUG-2215 | ai:codex
fn claim_status(req: &mut Requirement, claimant: &str) -> bool {
    let bumped = approved_to_in_progress_bump(req, aida_core::conflict::SESSION_START_AUTHOR);
    if matches!(req.status, RequirementStatus::InProgress) && req.owner != claimant {
        req.owner = claimant.to_string();
        req.modified_at = chrono::Utc::now();
    }
    bumped
}

// Re-read the target under the store writer lock; a concurrent completion or
// deliberate status edit must not be overwritten by the earlier lookup.
// trace:BUG-2215 | ai:codex
pub(crate) fn persist_claim_status(
    project_root: &std::path::Path,
    target: &Requirement,
    claimant: &str,
) -> Result<bool> {
    Ok(persist_claim_status_with_receipt(project_root, target, claimant)?.bumped)
}

// Keep the locked before/after values for a failed launch's conditional undo.
// trace:BUG-2215 TASK-2326 | ai:codex
pub(crate) struct ClaimStatusChange {
    before: Requirement,
    after: Requirement,
    pub(crate) bumped: bool,
}

impl ClaimStatusChange {
    pub(crate) fn changed(&self) -> bool {
        self.before.status != self.after.status || self.before.owner != self.after.owner
    }
}

// trace:BUG-2215 TASK-2326 | ai:codex
pub(crate) fn persist_claim_status_with_receipt(
    project_root: &std::path::Path,
    target: &Requirement,
    claimant: &str,
) -> Result<ClaimStatusChange> {
    let storage = claim_storage(project_root)?;
    let mut before = None;
    let mut bumped = false;
    let after = storage
        .update_spec_atomically(target, |req| {
            before = Some(req.clone());
            bumped = claim_status(req, claimant);
        })?
        .context("claimed requirement disappeared before status write")?;
    Ok(ClaimStatusChange {
        before: before.context("claim write did not read its target")?,
        after,
        bumped,
    })
}

// Restore only the values this write still owns. A later owner/status/edit
// retains its state; never save a pre-launch whole-store snapshot.
// trace:BUG-2215 TASK-2326 | ai:codex
pub(crate) fn rollback_claim_status(
    root: &std::path::Path,
    change: &ClaimStatusChange,
) -> Result<bool> {
    let mut restored = false;
    claim_storage(root)?.update_spec_atomically(&change.after, |req| {
        if !change.changed()
            || req.status != change.after.status
            || req.owner != change.after.owner
            || req.modified_at != change.after.modified_at
        {
            return;
        }
        if req.status != change.before.status {
            aida_core::conflict::set_status_recorded(
                req,
                change.before.status.clone(),
                aida_core::conflict::SESSION_START_AUTHOR,
            );
        }
        req.owner = change.before.owner.clone();
        req.modified_at = chrono::Utc::now();
        restored = true;
    })?;
    Ok(restored)
}

// trace:BUG-2215 | ai:codex
fn claim_storage(project_root: &std::path::Path) -> Result<Storage> {
    let path = match detect_distributed_store_from(project_root) {
        Some(path) => path,
        None => aida_core::resolve_requirements_path_in(
            project_root,
            None,
            None,
            std::path::Path::new(""),
        )?,
    };
    Ok(Storage::new(path))
}

// trace:BUG-2215 | ai:codex
fn persist_unclaim_status(
    project_root: &std::path::Path,
    target: &Requirement,
    pr: &PrLookup,
) -> Result<()> {
    let live_sessions = process_probe::probe_live_claude_sessions();
    let scope = claim_scope_id(target);
    claim_storage(project_root)?.update_spec_atomically(target, |req| {
        // A new claim can arrive during the forge lookup. Its locked write
        // must not be demoted by the earlier unclaim's stale observation.
        // trace:BUG-2215 | ai:codex
        if req.owner != target.owner || req.modified_at != target.modified_at {
            return;
        }
        let held = list_leases(project_root).iter().any(|lease| {
            lease.scope.eq_ignore_ascii_case(&scope)
                && ((lease.claim_verb
                    && lease.creator_pid.is_some_and(|pid| {
                        process_probe::process_identity_is_alive(
                            pid,
                            lease.creator_pid_start_time.as_deref(),
                        )
                    }))
                    || matches!(
                        lease_state_for(lease, &live_sessions, chrono::Utc::now()),
                        LeaseState::Live
                    ))
        });
        if !held {
            unclaim_status(req, pr);
        }
    })?;
    Ok(())
}

// trace:BUG-2215 | ai:codex
fn unclaim_status(req: &mut Requirement, pr: &PrLookup) {
    if req.status == RequirementStatus::InProgress && matches!(pr, PrLookup::NoOpenPr) {
        aida_core::conflict::set_status_recorded(
            req,
            RequirementStatus::Approved,
            aida_core::conflict::SESSION_START_AUTHOR,
        );
        req.modified_at = chrono::Utc::now();
    }
}

/// Record a spec-scoped CLAIM so advisor-fanned work is visible to the
/// duplicate-dispatch gates.
///
/// AIDA-launched work (`aida queue work`, `aida agent new --spec`) writes a
/// spec-scoped session lease, so a second pickup of the same spec is
/// refused. But when an advisor fans an implementer via the Claude Agent
/// tool (a worktree-isolated subagent), that work takes only a generic
/// `harness-worktree` lease — NOT spec-scoped — so the gate can't see it.
/// `aida claim <spec>` closes that gap: it writes a lightweight advisory
/// lease whose scope IS the spec id, keyed to the calling session's pid, so
/// the same gate now refuses a duplicate `aida queue work <spec>` and warns
/// on `aida edit <spec>`.
///
/// Liveness-aware: the claim only holds while the claiming process is alive;
/// a dead claimer's claim is ignored (no crash-deadlock). Idempotent —
/// re-claiming your own spec is a no-op refresh. Clear it with
/// `aida unclaim <spec>`. Surfaces in `aida status <spec>` and `aida ps`.
// trace:TASK-957 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "claim")]
pub(crate) struct ClaimArgs {
    /// SPEC-ID, agreed-id, or UUID to claim.
    pub spec: String,

    /// Optional worktree path to record on the claim (where the fanned
    /// implementer is working). Informational; the claim's liveness is the
    /// claiming process, not this path.
    #[clap(long, value_name = "PATH")]
    pub worktree: Option<String>,
}

// TASK-957: `aida claim` / `aida unclaim` self-load the store read-only to
// resolve the spec id forms, then write/remove a lightweight advisory lease
// under `.aida/sessions/`. Like `aida ps` / `aida status <spec>` they touch
// only the local lease dir + a cache-backed lookup, so they need no shared
// storage handle — dispatch early. trace:TASK-957 | ai:claude
fn run_claim(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <ClaimArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let ClaimArgs { spec, worktree } = &args;
    return handle_claim(spec, worktree.as_deref());
}

// trace:STORY-1728 | ai:claude
pub(crate) static CLAIM: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "claim",
        help_after: "undefer",
        command: <ClaimArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_claim),
    };

/// Inverse of `aida claim` — removes the caller's spec-scoped claim so a
/// fresh `aida queue work <spec>` / `aida edit <spec>` no longer sees it.
// trace:TASK-957 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "unclaim")]
pub(crate) struct UnclaimArgs {
    /// SPEC-ID, agreed-id, or UUID to unclaim.
    pub spec: String,
}

fn run_unclaim(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <UnclaimArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let UnclaimArgs { spec } = &args;
    return handle_unclaim(spec);
}

// trace:STORY-1728 | ai:claude
pub(crate) static UNCLAIM: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "unclaim",
        help_after: "claim",
        command: <UnclaimArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_unclaim),
    };

#[cfg(test)]
#[path = "tests/bug_2215_claim_status_tests.rs"]
mod status_tests;
