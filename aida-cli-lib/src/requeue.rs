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
//! stale tag. That tag is now cleared here.
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
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => note.push_str(&format!(" Triage reason: {r}")),
            None => note.push_str(" No triage reason was given."),
        }
        note
    }
}

/// Clear the markers a shelve, punt or escalation left on `req`, so a spec
/// triaged out of NeedsAttention re-enters the drain's candidate set. The
/// caller has already changed the status and passed the advisor-authority
/// gate. A pending decision request is left untouched on purpose (see the
/// module docs).
pub(crate) fn clear_shelve_markers(req: &mut Requirement) -> ClearedMarkers {
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
    for t in &removed_tags {
        req.tags.remove(t);
    }
    ClearedMarkers {
        removed_tags,
        failure_summary,
        punt_summary,
    }
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
        let cleared = clear_shelve_markers(&mut r);
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
        clear_shelve_markers(&mut r);
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
        clear_shelve_markers(&mut r);
        assert!(r.decision_request.as_ref().unwrap().is_pending());
    }

    #[test]
    fn audit_note_records_why_it_returned() {
        let mut r = shelved_req();
        let cleared = clear_shelve_markers(&mut r);
        let note = cleared.audit_note("`aida queue rework`", "Approved", Some("fixed CI"));
        assert!(note.contains("ci/ci-red: clippy failed"), "{note}");
        assert!(note.contains("needs-human"), "{note}");
        assert!(note.contains("Triage reason: fixed CI"), "{note}");
        let bare = ClearedMarkers::default().audit_note("x", "Approved", None);
        assert!(bare.contains("No triage reason was given"), "{bare}");
    }

    #[test]
    fn hint_names_the_resulting_status_and_decision_first() {
        let h = requeue_hint("BUG-1", false);
        assert!(h.contains("aida queue rework BUG-1") && h.contains("Approved"));
        let d = requeue_hint("BUG-1", true);
        assert!(d.starts_with("answer its open decision first"), "{d}");
    }
}
