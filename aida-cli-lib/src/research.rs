//! STORY-568: the research/spike lane — dispatch a SPIKE spec to a headless
//! research agent (reusing the `deep-research` skill), capture a source-grounded
//! analysis as the deliverable, and ESCALATE the decision (never auto-apply it)
//! via the questions inbox. Propose-mode by design: the agent does the legwork,
//! the human/advisor makes the call.
//!
//! A spike's deliverable is an analysis + a recommendation, NOT a mergeable PR,
//! so the implementer drain (implement -> CI -> review -> merge) has no lane for
//! it and historically mislabelled it "human-only". This lane is the missing
//! half: the agent-able research path. The strategic *decision* the research
//! informs is the only thing escalated to the human.
//!
//! This module is the side-effect-free core — prompt composition, artifact-path
//! derivation, and parsing the agent's structured decision sidecar into a
//! `DecisionRequest`. The headless spawn and the backend writes live in the
//! `main.rs` handler; keeping the transforms pure makes them exhaustively
//! unit-testable without spawning a model.
// trace:STORY-568 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// The structured deliverable the research agent writes to its decision
/// sidecar JSON alongside the prose analysis. Mirrors a `DecisionRequest`
/// (question + enumerated choices + recommendation) but is the agent's raw,
/// pre-validation output — `sidecar_to_decision_request` reduces it to the
/// persisted form. `options` reuses `DecisionChoice` so each choice already
/// carries the deterministic `resolution` token the questions loop expects.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ResearchSidecar {
    /// One-paragraph synthesis of what the research found.
    pub summary: String,
    /// The strategic decision to escalate, or `None` when the research is
    /// purely informational (no fork for the human to resolve).
    #[serde(default)]
    pub decision_question: Option<String>,
    /// Enumerated, actionable options (each with a deterministic resolution
    /// token). Only escalated when there is a `decision_question` and >= 2.
    #[serde(default)]
    pub options: Vec<aida_core::DecisionChoice>,
    /// 1-based index of the agent's recommended option (as written in the
    /// sidecar). Converted to 0-based when building the `DecisionRequest`.
    #[serde(default)]
    pub recommended: Option<usize>,
    /// Why the recommended option is recommended.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// Derive the `(analysis, sidecar)` output paths for a dispatch. `date` is the
/// caller-supplied `YYYY-MM-DD` stamp (kept a parameter so the function stays
/// pure and testable — the handler passes `chrono::Utc::now()`).
pub(crate) fn research_artifact_paths(dir: &Path, spec_id: &str, date: &str) -> (PathBuf, PathBuf) {
    let slug = spec_id.trim().to_ascii_lowercase();
    let analysis = dir.join(format!("{date}-{slug}-research.md"));
    let sidecar = dir.join(format!("{date}-{slug}-decision.json"));
    (analysis, sidecar)
}

/// Compose the headless research prompt. The agent is told to USE the existing
/// `deep-research` skill (do NOT reimplement research orchestration), persist
/// the prose analysis to `analysis_path`, and persist the structured decision
/// to `sidecar_path` — then STOP without applying any direction change.
///
/// `acceptance` is the spike's body (description + acceptance criteria),
/// already assembled by the caller. Pure: returns the prompt string.
pub(crate) fn build_research_prompt(
    display_id: &str,
    title: &str,
    body: &str,
    analysis_path: &Path,
    sidecar_path: &Path,
) -> String {
    let analysis = analysis_path.display();
    let sidecar = sidecar_path.display();
    format!(
        "You are AIDA's research-lane agent, dispatched to investigate a SPIKE \
and bring back an analysis plus a recommendation. You do the legwork; a human \
makes the final call. Do NOT change project direction, edit specs, merge \
anything, or apply a decision — your job ends at producing the two artifacts \
below.\n\
\n\
## Spike under investigation: {display_id} — {title}\n\
\n\
{body}\n\
\n\
## How to research\n\
\n\
Use the `deep-research` skill to investigate this question deeply: fan out \
across sources (web + this repo's substrate where relevant), fetch and read \
primary sources, adversarially verify load-bearing claims, and tag provenance \
per claim (verified-by-you vs delegated/secondary). Do NOT reimplement research \
orchestration — compose the existing skill. Where the question is competitive / \
strategic, prefer source-grounded evidence over recall.\n\
\n\
## Required outputs (write BOTH files, then stop)\n\
\n\
1. The full analysis as markdown at:\n\
     {analysis}\n\
   Include: the question, what you found, the evidence with citations/links, \
the options on the table with trade-offs, and your recommendation with \
reasoning. This is the deliverable a human will read.\n\
\n\
2. A machine-readable decision sidecar (strict JSON, no prose around it) at:\n\
     {sidecar}\n\
   Shape:\n\
     {{\n\
       \"summary\": \"one-paragraph synthesis of the finding\",\n\
       \"decision_question\": \"the single strategic decision to escalate, or null if the research is purely informational\",\n\
       \"options\": [\n\
         {{\"label\": \"short pick\", \"consequence\": \"what choosing this means\", \"resolution\": \"a deterministic action token, e.g. status:approved or tag:+deferred:post-stability or noop\"}}\n\
       ],\n\
       \"recommended\": 1,\n\
       \"rationale\": \"why the recommended option\"\n\
     }}\n\
   Rules: if there IS a decision for the human, give >= 2 options and a 1-based \
`recommended` index; each option's `resolution` MUST be a parseable token \
(never free-form prose). If the spike is purely informational with no fork, set \
`decision_question` to null and `options` to []. Write valid JSON only — it is \
parsed programmatically.\n\
\n\
Do not call AskUserQuestion (there is no human in this session). When both \
files are written, you are done."
    )
}

/// Parse the agent's decision sidecar JSON into a [`ResearchSidecar`].
pub(crate) fn parse_research_sidecar(json: &str) -> Result<ResearchSidecar> {
    serde_json::from_str(json).context("parsing research decision sidecar JSON")
}

/// Reduce a sidecar to a persistable `DecisionRequest` — `Some` only when the
/// research surfaced a real fork (a non-empty `decision_question` AND >= 2
/// options). Purely informational spikes return `None` (deliverable produced,
/// nothing to escalate). The 1-based `recommended` is converted to 0-based and
/// dropped if out of range (a bad index must not sink the whole escalation).
pub(crate) fn sidecar_to_decision_request(
    s: &ResearchSidecar,
    asked_at: chrono::DateTime<chrono::Utc>,
) -> Option<aida_core::DecisionRequest> {
    let question = s.decision_question.as_ref()?.trim().to_string();
    if question.is_empty() || s.options.len() < 2 {
        return None;
    }
    let recommended = s.recommended.and_then(|n| {
        // 1-based in the sidecar; 0-based in the model. Out-of-range -> None.
        if n >= 1 && n <= s.options.len() {
            Some(n - 1)
        } else {
            None
        }
    });
    Some(aida_core::DecisionRequest {
        question,
        choices: s.options.clone(),
        recommended,
        rationale: s.rationale.clone(),
        answered: None,
        note: None,
        asked_at: Some(asked_at),
        answered_at: None,
    })
}

/// The provenance-tagged comment body attached to the spike pointing at the
/// analysis artifact. Keeps the deliverable discoverable from `aida show`
/// while the full report lives in the dated file.
pub(crate) fn provenance_comment(analysis_rel: &str, summary: &str) -> String {
    format!(
        "[research-lane] source-grounded analysis (agent: claude / deep-research).\n\n\
         {summary}\n\n\
         Full analysis + citations: `{analysis_rel}`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn artifact_paths_lowercase_slug_and_dated() {
        let (analysis, sidecar) =
            research_artifact_paths(Path::new("docs/research"), "SPIKE-53", "2026-06-12");
        assert_eq!(
            analysis,
            PathBuf::from("docs/research/2026-06-12-spike-53-research.md")
        );
        assert_eq!(
            sidecar,
            PathBuf::from("docs/research/2026-06-12-spike-53-decision.json")
        );
    }

    #[test]
    fn prompt_names_skill_and_both_output_paths() {
        let prompt = build_research_prompt(
            "SPIKE-53",
            "Evaluate X",
            "## Acceptance\n- [ ] decide",
            Path::new("docs/research/r.md"),
            Path::new("docs/research/d.json"),
        );
        // Reuses the skill (not a reimplementation) and pins both artifacts.
        assert!(prompt.contains("deep-research"));
        assert!(prompt.contains("docs/research/r.md"));
        assert!(prompt.contains("docs/research/d.json"));
        // Carries the spike context + the propose-only guardrail.
        assert!(prompt.contains("SPIKE-53 — Evaluate X"));
        assert!(prompt.contains("- [ ] decide"));
        assert!(prompt.contains("apply a decision"));
    }

    #[test]
    fn sidecar_with_fork_becomes_decision_request() {
        let s = parse_research_sidecar(
            r#"{
                "summary": "found three viable paths",
                "decision_question": "Which storage backend do we commit to?",
                "options": [
                    {"label": "Postgres", "consequence": "ops cost up", "resolution": "tag:+backend:postgres"},
                    {"label": "Stay git", "consequence": "status quo", "resolution": "noop"}
                ],
                "recommended": 2,
                "rationale": "git-canonical already proven"
            }"#,
        )
        .unwrap();
        let dr = sidecar_to_decision_request(&s, now()).expect("a fork escalates");
        assert_eq!(dr.question, "Which storage backend do we commit to?");
        assert_eq!(dr.choices.len(), 2);
        // 1-based 2 -> 0-based 1.
        assert_eq!(dr.recommended, Some(1));
        assert_eq!(
            dr.rationale.as_deref(),
            Some("git-canonical already proven")
        );
        assert!(dr.is_pending());
    }

    #[test]
    fn informational_spike_does_not_escalate() {
        // No decision_question -> nothing to escalate (deliverable only).
        let s = parse_research_sidecar(
            r#"{"summary": "just an FYI", "decision_question": null, "options": []}"#,
        )
        .unwrap();
        assert!(sidecar_to_decision_request(&s, now()).is_none());

        // A question but < 2 options is not a real fork either.
        let s2 = parse_research_sidecar(
            r#"{"summary":"x","decision_question":"pick?","options":[{"label":"a","consequence":"b","resolution":"noop"}]}"#,
        )
        .unwrap();
        assert!(sidecar_to_decision_request(&s2, now()).is_none());
    }

    #[test]
    fn out_of_range_recommended_is_dropped_not_fatal() {
        let s = parse_research_sidecar(
            r#"{
                "summary":"s",
                "decision_question":"q",
                "options":[
                    {"label":"a","consequence":"c","resolution":"noop"},
                    {"label":"b","consequence":"c","resolution":"noop"}
                ],
                "recommended": 9
            }"#,
        )
        .unwrap();
        let dr = sidecar_to_decision_request(&s, now()).unwrap();
        assert_eq!(dr.recommended, None);
    }

    #[test]
    fn provenance_comment_tags_lane_and_points_at_artifact() {
        let c = provenance_comment("docs/research/2026-06-12-spike-53-research.md", "the gist");
        assert!(c.contains("[research-lane]"));
        assert!(c.contains("the gist"));
        assert!(c.contains("docs/research/2026-06-12-spike-53-research.md"));
    }
}
