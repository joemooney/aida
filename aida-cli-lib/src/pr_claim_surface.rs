//! STORY-1416: write-triggered, entity-scoped surfacing for PR claims.
//!
//! Writing a claim ABOUT a pull request — placing a merge-hold marker, or
//! recording a review verdict with `--pr` — first shows what is already on
//! record for that PR: the existing marker (its text and who placed it) and
//! every verdict recorded under a key the PR is known by (with its seat and
//! sha). The gap this closes is consultation, not durability: the
//! contradicting fact was always on disk, it was just never read at the moment
//! of the write.
//!
//! Precision rule (acceptance criterion 2): a collision is flagged only when
//! the marker's text ASSERTS something about a verdict (a verdict kind, a
//! reviewed sha, a recording seat) and a verdict disagrees with it. A marker
//! that says it is WAITING on a verdict makes no such claim, so the verdict
//! arriving confirms it rather than contradicting it — timing alone never
//! fires.
//!
//! Informational only (criterion 3): nothing here refuses or delays a write.
//! Scope (criterion 5): merge-hold markers and PR-keyed verdicts only.
// trace:STORY-1416 | ai:claude

use std::path::Path;

use crate::merge_hold::{self, MergeHoldRecord};
use crate::review_verdict::{self, RecordedVerdict, VerdictKind};

/// What is on record for one PR at the moment of a write.
#[derive(Debug, Clone, Default)]
pub(crate) struct PrRecord {
    pub marker: Option<MergeHoldRecord>,
    /// When the marker file was last written (for ordering against verdicts).
    pub marker_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `(key, verdict)` for every verdict key the PR is known by.
    pub verdicts: Vec<(String, RecordedVerdict)>,
}

impl PrRecord {
    pub(crate) fn is_empty(&self) -> bool {
        self.marker.is_none() && self.verdicts.is_empty()
    }
}

/// What a marker's text asserts about a verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VerdictClaim {
    pub key: Option<String>,
    pub kind: Option<VerdictKind>,
    pub sha: Option<String>,
    pub recorded_by: Option<String>,
}

/// A verdict about to be recorded (criterion 1b).
#[derive(Debug, Clone)]
pub(crate) struct IncomingVerdict<'a> {
    pub key: &'a str,
    pub kind: VerdictKind,
    pub sha: Option<&'a str>,
    pub recorded_by: &'a str,
}

/// Read the marker and every verdict the PR is known by: `PR-<n>`, the key the
/// marker references, the spec the marker was placed for, and `extra_keys`.
/// Filesystem-only and cheap; no forge or store access.
pub(crate) fn read_pr_record(root: &Path, pr: u64, extra_keys: &[&str]) -> PrRecord {
    let marker = merge_hold::read_hold_record(root, pr);
    let marker_at = std::fs::metadata(merge_hold::hold_path(root, pr))
        .and_then(|m| m.modified())
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from);
    let mut keys: Vec<String> = vec![format!("PR-{pr}")];
    if let Some(m) = &marker {
        if let Some(r) = &m.verdict_ref {
            keys.push(r.key.clone());
        }
        if let Some(s) = &m.spec {
            keys.push(s.clone());
        }
    }
    keys.extend(extra_keys.iter().map(|k| k.to_string()));
    let mut seen = Vec::<String>::new();
    let mut verdicts = Vec::new();
    for key in keys {
        let key = key.trim().to_ascii_uppercase();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key.clone());
        if let Some(v) = review_verdict::read_recorded_verdict(root, &key) {
            verdicts.push((key, v));
        }
    }
    PrRecord {
        marker,
        marker_at,
        verdicts,
    }
}

/// The verdict a marker's text claims, or `None` when it claims none (a
/// pending/waiting marker, a supervision hold with no verdict in it).
pub(crate) fn marker_verdict_claim(marker: &MergeHoldRecord) -> Option<VerdictClaim> {
    let prose = prose_claim(&marker.detail);
    match (&marker.verdict_ref, prose) {
        (Some(r), prose) => Some(VerdictClaim {
            key: Some(r.key.clone()),
            kind: prose.as_ref().and_then(|p| p.kind),
            sha: r
                .reviewed_sha
                .clone()
                .or_else(|| prose.as_ref().and_then(|p| p.sha.clone())),
            recorded_by: r.recorded_by.clone(),
        }),
        (None, prose) => prose,
    }
}

/// Parse a verdict assertion out of marker prose. A marker that says it is
/// waiting/pending on a verdict asserts nothing about one.
fn prose_claim(detail: &str) -> Option<VerdictClaim> {
    let lower = detail.to_ascii_lowercase();
    if ["waiting", "awaiting", "pending", "until"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return None;
    }
    let kind = if [
        "changes requested",
        "request-changes",
        "request changes",
        "requested changes",
        "request_changes",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        VerdictKind::RequestChanges
    } else if lower.contains("rejected") {
        VerdictKind::Rejected
    } else if lower.contains("approved") {
        VerdictKind::Approved
    } else {
        return None;
    };
    // A sha is cited as "at <hex>" (the shape `aida review record` writes).
    let words: Vec<&str> = detail.split_whitespace().collect();
    let sha = words.windows(2).find_map(|w| {
        let cand = w[1].trim_matches(|c: char| !c.is_ascii_hexdigit());
        (w[0].eq_ignore_ascii_case("at")
            && cand.len() >= 7
            && cand.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| cand.to_string())
    });
    Some(VerdictClaim {
        key: None,
        kind: Some(kind),
        sha,
        recorded_by: None,
    })
}

/// How a claim disagrees with a verdict, or `None` when it does not.
fn disagreement(
    claim: &VerdictClaim,
    kind: VerdictKind,
    sha: Option<&str>,
    recorded_by: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ck) = claim.kind {
        if kind != VerdictKind::Unknown && ck != kind {
            parts.push(format!(
                "it says {} but the verdict is {}",
                ck.label(),
                kind.label()
            ));
        }
    }
    // The same seat recording the same outcome at a newer head is a later
    // round of the SAME claim, not a contradiction of it.
    let same_seat_same_outcome = parts.is_empty()
        && matches!(
            (claim.recorded_by.as_deref(), recorded_by),
            (Some(cb), Some(vb)) if cb.trim() == vb.trim()
        );
    if let (Some(cs), Some(vs), false) = (claim.sha.as_deref(), sha, same_seat_same_outcome) {
        if !review_verdict::same_reviewed_sha(cs, vs) {
            parts.push(format!(
                "it cites {} but the verdict is at {}",
                review_verdict::short_sha(cs),
                review_verdict::short_sha(vs)
            ));
        }
    }
    if let (Some(cb), Some(vb)) = (claim.recorded_by.as_deref(), recorded_by) {
        if cb.trim() != vb.trim() {
            parts.push(format!(
                "it cites {cb} but the verdict was recorded by {vb}"
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn verdict_is_newer(
    v: &RecordedVerdict,
    marker_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<bool> {
    let at = chrono::DateTime::parse_from_rfc3339(v.recorded_at.as_deref()?.trim()).ok()?;
    Some(at.with_timezone(&chrono::Utc) > marker_at?)
}

fn marker_line(m: &MergeHoldRecord) -> String {
    let first = m.detail.lines().next().unwrap_or("").trim();
    let mut meta = vec![format!(
        "placed by {}",
        m.placed_by.as_deref().unwrap_or("(not recorded)")
    )];
    if let Some(sha) = m
        .target_head_sha
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        meta.push(format!("head {}", review_verdict::short_sha(sha)));
    }
    if let Some(r) = &m.verdict_ref {
        let mut cite = format!("cites verdict {}", r.key);
        if let Some(by) = &r.recorded_by {
            cite.push_str(&format!(" by {by}"));
        }
        if let Some(sha) = &r.reviewed_sha {
            cite.push_str(&format!(" at {}", review_verdict::short_sha(sha)));
        }
        meta.push(cite);
    }
    format!("merge-hold marker: \"{first}\" ({})", meta.join(", "))
}

fn verdict_line(key: &str, v: &RecordedVerdict) -> String {
    format!(
        "verdict {key}: {} at {}, recorded by {}{}",
        v.kind.label(),
        v.reviewed_sha
            .as_deref()
            .map(review_verdict::short_sha)
            .unwrap_or("(no sha)"),
        v.recorded_by.as_deref().unwrap_or("(not recorded)"),
        v.recorded_at
            .as_deref()
            .map(|t| format!(" at {t}"))
            .unwrap_or_default()
    )
}

/// The record lines plus any collision between the standing marker's claim and
/// the standing verdicts (criterion 2: only when the marker claims a verdict).
fn standing_lines(rec: &PrRecord) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = &rec.marker {
        out.push(marker_line(m));
    }
    for (key, v) in &rec.verdicts {
        out.push(verdict_line(key, v));
    }
    if let Some(claim) = rec.marker.as_ref().and_then(marker_verdict_claim) {
        for (key, v) in &rec.verdicts {
            if claim.key.as_deref().is_some_and(|k| k != key) {
                continue;
            }
            if verdict_is_newer(v, rec.marker_at) == Some(false) {
                continue;
            }
            if let Some(why) = disagreement(
                &claim,
                v.kind,
                v.reviewed_sha.as_deref(),
                v.recorded_by.as_deref(),
            ) {
                out.push(format!(
                    "CONTRADICTION: the marker's claim does not match the newer verdict {key} — {why}"
                ));
            }
        }
    }
    out
}

/// Criterion 1a: lines to show before a merge-hold marker is written. Quiet
/// (empty) when the PR has neither a marker nor a verdict on record.
pub(crate) fn lines_for_marker_write(rec: &PrRecord, new_marker: &MergeHoldRecord) -> Vec<String> {
    if rec.is_empty() {
        return Vec::new();
    }
    let mut out = standing_lines(rec);
    if let Some(claim) = marker_verdict_claim(new_marker) {
        for (key, v) in &rec.verdicts {
            if claim.key.as_deref().is_some_and(|k| k != key) {
                continue;
            }
            if let Some(why) = disagreement(
                &claim,
                v.kind,
                v.reviewed_sha.as_deref(),
                v.recorded_by.as_deref(),
            ) {
                out.push(format!(
                    "CONTRADICTION: the marker you are writing does not match verdict {key} — {why}"
                ));
            }
        }
    }
    out
}

/// Criterion 1b: lines to show before a verdict is recorded for a PR. Quiet
/// when there is neither a marker nor a prior verdict.
pub(crate) fn lines_for_verdict_write(rec: &PrRecord, incoming: &IncomingVerdict) -> Vec<String> {
    if rec.is_empty() {
        return Vec::new();
    }
    let mut out = standing_lines(rec);
    if let Some(claim) = rec.marker.as_ref().and_then(marker_verdict_claim) {
        let same_key = claim
            .key
            .as_deref()
            .is_none_or(|k| k.eq_ignore_ascii_case(incoming.key));
        if same_key {
            if let Some(why) = disagreement(
                &claim,
                incoming.kind,
                incoming.sha,
                Some(incoming.recorded_by),
            ) {
                out.push(format!(
                    "CONTRADICTION: the standing marker's claim does not match the verdict you are recording — {why}"
                ));
            }
        }
    }
    out
}

/// Print the surfaced record to stderr. Never fails, never blocks.
pub(crate) fn print(pr: u64, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    eprintln!("  Already on record for PR #{pr}:");
    for line in lines {
        eprintln!("    {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge_hold::{typed_hold, HoldReasonKind, VerdictRef};

    fn write_verdict(root: &Path, key: &str, verdict: &str, sha: &str, by: &str, at: &str) {
        let dir = root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        let body = serde_json::json!({
            "spec": key, "verdict": verdict, "reviewed_sha": sha,
            "recorded_by": by, "recorded_at": at,
        });
        std::fs::write(dir.join(format!("{key}.json")), body.to_string()).unwrap();
    }

    fn rework_marker(pr: u64, detail: &str, by: &str, sha: &str) -> MergeHoldRecord {
        let mut m = typed_hold(pr, HoldReasonKind::Rework, detail, Some(sha.into()));
        m.verdict_ref = Some(VerdictRef::new(
            "STORY-9",
            Some(pr),
            Some(sha.into()),
            Some(by.into()),
        ));
        m.spec = Some("STORY-9".into());
        m.placed_by = Some("driver-advisor".into());
        m
    }

    #[test]
    fn quiet_when_nothing_is_on_record() {
        let dir = tempfile::tempdir().unwrap();
        let rec = read_pr_record(dir.path(), 5, &["STORY-9"]);
        assert!(rec.is_empty());
        let m = typed_hold(5, HoldReasonKind::Supervision, "held", None);
        assert!(lines_for_marker_write(&rec, &m).is_empty());
        let inc = IncomingVerdict {
            key: "STORY-9",
            kind: VerdictKind::Approved,
            sha: Some("abcdef1234"),
            recorded_by: "r",
        };
        assert!(lines_for_verdict_write(&rec, &inc).is_empty());
    }

    // Criterion 1a: a marker write shows the existing marker AND the verdict,
    // each with its recorded_by and sha.
    #[test]
    fn marker_write_surfaces_existing_marker_and_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        merge_hold::write_typed_hold(
            root,
            &rework_marker(
                2034,
                "CHANGES REQUESTED for STORY-9 at 1111111",
                "drain reviewer",
                "1111111aaaa",
            ),
        )
        .unwrap();
        write_verdict(
            root,
            "STORY-9",
            "approved",
            "2222222bbbb",
            "claude-reviewer-1",
            "2999-01-01T00:00:00Z",
        );
        let rec = read_pr_record(root, 2034, &[]);
        let new = typed_hold(2034, HoldReasonKind::Supervision, "held by hand", None);
        let lines = lines_for_marker_write(&rec, &new).join("\n");
        assert!(lines.contains("placed by driver-advisor"), "{lines}");
        assert!(
            lines.contains("cites verdict STORY-9 by drain reviewer at 1111111"),
            "{lines}"
        );
        assert!(lines.contains("recorded by claude-reviewer-1"), "{lines}");
        assert!(lines.contains("2222222"), "{lines}");
        // The #2034 shape: the marker quotes a superseded refusal by another seat.
        assert!(lines.contains("CONTRADICTION"), "{lines}");
        assert!(lines.contains("recorded by claude-reviewer-1"), "{lines}");
    }

    // Criterion 1b: recording a verdict for a PR with a marker shows the
    // marker's text and who wrote it.
    #[test]
    fn verdict_write_surfaces_marker_text_and_author() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut m = typed_hold(
            1972,
            HoldReasonKind::Supervision,
            "operator review needed",
            None,
        );
        m.placed_by = Some("driver-advisor".into());
        merge_hold::write_typed_hold(root, &m).unwrap();
        let rec = read_pr_record(root, 1972, &["STORY-9"]);
        let inc = IncomingVerdict {
            key: "STORY-9",
            kind: VerdictKind::Approved,
            sha: Some("abcdef1234"),
            recorded_by: "claude-reviewer-1",
        };
        let lines = lines_for_verdict_write(&rec, &inc).join("\n");
        assert!(lines.contains("\"operator review needed\""), "{lines}");
        assert!(lines.contains("placed by driver-advisor"), "{lines}");
        assert!(
            !lines.contains("CONTRADICTION"),
            "no verdict claim → no collision: {lines}"
        );
    }

    // Criterion 2, the #1989 case: a marker WAITING on a verdict is confirmed,
    // not contradicted, by the verdict arriving — even with identical timing.
    #[test]
    fn waiting_marker_is_confirmed_not_contradicted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let m = typed_hold(
            1989,
            HoldReasonKind::Supervision,
            "waiting on claude-reviewer-1's verdict before merge",
            None,
        );
        merge_hold::write_typed_hold(root, &m).unwrap();
        write_verdict(
            root,
            "PR-1989",
            "request-changes",
            "3333333cccc",
            "claude-reviewer-1",
            "2999-01-01T00:00:00Z",
        );
        let rec = read_pr_record(root, 1989, &[]);
        let lines = standing_lines(&rec).join("\n");
        assert!(lines.contains("verdict PR-1989"), "{lines}");
        assert!(!lines.contains("CONTRADICTION"), "{lines}");
        let inc = IncomingVerdict {
            key: "PR-1989",
            kind: VerdictKind::Approved,
            sha: Some("4444444dddd"),
            recorded_by: "claude-reviewer-1",
        };
        assert!(!lines_for_verdict_write(&rec, &inc)
            .join("\n")
            .contains("CONTRADICTION"));
    }

    // Criterion 2, the #2034 case on the 1b direction: recording a verdict that
    // disagrees with what the standing marker quotes flags the collision.
    #[test]
    fn verdict_write_flags_a_marker_quoting_a_different_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        merge_hold::write_typed_hold(
            root,
            &rework_marker(
                2034,
                "CHANGES REQUESTED for STORY-9 at 1111111",
                "aida drain reviewer",
                "1111111aaaa",
            ),
        )
        .unwrap();
        let rec = read_pr_record(root, 2034, &[]);
        let inc = IncomingVerdict {
            key: "STORY-9",
            kind: VerdictKind::Approved,
            sha: Some("5555555eeee"),
            recorded_by: "claude-reviewer-1",
        };
        let lines = lines_for_verdict_write(&rec, &inc).join("\n");
        assert!(lines.contains("CONTRADICTION"), "{lines}");
        assert!(
            lines.contains("it says CHANGES REQUESTED but the verdict is APPROVED"),
            "{lines}"
        );
        assert!(lines.contains("recorded by claude-reviewer-1"), "{lines}");
    }

    // A verdict older than the marker it would contradict does not fire: the
    // condition is a NEWER verdict.
    #[test]
    fn older_verdict_does_not_contradict_a_newer_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        merge_hold::write_typed_hold(
            root,
            &rework_marker(
                7,
                "CHANGES REQUESTED for STORY-9 at 1111111",
                "r1",
                "1111111aaaa",
            ),
        )
        .unwrap();
        write_verdict(
            root,
            "STORY-9",
            "approved",
            "2222222bbbb",
            "r2",
            "2000-01-01T00:00:00Z",
        );
        let rec = read_pr_record(root, 7, &[]);
        assert!(!standing_lines(&rec).join("\n").contains("CONTRADICTION"));
    }

    // A later round by the same seat with the same outcome at a new head is
    // not a contradiction — only the head moved.
    #[test]
    fn same_seat_same_outcome_new_head_is_not_a_contradiction() {
        let claim = VerdictClaim {
            key: Some("STORY-9".into()),
            kind: Some(VerdictKind::RequestChanges),
            sha: Some("1111111aaaa".into()),
            recorded_by: Some("r1".into()),
        };
        assert!(disagreement(
            &claim,
            VerdictKind::RequestChanges,
            Some("2222222bbbb"),
            Some("r1")
        )
        .is_none());
        assert!(disagreement(
            &claim,
            VerdictKind::RequestChanges,
            Some("2222222bbbb"),
            Some("r2")
        )
        .is_some());
    }

    #[test]
    fn prose_claim_parses_kind_and_sha_and_ignores_waiting() {
        let c = prose_claim("CHANGES REQUESTED for BUG-1 at abcdef1").unwrap();
        assert_eq!(c.kind, Some(VerdictKind::RequestChanges));
        assert_eq!(c.sha.as_deref(), Some("abcdef1"));
        assert!(prose_claim("pending reviewer verdict").is_none());
        assert!(prose_claim("held by hand — merge requires human review").is_none());
    }
}
