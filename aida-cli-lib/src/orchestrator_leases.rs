//! Orchestrated phase-1 workspace, lease receipts and child refusals, implementer phase args, implementer branch push and orchestrator PR open (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// BUG-1629: the workspace phase 1 launches. `fresh` marks a branch the
/// pickup resolver chose as free that has not been launched yet; see
/// [`RealPhaseDriver::phase1_launch_workspace`].
// trace:BUG-1629 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Phase1Workspace {
    pub(crate) worktree: std::path::PathBuf,
    pub(crate) branch: String,
    pub(crate) fresh: bool,
}

/// BUG-1629: a freshly resolved branch that exists at launch was created by
/// someone else after resolution.
// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_fresh_branch_raced(fresh: bool, branch_exists_now: bool) -> bool {
    fresh && branch_exists_now
}

/// BUG-1629: exactly one clean replacement launch per phase-1 run.
// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_lost_child_retry_allowed(retries_used: usize) -> bool {
    retries_used == 0
}

/// BUG-1769: is this spec's work finished? `Done` means committed on a branch
/// and `Completed` means landed on the default branch, so both say the phase's
/// goal was reached. `Rejected` / `Superseded` / `NeedsAttention` are terminal
/// or parked but are NOT finished work, and reading them as success would turn
/// a declined spec into a shipped one.
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_status_is_finished(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Done | RequirementStatus::Completed
    )
}

/// BUG-1769: did THIS run finish the work? The discriminator is BUG-1524's own
/// before-and-after principle applied to spec status: an advance counts only
/// when the entry reading was not already finished, so a spec that was `Done`
/// *before* phase 1 began is still refused exactly as it is today.
///
/// Either reading being absent is UNKNOWN, not "was not finished", so it can
/// never license the success path — the fail-safe answer is the behaviour that
/// shipped before this fix. That matters because the no-store case is real: a
/// fixture or a checkout without `.aida-store` makes `spec_status` return
/// `None` at both ends.
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_advanced_to_finished_during_run(
    entry: Option<&RequirementStatus>,
    current: Option<&RequirementStatus>,
) -> bool {
    match (entry, current) {
        (Some(entry), Some(current)) => {
            phase1_status_is_finished(current) && !phase1_status_is_finished(entry)
        }
        _ => false,
    }
}

/// BUG-1769: render a substrate status reading for a diagnostic message, so a
/// lost-child failure can say what it actually read instead of asserting an
/// un-read "was not advanced".
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_spec_status_text(status: Option<&RequirementStatus>) -> String {
    match status {
        Some(status) => status.to_string(),
        None => "unreadable (no store, or the spec is not in it)".to_string(),
    }
}

// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_branch_race_reason(spec: &str, branch: &str) -> String {
    format!(
        "phase 1 workspace race for {spec}: branch `{branch}` was free when phase 1 resolved \
         it but exists now. `aida queue work --branch` reuses an existing branch, so launching \
         would adopt a branch this run did not create; refused before launching. Re-run to \
         resolve a fresh workspace, or inspect `{branch}` first"
    )
}

/// BUG-1629: retry cause recorded for the phase-1 replacement launch.
// trace:BUG-1629 | ai:claude
pub(crate) const PHASE1_LOST_CHILD_STATE_CAUSE: &str = "lost-child-state";

/// BUG-1629: is `worktree` free of every lease? Lease paths are stored
/// canonicalized while a pinned path may not be (a symlinked checkout, macOS
/// `/var`), so both sides are canonicalized before comparing. Fail safe: if
/// either side cannot be canonicalized, the worktree counts as leased, so
/// the replacement never passes `--force-claim` over a path it cannot prove
/// is free.
// trace:BUG-1629 | ai:claude
pub(crate) fn worktree_is_unleased(
    worktree: &std::path::Path,
    lease_worktrees: &[std::path::PathBuf],
) -> bool {
    let Ok(pinned) = worktree.canonicalize() else {
        return false;
    };
    lease_worktrees
        .iter()
        .all(|leased| leased.canonicalize().is_ok_and(|leased| leased != pinned))
}

/// BUG-1629: where `queue work` records its own refusal for an orchestrated
/// launch — a sibling of the handoff receipt, keyed by the same session id.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_path(receipt: &std::path::Path) -> std::path::PathBuf {
    receipt.with_extension("refusal.txt")
}

/// BUG-1629: called on the error path of every `aida` invocation. When this
/// process is an orchestrated phase-1 `queue work` child that failed before
/// publishing its handoff receipt, record the refusal next to the receipt so
/// the parent can name the real reason. Best-effort; see
/// [`orchestrated_child_refusal_to_record`] for the gates.
// trace:BUG-1629 | ai:claude
pub(crate) fn record_orchestrated_child_refusal(message: &str) {
    let receipt_env = std::env::var(ORCHESTRATED_LEASE_RECEIPT_ENV).ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some((refusal, summary)) =
        orchestrated_child_refusal_to_record(receipt_env.as_deref(), &args, message, |path| {
            path.exists()
        })
    {
        let _ = write_atomic(&refusal, &summary);
    }
}

/// BUG-1629: the pure decision behind [`record_orchestrated_child_refusal`].
/// Records `(refusal path, summary)` only when all hold: the process carries
/// the orchestrator's receipt env var, it is a `queue work` invocation, the
/// receipt does not exist yet (a child that got that far is not a pre-lease
/// refusal), no refusal was recorded already (never overwrite the first
/// cause), and the summary is non-empty.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_to_record(
    receipt_env: Option<&str>,
    args: &[String],
    message: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<(std::path::PathBuf, String)> {
    let receipt = std::path::PathBuf::from(receipt_env.filter(|r| !r.trim().is_empty())?);
    if args.len() < 2 || args[0] != "queue" || args[1] != "work" {
        return None;
    }
    let refusal = orchestrated_child_refusal_path(&receipt);
    if exists(&receipt) || exists(&refusal) {
        return None;
    }
    let summary = orchestrated_child_refusal_summary(message);
    (!summary.is_empty()).then_some((refusal, summary))
}

/// BUG-1629: the most characters of child refusal text either side keeps.
// trace:BUG-1629 | ai:claude
pub(crate) const ORCHESTRATED_CHILD_REFUSAL_MAX_CHARS: usize = 400;

/// BUG-1629: remove ANSI escape sequences and every other control character
/// (ESC, CR, NUL, ...) so child text cannot restyle or rewrite the parent's
/// stderr line, the `SpecRetried` detail, or the shelve reason.
// trace:BUG-1629 | ai:claude
pub(crate) fn strip_control_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                // CSI: ESC [ params... final byte in @..~
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Two-character escape: drop the next character too.
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// BUG-1629: the first non-empty line of an error, control characters
/// stripped, bounded for a message.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_summary(message: &str) -> String {
    message
        .lines()
        .map(|line| strip_control_text(line).trim().to_string())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(ORCHESTRATED_CHILD_REFUSAL_MAX_CHARS)
        .collect()
}

/// BUG-1629: read and remove the refusal a phase-1 child recorded. The read
/// is capped (4 KiB) and re-sanitized to the writer's bound, so a file that
/// was not written by the writer above cannot flood or restyle the message.
// trace:BUG-1629 | ai:claude
pub(crate) fn take_orchestrated_child_refusal(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<String> {
    use std::io::Read;
    let path = orchestrated_child_refusal_path(&orchestrated_lease_receipt_path(
        project_root,
        session_uuid,
    ));
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&path)
        .and_then(|file| file.take(4096).read_to_end(&mut bytes))
        .is_ok();
    let _ = std::fs::remove_file(&path);
    if !read {
        return None;
    }
    let summary = orchestrated_child_refusal_summary(&String::from_utf8_lossy(&bytes));
    (!summary.is_empty()).then_some(summary)
}

/// BUG-1629: drop any refusal file for a session whose child did establish
/// its lease or receipt. A child can write its receipt and still fail later,
/// or a refusal can survive a receipt that was published afterwards; the
/// file is keyed by the session uuid so it is harmless, but the parent
/// clears it so it never accumulates.
// trace:BUG-1629 | ai:claude
pub(crate) fn discard_orchestrated_child_refusal(
    project_root: &std::path::Path,
    session_uuid: &str,
) {
    let _ = std::fs::remove_file(orchestrated_child_refusal_path(
        &orchestrated_lease_receipt_path(project_root, session_uuid),
    ));
}

/// BUG-1629: the precise recovery state when a phase-1 child kept neither its
/// session lease nor its handoff receipt.
// trace:BUG-1629 | ai:claude
pub(crate) fn lost_child_state_reason(
    session_uuid: &str,
    exit: &str,
    elapsed: std::time::Duration,
) -> String {
    format!(
        "phase 1 child session {} exited {exit} after {}ms without a session lease or an \
         orchestrator handoff receipt: no implementer session started, so there is nothing \
         to resume",
        &session_uuid[..session_uuid.len().min(8)],
        elapsed.as_millis(),
    )
}

/// Lease ids present in `.aida/sessions/` — `<id>.toml` files only, *not*
/// the `<id>.activity.toml` / `<id>.manifest.toml` companions. A lease id is
/// a dot-free UUID, so a real lease file's name has exactly one `.`; every
/// companion file has two. (The old set-diff filter excluded only
/// `.activity.toml`, so it miscounted `.manifest.toml` as a lease — BUG-114.)
/// trace:BUG-114 | ai:claude
pub(crate) fn lease_ids_in(sessions_dir: &std::path::Path) -> Vec<String> {
    let mut ids = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sessions_dir) {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(stem) = name.strip_suffix(".toml") {
                    if !stem.contains('.') {
                        ids.push(stem.to_string());
                    }
                }
            }
        }
    }
    ids.sort();
    ids
}

pub(crate) const ORCHESTRATED_LEASE_RECEIPT_ENV: &str = "AIDA_ORCHESTRATED_LEASE_RECEIPT";

pub(crate) fn orchestrated_lease_receipt_path(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("orchestrator-handoffs")
        .join(format!("{claude_session_id}.json"))
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct OrchestratedLeaseReceipt {
    pub(crate) claude_session_id: String,
    pub(crate) lease_id: String,
    pub(crate) branch: String,
    pub(crate) worktree_path: std::path::PathBuf,
}

pub(crate) fn write_orchestrated_lease_receipt(
    path: &std::path::Path,
    claude_session_id: &str,
    lease: &SessionLease,
) -> Result<()> {
    let receipt = OrchestratedLeaseReceipt {
        claude_session_id: claude_session_id.to_string(),
        lease_id: lease.id.clone(),
        branch: lease.branch.clone(),
        worktree_path: lease.worktree_path.clone(),
    };
    let body = serde_json::to_string(&receipt)?;
    Ok(write_atomic(path, &body)?)
}

pub(crate) fn publish_orchestrated_lease_receipt_from_env(
    claude_session_id: Option<&str>,
    lease: &SessionLease,
) -> Result<()> {
    if let (Ok(path), Some(claude_id)) = (
        std::env::var(ORCHESTRATED_LEASE_RECEIPT_ENV),
        claude_session_id,
    ) {
        write_orchestrated_lease_receipt(std::path::Path::new(&path), claude_id, lease)
            .with_context(|| format!("writing orchestrator lease receipt {}", path))?;
    }
    Ok(())
}

pub(crate) fn prepare_orchestrated_lease_receipt(
    cmd: &mut std::process::Command,
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> std::path::PathBuf {
    let receipt = orchestrated_lease_receipt_path(project_root, claude_session_id);
    if let Some(parent) = receipt.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&receipt);
    // trace:BUG-1629 | ai:claude
    let _ = std::fs::remove_file(orchestrated_child_refusal_path(&receipt));
    cmd.env(ORCHESTRATED_LEASE_RECEIPT_ENV, &receipt);
    receipt
}

pub(crate) fn read_orchestrated_lease_receipt(
    path: &std::path::Path,
    claude_session_id: &str,
) -> Option<(String, String, std::path::PathBuf, Option<u32>)> {
    let body = std::fs::read_to_string(path).ok()?;
    let receipt: OrchestratedLeaseReceipt = serde_json::from_str(&body).ok()?;
    (receipt.claude_session_id == claude_session_id).then_some((
        receipt.lease_id,
        receipt.branch,
        receipt.worktree_path,
        None,
    ))
}

/// Find the session lease that `aida queue work --session-id <uuid>` created,
/// by matching the orchestrator-minted `claude_session_id` against the value
/// `aida queue work` records in each session manifest. This is the
/// deterministic replacement for the lease-set-diff heuristic: it pins the
/// orchestrated session by id, so concurrent leases (parallel user sessions,
/// nested `/aida-pickup`) and the `.manifest.toml` companion file cannot
/// confuse it. Returns `(lease_id, branch, worktree_path)`.
/// trace:BUG-114 trace:BUG-223 | ai:claude
pub(crate) fn find_orchestrated_lease(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> Option<(String, String, std::path::PathBuf, Option<u32>)> {
    let manifest = session_manifest::list_all(project_root)
        .into_iter()
        .find(|m| m.claude_session_id.as_deref() == Some(claude_session_id))?;
    let lease_path = leases_dir(project_root).join(format!("{}.toml", manifest.session_id));
    let body = std::fs::read_to_string(&lease_path).ok()?;
    let peek: LeasePeek = toml::from_str(&body).ok()?;
    Some((
        peek.id,
        peek.branch,
        peek.worktree_path,
        peek.active_pid.or(peek.creator_pid),
    ))
}

/// BUG-1285: when [`RealPhaseDriver::discover_orchestrated_lease`] comes up
/// empty, the dominant real-world cause is a same-scope lease conflict — the
/// child `aida queue work`'s claim gate refused before it ever minted a
/// lease under the orchestrator's session id, most often because a PREVIOUS
/// (now-dead) drain run's own lease on this spec's scope is still on disk.
/// Name that lease and its holder's liveness — per the SAME identity-aware
/// check `stale_lease_recovery_for_lease` uses (pid + kernel start time,
/// degrading to a plain pid check only when start-time data is missing) —
/// instead of leaving the caller to cross-reference a bare id list against
/// `aida ps` by hand. `None` when no lease on disk holds this spec's own
/// scope (a genuinely unmatched-candidates case), which leaves the existing
/// bare-list diagnostic standing.
// trace:BUG-1285 | ai:claude
pub(crate) fn scope_conflict_lease_failure(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<auto_complete::PhaseFailure> {
    let conflict = find_scope_lease_conflict(&list_leases(project_root), spec)?;
    let holder_pid = conflict.active_pid.or(conflict.creator_pid);
    let holder_start = if conflict.active_pid.is_some() {
        conflict.active_pid_start_time.as_deref()
    } else {
        conflict.creator_pid_start_time.as_deref()
    };
    let liveness = match holder_pid {
        Some(pid) if process_probe::process_identity_is_alive(pid, holder_start) => {
            format!("holder pid {pid} is alive")
        }
        Some(pid) => {
            let dead_since = std::fs::metadata(lease_path(project_root, &conflict.id))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| {
                    format!(
                        ", dead since ~{} ago",
                        humanize_secs_short(d.as_secs() as i64)
                    )
                })
                .unwrap_or_default();
            format!("holder pid {pid} is dead{dead_since}")
        }
        None => "the lease records no process-liveness signal".to_string(),
    };
    let short_id = &conflict.id[..conflict.id.len().min(8)];
    Some(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::LeaseConflict,
        format!(
            "the previous attempt's lease {short_id} is still held on scope `{spec}` \
             ({liveness}) — that is why this run's claim never minted a session lease \
             under it. Recovery: `aida queue work {spec} --force-claim` if the holder is \
             dead and its worktree is clean, or `aida queue work {spec} --resume` to \
             continue that session."
        ),
    ))
}

/// Decide whether the worktree's live branch represents a mid-phase swap
/// away from the branch the lease recorded at session-start. Returns the
/// new branch when a genuine swap is detected; `None` when the recorded
/// branch should stand — unchanged, detached HEAD (`rev-parse
/// --abbrev-ref` yields the literal `HEAD`), or the branch was
/// undetectable (the worktree is gone). Pure — split from
/// [`reconcile_orchestrated_branch`] so the swap rule is unit-testable
/// without a git fixture. trace:BUG-223 | ai:claude
pub(crate) fn swapped_branch(live: Result<String>, recorded: &str) -> Option<String> {
    match live {
        Ok(b) if !b.is_empty() && b != "HEAD" && b != recorded => Some(b),
        _ => None,
    }
}

/// Rewrite the `branch` field of an existing session lease, leaving every
/// other field intact. Used by the orchestrator when it detects a
/// mid-phase branch swap (BUG-223). The orchestrated session has already
/// exited by the time this runs, so there is no concurrent writer — but
/// the write is still atomic for consistency with the other lease-file
/// writers. trace:BUG-223 | ai:claude
pub(crate) fn update_lease_branch(
    project_root: &std::path::Path,
    lease_id: &str,
    branch: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut lease: SessionLease =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    lease.branch = branch.to_string();
    let content = toml::to_string_pretty(&lease)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// TASK-358: stamp the lease's `escalated_to_human` timestamp. The
/// orchestrator's `--escalate-blocks` path calls this after the advisor
/// escalates a punted design-fork to a human — the marker tells later
/// cleanup paths (`aida edit --status` out of `NeedsAttention`,
/// `aida session prune --escalations`) that this lease is safe to remove.
/// trace:TASK-358 | ai:claude
pub(crate) fn mark_lease_escalated_to_human(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut lease: SessionLease =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    lease.escalated_to_human = Some(chrono::Utc::now());
    let content = toml::to_string_pretty(&lease)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// BUG-778: stamp `manual_enter_at = now` on a lease `aida worktree enter|add`
/// just handed to a human — the provenance marker that keeps `aida ps` from
/// reading the operator's launch-lag as a dead agent (and from offering the
/// `aida queue work` re-dispatch that would race them).
///
/// Patched as a single generic `toml::Value` key insert, the
/// [`crate::worktree_lock`] pattern, rather than a `SessionLease`
/// round-trip: a lease may carry keys this struct doesn't model (STORY-711's
/// `authorized_by` is written the same way), and a deserialize/reserialize
/// would silently drop them.
///
/// Idempotent by construction — re-entering refreshes the stamp, which is
/// exactly right: the worktree is being handed over again.
// trace:BUG-778 | ai:claude
pub(crate) fn mark_lease_manual_enter(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut value: toml::Value =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    let table = value.as_table_mut().context("lease TOML is not a table")?;
    table.insert(
        "manual_enter_at".to_string(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    let content = toml::to_string_pretty(&value)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// Re-derive the worktree's current branch and, when it has drifted from
/// the branch the lease recorded at session-start, rewrite the lease so
/// every downstream consumer (`aida session end`, `aida session leases`)
/// sees the live branch. Returns the branch the orchestrator should drive
/// the rest of the pipeline with.
///
/// BUG-223: `/aida-pr`'s BUG-88 merged-branch-name guard can move the
/// commits to a fresh branch mid-implementer-phase. The lease's recorded
/// branch then goes stale and the phase-1 `gh pr list --head <branch>`
/// lookup reports a false "opened no PR". The worktree's live HEAD is the
/// only ground truth. trace:BUG-223 | ai:claude
pub(crate) fn reconcile_orchestrated_branch(
    project_root: &std::path::Path,
    lease_id: &str,
    worktree_path: &std::path::Path,
    recorded: &str,
) -> String {
    let live = aida_core::git_ops::current_branch(worktree_path);
    match swapped_branch(live, recorded) {
        Some(new_branch) => {
            match update_lease_branch(project_root, lease_id, &new_branch) {
                Ok(()) => eprintln!(
                    "  {} branch swapped during the implementer phase: `{}` → `{}` \
                     (lease updated)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    recorded,
                    new_branch,
                ),
                Err(e) => eprintln!(
                    "  {} branch swapped to `{}` but the lease could not be updated: {} \
                     (continuing with the live branch)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    new_branch,
                    e,
                ),
            }
            new_branch
        }
        None => recorded.to_string(),
    }
}

// trace:BUG-1199 trace:TASK-1262 | ai:codex
pub(crate) fn resolve_aida_exe_from(current: Option<std::path::PathBuf>) -> std::path::PathBuf {
    if let Some(p) = current {
        // Linux's `/proc/self/exe` can return "<path> (deleted)" when the
        // executable file has been unlinked. Strip that before checking.
        let lossy = p.to_string_lossy();
        let cleaned = lossy
            .strip_suffix(" (deleted)")
            .map(std::path::PathBuf::from)
            .unwrap_or(p);
        if cleaned.exists() {
            return cleaned;
        }
    }
    // Fall back to PATH search. Either the OS lookup failed, or the
    // resolved path no longer points at an existing file.
    std::path::PathBuf::from("aida")
}

/// BUG-766: make every child process resolve `aida` to THIS build.
///
/// Drive/drain child sessions (and anything they spawn) look up a bare
/// `aida` on PATH; on a machine with an old installed binary that lookup
/// can win over the coordinating build and run pre-guard bulk store saves.
/// Called once at the top of `main_entry` (single-threaded, before any
/// command dispatch): sets `AIDA_BIN` to the resolved coordinating binary
/// and prepends its directory to PATH so children inherit both. No-op when
/// the binary can't be resolved to an absolute existing path.
// trace:BUG-766 | ai:claude
pub(crate) fn export_coordinating_bin_env() {
    // Keep an invalid operator override intact so the launch path can report
    // it clearly; silently replacing it with current_exe would hide the error.
    // trace:TASK-1499 | ai:codex
    let ambient_override = std::env::var_os("AIDA_BIN").map(std::path::PathBuf::from);
    let has_aida_parent = ambient_override
        .as_ref()
        .is_some_and(|_| aida_bin::has_aida_ancestor());
    let resolution = aida_bin::process();
    if let Some(ambient_override) = ambient_override.as_ref() {
        // An extension calling back through the `AIDA_BIN` it was handed
        // names this very binary: nothing is being declined, so stay quiet.
        // trace:STORY-1641 | ai:claude
        let names_self =
            external_cmd::same_binary(ambient_override, &aida_bin::running_executable_fallback());
        if !has_aida_parent && !names_self {
            match &resolution {
                Ok(resolved) => eprintln!(
                    "AIDA_BIN={} is set but was not honored because no AIDA parent was found; using {} ({})",
                    ambient_override.display(), resolved.path.display(), resolved.source
                ),
                Err(error) => eprintln!(
                    "AIDA_BIN={} is set but was not honored because no AIDA parent was found; binary resolution failed: {error:#}",
                    ambient_override.display()
                ),
            }
        }
    }
    if ambient_override.is_some() && has_aida_parent && resolution.is_err() {
        return;
    }
    let exe = resolution
        .map(|resolved| resolved.path)
        .unwrap_or_else(|_| aida_bin::running_executable_fallback());
    if !exe.is_absolute() || !exe.exists() {
        return;
    }
    let Some(dir) = exe.parent().map(std::path::Path::to_path_buf) else {
        return;
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let updated = prepend_dir_to_path(&dir, &path);
    // `set_var` is wrapped in `unsafe` for Edition-2024 forward-compat
    // (mirrors `apply_session_env_to_process`); safe here because
    // `main_entry` hasn't spawned any threads yet.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("AIDA_BIN", &exe);
        std::env::set_var("PATH", updated);
    }
}

/// BUG-766: pure PATH computation for `export_coordinating_bin_env`, split
/// out so tests can assert the shape without mutating process env. Puts
/// `dir` first and drops any later occurrence of it, so nested
/// aida→claude→aida chains don't grow PATH without bound and a stale
/// binary earlier on the original PATH can never shadow the coordinating
/// build.
// trace:BUG-766 | ai:claude
pub(crate) fn prepend_dir_to_path(
    dir: &std::path::Path,
    path: &std::ffi::OsStr,
) -> std::ffi::OsString {
    let mut parts: Vec<std::path::PathBuf> = std::env::split_paths(path)
        .filter(|p| p.as_path() != dir)
        .collect();
    parts.insert(0, dir.to_path_buf());
    std::env::join_paths(parts).unwrap_or_else(|_| path.to_os_string())
}

#[cfg(test)]
#[path = "tests/resolve_aida_exe_tests.rs"]
mod resolve_aida_exe_tests;

#[cfg(test)]
#[path = "tests/story_1418_completion_seam_tests.rs"]
mod story_1418_completion_seam_tests;

/// BUG-376: tests that pin both substrate halves of the implementer-
/// complete signal — the `aida pr ship` banner emits the load-bearing
/// "EXIT NOW" phrases, and the `aida-implement.md` skill template
/// embeds the matching directive so an implementer agent picking up
/// the skill at session start sees the same rule the substrate enforces
/// at session end. Together they cover acceptance #3 ("run 'aida queue
/// work TASK-X' on a no-op test spec, confirm agent receives the exit
/// directive") at the unit level: the skill is what the agent reads on
/// the way in, the banner is what it reads on the way out.
#[cfg(test)]
#[path = "tests/bug_376_implementer_exit_directive_tests.rs"]
mod bug_376_implementer_exit_directive_tests;

/// The real [`auto_complete::PhaseDriver`]. Holds the project root + the
/// state discovered as phases run (branch, lease id, PR number).
/// trace:STORY-246 | ai:claude
/// BUG-311: build the argv for the orchestrator's phase-1 `aida queue work`
/// subprocess. Pure helper so the steal-threading + headless flag rules are
/// pinnable by unit test without spawning a process. The orchestrator always
/// sets `--session-id` (so it can locate the phase's lease via
/// `claude_session_id`), and conditionally appends `--steal`, `--no-human`,
/// and `--permission-mode <m>`. Order matches the existing inline code.
/// trace:BUG-311 | ai:claude
pub(crate) fn build_implementer_phase_args(
    spec: &str,
    session_uuid: &str,
    steal: bool,
    force_claim: bool,
    branch: Option<&str>,
    path: Option<&std::path::Path>,
    headless_implementer: bool,
    permission_mode: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "queue".into(),
        "work".into(),
        spec.into(),
        "--session-id".into(),
        session_uuid.into(),
    ];
    // BUG-311: the user's outer `--steal` is threaded so phase 1's
    // `handle_queue_work` can clear a dormant lease on this scope instead of
    // bailing with the canned "pass --steal" message — which was the BUG-311
    // symptom (the flag was passed but the inner subprocess never saw it).
    if steal {
        args.push("--steal".into());
    }
    if force_claim {
        args.push("--force-claim".into());
    }
    if let Some(branch) = branch {
        args.push("--branch".into());
        args.push(branch.into());
    }
    if let Some(path) = path {
        args.push("--path".into());
        args.push(path.to_string_lossy().into_owned());
    }
    // STORY-276: under `--no-human=both` the implementer phase runs headless;
    // `ReviewerOnly` (and a plain `--auto-complete`) leave phase 1 interactive.
    // trace:STORY-276 | ai:claude
    if headless_implementer {
        args.push("--no-human".into());
    }
    if let Some(pm) = permission_mode {
        args.push("--permission-mode".into());
        args.push(pm.into());
    }
    args
}

/// TASK-262: the argv `RealPhaseDriver::attempt_phase3_auto_rebase` hands to
/// the `aida` subprocess for a one-shot clean rebase of the reviewer-phase PR.
/// Factored out of the inline `Command` builder so the subprocess wiring (the
/// `pr rebase <N> --no-smoke` contract) is unit-testable without spawning.
// trace:TASK-262 | ai:claude
pub(crate) fn build_phase3_auto_rebase_args(pr_number: u64) -> Vec<String> {
    vec![
        "pr".into(),
        "rebase".into(),
        pr_number.to_string(),
        "--no-smoke".into(),
    ]
}

/// BUG-1295: classify a finished `aida pr rebase <N> --no-smoke` subprocess
/// by its EXIT CODE alone — never by matching its stdout/stderr text. Pure
/// so the classification is unit-testable without spawning: feed it a
/// process exit code and it names the `FailureKind` the orchestrator routes
/// on, independent of whatever prose the bail site that produced that code
/// happened to print. `None` covers every other outcome (unrecognized code,
/// signal death) — the caller falls back to an untyped
/// `auto_complete::PhaseFailure`.
// trace:BUG-1295 | ai:claude
pub(crate) fn classify_rebase_subprocess_exit(
    code: Option<i32>,
) -> Option<auto_complete::FailureKind> {
    match code {
        Some(c) if c == pr_rebase::REBASE_EXIT_CODE_REFUSED => {
            Some(auto_complete::FailureKind::StaleBaseRefused)
        }
        Some(c) if c == pr_rebase::REBASE_EXIT_CODE_CONFLICT => {
            Some(auto_complete::FailureKind::StaleBaseConflict)
        }
        _ => None,
    }
}

/// BUG-1295: the process exit code `main_entry` returns for a top-level
/// error. Pulled out of `main_entry` so it's unit-testable: a
/// `pr_rebase::RebaseFailureExit` (the sentinel a `pr rebase` conflict /
/// force-push-refusal bail site wraps its message in) carries its own exit
/// code; every other error keeps the historical exit code 1. This is the
/// producer-side half of the exit-code contract `classify_rebase_subprocess_exit`
/// reads on the consumer side.
// trace:BUG-1295 | ai:claude
pub(crate) fn exit_code_for_error(err: &anyhow::Error) -> i32 {
    // trace:TASK-1606 | ai:codex
    if let Some(failure) = err.downcast_ref::<pr_ship::ShipFailure>() {
        return failure.reason as i32;
    }
    err.downcast_ref::<pr_rebase::RebaseFailureExit>()
        .map(|sig| sig.code)
        .unwrap_or(1)
}

/// STORY-335: the argv `aida queue integrate --rebase` hands to the `aida`
/// subprocess to rebase a ready member's PR branch onto current main before
/// merging it. Mirrors the phase-3 auto-rebase (`pr rebase <N> --no-smoke`):
/// the local smoke is skipped because the subsequent `--from-pr` drive runs CI.
/// Factored out so the subprocess contract is unit-testable without spawning.
/// trace:STORY-335 | ai:claude
pub(crate) fn build_integrate_rebase_args(pr_number: u32) -> Vec<String> {
    vec![
        "pr".into(),
        "rebase".into(),
        pr_number.to_string(),
        "--no-smoke".into(),
    ]
}

// trace:STORY-1033 | ai:codex
pub(crate) fn agent_seat_for_phase(
    phase: auto_complete::Phase,
) -> aida_core::agents_config::AgentSeat {
    match phase {
        auto_complete::Phase::Implementer | auto_complete::Phase::Ci => {
            aida_core::agents_config::AgentSeat::Implementer
        }
        auto_complete::Phase::Reviewer => aida_core::agents_config::AgentSeat::Reviewer,
        auto_complete::Phase::Merge | auto_complete::Phase::Pull | auto_complete::Phase::Build => {
            aida_core::agents_config::AgentSeat::Integrator
        }
    }
}

/// TASK-1080: live three-state probe — is `branch` GONE on origin?
/// `git ls-remote --exit-code origin refs/heads/<branch>` distinguishes
/// "ref absent" (exit 2 → `Some(true)`, the merged+deleted signature) from
/// "ref present" (exit 0 → `Some(false)`) from "couldn't tell" (spawn error /
/// offline / auth → `None`). The three states matter: the stacked-promotion
/// path must never read an offline probe as "parent merged" — a false
/// promotion would rewrite a PR branch that still depends on an open parent.
// trace:TASK-1080 | ai:claude
pub(crate) fn remote_branch_gone(project_root: &std::path::Path, branch: &str) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "ls-remote",
            "--exit-code",
            "origin",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(false),
        Some(2) => Some(true),
        _ => None,
    }
}

/// TASK-262: the argv `RealPhaseDriver::auto_punt_text_question` hands to the
/// `aida` subprocess to record a headless implementer's design-fork punt.
/// Factored out of the inline `Command` builder so the subprocess wiring (the
/// `punt <spec> --category design-fork --reason <r> --lean <l>` contract) is
/// unit-testable without spawning.
// trace:TASK-262 | ai:claude
pub(crate) fn build_auto_punt_args(spec: &str, reason: &str, lean: &str) -> Vec<String> {
    vec![
        "punt".into(),
        spec.into(),
        "--category".into(),
        "design-fork".into(),
        "--reason".into(),
        reason.into(),
        "--lean".into(),
        lean.into(),
    ]
}

pub(crate) fn orchestrator_phase_child_env(
    run_token: &str,
    phase: auto_complete::Phase,
    variant: auto_complete::AutoCompleteVariant,
    queue_user_id: &str,
) -> Vec<(&'static str, String)> {
    let phase_role = match phase {
        auto_complete::Phase::Implementer => Some("implementer"),
        auto_complete::Phase::Reviewer => Some("reviewer"),
        _ => None,
    };
    // trace:BUG-901 | ai:codex
    let mut env = vec![
        (orchestrator::AUTO_COMPLETE_ENV, "1".to_string()),
        (orchestrator::TOKEN_ENV, run_token.to_string()),
        (orchestrator::VARIANT_ENV, variant.slug().to_string()),
        (orchestrator::PHASE_ENV, phase.index().to_string()),
    ];
    // trace:BUG-1038 | ai:codex
    if !queue_user_id.trim().is_empty() {
        env.push(("AIDA_USER", queue_user_id.to_string()));
    }
    if let Some(role) = phase_role {
        env.push(("AIDA_SESSION_ROLE", role.to_string()));
    }
    env
}

pub(crate) fn git_output_checked(
    worktree: &std::path::Path,
    args: &[&str],
) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("could not invoke `git {}`: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("`git {}` exited {}", args.join(" "), out.status)
        } else {
            format!("`git {}` failed: {stderr}", args.join(" "))
        })
    }
}

pub(crate) fn parse_git_count(s: &str, context: &str) -> Result<u32, auto_complete::PhaseFailure> {
    s.trim().parse::<u32>().map_err(|_| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::Internal,
            format!(
                "could not parse `{context}` count from git output `{}`",
                s.trim()
            ),
        )
    })
}

pub(crate) fn push_branch_from_implementer_worktree(
    worktree: &std::path::Path,
    branch: &str,
) -> Result<(), auto_complete::PhaseFailure> {
    // trace:BUG-878 | ai:codex
    let refspec = format!("HEAD:refs/heads/{branch}");
    let out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(["push", "-u", "origin", &refspec])
        .output()
        .map_err(|e| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Spawn,
                format!("could not invoke `git push -u origin {refspec}`: {e}"),
            )
        })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(auto_complete::PhaseFailure::new(format!(
        "could not push implementer branch `{branch}` from `{}` before phase-2 CI/teardown{}",
        worktree.display(),
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )))
}

pub(crate) fn ensure_implementer_branch_pushed(
    worktree: &std::path::Path,
    branch: &str,
    json: bool,
) -> Result<(), auto_complete::PhaseFailure> {
    // trace:BUG-878 | ai:codex
    let current = git_output_checked(worktree, &["branch", "--show-current"]).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify implementer branch before phase-2 CI/teardown: {e}"
        ))
    })?;
    if current != branch {
        return Err(auto_complete::PhaseFailure::new(format!(
            "implementer worktree `{}` is on `{}` but phase 2 is driving `{}` — refusing to push or tear down",
            worktree.display(),
            if current.is_empty() { "DETACHED" } else { &current },
            branch
        )));
    }

    match git_output_checked(
        worktree,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        Ok(upstream) => {
            let ahead = git_output_checked(worktree, &["rev-list", "--count", &format!("{upstream}..HEAD")])
                .map_err(|e| {
                    auto_complete::PhaseFailure::new(format!(
                        "could not compare implementer branch `{branch}` to `{upstream}` before phase-2 CI/teardown: {e}"
                    ))
                })
                .and_then(|s| parse_git_count(&s, "ahead-of-upstream"))?;
            if ahead == 0 {
                return Ok(());
            }
            if !json {
                eprintln!(
                    "  {} implementer branch `{}` is {} commit(s) ahead of `{}` — pushing before CI/teardown",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    branch,
                    ahead,
                    upstream,
                );
            }
            push_branch_from_implementer_worktree(worktree, branch)
        }
        Err(_) => {
            if !json {
                eprintln!(
                    "  {} implementer branch `{}` has no upstream — pushing before CI/teardown",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    branch,
                );
            }
            push_branch_from_implementer_worktree(worktree, branch)
        }
    }
}

pub(crate) fn orchestrator_pr_title_and_body(commit_msg: &str) -> Result<(String, String)> {
    // trace:BUG-893 | ai:codex
    let title = pr_ship::derive_pr_title_from_commit(commit_msg);
    if title.is_empty() {
        anyhow::bail!("could not derive a non-empty PR title from the latest commit");
    }
    let commit_body = pr_ship::derive_pr_body_from_commit(commit_msg);
    let note = "Opened by the AIDA orchestrator because the implementer completed with committed work but no open PR.";
    let body = if commit_body.trim().is_empty() {
        note.to_string()
    } else {
        format!("{note}\n\n{commit_body}")
    };
    Ok((title, body))
}

// TASK-1443: adopt an already-open PR for `branch` instead of opening a
// second one. #2042 and #2043 shared a head and were created 106 seconds
// apart — two orchestrator drives raced the same branch and neither one
// saw the other's freshly-opened PR before calling `open_change` again.
// Reuses the same forge-neutral lookup `aida pr ship` uses to resume onto
// an existing PR (`branch_pr_resolution_from_lookup`, TASK-141/STORY-516)
// rather than re-deriving branch->PR lookup here. `Found` adopts (returns
// the existing PR id); `Create` and every inconclusive lookup state
// (`LookupFailed`) fall through to the caller's normal open-PR path — a
// lookup we cannot trust must not block recovery, it only means this
// race-guard cannot help for that call. trace:TASK-1443 | ai:claude
pub(crate) fn existing_open_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
) -> Option<u64> {
    // Route through the EXPLICIT `forge_kind` the caller already resolved for
    // this drive, not `change_lookup_for_branch`'s own auto-detection — a
    // fixture/test remote (or a repo mid-migration) can auto-resolve to
    // `PureGitForge`, whose `change_for_branch` deliberately reports the
    // branch itself as `Found(id: 0)` (no PR concept). That sentinel is not
    // an adoptable PR number, so it is filtered out here in addition to
    // matching the caller's real forge. trace:TASK-1443 | ai:claude
    let lookup = crate::forge::forge_for_kind(project_root, forge_kind)
        .change_for_branch(branch)
        .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}")));
    match crate::pr_ship::branch_pr_resolution_from_lookup(&lookup) {
        crate::pr_ship::BranchPrResolution::Found(id) if id != 0 => Some(id),
        _ => None,
    }
}

pub(crate) fn open_orchestrator_pr_for_implementer_worktree(
    project_root: &std::path::Path,
    worktree: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Result<u64> {
    // trace:BUG-893 | ai:codex
    push_branch_from_implementer_worktree(worktree, branch)
        .map_err(|e| anyhow::anyhow!("{}", e.reason))?;
    // TASK-1442 follow-up (containment for BUG-1510): before opening the PR,
    // verify the branch actually carries a commit trailered for the spec
    // this drive is for.
    ensure_pr_open_spec_attribution(worktree, branch, spec)?;
    // TASK-1443: a concurrent drive may have already opened a PR for this
    // head between the push above and this point — adopt it rather than
    // opening a second one. trace:TASK-1443 | ai:claude
    if let Some(existing) = existing_open_pr_for_branch(project_root, branch, forge_kind) {
        return Ok(existing);
    }
    let commit_msg_out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(["log", "-1", "--format=%B"])
        .output()
        .context("could not invoke `git log` to derive orchestrator PR title/body")?;
    if !commit_msg_out.status.success() {
        anyhow::bail!(
            "`git log -1 --format=%B` failed: {}",
            String::from_utf8_lossy(&commit_msg_out.stderr).trim()
        );
    }
    let commit_msg = String::from_utf8_lossy(&commit_msg_out.stdout).to_string();
    let (title, body) = orchestrator_pr_title_and_body(&commit_msg)?;
    let change = crate::forge::forge_for_kind(project_root, forge_kind)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title,
            body,
            draft: false,
        })
        .context("could not open orchestrator-created pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the orchestrator-created pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

pub(crate) fn origin_branch_ref(branch: &str) -> String {
    format!("origin/{branch}")
}

pub(crate) fn origin_default_ref_for_branch_recovery(
    project_root: &std::path::Path,
) -> Option<String> {
    resolve_default_branch_ref(project_root).map(|r| {
        if r.starts_with("origin/") {
            r
        } else {
            format!("origin/{r}")
        }
    })
}

pub(crate) fn pushed_branch_commits_ahead_default(
    project_root: &std::path::Path,
    branch: &str,
) -> Result<u32, auto_complete::PhaseFailure> {
    // trace:BUG-895 | ai:codex
    let branch_ref = origin_branch_ref(branch);
    git_output_checked(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch_ref}^{{commit}}"),
        ],
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify pushed branch `{branch_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    let default_ref = origin_default_ref_for_branch_recovery(project_root).ok_or_else(|| {
        auto_complete::PhaseFailure::new(
            "could not resolve origin default branch before phase-3 PR recovery",
        )
    })?;
    git_output_checked(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{default_ref}^{{commit}}"),
        ],
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify origin default branch `{default_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    let range = format!("{default_ref}..{branch_ref}");
    let ahead = git_output_checked(project_root, &["rev-list", "--count", &range]).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not compare pushed branch `{branch_ref}` to `{default_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    parse_git_count(&ahead, "pushed-branch-ahead-default")
}

impl RealPhaseDriver {
    pub(crate) fn run_one_agent_gate(
        &mut self,
        pr: u32,
        gate: &AgentGateConfig,
    ) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
        let verdict_dir = self.project_root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&verdict_dir).map_err(|e| {
            auto_complete::PhaseFailure::new(format!(
                "could not create {}: {e}",
                verdict_dir.display()
            ))
        })?;
        let gate_token = sanitize_gate_file_token(&gate.name);
        let verdict_path = verdict_dir.join(format!("PR-{pr}-{gate_token}.json"));
        let _ = std::fs::remove_file(&verdict_path);

        let gate_started_at = std::time::SystemTime::now();
        let session_uuid = uuid::Uuid::now_v7().to_string();
        let headless_vendor = session::resolve_headless_vendor(&self.project_root);
        // BUG-1290: an agent gate always runs after `run_reviewer` already
        // announced the Reviewer phase — attach this gate's session, don't
        // re-announce the entry.
        self.mark_drain_phase_session(
            auto_complete::Phase::Reviewer,
            &session_uuid,
            headless_vendor,
            false,
        );
        let scope = format!("PR-{pr}");
        release_dead_phase_predecessor_leases(
            &self.project_root,
            &scope,
            auto_complete::Phase::Reviewer,
        )?;

        let mut cmd = std::process::Command::new(self.aida_exe());
        cmd.current_dir(&self.project_root)
            .args([
                "queue",
                "work",
                &scope,
                "--role",
                &gate.role,
                "--session-id",
                &session_uuid,
                "--no-pull",
            ])
            .env("AIDA_REVIEW_VERDICT_FILE", &verdict_path)
            .env("AIDA_AGENT_GATE_NAME", &gate.name)
            .env("AIDA_AGENT_GATE_ROLE", &gate.role)
            .env("AIDA_AGENT_GATE_APPLIES_TO", &gate.applies_to);
        for (key, value) in orchestrator_phase_child_env(
            &self.run_token,
            auto_complete::Phase::Reviewer,
            self.variant,
            &self.queue_user_id,
        ) {
            cmd.env(key, value);
        }
        cmd.env("AIDA_SESSION_ROLE", &gate.role);
        if let Some(mode) = self.no_human {
            cmd.env(orchestrator::NO_HUMAN_MODE_ENV, mode.slug());
        }
        if let Some(pm) = &self.permission_mode {
            cmd.args(["--permission-mode", pm]);
        }
        if self.no_human.is_some() {
            cmd.arg("--no-human");
        }
        if self.allow_stale_base {
            cmd.arg("--allow-stale-base");
        }

        let sentinel = exit_signal::sentinel_path(&self.sessions_dir(), &session_uuid);
        let mut watchdog = self.no_human.is_some().then(|| {
            PhaseWatchdog::new_for_phase(
                self.project_root.clone(),
                session_uuid.clone(),
                headless_vendor,
                self.drain_tuning.no_progress,
                self.drain_tuning.ceiling,
                self.drain_tuning.idle_config(),
                auto_complete::Phase::Reviewer,
            )
        });
        let mut wd_closure = watchdog.as_mut().map(|w| move || w.check());
        let wd_dyn: Option<&mut dyn FnMut() -> Option<String>> = wd_closure
            .as_mut()
            .map(|c| c as &mut dyn FnMut() -> Option<String>);
        let outcome = exit_signal::spawn_and_wait_watched(
            cmd,
            &sentinel,
            &self.exit_cfg,
            wd_dyn,
            self.no_human.is_some(),
        )
        .map_err(|e| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Spawn,
                format!("could not launch agent gate `{}`: {e}", gate.name),
            )
        })?;
        if let exit_signal::ExitOutcome::WatchdogTripped(reason) = &outcome {
            // BUG-1299: the hint was resolved alongside `reason` in the same
            // `watchdog_trip_report` match arm — attach it verbatim rather
            // than let `recovery_hint` re-derive one from `FailureKind`
            // alone (which cannot tell NoProgress from Ceiling).
            // trace:BUG-1299 | ai:claude
            let mut failure = auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Watchdog,
                format!(
                    "agent gate `{}` watchdog stopped the session — {reason}",
                    gate.name
                ),
            );
            if let Some(hint) = watchdog.as_mut().and_then(|w| w.take_trip_hint()) {
                failure = failure.with_hint_override(hint);
            }
            return Err(failure);
        }
        if let exit_signal::ExitOutcome::Natural(status) = &outcome {
            if !status.success() {
                return Err(auto_complete::PhaseFailure::new(format!(
                    "agent gate `{}` session exited {}",
                    gate.name,
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "with a signal".to_string())
                )));
            }
        } else if !self.json {
            eprintln!(
                "  {} agent gate `{}` signalled completion — session reaped",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                gate.name
            );
        }

        let gate_head_sha = pr_head_sha_best_effort(self, pr);
        // TASK-1460: a skill heredoc write bypassed the record path; archive
        // it, then let a fresh archived verdict AT the head answer when the
        // current file does not. trace:TASK-1460 | ai:claude
        let _ = review_verdict::adopt_direct_write(&verdict_path);
        let outcome = match read_verdict_file_for_head(&verdict_path, gate_head_sha.as_deref())
            .or_else(|e| {
                phase3_head_archive_fallback(
                    &verdict_path,
                    gate_head_sha.as_deref(),
                    gate_started_at,
                )
                .ok_or(e)
            }) {
            Ok(o) => o,
            Err(primary_failure) => {
                if verdict_path.is_file() {
                    return Err(primary_failure);
                }
                let fallback = spec_verdict_fallback_for_phase3(
                    &self.project_root,
                    &self.spec,
                    gate_started_at,
                    gate_head_sha.as_deref(),
                )?;
                let fallback = match fallback {
                    some @ Some(_) => some,
                    None => sibling_verdict_sweep_for_phase3(
                        &self.project_root,
                        pr,
                        &self.spec,
                        gate_started_at,
                        gate_head_sha.as_deref(),
                    )?,
                };
                if let Some(o) = fallback {
                    o
                } else if self.no_human.is_some() {
                    return Err(enrich_no_verdict_with_headless_diagnostic(
                        primary_failure,
                        &self.project_root,
                        gate_started_at,
                    ));
                } else {
                    return Err(primary_failure);
                }
            }
        };
        capture_review_calibration_for_spec(&self.project_root, &verdict_path, &self.spec);

        if let Ok((gate_lease, _, _)) = self.discover_orchestrated_lease(&session_uuid) {
            let _ = std::process::Command::new(self.aida_exe())
                .current_dir(&self.project_root)
                .args(["session", "end", &gate_lease, "--yes", "--skip-ci"])
                .status_retrying_etxtbsy();
        }

        Ok(outcome)
    }
}

pub(crate) fn head_commit_message(project_root: &std::path::Path, rev: &str) -> Result<String> {
    let commit_msg_out = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "-1", "--format=%B", rev])
        .output()
        .context("could not invoke `git log` to derive orchestrator PR title/body")?;
    if !commit_msg_out.status.success() {
        anyhow::bail!(
            "`git log -1 --format=%B {rev}` failed: {}",
            String::from_utf8_lossy(&commit_msg_out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&commit_msg_out.stdout).to_string())
}

pub(crate) fn open_orchestrator_pr_for_pushed_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Result<u64> {
    // BUG-895: phase 2 may have already pushed and removed the implementer
    // worktree. Recover from origin/<branch> without trying to push again.
    // trace:BUG-895 | ai:codex
    let ahead = pushed_branch_commits_ahead_default(project_root, branch)
        .map_err(|e| anyhow::anyhow!("{}", e.reason))?;
    if ahead == 0 {
        anyhow::bail!("pushed branch `origin/{branch}` has no commits ahead of origin default");
    }
    let branch_ref = origin_branch_ref(branch);
    // TASK-1442 follow-up (containment for BUG-1510): before opening the PR,
    // verify the pushed branch actually carries a commit trailered for the
    // spec this drive is for.
    ensure_pr_open_spec_attribution(project_root, &branch_ref, spec)?;
    // TASK-1443: adopt an already-open PR for this head instead of opening a
    // second one (see `existing_open_pr_for_branch` above). trace:TASK-1443 | ai:claude
    if let Some(existing) = existing_open_pr_for_branch(project_root, branch, forge_kind) {
        return Ok(existing);
    }
    let commit_msg = head_commit_message(project_root, &branch_ref)?;
    let (title, body) = orchestrator_pr_title_and_body(&commit_msg)?;
    let change = crate::forge::forge_for_kind(project_root, forge_kind)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title,
            body,
            draft: false,
        })
        .context("could not open orchestrator-created pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the orchestrator-created pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

pub(crate) fn try_open_orchestrator_pr_for_no_pr_worktree(
    project_root: &std::path::Path,
    worktree: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Option<(u32, u64)> {
    // trace:BUG-893 trace:BUG-1037 | ai:codex
    let ahead = branch_commits_ahead_main(worktree, branch).unwrap_or(0);
    if ahead == 0 {
        // BUG-1485: `ahead == 0` does NOT mean there is nothing to recover. It
        // is also what a MISSING WORKTREE produces: `git -C <gone>` fails, the
        // Option is None, and `unwrap_or(0)` flattens "could not look" into
        // "looked and found nothing". Phase 2 may have already pushed the
        // branch and torn the worktree down, in which case the work is safe on
        // `origin/<branch>` and perfectly recoverable — but phase 1 asked the
        // wrong repository and concluded there was nothing there, so recovery
        // fell through to the generic "run /aida-pr inside the session"
        // failure, against a session that no longer exists.
        //
        // Phase 3 already solved this for its own NoPr case (BUG-895). Reuse
        // that helper rather than re-deriving the logic: it verifies
        // `origin/<branch>` and the origin default ref, counts ahead between
        // them, and opens the PR from the pushed ref WITHOUT pushing again.
        // It returns None when the branch genuinely is not ahead, so the
        // no-work case still falls through exactly as before.
        // trace:BUG-1485 | ai:claude
        return try_open_orchestrator_pr_for_no_pr_pushed_branch(
            project_root,
            branch,
            forge_kind,
            spec,
        );
    }
    match open_orchestrator_pr_for_implementer_worktree(
        project_root,
        worktree,
        branch,
        forge_kind,
        spec,
    ) {
        Ok(pr) => Some((ahead, pr)),
        Err(e) => {
            eprintln!(
                "  {} could not auto-open a PR from the implementer worktree \
                 ({e:#}) — retrying from the pushed branch",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            // The worktree exists and is ahead, so the failure was the push or
            // the forge call. If the branch had already reached origin, the
            // pushed-branch path can still succeed; if it never did, this
            // returns None and the caller falls back to punt/fail as before.
            // trace:BUG-1485 | ai:claude
            try_open_orchestrator_pr_for_no_pr_pushed_branch(project_root, branch, forge_kind, spec)
        }
    }
}

/// Preserve the watchdog diagnosis while adding the concrete review artifact
/// it left behind. The caller only uses this after PR recovery failed, so the
/// shelve record must tell triage that this is reviewable branch work rather
/// than an empty timed-out session.
// trace:BUG-1450 | ai:codex
pub(crate) fn watchdog_failure_with_committed_work(
    mut failure: auto_complete::PhaseFailure,
    worktree: &std::path::Path,
    branch: &str,
) -> auto_complete::PhaseFailure {
    let ahead = branch_commits_ahead_main(worktree, branch).unwrap_or(0);
    if ahead > 0 {
        failure.reason = format!(
            "{}; left {ahead} committed commit(s) on reviewable branch `{branch}` with no open PR",
            failure.reason
        );
        failure.hint_override = Some(format!(
            "Reviewable work survived on `{branch}` ({ahead} commit(s) ahead); open or recover its PR before re-driving the implementer"
        ));
    } else {
        failure.reason = format!("{}; left no committed work", failure.reason);
    }
    failure
}

pub(crate) fn try_open_orchestrator_pr_for_no_pr_pushed_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Option<(u32, u64)> {
    // trace:BUG-895 trace:BUG-1037 | ai:codex
    let ahead = match pushed_branch_commits_ahead_default(project_root, branch) {
        Ok(ahead) if ahead > 0 => ahead,
        _ => return None,
    };
    match open_orchestrator_pr_for_pushed_branch(project_root, branch, forge_kind, spec) {
        Ok(pr) => Some((ahead, pr)),
        Err(e) => {
            eprintln!(
                "  {} could not auto-open a PR for pushed branch `{branch}` \
                 ({e:#}) — falling back to phase-3 NoPr",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            None
        }
    }
}

/// Run STORY-1424's deterministic rung in an isolated checkout of the exact
/// reviewed head. `None` means the spec has no executable criteria and the
/// legacy reviewer path must remain byte-for-byte unchanged.
// trace:STORY-1424 trace:ADR-55 | ai:codex
pub(crate) fn prepare_graded_review(
    project_root: &std::path::Path,
    spec: &str,
    pr: u32,
    reviewed_sha: &str,
    branch: Option<&str>,
) -> Result<Option<graded_review::GradedReviewVerdict>, auto_complete::PhaseFailure> {
    let store = load_store_for_lookup(project_root).ok_or_else(|| {
        auto_complete::PhaseFailure::new("could not load spec store for graded review")
    })?;
    let req = store.get_requirement_by_spec_id(spec).ok_or_else(|| {
        auto_complete::PhaseFailure::new(format!("could not resolve {spec} for graded review"))
    })?;
    let criteria = graded_review::parse_acceptance_criteria(&req.description);
    if !criteria
        .iter()
        .any(|c| matches!(c, graded_review::CriterionKind::Executable { .. }))
    {
        return Ok(None);
    }
    // Trust snapshot, taken once per review before anything is spawned and
    // never re-read (no check-then-reread gap). Global config only; the repo
    // read below is display-only. trace:STORY-1476 | ai:claude
    let mut policy = acceptance_command_policy_global();
    policy.repo_optin_ignored = repo_review_optin_present(project_root);

    // The branch is a forge-reported PR head and the sha a forge-reported
    // commit; keep both from reading as git options. trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(reviewed_sha.trim()) {
        return Err(auto_complete::PhaseFailure::new(format!(
            "reviewed head `{reviewed_sha}` is not a commit ID; refusing graded review"
        )));
    }
    if let Some(branch) = branch {
        if let Err(e) = git_arg_guard::reject_option_like("branch", branch) {
            return Err(auto_complete::PhaseFailure::new(e.to_string()));
        }
        let fetched = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["fetch", git_arg_guard::END_OF_OPTIONS, "origin", branch])
            .output();
        if !fetched.as_ref().is_ok_and(|o| o.status.success()) {
            return Err(auto_complete::PhaseFailure::new(format!(
                "could not fetch branch {branch} before graded review"
            )));
        }
    }
    let checkout =
        std::env::temp_dir().join(format!("aida-graded-pr-{pr}-{}", uuid::Uuid::now_v7()));
    let added = std::process::Command::new("git")
        .current_dir(project_root)
        // trace:BUG-1622 | ai:claude
        .args(["worktree", "add", "--detach", "--"])
        .arg(&checkout)
        .arg(reviewed_sha.trim())
        .output()
        .map_err(|e| {
            auto_complete::PhaseFailure::new(format!(
                "could not create graded-review checkout: {e}"
            ))
        })?;
    if !added.status.success() {
        return Err(auto_complete::PhaseFailure::new(format!(
            "could not check out reviewed head {reviewed_sha}: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        )));
    }

    let diff = std::process::Command::new("gh")
        .current_dir(project_root)
        .args(["pr", "diff", &pr.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let evaluator: Option<Box<dyn evaluator::EvaluatorEngine>> =
        evaluator::JevEvaluator::from_env()
            .ok()
            .map(|v| Box::new(v) as Box<dyn evaluator::EvaluatorEngine>)
            .or_else(|| {
                std::env::var("AIDA_LOCAL_LLM_ENDPOINT").ok().map(|_| {
                    Box::new(evaluator::LocalLlmEvaluator::from_env())
                        as Box<dyn evaluator::EvaluatorEngine>
                })
            });
    let result = graded_review::execute_graded_review(
        spec,
        &req.title,
        &req.description,
        &diff,
        reviewed_sha,
        &checkout,
        evaluator.as_deref(),
        &policy,
    )
    .map_err(|e| auto_complete::PhaseFailure::new(format!("graded review failed closed: {e:#}")));
    let _ = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["worktree", "remove", "--force"])
        .arg(&checkout)
        .status();
    let verdict = result?;
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr}-graded.json"));
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not create graded review record directory: {e}"
        ))
    })?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&verdict).unwrap_or_default(),
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not write graded review record {}: {e}",
            path.display()
        ))
    })?;
    Ok(Some(verdict))
}

/// TASK-1421: the publication boundary driven end to end through an injected
/// forge — no env var, no global, just the driver's `forge_factory` field.
// trace:TASK-1421 | ai:claude
#[cfg(test)]
mod forge_seam_tests {
    use super::*;
    use crate::forge::fake::RecordingForge;
    use crate::forge::{ChangeLookup, ChangeRef};

    fn driver_with(root: &std::path::Path, forge: &RecordingForge) -> RealPhaseDriver {
        let mut driver = RealPhaseDriver::new(
            root.to_path_buf(),
            "TASK-1421".into(),
            "test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "test-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::ThroughCi,
        );
        driver.forge_factory = Some(forge.factory());
        driver
    }

    fn change(id: u64, branch: &str) -> ChangeRef {
        ChangeRef {
            id,
            url: format!("https://example.invalid/pull/{id}"),
            branch: branch.into(),
            base: String::new(),
            title: Some("feat: work the guards refused".into()),
        }
    }

    #[test]
    fn refused_preflight_closes_the_open_change() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(42, "claude/task-1421"));
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed:\ndrift");

        let closed = forge.closed();
        assert_eq!(closed.len(), 1, "exactly one close_change call: {closed:?}");
        assert_eq!(closed[0].0, 42, "closed the PR the implementer opened");
        assert!(
            closed[0].1.contains("guard `fmt` failed"),
            "the retraction note carries the guard output: {}",
            closed[0].1
        );
    }

    #[test]
    fn refused_preflight_leaves_an_already_merged_change_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.merged_for_branch = ChangeLookup::Found(change(7, "claude/task-1421"));
        let mut driver = driver_with(tmp.path(), &forge);

        // The scenario is real only if the lookup resolves to AlreadyMerged
        // (no open change, the branch's change merged) through the fake.
        assert!(matches!(
            driver.detect_phase1_pr("claude/task-1421"),
            Phase1PrResolve::AlreadyMerged(ref pr) if pr.number == 7
        ));
        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "a merged change must never be closed: {:?}",
            forge.closed()
        );
    }

    #[test]
    fn refused_preflight_leaves_same_branch_pr_by_another_user_open() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(43, "claude/task-1421"));
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "same-branch PR by a different forge user must remain untouched"
        );
    }

    /// BUG-1714 AC1: an inconclusive-only preflight outcome leaves the PR
    /// OPEN, stamps a merge-hold marker so it cannot merge unreviewed, and
    /// explains on the PR — without the word "refused" — that the guards
    /// could not complete and will be retried.
    // trace:BUG-1714 | ai:claude
    #[test]
    fn inconclusive_preflight_holds_the_open_change_instead_of_closing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(42, "claude/bug-1714"));
        let mut driver = driver_with(tmp.path(), &forge);

        let detail = implementer_preflight::inconclusive_detail(&[(
            "Run clippy".into(),
            "timed out after 900s; binary: /t/aida (build timed out after 900s)".into(),
        )]);
        driver.hold_inconclusive_publication("claude/bug-1714", &detail);

        assert!(
            forge.closed().is_empty(),
            "an inconclusive outcome must never close the PR: {:?}",
            forge.closed()
        );
        let record = crate::merge_hold::read_hold_record(tmp.path(), 42)
            .expect("the PR must be left unmergeable by a typed merge-hold marker");
        assert!(
            record.detail.contains("could not complete"),
            "the hold explains itself: {}",
            record.detail
        );
        let commented = forge.commented();
        assert_eq!(commented.len(), 1, "the hold is explained on the PR");
        assert_eq!(commented[0].0, 42);
        assert!(
            !commented[0].1.to_ascii_lowercase().contains("refus"),
            "nothing refused an unverified change: {}",
            commented[0].1
        );
        assert!(commented[0].1.contains("retried"), "{}", commented[0].1);
    }

    #[test]
    fn inconclusive_preflight_closes_when_both_hold_layers_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(45, "claude/bug-1714"));
        let mut driver = driver_with(tmp.path(), &forge);

        // Occupy the hold directory path with a file, forcing the marker write
        // to fail. Label sync only fails when a forge is declared but the repo
        // cannot be pinned (a pure-git root short-circuits to Ok): a git repo
        // with a github provider and no origin remote is unpinnable.
        std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join(".aida/merge-holds"), "occupied").unwrap();
        let detail = "guard `Run clippy` could not complete: timed out";
        driver.hold_inconclusive_publication("claude/bug-1714", detail);

        let closed = forge.closed();
        assert_eq!(
            closed.len(),
            1,
            "double failure must retract the PR: {closed:?}"
        );
        assert_eq!(closed[0].0, 45);
        let notice = &closed[0].1;
        assert!(notice.contains("guards could not complete"), "{notice}");
        assert!(
            notice.contains("merge-hold could not be applied"),
            "{notice}"
        );
        assert!(notice.contains("Merge-hold marker error:"), "{notice}");
        assert!(notice.contains("Merge-hold label error:"), "{notice}");
        assert!(
            notice.contains("closed only because it could not be made unmergeable"),
            "{notice}"
        );
        assert!(notice.contains("branch is preserved"), "{notice}");
        assert!(notice.contains("guards will be retried"), "{notice}");
        // The close must never reuse the refused-path notice: the guards did
        // not refuse this change, they could not complete. (The embedded
        // label error may legitimately say "refusing to call `gh`".)
        assert!(!notice.contains("guards refused"), "{notice}");
        assert!(!notice.contains("Closed automatically:"), "{notice}");
    }

    #[test]
    fn inconclusive_preflight_warns_on_the_pr_when_it_can_neither_hold_nor_close() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(46, "claude/bug-1714"));
        // A foreign author forbids the close; with both hold layers down the
        // last lever is a warning comment on the PR itself.
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join(".aida/merge-holds"), "occupied").unwrap();
        let detail = "guard `Run clippy` could not complete: timed out";
        driver.hold_inconclusive_publication("claude/bug-1714", detail);

        assert!(
            forge.closed().is_empty(),
            "a foreign PR must not be closed: {:?}",
            forge.closed()
        );
        let commented = forge.commented();
        assert_eq!(
            commented.len(),
            1,
            "an uncloseable unheld PR must carry a warning comment: {commented:?}"
        );
        assert_eq!(commented[0].0, 46);
        let warning = &commented[0].1;
        assert!(warning.contains("Merge-hold marker error:"), "{warning}");
        assert!(warning.contains("Merge-hold label error:"), "{warning}");
        assert!(warning.contains("neither held nor closed"), "{warning}");
        assert!(warning.contains("OPEN and UNVERIFIED"), "{warning}");
        assert!(warning.contains("guards will be retried"), "{warning}");
        // The PR stayed open, so the comment must not claim it was closed.
        assert!(!warning.contains("was closed"), "{warning}");
    }

    #[test]
    fn refused_preflight_fails_closed_when_pr_author_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(44, "claude/task-1421"));
        forge.author = None;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "unknown ownership must fail closed"
        );
    }

    /// TASK-1552 finding 1: a refused change this identity did not open is
    /// left open — but it must be left UNMERGEABLE, not merely talked about
    /// on stderr. Under the branch-keyed ownership rule this is the common
    /// rework-round shape: round 1's PR is still open when round 2 refuses.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_a_foreign_open_change_unmergeable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(47, "claude/task-1552"));
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed:\ndrift");

        assert!(
            forge.closed().is_empty(),
            "a foreign PR must not be closed: {:?}",
            forge.closed()
        );
        let record = crate::merge_hold::read_hold_record(tmp.path(), 47)
            .expect("the unclosable refused PR must carry a typed merge-hold marker");
        assert!(
            record.detail.contains("refused"),
            "the hold says a guard objected: {}",
            record.detail
        );
        assert!(
            record.detail.contains("guard `fmt` failed"),
            "the hold carries the guard output: {}",
            record.detail
        );
        let commented = forge.commented();
        assert_eq!(commented.len(), 1, "the hold is explained on the PR");
        assert_eq!(commented[0].0, 47);
        assert!(
            commented[0].1.contains("aida merge-hold clear"),
            "the PR comment names the release lever: {}",
            commented[0].1
        );
    }

    /// TASK-1552 finding 1, unverified arm: when the author cannot be
    /// confirmed the change may still be ours and still carries failing
    /// guards — it gets the same hold.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_an_author_unknown_change_unmergeable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(48, "claude/task-1552"));
        forge.author = None;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "unknown ownership must fail closed"
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 48).is_some(),
            "an author-unknown refused PR must be held unmergeable"
        );
    }

    /// TASK-1552 finding 3: the unreadable-forge path. The ownership probe
    /// itself fails — the PR's state is unknowable, so the close is refused
    /// AND the hold is stamped (it only tightens).
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_when_the_ownership_probe_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(49, "claude/task-1552"));
        forge.metadata_error = Some("change_metadata unavailable".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "an unverifiable PR must not be closed: {:?}",
            forge.closed()
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 49).is_some(),
            "an unverifiable refused PR must be held unmergeable"
        );
    }

    /// TASK-1552: a change that resolved but is no longer open needs no
    /// retraction and no hold — holding a closed PR would strand a stale
    /// marker on a number that may be reused by the forge UI as noise.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_leaves_a_no_longer_open_change_unheld() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(50, "claude/task-1552"));
        forge.metadata_state = crate::forge::ChangeState::Closed;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "a no-longer-open change must be left alone: {:?}",
            forge.closed()
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 50).is_none(),
            "a no-longer-open change must not be held"
        );
        assert!(
            forge.commented().is_empty(),
            "nothing to explain on a change that is already resolved: {:?}",
            forge.commented()
        );
    }

    /// TASK-1552 finding 1: even OUR OWN refused PR whose close call fails
    /// stays open and guards-failing — it gets the hold too.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_when_the_close_call_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(51, "claude/task-1552"));
        forge.close_error = Some("close rejected by the forge".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert_eq!(
            forge.closed().len(),
            1,
            "the close must have been attempted"
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 51).is_some(),
            "a refused PR whose close failed must be held unmergeable"
        );
    }

    /// TASK-1552 finding 3: a retraction whose PR was resolved through the
    /// spec-id search (the branch-keyed lookup missed) closes that PR like
    /// any other — the ownership probe and close run on the recovered number.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_closes_a_spec_search_resolved_change() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_spec = ChangeLookup::Found(change(52, "claude/task-1421-swapped"));
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        let closed = forge.closed();
        assert_eq!(
            closed.len(),
            1,
            "the spec-search-recovered PR must be retracted: {closed:?}"
        );
        assert_eq!(closed[0].0, 52);
    }
}

// The BUG-1690 reused-draft-PR seam tests live in their own file so the
// `aida criteria` scanner can anchor their AC markers (the convention for
// every AC-traced driver test).
// trace:BUG-1690 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1690_undraft_reuse_tests.rs"]
mod bug_1690_undraft_reuse_tests;

/// BUG-1628: auto-complete phase 1 and ordinary pickup resolve the same
/// branch + worktree for a custom-prefix requirement. Temp repos only.
// trace:BUG-1628 | ai:claude
#[cfg(test)]
mod bug_1628_pickup_resolver_tests {
    use super::*;

    fn temp_project(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap();
        assert!(status.success());
        (temp, root)
    }

    fn driver_for(root: &std::path::Path, spec: &str) -> RealPhaseDriver {
        RealPhaseDriver::new(
            root.to_path_buf(),
            spec.into(),
            "test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "test-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::ThroughCi,
        )
    }

    #[test]
    fn bug_1628_auto_complete_resolves_same_workspace_as_pickup_for_custom_prefix() {
        let (temp, root) = temp_project("qci");
        let driver = driver_for(&root, "SPEC-016");

        let (auto_path, auto_branch) = auto_complete::PhaseDriver::implementer_workspace(&driver)
            .expect("resolver succeeds")
            .expect("a workspace is resolved");
        // Ordinary pickup (`queue work` / `session_start`) resolver.
        let (pickup_path, pickup_branch) =
            resolve_pickup_workspace(&root, "SPEC-016", "auto").unwrap();

        assert_eq!(auto_path, pickup_path.display().to_string());
        assert_eq!(auto_branch, pickup_branch);
        // Project slug kept, ID separator kept, no epic `-work` branch.
        assert_eq!(pickup_path, temp.path().join("qci-spec-016"));
        assert_eq!(pickup_branch, "spec-016");
        assert!(!auto_path.contains("aida-spec016"), "{auto_path}");
        assert!(!auto_branch.ends_with("-work"), "{auto_branch}");
        assert!(crate::workflow_hints::branch_belongs_to_spec(
            &auto_branch,
            "SPEC-016"
        ));
    }

    #[test]
    fn bug_1628_pickup_resolver_skips_a_taken_branch_for_both_paths() {
        let (_temp, root) = temp_project("qci");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["branch", "spec-016"]);

        let driver = driver_for(&root, "SPEC-016");
        let (_, auto_branch) = auto_complete::PhaseDriver::implementer_workspace(&driver)
            .unwrap()
            .unwrap();
        let (_, pickup_branch) = resolve_pickup_workspace(&root, "SPEC-016", "auto").unwrap();
        assert_eq!(auto_branch, pickup_branch);
        assert_eq!(pickup_branch, "spec-016-2");
        assert!(crate::workflow_hints::branch_belongs_to_spec(
            &auto_branch,
            "SPEC-016"
        ));
    }
}

// trace:BUG-1629 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1629_phase1_recovery_tests.rs"]
mod bug_1629_phase1_recovery_tests;
