//! Findings command: promote, dismiss, add, recur, gate authority and promote-to-gate, reviewer-verdict and non-blocking findings (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

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
