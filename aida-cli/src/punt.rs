//! `aida punt` — the design-fork punt safety net for autonomous drains
//! (STORY-332).
//!
//! When an autonomous implementer/reviewer (a `--no-human` drain) hits a
//! design-fork it cannot safely resolve, guessing produces a *silent wrong
//! implementation*. The honest move is to **punt**: flip the spec to
//! `NeedsAttention`, record the structured why, and return control so the
//! orchestrator advances to the next item rather than stalling.
//!
//! This module owns the deterministic pieces — the punt-ledger record and
//! its append, plus the CLI category parser. The `aida punt` command handler
//! (which loads the spec, enforces the transition, and persists) lives in
//! `main.rs` next to the other command handlers, mirroring `findings.rs`.
//!
//! The ledger (`.aida/punts.jsonl`) is the forward-compatible seed for
//! STORY-325's analysis layer: STORY-332 writes one structured record per
//! punt; STORY-325 builds classification + pattern analysis on top. The file
//! lives under `.aida/` (gitignored) and is local-only — a punt record names
//! the spec and the fork, so it stays the project's own decision history.
//!
//! trace:STORY-332 | ai:claude

use std::io::Write;
use std::path::{Path, PathBuf};

use aida_core::PuntCategory;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One append-only line in `.aida/punts.jsonl` — a single punt decision.
///
/// Field shape is deliberately forward-compatible with STORY-325's richer
/// ledger: STORY-325 adds derived fields (classification, escalation reason,
/// outcome) without breaking these. `resolution_path` starts at `"punted"`
/// and STORY-325's analysis layer is what later records how it resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PuntRecord {
    /// When the punt was raised.
    pub timestamp: DateTime<Utc>,
    /// Display ID of the spec that was punted.
    pub spec: String,
    /// The obstacle category the raiser picked.
    pub category: PuntCategory,
    /// Human-readable description of the fork / obstacle.
    pub detail: String,
    /// The raiser's best guess if forced to choose — distinct from `detail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<String>,
    /// Role / agent that raised the punt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raised_by: Option<String>,
    /// How the punt resolved. `"punted"` for a plain implementer punt;
    /// STORY-306's advisor tier writes `"advisor-resolved"`,
    /// `"escalated-to-human"`, or `"escalate-defaulted"`; STORY-325's
    /// analysis layer records later outcomes. Kept a string for forward
    /// compatibility.
    pub resolution_path: String,
    /// STORY-306: the A/B/C calibration class a headless advisor assigned
    /// this fork (`"A"` recorded-principle, `"B"` recorded-preference,
    /// `"C"` synthesized-context). `None` for a plain implementer punt the
    /// advisor never judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// STORY-306: on an advisor escalation, the categorized reason a human
    /// is needed (e.g. `"strategy"`, `"irreversible"`, `"unrecorded-context"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
    /// STORY-306: the answer the advisor resolved the fork with — present
    /// when `resolution_path` is `"advisor-resolved"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// STORY-306: who produced the resolution — e.g. `"advisor"`. `None` for
    /// a plain implementer punt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    /// STORY-325: What was decided — resolved, escalated, deferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// STORY-325: Link to the memory entry or recorded principle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principle_link: Option<String>,
    /// STORY-325: When calibration shadow verdicts ran side-by-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_pair: Option<String>,
    /// STORY-325: Timestamp when the design-fork paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<DateTime<Utc>>,
    /// STORY-325: Timestamp when the design-fork resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Path to the punt ledger for a project, given its root directory.
pub fn ledger_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("punts.jsonl")
}

/// The three empirical design forks of the day, seeded for backfill as a structured corpus.
pub fn get_backfill_seeds() -> Vec<PuntRecord> {
    use chrono::TimeZone;
    vec![
        PuntRecord {
            timestamp: Utc.with_ymd_and_hms(2026, 5, 22, 20, 0, 0).unwrap(),
            spec: "TASK-340".to_string(),
            category: PuntCategory::DesignFork,
            detail: "implementer punted on PuntRecord.resolved_at + drain-state persistence design forks".to_string(),
            lean: None,
            raised_by: Some("implementer".to_string()),
            resolution_path: "punted".to_string(),
            classification: Some("PREREQ-GAP".to_string()),
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: Some("deferred".to_string()),
            principle_link: None,
            calibration_pair: None,
            paused_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 20, 0, 0).unwrap()),
            resolved_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 21, 30, 0).unwrap()),
        },
        PuntRecord {
            timestamp: Utc.with_ymd_and_hms(2026, 5, 22, 21, 0, 0).unwrap(),
            spec: "TASK-403".to_string(),
            category: PuntCategory::Other,
            detail: "demo-meta-task misrouted to implementer queue".to_string(),
            lean: None,
            raised_by: Some("implementer".to_string()),
            resolution_path: "advisor-resolved".to_string(),
            classification: Some("SCOPE-MISMATCH".to_string()),
            escalation_reason: None,
            answer: Some("route to operator queue".to_string()),
            answered_by: Some("advisor".to_string()),
            decision: Some("route-to-operator".to_string()),
            principle_link: Some("established operator curation tasks".to_string()),
            calibration_pair: None,
            paused_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 21, 0, 0).unwrap()),
            resolved_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 21, 5, 0).unwrap()),
        },
        PuntRecord {
            timestamp: Utc.with_ymd_and_hms(2026, 5, 22, 22, 0, 0).unwrap(),
            spec: "TASK-440".to_string(),
            category: PuntCategory::DesignFork,
            detail: "spec body says 'defer until first MCP client surfaces need' but spec sits Approved".to_string(),
            lean: None,
            raised_by: Some("implementer".to_string()),
            resolution_path: "escalated-to-human".to_string(),
            classification: Some("DEFER-VS-DO-CONTRADICTION".to_string()),
            escalation_reason: Some("genuinely-needs-human-judgment".to_string()),
            answer: None,
            answered_by: Some("advisor".to_string()),
            decision: Some("escalated".to_string()),
            principle_link: None,
            calibration_pair: None,
            paused_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 22, 0, 0).unwrap()),
            resolved_at: Some(Utc.with_ymd_and_hms(2026, 5, 22, 22, 45, 0).unwrap()),
        },
    ]
}

/// `resolution_path` slug for an orchestrator shelving on phase failure
/// (EPIC-28). The punt ledger is the project's "paused decisions" corpus —
/// a phase failure that parked a spec in `NeedsAttention` belongs in the
/// same file as a design-fork punt, distinguished by this slug.
/// trace:EPIC-28 | ai:claude
pub const RESOLUTION_SHELVED_FAILURE: &str = "shelved-by-failure";

/// Append a `shelved-by-failure` record to `.aida/punts.jsonl` derived from
/// the structured [`aida_core::FailureReason`] the orchestrator just wrote
/// onto the spec. Reuses the punt ledger (rather than a sibling file) so
/// STORY-325 analysis sees one corpus of "paused decisions"; the discriminator
/// is `PuntRecord::resolution_path == RESOLUTION_SHELVED_FAILURE`.
///
/// Best-effort: a ledger-write failure here must not undo the spec's status
/// flip, which is the load-bearing part. trace:EPIC-28 | ai:claude
pub fn append_failure_to_ledger(
    project_root: &Path,
    spec: &str,
    fr: &aida_core::FailureReason,
) -> anyhow::Result<()> {
    let record = PuntRecord {
        timestamp: fr.shelved_at,
        spec: spec.to_string(),
        category: PuntCategory::Other,
        detail: fr.detail.clone(),
        lean: None,
        raised_by: fr.shelved_by.clone(),
        resolution_path: RESOLUTION_SHELVED_FAILURE.to_string(),
        classification: None,
        escalation_reason: None,
        answer: None,
        answered_by: None,
        // The phase slug rides on `decision` so consumers can filter by
        // `failure:ci` etc. without parsing `detail`.
        decision: Some(format!("failure:{}", fr.phase)),
        principle_link: None,
        calibration_pair: None,
        paused_at: Some(fr.shelved_at),
        resolved_at: None,
    };
    append_to_ledger(project_root, &record)
}

/// Append one punt record to `.aida/punts.jsonl`, creating the file (and the
/// `.aida/` directory) if needed. One JSON object per line.
///
/// STORY-361: the serialized line + `\n` is written in a single `write_all`
/// call so POSIX `O_APPEND` atomicity holds under concurrent writers. (The
/// earlier `writeln!` form made multiple `write(2)` syscalls per record, so
/// two concurrent writers could interleave content and newline, producing
/// a torn JSON line both consumers dropped.) trace:STORY-361
pub fn append_to_ledger(project_root: &Path, record: &PuntRecord) -> anyhow::Result<()> {
    // Check telemetry opt-out
    if !crate::usage::is_enabled(Some(project_root)) {
        return Ok(());
    }
    let path = ledger_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Read every punt-ledger record from `.aida/punts.jsonl`, in append order
/// (oldest first). Bad or forward-incompatible lines are skipped rather than
/// aborting the read; an absent ledger reads as empty. trace:STORY-306
pub fn read_ledger(project_root: &Path) -> Vec<PuntRecord> {
    let path = ledger_path(project_root);
    if !path.exists() {
        // Automatically backfill with the empirical seeds if telemetry is allowed
        let seeds = get_backfill_seeds();
        if crate::usage::is_enabled(Some(project_root)) {
            for seed in &seeds {
                let _ = append_to_ledger(project_root, seed);
            }
        }
        return seeds;
    }
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<PuntRecord>(l).ok())
        .collect()
}

// --- Autonomy maturity roll-up (TASK-340) -----------------------------------
//
// The honest maturity signal the operator asked for: how often a drain had to
// stop and ask a human. A `resolution_path == "escalated-to-human"` punt record
// is exactly one such intervention. The COUNT trending toward zero as the
// autonomy machinery matures (--zen, headless implementer, orchestrator
// hardening) is the clean signal — unlike a raw duration fraction, it is NOT
// polluted by human-availability latency (an overnight wait inflates "waiting"
// without the system being any less mature).
//
// Punt records carry no explicit drain id, so we roll up by the UTC calendar
// day the escalation was raised — the "rolled up per day" view the spec calls
// for, and the dated grain that makes the across-drains trend readable. This
// is a deliberate approximation (one day ≈ one drain session for the typical
// overnight/at-keyboard cadence); the duration fraction is skipped entirely
// (operator decision 2026-06-06) until it is actually wanted.
// trace:TASK-340 | ai:claude

/// `resolution_path` slug for a punt that the headless advisor tier could not
/// resolve and handed back to a human — the unit of "human intervention" the
/// autonomy-maturity metric counts. trace:TASK-340 | ai:claude
pub const RESOLUTION_ESCALATED_TO_HUMAN: &str = "escalated-to-human";

/// One day's worth of human-intervention activity, derived from the punt
/// ledger. `date` is the UTC calendar day; `interventions` is the number of
/// escalate-to-human punt records raised that day. trace:TASK-340 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutonomyDay {
    /// UTC calendar day, `YYYY-MM-DD`.
    pub date: String,
    /// Count of escalate-to-human punt records raised that day.
    pub interventions: usize,
}

/// Whether a punt record represents a human intervention (an escalation a
/// human had to resolve). Both the `resolution_path` slug and STORY-325's
/// `decision == "escalated"` field are honored so older and newer record
/// shapes both count. trace:TASK-340 | ai:claude
pub fn is_human_intervention(r: &PuntRecord) -> bool {
    r.resolution_path == RESOLUTION_ESCALATED_TO_HUMAN || r.decision.as_deref() == Some("escalated")
}

/// Roll up escalate-to-human punt records into a per-day intervention count,
/// newest day first. Pure over a slice of records so it is unit-testable
/// against fixture data. Days with zero escalations are omitted (the ledger
/// only records decisions, not idle time). trace:TASK-340 | ai:claude
pub fn human_interventions_by_day(records: &[PuntRecord]) -> Vec<AutonomyDay> {
    use std::collections::BTreeMap;
    let mut by_day: BTreeMap<String, usize> = BTreeMap::new();
    for r in records.iter().filter(|r| is_human_intervention(r)) {
        let day = r.timestamp.format("%Y-%m-%d").to_string();
        *by_day.entry(day).or_insert(0) += 1;
    }
    // BTreeMap iterates oldest-first by string date; reverse for newest-first.
    by_day
        .into_iter()
        .rev()
        .map(|(date, interventions)| AutonomyDay {
            date,
            interventions,
        })
        .collect()
}

/// Total escalate-to-human interventions across all supplied records.
/// trace:TASK-340 | ai:claude
pub fn total_human_interventions(records: &[PuntRecord]) -> usize {
    records.iter().filter(|r| is_human_intervention(r)).count()
}

// --- Orchestrator punt-signal handshake (STORY-276) -------------------------
//
// A headless `--auto-complete --no-human=both` implementer that hits a
// design-fork runs `aida punt` — but the orchestrator, watching from the
// main worktree, has no in-band way to learn the spec was parked rather than
// shipped. So `aida punt` drops a small signal file the orchestrator polls
// for after the implementer session exits. This mirrors the reviewer's
// `AIDA_REVIEW_VERDICT_FILE` handshake exactly: the orchestrator provisions
// an absolute path, passes it via an env var, and reads it back — making the
// handshake independent of how `.aida/` resolves across git worktrees.
// trace:STORY-276 | ai:claude

/// Env var the `--auto-complete` orchestrator sets on the implementer
/// subprocess. `aida punt` writes its signal file here when the var is set.
pub const SIGNAL_FILE_ENV: &str = "AIDA_PUNT_SIGNAL_FILE";

/// STORY-306: env var the orchestrator sets on the headless *advisor*
/// subprocess, pointing at the [`PuntRequest`] payload file it should read.
pub const REQUEST_FILE_ENV: &str = "AIDA_PUNT_REQUEST_FILE";

/// STORY-306: env var the orchestrator sets on the headless *advisor*
/// subprocess, pointing at the path it should write its [`PuntResponse`] to.
pub const RESPONSE_FILE_ENV: &str = "AIDA_PUNT_RESPONSE_FILE";

/// The payload of a punt signal file — enough for the orchestrator to confirm
/// a punt happened and name the fork in its run epilogue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PuntSignal {
    /// Display ID of the punted spec.
    pub spec: String,
    /// The obstacle category the raiser picked.
    pub category: PuntCategory,
    /// Human-readable description of the fork / obstacle.
    pub detail: String,
    /// The raiser's best guess if forced to choose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<String>,
}

impl PuntSignal {
    /// One-line punt summary for the orchestrator's run epilogue.
    pub fn summary(&self) -> String {
        match &self.lean {
            Some(l) => format!("[{}] {} (lean: {l})", self.category, self.detail),
            None => format!("[{}] {}", self.category, self.detail),
        }
    }
}

/// Path the orchestrator provisions for a spec's punt signal —
/// `.aida/punt-signals/<spec>.json` under the (main) project root.
pub fn signal_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("punt-signals")
        .join(format!("{spec}.json"))
}

/// Write a punt signal file, creating `.aida/punt-signals/` if needed.
pub fn write_signal(path: &Path, signal: &PuntSignal) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(signal)?)?;
    Ok(())
}

/// Read a punt signal file. `None` when the file is absent or unparseable —
/// either way the orchestrator reads it as "no punt happened".
pub fn read_signal(path: &Path) -> Option<PuntSignal> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

// --- Orchestrator PR-hold handshake (BUG-250) -------------------------------
//
// A deliberate *push-branch, hold-PR* finish is a legitimate phase-1 outcome:
// the implementer backed the branch up on origin but is holding the PR until a
// manual gate runs (a smoke test, an out-of-band review, an operator decision).
// Without a signal the orchestrator sees "branch pushed, no PR" and mis-files
// it as a phase-1 *failure* with a wrong recovery hint ("run /aida-pr"), which
// for a deliberate hold would ship un-gated code (BUG-250).
//
// So `aida pr hold` drops a small signal file — mirroring the punt handshake
// exactly: the orchestrator provisions an absolute path under the *main*
// worktree root, passes it via [`HOLD_SIGNAL_FILE_ENV`], and reads it back
// after the implementer session exits. A hold is NOT a punt (no design-fork,
// no NeedsAttention) — it is a clean, deliberate stop — so it gets its own
// signal type and its own [`crate::auto_complete::ImplementerOutcome::Held`]
// drain action. trace:BUG-250 | ai:claude

/// Env var the `--auto-complete` orchestrator sets on the implementer
/// subprocess. `aida pr hold` writes its signal file here when the var is set.
pub const HOLD_SIGNAL_FILE_ENV: &str = "AIDA_HOLD_SIGNAL_FILE";

/// The payload of a PR-hold signal file — enough for the orchestrator to
/// confirm a deliberate hold happened and name the held branch + reason in its
/// run epilogue and recovery hint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HoldSignal {
    /// Display ID of the spec whose PR is deliberately held.
    pub spec: String,
    /// The branch that was pushed (so the epilogue's `gh pr create` hint and a
    /// later resume can target it).
    pub branch: String,
    /// Why the PR is held — the manual gate the operator is running first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HoldSignal {
    /// One-line hold summary for the orchestrator's run epilogue.
    pub fn summary(&self) -> String {
        match &self.reason {
            Some(r) => format!("PR held on `{}` — {r}", self.branch),
            None => format!("PR held on `{}`", self.branch),
        }
    }
}

/// Path the orchestrator provisions for a spec's PR-hold signal —
/// `.aida/pr-holds/<spec>.json` under the (main) project root.
pub fn hold_signal_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("pr-holds")
        .join(format!("{spec}.json"))
}

/// Write a PR-hold signal file, creating `.aida/pr-holds/` if needed.
pub fn write_hold_signal(path: &Path, signal: &HoldSignal) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(signal)?)?;
    Ok(())
}

/// Read a PR-hold signal file. `None` when the file is absent or unparseable —
/// either way the orchestrator reads it as "no deliberate hold happened".
pub fn read_hold_signal(path: &Path) -> Option<HoldSignal> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

// --- Advisor punt-request / punt-response channel (STORY-306) ---------------
//
// STORY-306 inserts a headless *advisor* tier between the implementer's punt
// and the human. When phase 1 punts, the orchestrator assembles a rich,
// ultraplan-grade payload — a `PuntRequest` — writes it to a file, and spawns
// a headless advisor. The advisor judges the fork and writes a `PuntResponse`
// back. The channel is the proven file-based async handshake (STORY-263
// verdict files, STORY-285 findings, the TASK-329 sentinel): one request file
// in, one response file out, both under `.aida/punts/`. trace:STORY-306

/// What a headless advisor decided about a punted design-fork.
/// trace:STORY-306 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PuntResolution {
    /// The advisor judged the fork — `PuntResponse::answer` carries the call.
    Resolved,
    /// The advisor escalated the fork to a human — it could not safely judge
    /// it (strategy, irreversibility, un-recorded context).
    Escalated,
}

/// The rich, ultraplan-grade payload the orchestrator writes for a headless
/// advisor to judge a punted design-fork. Everything an advisor with **no
/// session context** needs: the structured fork (`question` + `options` +
/// `stakes` + `lean`) plus a markdown brief (`context_markdown` — the spec,
/// its acceptance, graph context, trace-graph helpers). trace:STORY-306
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PuntRequest {
    /// Display ID of the punted spec.
    pub spec: String,
    /// The obstacle category the implementer picked when it punted.
    pub category: PuntCategory,
    /// The fork as a question — what the advisor must decide.
    pub question: String,
    /// The candidate answers the implementer enumerated, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The code area the fork lives in, if the implementer named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_area: Option<String>,
    /// Why the fork matters — what a wrong call would cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stakes: Option<String>,
    /// The implementer's best guess if forced to choose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<String>,
    /// The assembled ultraplan-grade brief — spec description, acceptance,
    /// parent/child/sibling context, trace-graph helpers — as markdown.
    pub context_markdown: String,
}

/// The headless advisor's answer to a [`PuntRequest`], written back for the
/// orchestrator to act on. trace:STORY-306 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PuntResponse {
    /// Whether the advisor resolved the fork or escalated it.
    pub resolution: PuntResolution,
    /// The chosen answer — present (and load-bearing) when `resolution` is
    /// [`PuntResolution::Resolved`]; absent on an escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// The advisor's reasoning — why it resolved this way, or why it could
    /// not. Always present: every advisor decision is auditable.
    pub reasoning: String,
    /// The A/B/C calibration class the advisor assigned the fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// On an escalation, the categorized reason a human is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
}

/// Path the orchestrator provisions for a spec's punt-request payload —
/// `.aida/punts/<spec>.request.json` under the (main) project root.
pub fn punt_request_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("punts")
        .join(format!("{spec}.request.json"))
}

/// Path the headless advisor writes its response to —
/// `.aida/punts/<spec>.response.json`.
pub fn punt_response_path(project_root: &Path, spec: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("punts")
        .join(format!("{spec}.response.json"))
}

/// Write a punt-request file, creating `.aida/punts/` if needed.
pub fn write_punt_request(path: &Path, request: &PuntRequest) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(request)?)?;
    Ok(())
}

/// Read a punt-response file. `None` when the file is absent or unparseable
/// — either way the orchestrator reads it as "the advisor produced no usable
/// answer" and falls back to the escalate path.
pub fn read_punt_response(path: &Path) -> Option<PuntResponse> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Parse a `--category` value, or return a help-shaped error listing the
/// valid kebab-case categories.
pub fn parse_punt_category(raw: &str) -> Result<PuntCategory, String> {
    PuntCategory::from_str(raw).ok_or_else(|| {
        let valid: Vec<String> = PuntCategory::all().iter().map(|c| c.to_string()).collect();
        format!(
            "invalid punt category `{raw}` — expected one of: {}",
            valid.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_punt_category_accepts_kebab_and_rejects_garbage() {
        assert_eq!(
            parse_punt_category("design-fork"),
            Ok(PuntCategory::DesignFork)
        );
        // Tolerant of casing and `_`/space separators.
        assert_eq!(
            parse_punt_category("Blocked_Dependency"),
            Ok(PuntCategory::BlockedDependency)
        );
        assert_eq!(
            parse_punt_category("missing context"),
            Ok(PuntCategory::MissingContext)
        );
        let err = parse_punt_category("flibbertigibbet").unwrap_err();
        assert!(err.contains("invalid punt category"), "{err}");
        assert!(
            err.contains("design-fork"),
            "error should list valid: {err}"
        );
    }

    // --- Autonomy maturity roll-up (TASK-340) ---------------------------

    /// Build a minimal punt record on `day` (`YYYY-MM-DD`) with the given
    /// resolution path.
    fn rec(day: &str, resolution_path: &str) -> PuntRecord {
        use chrono::TimeZone;
        let parts: Vec<u32> = day.split('-').map(|p| p.parse().unwrap()).collect();
        let ts = Utc
            .with_ymd_and_hms(parts[0] as i32, parts[1], parts[2], 12, 0, 0)
            .unwrap();
        PuntRecord {
            timestamp: ts,
            spec: "TASK-1".to_string(),
            category: PuntCategory::DesignFork,
            detail: "fixture".to_string(),
            lean: None,
            raised_by: Some("implementer".to_string()),
            resolution_path: resolution_path.to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        }
    }

    #[test]
    fn human_interventions_roll_up_per_day_newest_first() {
        let records = vec![
            rec("2026-06-01", "escalated-to-human"),
            rec("2026-06-01", "advisor-resolved"), // not an intervention
            rec("2026-06-01", "escalated-to-human"),
            rec("2026-06-03", "escalated-to-human"),
            rec("2026-06-02", "punted"), // not an intervention
        ];
        let days = human_interventions_by_day(&records);
        assert_eq!(
            days,
            vec![
                AutonomyDay {
                    date: "2026-06-03".to_string(),
                    interventions: 1,
                },
                AutonomyDay {
                    date: "2026-06-01".to_string(),
                    interventions: 2,
                },
            ],
            "newest day first; only escalate-to-human counts; \
             zero-escalation days omitted"
        );
        assert_eq!(total_human_interventions(&records), 3);
    }

    #[test]
    fn decision_escalated_also_counts_as_intervention() {
        // STORY-325 record shape: resolution_path may not be the slug but
        // decision == "escalated" still marks a human intervention.
        let mut r = rec("2026-06-04", "");
        r.decision = Some("escalated".to_string());
        assert!(is_human_intervention(&r));
        let days = human_interventions_by_day(std::slice::from_ref(&r));
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].interventions, 1);
    }

    #[test]
    fn empty_ledger_has_no_interventions() {
        assert!(human_interventions_by_day(&[]).is_empty());
        assert_eq!(total_human_interventions(&[]), 0);
    }

    #[test]
    fn punt_signal_round_trips_and_summarises() {
        let dir = tempfile::tempdir().unwrap();
        let path = signal_path(dir.path(), "STORY-276");
        assert!(
            path.ends_with("punt-signals/STORY-276.json"),
            "{}",
            path.display()
        );
        // Absent file → no punt.
        assert_eq!(read_signal(&path), None);

        let signal = PuntSignal {
            spec: "STORY-276".to_string(),
            category: PuntCategory::DesignFork,
            detail: "two valid auth flows; spec doesn't say which".to_string(),
            lean: Some("OAuth".to_string()),
        };
        write_signal(&path, &signal).unwrap();
        assert_eq!(read_signal(&path), Some(signal.clone()));
        let summary = signal.summary();
        assert!(summary.contains("design-fork"), "{summary}");
        assert!(summary.contains("lean: OAuth"), "{summary}");

        // A garbage file reads as "no punt" rather than erroring.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_signal(&path), None);
    }

    #[test]
    fn hold_signal_round_trips_and_summarises() {
        let dir = tempfile::tempdir().unwrap();
        let path = hold_signal_path(dir.path(), "STORY-306");
        assert!(
            path.ends_with("pr-holds/STORY-306.json"),
            "{}",
            path.display()
        );
        // Absent file → no deliberate hold.
        assert_eq!(read_hold_signal(&path), None);

        let signal = HoldSignal {
            spec: "STORY-306".to_string(),
            branch: "story-306".to_string(),
            reason: Some("SPIKE-7 smoke before merge".to_string()),
        };
        write_hold_signal(&path, &signal).unwrap();
        assert_eq!(read_hold_signal(&path), Some(signal.clone()));
        let summary = signal.summary();
        assert!(summary.contains("story-306"), "{summary}");
        assert!(summary.contains("SPIKE-7 smoke"), "{summary}");

        // A garbage file reads as "no hold" rather than erroring.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_hold_signal(&path), None);
    }

    #[test]
    fn hold_signal_summary_omits_absent_reason() {
        let signal = HoldSignal {
            spec: "STORY-306".to_string(),
            branch: "story-306".to_string(),
            reason: None,
        };
        let summary = signal.summary();
        assert!(summary.contains("PR held on `story-306`"), "{summary}");
        assert!(!summary.contains("—"), "{summary}");
    }

    #[test]
    fn punt_signal_summary_omits_absent_lean() {
        let signal = PuntSignal {
            spec: "STORY-276".to_string(),
            category: PuntCategory::AmbiguousSpec,
            detail: "the acceptance criteria contradict the parent".to_string(),
            lean: None,
        };
        let summary = signal.summary();
        assert!(summary.contains("ambiguous-spec"), "{summary}");
        assert!(!summary.contains("lean:"), "{summary}");
    }

    #[test]
    fn punt_appends_ledger_record() {
        let dir = tempfile::tempdir().unwrap();
        let record = PuntRecord {
            timestamp: Utc::now(),
            spec: "STORY-276".to_string(),
            category: PuntCategory::DesignFork,
            detail: "two valid auth flows; spec doesn't say which".to_string(),
            lean: Some("OAuth".to_string()),
            raised_by: Some("implementer".to_string()),
            resolution_path: "punted".to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        };
        append_to_ledger(dir.path(), &record).unwrap();
        // A second punt appends rather than overwrites.
        append_to_ledger(dir.path(), &record).unwrap();

        let contents = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "each punt is one line");
        let parsed: PuntRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed, record);
        // Category serialises kebab-case so the ledger is human-readable.
        assert!(lines[0].contains("\"design-fork\""), "{}", lines[0]);
    }

    /// EPIC-28: the shelving helper writes one ledger line with the
    /// `shelved-by-failure` discriminator + a `failure:<phase>` slug on
    /// `decision`, so STORY-325 analysis can filter punts out of failures.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn append_failure_to_ledger_writes_shelved_marker() {
        let dir = tempfile::tempdir().unwrap();
        let fr = aida_core::FailureReason {
            phase: "ci".into(),
            phase_index: 2,
            kind: "ci-red".into(),
            detail: "Linux CI red — 3 tests panicked".into(),
            recovery_hint: Some("gh run view 12345".into()),
            shelved_by: Some("implementer".into()),
            shelved_at: Utc::now(),
        };
        append_failure_to_ledger(dir.path(), "STORY-99", &fr).unwrap();

        let contents = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        let line = contents.lines().next().expect("one line written");
        let rec: PuntRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.spec, "STORY-99");
        assert_eq!(rec.resolution_path, RESOLUTION_SHELVED_FAILURE);
        assert_eq!(rec.decision.as_deref(), Some("failure:ci"));
        assert_eq!(rec.detail, "Linux CI red — 3 tests panicked");
        assert_eq!(rec.raised_by.as_deref(), Some("implementer"));
        assert_eq!(rec.paused_at, Some(fr.shelved_at));
        assert!(rec.resolved_at.is_none());
    }

    /// EPIC-28: shelving records sit in the same ledger as punts but use
    /// `category = Other`. STORY-325 punt-frequency views can subtract
    /// shelved records by the `resolution_path` discriminator without
    /// having to special-case the category. trace:EPIC-28 | ai:claude
    #[test]
    fn append_failure_to_ledger_preserves_punt_category_other() {
        let dir = tempfile::tempdir().unwrap();
        let fr = aida_core::FailureReason {
            phase: "build".into(),
            phase_index: 6,
            kind: "build-failed".into(),
            detail: "cargo build exit 101".into(),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: Utc::now(),
        };
        append_failure_to_ledger(dir.path(), "TASK-1", &fr).unwrap();
        let contents = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        let rec: PuntRecord = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
        assert_eq!(rec.category, PuntCategory::Other);
        assert_eq!(rec.decision.as_deref(), Some("failure:build"));
    }

    #[test]
    fn punt_request_roundtrips_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = punt_request_path(dir.path(), "STORY-306");
        assert!(
            path.ends_with("punts/STORY-306.request.json"),
            "{}",
            path.display()
        );
        let request = PuntRequest {
            spec: "STORY-306".to_string(),
            category: PuntCategory::DesignFork,
            question: "flag name --json vs --format json?".to_string(),
            options: vec!["--json bool".to_string(), "--format json".to_string()],
            code_area: Some("aida-cli/src/cli.rs".to_string()),
            stakes: Some("a published flag is hard to rename".to_string()),
            lean: Some("--json bool — AIDA's recorded convention".to_string()),
            context_markdown: "## Requirement\n\nAdd a --json flag.".to_string(),
        };
        write_punt_request(&path, &request).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: PuntRequest = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, request);
    }

    #[test]
    fn punt_response_resolved_and_escalated_parse() {
        // Resolved — answer + reasoning + classification present.
        let resolved = PuntResponse {
            resolution: PuntResolution::Resolved,
            answer: Some("use a bare --json bool".to_string()),
            reasoning: "AIDA's recorded flag convention is a bare --json bool".to_string(),
            classification: Some("A".to_string()),
            escalation_reason: None,
        };
        let json = serde_json::to_string(&resolved).unwrap();
        assert!(json.contains("\"resolution\":\"resolved\""), "{json}");
        assert_eq!(
            serde_json::from_str::<PuntResponse>(&json).unwrap(),
            resolved
        );

        // Escalated — no answer, an escalation_reason instead.
        let escalated = PuntResponse {
            resolution: PuntResolution::Escalated,
            answer: None,
            reasoning: "a project-strategy call with no recorded principle".to_string(),
            classification: Some("C".to_string()),
            escalation_reason: Some("strategy".to_string()),
        };
        let json = serde_json::to_string(&escalated).unwrap();
        assert!(json.contains("\"resolution\":\"escalated\""), "{json}");
        assert_eq!(
            serde_json::from_str::<PuntResponse>(&json).unwrap(),
            escalated
        );
    }

    #[test]
    fn read_punt_response_none_on_absent_or_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = punt_response_path(dir.path(), "STORY-306");
        // Absent file → no usable response.
        assert_eq!(read_punt_response(&path), None);
        // A garbage file reads as "no response" rather than erroring.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_punt_response(&path), None);
    }

    #[test]
    fn punt_record_carries_advisor_fields() {
        // A record with the STORY-306 advisor fields round-trips.
        let record = PuntRecord {
            timestamp: Utc::now(),
            spec: "STORY-306".to_string(),
            category: PuntCategory::DesignFork,
            detail: "flag naming fork".to_string(),
            lean: None,
            raised_by: Some("implementer".to_string()),
            resolution_path: "advisor-resolved".to_string(),
            classification: Some("A".to_string()),
            escalation_reason: None,
            answer: Some("use a bare --json bool".to_string()),
            answered_by: Some("advisor".to_string()),
            decision: None,
            principle_link: None,
            calibration_pair: None,
            paused_at: None,
            resolved_at: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<PuntRecord>(&json).unwrap(), record);

        // An old record with the advisor fields absent still deserializes —
        // they are all `#[serde(default)]`.
        let old = r#"{"timestamp":"2026-05-19T10:00:00Z","spec":"STORY-1","category":"design-fork","detail":"x","resolution_path":"punted"}"#;
        let parsed: PuntRecord = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.classification, None);
        assert_eq!(parsed.answer, None);
    }
}
