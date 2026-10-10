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

    let worktree_path = worktree
        .map(|w| {
            let p = std::path::PathBuf::from(w);
            p.canonicalize().unwrap_or(p)
        })
        .unwrap_or_default();

    let branch = current_branch_at(&project_root).unwrap_or_default();
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());

    let id = match existing {
        Some(l) => l.id.clone(),
        None => uuid::Uuid::now_v7().to_string().replace('-', "")[..12].to_string(),
    };
    let refreshed = existing.is_some();

    let lease = SessionLease {
        id: id.clone(),
        scope: scope.clone(),
        slug: slugify(&scope),
        owner,
        worktree_path,
        branch,
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        // trace:TASK-1593 | ai:antigravity
        role: crate::seat_authority::current_seat(&project_root),
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
    };

    std::fs::create_dir_all(leases_dir(&project_root))?;
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(
        &lease_path(&project_root, &id),
        toml::to_string_pretty(&lease)?,
    )?;

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
    println!(
        "{} released claim on {} ({} lease{} removed)",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        scope.cyan().bold(),
        removed,
        if removed == 1 { "" } else { "s" },
    );
    Ok(())
}
