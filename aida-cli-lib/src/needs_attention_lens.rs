//! TASK-2003 (option A): a display-only lens for `aida list`. A review
//! refusal or merge hold parks a spec at NeedsAttention until the PR merges,
//! so a spec whose rework is pushed and awaiting review looked the same as an
//! abandoned one. This reads local evidence only — the spec's Rework hold, the
//! newer PR head lane-liveness recorded, and the verdict's recorded rounds —
//! and labels the row `in review: PR-N @sha8, round k`. Every other
//! NeedsAttention row shows how long ago it last changed. Nothing is written
//! to the store: the status stays NeedsAttention (BUG-1494 keeps the flip back
//! to In Progress an advisor decision), so a new refusal needs no undo.
// trace:TASK-1951 trace:TASK-2003 | ai:claude

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::{DateTime, Utc};

/// The Rework hold standing on a spec: its PR and the head it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecHold {
    pub(crate) pr: u64,
    pub(crate) target_head: Option<String>,
    /// The verdict record the refusal lives under (spec id or `PR-N`).
    pub(crate) verdict_key: String,
}

/// The label for one NeedsAttention row. `new_heads` maps a PR to the head
/// lane-liveness saw pushed past its hold; `rounds` counts the reviews
/// already recorded under the hold's verdict, so the head awaiting review is
/// round `rounds + 1`.
pub(crate) fn lens_label(
    hold: Option<&SpecHold>,
    new_heads: &BTreeMap<u64, String>,
    rounds: usize,
    modified_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<String> {
    if let Some(hold) = hold {
        if let Some(head) = new_heads.get(&hold.pr) {
            let refused_at_head = hold.target_head.as_deref().is_some_and(|target| {
                let n = head.len().min(target.len());
                n >= 7 && head[..n].eq_ignore_ascii_case(&target[..n])
            });
            if !refused_at_head {
                return Some(format!(
                    "in review: PR-{} @{head}, round {}",
                    hold.pr,
                    rounds + 1
                ));
            }
        }
    }
    let modified = modified_at?;
    let age = (now - modified).num_seconds();
    Some(format!(
        "last activity {}",
        crate::last_drain::format_age(age)
    ))
}

fn is_needs_attention(status: &str) -> bool {
    let s: String = status
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    s.eq_ignore_ascii_case("needsattention")
}

/// Rework holds keyed by upper-cased spec id, read from the primary's
/// markers. A spec held on several PRs shows one hold: the one whose rework
/// head is ready for review, else the newest PR ([`prefer_hold`]).
// trace:TASK-2251 | ai:claude
fn rework_holds_by_spec(root: &Path, ready: &BTreeMap<u64, String>) -> HashMap<String, SpecHold> {
    let mut out = HashMap::new();
    for (pr, _) in crate::gate::merge_hold_core::list_holds(root) {
        let Some(record) = crate::gate::merge_hold_core::read_hold_record(root, pr) else {
            continue;
        };
        if record.reason_kind != crate::gate::merge_hold_core::HoldReasonKind::Rework {
            continue;
        }
        let Some(spec) = record.spec.as_deref().filter(|s| !s.starts_with("PR-")) else {
            continue;
        };
        let verdict_key = record
            .verdict_ref
            .as_ref()
            .map_or_else(|| spec.to_string(), |r| r.key.clone());
        let candidate = SpecHold {
            pr,
            target_head: record.target_head_sha.clone(),
            verdict_key,
        };
        out.entry(spec.to_ascii_uppercase())
            .and_modify(|current: &mut SpecHold| {
                if prefer_hold(&candidate, current, ready) {
                    *current = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    out
}

/// Whether `candidate` should replace `current` as a spec's displayed hold:
/// a hold with a ready rework head beats one without, then the newer PR wins.
// trace:TASK-2251 | ai:claude
fn prefer_hold(candidate: &SpecHold, current: &SpecHold, ready: &BTreeMap<u64, String>) -> bool {
    let rank = |h: &SpecHold| (ready.contains_key(&h.pr), h.pr);
    rank(candidate) > rank(current)
}

/// The lens label of every NeedsAttention row, keyed by requirement id.
/// Empty when there is no project root or no NeedsAttention row.
pub(crate) fn lens_map(
    project_root: Option<&Path>,
    reqs: &[aida_core::RequirementSummary],
) -> HashMap<uuid::Uuid, String> {
    let mut out = HashMap::new();
    let Some(project_root) = project_root else {
        return out;
    };
    if !reqs.iter().any(|r| is_needs_attention(&r.status)) {
        return out;
    }
    let root = crate::gate::merge_hold_core::hold_store_root(project_root);
    let new_heads = crate::lane_liveness::unpicked_ready_heads(&root);
    let holds = rework_holds_by_spec(&root, &new_heads);
    let now = Utc::now();
    for r in reqs.iter().filter(|r| is_needs_attention(&r.status)) {
        let hold = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|id| holds.get(&id.to_ascii_uppercase()));
        let rounds = hold
            .filter(|h| new_heads.contains_key(&h.pr))
            .and_then(|h| {
                crate::gate::review_verdict::archived_recordings(
                    &crate::gate::review_verdict::verdict_path(&root, &h.verdict_key),
                )
                .ok()
            })
            .map_or(0, |recordings| recordings.len());
        let modified = DateTime::parse_from_rfc3339(&r.modified_at)
            .ok()
            .map(|t| t.with_timezone(&Utc));
        if let Some(label) = lens_label(hold, &new_heads, rounds, modified, now) {
            out.insert(r.id, label);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold() -> SpecHold {
        SpecHold {
            pr: 2638,
            target_head: Some("85c6036d67aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            verdict_key: "PR-2638".into(),
        }
    }

    /// A rework head pushed past the refusal reads as in review, with its
    /// PR, head and round; the refused head itself, or no recorded new
    /// head, reads as the age of the last change.
    // trace:TASK-1951 trace:TASK-2003 | ai:claude
    #[test]
    fn lens_names_the_pr_head_and_round_or_the_last_activity() {
        let now: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
        let modified = Some("2026-10-22T07:00:00Z".parse().unwrap());
        let mut heads = BTreeMap::new();
        heads.insert(2638, "c0bad0b0".to_string());
        assert_eq!(
            lens_label(Some(&hold()), &heads, 2, modified, now).as_deref(),
            Some("in review: PR-2638 @c0bad0b0, round 3")
        );
        heads.insert(2638, "85c6036d".to_string());
        assert_eq!(
            lens_label(Some(&hold()), &heads, 2, modified, now).as_deref(),
            Some("last activity 5h ago")
        );
        assert_eq!(
            lens_label(None, &BTreeMap::new(), 0, modified, now).as_deref(),
            Some("last activity 5h ago")
        );
        assert_eq!(lens_label(None, &BTreeMap::new(), 0, None, now), None);
    }

    /// A spec held on two PRs shows the hold whose rework head is ready,
    /// else the newer PR, whatever order the markers are listed in.
    // trace:TASK-2251 | ai:claude
    #[test]
    fn a_spec_held_on_two_prs_shows_the_ready_or_newest_hold() {
        let older = hold();
        let newer = SpecHold { pr: 2700, ..hold() };
        let none = BTreeMap::new();
        assert!(prefer_hold(&newer, &older, &none));
        assert!(!prefer_hold(&older, &newer, &none));
        let mut ready = BTreeMap::new();
        ready.insert(2638, "c0bad0b0".to_string());
        assert!(prefer_hold(&older, &newer, &ready));
        assert!(!prefer_hold(&newer, &older, &ready));
    }

    /// The map reads the hold marker, lane-liveness's routed findings and the
    /// verdict archive from disk, and only for NeedsAttention rows.
    // trace:TASK-1951 trace:TASK-2003 | ai:claude
    #[test]
    fn lens_map_reads_the_hold_routed_head_and_rounds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut record = crate::gate::merge_hold_core::typed_hold(
            2638,
            crate::gate::merge_hold_core::HoldReasonKind::Rework,
            "CHANGES REQUESTED for TASK-77",
            Some("85c6036d67aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        );
        record.spec = Some("TASK-77".into());
        crate::gate::merge_hold_core::write_typed_hold(root, &record).unwrap();
        let routed = root.join(".aida/lane-liveness/routed.json");
        std::fs::create_dir_all(routed.parent().unwrap()).unwrap();
        std::fs::write(
            &routed,
            r#"{"lane-liveness:unpicked-ready-pr:PR-2638@c0bad0b0":"2026-10-22T07:00:00Z"}"#,
        )
        .unwrap();
        crate::gate::review_verdict::record_verdict(
            root,
            "TASK-77",
            Some("request-changes"),
            Some("85c6036d67aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            None,
            Some("blocking"),
            &[],
            "reviewer-a",
        )
        .unwrap();

        let row = |spec: &str, status: &str| aida_core::RequirementSummary {
            id: uuid::Uuid::new_v4(),
            spec_id: Some(spec.to_string()),
            agreed_id: None,
            title: String::new(),
            description: String::new(),
            status: status.to_string(),
            priority: "medium".to_string(),
            owner: String::new(),
            assignee: None,
            feature: String::new(),
            req_type: "Task".to_string(),
            tags: Vec::new(),
            created_at: String::new(),
            modified_at: "2026-10-22T07:00:00Z".to_string(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            deferred_reason: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            blocked: false,
            has_pending_decision: false,
            execution_mode: None,
            weight: None,
            origin: None,
            completed_at: None,
            yaml_path: String::new(),
        };
        let held = row("TASK-77", "NeedsAttention");
        let other = row("TASK-78", "Needs Attention");
        let open = row("TASK-79", "InProgress");
        let map = lens_map(Some(root), &[held.clone(), other.clone(), open.clone()]);
        assert_eq!(
            map.get(&held.id).map(String::as_str),
            Some("in review: PR-2638 @c0bad0b0, round 2")
        );
        assert!(map[&other.id].starts_with("last activity "), "{map:?}");
        assert!(!map.contains_key(&open.id));
    }
}
