// trace:TASK-3428 | ai:codex
use crate::RealPhaseDriver;

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
