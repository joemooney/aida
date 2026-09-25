//! The NeedsAttention -> back-in-flight transition.
//!
//! A drain parks a spec in NeedsAttention (a punt, an orchestrator shelve, or
//! an advisor escalation) and moves on. Triage then returns it to Approved or
//! In Progress through one of four doors: `aida queue rework <ID>` (and its
//! one-keystroke loop, bare `aida rework`), `aida edit <ID> --status`, the
//! `queue_rework` MCP tool, and the re-drive supervisor. The doors gate on
//! advisor authority at the call site; this module owns what happens to the
//! spec once the gate has passed: [`return_to_flight`] is the one transition
//! every door runs on the one spec, re-reading the status just before its
//! single-spec write.
//!
//! The drain's ready set is "Approved + queued + not parking-tagged + no
//! pending decision" (`burndown::classify`). Before this module, triage flipped
//! the status but left the markers the shelve wrote, and one of them, the
//! `needs-human` tag the headless advisor adds on escalation, is a parking tag.
//! A triaged spec therefore stayed out of the drain until someone noticed the
//! stale tag. That tag is now cleared here, but ONLY when a human is present at
//! an interactive terminal. The escalation is the advisor handing the spec to a
//! human; advisor authority is also held by a non-TTY advisor agent and by an
//! orchestrated drain phase, and neither may undo it. Those callers still get
//! the status flip they are authorized for, but the tag stays and they are
//! told the spec remains parked until a human clears the escalation.
//!
//! One case must NOT re-enter: a spec whose design fork is still an open
//! question. A pending `DecisionRequest` is left in place, so
//! `burndown::classify` keeps parking the spec until the decision is answered,
//! and the drain cannot be re-dispatched into the same fork.
// trace:TASK-1311 | ai:claude

use aida_core::{Requirement, RequirementStatus};

/// Tags written by the drain machinery to mark a spec as parked for a human.
/// They are cleared once triage has returned the spec to flight. Hand-applied
/// intent tags (`needs-decision`, `needs-design-signoff`,
/// `needs-supervised-build`, `deferred:*`) are deliberately NOT listed: they
/// describe the spec rather than the shelve, and removing them silently would
/// override an explicit human choice.
const SHELVE_MARKER_TAGS: &[&str] = &["needs-human"];

/// What [`clear_shelve_markers`] removed, for the caller's output and audit
/// note.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ClearedMarkers {
    /// Parking tags removed from the spec.
    pub removed_tags: Vec<String>,
    /// Escalation tags kept because no human was present to clear them. The
    /// spec stays parked until a human does.
    pub kept_tags: Vec<String>,
    /// The shelve's one-line summary (`phase/kind: detail`), when an
    /// orchestrator FailureReason was present.
    pub failure_summary: Option<String>,
    /// The punt's one-line summary (`category: detail`), when an
    /// AttentionReason was present.
    pub punt_summary: Option<String>,
}

impl ClearedMarkers {
    /// One line suitable for a spec comment recording why the spec re-entered
    /// flight, so a wrong return can be told apart from a correct one.
    pub(crate) fn audit_note(&self, via: &str, target: &str, reason: Option<&str>) -> String {
        let mut note = format!("Returned from NeedsAttention to {target} via {via}.");
        if let Some(f) = &self.failure_summary {
            note.push_str(&format!(" It was shelved on: {f}."));
        }
        if let Some(p) = &self.punt_summary {
            note.push_str(&format!(" It was punted on: {p}."));
        }
        if !self.removed_tags.is_empty() {
            note.push_str(&format!(
                " Cleared parking tag(s): {}.",
                self.removed_tags.join(", ")
            ));
        }
        if !self.kept_tags.is_empty() {
            note.push_str(&format!(
                " Kept escalation tag(s) {} (no human at a terminal); still parked for a human.",
                self.kept_tags.join(", ")
            ));
        }
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => note.push_str(&format!(" Triage reason: {r}")),
            None => note.push_str(" No triage reason was given."),
        }
        note
    }
}

/// Whether this caller may clear an advisor's escalation to a human. Only a
/// human present at an interactive terminal may: the integrity-floor predicate,
/// which no role or orchestrator corroboration can satisfy. Advisor authority
/// is NOT enough, because a non-TTY advisor agent and an orchestrated drain
/// phase both hold it.
pub(crate) fn may_clear_escalation(human_present: bool) -> bool {
    crate::integrity_floor_authority_from(human_present)
}

/// The same decision for the running process: stdin is a terminal. Always
/// false for the MCP server, whose stdin is the protocol pipe.
pub(crate) fn caller_may_clear_escalation() -> bool {
    use std::io::IsTerminal;
    may_clear_escalation(std::io::stdin().is_terminal())
}

/// Clear the markers a shelve, punt or escalation left on `req`, so a spec
/// triaged out of NeedsAttention re-enters the drain's candidate set. The
/// caller has already changed the status and passed the advisor-authority
/// gate. The escalation tag is cleared only when `clear_escalation` is true
/// (see [`may_clear_escalation`]); otherwise it is kept and reported in
/// `kept_tags`. A pending decision request is left untouched on purpose (see
/// the module docs).
pub(crate) fn clear_shelve_markers(
    req: &mut Requirement,
    clear_escalation: bool,
) -> ClearedMarkers {
    let failure_summary = req.failure_reason.take().map(|fr| {
        let detail = fr.detail.lines().next().unwrap_or("").trim().to_string();
        format!("{}/{}: {}", fr.phase, fr.kind, detail)
    });
    let punt_summary = req.attention_reason.take().map(|a| {
        let detail = a.detail.lines().next().unwrap_or("").trim().to_string();
        format!("{}: {}", a.category, detail)
    });
    let mut removed_tags: Vec<String> = req
        .tags
        .iter()
        .filter(|t| {
            SHELVE_MARKER_TAGS
                .iter()
                .any(|m| t.trim().eq_ignore_ascii_case(m))
        })
        .cloned()
        .collect();
    removed_tags.sort();
    if !clear_escalation {
        return ClearedMarkers {
            removed_tags: Vec::new(),
            kept_tags: removed_tags,
            failure_summary,
            punt_summary,
        };
    }
    for t in &removed_tags {
        req.tags.remove(t);
    }
    ClearedMarkers {
        removed_tags,
        kept_tags: Vec::new(),
        failure_summary,
        punt_summary,
    }
}

/// The one-line warning for a caller that flipped the status but could not
/// clear the escalation. `None` when nothing was kept.
pub(crate) fn kept_escalation_warning(id: &str, cleared: &ClearedMarkers) -> Option<String> {
    if cleared.kept_tags.is_empty() {
        return None;
    }
    Some(format!(
        "{id} stays parked: the `{}` escalation is for a human, and only a human at an \
         interactive terminal can clear it",
        cleared.kept_tags.join("`, `")
    ))
}

/// The one-keystroke requeue command for a NeedsAttention spec. It is the
/// same verb for punts, shelves and escalations: `queue rework` moves the spec
/// to Approved, clears the shelve markers and puts it back on the queue.
pub(crate) fn requeue_command(id: &str) -> String {
    format!("aida queue rework {id}")
}

// ---------------------------------------------------------------------------
// The single owner of the NeedsAttention exit.
//
// Four doors move a spec out of NeedsAttention: `aida queue rework`, `aida
// edit --status`, the `queue_rework` MCP tool and the re-drive supervisor.
// Each used to carry its own copy of the transition, and each computed the
// target from a status it had read before its write. The owner below is pure:
// every door runs it on the one spec it read, re-reads the status just before
// its targeted single-spec write, and writes nothing if the spec moved. (No
// door uses the git store's whole-store load/save: it is lock-free and can
// drop concurrently added specs.)
// trace:STORY-1429 | ai:claude
// ---------------------------------------------------------------------------

/// Who is returning a spec to flight, and through which door.
#[derive(Debug, Clone)]
pub(crate) struct ReturnCtx {
    /// The door as the audit note names it, e.g. "`aida queue rework`".
    pub via: String,
    /// The door as the `SpecRequeued` event names it, e.g. `queue-rework`.
    pub via_slug: &'static str,
    /// Comment author for the audit note.
    pub author: String,
    /// Whether this caller may clear a human escalation (see
    /// [`may_clear_escalation`]).
    pub clear_escalation: bool,
    /// The triage reason. It goes into the audit note and nowhere else, so
    /// the spec never carries the same reason twice.
    pub reason: Option<String>,
}

/// What [`return_to_flight`] did to the copy it was handed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ReturnOutcome {
    /// The status matched, the transition was applied and one audit note was
    /// written.
    Returned {
        from: RequirementStatus,
        to: RequirementStatus,
        cleared: ClearedMarkers,
    },
    /// The spec is already at the target status (a second requeue of the same
    /// spec). Nothing was changed: no status write, no note, no event.
    AlreadyInFlight {
        status: RequirementStatus,
        since: chrono::DateTime<chrono::Utc>,
    },
    /// The status moved between the caller's read and its write, for example
    /// a drain claimed the spec. Nothing was changed.
    StatusMoved {
        expected: RequirementStatus,
        actual: RequirementStatus,
    },
    /// The spec is no longer in the store.
    Missing,
}

impl ReturnOutcome {
    /// True when the transition was applied.
    pub(crate) fn applied(&self) -> bool {
        matches!(self, ReturnOutcome::Returned { .. })
    }
}

/// Apply a NeedsAttention exit to `req`, the copy the caller read inside its
/// atomic write. Compares the status against `expected` (compare-and-swap);
/// on a match sets `target`, clears the shelve markers when leaving
/// NeedsAttention, and writes ONE audit note that carries the triage reason.
/// Changes nothing else. Pure: no I/O, no event.
// trace:STORY-1429 | ai:claude
pub(crate) fn return_to_flight(
    req: &mut Requirement,
    expected: &RequirementStatus,
    target: &RequirementStatus,
    ctx: &ReturnCtx,
) -> ReturnOutcome {
    if &req.status != expected {
        if &req.status == target {
            return ReturnOutcome::AlreadyInFlight {
                status: req.status.clone(),
                since: req.modified_at,
            };
        }
        return ReturnOutcome::StatusMoved {
            expected: expected.clone(),
            actual: req.status.clone(),
        };
    }
    let from = req.status.clone();
    req.set_status_from_str(&format!("{target:?}"));
    let cleared = if from == RequirementStatus::NeedsAttention
        && req.status != RequirementStatus::NeedsAttention
    {
        clear_shelve_markers(req, ctx.clear_escalation)
    } else {
        ClearedMarkers::default()
    };
    let note = cleared.audit_note(&ctx.via, &req.status.to_string(), ctx.reason.as_deref());
    req.add_comment(aida_core::Comment::new(ctx.author.clone(), note));
    req.modified_at = chrono::Utc::now();
    ReturnOutcome::Returned {
        from,
        to: req.status.clone(),
        cleared,
    }
}

/// Run [`return_to_flight`] on the copy of `id` held by `store`: the body of
/// the file-backed stores' locked `update_atomically` closure. Returns the outcome and, when
/// the spec exists, the resulting copy.
// trace:STORY-1429 | ai:claude
pub(crate) fn return_in_store(
    store: &mut aida_core::RequirementsStore,
    id: uuid::Uuid,
    expected: &RequirementStatus,
    target: &RequirementStatus,
    ctx: &ReturnCtx,
) -> (ReturnOutcome, Option<Requirement>) {
    match store.requirements.iter_mut().find(|r| r.id == id) {
        Some(r) => {
            let outcome = return_to_flight(r, expected, target, ctx);
            (outcome, Some(r.clone()))
        }
        None => (ReturnOutcome::Missing, None),
    }
}

/// What a status re-read just before a write says about an applied return.
/// `None` when the spec is still at `expected` and the write may proceed.
// trace:STORY-1429 | ai:claude
pub(crate) fn recheck_before_write(
    fresh: Option<&Requirement>,
    expected: &RequirementStatus,
    target: &RequirementStatus,
) -> Option<ReturnOutcome> {
    match fresh {
        None => Some(ReturnOutcome::Missing),
        Some(f) if &f.status == expected => None,
        Some(f) if &f.status == target => Some(ReturnOutcome::AlreadyInFlight {
            status: f.status.clone(),
            since: f.modified_at,
        }),
        Some(f) => Some(ReturnOutcome::StatusMoved {
            expected: expected.clone(),
            actual: f.status.clone(),
        }),
    }
}

/// The targeted single-spec door (`aida edit`'s sibling helpers, the
/// supervisor, and `queue rework` / the MCP tool on the git store). Reads the
/// one spec, runs the owner on it, re-reads and compares the status just
/// before the write, then writes that one spec. The cleared failure is part
/// of that same write. It never loads or saves the whole store (a lock-free
/// whole-store save on the git store can drop concurrently added specs).
// trace:STORY-1429 | ai:claude
pub(crate) fn return_to_flight_in_backend<B: aida_core::DatabaseBackend>(
    backend: &B,
    id: uuid::Uuid,
    expected: &RequirementStatus,
    target: &RequirementStatus,
    ctx: &ReturnCtx,
) -> anyhow::Result<(ReturnOutcome, Option<Requirement>)> {
    let Some(mut r) = backend.get_requirement(&id)? else {
        return Ok((ReturnOutcome::Missing, None));
    };
    let outcome = return_to_flight(&mut r, expected, target, ctx);
    if !outcome.applied() {
        return Ok((outcome, Some(r)));
    }
    let fresh = backend.get_requirement(&id)?;
    if let Some(moved) = recheck_before_write(fresh.as_ref(), expected, target) {
        return Ok((moved, fresh));
    }
    backend.update_requirement(&r)?;
    Ok((outcome, Some(r)))
}

/// The `Storage` door (CLI `queue rework`, the MCP tool). On the git store it
/// takes the targeted single-spec path above; the file-backed stores keep
/// their locked `update_atomically`, where the owner runs on the copy read
/// under the lock.
// trace:STORY-1429 | ai:claude
pub(crate) fn return_to_flight_in_storage(
    storage: &aida_core::Storage,
    id: uuid::Uuid,
    expected: &RequirementStatus,
    target: &RequirementStatus,
    ctx: &ReturnCtx,
) -> anyhow::Result<(ReturnOutcome, Option<Requirement>)> {
    if storage.path().is_dir() {
        let backend = crate::queue_cmd::advance_backend(storage.path())?;
        return return_to_flight_in_backend(&backend, id, expected, target, ctx);
    }
    let mut result = (ReturnOutcome::Missing, None);
    storage.update_atomically(|s| {
        result = return_in_store(s, id, expected, target, ctx);
    })?;
    Ok(result)
}

/// Build the `SpecRequeued` record for an applied return. `None` for any
/// outcome that changed nothing, and for an exit that does not go back into
/// flight (dropping a spec to Rejected, Superseded or Draft is recorded by
/// the disposition event, not as a requeue).
// trace:STORY-1429 | ai:claude
pub(crate) fn requeued_event(
    spec: &str,
    ctx: &ReturnCtx,
    outcome: &ReturnOutcome,
) -> Option<crate::events::Event> {
    let ReturnOutcome::Returned { from, to, cleared } = outcome else {
        return None;
    };
    if !matches!(
        to,
        RequirementStatus::Approved | RequirementStatus::Planned | RequirementStatus::InProgress
    ) {
        return None;
    }
    let mut ev = crate::events::Event::new(
        Some(spec.to_string()),
        "",
        crate::events::EventKind::SpecRequeued {
            via: ctx.via_slug.to_string(),
            actor: Some(ctx.author.clone()).filter(|a| !a.trim().is_empty()),
            from: from.to_string(),
            to: to.to_string(),
            cleared_tags: cleared.removed_tags.clone(),
            kept_tags: cleared.kept_tags.clone(),
        },
    );
    ev.seat = crate::events::active_seat();
    Some(ev)
}

/// Emit `SpecRequeued` for an applied return. The transition is already
/// committed; the event is telemetry, so a failed emit is only logged.
// trace:STORY-1429 | ai:claude
pub(crate) fn emit_requeued(
    project_root: &std::path::Path,
    spec: &str,
    ctx: &ReturnCtx,
    outcome: &ReturnOutcome,
) {
    if let Some(ev) = requeued_event(spec, ctx, outcome) {
        crate::events::emit(project_root, &ev);
    }
}

/// The user-facing line for a return that changed nothing. `None` for an
/// applied return.
// trace:STORY-1429 | ai:claude
pub(crate) fn unchanged_message(id: &str, outcome: &ReturnOutcome) -> Option<String> {
    match outcome {
        ReturnOutcome::Returned { .. } => None,
        ReturnOutcome::AlreadyInFlight { status, since } => Some(format!(
            "{id} is already {status} (since {}); already requeued, nothing changed",
            since.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")
        )),
        ReturnOutcome::StatusMoved { expected, actual } => Some(format!(
            "{id} moved from {expected} to {actual} while this command ran; nothing was \
             changed. Re-check it with `aida show {id}`"
        )),
        ReturnOutcome::Missing => Some(format!("{id} is no longer in the store")),
    }
}

// ---------------------------------------------------------------------------
// The preview: what a requeue WOULD do, shown before it is taken. The same
// preview renders the listing hint and the interactive keystroke, so the two
// cannot disagree. trace:STORY-1429 | ai:claude
// ---------------------------------------------------------------------------

/// Why a parked spec cannot be requeued with one keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotOfferable {
    /// An open decision must be answered first, or the spec lands back on a
    /// parked fork.
    PendingDecision,
    /// Work no agent can do; requeueing it would only park it again.
    HumanOnly,
    /// The spec was handed to a successor.
    Superseded,
    /// The spec is not parked in NeedsAttention.
    NotParked(RequirementStatus),
}

/// What requeueing a parked spec would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequeuePreview {
    /// The resulting status. Always the metadata-only rework target, never a
    /// caller-supplied status.
    pub target: RequirementStatus,
    /// The queue route the spec lands on (`None` when it has no route yet;
    /// rework defaults it to implementer).
    pub role: Option<String>,
    /// An unmet dependency the drain will still wait on after the requeue.
    pub waits_on: Option<String>,
    /// The `needs-human` escalation stays because this caller may not clear
    /// it, so the spec stays parked after the requeue.
    pub keeps_escalation: bool,
    /// Set when `[r]` must not be offered.
    pub not_offerable: Option<NotOfferable>,
}

impl RequeuePreview {
    pub(crate) fn offerable(&self) -> bool {
        self.not_offerable.is_none()
    }

    /// "to Approved, queued for implementer (head), waits on STORY-52 (...)".
    pub(crate) fn summary(&self) -> String {
        let mut s = format!(
            "to {}, queued for {} (head)",
            self.target,
            self.role.as_deref().unwrap_or("implementer")
        );
        if let Some(w) = &self.waits_on {
            s.push_str(&format!(", waits on {w}"));
        }
        if self.keeps_escalation {
            s.push_str("; stays parked until a human at a terminal clears needs-human");
        }
        s
    }
}

/// Preview a requeue of `req`. `store` resolves BlockedBy targets (without it
/// no dependency is reported); `role` is the spec's current queue route.
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_preview(
    req: &Requirement,
    store: Option<&aida_core::RequirementsStore>,
    role: Option<&str>,
    clear_escalation: bool,
) -> RequeuePreview {
    use aida_core::pickability::{pickability, BlockedReason, Pickability};
    let target = crate::rework_target_for_mode(&RequirementStatus::NeedsAttention, false)
        .unwrap_or(RequirementStatus::Approved);
    let mut after = req.clone();
    after.status = target.clone();
    let (human_only, waits_on) = match store {
        Some(store) => match pickability(&after, store) {
            Pickability::Blocked(BlockedReason::HumanOnly) => (true, None),
            Pickability::Blocked(BlockedReason::UnsatisfiedBlocker {
                target_spec,
                target_status,
            }) => (false, Some(format!("{target_spec} ({target_status})"))),
            Pickability::Blocked(BlockedReason::PermanentlyBlocked { target_spec }) => {
                (false, Some(format!("{target_spec} (Rejected)")))
            }
            _ => (false, None),
        },
        None => (req.human_only, None),
    };
    let pending_decision = req
        .decision_request
        .as_ref()
        .is_some_and(|d| d.is_pending());
    let not_offerable = if req.status == RequirementStatus::Superseded {
        Some(NotOfferable::Superseded)
    } else if req.status != RequirementStatus::NeedsAttention {
        Some(NotOfferable::NotParked(req.status.clone()))
    } else if pending_decision {
        Some(NotOfferable::PendingDecision)
    } else if human_only {
        Some(NotOfferable::HumanOnly)
    } else {
        None
    };
    let keeps_escalation = !clear_escalation
        && req.tags.iter().any(|t| {
            SHELVE_MARKER_TAGS
                .iter()
                .any(|m| t.trim().eq_ignore_ascii_case(m))
        });
    RequeuePreview {
        target,
        role: role.map(str::to_string),
        waits_on,
        keeps_escalation,
        not_offerable,
    }
}

/// The requeue hint shown next to a NeedsAttention spec. It names the
/// resulting status (from the preview) before the command is run. A spec with
/// an open decision must have the decision answered first, otherwise the
/// requeue lands it back on a parked spec.
pub(crate) fn requeue_hint(id: &str, preview: &RequeuePreview) -> String {
    match &preview.not_offerable {
        Some(NotOfferable::PendingDecision) => format!(
            "answer its open decision first (`aida decide {id}`), then `{}` ({})",
            requeue_command(id),
            preview.summary()
        ),
        Some(NotOfferable::HumanOnly) => format!(
            "human-only work: finish it by hand, or drop it with `aida edit {id} --status rejected`"
        ),
        Some(NotOfferable::Superseded) => {
            format!("superseded: rework its successor instead (`aida show {id}`)")
        }
        Some(NotOfferable::NotParked(status)) => format!("{id} is {status}, not parked"),
        None => format!("requeue: `{}` ({})", requeue_command(id), preview.summary()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shelved_req() -> Requirement {
        let mut r = Requirement::new("t".into(), String::new());
        r.spec_id = Some("TASK-9001".into());
        r.status = aida_core::RequirementStatus::NeedsAttention;
        r.failure_reason = Some(aida_core::FailureReason {
            phase: "ci".into(),
            phase_index: 2,
            kind: "ci-red".into(),
            detail: "clippy failed\nmore".into(),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        });
        r.tags.insert("needs-human".into());
        r.tags.insert("needs-decision".into());
        r.tags.insert("batch:x".into());
        r
    }

    #[test]
    fn clears_failure_reason_and_escalation_tag_only() {
        let mut r = shelved_req();
        let cleared = clear_shelve_markers(&mut r, true);
        assert!(r.failure_reason.is_none());
        assert!(r.attention_reason.is_none());
        assert!(!r.tags.contains("needs-human"));
        // Hand-applied intent tags survive.
        assert!(r.tags.contains("needs-decision"));
        assert!(r.tags.contains("batch:x"));
        assert_eq!(cleared.removed_tags, vec!["needs-human".to_string()]);
        assert_eq!(
            cleared.failure_summary.as_deref(),
            Some("ci/ci-red: clippy failed")
        );
    }

    #[test]
    fn triaged_spec_re_enters_the_drain_ready_set() {
        let mut r = shelved_req();
        r.tags.remove("needs-decision");
        r.status = aida_core::RequirementStatus::Approved;
        let tags: Vec<String> = r.tags.iter().cloned().collect();
        assert!(
            crate::burndown::parking_tag(&tags).is_some(),
            "precondition: the escalation tag parks the spec"
        );
        clear_shelve_markers(&mut r, true);
        let tags: Vec<String> = r.tags.iter().cloned().collect();
        assert_eq!(crate::burndown::parking_tag(&tags), None);
    }

    #[test]
    fn pending_decision_is_left_in_place() {
        let mut r = shelved_req();
        r.decision_request = Some(aida_core::DecisionRequest {
            question: "which fork?".into(),
            choices: Vec::new(),
            recommended: None,
            rationale: None,
            answered: None,
            note: None,
            asked_at: None,
            answered_at: None,
        });
        clear_shelve_markers(&mut r, true);
        assert!(r.decision_request.as_ref().unwrap().is_pending());
    }

    #[test]
    fn audit_note_records_why_it_returned() {
        let mut r = shelved_req();
        let cleared = clear_shelve_markers(&mut r, true);
        let note = cleared.audit_note("`aida queue rework`", "Approved", Some("fixed CI"));
        assert!(note.contains("ci/ci-red: clippy failed"), "{note}");
        assert!(note.contains("needs-human"), "{note}");
        assert!(note.contains("Triage reason: fixed CI"), "{note}");
        let bare = ClearedMarkers::default().audit_note("x", "Approved", None);
        assert!(bare.contains("No triage reason was given"), "{bare}");
    }

    /// Only a human at a terminal may undo an escalation. A non-TTY advisor
    /// and an orchestrated caller both hold advisor authority, and both must
    /// keep the tag.
    #[test]
    fn only_a_tty_human_may_clear_the_escalation() {
        assert!(may_clear_escalation(true));
        // Non-TTY advisor agent, and an orchestrated drain phase: both hold
        // advisor authority but no human is present.
        assert!(crate::advisor_authority_from("advisor", false, false));
        assert!(crate::advisor_authority_from("implementer", false, true));
        assert!(!may_clear_escalation(false));
    }

    #[test]
    fn non_human_caller_keeps_the_tag_and_is_warned() {
        let mut r = shelved_req();
        r.tags.remove("needs-decision");
        let cleared = clear_shelve_markers(&mut r, may_clear_escalation(false));
        assert!(r.tags.contains("needs-human"), "{:?}", r.tags);
        assert!(r.failure_reason.is_none(), "shelve metadata still clears");
        assert!(cleared.removed_tags.is_empty());
        assert_eq!(cleared.kept_tags, vec!["needs-human".to_string()]);
        let tags: Vec<String> = r.tags.iter().cloned().collect();
        assert!(
            crate::burndown::parking_tag(&tags).is_some(),
            "still parked"
        );
        let w = kept_escalation_warning("BUG-1", &cleared).unwrap();
        assert!(
            w.contains("stays parked") && w.contains("needs-human"),
            "{w}"
        );
        assert!(cleared
            .audit_note("x", "Approved", None)
            .contains("Kept escalation"));
    }

    #[test]
    fn tty_human_clears_the_tag_without_warning() {
        let mut r = shelved_req();
        let cleared = clear_shelve_markers(&mut r, may_clear_escalation(true));
        assert!(!r.tags.contains("needs-human"));
        assert_eq!(kept_escalation_warning("BUG-1", &cleared), None);
    }

    fn parked_preview(r: &Requirement) -> RequeuePreview {
        requeue_preview(r, None, Some("implementer"), true)
    }

    #[test]
    fn hint_names_the_resulting_status_and_decision_first() {
        let mut r = shelved_req();
        let h = requeue_hint("BUG-1", &parked_preview(&r));
        assert!(
            h.contains("aida queue rework BUG-1") && h.contains("Approved"),
            "{h}"
        );
        r.decision_request = Some(pending_decision());
        let d = requeue_hint("BUG-1", &parked_preview(&r));
        assert!(d.starts_with("answer its open decision first"), "{d}");
    }

    fn pending_decision() -> aida_core::DecisionRequest {
        aida_core::DecisionRequest {
            question: "which fork?".into(),
            choices: Vec::new(),
            recommended: None,
            rationale: None,
            answered: None,
            note: None,
            asked_at: None,
            answered_at: None,
        }
    }

    fn ctx(reason: Option<&str>) -> ReturnCtx {
        ReturnCtx {
            via: "`aida queue rework`".into(),
            via_slug: "queue-rework",
            author: "tester".into(),
            clear_escalation: true,
            reason: reason.map(str::to_string),
        }
    }

    // STORY-1429: the owner compares against the copy it is handed. A spec
    // that moved (a drain claimed it) is left exactly as it was.
    // trace:STORY-1429 | ai:claude
    #[test]
    fn return_to_flight_cas_refuses_when_status_moved() {
        let mut r = shelved_req();
        r.status = RequirementStatus::InProgress;
        let before = r.clone();
        let out = return_to_flight(
            &mut r,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(Some("why")),
        );
        assert_eq!(
            out,
            ReturnOutcome::StatusMoved {
                expected: RequirementStatus::NeedsAttention,
                actual: RequirementStatus::InProgress,
            }
        );
        assert!(!out.applied());
        assert_eq!(r.status, before.status);
        assert_eq!(r.comments.len(), before.comments.len(), "no note written");
        assert_eq!(r.tags, before.tags, "no marker cleared");
        assert!(r.failure_reason.is_some(), "no marker cleared");
        assert!(requeued_event("TASK-9001", &ctx(None), &out).is_none());
        let msg = unchanged_message("TASK-9001", &out).unwrap();
        assert!(
            msg.contains("In Progress") && msg.contains("nothing was"),
            "{msg}"
        );
    }

    // trace:STORY-1429 | ai:claude
    #[test]
    fn double_requeue_is_noop_with_one_audit_comment() {
        let mut r = shelved_req();
        let first = return_to_flight(
            &mut r,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(Some("fixed CI")),
        );
        assert!(first.applied());
        assert_eq!(r.status, RequirementStatus::Approved);
        let second = return_to_flight(
            &mut r,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(Some("fixed CI")),
        );
        assert!(matches!(second, ReturnOutcome::AlreadyInFlight { .. }));
        assert_eq!(r.comments.len(), 1, "exactly one audit note");
        // The reason lives in the audit note, once.
        let with_reason = r
            .comments
            .iter()
            .filter(|c| c.content.contains("fixed CI"))
            .count();
        assert_eq!(with_reason, 1);
        assert!(requeued_event("TASK-9001", &ctx(None), &second).is_none());
        assert!(unchanged_message("TASK-9001", &second)
            .unwrap()
            .contains("already requeued, nothing changed"));
    }

    // The door body runs on the copy INSIDE the store the atomic write loaded,
    // not on the caller's earlier read. trace:STORY-1429 | ai:claude
    #[test]
    fn door_status_check_reads_the_copy_under_the_write() {
        let parked = shelved_req();
        let id = parked.id;
        let mut store = aida_core::RequirementsStore::default();
        let mut moved = parked.clone();
        moved.status = RequirementStatus::InProgress;
        store.requirements.push(moved);
        // The caller read NeedsAttention earlier; the store now says InProgress.
        let (out, copy) = return_in_store(
            &mut store,
            id,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(None),
        );
        assert!(matches!(out, ReturnOutcome::StatusMoved { .. }));
        assert_eq!(copy.unwrap().status, RequirementStatus::InProgress);
        assert!(store.requirements[0].comments.is_empty());
        let (missing, none) = return_in_store(
            &mut store,
            uuid::Uuid::new_v4(),
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(None),
        );
        assert_eq!(missing, ReturnOutcome::Missing);
        assert!(none.is_none());
    }

    // The re-read just before a targeted write: the write proceeds only
    // while the spec is still at `expected`. trace:STORY-1429 | ai:claude
    #[test]
    fn recheck_before_write_refuses_a_moved_spec() {
        let na = RequirementStatus::NeedsAttention;
        let ap = RequirementStatus::Approved;
        let mut r = shelved_req();
        assert_eq!(recheck_before_write(Some(&r), &na, &ap), None);
        r.status = RequirementStatus::InProgress;
        assert!(matches!(
            recheck_before_write(Some(&r), &na, &ap),
            Some(ReturnOutcome::StatusMoved { .. })
        ));
        r.status = ap.clone();
        assert!(matches!(
            recheck_before_write(Some(&r), &na, &ap),
            Some(ReturnOutcome::AlreadyInFlight { .. })
        ));
        assert_eq!(
            recheck_before_write(None, &na, &ap),
            Some(ReturnOutcome::Missing)
        );
    }

    // trace:STORY-1429 | ai:claude
    #[test]
    fn applied_return_builds_a_spec_requeued_event() {
        let mut r = shelved_req();
        let out = return_to_flight(
            &mut r,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Approved,
            &ctx(None),
        );
        let mut dropped = shelved_req();
        let drop_out = return_to_flight(
            &mut dropped,
            &RequirementStatus::NeedsAttention,
            &RequirementStatus::Rejected,
            &ctx(None),
        );
        assert!(drop_out.applied(), "the owner covers every exit");
        assert!(
            requeued_event("TASK-9001", &ctx(None), &drop_out).is_none(),
            "dropping a spec is not a requeue"
        );
        let ev = requeued_event("TASK-9001", &ctx(None), &out).unwrap();
        match ev.kind {
            crate::events::EventKind::SpecRequeued {
                via,
                from,
                to,
                cleared_tags,
                kept_tags,
                ..
            } => {
                assert_eq!(via, "queue-rework");
                assert_eq!(from, RequirementStatus::NeedsAttention.to_string());
                assert_eq!(to, RequirementStatus::Approved.to_string());
                assert_eq!(cleared_tags, vec!["needs-human".to_string()]);
                assert!(kept_tags.is_empty());
            }
            other => panic!("expected SpecRequeued, got {other:?}"),
        }
    }

    // trace:STORY-1429 | ai:claude
    #[test]
    fn preview_names_target_role_blocker_and_decision() {
        let mut blocker = Requirement::new("blocker".into(), String::new());
        blocker.spec_id = Some("STORY-52".into());
        blocker.status = RequirementStatus::NeedsAttention;
        let mut r = shelved_req();
        r.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::BlockedBy,
            target_id: blocker.id,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        });
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(blocker);
        store.requirements.push(r.clone());

        let p = requeue_preview(&r, Some(&store), Some("reviewer"), true);
        assert_eq!(p.target, RequirementStatus::Approved);
        assert_eq!(p.role.as_deref(), Some("reviewer"));
        assert!(p.offerable(), "a dependency does not block the requeue");
        let waits = p.waits_on.clone().unwrap();
        assert!(waits.starts_with("STORY-52"), "{waits}");
        let summary = p.summary();
        assert!(
            summary.contains("to Approved")
                && summary.contains("queued for reviewer (head)")
                && summary.contains("waits on STORY-52"),
            "{summary}"
        );
        // A caller that may not clear the escalation is told the spec stays
        // parked.
        let kept = requeue_preview(&r, Some(&store), None, false);
        assert!(kept.keeps_escalation);
        assert!(kept.summary().contains("queued for implementer"));
        assert!(kept.summary().contains("stays parked"));

        r.decision_request = Some(pending_decision());
        let d = requeue_preview(&r, Some(&store), None, true);
        assert_eq!(d.not_offerable, Some(NotOfferable::PendingDecision));
    }

    // trace:STORY-1429 | ai:claude
    #[test]
    fn preview_not_offerable_for_pending_decision_or_human_only() {
        let mut r = shelved_req();
        r.decision_request = Some(pending_decision());
        assert_eq!(
            parked_preview(&r).not_offerable,
            Some(NotOfferable::PendingDecision)
        );
        let mut h = shelved_req();
        h.human_only = true;
        let store = aida_core::RequirementsStore {
            requirements: vec![h.clone()],
            ..Default::default()
        };
        let p = requeue_preview(&h, Some(&store), None, true);
        assert_eq!(p.not_offerable, Some(NotOfferable::HumanOnly));
        assert!(requeue_hint("TASK-9001", &p).starts_with("human-only work"));
        let mut s = shelved_req();
        s.status = RequirementStatus::Superseded;
        assert_eq!(
            parked_preview(&s).not_offerable,
            Some(NotOfferable::Superseded)
        );
        let mut a = shelved_req();
        a.status = RequirementStatus::Approved;
        assert!(matches!(
            parked_preview(&a).not_offerable,
            Some(NotOfferable::NotParked(_))
        ));
    }
}
