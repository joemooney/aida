pub(crate) fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

use aida_core::{
    check_migration_status,
    check_scaffold_status,
    determine_requirements_path,
    forbidden_attention_transition,
    merge_agents_md_aida_block,
    seed_meta_requirements,
    // trace:TASK-331 — shared atomic-write util, promoted from BUG-228's local copy
    write_atomic,
    AgentsMdBlockMerge,
    AttentionReason,
    Comment,
    DatabaseBackend,
    FieldChange,
    FileStatus,
    // GitLab integration
    GitLabClient,
    GitLabConfig,
    IdFormat,
    IssueFilter,
    IssueState,
    MigrationCheck,
    NumberingStrategy,
    RelationshipType,
    Requirement,
    RequirementPriority,
    RequirementStatus,
    RequirementType,
    RequirementsStore,
    ScaffoldConfig,
    Scaffolder,
    Storage,
};

use crate::cli::{
    AdvisorCommand, AgentCommand, AgentNewCommand, ApprovalCommand, BacklogCommand, BlockCommand,
    BriefCommand, CacheCommand, Cli, Command, CommentCommand, ConfigCommand, DbCommand,
    DepsCommand, DevCommand, DrainCommand, FindingsCommand, FocusCommand, GitHubCommand,
    GitLabCommand, GlyphCommand, GraphCommand, HeadlessCommand, HistoryCommand, IdentityCommand,
    JiraCommand, LoadCommand, MailboxCommand, McpCommand, MemoriesCommand, NodeCommand,
    OrchestratorCommand, OutputFormat, PlanCommand, PrCommand, PuntsCommand, QuestionsCommand,
    QueueCommand, RelationshipCommand, ReleaseCommand, ReviewCommand, RoleCommand,
    RolePromptCommand, RoleScopeCommand, ScaffoldCommand, SessionCommand, SessionManifestCommand,
    SkillCommand, SoloAction, SpecCommand, StackCommand, TeamCommand, TerminalCommand,
    TraceCommand, UpgradeCommand, UsageCommand, WorkerCommand, WorktreeCommand,
    WorktreePoolCommand, ZenCommand,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreAutoPushMode {
    Manual,
    SessionEnd,
    PerWrite,
    Periodic,
}

impl StoreAutoPushMode {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "session-end" | "session_end" | "sessionend" => Some(Self::SessionEnd),
            "per-write" | "per_write" | "perwrite" => Some(Self::PerWrite),
            "periodic" => Some(Self::Periodic),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::SessionEnd => "session-end",
            Self::PerWrite => "per-write",
            Self::Periodic => "periodic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreSyncConfig {
    pub(crate) auto_push: StoreAutoPushMode,
    pub(crate) periodic_threshold: Option<u64>,
    pub(crate) periodic_interval: Option<String>,
    // Extra remotes to mirror the store push to after `origin` succeeds — the
    // drift-prevention fan-out. Best-effort: a non-ff / unreachable mirror leg
    // warns, never fails the sync. trace:TASK-1096 | ai:claude
    pub(crate) mirror_remotes: Vec<String>,
    pub(crate) source: String,
}

impl Default for StoreSyncConfig {
    fn default() -> Self {
        Self {
            auto_push: StoreAutoPushMode::Manual,
            periodic_threshold: None,
            periodic_interval: None,
            mirror_remotes: Vec::new(),
            source: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreAllocationConfig {
    pub(crate) retry_max: usize,
}

impl Default for StoreAllocationConfig {
    fn default() -> Self {
        Self { retry_max: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdCollision {
    pub(crate) spec_id: String,
    pub(crate) claimants: Vec<SpecIdClaimant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdClaimant {
    pub(crate) uuid: Uuid,
    pub(crate) title: String,
}

/// BUG-1637: record a caller-authored (human-class) status transition
/// `from -> req.status` under the caller's identity ([`get_default_author`])
/// through the one shared helper, `aida_core::conflict::record_status_transition`.
/// Every human status writer (`aida edit --status`, MCP `update_requirement`,
/// queue done/rework/advance, findings, questions, punt) calls this, so a human
/// change after an automated one leaves mixed history and the BUG-1625 merge
/// guard lets the human win.
// trace:BUG-1637 | ai:claude
pub(crate) fn record_caller_status_transition(req: &mut Requirement, from: &RequirementStatus) {
    aida_core::conflict::record_status_transition(req, &get_default_author(), from);
}

/// BUG-1637: the `aida findings promote --auto-complete` persist step: record
/// the into-Completed transition under the caller and stamp `modified_at`.
// trace:BUG-1637 | ai:claude
pub(crate) fn record_promote_completion(
    req: &mut Requirement,
    prior: &RequirementStatus,
    now: chrono::DateTime<chrono::Utc>,
) {
    record_caller_status_transition(req, prior);
    req.modified_at = now;
}

/// BUG-1638: a caller-authored findings audit comment dated `now`.
// trace:BUG-1638 | ai:claude
pub(crate) fn findings_audit_comment(
    content: String,
    now: chrono::DateTime<chrono::Utc>,
) -> Comment {
    Comment {
        id: Uuid::now_v7(),
        content,
        author: get_default_author(),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    }
}

/// TASK-404: the optional "Promoted by" rationale comment for `reason`.
// trace:TASK-404 trace:BUG-1638 | ai:claude
pub(crate) fn findings_promote_reason_comment(
    reason: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Comment> {
    let text = reason.map(str::trim).filter(|s| !s.is_empty())?;
    Some(findings_audit_comment(
        format!(
            "Promoted by {author} {date}: {text}",
            author = get_default_author(),
            date = now.format("%Y-%m-%d")
        ),
        now,
    ))
}

/// BUG-1647: what `aida findings promote --auto-complete`'s write did.
// trace:BUG-1647 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromoteAutoComplete {
    /// The finding moved into Completed.
    Completed,
    /// It was already Completed; nothing was changed.
    AlreadyCompleted,
}

/// BUG-1638: `aida findings promote --auto-complete`'s write (TASK-579): the
/// audit comments and the caller-authored move into Completed, applied to the
/// copy re-read under the store lock and written as that one spec, so an edit
/// made after `req` was read is kept. Fails without writing when the finding
/// was deleted meanwhile.
///
/// BUG-1647: a finding already Completed under the lock gets no second audit
/// comment, history entry or ship record ([`PromoteAutoComplete::AlreadyCompleted`]);
/// one Rejected or Superseded is refused without writing.
// trace:TASK-579 trace:STORY-1418 trace:BUG-1637 trace:BUG-1638 trace:BUG-1647 | ai:claude
pub(crate) fn findings_promote_auto_complete_write(
    backend: &aida_core::CachedGitBackend,
    req: &Requirement,
    project_root: &std::path::Path,
    sha: &str,
    subject: &str,
    reason: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PromoteAutoComplete> {
    let display_id = req.display_id();
    let mut comments = vec![findings_audit_comment(
        format!(
            "Auto-completed on promote {date}: origin-ID fix already \
             merged ({sha} \"{subject}\"). No fresh work to queue.",
            date = now.format("%Y-%m-%d")
        ),
        now,
    )];
    comments.extend(findings_promote_reason_comment(reason, now));
    let outcome = completion::transition_to_completed_atomically(
        backend,
        req,
        Some(project_root),
        &display_id,
        sha,
        "promote",
        |r, prior| {
            r.comments.extend(comments);
            record_promote_completion(r, prior, now);
        },
    )?;
    match outcome {
        completion::AtomicCompletion::Completed => Ok(PromoteAutoComplete::Completed),
        completion::AtomicCompletion::AlreadyCompleted => Ok(PromoteAutoComplete::AlreadyCompleted),
        completion::AtomicCompletion::Gone => anyhow::bail!(
            "{display_id} no longer exists: it was deleted while promoting, so nothing was \
             changed."
        ),
        completion::AtomicCompletion::Refused(status) => anyhow::bail!(
            "{display_id} is now {status}, a final status, so it was not auto-completed; \
             nothing was changed."
        ),
    }
}

/// BUG-1638: `aida findings promote`'s Approved write: the optional rationale
/// comment and the caller-authored move to Approved, applied to the copy
/// re-read under the store lock and written as that one spec. Fails without
/// writing when the finding was deleted meanwhile.
///
/// BUG-1647: also fails without writing when the copy read under the lock is
/// in a final status ("is now X"). The error says why the write did not land;
/// the caller says what else was (or was not) undone.
// trace:BUG-231 trace:BUG-1637 trace:BUG-1638 trace:BUG-1647 | ai:claude
pub(crate) fn findings_promote_approve_write(
    backend: &aida_core::CachedGitBackend,
    req: &Requirement,
    reason: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let comment = findings_promote_reason_comment(reason, now);
    let mut terminal = None;
    let written = backend.update_spec_atomically(req, |r| {
        if aida_core::conflict::is_terminal_status(&r.status) {
            terminal = Some(r.status.clone());
            return;
        }
        r.comments.extend(comment);
        let from = std::mem::replace(&mut r.status, RequirementStatus::Approved);
        record_caller_status_transition(r, &from);
        r.modified_at = now;
    })?;
    if written.is_none() {
        anyhow::bail!(
            "{} no longer exists: it was deleted while promoting",
            req.display_id()
        );
    }
    if let Some(status) = terminal {
        anyhow::bail!(
            "{} is now {status}, a final status, so it was not promoted",
            req.display_id()
        );
    }
    Ok(())
}

/// BUG-1647: `aida findings promote --to work`: add the finding to the role's
/// work queue (BUG-231: queue first, so a queue failure leaves it a clean,
/// retryable draft), then write Approved. When the status write does not land
/// (the finding was deleted or reached a final status meanwhile, or the write
/// failed), the queue entry is withdrawn (any entry the spec had before is put
/// back as it was); when the withdrawal also fails, the error says the entry
/// remains and how to remove it. Returns the role it was queued for.
///
/// BUG-1651: the withdrawal is compare-and-swap. It re-reads the queue and
/// acts only while the spec's entry is still the one this call added, so a
/// concurrent `aida queue remove`/`done` or re-add in the window is kept and
/// the error says so.
// trace:BUG-231 trace:BUG-1638 trace:BUG-1647 trace:BUG-1651 | ai:claude
pub(crate) fn findings_promote_to_work(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    req: &Requirement,
    display_id: &str,
    for_role: Option<&str>,
    reason: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String> {
    let user_id = current_user_id(None);
    let storage = Storage::new(store_path);
    // `queue_add` upserts, so remember the entry it may replace.
    let prior_entry = storage
        .queue_list(&user_id, true)
        .ok()
        .and_then(|entries| entries.into_iter().find(|e| e.requirement_id == req.id));
    let ours = queue_promoted_finding_entry(store_path, req.id, display_id, for_role)?;
    let role = ours.for_role.clone().unwrap_or_default();
    // The race-test seam between the queue add and the status write.
    status_write_race_seam(store_path);
    if let Err(write_err) = findings_promote_approve_write(backend, req, reason, now) {
        let undo = withdraw_promoted_queue_entry(&storage, &user_id, &ours, prior_entry.as_ref());
        anyhow::bail!(
            "{}",
            promote_rollback_error(&write_err, &role, display_id, &undo)
        );
    }
    Ok(role)
}

/// BUG-1651: the error a promote whose status write failed returns, chosen
/// by what the queue withdrawal did. Only a failed withdrawal (the entry may
/// remain) suggests `aida queue remove`.
// trace:BUG-1651 | ai:claude
pub(crate) fn promote_rollback_error(
    write_err: &anyhow::Error,
    role: &str,
    display_id: &str,
    undo: &Result<PromoteQueueWithdrawal>,
) -> String {
    match undo {
        Ok(PromoteQueueWithdrawal::Withdrawn) => format!(
            "{write_err:#}. Its {role} queue entry was withdrawn; the finding is unchanged."
        ),
        Ok(PromoteQueueWithdrawal::WithdrawnPositionInexact(reorder_err)) => format!(
            "{write_err:#}. Its {role} queue entry was withdrawn and the earlier entry put \
             back, but its queue position could not be restored exactly ({reorder_err}); \
             check it with `aida queue list`. The finding is unchanged."
        ),
        Ok(PromoteQueueWithdrawal::LeftRemoved) => format!(
            "{write_err:#}. Its {role} queue entry had already been removed by someone \
             else meanwhile, so the queue was left as it is; the finding is unchanged."
        ),
        Ok(PromoteQueueWithdrawal::LeftReplaced) => format!(
            "{write_err:#}. Its queue entry was changed by someone else meanwhile, so it \
             was left as it is (check it with `aida queue list`); the finding is unchanged."
        ),
        Err(undo_err) => format!(
            "{write_err:#}. It had already been added to the {role} queue, and that entry \
             could not be withdrawn ({undo_err:#}); remove it with \
             `aida queue remove {display_id} --for {role}`."
        ),
    }
}

/// BUG-1651: what the compare-and-swap withdrawal of a promote's queue entry
/// did.
// trace:BUG-1651 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromoteQueueWithdrawal {
    /// The entry was still ours: it was removed, or the earlier entry put back.
    Withdrawn,
    /// The earlier entry was put back, but re-setting its exact `i64::MAX`
    /// position failed (the reason); it sits where `queue_add` placed it.
    WithdrawnPositionInexact(String),
    /// The spec has no queue entry any more; nothing was written.
    LeftRemoved,
    /// The spec's entry is no longer the one this call added; nothing was
    /// written.
    LeftReplaced,
}

/// BUG-1651: the entry `current` is the one `ours` wrote. Positions are not
/// compared: the backend resolves the append sentinel on write.
// trace:BUG-1651 | ai:claude
pub(crate) fn is_same_queue_entry(
    current: &aida_core::QueueEntry,
    ours: &aida_core::QueueEntry,
) -> bool {
    // Relies on the git backend round-tripping `for_role` and the exact
    // (nanosecond) `added_at` through its YAML queue file.
    current.requirement_id == ours.requirement_id
        && current.note == ours.note
        && current.added_at == ours.added_at
        && current.for_role == ours.for_role
}

/// BUG-1651: undo a promote's queue add only if the spec's entry is still the
/// one it added (`ours`): restore `prior` when there was one, otherwise remove
/// ours. Anything else means someone changed the entry meanwhile, and it is
/// left alone. Queue writes are not locked, so this narrows the window to the
/// re-read; it does not close it.
///
/// A `prior` entry positioned at `i64::MAX` (a legacy unresolved sentinel) is
/// put back at exactly that position: `queue_add` would re-resolve it to the
/// bottom, so the position is re-set with `queue_reorder`, which stores it as
/// given.
// trace:BUG-1651 | ai:claude
pub(crate) fn withdraw_promoted_queue_entry(
    storage: &Storage,
    user_id: &str,
    ours: &aida_core::QueueEntry,
    prior: Option<&aida_core::QueueEntry>,
) -> Result<PromoteQueueWithdrawal> {
    let current: Vec<aida_core::QueueEntry> = storage
        .queue_list(user_id, true)?
        .into_iter()
        .filter(|e| e.requirement_id == ours.requirement_id)
        .collect();
    if current.is_empty() {
        return Ok(PromoteQueueWithdrawal::LeftRemoved);
    }
    if !current.iter().any(|e| is_same_queue_entry(e, ours)) {
        return Ok(PromoteQueueWithdrawal::LeftReplaced);
    }
    match prior {
        Some(prior) => {
            storage.queue_add(prior.clone())?;
            // Best-effort: the entry is already back, so a failure here is
            // reported as an inexact position, not a failed withdrawal.
            if prior.position == i64::MAX {
                if let Err(e) = storage.queue_reorder(user_id, &[(prior.requirement_id, i64::MAX)])
                {
                    return Ok(PromoteQueueWithdrawal::WithdrawnPositionInexact(format!(
                        "{e:#}"
                    )));
                }
            }
        }
        None => storage.queue_remove_for_role(
            user_id,
            &ours.requirement_id,
            ours.for_role.as_deref(),
        )?,
    }
    Ok(PromoteQueueWithdrawal::Withdrawn)
}

/// BUG-1647: what `aida findings dismiss` did.
// trace:BUG-1647 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DismissOutcome {
    /// The finding moved to Rejected.
    Dismissed,
    /// It was already Rejected; nothing was changed.
    AlreadyDismissed,
}

/// BUG-1647: `aida findings dismiss`: the dismissal audit comment and the
/// caller-authored move to Rejected, applied to the copy re-read under the
/// store lock and written as that one spec (like promote since BUG-1638), so
/// an edit made after the finding was read is kept. Fails without writing
/// when the finding was deleted meanwhile.
///
/// The copy read under the lock decides, mirroring the promote Approved
/// write: already Rejected is a no-op ([`DismissOutcome::AlreadyDismissed`],
/// no second comment or history entry); Completed or Superseded is refused
/// ("is now X") and nothing is written.
// trace:TASK-404 trace:BUG-1637 trace:BUG-1647 | ai:claude
pub(crate) fn findings_dismiss(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    id: &str,
    reason: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<DismissOutcome> {
    let req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    if !findings::is_finding(&tags) {
        anyhow::bail!(
            "{id} is not a finding (no `from-review:`/`from-implementer:`/`from-advisor:` tag) — \
             `aida findings` only triages real findings. \
             Use `aida edit {id} --status rejected` for a general status change."
        );
    }
    // TASK-404: the bare "Dismissed" marker said nothing about *why*, so
    // rationale used to require a separate `aida comment add` — which most
    // dismissals skipped. With `--reason`, the rationale lands in the same
    // audit comment in one command.
    let content = match reason.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => format!(
            "Dismissed by {author} {date}: {text}",
            author = get_default_author(),
            date = now.format("%Y-%m-%d")
        ),
        None => "Dismissed by advisor during findings triage.".to_string(),
    };
    let comment = findings_audit_comment(content, now);
    // The race-test seam between the read and the write.
    status_write_race_seam(store_path);
    let mut seen: Option<RequirementStatus> = None;
    let written = backend.update_spec_atomically(&req, |r| {
        if aida_core::conflict::is_terminal_status(&r.status) {
            seen = Some(r.status.clone());
            return;
        }
        r.comments.push(comment);
        // BUG-1637: caller-authored.
        let from = std::mem::replace(&mut r.status, RequirementStatus::Rejected);
        record_caller_status_transition(r, &from);
        r.modified_at = now;
    })?;
    if written.is_none() {
        anyhow::bail!(
            "{id} no longer exists: it was deleted while dismissing, so nothing was changed."
        );
    }
    match seen {
        None => Ok(DismissOutcome::Dismissed),
        Some(RequirementStatus::Rejected) => Ok(DismissOutcome::AlreadyDismissed),
        Some(status) => anyhow::bail!(
            "{id} is now {status}, a final status, so it was not dismissed; nothing was changed."
        ),
    }
}

/// BUG-1637: record a legacy `aida edit`'s field changes under `author`: the
/// non-status fields as one history entry, the status change through
/// `aida_core::conflict::record_status_transition` (the one status-history
/// helper).
// trace:BUG-1637 | ai:claude
pub(crate) fn record_edit_changes(
    req: &mut Requirement,
    author: &str,
    changes: &[aida_core::FieldChange],
) {
    let others: Vec<aida_core::FieldChange> = changes
        .iter()
        .filter(|c| c.field_name != "status")
        .cloned()
        .collect();
    req.record_change(author.to_string(), others);
    if let Some(change) = changes.iter().find(|c| c.field_name == "status") {
        match parse_status(&change.old_value) {
            Ok(from) => aida_core::conflict::record_status_transition(req, author, &from),
            // Unreachable for the edit paths (they build the change from the
            // enum), but never drop a status change from the history.
            Err(_) => req.record_change(author.to_string(), vec![change.clone()]),
        }
    }
}

/// BUG-99: restore SIGPIPE's default behavior so `aida ... | head -N` exits
/// cleanly (status 141) instead of triggering Rust's "failed printing to
/// stdout: Broken pipe" panic. Rust deliberately ignores SIGPIPE by default
/// so library code can decide how to handle it, but for a Unix CLI we want
/// the classic "downstream closed, terminate quietly" semantics. Windows
/// has no SIGPIPE; this is a no-op there. trace:BUG-99 | ai:claude
#[cfg(unix)]
pub(crate) fn install_sigpipe_handler() {
    // Safety: signal() with SIG_DFL is async-signal-safe and is the
    // documented way to restore default disposition. Running here before
    // any output happens means the default kicks in for every println!
    // / eprintln! / write! in the binary.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub(crate) fn install_sigpipe_handler() {}

/// STORY-737 (delight #5): a sentinel error meaning "a soft, non-error signpost
/// was already rendered to stderr by the command; the top-level handler must NOT
/// re-print it as a red `Error:`". An empty queue on day one is the EXPECTED
/// state, not a failure — a brand-new user running `aida queue work` should get
/// a forward-pointing nudge, not a red error. The exit code still goes non-zero
/// (scripts that gate on it keep working); only the human-facing RENDER is
/// softened. The signpost itself is emitted at the call site (with the project's
/// info glyph) so this carries no message of its own.
// trace:STORY-737 | ai:claude
#[derive(Debug)]
pub(crate) struct SoftSignpostShown;

impl std::fmt::Display for SoftSignpostShown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Kept terse: only ever surfaces if some future caller wraps it in
        // context (the human render path suppresses it entirely).
        f.write_str("queue is empty")
    }
}

impl std::error::Error for SoftSignpostShown {}

/// BUG-1745: the command already wrote its COMPLETE typed document (JSON/TOON)
/// to stdout with the failure reason inside it. The global renderer must not
/// append a second document to stdout, and must not print a human `Error:` —
/// the payload already carries the reason. Exit code only.
// trace:BUG-1745 | ai:codex
#[derive(Debug)]
pub(crate) struct TypedPayloadEmitted;

impl std::fmt::Display for TypedPayloadEmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("typed payload already emitted")
    }
}

impl std::error::Error for TypedPayloadEmitted {}

/// STORY-737 (delight #4): should `aida history` hide the stateless internal
/// META prompt-template rows? Yes by default — they're plumbing seeded by
/// `aida init`, not user-authored work, and drown a fresh project's one real
/// spec (matching `aida list` and `aida status`, which already exclude them).
/// An explicit `--include-meta` OR `--type meta` overrides the hide so META
/// stays reachable.
// trace:STORY-737 | ai:claude
pub(crate) fn history_should_exclude_meta(include_meta: bool, type_filter: Option<&str>) -> bool {
    if include_meta {
        return false;
    }
    let asked_for_meta = type_filter
        .map(|t| t.eq_ignore_ascii_case("meta"))
        .unwrap_or(false);
    !asked_for_meta
}

// trace:BUG-1745 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1745_error_channel_tests.rs"]
mod bug_1745_error_channel_tests;
#[cfg(test)]
#[path = "tests/bug_1749_doctor_toon_tests.rs"]
mod bug_1749_doctor_toon_tests;
#[cfg(test)]
#[path = "tests/story_737_delight_tests.rs"]
mod story_737_delight_tests;
// trace:TASK-1555 | ai:claude
#[cfg(test)]
#[path = "tests/task_1555_ci_gate_tiers_tests.rs"]
mod task_1555_ci_gate_tiers_tests;
// trace:TASK-1562 | ai:claude
#[cfg(test)]
#[path = "tests/task_1562_worktree_reclaim_tests.rs"]
mod task_1562_worktree_reclaim_tests;
// trace:BUG-1723 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1723_config_trust_tests.rs"]
mod bug_1723_config_trust_tests;

/// The whole CLI: sigpipe setup, telemetry wrapping, error rendering,
/// dispatch. The `aida` binary is a stub that calls this — keeping the
/// entry point in the lib means only this one symbol is `pub` and every
/// `pub(crate)` item stays crate-private.
// trace:STORY-772 trace:ADR-16 | ai:claude
pub fn main_entry() {
    install_sigpipe_handler();
    // trace:BUG-766 | ai:claude
    // Export this binary's identity (AIDA_BIN + a PATH prepend of its
    // directory) into the process env BEFORE any command runs, so every
    // child this process spawns — claude/codex drive and drain sessions,
    // shells they open, nested `aida` invocations — resolves a bare `aida`
    // to THIS coordinating build instead of whatever stale installed binary
    // the raw non-interactive PATH finds first. A months-old installed
    // binary picked up that way ran unguarded bulk store saves (the writer
    // behind the deferred-shelf wipes); making the coordinating build win
    // PATH resolution closes that seam for every spawn site at once.
    export_coordinating_bin_env();
    register_filing_identity();
    // STORY-122: per-invocation telemetry. Wraps run() so every CLI
    // entry point gets recorded with cmd shape + duration + exit code.
    // Local-only; opt-out via `[telemetry] enabled = false` or
    // `AIDA_TELEMETRY=0`. Recording is best-effort and never aborts the
    // foreground command. trace:STORY-122 | ai:claude
    let argv: Vec<String> = std::env::args().collect();
    let started = std::time::Instant::now();

    // TASK-69: render anyhow-propagated errors in red instead of anyhow's
    // default plain-text formatter. Centralizes the coloring so every
    // `bail!` / `?` / `anyhow!` site automatically gets the highlight
    // without per-site refactoring. Exit code 1 on error, 0 on success.
    // trace:TASK-69 | ai:claude
    // trace:TASK-1526 | ai:codex
    let cache_scope = aida_core::db::cache_refresh::CacheReadScope::new();
    let exit_code: i32 = match run() {
        Ok(()) => 0,
        Err(err) if err.is::<aida_core::db::cache_refresh::AdvisoryCacheUnavailable>() => 0,
        Err(err) => {
            record_ambiguous_id_refusal(&err);
            let msg = format!("{:?}", err);
            // trace:BUG-1629 | ai:claude
            record_orchestrated_child_refusal(&msg);
            // TASK-972 (AXI #6): agents read STDOUT. An error printed to stderr
            // with a human `Error:` prefix is invisible to the agent loop,
            // forcing blind retries. In AGENT MODE emit the error as a
            // structured TOON `error:`/`help:` block on STDOUT instead, with an
            // actionable next-command suggestion where the message embeds one.
            // The exit code is unchanged (this only moves the OUTPUT channel),
            // and the human-at-a-TTY path below is byte-identical to before.
            // trace:TASK-972
            if err.downcast_ref::<TypedPayloadEmitted>().is_some() {
                // Print nothing on either channel. trace:BUG-1745 | ai:codex
            } else if agent_output_mode() {
                println!("{}", agent_error_block(&err, &msg));
            } else if err.downcast_ref::<SoftSignpostShown>().is_some() {
                // STORY-737 (delight #5): the command already rendered a soft,
                // forward-pointing signpost to stderr — re-printing it as a red
                // `Error:` would undo the whole point. Exit non-zero silently.
                // trace:STORY-737 | ai:claude
            } else {
                // Anyhow's Debug format prints the chain as
                //     summary
                //     \n
                //     Caused by:\n    inner1\n    inner2
                // Color the first non-empty line bold-red; dim the chain so
                // root summary stands out from causal background.
                let mut lines = msg.lines();
                if let Some(first) = lines.next() {
                    eprintln!("{} {}", "Error:".red().bold(), first.red());
                }
                for rest in lines {
                    eprintln!("{}", rest.dimmed());
                }
                // TASK-1082: when the error is a "Requirement not found" near-miss
                // of a real spec id, add a `did you mean <ID>?` line — the same
                // affordance clap gives for mistyped subcommands. Best-effort and
                // only on this branch; the exit code stays non-zero.
                // trace:TASK-1082 | ai:claude
                if let Some(hint) = did_you_mean_for_not_found(&msg) {
                    eprintln!("  {}", hint.dimmed());
                }
            }
            // BUG-1295: `aida pr rebase` bail sites that must be
            // distinguishable to a subprocess caller (conflict vs.
            // refused-force-push) wrap themselves in a `RebaseFailureExit`
            // instead of a bare `anyhow::bail!`. The printed message above is
            // unchanged either way — only the exit code this process returns
            // to its parent (the orchestrator's `attempt_phase3_auto_rebase`)
            // differs, so the parent can classify by exit code instead of by
            // matching this process's prose. Every other error keeps exit 1.
            // trace:BUG-1295 | ai:claude
            exit_code_for_error(&err)
        }
    };

    // trace:TASK-1526 | ai:codex
    cache_output::finish(&cache_scope, exit_code == 0, &argv);

    // STORY-122: append the usage record after the command completed
    // so we capture exit_code / duration_ms. Read the env-side opt-out
    // first; checking the project's `.aida/config.toml` requires a
    // project root, which not every invocation has, so fall back to
    // "no project context" gracefully.
    let project_root = find_main_worktree_root().ok();
    if usage::is_enabled(project_root.as_deref()) {
        let ev = usage::UsageEvent {
            ts: chrono::Utc::now().to_rfc3339(),
            cmd: usage::derive_cmd_shape(&argv),
            args_count: usage::count_args(&argv),
            exit_code,
            duration_ms: started.elapsed().as_millis() as u64,
            binary_sha: build_sha_short(),
            role: std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty()),
            scope: std::env::var("AIDA_SESSION_SCOPE")
                .ok()
                .filter(|s| !s.is_empty()),
            // BUG-1600: for `schedule tick` only, record which driver
            // invoked it (hook / cron / manual) alongside the exit_code and
            // duration_ms already captured above — a config error (e.g. an
            // unsupported flag on an installed cron entry) is now visible
            // in scheduler telemetry, not just as silent repeated failures.
            // trace:BUG-1600 | ai:claude
            schedule_source: usage::schedule_invocation_source(&argv),
        };
        usage::append_event(&ev);
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Register this binary's identity (version, build SHA, filing agent vendor)
/// with the core filing-provenance capture, so every spec this process files
/// is stamped with the tooling that filed it. Uses the existing version /
/// build-SHA helpers; best-effort — an unknown vendor is simply omitted.
// trace:CR-8 | ai:claude
pub(crate) fn register_filing_identity() {
    let vendor = agent_registry::detect_agent_type();
    aida_core::provenance::register_tool_identity(aida_core::provenance::ToolIdentity {
        version: Some(current_version().to_string()),
        build_sha: build_sha_short(),
        vendor: (vendor != "other").then_some(vendor),
    });
}

/// True when the working directory is inside the aida source repo itself —
/// the one project where a spec's filing code SHA and the filing binary's
/// build SHA describe the same code, so a mismatch is meaningful.
// trace:CR-8 | ai:claude
pub(crate) fn cwd_is_aida_source_repo() -> bool {
    std::env::current_dir()
        .map(|cwd| {
            cwd.ancestors().any(|dir| {
                dir.join("aida-cli-lib").join("build.rs").is_file()
                    && dir.join("aida-core").join("Cargo.toml").is_file()
            })
        })
        .unwrap_or(false)
}

pub(crate) const ASCIINEMA_WRAPPED_ENV: &str = "AIDA_ASCIINEMA_WRAPPED";
// Keep generated cast names readable while preventing pathological command
// lines from becoming filesystem-hostile. Truncated slugs end in "-trunc".
// trace:STORY-423 | ai:codex
pub(crate) const ASCIINEMA_SLUG_MAX_CHARS: usize = 80;

// EPIC-28: default safety cap for `--auto-complete --batch` drains — after
// this many phase failures shelve in one batch the drain stops rather than
// keep parking innocent specs because the environment is broken. The user
// can override per-invocation with `--max-failures N`, and `--max-failures 0`
// turns the cap off (falls back to the historical "first failure stops"
// behaviour). trace:EPIC-28 | ai:claude
pub(crate) const DEFAULT_MAX_FAILURES: usize = 5;

// SPIKE-70: `--sequential` names + guards the ordered, per-member-PR SHAPE of the
// batch drain (`auto_complete::drain_batch*`) rather than introducing a parallel
// knob. TASK-185: it does NOT pin concurrency — STORY-1091 made the batch drain
// honour `[drain] pipeline_depth`, so the one-member-at-a-time property comes from
// `drain_state::default_pipeline_depth()` (1), not from this flag. The old
// `SEQUENTIAL_DRAIN_CONCURRENCY` const asserted the pinned-to-1 invariant and was
// removed with the claim; read the default depth instead.
// trace:TASK-1005 trace:TASK-185 | ai:claude

// Requester intake must remain a standalone Draft. Both the CLI and MCP gates
// consume this list so relationship and grooming-field policy cannot drift.
// trace:BUG-1211 | ai:codex
pub(crate) const REQUESTER_INTAKE_FORBIDDEN_FIELDS: &[&str] = &["parent", "feature", "owner"];

// TASK-970: default row cap for a bare `aida list` in AGENT MODE. ~925
// unbounded rows is a token blowout when an agent reads the listing as
// context; cap to the N most-recent (post-sort, post-filter) and emit a
// `count: N of M` header + a widen hint. The human TTY path is unbounded
// (no surprise cap); an explicit `--limit`/`--all` always overrides.
// trace:TASK-970
pub(crate) const AGENT_LIST_DEFAULT_LIMIT: usize = 30;

// TASK-970: number of queued items the content-first bare `aida` (agent mode)
// lists beneath the status snapshot. trace:TASK-970
pub(crate) const AGENT_BARE_QUEUE_TOPN: usize = 5;

// trace:STORY-1028 | ai:codex
pub(crate) fn note_hidden_alias(old: &str, new: &str) {
    eprintln!("note: {old} is now {new}");
}

// trace:STORY-1028 | ai:codex
pub(crate) fn normalize_release_mode(
    patch: bool,
    minor: bool,
    major: bool,
    check: bool,
    cmd: Option<&ReleaseCommand>,
) -> (bool, bool, bool, bool) {
    match cmd {
        Some(ReleaseCommand::Patch) => (true, false, false, false),
        Some(ReleaseCommand::Minor) => (false, true, false, false),
        Some(ReleaseCommand::Major) => (false, false, true, false),
        Some(ReleaseCommand::Check) => (false, false, false, true),
        None => {
            if patch {
                note_hidden_alias(concat!("aida release ", "--patch"), "aida release patch");
            }
            if minor {
                note_hidden_alias(concat!("aida release ", "--minor"), "aida release minor");
            }
            if major {
                note_hidden_alias(concat!("aida release ", "--major"), "aida release major");
            }
            if check {
                note_hidden_alias(concat!("aida release ", "--check"), "aida release check");
            }
            (patch, minor, major, check)
        }
    }
}

// trace:STORY-1028 | ai:codex
pub(crate) fn normalize_upgrade_mode(
    check: bool,
    diff: bool,
    cmd: Option<&UpgradeCommand>,
) -> (bool, bool) {
    match cmd {
        Some(UpgradeCommand::Check) => (true, false),
        Some(UpgradeCommand::Diff) => (false, true),
        None => {
            if check {
                note_hidden_alias(concat!("aida upgrade ", "--check"), "aida upgrade check");
            }
            if diff {
                note_hidden_alias(concat!("aida upgrade ", "--diff"), "aida upgrade diff");
            }
            (check, diff)
        }
    }
}

// trace:STORY-1028 | ai:codex
#[allow(clippy::type_complexity)]
pub(crate) fn normalize_usage_mode<'a>(
    unused: Option<&'a str>,
    errors: bool,
    auto_complete: bool,
    failures: bool,
    pattern: bool,
    health: bool,
    slowest: bool,
    events: bool,
    action: Option<&'a UsageCommand>,
) -> (
    Option<&'a str>,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    match action {
        Some(UsageCommand::Rebuild { .. } | UsageCommand::Show { .. }) => {
            (None, false, false, false, false, false, false, false, false)
        }
        Some(UsageCommand::Slowest) => {
            (None, false, false, false, false, false, true, false, false)
        }
        Some(UsageCommand::Unused { duration }) => (
            Some(duration.as_str()),
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        ),
        Some(UsageCommand::Errors) => (None, true, false, false, false, false, false, false, false),
        Some(UsageCommand::Events) => (None, false, false, false, false, false, false, true, false),
        // TASK-1481: `aida usage timeline` — the compact one-line-per-invocation
        // view. Brand new surface (no legacy flag predates it), so it's
        // subcommand-only: no hidden `--timeline` alias to normalize.
        Some(UsageCommand::Timeline) => {
            (None, false, false, false, false, false, false, false, true)
        }
        Some(UsageCommand::Drains { failures, pattern }) => (
            None, false, true, *failures, *pattern, false, false, false, false,
        ),
        Some(UsageCommand::Health) => (None, false, false, false, false, true, false, false, false),
        None => {
            if slowest {
                note_hidden_alias(concat!("aida usage ", "--slowest"), "aida usage slowest");
            }
            if unused.is_some() {
                note_hidden_alias(concat!("aida usage ", "--unused"), "aida usage unused");
            }
            if errors {
                note_hidden_alias(concat!("aida usage ", "--errors"), "aida usage errors");
            }
            if events {
                note_hidden_alias(concat!("aida usage ", "--events"), "aida usage events");
            }
            if auto_complete {
                note_hidden_alias(
                    concat!("aida usage ", "--auto-complete"),
                    "aida usage drains",
                );
            }
            if health {
                note_hidden_alias(concat!("aida usage ", "--health"), "aida usage health");
            }
            (
                unused,
                errors,
                auto_complete,
                failures,
                pattern,
                health,
                slowest,
                events,
                false,
            )
        }
    }
}

/// TASK-1244 / ADR-41: map a merge-lease acquisition error to the drain's
/// typed failure. A `WouldBlock` (another live merger held the lease past the
/// bounded wait) is the shelvable `lease-conflict` cause — NeedsAttention for
/// triage, never retried as a transient. Any other IO fault (lock dir
/// unwritable, …) is a plain merge failure.
// trace:TASK-1244 | ai:claude
pub(crate) fn drain_merge_lease_failure(
    e: &std::io::Error,
    target: &str,
    pr: u32,
) -> auto_complete::PhaseFailure {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::LeaseConflict,
            format!("another merger holds the '{target}' merge-lease while merging PR-{pr}: {e}"),
        )
    } else {
        auto_complete::PhaseFailure::new(format!(
            "could not take the '{target}' merge-lease for PR-{pr}: {e}"
        ))
    }
}

/// BUG-1316: distinguish the substrate's deliberate supervised-hold refusal
/// from a forge CLI failure. The typed error is emitted at the single
/// `Forge::merge_change` chokepoint and its display includes both the persisted
/// hold reason and exact clear command, so carry that evidence into the hint.
// trace:BUG-1316 trace:BUG-1435 trace:BUG-1447 | ai:codex
pub(crate) fn classify_drain_merge_failure(
    forge: crate::forge::ForgeKind,
    pr: u32,
    error: &anyhow::Error,
) -> auto_complete::PhaseFailure {
    let detail = format!("{error:#}");
    if error
        .downcast_ref::<crate::forge::MergeHoldRefusal>()
        .is_some()
    {
        let reason = format!(
            "{} merge refused for {}-{pr}: {detail}",
            forge
                .cli_name()
                .is_empty()
                .then_some("pure-git")
                .unwrap_or(forge.cli_name()),
            forge.change_noun(),
        );
        return auto_complete::PhaseFailure::of(auto_complete::FailureKind::MergeHold, &reason)
            .with_hint_override(format!("A human/advisor merge-hold is open: {detail}"));
    }
    // GitHub's forge-enforced branch protection is the remote half of the
    // same supervised hold. It arrives as CLI text rather than our local typed
    // `MergeHoldRefusal`, so promote its distinctive policy detail to the same
    // non-transient kind before generic tool-exit handling.
    if forge == crate::forge::ForgeKind::GitHub
        && auto_complete::is_branch_policy_merge_refusal(&detail)
    {
        return auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::MergeHold,
            format!("gh merge refused for PR-{pr}: {detail}"),
        )
        .with_hint_override(format!(
            "A human/advisor merge-hold is enforcing branch policy on PR-{pr}. \
             Clear it after review with `aida merge-hold clear {pr}`."
        ));
    }
    let merge_tool = match forge.cli_name() {
        "" => "pure-git",
        cli => cli,
    };
    auto_complete::PhaseFailure::new(format!(
        "{merge_tool} merge failed for {}-{pr}: {detail}",
        forge.change_noun()
    ))
}

#[cfg(test)]
mod bug_1316_merge_hold_failure_tests {
    use super::*;

    #[test]
    fn task_1293_pr_2003_hold_is_typed_and_real_tool_failure_stays_retryable() {
        let production = crate::forge::MergeHoldRefusal::new(2003, "advisor hold: PR interaction");
        assert_eq!(
            production.to_string(),
            "refusing to merge PR-2003: supervised merge-hold — advisor hold: PR interaction. \
             A human/advisor must review, then clear the hold (`aida merge-hold clear 2003`) before merging."
        );

        // Deliberately unlike the production sentence: classification follows
        // the error type and cannot decay when the human-facing prose changes.
        // trace:BUG-1435 | ai:codex
        let held = anyhow::Error::new(crate::forge::MergeHoldRefusal::with_detail(
            2003,
            "advisor hold: PR interaction",
            "aida merge-hold clear 2003",
            "PR-2003 remains gated for advisor hold: PR interaction; release it with \
             `aida merge-hold clear 2003` after review.",
        ));
        let failure = classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2003, &held);
        assert_eq!(failure.kind, auto_complete::FailureKind::MergeHold);
        assert!(!auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
        let hint = failure.hint_override.expect("hold-specific recovery hint");
        assert!(hint.contains("advisor hold: PR interaction"), "{hint}");
        assert!(hint.contains("aida merge-hold clear 2003"), "{hint}");

        let tool = anyhow::anyhow!("HTTP 502 from GitHub while merging");
        let failure = classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2003, &tool);
        assert_eq!(failure.kind, auto_complete::FailureKind::Failed);
        assert!(auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
    }

    /// BUG-1447: the exact forge refusal observed on PR-2014 is the remote
    /// supervised-hold shape, despite sharing "not mergeable" with conflicts.
    // trace:BUG-1447 | ai:codex
    #[test]
    fn pr_2014_branch_policy_refusal_is_typed_with_clear_hint() {
        let observed = anyhow::anyhow!(
            "gh pr merge failed for #2014: X Pull request joemooney/aida#2014 is not mergeable:\n\
             the base branch policy prohibits the merge."
        );
        let failure =
            classify_drain_merge_failure(crate::forge::ForgeKind::GitHub, 2014, &observed);
        assert_eq!(failure.kind, auto_complete::FailureKind::MergeHold);
        assert!(!auto_complete::is_transient_retry_cause(
            failure.kind.cause_slug()
        ));
        assert!(!auto_complete::is_merge_conflict_failure(&failure.reason));
        let hint = failure.hint_override.expect("hold-specific recovery hint");
        assert!(hint.contains("merge-hold"), "{hint}");
        assert!(hint.contains("aida merge-hold clear 2014"), "{hint}");
    }
}

#[cfg(test)]
mod task_1244_drain_merge_lease_tests {
    use super::*;

    #[test]
    fn would_block_maps_to_shelvable_lease_conflict() {
        let blocked = std::io::Error::new(std::io::ErrorKind::WouldBlock, "held by a live merger");
        let f = drain_merge_lease_failure(&blocked, "main", 1875);
        assert_eq!(f.kind, auto_complete::FailureKind::LeaseConflict);
        assert!(f.kind.is_shelvable());
        assert!(
            f.reason.contains("PR-1875") && f.reason.contains("'main'"),
            "{}",
            f.reason
        );
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "locks dir");
        let f = drain_merge_lease_failure(&io, "main", 1875);
        assert_ne!(f.kind, auto_complete::FailureKind::LeaseConflict);
    }

    /// Source-shape guard: both AIDA merge paths take the lease before they
    /// merge. Needles are split so this file cannot match its own literals.
    #[test]
    fn drain_merge_and_wave_merge_take_the_merge_lease() {
        let src = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        let wave_start = src
            .find(concat!("fn merge_wave_pr", "(project_root"))
            .expect("wave fn");
        // Window widened for BUG-1562's typed spec on the wave hold.
        let wave_body = &src[wave_start..wave_start + 5000];
        let acquire = concat!("merge_lock::", "acquire(");
        let merge_call = concat!(".merge_change(", "&change_ref");
        let a = wave_body
            .find(acquire)
            .expect("wave merge acquires the lease");
        let m = wave_body.find(merge_call).expect("wave merge merges");
        assert!(a < m, "the wave lease must be taken BEFORE merge_change");
        let drain_start = src
            .find(concat!("\"aida queue work ", "(drain merge phase)\""))
            .expect("drain merge phase acquires the lease");
        let after = &src[drain_start..drain_start + 4000];
        assert!(
            after.contains(merge_call),
            "the drain merge_change follows the lease"
        );
    }
}

#[cfg(all(test, unix))]
mod bug_1265_finish_ci_tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: std::path::PathBuf,
        gh: std::path::PathBuf,
        mode: std::path::PathBuf,
        reads: std::path::PathBuf,
        calls: std::path::PathBuf,
    }

    /// Runs the production CI driver through the real drain orchestration
    /// boundary. Only the CI implementation is delegated: the remaining
    /// phases are unreachable because these regressions resume at phase 2 and
    /// stop at `ThroughCi`.
    // trace:BUG-1265 | ai:codex
    struct DrainHarness {
        real: RealPhaseDriver,
        finish_ci_calls: usize,
        shelf_calls: usize,
    }

    impl auto_complete::PhaseDriver for DrainHarness {
        fn run_implementer(
            &mut self,
        ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
            unreachable!("CI-resume harness must not run phase 1")
        }

        fn finish_ci(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            self.finish_ci_calls += 1;
            auto_complete::PhaseDriver::finish_ci(&mut self.real)
        }

        fn run_reviewer(
            &mut self,
        ) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must stop after phase 2")
        }

        fn merge(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not merge")
        }

        fn pull(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not pull")
        }

        fn build(&mut self) -> Result<(), auto_complete::PhaseFailure> {
            unreachable!("ThroughCi harness must not build")
        }

        fn hint_context(&self) -> auto_complete::HintContext {
            auto_complete::PhaseDriver::hint_context(&self.real)
        }

        fn transient_retry_budget(&self) -> usize {
            0
        }

        fn shelve_on_failure(
            &mut self,
            _spec: &str,
            phase: auto_complete::Phase,
            failure: &auto_complete::PhaseFailure,
            recovery_hint: &str,
        ) -> anyhow::Result<Option<aida_core::FailureReason>> {
            self.shelf_calls += 1;
            Ok(Some(aida_core::FailureReason {
                phase: phase.slug().to_string(),
                phase_index: phase.index() as u8,
                kind: failure.kind.cause_slug().to_string(),
                detail: failure.reason.clone(),
                recovery_hint: Some(recovery_hint.to_string()),
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            }))
        }
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("repo");
            std::fs::create_dir_all(&root).unwrap();
            let git = |args: &[&str]| {
                let out = std::process::Command::new("git")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "git {args:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            };
            git(&["init", "-q", "-b", "main"]);
            git(&[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/finish-ci-fixture.git",
            ]);

            let mode = temp.path().join("mode");
            let reads = temp.path().join("reads");
            let calls = temp.path().join("calls");
            let gh = temp.path().join("gh");
            let script = format!(
                r###"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
if [ "${{1:-}}" = "--version" ]; then echo 'gh version test'; exit 0; fi
if [ "${{1:-}} ${{2:-}}" = "pr view" ]; then
  printf '%s\n' '{{"state":"OPEN","title":"test","baseRefName":"main","headRefName":"bug-1265","headRefOid":"deadbeefdeadbeefdeadbeef","isCrossRepository":false,"isDraft":false}}'
  exit 0
fi
if [ "${{1:-}} ${{2:-}}" = "pr list" ]; then
  printf '%s\n' '[{{"number":1265,"statusCheckRollup":[{{"name":"merge-hold-gate","status":"COMPLETED","conclusion":"FAILURE"}}]}}]'
  exit 0
fi
if [ "${{1:-}} ${{2:-}}" = "pr checks" ]; then
  mode=$(cat '{}')
  if [ "$mode" = unavailable ]; then
    if printf '%s' "$*" | grep -q -- '--required'; then
      printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"pass"}}]'
      exit 1
    fi
    n=0; test ! -f '{}' || n=$(cat '{}'); n=$((n+1)); echo "$n" > '{}'
    echo 'temporary rows failure' >&2
    exit 1
  fi
  if [ "$mode" = real ]; then
    printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"fail"}}]'
  else
    printf '%s\n' '[{{"name":"merge-hold-gate","workflow":"merge-hold-gate","bucket":"fail"}},{{"name":"Build","workflow":"CI","bucket":"pass"}}]'
  fi
  exit 1
fi
echo "unexpected gh call: $*" >&2
exit 2
"###,
                calls.display(),
                mode.display(),
                reads.display(),
                reads.display(),
                reads.display()
            );
            crate::test_exec::write_executable(&gh, script);
            merge_hold::write_hold(&root, 1265, "supervised test hold").unwrap();
            Self {
                _temp: temp,
                root,
                gh,
                mode,
                reads,
                calls,
            }
        }

        fn driver(&self) -> RealPhaseDriver {
            let mut driver = RealPhaseDriver::new(
                self.root.clone(),
                "BUG-1265".into(),
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
            driver.branch = Some("bug-1265".into());
            driver.from_pr = true; // an already-open PR needs no worktree push
            driver
        }

        fn set_mode(&self, mode: &str) {
            std::fs::write(&self.mode, mode).unwrap();
            let _ = std::fs::remove_file(&self.reads);
            let _ = std::fs::remove_file(&self.calls);
        }

        fn run_drain(&self, mode: &str) -> (auto_complete::OrchestrationResult, usize, usize) {
            self.set_mode(mode);
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(self.gh.parent().unwrap().to_path_buf())
                    .chain(std::env::split_paths(&inherited)),
            )
            .unwrap();
            let gh = self.gh.to_string_lossy().into_owned();
            let path = path.to_string_lossy().into_owned();
            let _env = crate::test_env::EnvVarsGuard::set(&[
                ("AIDA_TEST_GH_BINARY", gh.as_str()),
                ("PATH", path.as_str()),
            ]);
            let mut harness = DrainHarness {
                real: self.driver(),
                finish_ci_calls: 0,
                shelf_calls: 0,
            };
            let result = auto_complete::orchestrate_with_resume(
                &mut harness,
                "BUG-1265",
                auto_complete::AutoCompleteVariant::ThroughCi,
                true,
                auto_complete::EscalateMode::Blocks,
                auto_complete::LifecycleSkip::none(),
                true,
                auto_complete::Phase::Ci,
            );
            (result, harness.finish_ci_calls, harness.shelf_calls)
        }
    }

    /// The fake `gh` must tolerate probes with fewer than two arguments and
    /// reach its normal fallback under `set -u` instead of aborting while
    /// expanding an unset positional parameter.
    //
    // `fixture.gh` is a script this test suite just wrote to disk, so the
    // spawn can race a concurrent fork that briefly still holds the file
    // open for writing (ETXTBSY) — the same class fixed at forge.rs and
    // pr_cmd.rs for production `gh` invocations. Production never hits this
    // because it launches an already-installed `gh`, never a freshly
    // written binary, so only the fixture side needs the retry. Route
    // through the same process_retry helper those sites use rather than
    // a bespoke loop.
    // trace:BUG-1544 | ai:claude
    // trace:BUG-1460 | ai:codex
    #[test]
    fn fake_gh_short_argv_reaches_fallback() {
        let fixture = Fixture::new();
        for args in [&[][..], &["pr"][..]] {
            let mut command = std::process::Command::new(&fixture.gh);
            command.args(args);
            let output = crate::test_exec::output(&mut command).unwrap();
            assert_eq!(output.status.code(), Some(2));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("unexpected gh call:"), "stderr: {stderr}");
            assert!(!stderr.contains("parameter not set"), "stderr: {stderr}");
        }
    }

    /// Exercise the real drain driver, not just `ci_gate::classify_red`: this
    /// is the seam that originally bypassed refinement and shelved the coarse
    /// hold-gate verdict as `ci-red`.
    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_completes_for_hold_gate_only_red() {
        let fixture = Fixture::new();
        assert!(
            merge_hold::read_hold(&fixture.root, 1265).is_some(),
            "the regression requires a real local supervised-hold marker"
        );
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("hold");
        assert_eq!(
            finish_ci_calls, 1,
            "drain must execute production finish_ci"
        );
        assert_eq!(shelf_calls, 0, "the supervised hold is not a failure shelf");
        assert!(result.failed_phase.is_none(), "{result:?}");
        assert!(result.shelved_reason.is_none(), "{result:?}");
        assert_eq!(result.process_exit_code(), auto_complete::DRIVE_EXIT_CLEAN);
        assert!(
            merge_hold::read_hold(&fixture.root, 1265).is_some(),
            "finish_ci must not clear the supervised hold marker"
        );
    }

    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_shelves_real_red_naming_only_the_genuine_check() {
        let fixture = Fixture::new();
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("real");
        assert_eq!(
            finish_ci_calls, 1,
            "drain must execute production finish_ci"
        );
        assert_eq!(shelf_calls, 1, "a genuine required red must shelf once");
        let shelf = result
            .shelved_reason
            .as_ref()
            .expect("genuine red must shelve");
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Ci));
        assert_eq!(
            result.process_exit_code(),
            auto_complete::DRIVE_EXIT_SHELVED
        );
        assert_eq!(shelf.kind, "ci-red");
        assert!(shelf.detail.contains("Build"), "{}", shelf.detail);
        assert!(
            !shelf.detail.contains("merge-hold-gate"),
            "{}",
            shelf.detail
        );
        let calls = std::fs::read_to_string(&fixture.calls).unwrap();
        assert!(
            calls
                .lines()
                .any(|call| call.contains("pr checks") && call.contains("--required")),
            "finish_ci must discover which red checks are genuinely required: {calls}"
        );
    }

    // trace:BUG-1265 | ai:codex
    #[test]
    fn drain_finish_ci_retries_unavailable_rows_then_shelves_ci_unavailable() {
        let fixture = Fixture::new();
        let (result, finish_ci_calls, shelf_calls) = fixture.run_drain("unavailable");
        assert_eq!(finish_ci_calls, 1, "row retries belong inside finish_ci");
        assert_eq!(
            shelf_calls, 1,
            "persistent row unavailability must shelf once"
        );
        let shelf = result
            .shelved_reason
            .as_ref()
            .expect("unavailable rows under a hold must shelve");
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Ci));
        assert_eq!(
            result.process_exit_code(),
            auto_complete::DRIVE_EXIT_SHELVED
        );
        assert_eq!(shelf.kind, "ci-unavailable");
        assert_ne!(shelf.kind, "ci-red");
        assert_eq!(
            std::fs::read_to_string(&fixture.reads).unwrap().trim(),
            "3",
            "initial row read plus two bounded retries"
        );
        let calls = std::fs::read_to_string(&fixture.calls).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|call| call.contains("pr checks") && !call.contains("--required"))
                .count(),
            3,
            "finish_ci must make the initial row read plus two bounded retries: {calls}"
        );
    }
}

#[cfg(test)]
mod bug_1205_ci_phase_fallthrough_tests {
    use super::*;

    #[test]
    fn headless_lease_conflict_is_decided_from_the_substrate() {
        assert_eq!(
            lease_conflict_decision(false, false),
            LeaseConflictDecision::Prompt
        );
        assert_eq!(
            lease_conflict_decision(false, true),
            LeaseConflictDecision::Prompt
        );
        assert_eq!(
            lease_conflict_decision(true, false),
            LeaseConflictDecision::AutoEnd
        );
        assert_eq!(
            lease_conflict_decision(true, true),
            LeaseConflictDecision::Refuse
        );
    }

    /// Source-shape guard: the CI phase's informational-only red must NOT
    /// return early — it falls through to the shared post-probe steps (ending
    /// the implementer session). Needles are split so this file cannot match
    /// its own literals.
    #[test]
    fn informational_red_falls_through_to_the_green_steps() {
        let src = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        // BUG-1265 added drain-level test harnesses that also `impl PhaseDriver`,
        // so searching the file for the first `fn finish_ci` now lands on a mock
        // whose body is a one-line delegation. Anchor on the REAL impl block
        // first, then find finish_ci inside it, so the guard keeps asserting the
        // production path no matter how many harnesses exist.
        // trace:BUG-1205 trace:BUG-1265 | ai:claude
        let real_impl = src
            .find(concat!(
                "impl auto_complete::PhaseDriver ",
                "for RealPhaseDriver {"
            ))
            .expect("real PhaseDriver impl present");
        let start = src[real_impl..]
            .find(concat!(
                "fn finish_ci",
                "(&mut self) -> Result<(), auto_complete::PhaseFailure>"
            ))
            .map(|i| real_impl + i)
            .expect("finish_ci present in the real impl");
        let end = src[start..]
            .find(concat!("    fn ", "run_reviewer("))
            .map(|e| start + e)
            .unwrap_or(src.len());
        let body = &src[start..end];
        let red_arm = body.find(concat!("CiProbe::", "Red {")).expect("Red arm");
        let not_failure = body[red_arm..]
            .find(concat!("is not a ", "failure"))
            .map(|i| red_arm + i)
            .expect("not-a-failure note");
        let window = &body[not_failure..not_failure + 400];
        assert!(
            !window.contains(concat!("return ", "Ok(())")),
            "early return is back"
        );
        assert!(body.contains(concat!("let refined_", "green")));
        assert!(body.contains(concat!("self.end_implementer_", "session()")));
    }
}

#[cfg(test)]
mod bug_1195_review_story_lookup_tests {
    use super::*;

    fn story(title: &str, status: aida_core::RequirementStatus) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new(title.to_string(), String::new());
        r.req_type = aida_core::RequirementType::Story;
        r.status = status;
        r.spec_id = Some("STORY-1191".to_string());
        r
    }
    fn entry(user: &str, id: uuid::Uuid) -> aida_core::QueueEntry {
        aida_core::QueueEntry {
            user_id: user.to_string(),
            requirement_id: id,
            position: 1000,
            added_by: user.to_string(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        }
    }

    // BUG-1195 / BUG-1193 shape: the review story was filed by a sibling
    // worktree session under `joe`, the drain looks it up as
    // `role:implementer`. The role-fallback listing hands the classifier the
    // foreign entry; it must be Found — not "no story".
    #[test]
    fn review_story_filed_by_another_user_is_found_through_the_role_route() {
        let s = story(
            "Review PR-1901: [AI:codex] fix(tests): normalize review envelope source guard (BUG-1193)",
            aida_core::RequirementStatus::Approved,
        );
        let store = RequirementsStore {
            requirements: vec![s.clone()],
            ..Default::default()
        };
        let foreign = vec![entry("joe", s.id)];
        assert_eq!(
            classify_review_story_lookup(&foreign, &store, ReviewForge::GitHub, 1901),
            ReviewStoryLookup::Found("STORY-1191".to_string())
        );
        assert!(matches!(
            classify_review_story_lookup(&foreign, &store, ReviewForge::GitHub, 1900),
            ReviewStoryLookup::NoTitleMatch { entries: 1 }
        ));
        assert_eq!(
            classify_review_story_lookup(&[], &store, ReviewForge::GitHub, 1901),
            ReviewStoryLookup::NoEntries
        );
        let done = story("Review PR-1901: x", aida_core::RequirementStatus::Done);
        let store = RequirementsStore {
            requirements: vec![done.clone()],
            ..Default::default()
        };
        match classify_review_story_lookup(
            &[entry("joe", done.id)],
            &store,
            ReviewForge::GitHub,
            1901,
        ) {
            ReviewStoryLookup::NotPickable { story, reason } => {
                assert_eq!(story, "STORY-1191");
                assert!(reason.contains("awaiting merge"), "{reason}");
            }
            other => panic!("expected NotPickable, got {other:?}"),
        }
        assert!(ReviewStoryLookup::Failed("boom".into())
            .describe()
            .contains("lookup failed"));
    }

    /// Source-shape guard: the lookup reads the queue through the role
    /// fallback (the plan-builder's view), never the bare per-user list.
    #[test]
    fn review_story_lookup_uses_the_role_fallback_listing() {
        let src = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        let start = src
            .find(concat!("fn queued_review_story_for_pr", "("))
            .unwrap();
        let body = &src[start..start + 1200];
        assert!(body.contains(concat!("queue_list_with_role_", "fallback(")));
        assert!(!body.contains(concat!("storage.queue_", "list(")));
    }
}

#[cfg(test)]
mod bug_1173_detection_hold_tests {
    // BUG-1236 (supersedes the BUG-1173 guard this module held): set_pr_number stamps the
    // marker AND mirrors the aida:merge-hold LABEL at PR-detection. The early
    // label is safe since BUG-1180 / ADR-39 (the CI watch and `aida pr ship`
    // classify a red merge-hold-gate as the hold when the marker exists), and
    // deferring it let three supervised PRs go unlabelled while the required
    // check read pass. A sync failure must be SURFACED (the `--fix` hint), never
    // swallowed. trace:BUG-1173 trace:BUG-1236 | ai:claude
    #[test]
    fn set_pr_number_stamps_marker_and_syncs_label_surfacing_failure() {
        let src = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        // build the needle from split pieces so this test's own source cannot self-match
        // (the test module sits before the real function in the file).
        let needle = concat!("fn set_pr_number", "(&mut self, pr: u32) {");
        let start = src.find(needle).expect("set_pr_number present");
        // bound the body at the next 4-space-indented method so we read ONLY set_pr_number.
        let after = &src[start + 38..];
        let end = after
            .find("    fn ")
            .map(|e| start + 38 + e)
            .unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            body.contains("merge_hold::write_typed_hold")
                && body.contains("HoldReasonKind::Supervision"),
            "set_pr_number must stamp a typed supervision marker at PR-detection"
        );
        assert!(
            body.contains("merge_hold::sync_label"),
            "BUG-1236 regression: set_pr_number must mirror the aida:merge-hold LABEL at \
             PR-detection so the required merge-hold-gate check enforces Layer 2"
        );
        assert!(
            body.contains("merge-hold list --fix"),
            "a label sync failure must be surfaced with the repair hint, never swallowed"
        );
    }
}

#[cfg(test)]
mod story_1028_mode_alias_tests {
    use super::*;

    // trace:STORY-1028 | ai:codex
    #[test]
    fn release_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_release_mode(true, false, false, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Patch))
        );
        assert_eq!(
            normalize_release_mode(false, true, false, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Minor))
        );
        assert_eq!(
            normalize_release_mode(false, false, true, false, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Major))
        );
        assert_eq!(
            normalize_release_mode(false, false, false, true, None),
            normalize_release_mode(false, false, false, false, Some(&ReleaseCommand::Check))
        );
    }

    // trace:STORY-1028 | ai:codex
    #[test]
    fn upgrade_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_upgrade_mode(true, false, None),
            normalize_upgrade_mode(false, false, Some(&UpgradeCommand::Check))
        );
        assert_eq!(
            normalize_upgrade_mode(false, true, None),
            normalize_upgrade_mode(false, false, Some(&UpgradeCommand::Diff))
        );
    }

    // trace:STORY-1028 | ai:codex
    #[test]
    fn usage_hidden_flags_normalize_like_subcommands() {
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, false, true, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Slowest)
            )
        );
        assert_eq!(
            normalize_usage_mode(
                Some("30d"),
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                None
            ),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Unused {
                    duration: "30d".to_string()
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, true, false, false, false, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Errors)
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, false, false, true, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Events)
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, true, true, false, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Drains {
                    failures: true,
                    pattern: false
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, true, false, true, false, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Drains {
                    failures: false,
                    pattern: true
                })
            )
        );
        assert_eq!(
            normalize_usage_mode(None, false, false, false, false, true, false, false, None),
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Health)
            )
        );
        // TASK-1481: `timeline` is subcommand-only (no legacy flag predates
        // it) — assert it flips only the new 9th (timeline) slot.
        assert_eq!(
            normalize_usage_mode(
                None,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
                Some(&UsageCommand::Timeline)
            ),
            (None, false, false, false, false, false, false, false, true)
        );
    }
}

/// TASK-970: the agent-ergonomics output gate. Two AIDA surfaces lean toward
/// agent-friendly output when the caller is a non-interactive agent rather than
/// a human at a TTY: bare `aida` routes to the status snapshot (not the
/// getting-started menu), and bare `aida list` applies a default row cap.
///
/// AGENT MODE is true when EITHER `AIDA_AGENT_OUTPUT` is set to a truthy value
/// OR stdout is not a TTY (piped / headless / MCP). `AIDA_AGENT_OUTPUT=0`
/// (or `false`/`no`/`off`) force-selects the HUMAN path even when piped, which
/// also makes the human path testable without a real terminal. The human-at-a-
/// TTY path is left byte-identical; everything gated on this is agent-only.
// trace:TASK-970
pub(crate) fn agent_output_mode_quiet() -> bool {
    let pin = output_format_override();
    let env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    let stdout_is_tty = std::io::stdout().is_terminal();
    resolve_agent_mode(pin, env.as_deref(), stdout_is_tty)
}

pub(crate) fn agent_output_mode() -> bool {
    // STORY-764: an explicit `--format` / `AIDA_OUTPUT_FORMAT` pin wins over the
    // `AIDA_AGENT_OUTPUT` env + TTY default. `human` selects the human path;
    // `toon` / `json` both select the agent (machine) path. This is the durable
    // escape hatch scripts use so piped/captured output no longer silently
    // switches format out from under them (BUG-707). trace:STORY-764 | ai:claude
    let pin = output_format_override();
    let env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    let stdout_is_tty = std::io::stdout().is_terminal();
    let agent = resolve_agent_mode(pin, env.as_deref(), stdout_is_tty);
    // STORY-764: loud-but-once. When we auto-switched to the compact agent format
    // purely because stdout is not a terminal — no explicit pin, no explicit
    // `AIDA_AGENT_OUTPUT` — nudge the caller (on STDERR, so piped STDOUT stays
    // clean) toward `--format` the first time. trace:STORY-764 | ai:claude
    if agent && pin.is_none() && env.is_none() && !stdout_is_tty {
        maybe_emit_toon_switch_hint();
    }
    agent
}

/// STORY-764: pure core of the agent-mode decision with the explicit `--format`
/// pin layered on top of the historical `AIDA_AGENT_OUTPUT`/TTY logic. An
/// explicit pin wins outright (`human` → human path, `toon`/`json` → agent
/// path); with no pin the behavior is byte-identical to [`agent_output_mode_from`].
/// Testable without touching the real env or a real terminal.
// trace:STORY-764 | ai:claude
pub(crate) fn resolve_agent_mode(
    pin: Option<OutputFormat>,
    agent_env: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    if let Some(fmt) = pin {
        return !matches!(fmt, OutputFormat::Human);
    }
    agent_output_mode_from(agent_env, stdout_is_tty)
}

/// STORY-764: process-global explicit output-format override, installed exactly
/// once from the global `--format` flag (or `AIDA_OUTPUT_FORMAT`) right after
/// argv parsing. Everything that renders consults [`agent_output_mode`] /
/// [`output_format_is_json`], which read this first — so the pin is honored
/// uniformly without threading a format param through every handler.
// trace:STORY-764 | ai:claude
pub(crate) static OUTPUT_FORMAT_OVERRIDE: std::sync::OnceLock<Option<OutputFormat>> =
    std::sync::OnceLock::new();

/// Resolve + install the [`OUTPUT_FORMAT_OVERRIDE`] once. Precedence: the
/// `--format` flag wins; else `AIDA_OUTPUT_FORMAT` (human|toon|json,
/// case-insensitive); else `None` (fall back to the TTY-based default).
/// Idempotent — a later call is a no-op (the first install sticks).
// trace:STORY-764 | ai:claude
pub(crate) fn set_output_format_override(flag: Option<OutputFormat>) {
    let resolved = flag.or_else(output_format_from_env);
    let _ = OUTPUT_FORMAT_OVERRIDE.set(resolved);
}

/// Parse `AIDA_OUTPUT_FORMAT` into an [`OutputFormat`]. Unset or unrecognized
/// values yield `None` (defer to the TTY-based default rather than erroring, so
/// a stray value never breaks a command).
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_from_env() -> Option<OutputFormat> {
    parse_output_format(std::env::var("AIDA_OUTPUT_FORMAT").ok().as_deref())
}

/// Pure parse of an `AIDA_OUTPUT_FORMAT` value into an [`OutputFormat`]. Unset
/// (`None`) or an unrecognized value yields `None` (defer to the TTY-based
/// default rather than erroring). Case-insensitive; `table`/`agent` are
/// accepted spellings of `human`/`toon`.
// trace:STORY-764 | ai:claude
pub(crate) fn parse_output_format(raw: Option<&str>) -> Option<OutputFormat> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "human" | "table" => Some(OutputFormat::Human),
        "toon" | "agent" => Some(OutputFormat::Toon),
        "json" => Some(OutputFormat::Json),
        _ => None,
    }
}

/// The installed explicit override, if any. `None` until
/// [`set_output_format_override`] runs (or if no pin was given).
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_override() -> Option<OutputFormat> {
    OUTPUT_FORMAT_OVERRIDE.get().copied().flatten()
}

/// True when the explicit pin selects JSON. Commands that carry a `--json`
/// flag OR this in, so `--format json` / `AIDA_OUTPUT_FORMAT=json` reaches them
/// uniformly.
// trace:STORY-764 | ai:claude
pub(crate) fn output_format_is_json() -> bool {
    matches!(output_format_override(), Some(OutputFormat::Json))
}

/// Resolve the selected clap leaf and reject a JSON format pin unless that
/// leaf advertises a dedicated JSON projection. `--format` is global, so clap
/// otherwise accepts it for every command and lets unsupported handlers emit
/// successful human/TOON text. A local `--json` argument is the executable
/// declaration that the leaf owns a machine contract; the renderer must then
/// make both spellings identical.
// trace:BUG-1502 | ai:codex
pub(crate) fn enforce_json_format_capability(argv: &mut Vec<String>) -> Result<()> {
    if !output_format_is_json() {
        return Ok(());
    }

    use clap::CommandFactory;
    let root = Cli::command();
    let matches = root.clone().try_get_matches_from(argv.clone())?;
    let mut command = &root;
    let mut selected = &matches;
    let mut path = vec!["aida".to_string()];
    // BUG-1631: a `global = true` `--json` on an ancestor (e.g. `aida
    // history`) also covers its subcommands (`aida history events`).
    // trace:BUG-1631 | ai:claude
    let mut inherited_json = false;
    while let Some((name, submatches)) = selected.subcommand() {
        inherited_json |= command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json" && argument.is_global_set());
        let Some(subcommand) = command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == name)
        else {
            anyhow::bail!(
                "could not resolve JSON capability for command `{}`",
                path.join(" ")
            );
        };
        path.push(name.to_string());
        command = subcommand;
        selected = submatches;
    }

    let supports_json = inherited_json
        || command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json");
    if !supports_json {
        anyhow::bail!(
            "`{} --format json` is unsupported: this command has no JSON projection; use `--format human` or `--format toon`",
            path.join(" ")
        );
    }

    // Materialize the global spelling as the selected leaf's dedicated flag.
    // This is the load-bearing dispatch guarantee: every existing handler sees
    // exactly the same parsed boolean for `--format json` as for `--json`, so
    // capability cannot drift from renderer wiring (for example, fasttrack
    // status previously declared `--json` but only inspected that local bool).
    // trace:BUG-1502 | ai:codex
    if !selected.get_flag("json") {
        argv.push("--json".to_string());
    }
    Ok(())
}

/// Build and dispatch the shared tail resolver from clap-parsed arguments.
/// Both `aida tail drain` and `aida drain tail` enter here, then flow through
/// `tail_cmd::handle_tail`.
// trace:TASK-1209 | ai:codex
pub(crate) fn handle_tail_cli(
    target: Option<String>,
    list: bool,
    json: bool,
    lines: Option<usize>,
    since: Option<&str>,
    no_follow: bool,
    with_tools: bool,
    no_timestamp: bool,
    annotate: bool,
) -> Result<()> {
    // trace:BUG-1289 | ai:claude
    let json = json || output_format_is_json();
    let project_root = find_main_worktree_root()
        .or_else(|_| find_project_root())
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let since_duration = match since {
        Some(s) => Some(headless_tail::parse_since(s)?),
        None => None,
    };
    let sessions: Vec<tail_cmd::SessionRef> = list_leases(&project_root)
        .into_iter()
        .map(|l| tail_cmd::SessionRef {
            id: l.id,
            scope: l.scope,
            branch: l.branch,
            role: l.role,
        })
        .collect();
    let opts = tail_cmd::TailOptions {
        target,
        list,
        json,
        lines,
        since: since_duration,
        no_follow,
        with_tools,
        color: std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none(),
        no_timestamp,
        annotate,
    };
    tail_cmd::handle_tail(&project_root, sessions, &opts)
}

/// STORY-764: emit the first-pipe format hint at most once. Guarded first by a
/// process-once `OnceLock`, then by a per-project marker file under `.aida/` so
/// the nudge shows once and then gets out of the way (the durable reference is
/// the README + `docs/environment-variables.md`). All best-effort: a missing
/// project root or an unwritable marker just falls back to once-per-process.
// trace:STORY-764 | ai:claude
pub(crate) fn maybe_emit_toon_switch_hint() {
    static PROCESS_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if PROCESS_ONCE.set(()).is_err() {
        return; // already fired in this process
    }
    // Per-session/project suppression: skip if we've shown it here before.
    // Fall back to global if run outside a project. trace:BUG-1813 | ai:antigravity
    let marker = find_project_root()
        .ok()
        .map(|root| root.join(".aida").join(".format-hint-shown"))
        .or_else(|| crate::home_dir().map(|h| h.join(".aida").join(".format-hint-shown")));
    if let Some(path) = &marker {
        if path.exists() {
            return;
        }
    }
    eprintln!(
        "note: stdout is not a terminal — aida switched to compact TOON output. \
         Pin a format with `--format human|toon|json` or AIDA_OUTPUT_FORMAT to keep scripts stable."
    );
    if let Some(path) = &marker {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, b"");
    }
}

/// Pure core of [`agent_output_mode`] (testable without touching the real env
/// or a real terminal). `env` is the `AIDA_AGENT_OUTPUT` value (None = unset);
/// `stdout_is_tty` is whether stdout is a terminal.
// trace:TASK-970
pub(crate) fn agent_output_mode_from(env: Option<&str>, stdout_is_tty: bool) -> bool {
    match env {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        None => !stdout_is_tty,
    }
}

/// True when there is no human at the keyboard to answer a `Type 'y' to
/// confirm:` prompt — agent output mode is forced, or stdin is not a TTY. In
/// this state reading the prompt only hits EOF: the answer is never 'y', so the
/// historical `if !yes { read_line }` gate CANCELLED the write while an agent
/// capturing stdout (the "Cancelled" notice went to stderr) believed it
/// succeeded — a silent no-op that derails the implement -> done -> merge chain.
/// Callers must branch on this up front: AUTO-CONFIRM a reversible write (the
/// agent explicitly invoked the verb), or FAIL LOUDLY on a destructive one.
/// Never silently cancel a non-TTY write.
// trace:BUG-671
pub(crate) fn non_interactive_confirm() -> bool {
    non_interactive_confirm_from(agent_output_mode(), std::io::stdin().is_terminal())
}

/// Pure core of [`non_interactive_confirm`] (testable without a real terminal).
/// `agent_mode` is [`agent_output_mode`]'s verdict; `stdin_is_tty` is whether
/// stdin is a terminal.
// trace:BUG-671
pub(crate) fn non_interactive_confirm_from(agent_mode: bool, stdin_is_tty: bool) -> bool {
    agent_mode || !stdin_is_tty
}

/// BUG-721: gate for the interactive `aida review <spec>` verb's reviewer
/// launch. The verb spawns a `claude -p` reviewer subprocess against the live
/// repo and then drives a follow-up menu — it needs a human at the terminal to
/// watch that reviewer and answer. Returns true (may launch) only when BOTH
/// stdin and stdout are TTYs. When it returns false the verb must NOT spawn a
/// reviewer: a non-interactive `aida review` that silently launched a blind
/// headless reviewer the caller could neither see nor interrupt is exactly the
/// bug. This gate covers ONLY the interactive verb — the orchestrator's drain
/// review phase never reaches this code (it shells out to `aida queue work
/// PR-N` with `AIDA_AUTO_COMPLETE=1` and launches its headless reviewer there
/// on purpose, `run_reviewer`), so the autonomous drain is untouched.
// trace:BUG-721
pub(crate) fn review_may_launch_reviewer(stdin_is_tty: bool, stdout_is_tty: bool) -> bool {
    stdin_is_tty && stdout_is_tty
}

/// TASK-972 (AXI #6): split a formatted anyhow error into the one-line summary
/// the agent error block shows plus an optional suggested next command. The
/// summary is the first line with any leading human `Error:` prefix stripped
/// (several `bail!` sites embed one, which would otherwise read `error: Error:
/// …` once nested under the TOON `error:` key). The suggestion reuses the rich
/// hints the not_found module and friends already embed — see
/// [`extract_aida_suggestion`] — so "where known" comes for free.
// trace:TASK-972
/// BUG-684: the soft-signpost line for a truly-EMPTY `aida list` (human path).
/// Returns `Some(line)` only when nothing is merely hidden behind a filter —
/// a fresh zero-spec repo — so the hint teaches the create move. When rows exist
/// but are filtered out (closed/archived/deferred), returns `None` because the
/// hidden-hints already point at `--all`. Pure, so the empty-state decision is
/// unit-testable without the surrounding render machinery.
// trace:BUG-684 | ai:claude
// BUG-781: the accepted-decision axis counts as "hidden behind a filter" too —
// a project whose only remaining specs are ratified ADRs is not a fresh repo,
// so it must not be told to file its first spec.
// trace:BUG-781 | ai:claude
pub(crate) fn empty_list_hint_line(
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Option<&'static str> {
    if closed_hidden == 0
        && archived_hidden == 0
        && deferred_hidden == 0
        && accepted_decisions_hidden == 0
    {
        Some("Nothing here yet — file your first spec: aida add --title \"...\"")
    } else {
        None
    }
}

/// BUG-783: build the `aida list` footer hint lines, in render order.
///
/// Two families live here and they are NOT the same thing:
///
/// * **Lens hints** — closed rows and accepted (terminal) decisions the default
///   OPEN lens drops. These explain why a spec the user KNOWS exists is absent
///   from the rows they just asked for, so they always print.
/// * **View-tier hints** — the archived and deferred tiers. On an explicit
///   open-work request these are noise: the operator asked for open work and
///   does not need reminding on every invocation that the other two tiers
///   exist. Suppressed unless `[list] show_hidden_hints = true` opts back in.
///
/// The tiers stay reachable either way — `--archived`, `--deferred` and `--all`
/// are unaffected (they clear the default lens entirely, so nothing is hidden
/// and no hint applies), and each prints its own row count.
///
/// Returns undecorated lines (2-space indented); the caller dims and prints.
/// Pure so the "default view is quiet" contract is unit-testable without a
/// cache DB or a terminal.
// trace:BUG-783 | ai:claude
pub(crate) fn list_hidden_hint_lines(
    show_view_tier_hints: bool,
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    // STORY-723: the open lens hides closed history — say so.
    if closed_hidden > 0 {
        lines.push(format!(
            "  ({closed_hidden} closed hidden — open lens; pass --all or `--status closed` to see them)"
        ));
    }
    // STORY-441 / STORY-584: the archived + deferred view tiers. Opt-in.
    if show_view_tier_hints {
        if archived_hidden > 0 {
            lines.push(format!(
                "  ({archived_hidden} archived hidden — pass --all or --archived to see them)"
            ));
        }
        if deferred_hidden > 0 {
            lines.push(format!(
                "  ({deferred_hidden} deferred hidden — pass --all or --deferred to see them)"
            ));
        }
    }
    // BUG-781: accepted (terminal) decisions are hidden by the open lens — say
    // so, and name the flag that brings them back.
    if accepted_decisions_hidden > 0 {
        lines.push(format!(
            "  ({accepted_decisions_hidden} accepted decision{} hidden — pass --all or --type decision to see them)",
            if accepted_decisions_hidden == 1 { "" } else { "s" }
        ));
    }
    lines
}

#[cfg(test)]
#[path = "tests/bug_783_list_hidden_hints_tests.rs"]
mod bug_783_list_hidden_hints_tests;

/// Render the agent-mode (TOON) error block for `err`, whose Debug-formatted
/// text is `msg`. A typed error that carries its own help list renders every
/// item, so multi-line guidance survives; anything else keeps the TASK-972
/// shape of a one-line summary plus the first embedded `aida …` command.
// trace:TASK-972 trace:TASK-1486 | ai:claude
pub(crate) fn agent_error_block(err: &anyhow::Error, msg: &str) -> String {
    if let Some(no_project) = err
        .chain()
        .find_map(|e| e.downcast_ref::<aida_core::NoProjectFound>())
    {
        return toon::error_block_with_help_list(no_project.summary(), &no_project.help_lines());
    }
    let (summary, help) = agent_error_summary_help(msg);
    // TASK-1082: fold a did-you-mean suggestion into the agent-mode
    // summary when a "Requirement not found" error is a near-miss of
    // a real spec id. trace:TASK-1082 | ai:claude
    let summary_owned = match did_you_mean_for_not_found(msg) {
        Some(hint) => format!("{summary} ({hint})"),
        None => summary.to_string(),
    };
    toon::error_block(&summary_owned, help.as_deref())
}

pub(crate) fn agent_error_summary_help(msg: &str) -> (&str, Option<String>) {
    let first = msg.lines().next().unwrap_or("").trim();
    let summary = first
        .strip_prefix("Error: ")
        .or_else(|| first.strip_prefix("error: "))
        .unwrap_or(first);
    (summary, extract_aida_suggestion(msg))
}

/// TASK-972: pull the first backtick-quoted `aida …` command out of an error
/// message, to reuse as the agent error block's `help:` next-command
/// suggestion. AIDA's rich not-found / parse-failure errors already embed
/// hints like ``try `aida list` or `aida search <terms>` ``; surfacing the
/// first one keeps the suggestion truthful without a brittle per-error table.
/// Returns None when the message embeds no such command.
// trace:TASK-972
pub(crate) fn extract_aida_suggestion(msg: &str) -> Option<String> {
    let mut rest = msg;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let close = after.find('`')?;
        let candidate = &after[..close];
        if candidate.starts_with("aida ") {
            return Some(candidate.to_string());
        }
        rest = &after[close + 1..];
    }
    None
}

/// Pure decision for the TASK-970 `aida list` default row cap. Returns the cap
/// to apply when AGENT MODE should bound a bare `aida list`, else `None`. The
/// cap is scoped to the default human-readable table render: an explicit
/// `--limit`/`--all`, the machine shapes (`--short`/`--json`), the grouped
/// `--tree` view, and the human TTY path all bypass it.
// trace:TASK-970
pub(crate) fn agent_list_default_cap(
    limit: Option<usize>,
    all: bool,
    short: bool,
    json: bool,
    tree: bool,
    agent_mode: bool,
) -> Option<usize> {
    if limit.is_none() && !all && !short && !json && !tree && agent_mode {
        Some(AGENT_LIST_DEFAULT_LIMIT)
    } else {
        None
    }
}

// ── TASK-964: TOON agent-output renderers (AXI #1 token-efficient output +
// #2 minimal default schemas). These run ONLY in agent mode; the human emoji /
// table path is left byte-identical. Each command declares a minimal default
// field set; `--fields` expands `aida list`. trace:TASK-964

/// The minimal default column schema for `aida list` in agent mode (AXI #2).
/// Five fields cover the agent's routine memory-lane need — id / title /
/// status / type / modified_at — instead of the human table's
/// id/origin/type/status/title/tags/glyph spread. `--fields` widens it.
// trace:TASK-964
// trace:TASK-1215 | ai:codex
pub(crate) const TOON_LIST_DEFAULT_FIELDS: &[&str] =
    &["id", "title", "status", "type", "modified_at"];

/// The list fields the agent-mode `--fields` selector understands, mapped onto
/// the cache summary plus the work-routing axis. Unknown names are rejected with
/// this list in the error so the agent can self-correct.
// trace:TASK-964
pub(crate) const TOON_LIST_KNOWN_FIELDS: &[&str] = &[
    "id",
    "title",
    "status",
    "type",
    "priority",
    "feature",
    "owner",
    "assignee",
    "tags",
    "heft",
    // trace:TASK-1215 | ai:codex — freshness signal for authored lane memory.
    "modified_at",
    // trace:FR-283 | ai:claude — the optional numeric weight/score.
    "weight",
    "queued",
    "in_flight",
    "blocked",
    // trace:STORY-776 | ai:claude — the advisor's dispatch classification.
    "mode",
    // trace:STORY-634 | ai:claude — the multi-repo repo/component dimension.
    "origin",
    "deferred_reason",
];

/// Resolve the requested `--fields` selection for agent-mode `aida list` into a
/// validated, ordered field list. `None` (no `--fields`) yields the minimal
/// default. A `csv` string is split on commas, trimmed, lowercased, and checked
/// against [`TOON_LIST_KNOWN_FIELDS`]; an unknown name is an error naming the
/// valid set. Empty selection falls back to the default.
// trace:TASK-964
pub(crate) fn toon_list_fields(csv: Option<&str>) -> Result<Vec<String>> {
    let Some(csv) = csv else {
        return Ok(TOON_LIST_DEFAULT_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect());
    };
    let mut out = Vec::new();
    for raw in csv.split(',') {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        // `req_type` is a friendly alias for `type`.
        let name = if name == "req_type" {
            "type".to_string()
        } else {
            name
        };
        if !TOON_LIST_KNOWN_FIELDS.contains(&name.as_str()) {
            anyhow::bail!(
                "unknown --fields entry `{name}`; valid fields: {}",
                TOON_LIST_KNOWN_FIELDS.join(", ")
            );
        }
        out.push(name);
    }
    if out.is_empty() {
        return Ok(TOON_LIST_DEFAULT_FIELDS
            .iter()
            .map(|s| s.to_string())
            .collect());
    }
    Ok(out)
}

/// Render a numeric weight/score for display: integral values drop the
/// fractional part (`42`, not `42.0`); everything else uses the shortest
/// round-trip float formatting (`0.75`).
// trace:FR-283 | ai:claude
pub(crate) fn format_weight(w: f64) -> String {
    if w.fract() == 0.0 && w.abs() < 1e15 {
        format!("{}", w as i64)
    } else {
        format!("{}", w)
    }
}

/// Project one requirement summary + its routing triple `(in_flight, blocked,
/// queued)` onto a single named list field, as the cell string for a TOON row.
// trace:TASK-964
pub(crate) fn toon_list_cell(
    r: &aida_core::RequirementSummary,
    routing: (bool, bool, bool),
    field: &str,
) -> String {
    let (in_flight, blocked, queued) = routing;
    match field {
        "id" => r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("")
            .to_string(),
        "title" => r.title.clone(),
        // BUG-781: the agent surface reports the TERMINAL truth for an accepted
        // decision — `accepted`, the recognized ADR verb — not the `approved`
        // token that reads as "cleared to start". trace:BUG-781 | ai:claude
        "status" => toon_status_token(crate::status_display::display_status_for_type(
            &r.req_type,
            &r.status,
        )),
        "type" => r.req_type.to_ascii_lowercase(),
        "priority" => r.priority.to_ascii_lowercase(),
        "feature" => r.feature.clone(),
        "owner" => r.owner.clone(),
        "assignee" => r.assignee.clone().unwrap_or_default(),
        "tags" => r.tags.join(" "),
        "heft" => r.heft.to_string(),
        // trace:TASK-1215 | ai:codex
        "modified_at" => r.modified_at.clone(),
        // trace:FR-283 | ai:claude — empty cell = no weight set.
        "weight" => r.weight.map(format_weight).unwrap_or_default(),
        "queued" => queued.to_string(),
        "in_flight" => in_flight.to_string(),
        "blocked" => blocked.to_string(),
        // trace:STORY-776 | ai:claude — empty cell = ungroomed.
        "mode" => r.execution_mode.clone().unwrap_or_default(),
        // trace:STORY-634 | ai:claude — empty cell = single-repo.
        "origin" => r.origin.clone().unwrap_or_default(),
        "deferred_reason" => r.deferred_reason.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Normalize a cache status string (`InProgress`, `in-progress`, `NeedsAttention`,
/// …) to a stable lowercase-hyphen token for agent output, so the TOON value is
/// uniform regardless of how the cache spelled it.
// trace:TASK-964
pub(crate) fn toon_status_token(status: &str) -> String {
    let s = status.trim();
    match s.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
        "inprogress" | "in-progress" => "in-progress".to_string(),
        "needsattention" | "needs-attention" => "needs-attention".to_string(),
        other => other.to_string(),
    }
}

/// The column header label for a `--fields` field name. The same field
/// vocabulary [`toon_list_cell`] / [`toon_list_fields`] understand, mapped to a
/// human-friendly capitalized header for the `aida list --fields` table.
// trace:STORY-734 | ai:claude
pub(crate) fn list_field_header(field: &str) -> String {
    match field {
        "id" => "ID",
        "title" => "Title",
        "status" => "Status",
        "type" => "Type",
        "priority" => "Priority",
        "feature" => "Feature",
        "owner" => "Owner",
        "assignee" => "Assignee",
        "tags" => "Tags",
        "heft" => "Heft",
        // trace:TASK-1215 | ai:codex
        "modified_at" => "Modified",
        // trace:FR-283 | ai:claude
        "weight" => "Weight",
        "queued" => "Queued",
        "in_flight" => "In-Flight",
        "blocked" => "Blocked",
        // Validated against the known set upstream, so this is unreachable in
        // practice; fall back to the raw name for robustness.
        other => return other.to_string(),
    }
    .to_string()
}

/// Build the lines of the human `aida list --fields <csv>` table: exactly the
/// requested columns, in the requested order, each left-aligned and sized to its
/// widest cell (header included). Cell values reuse [`toon_list_cell`] so the
/// human and agent surfaces share one field vocabulary; the `status` and `id`
/// columns are coloured AFTER padding so ANSI escapes don't break alignment, and
/// the trailing column is not padded. Returned as a `Vec<String>` (header,
/// divider, then one line per row) so the layout is unit-testable; the colour
/// helpers no-op when stdout isn't a TTY.
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn build_list_fields_lines<F>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    // Plain (uncoloured) cell text per row per column — width math runs on this.
    let plain: Vec<Vec<String>> = reqs
        .iter()
        .map(|r| {
            let routing = routing_of(r);
            fields
                .iter()
                .map(|f| toon_list_cell(r, routing, f))
                .collect()
        })
        .collect();

    // Column width = widest of (header, every cell). The last column is left
    // unpadded so a long Title never trails whitespace.
    let widths: Vec<usize> = fields
        .iter()
        .enumerate()
        .map(|(c, f)| {
            let header = list_field_header(f).len();
            plain
                .iter()
                .map(|row| row[c].len())
                .max()
                .unwrap_or(0)
                .max(header)
        })
        .collect();
    let last = fields.len().saturating_sub(1);

    let mut lines = Vec::with_capacity(reqs.len() + 2);

    // Header row + divider.
    let mut header = String::new();
    for (c, f) in fields.iter().enumerate() {
        if c > 0 {
            header.push(' ');
        }
        let label = list_field_header(f);
        if c == last {
            header.push_str(&label);
        } else {
            header.push_str(&format!("{:<width$}", label, width = widths[c]));
        }
    }
    lines.push(header.bold().to_string());
    let rule: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    lines.push("─".repeat(rule));

    // Data rows: pad the plain cell to its column width, THEN colour status / id
    // so the ANSI escapes don't inflate the `{:<}` byte count. trace:STORY-734
    for (row, req) in plain.iter().zip(reqs.iter()) {
        let mut line = String::new();
        for (c, f) in fields.iter().enumerate() {
            if c > 0 {
                line.push(' ');
            }
            let padded = if c == last {
                row[c].clone()
            } else {
                format!("{:<width$}", row[c], width = widths[c])
            };
            let cell = match f.as_str() {
                // BUG-781: paint on the DISPLAY label, so an accepted decision
                // gets the terminal green rather than the un-started cyan.
                // trace:BUG-781 | ai:claude
                "status" => status_display::paint_status(
                    &padded,
                    status_display::display_status_for_type(&req.req_type, &req.status),
                )
                .to_string(),
                "id" => padded.bold().to_string(),
                _ => padded,
            };
            line.push_str(&cell);
        }
        lines.push(line);
    }
    lines
}

/// Print the human `aida list --fields <csv>` table. Thin wrapper over
/// [`build_list_fields_lines`].
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn render_list_fields_table<F>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
) where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    for line in build_list_fields_lines(reqs, fields, routing_of) {
        println!("{line}");
    }
}

/// Render the body of `aida search --fields <csv>` for a non-empty result set,
/// branching on agent vs human surface the same way `aida list --fields` does.
/// In agent mode (`agent == true`) it's the lean TOON `specs[N]{...}` projection
/// — the ~2x token win the agent surface exists for, instead of the verbose
/// box-table an agent would otherwise pay for. On a human TTY it's the aligned
/// box-table shared with `aida list` via [`build_list_fields_lines`]. Both
/// surfaces resolve cells through [`toon_list_cell`] over the already-validated
/// `selected` field set, so the two surfaces never drift in field vocabulary.
/// Search rows carry no work-routing axis, so queued/in_flight/blocked render
/// false. Returned as a `String` so the branch is unit-testable.
// trace:STORY-734 trace:BUG-668 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn render_search_fields(
    results: &[aida_core::RequirementSummary],
    selected: &[String],
    agent: bool,
) -> String {
    if agent {
        let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|r| {
                selected
                    .iter()
                    .map(|f| toon_list_cell(r, (false, false, false), f))
                    .collect()
            })
            .collect();
        let mut out = format!("count: {} results\n", results.len());
        out.push_str(&crate::toon::table_raw("specs", &field_refs, &rows));
        out
    } else {
        let mut lines = build_list_fields_lines(results, selected, |_| (false, false, false));
        lines.push(format!("\n{} results", results.len()));
        lines.join("\n")
    }
}

#[cfg(test)]
#[path = "tests/task970_agent_output_tests.rs"]
mod task970_agent_output_tests;

#[cfg(test)]
#[path = "tests/task_1486_store_refusal_tests.rs"]
mod task_1486_store_refusal_tests;

#[cfg(test)]
#[path = "tests/task_1487_store_polish_tests.rs"]
mod task_1487_store_polish_tests;

pub(crate) fn maybe_run_asciinema_wrapper(raw_args: &[String], cli: &Cli) -> Result<Option<i32>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "{} --asciinema requested but stdin/stdout is not a TTY; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    if !asciinema_available() {
        eprintln!(
            "{} --asciinema requested but `asciinema` is not installed or not on PATH; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    let aida_exe = resolve_aida_exe();
    let inner_args = strip_asciinema_wrapper_args(raw_args);
    let cast_path = resolve_cast_output_path(cli.cast_out.as_ref(), &inner_args)?;
    if let Some(parent) = cast_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cast output directory {}", parent.display()))?;
    }
    let title = cli
        .cast_title
        .clone()
        .unwrap_or_else(|| default_cast_title(&inner_args));
    let command = shell_command_for_asciinema(&aida_exe, &inner_args);

    let status = std::process::Command::new("asciinema")
        .args(["rec", "--command", &command, "--title", &title])
        .arg(&cast_path)
        .env(ASCIINEMA_WRAPPED_ENV, "1")
        .status()
        .context("could not invoke `asciinema rec`")?;

    if status.success() {
        println!("Cast saved: {}", cast_path.display());
    }
    Ok(Some(status.code().unwrap_or(1)))
}

pub(crate) fn asciinema_available() -> bool {
    std::process::Command::new("asciinema")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn strip_asciinema_wrapper_args(raw_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(bin) = raw_args.first() {
        out.push(bin.clone());
    }
    let mut i = 1;
    while i < raw_args.len() {
        let arg = &raw_args[i];
        match arg.as_str() {
            "--asciinema" => {
                i += 1;
            }
            "--cast-out" | "--cast-title" => {
                i += 2;
            }
            _ if arg.starts_with("--cast-out=") || arg.starts_with("--cast-title=") => {
                i += 1;
            }
            _ => {
                out.push(arg.clone());
                i += 1;
            }
        }
    }
    out
}

pub(crate) fn resolve_cast_output_path(
    override_path: Option<&std::path::PathBuf>,
    inner_args: &[String],
) -> Result<std::path::PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.clone());
    }
    let base_dir =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    let slug = asciinema_command_slug(inner_args);
    Ok(base_dir
        .join(".aida")
        .join("casts")
        .join(format!("{stamp}-{slug}.cast")))
}

pub(crate) fn derive_semantic_title(inner_args: &[String]) -> Option<String> {
    let has_queue_work =
        inner_args.iter().any(|arg| arg == "queue") && inner_args.iter().any(|arg| arg == "work");
    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");

    if has_queue_work {
        let mut specs = Vec::new();
        let mut batch_name = None;

        let mut i = 1;
        let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
        while i < inner_args.len() {
            let arg = &inner_args[i];
            if arg == "--batch" && i + 1 < inner_args.len() {
                batch_name = Some(inner_args[i + 1].clone());
                i += 2;
                continue;
            } else if let Some(val) = arg.strip_prefix("--batch=") {
                batch_name = Some(val.to_string());
                i += 1;
                continue;
            }

            if spec_re.is_match(arg) {
                specs.push(arg.to_uppercase());
            }
            i += 1;
        }

        if !specs.is_empty() {
            return Some(format!("AIDA drain: {}", specs.join(", ")));
        } else if let Some(batch) = batch_name {
            return Some(format!("AIDA drain: batch {batch}"));
        }
    } else if has_pr_ship {
        let pr_re = regex::Regex::new(r"^\d+$").unwrap();
        for arg in inner_args.iter().skip(1) {
            if pr_re.is_match(arg) {
                return Some(format!("AIDA pr ship: PR-{arg}"));
            }
        }
    }
    None
}

pub(crate) fn default_cast_title(inner_args: &[String]) -> String {
    if let Some(semantic) = derive_semantic_title(inner_args) {
        return semantic;
    }
    let rest = inner_args
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    if rest.trim().is_empty() {
        "aida".to_string()
    } else {
        format!("aida {rest}")
    }
}

pub(crate) fn slugify_str(s: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn derive_spec_aware_slug(inner_args: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    let mut i = 1;

    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");
    let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
    let pr_re = regex::Regex::new(r"^\d+$").unwrap();

    while i < inner_args.len() {
        let arg = &inner_args[i];

        if arg == "--batch" {
            if i + 1 < inner_args.len() {
                let val = slugify_str(&inner_args[i + 1]);
                parts.push(format!("batch-{val}"));
                i += 2;
                continue;
            }
        } else if let Some(raw) = arg.strip_prefix("--batch=") {
            let val = slugify_str(raw);
            parts.push(format!("batch-{val}"));
            i += 1;
            continue;
        }

        if spec_re.is_match(arg) {
            parts.push(arg.to_lowercase());
        } else if has_pr_ship && pr_re.is_match(arg) {
            parts.push(format!("pr-{arg}"));
        }

        i += 1;
    }

    if !parts.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let mut unique_parts = Vec::new();
        for p in parts {
            if seen.insert(p.clone()) {
                unique_parts.push(p);
            }
        }
        Some(unique_parts.join("-"))
    } else {
        None
    }
}

pub(crate) fn asciinema_command_slug(inner_args: &[String]) -> String {
    if let Some(spec_slug) = derive_spec_aware_slug(inner_args) {
        return truncate_asciinema_slug(&spec_slug);
    }

    // Generic fallback: use subcommand + first non-flag arg
    let pos_args: Vec<String> = inner_args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(|arg| slugify_str(arg))
        .filter(|arg| !arg.is_empty())
        .collect();

    let fallback_slug = if pos_args.len() >= 2 {
        format!("{}-{}", pos_args[0], pos_args[1])
    } else if pos_args.len() == 1 {
        pos_args[0].clone()
    } else {
        "aida".to_string()
    };

    truncate_asciinema_slug(&fallback_slug)
}

pub(crate) fn truncate_asciinema_slug(slug: &str) -> String {
    let count = slug.chars().count();
    if count <= ASCIINEMA_SLUG_MAX_CHARS {
        return slug.to_string();
    }
    let keep = ASCIINEMA_SLUG_MAX_CHARS.saturating_sub("-trunc".len());
    let mut truncated = slug.chars().take(keep).collect::<String>();
    truncated = truncated.trim_end_matches('-').to_string();
    format!("{truncated}-trunc")
}

pub(crate) fn shell_command_for_asciinema(
    aida_exe: &std::path::Path,
    inner_args: &[String],
) -> String {
    std::iter::once(aida_exe.to_string_lossy().to_string())
        .chain(inner_args.iter().skip(1).cloned())
        .map(|arg| shell_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
#[path = "tests/story_423_asciinema_tests.rs"]
mod story_423_asciinema_tests;

/// TASK-822 / 824 / 828 / 831: rewrite the `aida list <lens>` discoverability
/// aliases into their canonical commands before clap parses, so the list family
/// (`open` / `human` / `queue` / `why` / `advisor` / `inflight`) reads uniformly
/// while each alias is a pure passthrough of the target command's flags. The
/// target may be one token (`aida advisor`) or two (`aida queue list`). Unmatched
/// input is returned unchanged. `args[0]` is the binary name.
/// trace:TASK-822 trace:TASK-824 trace:TASK-828 trace:TASK-831
/// STORY-623: rewrite `aida advisor assess [args]` → `aida groom [args]` before
/// clap, so the advisor's draft-disposition verb is reachable under its seat
/// while the implementation stays the single `groom` (aka deprecated `assess` /
/// `intake`) command. STORY-708: the canonical target is now `groom` — the
/// advisor-seat spelling normalizes straight to the canonical verb.
/// Unmatched input is returned unchanged.
// trace:STORY-623 trace:STORY-708 | ai:claude
pub(crate) fn rewrite_advisor_assess(args: &[String]) -> Vec<String> {
    if args.len() >= 3 && args[1] == "advisor" && args[2] == "assess" {
        let mut out = Vec::with_capacity(args.len() - 1);
        out.push(args[0].clone());
        out.push("groom".to_string());
        out.extend_from_slice(&args[3..]);
        return out;
    }
    args.to_vec()
}

/// STORY-708: `groom` is the canonical advisor-disposition verb; `assess` and
/// `intake` are deprecated aliases. clap still accepts both (the `Groom` command
/// carries `#[command(alias = "assess", alias = "intake")]`), but we normalize a
/// top-level `aida assess`/`aida intake` to `aida groom` BEFORE clap so the
/// dispatch sees one canonical token, and we print a one-line, non-blocking
/// deprecation hint to stderr pointing at `aida groom`. The rewrite only fires
/// when the deprecated verb is the SUBCOMMAND (argv[1]) — a later positional or
/// flag value that happens to read `assess`/`intake` is left untouched. Returns
/// (rewritten argv, deprecated-verb-seen) so the caller can emit the hint after
/// argv parsing succeeds.
// trace:STORY-708 | ai:claude
pub(crate) fn rewrite_groom_alias(args: &[String]) -> (Vec<String>, Option<String>) {
    if args.len() >= 2 && (args[1] == "assess" || args[1] == "intake") {
        let deprecated = args[1].clone();
        let mut out = Vec::with_capacity(args.len());
        out.push(args[0].clone());
        out.push("groom".to_string());
        out.extend_from_slice(&args[2..]);
        return (out, Some(deprecated));
    }
    (args.to_vec(), None)
}

/// TASK-858: bare `aida agent` (no recognized subcommand) defaults to
/// `aida agent new`, git-style, forwarding any flags/args to `new`. The
/// recognized subcommands (`new`, `register`, `ls`, `status`, `gc`,
/// `dispatch-health`, `pause`, `resume`, `stop`, `list-roles`) pass through
/// unchanged, and `aida agent --help` / `-h` keeps clap's parent help (so the
/// surface stays discoverable). Mirrors the pre-clap argv-rewrite pattern used
/// for the `list` lens aliases and `advisor assess`.
// trace:TASK-858 | ai:claude
pub(crate) fn rewrite_agent_default_new(args: &[String]) -> Vec<String> {
    if args.len() >= 2 && args[1] == "agent" {
        // The next token decides: a recognized subcommand or a help flag is
        // left alone; anything else (including bare `aida agent`) gets `new`
        // spliced in so flags/args forward to the `new` launcher.
        const KNOWN_SUBCOMMANDS: &[&str] = &[
            "new",
            "register",
            "ls",
            "status",
            "gc",
            "dispatch-health",
            "pause",
            "resume",
            "stop",
            "list-roles",
            "help",
        ];
        let next = args.get(2).map(String::as_str);
        let passthrough = matches!(next, Some(t) if
            KNOWN_SUBCOMMANDS.contains(&t) || t == "--help" || t == "-h");
        if !passthrough {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.push(args[0].clone());
            out.push("agent".to_string());
            out.push("new".to_string());
            out.extend_from_slice(&args[2..]);
            return out;
        }
    }
    args.to_vec()
}

/// TASK-881: bare `aida queue` (no recognized subcommand) defaults to
/// `aida queue list`, matching the `aida list` / `aida status` "bare form is
/// the read view" ergonomics — instead of dumping the clap subcommand help
/// wall (exit 2), which reads as an error to a newcomer asking "what's
/// queued?". Recognized subcommands pass through unchanged, and `aida queue
/// --help` / `-h` / `help` keep clap's parent help so the surface stays
/// discoverable. Mirrors `rewrite_agent_default_new` (TASK-858); flags-only
/// invocations (`aida queue --role implementer`) forward to `list`.
// trace:TASK-881 | ai:claude
pub(crate) fn rewrite_queue_default_list(args: &[String]) -> Vec<String> {
    if args.len() >= 2 && args[1] == "queue" {
        // The next token decides: a recognized subcommand or a help flag is
        // left alone; anything else (including bare `aida queue` and
        // flags-only forms) gets `list` spliced in so flags forward to it.
        const KNOWN_SUBCOMMANDS: &[&str] = &[
            "list",
            "add",
            "load",
            "remove",
            "move",
            "clear",
            "prune",
            "gc",
            "next",
            "advance",
            "done",
            "work",
            "progress",
            "rework",
            "recover",
            "integrate",
            "help",
        ];
        let next = args.get(2).map(String::as_str);
        let passthrough = matches!(next, Some(t) if
            KNOWN_SUBCOMMANDS.contains(&t) || t == "--help" || t == "-h");
        if !passthrough {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.push(args[0].clone());
            out.push("queue".to_string());
            out.push("list".to_string());
            out.extend_from_slice(&args[2..]);
            return out;
        }
    }
    args.to_vec()
}

/// One `aida list <lens>` argv-rewrite row. See [`LIST_LENS_ALIASES`].
/// trace:STORY-667 | ai:claude
pub(crate) struct ListLensAlias {
    /// The positional lens token(s) that trigger this rewrite (first is canonical).
    pub tokens: &'static [&'static str],
    /// The canonical command tokens the lens expands to (after `aida`).
    pub canonical: &'static [&'static str],
    /// One-line human meaning for the `aida alias` registry.
    pub meaning: &'static str,
}

/// The `aida list <lens>` argv-rewrite table — the SINGLE SOURCE OF TRUTH for
/// the list-lens aliases. Each row maps the positional lens token(s) it accepts
/// to the canonical command tokens it expands to, plus a one-line meaning for
/// the `aida alias` registry. `rewrite_list_alias` resolves against this table
/// and the `aida alias` registry enumerates it — so the surface and its catalog
/// can't drift. trace:STORY-667 | ai:claude
pub(crate) const LIST_LENS_ALIASES: &[ListLensAlias] = &[
    ListLensAlias {
        tokens: &["queue"],
        canonical: &["queue", "list"],
        meaning: "your personal work queue",
    },
    ListLensAlias {
        tokens: &["why"],
        canonical: &["burndown", "explain"],
        meaning: "why each open spec sits where it does",
    },
    ListLensAlias {
        tokens: &["advisor"],
        canonical: &["advisor"],
        meaning: "the live-advisor registration view",
    },
    // TASK-831: active work — leased specs + drain in-flight status.
    ListLensAlias {
        tokens: &["inflight", "in-flight"],
        canonical: &["burndown", "status"],
        meaning: "active work — leased specs + drain in-flight status",
    },
];

/// TASK-862: top-level personal-view shortcuts. `aida mylist` rewrites to
/// `aida list me` (the current user's owned/assigned specs — STORY-662's `me`
/// filter), forwarding any extra flags (`aida mylist --status open`). `aida
/// myqueue` rewrites to `aida queue list`, which is already user-scoped via
/// `current_user_id()` (BUG-89) — so `myqueue` is just a discoverable alias for
/// "my queue". Both are pre-clap argv rewrites, the same idiom as
/// `rewrite_list_alias` / `rewrite_advisor_assess`, so every downstream flag
/// passes through unchanged. They deliberately do NOT touch the `aida list`
/// default scope (`aida list` still shows ALL) — the default-scope question is
/// recorded on the spec for the operator, not actioned here.
// trace:TASK-862 | ai:claude
pub(crate) fn rewrite_personal_view_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 2 {
        match args[1].as_str() {
            "mylist" => {
                let mut out = Vec::with_capacity(args.len() + 1);
                out.push(args[0].clone());
                out.push("list".to_string());
                out.push("me".to_string());
                out.extend_from_slice(&args[2..]);
                return out;
            }
            "myqueue" => {
                let mut out = Vec::with_capacity(args.len() + 1);
                out.push(args[0].clone());
                out.push("queue".to_string());
                out.push("list".to_string());
                out.extend_from_slice(&args[2..]);
                return out;
            }
            _ => {}
        }
    }
    args.to_vec()
}

pub(crate) fn rewrite_list_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 3 && args[1] == "list" {
        let target: Option<&[&str]> = LIST_LENS_ALIASES
            .iter()
            .find(|a| a.tokens.contains(&args[2].as_str()))
            .map(|a| a.canonical);
        if let Some(tokens) = target {
            let mut out = Vec::with_capacity(args.len());
            out.push(args[0].clone());
            out.extend(tokens.iter().map(|s| s.to_string()));
            out.extend_from_slice(&args[3..]);
            return out;
        }
    }
    args.to_vec()
}

// trace:STORY-1050 | ai:antigravity
pub(crate) fn rewrite_type_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 2 {
        let ty = args[1].to_lowercase();
        // AIDA's built-in requirement types
        let valid_types = [
            "functional",
            "non-functional",
            "system",
            "user",
            "change-request",
            "bug",
            "epic",
            "story",
            "task",
            "spike",
            "sprint",
            "folder",
            "meta",
            "principle",
            "vision",
            "constraint",
            "decision",
            "term",
            "faq",
            "cr",
            "fr",
        ];

        if valid_types.contains(&ty.as_str()) {
            if args.len() == 2 {
                // aida bug -> aida list --type bug
                return vec![
                    args[0].clone(),
                    "list".to_string(),
                    "--type".to_string(),
                    ty,
                ];
            } else if args[2] == "list" {
                // aida bug list ... -> aida list --type bug ...
                let mut out = vec![
                    args[0].clone(),
                    "list".to_string(),
                    "--type".to_string(),
                    ty,
                ];
                out.extend_from_slice(&args[3..]);
                return out;
            } else if args[2] == "add" {
                // aida bug add ... -> aida add --type bug ...
                let mut out = vec![args[0].clone(), "add".to_string(), "--type".to_string(), ty];
                out.extend_from_slice(&args[3..]);
                return out;
            }
        }
    }
    args.to_vec()
}

// trace:STORY-1027 | ai:codex
pub(crate) fn parse_help_commands_args(args: &[String]) -> Result<(bool, bool, bool)> {
    let mut flags = false;
    let mut json = false;
    let mut hidden = false;
    for arg in args {
        match arg.as_str() {
            "--flags" => flags = true,
            "--json" => json = true,
            "--hidden" => hidden = true,
            other => anyhow::bail!(
                "unknown option for `aida help commands`: {other}\n\
                 supported options: --flags, --json, --hidden"
            ),
        }
    }
    Ok((flags, json, hidden))
}

pub(crate) fn run() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();
    // Intercept --version / -V before clap so we can include the build-time
    // banner (build.rs stamps build time + git sha + dirty flag). Clap's
    // built-in #[clap(version)] only knows the package version, which can't
    // distinguish two binaries at the same version built at different times.
    if raw_args.len() == 2 && (raw_args[1] == "--version" || raw_args[1] == "-V") {
        println!("aida {}", build_banner());
        return Ok(());
    }

    // trace:TASK-1718 | ai:antigravity
    if raw_args.len() == 2 && raw_args[1] == "--path" {
        let exe = crate::resolve_aida_exe();
        println!("{}", exe.display());
        return Ok(());
    }

    // Tiered help: bare `aida` and `aida help` LEAD with a small curated
    // "Getting started" set + a teaser of the grouped surface, so a newcomer
    // sees an approachable path instead of a flat 40-command clap dump. The
    // full grouped inventory is one step away via `aida help --all` (or the
    // legacy `aida help-all`). Intercepting here is required because bare
    // `aida` otherwise fails clap parsing (subcommand required → exit 2).
    // trace:STORY-556
    {
        let first = raw_args.get(1).map(String::as_str);
        // Args after the `help` verb (empty for bare `aida`).
        let rest = raw_args.get(2..).unwrap_or(&[]);
        // `aida help` (clap's builtin help verb) with no further positional —
        // optionally `--all`. `aida --help` / `aida -h` keep clap's own flag
        // help (a deliberate escape hatch to the raw flag-level usage).
        let is_help_verb = first == Some("help") && rest.iter().all(|a| a == "--all" || a == "-a");
        let is_bare = raw_args.len() == 1;
        // `aida help <topic>` — the middle granularity between bare `help`
        // (group names) and `help --all` (everything): expand one group. A
        // single non-flag positional after `help` is a topic name.
        // trace:TASK-861 | ai:claude
        let help_topic: Option<&str> =
            if first == Some("help") && rest.len() == 1 && !rest[0].starts_with('-') {
                Some(rest[0].as_str())
            } else {
                None
            };
        if first == Some("help")
            && rest
                .first()
                .is_some_and(|topic| topic.eq_ignore_ascii_case("commands"))
        {
            let (flags, json, hidden) = parse_help_commands_args(&rest[1..])?;
            help_catalog::print_command_catalog(flags, hidden, json);
            return Ok(());
        }
        if let Some(topic) = help_topic {
            // `aida help commands` — the flat clap-derived catalog of every
            // command and subcommand, one line each. Intercepted before the
            // group-topic resolver so it can't be shadowed by a group name.
            // trace:TASK-1098 | ai:claude
            if topic.eq_ignore_ascii_case("commands") {
                help_catalog::print_command_catalog(false, false, false);
                return Ok(());
            }
            // Exact command help keeps clap's native rendering ahead of the
            // semantic concept/FTS layers. trace:STORY-837 | ai:codex
            if help_catalog::print_exact_command_help(topic) {
                return Ok(());
            }
            return print_help_topic(topic);
        }
        // TASK-970: content-first bare `aida` in AGENT MODE. A human at a TTY
        // keeps the getting-started menu (the Trojan-horse first impression);
        // an agent (non-TTY / `AIDA_AGENT_OUTPUT`) gets the `aida status`
        // snapshot + top queued instead, since CLAUDE.md names `aida status`
        // as the entry point and a help dump is noise to a coding agent. The
        // explicit `aida help` verb is unchanged in both modes.
        // trace:TASK-970
        if is_bare && agent_output_mode() {
            return handle_bare_agent_status();
        }
        if is_help_verb || is_bare {
            let want_all = rest.iter().any(|a| a == "--all" || a == "-a");
            if want_all {
                print_help_all();
            } else {
                print_tiered_help();
            }
            return Ok(());
        }
    }

    // TASK-822 / TASK-824 / STORY-623: pre-clap argv rewrites so aliases pass
    // every downstream flag through unchanged. `aida list <lens>` → its command
    // (queue/why/advisor/inflight); `aida advisor assess` → `aida assess` (the
    // advisor's verb spelled under its seat). `aida assess` itself + the `intake`
    // alias are handled by clap's visible_alias on the Assess variant.
    // trace:TASK-822 trace:TASK-824 trace:STORY-623 | ai:claude
    // TASK-862: `aida mylist` / `aida myqueue` personal-view shortcuts rewrite
    // before the list-lens / advisor-assess passes so `mylist` reaches the same
    // `list me` path the lens resolvers feed.
    // trace:TASK-881 | ai:claude — bare `aida queue` -> `aida queue list`.
    // TASK-877: user/project-defined aliases expand FIRST (outermost), so an
    // alias's stored expansion (e.g. `list draft`) then flows through the
    // built-in personal-view / list-lens / queue-default passes. The expansion
    // is gated on an interactive HUMAN caller inside `user_alias::expand` — an
    // agent / headless / MCP / non-TTY caller gets argv back unchanged, so the
    // canonical surface every vendor sees is never reshaped by a user alias.
    // trace:TASK-877 | ai:claude
    let expanded = user_alias::expand(&raw_args);
    // STORY-708: normalize the deprecated `aida assess` / `aida intake` verbs to
    // the canonical `aida groom` before clap, and capture which deprecated
    // spelling (if any) the operator typed so we can print a non-blocking hint
    // once argv parsing succeeds. trace:STORY-708 | ai:claude
    let (after_alias_rewrites, groom_deprecated_verb) =
        rewrite_groom_alias(&rewrite_queue_default_list(&rewrite_agent_default_new(
            &rewrite_list_alias(&rewrite_type_alias(&rewrite_advisor_assess(
                &rewrite_personal_view_alias(&expanded),
            ))),
        )));
    let mut cli = Cli::parse_from(after_alias_rewrites.clone());

    // STORY-764: install the explicit output-format pin before any handler
    // renders. `--format` wins; else `AIDA_OUTPUT_FORMAT`; else the TTY-based
    // default stands. trace:STORY-764 | ai:claude
    set_output_format_override(cli.format);
    // trace:TASK-1526 | ai:codex
    cache_output::set_toon_cache_output(matches!(
        &cli.command,
        Command::History { .. } | Command::Graph { .. }
    ));
    // Only human output at a real terminal may perform the pre-C inline full
    // rebuild. Explicit JSON/TOON and advisory paths stay bounded at a TTY too.
    // trace:TASK-1526 | ai:codex
    let advisory = raw_args
        .iter()
        .any(|s| s == "--notice" || s == "statusline" || s == "statusbar");
    let machine =
        raw_args.iter().any(|s| s == "--json" || s == "--toon") || agent_output_mode_quiet();
    // A single-spec read must not inherit the 1500ms refresh wait or a
    // terminal's inline full rebuild. Its object is read canonically; the
    // existing snapshot labels disclose stale derived graph context.
    // Strict mutations and explicit cache refresh keep their own policy.
    // trace:BUG-1801 | ai:codex
    let single_spec_read = matches!(&cli.command, Command::Show { .. });
    aida_core::db::cache_refresh::configure_read_policy(
        std::io::IsTerminal::is_terminal(&std::io::stdout())
            && !machine
            && !advisory
            && !single_spec_read,
        (advisory || single_spec_read).then_some(aida_core::db::cache_refresh::ReadBudget(
            std::time::Duration::ZERO,
        )),
    );

    // A global clap flag is syntactically accepted everywhere. Convert an
    // unsupported JSON request into an explicit error before any command can
    // silently fall back to prose. trace:BUG-1502 | ai:codex
    let mut dispatch_argv = after_alias_rewrites;
    let dispatch_len = dispatch_argv.len();
    enforce_json_format_capability(&mut dispatch_argv)?;
    // Reparse only when capability materialization changed argv. This makes
    // the local `json` field authoritative for every downstream dispatcher.
    if dispatch_argv.len() != dispatch_len {
        cli = Cli::parse_from(dispatch_argv);
    }

    // STORY-708: one-line, non-blocking deprecation hint. Printed to stderr (so
    // it never pollutes machine-readable stdout) only when the operator reached
    // groom via a deprecated alias. trace:STORY-708 | ai:claude
    if let Some(verb) = &groom_deprecated_verb {
        eprintln!("note: `aida {verb}` is deprecated — use `aida groom` (the canonical disposition verb).");
    }

    if cli.asciinema && std::env::var_os(ASCIINEMA_WRAPPED_ENV).is_none() {
        if let Some(exit_code) = maybe_run_asciinema_wrapper(&raw_args, &cli)? {
            std::process::exit(exit_code);
        }
    }

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.server.is_none() {
        cli.server = std::env::var("AIDA_SERVER").ok();
    }

    enforce_stakeholder_role_capabilities(&mut cli.command)?;

    // TASK-756: operator-presence TTY auto-flip. Any interactive aida
    // command (stdout+stdin are a TTY) means the operator is demonstrably
    // back — flip a stored `away` to `home`. Cheap (only writes when a flip
    // is needed) and non-fatal (a presence-file error never breaks the
    // actual command). This is the one consumer-free hook for the primitive;
    // it does NOT change any command's behavior. trace:TASK-756 | ai:claude
    presence::auto_flip_if_interactive();

    // BUG-108: a shell whose worktree was removed by `aida session end`
    // (in this or another terminal) has a dangling cwd — flag it before
    // any project-root lookup silently degrades to empty state and makes
    // `aida queue list` print "Your queue is empty". trace:BUG-108
    warn_if_cwd_removed();

    // BUG-1044: when explicitly enabled, an interactive shell that lost its
    // role env after a reboot gets one early, once-per-terminal restoration
    // offer before a later advisor gate surprises it.
    maybe_prompt_role_restore(&cli.command)?;

    // Handle init before path resolution (no DB exists yet)
    if let Command::Init {
        no_skills,
        with_mcp,
        agent,
        no_hooks,
        no_post_hooks,
        no_roles,
        no_agent_config,
        no_schedule,
        force,
        footprint,
        distributed: _,
        centralized,
        sibling,
        attach,
        store_path,
        registry_remote,
        verbose,
        name,
        with_memories,
        refresh,
        focus,
        git_init,
        commit_scaffold,
        node_name,
        minimal,
        dir,
        lang,
        github,
        public,
        remote,
        forge,
    } = &cli.command
    {
        // STORY-757: the markdown-only first run — scaffold just a `specs/`
        // folder + a runnable `aida why` demo, none of the orphan-branch / cache
        // / MCP / skills / roles machine. Proves the 60-second magic needs no
        // setup. Short-circuits before every heavy init path. trace:STORY-757
        if *minimal {
            handle_init_minimal(*force)?;
            return Ok(());
        }
        // STORY-780: `aida init <DIR>` bootstraps a brand-new project. The
        // pre-steps (mkdir → native-tool scaffold → git init → first commit)
        // run from the caller's cwd, then cwd moves INTO the new dir so the
        // entire standard init below runs there unchanged; the remote/push
        // post-steps run at the end of this block, after the store branch
        // exists. trace:STORY-780 | ai:claude
        let bootstrap: Option<init_bootstrap::BootstrapPlan> = match dir {
            Some(d) => {
                let mut plan = init_bootstrap::BootstrapPlan {
                    dir: std::path::PathBuf::from(d),
                    lang: lang
                        .as_deref()
                        .map(init_bootstrap::Lang::parse)
                        .transpose()?,
                    github: *github,
                    public: *public,
                    remote: remote.clone(),
                    forge: forge.clone(),
                    github_repo: None,
                };
                init_bootstrap::resolve_forge_profile(&mut plan)?;
                init_bootstrap::preflight(&plan)?;
                init_bootstrap::create_and_scaffold(&plan)?;
                std::env::set_current_dir(&plan.dir)
                    .with_context(|| format!("could not enter {}", plan.dir.display()))?;
                Some(plan)
            }
            None => None,
        };
        // Default: distributed (git-canonical) mode per EPIC-1-001.
        // --sibling implies distributed-sibling. --centralized opts into
        // the deprecated SQLite-canonical path.
        // trace:EPIC-1-001 | ai:claude
        if *centralized {
            init_cmd::handle_init_command(
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *force,
                *verbose,
                name.as_deref(),
                *footprint,
            )?;
        } else if *sibling || store_path.is_some() {
            // STORY-676: --store-path implies the sibling storage model at an
            // explicit path; --sibling is sugar for --store-path ../aida-store.
            init_cmd::handle_init_distributed_sibling(
                registry_remote.as_deref(),
                *force,
                *attach,
                store_path.as_deref(),
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *verbose,
                name.as_deref(),
                *footprint,
            )?;
        } else {
            init_cmd::handle_init_distributed_worktree(
                *force,
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *verbose,
                name.as_deref(),
                *git_init,
                *commit_scaffold,
                node_name.as_deref(),
                *footprint,
            )?;
        }
        // TASK-638: bootstrap the default GLOBAL role set so a fresh machine is
        // ready out of the box — consistent with how init already scaffolds
        // skills / hooks / discipline-pack. Idempotent + non-destructive
        // (existing roles preserved). The write lands in GLOBAL ~/.aida/roles/
        // from a project-scoped command, so report it explicitly. Non-fatal:
        // a hiccup writing global state must not abort an otherwise-successful
        // init. trace:TASK-638 | ai:claude
        let init_footprint =
            init_cmd::resolve_init_footprint(&statusline_project_root(), *footprint)?;
        if !*no_roles && init_footprint == cli::InitFootprint::Full {
            match scaffold_starter_roles(&statusline_project_root()) {
                Ok((created, skipped)) => {
                    println!(
                        "  {} global roles installed: implementer, product, advisor, reviewer, integrator",
                        crate::glyph(crate::glyphs::Glyph::Check).green()
                    );
                    if created.is_empty() {
                        println!(
                            "  {} starter global roles already present at ~/.aida/roles/ ({} role(s))",
                            "Note:".dimmed(),
                            skipped.len()
                        );
                    } else {
                        println!(
                            "  {} scaffolded {} starter global role(s) at ~/.aida/roles/: {}",
                            "+".green(),
                            created.len(),
                            created.join(", ")
                        );
                    }
                }
                Err(e) => {
                    eprintln!("  {} starter roles skipped: {}", "Note:".dimmed(), e);
                }
            }
        }
        // TASK-698 / TASK-1233: first-machine setup plus existing-project
        // posture repair. The first-machine prompt writes GLOBAL ~/.aida/ and,
        // for contained, the matching ~/.codex/config.toml sandbox table. A
        // re-init over existing config never re-prompts unless the read-only
        // posture report already flags an incoherent state, and even then only
        // at a TTY. Non-interactive / --no-agent-config init writes nothing.
        // trace:TASK-698 trace:TASK-1233 | ai:codex
        if !*no_agent_config && init_footprint == cli::InitFootprint::Full {
            if let Err(e) = maybe_prompt_agent_posture() {
                eprintln!(
                    "  {} agent permission posture skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
            if let Err(e) =
                config_cmd::maybe_offer_permission_posture_fix(&statusline_project_root())
            {
                eprintln!(
                    "  {} agent permission posture check skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
            if let Err(e) = maybe_prompt_confirm_bypass() {
                eprintln!(
                    "  {} bypass confirmation setting skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
        }
        if init_footprint == cli::InitFootprint::Full {
            let setting = bypass_confirm::load(&statusline_project_root());
            println!(
                "  Agent bypass confirmation: {} ({})",
                if setting.on { "on" } else { "off" },
                setting.source
            );
        }
        // STORY-1463: a registered `[schedule]` job only ever runs when
        // something invokes `aida schedule tick`; nothing does that by
        // default. At a TTY, offer to install the crontab entry that drives
        // it (default: no — writing to the operator's crontab is a real
        // side effect). Non-interactive init and --no-schedule never prompt
        // and never install anything. trace:STORY-1463 | ai:claude
        if !*no_schedule && init_footprint == cli::InitFootprint::Full {
            maintenance_schedule::maybe_offer_tick_install(&statusline_project_root());
        }
        // STORY-831: minimal-footprint projects intentionally do not commit
        // agent instruction files, so the machine-global awareness snippet is
        // the path that lets a fresh agent notice AIDA. Keep every other
        // first-machine global setup full-only, but offer this idempotent
        // snippet for both full and minimal TTY init. Non-interactive init and
        // a declined prompt leave user files untouched.
        if let Err(e) = maybe_offer_user_aida_instructions() {
            eprintln!(
                "  {} user AIDA instructions skipped: {}",
                "Note:".dimmed(),
                e
            );
        }
        // Starter memory pack — opt-in, orthogonal to storage mode. Failure
        // is non-fatal: it writes outside the project root, so a hiccup
        // there must not abort an otherwise-successful init. trace:STORY-255
        if (*with_memories || *refresh) && init_footprint == cli::InitFootprint::Full {
            // STORY-362: --focus <subsystem> scopes the pack to universal
            // (untagged) memories plus those whose `subsystem:` frontmatter
            // matches. trace:STORY-362 | ai:claude
            if let Err(e) = scaffold_memory_pack(*refresh, focus.as_deref()) {
                eprintln!("  {} starter memory pack skipped: {}", "Note:".dimmed(), e);
            }
        }
        // TASK-1170: --refresh is not memory-pack-only any more. The same
        // edit-preserving overlay now converges every installed agent pack
        // (Claude skills/commands, Codex skills, Antigravity skills, and the
        // machine-global Codex custom prompts) onto this binary's templates,
        // so a template fix reaches a non-Claude agent without --force.
        if *refresh && init_footprint == cli::InitFootprint::Full {
            let packs = scaffold_refresh::refresh_agent_packs(&statusline_project_root(), None);
            scaffold_refresh::print_refresh_summary(&packs);
            // Seed into THIS project's store: the distributed store first, then
            // a local legacy store. Never the registry's ambient default, which
            // could be an unrelated project's database. trace:BUG-1603 | ai:claude
            // A distributed project whose store isn't attached here must not
            // fall through to a stale `requirements.db` in the cwd either: try
            // to attach the store, and otherwise skip seeding. trace:TASK-1486 | ai:claude
            let cwd = std::env::current_dir()?;
            let target = refresh_seed_target(
                detect_distributed_store(),
                unattached_distributed_root(&cwd).is_some(),
                || determine_requirements_path(None),
            );
            let target = match target {
                RefreshSeedTarget::DistributedUnattached => {
                    match unattached_distributed_root(&cwd) {
                        // BUG-433 shape: the store is already physically
                        // attached at `<root>/.aida-store` (no fetch/attach
                        // needed) — use it directly, matching the main
                        // resolver's `store_path_opt` computation.
                        // trace:TASK-1487 | ai:claude
                        Some(ref root) if attached_store_present(root) => {
                            RefreshSeedTarget::Store(root.join(".aida-store"))
                        }
                        Some(root) if branch_exists_anywhere(&root, "aida-store") => {
                            match try_attach_store_worktree(&root) {
                                Ok(store_path) => {
                                    eprintln!(
                                        "  {} attached the AIDA store worktree from \
                                         `aida-store`",
                                        "Note:".dimmed()
                                    );
                                    RefreshSeedTarget::Store(store_path)
                                }
                                // Auto-attach failed (offline, diverged/locked
                                // branch, git too old, …) — print the cause,
                                // as the main resolver does for the same
                                // failure, instead of silently dropping it.
                                // trace:TASK-1487 | ai:claude
                                Err(e) => {
                                    eprintln!(
                                        "  {} couldn't auto-attach the store worktree: {}",
                                        "Note:".dimmed(),
                                        e
                                    );
                                    RefreshSeedTarget::DistributedUnattached
                                }
                            }
                        }
                        _ => RefreshSeedTarget::DistributedUnattached,
                    }
                }
                other => other,
            };
            match target {
                RefreshSeedTarget::Store(store_path) => {
                    let storage = Storage::new(store_path);
                    let seeded = protocol_cmd::seed_missing_protocols(&storage)?;
                    if seeded > 0 {
                        println!("  {} seeded {seeded} missing type protocol(s)", "+".green());
                    }
                }
                RefreshSeedTarget::DistributedUnattached => eprintln!(
                    "  {} type protocols not seeded: this is a distributed AIDA project, but \
                     its store isn't attached in this working copy (no `.aida-store/` \
                     worktree). A local legacy requirements store here was left untouched.",
                    "Note:".dimmed()
                ),
                RefreshSeedTarget::NoStore => eprintln!(
                    "  {} type protocols not seeded: no requirements store found here",
                    "Note:".dimmed()
                ),
            }
        }
        // TASK-859: surface a small curated set of high-value config knobs that
        // are otherwise silent defaults (telemetry opt-out today) and offer to
        // open `aida config menu` for the full surface. TTY-gated + idempotent,
        // mirroring the agent-posture prompt above — non-interactive init writes
        // nothing and keeps every default. Non-fatal: a hiccup here must not
        // abort an otherwise-successful init. trace:TASK-859 | ai:claude
        if init_footprint == cli::InitFootprint::Full {
            if let Err(e) = maybe_prompt_init_config(&statusline_project_root()) {
                eprintln!("  {} config prompts skipped: {}", "Note:".dimmed(), e);
            }
        }
        // STORY-780: remote + ordered pushes, only for a bootstrap init and
        // only now that the store branch exists. trace:STORY-780 | ai:claude
        if let Some(plan) = &bootstrap {
            init_bootstrap::finish_remote(plan)?;
        }
        // STORY-1467: classify forge / CI / local-only capabilities now that
        // any bootstrap remote exists, record them under
        // `[project.capabilities]`, and print them so forge-dependent features
        // degrade explicitly. Best-effort. trace:STORY-1467 | ai:claude
        project_capabilities::record_and_report(&statusline_project_root());
        // Register the initialized project in the machine-global project
        // registry after bootstrap remote setup, so `repo` reflects the final
        // origin URL when one was configured. Best-effort because this writes
        // outside the project root. trace:STORY-827 | ai:codex
        if init_footprint == cli::InitFootprint::Full {
            if let Err(e) = register_project_in_global_registry(&statusline_project_root()) {
                eprintln!("  {} project registry skipped: {}", "Note:".dimmed(), e);
            }
        }
        // STORY-828: machine-global personal hooks run only after the whole
        // init lifecycle has succeeded, including bootstrap remote setup and
        // the best-effort global project registry write.
        if !*no_post_hooks && init_footprint == cli::InitFootprint::Full {
            let project_root = statusline_project_root();
            let remote_url = init_cmd::git_origin_url(&project_root).unwrap_or_default();
            let preferred_forge = bootstrap.as_ref().and_then(|plan| {
                if plan.github {
                    Some("github")
                } else if plan.forge.is_some() && plan.remote.is_some() {
                    Some("gitlab")
                } else {
                    None
                }
            });
            let forge = init_cmd::detect_init_forge(preferred_forge, &remote_url);
            let context = init_cmd::PostInitHookContext {
                project_root,
                project_name: init_cmd::init_project_name(name.as_deref()),
                lang: lang.clone().unwrap_or_default(),
                remote_url,
                forge,
            };
            if let Err(e) = init_cmd::run_post_init_hooks(&context) {
                eprintln!("  {} post-init hooks skipped: {}", "Note:".dimmed(), e);
            }
        }
        return Ok(());
    }

    // Handle upgrade before storage resolution — it needs no DB.
    // trace:EPIC-1-001 | ai:claude
    if let Command::Upgrade {
        check,
        version,
        yes,
        target,
        diff,
        cmd,
    } = &cli.command
    {
        let (check, diff) = normalize_upgrade_mode(*check, *diff, cmd.as_ref());
        return upgrade_cmd::handle_upgrade_command(
            check,
            version.as_deref(),
            *yes,
            target.as_deref(),
            diff,
        );
    }

    // STORY-410: `aida memories check` compares the local Claude Code memory
    // pack to the binary's embedded master — no store access needed, so handle
    // it before storage resolution (like init/upgrade). trace:STORY-410
    if let Command::Memories(MemoriesCommand::Check { verbose, json }) = &cli.command {
        return memories_cmd::handle_memories_check(*verbose, *json);
    }

    // `aida schema` is a pure reflection read — it touches no store, so
    // dispatch it before storage resolution (like `memories`/`dev`).
    // trace:STORY-538 | ai:claude
    if let Command::Schema {
        object,
        all,
        explain,
        json,
    } = &cli.command
    {
        // `--all` is the whole-catalog dump; a positional `<object>` is a
        // single-object request. The two are contradictory, so `--all` carries
        // `conflicts_with = "object"` in the clap definition (cli.rs) — clap
        // rejects the combination with a clear error before this dispatch runs,
        // so no runtime guard is needed here. trace:TASK-775 | ai:claude
        match object.as_deref().map(str::to_lowercase).as_deref() {
            // `--all` (human) and no-arg `--json` are both a full dump: the
            // catalog plus every object's reflection-derived field detail in
            // catalog order. For JSON we always include the per-object fields
            // (a no-arg `--json` was field-less before), so the machine surface
            // is a true one-fetch full dump. trace:TASK-799 trace:TASK-775 | ai:claude
            None if *all || *json => schema::print_all(*json, *explain),
            // `aida schema --explain` (no object) adds the lifecycle blocks to
            // the catalog view. trace:STORY-630 | ai:claude
            None if *explain => schema::print_catalog(*json, true),
            None => schema::print_catalog(*json, false),
            Some("requirement") | Some("requirements") | Some("req") => {
                schema::print_requirement(*json, *explain)
            }
            Some(other) if schema::is_catalog_object(other) => {
                // A catalog object other than Requirement — render its
                // reflection-derived field table. STORY-538 shipped Requirement
                // detail only; TASK-714 extended the reflection registry to
                // every remaining kind. trace:STORY-538 trace:TASK-714
                schema::print_object(other, *json, *explain)
            }
            Some(other) => {
                anyhow::bail!(
                    "unknown schema object `{other}` — run `aida schema` for the catalog, \
                     or `aida schema requirement` for the Requirement detail"
                );
            }
        }
        return Ok(());
    }

    // Handle skill commands before storage resolution — needs no DB.
    if let Command::Skill(skill_cmd) = &cli.command {
        match skill_cmd {
            SkillCommand::Render { name } => {
                let project_root = find_project_root()
                    .or_else(|_| std::env::current_dir())
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Error: could not find project root or current directory: {}",
                            e
                        )
                    })?;
                let skills_dir = project_root.join(".claude").join("skills");
                let stock_path = skills_dir.join(format!("{}.md", name));

                if !stock_path.exists() {
                    anyhow::bail!("Error: skill '{}' not found under .claude/skills/", name);
                }

                let stock_content = std::fs::read_to_string(&stock_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read skill file: {}", e))?;

                let local_path = skills_dir.join(format!("{}.local.md", name));
                if local_path.exists() {
                    let local_content = std::fs::read_to_string(&local_path)
                        .map_err(|e| anyhow::anyhow!("Failed to read local skill file: {}", e))?;

                    let sep = if stock_content.ends_with('\n') {
                        ""
                    } else {
                        "\n"
                    };
                    print!("{}{}{}", stock_content, sep, local_content);
                } else {
                    print!("{}", stock_content);
                }
                return Ok(());
            }
            // `aida skill lint` reads `.claude/skills/*.md` and the plan files
            // they reference — purely filesystem, no store handle. Dispatch it
            // here alongside the other skill tooling. trace:TASK-927 | ai:claude
            SkillCommand::Lint { skill, json, quiet } => {
                return lint_skills(skill.as_deref(), *json, *quiet);
            }
        }
    }

    // Handle dev commands before storage resolution — most need no DB.
    // (Dev::Serve does interact with storage but spawns aida-server which
    // handles that itself; the wrapper just supervises the children.)
    // trace:EPIC-1-001 | ai:claude
    if let Command::Dev(dev_cmd) = &cli.command {
        return crate::dev_cmd::handle_dev_command(dev_cmd);
    }

    // STORY-472: `aida release` — a memorable top-level verb wrapping the dev
    // release flow (store pull → scripts/release.sh → tarball wait → sibling
    // upgrade). Operates on the repo, not the requirements store, so dispatch
    // early like `dev`. trace:STORY-472 | ai:claude
    if let Command::Release {
        patch,
        minor,
        major,
        check,
        after_pr,
        skip_xplat_check,
        cmd,
    } = &cli.command
    {
        let (patch, minor, major, check) =
            normalize_release_mode(*patch, *minor, *major, *check, cmd.as_ref());
        return handle_release(patch, minor, major, check, *after_pr, *skip_xplat_check);
    }

    // STORY-527: `aida burndown` reads the requirement graph (like `graph` /
    // `plan`) — no shared storage handle needed. Dispatch early.
    // trace:STORY-527 | ai:claude
    if let Command::Burndown(cmd) = &cli.command {
        return handle_burndown_command(cmd);
    }

    // STORY-658: `aida health` self-loads the store + probes the runtime
    // substrate (leases, drain lock, queue) — no shared storage handle needed,
    // so dispatch early like burndown. trace:STORY-658 | ai:claude
    if let Command::Health { json, brief } = &cli.command {
        return health_vitals_cmd::handle_health_vitals_command(*json, *brief);
    }

    // STORY-560: `aida groom` (canonical; `assess`/`intake` are deprecated
    // aliases — STORY-708) self-loads the store to compute its candidate
    // fence and then launches a headless `claude -p` — no shared storage handle
    // needed. Dispatch early, like burndown. trace:STORY-560 trace:STORY-708 | ai:claude
    if let Command::Groom {
        apply,
        max_approvals,
        only_tag,
        exclude_tag,
        risk,
        then_drain,
        dry_run,
        permission_mode,
    } = &cli.command
    {
        return handle_intake_command(
            *apply,
            *max_approvals,
            only_tag.as_deref(),
            exclude_tag.as_deref(),
            risk,
            *then_drain,
            *dry_run,
            permission_mode.as_deref(),
        );
    }

    // TASK-1147: `aida autopilot` (inspect / audit / challenge) is the read-only
    // auditability + reversal surface over the policy envelope. Like `aida
    // groom` it self-loads the store to compute its candidate fence and writes
    // nothing to any spec — dispatch early, no shared storage handle needed.
    // trace:TASK-1147 | ai:claude
    if let Command::Autopilot(cmd) = &cli.command {
        return handle_autopilot_command(cmd);
    }

    // STORY-547: `aida why <ID>` reads the requirement graph like burndown —
    // dispatch early, no shared storage handle needed. trace:STORY-547
    if let Command::Why { id, plain, json } = &cli.command {
        return handle_why(id, *plain, *json);
    }

    // trace:EPIC-72 trace:TASK-1436 | ai:antigravity
    if let Command::Explain {
        spec,
        audience,
        refresh,
        force,
        json,
    } = &cli.command
    {
        return exposition::handle_explain_command(spec, audience, *refresh, *force, *json);
    }

    // trace:EPIC-72 trace:TASK-1439 | ai:antigravity
    if let Command::Wiki(command) = &cli.command {
        return wiki::handle_wiki_command(command);
    }

    // STORY-694: `aida status <spec>` is the per-spec liveness view — it reads
    // the local session leases + probes pid liveness and self-loads the store
    // read-only for the spec's lifecycle status. Like `aida why` it needs no
    // shared storage handle, so dispatch early (before storage init). The
    // bare `aida status` (no spec) still flows through the normal dispatch.
    // trace:STORY-694 | ai:claude
    if let Command::Status {
        spec: Some(spec),
        idle_minutes,
        json,
        ..
    } = &cli.command
    {
        // trace:BUG-1289 | ai:claude
        return handle_status_spec(spec, *idle_minutes, *json || output_format_is_json());
    }

    // TASK-188: make dev-binary staleness visible at the point of use. One
    // stderr line, local git only (no network), silent outside the AIDA
    // workspace or when the running build matches the default branch.
    // trace:TASK-188 | ai:claude
    if let Command::Status { spec: None, .. } = &cli.command {
        crate::freshness_gate::warn_if_running_binary_stale();
    }

    // STORY-769: the `aida awaiting --notice` per-turn hook ALWAYS leads with a
    // "Current date/time + Timing" line — a deliberate contract change from the
    // old silent-when-empty behaviour. Emit it EARLY, before store init, so the
    // hook still gets its time context even if the store/backend can't be
    // resolved (offline, cache-locked, unattached), and so the last-human-input
    // presence oracle is stamped every turn. The compact "Awaiting you" line
    // still prints later from `handle_awaiting_command`; this only prepends the
    // always-on time line. trace:STORY-769 | ai:claude
    if let Command::Awaiting {
        notice: true,
        json: false,
        ..
    } = &cli.command
    {
        // Arm before any notice-specific filesystem work. Session-start
        // registration and the turn-clock stamp are intentionally fail-open,
        // but can still stall on a contended filesystem.
        arm_notice_deadline();
        emit_notice_time_line();
    }

    // STORY-696: `aida ps` is the GLOBAL running-work table — the project-wide
    // companion to `aida status <spec>`. Like that command it reads the local
    // session leases + probes pid liveness and self-loads the store read-only
    // (to find orphaned In-Progress specs), so it needs no shared storage
    // handle — dispatch early. trace:STORY-696 | ai:claude
    if let Command::Ps { json, all } = &cli.command {
        // trace:BUG-1289 | ai:claude
        return handle_ps(*json || output_format_is_json(), *all);
    }

    // `aida merge-lock` shows active branch merge-leases (STORY-1171). Read-only
    // (reads `.aida/merge-locks/`), needs no store handle — dispatch early like
    // `aida ps`. trace:STORY-1171 | ai:claude
    if let Command::MergeLock { json } = &cli.command {
        return handle_merge_lock_status(*json);
    }

    // `aida merge-hold list|clear` inspects/clears supervised merge-hold markers.
    // Read/writes only `.aida/merge-holds/` (+ a best-effort `gh` label/merged
    // probe), needs no store handle — dispatch early like `aida merge-lock`.
    // trace:TASK-161 | ai:claude
    if let Command::MergeHold { action } = &cli.command {
        return handle_merge_hold(action);
    }

    // `aida tail` is `aida ps`'s streaming companion: it resolves a session /
    // spec / drain id to the ONE log file that work streams into and tails it.
    // Reads only `.aida/burndown/`, `.aida/headless-logs/` and the lease dir —
    // no store, no network — so dispatch early like `aida ps`.
    // trace:TASK-1167 | ai:claude
    if let Command::Tail {
        target,
        list,
        json,
        lines,
        since,
        no_follow,
        with_tools,
        no_timestamp,
        annotate,
    } = &cli.command
    {
        return handle_tail_cli(
            target.clone(),
            *list,
            *json,
            *lines,
            since.as_deref(),
            *no_follow,
            *with_tools,
            *no_timestamp,
            *annotate,
        );
    }

    // `aida statusbar` — the ambient, read-only terminal-title meter. Like
    // `aida ps` / `aida integrate` it self-resolves everything it needs
    // (queue YAML, session leases, cache-backed awaiting channels) and
    // degrades gracefully offline, so it needs no shared storage handle —
    // dispatch early. trace:STORY-715 | ai:claude
    if let Command::Statusbar {
        interval,
        once,
        plain,
        restore_title,
    } = &cli.command
    {
        return statusbar_cmd::handle_statusbar_command(*interval, *once, *plain, *restore_title);
    }

    if let Command::Contract { json: _ } = &cli.command {
        return monitor_contract::print_json();
    }

    // `aida watch` is a read-only consumer of the slice-1 `.aida/events.jsonl`
    // stream (STORY-712): it tails the file, classifies each event in cheap
    // code, and wakes only on actionable verbs. It touches no store/backend, so
    // dispatch early like `aida ps`. trace:TASK-990 | ai:claude
    if let Command::Watch {
        emit_wakes: _,
        all,
        json,
        verbose,
        once,
        backlog,
    } = &cli.command
    {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return watch::handle_watch(
            &project_root,
            &watch::WatchOpts {
                all: *all,
                // trace:BUG-1289 | ai:claude
                json: *json || output_format_is_json(),
                // trace:TASK-994 | ai:claude
                verbose: *verbose,
                once: *once,
                backlog: *backlog,
            },
        )
        .and_then(|()| notify::passive_check(&project_root));
    }

    // `aida notify` has no need for the requirement store: it consumes the
    // local event stream plus `.aida/config.toml`, so it is safe for cron and
    // for hooks that run while the cache/store may be locked.
    // trace:STORY-1029 | ai:codex
    if let Command::Notify(cmd) = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return notify::handle_notify_command(&project_root, cmd);
    }

    // `aida integrate` is the integrator front door. Bare remains the read-only
    // throughput/merge-queue view (TASK-1034); `--run` / `--watch` / `--dry-run`
    // enter the same serialized engine as `aida queue integrate`, with rebase
    // enabled by default so Done-with-PR specs land through one current-main
    // queue instead of racing each other stale. trace:STORY-1024 | ai:codex
    if let Command::Integrate {
        json,
        run,
        dry_run,
        watch,
        interval,
        max,
        wait_ci: _,
        no_wait_ci,
        no_rebase,
        strategy,
        focus,
        idle_minutes,
        force,
        user,
    } = &cli.command
    {
        return handle_integrate(IntegrateCommandOpts {
            // trace:BUG-1289 | ai:claude
            json: *json || output_format_is_json(),
            run: *run,
            dry_run: *dry_run,
            watch: *watch,
            interval: *interval,
            max: *max,
            wait_ci: !*no_wait_ci,
            rebase: !*no_rebase,
            strategy: *strategy,
            focus: focus.clone(),
            idle_minutes: *idle_minutes,
            force: *force,
            user: user.clone(),
        });
    }

    // TASK-957: `aida claim` / `aida unclaim` self-load the store read-only to
    // resolve the spec id forms, then write/remove a lightweight advisory lease
    // under `.aida/sessions/`. Like `aida ps` / `aida status <spec>` they touch
    // only the local lease dir + a cache-backed lookup, so they need no shared
    // storage handle — dispatch early. trace:TASK-957 | ai:claude
    if let Command::Claim { spec, worktree } = &cli.command {
        return handle_claim(spec, worktree.as_deref());
    }
    if let Command::Unclaim { spec } = &cli.command {
        return handle_unclaim(spec);
    }

    // STORY-631: `aida intent <ID>` self-loads the store (read for cache-print,
    // read+write for generate), and may shell out to a headless `claude -p`
    // running `/aida-intent`. Like `aida why` / `aida intake`, it needs no
    // shared storage handle — dispatch early. trace:STORY-631 | ai:claude
    if let Command::Intent {
        id,
        audience,
        refresh,
        json,
    } = &cli.command
    {
        return handle_intent(id, audience, *refresh, *json);
    }

    // STORY-656: `aida spec dryrun <ID>` self-loads the store (read-only) for
    // the deterministic pre-check, and with `--ai` may shell out to a headless
    // `claude -p`. Like `aida intent`, it needs no shared storage handle —
    // dispatch early. trace:STORY-656 | ai:claude
    if let Command::Spec(SpecCommand::Dryrun { id, ai, json }) = &cli.command {
        return handle_spec_dryrun(id, *ai, *json);
    }

    // STORY-657: `aida spec interview <ID>` reuses dryrun's scorer to derive
    // clarifying questions, prompts the human (TTY) or emits them as JSON
    // (headless), and with `--apply` folds the answers back into the spec. Like
    // dryrun it self-loads the store; dispatch early. trace:STORY-657 | ai:claude
    if let Command::Spec(SpecCommand::Interview {
        id,
        apply,
        ai,
        answers,
        json,
    }) = &cli.command
    {
        return handle_spec_interview(id, *apply, *ai, answers.as_deref(), *json);
    }

    // STORY-716: `aida worktree` (EPIC-55 workspace layer) operates on git +
    // the filesystem; `enter` emits `cd` shell on stdout for the `aida()`
    // wrapper to auto-eval. Like `role enter` it needs no shared storage
    // handle and its stdout must stay clean of store-init noise, so it
    // dispatches early. trace:STORY-716 | ai:claude
    if let Command::Worktree(wt_cmd) = &cli.command {
        return handle_worktree_command(wt_cmd);
    }

    // STORY-563: `aida human <subcommand>` self-loads the store like `aida why`
    // / `burndown explain`, or delegates to the top-level presence handlers.
    // Dispatch early, no shared storage handle needed.
    // trace:STORY-563 | ai:claude
    // Presence + unblock subcommands need no storage handle, so dispatch them
    // here before store init. The STORY-611 action aliases (`answer`/`review`/
    // `decide`) DO need the backend — they fall through to the main dispatch.
    // trace:STORY-611 | ai:claude
    if let Command::Human {
        command: Some(human_cmd),
        ..
    } = &cli.command
    {
        if human_subcommand_needs_no_storage(human_cmd) {
            return handle_human_subcommand(human_cmd);
        }
    }

    // Doctor commands run before storage init — they may need to operate
    // on broken or partially-migrated stores. trace:EPIC-19 | ai:claude
    if let Command::Doctor {
        heal,
        yes,
        category,
        json,
        force,
        all,
        since,
        fix_sandbox,
        contradictions,
        contradictions_limit,
        contradictions_offset,
        cmd,
    } = &cli.command
    {
        // STORY-665: `aida doctor --fix-sandbox` is a guided printer for bringing
        // bwrap OS-confinement up on a new machine. Short-circuit before the
        // multi-agent drift scan — it's a standalone setup helper, not a check.
        // trace:STORY-665 | ai:claude
        if *fix_sandbox {
            return doctor_cmd::doctor_fix_sandbox();
        }
        // STORY-1426: `aida doctor --contradictions` runs the store-wide semantic
        // contradiction sweep combining mechanical joins with Jev choice queries.
        // trace:STORY-1426 | ai:antigravity
        if *contradictions {
            return doctor_cmd::doctor_contradictions(
                *json,
                *contradictions_limit,
                *contradictions_offset,
                *all,
            );
        }
        return doctor_cmd::handle_doctor_command(
            *heal,
            *yes,
            category.as_deref(),
            *json,
            *force,
            *all,
            since.as_deref(),
            cmd.as_ref(),
        );
    }

    // Store commands inspect git state, no AIDA storage needed.
    // trace:EPIC-21 | ai:claude
    if let Command::Store(store_cmd) = &cli.command {
        return store_cmd::handle_store_command(store_cmd);
    }

    // `aida remote create`/`attach` bootstrap a git origin — they operate on a
    // project that, by definition, may not have a working store/remote yet, so
    // dispatch before storage init. trace:STORY-537 | ai:claude
    if let Command::Remote(remote_cmd) = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return match remote_cmd {
            crate::cli::RemoteCommand::Create {
                attach,
                github,
                gitlab,
                public,
            } => remote_create::handle_remote_create(
                &project_root,
                attach.as_deref(),
                *github,
                gitlab.as_deref(),
                !*public,
            ),
            crate::cli::RemoteCommand::Attach { url } => {
                remote_create::handle_remote_attach(&project_root, url)
            }
            crate::cli::RemoteCommand::Status { json, no_fetch } => {
                remote_create::handle_remote_status(&project_root, *json, *no_fetch)
            }
            // trace:TASK-1097 | ai:claude
            crate::cli::RemoteCommand::Mirror { name, url } => {
                remote_create::handle_remote_mirror(&project_root, name, url.as_deref())
            }
            crate::cli::RemoteCommand::MirrorPush {
                pushed_remote,
                dry_run,
            } => remote_create::handle_remote_mirror_push(&project_root, pushed_remote, *dry_run),
            // trace:BUG-1676 | ai:claude
            crate::cli::RemoteCommand::MirrorSync { json } => {
                remote_create::handle_remote_mirror_sync(&project_root, *json)
            }
            crate::cli::RemoteCommand::Reconcile { execute, json, yes } => {
                remote_create::handle_remote_reconcile(&project_root, *execute, *json, *yes)
            }
        };
    }

    // TASK-1330: the identity HYGIENE subcommands read only git state and
    // `.aida/config.toml` — no store needed, and the pre-push hook plumbing
    // must work even when the store is unavailable — so dispatch them before
    // storage init, like `remote`. The person-alias registry subcommands
    // (link/list/show) read the store and fall through to the normal path.
    // trace:TASK-1330 | ai:claude
    if let Command::Identity { cmd } = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        match cmd {
            crate::cli::IdentityCommand::Check => {
                return identity_gate::handle_identity_check(&project_root);
            }
            crate::cli::IdentityCommand::CheckPush { pushed_remote } => {
                return identity_gate::handle_identity_check_push(&project_root, pushed_remote);
            }
            crate::cli::IdentityCommand::InstallHook => {
                return identity_gate::handle_identity_install_hook(&project_root);
            }
            crate::cli::IdentityCommand::Link { .. }
            | crate::cli::IdentityCommand::List { .. }
            | crate::cli::IdentityCommand::Show { .. } => {}
        }
    }

    // Sandbox commands MANAGE a throwaway store directory; they don't read the
    // project's store, so dispatch before store detection (and so they work
    // even from a non-AIDA cwd). trace:SPIKE-48 | ai:claude
    if let Command::Sandbox(sandbox_cmd) = &cli.command {
        return sandbox_cmd::handle_sandbox_command(sandbox_cmd);
    }

    // Help-all / commands are pure text; no storage needed.
    if let Command::HelpAll = &cli.command {
        print_help_all();
        return Ok(());
    }
    if let Command::Commands {
        flags,
        json,
        hidden,
    } = &cli.command
    {
        help_catalog::print_command_catalog(*flags, *hidden, *json);
        return Ok(());
    }

    // `aida alias` / `aida alias list` is a built-in-shortcut registry (now also
    // surfacing user-defined aliases) — it reads no requirement store and needs
    // no storage handle, so dispatch it early like help-all. Bare `aida alias`
    // defaults to `list`. `add` / `remove` manage user aliases under the chosen
    // scope. trace:STORY-667 | ai:claude
    // trace:TASK-877 | ai:claude — user-alias CRUD dispatch.
    if let Command::Alias { json, command } = &cli.command {
        match command {
            Some(crate::cli::AliasCommand::Add {
                name,
                command: cmd_tokens,
                global,
                project,
            }) => {
                let scope = user_alias::resolve_scope(*project, *global)?;
                return user_alias::add(scope, name, cmd_tokens);
            }
            Some(crate::cli::AliasCommand::Remove {
                name,
                global,
                project,
            }) => {
                let scope = user_alias::resolve_scope(*project, *global)?;
                return user_alias::remove(scope, name);
            }
            Some(crate::cli::AliasCommand::List { json: sub_json }) => {
                return alias::run(*json || *sub_json);
            }
            None => return alias::run(*json),
        }
    }

    // Plan tooling is self-contained: `verify` reads a markdown file +
    // source files; `helpers` loads the store itself via
    // `load_store_for_lookup`. Neither needs the shared storage handle, so
    // dispatch the whole group early. trace:TASK-93 TASK-94 | ai:claude
    if let Command::Plan(plan_cmd) = &cli.command {
        return plan_cmd::handle_plan_command(plan_cmd);
    }

    // `aida deps sweep` self-loads the store via `load_store_for_lookup`
    // and scans source for trace comments — like `plan helpers`, it needs
    // no shared storage handle. Read-only. trace:STORY-447 | ai:claude
    if let Command::Deps(deps_cmd) = &cli.command {
        return deps_cmd::handle_deps_command(deps_cmd);
    }

    // `aida lint` self-loads the store via `load_store_for_lookup` and scores
    // requirement text with the EARS heuristics — read-only, no shared storage
    // handle, no LLM. trace:TASK-0417 | ai:claude
    if let Command::Lint { spec, scope, json } = &cli.command {
        return lint_cmd::handle_lint_command(spec.as_deref(), scope.as_deref(), *json);
    }

    // `aida gate` reads the embedded gate library (and, for `run`, self-loads
    // the store) — no shared storage handle. trace:STORY-1427 | ai:claude
    if let Command::Gate(gate_cmd) = &cli.command {
        return gate_cmd::handle_gate_command(gate_cmd);
    }

    // `aida lifecycle` (Phase 1, generate-only) is self-contained: it renders a
    // Mermaid diagram from the declared transition model in aida-core and
    // optionally pins it against `docs/lifecycle.md`. No storage handle, no LLM,
    // no behavior change to any other command. trace:TASK-737 | ai:claude
    if let Command::Lifecycle {
        diagram,
        check,
        write,
        doc,
        empirical,
        diff,
    } = &cli.command
    {
        return lifecycle_cmd::handle_lifecycle_command(
            *diagram,
            *check,
            *write,
            doc.as_deref(),
            *empirical,
            *diff,
        );
    }

    // STORY-498: `aida trace gate` is self-contained — it reads git for the
    // commit range and self-loads the store via `load_store_for_lookup` to
    // resolve each `(SPEC-ID)` trailer. Dispatch it early (the other `trace`
    // subcommands still need the shared storage handle and fall through to the
    // backend path). Exits non-zero on a dead/dangling reference.
    // trace:STORY-498 | ai:claude
    if let Command::Trace(TraceCommand::Gate { range, json }) = &cli.command {
        return handle_trace_gate(range.as_deref(), *json);
    }

    // STORY-499: `aida trace coverage` is self-contained the same way `gate` is
    // — it reads git for the diff + range and self-loads the store to resolve
    // `// trace:` anchors / `(SPEC-ID)` trailers. Dispatch it early so it never
    // needs the shared storage handle. trace:STORY-499 | ai:claude
    if let Command::Trace(TraceCommand::Coverage { range, json, block }) = &cli.command {
        return handle_trace_coverage(range.as_deref(), *json, *block);
    }

    // `aida changelog` is self-contained: git + read-only store, no shared
    // storage handle. Dispatch alongside Plan/Ultraplan. trace:TASK-299 | ai:claude
    if let Command::Changelog(cl_cmd) = &cli.command {
        return changelog_cmd::handle_changelog_command(cl_cmd);
    }

    // `aida manual <cmd>` only reads the markdown chapters under docs/cli/ —
    // no store, no cache, no network. Dispatch it before storage init like
    // changelog. trace:STORY-600 | ai:claude
    if let Command::Manual { command } = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return manual::run(command, &project_root);
    }

    // `aida derisk` is a thin advisor-session launcher; it uses the shared
    // agent-new machinery and does not need the shared storage handle.
    // trace:TASK-1235 | ai:codex
    if let Command::Derisk { spec } = &cli.command {
        return handle_derisk_command(spec);
    }

    // `aida ultraplan` also self-loads the store via `load_store_for_lookup`.
    // trace:TASK-113 | ai:claude
    if let Command::Ultraplan {
        spec,
        stdout,
        json,
        copy: _,
        no_comments,
    } = &cli.command
    {
        return ultraplan_cmd::handle_ultraplan_command(spec, *stdout, *json, *no_comments);
    }

    // `aida compete` self-loads the store (to assemble the brief) and
    // orchestrates worktrees + headless vendor runs + the objective gate.
    // Dispatch early alongside ultraplan — it doesn't need the shared storage
    // handle. trace:STORY-659 | ai:claude
    if let Command::Compete {
        spec,
        vendors,
        gate,
        dry_run,
        judge,
        judge_vendor,
    } = &cli.command
    {
        return compete_cmd::handle_compete_command(
            spec,
            vendors,
            gate.as_deref(),
            *dry_run,
            *judge,
            judge_vendor,
        );
    }

    // `aida commit` is self-contained — git + the staged diff (for REQ-ID
    // inference), no requirements store. Dispatch before storage init like
    // goal / ultraplan. trace:STORY-663 | ai:claude
    if let Command::Commit {
        commit_type,
        scope,
        message,
        spec,
        ai,
        no_ai,
        all,
        dry_run,
    } = &cli.command
    {
        return commit::handle_commit_command(&commit::CommitArgs {
            commit_type: commit_type.clone(),
            scope: scope.clone(),
            message: message.clone(),
            spec: spec.clone(),
            ai: ai.clone(),
            no_ai: *no_ai,
            all: *all,
            dry_run: *dry_run,
        });
    }

    // `aida internal advisor-code-gate` is the vendor-agnostic substrate
    // enforcement of the advisor-no-code-write invariant (STORY-684). The
    // scaffolded git pre-commit hook shells out to it so ANY vendor's
    // `git commit` (Codex, raw terminal, headless) hits the same gate the
    // Claude PreToolUse hook only ever gave Claude. Self-contained (git diff +
    // env + the solo marker), so it dispatches before storage init.
    // trace:STORY-684
    if let Command::Internal { command } = &cli.command {
        return internal_cmd::handle_internal_command(command);
    }

    // `aida goal` is a pure condition generator — no store needed.
    // trace:TASK-242 | ai:claude
    if let Command::Goal {
        batch,
        epic,
        spec,
        pr,
        queue_empty,
        copy,
        invoke,
        as_deep_link,
    } = &cli.command
    {
        return goal_cmd::handle_goal_command(
            batch.as_deref(),
            epic.as_deref(),
            spec.as_deref(),
            *pr,
            queue_empty.as_deref(),
            *copy,
            *invoke,
            *as_deep_link,
        );
    }

    // `aida tui` is the EPIC-26 TUI shell — a process supervisor that
    // holds no storage handle (it shells out to `aida` subcommands), so
    // it dispatches before storage init like Goal / Statusline.
    // trace:STORY-132 | ai:claude
    if let Command::Tui {
        scope,
        no_recover,
        launcher,
        intent_fd,
    } = &cli.command
    {
        return handle_tui_command(scope.clone(), *no_recover, *launcher, *intent_fd);
    }

    // `aida config menu` is a navigable TUI view over the resolved config
    // surface (STORY-661). Like `aida tui` it holds no storage handle — the
    // policy registry it renders resolves from `.aida/config.toml` + the
    // global files + env directly — so it dispatches before storage init.
    // trace:STORY-661 | ai:claude
    if let Command::Config(ConfigCommand::Menu) = &cli.command {
        return config_cmd::handle_config_menu_command();
    }

    // Roles + statusline dispatch before storage init — roles are TOML
    // files at .aida/roles/, statusline reads the cache directly.
    if let Command::Role(role_cmd) = &cli.command {
        return role_cmd::handle_role_command(role_cmd);
    }
    if let Command::Statusline {
        color,
        title,
        client,
        action,
    } = &cli.command
    {
        // trace:TASK-0414 — the opt-in `setup` subcommand bootstraps the
        // statusline; with no subcommand we render the one-liner (default).
        // trace:TASK-896 — `--title` emits the same one-liner wrapped in an
        // OSC terminal-title escape so the AIDA segment rides the terminal
        // title bar (the in-agent parity surface for clients without a
        // command-backed footer, e.g. Codex CLI).
        // trace:TASK-1479 — `--client` opts into merging a client's live
        // stdin JSON payload (model/context/activity/VCS) with the AIDA
        // segment; omitted, behavior is unchanged (no stdin read).
        match action {
            Some(cli::StatuslineAction::Title) => {
                return statusline_cmd::handle_statusline_command(color, true, client.as_deref());
            }
            Some(act) => return statusline_cmd::handle_statusline_setup_command(act),
            None => {
                if *title {
                    note_hidden_alias(
                        concat!("aida statusline ", "--title"),
                        "aida statusline title",
                    );
                }
                return statusline_cmd::handle_statusline_command(color, *title, client.as_deref());
            }
        }
    }
    // STORY-79: hidden background-fetch worker spawned by statusline.
    // Dispatch before storage init — this is a self-contained worker
    // that owns its own git/toml side effects. trace:STORY-79 | ai:claude
    if let Command::BgFetch { store_path } = &cli.command {
        return session_misc_cmd::handle_bg_fetch_command(store_path);
    }
    // TASK-756: operator-presence primitive. Dispatched before storage init —
    // these only touch the machine-global `~/.aida/presence.toml` file; no
    // requirement-store handle needed. trace:TASK-756 | ai:claude
    match &cli.command {
        Command::Away => return presence_cmd::handle_away_command(),
        Command::Home => return presence_cmd::handle_home_command(),
        // trace:TASK-1231 | ai:claude
        Command::Autoprogress {
            project,
            groom,
            max,
            dry_run,
        } => {
            return autoprogress::handle_autoprogress(autoprogress::AutoprogressOpts {
                project: project.clone(),
                groom: *groom,
                max: max.unwrap_or(5),
                dry_run: *dry_run,
            })
        }
        // TASK-851: `aida presence [away|home|status]` is the canonical surface;
        // bare `aida presence` shows status (back-compat). The top-level
        // `aida away` / `aida home` stay as hidden aliases above.
        // trace:TASK-851 | ai:claude
        Command::Presence { action } => {
            return match action {
                Some(cli::PresenceCommand::Away) => presence_cmd::handle_away_command(),
                Some(cli::PresenceCommand::Home) => presence_cmd::handle_home_command(),
                Some(cli::PresenceCommand::Status) | None => {
                    presence_cmd::handle_presence_command()
                }
            };
        }
        // STORY-624: solo-mode flag — machine-global ~/.aida/solo.toml, no
        // requirement store needed (mirrors the away/home early dispatch).
        Command::Solo {
            action,
            off,
            status,
            ttl,
            watch,
            dry_run,
            interval,
        } => {
            return handle_solo_command(
                *action,
                *off,
                *status,
                ttl.as_deref(),
                *watch,
                *dry_run,
                *interval,
            );
        }
        // TASK-784: pure read of the caller-identity resolvers (env vars +
        // current_user_id + detect_agent_type). No project store needed, so
        // it dispatches here alongside the other env-only commands.
        // trace:TASK-784
        Command::Whoami => return session_misc_cmd::handle_whoami_command(),
        _ => {}
    }
    // trace:FR-1-043 | ai:claude
    if let Command::Session(session_cmd) = &cli.command {
        return handle_session_command(session_cmd);
    }
    // trace:STORY-995 | ai:codex — terminal helper installation is user-local
    // and store-independent, so it belongs in the early command tier.
    if let Command::Terminal(terminal_cmd) = &cli.command {
        return handle_terminal_command(terminal_cmd);
    }
    // TASK-661 (ADR-3): disposition/triage lease. Dispatched before storage
    // init — it reads + writes only `.aida/triage-leases/` files and probes
    // PIDs; no requirement-store handle needed. trace:TASK-661 | ai:claude
    if let Command::Triage(triage_cmd) = &cli.command {
        return triage_cmd::handle_triage_command(triage_cmd);
    }
    // STORY-711 slice 1: `aida lock` reads/writes only `.aida/sessions/`
    // lease files (the same registry `aida session leases` uses) — no
    // requirement-store handle needed, same reasoning as `aida triage`
    // above. trace:STORY-711 | ai:claude
    if let Command::Lock(lock_cmd) = &cli.command {
        return lock_cmd::handle_lock_command(lock_cmd);
    }
    // EPIC-31 agent launchers supervise external CLIs and write process
    // registry state directly; no requirement store handle needed.
    // trace:STORY-432 | ai:codex
    if let Command::Agent(agent_cmd) = &cli.command {
        return handle_agent_command(agent_cmd);
    }

    // STORY-90: PR side-effects dispatch before storage init — the
    // command's side-effects all live in git / gh / self-invoked `aida
    // add` calls; it doesn't need the global Storage handle.
    // trace:STORY-90 | ai:claude
    if let Command::Pr(pr_cmd) = &cli.command {
        return handle_pr_command(pr_cmd);
    }

    // trace:TASK-1583 | ai:antigravity
    if let Command::List {
        shortcut: Some(s),
        json,
        ..
    } = &cli.command
    {
        if s == "pr" || s == "mr" {
            return pr_list_handler(*json);
        }
        // trace:TASK-1590 | ai:antigravity
        if s == "review" {
            return handle_review_list(*json);
        }
    }

    // STORY-720: `aida ship` dispatches before storage init for the same
    // reason as `aida pr` — every side-effect is a git / gh / self-invoked
    // `aida` call (the finish tail reuses `pr_ship_handler`). The spec is
    // resolved from the branch / lease, not the requirement store.
    // trace:STORY-720 | ai:claude
    if let Command::Ship {
        spec,
        message,
        no_merge,
        no_pr,
        keep_worktree,
        dry_run,
        no_trailer_check,
    } = &cli.command
    {
        return run_human_finish_ceremony(HumanFinishOptions {
            spec: spec.clone(),
            message: message.clone(),
            no_merge: *no_merge,
            no_pr: *no_pr,
            keep_worktree: *keep_worktree,
            dry_run: *dry_run,
            no_trailer_check: *no_trailer_check,
        });
    }

    // BUG-233 / TASK-336: orchestrator-context introspection. Dispatched
    // before storage init — it reads only env vars + the `.aida/drain-
    // state.json` file's `run_uuid` + PID, no requirement store. (Before
    // TASK-336 it read a sidecar `.aida/orchestrator-runs/<uuid>` marker
    // file; that has been folded into the drain-state file.)
    // trace:BUG-233 trace:TASK-336 | ai:claude
    if let Command::Orchestrator(orch_cmd) = &cli.command {
        return handle_orchestrator_command(orch_cmd);
    }

    // BUG-237: zen-context introspection. Like `orchestrator status`, reads
    // only env vars + the drain-state file + session leases — no requirement
    // store. The `aida zen <spec>` autonomous-drive form (STORY-721) needs the
    // store to validate the spec's status, so only the introspection
    // subcommands short-circuit here; the spec-drive path falls through to the
    // post-storage dispatch. trace:BUG-237 trace:TASK-336 trace:STORY-721 | ai:claude
    if let Command::Zen {
        command: Some(zen_cmd),
        ..
    } = &cli.command
    {
        return handle_zen_command(zen_cmd);
    }

    // STORY-722: `aida zen <spec> --compete` — the 2-agent bake-off. Like
    // `aida compete` it self-loads the store (to resolve the spec + assemble
    // the brief) and orchestrates worktrees + headless vendor runs, so it
    // dispatches early and works identically on both storage backends.
    // trace:STORY-722 | ai:claude
    if let Command::Zen {
        spec,
        compete: true,
        dry_run,
        command: None,
        ..
    } = &cli.command
    {
        let Some(spec) = spec.as_deref() else {
            anyhow::bail!(
                "`aida zen --compete` needs a SPEC id (e.g. `aida zen TASK-123 --compete`)"
            );
        };
        return compete_cmd::handle_zen_compete(spec, *dry_run);
    }

    // STORY-360: live-advisor registration. `aida advisor` writes to
    // `~/.aida/advisor.toml` (per-user, not per-project) and is intended to
    // be runnable from anywhere — including outside any AIDA project. Reads
    // no requirement store. trace:STORY-360 | ai:claude
    // STORY-262: `aida advisor schedule` needs the requirement store (it
    // files TASKs into the queue), so it must NOT short-circuit here — it
    // falls through to the post-storage-init dispatch. Only the
    // store-less registration subcommands run early.
    // trace:STORY-262 | ai:claude
    if let Command::Advisor { command, .. } = &cli.command {
        // STORY-559: `advisor status` (default = dashboard) aggregates the
        // requirement store (burndown / backlog / queue / leases), so it must
        // fall through to the post-storage dispatch. The narrow
        // `--registration` view reads only `~/.aida/advisor.toml` and stays in
        // this store-less early handler so it still works outside a project.
        // STORY-618: bare `aida advisor` (command = None) IS the worklist, which
        // reads the store, so it also falls through. trace:STORY-559 STORY-618
        let needs_store = match command {
            None => true,
            Some(c) => {
                matches!(c, AdvisorCommand::Schedule(_))
                    || matches!(
                        c,
                        AdvisorCommand::Status {
                            registration: false,
                            ..
                        }
                    )
            }
        };
        if !needs_store {
            if let Some(c) = command {
                return handle_advisor_command(c);
            }
        }
    }

    // STORY-301: drain-state introspection. Dispatched before storage init —
    // it reads only `.aida/drain-state.json`, no requirement store.
    // trace:STORY-301 | ai:claude
    if let Command::Drain(drain_cmd) = &cli.command {
        return handle_drain_command(drain_cmd);
    }

    // STORY-248: stack-graph introspection. Dispatched before storage
    // init — reads only `.aida/stacks.json`, no requirement store.
    // Mirrors the STORY-301 drain dispatch pattern. trace:STORY-248
    if let Command::Stack(stack_cmd) = &cli.command {
        return handle_stack_command(stack_cmd);
    }

    // TASK-294: worker-directive introspection. Dispatched before storage
    // init — reads only `.aida/worker.cmd`, no requirement store. Mirrors
    // the STORY-301 drain dispatch pattern exactly. trace:TASK-294 | ai:claude
    if let Command::Worker(worker_cmd) = &cli.command {
        return handle_worker_command(worker_cmd);
    }

    // TASK-398: headless-log tailer. Dispatched before storage init — reads
    // only `.aida/headless-logs/`, no requirement store. trace:TASK-398
    if let Command::Headless(headless_cmd) = &cli.command {
        return handle_headless_command(headless_cmd);
    }

    // STORY-325: punt ledger CLI command. Dispatched before storage init —
    // it reads only `.aida/punts.jsonl`, no requirement store.
    // trace:STORY-325 | ai:antigravity
    if let Command::Punts(punts_cmd) = &cli.command {
        return handle_punts_command(punts_cmd.clone());
    }

    // STORY-439: autonomy / calibration surface. Dispatched before storage
    // init — it reads only `.aida/complexity-calibration/`, no requirement
    // store. trace:STORY-439 | ai:claude
    if let Command::Autonomy { command } = &cli.command {
        return autonomy_cmd::handle_autonomy_command(command.as_ref());
    }

    // TASK-394: the `--no-human` acknowledgement marker just touches/removes a
    // file (no requirement store). Dispatch before storage init so it works on
    // a fresh clone too. trace:TASK-394 | ai:claude
    if let Command::NoHuman(no_human_cmd) = &cli.command {
        return handle_no_human_command(no_human_cmd);
    }

    // Determine which requirements file to use
    // trace:REQ-0231 | ai:claude:high
    let requirements_path = if let Some(ref explicit_file) = cli.file {
        let explicit_path = std::path::PathBuf::from(explicit_file);
        // If path is a directory, use GitBackend and route through the backend
        // API. Goes through `run_on_distributed_store` — not straight to
        // `handle_git_backend_command` — so an explicit `--file <dir>` gets
        // the same tracker (jira/github/gitlab) and mcp-serve special-casing
        // as every other distributed-store resolution path; the git-backend
        // handler alone has no arms for those commands and used to exit
        // "not yet supported". trace:TASK-1487 | ai:claude
        if explicit_path.is_dir()
            || (!explicit_path.exists() && explicit_path.extension().is_none())
        {
            // The explicit --file directory IS the project root here — there's
            // no separate project to walk cwd up to, and cwd may not even be
            // inside it (or may be inside some unrelated git repo). Pass it
            // through explicitly so `mcp-serve` doesn't derive the wrong
            // project root from cwd. trace:TASK-1487 | ai:claude
            return run_on_distributed_store(&cli.command, &explicit_path, Some(&explicit_path));
        }
        // User explicitly specified a file path - use it directly
        explicit_path
    } else {
        // Check for distributed mode config (.aida/config.toml with store_path).
        // The Jira / GitHub / GitLab tracker commands go through the same
        // distributed-first resolution as everything else: they used to skip
        // it, so inside a distributed project with no local db they fell to the
        // legacy resolver and wrongly reported there was no `.aida/config.toml`.
        // trace:TASK-1486 | ai:claude
        if let Some(store_path) = detect_distributed_store() {
            return run_on_distributed_store(&cli.command, &store_path, None);
        }
        // Distributed mode is declared in `.aida/config.toml` but the store
        // worktree isn't resolvable here — the hallmark of a freshly-cloned
        // AIDA project (`.aida-store/` is gitignored and only created by
        // `aida init`). We must NOT silently fall back to the legacy
        // requirements.yaml/SQLite: that shows STALE data with no signal
        // it's wrong (a first-user lands on someone's pre-migration specs
        // and never knows). trace:BUG-428 | ai:claude
        // BUG-442 / TASK-621: a project is distributed if local
        // `.aida/config.toml` declares it OR — on a FRESH CLONE with no
        // local config yet — the `aida-store` branch exists (origin or
        // local). Detecting from the git ref is what makes auto-attach fire
        // on a fresh clone AND, critically, prevents falling through to
        // `determine_requirements_path`'s global-registry fallback, which
        // would SILENTLY read an UNRELATED project's store (the worst
        // failure mode — wrong data, no warning). BUG-428's refuse-fallback
        // guard only covered the *declared* case; a fresh clone isn't
        // declared yet, which is exactly the gap this closes.
        // trace:BUG-442 trace:TASK-621 trace:BUG-428 | ai:claude
        let distributed_root = distributed_mode_declared().or_else(|| {
            let root = find_project_root().ok()?;
            if branch_exists_anywhere(&root, "aida-store") {
                return Some(root);
            }
            // BUG-433: the store is physically PRESENT here (`.aida-store/`
            // with an `objects/` dir — even via a symlink to the main store)
            // but there's no `.aida/config.toml` marker AND no detectable
            // `aida-store` branch — e.g. a session worktree forked from a
            // commit that predates the committed scaffolding, or a symlink
            // into another repo's store. Detect distributed mode from the
            // store's SHAPE so we use the real store instead of silently
            // falling through to the legacy backend (which serves
            // stale/unrelated data with no signal — the inverse of BUG-428).
            // trace:BUG-433 | ai:claude
            attached_store_present(&root).then_some(root)
        });
        if let Some(project_root) = distributed_root {
            // An already-attached fresh clone (no config, but the worktree
            // exists from a prior auto-attach) must be used directly — no
            // re-fetch, no repeated "attached…" noise on every command.
            let existing_worktree = project_root.join(".aida-store");
            let store_path_opt = if attached_store_present(&project_root) {
                Some(existing_worktree)
            } else if branch_exists_anywhere(&project_root, "aida-store") {
                // Auto-attach the store worktree from the `aida-store` branch
                // (the same fetch + worktree-add the post-clone init does).
                // Node-id / scaffolding stay with `aida init` / `aida node
                // acquire`: a node id is needed before WRITING, not reading.
                match try_attach_store_worktree(&project_root) {
                    Ok(store_path) => {
                        eprintln!(
                            "{} attached the AIDA store worktree from `aida-store`. \
                             Run `{}` to claim a node id before issuing new IDs (`aida add`).",
                            "Note:".dimmed(),
                            "aida node acquire".cyan()
                        );
                        Some(store_path)
                    }
                    // Auto-attach failed (offline, diverged/locked branch,
                    // git too old, …). Don't fall to legacy/ambient — drop to
                    // the explicit setup guidance below, naming the cause.
                    Err(e) => {
                        eprintln!(
                            "{} couldn't auto-attach the store worktree: {}",
                            "Note:".dimmed(),
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };
            if let Some(store_path) = store_path_opt {
                return run_on_distributed_store(&cli.command, &store_path, None);
            }
            let on_store_ref = branch_exists_anywhere(&project_root, "aida-store");
            let branch_hint = if on_store_ref {
                "\n  Its `aida-store` branch is available, ready to attach."
            } else {
                ""
            };
            anyhow::bail!(
                "This is a distributed AIDA project, but its store isn't set up in \
                 this working copy yet (no `.aida-store/` worktree).{branch_hint}\n\n  \
                 Run `aida init` to attach it — it creates the `.aida-store` worktree \
                 from the `aida-store` branch and rebuilds the cache.\n\n  \
                 (Refusing to fall back to a legacy or ambient requirements store: \
                 that would show stale or UNRELATED data with no indication it's wrong.)"
            );
        }

        // Auto-detect: first find the base path, then check migration status
        let initial_path = determine_requirements_path(cli.project.as_deref())?;

        // If path is already a .db file, skip migration check (no YAML to migrate from)
        if initial_path.extension().and_then(|e| e.to_str()) == Some("db") {
            initial_path
        } else {
            // Check for migration status (REQ-0231)
            // Storage class now auto-detects SQLite vs YAML by file extension
            match check_migration_status(&initial_path) {
                MigrationCheck::NoMigration(path) => path,
                MigrationCheck::MigratedToSqlite {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // YAML was officially migrated - use SQLite
                    eprintln!(
                        "{}: Using SQLite database: {}",
                        "INFO".blue(),
                        sqlite_path.display()
                    );
                    sqlite_path
                }
                MigrationCheck::PossibleStaleYaml {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // Both exist but no marker - default to SQLite as it's likely more current
                    eprintln!(
                        "{}: Both YAML and SQLite exist. Using SQLite.",
                        "INFO".blue()
                    );
                    eprintln!("Use --file requirements.yaml to use YAML instead.");
                    sqlite_path
                }
            }
        }
    };

    let storage = Storage::new(requirements_path.clone());

    match &cli.command {
        // TASK-728: `aida done` is a distributed-mode (default) verb. Legacy
        // --centralized mode keeps the explicit edit path. trace:TASK-727
        Command::Done { .. } => {
            anyhow::bail!(
                "`aida done` is available in the default (distributed) mode. In legacy \
                 --centralized mode, use `aida edit <spec> --status completed`."
            );
        }
        // STORY-741: `aida awaiting` (the unified coordination inbox) is a
        // distributed-mode surface — briefs, findings, escalations, reviewer
        // verdicts and mail are all distributed-mode concepts. trace:STORY-741
        Command::Awaiting { .. } => {
            anyhow::bail!("`aida awaiting` is available in the default (distributed) mode.");
        }
        Command::Protocol(_) => {
            anyhow::bail!("`aida protocol` is available in the default (distributed) mode.");
        }
        Command::Schedule(_) => {
            anyhow::bail!("`aida schedule` is available in the default (distributed) mode.");
        }
        // TASK-777: the fasttrack lane is a distributed-mode convention (it
        // queues + batches), so it isn't wired into the deprecated legacy
        // storage path. trace:TASK-777
        Command::Fasttrack { .. } => {
            anyhow::bail!(
                "`aida fasttrack` is available in the default (distributed) mode. In legacy \
                 --centralized mode, use `aida add \"<title>\" --status approved`."
            );
        }
        // STORY-706: `aida focus` scopes reads by walking the requirement
        // hierarchy graph — a distributed-mode (cache-backed) concept.
        Command::Focus { .. } => {
            anyhow::bail!(
                "`aida focus` is available in the default (distributed) mode. Legacy \
                 --centralized mode has no per-worktree focus context."
            );
        }
        Command::Add {
            title,
            title_positional,
            description,
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            prefix,
            parent,
            blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
            force_parent,
            interactive,
            // STORY-333: legacy centralized backend does not persist the
            // human-only marker; ignore the flags here. trace:TASK-130 | ai:claude
            human_only: _,
            no_human_only: _,
            effort: _,
            // TASK-754: --queue/--batch are git-backend-path features (they
            // reuse the backlog-groom enqueue helper); the deprecated
            // centralized backend ignores them. trace:TASK-754 | ai:claude
            // BUG-528: --for routes the queued spec; also git-backend-only.
            // trace:BUG-528 | ai:claude
            queue: _,
            batch: _,
            r#for: _,
            // FR-283: the numeric weight is git-canonical only; the deprecated
            // centralized backend ignores it. trace:FR-283 | ai:claude
            weight: _,
            // TASK-1267: execution modes are git-canonical only.
            mode: _,
            // STORY-1427: intake gates run on the git-canonical path only.
            gates: _,
        } => {
            // TASK-725: positional title (`aida add "do X"`) — --title wins.
            let title = title.clone().or_else(|| title_positional.clone());
            let title = &title;
            // trace:BUG-17 | ai:claude — resolve description from inline,
            // file, or stdin sources before dispatching.
            let resolved_description =
                resolve_description(description, description_from_file, *description_stdin)?;
            let description = &resolved_description;

            // trace:BUG-22 | ai:claude — warn if the title looks shell-mangled.
            if let Some(ref t) = title {
                if let Some(msg) = suspicious_title_signal(t) {
                    eprintln!("{} {}", "Warning:".yellow().bold(), msg);
                }
            }

            // Default to interactive mode if no specific arguments are provided
            let should_be_interactive = *interactive
                || (title.is_none()
                    && description.is_none()
                    && status.is_none()
                    && priority.is_none()
                    && r#type.is_none()
                    && owner.is_none()
                    && feature.is_none()
                    && tags.is_none()
                    && prefix.is_none()
                    && parent.is_none());

            if should_be_interactive {
                add_requirement_interactive(&storage)?;
            } else {
                add_requirement_cli(
                    &storage,
                    title,
                    description,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    prefix,
                    parent,
                    *force_parent,
                )?;
            }
        }
        Command::List {
            status,
            priority,
            r#type,
            feature,
            tags,
            json,
            ..
        } => {
            // Legacy SQLite path doesn't honor role scope (deprecated
            // backend). STORY-244 `--json` is git-backend only — the
            // legacy path stays human-only since `aida init --centralized`
            // is deprecated. trace:STORY-244 | ai:claude
            if *json {
                anyhow::bail!(
                    "`aida list --json` requires the default git-canonical \
                     backend; the deprecated --centralized SQLite mode does \
                     not emit JSON. Run `aida init` (no flags) on a new \
                     project, or upgrade an existing one."
                );
            }
            list_requirements(&storage, status, priority, r#type, feature, tags)?;
        }
        Command::Show { id, .. } => {
            // Legacy SQLite show_requirement always prints comments inline,
            // so the --comments flag is a no-op here. Git backend honors it.
            show_requirement(&storage, id)?;
        }
        Command::Approvals { .. } => {
            // trace:STORY-1173 | ai:codex
            anyhow::bail!(
                "aida approvals requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Graph {
            id: graph_id,
            blocked_by,
            blocks,
            tree,
            impact,
            follow,
            depth,
            json,
            cmd,
        } => {
            let store = storage.load_for_read()?;
            let (id, blocked_by, blocks, tree, impact) = match cmd {
                Some(GraphCommand::BlockedBy { id }) => (id.as_str(), true, false, false, false),
                Some(GraphCommand::Blocks { id }) => (id.as_str(), false, true, false, false),
                Some(GraphCommand::Tree { id }) => (id.as_str(), false, false, true, false),
                Some(GraphCommand::Impact { id }) => (id.as_str(), false, false, false, true),
                None => {
                    let id = graph_id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "aida graph needs a spec id, e.g. `aida graph tree STORY-1`"
                        )
                    })?;
                    if *blocked_by {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocked-by"),
                            "aida graph blocked-by <id>",
                        );
                    }
                    if *blocks {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocks"),
                            "aida graph blocks <id>",
                        );
                    }
                    if *tree {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--tree"),
                            "aida graph tree <id>",
                        );
                    }
                    if *impact {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--impact"),
                            "aida graph impact <id>",
                        );
                    }
                    (id, *blocked_by, *blocks, *tree, *impact)
                }
            };
            graph_cmd::handle_graph_command(
                &store, id, blocked_by, blocks, tree, impact, follow, *depth, *json,
            )?;
        }
        Command::Criteria {
            spec,
            json,
            window_days,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            // `coverage` / `gap` is the project-wide report, never a spec id.
            // trace:STORY-1487 | ai:claude
            criteria_coverage::dispatch_criteria(&project_root, &store, spec, *window_days, *json)?;
        }
        Command::Reconstitute {
            spec,
            json,
            dry_run,
            yes,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            reconstitute::handle_reconstitute_command(
                &project_root,
                &store,
                spec,
                reconstitute::ReconstituteOptions {
                    json: *json,
                    dry_run: *dry_run,
                    yes: *yes,
                },
            )?;
        }
        Command::Harvest {
            spec,
            from,
            pr,
            base,
            yes_all,
            dry_run,
            json,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            harvest::handle_harvest_command(
                &project_root,
                &store,
                spec,
                harvest::HarvestOptions {
                    from: from.clone(),
                    pr: *pr,
                    base: base.clone(),
                    yes_all: *yes_all,
                    dry_run: *dry_run,
                    json: *json,
                },
            )?;
        }
        Command::Brief {
            agent,
            spec,
            note,
            depends_on,
            as_deep_link,
            notify,
            authorized_by,
            cmd,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            brief_cmd::handle_brief_command(
                agent.as_deref(),
                spec.as_deref(),
                note.as_deref(),
                depends_on.as_deref(),
                *as_deep_link,
                *notify,
                authorized_by.as_deref(),
                cmd,
                &store,
                &project_root,
            )?;
        }
        Command::Agent(agent_cmd) => {
            handle_agent_command(agent_cmd)?;
        }
        Command::Terminal(cmd) => {
            handle_terminal_command(cmd)?;
        }
        Command::Edit {
            id,
            title,
            description,
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            add_tag,
            remove_tag,
            blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
            remove_blocked_by: _,
            // TASK-1176: the supersede lineage edge is git-canonical only, same
            // rule as blocked-by. trace:TASK-1176 | ai:claude
            superseded_by: _,
            // STORY-1434: the carve-out edge + comment are git-canonical
            // only, same rule as superseded_by. trace:STORY-1434 | ai:claude
            carve_out: _,
            carve_into: _,
            carve_reason: _,
            // STORY-476: external refs land on the git-backend path only;
            // the legacy SQLite path ignores them. trace:STORY-476 | ai:claude
            add_ref: _,
            remove_ref: _,
            interactive,
            strict: _, // legacy SQLite path ignores session leases
            force: _,  // TASK-47 guard only applies to git-canonical Edit
            // STORY-333: legacy centralized backend does not persist the
            // human-only marker; ignore the flags here.
            // trace:STORY-333 | ai:claude
            human_only: _,
            no_human_only: _,
            // TASK-1148: the narrative fields are git-canonical only; the legacy
            // centralized backend does not persist them. trace:TASK-1148 | ai:claude
            implementation_summary: _,
            risk_notes: _,
            test_coverage_notes: _,
            // STORY-776: execution_mode is git-canonical only, same rule.
            mode: _,
            // STORY-634: the multi-repo origin dimension is git-canonical
            // only, same rule.
            origin: _,
            // FR-283: the numeric weight is git-canonical only, same rule.
            weight: _,
        } => {
            // trace:BUG-1234 | ai:codex — keep legacy and git-canonical edit
            // behavior aligned for file/stdin description replacement.
            let resolved_description =
                resolve_edit_description(description, description_from_file, *description_stdin)?;
            let description = &resolved_description;
            // If any flags provided, use non-interactive mode; otherwise interactive
            // trace:TASK-351 | ai:claude — --add-tag / --remove-tag count too
            let has_flags = title.is_some()
                || description.is_some()
                || status.is_some()
                || priority.is_some()
                || r#type.is_some()
                || owner.is_some()
                || feature.is_some()
                || tags.is_some()
                || !add_tag.is_empty()
                || !remove_tag.is_empty();

            if *interactive || !has_flags {
                edit_requirement_interactive(&storage, id)?;
            } else {
                edit_requirement_cli(
                    &storage,
                    id,
                    title,
                    description,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    add_tag,
                    remove_tag,
                )?;
            }
        }
        Command::Del { id, yes } => {
            delete_requirement(&storage, id, *yes)?;
        }
        Command::Feature(feature_cmd) => {
            feature_cmd::handle_feature_command(feature_cmd, &storage)?;
        }
        Command::Db(db_cmd) => {
            db_cmd::handle_db_command(db_cmd, &requirements_path)?;
        }
        Command::Cache(_) => {
            anyhow::bail!(
                "aida cache commands are only available in git-canonical (distributed) mode. \
                 Run `aida init --distributed` first, or pass --file pointing to a git store."
            );
        }
        Command::Record(_) => {
            // STORY-582: the processing-record trail is a git-canonical field.
            anyhow::bail!(
                "aida record commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Node(_) => {
            anyhow::bail!(
                "aida node commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Mailbox(_) => {
            anyhow::bail!(
                "aida mailbox commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Supervise(_) => {
            anyhow::bail!(
                "aida supervise commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        // trace:STORY-1218 | ai:claude
        Command::Shift(_) => {
            anyhow::bail!(
                "aida shift commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Status {
            spec: _,
            idle_minutes: _,
            no_dev_context,
            short: _,
            json: _,
            queue: _,
            ci: _,
            no_ci: _,
            cleanup: _,
            activity: _,
            since: _,
            awaiting: _,
            verbose: _,
            no_hygiene: _,
            all: _,
            stale: _,
            full: _,
            no_focus: _,
        } => {
            handle_status_command(*no_dev_context, None, &storage)?;
        }
        // STORY-640: `aida team` reads the shared node roster on the orphan
        // `aida-store` branch — a distributed-mode concept. Legacy
        // --centralized stores have no roster. trace:STORY-640 | ai:claude
        Command::Team { .. } => {
            anyhow::bail!(
                "`aida team` is available in the default (distributed) mode — it lists the \
                 nodes sharing the orphan `aida-store` branch. Legacy --centralized mode has \
                 no node roster."
            );
        }
        // TASK-845: the person-alias registry lives on the orphan `aida-store`
        // branch — a distributed-mode concept. trace:TASK-845 | ai:claude
        Command::Identity { .. } => {
            anyhow::bail!(
                "`aida identity` is available in the default (distributed) mode — it manages the \
                 shared person-alias registry on the orphan `aida-store` branch. Legacy \
                 --centralized mode has no shared registry."
            );
        }
        Command::Usage {
            since,
            unused,
            errors,
            json,
            limit,
            auto_complete,
            failures,
            pattern,
            health,
            read_write,
            slowest,
            events,
            cmd,
            slower_than,
            action,
        } => {
            // TASK-1427's derived token ledger is separate from the historical
            // command-shape telemetry handled below.
            if let Some(action) = action {
                match action {
                    UsageCommand::Rebuild { project, json } => {
                        let root = project.clone().unwrap_or(find_project_root()?);
                        token_ledger::rebuild(&root, *json || output_format_is_json())?;
                        return Ok(());
                    }
                    UsageCommand::Show {
                        spec,
                        group_by,
                        cost,
                        json,
                        toon,
                    } => {
                        token_ledger::show(
                            &find_project_root()?,
                            spec,
                            group_by,
                            *cost,
                            *json || output_format_is_json(),
                            *toon,
                        )?;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            // TASK-266: only the `--auto-complete` view needs the store
            // (to resolve drafted-BUG statuses) — keep plain `aida usage`
            // store-load-free. STORY-530: the `--health` catalog also needs
            // the store for draft-inbox depth + burn-down velocity.
            let (
                unused,
                errors,
                auto_complete,
                failures,
                pattern,
                health,
                slowest,
                events,
                timeline,
            ) = normalize_usage_mode(
                unused.as_deref(),
                *errors,
                *auto_complete,
                *failures,
                *pattern,
                *health,
                *slowest,
                *events,
                action.as_ref(),
            );
            let store = if auto_complete || health {
                storage.load().ok()
            } else {
                None
            };
            usage_cmd::handle_usage_command(
                since,
                unused,
                errors,
                // trace:BUG-1289 | ai:claude
                *json || output_format_is_json(),
                *limit,
                auto_complete,
                failures,
                pattern,
                health,
                *read_write,
                slowest,
                events,
                timeline,
                cmd.as_deref(),
                *slower_than,
                store.as_ref(),
            )?;
        }
        Command::Metrics { cmd } => {
            // trace:STORY-477 | ai:claude — reporting layer over the local
            // telemetry logs; backend-agnostic.
            metrics_cmd::handle_metrics_command(cmd)?;
        }
        Command::FieldStudy { cmd } => {
            // trace:SPIKE-67 | ai:claude — observe-only rule-adherence study
            // over the git log; opt-in, local-only.
            field_study_cmd::handle_field_study_command(cmd)?;
        }
        Command::Push { .. } => {
            anyhow::bail!(
                "`aida push` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Pull { .. } => {
            anyhow::bail!(
                "`aida pull` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Digest { .. } => {
            anyhow::bail!(
                "`aida digest` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Fetch { .. } => {
            anyhow::bail!(
                "`aida fetch` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Rebase { .. } => {
            anyhow::bail!(
                "`aida rebase` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Memories(_) => unreachable!("memories is dispatched before storage init"),
        Command::Schema { .. } => unreachable!("schema is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::Release { .. } => unreachable!("release is dispatched before storage init"),
        Command::Burndown(_) => unreachable!("burndown is dispatched before storage init"),
        Command::Health { .. } => unreachable!("health is dispatched before storage init"),
        // trace:TASK-1231 | ai:claude
        Command::Autoprogress { .. } => {
            unreachable!("autoprogress is dispatched before storage init")
        }
        Command::Groom { .. } => {
            unreachable!("groom (assess/intake) is dispatched before storage init")
        }
        // trace:TASK-1147
        Command::Autopilot(_) => {
            unreachable!("autopilot is dispatched before storage init")
        }
        Command::Why { .. } => unreachable!("why is dispatched before storage init"),
        Command::Explain { .. } => unreachable!("explain is dispatched before storage init"),
        Command::Wiki(_) => unreachable!("wiki is dispatched before storage init"),
        Command::Intent { .. } => unreachable!("intent is dispatched before storage init"),
        // trace:STORY-696
        Command::Ps { .. } => unreachable!("ps is dispatched before storage init"),
        Command::MergeLock { .. } => {
            unreachable!("merge-lock is dispatched before storage init")
        }
        Command::MergeHold { .. } => {
            unreachable!("merge-hold is dispatched before storage init")
        }
        Command::Watch { .. } => unreachable!("watch is dispatched before storage init"),
        Command::Contract { .. } => unreachable!("contract is dispatched before storage init"),
        // trace:TASK-1034
        Command::Integrate { .. } => {
            unreachable!("integrate is dispatched before storage init")
        }
        // trace:TASK-957
        Command::Claim { .. } => unreachable!("claim is dispatched before storage init"),
        Command::Unclaim { .. } => unreachable!("unclaim is dispatched before storage init"),
        Command::Spec(_) => unreachable!("spec subcommands are dispatched before storage init"),
        Command::Worktree(_) => {
            unreachable!("worktree subcommands are dispatched before storage init")
        }
        Command::Doctor { .. } => unreachable!("doctor is dispatched before storage init"),
        Command::Store(_) => unreachable!("store is dispatched before storage init"),
        Command::Remote(_) => unreachable!("remote is dispatched before storage init"),
        Command::Sandbox(_) => unreachable!("sandbox is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Commands { .. } => unreachable!("commands is dispatched before storage init"),
        Command::Alias { .. } => unreachable!("alias is dispatched before storage init"),
        Command::Plan(_) => unreachable!("plan is dispatched before storage init"),
        Command::Deps(_) => unreachable!("deps is dispatched before storage init"),
        Command::Lint { .. } => unreachable!("lint is dispatched before storage init"),
        Command::Gate(_) => unreachable!("gate is dispatched before storage init"),
        Command::Lifecycle { .. } => {
            unreachable!("lifecycle is dispatched before storage init")
        }
        Command::Changelog(_) => unreachable!("changelog is dispatched before storage init"),
        Command::Manual { .. } => unreachable!("manual is dispatched before storage init"),
        Command::Derisk { .. } => unreachable!("derisk is dispatched before storage init"),
        Command::Ultraplan { .. } => unreachable!("ultraplan is dispatched before storage init"),
        Command::Compete { .. } => unreachable!("compete is dispatched before storage init"),
        Command::Goal { .. } => unreachable!("goal is dispatched before storage init"),
        Command::Commit { .. } => unreachable!("commit is dispatched before storage init"),
        Command::Internal { .. } => unreachable!("internal is dispatched before storage init"),
        Command::Tui { .. } => unreachable!("tui is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusbar { .. } => unreachable!("statusbar is dispatched before storage init"),
        // trace:TASK-1167
        Command::Tail { .. } => unreachable!("tail is dispatched before storage init"),
        Command::Statusline { .. } => unreachable!("statusline is dispatched before storage init"),
        Command::BgFetch { .. } => unreachable!("_bg-fetch is dispatched before storage init"),
        Command::Away | Command::Home | Command::Presence { .. } | Command::Solo { .. } => {
            unreachable!("presence/solo commands are dispatched before storage init")
        }
        // trace:TASK-784
        Command::Whoami => unreachable!("whoami is dispatched before storage init"),
        Command::Session(_) => unreachable!("session is dispatched before storage init"),
        Command::Triage(_) => unreachable!("triage is dispatched before storage init"),
        Command::Lock(_) => unreachable!("lock is dispatched before storage init"),
        Command::Pr(_) => unreachable!("pr is dispatched before storage init"),
        Command::Ship { .. } => unreachable!("ship is dispatched before storage init"),
        Command::Orchestrator(_) => {
            unreachable!("orchestrator is dispatched before storage init")
        }
        // STORY-721: `aida zen <spec>` autonomous implement+ship drive on the
        // legacy (centralized) storage path. The introspection subcommands
        // short-circuit before storage init; this arm only sees the spec-drive
        // form. trace:STORY-721 | ai:claude
        Command::Zen {
            spec,
            supervised,
            no_human,
            no_pull,
            force,
            solo,
            into_epic,
            dry_run,
            json,
            vendor,
            // trace:STORY-722 — the --compete form dispatches before storage init.
            compete: _,
            command: _,
        } => {
            // STORY-744: the machine-readable gate probe short-circuits the
            // drive entirely — it resolves + classifies and prints the verdict.
            if *json {
                run_zen_gate_json(&storage, spec.as_deref())?;
            } else {
                // TASK-1116: a per-invocation `--vendor`/`--agent` picks the
                // headless implementer for this drive (top precedence).
                apply_drive_vendor_override(vendor.as_deref())?;
                let user_id = current_user_id(None);
                run_zen_drive(
                    &storage,
                    None, // legacy storage path: no git backend for auto-approve
                    &user_id,
                    spec.as_deref(),
                    no_human.as_deref(),
                    *supervised,
                    *no_pull,
                    *force,
                    *solo,
                    *into_epic,
                    *dry_run,
                )?;
            }
        }
        Command::Drain(_) => unreachable!("drain is dispatched before storage init"),
        Command::Stack(_) => unreachable!("stack is dispatched before storage init"),
        Command::Worker(_) => unreachable!("worker is dispatched before storage init"),
        Command::Headless(_) => unreachable!("headless is dispatched before storage init"),
        Command::Punts(_) => unreachable!("punts is dispatched before storage init"),
        Command::Autonomy { .. } => unreachable!("autonomy is dispatched before storage init"),
        Command::NoHuman(_) => unreachable!("no-human is dispatched before storage init"),
        Command::Rel(rel_cmd) => {
            relationship_cmd::handle_relationship_command(rel_cmd, &storage)?;
        }
        Command::RelDef(rel_def_cmd) => {
            rel_def_cmd::handle_rel_def_command(rel_def_cmd, &storage)?;
        }
        Command::Comment(comment_cmd) => {
            comment_cmd::handle_comment_command(comment_cmd, &storage)?;
        }
        // STORY-633: glyph CLI surface — config.toml writer, no store needed.
        // trace:STORY-633 | ai:claude
        Command::Config(ConfigCommand::Glyph(glyph_cmd)) => {
            config_cmd::handle_config_glyph(glyph_cmd)?;
        }
        Command::Config(config_cmd) => {
            config_cmd::handle_config_command(config_cmd, &storage)?;
        }
        Command::Type(type_cmd) => {
            type_cmd::handle_type_command(type_cmd, &storage)?;
        }
        Command::Export { format, output, id } => {
            import_export_cmd::handle_export_command(
                &storage,
                format,
                output.as_deref(),
                id.as_deref(),
            )?;
        }
        Command::Import {
            file,
            parent,
            on_conflict,
        } => {
            import_export_cmd::handle_import_command(
                &storage,
                file,
                parent.as_deref(),
                on_conflict,
            )?;
        }
        Command::UserGuide { dark } => {
            open_user_guide(*dark)?;
        }
        Command::Server(server_cmd) => {
            server_cmd::handle_server_command(server_cmd, cli.server.as_deref())?;
        }
        Command::Trace(trace_cmd) => {
            crate::trace_cmd::handle_trace_command(trace_cmd, &storage)?;
        }
        Command::Review {
            spec,
            no_agent,
            allow_stale_base,
            target_branch,
            cmd,
        } => {
            // trace:STORY-553 | ai:claude — `aida review <SPEC>` drives the
            // human-decision review; `prompt` / `assemble` stay the helper
            // subcommands. The legacy (SQLite) path supports the prompt
            // helpers only; the spec-review verb needs the git-canonical
            // backend's surface resolution, so guide the user there.
            match (spec, cmd) {
                (Some(_), _) => anyhow::bail!(
                    "`aida review <SPEC>` requires the default git-canonical \
                     backend; the deprecated --centralized SQLite mode does \
                     not support it."
                ),
                (None, Some(review_cmd)) => handle_review_command(review_cmd, &storage)?,
                (None, None) => {
                    let _ = (no_agent, allow_stale_base, target_branch);
                    anyhow::bail!("pass a spec id (`aida review <SPEC>`) or a subcommand (`prompt` / `assemble`)");
                }
            }
        }
        Command::Report {
            recheck,
            command: report_cmd,
        } => {
            let db_path_str = requirements_path.display().to_string();
            report_cmd::handle_report_command(
                *recheck,
                report_cmd.as_ref(),
                &storage,
                &db_path_str,
            )?;
        }
        Command::Scaffold(scaffold_cmd) => {
            scaffold_cmd::handle_scaffold_command(scaffold_cmd, &storage, &requirements_path)?;
        }
        Command::Gitlab(gitlab_cmd) => {
            tracker_cmd::handle_gitlab_command(gitlab_cmd, &storage)?;
        }
        Command::Github(github_cmd) => {
            tracker_cmd::handle_github_command(github_cmd, &storage)?;
        }
        Command::Jira(jira_cmd) => {
            tracker_cmd::handle_jira_command(jira_cmd, &storage)?;
        }
        Command::McpServe => {
            // STORY-361: project root for coordination tools.
            let project_root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            mcp::run_mcp_server(&storage, project_root)?;
        }
        Command::Mcp(mcp_cmd) => {
            // STORY-361: management commands for the MCP coordination surface.
            handle_mcp_command(mcp_cmd)?;
        }
        Command::Docs(docs_cmd) => {
            // trace:FR-1-077 | ai:claude
            doc_cmd::handle_docs_command(docs_cmd, &storage)?;
        }
        Command::Grep {
            pattern,
            ignore_case,
            extended_regex,
            after_context,
            before_context,
            context,
            field,
            status,
            r#type,
            feature,
            files_with_matches,
            count,
            invert_match,
        } => {
            grep_requirements(
                &storage,
                pattern,
                *ignore_case,
                *extended_regex,
                *after_context,
                *before_context,
                *context,
                field.as_deref(),
                status.as_deref(),
                r#type.as_deref(),
                feature.as_deref(),
                *files_with_matches,
                *count,
                *invert_match,
            )?;
        }
        Command::Queue(queue_cmd) => {
            handle_queue_command(queue_cmd, &storage, &requirements_path)?;
        }
        Command::Do { spec, mode, force } => {
            run_do_drive(&storage, spec, mode.as_deref(), *force)?;
        }
        Command::Load(load_cmd) => {
            load_cmd::handle_load_command(load_cmd, &storage)?;
        }
        // STORY-444 + STORY-451: `aida backlog` owns grooming plus the
        // `load` alias for quantitative effort summaries.
        Command::Backlog(backlog_cmd) => match backlog_cmd {
            BacklogCommand::Load => load_cmd::handle_load_command(&LoadCommand::Backlog, &storage)?,
            _ => backlog::handle_backlog_command(backlog_cmd, &storage)?,
        },
        // TASK-218: top-level alias in the legacy SQLite dispatch path —
        // forwards to the same handler as `aida queue rework SPEC`.
        // trace:TASK-218 | ai:claude
        Command::Rework {
            id,
            work,
            r#for,
            tail,
            status,
            reason,
            resume,
            force,
            steal,
            permission_mode,
            no_pull,
            user,
        } => {
            // STORY-1429: an omitted ID opens the triage loop. trace:STORY-1429 | ai:claude
            crate::queue_cmd::handle_rework_entry(
                &storage,
                id.as_deref(),
                &crate::queue_cmd::ReworkFlags {
                    work: *work,
                    for_role: r#for.as_deref(),
                    tail: *tail,
                    status: status.as_deref(),
                    reason: reason.as_deref(),
                    resume: *resume,
                    force: *force,
                    steal: *steal,
                    permission_mode: permission_mode.as_deref(),
                    no_pull: *no_pull,
                    user: user.as_deref(),
                },
            )?;
        }
        Command::Search {
            query,
            case_sensitive,
            status,
            feature,
            ..
        } => {
            // Search is a simplified version of grep with sensible defaults:
            // - Case insensitive by default (unless -s/--case-sensitive)
            // - Searches all text fields (title, description, comments)
            // The --limit flag is honored by the git backend's FTS5 path; the
            // legacy grep walks the in-memory store and ignores it.
            grep_requirements(
                &storage,
                query,
                !case_sensitive, // invert: case_sensitive=false means ignore_case=true
                false,           // no extended regex
                0,               // no after context
                0,               // no before context
                None,            // no context
                None,            // search all fields
                status.as_deref(),
                None, // no type filter
                feature.as_deref(),
                false, // show full matches, not just IDs
                false, // don't show count
                false, // don't invert match
            )?;
        }
        Command::Init { .. } => {
            // Handled before path resolution above; unreachable
            unreachable!("Init command should be handled before path resolution");
        }
        Command::History {
            kind: Some(kind),
            since,
            until,
            author,
            limit,
            ..
        } => {
            // STORY-1436: the event feed is local runtime state, readable
            // without the orphan store. trace:STORY-1436 | ai:claude
            history_kind_report(
                kind,
                since.as_deref(),
                until.as_deref(),
                author.as_deref(),
                *limit,
            )?;
        }
        Command::History { .. } => {
            // History walks the orphan branch; only meaningful in git-canonical
            // mode, which is dispatched via handle_git_backend_command. Falling
            // through to legacy means the user is on a SQLite-only project.
            anyhow::bail!(
                "aida history requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Doc(_) => {
            // `aida doc` is git-canonical-only — Doc entries live in the
            // orphan store alongside every other requirement. Legacy SQLite
            // projects fall through to this arm. trace:STORY-104 | ai:claude
            anyhow::bail!(
                "aida doc requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Rules(_) => {
            // `aida rules` reads the cache projection — git-canonical only.
            // trace:SPIKE-31 | ai:claude
            anyhow::bail!(
                "aida rules requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Findings { .. } => {
            // `aida findings` queries the cache for `from-review:` /
            // `from-implementer:` tags; the deprecated SQLite backend has no
            // equivalent. trace:STORY-278 trace:STORY-285 | ai:claude
            anyhow::bail!(
                "aida findings requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Advisor { .. } => {
            // STORY-360: `aida advisor` (register/unregister/status) dispatches
            // before storage resolution. STORY-262's `advisor schedule`
            // dispatches in `handle_git_backend_command` (it needs the store).
            // This legacy-SQLite arm is unreachable for both. The schedule
            // subcommand and the STORY-618 bare worklist additionally require
            // the git-canonical store, so a legacy project never reaches it.
            // trace:STORY-360 trace:STORY-262 trace:STORY-618 | ai:claude
            unreachable!("Command::Advisor dispatched before legacy storage init");
        }
        Command::Questions { .. } => {
            // `aida questions` reads/writes the `decision_request` field on a
            // spec; the deprecated SQLite backend does not persist it.
            // trace:STORY-522 | ai:claude
            anyhow::bail!(
                "aida questions requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Decide { .. } => {
            // `aida decide` routes to `aida questions answer`/`clarify`, both of
            // which need the git-canonical store. trace:TASK-779 | ai:claude
            anyhow::bail!(
                "aida decide requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Research { .. } => {
            // `aida research` attaches a comment + escalates a decision_request
            // on a spike; the deprecated SQLite backend persists neither.
            // trace:STORY-568 | ai:claude
            anyhow::bail!(
                "aida research requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Punt { .. } => {
            // `aida punt` writes the NeedsAttention status + structured punt
            // metadata; the deprecated SQLite backend does not persist it.
            // trace:STORY-332 | ai:claude
            anyhow::bail!(
                "aida punt requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Archive { .. } | Command::Unarchive { .. } => {
            // STORY-441: archive uses the git-canonical write path.
            anyhow::bail!(
                "aida archive/unarchive requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Defer { .. } | Command::Undefer { .. } => {
            // STORY-584: defer uses the git-canonical write path.
            anyhow::bail!(
                "aida defer/undefer requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Assign { .. } | Command::Unassign { .. } => {
            // STORY-639: assignment uses the git-canonical write path.
            anyhow::bail!(
                "aida assign/unassign requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }

        Command::StateSnapshot { .. } => {
            // The finish-state preamble's Spec row needs git-canonical
            // metadata (spec_id + status). trace:TASK-391 | ai:claude
            anyhow::bail!(
                "aida state-snapshot requires the distributed git-canonical \
                 store (run `aida init` to migrate, or this project is on \
                 the deprecated --centralized backend)"
            );
        }
        Command::ImportPlan { .. } => {
            // The plan-review handshake tags + comments on the spec via the
            // git-canonical write path. trace:TASK-516 | ai:claude
            anyhow::bail!(
                "aida import-plan requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Human { .. } => {
            // The `aida human` bottleneck view reads the git-canonical store
            // (the burndown classifier needs the relationship graph + tags);
            // the deprecated --centralized backend can't back it. trace:TASK-746
            anyhow::bail!(
                "aida human requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Skill(_) => {
            unreachable!("Command::Skill dispatched before storage init");
        }
        Command::Notify(_) => {
            unreachable!("Command::Notify dispatched before storage init");
        }
    }

    Ok(())
}

/// Handle `aida findings {list,dismiss,promote}` — the advisor's triage surface
/// over findings the headless drain files as draft TASKs: review findings
/// (`from-review:`, STORY-278) and implementer findings (`from-implementer:`,
/// STORY-285). `list` is a read-only query; `dismiss`/`promote` are status
/// flips guarded so they only act on real findings of either source.
/// trace:STORY-278 trace:STORY-285 | ai:claude
/// STORY-306: the morning-after audit block for `aida findings list` — read
/// the punt ledger and summarise the headless advisor's decisions: how many
/// design-forks it resolved vs escalated, and the escalated rows (a human
/// still owns those). `None` when the advisor has decided nothing, so the
/// section is omitted entirely rather than printing an empty header.
/// trace:STORY-306 | ai:claude
pub(crate) fn render_advisor_decisions_footer(project_root: &std::path::Path) -> Option<String> {
    let records = punt::read_ledger(project_root);
    // Advisor decisions only — a plain implementer punt has no `answered_by`.
    let advisor: Vec<&punt::PuntRecord> = records
        .iter()
        .filter(|r| r.answered_by.as_deref() == Some("advisor"))
        .collect();
    if advisor.is_empty() {
        return None;
    }
    let resolved = advisor
        .iter()
        .filter(|r| r.resolution_path == "advisor-resolved")
        .count();
    let escalated: Vec<&punt::PuntRecord> = advisor
        .iter()
        .copied()
        .filter(|r| r.resolution_path == "escalated-to-human")
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        "Advisor decisions (recent)".magenta().bold()
    ));
    out.push_str(&format!(
        "  {resolved} resolved · {} escalated to a human\n",
        escalated.len()
    ));
    // The escalated rows — most recent first — a human still decides these.
    for r in escalated.iter().rev().take(10) {
        let why = r
            .escalation_reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("needs a human");
        out.push_str(&format!("  {:<14} {:<22} {}\n", r.spec, why, r.detail));
    }
    Some(out.trim_end().to_string())
}

/// The `aida findings list` tip that points at the interactive requeue loop.
// trace:STORY-1429 | ai:claude
pub(crate) fn findings_triage_tip(parked: usize) -> String {
    format!(
        "{parked} parked · `aida rework` to triage {} one keystroke each",
        if parked == 1 { "it" } else { "them" }
    )
}

pub(crate) fn handle_findings_command(
    cmd: &FindingsCommand,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        FindingsCommand::List {
            pr,
            source,
            kind,
            count,
            json,
        } => {
            // trace:BUG-1289 | ai:claude
            let json = *json || output_format_is_json();
            // Findings are draft requirements carrying a `from-review:` or
            // `from-implementer:` tag. `aida list --tags` is exact-match, so
            // the prefix glob can't be a list filter — query all drafts, then
            // prefix-match in build_findings_view. The draft set is small.
            // Role scope is deliberately NOT applied: the finding cohort is
            // its own axis. trace:STORY-285 | ai:claude
            let filter = aida_core::ListFilter {
                status: Some("draft".to_string()),
                ..Default::default()
            };
            let summaries = backend.list_summaries(&filter)?;
            let view_filter = findings::FindingsFilter {
                pr: *pr,
                source: *source,
                kind: kind.clone(),
            };
            let sections = findings::build_findings_view(&summaries, &view_filter);
            let findings_total = findings::count_findings(&sections);

            // STORY-332 / EPIC-28: a NeedsAttention spec is either a punt
            // (design-fork raised by an agent) or a shelving (phase failure
            // parked by the orchestrator). Triage both alongside findings;
            // sort each into its own section so the human-readable shape
            // matches the triage action. `--pr`/`--source`/`--kind` are
            // findings-specific axes — when any is set the caller asked
            // for a specific finding source, so punts and shelvings are
            // left out. trace:EPIC-28 | ai:claude
            let show_attention = pr.is_none() && source.is_none() && kind.is_none();
            let mut punts: Vec<aida_core::Requirement> = Vec::new();
            let mut shelved: Vec<aida_core::Requirement> = Vec::new();
            if show_attention {
                let na_filter = aida_core::ListFilter {
                    status: Some("needs-attention".to_string()),
                    ..Default::default()
                };
                for s in backend.list_summaries(&na_filter)? {
                    if let Some(did) = s.agreed_id.as_deref().or(s.spec_id.as_deref()) {
                        if let Some(r) = backend.get_requirement_by_spec_id(did)? {
                            // EPIC-28: if both reasons are populated (a
                            // re-shelving of a previously-punted spec), the
                            // shelve wins — the failure is the most recent
                            // reason a human needs to act on.
                            if r.failure_reason.is_some() {
                                shelved.push(r);
                            } else {
                                punts.push(r);
                            }
                        }
                    }
                }
                punts.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
                shelved.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
            }
            let total = findings_total + punts.len() + shelved.len();

            // trace:BUG-1289 | ai:claude
            // Matches `docs/monitor-contract-fixtures/findings-list.json`
            // (`{findings: array}`) plus the punt/shelve axes the human view
            // triages alongside findings.
            if json {
                let findings_json: Vec<serde_json::Value> = sections
                    .iter()
                    .flat_map(|section| {
                        let source_label = section.source.label();
                        section.groups.iter().flat_map(move |group| {
                            let origin = group.origin.clone();
                            group.rows.iter().map(move |row| {
                                serde_json::json!({
                                    "id": row.display_id,
                                    "title": row.title,
                                    "severity": row.severity.label(),
                                    "kind": row.kind,
                                    "recurrence": row.recurrence,
                                    "source": source_label,
                                    "origin": origin,
                                })
                            })
                        })
                    })
                    .collect();
                let punts_json: Vec<serde_json::Value> = punts
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.display_id(),
                            "title": r.title,
                            "category": r.attention_reason.as_ref().map(|a| a.category.to_string()),
                            "detail": r.attention_reason.as_ref().map(|a| a.detail.clone()),
                            // trace:TASK-1311 | ai:claude
                            "requeue": requeue::requeue_command(&r.display_id()),
                        })
                    })
                    .collect();
                let shelved_json: Vec<serde_json::Value> = shelved
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.display_id(),
                            "title": r.title,
                            "phase": r.failure_reason.as_ref().map(|f| f.phase.clone()),
                            "kind": r.failure_reason.as_ref().map(|f| f.kind.clone()),
                            // trace:TASK-1311 | ai:claude
                            "requeue": requeue::requeue_command(&r.display_id()),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    crate::cache_output::json_pretty(&serde_json::json!({
                        "findings": findings_json,
                        "findings_total": findings_total,
                        "punts": punts_json,
                        "shelved": shelved_json,
                        "total": total,
                    }))?
                );
                return Ok(());
            }

            if *count {
                println!("{total}");
                return Ok(());
            }
            // STORY-306: the overnight-advisor audit — what the headless
            // advisor tier resolved vs escalated. Shown even when nothing
            // awaits triage: the advisor may have resolved every fork.
            let advisor_footer = store_path
                .parent()
                .and_then(render_advisor_decisions_footer);
            if total == 0 {
                println!("{}", "No findings awaiting triage.".dimmed());
                if let Some(footer) = &advisor_footer {
                    println!();
                    println!("{footer}");
                }
                return Ok(());
            }

            // STORY-1429: one store load + queue handle for the requeue
            // previews under the parked rows (only when there are parks).
            // trace:STORY-1429 | ai:claude
            let preview_store = if punts.is_empty() && shelved.is_empty() {
                None
            } else {
                backend.load().ok()
            };
            let route_storage = Storage::new(store_path);
            println!("{}", format!("Findings awaiting triage ({total})").bold());
            for section in &sections {
                println!();
                println!(
                    "{}",
                    format!("From {}", section.source.label()).magenta().bold()
                );
                for group in &section.groups {
                    // BUG-641: the origin is the spec/PR the findings are
                    // ABOUT — render it `about <spec>` so it can't be misread
                    // as a finding's own id (each row below leads with
                    // `finding <id>`). trace:BUG-641
                    println!(
                        "  {}",
                        findings::render_origin_header(&group.origin).cyan().bold()
                    );
                    for row in &group.rows {
                        println!("    {}", findings::render_finding_row(row));
                    }
                }
            }
            // STORY-332: punts awaiting triage — paused specs an autonomous
            // agent could not safely resolve.
            if !punts.is_empty() {
                println!();
                println!("{}", "Punts awaiting triage".magenta().bold());
                for r in &punts {
                    let did = r
                        .agreed_id
                        .as_deref()
                        .or(r.spec_id.as_deref())
                        .unwrap_or("?");
                    match &r.attention_reason {
                        Some(a) => {
                            println!("  {:<20} {:<14} {}", a.category.to_string(), did, a.detail);
                            if let Some(lean) = &a.lean {
                                println!(
                                    "  {:<20} {:<14} {}",
                                    "",
                                    "",
                                    format!(
                                        "{} lean: {lean}",
                                        crate::glyph(crate::glyphs::Glyph::SubArrow)
                                    )
                                    .dimmed()
                                );
                            }
                        }
                        // A NeedsAttention spec with no recorded reason —
                        // status set by hand rather than via `aida punt`.
                        None => println!("  {:<20} {:<14} {}", "(no reason)", did, r.title),
                    }
                    print_requeue_hint_row(r, did, preview_store.as_ref(), &route_storage);
                }
            }
            // EPIC-28: failures the orchestrator shelved — phase failures the
            // batch drain parked rather than halting the whole batch. Shown
            // in a sibling section so the triage action (look at the recovery
            // hint, fix the failure, re-queue) is distinct from the punt
            // triage (decide the design-fork). trace:EPIC-28 | ai:claude
            if !shelved.is_empty() {
                println!();
                println!("{}", "Failures awaiting triage".magenta().bold());
                for r in &shelved {
                    let did = r
                        .agreed_id
                        .as_deref()
                        .or(r.spec_id.as_deref())
                        .unwrap_or("?");
                    match &r.failure_reason {
                        Some(fr) => {
                            let cause =
                                auto_complete_telemetry::failure_cause_label(Some(&fr.kind));
                            let detail = auto_complete_telemetry::failure_detail_first_line(Some(
                                &fr.detail,
                            ));
                            println!("  {:<20} {:<14} {}", cause, did, detail);
                            if let Some(hint) = &fr.recovery_hint {
                                println!(
                                    "  {:<20} {:<14} {}",
                                    "",
                                    "",
                                    format!(
                                        "{} hint: {hint}",
                                        crate::glyph(crate::glyphs::Glyph::SubArrow)
                                    )
                                    .dimmed()
                                );
                            }
                        }
                        None => println!("  {:<20} {:<14} {}", "(no reason)", did, r.title),
                    }
                    print_requeue_hint_row(r, did, preview_store.as_ref(), &route_storage);
                }
            }

            println!();
            if findings_total > 0 {
                println!(
                    "{}",
                    "Triage: `aida findings promote <ID>` joins the queue · \
                     `aida findings dismiss <ID>` rejects it"
                        .dimmed()
                );
            }
            if !punts.is_empty() {
                println!(
                    "{}",
                    "Punts: `aida show <ID>` for the fork · decide it, then requeue with \
                     `aida rework <ID>` (to Approved, back on the queue) · drop with \
                     `aida edit <ID> --status rejected`"
                        .dimmed()
                );
            }
            // EPIC-28 trace:EPIC-28 | ai:claude
            if !shelved.is_empty() {
                println!(
                    "{}",
                    "Failures: read the recovery hint · fix the underlying issue · \
                     requeue with `aida rework <ID>` (to Approved, back on the queue) · \
                     drop with `aida edit <ID> --status rejected`"
                        .dimmed()
                );
            }
            // STORY-1429: the one-line tip into the interactive loop. The
            // listing itself never prompts. trace:STORY-1429 | ai:claude
            let parked = punts.len() + shelved.len();
            if parked > 0 {
                println!("{}", findings_triage_tip(parked).dimmed());
            }
            // STORY-306: the overnight-advisor audit footer.
            if let Some(footer) = &advisor_footer {
                println!();
                println!("{footer}");
            }
        }

        FindingsCommand::Dismiss { id, reason } => {
            // BUG-1647: one per-spec atomic write; an already-Rejected
            // finding is a reported no-op. trace:BUG-1647 | ai:claude
            match findings_dismiss(
                backend,
                store_path,
                id,
                reason.as_deref(),
                chrono::Utc::now(),
            )? {
                DismissOutcome::Dismissed => {
                    println!("Dismissed finding {id} — status → Rejected.")
                }
                DismissOutcome::AlreadyDismissed => {
                    println!("Finding {id} is already dismissed (Rejected); nothing was changed.")
                }
            }
        }

        FindingsCommand::Promote {
            id,
            r#for,
            reason,
            auto_complete,
            force,
            to,
            detectable,
        } => {
            let req = backend
                .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
            let tags: Vec<String> = req.tags.iter().cloned().collect();
            if !findings::is_finding(&tags) {
                anyhow::bail!(
                    "{id} is not a finding (no `from-review:`/`from-implementer:`/`from-advisor:` tag) — \
                     `aida findings` only triages real findings. \
                     Use `aida edit {id} --status approved` for a general status change."
                );
            }

            // STORY-1428: the gate-candidate destination. `--to work` (the
            // default) keeps the original path below unchanged.
            // trace:STORY-1428 | ai:claude
            match to.trim().to_ascii_lowercase().as_str() {
                "work" => {}
                "gate" => {
                    return handle_findings_promote_gate(
                        backend,
                        store_path,
                        req,
                        id,
                        detectable.as_deref(),
                        reason.as_deref(),
                        r#for.as_deref(),
                        *force,
                    );
                }
                other => anyhow::bail!(
                    "unknown promote destination `{other}` — expected `work` or `gate`"
                ),
            }
            if detectable.is_some() {
                anyhow::bail!("--detectable only applies to `--to gate`");
            }
            let gate_threshold = findings::promote_threshold_for_project(store_path.parent());
            let gate_hint = if findings::offers_gate_route(&tags, gate_threshold) {
                findings::recurrence_gate_hint(
                    &tags,
                    gate_threshold,
                    req.spec_id.as_deref().unwrap_or(id.as_str()),
                )
            } else {
                None
            };

            // TASK-579: a finding's underlying fix may have already merged
            // referencing the id the finding carried *before* it became real
            // work (its origin-ID — `spec_id` here, distinct from a later
            // `agreed_id`). Promoting it as fresh work then strands it In
            // Progress / Approved forever, because the auto-bump scan finds no
            // commit referencing the *new* id. Scan the default branch for a
            // merged commit referencing any of the finding's ids; warn, or
            // (`--auto-complete`) bump straight to Completed instead of
            // queueing a no-op. `--force` skips the check entirely (reopen /
            // extend). trace:TASK-579 | ai:claude
            let project_root = store_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("cannot resolve project root from store path"))?;
            let mut candidate_ids: Vec<String> = Vec::new();
            if let Some(s) = req.spec_id.as_deref() {
                candidate_ids.push(s.to_string());
            }
            if let Some(a) = req.agreed_id.as_deref() {
                if !candidate_ids.iter().any(|c| c.eq_ignore_ascii_case(a)) {
                    candidate_ids.push(a.to_string());
                }
            }
            let already_merged = if *force {
                None
            } else {
                find_merged_commit_referencing_ids(project_root, &candidate_ids)
            };

            if let Some((sha, subject)) = &already_merged {
                if *auto_complete {
                    let display_id = req.display_id();
                    // BUG-1638: one per-spec atomic write, so an edit made
                    // since the finding was read is kept. trace:BUG-1638 | ai:claude
                    let outcome = findings_promote_auto_complete_write(
                        backend,
                        &req,
                        project_root,
                        sha,
                        subject,
                        reason.as_deref(),
                        chrono::Utc::now(),
                    )?;
                    // BUG-1647: a re-promote of a Completed finding changes
                    // nothing and says so. trace:BUG-1647 | ai:claude
                    if outcome == PromoteAutoComplete::AlreadyCompleted {
                        println!(
                            "Finding {id} is already Completed — origin-ID fix already merged \
                             ({sha}); nothing was changed."
                        );
                        return Ok(());
                    }
                    record_role_activity(&display_id, "auto-complete");
                    println!(
                        "Promoted finding {id} — origin-ID fix already merged ({sha}), \
                         status → Completed (no queue). Re-run with --force to queue anyway."
                    );
                    return Ok(());
                }
                eprintln!(
                    "Warning: {id}'s origin-ID fix appears already merged on the default \
                     branch ({sha} \"{subject}\"). Promoting it as fresh work may strand \
                     it open — the Done→Completed auto-bump can't fire with no commit \
                     referencing the promoted id. Re-run with --auto-complete to bump it \
                     straight to Completed, or --force to queue it anyway."
                );
            }

            // Add the finding to a work queue BEFORE flipping its status.
            // The old path printed "joins the work queue" but never called
            // `queue add` — a finding ended up Approved and in no queue, a
            // silent half-success. Queueing first means a queue failure
            // exits non-zero with the finding still draft (cleanly retryable),
            // and the success message names the role it routed to.
            // trace:BUG-231 | ai:claude
            // BUG-1638: one per-spec atomic write. BUG-1647: a status write
            // that does not land withdraws the queue entry it follows.
            // trace:BUG-1638 trace:BUG-1647 | ai:claude
            let display_id = req.spec_id.as_deref().unwrap_or(id.as_str());
            let role = findings_promote_to_work(
                backend,
                store_path,
                &req,
                display_id,
                r#for.as_deref(),
                reason.as_deref(),
                chrono::Utc::now(),
            )?;
            record_role_activity(display_id, "queue-add");
            println!("Promoted finding {id} — status → Approved, queued for {role}.");
            // STORY-1428: at/above the threshold, promoting to work is not
            // silent about the other destination. trace:STORY-1428 | ai:claude
            if let Some(hint) = gate_hint {
                println!("  {}", hint.dimmed());
            }
        }

        FindingsCommand::Calibration {
            action,
            since,
            agreement,
            disagreement,
            all,
            stats,
            last,
            json,
        } => {
            // The project root is `store_path.parent()` — `aida findings` is
            // already passing the same parent through to the advisor footer.
            let project_root = store_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("cannot resolve project root from store path"))?;
            match action {
                Some(cli::CalibrationAction::Annotate { punt_id, note }) => {
                    calibration::annotate_calibration(project_root, punt_id, note)?;
                    println!("Annotated calibration record {punt_id}.");
                    return Ok(());
                }
                None => {}
            }
            let records = calibration::read_all_calibrations(project_root);
            let bucket = if *all {
                None
            } else if *agreement {
                Some(calibration::AgreementBucket::Agreement)
            } else if *disagreement {
                Some(calibration::AgreementBucket::Disagreement)
            } else {
                // Default = disagreements (the triage signal). `--all` widens
                // the view; `--agreement` swaps to the agreement bucket.
                Some(calibration::AgreementBucket::Disagreement)
            };
            let since_dur = match since.as_deref() {
                Some(s) => Some(calibration::parse_since(s).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            let filtered = calibration::filter(
                &records,
                &calibration::CalibrationFilter {
                    since: since_dur,
                    bucket,
                },
            );

            if *stats {
                let now = chrono::Utc::now();
                let s = calibration::compute_stats(&records, *last, now);
                if *json {
                    let value = serde_json::json!({
                        "considered": s.considered,
                        "paired": s.paired,
                        "agreed": s.agreed,
                        "disagreed": s.disagreed,
                        "no_fork": s.no_fork,
                        "agreement_rate": if s.paired == 0 {
                            None
                        } else {
                            Some(s.agreed as f64 / s.paired as f64)
                        },
                        "weekly": s.weekly.iter().map(|w| serde_json::json!({
                            "week_start": w.week_start.to_rfc3339(),
                            "paired": w.paired,
                            "agreed": w.agreed,
                        })).collect::<Vec<_>>(),
                        "categories": {
                            "gap": s.categories.gap,
                            "in_flight": s.categories.in_flight,
                            "cold_boot_correct": s.categories.cold_boot_correct,
                            "unannotated": s.categories.unannotated,
                        },
                    });
                    println!("{}", crate::cache_output::json_pretty(&value)?);
                } else {
                    println!("{}", "Calibration stats".bold());
                    println!("  considered:    {}", s.considered);
                    println!("  paired:        {}", s.paired);
                    if s.paired > 0 {
                        let rate = s.agreed as f64 / s.paired as f64 * 100.0;
                        println!(
                            "  agreement:     {} / {} ({:.1}%)",
                            s.agreed, s.paired, rate
                        );
                    }
                    println!("  disagreed:     {}", s.disagreed);
                    println!("  no-fork:       {}", s.no_fork);
                    println!();
                    println!("{}", "4-week trend (most recent first)".dimmed());
                    for w in &s.weekly {
                        let rate = if w.paired > 0 {
                            format!("{:.0}%", w.agreed as f64 / w.paired as f64 * 100.0)
                        } else {
                            "—".to_string()
                        };
                        println!(
                            "  week of {}  paired {:>3}, agreed {:>3} ({})",
                            w.week_start.format("%Y-%m-%d"),
                            w.paired,
                            w.agreed,
                            rate
                        );
                    }
                    println!();
                    println!("{}", "Disagreement categories".dimmed());
                    println!("  gap:                {}", s.categories.gap);
                    println!("  in-flight:          {}", s.categories.in_flight);
                    println!("  cold-boot correct:  {}", s.categories.cold_boot_correct);
                    println!("  unannotated:        {}", s.categories.unannotated);
                }
                return Ok(());
            }

            if *json {
                println!("{}", crate::cache_output::json_pretty(&filtered)?);
                return Ok(());
            }

            if filtered.is_empty() {
                let label = match bucket {
                    Some(calibration::AgreementBucket::Agreement) => "agreement",
                    Some(calibration::AgreementBucket::Disagreement) => "disagreement",
                    Some(calibration::AgreementBucket::NoFork) => "no-fork",
                    None => "calibration",
                };
                println!("{}", format!("No {label} records.").dimmed());
                return Ok(());
            }

            println!(
                "{}",
                format!("Calibration records ({})", filtered.len()).bold()
            );
            for r in &filtered {
                let agreement_label = match r.agreement() {
                    Some(true) => "AGREE".green().to_string(),
                    Some(false) => "DISAGREE".yellow().bold().to_string(),
                    None => "no-fork".dimmed().to_string(),
                };
                println!();
                println!(
                    "  {}  {}  {}",
                    r.punt_id.bold(),
                    r.timestamp.format("%Y-%m-%d %H:%M").to_string().dimmed(),
                    agreement_label,
                );
                let cold_answer = r
                    .cold_boot
                    .answer
                    .as_deref()
                    .unwrap_or(&r.cold_boot.reasoning);
                println!(
                    "    cold-boot ({}): {}",
                    r.cold_boot.resolution.cyan(),
                    truncate(cold_answer, 100)
                );
                match &r.fork {
                    Some(f) => {
                        let fork_answer = f.answer.as_deref().unwrap_or(&f.reasoning);
                        println!(
                            "    fork      ({}): {}",
                            f.resolution.cyan(),
                            truncate(fork_answer, 100)
                        );
                    }
                    None => {
                        let why = r.fork_skip_reason.as_deref().unwrap_or("not run");
                        println!("    fork      (skipped): {}", why);
                    }
                }
                if let Some(note) = &r.annotation {
                    println!("    annotation: {}", note.dimmed());
                }
            }
            println!();
            println!(
                "{}",
                "Annotate: `aida findings calibration annotate <punt-id> \"<note>\"`".dimmed()
            );
        }

        FindingsCommand::Add {
            note,
            kind,
            title,
            severity,
            linked_specs,
            tags,
        } => handle_findings_add(
            backend,
            store_path,
            note,
            kind,
            title.as_deref(),
            severity.as_deref(),
            linked_specs,
            tags.as_deref(),
        )?,

        FindingsCommand::Recur { id, note } => {
            handle_findings_recur(backend, store_path, id, note.as_deref())?
        }
        // trace:STORY-1417 | ai:claude
        FindingsCommand::Classes { since, json } => handle_review_classes(since.as_deref(), *json)?,
    }
    Ok(())
}

// ===================================================================
// `aida questions` — the async decision inbox (STORY-522, slice 1).
//
// The advisor distills a fork it can't resolve into a structured
// DecisionRequest on the spec; the human batch-answers it OUTSIDE any
// agent (plain CLI, no LLM session). Slice 1 RECORDS the answer (a pure
// data op); the loop-resume auto-applier that applies the chosen
// resolution token is DEFERRED per the operator decision.
// trace:STORY-522 | ai:claude
// ===================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSweepScope {
    Backlog,
    Approved,
    Planned,
    InProgress,
    All,
}

impl QuestionSweepScope {
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self> {
        match raw.unwrap_or("backlog").trim().to_ascii_lowercase().as_str() {
            "" | "backlog" | "near-term" | "near_term" | "workable" => Ok(Self::Backlog),
            "approved" => Ok(Self::Approved),
            "planned" => Ok(Self::Planned),
            "in-progress" | "in_progress" | "inprogress" => Ok(Self::InProgress),
            "all" => Ok(Self::All),
            other => anyhow::bail!(
                "unknown questions sweep scope `{other}` — expected backlog, approved, planned, in-progress, or all"
            ),
        }
    }

    pub(crate) fn includes_status(self, status: &RequirementStatus) -> bool {
        match self {
            Self::Backlog => matches!(
                status,
                RequirementStatus::Approved
                    | RequirementStatus::Planned
                    | RequirementStatus::InProgress
            ),
            Self::Approved => matches!(status, RequirementStatus::Approved),
            Self::Planned => matches!(status, RequirementStatus::Planned),
            Self::InProgress => matches!(status, RequirementStatus::InProgress),
            // BUG-596: `all` includes DRAFTS. The advisor's distill section
            // points at under-specified drafts (the groom worklist), but the
            // old `all` scope excluded Draft by status, so `aida questions
            // clarify` (which defaults to the swept `all` set) could never
            // reach the very specs the hint flagged — the hint and the sweep
            // contradicted each other. Drafts are exactly where acceptance
            // criteria are missing, so the broadest scope must reach them.
            // (`backlog`/`approved`/etc. still exclude drafts by design.)
            // trace:BUG-596 | ai:claude
            Self::All => matches!(
                status,
                RequirementStatus::Draft
                    | RequirementStatus::Approved
                    | RequirementStatus::Planned
                    | RequirementStatus::InProgress
                    | RequirementStatus::NeedsAttention
            ),
        }
    }
}

/// What KIND of decision a swept candidate needs — which shape of
/// DecisionRequest [`formulate_sweep_decision_request`] should attach.
/// trace:STORY-555 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SweepKind {
    /// Under-specified / design-fork text: the proceed-as-written vs
    /// park-for-clarification question (the original slice-1 sweep shape).
    Clarify,
    /// A spec held ONLY by a disposition parking tag, with no explicit
    /// question — synthesize an approve / reject / keep-parked disposition so
    /// the tag-park inbox converges on the decision inbox. Carries the gating
    /// tag that triggered it. trace:STORY-555 (R2)
    Disposition(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionSweepCandidate {
    pub(crate) reason: String,
    pub(crate) kind: SweepKind,
}

pub(crate) fn requirement_text(req: &Requirement) -> String {
    let mut text = format!("{}\n{}", req.title, req.description);
    for comment in &req.comments {
        text.push('\n');
        text.push_str(&comment.content);
    }
    text
}

pub(crate) fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub(crate) fn is_advisor_resolvable(req: &Requirement) -> bool {
    if req.tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "advisor-resolvable" | "recorded-principle" | "recorded-preference"
        )
    }) {
        return true;
    }
    let text = requirement_text(req).to_ascii_lowercase();
    contains_any(
        &text,
        &[
            "advisor-resolvable",
            "recorded principle",
            "recorded preference",
            "advisor tier handles",
            "advisor tier can",
        ],
    )
}

pub(crate) fn has_open_decision_request(req: &Requirement) -> bool {
    req.decision_request
        .as_ref()
        .is_some_and(aida_core::DecisionRequest::is_pending)
}

pub(crate) fn is_in_questions_sweep_scope(req: &Requirement, scope: QuestionSweepScope) -> bool {
    !req.archived
        && !matches!(req.priority, RequirementPriority::Low)
        && !matches!(
            req.req_type,
            RequirementType::Sprint | RequirementType::Folder | RequirementType::Meta
        )
        && scope.includes_status(&req.status)
}

/// BUG-495: types an autonomous agent never "implements" — strategic
/// (`vision`), organizational (`folder`, `meta`), and knowledge-graph
/// (`principle`, `term`) entries. Flagging them for "missing acceptance
/// criteria" is noise, not signal: they will never be implementation
/// candidates, so the gap is moot.
// trace:BUG-495
pub(crate) fn is_implementable_type(req_type: &RequirementType) -> bool {
    !matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Folder
            | RequirementType::Meta
            | RequirementType::Principle
            | RequirementType::Term
    )
}

/// BUG-495: a spec whose work is already built or in-flight — at/past Done,
/// tagged `review:draft-only` (work done, awaiting human review), or holding
/// an active lease — has a moot acceptance gap. Excluded from the
/// missing-acceptance flag so the sweep fires only on genuinely-implementable,
/// not-yet-built specs. `in_flight_scopes` is the lowercased lease-scope set
/// from [`in_flight_lease_scopes`].
// trace:BUG-495
pub(crate) fn is_built_or_held(req: &Requirement, in_flight_scopes: &HashSet<String>) -> bool {
    let at_or_past_done = matches!(
        req.status,
        RequirementStatus::Done | RequirementStatus::Completed
    );
    let draft_only = req.tags.iter().any(|t| {
        t.trim()
            .eq_ignore_ascii_case(crate::pr_ship::DRAFT_ONLY_TAG)
    });
    let leased = req
        .spec_id
        .as_ref()
        .is_some_and(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()));
    at_or_past_done || draft_only || leased
}

pub(crate) fn question_sweep_candidate(
    req: &Requirement,
    all: &[Requirement],
    scope: QuestionSweepScope,
    in_flight_scopes: &HashSet<String>,
) -> Option<QuestionSweepCandidate> {
    if !is_in_questions_sweep_scope(req, scope) || has_open_decision_request(req) {
        return None;
    }
    if is_advisor_resolvable(req) {
        return None;
    }

    // STORY-555 (R2): a spec held only by a disposition parking tag, with no
    // explicit question, needs a human DISPOSITION (approve / reject / keep) —
    // not an authored answer. Detect it first (the most explicit park signal)
    // so the tag-park and decision-park inboxes converge on `aida questions`.
    // trace:STORY-555 | ai:claude
    if let Some(tag) = disposition_gating_tag(&req.tags) {
        return Some(QuestionSweepCandidate {
            reason: format!("parked by `{tag}`"),
            kind: SweepKind::Disposition(tag),
        });
    }

    let text = requirement_text(req).to_ascii_lowercase();
    let has_acceptance = contains_any(&text, &["acceptance", "acceptance criteria", "acceptance:"]);
    // BUG-495: the missing-acceptance flag only makes sense for specs an agent
    // would actually implement and that aren't already built/in-flight.
    // Strategic/organizational/knowledge types and built-or-held specs have a
    // moot acceptance gap — excluding them strips false positives. trace:BUG-495
    // BUG-596: a SPIKE legitimately has no `## Acceptance` section — its
    // deliverable is an analysis + a decision under a timebox, not a mergeable
    // PR with pass/fail criteria. Flagging spikes for "missing acceptance" is
    // noise (BUG-495 cleaned up non-implementable handling but spikes escaped),
    // so exclude the Spike type from THIS check. trace:BUG-596 | ai:claude
    if !has_acceptance
        && is_implementable_type(&req.req_type)
        && !matches!(req.req_type, RequirementType::Spike)
        && !is_built_or_held(req, in_flight_scopes)
    {
        return Some(QuestionSweepCandidate {
            reason: "missing acceptance criteria".to_string(),
            kind: SweepKind::Clarify,
        });
    }

    if contains_any(
        &text,
        &[
            "design fork",
            "design-fork",
            "open question",
            "needs human",
            "human decision",
            "operator decision needed",
            "should this even be built",
            "should we build",
            "choose between",
            "ambiguous",
            "unclear",
            "contradictory",
            "contradiction",
            "disagree",
            "tbd",
        ],
    ) {
        return Some(QuestionSweepCandidate {
            reason: "decision-marker text".to_string(),
            kind: SweepKind::Clarify,
        });
    }

    for rel in req
        .relationships
        .iter()
        .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
    {
        if let Some(blocker) = all.iter().find(|candidate| candidate.id == rel.target_id) {
            let blocker_text = requirement_text(blocker).to_ascii_lowercase();
            if matches!(blocker.status, RequirementStatus::Rejected)
                || contains_any(
                    &blocker_text,
                    &["ambiguous", "unclear", "design fork", "design-fork"],
                )
            {
                return Some(QuestionSweepCandidate {
                    reason: format!("blocked by ambiguous {}", blocker.display_id()),
                    kind: SweepKind::Clarify,
                });
            }
        }
    }

    None
}

pub(crate) fn formulate_sweep_decision_request(
    req: &Requirement,
    candidate: &QuestionSweepCandidate,
) -> aida_core::DecisionRequest {
    let now = chrono::Utc::now();
    match &candidate.kind {
        // STORY-555 (R2): a tag-parked spec with no explicit question. Offer a
        // DISPOSITION — answering `Approve` clears the gating tag and unparks +
        // queues; `Reject` kills it; `Keep parked` records why and holds. No
        // recommended default: a disposition must be an explicit human call,
        // not an Enter-through. trace:STORY-555 | ai:claude
        SweepKind::Disposition(tag) => aida_core::DecisionRequest {
            question: format!(
                "{} is parked by `{tag}` with no recorded question — what's the disposition?",
                req.display_id(),
            ),
            choices: vec![
                aida_core::DecisionChoice {
                    label: "Approve → ready".to_string(),
                    consequence: format!(
                        "Clears the `{tag}` gate and queues the spec for the burndown ready set."
                    ),
                    resolution: "disposition:approve-to-ready".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Reject".to_string(),
                    consequence: "Marks the spec Rejected — it will not be implemented.".to_string(),
                    resolution: "disposition:reject".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Keep parked".to_string(),
                    consequence: "Leaves the spec parked and records a why-open note.".to_string(),
                    resolution: "disposition:keep-parked".to_string(),
                },
            ],
            recommended: None,
            rationale: Some(format!(
                "Held by the `{tag}` parking tag with no question attached; a human disposition unparks it (or confirms the hold).",
            )),
            answered: None,
            note: None,
            asked_at: Some(now),
            answered_at: None,
        },
        SweepKind::Clarify => aida_core::DecisionRequest {
            question: format!(
                "{} appears to need a human decision before implementation ({}) - should it proceed as written or be parked for clarification?",
                req.display_id(),
                candidate.reason
            ),
            choices: vec![
                aida_core::DecisionChoice {
                    label: "Proceed as written".to_string(),
                    consequence: "The spec stays in the near-term queue and implementers use the current text.".to_string(),
                    resolution: "noop".to_string(),
                },
                aida_core::DecisionChoice {
                    label: "Park for clarification".to_string(),
                    consequence: "The spec is deferred until the ambiguity is clarified outside the drain.".to_string(),
                    // A recognized parking tag (matches burndown::parking_tag),
                    // so applying this answer actually keeps the spec parked.
                    // trace:STORY-555 | ai:claude
                    resolution: "tag:+needs-decision".to_string(),
                },
            ],
            recommended: Some(1),
            rationale: Some(format!(
                "The deterministic sweep flagged this as `{}`; parking avoids a mid-drain implementation guess.",
                candidate.reason
            )),
            answered: None,
            note: None,
            asked_at: Some(now),
            answered_at: None,
        },
    }
}

/// Parse a `--choice` string of the form `label|consequence|resolution`
/// into a [`DecisionChoice`]. Each of the three fields is trimmed; all
/// three are required and non-empty. The `resolution` is a deterministic
/// action token (e.g. `status:rejected`, `tag:+deferred`, `noop`) — its
/// shape isn't validated here (slice 1 only records it), but it must be
/// present so the eventual auto-applier has something to act on.
// trace:STORY-522 | ai:claude
pub(crate) fn parse_decision_choice(spec: &str) -> Result<aida_core::DecisionChoice> {
    let parts: Vec<&str> = spec.splitn(3, '|').collect();
    if parts.len() != 3 {
        anyhow::bail!(
            "invalid --choice {spec:?}: expected `label|consequence|resolution` \
             (three fields separated by `|`)"
        );
    }
    let label = parts[0].trim();
    let consequence = parts[1].trim();
    let resolution = parts[2].trim();
    if label.is_empty() || consequence.is_empty() || resolution.is_empty() {
        anyhow::bail!(
            "invalid --choice {spec:?}: label, consequence, and resolution \
             must all be non-empty"
        );
    }
    Ok(aida_core::DecisionChoice {
        label: label.to_string(),
        consequence: consequence.to_string(),
        resolution: resolution.to_string(),
    })
}

/// Resolve a user-typed choice token (`"2"`, `"default"`, `"recommended"`)
/// to a 0-based choice index, validated against `request`.
// trace:STORY-522 | ai:claude
pub(crate) fn resolve_choice_index(
    request: &aida_core::DecisionRequest,
    choice: &str,
) -> Result<usize> {
    let trimmed = choice.trim();
    if trimmed.eq_ignore_ascii_case("default") || trimmed.eq_ignore_ascii_case("recommended") {
        return request.recommended.ok_or_else(|| {
            anyhow::anyhow!(
                "no recommended default on this question — pick a choice number (1-{})",
                request.choices.len()
            )
        });
    }
    let one_based: usize = trimmed.parse().map_err(|_| {
        anyhow::anyhow!(
            "invalid choice {choice:?}: expected a number 1-{}, `default`, or `recommended`",
            request.choices.len()
        )
    })?;
    if one_based == 0 || one_based > request.choices.len() {
        anyhow::bail!(
            "choice {one_based} out of range — this question has {} choices (1-{})",
            request.choices.len(),
            request.choices.len()
        );
    }
    Ok(one_based - 1)
}

/// Record an answer on a (pending) DecisionRequest: resolve the user-typed
/// `choice` to an index, set `answered` + `answered_at`, return the index.
/// Pure (no I/O) so the record-and-flip logic is unit-testable without a
/// backend. Errors if already answered or the choice is invalid.
// trace:STORY-522 | ai:claude
pub(crate) fn record_answer(
    request: &mut aida_core::DecisionRequest,
    choice: &str,
) -> Result<usize> {
    if !request.is_pending() {
        anyhow::bail!("already answered");
    }
    let idx = resolve_choice_index(request, choice)?;
    request.answered = Some(idx);
    request.answered_at = Some(chrono::Utc::now());
    Ok(idx)
}

/// Confirm the recommended default on a (pending) DecisionRequest. Returns
/// the chosen index, or `None` when there is no default to confirm or the
/// request is already answered. Pure (no I/O). trace:STORY-522 | ai:claude
pub(crate) fn confirm_default(request: &mut aida_core::DecisionRequest) -> Option<usize> {
    if !request.is_pending() {
        return None;
    }
    let idx = request.recommended?;
    request.answered = Some(idx);
    request.answered_at = Some(chrono::Utc::now());
    Some(idx)
}

// ===================================================================
// STORY-555: the slice-2 auto-applier. The slice-1 `record_answer`
// only recorded the chosen index; here, answering a decision APPLIES
// the chosen resolution token (clears a gate / binds a decision /
// rejects), re-checks pickability, and — when the spec is now genuinely
// decision-free + unblocked — auto-queues it onto the burndown ready
// set. This closes the `aida questions` -> `aida burndown` loop: the
// human-decision drain is the symmetric complement of the autonomous
// drain. trace:STORY-555 | ai:claude
// ===================================================================

/// The parking tags that mark a spec as held for a human DISPOSITION
/// (approve / reject / keep-parked) rather than for an authored design answer.
/// A deliberate SUBSET of [`burndown::parking_tag`]'s set — `needs-supervised-
/// build`, `deferred:*`, and `review:draft-only` carry their own resolution
/// path and are NOT a yes/no disposition, so the sweep does not synthesize a
/// disposition for them. trace:STORY-555 | ai:claude
pub(crate) const DISPOSITION_GATING_TAGS: &[&str] = &[
    "needs-design-signoff",
    "needs-design",
    "needs-human",
    "operator-action",
];

/// The first disposition gating tag present on `tags` (case-insensitive),
/// preserving the spec's own casing in the returned value. trace:STORY-555
pub(crate) fn disposition_gating_tag(tags: &std::collections::HashSet<String>) -> Option<String> {
    tags.iter()
        .find(|t| DISPOSITION_GATING_TAGS.contains(&t.trim().to_ascii_lowercase().as_str()))
        .map(|t| t.trim().to_string())
}

/// The effect of applying one answered choice's resolution token to a spec.
/// `effects` are human-readable lines for the answer printout; `is_disposition`
/// suppresses the R1(a) `## Acceptance` refinement (a disposition is a
/// hold/approve/reject, not a design decision that binds the implementer);
/// `keeps_parked` suppresses the R3 auto-queue (the resolution deliberately
/// left the spec held — rejected, kept-parked, or newly parking-tagged).
/// `binds_acceptance` is true only when a genuine design refinement was made
/// that should be written into `## Acceptance` — `noop`, an unrecognized token,
/// or a no-op edit (empty tag, tag-not-present) mutate nothing and so must NOT
/// claim to have bound a decision (else the printout contradicts itself:
/// "no spec change (noop)" + "bound the decision into ## Acceptance"). trace:TASK-884
// trace:STORY-555 | ai:claude
pub(crate) struct ResolutionApplied {
    pub(crate) effects: Vec<String>,
    /// TASK-884: superseded by `binds_acceptance` for the bind decision (a
    /// disposition AND a noop both must not bind), but retained as the explicit
    /// disposition-vs-refinement classification the tests assert on.
    #[allow(dead_code)]
    pub(crate) is_disposition: bool,
    pub(crate) keeps_parked: bool,
    /// Whether this resolution made a real design refinement worth binding into
    /// `## Acceptance`. trace:TASK-884
    pub(crate) binds_acceptance: bool,
}

/// Apply a DecisionChoice `resolution` token to `req` in place — the slice-2
/// auto-applier `record_answer` deferred. The token grammar mirrors what
/// `aida questions ask --choice label|consequence|<resolution>` and the sweep
/// emit:
///   * `noop`                          — no spec change
///   * `tag:+<tag>` / `tag:-<tag>`     — add / remove a tag
///   * `status:<status>`               — set status (e.g. `status:rejected`)
///   * `disposition:approve-to-ready`  — clear every disposition gating tag (R2)
///   * `disposition:reject`            — set status Rejected (R2)
///   * `disposition:keep-parked`       — record a why-open note, leave parked (R2)
///
/// An unrecognized token is recorded-only (no mutation) so a typo never
/// silently mangles a spec. trace:STORY-555 | ai:claude
pub(crate) fn apply_resolution_token(
    req: &mut Requirement,
    token: &str,
    label: &str,
    consequence: &str,
) -> ResolutionApplied {
    let t = token.trim();

    // BUG-726: accept BARE disposition verbs + common synonyms. Advisors and
    // operators naturally write `defer` / `reject` / `queue`, not the internal
    // `disposition:...` form. Before this, a bare verb fell through to the
    // auto-queue at the bottom — the exact OPPOSITE of a `defer`/`reject` intent
    // — and `defer` had no verb at all.
    let lower = t.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "defer" | "park" | "hold" | "passive" | "passive-only" | "shelve"
    ) {
        // trace:BUG-730 | ai:codex
        req.deferred = true;
        if req.deferred_at.is_none() {
            req.deferred_at = Some(chrono::Utc::now());
        }
        let trigger = consequence.trim();
        if !trigger.is_empty() {
            req.deferred_until = Some(trigger.to_string());
        }
        return ResolutionApplied {
            effects: vec!["deferred — parked out of the queue".to_string()],
            is_disposition: true,
            keeps_parked: true,
            binds_acceptance: false,
        };
    }
    let t = match lower.as_str() {
        "reject" | "archive" | "close" | "drop" | "reject/archive" => "disposition:reject",
        "approve" | "queue" | "ready" | "approve-to-ready" | "approve+queue" => {
            "disposition:approve-to-ready"
        }
        "keep-parked" | "keep" | "leave-parked" | "keep-open" => "disposition:keep-parked",
        _ => t,
    };

    // R2: synthesized-disposition tokens — a yes/no/keep disposition on a
    // tag-parked spec, NOT a design refinement.
    if let Some(rest) = t.strip_prefix("disposition:") {
        return match rest {
            "approve-to-ready" => {
                let cleared: Vec<String> = req
                    .tags
                    .iter()
                    .filter(|tag| {
                        DISPOSITION_GATING_TAGS.contains(&tag.trim().to_ascii_lowercase().as_str())
                    })
                    .cloned()
                    .collect();
                for tag in &cleared {
                    req.tags.remove(tag);
                }
                let effects = if cleared.is_empty() {
                    vec!["no disposition gating tag to clear".to_string()]
                } else {
                    vec![format!("cleared gating tag(s): {}", cleared.join(", "))]
                };
                ResolutionApplied {
                    effects,
                    is_disposition: true,
                    keeps_parked: false,
                    binds_acceptance: false,
                }
            }
            "reject" => {
                req.status = RequirementStatus::Rejected;
                ResolutionApplied {
                    effects: vec!["set status → Rejected".to_string()],
                    is_disposition: true,
                    keeps_parked: true,
                    binds_acceptance: false,
                }
            }
            "keep-parked" => {
                push_why_open(req, label, consequence);
                ResolutionApplied {
                    effects: vec!["recorded why-open; left parked".to_string()],
                    is_disposition: true,
                    keeps_parked: true,
                    binds_acceptance: false,
                }
            }
            other => ResolutionApplied {
                effects: vec![format!(
                    "unrecognized disposition `{other}` — recorded only"
                )],
                is_disposition: true,
                keeps_parked: true,
                binds_acceptance: false,
            },
        };
    }

    // tag:+X / tag:-X
    if let Some(rest) = t.strip_prefix("tag:") {
        if let Some(tag) = rest.strip_prefix('+') {
            let tag = tag.trim();
            if tag.is_empty() {
                return ResolutionApplied {
                    effects: vec!["empty tag in resolution — recorded only".to_string()],
                    is_disposition: false,
                    keeps_parked: false,
                    binds_acceptance: false,
                };
            }
            let existed = req.tags.iter().any(|x| x.eq_ignore_ascii_case(tag));
            if !existed {
                req.tags.insert(tag.to_string());
            }
            // Does the tag we just added still park the spec? Ask the same
            // burndown predicate the gate uses so the two never disagree.
            let parks = burndown::parking_tag(&[tag.to_string()]).is_some();
            return ResolutionApplied {
                effects: vec![format!("added tag `{tag}`")],
                is_disposition: false,
                keeps_parked: parks,
                // A genuine tag mutation is a refinement worth binding; a
                // no-op (tag already present) is not. trace:TASK-884
                binds_acceptance: !existed,
            };
        }
        if let Some(tag) = rest.strip_prefix('-') {
            let needle = tag.trim().to_ascii_lowercase();
            let removed: Vec<String> = req
                .tags
                .iter()
                .filter(|x| x.trim().to_ascii_lowercase() == needle)
                .cloned()
                .collect();
            for tag in &removed {
                req.tags.remove(tag);
            }
            return ResolutionApplied {
                effects: vec![if removed.is_empty() {
                    format!("tag `{needle}` not present")
                } else {
                    format!("removed tag `{needle}`")
                }],
                is_disposition: false,
                keeps_parked: false,
                // Removing a present tag is a refinement; "not present" is a
                // no-op and does not bind. trace:TASK-884
                binds_acceptance: !removed.is_empty(),
            };
        }
    }

    // status:<status>
    if let Some(rest) = t.strip_prefix("status:") {
        return match parse_status(rest) {
            Ok(st) => {
                // A terminal Rejected is not queue-worthy; any other status
                // change does not, by itself, keep the spec parked.
                let keeps = matches!(st, RequirementStatus::Rejected);
                req.status = st.clone();
                ResolutionApplied {
                    effects: vec![format!("set status → {st}")],
                    is_disposition: false,
                    keeps_parked: keeps,
                    // A status refinement (e.g. status:planned) is a real
                    // decision worth binding. trace:TASK-884
                    binds_acceptance: true,
                }
            }
            Err(_) => ResolutionApplied {
                effects: vec![format!("unrecognized status `{rest}` — recorded only")],
                is_disposition: false,
                keeps_parked: false,
                binds_acceptance: false,
            },
        };
    }

    if t.eq_ignore_ascii_case("noop") {
        return ResolutionApplied {
            effects: vec!["no spec change (noop)".to_string()],
            is_disposition: false,
            keeps_parked: false,
            // noop mutates nothing — it must NOT claim to bind a decision. trace:TASK-884
            binds_acceptance: false,
        };
    }

    // BUG-726: a freeform resolution (a design DIRECTIVE with no executable verb,
    // e.g. "context-gated") is recorded as guidance and the spec is LEFT PARKED —
    // never silently queued. Auto-queuing here was the bug that made a `defer`/
    // `reject` answer land the spec in the burndown, the opposite of intent.
    ResolutionApplied {
        effects: vec![format!(
            "recorded as directive `{t}` — left parked for advisor disposition"
        )],
        is_disposition: false,
        keeps_parked: true,
        binds_acceptance: false,
    }
}

/// Record a `why-open:` note (the keep-parked disposition's audit trail) as a
/// comment on the spec. trace:STORY-555 | ai:claude
pub(crate) fn push_why_open(req: &mut Requirement, label: &str, consequence: &str) {
    let now = chrono::Utc::now();
    req.comments.push(Comment {
        id: uuid::Uuid::now_v7(),
        author: current_user_id(None),
        content: format!(
            "why-open: kept parked by operator decision ({}) — {label}: {consequence}",
            now.format("%Y-%m-%d")
        ),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    });
}

/// The `## Acceptance` refinement line that BINDS a chosen design decision into
/// the spec (R1(a) — a decision recorded only as a comment does not bind the
/// next implementer; an acceptance refinement does, per the
/// refinements-must-be-acceptance-criteria discipline). trace:STORY-555
pub(crate) fn resolved_acceptance_line(label: &str, consequence: &str) -> String {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    format!(
        "- Resolved ({date}, operator decision via `aida questions answer`): {label} — {consequence}"
    )
}

/// Append `line` to the spec body's `## Acceptance` section, creating the
/// section if absent. Inserts after the section's last non-blank content line
/// (before the next heading) so the refinement reads as the newest acceptance
/// item rather than landing in the blank gap before the following section.
/// trace:STORY-555 | ai:claude
pub(crate) fn append_resolved_to_acceptance(description: &str, line: &str) -> String {
    let lines: Vec<&str> = description.lines().collect();
    let heading_idx = lines.iter().position(|l| {
        let t = l.trim();
        if !t.starts_with('#') {
            return false;
        }
        let h = t.trim_start_matches('#').trim();
        h.eq_ignore_ascii_case("acceptance") || h.eq_ignore_ascii_case("acceptance criteria")
    });

    match heading_idx {
        Some(start) => {
            // The section runs until the next heading (any level) or EOF.
            let mut end = lines.len();
            for (offset, l) in lines.iter().enumerate().skip(start + 1) {
                if l.trim_start().starts_with('#') {
                    end = offset;
                    break;
                }
            }
            let mut insert_at = end;
            while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            let mut out: Vec<String> = lines[..insert_at].iter().map(|s| s.to_string()).collect();
            out.push(line.to_string());
            out.extend(lines[insert_at..].iter().map(|s| s.to_string()));
            out.join("\n")
        }
        None => {
            let mut out = description.trim_end().to_string();
            out.push_str("\n\n## Acceptance\n\n");
            out.push_str(line);
            out.push('\n');
            out
        }
    }
}

/// Re-run the burndown pickability gate against a spec AFTER its answer's
/// resolution has been applied — the honest "is it actually Ready now, or what
/// still holds it?" check (#2 round-trip + #4 honest partial-unpark). Pure over
/// the passed store snapshot. trace:STORY-555 | ai:claude
pub(crate) fn evaluate_unpark(req: &Requirement, all: &[Requirement]) -> burndown::Pickability {
    if req.deferred {
        // A structural defer is a view-state shelf, not a tag-only burndown
        // gate; it must stay parked until `aida undefer` clears it. trace:BUG-730
        let trigger = req
            .deferred_until
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("no trigger recorded");
        return burndown::Pickability::Parked(format!("deferred: {trigger}"));
    }

    let status_by_id: std::collections::HashMap<uuid::Uuid, RequirementStatus> =
        all.iter().map(|r| (r.id, r.status.clone())).collect();
    let has_unsatisfied_blocker = req.relationships.iter().any(|rel| {
        matches!(rel.rel_type, RelationshipType::BlockedBy)
            && status_by_id
                .get(&rel.target_id)
                .map(|s| *s != RequirementStatus::Completed)
                .unwrap_or(true)
    });
    let candidate = burndown::BurndownCandidate {
        id: req.display_id(),
        req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
        tags: req.tags.iter().cloned().collect(),
        has_unsatisfied_blocker,
        has_pending_decision: req
            .decision_request
            .as_ref()
            .map(|d| d.is_pending())
            .unwrap_or(false),
    };
    burndown::classify(&candidate)
}

/// Apply a just-recorded answer's resolution and, when it fully unparks the
/// spec, auto-queue it — the shared tail of every `aida questions answer` path
/// (single, interactive loop, `--all-defaults`). This is STORY-555's core: the
/// human's answer APPLIES (not just records) the resolution, binds a design
/// decision into `## Acceptance` (R1), clears a disposition gate (R2), and
/// hands the now-decision-free spec to the burndown ready set via the same
/// advisor-gated enqueue path a groom uses (R3). `req` arrives with the answer
/// already recorded on its DecisionRequest. trace:STORY-555 | ai:claude
pub(crate) fn finalize_answer(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    mut req: Requirement,
    idx: usize,
) -> Result<()> {
    let display_id = req.display_id();
    let title = req.title.clone();
    let choice = req
        .decision_request
        .as_ref()
        .and_then(|dr| dr.choices.get(idx).cloned())
        .ok_or_else(|| anyhow::anyhow!("{display_id}: answered choice {idx} is out of range"))?;

    // BUG-1637: the status before the answer; every status change the answer
    // makes (the resolution token and the Draft promotion below) is recorded
    // under the answering caller before the write. trace:BUG-1637 | ai:claude
    let status_before_answer = req.status.clone();

    // Apply the resolution token (mutates tags / status / comments in place).
    let applied = apply_resolution_token(
        &mut req,
        &choice.resolution,
        &choice.label,
        &choice.consequence,
    );

    // R1(a): bind a genuine design decision into ## Acceptance. A disposition
    // (approve/reject/keep) is NOT a design refinement; nor is a noop / no-op
    // edit — `binds_acceptance` is the single predicate so the printout below
    // and the actual mutation never disagree. trace:TASK-884
    if applied.binds_acceptance {
        req.description = append_resolved_to_acceptance(
            &req.description,
            &resolved_acceptance_line(&choice.label, &choice.consequence),
        );
    }

    // BUG-526: when the answerer holds advisor authority (the answer is itself
    // advisor-gated, TASK-647/ADR-3), promote a still-Draft spec to Approved as
    // part of applying the resolution, so the R3 auto-queue below can fire and
    // the ask-ahead/answer-async loop doesn't dead-end at "status is Draft".
    // The resolution must not deliberately keep the spec parked (rejected /
    // kept-parked stay where the chosen resolution left them), and a non-advisor
    // answerer keeps the gate — their answer leaves the spec Draft for triage.
    // trace:BUG-526
    let mut promoted_to_approved = false;
    if matches!(req.status, RequirementStatus::Draft)
        && !applied.keeps_parked
        && has_advisor_authority()
    {
        req.status = RequirementStatus::Approved;
        promoted_to_approved = true;
    }
    // trace:BUG-1637 | ai:claude
    record_caller_status_transition(&mut req, &status_before_answer);

    req.modified_at = chrono::Utc::now();
    backend.update_requirement(&req)?;

    println!(
        "{} {display_id} ({}) → {}",
        "Answered".green().bold(),
        title.dimmed(),
        choice.label.bold()
    );
    for effect in &applied.effects {
        println!(
            "  {} {effect}",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }
    if applied.binds_acceptance {
        println!(
            "  {} bound the decision into ## Acceptance",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }
    if promoted_to_approved {
        // trace:BUG-526
        println!(
            "  {} promoted Draft → Approved (advisor authority)",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    }

    // R3 + #2 + #4: re-check pickability and, if genuinely Ready, auto-queue.
    let all = backend.list_requirements(false)?;
    match evaluate_unpark(&req, &all) {
        burndown::Pickability::Ready if !applied.keeps_parked => {
            maybe_autoqueue(store_path, &req)?;
        }
        burndown::Pickability::Ready => {
            // keeps_parked — rejected / kept-parked: deliberately not queued.
            println!(
                "  {} not queued (left held by the chosen resolution)",
                crate::glyph(crate::glyphs::Glyph::InFlight).dimmed()
            );
        }
        burndown::Pickability::Parked(reason) => {
            // #4 honest partial-unpark: name what still holds it instead of
            // claiming Ready.
            println!(
                "  {} still parked: {} — not queued",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                reason.dimmed()
            );
        }
    }
    Ok(())
}

/// Queue a fully-unparked spec via the dispatch-gated enqueue path a groom
/// uses (R3). The spec is already Approved, so this routes rather than disposes,
/// and only queues Approved work (the burndown
/// ready scope); reports honestly when it can't. Idempotent — skips a spec
/// already queued. trace:STORY-555 | ai:claude
pub(crate) fn maybe_autoqueue(store_path: &std::path::Path, req: &Requirement) -> Result<()> {
    if !matches!(req.status, RequirementStatus::Approved) {
        println!(
            "  {} unparked, but status is {} (not Approved) — not auto-queued",
            crate::glyph(crate::glyphs::Glyph::InFlight).dimmed(),
            req.status
        );
        return Ok(());
    }
    if !has_dispatch_authority() {
        println!(
            "  {} unparked, but this session lacks dispatch authority — ask product, advisor, or integrator to queue",
            crate::glyph(crate::glyphs::Glyph::InFlight).dimmed()
        );
        return Ok(());
    }
    let project_root = find_project_root().ok();
    let already_queued = project_root
        .as_deref()
        .map(all_queued_requirement_ids)
        .map(|ids| ids.contains(&req.id))
        .unwrap_or(false);
    if already_queued {
        println!("  {} already queued — burndown-ready", "→".green());
        return Ok(());
    }
    let storage = Storage::new(store_path);
    let user_id = current_user_id(None);
    backlog::enqueue_groomed(
        &storage,
        req,
        None,
        Some("unparked by `aida questions answer`"),
        &user_id,
    )?;
    println!("  {} queued — now burndown-ready", "→".green().bold());
    Ok(())
}

/// Render a single pending DecisionRequest to stdout — question, numbered
/// choices (recommended marked), and rationale. Shared by the list view
/// and the interactive answer loop.
// trace:STORY-522 | ai:claude
pub(crate) fn print_decision_request(
    display_id: &str,
    title: &str,
    request: &aida_core::DecisionRequest,
) {
    println!(
        "{} {}  {}",
        "Decision needed:".yellow().bold(),
        display_id.cyan().bold(),
        title.dimmed()
    );
    println!("  {}", request.question);
    for (i, choice) in request.choices.iter().enumerate() {
        let marker = if request.recommended == Some(i) {
            " (recommended)".green().to_string()
        } else {
            String::new()
        };
        println!(
            "    {}{} {} — {}",
            format!("{}.", i + 1).bold(),
            marker,
            choice.label.bold(),
            choice.consequence.dimmed()
        );
    }
    if let Some(rationale) = &request.rationale {
        println!("  {} {}", "Why:".dimmed(), rationale);
    }
}

/// Load every requirement carrying a DecisionRequest, partitioned into
/// (pending, answered). Reads full objects (the cache does not project the
/// `decision_request` field, mirroring history). trace:STORY-522 | ai:claude
pub(crate) fn collect_decision_requests(
    backend: &aida_core::CachedGitBackend,
) -> Result<(Vec<aida_core::Requirement>, Vec<aida_core::Requirement>)> {
    let mut pending = Vec::new();
    let mut answered = Vec::new();
    for req in backend.list_requirements(false)? {
        match &req.decision_request {
            Some(dr) if dr.is_pending() => pending.push(req),
            Some(_) => answered.push(req),
            None => {}
        }
    }
    Ok((pending, answered))
}

/// TASK-1061 → TASK-1065: the store-based pending-DecisionRequest count. This is
/// now the **reference oracle** for the cache-backed
/// `CachedGitBackend::pending_decision_count`: the `has_pending_decision` cache
/// column projects exactly this predicate (non-archived +
/// `decision_request.is_pending()`) per row, so `aida status --full`'s
/// decision-inbox line reads the cache column instead of a full store load. Kept
/// (test-only) as the equivalence anchor its test module asserts against.
// trace:TASK-1065 (supersedes TASK-1061)
#[cfg(test)]
pub(crate) fn pending_decision_request_count(
    store: &aida_core::models::RequirementsStore,
) -> usize {
    store
        .requirements
        .iter()
        // Match `list_requirements(false)` exactly: archived rows are excluded
        // (the predicate the previous `collect_decision_requests` path used).
        .filter(|req| !req.archived)
        .filter(|req| {
            req.decision_request
                .as_ref()
                .map(|dr| dr.is_pending())
                .unwrap_or(false)
        })
        .count()
}

// TASK-1061: the store-backed pending-decision count that replaced the
// `--full` presence section's redundant second full-store load must agree
// EXACTLY with the prior `collect_decision_requests` pending count — same
// predicate (`decision_request.is_pending()`), same archived-excluded scope
// (`list_requirements(false)`). trace:TASK-1061
#[cfg(test)]
#[path = "tests/pending_decision_request_count_tests.rs"]
mod pending_decision_request_count_tests;

/// `aida questions` / `aida questions list` / `ask` / `answer` dispatch.
// trace:STORY-522 | ai:claude
pub(crate) fn handle_questions_command(
    cmd: Option<&QuestionsCommand>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        // Bare `aida questions` — list, then (TTY + pending) offer the loop.
        None => {
            let pending_count = questions_list(backend)?;
            let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
            if interactive && pending_count > 0 {
                print!("\nAnswer {pending_count} now? [Y/n] ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let answer = input.trim();
                if answer.is_empty() || answer.eq_ignore_ascii_case("y") {
                    questions_answer_loop(backend, store_path)?;
                }
            }
            Ok(())
        }
        Some(QuestionsCommand::List) => {
            questions_list(backend)?;
            Ok(())
        }
        Some(QuestionsCommand::Sweep { scope, apply }) => {
            questions_sweep(backend, scope.as_deref(), *apply)?;
            Ok(())
        }
        Some(QuestionsCommand::Clarify { specs, dry_run }) => {
            questions_clarify(backend, store_path, specs, *dry_run)
        }
        Some(QuestionsCommand::Remedy { spec, dry_run }) => {
            questions_remedy(backend, spec, *dry_run)
        }
        Some(QuestionsCommand::Ask {
            spec,
            question,
            choice,
            recommend,
            rationale,
            force,
        }) => questions_ask(
            backend,
            store_path,
            spec,
            question,
            choice,
            *recommend,
            rationale.as_deref(),
            *force,
        ),
        Some(QuestionsCommand::Answer {
            spec,
            choice,
            all_defaults,
            note,
        }) => {
            if *all_defaults {
                if spec.is_some() || choice.is_some() || note.is_some() {
                    anyhow::bail!(
                        "--all-defaults answers every recommended default in bulk; \
                         do not also pass <spec>/<choice>/--note"
                    );
                }
                return questions_answer_all_defaults(backend, store_path);
            }
            match (spec, choice) {
                // trace:TASK-791 | ai:claude
                (Some(spec), Some(choice)) => {
                    questions_answer_one(backend, store_path, spec, choice, note.as_deref())
                }
                (None, None) => {
                    if note.is_some() {
                        anyhow::bail!(
                            "--note attaches a counter-proposal to a specific answer; \
                             pass <spec> and <choice> too"
                        );
                    }
                    questions_answer_loop(backend, store_path)
                }
                _ => anyhow::bail!(
                    "answer takes either both <spec> and <choice> \
                     (non-interactive) or neither (interactive loop)"
                ),
            }
        }
    }
}

/// `aida decide <spec>` — the natural human-resolution entry point. A thin
/// dispatcher over the `aida questions` verbs: if the spec carries a PENDING
/// DecisionRequest (enumerated choices), route to the answer path; otherwise
/// the spec is under-specified, so route to the interactive clarifier. The
/// underlying logic stays in `aida questions` — this only picks the lane and
/// reuses the existing handlers.
// trace:TASK-779 | ai:claude
/// STORY-568: the research/spike lane. Dispatch a SPIKE to a headless research
/// agent (reusing the `deep-research` skill), capture the source-grounded
/// analysis as the deliverable (a dated artifact + a comment on the spike),
/// and ESCALATE any strategic decision to the questions inbox — never
/// auto-applied. The agent-able counterpart to the implementer drain; a
/// spike's "done" is deliverable-produced, not PR-merged.
///
/// `--dry-run` composes the prompt + classification + artifact paths and stops
/// (no spawn, no writes) so the advisor can inspect before paying for a run.
// trace:STORY-568 | ai:claude
pub(crate) fn handle_research_command(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    id: &str,
    dry_run: bool,
    artifact_dir: &str,
) -> Result<()> {
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.display_id();

    // Reclassify rather than flatly refuse: a genuinely human-only spike is
    // not agent-able; everything else is the research lane.
    if !matches!(req.req_type, aida_core::RequirementType::Spike) {
        eprintln!(
            "{} {display_id} is a {} — the research lane is for spikes. Proceeding anyway.",
            "Note:".yellow(),
            format!("{:?}", req.req_type).to_lowercase()
        );
    }
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    if matches!(
        crate::burndown::classify_spike_lane(&tags),
        crate::burndown::SpikeLane::HumanOnly
    ) {
        anyhow::bail!(
            "{display_id} is tagged `human-only` — human analysis required; the \
             research lane does not dispatch it. Remove the tag to make it \
             agent-able."
        );
    }

    // Resolve artifact paths (dated). Body = description + the spike's own
    // acceptance text already lives in `description` for AIDA specs.
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let project_root = store_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let dir = project_root.join(artifact_dir);
    let (analysis_path, sidecar_path) =
        crate::research::research_artifact_paths(&dir, &display_id, &date);
    let analysis_rel = format!(
        "{}/{}",
        artifact_dir.trim_end_matches('/'),
        analysis_path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    );

    let prompt = crate::research::build_research_prompt(
        &display_id,
        &req.title,
        &req.description,
        &analysis_path,
        &sidecar_path,
    );

    if dry_run {
        println!(
            "{} {display_id} — {}",
            "Research dispatch:".cyan().bold(),
            req.title
        );
        println!("  lane:     research-lane (agent-able)");
        println!("  analysis: {}", analysis_path.display());
        println!("  sidecar:  {}", sidecar_path.display());
        println!("\n{}", "── research prompt ──".dimmed());
        println!("{prompt}");
        println!(
            "\n{}",
            "Dry run — nothing spawned or written. Drop --dry-run to dispatch.".dimmed()
        );
        return Ok(());
    }

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating artifact dir {}", dir.display()))?;

    // Dispatch the headless research agent. The spawn is the integration
    // boundary; the pure transforms above/below are unit-tested.
    println!(
        "{} {display_id} — dispatching research agent (deep-research, headless)…",
        "Research:".cyan().bold()
    );
    let session_id = Uuid::now_v7().to_string();
    let log_path = project_root.join(format!(".aida/research/{display_id}-{date}.log"));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("research:{display_id}"));
    let status =
        crate::session::spawn_claude_headless(&prompt, &session_id, &log_path, &tee, false)
            .context("spawning headless research agent")?;
    if !status.success() {
        anyhow::bail!(
            "research agent exited with {} — see log {}",
            status.code().unwrap_or(-1),
            log_path.display()
        );
    }

    // Collect the deliverable. The analysis artifact is mandatory; the sidecar
    // is optional (informational spikes have no decision to escalate).
    if !analysis_path.exists() {
        anyhow::bail!(
            "research agent did not produce the analysis artifact at {} — see log {}",
            analysis_path.display(),
            log_path.display()
        );
    }
    let now = chrono::Utc::now();

    // Attach a provenance-tagged pointer comment so the deliverable is visible
    // from `aida show` while the full report lives in the dated file.
    let summary = std::fs::read_to_string(&sidecar_path)
        .ok()
        .and_then(|j| crate::research::parse_research_sidecar(&j).ok())
        .map(|s| s.summary)
        .unwrap_or_else(|| "Analysis produced (no machine-readable summary).".to_string());
    req.comments.push(aida_core::Comment {
        id: Uuid::now_v7(),
        content: crate::research::provenance_comment(&analysis_rel, &summary),
        author: "research-agent".to_string(),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    });

    // Escalate the decision (if any) — never auto-apply it. A pending decision
    // parks the spike from any drain and surfaces in `aida questions`.
    let mut escalated: Option<aida_core::DecisionRequest> = None;
    if let Ok(json) = std::fs::read_to_string(&sidecar_path) {
        if let Ok(sidecar) = crate::research::parse_research_sidecar(&json) {
            if let Some(dr) = crate::research::sidecar_to_decision_request(&sidecar, now) {
                if req
                    .decision_request
                    .as_ref()
                    .map(|d| d.is_pending())
                    .unwrap_or(false)
                {
                    eprintln!(
                        "{} {display_id} already has a pending decision — leaving it; \
                         the new analysis is attached as a comment.",
                        "Note:".yellow()
                    );
                } else {
                    req.decision_request = Some(dr.clone());
                    escalated = Some(dr);
                }
            }
        }
    }

    // Deliverable-produced = spike "done" (NOT PR-merged). Tag it so the lane
    // is auditable and the spike reads as research-complete.
    req.tags.insert("spike:deliverable-produced".to_string());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    println!(
        "{} analysis at {}",
        format!("{} deliverable:", crate::glyph(crate::glyphs::Glyph::Check))
            .green()
            .bold(),
        analysis_path.display()
    );
    println!("  pointer comment attached to {display_id}");
    if let Some(dr) = escalated {
        println!(
            "\n{}",
            "Decision escalated — answer in your own time:".bold()
        );
        print_decision_request(&display_id, &req.title, &dr);
        println!(
            "{}",
            "Answer with `aida questions answer` (or `aida questions` at a TTY).".dimmed()
        );
    } else {
        println!(
            "{}",
            "  no strategic decision surfaced — informational deliverable only.".dimmed()
        );
    }
    Ok(())
}

/// `aida questions sweep` - proactive producer for the async decision inbox.
/// The cheap pre-filter is deterministic; only flagged candidates receive a
/// formulated DecisionRequest, and specs with open requests are skipped for
/// idempotency. trace:STORY-523 | ai:codex
pub(crate) fn questions_sweep(
    backend: &aida_core::CachedGitBackend,
    raw_scope: Option<&str>,
    apply: bool,
) -> Result<usize> {
    let scope = QuestionSweepScope::parse(raw_scope)?;
    let all = backend.list_requirements(false)?;
    // BUG-495: exclude specs holding an active lease (work in-flight) from the
    // missing-acceptance flag. Reuse the same live-lease detection the queue +
    // session surfaces use, so the sweep and the rest of the system agree on
    // what's in-flight. trace:BUG-495
    let in_flight_scopes = find_project_root()
        .map(|root| in_flight_lease_scopes(&root))
        .unwrap_or_default();
    let mut candidates = Vec::new();

    for req in &all {
        if let Some(candidate) = question_sweep_candidate(req, &all, scope, &in_flight_scopes) {
            candidates.push((req.display_id(), candidate));
        }
    }

    if candidates.is_empty() {
        println!(
            "{}",
            "Question sweep found no new human-decision candidates.".dimmed()
        );
        return Ok(0);
    }

    // TASK-700: default to a read-only preview — the sweep is a coarse keyword
    // heuristic that MUTATES specs (attaches DecisionRequests), so it must not
    // write without an explicit --apply. Mirrors `integrate --dry-run`.
    // trace:TASK-700 | ai:claude
    let mut affected = 0usize;
    for (spec, candidate) in candidates {
        // trace:TASK-1468 | ai:claude
        let Some(mut req) = backend.get_requirement_unambiguous(&spec)? else {
            continue;
        };
        if has_open_decision_request(&req) {
            continue;
        }
        if apply {
            let request = formulate_sweep_decision_request(&req, &candidate);
            req.decision_request = Some(request);
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!(
                "  {} {} - {}",
                "Recorded".green(),
                spec.cyan(),
                candidate.reason.dimmed()
            );
        } else {
            println!(
                "  {} {} - {}",
                "[dry-run] would attach".dimmed(),
                spec.cyan(),
                candidate.reason.dimmed()
            );
        }
        affected += 1;
    }

    if apply {
        println!(
            "{} attached a DecisionRequest to {affected} spec{} — each is parked from the burndown until its question is answered (`aida questions answer`).",
            "Done.".green().bold(),
            if affected == 1 { "" } else { "s" }
        );
    } else {
        // BUG-495: name what --apply actually does. It attaches a DecisionRequest
        // that PARKS the spec from the burndown (records the gap); it does NOT
        // write or generate the missing acceptance criteria. Point at the real
        // fix for a missing-acceptance flag so the two are not conflated.
        // trace:BUG-495
        println!(
            "{} {affected} spec{} flagged. Re-run with {} to attach a DecisionRequest (parks each spec from the burndown until answered) — it does not write the missing acceptance criteria.",
            "Dry-run:".yellow().bold(),
            if affected == 1 { "" } else { "s" },
            "--apply".cyan()
        );
        println!(
            "  {} to resolve a `missing acceptance criteria` flag, add a {} section to the spec.",
            "Tip:".dimmed(),
            "## Acceptance".cyan()
        );
    }
    Ok(affected)
}

/// True when a spec must NOT be offered to the clarify loop, honouring the
/// BUG-495 exclusions: non-implementable types (vision/folder/meta/principle/
/// term), already-built/terminal status (Done/Completed/Rejected), and
/// held-for-review specs (`review:draft-only`). The active-lease exclusion is
/// applied separately by the caller (it needs the live lease set).
// trace:STORY-557 | ai:claude
pub(crate) fn is_clarify_excluded(req: &Requirement) -> bool {
    if matches!(
        req.req_type,
        RequirementType::Vision
            | RequirementType::Folder
            | RequirementType::Meta
            | RequirementType::Principle
            | RequirementType::Term
    ) {
        return true;
    }
    if matches!(
        req.status,
        RequirementStatus::Done | RequirementStatus::Completed | RequirementStatus::Rejected
    ) {
        return true;
    }
    if req.tags.iter().any(|t| t == "review:draft-only") {
        return true;
    }
    false
}

/// Resolve the default clarify set: specs the sweep would flag as
/// under-specified, minus the BUG-495 exclusions and any spec held by a live
/// lease. Returns display IDs in stable order. trace:STORY-557 | ai:claude
pub(crate) fn clarify_default_specs(backend: &aida_core::CachedGitBackend) -> Result<Vec<String>> {
    let all = backend.list_requirements(false)?;
    let project_root = find_project_root().ok();
    let in_flight: HashSet<String> = project_root
        .as_deref()
        .map(in_flight_lease_scopes)
        .unwrap_or_default();
    let mut out = Vec::new();
    for req in &all {
        if is_clarify_excluded(req) {
            continue;
        }
        // Active-lease exclusion: an implementer is mid-flight on this scope.
        let live = !in_flight.is_empty()
            && [req.agreed_id.as_deref(), req.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|s| in_flight.contains(&s.to_ascii_lowercase()));
        if live {
            continue;
        }
        // Reuse the sweep's under-specification detector across every workable
        // scope so clarify resolves exactly what sweep flags.
        if question_sweep_candidate(req, &all, QuestionSweepScope::All, &in_flight).is_some() {
            out.push(req.display_id());
        }
    }
    Ok(out)
}

/// `aida questions clarify [<spec>...]` — fire up an INTERACTIVE advisor
/// (`claude "/aida-clarify <specs>"`, never headless `claude -p`) that walks
/// the human through authoring acceptance criteria for under-specified specs.
/// The interactive counterpart to `aida burndown run` (headless): sweep
/// detects, clarify resolves. With no specs it defaults to the swept set.
// trace:STORY-557 | ai:claude
pub(crate) fn questions_clarify(
    backend: &aida_core::CachedGitBackend,
    _store_path: &std::path::Path,
    specs: &[String],
    dry_run: bool,
) -> Result<()> {
    // Resolve the target set: explicit args, or the default swept set.
    let targets: Vec<String> = if specs.is_empty() {
        let resolved = clarify_default_specs(backend)?;
        if resolved.is_empty() {
            println!(
                "{}",
                "No under-specified specs to clarify — the sweep is clean.".dimmed()
            );
            return Ok(());
        }
        resolved
    } else {
        // Explicit specs: drop any that are non-clarifiable, warn loudly.
        let mut kept = Vec::new();
        for raw in specs {
            // trace:TASK-1468 | ai:claude
            match backend.get_requirement_unambiguous(raw)? {
                Some(req) if is_clarify_excluded(&req) => {
                    println!(
                        "  {} {} skipped — not clarifiable (built/held/non-implementable type)",
                        "·".dimmed(),
                        req.display_id().yellow()
                    );
                }
                Some(req) => kept.push(req.display_id()),
                None => println!("  {} {} not found — skipped", "·".dimmed(), raw.yellow()),
            }
        }
        if kept.is_empty() {
            anyhow::bail!("no clarifiable specs in the given set");
        }
        kept
    };

    println!(
        "{} clarify",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold()
    );
    println!(
        "  {} {} spec(s): {}",
        "→".green(),
        targets.len(),
        targets.join(", ").cyan()
    );

    // Build the interactive invocation. Unlike the headless drain, clarify
    // needs a human at the keyboard, so it launches plain interactive
    // `claude "<prompt>"` — NOT `claude -p`.
    let prompt = format!("/aida-clarify {}", targets.join(" "));

    if dry_run {
        println!("\n{} dry run — not launching. Would run:", "·".dimmed());
        println!("  claude {prompt:?}");
        return Ok(());
    }

    println!(
        "  {} launching interactive `claude` — answer its questions to author acceptance",
        "→".green()
    );
    println!(
        "  {} {}",
        "→".green(),
        format!("claude {prompt:?}").dimmed()
    );
    println!();

    // Interactive: inherit the terminal so the human can converse. `claude` is
    // resolved off PATH (matching every other launch site); a missing binary
    // surfaces as a guided error, not a raw ENOENT. STORY-762: on a
    // codex-only machine the guidance names the alternatives instead of just
    // demanding an install the machine may not permit — the clarify loop
    // drives a Claude Code skill, so Codex parity arrives with the
    // skills-parity work, not by swapping the binary here.
    let project_root = find_project_root()?;
    let child_grant =
        seat_authority::issue_child(&project_root, "advisor", &current_user_id(None))?;
    let status_code = std::process::Command::new("claude")
        .arg(&prompt)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(seat_authority::GRANT_ENV, child_grant.id)
        .status()
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to launch `claude` ({e}) — the acceptance-authoring loop drives a \
                 Claude Code skill and needs the Claude Code CLI on PATH.\n\
                 On a Codex-only machine, instead:\n\
                 - answer questions directly: aida questions list / aida questions answer\n\
                 - or edit acceptance by hand: aida edit <spec-id> --description ..."
            )
        })?;

    if status_code.success() {
        Ok(())
    } else {
        let code = status_code.code().unwrap_or(1);
        std::process::exit(code);
    }
}

/// `aida questions remedy <spec>` — the headless, bounded acceptance-authoring
/// pass consumed by the TUI drive auto-remedy loop. Unlike
/// [`questions_clarify`], this path must not ask the human anything: it either
/// grounds minimal acceptance criteria from the existing substrate and writes
/// them, or exits non-zero so the caller can keep the gate held.
// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
    dry_run: bool,
) -> Result<()> {
    let req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| anyhow::anyhow!("{spec} not found"))?;
    if is_clarify_excluded(&req) {
        anyhow::bail!(
            "{} is not eligible for automatic acceptance remedy (built/held/non-implementable type)",
            req.display_id()
        );
    }

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let vendor = session::resolve_headless_vendor(&project_root);
    let prompt = questions_remedy_prompt(&req.display_id());
    let log_path = questions_remedy_log_path(&project_root, &req.display_id());

    println!("remedying {} with headless advisor", req.display_id());
    println!("  vendor: {}", vendor.as_str());
    println!("  log: {}", log_path.display());

    if dry_run {
        println!("\ndry run — not launching. Prompt:\n\n{prompt}");
        return Ok(());
    }

    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("remedy-{}", req.display_id().to_ascii_lowercase()));
    let session_id = uuid::Uuid::now_v7().to_string();
    let status = session::spawn_vendor_headless_with_seat(
        vendor,
        aida_core::agents_config::AgentSeat::Advisor,
        &prompt,
        &session_id,
        &log_path,
        &tee,
        false,
    );
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(1);
        std::process::exit(code);
    }
}

/// BUG-1186: the PR head SHA as the forge reports it right now, or `None` on
/// any forge fault. Used both to anchor the reviewer prompt (BUG-868) and as
/// the pre/post pair for the post-review integrity guard.
// trace:BUG-1186 | ai:claude
pub(crate) fn pr_head_sha_best_effort(driver: &RealPhaseDriver, pr: u32) -> Option<String> {
    let mut sink = crate::network_retry::NoopSink;
    driver
        .lifecycle_forge()
        .change_metadata(pr as u64, &mut sink)
        .ok()
        .map(|m| m.head_sha)
        .filter(|s| !s.trim().is_empty())
}

/// Accept a reconciliation-time CI result only when it is terminal green and
/// belongs to the exact head the PR/MR metadata reports. The error deliberately
/// names both heads so stale and incomplete forge evidence is diagnosable.
// trace:BUG-1819 | ai:codex
pub(crate) fn verified_from_pr_ci_head(
    pr: u32,
    expected_head: Option<&str>,
    evidence: &crate::forge::CiProbeEvidence,
) -> Result<String, auto_complete::PhaseFailure> {
    let expected = expected_head
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .unwrap_or("<unknown>");
    let observed = evidence
        .head_sha
        .as_deref()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .unwrap_or("<unknown>");
    let observed_change = match &evidence.probe {
        crate::forge::CiProbeResult::Green { change } => Some(*change),
        _ => None,
    };
    if observed_change != Some(u64::from(pr)) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "review of PR-{pr} refused: CI evidence is not terminal green for this change (observed change {}); expected head {expected}, observed head {observed}",
                observed_change
                    .map(|change| change.to_string())
                    .unwrap_or_else(|| "<nonterminal>".to_string())
            ),
        ));
    }
    if expected == "<unknown>"
        || observed == "<unknown>"
        || !expected.eq_ignore_ascii_case(observed)
    {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "review of PR-{pr} refused: CI evidence does not cover the current change; expected head {expected}, observed head {observed}"
            ),
        ));
    }
    Ok(observed.to_string())
}

#[cfg(test)]
mod bug_1819_from_pr_ci_evidence_tests {
    use super::*;

    const HEAD: &str = "a91957a858320c0e17f3a0eca7cfacbff50ea29a";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn evidence(
        probe: crate::forge::CiProbeResult,
        head: Option<&str>,
    ) -> crate::forge::CiProbeEvidence {
        crate::forge::CiProbeEvidence {
            probe,
            head_sha: head.map(str::to_string),
        }
    }

    #[test]
    fn direct_reviewer_reentry_accepts_exact_green_head() {
        let verified = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(
                crate::forge::CiProbeResult::Green { change: 26 },
                Some(HEAD),
            ),
        )
        .expect("an exact terminal green head is reviewable");
        assert_eq!(verified, HEAD);

        let tmp = tempfile::TempDir::new().unwrap();
        let mut driver = RealPhaseDriver::new(
            tmp.path().to_path_buf(),
            "BUG-205".into(),
            "tester".into(),
            None,
            false,
            None,
            AutonomyMode::Default,
            "run-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::Full,
        );
        driver.seed_resume_state(
            Some("bug-205".into()),
            Some(26),
            None,
            Some(verified),
            Some(true),
        );
        assert_eq!(driver.ci_terminal_sha.as_deref(), Some(HEAD));
        assert_eq!(driver.ci_terminal_green, Some(true));
    }

    #[test]
    fn stale_evidence_names_expected_and_observed_heads() {
        let failure = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(crate::forge::CiProbeResult::Green { change: 26 }, Some(OLD)),
        )
        .expect_err("a green result for an old head must be rejected");
        assert!(failure.reason.contains(HEAD), "{}", failure.reason);
        assert!(failure.reason.contains(OLD), "{}", failure.reason);
    }

    #[test]
    fn nonterminal_evidence_names_expected_and_observed_heads() {
        let failure = verified_from_pr_ci_head(
            26,
            Some(HEAD),
            &evidence(
                crate::forge::CiProbeResult::InProgress { change: 26 },
                Some(HEAD),
            ),
        )
        .expect_err("pending CI must not authorize review");
        assert!(failure.reason.contains("not terminal green"));
        assert!(failure.reason.contains(HEAD), "{}", failure.reason);
    }
}

/// TASK-1449: like [`pr_head_sha_best_effort`], but the PR's head branch
/// name — used by the rework no-op guard to check whether a PR OTHER than
/// the one it armed against (the BUG-1527 shape) is actually attributed to
/// the spec before treating its existence as anything at all. `None` on any
/// forge fault (e.g. pure-git, which has no PR metadata to read).
// trace:TASK-1449 | ai:claude
pub(crate) fn pr_head_ref_best_effort(driver: &RealPhaseDriver, pr: u32) -> Option<String> {
    let mut sink = crate::network_retry::NoopSink;
    driver
        .lifecycle_forge()
        .change_metadata(pr as u64, &mut sink)
        .ok()
        .map(|m| m.head_ref)
        .filter(|s| !s.trim().is_empty())
}

/// TASK-1449 (BUG-1522 AC5/AC6; hardened on re-review): the DISPATCHED
/// branch's head as ORIGIN reports it, read locally rather than via the
/// forge. Fetches `origin/<branch>` first (best effort) so a stale
/// remote-tracking ref is never read as truth — this fn is the ONLY reader
/// for both the arm-time baseline and the post-round comparison, so a ref
/// that was stale at arm time gets refreshed at arm time too, instead of
/// only on the later read (which would manufacture a false "content
/// changed": stale local W at arm time, freshly-fetched real X after —
/// X looks new but was already the state on origin before this round ran).
/// Deliberately does NOT fall back to a same-named local branch: a
/// dispatched round's open PR lives on origin by definition, and a
/// same-named local branch could be unrelated leftover state. `None` —
/// UNKNOWN, fail-closed — when `origin/<branch>` cannot be read at all (no
/// origin, branch never pushed, a git error).
// trace:TASK-1449 | ai:claude
pub(crate) fn dispatched_branch_head_sha(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    let _ = fetch_branch(project_root, branch, true);
    git_rev_parse_quiet(project_root, &format!("origin/{branch}"))
}

/// TASK-1448: the drain merge phase's approval-covers-head gate. `Err` is a
/// shelvable, never-retried `StaleApproval` failure carrying the same
/// refusal text `aida pr ship` prints — the drain parks the spec and moves
/// on rather than exiting. Pure over the verdicts + head so every branch is
/// testable without a forge.
// trace:TASK-1448 | ai:claude
pub(crate) fn drain_merge_approval_gate(
    candidates: &[review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    pr: u64,
) -> Result<(), auto_complete::PhaseFailure> {
    match pr_ship::approval_head_refusal(candidates, head_sha) {
        None => Ok(()),
        Some(refusal) => Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::StaleApproval,
            pr_ship::approval_head_refusal_message(pr, &refusal),
        )),
    }
}

/// TASK-1458: the `MergeOptions.match_head` pin for the drain's phase-4
/// merge — the approved commit once `drain_merge_approval_gate` has passed.
/// Kept beside the gate so the wiring test drives the same pair of calls the
/// real `merge()` makes.
// trace:TASK-1458 | ai:claude
pub(crate) fn drain_merge_match_head(
    candidates: &[review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
) -> Option<String> {
    pr_ship::approved_match_head(candidates, head_sha)
}

/// Add the orchestrator-owned review context to a reviewer-written PR verdict.
///
/// The reviewer owns the verdict, summary, findings, and any future fields;
/// `record_verdict` deliberately preserves those while adding the commit and
/// audit metadata only the drain can reliably know.
// trace:TASK-168 | ai:codex
pub(crate) fn stamp_pr_review_verdict(
    project_root: &std::path::Path,
    pr: u32,
    reviewed_sha: Option<&str>,
    reviewed_branch: Option<&str>,
    ci_terminal_sha: Option<&str>,
    ci_terminal_green: Option<bool>,
) -> std::io::Result<std::path::PathBuf> {
    let path = review_verdict::record_verdict(
        project_root,
        &format!("PR-{pr}"),
        None,
        reviewed_sha,
        reviewed_branch,
        None,
        &[],
        "aida drain reviewer",
    )?;
    if ci_terminal_sha.is_some() || ci_terminal_green.is_some() {
        let body = std::fs::read_to_string(&path)?;
        let mut value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let object = value.as_object_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "review verdict is not an object",
            )
        })?;
        if let Some(sha) = ci_terminal_sha {
            object.insert(
                "ci_terminal_sha".into(),
                serde_json::Value::String(sha.into()),
            );
        }
        if let Some(green) = ci_terminal_green {
            object.insert("ci_terminal_green".into(), serde_json::Value::Bool(green));
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    }
    Ok(path)
}

#[cfg(test)]
mod task_1448_drain_merge_approval_gate_tests {
    // trace:TASK-1448 | ai:claude
    use super::*;

    const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn approval(sha: Option<&str>) -> review_verdict::RecordedVerdict {
        review_verdict::RecordedVerdict {
            kind: review_verdict::VerdictKind::Approved,
            raw: "approved".into(),
            reviewed_sha: sha.map(str::to_string),
            ..Default::default()
        }
    }

    fn assert_shelves(result: Result<(), auto_complete::PhaseFailure>) -> String {
        let failure = result.expect_err("the merge phase must refuse");
        assert_eq!(failure.kind, auto_complete::FailureKind::StaleApproval);
        // Shelve-and-continue, never stop the drain; never a transient retry.
        assert!(failure.kind.is_shelvable());
        assert!(!auto_complete::should_retry_transient_failure(
            failure.kind,
            0,
            3
        ));
        assert!(
            failure.reason.contains("Re-review the current head"),
            "{}",
            failure.reason
        );
        failure.reason
    }

    #[test]
    fn approval_at_head_lets_the_drain_merge() {
        assert!(drain_merge_approval_gate(&[approval(Some(HEAD))], Some(HEAD), 5).is_ok());
    }

    #[test]
    fn approval_behind_head_shelves() {
        let reason = assert_shelves(drain_merge_approval_gate(
            &[approval(Some(OLD))],
            Some(HEAD),
            5,
        ));
        assert!(
            reason.contains(&OLD[..12]) && reason.contains(&HEAD[..12]),
            "{reason}"
        );
    }

    #[test]
    fn approval_without_sha_shelves() {
        assert_shelves(drain_merge_approval_gate(&[approval(None)], Some(HEAD), 5));
    }

    #[test]
    fn unreadable_head_shelves() {
        assert_shelves(drain_merge_approval_gate(&[approval(Some(HEAD))], None, 5));
    }

    #[test]
    fn stale_approval_recovery_hint_names_the_re_review() {
        let ctx = auto_complete::HintContext {
            spec: "TASK-1448".into(),
            pr_number: Some(5),
            ..Default::default()
        };
        let hint = auto_complete::recovery_hint(
            auto_complete::Phase::Merge,
            auto_complete::FailureKind::StaleApproval,
            &ctx,
        );
        assert!(hint.contains("Re-review the"), "{hint}");
        assert!(hint.contains("PR-5"), "{hint}");
        assert_eq!(
            auto_complete::FailureKind::StaleApproval.cause_slug(),
            "stale-approval"
        );
    }
}

#[cfg(test)]
mod task_168_pr_verdict_metadata_tests {
    use super::*;

    #[test]
    fn stamps_pr_verdict_metadata_without_losing_reviewer_fields() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = review_verdict::verdict_path(tmp.path(), "PR-1965");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"verdict":"Approved","summary":"clean","findings":["note"],"mode":"deep"}"#,
        )
        .unwrap();

        stamp_pr_review_verdict(
            tmp.path(),
            1965,
            Some("deadbeefdeadbeefdeadbeef"),
            Some("task-168-work"),
            Some("deadbeefdeadbeefdeadbeef"),
            Some(true),
        )
        .unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        // BUG-1505: the drain stamp canonicalizes the reviewer's spelling.
        assert_eq!(value["verdict"], "approved");
        assert_eq!(value["summary"], "clean");
        assert_eq!(value["findings"][0], "note");
        assert_eq!(value["mode"], "deep");
        assert_eq!(value["reviewed_sha"], "deadbeefdeadbeefdeadbeef");
        assert_eq!(value["reviewed_branch"], "task-168-work");
        assert_eq!(value["ci_terminal_sha"], "deadbeefdeadbeefdeadbeef");
        assert_eq!(value["ci_terminal_green"], true);
        assert_eq!(value["recorded_by"], "aida drain reviewer");
        assert!(value["recorded_at"].as_str().is_some());
    }
}

/// BUG-1186: `Some(message)` when the PR head moved between the pre- and
/// post-review probes — the reviewer seat wrote to the branch under review.
/// Pure so the decision is unit-testable; SHA comparison is whitespace- and
/// case-insensitive (forges differ in casing).
// trace:BUG-1186 | ai:claude
pub(crate) fn reviewer_wrote_message(pr: u32, pre: &str, post: &str) -> Option<String> {
    let pre = pre.trim().to_ascii_lowercase();
    let post = post.trim().to_ascii_lowercase();
    if pre.is_empty() || post.is_empty() || pre == post {
        return None;
    }
    let short = |s: &str| s.chars().take(9).collect::<String>();
    Some(format!(
        "the reviewer pushed to PR-{pr} during the review (head {} → {}) — a reviewer must \
         verdict, not implement; the CI the drain watched no longer covers this head",
        short(&pre),
        short(&post),
    ))
}

/// BUG-1186 / ADR-40: substrate-as-bouncer for the Done transition. The
/// implementer opened (or was handed) PR-{pr}; if it exited without `aida
/// queue done` the spec is still In Progress, and the review phase's `queue
/// work PR-N` would then see an in-flight spec carrying a rework reason — the
/// seat swap BUG-1186 reproduced. The orchestrator asserts Done itself rather
/// than relying on the agent remembering. Best-effort: a store fault prints a
/// note and the drive continues (the review envelope is the second guard).
///
/// BUG-1527: this write is itself a swap-acceptance hazard — `pr` may have
/// been found on a branch the implementer swapped to mid-phase (BUG-1485's
/// failure mode), which can be a DIFFERENT spec's PR entirely. Gate the flip
/// on the same TASK-1442 trailer-attribution check the PR-open recovery
/// paths already use (`ensure_pr_open_spec_attribution`) so a PR that
/// credits some other spec never marks THIS spec Done. The check runs
/// BEFORE the write, not after, so it can never contradict the BUG-245
/// `shipped_spec_id` mismatch report that follows phase 1 (that report is
/// what surfaces the swap as an anomaly; this function just goes quiet on a
/// mismatch instead of writing a false Done).
// trace:BUG-1186 trace:BUG-1527 | ai:claude
pub(crate) fn ensure_spec_done_after_pr(
    project_root: &std::path::Path,
    repo: &std::path::Path,
    branch: &str,
    spec: &str,
    pr: u32,
    json: bool,
) {
    // TASK-1457: this gate is intentionally coarse — a PR whose commits name
    // a different spec AND a PR whose commits carry no spec-ID trailer at
    // all both skip the Done write here (see the doc comment on
    // `ensure_pr_open_spec_attribution` for why this caller keeps the two
    // collapsed rather than adopting the finer three-way split the
    // branch-swap seam uses).
    if let Err(e) = ensure_pr_open_spec_attribution(repo, branch, spec) {
        if !json {
            eprintln!(
                "  {} PR-{} on `{}` does not credit {} — not marking it Done ({e})",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                pr,
                branch,
                spec,
            );
        }
        return;
    }
    let flipped = (|| -> anyhow::Result<bool> {
        let Some(store_path) = detect_distributed_store_from(project_root) else {
            return Ok(false);
        };
        let dispenser = load_dispenser(&store_path)?;
        let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
        let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;
        // trace:TASK-1468 | ai:claude
        let Some(req) = backend.get_requirement_unambiguous(spec)? else {
            return Ok(false);
        };
        // BUG-1638: the race-test seam between the read and the write.
        // trace:BUG-1638 | ai:claude
        status_write_race_seam(&store_path);
        // BUG-1637: the status check runs on the copy read under the store
        // lock; a refused flip writes nothing. trace:BUG-1637 | ai:claude
        let mut flipped = false;
        backend.update_spec_atomically(&req, |r| flipped = pr_open_done_flip(r))?;
        Ok(flipped)
    })();
    match flipped {
        Ok(true) => {
            if !json {
                eprintln!(
                    "  {} {} was still In Progress after PR-{} opened — marked Done (the \
                     implementer skipped `aida queue done`)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    spec,
                    pr,
                );
            }
        }
        Ok(false) => {}
        Err(e) => {
            if !json {
                eprintln!(
                    "  {} could not assert {} Done after PR-{} ({e}) — the review envelope \
                     still forces the reviewer seat",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    spec,
                    pr,
                );
            }
        }
    }
}

// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy_log_path(
    project_root: &std::path::Path,
    spec: &str,
) -> std::path::PathBuf {
    let safe: String = spec
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    project_root
        .join(".aida")
        .join("headless-logs")
        .join(format!("remedy-{safe}-{ts}.jsonl"))
}

// trace:TASK-1075 | ai:codex
pub(crate) fn questions_remedy_prompt(spec: &str) -> String {
    format!(
        r#"You are AIDA's bounded headless advisor remedy for {spec}.

Goal: make exactly one under-specified requirement drive-ready by adding a minimal, testable `## Acceptance` section when the current substrate is sufficient.

Rules:
- Do not ask the human questions and do not use AskUserQuestion.
- Read the requirement first: `aida show {spec}`.
- Read nearby context if needed: `aida graph tree {spec}` and `aida graph blocked-by {spec}`.
- If the current description, title, comments, or graph context are enough, replace the spec description with the same prose plus a `## Acceptance` section containing crisp observable bullets.
- Preserve all existing description content and existing headings. Do not change status, priority, type, relationships, queue state, code, or unrelated specs.
- Bind the edit with `aida edit {spec} --description "<full updated description>"`. For multi-line text, use a temp file and shell substitution.
- After editing, run `aida zen {spec} --json`. Exit 0 only if the verdict is `ready`.
- If grounded acceptance cannot be inferred, or the gate still holds for a reason you cannot fix by acceptance criteria, leave the spec unchanged if possible and exit non-zero.

This is an advisor-authoring remedy, not an implementation pass."#
    )
}

/// List the decision inbox. Returns the count of pending requests so the
/// bare-invocation caller can decide whether to offer the answer loop.
// trace:STORY-522 | ai:claude
pub(crate) fn questions_list(backend: &aida_core::CachedGitBackend) -> Result<usize> {
    let (pending, answered) = collect_decision_requests(backend)?;
    if pending.is_empty() && answered.is_empty() {
        println!(
            "{}",
            "Decision inbox empty — no questions recorded.".dimmed()
        );
        return Ok(0);
    }
    if !pending.is_empty() {
        println!(
            "{} ({})",
            "Pending decisions".yellow().bold(),
            pending.len()
        );
        for req in &pending {
            if let Some(dr) = &req.decision_request {
                println!();
                print_decision_request(&req.display_id(), &req.title, dr);
            }
        }
    }
    if !answered.is_empty() {
        println!();
        println!("{} ({})", "Answered".green().bold(), answered.len());
        for req in &answered {
            if let Some(dr) = &req.decision_request {
                if let Some(idx) = dr.answered {
                    let label = dr.choices.get(idx).map(|c| c.label.as_str()).unwrap_or("?");
                    println!(
                        "  {:<14} {} → {}",
                        req.display_id().cyan(),
                        req.title.dimmed(),
                        label.bold()
                    );
                    // TASK-791: surface the counter-proposal note so the
                    // implementer reads choice + note.
                    if let Some(note) = &dr.note {
                        println!("  {:<14} {} {}", "", "note:".dimmed(), note);
                    }
                }
            }
        }
    }
    Ok(pending.len())
}

/// `aida questions ask` — pose a DecisionRequest on a spec.
// trace:STORY-522 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn questions_ask(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    question: &str,
    choice_specs: &[String],
    recommend: Option<usize>,
    rationale: Option<&str>,
    force: bool,
) -> Result<()> {
    if choice_specs.len() < 2 {
        anyhow::bail!(
            "a decision needs at least two choices (got {}) — pass --choice \
             `label|consequence|resolution` twice or more",
            choice_specs.len()
        );
    }
    let choices: Vec<aida_core::DecisionChoice> = choice_specs
        .iter()
        .map(|c| parse_decision_choice(c))
        .collect::<Result<_>>()?;

    // recommend is 1-based on the CLI; store 0-based.
    let recommended = match recommend {
        Some(n) => {
            if n == 0 || n > choices.len() {
                anyhow::bail!(
                    "--recommend {n} out of range — there are {} choices (1-{})",
                    choices.len(),
                    choices.len()
                );
            }
            Some(n - 1)
        }
        None => None,
    };

    let mut req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let display_id = req.display_id();

    if let Some(existing) = &req.decision_request {
        if existing.is_pending() && !force {
            anyhow::bail!(
                "{display_id} already has a pending decision — answer it first, \
                 or pass --force to overwrite it"
            );
        }
    }

    let now = chrono::Utc::now();
    let request = aida_core::DecisionRequest {
        question: question.to_string(),
        choices,
        recommended,
        rationale: rationale.map(|s| s.to_string()),
        answered: None,
        note: None,
        asked_at: Some(now),
        answered_at: None,
    };
    req.decision_request = Some(request.clone());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    println!("{} {display_id}", "Decision recorded:".green().bold());
    print_decision_request(&display_id, &req.title, &request);
    println!(
        "{}",
        "Answer it with `aida questions answer` (or `aida questions` at a TTY).".dimmed()
    );
    Ok(())
}

/// Record one answer on a spec's DecisionRequest (the non-interactive,
/// agent-free data op). trace:STORY-522 | ai:claude
// trace:TASK-791 | ai:claude
pub(crate) fn questions_answer_one(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    choice: &str,
    note: Option<&str>,
) -> Result<()> {
    let mut req = backend
        .get_requirement_unambiguous(spec)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let display_id = req.display_id();

    let request = req
        .decision_request
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("{display_id} has no decision to answer"))?;
    if !request.is_pending() {
        anyhow::bail!("{display_id} is already answered");
    }
    let idx = record_answer(request, choice)?;
    // TASK-791: a free-text counter-proposal rides ALONGSIDE the picked option.
    // Pure data: recorded on the request, never interpreted at answer-time.
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        request.note = Some(n.to_string());
    }

    // STORY-555: record was slice 1; now APPLY the resolution + (if it unparks)
    // auto-queue, so answering closes the questions -> burndown loop.
    finalize_answer(backend, store_path, req, idx)
}

/// What the operator chose at the interactive decision prompt. The pure-pick
/// path (`Pick`) stays the default; `PickWithNote` / `Chat` are the TASK-791
/// escapes that mirror /aida-clarify + the AskUserQuestion model.
// trace:TASK-791 | ai:claude
pub(crate) enum DecisionPromptAction {
    /// Skip this question (`s`/`skip`) — leave it pending.
    Skip,
    /// No input + no recommended default — re-prompt / move on without acting.
    NoOp,
    /// The pure-pick path: a 1-based number or `default`/`recommended`.
    Pick(String),
    /// (4) TYPE SOMETHING — a pick plus a free-text counter-proposal note
    /// recorded alongside it (pure data; no LLM at answer-time).
    PickWithNote { choice: String, note: String },
    /// (5) CHAT — drop into the interactive clarifier to discuss before
    /// deciding.
    Chat,
}

/// Render the decision prompt + read one operator action. Shared by the answer
/// loop and the `aida decide` single-spec path so the option list and the two
/// escapes stay identical. trace:TASK-791 | ai:claude
pub(crate) fn prompt_decision_action(
    request: &aida_core::DecisionRequest,
) -> Result<DecisionPromptAction> {
    use std::io::Write;
    let prompt = match request.recommended {
        Some(r) => format!(
            "  Choice [1-{}, Enter={}, s=skip, t=type a note, c=chat]: ",
            request.choices.len(),
            r + 1
        ),
        None => format!(
            "  Choice [1-{}, s=skip, t=type a note, c=chat]: ",
            request.choices.len()
        ),
    };
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let entered = input.trim();

    if entered.eq_ignore_ascii_case("s") || entered.eq_ignore_ascii_case("skip") {
        return Ok(DecisionPromptAction::Skip);
    }
    // (5) CHAT — drop into `aida questions clarify <spec>` to discuss first.
    if entered.eq_ignore_ascii_case("c") || entered.eq_ignore_ascii_case("chat") {
        return Ok(DecisionPromptAction::Chat);
    }
    // (4) TYPE SOMETHING — pick an option, then attach a counter-proposal note.
    // Pure data op: no LLM at answer-time; the implementer reads choice + note.
    if entered.eq_ignore_ascii_case("t")
        || entered.eq_ignore_ascii_case("type")
        || entered.eq_ignore_ascii_case("note")
    {
        let pick = read_line_prompt(&format!(
            "  Which option does the note refine? [1-{}, Enter={}]: ",
            request.choices.len(),
            request
                .recommended
                .map(|r| (r + 1).to_string())
                .unwrap_or_else(|| "?".to_string()),
        ))?;
        let pick = pick.trim();
        let choice = if pick.is_empty() {
            if request.recommended.is_none() {
                println!("  {}", "no default — note needs a choice number".dimmed());
                return Ok(DecisionPromptAction::NoOp);
            }
            "default".to_string()
        } else {
            pick.to_string()
        };
        let note = read_line_prompt("  Your note / counter-proposal: ")?;
        let note = note.trim();
        if note.is_empty() {
            println!("  {}", "empty note — nothing recorded".dimmed());
            return Ok(DecisionPromptAction::NoOp);
        }
        return Ok(DecisionPromptAction::PickWithNote {
            choice,
            note: note.to_string(),
        });
    }

    if entered.is_empty() {
        // Enter accepts the recommended default when one exists.
        if request.recommended.is_none() {
            println!(
                "  {}",
                "no default — type a choice number, t to add a note, c to chat, or s to skip"
                    .dimmed()
            );
            return Ok(DecisionPromptAction::NoOp);
        }
        return Ok(DecisionPromptAction::Pick("default".to_string()));
    }
    Ok(DecisionPromptAction::Pick(entered.to_string()))
}

/// Print a prompt and read one trimmed line. trace:TASK-791 | ai:claude
pub(crate) fn read_line_prompt(prompt: &str) -> Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input)
}

/// Interactive answer loop over every pending DecisionRequest. Reads a
/// choice number (or `s`/`skip`, `t`=type a note, `c`=chat) from stdin per
/// question. trace:STORY-522 | ai:claude
// trace:TASK-791 | ai:claude
pub(crate) fn questions_answer_loop(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let (pending, _) = collect_decision_requests(backend)?;
    if pending.is_empty() {
        println!("{}", "No pending decisions to answer.".dimmed());
        return Ok(());
    }
    let mut answered = 0usize;
    for req in pending {
        let spec = req.display_id();
        let dr = match &req.decision_request {
            Some(dr) if dr.is_pending() => dr,
            _ => continue,
        };
        println!();
        print_decision_request(&spec, &req.title, dr);
        let (choice, note) = match prompt_decision_action(dr)? {
            DecisionPromptAction::Skip => {
                println!("  {}", "skipped".dimmed());
                continue;
            }
            DecisionPromptAction::NoOp => continue,
            DecisionPromptAction::Chat => {
                // (5) CHAT — hand off to the interactive clarifier on this spec,
                // then move on (the discussion may resolve it out-of-band).
                println!(
                    "  {} dropping into clarify — discuss, then re-run `aida questions answer`",
                    "→".green()
                );
                if let Err(e) =
                    questions_clarify(backend, store_path, std::slice::from_ref(&spec), false)
                {
                    println!("  {} {e}", "error:".red());
                }
                continue;
            }
            DecisionPromptAction::Pick(c) => (c, None),
            DecisionPromptAction::PickWithNote { choice, note } => (choice, Some(note)),
        };
        match questions_answer_one(backend, store_path, &spec, &choice, note.as_deref()) {
            Ok(()) => answered += 1,
            Err(e) => println!("  {} {e}", "error:".red()),
        }
    }
    println!();
    println!("{} {answered} answered.", "Done.".green().bold());
    Ok(())
}

/// `aida questions answer --all-defaults` — confirm every recommended
/// default in one shot. trace:STORY-522 | ai:claude
pub(crate) fn questions_answer_all_defaults(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let (pending, _) = collect_decision_requests(backend)?;
    let mut answered = 0usize;
    let mut skipped = 0usize;
    for mut req in pending {
        let request = match req.decision_request.as_mut() {
            Some(r) if r.is_pending() => r,
            _ => continue,
        };
        let Some(idx) = confirm_default(request) else {
            skipped += 1;
            continue;
        };
        // STORY-555: apply the resolution + auto-queue, same as the single and
        // interactive paths.
        finalize_answer(backend, store_path, req, idx)?;
        answered += 1;
    }
    if answered == 0 && skipped == 0 {
        println!("{}", "No pending decisions to answer.".dimmed());
    } else {
        println!(
            "{} {answered} answered{}.",
            "Done.".green().bold(),
            if skipped > 0 {
                format!(", {skipped} skipped (no recommended default)")
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

// BUG-531: `aida search` mirrors `aida list`'s output-mode flags. These tests
// assert the clap surface (flag + aliases parse, mutual exclusion enforced) at
// parse level — no backend / CWD / HOME / git. trace:BUG-531 | ai:claude
#[cfg(test)]
#[path = "tests/search_output_flags_tests.rs"]
mod search_output_flags_tests;

// STORY-522: `aida questions` parser + record/confirm logic. These tests are
// fully isolated — pure functions over in-memory DecisionRequest values, no
// backend / CWD / HOME / git. trace:STORY-522 | ai:claude
#[cfg(test)]
#[path = "tests/questions_tests.rs"]
mod questions_tests;

/// `aida findings add` — file an advisor observation as a `from-advisor:`
/// finding (STORY-467). Mirrors the doc-add minimal path: build the
/// Requirement, allocate a SPEC-ID via `update_atomically`, write the
/// object. Skips the parent / lease / block-dispenser ceremony of
/// `Command::Add` — an observation is unparented by design and the
/// finding cohort doesn't need a short id.
// trace:STORY-467 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_findings_add(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    note: &str,
    kind: &str,
    title: Option<&str>,
    severity: Option<&str>,
    linked_specs: &[String],
    extra_tags: Option<&str>,
) -> Result<()> {
    // `--note -` reads the body from stdin so a multi-paragraph
    // observation doesn't need shell-quoting.
    let note_body = if note == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("reading observation note from stdin")?;
        buf
    } else {
        note.to_string()
    };
    let note_trimmed = note_body.trim();
    if note_trimmed.is_empty() {
        anyhow::bail!(
            "--note is required and must contain text — an empty observation \
             gives the triage view nothing to act on."
        );
    }

    // Title: explicit flag wins; otherwise the first non-empty line of the
    // note, truncated. Keeping the title short keeps `aida findings list`
    // readable.
    let resolved_title = match title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            let first_line = note_trimmed
                .lines()
                .next()
                .unwrap_or("Advisor observation")
                .trim();
            const TITLE_MAX: usize = 80;
            if first_line.chars().count() > TITLE_MAX {
                let truncated: String = first_line.chars().take(TITLE_MAX - 1).collect();
                format!("{truncated}…")
            } else {
                first_line.to_string()
            }
        }
    };

    let mut req = Requirement::new(resolved_title.clone(), note_trimmed.to_string());
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::Draft;
    req.owner = get_default_author();

    // Origin: first linked spec, else `general`. The first spec doubles as
    // both the `from-advisor:<origin>` value AND a `linked:` tag — the
    // grouping key and the relationship signal stay consistent.
    let cleaned_specs: Vec<String> = linked_specs
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let origin = cleaned_specs
        .first()
        .cloned()
        .unwrap_or_else(|| "general".to_string());

    req.tags
        .insert(format!("{}{}", findings::FROM_ADVISOR_PREFIX, origin));
    req.tags
        .insert(format!("{}{}", findings::KIND_PREFIX, kind.trim()));

    if let Some(level) = severity.and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        // Validate against the known severity vocabulary so a typo doesn't
        // silently land as Unknown. trace:TASK-120 — vocabulary extended
        // with observation + note for lighter-weight findings.
        match findings::Severity::parse(level) {
            findings::Severity::Unknown => {
                anyhow::bail!(
                    "unrecognised severity `{}` — expected one of: \
                     major, minor, cosmetic, observation, note",
                    level
                );
            }
            sev => {
                req.tags
                    .insert(format!("{}{}", findings::SEVERITY_PREFIX, sev.label()));
            }
        }
    }

    for spec in &cleaned_specs {
        req.tags
            .insert(format!("{}{}", findings::LINKED_PREFIX, spec));
    }

    if let Some(raw) = extra_tags {
        // BUG-1770: same shared parser as the other `--tags` surfaces.
        // trace:BUG-1770 | ai:claude
        for tag in parse_tag_list(raw)? {
            req.tags.insert(tag);
        }
    }

    // Mirror `aida doc add`'s minimal persistence path — works regardless of
    // id_format policy (node-aware ids drop in when blocks aren't allocated).
    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
    })?;
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("add_requirement_with_id produced no requirement"))?;
    aida_core::object_store::write_object(&store_path.join("objects"), &written)?;

    let display_id = written.spec_id.as_deref().unwrap_or("?");
    record_role_activity(display_id, "findings-add");
    println!("Filed observation {} — {}", display_id, written.title);
    println!(
        "  {}",
        format!(
            "from-advisor:{origin} · kind:{kind} · severity:{sev}",
            sev = severity.unwrap_or("unknown")
        )
        .dimmed()
    );
    println!(
        "  {}",
        "Triage: `aida findings list` · promote with `aida findings promote <ID>` · \
         re-sight with `aida findings recur <ID>`"
            .dimmed()
    );
    Ok(())
}

/// `aida findings recur` — bump the `recurrence:N` counter on an existing
/// finding (STORY-467). The first re-sighting writes `recurrence:2`; each
/// subsequent call increments. An optional `--note` appends a timestamped
/// audit comment so the recurrence trail explains *what* you saw, not just
/// *that* you saw it.
// trace:STORY-467 | ai:claude
pub(crate) fn handle_findings_recur(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    id: &str,
    note: Option<&str>,
) -> Result<()> {
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    if !findings::is_finding(&tags) {
        anyhow::bail!(
            "{id} is not a finding (no `from-review:`/`from-implementer:`/`from-advisor:` tag) — \
             `aida findings recur` only bumps real findings. Re-file as an observation \
             with `aida findings add --note ...` if you meant to capture a new one."
        );
    }

    // Find and bump the existing counter; default to 2 when absent (the
    // first sighting is implicit recurrence:1).
    let current = findings::finding_recurrence(&tags);
    let next = current.saturating_add(1).max(2);
    req.tags
        .retain(|t| !t.starts_with(findings::RECURRENCE_PREFIX));
    req.tags
        .insert(format!("{}{}", findings::RECURRENCE_PREFIX, next));

    let now = chrono::Utc::now();
    let author = get_default_author();
    let comment_body = match note.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => format!(
            "Recurrence #{next} by {author} {date}: {text}",
            date = now.format("%Y-%m-%d")
        ),
        None => format!(
            "Recurrence #{next} by {author} {date}.",
            date = now.format("%Y-%m-%d")
        ),
    };
    req.comments.push(Comment {
        id: Uuid::now_v7(),
        content: comment_body,
        author,
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    });
    req.modified_at = now;
    backend.update_requirement(&req)?;

    let display_id = req.spec_id.as_deref().unwrap_or(id);
    println!("Recurred {} — recurrence count now ×{next}.", display_id);
    // Threshold is configurable via [findings] promote_threshold in
    // .aida/config.toml; default 3. trace:TASK-37 | ai:claude
    // STORY-1428: at the threshold the hint names BOTH destinations (work
    // and gate); a settled gate question is reported, not re-asked.
    // trace:STORY-1428 | ai:claude
    let threshold = findings::promote_threshold_for_project(Some(store_path));
    let bumped: Vec<String> = req.tags.iter().cloned().collect();
    if let Some(hint) = findings::recurrence_gate_hint(&bumped, threshold, display_id) {
        println!("  {}", hint.dimmed());
    }
    Ok(())
}

/// `aida findings promote <ID> --to gate` — the gate-candidate destination
/// (STORY-1428). Records the screening answer on the finding. A `mechanical`
/// or `agent` answer files a gate TASK for the class, linked both ways
/// (`References` edges plus a `gated-by:<ID>` tag); a `none` answer records
/// "stays prose" so later recurrences do not re-open it.
///
/// Authority follows `aida add --queue`: without advisor authority the gate
/// task is filed as Draft, and it is queued only with dispatch authority.
// trace:STORY-1428 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_findings_promote_gate(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    finding: Requirement,
    id: &str,
    detectable: Option<&str>,
    reason: Option<&str>,
    for_role: Option<&str>,
    force: bool,
) -> Result<()> {
    let raw = detectable.ok_or_else(|| {
        anyhow::anyhow!(
            "--to gate needs the screening answer: --detectable mechanical|agent|none \
             (mechanical = recognisable without judgement; agent = only an agent could \
             recognise it; none = stays prose)"
        )
    })?;
    let screen = findings::GateScreen::parse(raw).ok_or_else(|| {
        anyhow::anyhow!("unknown --detectable answer `{raw}` — expected mechanical, agent, or none")
    })?;
    let authority = GateAuthority {
        advisor: has_advisor_authority(),
        dispatch: has_dispatch_authority(),
    };
    let outcome = promote_finding_to_gate(
        backend, store_path, finding, id, screen, reason, for_role, force, authority,
    )?;
    println!("{}", outcome.message());
    Ok(())
}

/// The session's authority, resolved once by the caller so the gate
/// promotion itself is testable.
#[derive(Debug, Clone, Copy)]
// trace:STORY-1428 | ai:claude
pub(crate) struct GateAuthority {
    pub(crate) advisor: bool,
    pub(crate) dispatch: bool,
}

/// What a gate promotion did.
#[derive(Debug, Clone, PartialEq, Eq)]
// trace:STORY-1428 | ai:claude
pub(crate) enum GatePromoteOutcome {
    /// `--detectable none`: the class stays prose; nothing filed.
    StaysProse { finding: String },
    /// A gate task exists for the finding (newly filed or reused).
    Gated {
        finding: String,
        gate: String,
        screen: &'static str,
        reused: bool,
        status: RequirementStatus,
        /// `Some(role)` when queued; `None` when filed but not queued.
        queued_for: Option<String>,
    },
}

impl GatePromoteOutcome {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::StaysProse { finding } => format!(
                "Screened finding {finding} — stays prose (recorded; not re-opened on \
                 future recurrences)."
            ),
            Self::Gated {
                finding,
                gate,
                screen,
                reused,
                status,
                queued_for,
            } => {
                let verb = if *reused { "reused existing" } else { "filed" };
                let tail = match queued_for {
                    Some(role) => format!("queued for {role}"),
                    None => format!(
                        "{status}, filed, not queued — ask the advisor to approve and \
                         queue {gate}"
                    ),
                };
                format!(
                    "Promoted finding {finding} as a gate candidate ({screen}) — {verb} \
                     {gate}, {tail}."
                )
            }
        }
    }
}

/// The body of the gate route, with authority passed in. Idempotent: a
/// finding already `gated-by:` an existing task, or already referenced by a
/// gate-candidate task (a retry after a partial write), reuses that task
/// instead of filing a second one.
#[allow(clippy::too_many_arguments)]
// trace:STORY-1428 | ai:claude
pub(crate) fn promote_finding_to_gate(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    mut finding: Requirement,
    id: &str,
    screen: findings::GateScreen,
    reason: Option<&str>,
    for_role: Option<&str>,
    force: bool,
    authority: GateAuthority,
) -> Result<GatePromoteOutcome> {
    let tags: Vec<String> = finding.tags.iter().cloned().collect();
    let display_id = finding.spec_id.clone().unwrap_or_else(|| id.to_string());
    let threshold = findings::promote_threshold_for_project(store_path.parent());
    let recurrence = findings::finding_recurrence(&tags);
    if recurrence < threshold {
        anyhow::bail!(
            "{display_id} has recurred ×{recurrence}, below the gate threshold ({threshold}) — \
             promote it as work (`aida findings promote {display_id}`), or re-sight it with \
             `aida findings recur {display_id}` when it comes back."
        );
    }
    if let Some(prior) = findings::gate_decision(&tags) {
        if !force {
            anyhow::bail!(
                "{display_id}'s gate question is already settled (`{}{}`) — not re-opened. \
                 Pass --force to screen it again.",
                findings::GATE_DECISION_PREFIX,
                prior.label()
            );
        }
    }

    let now = chrono::Utc::now();
    let author = get_default_author();
    let new_comment = |content: String| Comment {
        id: Uuid::now_v7(),
        content,
        author: author.clone(),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
        relayed_from: None,
    };
    let mut screening = format!(
        "Gate screening by {author} {date} at recurrence ×{recurrence}: {}.",
        screen.screening_summary(),
        date = now.format("%Y-%m-%d")
    );
    if let Some(text) = reason.map(str::trim).filter(|s| !s.is_empty()) {
        screening.push_str(&format!(" Reason: {text}"));
    }

    // Record the screening answer on the finding FIRST, so a crash after
    // this point leaves the decision recorded and a retry needs --force.
    finding
        .tags
        .retain(|t| !t.starts_with(findings::GATE_DECISION_PREFIX));
    finding.tags.insert(format!(
        "{}{}",
        findings::GATE_DECISION_PREFIX,
        screen.label()
    ));

    if !screen.produces_gate() {
        screening.push_str(" Recorded so later recurrences do not re-open the question.");
        finding.comments.push(new_comment(screening));
        finding.modified_at = now;
        backend.update_requirement(&finding)?;
        return Ok(GatePromoteOutcome::StaysProse {
            finding: display_id,
        });
    }
    finding.modified_at = now;
    backend.update_requirement(&finding)?;

    // Find-or-file the gate task. Existing links win: the finding's own
    // `gated-by:` target, else any gate-candidate task that references the
    // finding (a retry after the task was written but before the finding
    // was tagged). Checked inside the atomic write so two racing promotes
    // cannot both file.
    let prior_gate_ref = findings::gated_by(&tags).map(str::to_string);
    let status = if authority.advisor {
        RequirementStatus::Approved
    } else {
        RequirementStatus::Draft
    };
    let mut gate = Requirement::new(
        findings::gate_task_title(&finding.title),
        findings::gate_task_description(&display_id, &finding.title, recurrence, screen),
    );
    gate.req_type = RequirementType::Task;
    gate.status = status.clone();
    gate.owner = author.clone();
    gate.tags.insert(findings::GATE_CANDIDATE_TAG.to_string());
    gate.tags.insert(format!(
        "{}{}",
        findings::GATE_DECISION_PREFIX,
        screen.label()
    ));
    gate.tags.insert("gates".to_string());
    gate.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::References,
        target_id: finding.id,
        created_at: Some(now),
        created_by: Some(author.clone()),
    });
    let finding_uuid = finding.id;
    let mut reused: Option<Requirement> = None;
    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut gate);
    let store = backend.update_atomically(|store| {
        let existing = store
            .requirements
            .iter()
            .find(|r| {
                prior_gate_ref.as_deref().is_some_and(|g| {
                    r.spec_id.as_deref() == Some(g) || r.agreed_id.as_deref() == Some(g)
                })
            })
            .or_else(|| {
                store.requirements.iter().find(|r| {
                    r.tags.contains(findings::GATE_CANDIDATE_TAG)
                        && r.relationships.iter().any(|rel| {
                            rel.target_id == finding_uuid
                                && rel.rel_type == RelationshipType::References
                        })
                })
            })
            .cloned();
        match existing {
            Some(r) => reused = Some(r),
            None => {
                let type_prefix = store.get_type_prefix(&gate.req_type);
                store.add_requirement_with_id(gate.clone(), None, type_prefix.as_deref());
            }
        }
    })?;
    let (written, was_reused) = match reused {
        Some(r) => (r, true),
        None => {
            let w = store.requirements.last().cloned().ok_or_else(|| {
                anyhow::anyhow!("add_requirement_with_id produced no requirement")
            })?;
            aida_core::object_store::write_object(&store_path.join("objects"), &w)?;
            (w, false)
        }
    };
    let gate_id = written
        .spec_id
        .clone()
        .unwrap_or_else(|| written.id.to_string());

    // Link the finding to the gate. The finding's own status is left as-is:
    // the gate task is the queued work, and an Approved finding outside any
    // queue would be a silent half-state (BUG-231).
    let mut fresh = backend
        .get_requirement_unambiguous(&display_id)?
        .unwrap_or(finding.clone());
    fresh.tags = finding.tags.clone();
    fresh
        .tags
        .retain(|t| !t.starts_with(findings::GATED_BY_PREFIX));
    fresh
        .tags
        .insert(format!("{}{}", findings::GATED_BY_PREFIX, gate_id));
    if !fresh
        .relationships
        .iter()
        .any(|r| r.target_id == written.id && r.rel_type == RelationshipType::References)
    {
        fresh.relationships.push(aida_core::models::Relationship {
            rel_type: RelationshipType::References,
            target_id: written.id,
            created_at: Some(now),
            created_by: Some(author.clone()),
        });
    }
    let verb = if was_reused { "reused" } else { "filed" };
    screening.push_str(&format!(" Gate task {verb}: {gate_id}."));
    fresh.comments.push(new_comment(screening));
    fresh.modified_at = now;
    backend.update_requirement(&fresh)?;

    // Queue only with dispatch authority and an enqueueable status — the
    // same gate `aida add --queue` applies. A reused task keeps its status.
    let route = for_role.unwrap_or("implementer");
    let queued_for = if gate_should_queue(&written.status, !authority.advisor, route, authority) {
        let role = queue_spec_for_role(
            store_path,
            written.id,
            route,
            format!("Gate task for finding {display_id} via `aida findings promote --to gate`"),
        )
        .with_context(|| {
            format!(
                "{gate_id} was filed but not queued — queue it with \
                 `aida queue add {gate_id} --for {route}`"
            )
        })?;
        record_role_activity(&gate_id, "queue-add");
        Some(role)
    } else {
        None
    };
    Ok(GatePromoteOutcome::Gated {
        finding: display_id,
        gate: gate_id,
        screen: screen.label(),
        reused: was_reused,
        status: written.status,
        queued_for,
    })
}

/// Whether the gate task may be queued: the `aida add --queue` refusal rule
/// (`queue_at_filing_refusal`) plus dispatch authority for execution routes.
// trace:STORY-1428 | ai:claude
pub(crate) fn gate_should_queue(
    status: &RequirementStatus,
    downgraded: bool,
    route: &str,
    authority: GateAuthority,
) -> bool {
    if queue_at_filing_refusal(status, downgraded, Some(route)).is_some() {
        return false;
    }
    authority.dispatch || !for_target_requires_dispatch_authority(Some(route))
}

/// TASK-516: the tag a spec carries while its imported plan is awaiting
/// master review. `aida import-plan --request-review` stamps it; the
/// master removes it (`aida edit <SPEC> --remove-tag plan-review:pending`)
/// once the plan is approved. `aida queue work` warns while it's present.
// trace:TASK-516 | ai:claude
pub(crate) const PLAN_REVIEW_PENDING_TAG: &str = "plan-review:pending";

/// TASK-516: the pure warn-decision for `aida queue work`. Returns the
/// warning message when the spec's tag set still carries
/// `plan-review:pending` — i.e. an imported plan is awaiting master review
/// and should NOT yet be treated as canonical for implementer pickup.
/// Fully isolated: no I/O, no store, no globals — just the tag set in, an
/// `Option<String>` out, so the decision is unit-testable in isolation.
// trace:TASK-516 | ai:claude
pub(crate) fn plan_review_warning(
    tags: &std::collections::HashSet<String>,
    spec_display: &str,
) -> Option<String> {
    if tags.contains(PLAN_REVIEW_PENDING_TAG) {
        Some(format!(
            "{} {}'s plan is still awaiting master review (tagged `{}`). \
             The archived plan is NOT yet canonical — the master hasn't \
             signed off. Proceeding will treat an unreviewed plan as the \
             implementation brief. Once reviewed, the master clears the tag \
             with `aida edit {} --remove-tag {}`.",
            "Plan-review pending:".yellow().bold(),
            spec_display,
            PLAN_REVIEW_PENDING_TAG,
            spec_display,
            PLAN_REVIEW_PENDING_TAG,
        ))
    } else {
        None
    }
}

/// `aida advisor` — manage the live-advisor registration the
/// `--no-human=both` orchestrator reads (STORY-360). Three actions:
///   - `register` writes the current session's UUID + project slug to
///     `~/.aida/advisor.toml`. Auto-detects from `CLAUDE_CODE_SESSION_ID`.
///   - `unregister` clears the file.
///   - `status` prints what's registered and an estimated $/fork.
///     trace:STORY-360 | ai:claude
pub(crate) fn handle_advisor_command(cmd: &AdvisorCommand) -> Result<()> {
    match cmd {
        AdvisorCommand::Register { uuid, project_slug } => {
            handle_advisor_register(uuid.as_deref(), project_slug.as_deref())
        }
        AdvisorCommand::Unregister => {
            advisor::clear_registration()?;
            println!("Advisor registration cleared.");
            Ok(())
        }
        // STORY-586: presence-gated fork-from-live watch loop. Store-less —
        // self-loads advisor config + presence. trace:STORY-586 | ai:claude
        AdvisorCommand::Watch {
            dry_run,
            once,
            triage_only,
            poll_interval,
            fork_interval,
        } => {
            let project_root = main_worktree_root_from(&find_project_root()?);
            advisor_watch::run_advisor_watch(
                &project_root,
                &advisor_watch::WatchOpts {
                    poll_interval_secs: *poll_interval,
                    fork_interval_secs: *fork_interval,
                    dry_run: *dry_run,
                    once: *once,
                    triage_only: *triage_only,
                },
            )
        }
        // STORY-559: only the narrow `--registration` view reaches this
        // store-less early handler; the default dashboard dispatches after
        // storage init. trace:STORY-559 | ai:claude
        AdvisorCommand::Status { json, .. } => handle_advisor_registration_status(*json),
        // STORY-262: `schedule` needs the requirement store, so it's routed
        // through the post-storage-init dispatch instead of this store-less
        // early handler. trace:STORY-262 | ai:claude
        AdvisorCommand::Schedule(_) => {
            unreachable!("advisor schedule dispatches after storage init")
        }
        // STORY-363: `handoff` writes a checked-in brief into a sibling
        // project. It reads no requirement store — parent identity is derived
        // from the current project root and the rest is an operator-authored
        // template — so it dispatches in this store-less early handler.
        // trace:STORY-363 | ai:claude
        AdvisorCommand::Handoff { to, focus, force } => handle_advisor_handoff(to, focus, *force),
    }
}

/// STORY-363 (SPIKE-10 Track B): write a checked-in advisor handoff brief into
/// a sibling project. The brief lands at `<to>/docs/<date>-advisor-handoff.md`
/// and carries five sections — parent identity (auto-derived from the current
/// project root), vision, decided things, substrate slice (scoped by
/// `--focus`), and latitude. Only parent identity and the focus topic are
/// filled in; the strategic sections are placeholders the operator authors
/// before committing the brief to the child project. Bounded by design: this
/// is a template generator, not a context-extraction pipeline.
// trace:STORY-363 | ai:claude
pub(crate) fn handle_advisor_handoff(to: &std::path::Path, focus: &str, force: bool) -> Result<()> {
    let focus = focus.trim();
    if focus.is_empty() {
        anyhow::bail!("--focus cannot be empty — name the topic this handoff hands off");
    }
    if !to.exists() {
        anyhow::bail!(
            "target project does not exist: {} — pass an existing project path with --to",
            to.display()
        );
    }
    // Parent identity is auto-derived from the current project root (falling
    // back to the cwd when not inside an AIDA project — the handoff is still
    // useful, just with a thinner identity line).
    let parent_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let parent_name = parent_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let authored_by = brief_generated_by();
    let body = render_advisor_handoff(&parent_name, &parent_root, focus, &date, &authored_by);

    let docs_dir = to.join("docs");
    std::fs::create_dir_all(&docs_dir)
        .with_context(|| format!("failed to create {}", docs_dir.display()))?;
    let path = docs_dir.join(format!("{date}-advisor-handoff.md"));

    if path.exists() && !force {
        anyhow::bail!(
            "a handoff brief for today already exists: {} (pass --force to overwrite)",
            path.display()
        );
    }
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;

    println!(
        "{} {}",
        "Wrote advisor handoff:".green().bold(),
        path.display()
    );
    println!(
        "  Fill in the vision / decided-things / latitude sections, then commit it to {}.",
        parent_name.cyan()
    );
    Ok(())
}

/// Pure assembly of the advisor-handoff brief body — no I/O, fully testable in
/// isolation. Five sections per SPIKE-10 Gap 2: parent identity (auto), vision
/// (operator), decided things (operator-pruned), substrate slice (scoped by
/// `--focus`), and latitude (operator). The strategic sections ship as
/// placeholders the operator authors before committing.
// trace:STORY-363 | ai:claude
pub(crate) fn render_advisor_handoff(
    parent_name: &str,
    parent_root: &std::path::Path,
    focus: &str,
    date: &str,
    authored_by: &str,
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("kind: {}\n", yaml_scalar("advisor-handoff")));
    out.push_str(&format!("parent: {}\n", yaml_scalar(parent_name)));
    out.push_str(&format!("focus: {}\n", yaml_scalar(focus)));
    out.push_str(&format!("date: {}\n", yaml_scalar(date)));
    out.push_str(&format!("authored_by: {}\n", yaml_scalar(authored_by)));
    out.push_str("status: draft\n");
    out.push_str("---\n\n");

    out.push_str(&format!("# Advisor handoff — {focus}\n\n"));
    out.push_str(&format!(
        "Generated {date} from **{parent_name}**. This brief hands off the advisor \
         context for **{focus}** to a sibling project. Parent identity and the focus \
         topic are filled in; author the remaining sections before committing.\n\n"
    ));

    out.push_str("## 1. Parent identity (auto)\n\n");
    out.push_str(&format!("- Parent project: **{parent_name}**\n"));
    out.push_str(&format!("- Parent root: `{}`\n", parent_root.display()));
    out.push_str(&format!("- Handoff focus: **{focus}**\n"));
    out.push_str(&format!("- Generated: {date}\n\n"));

    out.push_str("## 2. Vision (operator)\n\n");
    out.push_str(
        "_Why does this child project exist, and how does it relate to the parent's \
         mission? What does success look like 3-6 months out?_\n\n",
    );
    out.push_str("- TODO: state the vision in the operator's own words.\n\n");

    out.push_str("## 3. Decided things (operator-pruned)\n\n");
    out.push_str(
        "_Decisions the child should inherit rather than re-litigate — architecture \
         choices, conventions, tooling, non-goals. Prune anything that doesn't carry \
         over._\n\n",
    );
    out.push_str("- TODO: list the load-bearing decisions the child inherits.\n\n");

    out.push_str(&format!("## 4. Substrate slice — {focus}\n\n"));
    out.push_str(&format!(
        "_The slice of the parent's substrate (specs, docs, conventions, code) that is \
         relevant to **{focus}**. Point at concrete artifacts; do not paste them._\n\n"
    ));
    out.push_str("- TODO: link the specs / docs / modules that scope this focus.\n\n");

    out.push_str("## 5. Latitude (operator)\n\n");
    out.push_str(
        "_Where the child has freedom to diverge, and where it must stay aligned with \
         the parent. Name the guardrails and the open questions explicitly._\n\n",
    );
    out.push_str("- TODO: state the latitude and the guardrails.\n");

    out
}

#[cfg(test)]
#[path = "tests/story_363_handoff_tests.rs"]
mod story_363_handoff_tests;

pub(crate) fn handle_advisor_register(
    uuid: Option<&str>,
    project_slug: Option<&str>,
) -> Result<()> {
    let uuid = uuid
        .map(|s| s.to_string())
        .or_else(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no session UUID — pass `--uuid <id>` or run from a Claude session that \
                 exports CLAUDE_CODE_SESSION_ID"
            )
        })?;

    let cwd = std::env::current_dir().context("cannot read current directory")?;
    let project_slug = project_slug
        .map(|s| s.to_string())
        .unwrap_or_else(|| process_probe::encode_cwd_for_projects(&cwd));

    // Best-effort: record the ancestor `claude` PID so the liveness check has
    // something more authoritative than JSONL mtime.
    let claude_pid = process_probe::probe_live_claude_sessions()
        .into_iter()
        .find_map(|s| {
            // Match either by recent-jsonl uuid or by cwd containment so we
            // don't pick a stranger's claude.
            if s.jsonl
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|f| f.to_string_lossy() == uuid)
                .unwrap_or(false)
            {
                Some(s.pid)
            } else {
                None
            }
        });

    let reg = advisor::AdvisorRegistration {
        uuid: uuid.clone(),
        project_slug: project_slug.clone(),
        project_root: cwd.to_string_lossy().to_string(),
        registered_at: chrono::Utc::now().to_rfc3339(),
        claude_pid,
    };
    advisor::write_registration(&reg)?;

    println!("Registered live advisor session.");
    println!("  uuid:          {}", uuid);
    println!("  project slug:  {}", project_slug);
    println!("  project root:  {}", cwd.display());
    if let Some(pid) = claude_pid {
        println!("  claude pid:    {}", pid);
    } else {
        println!("  claude pid:    (none detected — liveness will use JSONL mtime)");
    }
    println!("\nThe `--no-human=both` orchestrator will now fork this session for advisor punts.");
    println!("Run `aida advisor unregister` to revert to cold-boot.");
    Ok(())
}

/// STORY-559: the narrow live-advisor registration block — the pre-dashboard
/// `aida advisor status` output, now reachable via `--registration` (and the
/// dashboard's "Live advisor" section). Reads only `~/.aida/advisor.toml` +
/// `.aida/config.toml [advisor]`; no requirement store. trace:STORY-559 | ai:claude
pub(crate) fn handle_advisor_registration_status(json: bool) -> Result<()> {
    let reg = advisor::read_registration();
    let cfg_root = find_project_root().ok();
    let cfg = match cfg_root.as_deref() {
        Some(root) => advisor::AdvisorConfig::load(root),
        None => advisor::AdvisorConfig::default(),
    };

    if json {
        let value = match &reg {
            Some(r) => {
                let advisor = locate_for_status(r);
                let (alive, size_bytes, est_cost) = match &advisor {
                    Some(a) => (
                        advisor::is_alive(a, r.claude_pid),
                        Some(a.jsonl_size_bytes),
                        Some(advisor::estimated_fork_cost_usd(a.jsonl_size_bytes)),
                    ),
                    None => (false, None, None),
                };
                serde_json::json!({
                    "registered": true,
                    "uuid": r.uuid,
                    "project_slug": r.project_slug,
                    "project_root": r.project_root,
                    "registered_at": r.registered_at,
                    "claude_pid": r.claude_pid,
                    "alive": alive,
                    "jsonl_size_bytes": size_bytes,
                    "estimated_fork_cost_usd": est_cost,
                    "fork_mode": match cfg.fork_mode {
                        advisor::ForkMode::Auto => "auto",
                        advisor::ForkMode::Always => "always",
                        advisor::ForkMode::Never => "never",
                    },
                })
            }
            None => serde_json::json!({
                "registered": false,
                "fork_mode": match cfg.fork_mode {
                    advisor::ForkMode::Auto => "auto",
                    advisor::ForkMode::Always => "always",
                    advisor::ForkMode::Never => "never",
                },
            }),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    match reg {
        Some(r) => {
            println!("Advisor registration");
            println!("  uuid:          {}", r.uuid);
            println!("  project slug:  {}", r.project_slug);
            println!("  project root:  {}", r.project_root);
            println!("  registered at: {}", r.registered_at);
            if let Some(pid) = r.claude_pid {
                println!("  claude pid:    {}", pid);
            }
            let advisor = locate_for_status(&r);
            match advisor {
                Some(a) => {
                    let alive = advisor::is_alive(&a, r.claude_pid);
                    let mb = a.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
                    let cost = advisor::estimated_fork_cost_usd(a.jsonl_size_bytes);
                    println!(
                        "  source jsonl:  {} ({:.2} MB)",
                        a.source_jsonl.display(),
                        mb
                    );
                    println!(
                        "  alive:         {}",
                        if alive {
                            "yes"
                        } else {
                            "no (cold-boot fallback)"
                        }
                    );
                    println!(
                        "  per-fork cost: ~${:.2} (first fork; subsequent within cache TTL ~$0.03)",
                        cost
                    );
                }
                None => {
                    println!("  source jsonl:  (missing — JSONL not found on disk)");
                    println!("  alive:         no (cold-boot fallback)");
                }
            }
        }
        None => {
            println!("No live advisor registered.");
            println!("The orchestrator will cold-boot the headless advisor for every punt.");
            println!("Run `aida advisor register` from inside your live advisor session to enable forking.");
        }
    }
    println!(
        "  fork_mode:     {} (set in .aida/config.toml [advisor])",
        match cfg.fork_mode {
            advisor::ForkMode::Auto => "auto",
            advisor::ForkMode::Always => "always",
            advisor::ForkMode::Never => "never",
        }
    );
    Ok(())
}

/// STORY-559: the advisor's read-only situational dashboard — the default
/// `aida advisor status`. PURE AGGREGATION of existing surfaces: every count
/// reuses the same query the dedicated command runs (no reimplemented counts),
/// and every row points at the canonical command to act on it. Read-only — no
/// writes, no auto-queue, no auto-approve. The narrow registration view (the
/// pre-dashboard output) lives behind `--registration`.
// trace:STORY-559 | ai:claude
pub(crate) fn handle_advisor_dashboard(
    json: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // --- Live advisor (the former narrow output, now one section) ----------
    let reg = advisor::read_registration();
    let cfg = advisor::AdvisorConfig::load(&project_root);
    let fork_mode_label = match cfg.fork_mode {
        advisor::ForkMode::Auto => "auto",
        advisor::ForkMode::Always => "always",
        advisor::ForkMode::Never => "never",
    };
    let live = reg.as_ref().and_then(|r| {
        let a = locate_for_status(r)?;
        let alive = advisor::is_alive(&a, r.claude_pid);
        let cost = advisor::estimated_fork_cost_usd(a.jsonl_size_bytes);
        Some((a, alive, cost))
    });

    // --- Intake: drafts to triage (same status filter /aida-triage uses) ---
    // Reuses the cache-backed status-filter query — no reimplemented count.
    // The cache excludes archived rows by default.
    // BUG-593: exclude META + the other standing-artifact types the SAME way
    // `aida status` does (BUG-464 / TASK-773) — a fresh `aida init` seeds 6 META
    // prompts that are NOT intake work. The status draft-count already strips
    // them; the advisor surface must agree or it over-reports the inbox.
    // trace:BUG-593 | ai:claude
    let drafts = backend
        .list_summaries(&aida_core::ListFilter {
            status: Some("draft".to_string()),
            ..Default::default()
        })
        .map(|s| {
            s.iter()
                .filter(|r| !is_standing_artifact_type(&r.req_type))
                .count()
        })
        .unwrap_or(0);

    // --- Decisions: pending questions (reuses `aida questions` query) ------
    let pending_decisions = collect_decision_requests(backend)
        .map(|(pending, _)| pending.len())
        .unwrap_or(0);

    // --- Findings awaiting triage (reuses `aida findings list` query) ------
    // findings = draft requirements carrying a finding tag; plus the
    // NeedsAttention cohort (punts + shelvings), exactly as findings list sums.
    let findings_filter = aida_core::ListFilter {
        status: Some("draft".to_string()),
        ..Default::default()
    };
    let findings_total = backend
        .list_summaries(&findings_filter)
        .map(|s| {
            let sections = findings::build_findings_view(&s, &findings::FindingsFilter::default());
            findings::count_findings(&sections)
        })
        .unwrap_or(0);
    let needs_attention = backend
        .list_summaries(&aida_core::ListFilter {
            status: Some("needs-attention".to_string()),
            ..Default::default()
        })
        .map(|s| s.len())
        .unwrap_or(0);
    let findings_awaiting = findings_total + needs_attention;

    // --- Backlog: approved/planned/draft not queued (reuses `aida backlog`)
    let storage = Storage::new(store_path.to_path_buf());
    let user_id = current_user_id(None);
    let queued = queued_requirement_ids(&storage, &user_id).unwrap_or_default();
    let store = storage.load()?;
    let backlog = store
        .requirements
        .iter()
        .filter(|r| {
            !r.archived && r.status == RequirementStatus::Approved && !queued.contains(&r.id)
        })
        .count();

    // --- Burndown readiness (reuses the exact `aida burndown plan` resolver)
    let (ready, awaiting_signoff, _serialize_held, parked, _supervised, _titles) =
        resolve_burndown_sets("approved", None, None, None).unwrap_or_else(|_| {
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Default::default(),
            )
        });

    // --- Queue depth + in-flight (Done-awaiting-merge) ---------------------
    let queue_depth = backend
        .queue_list(&user_id, false)
        .map(|q| q.len())
        .unwrap_or(0);
    let in_flight_specs = store
        .requirements
        .iter()
        .filter(|r| matches!(r.status, RequirementStatus::Done))
        .count();

    // --- Live sessions: leases, flagging any held on a closed spec --------
    let leases = list_leases(&project_root);
    // Map each lease scope to a requirement status (best-effort: scope is a
    // raw string, often a SPEC-ID). Flag leases on archived/rejected/completed
    // specs — catches BUG-492-style states where work is held on dead specs.
    let stale_lease = |scope: &str| -> Option<String> {
        let scope = scope.trim();
        let r = store.requirements.iter().find(|r| {
            r.spec_id.as_deref() == Some(scope)
                || r.agreed_id.as_deref() == Some(scope)
                || r.display_id() == scope
        })?;
        if r.archived {
            return Some("archived".to_string());
        }
        match r.status {
            RequirementStatus::Rejected => Some("rejected".to_string()),
            RequirementStatus::Completed => Some("completed".to_string()),
            _ => None,
        }
    };

    if json {
        let leases_json: Vec<serde_json::Value> = leases
            .iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id,
                    "scope": l.scope,
                    "owner": l.owner,
                    "branch": l.branch,
                    "stale_spec_state": stale_lease(&l.scope),
                })
            })
            .collect();
        let live_json = match &live {
            Some((a, alive, cost)) => serde_json::json!({
                "registered": true,
                "alive": alive,
                "jsonl_size_bytes": a.jsonl_size_bytes,
                "estimated_fork_cost_usd": cost,
            }),
            None => serde_json::json!({
                "registered": reg.is_some(),
                "alive": false,
            }),
        };
        let value = serde_json::json!({
            "live_advisor": live_json,
            "fork_mode": fork_mode_label,
            "intake_drafts": drafts,
            "pending_decisions": pending_decisions,
            "findings_awaiting_triage": findings_awaiting,
            "backlog_not_queued": backlog,
            "burndown": {
                "ready": ready.len(),
                "awaiting_signoff": awaiting_signoff.len(),
                "parked": parked.len(),
            },
            "queue_depth": queue_depth,
            "in_flight": in_flight_specs,
            "live_sessions": leases_json,
        });
        println!("{}", crate::cache_output::json_pretty(&value)?);
        return Ok(());
    }

    println!("{}", "Advisor dashboard".bold());
    println!();

    // Live advisor
    println!("{}", "Live advisor".cyan().bold());
    match (&reg, &live) {
        (Some(_), Some((a, alive, cost))) => {
            let mb = a.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
            println!(
                "  registered, {} — {} ({:.2} MB, ~${:.2}/fork)",
                if *alive {
                    "alive (fork-ready)".green().to_string()
                } else {
                    "not alive (cold-boot fallback)".yellow().to_string()
                },
                a.source_jsonl.display(),
                mb,
                cost
            );
        }
        (Some(_), None) => {
            println!(
                "  registered, but {} — source JSONL not found on disk",
                "cold-boot fallback".yellow()
            );
        }
        (None, _) => {
            println!(
                "  {} — orchestrator cold-boots every punt. Enable forking: {}",
                "no live advisor registered".dimmed(),
                "aida advisor register".cyan()
            );
        }
    }
    println!("  fork_mode: {fork_mode_label} (set in .aida/config.toml [advisor])");
    println!(
        "  Registration detail: {}",
        "aida advisor status --registration".dimmed()
    );
    println!();

    // Each remaining row: count + the command to act.
    println!("{}", "Intake".cyan().bold());
    println!(
        "  {drafts} draft(s) to triage → {}",
        "/aida-triage".dimmed()
    );
    println!();

    println!("{}", "Decisions".cyan().bold());
    println!(
        "  {pending_decisions} pending question(s) → {}",
        "aida questions".dimmed()
    );
    println!();

    println!("{}", "Findings".cyan().bold());
    println!(
        "  {findings_awaiting} awaiting triage (incl. shelved / needs-attention) → {}",
        "aida findings list".dimmed()
    );
    println!();

    println!("{}", "Backlog".cyan().bold());
    println!(
        "  {backlog} approved-not-queued → {} / {}",
        "aida backlog list".dimmed(),
        "groom".dimmed()
    );
    println!();

    println!("{}", "Burndown readiness".cyan().bold());
    println!(
        "  {} ready · {} awaiting sign-off · {} parked → {}",
        ready.len(),
        awaiting_signoff.len(),
        parked.len(),
        "aida burndown plan".dimmed()
    );
    println!();

    println!("{}", "Queue".cyan().bold());
    println!(
        "  depth {queue_depth} · {in_flight_specs} in-flight (Done — awaiting merge) → {}",
        "aida queue list".dimmed()
    );
    println!();

    println!("{}", "Live sessions".cyan().bold());
    if leases.is_empty() {
        println!(
            "  (no active sessions) → {}",
            "aida session leases".dimmed()
        );
    } else {
        for l in &leases {
            match stale_lease(&l.scope) {
                Some(state) => println!(
                    "  {} {} ({}) — {}",
                    l.scope,
                    l.owner.dimmed(),
                    l.branch.dimmed(),
                    format!(
                        "{} spec {state}",
                        crate::glyph(crate::glyphs::Glyph::Warning)
                    )
                    .yellow()
                ),
                None => println!("  {} {} ({})", l.scope, l.owner.dimmed(), l.branch.dimmed()),
            }
        }
        println!("  → {}", "aida session leases".dimmed());
    }

    Ok(())
}

pub(crate) fn locate_for_status(
    reg: &advisor::AdvisorRegistration,
) -> Option<advisor::LiveAdvisor> {
    let home = crate::home_dir()?;
    let jsonl = home
        .join(".claude")
        .join("projects")
        .join(&reg.project_slug)
        .join(format!("{}.jsonl", reg.uuid));
    let meta = std::fs::metadata(&jsonl).ok()?;
    Some(advisor::LiveAdvisor {
        uuid: reg.uuid.clone(),
        project_slug: reg.project_slug.clone(),
        source_jsonl: jsonl,
        jsonl_size_bytes: meta.len(),
        discovery: advisor::Discovery::Registration,
    })
}

/// `aida punt` — pause a spec in `NeedsAttention` with a structured reason
/// instead of guessing past a design-fork (STORY-332). The transition is
/// enforced (`InProgress → NeedsAttention` only); a punt record is appended
/// to the ledger; control returns immediately so an orchestrator advances.
/// trace:STORY-332 | ai:claude
pub(crate) fn handle_punt_command(
    id: &str,
    category: &str,
    reason: &str,
    lean: Option<&str>,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let category = punt::parse_punt_category(category).map_err(|e| anyhow::anyhow!(e))?;

    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());

    // STORY-332: a spec can only be punted out of In Progress — punting is
    // the "I was working this and hit a fork" move. This is the same edge
    // `forbidden_attention_transition` guards for `aida edit`, asserted here
    // directly so the error is punt-shaped.
    if forbidden_attention_transition(&req.status, &RequirementStatus::NeedsAttention).is_some() {
        anyhow::bail!(
            "{display_id} is {} — only an In Progress spec can be punted \
             (punting is the \"I was working this and hit a fork\" move). \
             Take it to In Progress first if you mean to pause it.",
            req.status
        );
    }

    let raised_by = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|r| !r.is_empty());
    let now = chrono::Utc::now();

    // BUG-1637: the CLI punt is caller-authored, recorded under the caller.
    // trace:BUG-1637 | ai:claude
    let from = std::mem::replace(&mut req.status, RequirementStatus::NeedsAttention);
    record_caller_status_transition(&mut req, &from);
    req.attention_reason = Some(AttentionReason {
        category,
        detail: reason.to_string(),
        lean: lean.map(|s| s.to_string()),
        raised_by: raised_by.clone(),
        raised_at: now,
    });
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "punt");

    // Append the punt ledger record (`.aida/punts.jsonl`). Best-effort — a
    // ledger-write failure must not undo the status flip, which is the
    // load-bearing part; warn and continue. trace:STORY-325 | ai:claude
    if let Ok(project_root) = find_project_root() {
        let record = punt::PuntRecord {
            timestamp: now,
            spec: display_id.clone(),
            category,
            detail: reason.to_string(),
            lean: lean.map(|s| s.to_string()),
            raised_by: raised_by.clone(),
            resolution_path: "punted".to_string(),
            // STORY-306 advisor fields — a plain implementer punt the advisor
            // has not yet judged; the orchestrator's advisor tier fills these.
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: Some(now),
            resolved_at: None,
        };
        if let Err(e) = punt::append_to_ledger(&project_root, &record) {
            eprintln!(
                "{} could not write punt ledger record: {e}",
                "Note:".dimmed()
            );
        }
    }

    // STORY-276: when an `--auto-complete` orchestrator launched this session
    // it set `AIDA_PUNT_SIGNAL_FILE` to a path it watches. Drop the signal
    // there so the orchestrator learns the spec was punted — not shipped —
    // once the implementer session exits. Best-effort: a missing var means a
    // standalone `aida punt` with no orchestrator, and a write failure must
    // not undo the status flip. trace:STORY-276 | ai:claude
    if let Some(signal_path) = std::env::var(punt::SIGNAL_FILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
    {
        let signal = punt::PuntSignal {
            spec: display_id.clone(),
            category,
            detail: reason.to_string(),
            lean: lean.map(|s| s.to_string()),
        };
        if let Err(e) = punt::write_signal(std::path::Path::new(&signal_path), &signal) {
            eprintln!("{} could not write punt signal file: {e}", "Note:".dimmed());
        }
    }

    println!(
        "{} {display_id} → {}",
        "Punted".magenta().bold(),
        "Needs Attention".magenta().bold()
    );
    println!("  {}  {category}", "Category:".dimmed());
    println!("  {}    {reason}", "Reason:".dimmed());
    if let Some(l) = lean {
        println!("  {}      {l}", "Lean:".dimmed());
    }
    println!(
        "{}",
        "The spec is paused awaiting triage. An orchestrator can continue to \
         the next item."
            .dimmed()
    );
    println!(
        "{}",
        format!(
            "Triage: `aida findings` lists it · resume with \
             `aida edit {display_id} --status in-progress` · \
             drop with `--status rejected`."
        )
        .dimmed()
    );

    // TASK-349: a blocked-dependency punt names the work it's blocked on — that
    // dependency belongs in the requirement graph as a blocked-by edge, not
    // just in the punt prose. If the reason/lean text named blocker spec(s),
    // nudge the operator to record the edge. A suggestion, not an auto-file:
    // id-parsing from free text is best-effort, so the operator confirms.
    // trace:TASK-349 | ai:claude
    if let Some(suggestion) = punt::suggest_blocked_by(&display_id, category, reason, lean) {
        let blockers = suggestion.blockers.join(", ");
        println!(
            "{} Looks blocked on {blockers} — record it in the graph so \
             `aida graph blocked-by {display_id}` sees the dependency:",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        println!("  {}", suggestion.suggested_command().cyan());
    }

    Ok(())
}

/// Send a best-effort notification message into `recipient`'s mailbox (the
/// fast local layer; STORY-643 auto-sync propagates it to other clones on the
/// next pull/push). Reuses the existing message-send path — no new notification
/// system. Failures are swallowed with a dimmed warning so a mailbox problem
/// never breaks the verb that triggered the notice (assign / comment).
/// trace:STORY-644 | ai:claude
pub(crate) fn send_notification(
    store_path: &std::path::Path,
    sender: &str,
    recipient: &str,
    body: String,
) {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let project_root = match store_path.parent() {
        Some(p) => p,
        None => return,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let msg = Message {
        subject: None,
        id: id.clone(),
        thread_id: id,
        from: sender.to_string(),
        to: Recipient::Agent(recipient.to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        // The caller names `sender` explicitly (a fixed system/CLI identity,
        // e.g. "web", "aida-session-reap"), not an ambiguous env fallback.
        // trace:BUG-1533 | ai:claude
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    if let Err(e) = mailbox_store::write_message(project_root, &msg) {
        eprintln!(
            "  {}",
            format!("(could not send mailbox notice to {recipient}: {e})").dimmed()
        );
    }
}

/// Extract distinct `@mention` handles from free text (STORY-644). Conservative
/// word-boundary parse: a `@` that is NOT preceded by a word char (so `foo@bar`
/// email locals and `a@b` are ignored) followed by `[A-Za-z0-9_.-]+`. The
/// trailing run is trimmed of `.` and `-` so sentence punctuation (`@bob.`) and
/// hyphen-tails don't leak into the handle. Returns handles in first-seen order,
/// deduped. trace:STORY-644 | ai:claude
pub(crate) fn extract_mentions(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '@' {
            // Word-boundary: skip `foo@bar` (email local-part / handle-in-word).
            let prev_is_word = i > 0 && {
                let p = chars[i - 1];
                p.is_alphanumeric() || p == '_'
            };
            if !prev_is_word {
                let mut j = i + 1;
                while j < chars.len() {
                    let c = chars[j];
                    if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let raw: String = chars[i + 1..j].iter().collect();
                // Trim trailing sentence punctuation so `@bob.` -> `bob`.
                let handle = raw.trim_end_matches(['.', '-']).to_string();
                if !handle.is_empty() && !out.contains(&handle) {
                    out.push(handle);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// One-line snippet of `text` for a mention notice: collapsed whitespace,
/// truncated to `max` chars with an ellipsis. trace:STORY-644 | ai:claude
pub(crate) fn mention_snippet(text: &str, max: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// `aida done <SPEC>` — the newcomer's "I finished it" verb. Marks a spec
/// Completed, the solo end of the capture → build → done loop. Found running a
/// novice's first session: there was no `aida done`, and `aida edit --status
/// completed` is jargon (and authority-gated off a TTY). A human at a terminal
/// IS the authority (TTY satisfies the advisor gate), so for a solo user this
/// just works. trace:TASK-727 | ai:claude
pub(crate) fn handle_done_command(
    id: &str,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    use aida_core::RequirementStatus;
    let mut req = backend
        .get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if matches!(req.status, RequirementStatus::Completed) {
        println!(
            "{} {display_id} is already done.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        return Ok(());
    }
    if status_advance_requires_advisor_authority(&req.status, &RequirementStatus::Completed)
        && !has_advisor_authority()
    {
        // BUG-585: name the ACTUAL escape hatches, not a circular re-run of the
        // same command. A non-TTY agent/script gets nothing new by re-running
        // `aida done`; the working path is a valid session grant established
        // through `aida role enter advisor` at a human TTY.
        // trace:BUG-585 | ai:claude
        anyhow::bail!(
            "marking {display_id} done needs advisor authority. Run `aida role enter advisor` at an interactive TTY, or ask an advisor session to do it."
        );
    }
    // BUG-1286 F1: `aida done` is an into-Completed transition and must emit the
    // durable ship record. STORY-1418: the stamp, the persist and the emission
    // go through the one seam so the rule and the call live in one place.
    // trace:BUG-1286 trace:STORY-1418 | ai:claude
    completion::transition_to_completed(
        &mut req,
        store_path.parent(),
        &display_id,
        "",
        "done",
        |req, prior| {
            // BUG-1637: caller-authored (same identity as before), through the
            // one shared history helper. trace:BUG-1637 | ai:claude
            aida_core::conflict::record_status_transition(req, &current_user_id(None), prior);
            backend.update_requirement(req)?;
            Ok(())
        },
    )?;
    record_role_activity(&display_id, "done");
    // STORY-738: `aida done` is always an into-Completed transition (the
    // already-Completed case returned early above), so the human path gets
    // the completion crescendo instead of the flat check-mark `done` line. The
    // agent/TOON surface keeps the terse machine line. trace:STORY-738
    if agent_output_mode() {
        println!(
            "{} {display_id} — {}",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            "done".green()
        );
    } else {
        render_completion_crescendo(&display_id, &req.title);
    }
    Ok(())
}

/// STORY-738: the completion crescendo. When a spec transitions INTO
/// Completed — the payoff state the thought→merged arc exists for — render a
/// distinct, felt close instead of the flat generic `Updated: <id>` line
/// (which is reused for any tag edit) or the bare check-mark `done` line. The
/// render names the spec + title, shows the arc terminating (filed → built →
/// merged → completed, the loop closed), and points forward to `aida show
/// <id>` for the commit that landed it. Honors STORY-700 AC3 (BINDING): the
/// completion is the felt moment, not a generic done message. Glyphs come
/// from the registry so it stays ascii-safe; the `aida show <id>` breadcrumb
/// is the one deliberate SPEC-ID in the copy (the caller just typed the id,
/// so naming it matches the done/Updated house style).
// trace:STORY-738 | ai:claude
pub(crate) fn render_completion_crescendo(display_id: &str, title: &str) {
    for line in completion_crescendo_lines(display_id, title) {
        println!("{line}");
    }
}

/// Pure renderer for [`render_completion_crescendo`] — returns the rendered
/// (colored) lines so the felt elements are unit-testable without capturing
/// stdout.
// trace:STORY-738 | ai:claude
pub(crate) fn completion_crescendo_lines(display_id: &str, title: &str) -> Vec<String> {
    use crate::glyphs::Glyph;
    let check = crate::glyph(Glyph::Check);
    let arrow = crate::glyph(Glyph::Arrow);
    let sub = crate::glyph(Glyph::SubArrow);
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {} reached {} — the loop closed.",
        check.green().bold(),
        display_id.bold(),
        "Completed".green().bold()
    ));
    let title = title.trim();
    if !title.is_empty() {
        lines.push(format!("  {}", title.dimmed()));
    }
    let arc = format!("filed {arrow} built {arrow} merged {arrow} completed");
    lines.push(format!("  {} {}", sub.dimmed(), arc.dimmed()));
    lines.push(format!(
        "  {} {}  to see the commit that landed it.",
        sub.dimmed(),
        format!("aida show {display_id}").cyan()
    ));
    lines
}

/// STORY-738: is this status edit a transition INTO Completed? True only when
/// the prior status was something other than Completed and the new canonical
/// status is `Completed` — a no-op re-set of an already-Completed spec is not
/// a crescendo moment.
// trace:STORY-738 | ai:claude
pub(crate) fn is_into_completed_transition(
    old: &aida_core::RequirementStatus,
    new_canonical: &str,
) -> bool {
    !matches!(old, aida_core::RequirementStatus::Completed) && new_canonical == "Completed"
}

/// STORY-738: which render the `aida edit` status-change path emits. The
/// crescendo fires only on a true into-Completed transition AND only on the
/// human surface; the agent/TOON path and every non-completion edit keep the
/// flat Updated line.
// trace:STORY-738 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EditCompletionRender {
    Crescendo,
    Updated,
}

pub(crate) fn edit_completion_render(
    into_completed: bool,
    agent_mode: bool,
) -> EditCompletionRender {
    if into_completed && !agent_mode {
        EditCompletionRender::Crescendo
    } else {
        EditCompletionRender::Updated
    }
}

#[cfg(test)]
#[path = "tests/story_738_completion_crescendo_tests.rs"]
mod story_738_completion_crescendo_tests;

/// Local truncate helper for archive listings; mirrors `history::shorten`.
/// Char-boundary-safe: titles carry emoji/unicode (e.g. the pause/arrow glyphs),
/// so slicing by raw byte index panics mid-codepoint. Truncate by chars instead.
// trace:BUG-424 | ai:claude
pub(crate) fn shorten_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// TASK-730: when the default `aida list` view (no status filter) shows a MIX of
/// finished and unfinished work, point a returning user at `aida list open` —
/// the "what's left to do" view they'd otherwise never discover (Completed
/// specs stay visible until archived, by design). No tip when everything's done
/// or everything's open, or when a status filter is already applied.
// trace:TASK-730 | ai:claude
pub(crate) fn maybe_print_whats_left_tip(
    status_filter: Option<&str>,
    reqs: &[aida_core::RequirementSummary],
) {
    if status_filter.is_some() {
        return;
    }
    let has_done = reqs
        .iter()
        .any(|r| r.status.eq_ignore_ascii_case("completed"));
    let has_open = reqs
        .iter()
        .any(|r| !r.status.eq_ignore_ascii_case("completed"));
    if has_done && has_open {
        println!(
            "{}",
            "  Tip: `aida list open` shows just what's left to do.".dimmed()
        );
    }
}

/// STORY-584 (criterion 4): in the `--deferred` view, print each spec's revisit
/// trigger so the operator can scan "what is primed, and what brings each back."
/// The trigger comes from the `deferred_until` field when set; for rows deferred
/// only via a legacy `deferred:*` parking tag, fall back to the tag's suffix
/// (e.g. `deferred:stabilization-first` → "stabilization-first"). Prints nothing
/// outside the deferred view or when no rows carry a discoverable trigger.
/// trace:STORY-584 | ai:claude
pub(crate) fn print_deferred_triggers(deferred_view: bool, reqs: &[aida_core::RequirementSummary]) {
    if !deferred_view || reqs.is_empty() {
        return;
    }
    // Derive a trigger string for a row: explicit field first, else the
    // suffix of the first `deferred:*` tag.
    let trigger_of = |r: &aida_core::RequirementSummary| -> Option<String> {
        if let Some(cond) = r.deferred_until.as_deref() {
            if !cond.is_empty() {
                return Some(cond.to_string());
            }
        }
        r.tags
            .iter()
            .find_map(|t| t.strip_prefix("deferred:"))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    println!("\n{}", "Revisit triggers:".bold());
    for r in reqs {
        let display_id = r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?");
        match trigger_of(r) {
            Some(cond) => println!("  {}  {}", display_id.bold(), cond.dimmed()),
            None => println!(
                "  {}  {}",
                display_id.bold(),
                "(no trigger — set one with `aida defer <id> --until \"…\"`)".dimmed()
            ),
        }
    }
}

pub(crate) fn handle_punts_command(cmd: PuntsCommand) -> Result<()> {
    let project_root = find_project_root()?;
    let records = punt::read_ledger(&project_root);

    match cmd {
        PuntsCommand::List {
            spec,
            category,
            resolution,
            all,
        } => {
            let mut filtered = records;
            // BUG-674: hide auto-filed session-end visibility warnings by
            // default so genuine decision-punts aren't buried; `--all` shows
            // them. trace:BUG-674 | ai:claude
            if !all {
                filtered.retain(|r| !punt::is_session_end_noise(r));
            }
            if let Some(s) = spec {
                filtered.retain(|r| r.spec.eq_ignore_ascii_case(&s));
            }
            if let Some(c) = category {
                filtered.retain(|r| r.category.to_string().eq_ignore_ascii_case(&c));
            }
            if let Some(r) = resolution {
                filtered.retain(|rec| rec.resolution_path.eq_ignore_ascii_case(&r));
            }

            if filtered.is_empty() {
                println!("No punt records found matching criteria.");
                return Ok(());
            }

            println!(
                "{:<24} {:<12} {:<20} {:<12} {}",
                "TIMESTAMP".dimmed(),
                "SPEC".dimmed(),
                "RESOLUTION PATH".dimmed(),
                "DECISION".dimmed(),
                "DETAIL".dimmed()
            );
            println!("{}", "─".repeat(80).dimmed());
            for r in filtered {
                let ts = r.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                let dec = r.decision.as_deref().unwrap_or("-");
                let dec_colored = match dec {
                    "resolved" => dec.green(),
                    "escalated" => dec.red(),
                    "deferred" => dec.yellow(),
                    _ => dec.normal(),
                };
                let res_colored = match r.resolution_path.as_str() {
                    "advisor-resolved" => r.resolution_path.green(),
                    "escalated-to-human" => r.resolution_path.red(),
                    "punted" => r.resolution_path.yellow(),
                    _ => r.resolution_path.normal(),
                };
                println!(
                    "{:<24} {:<12} {:<20} {:<12} {}",
                    ts.dimmed(),
                    r.spec.cyan().bold(),
                    res_colored,
                    dec_colored,
                    r.detail
                );
            }
        }
        PuntsCommand::Analyze { all } => {
            // BUG-674: exclude auto-filed session-end visibility warnings from
            // the analytics by default (they are not decision-punts); `--all`
            // includes them. trace:BUG-674 | ai:claude
            let records: Vec<punt::PuntRecord> = if all {
                records
            } else {
                records
                    .into_iter()
                    .filter(|r| !punt::is_session_end_noise(r))
                    .collect()
            };
            let total = records.len();
            if total == 0 {
                println!("The punt ledger is empty.");
                return Ok(());
            }

            let mut punted_count = 0;
            let mut resolved_count = 0;
            let mut escalated_count = 0;
            let mut total_judged = 0;

            let mut category_counts = std::collections::HashMap::new();
            let mut classification_counts = std::collections::HashMap::new();
            let mut durations = Vec::new();

            for r in &records {
                *category_counts.entry(r.category).or_insert(0) += 1;
                if let Some(ref cls) = r.classification {
                    *classification_counts.entry(cls.clone()).or_insert(0) += 1;
                }

                // BUG-674: only a genuinely OPEN record is "Awaiting Triage".
                // Resolved / escalated (incl. the CLI/MCP direct-close slugs)
                // land in their own buckets; dismissed + shelved-by-failure
                // records are closed and counted in neither triage bucket.
                if punt::is_open(r) {
                    punted_count += 1;
                } else if r.resolution_path == "advisor-resolved"
                    || r.resolution_path == punt::RESOLUTION_HUMAN_RESOLVED
                    || r.decision.as_deref() == Some("resolved")
                {
                    resolved_count += 1;
                    total_judged += 1;
                } else if r.resolution_path == "escalated-to-human"
                    || r.decision.as_deref() == Some("escalated")
                {
                    escalated_count += 1;
                    total_judged += 1;
                }

                if let (Some(paused), Some(resolved)) = (r.paused_at, r.resolved_at) {
                    if resolved >= paused {
                        durations.push(resolved.signed_duration_since(paused));
                    }
                }
            }

            let escalation_rate = if total_judged > 0 {
                (escalated_count as f64 / total_judged as f64) * 100.0
            } else {
                0.0
            };

            println!("{}", "◆ AIDA Punt Ledger Analytics ◆".magenta().bold());
            println!(
                "{}",
                "──────────────────────────────────────────────────".dimmed()
            );
            println!(
                "{:<30} {}",
                "Total Recorded Punts:",
                total.to_string().bold()
            );
            println!(
                "{:<30} {}",
                "  Punted (Awaiting Triage):",
                punted_count.to_string().yellow()
            );
            println!(
                "{:<30} {}",
                "  Resolved by Advisor:",
                resolved_count.to_string().green()
            );
            println!(
                "{:<30} {}",
                "  Escalated to Human:",
                escalated_count.to_string().red()
            );
            println!(
                "{:<30} {:.1}% ({}/{})",
                "Escalation Rate (Advisor):", escalation_rate, escalated_count, total_judged
            );

            if !durations.is_empty() {
                let total_sec: i64 = durations.iter().map(|d| d.num_seconds()).sum();
                let avg_sec = total_sec / durations.len() as i64;
                let avg_duration = chrono::Duration::seconds(avg_sec);
                println!(
                    "{:<30} {}m {}s",
                    "Average Resolution Time:",
                    avg_duration.num_minutes(),
                    avg_duration.num_seconds() % 60
                );
            }

            println!("\n{}", "Category Breakdown:".bold());
            let mut categories: Vec<_> = category_counts.into_iter().collect();
            categories.sort_by(|a, b| b.1.cmp(&a.1));
            for (cat, count) in categories {
                println!(
                    "  {:<28} {}",
                    cat.to_string().blue(),
                    count.to_string().bold()
                );
            }

            println!(
                "\n{}",
                "Classification Triage Patterns (Question Shapes):".bold()
            );
            if classification_counts.is_empty() {
                println!("  No classifications recorded yet.");
            } else {
                let mut classes: Vec<_> = classification_counts.into_iter().collect();
                classes.sort_by(|a, b| b.1.cmp(&a.1));
                for (cls, count) in classes {
                    println!("  {:<28} {}", cls.cyan(), count.to_string().bold());
                }
            }
            println!(
                "{}",
                "──────────────────────────────────────────────────".dimmed()
            );
        }
        PuntsCommand::Resolve {
            id,
            answer,
            reasoning,
            classification,
        } => {
            // BUG-674: resolve via the shared core (parity with the
            // `resolve_punt` MCP tool) — writes the orchestrator resume
            // response AND closes the open ledger record(s). trace:BUG-674
            let reasoning =
                reasoning.unwrap_or_else(|| "resolved via `aida punts resolve`".to_string());
            let (path, closed) = punt::resolve_punt_core(
                &project_root,
                &id,
                &answer,
                &reasoning,
                classification,
                Some("cli"),
            )?;
            if closed == 0 {
                println!(
                    "No open punt found for {} — wrote resolution response to {}.",
                    id.cyan(),
                    path.display().to_string().dimmed()
                );
            } else {
                println!(
                    "{} Resolved {} and closed {} ledger record{}. A live drain resumes on the response.",
                    glyph(crate::glyphs::Glyph::Check).green(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" }
                );
            }
        }
        PuntsCommand::Dismiss { id, reason } => {
            // BUG-674: dismiss just closes the ledger record — no orchestrator
            // response (a dismissed punt does not resume an implementer).
            let closed = punt::dismiss_punt_core(&project_root, &id, Some("cli"))?;
            if closed == 0 {
                println!("No open punt found for {}.", id.cyan());
            } else {
                let note = reason.map(|r| format!(" ({r})")).unwrap_or_default();
                println!(
                    "{} Dismissed {} — closed {} ledger record{}{}.",
                    glyph(crate::glyphs::Glyph::Check).green(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" },
                    note
                );
            }
        }
        PuntsCommand::Escalate {
            id,
            reasoning,
            escalation_reason,
            classification,
        } => {
            // BUG-674: escalate via the shared core (parity with the
            // `escalate_punt` MCP tool) — writes the park response AND closes
            // the ledger record as escalated-to-human. trace:BUG-674
            let (path, closed) = punt::escalate_punt_core(
                &project_root,
                &id,
                &reasoning,
                escalation_reason,
                classification,
                Some("cli"),
            )?;
            if closed == 0 {
                println!(
                    "No open punt found for {} — wrote escalation response to {}.",
                    id.cyan(),
                    path.display().to_string().dimmed()
                );
            } else {
                println!(
                    "{} Escalated {} to a human and closed {} ledger record{}.",
                    glyph(crate::glyphs::Glyph::Warning).yellow(),
                    id.cyan(),
                    closed,
                    if closed == 1 { "" } else { "s" }
                );
            }
        }
        PuntsCommand::Read { id } => {
            // Most recent record for the spec is the live one (mirrors the
            // `read_punt` MCP tool). trace:BUG-674
            let record = records
                .iter()
                .rev()
                .find(|r| r.spec.eq_ignore_ascii_case(&id))
                .ok_or_else(|| {
                    anyhow::anyhow!("No punt record found in ledger for spec ID '{}'", id)
                })?;
            println!("{}", serde_json::to_string_pretty(record)?);
        }
        PuntsCommand::Promote { id, memory_name } => {
            let record = records
                .iter()
                .find(|r| r.spec.eq_ignore_ascii_case(&id))
                .ok_or_else(|| {
                    anyhow::anyhow!("No punt record found in ledger for spec ID '{}'", id)
                })?;

            let target_path = if memory_name.starts_with("docs/") {
                project_root.join(&memory_name)
            } else if memory_name.starts_with("discipline/") {
                project_root.join("docs").join(&memory_name)
            } else {
                project_root.join(&memory_name)
            };

            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let title = record.classification.as_deref().unwrap_or(&record.detail);
            let slug = memory_name
                .trim_end_matches(".md")
                .replace(['/', ' ', '-'], "_");
            let date_str = chrono::Utc::now().format("%Y-%m-%d").to_string();

            let mut content = String::new();
            if !target_path.exists() {
                content.push_str(&format!(
                    "# {}\n\n\
                     **Last updated**: {}\n\
                     **Principle Trace**: `feedback_{}` | `{}`\n\n\
                     ## Context & Design Fork\n\
                     - **Spec**: {}\n\
                     - **Category**: {}\n\
                     - **Raised By**: {}\n\
                     - **Resolution Path**: {}\n\n\
                     ## Decision / Principle\n\
                     {}\n",
                    title,
                    date_str,
                    slug,
                    record.spec,
                    record.spec,
                    record.category,
                    record.raised_by.as_deref().unwrap_or("unknown"),
                    record.resolution_path,
                    record
                        .answer
                        .as_deref()
                        .or(record.classification.as_deref())
                        .unwrap_or(&record.detail)
                ));
                std::fs::write(&target_path, &content)?;
                println!(
                    "Created new memory pack entry at: {}",
                    target_path.display().to_string().cyan()
                );
            } else {
                content.push_str(&format!(
                    "\n---\n\n\
                     ## Principle Trace: `feedback_{}` | `{}`\n\
                     - **Spec**: {}\n\
                     - **Category**: {}\n\
                     - **Decision**: {}\n\n\
                     ### Context\n\
                     {}\n\n\
                     ### Resolution\n\
                     {}\n",
                    slug,
                    record.spec,
                    record.spec,
                    record.category,
                    record.decision.as_deref().unwrap_or("-"),
                    record.detail,
                    record
                        .answer
                        .as_deref()
                        .or(record.classification.as_deref())
                        .unwrap_or(&record.detail)
                ));
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&target_path)?;
                file.write_all(content.as_bytes())?;
                println!(
                    "Appended design-fork pattern to existing memory pack entry at: {}",
                    target_path.display().to_string().cyan()
                );
            }

            // Update the record's principle_link in the ledger
            let mut updated_records = records.clone();
            let mut found = false;
            for r in &mut updated_records {
                if r.spec.eq_ignore_ascii_case(&id) {
                    r.principle_link = Some(memory_name.clone());
                    found = true;
                    break;
                }
            }

            if found {
                let path = punt::ledger_path(&project_root);
                let mut file = std::fs::File::create(&path)?;
                for r in &updated_records {
                    let serialized = serde_json::to_string(r)?;
                    use std::io::Write;
                    writeln!(file, "{}", serialized)?;
                }
                println!(
                    "Updated punt record '{}' with principle link to '{}'.",
                    id.cyan(),
                    memory_name.green()
                );
            }
        }
    }

    Ok(())
}

/// STORY-439: `aida autonomy calibration mismatches` — the substrate-gap
/// signal. Walks `.aida/complexity-calibration/*.yaml`, drops records
/// missing a pickup-or-review half, and surfaces the rest ranked by
/// `|delta_steps|` descending. Tied gaps break by recency. The mismatch
/// view IS the calibration view this STORY adds; the broader autonomy
/// report (TASK-340) gains `--by` / `--calibration` slices in a
/// follow-up that hangs off this same parent enum.
/// trace:STORY-439 | ai:claude
/// Add a promoted finding to a role's work queue.
///
/// Findings are follow-ups that usually need an implementer, so the default
/// route is the `implementer` queue; `for_override` (the `--for` flag) picks
/// a different role. Returns the role it routed to so the caller can name it
/// in the success message.
///
/// Kept separate from the status flip in `FindingsCommand::Promote` so a
/// queue-add failure surfaces as a non-zero exit *before* the finding is
/// marked Approved — BUG-231 was a silent "Approved but in no queue"
/// half-state because the old promote path flipped status and printed
/// "joins the work queue" without ever calling `queue add`.
/// trace:BUG-231 | ai:claude
///
/// BUG-1651: production promotes go through [`queue_promoted_finding_entry`];
/// this role-returning form remains for the BUG-231 tests.
// trace:BUG-1651 | ai:claude
#[cfg(test)]
pub(crate) fn queue_promoted_finding(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    display_id: &str,
    for_override: Option<&str>,
) -> Result<String> {
    queue_promoted_finding_entry(store_path, requirement_id, display_id, for_override)
        .map(|entry| entry.for_role.unwrap_or_default())
}

/// `queue_promoted_finding`, returning the entry it wrote so a failed
/// promote can tell whether the queue still holds it (BUG-1651).
// trace:BUG-1651 | ai:claude
pub(crate) fn queue_promoted_finding_entry(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    display_id: &str,
    for_override: Option<&str>,
) -> Result<aida_core::QueueEntry> {
    let role = for_override.unwrap_or("implementer");
    queue_spec_entry_for_role(
        store_path,
        requirement_id,
        role,
        format!("Promoted from finding {display_id} via `aida findings promote`"),
    )
    .with_context(|| {
        format!(
            "failed to add {display_id} to the {role} queue — \
             finding left at draft, not promoted"
        )
    })
}

/// Append a spec to a role's work queue for the current user; returns the
/// role. Shared by the work and gate promote routes, each of which adds its
/// own failure context.
// trace:STORY-1428 | ai:claude
pub(crate) fn queue_spec_for_role(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    role: &str,
    note: String,
) -> Result<String> {
    queue_spec_entry_for_role(store_path, requirement_id, role, note)?;
    Ok(role.to_string())
}

/// [`queue_spec_for_role`], returning the entry as written (its `position`
/// still the append sentinel the backend resolves). BUG-1651: the promote
/// rollback matches the queue against it.
// trace:STORY-1428 trace:BUG-1651 | ai:claude
pub(crate) fn queue_spec_entry_for_role(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    role: &str,
    note: String,
) -> Result<aida_core::QueueEntry> {
    let role = role.to_string();
    let user_id = current_user_id(None);
    let storage = Storage::new(store_path);
    let entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id,
        // i64::MAX is the "append to bottom" sentinel the git backend
        // resolves to max_position + 1000 (STORY-72).
        position: i64::MAX,
        added_by: user_id,
        note: Some(note),
        added_at: chrono::Utc::now(),
        for_role: Some(role),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry.clone())?;
    Ok(entry)
}

// trace:TASK-0001 | ai:claude:high
/// Count requirements in a git-canonical store at `store_path`. Returns
/// None if the store doesn't exist or can't be opened — caller treats that
/// as "no data to lose, proceed".
/// trace:EPIC-1-001 | ai:claude
pub(crate) fn count_requirements_in_store(store_path: &std::path::Path) -> Option<usize> {
    if !store_path.is_dir() {
        return None;
    }
    let backend = aida_core::GitBackend::new(store_path).ok()?;
    let store = aida_core::DatabaseBackend::load(&backend).ok()?;
    Some(store.requirements.len())
}

/// Same as count_requirements_in_store but for a legacy SQLite-canonical
/// store at `db_path`.
pub(crate) fn count_requirements_in_sqlite(db_path: &std::path::Path) -> Result<usize> {
    let storage = Storage::new(db_path);
    Ok(storage.load()?.requirements.len())
}

/// Surface the data-loss risk of `aida init --force` on a populated store.
/// Returns true if the user confirmed (typed "reset"), false otherwise.
/// Bails the parent caller via Ok if the user cancels — caller pattern is
/// `if !confirm_destructive_reset(...)? { return Ok(()); }`.
/// trace:EPIC-1-001 | ai:claude
pub(crate) fn confirm_destructive_reset(
    count: usize,
    store_path: &std::path::Path,
) -> Result<bool> {
    eprintln!();
    eprintln!(
        "{} `aida init --force` will RESET the requirements store at {}.",
        "DANGER:".red().bold(),
        store_path.display()
    );
    eprintln!(
        "        {} existing requirement(s) will be lost.",
        count.to_string().red().bold()
    );
    eprintln!();
    eprintln!("If you only wanted to refresh scaffolding (CLAUDE.md, .claude/skills/, hooks),");
    eprintln!(
        "cancel here and run instead:  {}",
        "aida scaffold apply --force".cyan()
    );
    eprintln!();
    eprintln!(
        "Type `{}` (literally) to confirm the destructive reset, or anything else to cancel:",
        "reset".bold()
    );
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        eprintln!("Cancelled.");
        return Ok(false);
    }
    if answer.trim() == "reset" {
        eprintln!("{} proceeding with reset.", "Confirmed:".yellow());
        Ok(true)
    } else {
        eprintln!("Cancelled. Store untouched.");
        Ok(false)
    }
}

/// STORY-757: the markdown-only first run. Scaffolds ONLY a `specs/` folder with
/// one example spec + a sample source file carrying a trace comment, then points
/// at `aida why`. No orphan branch, cache, MCP, skills, roles, or queue — the
/// 60-second magic with zero machine, so a first-user gets the whole idea in one
/// command and only reaches for `aida init` when a folder of markdown isn't
/// enough.
// trace:STORY-757 | ai:claude
/// Write the minimal scaffold under `root`. Returns the created paths, or an
/// empty vec when a target already exists and `force` is false (caller reports
/// it). Split out from [`handle_init_minimal`] so it's testable without a
/// process-global `set_current_dir` (the BUG-697 race).
// trace:STORY-757 | ai:claude
pub(crate) fn scaffold_minimal_specs(
    root: &std::path::Path,
    force: bool,
) -> Result<Vec<std::path::PathBuf>> {
    let specs = root.join("specs");
    let spec_file = specs.join("EXAMPLE-1.md");
    let demo = root.join("example.py");
    if (spec_file.exists() || demo.exists()) && !force {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&specs).with_context(|| "creating specs/")?;
    std::fs::write(
        &spec_file,
        "---\nid: EXAMPLE-1\ntitle: Rate-limit the login endpoint\nstatus: draft\n---\n\
         We were seeing credential-stuffing attacks. Throttle login attempts to 5/min per IP.\n",
    )
    .with_context(|| "writing specs/EXAMPLE-1.md")?;
    std::fs::write(
        &demo,
        "def login(req):\n    # trace:EXAMPLE-1\n    if too_many_attempts(req.ip):\n        return deny()\n",
    )
    .with_context(|| "writing example.py")?;
    Ok(vec![spec_file, demo])
}

pub(crate) fn handle_init_minimal(force: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let created = scaffold_minimal_specs(&root, force)?;
    if created.is_empty() {
        println!(
            "  {} specs/EXAMPLE-1.md or example.py already exists — pass --force to overwrite.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        return Ok(());
    }

    let check = crate::glyph(crate::glyphs::Glyph::Check).green();
    println!("{check} minimal AIDA — a folder of markdown, zero machine.");
    println!();
    println!("  created  specs/EXAMPLE-1.md   a spec: a title + one line of why");
    println!("  created  example.py           code with a `# trace:EXAMPLE-1` comment");
    println!();
    println!("  Now ask your code why it exists:");
    println!("    {}", "aida why example.py:2".cyan().bold());
    println!();
    println!(
        "  {}",
        "That's the whole idea — add `# trace:<ID>` comments to your own code and ask.".dimmed()
    );
    println!(
        "  {}",
        "When a folder of markdown isn't enough, `aida init` adds the graph, IDs, and MCP."
            .dimmed()
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/init_minimal_tests.rs"]
mod init_minimal_tests;

#[derive(Default)]
pub(crate) struct DisciplinePackScaffoldReport {
    pub(crate) written: usize,
    pub(crate) relocated: usize,
    pub(crate) written_paths: Vec<std::path::PathBuf>,
    pub(crate) relocated_paths: Vec<std::path::PathBuf>,
}

/// Scaffold the discipline pack — every embedded `.aida/discipline/*`
/// template — into `<root>/.aida/discipline/`. Idempotent: an existing
/// canonical file is left alone even under refresh.
///
/// Migration: a previous `<root>/docs/aida/discipline/` pack is moved into the
/// canonical directory first. Existing destination files are never overwritten,
/// so user edits survive and an old path can remain as the one-release
/// fallback when there is a destination conflict.
/// trace:STORY-255 | STORY-443 | ai:claude
// trace:STORY-829 | ai:codex
pub(crate) fn ensure_discipline_pack_scaffold(
    root: &std::path::Path,
    _force: bool,
) -> Result<DisciplinePackScaffoldReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;
    let mut pack: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(".aida/discipline/").map(|n| (n, *v)))
        .collect();
    if pack.is_empty() {
        return Ok(DisciplinePackScaffoldReport::default());
    }
    pack.sort_by(|a, b| a.0.cmp(b.0));

    let dir = root.join(".aida").join("discipline");
    std::fs::create_dir_all(&dir)?;
    let mut report = DisciplinePackScaffoldReport::default();
    let old_dir = root.join("docs").join("aida").join("discipline");
    if old_dir.is_dir() {
        let mut old_entries = Vec::new();
        for entry in std::fs::read_dir(&old_dir)? {
            old_entries.push(entry?.path());
        }
        old_entries.sort();
        for old_path in old_entries {
            if !old_path.is_file() {
                continue;
            }
            let Some(name) = old_path.file_name() else {
                continue;
            };
            let dest = dir.join(name);
            if dest.exists() {
                continue;
            }
            std::fs::rename(&old_path, &dest)?;
            report.relocated += 1;
            report
                .relocated_paths
                .push(dest.strip_prefix(root).unwrap_or(&dest).to_path_buf());
        }
        remove_empty_parent_dirs(root, &old_dir)?;
    }

    for (name, content) in pack {
        let dest = dir.join(name);
        if dest.exists() {
            continue;
        }
        std::fs::write(&dest, format!("{content}\n"))?;
        report.written += 1;
        report
            .written_paths
            .push(dest.strip_prefix(root).unwrap_or(&dest).to_path_buf());
    }
    Ok(report)
}

pub(crate) fn remove_empty_parent_dirs(
    root: &std::path::Path,
    start: &std::path::Path,
) -> Result<()> {
    let mut dir = start.to_path_buf();
    while dir.starts_with(root) && dir != root {
        match std::fs::remove_dir(&dir) {
            Ok(()) => {
                dir.pop();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                dir.pop();
            }
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Scaffold a starter `docs/competitive-analysis/ecosystem-watch.md` with
/// today's local date stamped into the `Last updated` line. Idempotent:
/// leaves an existing file alone unless `force` is set. Returns `true` if
/// the file was written.
///
/// `scripts/release.sh` reads the `Last updated` line to verify the
/// ecosystem review is recent enough for a major/minor release; without
/// this scaffold, a fresh project's first `release.sh minor` would hit the
/// missing-file warning path (TASK-126 origin).
// trace:TASK-126 | ai:claude
/// Scaffold `.aida/project.toml` — the checked-in statement of what this
/// project IS — pre-filled from what is already knowable.
///
/// Idempotent in the same way the discipline pack and ecosystem-watch
/// scaffolds are: an existing file is left ALONE unless `force`. That is what
/// satisfies "`aida init --refresh` must not overwrite human edits" — the
/// manifest has no canonical master to overlay (unlike skills or memories,
/// whose refresh contract compares a checksum against an embedded template),
/// so the honest behaviour is simply never to rewrite it. Returns whether a
/// file was written.
///
/// PRE-FILLED, NEVER A BLANK FORM. Name, description and repository come from
/// the directory, the README and `origin`, so the file is worth something the
/// moment it exists even if nobody ever edits it. An empty form is exactly how
/// a metadata standard goes stale in a week.
// trace:STORY-781 | ai:claude
pub(crate) fn ensure_project_manifest_scaffold(
    root: &std::path::Path,
    force: bool,
) -> Result<bool> {
    use aida_core::project_manifest as pm;
    let dest = root.join(pm::MANIFEST_REL_PATH);
    if dest.exists() && !force {
        return Ok(false);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let facts = pm::derive_facts(root);
    std::fs::write(&dest, pm::render(&facts))?;
    Ok(true)
}

pub(crate) const PROJECTS_REGISTRY_HEADER: &str = "\
# AIDA project registry
# Machine-global index of local projects. Hand-maintained today; EPIC-22 makes this canonical.

";

pub(crate) fn register_project_in_global_registry(root: &std::path::Path) -> Result<bool> {
    let home =
        aida_home_dir().context("cannot resolve home directory for ~/.aida/projects.toml")?;
    register_project_in_registry_at(&home.join(".aida/projects.toml"), root)
}

pub(crate) fn register_project_in_registry_at(
    registry_path: &std::path::Path,
    root: &std::path::Path,
) -> Result<bool> {
    use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

    let existing = match std::fs::read_to_string(registry_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", registry_path.display())),
    };
    let before = existing.as_deref().unwrap_or("");
    let mut doc: DocumentMut = before
        .parse()
        .with_context(|| format!("parsing {}", registry_path.display()))?;

    if doc.get("project").is_none() {
        doc["project"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let projects = doc["project"]
        .as_array_of_tables_mut()
        .context("~/.aida/projects.toml key `project` must be an array of tables")?;

    let project_path = absolute_project_path(root)?;
    let project_path_s = project_path.to_string_lossy().to_string();
    let name = project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "project".to_string());
    let repo = git_origin_url(root);

    let mut found = false;
    for table in projects.iter_mut() {
        if table
            .get("path")
            .and_then(|i| i.as_str())
            .map(|p| p == project_path_s)
            .unwrap_or(false)
        {
            write_project_registry_table(table, &name, &project_path_s, repo.as_deref());
            found = true;
            break;
        }
    }
    if !found {
        let mut table = Table::new();
        write_project_registry_table(&mut table, &name, &project_path_s, repo.as_deref());
        projects.push(table);
    }

    let mut after = doc.to_string();
    if existing.is_none() {
        after = format!("{PROJECTS_REGISTRY_HEADER}{after}");
    }
    if after == before {
        return Ok(false);
    }
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(registry_path, after)?;
    Ok(true)
}

pub(crate) fn write_project_registry_table(
    table: &mut toml_edit::Table,
    name: &str,
    path: &str,
    repo: Option<&str>,
) {
    table["name"] = toml_edit::value(name);
    table["path"] = toml_edit::value(path);
    if let Some(repo) = repo.filter(|r| !r.trim().is_empty()) {
        table["repo"] = toml_edit::value(repo);
    } else {
        table.remove("repo");
    }
}

pub(crate) fn absolute_project_path(root: &std::path::Path) -> Result<std::path::PathBuf> {
    let abs = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };
    Ok(abs.canonicalize().unwrap_or(abs))
}

pub(crate) fn git_origin_url(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub(crate) fn ensure_ecosystem_watch_scaffold(root: &std::path::Path, force: bool) -> Result<bool> {
    let dir = root.join("docs").join("competitive-analysis");
    let dest = dir.join("ecosystem-watch.md");
    if dest.exists() && !force {
        return Ok(false);
    }
    let Some(template) = aida_core::templates::EMBEDDED_TEMPLATES
        .get("docs/competitive-analysis/ecosystem-watch.md")
    else {
        return Ok(false);
    };
    std::fs::create_dir_all(&dir)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let content = template.replace("{{LAST_UPDATED}}", &today);
    std::fs::write(&dest, content)?;
    Ok(true)
}

/// FNV-1a 64-bit hash, lowercase hex. Used as the starter memory pack's
/// "edited since scaffold?" fingerprint — deterministic across releases and
/// platforms, no dependency. trace:STORY-255 | ai:claude
pub(crate) fn fnv1a_hex(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Normalize Windows (`\r\n`) and classic-Mac (`\r`) line endings to `\n`
/// so the frontmatter parser is line-ending-agnostic. Git on Windows with
/// `autocrlf` checks the embedded memory templates out as CRLF, which
/// `build.rs` then embeds verbatim — without this the `---\n` frontmatter
/// fence never matches and the template is reported malformed.
/// trace:BUG-244 | ai:claude
pub(crate) fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Split a markdown file into (frontmatter, body) at the leading `---`
/// fenced YAML block. Returns `None` when there is no frontmatter. Callers
/// pass LF-normalized input (see `normalize_line_endings`).
pub(crate) fn split_md_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

/// Read a top-level scalar frontmatter field. Indented (nested) keys are
/// ignored on purpose — `originSessionId` must be a top-level key for the
/// refresh check to see it. trace:STORY-255 | ai:claude
pub(crate) fn frontmatter_field<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}: ");
    frontmatter
        .lines()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

/// Build the on-disk form of a starter memory from its embedded template:
/// stamp `originSessionId: aida-scaffold` and a `scaffoldChecksum` (an
/// FNV-1a fingerprint of the body) into the frontmatter, so a later
/// `--refresh` can tell a pristine pack file from a user-edited one.
/// trace:STORY-255 | ai:claude
pub(crate) fn build_scaffolded_memory(template: &str) -> Option<String> {
    let template = normalize_line_endings(template);
    let (fm, body) = split_md_frontmatter(&template)?;
    let body = body.trim_end();
    let checksum = fnv1a_hex(body.as_bytes());
    let mut new_fm = String::new();
    for line in fm.lines() {
        if line.starts_with("originSessionId:") || line.starts_with("scaffoldChecksum:") {
            continue;
        }
        new_fm.push_str(line);
        new_fm.push('\n');
    }
    new_fm.push_str("originSessionId: aida-scaffold\n");
    new_fm.push_str(&format!("scaffoldChecksum: {checksum}\n"));
    Some(format!("---\n{new_fm}---\n{body}\n"))
}

/// What `aida init --with-memories --refresh` should do with an existing
/// memory file. trace:STORY-255 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MemoryDisposition {
    /// Not a scaffolded pack file (no `originSessionId: aida-scaffold`) —
    /// the user wrote it; never touch it.
    UserOwned,
    /// Scaffolded, but the body no longer matches its recorded checksum —
    /// the user has edited it; keep their edits.
    Edited,
    /// Scaffolded and untouched since — safe to overlay a new version.
    Pristine,
}

/// Classify an existing memory file for `--refresh`.
pub(crate) fn memory_refresh_disposition(existing: &str) -> MemoryDisposition {
    let existing = normalize_line_endings(existing);
    let Some((fm, body)) = split_md_frontmatter(&existing) else {
        return MemoryDisposition::UserOwned;
    };
    if frontmatter_field(fm, "originSessionId") != Some("aida-scaffold") {
        return MemoryDisposition::UserOwned;
    }
    match frontmatter_field(fm, "scaffoldChecksum") {
        Some(stored) if stored == fnv1a_hex(body.trim_end().as_bytes()) => {
            MemoryDisposition::Pristine
        }
        _ => MemoryDisposition::Edited,
    }
}

/// Regenerate the `aida:scaffold-pack` block in the memory dir's MEMORY.md
/// index. Content outside the markers is the user's and is preserved.
/// trace:STORY-255 | ai:claude
pub(crate) fn update_memory_index(mem_dir: &std::path::Path, files: &[(&str, &str)]) -> Result<()> {
    const START: &str = "<!-- aida:scaffold-pack:start -->";
    const END: &str = "<!-- aida:scaffold-pack:end -->";

    let mut entries = String::new();
    for (filename, template) in files {
        let template = normalize_line_endings(template);
        let (fm, _) = split_md_frontmatter(&template).unwrap_or(("", ""));
        let label = frontmatter_field(fm, "name").unwrap_or(filename);
        let desc = frontmatter_field(fm, "description").unwrap_or("");
        entries.push_str(&format!("- [{label}]({filename}) — {desc}\n"));
    }
    let block = format!("{START}\n{entries}{END}");

    let index_path = mem_dir.join("MEMORY.md");
    let content = if index_path.exists() {
        let existing = std::fs::read_to_string(&index_path)?;
        match (existing.find(START), existing.find(END)) {
            (Some(s), Some(e)) if e > s => {
                format!("{}{}{}", &existing[..s], block, &existing[e + END.len()..])
            }
            _ => {
                let sep = if existing.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                format!("{existing}{sep}{block}\n")
            }
        }
    } else {
        let skeleton = aida_core::templates::EMBEDDED_TEMPLATES
            .get("memories/MEMORY.md")
            .copied()
            .unwrap_or("# Project Memory Index");
        match (skeleton.find(START), skeleton.find(END)) {
            (Some(s), Some(e)) if e > s => {
                format!(
                    "{}{}{}\n",
                    &skeleton[..s],
                    block,
                    &skeleton[e + END.len()..]
                )
            }
            _ => format!("{skeleton}\n\n{block}\n"),
        }
    };
    std::fs::write(&index_path, content)?;
    Ok(())
}

/// Per-disposition counts from a starter-memory-pack scaffold/refresh run.
/// trace:STORY-255 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoryPackReport {
    /// Files that did not exist and were freshly written.
    pub(crate) written: usize,
    /// Pristine pack files overlaid with a newer version (`--refresh`).
    pub(crate) refreshed: usize,
    /// Files left as-is — already current, or `--with-memories` without
    /// `--refresh` finding an existing file.
    pub(crate) unchanged: usize,
    /// Scaffolded files the user has edited — kept (`--refresh`).
    pub(crate) kept_edited: usize,
    /// Files with no `aida-scaffold` marker — the user's own; kept.
    pub(crate) kept_user: usize,
}

/// Decide whether a memory template loads under an active `--focus`
/// subsystem. A memory with no top-level `subsystem:` frontmatter key is
/// **universal** and always loads. A tagged memory loads only when its
/// `subsystem:` value matches `focus` (case-insensitive). When `focus` is
/// `None` the full pack loads (every member passes). trace:STORY-362 | ai:claude
pub(crate) fn memory_matches_focus(template: &str, focus: Option<&str>) -> bool {
    let Some(focus) = focus else {
        return true; // no focus → full pack
    };
    let template = normalize_line_endings(template);
    let Some((fm, _)) = split_md_frontmatter(&template) else {
        return true; // malformed/no frontmatter → treat as universal
    };
    match frontmatter_field(fm, "subsystem") {
        Some(subsystem) => subsystem.eq_ignore_ascii_case(focus),
        None => true, // untagged → universal, always loads
    }
}

/// Write (or `--refresh`) the starter memory pack into `mem_dir`. Every
/// embedded `memories/*` template except the MEMORY.md skeleton is a pack
/// member; MEMORY.md's `aida:scaffold-pack` index block is regenerated.
/// The pure core of `scaffold_memory_pack` — takes the target dir directly
/// so it is testable without touching the real `$HOME`.
///
/// `focus` scopes the pack to a subsystem (STORY-362): when `Some`, only
/// universal (untagged) memories plus those whose `subsystem:` frontmatter
/// matches are written/indexed; `None` writes the full pack.
/// trace:STORY-255 | ai:claude
pub(crate) fn scaffold_memory_pack_into(
    mem_dir: &std::path::Path,
    refresh: bool,
    focus: Option<&str>,
) -> Result<MemoryPackReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;

    let all_members = EMBEDDED_TEMPLATES.iter().any(|(k, _)| {
        k.strip_prefix("memories/")
            .is_some_and(|n| n != "MEMORY.md")
    });
    if !all_members {
        anyhow::bail!("no starter memories are embedded in this build");
    }

    // trace:STORY-362 | ai:claude — apply the --focus subsystem filter:
    // untagged memories are universal and always pass.
    let mut files: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("memories/")
                .filter(|n| *n != "MEMORY.md")
                .map(|n| (n, *v))
        })
        .filter(|(_, template)| memory_matches_focus(template, focus))
        .collect();
    if files.is_empty() {
        anyhow::bail!(
            "no starter memories match the requested focus subsystem (and none are universal)"
        );
    }
    files.sort_by(|a, b| a.0.cmp(b.0));

    std::fs::create_dir_all(mem_dir)?;
    let mut report = MemoryPackReport::default();

    for (name, template) in &files {
        let dest = mem_dir.join(name);
        let scaffolded = build_scaffolded_memory(template)
            .with_context(|| format!("malformed starter memory template: {name}"))?;

        if !dest.exists() {
            std::fs::write(&dest, &scaffolded)?;
            report.written += 1;
            continue;
        }
        if !refresh {
            // Plain --with-memories never overwrites an existing file.
            report.unchanged += 1;
            continue;
        }
        let existing = std::fs::read_to_string(&dest)?;
        match memory_refresh_disposition(&existing) {
            MemoryDisposition::UserOwned => report.kept_user += 1,
            MemoryDisposition::Edited => report.kept_edited += 1,
            MemoryDisposition::Pristine => {
                if aida_core::scaffolding::generated_text_matches(&existing, &scaffolded) {
                    report.unchanged += 1;
                } else {
                    std::fs::write(&dest, &scaffolded)?;
                    report.refreshed += 1;
                }
            }
        }
    }

    update_memory_index(mem_dir, &files)?;
    Ok(report)
}

/// Write (or `--refresh`) the starter memory pack into the Claude Code
/// project memory dir for the current working directory. `focus` scopes the
/// pack to a subsystem (STORY-362); `None` writes the full pack.
/// trace:STORY-255 | ai:claude
pub(crate) fn scaffold_memory_pack(refresh: bool, focus: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the starter memory pack")?;
    let slug = process_probe::encode_cwd_for_projects(&cwd);
    let mem_dir = home
        .join(".claude")
        .join("projects")
        .join(&slug)
        .join("memory");

    let report = scaffold_memory_pack_into(&mem_dir, refresh, focus)?;

    println!();
    if refresh {
        println!("  {}:", "Memory pack refreshed".bold());
        println!(
            "    {} new · {} updated · {} unchanged · {} kept (edited) · {} kept (yours)",
            report.written.to_string().green(),
            report.refreshed.to_string().blue(),
            report.unchanged,
            report.kept_edited.to_string().yellow(),
            report.kept_user,
        );
    } else {
        println!("  {}:", "Starter memory pack".bold());
        println!(
            "    {} written · {} already present",
            report.written.to_string().green(),
            report.unchanged.to_string().yellow(),
        );
    }
    println!("    {}", mem_dir.display().to_string().dimmed());
    // trace:STORY-362 | ai:claude
    if let Some(focus) = focus {
        println!(
            "    {} scoped to subsystem '{}' (universal memories always included)",
            "focus:".dimmed(),
            focus
        );
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// STORY-410: existing-project substrate-drift discovery.
//
// `aida init --with-memories --refresh` (STORY-255) lets a project overlay
// newer scaffolding-pack memories onto its existing dir, preserving user
// edits via body-checksum. But the user has to KNOW the pack is stale to run
// it. `aida memories check` is the discoverability surface: it compares the
// local memory dir against the binary's embedded master pack and reports
// drift, without writing anything. `aida status` surfaces a one-line summary
// when the pack is significantly behind.
// trace:STORY-410 | ai:claude
// ──────────────────────────────────────────────────────────────────────────

/// One pack member's relationship to the binary's embedded master.
/// trace:STORY-410 | ai:claude
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum MemoryDriftState {
    /// The master ships this memory; the local dir doesn't have it.
    /// `--refresh` would write it fresh.
    Missing,
    /// Present locally and pristine but byte-different from the current
    /// embedded version — `--refresh` would overlay the newer master.
    Stale,
    /// Present locally, pristine, and byte-identical to the master.
    UpToDate,
    /// Present locally, scaffolded, but the body no longer matches its
    /// recorded checksum — the user edited it. `--refresh` keeps the edit.
    Edited,
    /// Present locally with no `aida-scaffold` marker — the user's own file
    /// shadowing a pack name. `--refresh` never touches it.
    UserOwned,
}

/// One row of `aida memories check` output.
/// trace:STORY-410 | ai:claude
#[derive(Debug)]
pub(crate) struct MemoryDriftRow {
    /// Pack-member filename (e.g. `feedback_advocate_not_be_passive.md`).
    pub(crate) name: String,
    /// `name:` frontmatter label, falling back to the filename.
    pub(crate) label: String,
    /// One-line `description:` from the master template, when present.
    pub(crate) description: String,
    pub(crate) state: MemoryDriftState,
}

/// The full drift report: every embedded pack member classified against the
/// local memory dir. Pure data — printing lives in `print_memory_drift`.
/// trace:STORY-410 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct MemoryDriftReport {
    pub(crate) rows: Vec<MemoryDriftRow>,
}

impl MemoryDriftReport {
    pub(crate) fn count(&self, state: MemoryDriftState) -> usize {
        self.rows.iter().filter(|r| r.state == state).count()
    }
    pub(crate) fn missing(&self) -> usize {
        self.count(MemoryDriftState::Missing)
    }
    pub(crate) fn stale(&self) -> usize {
        self.count(MemoryDriftState::Stale)
    }
    pub(crate) fn up_to_date(&self) -> usize {
        self.count(MemoryDriftState::UpToDate)
    }
    pub(crate) fn edited(&self) -> usize {
        self.count(MemoryDriftState::Edited)
    }
    pub(crate) fn user_owned(&self) -> usize {
        self.count(MemoryDriftState::UserOwned)
    }
    /// How many master-pack members `--refresh` would land (write or overlay).
    /// This is the number that matters for "how behind am I?".
    pub(crate) fn behind(&self) -> usize {
        self.missing() + self.stale()
    }
}

/// Compute the embedded `name:` label + `description:` for a master template.
pub(crate) fn master_memory_meta(template: &str) -> (String, String) {
    let template = normalize_line_endings(template);
    let (fm, _) = split_md_frontmatter(&template).unwrap_or(("", ""));
    let label = frontmatter_field(fm, "name").unwrap_or("").to_string();
    let desc = frontmatter_field(fm, "description")
        .unwrap_or("")
        .trim_matches('"')
        .to_string();
    (label, desc)
}

/// Classify every embedded scaffolding-pack memory against an on-disk memory
/// dir. The pure core of `aida memories check` — takes the dir directly so it
/// is testable without touching the real `$HOME`. Mirrors the marker-driven
/// selection in `scaffold_memory_pack_into` so the two surfaces never diverge.
/// trace:STORY-410 | ai:claude
pub(crate) fn compute_memory_drift_into(mem_dir: &std::path::Path) -> Result<MemoryDriftReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;

    let mut files: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("memories/")
                .filter(|n| *n != "MEMORY.md")
                .map(|n| (n, *v))
        })
        .collect();
    if files.is_empty() {
        anyhow::bail!("no starter memories are embedded in this build");
    }
    files.sort_by(|a, b| a.0.cmp(b.0));

    let mut report = MemoryDriftReport::default();
    for (name, template) in &files {
        let scaffolded = build_scaffolded_memory(template)
            .with_context(|| format!("malformed starter memory template: {name}"))?;
        let (fm_label, description) = master_memory_meta(template);
        let label = if fm_label.is_empty() {
            name.to_string()
        } else {
            fm_label
        };

        let dest = mem_dir.join(name);
        let state = if !dest.exists() {
            MemoryDriftState::Missing
        } else {
            let existing = std::fs::read_to_string(&dest)?;
            match memory_refresh_disposition(&existing) {
                MemoryDisposition::UserOwned => MemoryDriftState::UserOwned,
                MemoryDisposition::Edited => MemoryDriftState::Edited,
                MemoryDisposition::Pristine => {
                    if aida_core::scaffolding::generated_text_matches(&existing, &scaffolded) {
                        MemoryDriftState::UpToDate
                    } else {
                        MemoryDriftState::Stale
                    }
                }
            }
        };

        report.rows.push(MemoryDriftRow {
            name: name.to_string(),
            label,
            description,
            state,
        });
    }
    Ok(report)
}

/// Resolve the Claude Code project memory dir for the current working
/// directory (same slug rule as `scaffold_memory_pack`).
pub(crate) fn project_memory_dir() -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the starter memory pack")?;
    let slug = process_probe::encode_cwd_for_projects(&cwd);
    Ok(home
        .join(".claude")
        .join("projects")
        .join(&slug)
        .join("memory"))
}

/// STORY-410 Phase 2: the `aida status` one-liner. Silent unless a local
/// memory pack exists AND is behind the binary's master — so a project that
/// never opted into `--with-memories` is never nagged, and a current pack
/// stays quiet (the line appearing is itself the signal). Best-effort: any
/// error (no $HOME, no embedded pack) degrades to silence rather than
/// breaking the status surface. trace:STORY-410 | ai:claude
pub(crate) fn print_status_memory_drift_section() {
    let Ok(mem_dir) = project_memory_dir() else {
        return;
    };
    // A project that never scaffolded the pack has no memory dir — don't nag.
    if !mem_dir.exists() {
        return;
    }
    let Ok(report) = compute_memory_drift_into(&mem_dir) else {
        return;
    };
    let behind = report.behind();
    if behind == 0 {
        return;
    }
    // Only surface when the pack is actually adopted (some scaffolded files
    // present). An all-missing dir means the user keeps their own memories
    // there but never took the pack — leave them alone.
    if report.up_to_date() + report.stale() + report.edited() == 0 {
        return;
    }
    println!(
        "  {} {} memor{} behind master — run {} for details",
        "Memory pack:".bold().yellow(),
        behind,
        if behind == 1 { "y" } else { "ies" },
        "aida memories check".cyan()
    );
    println!();
}

/// Append AIDA's `.gitignore` entries if any are missing. Returns `true` if a
/// new entry was written (so callers can echo "updated .gitignore"); `false`
/// if everything was already covered or the file had to be created from
/// scratch.
///
/// Writes two blocks:
/// - `<worktree_dir>/` (+ bare symlink form) — the orphan-branch worktree
///   (`.aida-store/` by default). Bare-name pattern needed because session
///   worktrees link to the canonical store. trace:EPIC-21 | ai:claude
/// - `.aida/*` deny-by-default + config allow-list entries — covers every
///   per-clone runtime file under `.aida/` (sessions,
///   roles, cache, session-env, server data, review prompts, future runtime
///   additions) without per-file whack-a-mole, while keeping team-shareable
///   project config tracked. trace:BUG-73 trace:TASK-877 | ai:claude
///
/// Migration: when invoked against a `.gitignore` from an older AIDA scaffold
/// (which listed each runtime path individually), the deny block is appended.
/// Legacy per-file entries become redundant but remain harmless.
pub(crate) fn add_aida_gitignore_entries(
    cwd: &std::path::Path,
    worktree_dir: &str,
) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = cwd.join(".gitignore");
    let store_entry = format!(
        "\n# AIDA distributed store (orphan branch worktree)\n\
         # Bare-name pattern catches session-worktree symlinks back to the\n\
         # canonical store. trace:EPIC-21 | ai:claude\n\
         {0}/\n\
         {0}\n",
        worktree_dir
    );
    let runtime_entry = "\n# AIDA runtime state — deny-by-default. Anything under .aida/ is\n\
         # per-clone runtime state (cache, sessions, roles, env shims, review\n\
         # prompts, server data, etc.) unless explicitly allow-listed below.\n\
         # Adding a new runtime file under .aida/ requires no gitignore change;\n\
         # tracking a new project-config file requires an explicit `!` line.\n\
         # trace:BUG-73 | ai:claude\n\
         .aida/*\n\
         \n\
         # Tracked exceptions: project-level config that lives in the repo.\n\
         !.aida/config.toml\n\
         # Project-level command aliases are shareable across the team.\n\
         # trace:TASK-877 | ai:claude\n\
         !.aida/aliases.toml\n\
         \n\
         # Per-project skill extensions are tracked project assets, not\n\
         # runtime state — they live under .claude/ which is tracked by\n\
         # default. Do not add ignore rules for .claude/skills/local/ or\n\
         # any <skill>.local.md file; the whole team should pick them up on\n\
         # `git pull`. See docs/extending-skills.md. trace:STORY-305 | ai:claude\n";

    // trace:STORY-781 | ai:claude — the project manifest is a TRACKED
    // project-config file under .aida/, so the deny-by-default `.aida/*` rule
    // needs an explicit allow-line or the manifest is silently gitignored and
    // never checked in — which would defeat the entire point of a manifest
    // that travels with the repository.
    //
    // This is deliberately a SEPARATE append rather than another line inside
    // `runtime_entry`: that block is only written when `.aida/*` is absent, so
    // an already-initialized project would never receive a newly-added
    // allow-line. Every existing repo needs this on its next `aida init`.
    let project_manifest_entry =
        "\n# The AIDA project manifest is checked in: it describes what this\n\
         # project IS (description, why it exists, liveness, stage, owner) and\n\
         # is meant to travel with the repository. trace:STORY-781 | ai:claude\n\
         !.aida/project.toml\n";

    let discipline_pack_entry =
        "\n# AIDA-using discipline guides are checked in with the project.\n\
         # trace:STORY-829 | ai:codex\n\
         !.aida/discipline/\n\
         !.aida/discipline/**\n";

    // trace:TASK-572 | ai:claude — CLAUDE.local.md is per-machine,
    // gitignored. Pattern mirrors CLAUDE.md but personal notes are
    // never team-shared. Comment block explains the convention so
    // operators new to the file see why.
    let claude_local_entry = "\n# Personal Claude Code notes — per-machine, never team-shared.\n\
         # CLAUDE.local.md loads alongside CLAUDE.md at session start but\n\
         # stays out of git so reviewer-feedback dumps + personal-habit\n\
         # reminders stay private. See the scaffolded file's own header\n\
         # for usage guidance.\n\
         CLAUDE.local.md\n";

    // trace:SPIKE-31 | ai:claude — `aida rules sync` regenerates path-
    // gated Claude Code rules from the spec graph. Committed copies would
    // thrash across worktrees as specs flip Active/Completed; keep them
    // per-clone.
    let rules_sync_entry = "\n# AIDA path-gated rules generated by `aida rules sync` — per-clone\n\
         # derived state. Hand-authored rules outside this subdir stay\n\
         # tracked. trace:SPIKE-31 | ai:claude\n\
         .claude/rules/aida-specs/\n";

    // trace:TASK-383 | ai:claude — a "planning pass" (a loop of `/aida-plan`
    // over queue items run from main) writes scratch plans to
    // docs/plans/_draft/. Gitignored so they never linger as untracked files
    // that abort a later PR's `git pull --ff-only` when the implementer's plan
    // lands at docs/plans/<same>.md. Promote a draft to docs/plans/ when adopted.
    let plans_draft_entry = "\n# AIDA planning-pass drafts — scratch plans; promote to\n\
         # docs/plans/<name>.md when adopted. Gitignored so they don't conflict\n\
         # with a later PR's landed plan. trace:TASK-383 | ai:claude\n\
         docs/plans/_draft/\n";

    // trace:BUG-484 — .claude/settings.local.json holds the per-user MCP
    // pre-approval (enabledMcpjsonServers: ["aida"]) that init scaffolds so a
    // fresh project trusts its own .mcp.json server. It MUST stay gitignored:
    // a committed pre-approval is the exact clone-attack vector Claude Code
    // guards against. Each clone's `aida init` grants its own local trust.
    let settings_local_entry = "\n# Per-user Claude Code overrides — never team-shared.\n\
         # .claude/settings.local.json holds local MCP trust (the aida server\n\
         # pre-approval) and personal settings. Gitignored so a committed\n\
         # pre-approval can't become a clone-attack vector. trace:BUG-484\n\
         .claude/settings.local.json\n";

    if !gitignore_path.exists() {
        std::fs::write(
            &gitignore_path,
            format!(
                "{}{}{}{}{}{}{}{}",
                store_entry,
                runtime_entry,
                project_manifest_entry,
                discipline_pack_entry,
                claude_local_entry,
                rules_sync_entry,
                plans_draft_entry,
                settings_local_entry
            ),
        )?;
        return Ok(false);
    }

    let content = std::fs::read_to_string(&gitignore_path)?;
    let mut wrote = false;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&gitignore_path)?;
    if !content.contains(worktree_dir) {
        file.write_all(store_entry.as_bytes())?;
        wrote = true;
    }
    if !has_aida_runtime_deny_pattern(&content) {
        file.write_all(runtime_entry.as_bytes())?;
        wrote = true;
    }
    // trace:STORY-781 — same idempotency pattern: match the bare path so an
    // operator-added variant doesn't produce a duplicate. Appended even when
    // the runtime block already exists, which is the whole point.
    if !content.lines().any(|line| {
        line.trim()
            .trim_start_matches('!')
            .trim_end_matches('/')
            .ends_with(".aida/project.toml")
    }) {
        file.write_all(project_manifest_entry.as_bytes())?;
        wrote = true;
    }
    if !gitignore_has_discipline_pack_allow_list(&content) {
        file.write_all(discipline_pack_entry.as_bytes())?;
        wrote = true;
    }
    // trace:TASK-572 | ai:claude — only append if not already covered.
    // Match against the bare filename to detect both this scaffolded
    // form and operator-added entries like `**/CLAUDE.local.md`.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("CLAUDE.local.md")
    }) {
        file.write_all(claude_local_entry.as_bytes())?;
        wrote = true;
    }
    // trace:SPIKE-31 | ai:claude — same idempotency pattern as
    // CLAUDE.local.md: scan for the bare path so operator-added globs
    // (e.g. `**/.claude/rules/aida-specs/`) don't trigger a duplicate.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with(".claude/rules/aida-specs")
    }) {
        file.write_all(rules_sync_entry.as_bytes())?;
        wrote = true;
    }
    // trace:TASK-383 | ai:claude — same idempotency pattern; match the bare
    // path so an operator-added glob doesn't trigger a duplicate.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("docs/plans/_draft")
    }) {
        file.write_all(plans_draft_entry.as_bytes())?;
        wrote = true;
    }
    // trace:BUG-484 — same idempotency pattern; match the bare path so an
    // operator-added glob (e.g. `**/settings.local.json`) doesn't duplicate it.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("settings.local.json")
    }) {
        file.write_all(settings_local_entry.as_bytes())?;
        wrote = true;
    }
    Ok(wrote)
}

/// Detect whether the deny-by-default `.aida/*` line is present (ignoring
/// comments and surrounding whitespace). trace:BUG-73 | ai:claude
pub(crate) fn has_aida_runtime_deny_pattern(content: &str) -> bool {
    content.lines().any(|line| line.trim() == ".aida/*")
}

pub(crate) fn gitignore_has_discipline_pack_allow_list(content: &str) -> bool {
    content.lines().any(|line| {
        line.trim()
            .trim_start_matches('!')
            .trim_end_matches('/')
            .ends_with(".aida/discipline/**")
    })
}

pub(crate) fn ensure_discipline_pack_gitignore_allow_list(cwd: &std::path::Path) -> Result<bool> {
    use std::io::Write;

    let gitignore_path = cwd.join(".gitignore");
    let Ok(content) = std::fs::read_to_string(&gitignore_path) else {
        return Ok(false);
    };
    if gitignore_has_discipline_pack_allow_list(&content) {
        return Ok(false);
    }
    let discipline_pack_entry =
        "\n# AIDA-using discipline guides are checked in with the project.\n\
         # trace:STORY-829 | ai:codex\n\
         !.aida/discipline/\n\
         !.aida/discipline/**\n";
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&gitignore_path)?;
    file.write_all(discipline_pack_entry.as_bytes())?;
    Ok(true)
}

// trace:BUG-588 | ai:claude
#[cfg(test)]
#[path = "tests/bug_588_history_id_resolves_uuid_tests.rs"]
mod bug_588_history_id_resolves_uuid_tests;

// trace:TASK-1480 | ai:claude
#[cfg(test)]
#[path = "tests/task_1480_history_id_alias_tests.rs"]
mod task_1480_history_id_alias_tests;

// trace:TASK-1507 | ai:claude
#[cfg(test)]
#[path = "tests/task_1507_history_cache_tests.rs"]
mod task_1507_history_cache_tests;

// trace:TASK-1508 | ai:claude
#[cfg(test)]
#[path = "tests/task_1508_history_source_tests.rs"]
mod task_1508_history_source_tests;

// trace:BUG-1631 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1631_history_spec_id_tests.rs"]
mod bug_1631_history_spec_id_tests;

// trace:BUG-1635 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1635_history_event_modes_tests.rs"]
mod bug_1635_history_event_modes_tests;

// trace:TASK-1512 | ai:claude
#[cfg(test)]
#[path = "tests/task_1512_history_transition_filters_tests.rs"]
mod task_1512_history_transition_filters_tests;

/// Detect if the current directory has a distributed store configured.
/// Walks up from CWD looking for `.aida/config.toml` with a store_path.
///
/// `AIDA_STORE` overrides everything: when it points at an existing
/// git-canonical store directory (one containing `objects/`), the whole CLI
/// (and the MCP server, which shares this resolver) retargets there instead of
/// walking up from CWD. This is the throwaway dev-playground / sandbox path
/// (SPIKE-48) — run drains and test scenarios against a discardable store
/// without touching the project's real `aida-store` orphan branch. The pointed
/// directory must already exist and look like a store; a missing or malformed
/// `AIDA_STORE` is ignored so a stale export never silently writes to a
/// half-formed location. trace:SPIKE-48 | ai:claude
/// trace:BUG-57 | ai:claude
pub(crate) fn detect_distributed_store() -> Option<std::path::PathBuf> {
    if let Some(store) = aida_store_override() {
        return Some(store);
    }
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// Run `command` against a resolved git-canonical store directory. The MCP
/// server and the Jira / GitHub / GitLab tracker commands take a `Storage`;
/// pointing it at the store directory makes its load/save delegate to
/// GitBackend, so their reads and writes land in `objects/` like every other
/// command's. Everything else goes through the git-backend dispatcher.
///
/// `project_root_hint`, when given, is used as `mcp-serve`'s project root
/// instead of walking cwd up to a `.git` — the explicit `--file <dir>` caller
/// passes its own directory, since that directory IS the project root there
/// (no separate project to find, and cwd may be unrelated to it entirely).
/// The other callers pass `None` and keep the existing cwd-walk behavior.
// trace:BUG-310 trace:TASK-1486 trace:TASK-1487 | ai:claude
pub(crate) fn run_on_distributed_store(
    command: &Command,
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
) -> Result<()> {
    let storage = Storage::new(store_path);
    match command {
        // MCP server reads/writes through the same canonical git store the CLI
        // uses. The previous YAML-snapshot approach wrote to
        // `.aida/mcp-cache.yaml` only — invisible to the CLI and overwritten on
        // every MCP restart. trace:BUG-310 | ai:claude
        Command::McpServe => {
            let project_root =
                mcp_serve_project_root(store_path, project_root_hint, find_project_root().ok());
            mcp::run_mcp_server(&storage, project_root)
        }
        // Tracker imports (`aida jira/github pull`) write new requirements
        // through this `Storage`, same as every other distributed-store
        // write path — they need the same node id and per-write auto-push
        // `handle_git_backend_command` gives its own callers, just reached
        // through a different signature (`&Storage`, not `&GitBackend`).
        // trace:TASK-1487 | ai:claude
        Command::Jira(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_jira_command(cmd, storage)
        }),
        Command::Github(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_github_command(cmd, storage)
        }),
        Command::Gitlab(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_gitlab_command(cmd, storage)
        }),
        _ => git_backend_cmd::handle_git_backend_command(store_path, command),
    }
}

/// Pure decision for `mcp-serve`'s project root: an explicit hint (the
/// `--file <dir>` caller's own directory) always wins over a cwd-derived git
/// root, since cwd may be unrelated to — or simply not inside — the
/// directory the user explicitly pointed at. Falls back to the store path
/// itself only when neither resolves. Takes the cwd-walk result as a plain
/// value (rather than calling `find_project_root()` itself) so this decision
/// stays unit-testable without touching the process's actual cwd.
///
/// The hint is a STORE path (the `--file`/`AIDA_STORE` convention: it points
/// at `<repo>/.aida-store`, the directory `GitBackend::new` expects
/// `objects/` directly under — see the ~20 `project_root.join(".aida-store")`
/// call sites), not the project root itself. Adopting it as-is regressed
/// leases, roles, mailbox and ledger paths (all keyed off the real project
/// root) and double-nested the store. `project_root_from_store_hint` derives
/// the real root from it instead.
// trace:TASK-1487 | ai:claude
pub(crate) fn mcp_serve_project_root(
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
    cwd_project_root: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    project_root_hint
        .map(project_root_from_store_hint)
        .or(cwd_project_root)
        .unwrap_or_else(|| store_path.to_path_buf())
}

/// Derive the project root a `--file <dir>`/`AIDA_STORE` hint implies, per
/// the store-path convention:
/// 1. The hint's final component is `.aida-store` → its parent is the root
///    (the overwhelmingly common case: `--file` pointed straight at the
///    worktree, the same as every other resolver in this codebase).
/// 2. Otherwise the hint already looks like a project root itself (it has
///    its own `.git` or `.aida/config.toml`) → use it as-is.
/// 3. Otherwise walk up from the hint looking for an enclosing project root,
///    guarded — like every other `.aida`-seeking walk-up in this codebase —
///    against ever adopting a shared system temp root (BUG-1598).
/// 4. Otherwise (a bare sandbox store — e.g. the `AIDA_STORE` dev-playground
///    path, SPIKE-48 — with no enclosing project) the hint has no project
///    root to point to; keep it as-is.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint(hint: &std::path::Path) -> std::path::PathBuf {
    project_root_from_store_hint_with_roots(hint, &aida_core::store_locate::real_temp_roots())
}

/// [`project_root_from_store_hint`], parameterized on the temp roots to
/// guard against, so a test can exercise the guard against a fake root
/// without touching the real, shared system temp dir.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint_with_roots(
    hint: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> std::path::PathBuf {
    if hint.file_name() == Some(std::ffi::OsStr::new(".aida-store")) {
        if let Some(parent) = hint.parent() {
            return parent.to_path_buf();
        }
    }
    if looks_like_project_root(hint) {
        return hint.to_path_buf();
    }
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = hint.parent();
    while let Some(dir) = current {
        // BUG-1598: never walk INTO a temp root and adopt it (or whatever's
        // in it) as the project — same guard every other walk-up here uses.
        if aida_core::store_locate::is_in_canonical_roots(dir, &canonical_roots) {
            break;
        }
        if looks_like_project_root(dir) {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    hint.to_path_buf()
}

/// Does `dir` look like an AIDA/git project root on its own — a `.git`
/// directory or an `.aida/config.toml` file? Shared by both the direct-hint
/// check and the walk-up in [`project_root_from_store_hint_with_roots`].
// trace:TASK-1487 | ai:claude
pub(crate) fn looks_like_project_root(dir: &std::path::Path) -> bool {
    dir.join(".git").exists() || dir.join(".aida").join("config.toml").exists()
}

/// Run a tracker (jira/github/gitlab) command against the distributed store
/// with the same two things `handle_git_backend_command` gives every other
/// distributed-store command — reused here, not reimplemented, since tracker
/// commands take a `&Storage` and can't go through that dispatcher's match:
/// - the dispenser's node id, so oplog entries this command writes (a tracker
///   pull's bulk import) aren't stamped node_id "0";
/// - the per-write store auto-push, gated the same way
///   `command_triggers_per_write_auto_push` gates every other write command,
///   and only fired on success (an error return skips it, same as the
///   git-backend dispatcher's post-match auto-push never running on an early
///   `?` return from inside its match).
// trace:TASK-1487 | ai:claude
pub(crate) fn run_tracker_command(
    store_path: &std::path::Path,
    command: &Command,
    handler: impl FnOnce(&Storage) -> Result<()>,
) -> Result<()> {
    let storage = Storage::new(store_path).with_dispenser(load_dispenser(store_path)?);
    handler(&storage)?;
    if command_triggers_per_write_auto_push(command) {
        maybe_auto_push_store(store_path, StoreAutoPushMode::PerWrite, "per-write");
    }
    Ok(())
}

/// Where `aida init --refresh` seeds missing type protocols.
// trace:TASK-1486 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshSeedTarget {
    /// A resolved store: the distributed store, or a legacy store in a project
    /// that is not distributed.
    Store(std::path::PathBuf),
    /// The project is distributed but its store isn't attached here. Seeding
    /// a legacy `requirements.db` would write into stale data.
    DistributedUnattached,
    /// No store resolves at all.
    NoStore,
}

/// Pure decision for [`RefreshSeedTarget`]. The legacy resolver runs only when
/// the project is not distributed, so a stale local `requirements.db` in a
/// distributed project is never chosen.
// trace:TASK-1486 | ai:claude
pub(crate) fn refresh_seed_target(
    distributed_store: Option<std::path::PathBuf>,
    distributed_unattached: bool,
    legacy: impl FnOnce() -> Result<std::path::PathBuf>,
) -> RefreshSeedTarget {
    if let Some(store) = distributed_store {
        return RefreshSeedTarget::Store(store);
    }
    if distributed_unattached {
        return RefreshSeedTarget::DistributedUnattached;
    }
    legacy()
        .map(RefreshSeedTarget::Store)
        .unwrap_or(RefreshSeedTarget::NoStore)
}

/// The project root when `start` is inside a distributed AIDA project: one
/// whose `.aida/config.toml` declares distributed mode, whose git repo has an
/// `aida-store` branch, or whose store is physically attached at
/// `<root>/.aida-store` despite neither of those (the BUG-433 shape: a
/// session worktree forked from a commit that predates the committed
/// scaffolding, or a symlink into another repo's store). The same signals the
/// main resolver uses (see the `distributed_root` computation above) to
/// refuse the legacy fallback when the store isn't resolvable through
/// `detect_distributed_store`.
// trace:TASK-1486 trace:BUG-428 trace:BUG-442 trace:BUG-433 trace:TASK-1487 | ai:claude
pub(crate) fn unattached_distributed_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from(start).or_else(|| {
        let root = start.ancestors().find(|d| d.join(".git").exists())?;
        if branch_exists_anywhere(root, "aida-store") {
            return Some(root.to_path_buf());
        }
        attached_store_present(root).then(|| root.to_path_buf())
    })
}

/// BUG-433: is a git-canonical store physically attached at `<root>/.aida-store`?
/// True when `.aida-store/objects/` is a directory — the hallmark of an attached
/// orphan-store worktree, resolved through a symlink too. Used to detect
/// distributed mode from the store's SHAPE when neither `.aida/config.toml` nor
/// an `aida-store` branch is present, so plain `aida` uses the real store
/// instead of silently serving legacy data. Delegates to `aida-core` so this
/// stays in lockstep with every other caller of the SAME resolver (e.g.
/// `aida-tui`'s mail scope).
// trace:BUG-433 trace:TASK-1141 | ai:claude
pub(crate) fn attached_store_present(project_root: &std::path::Path) -> bool {
    aida_core::store_locate::is_store_attached(project_root)
}

/// Classification of an `AIDA_STORE` value: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while the env wrapper decides
/// whether to print the BUG-567 fall-through notice. Re-exported from
/// `aida-core::store_locate`, the canonical resolver both this CLI and
/// `aida-tui` route through.
// trace:BUG-567 trace:TASK-1141 | ai:claude
use aida_core::store_locate::StoreOverride;

/// Resolve the `AIDA_STORE` env override into a usable store path, or `None`
/// when unset / pointing at something that isn't a git-canonical store. A valid
/// store directory is one that exists and contains an `objects/` subdirectory
/// (the per-spec YAML tree GitBackend manages). Validation is deliberately
/// strict — a typo'd or not-yet-created path falls THROUGH to normal
/// resolution rather than erroring, so the override is opt-in and never the
/// thing that breaks a forgotten-export shell (SPIKE-48). Use `aida sandbox
/// create` to produce a directory this accepts.
///
/// BUG-567 Finding 1: the fall-through is no longer SILENT. When `AIDA_STORE`
/// is set but unusable we emit exactly ONE stderr notice naming the path, the
/// reason, and that we fell back to normal resolution — then still fall
/// through (no error, no behavior change; informational only). Suppress with
/// `AIDA_QUIET` so scripts can mute it. The never-break-a-forgotten-export
/// intent (SPIKE-48) is preserved — we inform, we don't error.
/// trace:SPIKE-48 trace:BUG-567 | ai:claude
pub(crate) fn aida_store_override() -> Option<std::path::PathBuf> {
    let raw = std::env::var("AIDA_STORE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // trace:BUG-567 | ai:claude
    match aida_store_override_from(std::path::Path::new(raw)) {
        StoreOverride::Usable(p) => Some(p),
        StoreOverride::Unusable { reason } => {
            if !aida_quiet() {
                eprintln!(
                    "{} AIDA_STORE points at `{raw}` but it's unusable ({reason}); \
                     falling back to normal store resolution.\n  {} set AIDA_QUIET=1 \
                     to silence this, or point AIDA_STORE at a directory holding an \
                     `objects/` subdir (see `aida sandbox create`).",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    "→".cyan(),
                );
            }
            None
        }
    }
}

/// BUG-568: detect a "shared-store-but-multi-repo" context — a workspace where
/// a single `.aida-store` is shared by two or more code repos (via a
/// `.aida-workspace` manifest, as written by `aida init --sibling` /
/// `init_workspace`).
///
/// The completion/linkage scanners (`auto_bump_done_to_completed`,
/// `handle_db_reconcile_status`, `collect_git_linkage*`, `scan_trace_graph`)
/// each scan only the *local* repo's history/source tree. When the store spans
/// multiple repos, a spec whose referencing commit / trace comment lives in a
/// SIBLING repo is silently missed. Full multi-repo scanning is deferred to
/// SPIKE-62 (it needs the repo-identity model); until then we at least make the
/// limitation VISIBLE rather than silent.
///
/// Returns the other repo names (those whose `path` is not the current repo)
/// when a multi-repo workspace is detected, or `None` otherwise. The common
/// single-repo case (no manifest, or a manifest with <2 repos) returns `None`
/// → callers warn nothing → ZERO behavior change. The detection is cheap: walk
/// up from cwd for `.aida-workspace`, parse it, count repos.
// trace:BUG-568 | ai:claude
pub(crate) fn detect_multi_repo_shared_store(from: &std::path::Path) -> Option<Vec<String>> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    if manifest.repos.len() < 2 {
        return None;
    }
    // Identify which repo (if any) we're currently inside, so we can name the
    // OTHERS in the warning. Canonicalize both sides so a symlinked/relative
    // cwd still matches.
    let here = from.canonicalize().ok();
    let others: Vec<String> = manifest
        .repos
        .iter()
        .filter(|r| {
            let repo_abs = workspace_root.join(&r.path);
            match (&here, repo_abs.canonicalize().ok()) {
                (Some(h), Some(repo)) => !h.starts_with(&repo),
                _ => true,
            }
        })
        .map(|r| {
            if r.name.is_empty() {
                r.path.clone()
            } else {
                r.name.clone()
            }
        })
        .collect();
    Some(others)
}

/// Resolve the workspace repo slug for the repo containing `from` — the
/// canonical `origin.repo` join key (ADR-12) shared by specs and every
/// linkage artifact. `None` when no `.aida-workspace` manifest is
/// discoverable, or when `from` sits outside every manifest repo: the
/// single-repo case, where linkage stays unqualified (legacy bare-SHA
/// semantics, zero behavior change).
// trace:STORY-634 | ai:claude
pub(crate) fn workspace_repo_slug(from: &std::path::Path) -> Option<String> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    manifest.repo_slug_containing(&workspace_root, from)
}

/// BUG-568: emit ONE clear stderr warning that a completion/linkage scan
/// covered only the local repo while the store is shared across multiple repos,
/// so cross-repo completions/linkage may be missed. `scan_label` names the scan
/// (e.g. "auto-bump", "linkage scan") so the user can tell which surface was
/// limited. Suppressible via `AIDA_QUIET` (uniform with the BUG-567 store
/// fall-through notice). No-ops in the single-repo case (the detector returns
/// `None`). trace:BUG-568 | ai:claude
pub(crate) fn warn_multi_repo_scan_limited(from: &std::path::Path, scan_label: &str) {
    if aida_quiet() {
        return;
    }
    // Warn at most once per process — several scan sites can fire in one
    // command (e.g. `collect_git_linkage_opts` delegates to `scan_trace_graph`,
    // and a single `aida pull` runs auto-bump then renders linkage), and one
    // clear notice beats a wall of repeats. trace:BUG-568 | ai:claude
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let Some(others) = detect_multi_repo_shared_store(from) else {
        return;
    };
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let other_repos = if others.is_empty() {
        String::new()
    } else {
        format!(" (other repos sharing this store: {})", others.join(", "))
    };
    eprintln!(
        "{} {scan_label} covered only the local repo, but this store is shared across \
         multiple repos{other_repos}. Cross-repo completions/linkage may be missed \
         (full multi-repo scanning is not yet implemented).\n  {} run this from each \
         repo, or set AIDA_QUIET=1 to silence.",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        "→".cyan(),
    );
}

/// Is output suppression requested? BUG-567: honor `AIDA_QUIET` (any non-empty,
/// non-"0"/"false" value) so the informational store fall-through notice can be
/// muted by scripts. trace:BUG-567 | ai:claude
pub(crate) fn aida_quiet() -> bool {
    match std::env::var("AIDA_QUIET") {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

/// Path-resolution core of [`aida_store_override`], split out so it can be
/// unit-tested without mutating process env. Returns [`StoreOverride::Usable`]
/// (canonicalized) when `path` exists and holds an `objects/` directory, else
/// [`StoreOverride::Unusable`] carrying the specific reason it was rejected.
/// Delegates to `aida-core::store_locate::classify_store_path` — the
/// canonical resolver — so this CLI and any other caller (`aida-tui`'s mail
/// scope) never drift apart.
// trace:SPIKE-48 trace:BUG-567 trace:TASK-1141 | ai:claude
pub(crate) fn aida_store_override_from(path: &std::path::Path) -> StoreOverride {
    aida_core::store_locate::classify_store_path(path)
}

/// Walk up from `start` and return the project root whose `.aida/config.toml`
/// declares `mode = "distributed"`. Used to distinguish two states that look
/// identical to [`detect_distributed_store`] (both return `None` for the
/// store path): a *legacy* single-file project (no distributed config → the
/// YAML/SQLite fallback is correct) vs. a *distributed* project whose
/// `.aida-store/` worktree just isn't set up in this working copy — the
/// hallmark of a fresh clone, since the worktree is gitignored and only
/// created by `aida init`. In the latter case the caller must refuse the
/// legacy fallback (it would show stale data) and point at `aida init`.
/// trace:BUG-428 | ai:claude
pub(crate) fn distributed_mode_declared_from(
    start: &std::path::Path,
) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from_with_roots(start, &aida_core::store_locate::real_temp_roots())
}

/// [`distributed_mode_declared_from`], parameterized on the temp roots to
/// guard against. Must agree with `detect_distributed_store_from` on where
/// the walk-up stops — otherwise a stray `.aida/config.toml` sitting
/// directly in a temp root (see `aida_core::store_locate` for why that
/// happens) makes THIS function claim distributed mode where the store
/// resolver finds nothing, producing a misleading "run aida init" refusal
/// instead of the correct legacy fallback. Factored out (rather than calling
/// `is_system_temp_dir` inline) so a test can exercise the guard against a
/// fake root without mutating `TMPDIR` or touching the real, shared system
/// temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn distributed_mode_declared_from_with_roots(
    start: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = start;
    loop {
        if aida_core::store_locate::is_in_canonical_roots(current, &canonical_roots) {
            return None;
        }
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // The first `.aida/config.toml` we hit walking up decides the
            // mode — a config without distributed mode means a legacy
            // project, so stop (don't keep walking to a parent project).
            return config_declares_distributed(&content).then(|| current.to_path_buf());
        }
        current = current.parent()?;
    }
}

pub(crate) fn distributed_mode_declared() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    distributed_mode_declared_from(&cwd)
}

/// TASK-621: attach the `.aida-store` worktree from the existing `aida-store`
/// branch so read commands "just work" on a fresh clone without a manual
/// `aida init`. Reuses the exact fetch + worktree-add sequence the post-clone
/// init bootstrap uses (handle_init_post_clone) — but NOT its node-id /
/// scaffolding steps. Those stay with `aida init` / `aida node acquire`: a
/// node id is the namespace for newly-issued spec ids, needed only before
/// WRITING. Reading the store needs only the worktree. Idempotent at the
/// git-ops layer (create_store_worktree no-ops when the worktree already
/// exists), but in practice only called when the worktree is absent.
/// trace:TASK-621 | ai:claude
pub(crate) fn try_attach_store_worktree(
    project_root: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use aida_core::git_ops;
    let worktree_dir = ".aida-store";
    let branch = "aida-store";
    // BUG-559: on a fresh clone of an AIDA-on-GitLab repo the working tree can
    // land ON the orphan `aida-store` branch — GitLab's push-to-create adopts
    // the first-pushed branch as the project default, so a clone checks out the
    // internal YAML store as if it were the code. In that state
    // `git fetch origin aida-store:aida-store` refuses ("refusing to fetch into
    // branch refs/heads/aida-store checked out at <clone>") because git won't
    // write into a branch ref that's currently checked out.
    //
    // Fix (b) — harden auto-attach: instead of erroring, RECOVER. Switch the
    // working tree OFF `aida-store` first — to a code branch (`main`/`master`,
    // local or origin) when one exists, else to a detached HEAD — which frees
    // the `aida-store` ref so the fetch + worktree-add can proceed exactly as
    // on a healthy clone. This only fires in the already-failing GitLab state
    // (`aida-store` is the checked-out branch); a normal clone is untouched, so
    // it cannot regress the working path. The source-side prevention (making
    // the GitLab default branch `main` at init) is tracked separately under
    // BUG-559's follow-up.
    // trace:TASK-821 trace:BUG-559 | ai:claude
    if git_ops::current_branch(project_root).ok().as_deref() == Some(branch) {
        let candidates = local_code_branch_candidates(project_root);
        match choose_store_attach_recovery(project_root, &candidates) {
            StoreAttachRecovery::CheckoutCodeBranch(code_branch) => {
                git_ops::checkout_branch(project_root, &code_branch)?;
            }
            StoreAttachRecovery::DetachHead => {
                git_ops::detach_head(project_root)?;
            }
        }
    }
    // Create the local tracking branch from origin/<branch> (network fetch;
    // fails closed → caller drops to the explicit `aida init` guidance).
    git_ops::fetch_branch_into_local(project_root, "origin", branch)?;
    git_ops::create_store_worktree(project_root, worktree_dir, branch)
}

/// The recovery action chosen when `aida-store` is the checked-out branch on a
/// fresh clone (the BUG-559 GitLab state). Pure value so the decision is
/// unit-testable without running git. trace:BUG-559 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreAttachRecovery {
    /// Switch the working tree to this code branch before fetching `aida-store`.
    CheckoutCodeBranch(String),
    /// No code branch is available — detach HEAD to free the `aida-store` ref.
    DetachHead,
}

/// The conventional code-branch names to prefer when recovering, in priority
/// order. trace:BUG-559 | ai:claude
pub(crate) const CODE_BRANCH_PREFERENCE: [&str; 2] = ["main", "master"];

/// Collect the code-branch names available to switch to, in preference order:
/// a local `main`/`master` if it exists, else an `origin/main`/`origin/master`
/// (referenced by short name so `git checkout` sets up tracking). Used as input
/// to [`choose_store_attach_recovery`]; split out so the decision logic stays
/// pure and testable. trace:BUG-559 | ai:claude
pub(crate) fn local_code_branch_candidates(project_root: &std::path::Path) -> Vec<String> {
    use aida_core::git_ops;
    let mut out = Vec::new();
    for name in CODE_BRANCH_PREFERENCE {
        if git_ops::local_branch_exists(project_root, name) {
            out.push(name.to_string());
        } else if git_ops::remote_branch_exists(project_root, "origin", name) {
            // `git checkout main` against `origin/main` creates the local
            // tracking branch — git's DWIM behaviour.
            out.push(name.to_string());
        }
    }
    out
}

/// Decide how to get the working tree off the `aida-store` branch so a fetch
/// into the `aida-store` ref can succeed: check out the first available code
/// branch, or detach HEAD when none is available. Pure — given the candidate
/// list it returns the action, no git side effects. trace:BUG-559 | ai:claude
pub(crate) fn choose_store_attach_recovery(
    _project_root: &std::path::Path,
    code_branch_candidates: &[String],
) -> StoreAttachRecovery {
    match code_branch_candidates.first() {
        Some(branch) => StoreAttachRecovery::CheckoutCodeBranch(branch.clone()),
        None => StoreAttachRecovery::DetachHead,
    }
}

/// What `aida init` should push to a fresh/empty origin, and in what order, so a
/// forge that adopts the first-pushed branch as its default (GitLab's
/// push-to-create) ends up with the **code** branch as default — never the
/// orphan `aida-store` (which would make a fresh clone check out the internal
/// YAML store as if it were the project). Root-cause prevention for BUG-559.
/// trace:TASK-844 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InitPushPlan {
    /// A code branch exists and has commits → push it FIRST (so the forge
    /// adopts it as default), then the orphan store. The string is the code
    /// branch name to push.
    CodeBranchFirst(String),
    /// No code branch with commits to push (brand-new repo / nothing committed)
    /// → fall back to the legacy behavior: push only the orphan store. Pushing
    /// uncommitted/empty code would surprise the user (blast-radius guardrail),
    /// so we don't. The set-default-branch call can still run after the push.
    OrphanOnly,
}

/// Decide the init push plan from two facts about the working repo: whether a
/// code branch (`main`/`master`) **exists**, and whether it **has commits**.
/// Pure so the push-order decision is unit-testable without a live remote.
///
/// - code branch exists AND has commits → [`InitPushPlan::CodeBranchFirst`]
/// - otherwise → [`InitPushPlan::OrphanOnly`] (conservative fallback; never
///   pushes code the user hasn't committed).
///
/// trace:TASK-844 | ai:claude
pub(crate) fn decide_init_push_plan(
    code_branch: Option<&str>,
    code_branch_has_commits: bool,
) -> InitPushPlan {
    match code_branch {
        Some(name) if code_branch_has_commits => InitPushPlan::CodeBranchFirst(name.to_string()),
        _ => InitPushPlan::OrphanOnly,
    }
}

#[cfg(test)]
#[path = "tests/task_844_init_push_order_tests.rs"]
mod task_844_init_push_order_tests;

#[cfg(test)]
#[path = "tests/bug_559_clone_recovery_tests.rs"]
mod bug_559_clone_recovery_tests;

/// Walk-up resolver split out from `detect_distributed_store` so the search
/// path is testable without changing process cwd. Returns the absolute store
/// path on the first ancestor whose `.aida/config.toml` declares one.
///
/// Delegates to `aida-core::store_locate::detect_distributed_store_from` —
/// the canonical resolver (including its BUG-331 main-worktree fallback for
/// a linked/nested git worktree) — so this CLI and any other caller
/// (`aida-tui`'s mail scope) resolve to the exact same project root.
// trace:BUG-57 trace:BUG-331 trace:TASK-1141 | ai:claude
pub(crate) fn detect_distributed_store_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    aida_core::store_locate::detect_distributed_store_from(start)
}

/// Read the `[id_format] policy` from `.aida/config.toml`. Honors the legacy
/// `use_agreed_blocks` boolean as a fallback when the new section is missing
/// (so existing projects keep working unchanged).
///
/// Resolution order:
///   1. `[id_format] policy = "..."`  → parsed (errors on unknown values)
///   2. legacy `use_agreed_blocks = false`  → `node-aware-only`
///   3. legacy `use_agreed_blocks = true`   → `blocks-then-fallback`
///   4. neither set                          → `blocks-then-fallback` (default)
///      trace:EPIC-1-052 | ai:claude
///      Read the agreed-id counter for a given type from the orphan store's
///      `registry/agreed_counters.toml`. Returns 0 when the file doesn't exist
///      or the type has no entry — both mean "no ids issued yet for this type".
///      Used as the floor when claiming a new block so the block doesn't
///      overlap with already-issued agreed-ids.
///      trace:FR-1-073 | ai:claude
pub(crate) fn read_agreed_counter(store_path: &std::path::Path, type_prefix: &str) -> u32 {
    let path = store_path.join("registry").join("agreed_counters.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let prefix_upper = type_prefix.to_uppercase();
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let (key, val) = match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => (k.trim().trim_matches('"'), v.trim()),
            _ => continue,
        };
        if key.eq_ignore_ascii_case(&prefix_upper) {
            return val.parse::<u32>().unwrap_or(0);
        }
    }
    0
}

pub(crate) fn read_id_format_policy(project_dir: &std::path::Path) -> aida_core::IdFormatPolicy {
    read_id_format_settings(project_dir).0
}

/// Read counter_scope from `.aida/config.toml`. When absent, defaults to
/// PerType (back-compat — flipping a live store would conflate FR-100
/// and BUG-100 numerically). Projects created from 2026-05-09 onwards
/// have `counter_scope = "global"` written explicitly at init.
/// trace:FR-271 | ai:claude
#[allow(dead_code)]
pub(crate) fn read_id_counter_scope(project_dir: &std::path::Path) -> aida_core::IdCounterScope {
    read_id_format_settings(project_dir).1
}

/// Single pass over `.aida/config.toml` returning both the policy and
/// the counter scope. Cheaper than two separate reads when a caller
/// needs both. trace:FR-271 | ai:claude
pub(crate) fn read_id_format_settings(
    project_dir: &std::path::Path,
) -> (aida_core::IdFormatPolicy, aida_core::IdCounterScope) {
    let config_path = project_dir.join(".aida").join("config.toml");
    let default_policy = aida_core::IdFormatPolicy::default();
    let default_scope = aida_core::IdCounterScope::default();
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return (default_policy, default_scope);
    };

    let mut in_id_format = false;
    let mut policy_explicit: Option<aida_core::IdFormatPolicy> = None;
    let mut scope_explicit: Option<aida_core::IdCounterScope> = None;
    let mut legacy_use_blocks: Option<bool> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_id_format = line == "[id_format]";
            continue;
        }
        if in_id_format && line.starts_with("policy") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdFormatPolicy::parse(s) {
                    Ok(p) => policy_explicit = Some(p),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if in_id_format && line.starts_with("counter_scope") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdCounterScope::parse(s) {
                    Ok(c) => scope_explicit = Some(c),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if !in_id_format && line.starts_with("use_agreed_blocks") {
            if let Some(val) = line.split('=').nth(1) {
                legacy_use_blocks = Some(val.trim() != "false");
            }
        }
    }

    let policy = policy_explicit.unwrap_or(match legacy_use_blocks {
        Some(false) => aida_core::IdFormatPolicy::NodeAwareOnly,
        Some(true) => aida_core::IdFormatPolicy::BlocksThenFallback,
        None => default_policy,
    });
    let scope = scope_explicit.unwrap_or(default_scope);
    (policy, scope)
}

// trace:TASK-583 | ai:antigravity
pub(crate) fn extract_bughunter_severity(body: &str) -> Option<serde_json::Value> {
    for marker in &[
        "bughunter-severity:",
        "\"bughunter-severity\":",
        "'bughunter-severity':",
    ] {
        if let Some(idx) = body.find(marker) {
            let rest = &body[idx + marker.len()..];
            if let Some(start_brace) = rest.find('{') {
                if let Some(end_brace) = rest[start_brace..].find('}') {
                    let json_str = &rest[start_brace..=start_brace + end_brace];
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        return Some(val);
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn file_reviewer_verdict_unavailable_finding(
    project_root: &std::path::Path,
    pr_number: u32,
) -> anyhow::Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        anyhow::bail!("no distributed store found");
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let title = format!("Reviewer verdict unavailable for PR {}", pr_number);
    let note = "The delegated reviewer failed to provide a verdict. \
         Check-run or bughunter-severity data was missing, malformed, or timed out."
        .to_string();
    let mut req = aida_core::Requirement::new(title, note);
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::Draft;
    req.owner = get_default_author();

    req.tags.insert(format!("from-review:PR-{}", pr_number));
    req.tags
        .insert("kind:ReviewerVerdictUnavailable".to_string());
    req.tags.insert("severity:major".to_string());

    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
    })?;
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("add_requirement_with_id produced no requirement"))?;
    aida_core::object_store::write_object(&store_path.join("objects"), &written)?;

    let display_id = written.spec_id.as_deref().unwrap_or("?");
    record_role_activity(display_id, "findings-add");
    Ok(())
}

pub(crate) fn file_agent_gate_warning_finding(
    project_root: &std::path::Path,
    spec: &str,
    pr_number: u32,
    gate: &AgentGateConfig,
    verdict: auto_complete::Verdict,
) -> anyhow::Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        anyhow::bail!("no distributed store found");
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let title = format!(
        "Agent gate `{}` warned on {} for PR {}",
        gate.name, spec, pr_number
    );
    let note = format!(
        "The agent gate `{}` (role `{}`) returned {} for PR {}. \
         The gate is configured with on_fail='warn', so the drain continued.",
        gate.name,
        gate.role,
        verdict.label(),
        pr_number
    );
    let mut req = aida_core::Requirement::new(title, note);
    req.req_type = aida_core::RequirementType::Task;
    req.status = aida_core::RequirementStatus::Draft;
    req.owner = get_default_author();
    req.tags.insert(format!("from-review:PR-{}", pr_number));
    req.tags.insert(format!("from-advisor:{spec}"));
    req.tags.insert("kind:AgentGateWarning".to_string());
    req.tags.insert("severity:major".to_string());

    // CR-8: stamp filing provenance BEFORE the store write — the object is
    // rewritten below from the in-memory copy. trace:CR-8 | ai:claude
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
    })?;
    let written = store
        .requirements
        .last()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("add_requirement_with_id produced no requirement"))?;
    aida_core::object_store::write_object(&store_path.join("objects"), &written)?;

    let display_id = written.spec_id.as_deref().unwrap_or("?");
    record_role_activity(display_id, "findings-add");
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1421_carry_forward_findings_tests.rs"]
mod story_1421_carry_forward_findings_tests;

/// Pure planner behind [`try_emit_nonblocking_findings_on_completion`]: which
/// of `verdict`'s findings still need a successor filed, given the
/// `finding-hash:<hex>` tags already on record for this spec (`already_filed`).
///
/// Empty when `verdict` is not APPROVED (a blocking verdict's findings are
/// rework, not the "carried forward on an approval" case this spec covers)
/// or carries no findings at all. De-dupes both against the caller's history
/// AND within the verdict's own findings list (an accidental duplicate line),
/// so a re-run over an already-completed spec — or a verdict repeating a
/// finding — never yields the same text twice.
///
/// No I/O — takes an already-parsed [`review_verdict::RecordedVerdict`], so
/// this is exercised directly by unit tests; the git-store-touching shell
/// around it (`try_emit_nonblocking_findings_on_completion`) is not.
// trace:STORY-1421 | ai:claude
pub(crate) fn findings_needing_a_successor(
    verdict: &review_verdict::RecordedVerdict,
    already_filed: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    if !verdict.kind.approves() {
        return Vec::new();
    }
    // BUG-1799: The sweep skips findings whose round predates the approving round.
    // If findings were inherited verbatim (no --finding passed), they predate the
    // approving round. If they were explicitly re-recorded, they carry forward.
    // trace:BUG-1799 | ai:antigravity
    if verdict.inherited_findings {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    verdict
        .findings
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter_map(|text| {
            let hash_tag = format!("finding-hash:{}", &fnv1a_hex(text.as_bytes())[..8]);
            if already_filed.contains(&hash_tag) || !seen.insert(hash_tag.clone()) {
                None
            } else {
                Some((text, hash_tag))
            }
        })
        .collect()
}

/// STORY-1421: a non-blocking finding recorded on an APPROVED review verdict
/// has no successor once its spec reaches a terminal state — every surface
/// that would have surfaced it (reviewer queue, `aida awaiting`, the open
/// lens) stops looking, and the finding survives only inside a verdict record
/// keyed to a spec nobody reads any more. Called from the same completion
/// path `auto_bump_done_to_completed` / `close_verdict_on_merge` (BUG-1529)
/// already use, this gives each such finding a home in `aida findings` — the
/// existing triage surface — instead of a new child spec on the graph.
///
/// Best-effort: emission must never fail the pull/bump it rides along with.
/// Any error is logged to stderr and swallowed.
// trace:STORY-1421 | ai:claude
pub(crate) fn emit_nonblocking_findings_on_completion(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    spec_id: &str,
    sha: &str,
    pr_hint: Option<u64>,
) {
    if let Err(e) =
        try_emit_nonblocking_findings_on_completion(project_root, store_path, spec_id, sha, pr_hint)
    {
        eprintln!("warning: could not carry forward non-blocking findings for {spec_id}: {e:#}");
    }
}

/// Idempotent worker for [`emit_nonblocking_findings_on_completion`]. Keyed on
/// the source spec plus a hash of the finding text (`carried-from:<SPEC>` +
/// `finding-hash:<hex>` tags), so a re-run of the auto-bump scan over an
/// already-completed spec never files the same finding twice. Returns the
/// number of NEW findings filed.
///
/// BUG-1506: every new finding is created through `DatabaseBackend::
/// add_requirement` — the SAME targeted, single-object write `aida add`
/// itself uses on the git-canonical store (`git_backend_cmd.rs`'s add
/// handler). It only ever reads store METADATA (counters) to assign the new
/// spec_id, then writes exactly the one new object + a targeted `add
/// SPEC-ID` commit; it never loads or overwrites the full requirements list.
/// The old `CachedGitBackend::update_atomically` path this replaced does a
/// full-store load-then-save, which would silently drop any spec a
/// concurrent `aida add` wrote in between — exactly the class of bug
/// BUG-1506 was filed over, and live here because this runs inside `aida
/// pull`'s auto-bump, right after drain merges, when concurrent `aida add`
/// is common.
// trace:STORY-1421 trace:BUG-1506 | ai:claude
pub(crate) fn try_emit_nonblocking_findings_on_completion(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    spec_id: &str,
    sha: &str,
    pr_hint: Option<u64>,
) -> anyhow::Result<usize> {
    let verdict_file = review_verdict::verdict_path(project_root, spec_id);
    let Ok(body) = std::fs::read_to_string(&verdict_file) else {
        return Ok(0);
    };
    let Some(verdict) = review_verdict::parse_recorded_verdict(&body) else {
        return Ok(0);
    };
    // Cheap pre-check against an empty history: if nothing would ever be filed
    // regardless of what's already on record (not approved, or no findings),
    // skip opening the store entirely.
    if findings_needing_a_successor(&verdict, &std::collections::HashSet::new()).is_empty() {
        return Ok(0);
    }

    use aida_core::db::DatabaseBackend;
    let dispenser = load_dispenser(store_path)?;
    let inner = aida_core::GitBackend::new(store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let spec_upper = spec_id.trim().to_ascii_uppercase();
    let carried_from_tag = format!("carried-from:{spec_upper}");

    // Resolve the PR the review happened against, when not already known —
    // same lookup `emit_spec_completed` uses (commit-subject trailer off the
    // landing sha).
    let pr_number = pr_hint.or_else(|| {
        if sha.is_empty() {
            return None;
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args([
                "show",
                "-s",
                "--format=%s",
                git_arg_guard::END_OF_OPTIONS,
                sha,
                "--",
            ]) // trace:BUG-1622 | ai:claude
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        extract_pr_number_from_commit_subject(String::from_utf8_lossy(&output.stdout).trim())
    });
    let review_tag = match pr_number {
        Some(n) => format!("{}PR-{n}", findings::FROM_REVIEW_PREFIX),
        None => format!("{}{}", findings::FROM_REVIEW_PREFIX, spec_upper),
    };
    let reviewed_sha = verdict.reviewed_sha.clone().unwrap_or_default();

    // Idempotency: gather the hashes already on file for this spec (one cheap
    // tag-filtered cache query), then let the pure planner in
    // `findings_needing_a_successor` decide which of THIS verdict's findings
    // are genuinely new. A re-run over an already-completed spec (the
    // auto-bump scan can re-observe old history) then files nothing.
    // Strict: a snapshot served while another process writes the cache can
    // miss a successor filed since, and this set is what prevents a duplicate.
    // trace:BUG-1670 | ai:claude
    let already_filed: std::collections::HashSet<String> = backend
        .list_summaries_strict(&aida_core::ListFilter {
            tags: vec![carried_from_tag.clone()],
            archive: aida_core::ArchiveFilter::Both,
            ..Default::default()
        })?
        .iter()
        .flat_map(|r| r.tags.iter())
        .filter(|t| t.starts_with("finding-hash:"))
        .cloned()
        .collect();
    let to_file = findings_needing_a_successor(&verdict, &already_filed);

    let mut filed = 0usize;
    for (text, hash_tag) in to_file {
        const TITLE_MAX: usize = 80;
        let first_line = text.lines().next().unwrap_or(text.as_str()).trim();
        let title = if first_line.chars().count() > TITLE_MAX {
            let truncated: String = first_line.chars().take(TITLE_MAX - 1).collect();
            format!("{truncated}…")
        } else {
            first_line.to_string()
        };

        let mut note = format!(
            "Non-blocking finding carried forward from an APPROVED review verdict \
             on {spec_upper}, which has since completed — recorded here so it keeps \
             a queryable home once the verdict record stops being read.\n\n{text}"
        );
        if let Some(n) = pr_number {
            note.push_str(&format!("\n\nPR: #{n}"));
        }
        if !reviewed_sha.is_empty() {
            note.push_str(&format!("\nReviewed sha: {reviewed_sha}"));
        }

        let mut req = aida_core::Requirement::new(title, note);
        req.req_type = aida_core::RequirementType::Task;
        req.status = aida_core::RequirementStatus::Draft;
        req.owner = get_default_author();
        req.tags.insert(review_tag.clone());
        req.tags.insert(carried_from_tag.clone());
        req.tags.insert(hash_tag);
        req.tags.insert("kind:carried-forward".to_string());
        // `spec_id` left unset — the targeted `add_requirement` write below
        // assigns it (reading only the store's small metadata/counters file,
        // never the full requirements list).

        let written = backend.add_requirement(req)?;

        let display_id = written.spec_id.as_deref().unwrap_or("?");
        record_role_activity(display_id, "findings-add");
        filed += 1;
    }
    Ok(filed)
}

pub(crate) fn read_review_mode(project_root: &std::path::Path) -> String {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return "local".to_string();
    };
    parse_review_mode(&content)
}

pub(crate) fn parse_review_mode(content: &str) -> String {
    let mut in_review = false;
    for raw in content.lines() {
        let line = match raw.split('#').next() {
            Some(l) => l.trim(),
            None => continue,
        };
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_review = rest.trim_end_matches(']').trim() == "review";
            continue;
        }
        if !in_review {
            continue;
        }
        if let Some(rest) = line.strip_prefix("mode") {
            let val = match rest.split('=').nth(1) {
                Some(v) => v.trim().trim_matches('"').trim_matches('\'').to_string(),
                None => continue,
            };
            return val;
        }
    }
    "local".to_string()
}

#[cfg(test)]
#[path = "tests/delegated_reviewer_tests.rs"]
mod delegated_reviewer_tests;

/// Read node_id from the store's node.toml; defaults to 1 for unregistered nodes.
pub(crate) fn load_node_id(store_path: &std::path::Path) -> String {
    use aida_core::NodeConfig;
    let node_config_path = store_path.join(".aida").join("node.toml");
    if node_config_path.exists() {
        NodeConfig::load(&node_config_path)
            .map(|c| c.node_id)
            .unwrap_or_else(|_| "1".to_string())
    } else {
        // Fall back to dispenser.toml node_id (still a stringified-numeric
        // for legacy stores written before EPIC-9). trace:STORY-41
        let dispenser_path = store_path.join(".aida").join("dispenser.toml");
        if dispenser_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&dispenser_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("node_id") {
                        if let Some(val) = line.split('=').nth(1) {
                            let trimmed = val.trim().trim_matches('"');
                            if !trimmed.is_empty() {
                                return trimmed.to_string();
                            }
                        }
                    }
                }
            }
        }
        "1".to_string()
    }
}

/// Get the local hostname for informational block registry labels.
pub(crate) fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Load or create the distributed dispenser for a git-backed store.
/// Reads node config from {store}/.aida/node.toml; defaults to node_id=1
/// if no node registration has happened yet.
pub(crate) fn load_dispenser(
    store_path: &std::path::Path,
) -> Result<aida_core::models::DispenserHandle> {
    use aida_core::dispenser::{FileDispenser, IdMode};
    use aida_core::node::NodeConfig;

    let aida_dir = store_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;

    let node_config_path = aida_dir.join("node.toml");
    let dispenser_path = aida_dir.join("dispenser.toml");

    // Load node_id from config, or default to "1" for local-only.
    // trace:STORY-41 | ai:claude
    let node_id: String = if node_config_path.exists() {
        NodeConfig::load(&node_config_path)?.node_id
    } else {
        "1".to_string() // default for unregistered local node
    };

    let mode = IdMode::Distributed { node_id };
    let dispenser = FileDispenser::open(dispenser_path, mode)?;

    Ok(aida_core::models::DispenserHandle(std::sync::Arc::new(
        dispenser,
    )))
}

/// TASK-102: human-readable phrase for a relationship edge, shared by
/// `aida show`'s inline enumeration and the centralized renderer so the two
/// stay consistent. trace:TASK-102 | ai:claude
pub(crate) fn relationship_phrase(rt: &aida_core::models::RelationshipType) -> String {
    use aida_core::models::RelationshipType as R;
    match rt {
        R::Parent => "is parent of".to_string(),
        R::Child => "is child of".to_string(),
        R::Duplicate => "is duplicate of".to_string(),
        R::Verifies => "verifies".to_string(),
        R::VerifiedBy => "is verified by".to_string(),
        R::References => "references".to_string(),
        R::BlockedBy => "is blocked by".to_string(),
        R::Blocks => "blocks".to_string(),
        // trace:TASK-1176 | ai:claude
        R::SupersededBy => "is superseded by".to_string(),
        R::Supersedes => "supersedes".to_string(),
        R::Custom(name) => name.clone(),
    }
}

/// STORY-446: add a BlockedBy edge from `spec_id` to `blocker_id`, plus the
/// inverse Blocks edge on the blocker — atomically and idempotently. Returns
/// the blocker's display id for messaging. The pickability gate
/// (`aida_core::pickability`) already matches the typed BlockedBy variant, so
/// this is purely the ergonomic declaration path (STORY-333 added the variant).
/// trace:STORY-446 | ai:claude
pub(crate) fn add_blocked_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    blocker_id: &str,
) -> Result<String> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let blocker = backend
        .get_requirement_unambiguous(blocker_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(blocker_id, None))?;
    if blocker.id == req.id {
        anyhow::bail!("a requirement cannot be blocked by itself ({})", spec_id);
    }
    let blocker_display = blocker
        .spec_id
        .clone()
        .unwrap_or_else(|| blocker_id.to_string());

    // Idempotent: only add the BlockedBy edge if absent.
    if !req
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::BlockedBy) && r.target_id == blocker.id)
    {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id: blocker.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    // Inverse Blocks edge on the blocker (also idempotent).
    let mut blocker = backend.get_requirement_unambiguous(blocker_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    if !blocker
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::Blocks) && r.target_id == req.id)
    {
        blocker.relationships.push(Relationship {
            rel_type: RelationshipType::Blocks,
            target_id: req.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        blocker.modified_at = chrono::Utc::now();
        backend.update_requirement(&blocker)?;
    }
    Ok(blocker_display)
}

/// TASK-1176: record that `spec_id` was REPLACED BY `successor_id` — a typed
/// `SupersededBy` edge on the superseded spec plus the inverse `Supersedes`
/// edge on the successor, atomically and idempotently. Returns the successor's
/// display id for messaging.
///
/// This is the first-class alternative to the `superseded-by:ADR-N` string tag
/// downstream projects had to invent: a typed edge is walkable by `aida graph`
/// / `query_graph`, survives an id rename, and cannot silently disagree with
/// the status. Shaped deliberately like [`add_blocked_by_edge`] so the two
/// bidirectional-edge writers stay recognizably the same code.
// trace:TASK-1176 | ai:claude
pub(crate) fn add_superseded_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    successor_id: &str,
) -> Result<String> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let successor = backend
        .get_requirement_unambiguous(successor_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(successor_id, None))?;
    if successor.id == req.id {
        anyhow::bail!("a requirement cannot supersede itself ({})", spec_id);
    }
    let successor_display = successor
        .spec_id
        .clone()
        .unwrap_or_else(|| successor_id.to_string());

    if !req.relationships.iter().any(|r| {
        matches!(r.rel_type, RelationshipType::SupersededBy) && r.target_id == successor.id
    }) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::SupersededBy,
            target_id: successor.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    // Inverse Supersedes edge on the successor (also idempotent).
    let mut successor = backend.get_requirement_unambiguous(successor_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    if !successor
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::Supersedes) && r.target_id == req.id)
    {
        successor.relationships.push(Relationship {
            rel_type: RelationshipType::Supersedes,
            target_id: req.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        successor.modified_at = chrono::Utc::now();
        backend.update_requirement(&successor)?;
    }
    Ok(successor_display)
}

// STORY-1434: a carved-out acceptance criterion previously left two halves
// that could drift apart — stale text staying in the description as the
// authoritative gate, and the correction landing only in a comment the
// default `aida show` never surfaced. These names are the shared vocabulary
// for closing that gap: the relationship names the typed edge (a `Custom`
// variant, so no enum-wide match site needs updating — walkable today by
// `aida graph --follow carved-out-to` / `query_graph`), and the comment
// markers are the prefixes `aida show`'s default view treats as visible
// corrections rather than buried commentary. trace:STORY-1434 | ai:claude
/// The typed edge a carve-out writes on the SOURCE spec: "this spec carved a
/// criterion out, now carried by the target". Stored as
/// `RelationshipType::Custom(CARVED_OUT_TO_REL)` — see the module doc above
/// for why a new enum variant isn't needed.
pub(crate) const CARVED_OUT_TO_REL: &str = "carved-out-to";
/// The reciprocal edge on the TARGET spec: "this spec now carries a
/// criterion carved from the source".
pub(crate) const CARVED_FROM_REL: &str = "carved-from";

/// Comment-body prefixes `aida show`'s DEFAULT view (no `-c`/`--comments`)
/// surfaces inline right after the description, instead of leaving them
/// invisible until someone thinks to pass `-c` or scrolls hundreds of lines
/// into the YAML. Matched case-insensitively against the start of the
/// (trimmed) comment body. `CARVE-OUT` is what `--carve-out` itself writes;
/// `CORRECTION` and `PROXY DECISION` are the free-text conventions the
/// advisor seat was already using by hand for the same kind of "the
/// description says X but that's stale" note (the STORY-1434 filing cites
/// three real instances of exactly that shape).
pub(crate) const DEFAULT_VISIBLE_COMMENT_MARKERS: &[&str] =
    &["CARVE-OUT", "CORRECTION", "PROXY DECISION"];

/// True when `body` opens with one of [`DEFAULT_VISIBLE_COMMENT_MARKERS`]
/// (case-insensitive), followed by a non-alphanumeric character (`:`, `-`,
/// whitespace, end-of-string, …) so `CORRECTIONAL` doesn't false-positive on
/// the `CORRECTION` marker.
// trace:STORY-1434 | ai:claude
pub(crate) fn is_default_visible_comment(body: &str) -> bool {
    let trimmed = body.trim_start();
    // trace:BUG-1713 | ai:antigravity
    DEFAULT_VISIBLE_COMMENT_MARKERS.iter().any(|marker| {
        trimmed.len() >= marker.len()
            && trimmed.is_char_boundary(marker.len())
            && trimmed[..marker.len()].eq_ignore_ascii_case(marker)
            && trimmed[marker.len()..]
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true)
    })
}

/// Collect every comment (top-level and nested reply) whose body matches
/// [`is_default_visible_comment`], in document order.
// trace:STORY-1434 | ai:claude
pub(crate) fn collect_default_visible_comments<'a>(
    comments: &'a [aida_core::Comment],
    out: &mut Vec<&'a aida_core::Comment>,
) {
    for c in comments {
        if is_default_visible_comment(&c.content) {
            out.push(c);
        }
        collect_default_visible_comments(&c.replies, out);
    }
}

/// Strike `criterion` out of `description`, replacing the first verbatim
/// occurrence with a short pointer at the target that now carries it.
/// Returns `None` (no-op) when `criterion` isn't found — a carve-out of text
/// that doesn't match verbatim is refused by the caller rather than silently
/// doing nothing, so this stays a pure "did it match" signal. Pure and
/// backend-free so the substring/pointer behavior is unit-testable without a
/// git store.
// trace:STORY-1434 | ai:claude
pub(crate) fn carve_out_description(
    description: &str,
    criterion: &str,
    target_display: &str,
) -> Option<String> {
    if criterion.is_empty() || !description.contains(criterion) {
        return None;
    }
    let marker = format!("[carved out \u{2192} {target_display} \u{2014} see carve-out log]");
    Some(description.replacen(criterion, &marker, 1))
}

/// The target ids of every `carved-out-to` edge on `req` — i.e. the specs
/// that now carry a criterion this spec used to gate on. Empty when the spec
/// carries no carve-out.
// trace:STORY-1434 | ai:claude
pub(crate) fn carved_out_targets(req: &aida_core::Requirement) -> Vec<uuid::Uuid> {
    req.relationships
        .iter()
        .filter(|r| {
            matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_OUT_TO_REL))
        })
        .map(|r| r.target_id)
        .collect()
}

/// Apply a carve-out to an ALREADY-RESOLVED-AND-VALIDATED `req`: push the
/// typed `Custom("carved-out-to")` edge (idempotent) and the `CARVE-OUT:`
/// comment [`is_default_visible_comment`] recognizes. Pure — no backend I/O —
/// so the caller can fold this into the SAME `req` mutation the rest of
/// `aida edit`'s scalar fields go through and save it in the one atomic
/// write, instead of a separate follow-up write that could land only
/// half-applied (the description struck but the edge/comment lost, or vice
/// versa, on a failure in between).
// trace:STORY-1434 | ai:claude
pub(crate) fn apply_carve_out(
    req: &mut aida_core::Requirement,
    target_id: uuid::Uuid,
    source_display: &str,
    target_display: &str,
    criterion: &str,
    reason: Option<&str>,
) {
    use aida_core::models::{Relationship, RelationshipType};

    if !req.relationships.iter().any(|r| {
        matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_OUT_TO_REL))
            && r.target_id == target_id
    }) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::Custom(CARVED_OUT_TO_REL.to_string()),
            target_id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
    }

    let reason_line = reason
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .unwrap_or("no reason given");
    let comment_body = format!(
        "CARVE-OUT: \"{criterion}\" is no longer carried by {source_display} — it is now \
         carried by {target_display}.\n\nReason: {reason_line}"
    );
    let mut comment = aida_core::Comment::new(get_default_author(), comment_body);
    // trace:TASK-330 | ai:claude — stamp the producing session, same as
    // `aida comment add`'s comment.
    comment.session_id = resolve_current_session_id();
    req.comments.push(comment);
}

/// Write the inverse `Custom("carved-from")` edge on the carve-out TARGET,
/// after the source's atomic save has landed. Idempotent. The target is
/// re-fetched by UUID (never re-resolved from a user-typed id — the caller
/// already resolved and validated it once via `get_requirement_unambiguous`),
/// so this can only fail on a genuine backend error or the target having
/// disappeared between resolution and this call — either way that is an
/// error the caller must surface, not swallow as a warning: a one-sided
/// carve-out (source struck, no inverse edge) is exactly the kind of
/// half-applied state this spec exists to prevent.
// trace:STORY-1434 | ai:claude
pub(crate) fn add_carved_from_edge(
    backend: &aida_core::CachedGitBackend,
    target_id: uuid::Uuid,
    target_display: &str,
    source_id: uuid::Uuid,
    source_display: &str,
) -> Result<()> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut target = backend.get_requirement(&target_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "carve-out target {} disappeared before the inverse carved-from edge could be \
             written — the carve-out edge/comment on the source are saved, but the reciprocal \
             edge on the target is missing; re-run `aida rel add {} {} --type carved-from` \
             to repair it by hand",
            target_id,
            target_display,
            source_display
        )
    })?;
    if !target.relationships.iter().any(|r| {
        matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_FROM_REL))
            && r.target_id == source_id
    }) {
        target.relationships.push(Relationship {
            rel_type: RelationshipType::Custom(CARVED_FROM_REL.to_string()),
            target_id: source_id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        target.modified_at = chrono::Utc::now();
        backend.update_requirement(&target)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1434_carve_out_tests.rs"]
mod story_1434_carve_out_tests;

/// STORY-446: remove the BlockedBy edge from `spec_id` to `blocker_id` and the
/// inverse Blocks edge on the blocker. No-op (returns the display id) when the
/// edge is absent. trace:STORY-446 | ai:claude
pub(crate) fn remove_blocked_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    blocker_id: &str,
) -> Result<String> {
    use aida_core::models::RelationshipType;
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let blocker = backend
        .get_requirement_unambiguous(blocker_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(blocker_id, None))?;
    let blocker_display = blocker
        .spec_id
        .clone()
        .unwrap_or_else(|| blocker_id.to_string());

    let before = req.relationships.len();
    req.relationships.retain(|r| {
        !(matches!(r.rel_type, RelationshipType::BlockedBy) && r.target_id == blocker.id)
    });
    if req.relationships.len() != before {
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    let mut blocker = backend.get_requirement_unambiguous(blocker_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    let inv_before = blocker.relationships.len();
    blocker
        .relationships
        .retain(|r| !(matches!(r.rel_type, RelationshipType::Blocks) && r.target_id == req.id));
    if blocker.relationships.len() != inv_before {
        blocker.modified_at = chrono::Utc::now();
        backend.update_requirement(&blocker)?;
    }
    Ok(blocker_display)
}

/// Handle commands routed to the GitBackend (when --file points to a directory).
/// Resolve an `aida history --id <X>` / positional `aida history <X>`
/// argument to the canonical spec_id the orphan-branch event decoder keys
/// on. `aida history` walks the git log and tags every event with the
/// YAML's `spec_id`, so a UUID (which `aida show` prints) or an agreed_id
/// never matched the filter and produced an empty "(no recent activity)" —
/// the symptom BUG-588 reports. When the argument resolves to exactly one
/// live requirement (by spec_id, agreed_id, or UUID) we substitute its
/// canonical spec_id.
///
/// TASK-1480: this is also the "invalid or ambiguous IDs get a clear error"
/// gate. Two cases refuse outright: a string that can't possibly be a spec
/// id (BUG-599's format hint) and one that resolves to more than one
/// requirement ([`aida_core::id_collisions::AmbiguousIdError`], which
/// already names each candidate's unambiguous handle). A well-formed id
/// that simply isn't *live* right now — deleted, or never assigned — is
/// passed through unchanged rather than rejected here: a deleted spec can
/// still have real history to show, so `history::run` is the one that
/// decides, once it knows whether the id has any recorded events at all.
/// trace:BUG-588 | ai:claude
// trace:TASK-1480 | ai:claude
pub(crate) fn resolve_history_id_filter<B: aida_core::db::DatabaseBackend>(
    backend: &B,
    raw: &str,
) -> Result<String> {
    let trimmed = raw.trim();
    let looks_like_uuid = uuid::Uuid::parse_str(trimmed).is_ok();
    if !looks_like_uuid && !aida_core::object_store::valid_spec_id_format(trimmed) {
        return Err(crate::not_found::invalid_spec_id_format(trimmed));
    }
    // `get_requirement_unambiguous` already turns a multi-match into an
    // `anyhow::Error` (AmbiguousIdError's own Display), so `?` here IS the
    // "ambiguous id gets a clear error" behavior. trace:TASK-1480 | ai:claude
    // Read-only: tolerant resolver. trace:BUG-1670 | ai:claude
    match backend.get_requirement_unambiguous_for_read(trimmed)? {
        Some(req) => Ok(req.spec_id.clone().unwrap_or_else(|| trimmed.to_string())),
        None => Ok(trimmed.to_string()),
    }
}

pub(crate) fn command_triggers_per_write_auto_push(command: &Command) -> bool {
    match command {
        Command::Add { .. }
        | Command::Edit { .. }
        | Command::Del { .. }
        | Command::Rel(_)
        | Command::RelDef(_)
        | Command::Comment(_)
        | Command::Type(_)
        | Command::Import { .. }
        | Command::Doc(_)
        | Command::Docs(_)
        | Command::Punt { .. }
        | Command::Archive { .. }
        | Command::Unarchive { .. }
        | Command::Defer { .. }
        | Command::Undefer { .. }
        | Command::Assign { .. }
        | Command::Unassign { .. }
        | Command::Rework { .. } => true,
        Command::Queue(cmd) => matches!(
            cmd,
            QueueCommand::Add { .. }
                | QueueCommand::Remove { .. }
                | QueueCommand::Move { .. }
                | QueueCommand::Clear { .. }
                | QueueCommand::Done { .. }
                | QueueCommand::Rework { .. }
                | QueueCommand::Prune { .. }
                | QueueCommand::Gc { .. }
        ),
        // STORY-444: `aida backlog groom` writes (queue + tag); list /
        // analyze are read-only. trace:STORY-444 | ai:claude
        Command::Backlog(cmd) => matches!(cmd, BacklogCommand::Groom { .. }),
        // FR-267: trace add/remove always mutate the store; `trace scan`
        // only writes with `--update`. scan/list/gate/sweep without a write
        // are read-only. trace:FR-267 | ai:claude
        Command::Trace(cmd) => matches!(
            cmd,
            TraceCommand::Add { .. }
                | TraceCommand::Remove { .. }
                | TraceCommand::Scan { update: true, .. }
        ),
        // STORY-732: bare `aida findings` (None) defaults to list — read-only.
        Command::Findings { cmd } => cmd.as_ref().is_some_and(|c| {
            !matches!(
                c,
                FindingsCommand::List { .. } | FindingsCommand::Classes { .. }
            )
        }),
        // STORY-522: `aida questions ask` / `answer` write the
        // decision_request field; bare list / `list` are read-only.
        // trace:STORY-522 | ai:claude
        Command::Questions { cmd } => matches!(
            cmd,
            Some(QuestionsCommand::Ask { .. })
                | Some(QuestionsCommand::Answer { .. })
                | Some(QuestionsCommand::Remedy { .. })
        ),
        // TASK-779: `aida decide` may route to the answer path, which writes
        // the decision_request + status + queue — treat it as a store writer.
        // trace:TASK-779 | ai:claude
        Command::Decide { .. } => true,
        Command::Config(cmd) => matches!(
            cmd,
            ConfigCommand::Format { .. }
                | ConfigCommand::Numbering { .. }
                | ConfigCommand::Digits { .. }
                | ConfigCommand::Migrate { .. }
        ),
        // TASK-1487: `jira/github pull` bulk-import new requirements into the
        // local store (via `bulk_import_via_writer`) unless `--dry-run`; every
        // other tracker subcommand (config/test/list/show/push/sync/labels/…)
        // only reads the store or talks to the remote tracker. GitLab has no
        // local-write subcommand for a git-canonical store (`refresh`'s sync
        // state is SQLite-only). trace:TASK-1487 | ai:claude
        Command::Jira(JiraCommand::Pull { dry_run, .. }) => !dry_run,
        Command::Github(GitHubCommand::Pull { dry_run, .. }) => !dry_run,
        _ => false,
    }
}

pub(crate) fn active_stakeholder_role() -> Option<String> {
    std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .map(|role| canonical_role_name(role.trim()))
        .filter(|role| role == "guest" || role == "requester")
}

// trace:TASK-1594 | ai:claude
pub(crate) fn stakeholder_refusal_message(role: &str, action: &str) -> String {
    format!(
        "The '{role}' role is a least-privilege stakeholder role; refusing {action}. Ask an advisor to groom, route, or approve it."
    )
}

pub(crate) fn stakeholder_refusal(role: &str, action: &str) -> anyhow::Error {
    anyhow::anyhow!(stakeholder_refusal_message(role, action))
}

// trace:BUG-1197 | ai:codex
// trace:TASK-1264 | ai:codex
/// Shared capability vocabulary for the CLI and MCP stakeholder envelopes.
/// Keeping this decision independent of either transport prevents a new write
/// path from silently widening guest/requester authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StakeholderAction {
    Read,
    Intake,
    EditRequirement,
    Queue,
    Database,
    MergeGate,
    Session,
    Role,
    Relationship,
    Comment,
    Type,
    Trace,
    Finding,
    Question,
    Configuration,
    BuildLoop,
    Write,
}

pub(crate) fn stakeholder_action_allowed(role: &str, action: StakeholderAction) -> bool {
    match role {
        "guest" => action == StakeholderAction::Read,
        "requester" => matches!(action, StakeholderAction::Read | StakeholderAction::Intake),
        _ => true,
    }
}

pub(crate) fn stakeholder_action_label(action: StakeholderAction) -> &'static str {
    match action {
        StakeholderAction::Read => "reads",
        StakeholderAction::Intake => "adding requirements",
        StakeholderAction::EditRequirement => "editing requirements",
        StakeholderAction::Queue => "queue operations",
        StakeholderAction::Database => "database writes",
        StakeholderAction::MergeGate => "merge-gate writes",
        StakeholderAction::Session => "session writes",
        StakeholderAction::Role => "role writes",
        StakeholderAction::Relationship => "relationship writes",
        StakeholderAction::Comment => "comment writes",
        StakeholderAction::Type => "type writes",
        StakeholderAction::Trace => "trace writes",
        StakeholderAction::Finding => "finding writes",
        StakeholderAction::Question => "question writes",
        StakeholderAction::Configuration => "configuration writes",
        StakeholderAction::BuildLoop => "build-loop execution",
        StakeholderAction::Write => "writes",
    }
}

/// Tools that stakeholder personas may use as reads. This is deliberately
/// narrower than the MCP `read-only` profile: that profile also contains
/// command-producing helpers whose returned commands can mutate shared state.
// trace:BUG-1197 | ai:codex
pub(crate) fn stakeholder_mcp_read_allowed(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "list_requirements"
            | "show_requirement"
            | "search_requirements"
            | "query_graph"
            | "list_features"
            | "history"
            | "read_inbox"
            | "list_punts"
            | "read_punt"
            | "list_findings"
            | "list_active_leases"
            | "list_directives"
            | "list_briefs"
            | "read_brief"
            | "queue_list"
            | "queue_next"
            | "queue_progress"
            | "cache_status"
            | "schema"
            | "status_unified"
            | "usage_query"
            | "plan_verify"
            | "plan_helpers"
            | "ultraplan_assemble"
            | "goal_derive"
    )
}

pub(crate) fn requester_intake_type_allowed(type_name: &str) -> bool {
    parse_requirement_type(type_name)
        .map(|req_type| requester_add_type_allowed(&req_type))
        .unwrap_or(false)
}

pub(crate) fn parse_requester_add_type(raw: Option<&str>) -> Result<RequirementType> {
    Ok(raw
        .map(parse_requirement_type)
        .transpose()?
        .unwrap_or(RequirementType::ChangeRequest))
}

pub(crate) fn requester_add_type_allowed(req_type: &RequirementType) -> bool {
    matches!(
        req_type,
        RequirementType::ChangeRequest | RequirementType::Bug | RequirementType::User
    )
}

pub(crate) fn add_csv_tag(existing: &mut Option<String>, tag: &str) {
    let mut tags: Vec<String> = existing
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
        tags.push(tag.to_string());
    }
    *existing = Some(tags.join(","));
}

/// STORY-1110: hard CLI role envelope for external stakeholder personas.
/// `guest` is read-only. `requester` inherits read access plus one constrained
/// intake write: `aida add` for change-request/bug/user, always Draft and tagged
/// for advisor grooming. Everything else falls into the existing command/gate
/// machinery as a refusal, so these roles cannot become build-loop seats by
/// convention drift.
pub(crate) fn enforce_stakeholder_role_capabilities(command: &mut Command) -> Result<()> {
    let Some(role) = active_stakeholder_role() else {
        return Ok(());
    };
    enforce_stakeholder_role_capabilities_for_role(command, &role)
}

pub(crate) fn enforce_stakeholder_role_capabilities_for_role(
    command: &mut Command,
    role: &str,
) -> Result<()> {
    match role {
        "guest" => {
            let action = stakeholder_cli_action(command);
            if !stakeholder_action_allowed("guest", action) {
                return Err(stakeholder_refusal(
                    "guest",
                    stakeholder_action_label(action),
                ));
            }
        }
        "requester" => {
            if let Command::Add {
                status,
                r#type,
                owner,
                feature,
                tags,
                parent,
                queue,
                batch,
                r#for,
                interactive,
                ..
            } = command
            {
                if *interactive {
                    return Err(stakeholder_refusal("requester", "interactive add"));
                }
                if *queue || batch.is_some() || r#for.is_some() {
                    return Err(stakeholder_refusal("requester", "queueing intake"));
                }
                let forbidden_fields = [
                    ("parent", parent.is_some()),
                    ("feature", feature.is_some()),
                    ("owner", owner.is_some()),
                ];
                if REQUESTER_INTAKE_FORBIDDEN_FIELDS.iter().any(|field| {
                    forbidden_fields
                        .iter()
                        .any(|(candidate, is_set)| candidate == field && *is_set)
                }) {
                    return Err(stakeholder_refusal(
                        "requester",
                        "setting parent, feature, or owner during intake",
                    ));
                }
                let req_type = parse_requester_add_type(r#type.as_deref())?;
                if !requester_add_type_allowed(&req_type) {
                    return Err(stakeholder_refusal(
                        "requester",
                        "adding anything except change-request, bug, or user specs",
                    ));
                }
                *status = Some("draft".to_string());
                if r#type.is_none() {
                    *r#type = Some("change-request".to_string());
                }
                add_csv_tag(tags, "intake:requester");
                return Ok(());
            }
            let action = stakeholder_cli_action(command);
            if !stakeholder_action_allowed("requester", action) {
                return Err(stakeholder_refusal(
                    "requester",
                    stakeholder_action_label(action),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod stakeholder_cli_policy_tests {
    use super::*;

    fn guest_refusal(args: &[&str]) -> anyhow::Error {
        let mut cli = Cli::try_parse_from(args).expect("valid CLI command");
        enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "guest")
            .expect_err("guest command must be refused")
    }

    #[test]
    fn guest_refuses_db_sync() {
        assert!(guest_refusal(&["aida", "db", "sync"])
            .to_string()
            .contains("refusing database writes"));
    }

    #[test]
    fn guest_refuses_session_start() {
        assert!(
            guest_refusal(&["aida", "session", "start", "--owns", "BUG-1197"])
                .to_string()
                .contains("refusing session writes")
        );
    }

    #[test]
    fn guest_allows_mcp_and_coordination_reads() {
        for args in [
            &["aida", "mcp-serve"][..],
            &["aida", "awaiting", "--no-ci"][..],
            &["aida", "queue", "list"][..],
            &["aida", "mailbox", "inbox", "--peek"][..],
        ] {
            let mut cli = Cli::try_parse_from(args).expect("valid CLI command");
            enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "guest")
                .unwrap_or_else(|err| panic!("guest read {args:?} was refused: {err}"));
        }
    }

    #[test]
    fn guest_refuses_role_enter() {
        assert!(guest_refusal(&["aida", "role", "enter", "advisor"])
            .to_string()
            .contains("refusing role writes"));
    }

    #[test]
    fn requester_cli_rejects_every_shared_forbidden_intake_field() {
        for field in REQUESTER_INTAKE_FORBIDDEN_FIELDS {
            let flag = format!("--{field}");
            let mut cli = Cli::try_parse_from([
                "aida", "add", "--title", "request", "--type", "bug", &flag, "EPIC-5",
            ])
            .expect("valid CLI command");
            let err = enforce_stakeholder_role_capabilities_for_role(&mut cli.command, "requester")
                .expect_err("shared requester intake field must be refused");
            assert!(
                err.to_string()
                    .contains("setting parent, feature, or owner during intake"),
                "field {field}: {err}"
            );
        }
    }
}

pub(crate) fn stakeholder_cli_action(command: &Command) -> StakeholderAction {
    if matches!(command, Command::Add { .. }) {
        StakeholderAction::Intake
    } else if matches!(
        command,
        Command::Why { .. }
            | Command::Explain { .. }
            | Command::Wiki(_)
            | Command::List { .. }
            | Command::Show { .. }
            | Command::Status { .. }
            | Command::Graph { .. }
            | Command::Search { .. }
            | Command::Digest { .. }
            | Command::History { .. }
            | Command::McpServe
            | Command::Awaiting { .. }
            | Command::Protocol(_)
            | Command::Queue(QueueCommand::List { .. })
            | Command::Queue(QueueCommand::Next { .. })
            | Command::Queue(QueueCommand::Progress { .. })
            | Command::Mailbox(MailboxCommand::Inbox { .. })
            | Command::Mailbox(MailboxCommand::Thread { .. })
            | Command::Brief {
                cmd: Some(BriefCommand::List { .. } | BriefCommand::Read { .. }),
                ..
            }
            | Command::Findings {
                cmd: None | Some(FindingsCommand::List { .. } | FindingsCommand::Classes { .. }),
            }
            | Command::Punts(PuntsCommand::List { .. } | PuntsCommand::Read { .. })
            | Command::Worker(WorkerCommand::Directives { .. })
            | Command::Comment(CommentCommand::List { .. })
            | Command::Role(
                RoleCommand::List
                    | RoleCommand::Show { .. }
                    | RoleCommand::Active
                    | RoleCommand::Current { .. },
            )
            | Command::Session(SessionCommand::Leases { .. } | SessionCommand::Show { .. })
            | Command::Schema { .. }
            | Command::Contract { .. }
            | Command::Cache(CacheCommand::Status)
            // trace:BUG-1486 | ai:claude — `aida db` gates per-verb: these
            // are pure reads (path/info/status printouts, block
            // list/status/verify reports) and must not inherit the
            // "refusing database writes" refusal meant for the genuinely
            // mutating verbs (Migrate/Sync/MergeGate/ReconcileStatus/
            // Block::Claim/...), which stay gated below via the
            // `Command::Db(_) => StakeholderAction::Database` fail-closed
            // default.
            | Command::Db(DbCommand::Path)
            | Command::Db(DbCommand::Info)
            | Command::Db(DbCommand::Status)
            | Command::Db(DbCommand::Block {
                subcommand: BlockCommand::List | BlockCommand::Status | BlockCommand::Verify,
            })
            | Command::Usage { .. }
            | Command::Plan(PlanCommand::Verify { fix: false, .. })
            | Command::Plan(PlanCommand::Helpers { append: None, .. })
            | Command::Ultraplan { .. }
            | Command::Goal { .. }
            | Command::Criteria { .. }
            | Command::HelpAll
    ) {
        StakeholderAction::Read
    } else {
        match command {
            Command::Add { .. } => StakeholderAction::Intake,
            Command::Edit { .. } => StakeholderAction::EditRequirement,
            Command::Queue(_) => StakeholderAction::Queue,
            Command::Db(DbCommand::MergeGate) => StakeholderAction::MergeGate,
            Command::Db(_) => StakeholderAction::Database,
            Command::Session(_) => StakeholderAction::Session,
            Command::Role(_) => StakeholderAction::Role,
            Command::Rel(_) => StakeholderAction::Relationship,
            Command::Comment(_) => StakeholderAction::Comment,
            Command::Type(_) => StakeholderAction::Type,
            Command::Trace(_) => StakeholderAction::Trace,
            Command::Findings { .. } => StakeholderAction::Finding,
            Command::Questions { .. } => StakeholderAction::Question,
            Command::Config(_) => StakeholderAction::Configuration,
            Command::Zen { .. }
            | Command::Do { .. }
            | Command::Ship { .. }
            | Command::Integrate { .. } => StakeholderAction::BuildLoop,
            _ => StakeholderAction::Write,
        }
    }
}

pub(crate) fn create_agent_brief(
    project_root: &std::path::Path,
    store: &RequirementsStore,
    agent: &str,
    spec: &str,
    note: Option<&str>,
    depends_on: Option<&str>,
    authorized_by: Option<&str>,
) -> Result<std::path::PathBuf> {
    let agent = validate_brief_agent(agent)?;
    // TASK-1468: a brief never routes work at a guessed spec.
    // trace:TASK-1468 | ai:claude
    let req = store
        .get_requirement_unambiguous(spec)?
        .ok_or_else(|| not_found::requirement_not_found(spec, None))?;
    let spec_id = req.spec_id.as_deref().unwrap_or(spec);
    let depends_on = normalize_brief_dependency(store, depends_on)?;
    ensure_brief_dependency_is_acyclic(project_root, agent, spec_id, depends_on.as_deref())?;
    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ").to_string();
    let brief_dir = project_root.join(".aida").join("agent-briefs").join(agent);
    std::fs::create_dir_all(&brief_dir)
        .with_context(|| format!("failed to create {}", brief_dir.display()))?;
    let path = brief_dir.join(format!("{spec_id}-{generated_at}.md"));
    let body = render_agent_brief(
        project_root,
        agent,
        spec_id,
        req,
        store,
        &generated_at,
        note,
        depends_on.as_deref(),
        authorized_by,
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "brief already exists or cannot be written: {}",
                path.display()
            )
        })?;
    std::io::Write::write_all(&mut file, body.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// STORY-569: does the agent's mailbox already hold a PENDING brief for this
/// spec? An ack renames the file to `*.md.acked`, so a pending brief is a
/// plain `.md` whose name starts with `<SPEC>-` (the [`create_agent_brief`]
/// naming scheme). Keeps the zen-finish handoff idempotent — the finish gate
/// can be consulted more than once per checkpoint without double-filing.
// trace:STORY-569 | ai:claude
pub(crate) fn pending_brief_exists(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
) -> bool {
    let dir = project_root.join(".aida").join("agent-briefs").join(agent);
    let prefix = format!("{}-", spec_id.to_ascii_uppercase());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name.to_ascii_uppercase().starts_with(&prefix) && name.ends_with(".md")
    })
}

/// STORY-569: the clean-finish build→review handoff. When a standalone
/// `--zen` session finishes with an open PR on its lease branch, file a
/// pickup brief to the advisor's mailbox (`.aida/agent-briefs/<agent>/`)
/// carrying the spec id, the PR number/url, and the pointer to review
/// against the spec's resolved-design block — so the reviewing session
/// learns about the PR through the substrate, never an operator relay.
///
/// Returns `Ok(None)` when there is nothing to do: the handoff is disabled
/// (`[zen] review_brief_agent = ""`), the lease has no open PR (or the
/// forge is unreachable — fail open, a notify must never block a finish),
/// or a pending brief for the spec already sits in the mailbox
/// (idempotent). Reuses the existing brief writer + `.pending` sentinel;
/// nothing reinvented. trace:STORY-569 | ai:claude
pub(crate) fn file_zen_review_brief(
    project_root: &std::path::Path,
    lease: &SessionLease,
) -> Result<Option<(String, std::path::PathBuf)>> {
    let Some(agent) = read_zen_review_brief_agent(project_root) else {
        return Ok(None);
    };
    let spec_id = lease.scope.trim().to_string();
    if spec_id.is_empty() {
        return Ok(None);
    }
    let crate::forge::ChangeLookup::Found(change) =
        change_lookup_for_branch(project_root, &lease.branch)
    else {
        return Ok(None);
    };
    if pending_brief_exists(project_root, &agent, &spec_id) {
        return Ok(None);
    }
    let store = load_store_for_lookup(project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {}",
            project_root.display()
        )
    })?;
    let note = format!(
        "PR #{} is ready for review: {}\n\
         Review the diff against {}'s acceptance criteria and resolved-design block \
         (`aida show {}`), then ack this brief.",
        change.id, change.url, spec_id, spec_id
    );
    let path = create_agent_brief(
        project_root,
        &store,
        &agent,
        &spec_id,
        Some(&note),
        None,
        None,
    )?;
    // Urgent-path sentinel so the advisor's `aida status` surfaces it
    // without waiting on a mailbox poll (the `--notify` mechanic).
    add_pending_brief(project_root, &agent, &path)?;
    Ok(Some((agent, path)))
}

pub(crate) fn validate_brief_agent(agent: &str) -> Result<&str> {
    let agent = agent.trim();
    if agent.is_empty() {
        anyhow::bail!("agent name cannot be empty");
    }
    if !agent
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        anyhow::bail!("agent name must contain only ASCII letters, digits, '.', '_' or '-'");
    }
    Ok(agent)
}

pub(crate) fn render_agent_brief(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
    req: &Requirement,
    store: &RequirementsStore,
    generated_at: &str,
    note: Option<&str>,
    depends_on: Option<&str>,
    authorized_by: Option<&str>,
) -> String {
    let generated_by = brief_generated_by();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("spec_id: {}\n", yaml_scalar(spec_id)));
    out.push_str(&format!("agent: {}\n", yaml_scalar(agent)));
    out.push_str(&format!("generated_at: {}\n", yaml_scalar(generated_at)));
    out.push_str(&format!("generated_by: {}\n", yaml_scalar(&generated_by)));
    if let Some(note) = note {
        out.push_str("note: |-\n");
        out.push_str(&indent_block(note, "  "));
    }
    if let Some(depends_on) = depends_on {
        out.push_str(&format!("depends_on: {}\n", yaml_scalar(depends_on)));
    }
    // STORY-711 slice 2: the authorizing advisor's id, carried into the
    // receiving agent's role-context snapshot at launch so the automatic
    // advisor-lock gate can read it at commit time. trace:TASK-1140 | ai:claude
    if let Some(authorized_by) = authorized_by.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("authorized_by: {}\n", yaml_scalar(authorized_by)));
    }
    out.push_str("status: pending\n");
    out.push_str("---\n\n");

    out.push_str(&format!(
        "## Routing\n\nThis brief is for: {agent}. Generated {generated_at} from the AIDA spec graph.\n\n"
    ));
    out.push_str("## Optional preamble\n\n");
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        out.push_str(note);
        out.push_str("\n\n");
    } else {
        out.push_str("_No operator note provided._\n\n");
    }
    out.push_str("## Spec\n\n");
    out.push_str(&format!("- ID: {spec_id}\n"));
    out.push_str(&format!("- Title: {}\n", req.title));
    out.push_str(&format!("- Status: {}\n", req.status));
    out.push_str(&format!("- Type: {}\n", req.req_type));
    out.push_str(&format!("- Priority: {}\n", req.priority));
    out.push_str(&format!(
        "- Tags: {}\n\n",
        sorted_tags(&req.tags).unwrap_or_else(|| "none".to_string())
    ));
    out.push_str(&req.description);
    out.push_str("\n\n");

    out.push_str("## Composes with\n\n");
    let relations = brief_relationship_lines(req, store);
    if relations.is_empty() {
        out.push_str("_No direct relationships recorded._\n\n");
    } else {
        for line in relations {
            out.push_str("- ");
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("## Discipline\n\n");
    out.push_str("- Start with AGENTS.md for Codex-oriented project discipline.\n");
    out.push_str(&format!(
        "- Agent setup convention: docs/agents/{agent}-mcp-setup.md if present.\n\n"
    ));

    // BUG-583: the Setup block must reference the TARGET PROJECT's location
    // (resolved at runtime from the invocation's project root), never the AIDA
    // binary's compiled-in source-repo path. A cold vendor agent following these
    // steps literally must stay inside its own project, not `cd` into wherever
    // aida happened to be built. The sibling worktree is derived from the
    // project root's basename + its parent dir; both are anchored to the project
    // being briefed, not the binary. trace:BUG-583
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let project_root_display = project_root.display();
    let worktree_path_for = |branch: &str| -> std::path::PathBuf {
        project_root
            .parent()
            .map(|parent| parent.join(format!("{project_name}-{branch}")))
            .unwrap_or_else(|| project_root.join(format!("../{project_name}-{branch}")))
    };

    // BUG-1525: never emit a fresh-start `git worktree add -b <new> origin/main`
    // when the spec already has a branch (local or `origin/<branch>`) carrying
    // its work — that starts a second lineage with none of the reviewed
    // commits, silently, because the fresh branch name doesn't collide with
    // anything. Resolve what's actually there first. trace:BUG-1525 | ai:claude
    let target = resolve_brief_branch_target(project_root, spec_id);
    out.push_str("## Setup\n\n");
    match &target {
        BriefBranchTarget::Ambiguous { candidates } => {
            out.push_str(&format!(
                "_Cannot determine this spec's branch automatically — {} branches reference {spec_id} ({}). \
                 Check `aida show {spec_id}` for the branch its open PR (if any) actually points at, then \
                 `cd` into that branch's existing worktree, or `git worktree add <path> <branch>` to attach \
                 one — do NOT create a new branch off `origin/main`._\n\n",
                candidates.len(),
                candidates.join(", ")
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str("# resolve the branch above by hand before attaching a worktree\n");
            out.push_str("```\n\n");
        }
        BriefBranchTarget::ExistingWorktree { branch, worktree } => {
            let worktree_display = worktree.display();
            out.push_str(&format!(
                "This spec already has branch `{branch}` checked out in an existing worktree — \
                 continue it, don't start a second lineage.\n\n"
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {worktree_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str(
                "# realign if diverged from main before continuing:\n\
                 #   git rebase origin/main   (or: git merge origin/main)\n",
            );
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
        BriefBranchTarget::ExistingBranch {
            branch,
            remote_only,
        } => {
            let worktree_dir = worktree_path_for(branch);
            let worktree_display = worktree_dir.display();
            out.push_str(&format!(
                "This spec already has branch `{branch}` with commits ahead of the default branch — \
                 continue it, don't start a fresh one.\n\n"
            ));
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            if *remote_only {
                out.push_str(&format!(
                    "git worktree add {worktree_display} -B {branch} origin/{branch}\n"
                ));
            } else {
                out.push_str(&format!("git worktree add {worktree_display} {branch}\n"));
            }
            out.push_str(&format!("cd {worktree_display}\n"));
            out.push_str(
                "# realign if diverged from main before continuing:\n\
                 #   git rebase origin/main   (or: git merge origin/main)\n",
            );
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
        BriefBranchTarget::Fresh { branch } => {
            let worktree_dir = worktree_path_for(branch);
            let worktree_display = worktree_dir.display();
            out.push_str("```bash\n");
            out.push_str(&format!("cd {project_root_display}\n"));
            out.push_str("git fetch origin main\n");
            out.push_str(&format!(
                "git worktree add {worktree_display} -b {branch} origin/main\n"
            ));
            out.push_str(&format!("cd {worktree_display}\n"));
            // BUG-331: no `.aida-store` symlink needed — sibling worktrees now
            // resolve the canonical store at the main worktree via
            // git-common-dir. trace:BUG-331
            out.push_str(&format!(
                "aida session start --owns {spec_id} --branch {branch} --path {worktree_display} --reuse-branch\n"
            ));
            out.push_str("```\n\n");
        }
    }

    out.push_str("## Trailer reminder\n\n");
    out.push_str(&format!(
        "Use a trailing-parens spec trailer in the commit subject: `({spec_id})` for a single-spec ship, or include every shipped spec in the same trailing parens.\n"
    ));
    out
}

/// BUG-1525: what the brief's Setup block should tell the implementer to do
/// about branches, derived from what already exists rather than assumed
/// fresh.
// trace:BUG-1525 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BriefBranchTarget {
    /// No branch anywhere references this spec — safe to start fresh off the
    /// default branch, using AIDA's `<spec>-work` naming convention (the
    /// shape the drain itself creates, per BUG-1525).
    Fresh { branch: String },
    /// Exactly one existing branch was found, and it's already checked out
    /// in an existing worktree — `cd` into it, don't create a second one.
    ExistingWorktree {
        branch: String,
        worktree: std::path::PathBuf,
    },
    /// Exactly one existing branch was found but no worktree currently has
    /// it checked out — attach a worktree to that branch (`remote_only`
    /// picks `-B <branch> origin/<branch>` over a plain local checkout).
    ExistingBranch { branch: String, remote_only: bool },
    /// More than one branch plausibly belongs to this spec — refuse to
    /// guess which one is live; say so explicitly rather than emit a
    /// confident fresh-start recipe.
    Ambiguous { candidates: Vec<String> },
}

/// Git-only scan (no `gh`/forge call — matches the pattern in
/// `collect_git_linkage_opts`) for a branch that already carries this spec's
/// work: any local or `origin/<branch>` ref whose name references the spec
/// id (`branch_name_references_spec`), excluding the `aida-store` orphan
/// branch. Used to decide whether the brief's Setup block should reuse an
/// existing lineage instead of starting a fresh one off `origin/main`.
// trace:BUG-1525 | ai:claude
pub(crate) fn resolve_brief_branch_target(
    project_root: &std::path::Path,
    spec_id: &str,
) -> BriefBranchTarget {
    let fresh_branch = format!("{}-work", spec_id.trim().to_ascii_lowercase());

    let git = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };

    let mut candidates: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(out) = git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]) {
        for line in out.lines() {
            let name = line.trim();
            if !name.is_empty()
                && branch_name_references_spec(name, spec_id)
                && !is_orphan_store_branch(name)
            {
                candidates.insert(name.to_string());
            }
        }
    }
    if let Some(out) = git(&[
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/remotes/origin/",
    ]) {
        for line in out.lines() {
            let Some(short) = line.trim().strip_prefix("origin/") else {
                continue;
            };
            if short.is_empty() || short == "HEAD" {
                continue;
            }
            if branch_name_references_spec(short, spec_id) && !is_orphan_store_branch(short) {
                candidates.insert(short.to_string());
            }
        }
    }

    if candidates.is_empty() {
        return BriefBranchTarget::Fresh {
            branch: fresh_branch,
        };
    }
    if candidates.len() > 1 {
        return BriefBranchTarget::Ambiguous {
            candidates: candidates.into_iter().collect(),
        };
    }
    let branch = candidates.into_iter().next().unwrap();

    for wt in list_worktrees(project_root) {
        if wt.branch.as_deref() == Some(branch.as_str()) {
            return BriefBranchTarget::ExistingWorktree {
                branch,
                worktree: wt.path,
            };
        }
    }

    let local_exists = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_some();
    BriefBranchTarget::ExistingBranch {
        branch,
        remote_only: !local_exists,
    }
}

pub(crate) fn brief_generated_by() -> String {
    std::env::var("AIDA_SESSION_ID")
        .or_else(|_| std::env::var("CLAUDE_CODE_SESSION_ID"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn yaml_scalar(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "''"))
    }
}

pub(crate) fn indent_block(value: &str, prefix: &str) -> String {
    if value.is_empty() {
        return format!("{prefix}\n");
    }
    value
        .lines()
        .map(|line| format!("{prefix}{line}\n"))
        .collect()
}

pub(crate) fn sorted_tags(tags: &HashSet<String>) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    let mut tags = tags.iter().cloned().collect::<Vec<_>>();
    tags.sort();
    Some(tags.join(", "))
}

pub(crate) fn brief_relationship_lines(
    req: &Requirement,
    store: &RequirementsStore,
) -> Vec<String> {
    let mut lines = Vec::new();
    for rel in &req.relationships {
        if let Some(target) = store.get_requirement_by_id(&rel.target_id) {
            let target_id = target
                .spec_id
                .as_deref()
                .or(target.agreed_id.as_deref())
                .unwrap_or("<no-spec-id>");
            lines.push(format!(
                "{}: {} — {}",
                rel.rel_type, target_id, target.title
            ));
        }
    }
    for other in &store.requirements {
        for rel in &other.relationships {
            if rel.target_id == req.id {
                let other_id = other
                    .spec_id
                    .as_deref()
                    .or(other.agreed_id.as_deref())
                    .unwrap_or("<no-spec-id>");
                lines.push(format!(
                    "referenced by {}: {} — {}",
                    rel.rel_type, other_id, other.title
                ));
            }
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

// trace:TASK-714
#[derive(Debug, Clone, PartialEq, Eq, ts_rs_forge::TS)]
pub(crate) struct BriefListEntry {
    pub(crate) path: std::path::PathBuf,
    pub(crate) spec_id: String,
    pub(crate) agent: String,
    pub(crate) generated_at: String,
    pub(crate) depends_on: Option<String>,
    pub(crate) acked: bool,
    /// STORY-711 slice 2: the authorizing advisor's id, when the brief was
    /// created with `--authorized-by`.
    // trace:TASK-1140 | ai:claude
    pub(crate) authorized_by: Option<String>,
}

pub(crate) fn list_agent_briefs(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
) -> Result<()> {
    if let Some(agent) = for_agent {
        validate_brief_agent(agent)?;
    }
    let entries = collect_agent_briefs(project_root, for_agent, include_acked)?;
    if entries.is_empty() {
        println!("No briefs found.");
        return Ok(());
    }
    for entry in entries {
        let status = if entry.acked { "acked" } else { "pending" };
        // STORY-711 slice 2: surface the authorizing advisor when present.
        // trace:TASK-1140 | ai:claude
        match entry.authorized_by.as_deref() {
            Some(by) => println!(
                "{}  {}  {}  {}  {}  authorized_by={}",
                entry.generated_at,
                entry.agent,
                entry.spec_id,
                status,
                entry.path.display(),
                by
            ),
            None => println!(
                "{}  {}  {}  {}  {}",
                entry.generated_at,
                entry.agent,
                entry.spec_id,
                status,
                entry.path.display()
            ),
        }
    }
    Ok(())
}

pub(crate) fn collect_agent_briefs(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
) -> Result<Vec<BriefListEntry>> {
    // Internal scans (notification banners, dependency walks, drain pickups)
    // resolve a bare agent-type name-class and must stay silent; only an
    // explicit user `--for-agent <target>` should surface ambiguity.
    collect_agent_briefs_inner(project_root, for_agent, include_acked, true)
}

// BUG-569: the ambiguity warning from `resolve_brief_directories` is only a real
// user error when the caller explicitly targeted an agent. Internal callers that
// pass a bare agent-TYPE name-class (e.g. `detect_agent_type()` → "antigravity")
// to scan that type's briefs were inheriting the warning and printing spurious
// "agent target 'antigravity' is ambiguous" noise on every advisor edit/comment
// when 2+ agents of that type were registered. Route those through
// `warn_on_ambiguity = false`. trace:BUG-569 | ai:claude
pub(crate) fn collect_agent_briefs_inner(
    project_root: &std::path::Path,
    for_agent: Option<&str>,
    include_acked: bool,
    warn_on_ambiguity: bool,
) -> Result<Vec<BriefListEntry>> {
    let root = project_root.join(".aida").join("agent-briefs");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let allowed_dirs: Option<Vec<String>> = if let Some(target) = for_agent {
        let (dirs, warning) =
            resolve_brief_dirs_with_optional_warning(project_root, target, warn_on_ambiguity);
        if let Some(warn) = warning {
            eprintln!("{}", warn);
        }
        Some(dirs)
    } else {
        None
    };

    let mut entries = Vec::new();
    for agent_dir in
        std::fs::read_dir(&root).with_context(|| format!("failed to read {}", root.display()))?
    {
        let agent_dir = agent_dir?;
        if !agent_dir.file_type()?.is_dir() {
            continue;
        }
        let agent = agent_dir.file_name().to_string_lossy().to_string();
        if let Some(ref allowed) = allowed_dirs {
            if !allowed.iter().any(|d| d.eq_ignore_ascii_case(&agent)) {
                continue;
            }
        }
        for file in std::fs::read_dir(agent_dir.path())? {
            let file = file?;
            if !file.file_type()?.is_file() {
                continue;
            }
            let path = file.path();
            let name = file.file_name().to_string_lossy().to_string();
            let acked = name.ends_with(".acked");
            if acked && !include_acked {
                continue;
            }
            if !(name.ends_with(".md") || acked) {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let spec_id = frontmatter_value(&content, "spec_id")
                .unwrap_or_else(|| spec_id_from_brief_filename(&name).unwrap_or_default());
            let generated_at = frontmatter_value(&content, "generated_at").unwrap_or_default();
            let depends_on = frontmatter_value(&content, "depends_on");
            // trace:TASK-1140 | ai:claude
            let authorized_by = frontmatter_value(&content, "authorized_by");
            entries.push(BriefListEntry {
                path,
                spec_id,
                agent: agent.clone(),
                generated_at,
                depends_on,
                acked,
                authorized_by,
            });
        }
    }
    sort_brief_entries_topologically(&mut entries)?;
    Ok(entries)
}

/// BUG-569: resolve brief directories for `target`, dropping the ambiguity
/// warning entirely when `warn_on_ambiguity` is false. The warning is only a
/// real user error when the caller explicitly targeted an agent (e.g.
/// `aida brief list --for-agent <target>`); internal scans that pass a bare
/// agent-TYPE name-class must stay silent. Split out so the suppression
/// decision is unit-testable without capturing stderr. trace:BUG-569 | ai:claude
pub(crate) fn resolve_brief_dirs_with_optional_warning(
    project_root: &std::path::Path,
    target: &str,
    warn_on_ambiguity: bool,
) -> (Vec<String>, Option<String>) {
    let (dirs, warning) = agent_registry::resolve_brief_directories(project_root, target);
    let warning = if warn_on_ambiguity { warning } else { None };
    (dirs, warning)
}

pub(crate) fn normalize_brief_dependency(
    store: &RequirementsStore,
    depends_on: Option<&str>,
) -> Result<Option<String>> {
    let Some(depends_on) = depends_on.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    // trace:TASK-1468 | ai:claude
    let req = store
        .get_requirement_unambiguous(depends_on)?
        .ok_or_else(|| not_found::requirement_not_found(depends_on, None))?;
    Ok(Some(
        req.spec_id.as_deref().unwrap_or(depends_on).to_string(),
    ))
}

pub(crate) fn ensure_brief_dependency_is_acyclic(
    project_root: &std::path::Path,
    agent: &str,
    spec_id: &str,
    depends_on: Option<&str>,
) -> Result<()> {
    let Some(depends_on) = depends_on else {
        return Ok(());
    };
    if depends_on == spec_id {
        anyhow::bail!("brief dependency cycle: {spec_id} cannot depend on itself");
    }

    // BUG-569: internal dependency walk — stay silent on type-class ambiguity.
    let mut deps = collect_agent_briefs_inner(project_root, Some(agent), true, false)?
        .into_iter()
        .filter_map(|entry| entry.depends_on.map(|dep| (entry.spec_id, dep)))
        .collect::<std::collections::HashMap<_, _>>();
    deps.insert(spec_id.to_string(), depends_on.to_string());

    let mut cursor = depends_on;
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = deps.get(cursor).map(String::as_str) {
        if next == spec_id {
            anyhow::bail!(
                "brief dependency cycle: adding {spec_id} --depends-on {depends_on} would create a cycle"
            );
        }
        if !seen.insert(next.to_string()) {
            anyhow::bail!("brief dependency cycle detected involving {next}");
        }
        cursor = next;
    }
    Ok(())
}

pub(crate) fn brief_entry_depends_on(
    deps: &std::collections::HashMap<String, String>,
    start: &str,
    target: &str,
) -> bool {
    let mut cursor = start;
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = deps.get(cursor).map(String::as_str) {
        if next == target {
            return true;
        }
        if !seen.insert(next.to_string()) {
            return false;
        }
        cursor = next;
    }
    false
}

pub(crate) fn sort_brief_entries_topologically(entries: &mut [BriefListEntry]) -> Result<()> {
    let deps = entries
        .iter()
        .filter_map(|entry| {
            entry
                .depends_on
                .as_ref()
                .map(|depends_on| (entry.spec_id.clone(), depends_on.clone()))
        })
        .collect::<std::collections::HashMap<_, _>>();

    entries.sort_by(|a, b| {
        if a.agent == b.agent {
            if brief_entry_depends_on(&deps, &a.spec_id, &b.spec_id) {
                return std::cmp::Ordering::Greater;
            }
            if brief_entry_depends_on(&deps, &b.spec_id, &a.spec_id) {
                return std::cmp::Ordering::Less;
            }
        }
        a.agent
            .cmp(&b.agent)
            .then(a.generated_at.cmp(&b.generated_at))
            .then(a.path.cmp(&b.path))
    });
    Ok(())
}

/// BUG-378: substrate-as-bouncer for agent scratchpad drift.
///
/// When an agent is about to declare a spec Done/Completed, scan the brief
/// surface for unacked briefs targeting THIS agent's type and print a loud
/// banner to stderr naming each file path. The agent — about to loop on its
/// own internal scratchpad and re-render "all done" — gets told by the
/// substrate that new work is queued before it gets a chance to exit.
///
/// Gating rules (kept narrow on purpose — false positives would teach agents
/// to ignore the banner):
/// - Agent type detected as `claude`, `codex`, or `antigravity` only. The
///   `"other"` fallback (raw shell, untagged caller) does NOT scan — would
///   noise up every interactive `aida queue done` run by a human.
/// - Only briefs matching the running agent's type are listed. A pending
///   `antigravity` brief never fires when Codex is running.
/// - Banner goes to stderr in bold red so it survives stdout piping and
///   stands out in a wall of green check marks. trace:BUG-378 | ai:claude
pub(crate) fn warn_pending_briefs_for_running_agent(project_root: &std::path::Path) {
    let agent_type = agent_registry::detect_agent_type();
    let Some(lines) = pending_brief_banner_lines(project_root, &agent_type) else {
        return;
    };
    for line in lines {
        eprintln!("{}", line);
    }
}

/// Pure-function core of [`warn_pending_briefs_for_running_agent`] —
/// returns the banner lines (already styled with ANSI red/bold) or `None`
/// when the gate stays silent. Split out so unit tests can assert directly
/// on the rendered output without capturing stderr. trace:BUG-378 | ai:claude
pub(crate) fn pending_brief_banner_lines(
    project_root: &std::path::Path,
    agent_type: &str,
) -> Option<Vec<String>> {
    if !matches!(agent_type, "claude" | "codex" | "antigravity") {
        return None;
    }
    // BUG-569: bare agent-type scan — stay silent on type-class ambiguity.
    let entries = collect_agent_briefs_inner(project_root, Some(agent_type), false, false).ok()?;
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(entries.len() + 4);
    out.push(String::new());
    out.push(
        format!(
            "{} NEW BRIEF(S) PENDING for agent `{}` — read before exiting:",
            crate::glyph(crate::glyphs::Glyph::Warning),
            agent_type
        )
        .red()
        .bold()
        .to_string(),
    );
    for entry in &entries {
        out.push(format!("    {}", entry.path.display()).red().to_string());
    }
    out.push(format!(
        "{} {}",
        "  Run:".red(),
        format!("aida brief list --for-agent {}", agent_type)
            .red()
            .bold()
    ));
    out.push(
        "  Your internal task.md / scratchpad is NOT ground truth — \
         poll the brief surface before declaring work complete."
            .red()
            .to_string(),
    );
    out.push(String::new());
    Some(out)
}

pub(crate) fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }
    let prefix = format!("{key}:");
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(raw) = line.strip_prefix(&prefix) {
            return Some(raw.trim().trim_matches('\'').to_string());
        }
    }
    None
}

pub(crate) fn spec_id_from_brief_filename(name: &str) -> Option<String> {
    let name = name.strip_suffix(".acked").unwrap_or(name);
    let name = name.strip_suffix(".md").unwrap_or(name);
    let (spec, _) = name.rsplit_once('-')?;
    Some(spec.to_string())
}

/// TASK-502: the `.pending` sentinel for an agent — one urgent (unacked,
/// `--notify`'d) brief path per line, project-relative. Lives in the agent's
/// brief dir so `ack`, which only has the brief path, can find it from the
/// brief's parent without needing the project root or agent name.
pub(crate) fn pending_briefs_path(
    project_root: &std::path::Path,
    agent: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("agent-briefs")
        .join(agent)
        .join(".pending")
}

/// Read the current `.pending` entries (project-relative brief paths), trimming
/// blanks. Missing file → empty list.
pub(crate) fn read_pending_briefs(pending_path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(pending_path)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Rewrite `.pending` from a list — delete the file when the list is empty so
/// an empty inbox leaves no sentinel behind.
pub(crate) fn write_pending_briefs(
    pending_path: &std::path::Path,
    entries: &[String],
) -> Result<()> {
    if entries.is_empty() {
        if pending_path.exists() {
            std::fs::remove_file(pending_path)
                .with_context(|| format!("failed to remove empty {}", pending_path.display()))?;
        }
        return Ok(());
    }
    if let Some(parent) = pending_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(pending_path, format!("{}\n", entries.join("\n")))
        .with_context(|| format!("failed to write {}", pending_path.display()))?;
    Ok(())
}

/// Add a brief to the agent's `.pending` sentinel, idempotently (re-notifying
/// the same brief does not duplicate the entry). Stored project-relative.
pub(crate) fn add_pending_brief(
    project_root: &std::path::Path,
    agent: &str,
    brief_path: &std::path::Path,
) -> Result<()> {
    let pending_path = pending_briefs_path(project_root, agent);
    // Store the project-relative key with forward slashes regardless of OS so
    // the `.pending` sentinel is portable and matches the `.aida/agent-briefs/`
    // convention used everywhere else. `Path::display()` emits `\` on Windows,
    // which broke task_492_brief_tests on the cross-platform runner.
    // trace:BUG-466 | ai:claude
    // trace:BUG-1648 | ai:claude — shared with the plan-path writer.
    let rel = plan_rel_path(brief_path, project_root);
    let mut entries = read_pending_briefs(&pending_path);
    if !entries.iter().any(|e| e == &rel) {
        entries.push(rel);
    }
    write_pending_briefs(&pending_path, &entries)
}

/// Remove a brief from its agent's `.pending` sentinel (called on ack). The
/// agent dir is the brief's parent, so we don't need the project root. Matches
/// either the bare brief name or any path entry ending in it, so an
/// absolute-vs-relative mismatch doesn't strand the entry.
pub(crate) fn clear_pending_brief(brief_path: &std::path::Path) {
    let Some(dir) = brief_path.parent() else {
        return;
    };
    let pending_path = dir.join(".pending");
    if !pending_path.exists() {
        return;
    }
    let Some(name) = brief_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let kept: Vec<String> = read_pending_briefs(&pending_path)
        .into_iter()
        .filter(|e| {
            // Drop the entry whose final path component is this brief's file
            // name (handles project-relative vs absolute storage).
            std::path::Path::new(e).file_name().and_then(|n| n.to_str()) != Some(name)
        })
        .collect();
    let _ = write_pending_briefs(&pending_path, &kept);
}

/// TASK-502: scan `.aida/agent-briefs/*/.pending` and return `(agent, count)`
/// for every agent with at least one urgent (notify'd, unacked) brief. When
/// `AIDA_AGENT_NAME` resolves to a known brief-agent, narrow to that one (the
/// "current agent's identity" case); otherwise report all agents.
pub(crate) fn collect_pending_brief_counts(project_root: &std::path::Path) -> Vec<(String, usize)> {
    let briefs_root = project_root.join(".aida").join("agent-briefs");
    let Ok(entries) = std::fs::read_dir(&briefs_root) else {
        return Vec::new();
    };
    // Derive the current brief-agent from the stable session name
    // (e.g. "claude-3f2a" → "claude"), if any.
    let current_agent = std::env::var("AIDA_AGENT_NAME").ok().and_then(|name| {
        ["claude", "codex", "antigravity"]
            .into_iter()
            .find(|a| name == *a || name.starts_with(&format!("{a}-")))
            .map(str::to_string)
    });
    let mut out: Vec<(String, usize)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(agent) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if let Some(cur) = &current_agent {
            if &agent != cur {
                continue;
            }
        }
        let count = read_pending_briefs(&entry.path().join(".pending")).len();
        if count > 0 {
            out.push((agent, count));
        }
    }
    out.sort();
    out
}

pub(crate) fn ack_agent_brief(brief_file: &std::path::Path) -> Result<()> {
    let path = brief_file;
    if !path.exists() {
        anyhow::bail!("brief file not found: {}", path.display());
    }
    if path.extension().and_then(|e| e.to_str()) == Some("acked") {
        println!("Already acknowledged: {}", path.display());
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid brief file path: {}", path.display()))?;
    let acked_path = path.with_file_name(format!("{file_name}.acked"));
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read brief {}", path.display()))?;
    let content = if content.contains("\nstatus: pending\n") {
        content.replacen("\nstatus: pending\n", "\nstatus: acked\n", 1)
    } else {
        content
    };
    std::fs::write(path, content)
        .with_context(|| format!("failed to update brief status in {}", path.display()))?;
    std::fs::rename(path, &acked_path).with_context(|| {
        format!(
            "failed to acknowledge brief {} -> {}",
            path.display(),
            acked_path.display()
        )
    })?;
    // TASK-502: drop this brief from the agent's `.pending` sentinel (no-op if
    // it was never --notify'd). Uses the pre-rename path's file name.
    clear_pending_brief(path);
    println!("Acknowledged: {}", acked_path.display());
    Ok(())
}

pub(crate) fn read_agent_brief(
    project_root: &std::path::Path,
    brief_file: &str,
    latest: bool,
) -> Result<()> {
    let resolved_path = if latest {
        let agent = validate_brief_agent(brief_file)?;
        let (dirs, warning) = agent_registry::resolve_brief_directories(project_root, agent);
        if let Some(warn) = warning {
            eprintln!("{}", warn);
        }
        let chosen_agent = if dirs.is_empty() {
            agent.to_string()
        } else {
            dirs[0].clone()
        };
        // BUG-569: the explicit-target warning already fired above via
        // resolve_brief_directories; this re-scan stays silent to avoid a
        // double warning.
        let entries = collect_agent_briefs_inner(project_root, Some(&chosen_agent), false, false)?;
        let last_entry = entries.last().ok_or_else(|| {
            anyhow::anyhow!(
                "Error: no pending briefs found for agent \"{}\". Use 'aida brief list' to view available briefs.",
                chosen_agent
            )
        })?;
        last_entry.path.clone()
    } else {
        let raw_path = std::path::Path::new(brief_file);
        if raw_path.exists() {
            raw_path.to_path_buf()
        } else {
            let relative_path = project_root.join(brief_file);
            if relative_path.exists() {
                relative_path
            } else if let Some((agent, filename)) = brief_file.split_once('/') {
                let agent = validate_brief_agent(agent)?;
                let (dirs, warning) =
                    agent_registry::resolve_brief_directories(project_root, agent);
                if let Some(warn) = warning {
                    eprintln!("{}", warn);
                }
                let chosen_agent = if dirs.is_empty() {
                    agent.to_string()
                } else {
                    dirs[0].clone()
                };
                let base_path = project_root
                    .join(".aida")
                    .join("agent-briefs")
                    .join(&chosen_agent)
                    .join(filename);
                if base_path.exists() {
                    base_path
                } else {
                    let md_path = base_path.with_extension("md");
                    if md_path.exists() {
                        md_path
                    } else {
                        let acked_path = base_path.with_extension("acked");
                        if acked_path.exists() {
                            acked_path
                        } else {
                            let filename_str = filename.to_string();
                            if filename_str.ends_with(".md") {
                                let without_ext = &filename_str[..filename_str.len() - 3];
                                let acked =
                                    base_path.with_file_name(format!("{}.acked", without_ext));
                                if acked.exists() {
                                    acked
                                } else {
                                    anyhow::bail!(
                                        "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                        brief_file
                                    );
                                }
                            } else if filename_str.ends_with(".acked") {
                                let without_ext = &filename_str[..filename_str.len() - 6];
                                let md = base_path.with_file_name(format!("{}.md", without_ext));
                                if md.exists() {
                                    md
                                } else {
                                    anyhow::bail!(
                                        "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                        brief_file
                                    );
                                }
                            } else {
                                anyhow::bail!(
                                    "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                                    brief_file
                                );
                            }
                        }
                    }
                }
            } else {
                anyhow::bail!(
                    "Error: brief not found at \"{}\". Use 'aida brief list' to view available briefs.",
                    brief_file
                );
            }
        }
    };

    let body = std::fs::read_to_string(&resolved_path)
        .with_context(|| format!("failed to read brief at {}", resolved_path.display()))?;
    print!(
        "{}",
        render_agent_brief_read(project_root, &resolved_path, &body)?
    );
    Ok(())
}

pub(crate) fn render_agent_brief_read(
    project_root: &std::path::Path,
    path: &std::path::Path,
    body: &str,
) -> Result<String> {
    let Some(depends_on) = frontmatter_value(body, "depends_on") else {
        return Ok(body.to_string());
    };
    let agent = frontmatter_value(body, "agent")
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if agent.is_empty() {
        return Ok(body.to_string());
    }
    // BUG-569: internal render scan — stay silent on type-class ambiguity.
    let pending = collect_agent_briefs_inner(project_root, Some(&agent), false, false)?;
    if pending.iter().any(|entry| {
        entry.spec_id == depends_on
            && entry.path != path
            && entry.path.with_extension("md") != path.with_extension("md")
    }) {
        return Ok(format!("Blocked by: {depends_on}\n\n{body}"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
#[path = "tests/task_492_brief_tests.rs"]
mod task_492_brief_tests;

/// Validate a status string against the canonical set. Accepts case-
/// insensitive matches and common spelling variants (`in-progress`,
/// `inprogress`, `in_progress`). Returns Ok with the canonical form, or
/// Err with a list-of-valid-values message. Use at the CLI layer before
/// calling `Requirement::set_status_from_str` to prevent typos like
/// `approvedxxx` from silently landing as a `custom_status`. trace:BUG-47
/// The canonical status list echoed by every "invalid status" refusal, so the
/// plain validator and its type-aware wrapper can never disagree on what the
/// CLI accepts.
// trace:TASK-1176 | ai:claude
pub(crate) const VALID_STATUS_INPUTS: &str =
    "draft, approved, planned, in-progress, done, completed, rejected, superseded, needs-attention";

pub fn validate_status_input(raw: &str) -> Result<&'static str, String> {
    let normalized: String = raw
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' | '_' => None,
            c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            c => Some(c),
        })
        .collect();
    match normalized.as_str() {
        "draft" => Ok("Draft"),
        "approved" => Ok("Approved"),
        "planned" => Ok("Planned"),
        "inprogress" => Ok("InProgress"),
        // trace:STORY-86 | ai:claude — "done" is now its own state, not an alias for Completed.
        "done" => Ok("Done"),
        "completed" => Ok("Completed"),
        "rejected" => Ok("Rejected"),
        // trace:TASK-1176 | ai:claude — adopted-then-replaced (NOT declined).
        "superseded" => Ok("Superseded"),
        // trace:STORY-332 | ai:claude — the punt/pause state.
        "needsattention" => Ok("NeedsAttention"),
        _ => Err(format!(
            "invalid status `{}` — expected one of: {}",
            raw, VALID_STATUS_INPUTS
        )),
    }
}

/// Same shape, for priority. trace:BUG-47
pub fn validate_priority_input(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" => Ok("High"),
        "medium" | "med" => Ok("Medium"),
        "low" => Ok("Low"),
        _ => Err(format!(
            "invalid priority `{}` — expected one of: high, medium, low",
            raw
        )),
    }
}

/// Detect signs that a `--title` was mangled by shell command-substitution
/// (backticks the user forgot to escape, an unmatched quote that lost the
/// rest of the string). Returns Some(message) if suspicious. Caller should
/// print as a warning — never reject — since false positives are possible.
/// trace:BUG-22 | ai:claude
pub(crate) fn suspicious_title_signal(title: &str) -> Option<String> {
    if title.contains('`') {
        return Some(
            "title contains a backtick — if you meant a literal `, escape it (\\\\`) \
             or quote the whole title in single quotes; otherwise the shell may have \
             mangled it"
                .to_string(),
        );
    }
    // An odd count of unescaped double-quotes is a strong signal of broken quoting.
    let dq_count = title.chars().filter(|c| *c == '"').count();
    if dq_count % 2 == 1 {
        return Some(
            "title contains an unbalanced double-quote — likely a shell-quoting artifact"
                .to_string(),
        );
    }
    None
}

pub(crate) fn config_path_for_project(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("config.toml")
}

pub(crate) fn config_parse_error_message(
    path: &std::path::Path,
    body: &str,
    err: &toml::de::Error,
) -> String {
    let loc = err
        .span()
        .map(|span| byte_offset_line_col(body, span.start))
        .map(|(line, col)| format!(":{line}:{col}"))
        .unwrap_or_default();
    format!(
        "{}{}: failed to parse AIDA config: {err}\n  Fix the TOML syntax, then re-run the command. If this came from `aida pull`, inspect any sibling `{}.conflicted` file and merge it manually.",
        path.display(),
        loc,
        path.display()
    )
}

pub(crate) fn byte_offset_line_col(body: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, ch) in body.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub(crate) fn config_conflict_marker_line(body: &str) -> Option<usize> {
    body.lines().enumerate().find_map(|(idx, line)| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<<<<<<< ")
            || trimmed == "<<<<<<<"
            || trimmed.starts_with("=======")
            || trimmed.starts_with(">>>>>>> ")
            || trimmed == ">>>>>>>"
        {
            Some(idx + 1)
        } else {
            None
        }
    })
}

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    pub(crate) path: std::path::PathBuf,
    pub(crate) before: Option<String>,
}

pub(crate) fn known_project_config_paths(
    project_root: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let aida = project_root.join(".aida");
    vec![aida.join("config.toml"), aida.join("agents.toml")]
}

pub(crate) fn snapshot_known_project_configs(
    project_root: &std::path::Path,
) -> Vec<ConfigSnapshot> {
    known_project_config_paths(project_root)
        .into_iter()
        .map(|path| ConfigSnapshot {
            before: std::fs::read_to_string(&path).ok(),
            path,
        })
        .collect()
}

pub(crate) fn validate_and_restore_project_configs_after_pull(
    snapshots: &[ConfigSnapshot],
) -> Result<()> {
    for snapshot in snapshots {
        let Ok(after) = std::fs::read_to_string(&snapshot.path) else {
            continue;
        };
        let failure = if let Some(line) = config_conflict_marker_line(&after) {
            Some(format!(
                "{}:{line}: conflict marker found in AIDA config after pull",
                snapshot.path.display()
            ))
        } else {
            match toml::from_str::<toml::Value>(&after) {
                Ok(_) => None,
                Err(err) => Some(config_parse_error_message(&snapshot.path, &after, &err)),
            }
        };
        let Some(message) = failure else {
            continue;
        };

        let conflicted = snapshot.path.with_extension("toml.conflicted");
        aida_core::write_atomic(&conflicted, after)
            .with_context(|| format!("failed to quarantine {}", conflicted.display()))?;
        match &snapshot.before {
            Some(before) => {
                // trace:BUG-1025 | ai:codex
                aida_core::write_atomic(&snapshot.path, before.clone())
                    .with_context(|| format!("failed to restore {}", snapshot.path.display()))?;
            }
            None => match std::fs::remove_file(&snapshot.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("failed to remove {}", snapshot.path.display()));
                }
            },
        }
        anyhow::bail!(
            "{message}\n  Quarantined the conflicted version at {}.\n  Restored the pre-pull version at {}.\n  Manual merge step: compare both files, edit {}, then re-run `aida pull`.",
            conflicted.display(),
            snapshot.path.display(),
            snapshot.path.display()
        );
    }
    Ok(())
}

/// TASK-304: `[ultraplan] mode` governs whether AIDA proactively suggests
/// `aida ultraplan <SPEC>` for chunky specs. Planning is human/agent driven,
/// so the realistic surface is `never | on-demand | suggested` — never a
/// "frequently auto-pull" mode.
/// trace:TASK-304 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UltraplanMode {
    /// `aida ultraplan` refuses with a configured-off message; no clipboard.
    Never,
    /// Default — preserves current behavior: the user runs `aida ultraplan
    /// SPEC` explicitly, no pickup-time hints.
    OnDemand,
    /// Pickup surfaces (`/aida-pickup`, `aida queue work` no-arg head,
    /// `aida queue list` head row) hint when the head spec is chunky.
    Suggested,
}

impl UltraplanMode {
    pub(crate) fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "never" => Some(Self::Never),
            "on-demand" | "on_demand" | "ondemand" => Some(Self::OnDemand),
            "suggested" => Some(Self::Suggested),
            _ => None,
        }
    }
}

/// TASK-697: which heuristic the `suggested` mode uses to decide a spec is
/// worth a planning prompt.
///
/// SPIKE-8 (`docs/spikes/2026-06-07-spike-8-ultraplan-comparison.md`) found
/// that `aida ultraplan`'s value is a *context-assembly* aid: it helps most on
/// thin / under-specified specs and least on well-formed ones (which already
/// carry their own `## Proposed shape` + Acceptance — the spec *is* the plan).
/// So acceptance-bullet count *anti-correlates* with where planning helps — a
/// 9-bullet spec with a design block needs planning less than a 2-bullet spec
/// with none. `Thinness` keys on that signal and is the default; the legacy
/// `acceptance-bullets>N` heuristic is still honored for backward compat.
/// trace:TASK-697 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestThreshold {
    /// Legacy (TASK-304): suggest when the `## Acceptance` checkbox count is
    /// strictly greater than `N`.
    BulletCount(usize),
    /// TASK-697: suggest when the spec is *under-specified* — no design
    /// section yet a substantive body (see `is_spec_thin`).
    Thinness,
}

/// TASK-304/TASK-697: parsed `[ultraplan]` config. Both fields default safely
/// so a project with no `[ultraplan]` block (or an unparseable one) keeps the
/// opt-in behavior: `mode = on-demand`, threshold `spec-thinness`.
/// trace:TASK-697 | ai:claude
pub(crate) struct UltraplanConfig {
    pub(crate) mode: UltraplanMode,
    /// Which heuristic `suggested` mode applies. Defaults to `Thinness`.
    pub(crate) threshold: SuggestThreshold,
}

impl Default for UltraplanConfig {
    fn default() -> Self {
        Self {
            mode: UltraplanMode::OnDemand,
            threshold: SuggestThreshold::Thinness,
        }
    }
}

/// TASK-304: read `[ultraplan]` from `.aida/config.toml`. Missing file,
/// missing block, unparseable TOML, or unknown token values all fall back to
/// the defaults — the suggestion layer is a soft feature, never load-bearing.
/// trace:TASK-304 | ai:claude
pub(crate) fn read_ultraplan_config(project_root: &std::path::Path) -> UltraplanConfig {
    let mut cfg = UltraplanConfig::default();
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return cfg;
    };
    let Some(table) = value.get("ultraplan") else {
        return cfg;
    };
    if let Some(mode) = table
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(UltraplanMode::from_token)
    {
        cfg.mode = mode;
    }
    if let Some(threshold) = table
        .get("suggest_threshold")
        .and_then(|v| v.as_str())
        .and_then(parse_suggest_threshold)
    {
        cfg.threshold = threshold;
    }
    cfg
}

/// TASK-304: parse a `suggest_threshold` token. Only `acceptance-bullets>N`
/// is honored today (the spec's chosen default heuristic — mechanical, no
/// NLP, falsifiable); the `N` is extracted. Unknown tokens return `None` so
/// the caller keeps the default threshold. trace:TASK-304 | ai:claude
pub(crate) fn parse_acceptance_bullet_threshold(token: &str) -> Option<usize> {
    token
        .trim()
        .to_ascii_lowercase()
        .strip_prefix("acceptance-bullets>")?
        .trim()
        .parse::<usize>()
        .ok()
}

/// TASK-697: parse a `suggest_threshold` token into a `SuggestThreshold`.
/// `acceptance-bullets>N` → `BulletCount(N)` (legacy, still honored);
/// `spec-thinness` / `thinness` / `under-specified` → `Thinness` (the SPIKE-8
/// recommendation, the new default). Unknown tokens return `None` so the
/// caller keeps the default threshold. trace:TASK-697 | ai:claude
pub(crate) fn parse_suggest_threshold(token: &str) -> Option<SuggestThreshold> {
    if let Some(n) = parse_acceptance_bullet_threshold(token) {
        return Some(SuggestThreshold::BulletCount(n));
    }
    match token.trim().to_ascii_lowercase().as_str() {
        "spec-thinness" | "thinness" | "under-specified" => Some(SuggestThreshold::Thinness),
        _ => None,
    }
}

/// TASK-697: heading prefixes that mark a spec as carrying its own design —
/// the signal that planning would add little (the spec already *is* a plan).
/// Matched case-insensitively against the heading text after the `#` markers.
/// trace:TASK-697 | ai:claude
pub(crate) const DESIGN_SECTION_MARKERS: &[&str] = &[
    "proposed shape",
    "proposed solution",
    "proposed design",
    "design",
    "approach",
    "implementation",
];

/// TASK-697: a spec with `< THIN_MIN_BODY_CHARS` of body is too trivial to
/// plan — planning overhead would exceed the work. Above it, a design-less
/// spec is "thin" in the sense that matters (under-specified, not too-small).
pub(crate) const THIN_MIN_BODY_CHARS: usize = 240;

/// TASK-697: does the spec carry a design / proposed-shape section? Scans
/// markdown headings (any level) for a `DESIGN_SECTION_MARKERS` prefix.
/// trace:TASK-697 | ai:claude
pub(crate) fn has_design_section(description: &str) -> bool {
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            if DESIGN_SECTION_MARKERS
                .iter()
                .any(|marker| heading.starts_with(marker))
            {
                return true;
            }
        }
    }
    false
}

/// TASK-697: is this spec "thin" — under-specified enough that a planning
/// prompt would plausibly add value? True when it has NO design section yet a
/// substantive body. Well-specified specs (which embed `## Proposed shape` and
/// the like) stay quiet — exactly the SPIKE-8 finding that bullet count is the
/// wrong signal. Trivial one-liners also stay quiet (too small to plan).
/// trace:TASK-697 | ai:claude
pub(crate) fn is_spec_thin(description: &str) -> bool {
    !has_design_section(description) && description.trim().chars().count() >= THIN_MIN_BODY_CHARS
}

/// TASK-304: count the markdown task-list bullets (`- [ ]` / `- [x]`) inside
/// a spec's `## Acceptance` section — a rough complexity proxy. Returns 0
/// when there's no Acceptance section. Checked and unchecked items both
/// count: a long checklist is chunky regardless of how much is already
/// ticked. Tolerates `*`/`+` bullet markers and `## Acceptance Criteria`
/// heading variants. trace:TASK-304 | ai:claude
pub(crate) fn count_acceptance_bullets(description: &str) -> usize {
    let mut in_section = false;
    let mut count = 0;
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            // A markdown heading at any level — entering or leaving the
            // Acceptance section resets the counter scope.
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            in_section = heading.starts_with("acceptance");
            continue;
        }
        if in_section && is_task_bullet(trimmed) {
            count += 1;
        }
    }
    count
}

/// TASK-304: is this a markdown task-list item (`- [ ]`, `* [x]`, `+ [X]`)?
pub(crate) fn is_task_bullet(trimmed: &str) -> bool {
    let after_marker = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "));
    match after_marker {
        Some(rest) => {
            let r = rest.trim_start();
            r.starts_with("[ ]") || r.starts_with("[x]") || r.starts_with("[X]")
        }
        None => false,
    }
}

/// TASK-304: the pickup-time suggestion hint. Returns `Some(message)` only
/// when `[ultraplan] mode = "suggested"` AND the spec's acceptance checklist
/// is longer than the configured threshold; `None` otherwise (the common
/// case — on-demand/never, or a spec that isn't chunky). The message format
/// is fixed by the spec so downstream surfaces render it identically.
/// trace:TASK-304 | ai:claude
pub(crate) fn ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) -> Option<String> {
    let cfg = read_ultraplan_config(project_root);
    if cfg.mode != UltraplanMode::Suggested {
        return None;
    }
    let id = req.display_id();
    match cfg.threshold {
        // Legacy (TASK-304): fire on a long acceptance checklist.
        SuggestThreshold::BulletCount(n) => {
            let bullets = count_acceptance_bullets(&req.description);
            if bullets <= n {
                return None;
            }
            Some(format!(
                "{id} has {bullets} acceptance bullets — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
        // TASK-697: fire on an under-specified (design-less) spec — where
        // planning actually adds value.
        SuggestThreshold::Thinness => {
            if !is_spec_thin(&req.description) {
                return None;
            }
            Some(format!(
                "{id} has no design section yet — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
    }
}

/// TASK-304: render the ultraplan suggestion hint to stdout under the
/// shared pickup surfaces (queue next / work / list). No-op when the helper
/// returns `None`. trace:TASK-304 | ai:claude
pub(crate) fn print_ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) {
    if let Some(hint) = ultraplan_suggestion_hint(project_root, req) {
        println!();
        println!("  {} {}", "⤷".cyan(), hint);
    }
}

/// TASK-304: the `[ultraplan]` block `aida init` scaffolds into a new
/// project's `.aida/config.toml`. Ships `mode = "on-demand"` (preserves
/// current behavior) plus a comment explaining the never/on-demand/suggested
/// trade-off and why "frequently/auto-pull" isn't an option. Appended like
/// the `[forge]` section so all three init paths share one source of truth.
/// trace:TASK-304 | ai:claude
pub(crate) fn init_ultraplan_config_section() -> &'static str {
    "\n# trace:TASK-304 | ai:claude  (suggest_threshold: trace:TASK-697)\n\
     # Whether AIDA proactively suggests `aida ultraplan <SPEC>` for specs\n\
     # where planning would help. Planning is human/agent driven, so there\n\
     # is no \"auto-pull\" mode — only:\n\
     #   never      — `aida ultraplan` is disabled (refuses with a message)\n\
     #   on-demand  — current behavior: run `aida ultraplan SPEC` yourself\n\
     #   suggested  — pickup surfaces (/aida-pickup, queue work/list head)\n\
     #                hint when the head spec passes suggest_threshold\n\
     # suggest_threshold only applies in `suggested` mode. Heuristics:\n\
     #   spec-thinness       — (default) suggest when the spec is\n\
     #                         under-specified: no design section (## Proposed\n\
     #                         shape / Design / Approach / ...) yet a\n\
     #                         substantive body. Well-specified specs already\n\
     #                         carry their plan, so they stay quiet (SPIKE-8).\n\
     #   acceptance-bullets>N — (legacy) suggest when the `## Acceptance`\n\
     #                         checkbox count exceeds N. Note: bullet count\n\
     #                         anti-correlates with where planning helps.\n\
     [ultraplan]\n\
     mode = \"on-demand\"\n\
     suggest_threshold = \"spec-thinness\"\n"
}

// trace:TASK-760 | ai:claude
// The commented `[intake]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. `aida intake` works with zero config (every
// knob has a safe default), so the block ships fully commented-out — it exists
// purely to make the policy knobs discoverable next to the other sections.
// Appended like the `[forge]` / `[ultraplan]` sections so all init paths
// share one source of truth. The example values mirror the defaults in
// `intake::IntakeConfig::default()` — keep them in lockstep.
pub(crate) fn init_intake_config_section() -> &'static str {
    "\n# Policy for `aida intake` — the headless advisor pass that reads open\n\
     # specs and proposes approve/reject/park/queue dispositions (propose-only\n\
     # by default; `--apply` executes). Every knob has a safe default, so this\n\
     # whole section is optional — uncomment a line only to change a default.\n\
     #\n\
     # disposition_bias — how aggressively the agent proposes approve:\n\
     #   approve-eligible  — (default) propose approve for every eligible\n\
     #                       spec; the propose-mode review is the filter\n\
     #   park-aligned      — approve only when a spec is BOTH eligible AND\n\
     #                       clearly aligned with project priorities; else\n\
     #                       park for a human\n\
     #   park-conservative — park whenever unsure\n\
     #\n\
     # do_not_approve_classes — requirement types the agent can NEVER propose\n\
     # approve for; they are fenced out of its candidate set entirely. The\n\
     # default is the strategic + knowledge-graph types. An empty list opens\n\
     # the gate (the propose-mode review still applies).\n\
     #\n\
     # on_apply — what `--apply` does after queuing approved specs:\n\
     #   queue — (default) stop at queuing; draining stays a separate,\n\
     #           explicit command\n\
     #   drain — chain straight into an implementer drain after queuing\n\
     #\n\
     # [intake]\n\
     # disposition_bias = \"approve-eligible\"\n\
     # do_not_approve_classes = [\"vision\", \"epic\", \"principle\", \"constraint\", \"decision\", \"term\"]\n\
     # on_apply = \"queue\"\n"
}

// trace:TASK-1021 trace:TASK-0429 | ai:claude
// The commented `[autopilot]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. Same contract as the `[intake]` block above:
// every knob has a conservative default, so the whole section ships
// commented-out and exists purely to make the authority map discoverable next
// to the policy section it composes with.
//
// ONLY the nine action keys are documented, because only those parse today
// (`autopilot::ActionClass::parse` x `autopilot::Authority::parse`). The
// example values mirror `autopilot::AutopilotEnvelope::default()` — keep them
// in lockstep. The comment text is user-facing scaffolding, so it carries no
// SPEC-IDs and states plainly which surfaces honor the envelope today.
pub(crate) fn init_autopilot_config_section() -> &'static str {
    "\n# Authority envelope for advisor autopilot (`aida autopilot`). Autopilot is\n\
     # not a second disposition engine — it is a bounded-authority wrapper over\n\
     # the same grooming pass `[intake]` above configures. For each disposition\n\
     # the advisor proposes, the envelope decides one of: auto-execute, hold for\n\
     # a human, or escalate. Every knob has a conservative default, so this whole\n\
     # section is optional — uncomment a line only to change a default.\n\
     #\n\
     # One key per action class; each takes one of three values:\n\
     #   auto    — autopilot may perform the action on its own\n\
     #   propose — autopilot may only propose it; a human reviews before it runs\n\
     #   never   — autopilot may never perform it\n\
     #\n\
     # Defaults: the reversible, low-blast actions are `auto`; the two\n\
     # irreversible dispositions (approve, reject) are `propose`. An unknown key\n\
     # or an unknown value is ignored, leaving that action at its default.\n\
     #\n\
     # These knobs are a CEILING, not a blanket grant. Three bounds sit OUTSIDE\n\
     # the map and cannot be widened here:\n\
     #   * the candidate fence — which specs are touchable at all (`[intake]`);\n\
     #   * grounding — a call resting on judgment a freshly-started agent cannot\n\
     #     reconstruct from what is written down always escalates;\n\
     #   * the risk ceiling — high-risk and unknown-blast-radius work always\n\
     #     escalates.\n\
     # Context can only TIGHTEN the envelope, never widen it: an unattended run\n\
     # demotes every `propose` to `never` (nobody is there to be asked), and an\n\
     # active solo posture on keystone work demotes everything to `never`.\n\
     #\n\
     # What the envelope drives today: `aida autopilot inspect` — a read-only\n\
     # dry-run of what autopilot WOULD decide over the current groom candidates,\n\
     # recorded to a local audit log you can review (`aida autopilot audit`) and\n\
     # reverse (`aida autopilot challenge`) — and the draft approve-gate on\n\
     # `aida zen <spec>`. There is deliberately NO autopilot execution path on\n\
     # the grooming pass itself yet, so widening a key below changes those two\n\
     # surfaces and nothing else.\n\
     #\n\
     # [autopilot]\n\
     # approve = \"propose\"\n\
     # reject = \"propose\"\n\
     # dedupe = \"auto\"\n\
     # tag = \"auto\"\n\
     # queue = \"auto\"\n\
     # park = \"auto\"\n\
     # route = \"auto\"\n\
     # comment = \"auto\"\n\
     # ask = \"auto\"\n"
}

/// The `[store.sync] mirror_remotes` scaffold — commented. Automatic store
/// pushes go to `origin` only by default; explicit sync commands and an
/// opt-in `hub-mirror-sync` schedule can update configured mirror hubs.
// trace:TASK-1096 | ai:claude
// trace:BUG-1746 | ai:codex
pub(crate) fn init_store_mirror_config_section() -> &'static str {
    r#"
# Store-sync fan-out (STORY-760). By default automatic store pushes send
# the orphan store to `origin` only. Mirror hubs are also updated by
# `aida db sync --push`, `aida pull`, `aida remote mirror-sync`, or an
# opt-in `hub-mirror-sync` schedule job. Register that job hourly (strictly
# shorter than any drift guard interval) so transient failures get retries;
# no mirror-sync route job is intended, since `hub-drift-guard` escalates
# drift that persists. A non-fast-forward mirror-sync failure exits non-zero
# and must be reconciled deliberately; ordinary store writes remain best-effort.
# Check drift anytime with `aida remote status`.
#
# [store.sync]
# mirror_remotes = ["gitlab"]
"#
}

// trace:TASK-1522 | ai:antigravity
pub(crate) fn init_capture_config_section() -> &'static str {
    "\n# Effort-balance intent-capture target (TASK-1522): advisory floor on the\n\
     # share of newly completed specs with at least one traced test criterion.\n\
     # Reported by `aida criteria coverage` and `aida status`; not enforced.\n\
     #\n\
     # [capture]\n\
     # criterion_test_floor_pct = 50\n"
}

/// The `[worktree]` scaffold section. AIDA-created worktrees are expected to be
/// ready for builds, so recursive submodule initialization is default-on with a
/// visible opt-out.
// trace:BUG-899 | ai:codex
pub(crate) fn init_worktree_config_section() -> &'static str {
    "\n# Worktree creation. AIDA initializes recursive git submodules by default\n\
     # after creating or claiming a worktree, so repos that vendor dependencies\n\
     # through `.gitmodules` are build-ready before pickup succeeds. Set this\n\
     # false to skip the potentially-expensive init; AIDA will print the exact\n\
     # recovery command when `.gitmodules` is present.\n\
     [worktree]\n\
     init_submodules = true\n"
}

/// The `[worktree_pool]` scaffold section. Pooling is ON by default (TASK-985):
/// `aida session start` (and the agent-new / queue-work / orchestrator paths)
/// reuse a recycled warm worktree instead of `git worktree add`, keeping the
/// build cache warm across fan-out (~30× faster per-spec builds — see
/// docs/research/2026-06-29-warm-pool-build-delta.md).
// trace:TASK-985 | ai:claude
pub(crate) fn init_worktree_pool_config_section() -> &'static str {
    "\n# Worktree warm-pool (STORY-714). Pooling is ON by default: a session's\n\
     # worktree is acquired from a recycled pool and RESET (not deleted) on\n\
     # hand-back, so its compiled `target/` stays warm across fan-out. Escape\n\
     # hatches: set `enabled = false` here, pass `--no-pool` to a single\n\
     # `aida session start`, or pass `--remove` to `aida session end` to delete\n\
     # the worktree instead of returning it.\n\
     [worktree_pool]\n\
     enabled = true\n\
     # max_trees = 16   # cap on pooled worktrees (default 16)\n\
     # worktree_parent = \"../aida-worktrees\"   # nest every AIDA-created worktree\n\
     #                          # under ONE directory instead of scattering them as\n\
     #                          # siblings of the repo. An editor/agent folder-trust\n\
     #                          # grant inherits to children, so you trust this one\n\
     #                          # directory once and no future worktree prompts\n\
     #                          # again. Relative paths resolve against the repo\n\
     #                          # root; unset keeps the sibling layout (default)\n\
     # lease_ttl_secs = 21600   # a durable lease older than this (with no live\n\
     #                          # owner) is treated as EXPIRED and reclaimed by\n\
     #                          # the next acquire — guards against reservation\n\
     #                          # leaks when a session dies (default 6h)\n\
     #\n\
     # Pre-warm on create: set `prewarm_build = true` to kick off a backgrounded\n\
     # `cargo build` when a NEW pool tree is created, so its `target/` is warm\n\
     # before the first fanned agent builds in it (best-effort, non-blocking).\n\
     # Like every worktree-pool hook, this executes a build, so it is honored\n\
     # ONLY from your machine-global `~/.aida/config.toml` — never a checked-in\n\
     # repo config — and is ignored here.\n"
}

#[cfg(test)]
#[path = "tests/story_569_review_brief_tests.rs"]
mod story_569_review_brief_tests;

#[cfg(test)]
#[path = "tests/task_760_intake_config_section_tests.rs"]
mod task_760_intake_config_section_tests;

#[cfg(test)]
#[path = "tests/task_1021_autopilot_config_section_tests.rs"]
mod task_1021_autopilot_config_section_tests;

#[cfg(test)]
#[path = "tests/task_304_ultraplan_cadence_tests.rs"]
mod task_304_ultraplan_cadence_tests;

pub(crate) fn line_for_key(body: &str, section: &str, key: &str) -> Option<usize> {
    let mut in_section = false;
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed.trim_matches(&['[', ']'][..]) == section;
            continue;
        }
        if in_section && trimmed.starts_with(key) {
            return Some(idx + 1);
        }
    }
    None
}

pub(crate) fn read_store_sync_config(project_root: &std::path::Path) -> Result<StoreSyncConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreSyncConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(sync) = value.get("store").and_then(|s| s.get("sync")) else {
        return Ok(StoreSyncConfig {
            source: path.display().to_string(),
            ..StoreSyncConfig::default()
        });
    };
    let raw = sync
        .get("auto_push")
        .and_then(|v| v.as_str())
        .unwrap_or("manual");
    let Some(auto_push) = StoreAutoPushMode::parse(raw) else {
        let line = line_for_key(&body, "store.sync", "auto_push")
            .map(|n| format!(":{n}"))
            .unwrap_or_default();
        anyhow::bail!(
            "{}{}: invalid [store.sync] auto_push value `{}` (expected: manual, session-end, per-write, periodic)",
            path.display(),
            line,
            raw
        );
    };
    // TASK-1096: mirror_remotes = ["gitlab", ...] — extra hubs the store push
    // fans out to. Non-string entries are skipped; a bare string is tolerated as
    // a single-remote shorthand.
    let mirror_remotes = match sync.get("mirror_remotes") {
        Some(toml::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
        Some(toml::Value::String(s)) => vec![s.to_string()],
        _ => Vec::new(),
    };
    Ok(StoreSyncConfig {
        auto_push,
        periodic_threshold: sync
            .get("periodic_threshold")
            .and_then(|v| v.as_integer())
            .and_then(|n| u64::try_from(n).ok()),
        periodic_interval: sync
            .get("periodic_interval")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        mirror_remotes,
        source: path.display().to_string(),
    })
}

pub(crate) fn read_store_allocation_config(
    project_root: &std::path::Path,
) -> Result<StoreAllocationConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreAllocationConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(allocation) = value
        .get("store")
        .and_then(|s| s.get("allocation"))
        .and_then(|v| v.as_table())
    else {
        return Ok(StoreAllocationConfig::default());
    };
    let retry_max = allocation
        .get("retry_max")
        .and_then(|v| v.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(StoreAllocationConfig::default().retry_max);
    Ok(StoreAllocationConfig { retry_max })
}

pub(crate) fn warn_if_periodic_auto_push(project_root: &std::path::Path) {
    if let Ok(cfg) = read_store_sync_config(project_root) {
        if cfg.auto_push == StoreAutoPushMode::Periodic {
            eprintln!(
                "{} [store.sync] auto_push = \"periodic\" requires aida-worker (EPIC-30); falling back to manual until shipped",
                "Warning:".yellow().bold()
            );
        }
    }
}

pub(crate) fn find_spec_id_collisions(store: &RequirementsStore) -> Vec<SpecIdCollision> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for req in &store.requirements {
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        by_spec
            .entry(spec_id.to_ascii_uppercase())
            .or_default()
            .push(SpecIdClaimant {
                uuid: req.id,
                title: req.title.clone(),
            });
    }

    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            let unique_uuids: BTreeSet<Uuid> = claimants.iter().map(|c| c.uuid).collect();
            if unique_uuids.len() <= 1 {
                return None;
            }
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

pub(crate) fn spec_id_collision_recovery_message(
    collisions: &[SpecIdCollision],
    store_path: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str("duplicate AIDA spec IDs detected after syncing the git-canonical store\n");
    out.push_str(
        "AIDA is refusing to continue before a divergent SPEC-ID silently drops content.\n\n",
    );
    for collision in collisions.iter().take(5) {
        out.push_str(&format!("  {} is claimed by:\n", collision.spec_id));
        for claimant in &collision.claimants {
            out.push_str(&format!("    - {} — {}\n", claimant.uuid, claimant.title));
        }
    }
    if collisions.len() > 5 {
        out.push_str(&format!(
            "  ... plus {} more duplicate id(s)\n",
            collisions.len() - 5
        ));
    }
    out.push_str("\nPaste-ready recovery:\n");
    out.push_str(&format!("  cd {}\n", store_path.display()));
    out.push_str("  git status\n");
    out.push_str("  # inspect the duplicate object(s), then preserve both contents manually\n");
    out.push_str("  # planned tooling: aida db check --collisions --show-conflict\n");
    out.push_str("  # planned tooling: aida db check --collisions --repair\n");
    out.push_str(
        "\nDo not use `git rebase --skip` unless you intentionally want to drop one side.\n",
    );
    out
}

/// Group the flat `(spec_id, uuid, title)` rows the cache returns into
/// `SpecIdCollision`s — one per spec_id claimed by ≥2 distinct uuids. Same shape
/// `find_spec_id_collisions` produces from a full store, so the recovery message
/// is identical whichever path detected the clash.
// trace:BUG-701 | ai:claude
pub(crate) fn group_spec_id_collisions(rows: Vec<(String, Uuid, String)>) -> Vec<SpecIdCollision> {
    use std::collections::BTreeMap;
    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for (spec_id, uuid, title) in rows {
        by_spec
            .entry(spec_id)
            .or_default()
            .push(SpecIdClaimant { uuid, title });
    }
    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            claimants.dedup_by(|a, b| a.uuid == b.uuid);
            if claimants.len() <= 1 {
                return None;
            }
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

pub(crate) fn ensure_no_spec_id_collisions(store_path: &std::path::Path) -> Result<()> {
    // BUG-701: on the hot `aida add` path this ran a full O(n) `GitBackend::load()`
    // (re-parsing every spec YAML) purely to detect duplicate spec_ids — ~2s and
    // growing with the store. Use the cache's indexed spec_id group-by instead
    // (sub-ms, size-independent). `spec_id_collisions` freshens the cache first,
    // so a collision a just-completed pre-allocation pull introduced is still
    // caught. Fall back to the authoritative full-store scan whenever the cache
    // is unavailable/unreadable (legacy centralized mode, torn cache) so the
    // duplicate guard never silently weakens. trace:BUG-701 | ai:claude
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    if let Ok(backend) = aida_core::CachedGitBackend::open(store_path, &cache_path) {
        if let Ok(rows) = backend.spec_id_collisions() {
            let collisions = group_spec_id_collisions(rows);
            if collisions.is_empty() {
                return Ok(());
            }
            anyhow::bail!(
                "{}",
                spec_id_collision_recovery_message(&collisions, store_path)
            );
        }
    }

    // Fallback: authoritative full-store scan (cache absent or unreadable).
    let backend = aida_core::GitBackend::new(store_path)?;
    let store = backend.load()?;
    let collisions = find_spec_id_collisions(&store);
    if collisions.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        spec_id_collision_recovery_message(&collisions, store_path)
    );
}

// TASK-857: opt-out for the pre-allocation remote sync. `aida add`'s remaining
// multi-second cost is the unconditional network round-trips guarding the
// cross-clone duplicate-id race. Setting AIDA_ADD_NO_REMOTE_SYNC truthy makes
// every store-syncing step on the add path purely local — no ls-remote probe,
// no pull --rebase, no push. The local collision check still runs, so the
// in-clone duplicate guard is preserved; the cross-clone guard degrades to
// "reconverge on the next online add / `aida db sync`". For fully-offline or
// solo-clone workflows where the network floor is pure latency.
// trace:TASK-857 | ai:claude
pub(crate) fn add_remote_sync_disabled() -> bool {
    std::env::var("AIDA_ADD_NO_REMOTE_SYNC")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(crate) fn pull_store_before_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        ensure_no_spec_id_collisions(store_path)?;
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — skip every network leg, keep the
    // local collision guard. trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return ensure_no_spec_id_collisions(store_path);
    }

    if git_ops::has_changes(store_path).unwrap_or(false) {
        let _ = git_ops::add(store_path, &["."]);
        let _ = git_ops::commit(
            store_path,
            "chore: sync pending changes before id allocation",
        );
    }

    let cfg = read_store_allocation_config(project_root)?;
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());

    // TASK-857 (option a): the network floor. Instead of an UNCONDITIONAL full
    // `pull --rebase` (fetch + rebase the whole orphan store) on every add,
    // first ask the remote for just the branch head SHA via a single
    // `ls-remote` (refs only, no object transfer). Three outcomes:
    //
    //   * remote head == local HEAD  → the local store already contains the
    //     remote's latest state. Allocating from it is provably safe, so SKIP
    //     the heavy pull entirely and just run the local collision check. This
    //     is the common steady-state case and the whole latency win.
    //   * remote head != local HEAD  → the remote moved; fall through to the
    //     existing pull --rebase retry loop to converge before allocating
    //     (duplicate-id guarantee preserved exactly as before).
    //   * ls-remote returns None     → remote unreachable (offline). Rather
    //     than hard-failing every add while offline, degrade gracefully: warn
    //     once and allocate from local state under the local collision guard.
    //     The push-after-allocation leg re-converges with the remote (and is
    //     itself offline-tolerant), so a sibling clone reconciles on its next
    //     online add. trace:TASK-857 | ai:claude
    match git_ops::remote_branch_head_sha(store_path, "origin", &branch) {
        Some(remote_sha) => {
            if let Ok(local_sha) = git_ops::head_sha(store_path) {
                if remote_sha == local_sha {
                    // Already current with origin — no objects to fetch.
                    return ensure_no_spec_id_collisions(store_path);
                }
            }
            // Remote diverged (or couldn't read local HEAD) → heavy pull below.
        }
        None => {
            // Offline / unreachable: file locally, reconverge later.
            eprintln!(
                "{} origin unreachable — filing offline; the new id syncs on the next online `aida add` or `aida db sync --push`.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
            return ensure_no_spec_id_collisions(store_path);
        }
    }

    for attempt in 0..cfg.retry_max {
        match git_ops::pull_rebase(store_path, "origin", &branch) {
            Ok(()) => return ensure_no_spec_id_collisions(store_path),
            Err(e) if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store allocation pull failed ({}) — retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    e,
                    attempt + 1,
                    cfg.retry_max
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "store allocation pull failed after {} attempt(s): {}\n\
                     To recover:\n  cd {} && git rebase --abort\n  aida db sync --pull",
                    cfg.retry_max,
                    e,
                    store_path.display()
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn push_store_after_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    spec_id: &str,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — the new id is committed locally and
    // will publish on the next online `aida add` / `aida db sync --push`.
    // trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return Ok(());
    }

    let cfg = read_store_allocation_config(project_root)?;
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
    for attempt in 0..cfg.retry_max {
        ensure_no_spec_id_collisions(store_path)?;
        match git_ops::push(store_path, "origin", &branch) {
            Ok(true) => return Ok(()),
            Ok(false) if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store push rejected after allocating {} — pulling/retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    spec_id,
                    attempt + 1,
                    cfg.retry_max
                );
                git_ops::pull_rebase(store_path, "origin", &branch)?;
                ensure_no_spec_id_collisions(store_path)?;
            }
            Ok(false) => {
                anyhow::bail!(
                    "store push rejected after allocating {} and {} attempt(s) were exhausted.\n\
                     Your local store still has the new spec commit; do not re-file blindly.\n\
                     To recover:\n  aida db sync --pull\n  aida db sync --push",
                    spec_id,
                    cfg.retry_max
                );
            }
            // TASK-857: a push that ERRORS (vs is rejected) is the offline /
            // network-down case. A spec is already filed and committed locally;
            // the duplicate-id guarantee only requires the publish to land
            // *eventually* (a sibling clone reconciles when it next pulls). So
            // DEFER rather than fail the whole `aida add`: warn and return Ok.
            // The CAS-reject retry above is untouched — that's the real
            // serialize-the-winners guard and only fires when online.
            // trace:TASK-857 | ai:claude
            Err(_) => {
                eprintln!(
                    "{} {} filed locally but not yet published (origin unreachable). \
                     It syncs on the next online `aida add` or `aida db sync --push`.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec_id
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

/// STORY-441: read `[archive] auto_after_days` from `.aida/config.toml`.
/// Returns `Some(days)` when the user has opted in, `None` when the key is
/// absent (auto-sweep stays off). Clamps below 7 to 7 with a stderr warning
/// — auto-archiving a freshly-shipped spec defeats the whole point.
/// Missing config file or unparseable TOML returns `None` silently (the
/// optional auto-sweep is a soft feature, not load-bearing).
/// trace:STORY-441 | ai:claude
pub(crate) fn read_archive_auto_after_days(project_root: &std::path::Path) -> Option<u64> {
    let path = config_path_for_project(project_root);
    let body = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("archive")
        .and_then(|t| t.get("auto_after_days"))
        .and_then(|v| v.as_integer())?;
    let days = u64::try_from(raw).ok()?;
    if days < 7 {
        eprintln!(
            "{} [archive] auto_after_days = {days} clamped to minimum of 7 days \
             (archiving sooner hides freshly-shipped specs)",
            "Warning:".yellow().bold()
        );
        Some(7)
    } else {
        Some(days)
    }
}

/// BUG-783: read `[list] show_hidden_hints` from `.aida/config.toml`. Governs
/// whether the default `aida list` view footers the archived / deferred
/// view-tier counts ("(138 archived hidden — pass --all or --archived to see
/// them)"). Default is `false` — on an explicit open-work request those lines
/// are pure noise on every single invocation, and the tiers stay one flag away
/// (`--archived` / `--deferred` / `--all`). Set the key to `true` to bring the
/// nudges back. Missing config file / key / unparseable TOML → the default.
// trace:BUG-783 | ai:claude
pub(crate) fn read_list_show_hidden_hints(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return false;
    };
    value
        .get("list")
        .and_then(|t| t.get("show_hidden_hints"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// STORY-717: read `[focus] out_of_scope` from `.aida/config.toml`. Governs the
/// focus-scope drift guard at work-start (off/warn/block). Missing config /
/// key / unparseable TOML falls back to the default (`warn`). The optional
/// `none`/`silent` and `refuse`/`hard` aliases are accepted by the parser.
// trace:STORY-717 | ai:claude
pub(crate) fn read_focus_out_of_scope_policy(
    project_root: &std::path::Path,
) -> focus::OutOfScopePolicy {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return focus::OutOfScopePolicy::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return focus::OutOfScopePolicy::default();
    };
    value
        .get("focus")
        .and_then(|t| t.get("out_of_scope"))
        .and_then(|v| v.as_str())
        .map(focus::parse_out_of_scope_policy)
        .unwrap_or_default()
}

/// STORY-717: derive a cheap "did you mean" focus suggestion for an out-of-scope
/// target — its `parent:` tag when present (the AIDA convention is
/// `parent:EPIC-NN`). Returns `None` when no cheap hint exists, in which case
/// the nudge just names the mismatch (per the spec: "if cheap; else just name
/// the mismatch").
// trace:STORY-717 | ai:claude
pub(crate) fn suggested_focus_for(target: &aida_core::Requirement) -> Option<String> {
    target
        .tags
        .iter()
        .find_map(|t| t.strip_prefix("parent:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// STORY-717: the focus-scope drift guard. At a work-START moment, if the
/// current worktree has a focus set (STORY-706) and `target` is OUTSIDE that
/// focus's transitive subtree, apply the configured `[focus] out_of_scope`
/// policy. `force` ALWAYS overrides. Membership reuses the cache's
/// `descendant_ids` closure (TASK-955) — the same subtree the focus read-scope
/// uses — rather than re-walking the hierarchy. An unresolvable focus spec
/// skips the guard rather than blocking real work. The subtree is read with a
/// strict cache refresh, and a cache or refresh error fails the start (fail
/// closed) instead of judging scope on a stale graph; a `Block` policy also
/// returns `Err` on a genuine out-of-scope start.
// trace:STORY-717 | ai:claude
// trace:BUG-1670 | ai:claude
pub(crate) fn focus_scope_guard(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    target: &aida_core::Requirement,
    force: bool,
) -> Result<()> {
    // No focus set in this worktree → no check.
    let Some(focus_ref) = focus::resolve_focus(project_root) else {
        return Ok(());
    };
    let policy = read_focus_out_of_scope_policy(project_root);
    // Cheap exit before touching the cache when the guard is muted anyway.
    if policy == focus::OutOfScopePolicy::Off {
        return Ok(());
    }
    // The focus label must still resolve to a real spec; if it no longer does,
    // don't block work (the read-scope path warns about a dangling focus).
    let Some(focus_req) = backend.get_requirement_by_spec_id(&focus_ref)? else {
        return Ok(());
    };
    // REUSE the TASK-955 subtree closure (includes the root) for membership.
    // Strict: this gates a write, so a child added or re-parented by another
    // writer since the last cache refresh must be classified on the current
    // graph, not on a snapshot served while the cache is being written.
    // trace:BUG-1670 | ai:claude
    let subtree = backend.descendant_ids_strict(&focus_req.id)?;
    let in_scope = focus::is_in_focus_scope(&target.id, &subtree);
    match focus::decide_focus_action(policy, in_scope, force) {
        focus::FocusGuardAction::Proceed => Ok(()),
        focus::FocusGuardAction::Warn => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            eprintln!(
                "{} {} (--force to silence)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                core
            );
            Ok(())
        }
        focus::FocusGuardAction::Block => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            anyhow::bail!("{} (pass --force to override)", core)
        }
    }
}

/// STORY-717: focus guard for callers that don't already hold a backend
/// (`queue work`, `agent new --spec`). Resolves `spec` against a cache-backed
/// backend, then defers to [`focus_scope_guard`]. Skips early (no backend work)
/// when no focus is set; best-effort on store/spec resolution. A `Block` policy
/// still propagates the `Err`.
// trace:STORY-717 | ai:claude
pub(crate) fn focus_scope_guard_for_spec(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
) -> Result<()> {
    // Fast path: no focus → skip the (potentially cache-rebuilding) backend open.
    if focus::resolve_focus(project_root).is_none() {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    focus_scope_guard(project_root, &backend, &target, force)
}

// BUG-653: `aida agent new --spec <epic>` used to dead-end. The launch path
// routes through `session_start` -> `preflight_spec_status`, which refuses a
// Draft spec with "transition it to Approved first". But an epic's status is a
// read-only rollup of its children (BUG-626) -- `aida edit <epic> --status
// approved` is rejected -- so that advice sends the operator to a command that
// will refuse. And you don't implement an epic directly anyway; work happens on
// its children. Detect the epic up front and give an epic-appropriate message
// instead of routing into the implement-readiness gate. Non-epic specs (incl.
// genuinely-Draft tasks/stories) fall through untouched, so their correct
// "transition to Approved" refusal is preserved. trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_guard(project_root: &std::path::Path, spec: &str) -> Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    match epic_agent_new_refusal(&target.req_type, &target.display_id()) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// BUG-653: pure decision half of [`epic_agent_new_guard`] — returns the
/// epic-appropriate refusal message when the resolved `--spec` is an epic, or
/// `None` for every other type (so non-epic specs fall through to the existing
/// readiness gate untouched). Pure so the message + the type gate are
/// unit-testable without a store fixture.
// trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_refusal(
    req_type: &RequirementType,
    display_id: &str,
) -> Option<String> {
    if !matches!(req_type, RequirementType::Epic) {
        return None;
    }
    Some(format!(
        "{display_id}'s status is a read-only rollup of its children -- you don't implement an \
         epic directly.\n  \
         Pick an approved child and launch on that:\n    \
         aida list --parent {display_id} --status approved\n  \
         Or open a design/advisor session scoped to the epic in a focused worktree (no --spec):\n    \
         aida worktree enter {display_id}   # then a plain `aida agent new ...` in that worktree"
    ))
}

// BUG-1701: route a drain-mode spec away from the interactive lane. `aida agent
// new --spec <ID>` spawns a vendor TUI and BLOCKS until that TUI exits; a TUI
// does not exit when its turn ends, so an orchestrator that dispatched a
// drain-groomed spec here stranded both the seat and itself. The one-shot lane
// already exists -- `aida do <SPEC>` routes by groomed execution mode, and for
// drain that is `aida queue work <SPEC> --auto-complete`, which runs headless
// and returns an exit status. Nothing steered callers there, so this guard does.
// Sits beside the epic and focus-scope guards: AFTER the dry-preview returns (a
// preview is never blocked) and BEFORE any worktree/lease/status side effect.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_guard(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
    holds_caller: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    // Fail OPEN on a lookup failure as well as a miss. `?` here would abort the
    // launch when the store is unreadable, which is the opposite of this guard's
    // contract: it exists to REDIRECT a drain-groomed spec, never to become a new
    // way for every launch in the fleet to fail. An independent review of PR #2249
    // caught the `?`. trace:BUG-1701 | ai:claude
    let Ok(Some(target)) = backend.get_requirement_by_spec_id(spec) else {
        return Ok(());
    };
    match drain_mode_agent_new_refusal(target.execution_mode, &target.display_id(), holds_caller) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// Pure decision half of [`drain_mode_agent_new_guard`]: the refusal message for a
/// spec groomed `execution_mode = drain`, or `None` for every other mode (and for
/// an ungroomed spec, which must keep working -- the fail-open default).
///
/// Only `drain` is refused. The other modes legitimately want a human-attended
/// seat, which is exactly what this lane provides; refusing them would take away
/// the only lane they have.
///
/// `holds_caller` distinguishes the foreground launch (which blocks the caller on
/// the child TUI -- the BUG-1701 stall) from `--bg` (which detaches, so the lane
/// is still wrong but for the pipeline reason alone). Stating only what is true of
/// the lane actually being refused keeps the message trustworthy.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_refusal(
    mode: Option<aida_core::ExecutionMode>,
    display_id: &str,
    holds_caller: bool,
) -> Option<String> {
    if mode != Some(aida_core::ExecutionMode::Drain) {
        return None;
    }
    // The first line must stand alone: in agent mode only the first line becomes
    // the `error:` summary. And `aida do` must be the FIRST backtick-quoted
    // `aida ...` command in the whole message, because that is what agent mode
    // lifts into `help:` -- an unbackticked recommendation silently loses to a
    // backticked one further down. trace:BUG-1701 | ai:claude
    let lane = if holds_caller {
        "which spawns an interactive seat and holds your caller until that seat's TUI exits -- \
         and a TUI does not exit when its turn ends"
    } else {
        "which runs neither CI, the reviewer phase, nor the merge for you"
    };
    Some(format!(
        "{display_id} is groomed `execution_mode = drain` -- use `aida do {display_id}`, not this \
         lane, {lane}.\n  \
         `aida do` routes by the groomed mode; for drain it runs \
         `aida queue work {display_id} --auto-complete`, which is headless, runs to completion, \
         and returns an exit status.\n  \
         To sit in this spec interactively anyway (to debug it by hand), pass --force."
    ))
}

/// STORY-564: read `[zen] auto_exit` from `.aida/config.toml`. Returns the
/// operator's persistent preference for whether a clean standalone-`--zen`
/// finish auto-exits (`true`, the default) or always pauses (`false`).
/// Missing config / key / unparseable TOML → `true` (the new default
/// behavior). The per-invocation `--pause-always` flag (→ `AIDA_ZEN_PAUSE_ALWAYS`)
/// overrides this toward pausing; this is the standing preference when the
/// flag isn't passed. trace:STORY-564 | ai:claude
pub(crate) fn read_zen_auto_exit(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return true;
    };
    value
        .get("zen")
        .and_then(|t| t.get("auto_exit"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// STORY-564: should the standalone-`--zen` finish *pause* (vs auto-exit)
/// because the operator asked it to? True when `--pause-always` was passed
/// (propagated as `AIDA_ZEN_PAUSE_ALWAYS=1`) OR `[zen] auto_exit = false` is
/// configured. Either is the operator electing to drive grab-next by hand.
/// trace:STORY-564 | ai:claude
pub(crate) fn zen_pause_always_in_force(project_root: &std::path::Path) -> bool {
    std::env::var(zen::ZEN_PAUSE_ALWAYS_ENV).as_deref() == Ok("1")
        || !read_zen_auto_exit(project_root)
}

/// STORY-569: read `[zen] review_brief_agent` from `.aida/config.toml` — the
/// mailbox target for the clean-finish build→review handoff. Missing config /
/// key / unparseable TOML → the default `advisor`. An explicitly empty string
/// disables the handoff. trace:STORY-569 | ai:claude
pub(crate) fn read_zen_review_brief_agent(project_root: &std::path::Path) -> Option<String> {
    let default = || Some("advisor".to_string());
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return default();
    };
    match value
        .get("zen")
        .and_then(|t| t.get("review_brief_agent"))
        .and_then(|v| v.as_str())
    {
        None => default(),
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

/// STORY-441: opt-out env var matching the `AIDA_AUTO_BUMP` shape.
pub(crate) fn auto_archive_enabled() -> bool {
    match std::env::var("AIDA_AUTO_ARCHIVE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// STORY-441: run the same `--older-than N days --status completed,rejected`
/// sweep that `aida archive --older-than` exposes, but as a side-effect of
/// `aida pull` once the auto-bump has finished. Off by default — gated on
/// `[archive] auto_after_days` being set. Best-effort: any error is printed
/// as a warning, never fails the pull. trace:STORY-441 | ai:claude
pub(crate) fn maybe_auto_archive_sweep(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    quiet: bool,
) {
    if !auto_archive_enabled() {
        return;
    }
    let Some(days) = read_archive_auto_after_days(project_root) else {
        return;
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
    let statuses = ["Completed", "Rejected"];
    let mut to_archive: Vec<aida_core::Requirement> = Vec::new();
    // cache-tolerant-read: selection only — each candidate's YAML object is
    // re-read and its eligibility re-decided inside the store write lock
    // (`bulk_update_atomically`) before anything is written.
    // trace:BUG-1671 | ai:claude
    for s in &statuses {
        let filter = aida_core::ListFilter {
            status: Some((*s).to_string()),
            archive: aida_core::ArchiveFilter::NonArchivedOnly,
            ..Default::default()
        };
        let Ok(rows) = backend.list_summaries(&filter) else {
            continue;
        };
        for row in rows {
            let stale = chrono::DateTime::parse_from_rfc3339(&row.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc) < cutoff)
                .unwrap_or(false);
            if !stale {
                continue;
            }
            // BUG-1664: `row` may come from a stale cache snapshot. The
            // object read is authoritative, so re-check status and age on it,
            // not only the archive flag.
            // trace:BUG-1664 | ai:claude
            if let Ok(Some(req)) = backend.get_requirement(&row.id) {
                if archive_cmd::archive_sweep_still_eligible(&req, &statuses, cutoff) {
                    to_archive.push(req);
                }
            }
        }
    }
    if to_archive.is_empty() {
        return;
    }
    let now = chrono::Utc::now();
    crate::sweep_test_hook::fire(backend.path());
    // BUG-1671: re-decide each candidate on the object read INSIDE the store
    // write lock, so a spec reopened between the pass above and this write is
    // skipped instead of being reverted by the whole-object write. Collapses
    // the old commit-per-spec loop into one commit as well.
    // trace:BUG-1671 | ai:claude
    let count = backend
        .bulk_update_atomically(&to_archive, "chore(archive)", |req| {
            if !archive_cmd::archive_sweep_still_eligible(req, &statuses, cutoff) {
                return false;
            }
            req.archived = true;
            req.archived_at = Some(now);
            req.modified_at = now;
            true
        })
        .map(|report| report.written.len())
        .unwrap_or(0);
    if !quiet && count > 0 {
        println!(
            "  {} {count} spec(s) older than {days}d (auto-sweep, opt out via AIDA_AUTO_ARCHIVE=0)",
            "auto-archived:".cyan()
        );
    }
}

#[cfg(test)]
#[path = "tests/story_760_store_mirror_tests.rs"]
mod story_760_store_mirror_tests;

#[cfg(test)]
#[path = "tests/story_441_archive_config_tests.rs"]
mod story_441_archive_config_tests;

#[cfg(test)]
#[path = "tests/story_284_store_sync_tests.rs"]
mod story_284_store_sync_tests;

/// Ensure git/claude hook files are executable. Called from scaffolder
/// write paths so freshly-scaffolded hooks don't trigger git's "hook was
/// ignored because not executable" warning.
/// trace:BUG-21 | ai:claude
pub(crate) fn ensure_executable_if_hook(rel_path: &std::path::Path, full_path: &std::path::Path) {
    let s = rel_path.to_string_lossy();
    let is_hook =
        s.starts_with(".git/hooks/") || s.starts_with(".claude/hooks/") || s.ends_with(".sh");
    if !is_hook {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(full_path) {
            let mut p = meta.permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(full_path, p);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = full_path; // no-op on non-unix
    }
}

/// Resolve the description from one of three sources: inline `--description`,
/// `--description-from-file PATH`, or `--description-stdin`. The CLI struct
/// already enforces mutual exclusion via `conflicts_with_all`; here we just
/// fetch the content from the right source. Returns `Ok(None)` when no
/// source is set (caller falls back to empty / interactive prompt).
/// trace:BUG-17 | ai:claude
pub(crate) fn resolve_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    if let Some(d) = description {
        return Ok(Some(d.clone()));
    }
    if let Some(path) = description_from_file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read description from {}", path.display()))?;
        return Ok(Some(body));
    }
    if description_stdin {
        use std::io::Read;
        let mut body = String::new();
        std::io::stdin()
            .read_to_string(&mut body)
            .context("failed to read description from stdin")?;
        return Ok(Some(body));
    }
    Ok(None)
}

// trace:BUG-1234 | ai:codex
/// Resolve an edit description and refuse an empty file/stdin body so a
/// failed producer cannot erase an existing requirement description.
/// Inline `--description ""` remains the explicit way to clear the field.
pub(crate) fn resolve_edit_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    let resolved = resolve_description(description, description_from_file, description_stdin)?;
    if (description_from_file.is_some() || description_stdin)
        && resolved.as_deref().is_some_and(str::is_empty)
    {
        anyhow::bail!("description input is empty; existing description left unchanged");
    }
    Ok(resolved)
}

pub(crate) fn parse_requirement_type(s: &str) -> Result<RequirementType> {
    match s.to_lowercase().as_str() {
        "functional" | "fr" => Ok(RequirementType::Functional),
        "non-functional" | "nonfunctional" | "nfr" => Ok(RequirementType::NonFunctional),
        "system" | "sr" => Ok(RequirementType::System),
        "user" | "ur" => Ok(RequirementType::User),
        // Workflow type: a proposed change. trace:TASK-716 | ai:claude
        "change-request" | "changerequest" | "change" | "cr" => Ok(RequirementType::ChangeRequest),
        "bug" => Ok(RequirementType::Bug),
        "epic" => Ok(RequirementType::Epic),
        "story" => Ok(RequirementType::Story),
        "task" => Ok(RequirementType::Task),
        "spike" => Ok(RequirementType::Spike),
        "sprint" => Ok(RequirementType::Sprint),
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        // Docs-layer types (FR-1-074). Aliases match the type prefix used
        // in agreed-id format (`PRIN`, `VIS`, `CON`, `ADR`, `TERM`).
        // trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
        // trace:STORY-104 | ai:claude
        "doc" | "documentation" => Ok(RequirementType::Doc),
        "faq" => Ok(RequirementType::Faq),
        _ => anyhow::bail!("Unknown requirement type: {}", s),
    }
}

#[cfg(test)]
#[path = "tests/parse_requirement_type_tests.rs"]
mod parse_requirement_type_tests;

/// Initialize distributed mode using an orphan branch + worktree.
/// This is the default for single-repo projects.
/// Store lives at .aida-store/ (worktree of orphan branch 'aida-store').
/// BUG-446 / TASK-686: immediate-child directories of `cwd` that are their OWN
/// project — a nested git repo (a `.git` directory, or a `.git` file =
/// gitlink/submodule worktree) OR an AIDA project (a child `.aida/` dir) — and
/// are NOT registered submodules. A non-empty result means `cwd` is a
/// workspace-/parent-of-projects, not a single project.
///
/// We deliberately test for a child's own `.git`/`.aida` entry rather than
/// calling `git_ops::is_git_repo` (which shells out to `git rev-parse`): from
/// inside `cwd` — itself a git repo by the time this runs — `rev-parse`
/// resolves to `cwd`'s git-dir for EVERY subdirectory, so it would flag plain
/// subdirs too. Dotted children (`.git`, `.aida-store`, …) and
/// `.gitmodules`-declared submodules are intentional and excluded.
///
/// TASK-686 added the child-`.aida/` arm: a parent of AIDA projects that aren't
/// all plain git repos (the `~/ai/` case — ~80 children) would otherwise slip
/// the guard and leave a scaffold every child inherits via ancestor CLAUDE.md.
/// trace:BUG-446 trace:TASK-686 | ai:claude
pub(crate) fn unmanaged_nested_projects(cwd: &std::path::Path) -> Vec<String> {
    let submodule_paths = gitmodule_child_paths(cwd);
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || submodule_paths.contains(&name) {
            continue;
        }
        if path.join(".git").exists() || path.join(".aida").is_dir() {
            found.push(name);
        }
    }
    found.sort();
    found
}

/// First path component of each `path = …` entry in the top-level `.gitmodules`
/// — enough to exclude an immediate-child submodule directory from the
/// workspace-of-projects guard. trace:BUG-446 | ai:claude
pub(crate) fn gitmodule_child_paths(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut paths = std::collections::HashSet::new();
    let Ok(content) = std::fs::read_to_string(cwd.join(".gitmodules")) else {
        return paths;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("path") {
            if let Some(value) = rest.split('=').nth(1) {
                if let Some(first) = value.trim().split('/').next() {
                    if !first.is_empty() {
                        paths.insert(first.to_string());
                    }
                }
            }
        }
    }
    paths
}

/// What to do when `aida init` runs in a directory that isn't a git repo yet.
/// trace:STORY-552 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GitInitDecision {
    /// Run `git init` without asking (explicit `--git-init`).
    Yes,
    /// Offer interactively (TTY, no flag).
    Prompt,
    /// Keep the safe bail+recipe (non-interactive, no flag — don't
    /// silently git-init in scripts).
    Bail,
}

/// Decide how to handle a non-git folder at the front of init. `--git-init`
/// always wins; otherwise prompt at a TTY and bail elsewhere. Pure so it can be
/// unit-tested without a terminal. trace:STORY-552 | ai:claude
pub(crate) fn git_init_decision(git_init_flag: bool, at_tty: bool) -> GitInitDecision {
    if git_init_flag {
        GitInitDecision::Yes
    } else if at_tty {
        GitInitDecision::Prompt
    } else {
        GitInitDecision::Bail
    }
}

/// Get a git config value from the global config.
pub(crate) fn _git_config_get_global(key: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", key])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("git config {} not set", key)
    }
}

pub(crate) fn add_requirement_interactive(storage: &Storage) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Prompt user for requirement details
    let requirement = crate::prompts::prompt_new_requirement(&mut store)?;
    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );
    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    Ok(())
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_requirement_cli(
    storage: &Storage,
    title: &Option<String>,
    description: &Option<String>,
    status_str: &Option<String>,
    priority_str: &Option<String>,
    type_str: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags_str: &Option<String>,
    prefix: &Option<String>,
    parent: &Option<String>,
    force_parent: bool,
) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Check required fields
    let title = match title {
        Some(t) => t.clone(),
        None => anyhow::bail!("Title is required. Use --title to specify a title."),
    };

    let description = match description {
        Some(d) => d.clone(),
        None => String::new(),
    };

    // Validate parent exists if specified
    let parent_uuid = if let Some(parent_id) = parent {
        let uuid = parse_requirement_id(parent_id, &store)?;
        // BUG-64: terminal-status guard. Refuse to file a new child
        // under a Completed/Rejected parent unless --force-parent.
        // trace:BUG-64 | ai:claude
        if !force_parent {
            let pr = store
                .get_requirement_by_id(&uuid)
                .ok_or_else(|| anyhow::anyhow!("parent {} not found", parent_id))?;
            if is_terminal_status(&pr.status) {
                anyhow::bail!(
                    "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                     Pass `--force-parent` to override.",
                    pr.spec_id.as_deref().unwrap_or(parent_id),
                    pr.status,
                );
            }
        }
        Some(uuid)
    } else {
        None
    };

    // Create a requirement with basic data
    let mut requirement = Requirement::new(title, description);

    // Set optional fields
    if let Some(status) = status_str {
        requirement.status = parse_status(status)?;
    }

    // TASK-647 (ADR-3): advisor-gate the production of approved+ specs. A
    // non-advisor, non-TTY caller (headless agent, drain/auto capture) can
    // only file `draft`; a requested approved+ status is downgraded with a
    // one-line triage notice. Advisor role or an interactive human is
    // unaffected. trace:TASK-647 | ai:claude
    let intake_downgraded_from =
        if status_requires_advisor_authority(&requirement.status) && !has_advisor_authority() {
            let from = requirement.status;
            requirement.status = RequirementStatus::Draft;
            Some(from)
        } else {
            // BUG-498: an approved+ status that was NOT downgraded means the
            // caller exercised advisor authority — if that authority came from
            // an `AIDA_SESSION_ROLE=advisor` prefix rather than a seated role,
            // nudge them to seat it. trace:BUG-498 | ai:claude
            if status_requires_advisor_authority(&requirement.status) {
                maybe_hint_advisor_seat();
            }
            None
        };

    if let Some(priority) = priority_str {
        requirement.priority = parse_priority(priority)?;
    }

    if let Some(req_type) = type_str {
        requirement.req_type = parse_type(req_type)?;
    }

    // Set owner: use explicit value, AIDA_AUTHOR env var, or system username
    requirement.owner = owner.clone().unwrap_or_else(get_default_author);

    if let Some(feature_val) = feature {
        requirement.feature = feature_val.clone();
    }

    if let Some(tags) = tags_str {
        let tag_set: HashSet<String> = tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        requirement.tags = tag_set;
    }

    // Set prefix override if specified
    if let Some(prefix_val) = prefix {
        requirement
            .set_prefix_override(prefix_val)
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    // Add parent relationship if specified.
    //
    // BUG-58: previously stored `(child, Parent, parent)` with
    // bidirectional=false, which is doubly broken:
    //   1. Relationship type is "I am X to target", so Parent on the
    //      child says "I AM the parent of <parent>" — backwards.
    //   2. Without bidirectional=true, the parent never gets the inverse
    //      Parent edge pointing at the child, so `rel list <parent>`
    //      didn't show its new child.
    // Fix: store `Child` on the source (child) pointing at the parent,
    // bidirectional so the parent gets the matching `Parent` edge.
    // Matches the convention already used by the git-canonical add
    // path (FR-215). trace:BUG-58 | ai:claude
    if let Some(parent_id) = parent_uuid {
        store
            .add_relationship(&id, RelationshipType::Child, &parent_id, true)
            .map_err(|e| anyhow::anyhow!("Failed to add parent relationship: {}", e))?;
    }

    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    // TASK-647 (ADR-3): tell the caller their requested status was held for
    // triage. Quiet and non-fatal — the spec is filed, just as draft.
    if let Some(from) = intake_downgraded_from {
        eprintln!(
            "{} filed as {} (requested {} needs advisor authority) — queued for advisor triage.",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            "draft".yellow(),
            from.to_string().to_lowercase().dimmed()
        );
    }

    // Show parent relationship if created
    if let Some(parent_id_str) = parent {
        println!("Parent: {}", parent_id_str.cyan());
    }

    Ok(())
}

/// TASK-527: match one `--tags` filter token against a spec's tag set, with
/// prefix-glob support. A trailing `*` (`aida:queue:*`) matches any tag starting
/// with the literal prefix, plus the bare prefix without its trailing `:` (so
/// `aida:queue:*` also matches an exact `aida:queue` tag). Without `*` it's the
/// existing exact membership test. Comma-OR composition happens at the caller.
/// trace:TASK-527 | ai:claude
pub(crate) fn tag_filter_matches(filter: &str, tags: &std::collections::HashSet<String>) -> bool {
    if let Some(prefix) = filter.strip_suffix('*') {
        let bare = prefix.strip_suffix(':').unwrap_or(prefix);
        tags.iter().any(|t| t.starts_with(prefix) || t == bare)
    } else {
        tags.contains(filter)
    }
}

#[cfg(test)]
#[path = "tests/task_527_tag_glob_tests.rs"]
mod task_527_tag_glob_tests;

pub(crate) fn list_requirements(
    storage: &Storage,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
) -> Result<()> {
    // Load requirements
    let store = storage.load_for_read()?;
    let mut requirements = store.requirements.clone();

    // Apply filters if provided
    if let Some(status_str) = status {
        let status_filter = parse_list_status_filter(status_str)?;
        requirements.retain(|r| requirement_matches_status_filter(&store, r, &status_filter));
    }

    if let Some(priority_str) = priority {
        let priority_filter = parse_priority(priority_str)?;
        requirements.retain(|r| r.priority == priority_filter);
    }

    if let Some(type_str) = req_type {
        let type_filter = parse_type(type_str)?;
        requirements.retain(|r| r.req_type == type_filter);
    }

    if let Some(feature_str) = feature {
        requirements.retain(|r| r.feature == *feature_str);
    }

    if let Some(tags_str) = tags {
        let tag_filters: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        requirements.retain(|r| tag_filters.iter().any(|f| tag_filter_matches(f, &r.tags)));
    }

    // Display the requirements
    if requirements.is_empty() {
        println!("{}", "No requirements found.".yellow());
        return Ok(());
    }

    const STATUS_COLUMN_WIDTH: usize = 22;
    println!(
        "{:<10} | {:<36} | {:<30} | {:<STATUS_COLUMN_WIDTH$} | {:<10} | {:<15}",
        "SPEC-ID", "UUID", "Title", "Status", "Priority", "Feature"
    );
    println!("{}", "-".repeat(132));

    for req in requirements {
        let display_status = effective_display_status(&store, &req);
        let status_str =
            list_requirement_status_cell(&store, &req, &display_status, STATUS_COLUMN_WIDTH);
        let priority_str = match req.priority {
            RequirementPriority::High => "High".red(),
            RequirementPriority::Medium => "Medium".yellow(),
            RequirementPriority::Low => "Low".green(),
        };

        let spec_id_display = req.spec_id.as_deref().unwrap_or("-");

        println!(
            "{:<10} | {:<36} | {:<30} | {} | {:<10} | {:<15}",
            spec_id_display,
            req.id.to_string(),
            req.title,
            status_str,
            priority_str,
            req.feature
        );
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListStatusFilter {
    Stored(RequirementStatus),
    Shelved,
    NeedsDecision,
    // trace:BUG-1687 | ai:claude — the deferred VIEW axis as a `--status`
    // token. Kept in step with the git-backend path on purpose: BUG-1771's
    // lesson was that a token accepted on one listing path and refused on the
    // other is worse than one refused everywhere.
    Deferred,
}

// trace:STORY-1023 | ai:codex
// trace:BUG-1771 | ai:claude — the token set now comes from the shared
// `status_display::LENS_FILTER_TOKENS` table the git-backend list path and its
// "Unknown status filter" refusal also read, instead of a second hand-written
// `match`. The two paths had already drifted once: these tokens worked here and
// errored on the shipped (distributed-mode) CLI.
pub(crate) fn parse_list_status_filter(status_str: &str) -> Result<ListStatusFilter> {
    if let Some(key) = status_display::lens_filter_key(status_str) {
        return match key {
            "Shelved" => Ok(ListStatusFilter::Shelved),
            "NeedsDecision" => Ok(ListStatusFilter::NeedsDecision),
            // A token in the shared table with no arm here is a wiring gap, not
            // a stored status — say so rather than letting `parse_status` reject
            // it with a message that denies the token exists.
            key => anyhow::bail!(
                "status lens '{status_str}' (key '{key}') is accepted by \
                 `aida list --status` but not wired into this listing path"
            ),
        };
    }
    // trace:BUG-1687 | ai:claude — same single-table discipline for the view
    // axes: the token set lives in `VIEW_FILTER_TOKENS`, not in a second
    // hand-written match that can drift from it.
    if let Some(axis) = status_display::view_filter_axis(status_str) {
        return match axis {
            status_display::ViewFilterAxis::Deferred => Ok(ListStatusFilter::Deferred),
        };
    }
    parse_status(status_str).map(ListStatusFilter::Stored)
}

pub(crate) fn requirement_matches_status_filter(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    status_filter: &ListStatusFilter,
) -> bool {
    // trace:STORY-1023 | ai:codex
    let display_status = effective_display_status(store, req);
    match status_filter {
        ListStatusFilter::Stored(status) => display_status == *status,
        ListStatusFilter::Shelved => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::Shelved { .. })
        ),
        ListStatusFilter::NeedsDecision => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::NeedsDecision { .. })
        ),
        // trace:BUG-1687 | ai:claude — the defer axis, read through the same
        // flag-OR-legacy-tag predicate the cache query uses, so the two listing
        // paths return the same set.
        ListStatusFilter::Deferred => status_display::is_deferred(req),
    }
}

pub(crate) fn list_requirement_status_cell(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
    width: usize,
) -> String {
    // trace:STORY-1023 | ai:codex
    let (label, palette_key) = if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(store, req, display_status)
            .map(|lens| {
                let label = lens.label();
                let key = lens.palette_key();
                (label, key)
            })
            .unwrap_or_else(|| ("Needs Decision".to_string(), "NeedsDecision"))
    } else {
        let label = match display_status {
            RequirementStatus::Draft => "Draft",
            RequirementStatus::Approved => "Approved",
            RequirementStatus::Planned => "Planned",
            RequirementStatus::InProgress => "In Progress",
            RequirementStatus::Done => "Done",
            RequirementStatus::Completed => "Completed",
            RequirementStatus::Rejected => "Rejected",
            RequirementStatus::Superseded => "Superseded",
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        };
        (label.to_string(), label)
    };
    let padded = format!("{label:<width$}");
    status_display::paint_status(&padded, palette_key).to_string()
}

pub(crate) fn needs_attention_badge_for_lens(lens: &status_display::NeedsAttentionLens) -> String {
    let label = lens.label();
    let key = lens.palette_key();
    format!(
        "{} {}",
        status_display::status_glyph(key),
        status_display::paint_status(&label, key)
    )
}

pub(crate) fn effective_needs_attention_lens(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<status_display::NeedsAttentionLens> {
    effective_needs_attention_lens_with_source(store, req, display_status).map(|(_, lens)| lens)
}

pub(crate) fn effective_needs_attention_lens_with_source<'a>(
    store: &'a aida_core::RequirementsStore,
    req: &'a aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<(
    &'a aida_core::models::Requirement,
    status_display::NeedsAttentionLens,
)> {
    if !matches!(display_status, RequirementStatus::NeedsAttention) {
        return None;
    }
    if let Some(lens) = status_display::needs_attention_lens(req) {
        return Some((req, lens));
    }
    if req.req_type == RequirementType::Epic {
        let mut decision_lens = None;
        // trace:STORY-1023 | ai:codex
        for child_id in aida_core::graph_walk::subtree_ids(store, req.id, None).nodes {
            let Some(child) = store.get_requirement_by_id(&child_id) else {
                continue;
            };
            let child_display_status = effective_display_status(store, child);
            match effective_needs_attention_lens_with_source(store, child, &child_display_status) {
                Some((source, status_display::NeedsAttentionLens::Shelved { cause })) => {
                    return Some((
                        source,
                        status_display::NeedsAttentionLens::Shelved { cause },
                    ));
                }
                Some((source, status_display::NeedsAttentionLens::NeedsDecision { reason })) => {
                    decision_lens.get_or_insert((
                        source,
                        status_display::NeedsAttentionLens::NeedsDecision { reason },
                    ));
                }
                None => {}
            }
        }
        if decision_lens.is_some() {
            return decision_lens;
        }
    }
    Some((
        req,
        status_display::NeedsAttentionLens::NeedsDecision { reason: None },
    ))
}

#[cfg(test)]
mod story_1023_list_render_tests {
    use super::*;

    #[test]
    fn list_status_cell_splits_shelved_from_needs_decision() {
        let mut shelved =
            aida_core::models::Requirement::new("stale base".to_string(), String::new());
        shelved.status = RequirementStatus::NeedsAttention;
        shelved.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });

        let mut decision =
            aida_core::models::Requirement::new("design fork".to_string(), String::new());
        decision.status = RequirementStatus::NeedsAttention;
        decision.attention_reason = Some(aida_core::AttentionReason {
            category: aida_core::PuntCategory::DesignFork,
            detail: "choose the public API shape".to_string(),
            lean: None,
            raised_by: Some("codex".to_string()),
            raised_at: chrono::Utc::now(),
        });

        let store = aida_core::RequirementsStore::new();
        colored::control::set_override(false);
        let shelved_cell = list_requirement_status_cell(&store, &shelved, &shelved.status, 22);
        let decision_cell = list_requirement_status_cell(&store, &decision, &decision.status, 22);
        colored::control::unset_override();

        assert!(
            shelved_cell.contains("Shelved (stale-base)"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            !shelved_cell.contains("Needs Attention"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            decision_cell.contains("Needs Decision (design-fork)"),
            "cell: {decision_cell:?}"
        );
    }

    #[test]
    fn status_filter_uses_effective_display_status_and_parked_lens() {
        let mut store = aida_core::RequirementsStore::new();

        let mut epic = aida_core::models::Requirement::new("Epic".to_string(), String::new());
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Approved;
        let mut child =
            aida_core::models::Requirement::new("stale child".to_string(), String::new());
        child.status = RequirementStatus::NeedsAttention;
        child.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });
        child.relationships.push(aida_core::models::Relationship {
            rel_type: aida_core::models::RelationshipType::Parent,
            target_id: epic.id,
            created_at: None,
            created_by: None,
        });

        let epic_id = epic.id;
        let child_id = child.id;
        store.requirements.push(epic);
        store.requirements.push(child);

        let epic = store.get_requirement_by_id(&epic_id).unwrap();
        assert_eq!(
            effective_display_status(&store, epic),
            RequirementStatus::NeedsAttention
        );
        assert_ne!(epic.status, RequirementStatus::NeedsAttention);

        colored::control::set_override(false);
        let epic_cell =
            list_requirement_status_cell(&store, epic, &RequirementStatus::NeedsAttention, 22);
        colored::control::unset_override();
        assert!(
            epic_cell.contains("Shelved (stale-base)"),
            "cell: {epic_cell:?}"
        );

        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Stored(RequirementStatus::NeedsAttention)
        ));
        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Shelved
        ));
        assert!(!requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::NeedsDecision
        ));

        let (source, lens) = effective_needs_attention_lens_with_source(
            &store,
            epic,
            &RequirementStatus::NeedsAttention,
        )
        .expect("effective lens");
        assert_eq!(source.id, child_id);
        assert_eq!(lens.label(), "Shelved (stale-base)");
    }
}

/// Read the `[external_refs]` provider → base-URL map from `.aida/config.toml`.
///
/// Each line under the section is `provider = "https://..."` (e.g.
/// `linear = "https://linear.app/acme/issue/"`). The project root is the
/// parent of the orphan store path; fall back to `find_project_root()` and
/// finally CWD. Missing file or section yields an empty map (callers then use
/// the built-in defaults — see `aida_core::external_refs::render_ref_url`).
/// Mirrors the line-by-line `read_config_workflow_hints` pattern.
/// trace:STORY-476 | ai:claude
pub(crate) fn read_external_ref_base_urls(
    store_path: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(|| find_project_root().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let config_path = project_root.join(".aida").join("config.toml");
    let mut map = std::collections::HashMap::new();
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let mut in_section = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "external_refs";
            continue;
        }
        if in_section {
            if let Some((key, val)) = line.split_once('=') {
                let provider = key.trim().to_ascii_lowercase();
                // Strip a trailing inline `# comment`, then surrounding quotes.
                let v = val
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                if !provider.is_empty() && !v.is_empty() {
                    map.insert(provider, v);
                }
            }
        }
    }
    map
}

/// The status to DISPLAY for a requirement. For an EPIC this is the read-only
/// rollup of its children (`derive_epic_status`), so the full-store display
/// surfaces (`aida show`, `aida why`, `aida status`) agree with the cache-backed
/// `aida list`, whose `status` column already holds the derived value. For every
/// non-epic, and for an epic whose only children are Rejected (the derivation
/// declines to auto-reject), this is the stored status.
// trace:BUG-626 | ai:claude
pub(crate) fn effective_display_status(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
) -> RequirementStatus {
    if req.req_type == RequirementType::Epic {
        if let Some(derived) = aida_core::rollup::derive_epic_status(store, req.id) {
            return derived;
        }
    }
    req.status.clone()
}

pub(crate) fn show_requirement(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store = storage.load_for_read()?;

    // Parse UUID or SPEC-ID
    let id = parse_requirement_id(id_str, &store)?;

    // Find the specified requirement
    let req = store
        .get_requirement_by_id(&id)
        .context("Requirement not found")?;

    // Display the requirement details
    println!("{}: {}", "ID".blue(), req.id);
    if let Some(spec_id) = &req.spec_id {
        println!("{}: {}", "SPEC-ID".blue(), spec_id);
    }
    println!("{}: {}", "Title".blue(), req.title);
    println!("{}: {}", "Description".blue(), req.description);

    // BUG-626: an epic's displayed status is the read-only rollup of its
    // children, not the stored field. trace:BUG-626 | ai:claude
    let display_status = effective_display_status(&store, req);
    // BUG-1687: deferral is a display override that outranks both the parked
    // lens and the stored status — a deferred spec must not read as "act now".
    // trace:BUG-1687 | ai:claude
    let status_str = if let Some(badge) = status_display::deferred_badge(req) {
        badge
    } else if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(&store, req, &display_status)
            .map(|lens| needs_attention_badge_for_lens(&lens))
            .unwrap_or_else(|| status_display::parked_status_badge(req))
    } else {
        match display_status {
            RequirementStatus::Draft => "Draft".yellow().to_string(),
            RequirementStatus::Approved => "Approved".blue().to_string(),
            RequirementStatus::Planned => "Planned".cyan().to_string(),
            RequirementStatus::InProgress => "In Progress".magenta().to_string(),
            RequirementStatus::Done => "Done".bright_green().bold().to_string(),
            RequirementStatus::Completed => "Completed".green().to_string(),
            RequirementStatus::Rejected => "Rejected".red().to_string(),
            // trace:TASK-1176 | ai:claude
            RequirementStatus::Superseded => "Superseded".green().dimmed().to_string(),
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        }
    };
    println!("{}: {}", "Status".blue(), status_str);

    let priority_str = match req.priority {
        RequirementPriority::High => "High".red(),
        RequirementPriority::Medium => "Medium".yellow(),
        RequirementPriority::Low => "Low".green(),
    };
    println!("{}: {}", "Priority".blue(), priority_str);

    let type_str = match req.req_type {
        RequirementType::Functional => "Functional",
        RequirementType::NonFunctional => "Non-Functional",
        RequirementType::System => "System",
        RequirementType::User => "User",
        RequirementType::ChangeRequest => "Change Request",
        RequirementType::Bug => "Bug",
        RequirementType::Epic => "Epic",
        RequirementType::Story => "Story",
        RequirementType::Task => "Task",
        RequirementType::Spike => "Spike",
        RequirementType::Sprint => "Sprint",
        RequirementType::Folder => "Folder",
        RequirementType::Meta => "Meta",
        RequirementType::Principle => "Principle",
        RequirementType::Vision => "Vision",
        RequirementType::Constraint => "Constraint",
        RequirementType::Decision => "Decision",
        RequirementType::Term => "Term",
        RequirementType::Doc => "Doc",
        RequirementType::Faq => "Faq",
    };
    println!("{}: {}", "Type".blue(), type_str);

    println!("{}: {}", "Owner".blue(), req.owner);
    // STORY-639: render assignee only when set. trace:STORY-639 | ai:claude
    if let Some(assignee) = req.assignee.as_deref() {
        println!("{}: {}", "Assignee".blue(), assignee);
    }
    println!("{}: {}", "Feature".blue(), req.feature);
    println!("{}: {}", "Created".blue(), req.created_at);
    println!("{}: {}", "Modified".blue(), req.modified_at);

    if !req.tags.is_empty() {
        let tags_str = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        println!("{}: {}", "Tags".blue(), tags_str);
    }

    if !req.dependencies.is_empty() {
        let deps_str = req
            .dependencies
            .iter()
            .map(|uuid| uuid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("{}: {}", "Dependencies".blue(), deps_str);
    }

    // STORY-542: user-facing interface changes captured at close — the
    // deterministic source the operator digest reads. trace:STORY-542
    if let Some(ic) = req.interface_changes.as_ref() {
        if !ic.is_empty() {
            println!("\n{}:", "Interface changes".green());
            let surfaces: [(&str, &Vec<String>); 4] = [
                ("CLI", &ic.cli),
                ("MCP", &ic.mcp),
                ("TUI", &ic.tui),
                ("Other", &ic.other),
            ];
            for (label, lines) in surfaces {
                for line in lines {
                    println!("  {} {}", format!("[{label}]").cyan(), line);
                }
            }
        }
    }

    // STORY-698: the verification steps the builder ran, captured at
    // `aida queue done` — the implementation audit trail the PR body surfaces.
    // trace:STORY-698 | ai:claude
    if let Some(steps) = req
        .implementation_info
        .as_ref()
        .and_then(|info| info.test_coverage_notes.as_ref())
        .filter(|s| !s.trim().is_empty())
    {
        println!("\n{}:", "Verification steps".green());
        for line in steps.lines().filter(|l| !l.trim().is_empty()) {
            println!("  {} {}", "-".dimmed(), line);
        }
    }

    if !req.relationships.is_empty() {
        println!("\n{}:", "Relationships".green());
        for relationship in &req.relationships {
            let target = store.get_requirement_by_id(&relationship.target_id);
            if let Some(target_req) = target {
                let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

                // TASK-102: shared phrase mapping (STORY-333 added typed
                // blocked-by/blocks prose; now via `relationship_phrase`).
                let description = relationship_phrase(&relationship.rel_type);

                println!(
                    "  {} {} - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    target_req.title
                );
            } else {
                println!(
                    "  {} {} {}",
                    relationship.rel_type.to_string().cyan(),
                    relationship.target_id.to_string().yellow(),
                    "(not found)".red()
                );
            }
        }
    }

    if !req.comments.is_empty() {
        println!("\n{}:", "Comments".green());
        for comment in &req.comments {
            print_comment(comment, 0);
        }
    }

    if !req.history.is_empty() {
        println!("\n{}:", "History".green());
        for entry in &req.history {
            println!(
                "\n{}:",
                entry
                    .timestamp
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .yellow()
            );
            println!("  {} {}", "By:".dimmed(), entry.author.cyan());
            for change in &entry.changes {
                println!(
                    "  {} {} → {}",
                    change.field_name.magenta(),
                    change.old_value.red(),
                    change.new_value.green()
                );
            }
        }
    }

    Ok(())
}

/// STORY-582: render the durable processing-record audit trail for `aida
/// show`. One block per record: timestamp + agent header, the linkage
/// (PR/commit/brief), the summary, and the decisions / punted / verdict
/// tails when present. trace:STORY-582 | ai:claude
pub(crate) fn print_processing_records(records: &[aida_core::ProcessingRecord]) {
    println!("\n{}:", "Processing record".green().bold());
    for rec in records {
        println!(
            "\n{}  {}",
            rec.timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
                .yellow(),
            format!("by {}", rec.agent).cyan(),
        );
        // Linkage line — PR / commit / brief, whichever are present.
        let mut linkage: Vec<String> = Vec::new();
        if let Some(pr) = rec.pr {
            linkage.push(format!("PR #{pr}"));
        }
        if let Some(sha) = rec.commit_sha.as_deref() {
            linkage.push(format!("commit {}", &sha[..sha.len().min(8)]));
        }
        if let Some(brief) = rec.brief_ref.as_deref() {
            linkage.push(format!("brief {brief}"));
        }
        if let Some(v) = rec.review_verdict.as_deref() {
            linkage.push(format!("verdict {v}"));
        }
        if !linkage.is_empty() {
            println!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                linkage.join(" · ").dimmed()
            );
        }
        println!("  {}", rec.summary);
        for d in &rec.decisions {
            println!("  {} {}", "decision:".magenta(), d);
        }
        for p in &rec.punted {
            println!("  {} {}", "punted:".yellow(), p);
        }
    }
}

/// STORY-582: `aida record list|prune` — inspect or trim the durable
/// processing-record audit trail. List is read-only (backend); prune writes
/// through `Storage::update_atomically`, propose-by-default. trace:STORY-582
// trace:REQ-0232 | ai:claude:high
/// What a `--add-tag` / `--remove-tag` pass actually did.
///
/// BUG-1770: the bool this replaces could not distinguish "removed nothing
/// because the tag was absent" from "removed nothing because something went
/// wrong", and the caller rendered both as `No changes specified` — a message
/// that sends the user looking for a flag they already passed. Naming the tags
/// that matched nothing turns a silent no-op into a visible one.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TagDeltaReport {
    /// Tags newly inserted.
    pub(crate) added: Vec<String>,
    /// Tags actually removed.
    pub(crate) removed: Vec<String>,
    /// `--add-tag` values that were already present.
    pub(crate) already_present: Vec<String>,
    /// `--remove-tag` values that matched no existing tag.
    pub(crate) absent: Vec<String>,
}

impl TagDeltaReport {
    pub(crate) fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }

    /// One line per outcome, or `None` when nothing was requested at all.
    /// Deterministic order so output is stable for tests and scripts.
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        if !self.added.is_empty() {
            out.push(format!(
                "added {}: {}",
                plural(self.added.len(), "tag"),
                list(&self.added)
            ));
        }
        if !self.removed.is_empty() {
            out.push(format!(
                "removed {}: {}",
                plural(self.removed.len(), "tag"),
                list(&self.removed)
            ));
        }
        if !self.already_present.is_empty() {
            out.push(format!(
                "already present, nothing added: {}",
                list(&self.already_present)
            ));
        }
        if !self.absent.is_empty() {
            out.push(format!(
                "no matching {} to remove: {}",
                if self.absent.len() == 1 {
                    "tag"
                } else {
                    "tags"
                },
                list(&self.absent)
            ));
        }
        out
    }
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The flag name used in a whitespace refusal raised for `aida add --tags` /
/// `aida edit --tags`, where the pasteable repair is one comma-separated value.
// trace:BUG-1770 | ai:claude
pub(crate) const TAGS_FLAG: &str = "--tags";

/// How a refused tag value arrived, so the refusal can show a repair the
/// caller can actually paste back.
///
/// A refusal that suggests `--tags a,b` to an MCP client, or a comma list to a
/// repeatable flag, names a form that surface does not accept — which is a
/// worse failure than the blob, because it reads as authoritative.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagRepair {
    /// A comma-separated argument: `--tags a,b`.
    CommaFlag(&'static str),
    /// A repeatable single-value flag: `--add-tag a --add-tag b`.
    RepeatedFlag(&'static str),
    /// A JSON array field on an MCP tool call: `"tags": ["a", "b"]`.
    JsonArray(&'static str),
}

impl TagRepair {
    /// The pasteable repair for `parts`, in this surface's own syntax.
    pub(crate) fn suggestion(&self, parts: &[&str]) -> String {
        match self {
            Self::CommaFlag(flag) => format!("{flag} {}", parts.join(",")),
            Self::RepeatedFlag(flag) => parts
                .iter()
                .map(|p| format!("{flag} {p}"))
                .collect::<Vec<_>>()
                .join(" "),
            Self::JsonArray(field) => format!(
                "\"{field}\": [{}]",
                parts
                    .iter()
                    .map(|p| format!("\"{p}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Refuse a tag value that contains whitespace.
///
/// A space-separated `--tags` argument is stored as ONE tag whose human
/// rendering is byte-identical to the N tags it was meant to be, so the
/// malformation is invisible by eye while being unmatchable by `--remove-tag`
/// and invisible to every tag-keyed filter and sweep. Refusing is preferred
/// over silently splitting on whitespace: splitting would make a genuinely
/// intended multi-word tag unrepresentable without warning, and the caller's
/// intent here is unambiguous enough to just report.
///
/// `flag` is the flag the value arrived on, so the suggestion is pasteable.
/// `--remove-tag` is deliberately NOT validated: a tag already malformed in the
/// store can only be named by reproducing it verbatim, and refusing that would
/// leave the existing blobs unrepairable by the incremental form.
// trace:BUG-1770 | ai:claude
pub(crate) fn validate_tag_value(tag: &str, repair: TagRepair) -> anyhow::Result<()> {
    if !tag.chars().any(char::is_whitespace) {
        return Ok(());
    }
    let parts: Vec<&str> = tag.split_whitespace().collect();
    let suggestion = repair.suggestion(&parts);
    anyhow::bail!(
        "tag \"{tag}\" contains whitespace — did you mean `{suggestion}` ?\n  \
         A tag may not contain whitespace: the whole value would be stored as ONE tag that \
         renders identically to the {} separate tags it looks like, and would then be \
         invisible to --remove-tag and to every tag-keyed filter.",
        parts.len()
    )
}

/// Parse a comma-separated `--tags` argument into validated tag values,
/// trimmed, empties dropped, order preserved.
///
/// Every `--tags` write path goes through this so the whitespace rule cannot be
/// reintroduced by a fourth site splitting the string itself.
// trace:BUG-1770 | ai:claude
pub(crate) fn parse_tag_list(raw: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for tag in raw.split(',') {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_tag_value(trimmed, TagRepair::CommaFlag(TAGS_FLAG))?;
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// The reporting form. [`apply_tag_deltas`] is the bool-returning wrapper kept
/// for callers that only need "did anything change".
// trace:BUG-1770 | ai:claude
pub(crate) fn apply_tag_deltas_report(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<TagDeltaReport> {
    // BUG-1770: validate the entire `add` list BEFORE mutating anything, so a
    // refusal can never leave a half-applied tag set behind for the caller to
    // save. trace:BUG-1770 | ai:claude
    for raw in add {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            validate_tag_value(trimmed, TagRepair::RepeatedFlag("--add-tag"))?;
        }
    }
    let mut report = TagDeltaReport::default();
    for raw in add {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.insert(trimmed.to_string()) {
            report.added.push(trimmed.to_string());
        } else {
            report.already_present.push(trimmed.to_string());
        }
    }
    for raw in remove {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.remove(trimmed) {
            report.removed.push(trimmed.to_string());
        } else {
            report.absent.push(trimmed.to_string());
        }
    }
    Ok(report)
}

/// Apply additive (`--add-tag`) and subtractive (`--remove-tag`) tag deltas
/// without disturbing tags the caller didn't name. Empty / whitespace-only
/// entries are ignored. Adding a present tag or removing an absent one is
/// a graceful no-op. Returns whether the set actually changed.
// trace:TASK-351 | ai:claude
pub(crate) fn apply_tag_deltas(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<bool> {
    Ok(apply_tag_deltas_report(tags, add, remove)?.changed())
}

/// Build the loud-on-clobber warning shown when `aida edit --tags` REPLACES the
/// whole tag set. `--tags` is a full replace (other scripts depend on that), so
/// we don't change its semantics — we surface the old→new diff so the caller
/// sees that provenance/routing tags were dropped, and point at the incremental
/// `--add-tag` / `--remove-tag` forms. Returns `None` when the set is unchanged
/// (no warning needed). The returned string is sorted for deterministic output.
/// trace:BUG-545 | ai:claude
pub(crate) fn tags_replace_warning(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Option<String> {
    if old_tags == new_tags {
        return None;
    }
    let fmt_set = |s: &HashSet<String>| -> String {
        if s.is_empty() {
            "(none)".to_string()
        } else {
            let mut v: Vec<&String> = s.iter().collect();
            v.sort();
            v.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(",")
        }
    };
    Some(format!(
        "--tags REPLACES all tags (was: {} → now: {}). \
         Use --add-tag/--remove-tag to modify incrementally.",
        fmt_set(old_tags),
        fmt_set(new_tags)
    ))
}

// trace:BUG-1252 | ai:codex
/// Structural tags participate in routing and graph integrity. A full tag-set
/// replacement must not silently erase them.
pub(crate) fn dropped_structural_tags(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Vec<String> {
    const PREFIXES: &[&str] = &[
        "parent:",
        "batch:",
        "lane:",
        "severity:",
        "lifecycle:",
        "aida:",
    ];
    let mut dropped: Vec<String> = old_tags
        .difference(new_tags)
        .filter(|tag| PREFIXES.iter().any(|prefix| tag.starts_with(prefix)))
        .cloned()
        .collect();
    dropped.sort();
    dropped
}

pub(crate) fn enforce_structural_tag_replacement(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
    force: bool,
) -> Result<Vec<String>> {
    let dropped = dropped_structural_tags(old_tags, new_tags);
    if !dropped.is_empty() && !force {
        anyhow::bail!(
            "AIDA_AGENT_OUTPUT structural_tags_dropped=[{}] refusal=use_--add-tag/--remove-tag_or_pass_--force; refusing --tags replacement because it would drop structural tags: {}. Use --add-tag/--remove-tag for incremental edits, or --force/--replace-tags to replace intentionally.",
            dropped.join(","), dropped.join(", ")
        );
    }
    Ok(dropped)
}

/// STORY-439: stamp `complexity:<level>` / `estimated-assistance:<level>`
/// tags on `spec` so the new dimension composes with existing tag tooling
/// (`aida queue list --tag-prefix complexity:`, batch routing). Mirrors
/// `load_store_for_lookup` for backend resolution. Best-effort — a missing
/// store / missing spec / save failure logs and returns; the pickup itself
/// is unaffected. trace:STORY-439 | ai:claude
pub(crate) fn apply_calibration_tags(
    storage: &Storage,
    spec: &str,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    assist_est: Option<complexity_calibration::AssistanceLevel>,
) {
    if complexity.is_none() && assist_est.is_none() {
        return;
    }
    // Git-canonical path — direct backend write, exactly like Command::Edit.
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            let mut changed = false;
            if let Some(c) = complexity {
                changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
            }
            if let Some(a) = assist_est {
                changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
            }
            if changed {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp calibration tags on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    // Legacy fallback — load the whole store, mutate, save. Heavier but
    // only fires on projects that haven't migrated.
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    let mut changed = false;
    if let Some(c) = complexity {
        changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
    }
    if let Some(a) = assist_est {
        changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
    }
    if changed {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save calibration tags on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

/// STORY-451: stamp `effort:<touchpoint>:<bucket>` while preserving the
/// other effort touchpoints. Best-effort sibling of [`apply_calibration_tags`].
/// trace:STORY-451 | ai:codex
pub(crate) fn apply_effort_tag(
    storage: &Storage,
    spec: &str,
    touchpoint: effort_calibration::EffortTouchpoint,
    effort: Option<effort_calibration::EffortBucket>,
) {
    let Some(effort) = effort else {
        return;
    };
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp effort tag on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save effort tag on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/apply_tag_deltas_tests.rs"]
mod apply_tag_deltas_tests;

#[cfg(test)]
#[path = "tests/bug_1770_tag_whitespace_tests.rs"]
mod bug_1770_tag_whitespace_tests;

#[cfg(test)]
#[path = "tests/tags_replace_warning_tests.rs"]
mod tags_replace_warning_tests;

/// Edit a requirement non-interactively using CLI flags
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edit_requirement_cli(
    storage: &Storage,
    id_str: &str,
    title: &Option<String>,
    description: &Option<String>,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
    // trace:TASK-351 | ai:claude
    add_tag: &[String],
    remove_tag: &[String],
) -> Result<()> {
    // Load requirements
    let store_for_lookup = storage.load()?;
    let id = parse_requirement_id(id_str, &store_for_lookup)?;

    let mut store = storage.load()?;
    let req = store
        .get_requirement_by_id_mut(&id)
        .context("Requirement not found")?;

    let mut changes: Vec<FieldChange> = Vec::new();
    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());

    // Update title
    if let Some(new_title) = title {
        if !new_title.is_empty() && new_title != &req.title {
            changes.push(Requirement::field_change(
                "title",
                req.title.clone(),
                new_title.clone(),
            ));
            req.title = new_title.clone();
        }
    }

    // Update description
    if let Some(new_desc) = description {
        if new_desc != &req.description {
            changes.push(Requirement::field_change(
                "description",
                req.description.clone(),
                new_desc.clone(),
            ));
            req.description = new_desc.clone();
        }
    }

    // Update status
    let mut left_needs_attention = false;
    if let Some(status_str) = status {
        let new_status = match status_str.to_lowercase().as_str() {
            "draft" => RequirementStatus::Draft,
            "approved" => RequirementStatus::Approved,
            "planned" => RequirementStatus::Planned,
            "in_progress" | "in-progress" | "inprogress" => RequirementStatus::InProgress,
            "done" => RequirementStatus::Done,
            "completed" => RequirementStatus::Completed,
            "rejected" => RequirementStatus::Rejected,
            "needs_attention" | "needs-attention" | "needsattention" => {
                RequirementStatus::NeedsAttention
            }
            _ => anyhow::bail!("Invalid status '{}'. Use: draft, approved, planned, in_progress, done, completed, rejected, needs-attention", status_str),
        };
        // STORY-332: enforce the NeedsAttention transition rules here too.
        if let Some(msg) = forbidden_attention_transition(&req.status, &new_status) {
            anyhow::bail!(msg);
        }
        if new_status != req.status {
            // TASK-358: a triage that takes a spec out of NeedsAttention is
            // the trigger for cleaning up any orchestrator-escalated worktree
            // for it. Capture the transition direction before the field
            // changes — the actual cleanup fires after the store save so a
            // failed save doesn't leave a half-cleaned state.
            // trace:TASK-358 | ai:claude
            left_needs_attention = matches!(req.status, RequirementStatus::NeedsAttention)
                && !matches!(new_status, RequirementStatus::NeedsAttention);
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            let prior = req.status.clone();
            req.status = new_status;
            // trace:TASK-1600 | ai:codex
            completion::record_reopen(req, &prior, Some(&queue_cmd::requeue_project_root(storage)));
        }
    }

    // Update priority
    if let Some(priority_str) = priority {
        let new_priority = match priority_str.to_lowercase().as_str() {
            "high" => RequirementPriority::High,
            "medium" | "med" => RequirementPriority::Medium,
            "low" => RequirementPriority::Low,
            _ => anyhow::bail!(
                "Invalid priority '{}'. Use: high, medium, low",
                priority_str
            ),
        };
        if new_priority != req.priority {
            changes.push(Requirement::field_change(
                "priority",
                format!("{:?}", req.priority),
                format!("{:?}", new_priority),
            ));
            req.priority = new_priority;
        }
    }

    // Update type
    if let Some(type_str) = req_type {
        let new_type = match type_str.to_lowercase().as_str() {
            "functional" | "func" => RequirementType::Functional,
            "non-functional" | "nonfunctional" | "nfr" => RequirementType::NonFunctional,
            "system" | "sys" => RequirementType::System,
            "user" => RequirementType::User,
            "change-request" | "change" | "cr" => RequirementType::ChangeRequest,
            "bug" => RequirementType::Bug,
            "epic" => RequirementType::Epic,
            "story" => RequirementType::Story,
            "task" => RequirementType::Task,
            "spike" => RequirementType::Spike,
            "sprint" => RequirementType::Sprint,
            "folder" => RequirementType::Folder,
            "meta" => RequirementType::Meta,
            // ADR / knowledge-graph family (FR-1-074). trace:TASK-716 | ai:claude
            "principle" | "prin" => RequirementType::Principle,
            "vision" | "vis" => RequirementType::Vision,
            "constraint" | "con" => RequirementType::Constraint,
            "decision" | "adr" => RequirementType::Decision,
            "term" | "glossary" => RequirementType::Term,
            // trace:STORY-104 | ai:claude
            "doc" | "documentation" => RequirementType::Doc,
            "faq" => RequirementType::Faq,
            _ => anyhow::bail!("Invalid type '{}'. Use: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq", type_str),
        };
        if new_type != req.req_type {
            changes.push(Requirement::field_change(
                "type",
                format!("{:?}", req.req_type),
                format!("{:?}", new_type),
            ));
            req.req_type = new_type;
        }
    }

    // Update owner
    if let Some(new_owner) = owner {
        if new_owner != &req.owner {
            changes.push(Requirement::field_change(
                "owner",
                req.owner.clone(),
                new_owner.clone(),
            ));
            req.owner = new_owner.clone();
        }
    }

    // Update feature
    if let Some(new_feature) = feature {
        if new_feature != &req.feature {
            changes.push(Requirement::field_change(
                "feature",
                req.feature.clone(),
                new_feature.clone(),
            ));
            req.feature = new_feature.clone();
        }
    }

    // Update tags
    if let Some(tags_str) = tags {
        // BUG-1770: this is the SECOND edit backend the bug's acceptance names,
        // and it split the argument itself rather than sharing a parser — so
        // the whitespace rule has to be routed through `parse_tag_list` here
        // too or `aida edit --tags "a b"` stays reachable from this path.
        // trace:BUG-1770 | ai:claude
        let new_tags: HashSet<String> = parse_tag_list(tags_str)?.into_iter().collect();
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let new_tags_str: String = new_tags.iter().cloned().collect::<Vec<_>>().join(", ");
        if new_tags != req.tags {
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
            req.tags = new_tags;
        }
    }

    // TASK-351: partial tag edits — additive / subtractive variants that
    // don't clobber the rest. `--tags` and `--add-tag`/`--remove-tag` are
    // mutually exclusive at the clap layer, so at most one of the two
    // blocks fires.
    // trace:TASK-351 | ai:claude
    // BUG-1770: keep the REPORT here too. This backend discarded the bool and
    // then fell through to "No changes made to <ID>" — the same conflation of
    // "nothing was asked" with "a tag flag matched nothing" that AC3 narrows on
    // the canonical backend. trace:BUG-1770 | ai:claude
    let mut tag_report = TagDeltaReport::default();
    if !add_tag.is_empty() || !remove_tag.is_empty() {
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        tag_report = apply_tag_deltas_report(&mut req.tags, add_tag, remove_tag)?;
        if tag_report.changed() {
            let new_tags_str: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
        }
    }

    if changes.is_empty() {
        let tag_lines = tag_report.summary_lines();
        if tag_lines.is_empty() {
            println!("{} No changes made to {}", "!".yellow(), spec_id);
        } else {
            for line in tag_lines {
                println!("{line}");
            }
        }
        return Ok(());
    }

    // BUG-1637: record under the caller's identity (was the literal "CLI"),
    // and the status change through the one shared history helper.
    // trace:BUG-1637 | ai:claude
    record_edit_changes(req, &get_default_author(), &changes);

    // Save changes
    storage.save(&store)?;
    println!(
        "{} Updated {} ({} field(s) changed)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        spec_id,
        changes.len()
    );
    // BUG-1770: say what the tag flags did on the success path too — a caller
    // who removes two tags and mistypes one must not see only "Updated".
    // trace:BUG-1770 | ai:claude
    for line in tag_report.summary_lines() {
        println!("  {line}");
    }

    // TASK-358: triage out of NeedsAttention — clean up any orchestrator-
    // escalated worktree for this spec. The lease's `escalated_to_human`
    // marker (stamped by the `--escalate-blocks` path) is the safety gate:
    // an interactive user session on the same spec, or an
    // `--escalate-defaults` advisor-resume, has the marker absent and is
    // left alone. trace:TASK-358 | ai:claude
    if left_needs_attention {
        if let Ok(project_root) = find_project_root() {
            cleanup_escalated_leases_for_spec(&project_root, &spec_id);
            // BUG-674: close the now-stale open punt-ledger record for a spec
            // resumed out of NeedsAttention. trace:BUG-674 | ai:claude
            let _ = punt::close_open_records(
                &project_root,
                &spec_id,
                punt::RESOLUTION_HUMAN_RESOLVED,
                "resolved",
                None,
                Some("resume"),
            );
        }
    }

    Ok(())
}
