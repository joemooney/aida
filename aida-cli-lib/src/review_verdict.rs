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
    /// STORY-1419: the seat that recorded this verdict. Already written by
    /// `record_verdict`; it was simply never parsed back, so a consumer wanting
    /// to route a row to the reviewer WHO REFUSED had no way to ask.
    // trace:STORY-1419 | ai:claude
    pub recorded_by: Option<String>,
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
        recorded_by: str_field("recorded_by"),
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
type JsonObj = serde_json::Map<String, serde_json::Value>;

/// The commit a round was taken against. `reviewed_sha` is the current key;
/// `head` is the older one and is still the only provenance on 42 of the
/// verdict files on disk. A round carrying neither is unidentifiable.
fn round_sha(m: &JsonObj) -> Option<String> {
    m.get("reviewed_sha")
        .or_else(|| m.get("head"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn round_findings(m: &JsonObj) -> Vec<String> {
    m.get("findings")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

type RecordingKey = (Option<String>, String, String, Vec<String>);

/// Everything about a round that carries review signal. Two rounds with equal
/// keys are the same recording, so retaining the second adds nothing; any
/// difference — a different reviewer above all — is signal that overwriting
/// would destroy.
fn recording_key(m: &JsonObj) -> RecordingKey {
    let field = |k: &str| {
        m.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    (
        round_sha(m),
        field("recorded_by"),
        field("verdict"),
        round_findings(m),
    )
}

fn surviving_against_previous_round(
    obj: &serde_json::Map<String, serde_json::Value>,
    current: &[String],
) -> Vec<String> {
    if current.is_empty() {
        return Vec::new();
    }
    let rounds: Vec<&JsonObj> = obj
        .get("rounds")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object()).collect())
        .unwrap_or_default();
    let current_sha = round_sha(obj);
    // A finding SURVIVED only if the implementer pushed new code and it is
    // still there. Two reviewers recording at one head is a collision, not a
    // round, so a predecessor must sit at a DIFFERENT commit — otherwise the
    // second reviewer's overlapping findings would all read as survivors and
    // send someone to rewrite a brief that was fine. An unidentifiable round
    // cannot be established as a different commit, so it is not a predecessor.
    let Some(previous_sha) = rounds
        .iter()
        .rev()
        .filter_map(|r| round_sha(r))
        .find(|sha| Some(sha) != current_sha.as_ref())
    else {
        return Vec::new();
    };
    // Every round at that commit, unioned: a finding either reviewer raised
    // there and that is still open now did survive the round.
    let previous: Vec<String> = rounds
        .iter()
        .filter(|r| round_sha(r).as_deref() == Some(previous_sha.as_str()))
        .flat_map(|r| round_findings(r))
        .collect();
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
///
/// The head is read from `reviewed_sha` OR the older `head` key. Measured over
/// the 501 verdict files on disk: 40 carry `reviewed_sha`, 42 carry `head`, and
/// 419 carry neither. Keying on `reviewed_sha` alone would cover 8% of them.
///
/// When NEITHER key is present the round is unidentifiable, and this REFUSES to
/// archive rather than archiving blind. Archiving an unidentifiable round makes
/// a re-record its own predecessor, so every finding reads as surviving — and a
/// false survivor sends someone to rewrite a brief that was fine, which is the
/// exact harm this spec exists to prevent. A missed survivor costs one wasted
/// round; a false one corrupts the signal. The asymmetry decides it.
///
/// The unidentifiable case is live rather than historical: the PR-keyed
/// handshake writer still emits verdicts with no reviewed commit recorded.
// trace:BUG-1466 | ai:claude
// trace:STORY-1391 | ai:claude
fn archive_current_round(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    incoming: &RecordingKey,
) {
    let Some(verdict) = obj.get("verdict").and_then(|v| v.as_str()) else {
        return;
    };
    if verdict.trim().is_empty() {
        return;
    }
    if round_sha(obj).is_none() {
        // Unidentifiable round — see the note above. Refusing is the safe side.
        return;
    }
    let mut snapshot = obj.clone();
    snapshot.remove("rounds");
    let mut rounds = obj
        .get("rounds")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    // Retain iff something would otherwise be LOST. A byte-identical
    // re-recording carries nothing new; anything else does — including a
    // second reviewer at the SAME commit, which is the collision this spec
    // exists for and the one case an is-it-the-same-head test cannot see.
    let key = recording_key(&snapshot);
    if &key == incoming {
        return;
    }
    if rounds
        .iter()
        .filter_map(|r| r.as_object())
        .any(|r| recording_key(r) == key)
    {
        return;
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

/// Resolve `raw` (full or abbreviated) to the full 40-character commit sha
/// `git` knows it by, or `None` when this repo cannot resolve it (never seen
/// the commit, a deleted/never-fetched branch, or `raw` is not a revision at
/// all). Never panics, never shells out to anything but `git`, and never
/// blocks a caller on a slow or missing repo: `git` failing to run at all is
/// indistinguishable from it saying "no such commit" here, both `None`.
///
/// Shared by the write-boundary normalization in [`record_verdict`]
/// (BUG-1516 criterion 2) and [`backfill_abbreviated_shas`] (criterion 3) —
/// "the two criteria want the same helper," per the spec's own comment
/// thread, so one bug in commit resolution cannot diverge between them.
// trace:BUG-1516 | ai:claude
fn resolve_full_sha(project_root: &Path, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{raw}^{{commit}}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // git rev-parse always emits the full object name for `^{commit}`, but
    // guard the length anyway — treating a partial or malformed answer as
    // "resolved" would be worse than leaving the original string alone.
    if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(sha)
    } else {
        None
    }
}

/// One-time repair for BUG-1516 criterion 3: sweep every verdict file under
/// `.aida/review-verdicts/` and, for each abbreviated `reviewed_sha`, either
/// expand it to the full 40-character sha (when this repo can still resolve
/// the commit) or mark it explicitly unresolvable (when it cannot) — never
/// silently drop the original value either way. A sha that is already full,
/// or a file with no `reviewed_sha` at all, is left untouched.
///
/// Not run automatically anywhere; it is exposed for an operator to invoke
/// deliberately against a real, live corpus — see `aida review normalize-
/// shas`. Deliberately conservative: this NEVER rewrites `.aida/review-
/// verdicts/` files on its own initiative, because those files are live
/// coordination state other seats may be reading and writing concurrently.
// trace:BUG-1516 | ai:claude
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShaBackfillReport {
    /// Files whose abbreviated `reviewed_sha` was expanded to the full sha.
    pub resolved: Vec<String>,
    /// Files whose abbreviated `reviewed_sha` could not be resolved in this
    /// repo — marked `reviewed_sha_unresolvable: true`, original value kept.
    pub unresolvable: Vec<String>,
    /// Files that already carried a full 40-character sha.
    pub already_full: usize,
    /// Files with no `reviewed_sha` at all (nothing for this sweep to do).
    pub skipped_no_sha: usize,
}

pub fn backfill_abbreviated_shas(
    project_root: &Path,
    dry_run: bool,
) -> std::io::Result<ShaBackfillReport> {
    let mut report = ShaBackfillReport::default();
    let dir = project_root.join(".aida").join("review-verdicts");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(report);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(serde_json::Value::Object(mut obj)) = serde_json::from_str(&body) else {
            continue;
        };
        let Some(sha) = obj
            .get("reviewed_sha")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            report.skipped_no_sha += 1;
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if sha.len() == 40 {
            report.already_full += 1;
            continue;
        }
        match resolve_full_sha(project_root, &sha) {
            Some(full) => {
                obj.insert("reviewed_sha".to_string(), serde_json::Value::String(full));
                obj.remove("reviewed_sha_unresolvable");
                report.resolved.push(name);
            }
            None => {
                obj.insert(
                    "reviewed_sha_unresolvable".to_string(),
                    serde_json::Value::Bool(true),
                );
                report.unresolvable.push(name);
            }
        }
        if !dry_run {
            let body = serde_json::to_string_pretty(&serde_json::Value::Object(obj))
                .unwrap_or_else(|_| "{}".to_string());
            std::fs::write(&path, format!("{body}\n"))?;
        }
    }
    Ok(report)
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
    // BUG-1516 criterion 2: normalize AT THE WRITE BOUNDARY rather than
    // teaching every comparison site to tolerate an abbreviated sha. One
    // writer (the drain's forge-resolved head) already always writes full;
    // every abbreviated record on disk came from a caller-supplied sha
    // (a human or seat pasting a short `git rev-parse` / `gh` value)
    // passed straight through. Best-effort: a sha this repo cannot resolve
    // (a deleted branch, a sha from a different clone) is kept verbatim
    // rather than dropped — recording a verdict must never fail just
    // because an unrelated git lookup could not run.
    // trace:BUG-1516 | ai:claude
    let reviewed_sha = reviewed_sha
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| resolve_full_sha(project_root, s).unwrap_or_else(|| s.to_string()));
    let reviewed_sha = reviewed_sha.as_deref();
    // STORY-1391: a finding that survives a round is evidence about the BRIEF,
    // and nothing could detect it because this function overwrote the prior
    // round in place. Snapshot the round being replaced into `rounds` first, so
    // the comparison is possible at all. Append-only; the current round stays
    // at the top level so every existing reader is untouched.
    // trace:STORY-1391 | ai:claude
    let incoming_key: RecordingKey = (
        reviewed_sha
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        recorded_by.trim().to_string(),
        verdict.unwrap_or_default().trim().to_string(),
        findings
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    );
    archive_current_round(&mut obj, &incoming_key);
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
        return non_blocking_verdict_gate(display_id, v, relation);
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

/// BUG-1466 / BUG-1538: the non-blocking (typically APPROVED) side of the
/// gate. Reaching here already means `v.reviewed_sha` is `Some` — the
/// no-provenance case is refused above, for every kind. What was still
/// missing is this: an approval carries a specific commit, and if the branch
/// has since moved past it with commits the review never saw, treating that
/// as an unqualified "proceed" is the exact PRIN-5 failure this pair of specs
/// names — absent evidence (a review of code that no longer exists) reads as
/// good evidence (an approval of the code about to ship).
///
/// `AdvancedPast` and `Rewritten` WARN rather than refuse: unlike a blocking
/// verdict (where new commits are presumed to be the fix), new commits after
/// an APPROVAL were never reviewed at all, so the honest response is to say
/// so loudly and let the human confirm — not to silently bless them, and not
/// to hard-refuse ordinary post-approval churn (a merge commit, a rebase)
/// that this check cannot itself judge as safe or not. `Unknown` still
/// refuses: a gate that cannot establish the relationship must not answer
/// confidently in either direction.
// trace:BUG-1466 | ai:claude
// trace:BUG-1538 | ai:claude
fn non_blocking_verdict_gate(
    display_id: &str,
    v: &RecordedVerdict,
    relation: TipRelation,
) -> VerdictGate {
    let named_sha = v
        .reviewed_sha
        .as_deref()
        .map(|s| short_sha(s).to_string())
        .unwrap_or_else(|| "an unrecorded commit".to_string());
    match relation {
        TipRelation::AtReviewedSha => VerdictGate::Proceed,
        TipRelation::AdvancedPast => VerdictGate::Warn(vec![format!(
            "warning: {display_id} was {} against {named_sha}, and new commits have landed on \
             the branch since — they were not covered by that review. Confirm they don't need a \
             fresh look before treating this as reviewed.",
            v.kind.label()
        )]),
        TipRelation::Rewritten => VerdictGate::Warn(vec![format!(
            "warning: {display_id} was {} against {named_sha}, and that commit is no longer in \
             the branch's history (amended / rebased / force-pushed) — the approval may not cover \
             the code about to ship.",
            v.kind.label()
        )]),
        TipRelation::Unknown => VerdictGate::Refuse(
            vec![
            format!(
                "error: aida queue done refused (exit 1) — {display_id} was {} against \
                 {named_sha}, and this check could not establish whether the branch has moved \
                 past it since.",
                v.kind.label()
            ),
            summary_line(v),
            "A review gate that cannot answer must not wave work through. Re-review the current \
             branch, or record a fresh verdict against the current head."
                .to_string(),
            format!("Override: `aida queue done {display_id} --force`."),
        ]
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect(),
        ),
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
