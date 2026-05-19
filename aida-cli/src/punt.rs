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
}

/// Path to the punt ledger for a project, given its root directory.
pub fn ledger_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("punts.jsonl")
}

/// Append one punt record to `.aida/punts.jsonl`, creating the file (and the
/// `.aida/` directory) if needed. One JSON object per line.
pub fn append_to_ledger(project_root: &Path, record: &PuntRecord) -> anyhow::Result<()> {
    let path = ledger_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(record)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    Ok(())
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
