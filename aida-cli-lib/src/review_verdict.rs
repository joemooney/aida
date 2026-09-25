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
///
/// BUG-1505: this enum is the CANONICAL verdict vocabulary and
/// [`VerdictKind::parse`] is the ONE normalizing parser every reader and
/// writer routes through. The on-disk corpus carries many spellings of the
/// same two outcomes (`Approved` / `APPROVED` / `approved` / `approve`,
/// `CHANGES REQUESTED` / `RequestChanges` / `request-changes`); they all map
/// here, and every writer persists [`VerdictKind::canonical`] instead of the
/// raw word.
///
/// Normalization is deliberately SHALLOW — case, and the separator between
/// words — and the match is on the WHOLE token. A value that carries
/// qualifying prose around an approval word (`APPROVED pending
/// cross-platform green`, `CONTENT APPROVED — MERGE WITHHELD FOR
/// INDEPENDENCE`) is therefore [`VerdictKind::Unknown`], never Approved: the
/// qualification is exactly what makes it not an approval, and no substring
/// or prefix match is ever performed.
// trace:BUG-1505 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerdictKind {
    /// The review passed — nothing blocks the spec being marked done.
    Approved,
    /// The reviewer asked for changes — blocking until new work lands.
    RequestChanges,
    /// The reviewer rejected the work outright — blocking.
    Rejected,
    /// An unrecognised or ambiguous verdict word (including a qualified
    /// approval). NEVER approving (PRIN-5: absent or unreadable evidence is
    /// not good evidence) — every gate that asks "was this approved?" answers
    /// no, and the `queue done` gate refuses outright. The raw word is kept on
    /// [`RecordedVerdict::raw`] so surfaces can show it.
    #[default]
    Unknown,
}

impl VerdictKind {
    /// Normalize a verdict word. Accepts the spellings the `/aida-review`
    /// skill, the orchestrator, and humans actually write. Case-insensitive;
    /// spaces, underscores and hyphens between words are equivalent and may
    /// be absent (`RequestChanges`). Anything else is `Unknown`.
    // trace:BUG-1505 | ai:claude
    pub fn parse(raw: &str) -> VerdictKind {
        let norm: String = raw
            .trim()
            .chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect();
        match norm.as_str() {
            "approved" | "approve" | "lgtm" | "pass" | "passed" | "ok" => VerdictKind::Approved,
            "requestchanges" | "requestedchanges" | "changes" | "changesrequested"
            | "needschanges" | "partial" => VerdictKind::RequestChanges,
            "rejected" | "reject" | "fail" | "failed" | "blocked" => VerdictKind::Rejected,
            _ => VerdictKind::Unknown,
        }
    }

    /// The one canonical on-disk spelling for this outcome. `None` for
    /// `Unknown` — an unrecognised word has no canonical form and must not be
    /// silently rewritten into one.
    // trace:BUG-1505 | ai:claude
    pub fn canonical(&self) -> Option<&'static str> {
        match self {
            VerdictKind::Approved => Some("approved"),
            VerdictKind::RequestChanges => Some("request-changes"),
            VerdictKind::Rejected => Some("rejected"),
            VerdictKind::Unknown => None,
        }
    }

    /// Is this an approval? Only `Approved` — `Unknown` never approves.
    // trace:BUG-1505 | ai:claude
    pub fn approves(&self) -> bool {
        matches!(self, VerdictKind::Approved)
    }

    /// Display label for terminal output.
    pub fn label(&self) -> &'static str {
        match self {
            VerdictKind::Approved => "APPROVED",
            VerdictKind::RequestChanges => "CHANGES REQUESTED",
            VerdictKind::Rejected => "REJECTED",
            VerdictKind::Unknown => "UNKNOWN",
        }
    }

    /// Does this verdict block "mark it done" until the branch moves on?
    /// `Unknown` is not an explicit refusal (it is not reported as one), but
    /// gates must still treat it as not-approving — see [`Self::approves`].
    pub fn blocks_done(&self) -> bool {
        matches!(self, VerdictKind::RequestChanges | VerdictKind::Rejected)
    }
}

/// Canonicalize a verdict word for persistence: the canonical spelling when
/// the word is recognised, otherwise the trimmed raw word unchanged (an
/// unrecognised word is preserved for a human, never guessed at).
// trace:BUG-1505 | ai:claude
pub fn canonical_verdict_word(raw: &str) -> String {
    VerdictKind::parse(raw)
        .canonical()
        .map(str::to_string)
        .unwrap_or_else(|| raw.trim().to_string())
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
    /// BUG-1529: the commit that closed this verdict out, when a refused
    /// spec's reworked PR later merged. Distinct from a fresh `verdict`
    /// overwrite — the refusal itself is left intact (audit trail), this
    /// only records that the branch shipped past it. `None` on every file
    /// this repo has never closed, including all pre-BUG-1529 records, so
    /// old files keep reading fine.
    // trace:BUG-1529 | ai:claude
    pub closed_by_merge: Option<String>,
    /// RFC-3339 timestamp of when `closed_by_merge` was recorded.
    // trace:BUG-1529 | ai:claude
    pub closed_at: Option<String>,
}

impl RecordedVerdict {
    /// True once a merge has closed this verdict out — see
    /// `closed_by_merge`. A closed verdict is not a fresh approval; it is
    /// the original verdict PLUS a record that the branch moved on.
    // trace:BUG-1529 | ai:claude
    pub fn is_closed(&self) -> bool {
        self.closed_by_merge.is_some()
    }
}

/// BUG-1529 criterion 3: does this recorded verdict represent a refusal that
/// is still OUTSTANDING — i.e. something a reader building an "outstanding
/// refusals" report should surface?
///
/// Two independent ways a blocking verdict stops being outstanding:
///   - it was explicitly closed by a merge (`closed_by_merge`, set going
///     forward by `close_verdict_on_merge`), or
///   - the caller's own store already shows the spec as `Completed` — the
///     BUG-1529 criterion 4 fallback for the pre-existing corpus this fix
///     cannot retroactively rewrite (this module never touches a live verdict
///     file except at the moment a merge is observed). A Completed spec's
///     work shipped by definition, so a still-`request-changes` record on it
///     is exactly the false positive this bug measured (STORY-1033, STORY-818)
///     and must not read as outstanding regardless of whether it was ever
///     closed.
///
/// `spec_completed` is supplied by the caller (this module deliberately does
/// not depend on `aida_core`'s store/status types, to stay a small, pure,
/// filesystem-only module).
// trace:BUG-1529 | ai:claude
pub fn is_outstanding_refusal(verdict: &RecordedVerdict, spec_completed: bool) -> bool {
    verdict.kind.blocks_done() && !verdict.is_closed() && !spec_completed
}

/// Path of the per-spec verdict file. Spec ids are upper-cased so
/// `bug-775` and `BUG-775` resolve to the same record.
/// Stage `body` in a temp file next to `path`, rename it into place (atomic
/// on the same filesystem — no reader ever observes a half-written file),
/// then read the file back and confirm the bytes landed. BUG-1571: a bare
/// `fs::write` returning `Ok(())` is not proof the artefact is durably on
/// disk under concurrent activity (another writer, a sweep, a racy
/// filesystem) — this makes "written" mean "confirmed present with the
/// staged content", not "the syscall returned". Every writer in this module
/// routes through this one boundary so the guarantee is uniform.
// trace:BUG-1571 | ai:claude
pub(crate) fn write_verdict_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("verdict.json");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = dir.join(format!(".{file_name}.tmp-{}-{nanos}", std::process::id()));
    let staged = (|| -> std::io::Result<()> {
        std::fs::write(&tmp_path, body)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(std::io::Error::new(
            e.kind(),
            format!("could not write {}: {e}", path.display()),
        ));
    }
    // Verify: an honest "written" claim reads the artefact back rather than
    // trusting the write syscall's return value.
    match std::fs::read_to_string(path) {
        Ok(on_disk) if on_disk == body => Ok(()),
        Ok(_) => Err(std::io::Error::other(format!(
            "wrote {} but the content on disk does not match what was staged",
            path.display()
        ))),
        Err(e) => Err(std::io::Error::other(format!(
            "wrote {} but could not read it back to confirm: {e}",
            path.display()
        ))),
    }
}

pub fn verdict_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("{}.json", spec.trim().to_ascii_uppercase()))
}

/// Legacy key aliases, per fact, in preference order. BUG-1505: the corpus
/// carries several key shapes for the same fact (47 distinct key sets across
/// the verdict files measured 2026-09-23); the reader tries every observed
/// spelling here, in ONE place, so no consumer hand-rolls its own fallbacks.
/// The first entry of each list is the canonical key every writer emits.
// trace:BUG-1505 | ai:claude
pub(crate) const SHA_KEYS: &[&str] = &["reviewed_sha", "head_sha", "head", "sha"];
pub(crate) const RECORDER_KEYS: &[&str] = &["recorded_by", "reviewer"];
pub(crate) const RECORDED_AT_KEYS: &[&str] = &["recorded_at", "reviewed_at", "date"];
pub(crate) const FINDINGS_KEYS: &[&str] = &["findings", "blocking_findings"];

/// Parse a verdict file body. `None` when it is not a JSON object or carries
/// no `verdict` field (an incomplete artifact is not a verdict).
///
/// This is the only supported way to consume a verdict file (BUG-1505): the
/// verdict word goes through [`VerdictKind::parse`], and every legacy key
/// alias ([`SHA_KEYS`], [`RECORDER_KEYS`], [`RECORDED_AT_KEYS`],
/// [`FINDINGS_KEYS`]) is resolved here. A file explicitly marked
/// `unverifiable: true` is read with NO reviewed sha, whatever else it
/// carries — its author has said it cannot be placed against a head.
// trace:BUG-1505 | ai:claude
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
    let first_of = |keys: &[&str]| keys.iter().find_map(|k| str_field(k));
    let raw = str_field("verdict")?;
    let findings: Vec<String> = FINDINGS_KEYS
        .iter()
        .find_map(|k| obj.get(*k).and_then(|v| v.as_array()))
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
    let reviewed_sha = if is_marked_unverifiable(obj) {
        None
    } else {
        first_of(SHA_KEYS)
    };
    Some(RecordedVerdict {
        kind: VerdictKind::parse(&raw),
        raw,
        reviewed_sha,
        reviewed_branch: str_field("reviewed_branch"),
        recorded_by: first_of(RECORDER_KEYS),
        recorded_at: first_of(RECORDED_AT_KEYS),
        closed_by_merge: str_field("closed_by_merge"),
        closed_at: str_field("closed_at"),
        summary: str_field("summary"),
        comment_url: str_field("comment_url"),
        review_comment: str_field("review_comment")
            .or_else(|| str_field("comment_body"))
            .or_else(|| str_field("body")),
        surviving_findings: surviving_against_previous_round(obj, &findings),
        findings,
    })
}

/// `unverifiable: true` (bool, or the string `"true"`).
// trace:BUG-1505 | ai:claude
fn is_marked_unverifiable(obj: &JsonObj) -> bool {
    match obj.get("unverifiable") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
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
    // trace:BUG-1505 | ai:claude — same alias set as the reader.
    SHA_KEYS.iter().find_map(|k| {
        m.get(*k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
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
        // BUG-1505: a re-spelling of the same verdict is the same recording.
        canonical_verdict_word(&field("verdict")),
        round_findings(m),
    )
}

// trace:BUG-1490 | ai:claude — widened from private so awaiting_you's
// caller (lib.rs) can apply the same prefix-tolerant sha match a recorded
// verdict already uses elsewhere, instead of re-deriving comparison rules.
pub(crate) fn same_reviewed_sha(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    let common = a.len().min(b.len());
    common >= 7
        && a.bytes().all(|c| c.is_ascii_hexdigit())
        && b.bytes().all(|c| c.is_ascii_hexdigit())
        && (a.eq_ignore_ascii_case(b)
            || (a.len() < b.len() && b[..a.len()].eq_ignore_ascii_case(a))
            || (b.len() < a.len() && a[..b.len()].eq_ignore_ascii_case(b)))
}

/// Explain an irreconcilable pair of independent verdicts at the artifact's
/// current reviewed commit. Historical disagreement at an older commit is an
/// audit trail, not a veto on a later review.
// trace:BUG-1581 | ai:codex
pub fn verdict_conflict_for_current_sha(body: &str) -> Option<String> {
    let serde_json::Value::Object(obj) = serde_json::from_str(body).ok()? else {
        return None;
    };
    let current_sha = round_sha(&obj)?;
    let mut rounds: Vec<&JsonObj> = vec![&obj];
    rounds.extend(
        obj.get("rounds")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_object()),
    );

    let mut approvals = Vec::new();
    let mut blockers = Vec::new();
    for round in rounds {
        let Some(_) = round_sha(round).filter(|sha| same_reviewed_sha(sha, &current_sha)) else {
            continue;
        };
        let reviewer = round
            .get("recorded_by")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown reviewer")
            .to_string();
        match VerdictKind::parse(
            round
                .get("verdict")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
        ) {
            VerdictKind::Approved => approvals.push(reviewer),
            VerdictKind::RequestChanges | VerdictKind::Rejected => blockers.push(reviewer),
            VerdictKind::Unknown => {}
        }
    }
    approvals.sort();
    approvals.dedup();
    blockers.sort();
    blockers.dedup();
    let independent = approvals
        .iter()
        .any(|approved| blockers.iter().any(|blocked| approved != blocked));
    if approvals.is_empty() || blockers.is_empty() || !independent {
        return None;
    }
    Some(format!(
        "conflicting review verdicts at {}: approved by {}; blocked by {}",
        short_sha(&current_sha),
        approvals.join(", "),
        blockers.join(", ")
    ))
}

/// Reconcile every review recording from multiple artifacts at `current_sha`.
/// Retained rounds are durable evidence, so an ill-formed or unattributed
/// round is an integrity error rather than something a later approval may
/// silently hide.
// trace:BUG-1581 | ai:codex
pub fn reconcile_artifacts_for_sha<'a>(
    bodies: impl IntoIterator<Item = &'a str>,
    current_sha: &str,
) -> Result<Option<VerdictKind>, String> {
    let mut approvals = Vec::new();
    let mut blockers = Vec::new();
    let mut saw_current = false;

    for body in bodies {
        let value: serde_json::Value = serde_json::from_str(body)
            .map_err(|e| format!("verdict artifact is not valid JSON: {e}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "verdict artifact is not a JSON object".to_string())?;
        let mut recordings = vec![(obj, false)];
        if let Some(rounds) = obj.get("rounds") {
            let rounds = rounds
                .as_array()
                .ok_or_else(|| "retained `rounds` is not an array".to_string())?;
            for (index, round) in rounds.iter().enumerate() {
                let round = round
                    .as_object()
                    .ok_or_else(|| format!("retained round {} is not a JSON object", index + 1))?;
                recordings.push((round, true));
            }
        }

        for (recording, retained) in recordings {
            let sha = round_sha(recording).ok_or_else(|| {
                if retained {
                    "retained round has no reviewed_sha/head provenance".to_string()
                } else {
                    "verdict artifact has no reviewed_sha/head provenance".to_string()
                }
            })?;
            let raw = recording
                .get("verdict")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| "review recording has no verdict".to_string())?;
            let kind = VerdictKind::parse(raw);
            if kind == VerdictKind::Unknown {
                return Err(format!("review recording has unrecognised verdict `{raw}`"));
            }
            let reviewer = recording
                .get("recorded_by")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            if retained && reviewer.is_none() {
                return Err("retained round has no recorded_by provenance".to_string());
            }
            if !same_reviewed_sha(&sha, current_sha) {
                continue;
            }
            saw_current = true;
            let reviewer = reviewer.unwrap_or("unknown reviewer").to_string();
            match kind {
                VerdictKind::Approved => approvals.push(reviewer),
                VerdictKind::RequestChanges | VerdictKind::Rejected => blockers.push(reviewer),
                VerdictKind::Unknown => unreachable!(),
            }
        }
    }

    approvals.sort();
    approvals.dedup();
    blockers.sort();
    blockers.dedup();
    if approvals
        .iter()
        .any(|approved| blockers.iter().any(|blocked| approved != blocked))
    {
        return Err(format!(
            "conflicting review verdicts at {}: approved by {}; blocked by {}",
            short_sha(current_sha),
            approvals.join(", "),
            blockers.join(", ")
        ));
    }
    if !blockers.is_empty() {
        Ok(Some(VerdictKind::RequestChanges))
    } else if !approvals.is_empty() {
        Ok(Some(VerdictKind::Approved))
    } else if saw_current {
        unreachable!()
    } else {
        Ok(None)
    }
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
            // trace:BUG-1622 | ai:claude
            crate::git_arg_guard::END_OF_OPTIONS,
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
            write_verdict_atomic(&path, &format!("{body}\n"))?;
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
    record_verdict_at_path(
        project_root,
        &path,
        verdict,
        reviewed_sha,
        reviewed_branch,
        summary,
        findings,
        recorded_by,
    )
}

/// Build the verdict JSON object for `path` without writing it. Split out of
/// [`record_verdict_at_path`] so a caller that must layer extra fields onto
/// the same record (the orchestrator's phase-3 handshake adds `mode`) can do
/// so before the single durable write, instead of writing once and then
/// read-modify-writing the same path again — every extra write to one path
/// is another window for BUG-1571's "reported written, wasn't" failure mode.
// trace:BUG-1571 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_verdict_object(
    project_root: &Path,
    path: &Path,
    verdict: Option<&str>,
    reviewed_sha: Option<&str>,
    reviewed_branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    recorded_by: &str,
) -> std::io::Result<serde_json::Map<String, serde_json::Value>> {
    let mut obj = std::fs::read_to_string(path)
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
        // Kept verbatim only when it is a commit ID: a value that later
        // reaches `git diff` must never be readable as an option.
        // trace:BUG-1622 | ai:claude
        .and_then(|s| {
            resolve_full_sha(project_root, s)
                .or_else(|| crate::git_arg_guard::is_hex_sha(s).then(|| s.to_string()))
        });
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
        canonical_verdict_word(verdict.unwrap_or_default()),
        findings
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    );
    archive_current_round(&mut obj, &incoming_key);
    // BUG-1529 review fix: a close belongs to the round it closed. A new
    // round of review must not inherit it, or a fresh refusal reads as
    // resolved. trace:BUG-1529 | ai:claude
    obj.remove("closed_by_merge");
    obj.remove("closed_at");
    // BUG-1505: every writer persists the canonical spelling, produced by the
    // one shared function rather than by each writer agreeing to be careful.
    // When the caller supplies no verdict (the drain stamping provenance onto
    // a reviewer-written file), the word already on disk is canonicalized in
    // place — that is how `aida drain reviewer` stops persisting whatever
    // spelling the reviewer phase produced. An unrecognised word is kept
    // verbatim: it has no canonical form, and readers treat it as Unknown.
    // trace:BUG-1505 | ai:claude
    let verdict_word = verdict
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            obj.get("verdict")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .map(|w| canonical_verdict_word(&w));
    let mut set = |k: &str, v: Option<&str>| {
        if let Some(v) = v.map(str::trim).filter(|s| !s.is_empty()) {
            obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
        }
    };
    set("verdict", verdict_word.as_deref());
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
        // STORY-1417: `finding_classes` is positional against `findings`, so
        // new findings invalidate the old classes. A caller that has classes
        // for these findings re-applies them via
        // `review_classes::apply_finding_classes`.
        // trace:STORY-1417 | ai:claude
        obj.remove("finding_classes");
    }
    Ok(obj)
}

/// Record a verdict at an explicitly anchored artifact path. The orchestrator
/// uses this for `PR-N.json`; sharing this boundary with spec-keyed records is
/// what prevents the authoritative handshake from silently clobbering another
/// reviewer's evidence. Builds the object via [`build_verdict_object`] and
/// performs exactly one durable, verified write (BUG-1571) — a caller that
/// needs to add fields on top of the same record should call
/// `build_verdict_object` + `write_verdict_object` directly rather than
/// writing here and then again, which is the double-write BUG-1571 removed.
// trace:BUG-1581 | ai:codex
// trace:BUG-1571 | ai:claude
#[allow(clippy::too_many_arguments)]
pub fn record_verdict_at_path(
    project_root: &Path,
    path: &Path,
    verdict: Option<&str>,
    reviewed_sha: Option<&str>,
    reviewed_branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    recorded_by: &str,
) -> std::io::Result<PathBuf> {
    let obj = build_verdict_object(
        project_root,
        path,
        verdict,
        reviewed_sha,
        reviewed_branch,
        summary,
        findings,
        recorded_by,
    )?;
    write_verdict_object(path, &obj)?;
    Ok(path.to_path_buf())
}

/// Serialize `obj` and durably write it to `path` (BUG-1571: atomic
/// rename + read-back verification via [`write_verdict_atomic`]). Public so
/// a caller that folds extra fields onto a [`build_verdict_object`] result
/// (the orchestrator's phase-3 handshake) still writes through the one
/// verified boundary instead of a bare `fs::write`.
// trace:BUG-1571 | ai:claude
pub(crate) fn write_verdict_object(
    path: &Path,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    // TASK-1460: files recorded before the per-sha archive existed are
    // migrated into it once, lazily, the first time anything records here —
    // BEFORE this write, so the migration sees the pre-existing state.
    // Best-effort: a failed migration never costs the record being written.
    // trace:TASK-1460 | ai:claude
    if let Some(dir) = path.parent() {
        if let Err(e) = ensure_sidecars_archived(dir) {
            eprintln!(
                "warning: could not migrate existing verdicts in {} into the per-commit archive: {e}",
                dir.display()
            );
        }
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(obj.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    write_verdict_atomic(path, &format!("{body}\n"))?;
    // BUG-1539: the current file above is the compatibility contract every
    // existing reader depends on; the per-sha archive is the history. It is
    // written second so a failure here can never cost the current record,
    // and it is best-effort for the same reason — but loudly, because a
    // silently missing archive is exactly the invisible loss this fixes.
    // trace:BUG-1539 | ai:claude
    if let Err(e) = archive_round_by_sha(path, obj) {
        eprintln!(
            "warning: recorded {} but could not archive the round by reviewed commit: {e}",
            path.display()
        );
    }
    Ok(())
}

/// BUG-1539: the directory holding every archived round of the verdict file
/// at `path` — `.aida/review-verdicts/PR-<N>.json` archives into
/// `.aida/review-verdicts/PR-<N>/`, one `<reviewed_sha>.json` per commit
/// reviewed. A directory carries no `.json` extension, so every reader that
/// walks `.aida/review-verdicts/` for `*.json` files is untouched by it.
// trace:BUG-1539 | ai:claude
pub fn verdict_archive_dir(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?.trim();
    if stem.is_empty() {
        return None;
    }
    Some(path.parent()?.join(stem))
}

/// A reviewed sha usable as a file name: hex only, long enough to be a
/// commit. Anything else is unidentifiable and is not archived (the same
/// refusal [`archive_current_round`] makes for a round with no commit).
fn archivable_sha(sha: &str) -> Option<&str> {
    let sha = sha.trim();
    (sha.len() >= 7 && sha.len() <= 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some(sha)
}

/// BUG-1539: a verdict is about a COMMIT, but the current file is keyed by
/// PR number or spec, so a later round replaces it. Archive this round at
/// `<stem>/<reviewed_sha>.json` so a round never destroys the one before it
/// and a reader can ask "is there a verdict for THIS commit".
///
/// Idempotent per commit: re-recording the same review at the same sha
/// rewrites one file and adds nothing. A DIFFERENT recording at the same sha
/// (a second reviewer) is kept in that file's own `rounds`, through the same
/// [`archive_current_round`] rule the current file uses, so two reviewers at
/// one commit read as one commit with two recordings, never as a silent
/// overwrite.
// trace:BUG-1539 | ai:claude
fn archive_round_by_sha(path: &Path, obj: &JsonObj) -> std::io::Result<()> {
    let Some(sha) = round_sha(obj) else {
        return Ok(());
    };
    let Some(sha) = archivable_sha(&sha) else {
        return Ok(());
    };
    if obj
        .get("verdict")
        .and_then(|v| v.as_str())
        .is_none_or(|v| v.trim().is_empty())
    {
        return Ok(());
    }
    let Some(dir) = verdict_archive_dir(path) else {
        return Ok(());
    };
    let archive = dir.join(format!("{}.json", sha.to_ascii_lowercase()));
    let mut snapshot = obj.clone();
    snapshot.remove("rounds");
    let mut existing: JsonObj = std::fs::read_to_string(&archive)
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    archive_current_round(&mut existing, &recording_key(&snapshot));
    if let Some(rounds) = existing.remove("rounds") {
        snapshot.insert("rounds".to_string(), rounds);
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(snapshot))
        .unwrap_or_else(|_| "{}".to_string());
    write_verdict_atomic(&archive, &format!("{body}\n"))
}

/// BUG-1539: the verdict recorded under `key` (a `PR-<N>` or spec id) for the
/// commit `sha`, whichever round it was. The current file answers first when
/// it was taken at `sha`; otherwise the per-sha archive does. Prefix-tolerant
/// the same way every other sha comparison here is ([`same_reviewed_sha`]).
// trace:BUG-1539 | ai:claude
// trace:TASK-1460 | ai:claude
pub fn read_verdict_for_sha(project_root: &Path, key: &str, sha: &str) -> Option<RecordedVerdict> {
    let path = verdict_file_for_sha(project_root, key, sha)?;
    std::fs::read_to_string(path)
        .ok()
        .and_then(|b| parse_recorded_verdict(&b))
}

/// TASK-1460: the FILE holding the verdict recorded under `key` for commit
/// `sha` — the current file when it was taken at `sha`, else the newest
/// matching archive file. The phase-3 handshake needs the file (its reader
/// checks conflicts, escalation and freshness on the artifact itself), not
/// just the parsed verdict.
// trace:TASK-1460 | ai:claude
pub fn verdict_file_for_sha(project_root: &Path, key: &str, sha: &str) -> Option<PathBuf> {
    let path = verdict_path(project_root, key);
    let current_at_sha = std::fs::read_to_string(&path)
        .ok()
        .and_then(|b| parse_recorded_verdict(&b))
        .and_then(|v| v.reviewed_sha)
        .is_some_and(|r| same_reviewed_sha(&r, sha));
    if current_at_sha {
        return Some(path);
    }
    archived_verdict_file_for_sha(&path, sha)
}

/// TASK-1460: the archived round of the verdict file at `path` for commit
/// `sha` (never the current file itself), newest recording first when more
/// than one archive file matches a short sha.
// trace:TASK-1460 | ai:claude
pub fn archived_verdict_file_for_sha(path: &Path, sha: &str) -> Option<PathBuf> {
    archived_files_in_dir(&verdict_archive_dir(path)?, sha)
        .into_iter()
        .next()
        .map(|(p, _)| p)
}

/// BUG-1539: every archived verdict for commit `sha` under ANY key — the
/// answer for a duplicate or re-opened PR over the same tree, which has no
/// file of its own. One entry per archive file (per key), so one review
/// seen through two PR numbers reads as that review under each reference,
/// newest recording first.
// trace:BUG-1539 | ai:claude
pub fn verdicts_for_sha(project_root: &Path, sha: &str) -> Vec<RecordedVerdict> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<RecordedVerdict> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|d| archived_in_dir(&d, sha))
        .collect();
    out.sort_by(|a, b| b.recorded_at.cmp(&a.recorded_at));
    out
}

fn archived_in_dir(dir: &Path, sha: &str) -> Vec<RecordedVerdict> {
    archived_files_in_dir(dir, sha)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

fn archived_files_in_dir(dir: &Path, sha: &str) -> Vec<(PathBuf, RecordedVerdict)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, RecordedVerdict)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| same_reviewed_sha(stem, sha))
        })
        .filter_map(|p| {
            let v = parse_recorded_verdict(&std::fs::read_to_string(&p).ok()?)?;
            Some((p, v))
        })
        .collect();
    out.sort_by(|a, b| b.1.recorded_at.cmp(&a.1.recorded_at));
    out
}

/// TASK-1460: archive a verdict file that was written OUTSIDE the record
/// path — the `/aida-review` skill writes `AIDA_REVIEW_VERDICT_FILE` with a
/// shell heredoc — so its round lands in the per-sha archive exactly as a
/// recorded one would. The current file is left byte-for-byte as written.
/// A file with no verdict or no reviewed commit is unidentifiable and is not
/// archived (the same refusal the record path makes). Idempotent.
// trace:TASK-1460 | ai:claude
pub fn adopt_direct_write(path: &Path) -> std::io::Result<()> {
    let Some(obj) = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
    else {
        return Ok(());
    };
    archive_round_by_sha(path, &obj)
}

/// TASK-1460: the marker that says the one-shot sidecar migration has run in
/// a verdict directory. A dotfile with no `.json` extension, so every `*.json`
/// walker of the directory is untouched by it.
const MIGRATION_MARKER: &str = ".archive-migrated";

/// TASK-1460: run [`migrate_sidecars_to_archive`] once per verdict directory.
/// Called lazily from the record path; the marker makes every later call a
/// single `stat`.
// trace:TASK-1460 | ai:claude
pub fn ensure_sidecars_archived(dir: &Path) -> std::io::Result<usize> {
    let marker = dir.join(MIGRATION_MARKER);
    if marker.exists() || !dir.is_dir() {
        return Ok(0);
    }
    let migrated = migrate_sidecars_to_archive(dir)?;
    std::fs::write(&marker, format!("{migrated}\n"))?;
    Ok(migrated)
}

/// TASK-1460: move every verdict recorded before BUG-1539's per-sha archive
/// into it — each top-level `<KEY>.json` in `dir`, its archived `rounds`
/// oldest first and then its current round, lands at `<KEY>/<sha>.json`
/// through the same [`archive_round_by_sha`] the record path uses.
///
/// Idempotent: a commit that already has an archive file is skipped whole,
/// so a re-run (or a round the record path already archived) adds nothing
/// and can never reorder a newer recording under an older one. Rounds with
/// no reviewed commit are unidentifiable and stay where they are. The
/// top-level files are never modified. Returns the archive files created.
// trace:TASK-1460 | ai:claude
pub fn migrate_sidecars_to_archive(dir: &Path) -> std::io::Result<usize> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(0);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'))
        })
        .collect();
    files.sort();
    let mut created = 0;
    for path in files {
        let Some(obj) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
            .and_then(|v| v.as_object().cloned())
        else {
            continue;
        };
        let Some(archive_dir) = verdict_archive_dir(&path) else {
            continue;
        };
        let mut recordings: Vec<JsonObj> = obj
            .get("rounds")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
            .unwrap_or_default();
        let mut current = obj.clone();
        current.remove("rounds");
        recordings.push(current);
        // Decide per commit up front, so a commit is either migrated whole or
        // (already archived) not touched at all.
        let mut fresh: Vec<String> = Vec::new();
        for rec in &recordings {
            let Some(sha) = round_sha(rec) else { continue };
            let Some(sha) = archivable_sha(&sha).map(str::to_ascii_lowercase) else {
                continue;
            };
            if !fresh.contains(&sha) && !archive_dir.join(format!("{sha}.json")).exists() {
                fresh.push(sha);
            }
        }
        for rec in &recordings {
            let in_fresh = round_sha(rec)
                .as_deref()
                .and_then(archivable_sha)
                .is_some_and(|s| fresh.contains(&s.to_ascii_lowercase()));
            if in_fresh {
                archive_round_by_sha(&path, rec)?;
            }
        }
        created += fresh.len();
    }
    Ok(created)
}

/// BUG-1529 criterion 1: close a spec's outstanding refusal out when its
/// reworked work merges. Called from the same place `auto_bump_done_to_completed`
/// (and its stranded/stale-review siblings in `lib.rs`) already detect a
/// Done→Completed transition, so this reuses their merge-detection rather than
/// re-deriving it.
///
/// Deliberately narrow:
///   - a no-op (`Ok(false)`) when there is no verdict file, when the recorded
///     verdict is not blocking (nothing to close — an approval was never a
///     refusal), or when it is already closed (first closing sha wins; this
///     never overwrites `closed_by_merge`, so an idempotent re-run of the
///     auto-bump scan can't spuriously rewrite the record).
///   - never touches `verdict`, `summary`, `findings`, or `rounds` — closing
///     is an ANNOTATION on the refusal, not a new review. Criterion 2: a
///     closed refusal must stay distinguishable from a fresh approving
///     review, and overwriting the verdict word would erase that distinction
///     (and the audit trail STORY-1391 exists to keep).
///
/// `merge_ref` is normally the merge/landing commit sha; when the landing
/// commit is unknown (e.g. the stranded-review-PR path, which only has a PR
/// number from the forge) callers pass a `PR-<n>` marker instead — either way
/// it is a human-readable pointer to WHAT closed the refusal, and an empty
/// string is refused rather than silently recorded as a closer with no
/// evidence.
// trace:BUG-1529 | ai:claude
pub fn close_verdict_on_merge(
    project_root: &Path,
    spec: &str,
    merge_ref: &str,
) -> std::io::Result<bool> {
    let merge_ref = merge_ref.trim();
    if merge_ref.is_empty() {
        return Ok(false);
    }
    let path = verdict_path(project_root, spec);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(serde_json::Value::Object(mut obj)) = serde_json::from_str(&body) else {
        return Ok(false);
    };
    let Some(raw_verdict) = obj.get("verdict").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    if !VerdictKind::parse(raw_verdict).blocks_done() {
        return Ok(false);
    }
    let already_closed = obj
        .get("closed_by_merge")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if already_closed {
        return Ok(false);
    }
    obj.insert(
        "closed_by_merge".to_string(),
        serde_json::Value::String(merge_ref.to_string()),
    );
    obj.insert(
        "closed_at".to_string(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    let pretty = serde_json::to_string_pretty(&serde_json::Value::Object(obj.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    write_verdict_atomic(&path, &format!("{pretty}\n"))?;
    // TASK-1460: the current round is also archived at its reviewed commit;
    // close it there too, or `verdicts_for_sha` / `read_verdict_for_sha`
    // would still report this refusal as open. The archive's top level is
    // this same recording, so re-archiving replaces it with the closed copy
    // and keeps that commit's earlier recordings in its `rounds`.
    // trace:TASK-1460 | ai:claude
    archive_round_by_sha(&path, &obj)?;
    Ok(true)
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

/// Whether a routed reviewer-queue entry is actionable BY THE REVIEWER, or
/// whether it already has a verdict covering the current head and the real
/// next step lies elsewhere. BUG-1508: a routed entry that doesn't say this
/// reads as review-to-do even when four out of five are really rework, so a
/// queue that "reads five-deep" can in fact have one real review outstanding.
// trace:BUG-1508 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewActionability {
    /// No verdict exists that provably covers the current head: no verdict
    /// at all, a verdict whose sha differs from the head (advanced or
    /// rewritten), OR a verdict that can never be placed against a head
    /// (no `reviewed_sha` recorded) — criterion 8 treats that last case as
    /// ABSENT rather than reassuring, per PRIN-5. Actionable to the reviewer.
    NeedsReview,
    /// A blocking verdict (request-changes / rejected) covers the EXACT
    /// current head. The spec still needs attention — criterion 2 — but the
    /// attention is rework by the implementer, not a fresh review.
    AwaitingRework,
    /// A non-blocking verdict (approved) covers the exact current head.
    /// Nothing further is owed to the reviewer role for this head.
    Resolved,
}

impl ReviewActionability {
    /// Stable machine-readable token (used for `--json` output and the
    /// queue-list annotation). Never a user-facing sentence.
    // trace:BUG-1508 | ai:claude
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewActionability::NeedsReview => "needs-review",
            ReviewActionability::AwaitingRework => "awaiting-rework",
            ReviewActionability::Resolved => "resolved",
        }
    }
}

/// Classify a routed reviewer entry's actionability from its recorded
/// verdict and where the current head sits relative to it (criterion 1).
///
/// Deliberately re-derived from live inputs every call rather than cached:
/// when the head later moves, `relation` stops being `AtReviewedSha` on the
/// very next read and the entry reappears as `NeedsReview` with no special
/// handling required — criterion 3.
///
/// `TipRelation::Unknown` already covers both "no `reviewed_sha` was ever
/// recorded" (criterion 8's permanently-indeterminate population — 409 of
/// 543 verdict files corpus-wide) and "the ancestry probe itself failed" —
/// both fold into `NeedsReview` here, the fail-closed, PRIN-5-consistent
/// answer: absent evidence is never read as good evidence.
// trace:BUG-1508 | ai:claude
pub fn review_actionability(
    verdict: Option<&RecordedVerdict>,
    relation: TipRelation,
) -> ReviewActionability {
    let Some(v) = verdict else {
        return ReviewActionability::NeedsReview;
    };
    // BUG-1529: a refusal that was closed out by a merge is not something the
    // reviewer role still owes rework on — the branch that would have
    // answered it already shipped. Checked ahead of the sha relation so a
    // closed record reads Resolved even though the head has since moved past
    // the reviewed sha (it always has, by the time a merge closes it).
    if v.is_closed() {
        return ReviewActionability::Resolved;
    }
    if relation != TipRelation::AtReviewedSha {
        return ReviewActionability::NeedsReview;
    }
    if v.kind.blocks_done() {
        ReviewActionability::AwaitingRework
    } else {
        ReviewActionability::Resolved
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
    // An unrecognised verdict word is not evidence of approval. `handle_review_record`
    // already refuses to WRITE one (`VerdictKind::Unknown` bails before the file is
    // written), but an older/hand-edited file can still carry a word `parse` does not
    // recognise, and falling through the way `blocks_done() == false` handles a real
    // Approved verdict would silently treat "could not classify" as "passed" — the same
    // shape as the preflight Skipped-funnels-to-Open defect. Refuse and say why instead.
    // trace:BUG-1507 | ai:claude (PRIN-5: absent is not good evidence)
    if v.kind == VerdictKind::Unknown {
        return VerdictGate::Refuse(
            vec![
                format!(
                    "error: aida queue done refused (exit 1) — the last review of {display_id} \
                     recorded an unrecognised verdict word `{}`, so this check cannot determine \
                     whether the review passed or blocked.",
                    v.raw
                ),
                summary_line(v),
                "A review gate that cannot classify the verdict must not wave work through. \
                 Record a fresh, recognised verdict against the current head: `aida review \
                 record` accepts approved, request-changes, or rejected."
                    .to_string(),
            ]
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect(),
        );
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
            let body = std::fs::read_to_string(&p).unwrap_or_default();
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
#[path = "tests/review_verdict_tests.rs"]
mod review_verdict_tests;
