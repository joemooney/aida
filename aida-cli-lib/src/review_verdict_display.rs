//! Review verdict display (STORY-1889, from `review_verdict.rs`): the
//! rework-findings comment, the record-outcome and notice lines, and the
//! `aida doctor` audit of non-canonical verdict bodies. These render or
//! report verdicts; none decides whether one counts, so they sit outside
//! the protected `gate/` module and read it.
// trace:STORY-1889 | ai:claude

#[allow(unused_imports)]
use crate::gate::review_verdict::*;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};

// BUG-1213: every rework findings block starts with this, so the recurrence
/// guard can find the last recorded block by prefix.
// trace:BUG-1213 | ai:claude
pub(crate) const FINDINGS_BLOCK_PREFIX: &str = "REVIEW FINDINGS TO ADDRESS (";

/// True only for the canonical durable findings block written by the review
/// hand-off. Discussion that merely embeds or quotes the marker is not review
/// history and must not advance either the reviewer or rework round.
// trace:TASK-1291 | ai:codex
pub(crate) fn is_findings_block(content: &str) -> bool {
    content.starts_with(FINDINGS_BLOCK_PREFIX)
}

/// Render the review delta a rework implementer must address.
///
/// The verdict file is the durable hand-off between reviewer and rework. Older
/// files only carry `summary` + optional `comment_url`; newer files may carry
/// `findings[]` or a fuller comment body. Keep the renderer tolerant so a
/// rework session always sees the blocking review instead of silently looping.
// trace:BUG-814 | ai:codex
pub fn rework_findings_comment(
    spec_id: &str,
    review_ref: &str,
    verdict: &RecordedVerdict,
) -> Option<String> {
    if !verdict.kind.blocks_done() {
        return None;
    }
    let mut out = format!(
        "{}{}):\nVerdict: {}",
        FINDINGS_BLOCK_PREFIX,
        review_ref,
        verdict.kind.label()
    );
    if let Some(sha) = verdict.reviewed_sha.as_deref() {
        out.push_str(&format!("\nReviewed commit: {}", short_sha(sha)));
    }
    if let Some(at) = verdict.recorded_at.as_deref() {
        out.push_str(&format!("\nRecorded: {at}"));
    }
    if let Some(url) = verdict.comment_url.as_deref() {
        out.push_str(&format!("\nReview comment: {url}"));
    }

    let mut items: Vec<String> = verdict.findings.clone();
    if items.is_empty() {
        if let Some(body) = verdict.review_comment.as_deref() {
            items.extend(
                body.lines()
                    .map(str::trim)
                    .filter(|line| {
                        !line.is_empty()
                            && !line.starts_with('|')
                            && !line.starts_with("##")
                            && !line.starts_with("**CI**")
                            && !line.starts_with("**Recommendation**")
                    })
                    .map(|line| {
                        line.trim_start_matches(['-', '*'])
                            .trim()
                            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                            .trim()
                            .to_string()
                    })
                    .filter(|line| !line.is_empty()),
            );
        }
    }
    if items.is_empty() {
        if let Some(summary) = verdict.summary.as_deref() {
            items.push(summary.to_string());
        }
    }
    if items.is_empty() {
        items.push(format!(
            "{} has a blocking review verdict; inspect the review comment before changing code.",
            spec_id
        ));
    }

    out.push_str("\nFindings:");
    for (idx, item) in items.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", idx + 1, item));
    }
    // STORY-1391: a finding that survives a round is evidence about the BRIEF,
    // not about the implementer. The wording below is load-bearing and is the
    // spec's central point: an implementer-performance framing produces the
    // escalating firmness that demonstrably failed three times, so this says
    // what to DO — establish implementability — and never says "again".
    // trace:STORY-1391 | ai:claude
    if !verdict.surviving_findings.is_empty() {
        out.push_str("\nSurvived the previous round:");
        for item in &verdict.surviving_findings {
            out.push_str(&format!("\n- {item}"));
        }
        out.push_str(
            "\nThese were raised before and are unchanged. Restating them more firmly will not \
             help: establish whether each can be done AS WRITTEN. Supply the missing mechanism \
             (a constant, a helper, a terminating definition), split the finding into parts that \
             can be done, or say explicitly that it is blocked and why.",
        );
    }
    // trace:TASK-1190 | ai:codex
    out.push_str(
        "\nContract: produce at least one commit, or punt explicitly naming the finding you dispute; never pass through silently with no changes.",
    );
    Some(out)
}

/// BUG-2136: the one outcome line `aida review record` prints, e.g.
/// `recorded APPROVED for BUG-7 at 0123456789ab (round 2)` or
/// `unchanged: identical APPROVED already recorded at 0123456789ab`.
// trace:BUG-2136 | ai:claude
pub(crate) fn record_outcome_line(
    outcome: &RecordOutcome,
    label: &str,
    key: &str,
    sha: Option<&str>,
) -> String {
    let sha = sha.map(short_sha).unwrap_or("unknown commit");
    match outcome {
        RecordOutcome::Recorded { round } => {
            format!("recorded {label} for {key} at {sha} (round {round})")
        }
        RecordOutcome::Unchanged => {
            format!("unchanged: identical {label} already recorded at {sha}")
        }
    }
}

/// One-line summary of a recorded verdict for read-only surfaces (the pickup
/// view). `None` when there is nothing worth showing.
pub fn verdict_notice_line(v: &RecordedVerdict) -> String {
    let mut line = v.kind.label().to_string();
    if v.kind == VerdictKind::Unknown {
        line = format!("{} ({})", line, v.raw);
    }
    if let Some(sha) = v.reviewed_sha.as_deref() {
        line.push_str(&format!(" against {}", short_sha(sha)));
    } else {
        line.push_str(" — UNVERIFIABLE (missing reviewed_sha)");
    }
    if let Some(at) = v.recorded_at.as_deref() {
        line.push_str(&format!(" ({at})"));
    }
    if let Some(s) = v
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        line.push_str(&format!(" — {s}"));
    }
    line
}

/// BUG-1505: one verdict file that does not match the canonical shape, with
/// every reason it does not. Produced by [`audit_verdict_dir`] for the
/// `aida doctor --category review-verdicts` report. Report-only: the audit
/// never rewrites a file (the gitignored store has no history to recover a
/// bad rewrite from).
// trace:BUG-1505 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonCanonicalVerdict {
    /// File name under `.aida/review-verdicts/`.
    pub file: String,
    /// The verdict word as written (empty when absent/unreadable).
    pub raw: String,
    /// What the canonical parser reads it as.
    pub kind: VerdictKind,
    /// Human-readable reasons, one per deviation.
    pub issues: Vec<String>,
}

/// Why `body` is not a canonical verdict record. Empty = canonical. Pure, so
/// the audit's rules are unit-testable without a filesystem.
// trace:BUG-1505 | ai:claude
pub fn audit_verdict_body(body: &str) -> (String, VerdictKind, Vec<String>) {
    let mut issues = Vec::new();
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(body) else {
        issues.push("not a JSON object".to_string());
        return (String::new(), VerdictKind::Unknown, issues);
    };
    let raw = obj
        .get("verdict")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let kind = VerdictKind::parse(&raw);
    if raw.is_empty() {
        issues.push("no verdict".to_string());
    } else {
        match kind.canonical() {
            Some(c) if c == raw => {}
            Some(c) => issues.push(format!("verdict `{raw}` is not canonical (reads as `{c}`)")),
            None => issues.push(format!(
                "verdict `{raw}` is UNKNOWN or ambiguous — read as NOT approved; a human must decide it"
            )),
        }
    }
    for keys in [SHA_KEYS, RECORDER_KEYS, RECORDED_AT_KEYS, FINDINGS_KEYS] {
        for alias in &keys[1..] {
            if obj.contains_key(*alias) && !obj.contains_key(keys[0]) {
                issues.push(format!("legacy key `{alias}` (canonical: `{}`)", keys[0]));
            }
        }
    }
    let has_sha = !is_marked_unverifiable(&obj)
        && SHA_KEYS.iter().any(|k| {
            obj.get(*k)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.trim().is_empty())
        });
    if !has_sha {
        issues.push("no reviewed sha — cannot be placed against any head".to_string());
    }
    if !RECORDER_KEYS.iter().any(|k| obj.contains_key(*k)) {
        issues.push("no recorder".to_string());
    }
    (raw, kind, issues)
}

/// Audit every top-level `*.json` under `.aida/review-verdicts/` and return
/// the non-canonical ones, sorted by file name. Per-sha archives are not
/// walked — they are copies of rounds whose live file is audited here.
// trace:BUG-1505 | ai:claude
pub fn audit_verdict_dir(project_root: &Path) -> Vec<NonCanonicalVerdict> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<NonCanonicalVerdict> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| {
            let file = p.file_name()?.to_str()?.to_string();
            let body = read_verdict_body(&p).unwrap_or_default();
            let (raw, kind, issues) = audit_verdict_body(&body);
            (!issues.is_empty()).then_some(NonCanonicalVerdict {
                file,
                raw,
                kind,
                issues,
            })
        })
        .collect();
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

#[cfg(test)]
#[path = "tests/review_verdict_display_tests.rs"]
mod review_verdict_display_tests;
