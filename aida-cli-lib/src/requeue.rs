//! The NeedsAttention -> back-in-flight transition.
//!
//! A drain parks a spec in NeedsAttention (a punt, an orchestrator shelve, or
//! an advisor escalation) and moves on. Triage then returns it to Approved or
//! In Progress, either through `aida edit <ID> --status approved` or through the
//! one-keystroke `aida queue rework <ID>`. Both doors are gated on advisor
//! authority at the call site; this module owns what happens to the spec once
//! the gate has passed.
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

use aida_core::Requirement;

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

/// The requeue hint shown next to a NeedsAttention spec. It names the
/// resulting status before the command is run. A spec with an open decision
/// must have the decision answered first, otherwise the requeue lands it back
/// on a parked spec.
pub(crate) fn requeue_hint(id: &str, has_pending_decision: bool) -> String {
    if has_pending_decision {
        format!(
            "answer its open decision first (`aida questions`), then `{}` (to Approved, back on the queue)",
            requeue_command(id)
        )
    } else {
        format!(
            "requeue: `{}` (to Approved, back on the queue)",
            requeue_command(id)
        )
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

    #[test]
    fn hint_names_the_resulting_status_and_decision_first() {
        let h = requeue_hint("BUG-1", false);
        assert!(h.contains("aida queue rework BUG-1") && h.contains("Approved"));
        let d = requeue_hint("BUG-1", true);
        assert!(d.starts_with("answer its open decision first"), "{d}");
    }
}
