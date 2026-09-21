//! Review verdicts as first-class, queryable state — the substrate behind the
//! `aida queue done` review gate.
//!
//! # Why this exists
//!
//! A reviewer used to record "changes requested" as PROSE: a comment on the
//! spec (or on a PR) naming the reviewed commit and the blocking defects.
//! Prose is not a gate. A later implementer session merged the exact rejected
//! commit, marked the spec Done, and nothing in the tooling objected — the
//! three blocking defects shipped.
//!
//! The fix is the smallest structure that lets a gate *read* the verdict:
//! per-spec JSON at `.aida/review-verdicts/<SPEC>.json` carrying
//!
//!   - `verdict`   — the verdict word (approved / request-changes / rejected)
//!   - `reviewed_sha` — the commit the reviewer actually looked at
//!   - `reviewed_branch` / `recorded_at` / `summary` — context for humans
//!   - `findings` — concrete reviewer findings for rework handoff
//!
//! That file already existed (the `/aida-review` skill writes it via
//! `AIDA_REVIEW_VERDICT_FILE`); this module adds the sha + timestamp stamp,
//! a merge-preserving writer, a reader, and the two PURE decisions the gate
//! needs — [`classify_tip_relation`] and [`queue_done_verdict_gate`] — so the
//! whole policy is unit-testable with no git, no filesystem, and no reviewer.
//!
//! This is deliberately NOT a general review subsystem: no threads, no
//! per-file comments, no history. One current verdict per spec, enough to
//! answer "may this spec be marked done?".
//
// trace:BUG-775 | ai:claude

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

use std::path::{Path, PathBuf};

/// The verdict word, normalized from whatever the reviewer/skill wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerdictKind {
    /// The review passed — nothing blocks the spec being marked done.
    Approved,
    /// The reviewer asked for changes — blocking until new work lands.
    RequestChanges,
    /// The reviewer rejected the work outright — blocking.
    Rejected,
    /// An unrecognised verdict word. Treated as non-blocking (we never
    /// invent a refusal out of a string we do not understand), but the raw
    /// word is preserved so surfaces can show it.
    #[default]
    Other,
}

impl VerdictKind {
    /// Normalize a verdict word. Accepts the spellings the `/aida-review`
    /// skill, the orchestrator, and humans actually write.
    pub fn parse(raw: &str) -> VerdictKind {
        match raw
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "approved" | "approve" | "lgtm" | "pass" | "passed" | "ok" => VerdictKind::Approved,
            "requestchanges" | "request-changes" | "changes" | "changes-requested"
            | "needs-changes" | "partial" => VerdictKind::RequestChanges,
            "rejected" | "reject" | "fail" | "failed" | "blocked" => VerdictKind::Rejected,
            _ => VerdictKind::Other,
        }
    }

    /// Display label for terminal output.
    pub fn label(&self) -> &'static str {
        match self {
            VerdictKind::Approved => "APPROVED",
            VerdictKind::RequestChanges => "CHANGES REQUESTED",
            VerdictKind::Rejected => "REJECTED",
            VerdictKind::Other => "UNKNOWN",
        }
    }

    /// Does this verdict block "mark it done" until the branch moves on?
    pub fn blocks_done(&self) -> bool {
        matches!(self, VerdictKind::RequestChanges | VerdictKind::Rejected)
    }
}

/// The recorded verdict for one spec — the queryable state the gate reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecordedVerdict {
    /// Normalized verdict.
    pub kind: VerdictKind,
    /// The verdict word exactly as recorded.
    pub raw: String,
    /// The commit the reviewer examined, when recorded.
    pub reviewed_sha: Option<String>,
    /// The branch the reviewer examined, when recorded.
    pub reviewed_branch: Option<String>,
    /// RFC-3339 timestamp of when the verdict was recorded.
    pub recorded_at: Option<String>,
    /// One-line rationale.
    pub summary: Option<String>,
    /// URL of the PR comment carrying the full review, when the reviewer
    /// backfilled it.
    pub comment_url: Option<String>,
    /// Optional richer review body, preserved when a newer reviewer writes it.
    pub review_comment: Option<String>,
    /// Optional structured findings, preserved when a newer reviewer writes it.
    pub findings: Vec<String>,
    /// STORY-1391: findings that also appeared in the PREVIOUS round. Empty on
    /// a first round, and empty when every finding is new. Populated by
    /// [`parse_recorded_verdict`] from the retained `rounds`, so every caller
    /// that already reads a verdict gets the signal without a new argument.
    // trace:STORY-1391 | ai:claude
    pub surviving_findings: Vec<String>,
}

/// Path of the per-spec verdict file. Spec ids are upper-cased so
/// `bug-775` and `BUG-775` resolve to the same record.
pub fn verdict_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("{}.json", spec.trim().to_ascii_uppercase()))
}

/// Parse a verdict file body. `None` when it is not a JSON object or carries
/// no `verdict` field (an incomplete artifact is not a verdict).
pub fn parse_recorded_verdict(body: &str) -> Option<RecordedVerdict> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;
    let str_field = |k: &str| -> Option<String> {
        obj.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let raw = str_field("verdict")?;
    let findings: Vec<String> = obj
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(RecordedVerdict {
        kind: VerdictKind::parse(&raw),
        raw,
        reviewed_sha: str_field("reviewed_sha"),
        reviewed_branch: str_field("reviewed_branch"),
        recorded_at: str_field("recorded_at"),
        summary: str_field("summary"),
        comment_url: str_field("comment_url"),
        review_comment: str_field("review_comment")
            .or_else(|| str_field("comment_body"))
            .or_else(|| str_field("body")),
        surviving_findings: surviving_against_previous_round(obj, &findings),
        findings,
    })
}

/// STORY-1391: the findings in `current` that also appeared in the most recent
/// archived round. Shared by [`parse_recorded_verdict`] and
/// [`findings_surviving_round`] so the two can never disagree about what
/// "survived" means.
// trace:STORY-1391 | ai:claude
fn surviving_against_previous_round(
    obj: &serde_json::Map<String, serde_json::Value>,
    current: &[String],
) -> Vec<String> {
    if current.is_empty() {
        return Vec::new();
    }
    let previous: Vec<String> = obj
        .get("rounds")
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|r| r.get("findings"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    current
        .iter()
        .filter(|f| previous.iter().any(|p| p == *f))
        .cloned()
        .collect()
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

/// Read the recorded verdict for `spec`, if any.
pub fn read_recorded_verdict(project_root: &Path, spec: &str) -> Option<RecordedVerdict> {
    let body = std::fs::read_to_string(verdict_path(project_root, spec)).ok()?;
    parse_recorded_verdict(&body)
}

/// Read the recorded verdict trying several id forms (agreed id, spec id) —
/// whichever file exists first wins. Callers hold both forms and the verdict
/// may have been filed under either.
pub fn read_recorded_verdict_any(project_root: &Path, ids: &[&str]) -> Option<RecordedVerdict> {
    ids.iter()
        .filter(|id| !id.trim().is_empty() && **id != "???")
        .find_map(|id| read_recorded_verdict(project_root, id))
}

/// Write (or update) the verdict record for `spec`, preserving any fields the
/// reviewer skill already wrote that this call does not set. Returns the path.
/// STORY-1391: move the round currently at the top level into the append-only
/// `rounds` array, before `record_verdict` overwrites it.
///
/// A round is worth keeping only if it actually recorded a verdict; a partial
/// artifact with no `verdict` field is not a round.
///
/// A round is identified by the head it REVIEWED, not by when it was recorded.
/// `record_verdict` stamps `recorded_at` to now on every call, so keying on the
/// timestamp would make every re-record look like a new round — a reviewer
/// correcting its own summary before the head moves would manufacture one, and
/// `findings_surviving_round` would then report its own findings as surviving.
/// A verdict with no `reviewed_sha` cannot be deduplicated and is archived, on
/// the grounds that an unattributable round is better kept than silently
/// merged into another.
// trace:STORY-1391 | ai:claude
fn archive_current_round(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    incoming_sha: Option<&str>,
) {
    let Some(verdict) = obj.get("verdict").and_then(|v| v.as_str()) else {
        return;
    };
    if verdict.trim().is_empty() {
        return;
    }
    let sha_of = |m: &serde_json::Map<String, serde_json::Value>| {
        m.get("reviewed_sha")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let existing_sha = sha_of(obj);
    let incoming = incoming_sha
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Same head => the same round being re-recorded (a reviewer correcting its
    // own summary before anything was pushed). Archiving it would make the
    // round its own predecessor and `findings_surviving_round` would report
    // every finding as surviving on the very first re-record.
    if existing_sha.is_some() && existing_sha == incoming {
        return;
    }
    let mut snapshot = obj.clone();
    snapshot.remove("rounds");
    let mut rounds = obj
        .get("rounds")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    if let Some(sha) = existing_sha {
        if rounds
            .iter()
            .filter_map(|r| r.as_object())
            .any(|r| sha_of(r).as_deref() == Some(sha.as_str()))
        {
            return;
        }
    }
    rounds.push(serde_json::Value::Object(snapshot));
    obj.insert("rounds".to_string(), serde_json::Value::Array(rounds));
}

/// STORY-1391: findings in `current` that also appeared in the most recent
/// archived round.
///
/// Deliberately compares the PREVIOUS round only, not every round: a finding
/// that appeared in round 1, was fixed in round 2 and regressed in round 3 is a
/// regression, not a surviving finding, and the two want different responses.
///
/// Matching is exact after trimming. A reviewer that rewords a finding defeats
/// this, which is a known limit and preferable to fuzzy matching that would
/// report unrelated findings as survivors — a false survivor sends someone to
/// rewrite a brief that was fine.
// trace:STORY-1391 | ai:claude
pub fn findings_surviving_round(body: &str) -> Vec<String> {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str(body) else {
        return Vec::new();
    };
    let current: Vec<String> = obj
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    surviving_against_previous_round(&obj, &current)
}

pub fn record_verdict(
    project_root: &Path,
    spec: &str,
    verdict: Option<&str>,
    reviewed_sha: Option<&str>,
    reviewed_branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    recorded_by: &str,
) -> std::io::Result<PathBuf> {
    let path = verdict_path(project_root, spec);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut obj = std::fs::read_to_string(&path)
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    // STORY-1391: a finding that survives a round is evidence about the BRIEF,
    // and nothing could detect it because this function overwrote the prior
    // round in place. Snapshot the round being replaced into `rounds` first, so
    // the comparison is possible at all. Append-only; the current round stays
    // at the top level so every existing reader is untouched.
    // trace:STORY-1391 | ai:claude
    archive_current_round(&mut obj, reviewed_sha);
    let mut set = |k: &str, v: Option<&str>| {
        if let Some(v) = v.map(str::trim).filter(|s| !s.is_empty()) {
            obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
    };
    set("verdict", verdict);
    set("reviewed_sha", reviewed_sha);
    set("reviewed_branch", reviewed_branch);
    set("summary", summary);
    set("recorded_by", Some(recorded_by));
    set(
        "recorded_at",
        Some(chrono::Utc::now().to_rfc3339().as_str()),
    );
    let findings: Vec<_> = findings
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::Value::String(s.to_string()))
        .collect();
    if !findings.is_empty() {
        obj.insert("findings".to_string(), serde_json::Value::Array(findings));
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .unwrap_or_else(|_| "{}".to_string());
    std::fs::write(&path, format!("{body}\n"))?;
    Ok(path)
}

/// Where the branch tip sits relative to the commit the verdict named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipRelation {
    /// The tip IS the reviewed commit — no new work since the review.
    AtReviewedSha,
    /// The reviewed commit is an ancestor of a different tip — new commits
    /// landed after the review.
    AdvancedPast,
    /// The tip differs but the reviewed commit is not in its history — the
    /// branch was amended / rebased / force-pushed since the review.
    Rewritten,
    /// Could not be established: no sha recorded, the tip is unreadable, or
    /// the reviewed sha is not an object in this repo.
    Unknown,
}

/// Pure classifier. `reviewed_sha` / `tip_sha` must already be full,
/// repo-resolved object names (the caller expands them with `git rev-parse`);
/// `reviewed_is_ancestor_of_tip` is `git merge-base --is-ancestor`'s answer,
/// `None` when that probe itself failed.
pub fn classify_tip_relation(
    reviewed_sha: Option<&str>,
    tip_sha: Option<&str>,
    reviewed_is_ancestor_of_tip: Option<bool>,
) -> TipRelation {
    let (Some(reviewed), Some(tip)) = (reviewed_sha, tip_sha) else {
        return TipRelation::Unknown;
    };
    if reviewed.eq_ignore_ascii_case(tip) {
        return TipRelation::AtReviewedSha;
    }
    match reviewed_is_ancestor_of_tip {
        Some(true) => TipRelation::AdvancedPast,
        Some(false) => TipRelation::Rewritten,
        None => TipRelation::Unknown,
    }
}

/// The gate's decision. `Refuse` lines are printed and the command exits
/// non-zero; `Warn` lines are printed and the command continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictGate {
    /// No blocking verdict, or the branch clearly moved past it.
    Proceed,
    /// Proceed, but say something first.
    Warn(Vec<String>),
    /// Stop: the last word on this spec was "changes requested" and there is
    /// no evidence the branch answered it.
    Refuse(Vec<String>),
}

/// Pure policy: may `queue done` proceed given the recorded verdict and where
/// the branch tip sits relative to it?
///
/// - no verdict, or a non-blocking one → proceed
/// - blocking verdict, tip advanced past the reviewed sha → proceed
/// - blocking verdict, branch rewritten since the review → proceed, loudly
/// - blocking verdict, tip still at the reviewed sha → REFUSE
/// - blocking verdict, relation unknown (no sha recorded / unreadable tip)
///   → REFUSE. A gate that cannot establish the answer must not wave the
///   work through; that silent-skip is exactly how rejected code shipped.
pub fn queue_done_verdict_gate(
    display_id: &str,
    verdict: Option<&RecordedVerdict>,
    relation: TipRelation,
) -> VerdictGate {
    let Some(v) = verdict else {
        return VerdictGate::Proceed;
    };
    // An approval without a commit is not authorization for any particular
    // head. Fail closed for every verdict kind, including APPROVED: otherwise
    // a legacy hand-written file silently bypasses the post-review SHA guard.
    // trace:BUG-1467 | ai:codex
    if v.reviewed_sha.is_none() {
        return VerdictGate::Refuse(vec![
            format!(
                "error: aida queue done refused (exit 1) — the last review of {display_id} is UNVERIFIABLE because it records no reviewed_sha, so this check could not establish which head it covered."
            ),
            summary_line(v),
            format!(
                "Record a fresh verdict against the current head: `aida review record {display_id} --verdict approved --summary \"<why>\"`."
            ),
        ]
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect());
    }
    if !v.kind.blocks_done() {
        return VerdictGate::Proceed;
    }
    let when = v
        .recorded_at
        .as_deref()
        .map(|t| format!(" recorded {t}"))
        .unwrap_or_default();
    let named_sha = v
        .reviewed_sha
        .as_deref()
        .map(|s| short_sha(s).to_string())
        .unwrap_or_else(|| "an unrecorded commit".to_string());
    let override_line = format!(
        "Override (only when you know the review was answered another way): \
         `aida queue done {display_id} --force` — it prints the verdict it is overriding."
    );
    let clear_line = format!(
        "Or clear the verdict after a fresh look: \
         `aida review record {display_id} --verdict approved --summary \"<why>\"`."
    );
    match relation {
        TipRelation::AdvancedPast => VerdictGate::Proceed,
        TipRelation::Rewritten => VerdictGate::Warn(vec![format!(
            "warning: the last review of {display_id} was {}{} against {}, and that commit is no \
             longer in the branch's history (amended / rebased / force-pushed). Proceeding — \
             confirm the review's findings were actually addressed.",
            v.kind.label(),
            when,
            named_sha
        )]),
        TipRelation::AtReviewedSha => VerdictGate::Refuse(vec![
            format!(
                "error: aida queue done refused (exit 1) — the last review of {display_id} was {}{} \
                 against {}, and the branch tip is still that exact commit.",
                v.kind.label(),
                when,
                named_sha
            ),
            summary_line(v),
            "Nothing has changed since the review, so marking this done would ship the reviewed \
             code unchanged. Address the review, commit, then re-run."
                .to_string(),
            override_line,
            clear_line,
        ]
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect()),
        TipRelation::Unknown => VerdictGate::Refuse(vec![
            format!(
                "error: aida queue done refused (exit 1) — the last review of {display_id} was {}{}, \
                 and this check could not establish whether the branch has moved past {} since.",
                v.kind.label(),
                when,
                named_sha
            ),
            summary_line(v),
            "A review gate that cannot answer must not wave work through. Re-review the current \
             branch, or record the commit the review covered."
                .to_string(),
            override_line,
            clear_line,
        ]
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect()),
    }
}

/// The verdict's one-line rationale, rendered for a refusal block. Empty when
/// none was recorded (the caller filters empties out).
fn summary_line(v: &RecordedVerdict) -> String {
    v.summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("Reviewer said: {s}"))
        .unwrap_or_default()
}

/// First 12 chars of a sha (or the whole string when shorter) — enough to
/// identify a commit in a message without wrapping the line.
pub fn short_sha(sha: &str) -> &str {
    let s = sha.trim();
    if s.len() > 12 {
        &s[..12]
    } else {
        s
    }
}

/// One-line summary of a recorded verdict for read-only surfaces (the pickup
/// view). `None` when there is nothing worth showing.
pub fn verdict_notice_line(v: &RecordedVerdict) -> String {
    let mut line = v.kind.label().to_string();
    if v.kind == VerdictKind::Other {
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

#[cfg(test)]
#[path = "tests/review_verdict_tests.rs"]
mod review_verdict_tests;
