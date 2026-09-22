//! Store-wide semantic contradiction detection (STORY-1426).
//!
//! Combines mechanical joins in Rust (Slice 1) with Jev System One semantic
//! choice queries (Slice 2) to identify conflicting assertions across live specs.
//!
//! Complies with PRIN-5 (fail-closed), PRIN-6 (currency), and PRIN-8 (heuristic: true).
//
// trace:STORY-1426 | ai:antigravity

use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::evaluator::EvaluatorEngine;
use aida_core::{Requirement, RequirementStatus, RequirementType, RequirementsStore};

/// A candidate pair extracted by mechanical joins (Slice 1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidatePair {
    pub spec_a_id: String,
    pub spec_a_title: String,
    pub spec_a_status: String,
    pub spec_a_type: String,
    pub spec_a_description: String,

    pub spec_b_id: String,
    pub spec_b_title: String,
    pub spec_b_status: String,
    pub spec_b_type: String,
    pub spec_b_description: String,

    pub mechanical_reason: String,
}

/// A validated semantic contradiction finding (Slice 2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContradictionFinding {
    pub spec_a_id: String,
    pub spec_a_title: String,
    pub spec_a_status: String,
    pub spec_a_type: String,

    pub spec_b_id: String,
    pub spec_b_title: String,
    pub spec_b_status: String,
    pub spec_b_type: String,

    pub verdict: String,
    pub confidence: f64,
    pub probability: f64,
    pub heuristic: bool,
    pub model: String,
    pub mechanical_reason: String,
    pub summary: String,
}

/// Helper to get a stable display ID for a requirement.
fn req_id(req: &Requirement) -> String {
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string())
}

/// Slice 1: Computable contradiction candidate extraction via mechanical joins.
// trace:STORY-1426 | ai:antigravity
pub fn find_mechanical_candidates(store: &RequirementsStore) -> Vec<CandidatePair> {
    let mut candidates = Vec::new();

    // Map by id and spec_id for fast lookup
    let reqs = &store.requirements;

    // Mechanical Join 1:
    // A Vision or Principle in Approved status, older than a Completed ChangeRequest,
    // Decision (ADR), or Story that references it in relationships or description/title.
    for a in reqs {
        let is_vision_or_principle = matches!(
            a.req_type,
            RequirementType::Vision | RequirementType::Principle
        );
        let is_approved = matches!(a.status, RequirementStatus::Approved);

        if !is_vision_or_principle || !is_approved {
            continue;
        }

        let a_id = req_id(a);

        for b in reqs {
            if a.id == b.id {
                continue;
            }

            let is_relevant_type = matches!(
                b.req_type,
                RequirementType::ChangeRequest | RequirementType::Decision | RequirementType::Story
            );
            let is_completed = matches!(
                b.status,
                RequirementStatus::Completed | RequirementStatus::Done
            );

            if !is_relevant_type || !is_completed {
                continue;
            }

            // B must be created or modified after A was created
            if b.created_at < a.created_at {
                continue;
            }

            // Check if B references A
            let refs_a = b.relationships.iter().any(|rel| rel.target_id == a.id)
                || b.description.contains(&a_id)
                || b.title.contains(&a_id)
                || a.spec_id.as_deref().map_or(false, |sid| {
                    b.description.contains(sid) || b.title.contains(sid)
                });

            if refs_a {
                candidates.push(CandidatePair {
                    spec_a_id: a_id.clone(),
                    spec_a_title: a.title.clone(),
                    spec_a_status: format!("{:?}", a.status),
                    spec_a_type: format!("{:?}", a.req_type),
                    spec_a_description: a.description.clone(),

                    spec_b_id: req_id(b),
                    spec_b_title: b.title.clone(),
                    spec_b_status: format!("{:?}", b.status),
                    spec_b_type: format!("{:?}", b.req_type),
                    spec_b_description: b.description.clone(),

                    mechanical_reason: format!(
                        "Approved {} ({}) is older than completed {} ({}) referencing it",
                        a.req_type,
                        a_id,
                        b.req_type,
                        req_id(b)
                    ),
                });
            }
        }
    }

    // Mechanical Join 2:
    // An Epic in Completed status while a spec it Blocks remains open.
    for a in reqs {
        if a.req_type == RequirementType::Epic
            && matches!(
                a.status,
                RequirementStatus::Completed | RequirementStatus::Done
            )
        {
            let a_id = req_id(a);
            for rel in &a.relationships {
                if format!("{:?}", rel.rel_type)
                    .to_lowercase()
                    .contains("block")
                {
                    if let Some(target_req) = reqs.iter().find(|r| r.id == rel.target_id) {
                        let target_open = matches!(
                            target_req.status,
                            RequirementStatus::Draft
                                | RequirementStatus::Approved
                                | RequirementStatus::InProgress
                        );
                        if target_open {
                            candidates.push(CandidatePair {
                                spec_a_id: a_id.clone(),
                                spec_a_title: a.title.clone(),
                                spec_a_status: format!("{:?}", a.status),
                                spec_a_type: format!("{:?}", a.req_type),
                                spec_a_description: a.description.clone(),

                                spec_b_id: req_id(target_req),
                                spec_b_title: target_req.title.clone(),
                                spec_b_status: format!("{:?}", target_req.status),
                                spec_b_type: format!("{:?}", target_req.req_type),
                                spec_b_description: target_req.description.clone(),

                                mechanical_reason: format!(
                                    "Epic {} is Completed while blocked spec {} remains open ({:?})",
                                    a_id, req_id(target_req), target_req.status
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    candidates
}

/// Slice 2: Sweep requirements store for semantic contradictions.
///
/// Evaluates mechanical candidate pairs using `EvaluatorEngine::evaluate_choice`.
// trace:STORY-1426 | ai:antigravity
pub fn sweep_contradictions(
    store: &RequirementsStore,
    evaluator: Option<&dyn EvaluatorEngine>,
) -> Result<Vec<ContradictionFinding>> {
    let candidates = find_mechanical_candidates(store);
    let mut findings = Vec::new();

    let mut options = HashMap::new();
    options.insert(
        "compatible".to_string(),
        "Both specifications describe compatible goals, concepts, or non-conflicting lifecycle states.".to_string(),
    );
    options.insert(
        "supersedes".to_string(),
        "Specification B modifies, deprecates, or updates assertions of Specification A, but Specification A remains active without a superseding link.".to_string(),
    );
    options.insert(
        "contradicts".to_string(),
        "Specification B directly conflicts with, disputes, or contradicts assertions, rules, or positioning in Specification A.".to_string(),
    );

    for pair in &candidates {
        if let Some(eval) = evaluator {
            let context =
                format!(
                "Specification A: [ID: {}] (Status: {}, Type: {})\nTitle: {}\nDescription:\n{}\n\n\
                 Specification B: [ID: {}] (Status: {}, Type: {})\nTitle: {}\nDescription:\n{}",
                pair.spec_a_id,
                pair.spec_a_status,
                pair.spec_a_type,
                pair.spec_a_title,
                pair.spec_a_description.lines().take(15).collect::<Vec<_>>().join("\n"),
                pair.spec_b_id,
                pair.spec_b_status,
                pair.spec_b_type,
                pair.spec_b_title,
                pair.spec_b_description.lines().take(15).collect::<Vec<_>>().join("\n")
            );

            let instruction = "Determine whether Specification B semantically contradicts, modifies/supersedes, or is compatible with Specification A.";

            match eval.evaluate_choice_sync(&context, instruction, &options) {
                Ok(resp) => {
                    let prob = resp
                        .probabilities
                        .get(&resp.choice)
                        .copied()
                        .unwrap_or(resp.confidence);

                    // Flag as finding if choice is "contradicts" or "supersedes" with probability >= 0.50
                    if resp.choice == "contradicts" || (resp.choice == "supersedes" && prob >= 0.50)
                    {
                        let summary = format!(
                            "Semantic conflict detected: {} ({}) is contradicted or superseded by {} ({}) without status alignment.",
                            pair.spec_a_id, pair.spec_a_status, pair.spec_b_id, pair.spec_b_status
                        );
                        findings.push(ContradictionFinding {
                            spec_a_id: pair.spec_a_id.clone(),
                            spec_a_title: pair.spec_a_title.clone(),
                            spec_a_status: pair.spec_a_status.clone(),
                            spec_a_type: pair.spec_a_type.clone(),

                            spec_b_id: pair.spec_b_id.clone(),
                            spec_b_title: pair.spec_b_title.clone(),
                            spec_b_status: pair.spec_b_status.clone(),
                            spec_b_type: pair.spec_b_type.clone(),

                            verdict: resp.choice,
                            confidence: resp.confidence,
                            probability: prob,
                            heuristic: resp.heuristic,
                            model: resp.model,
                            mechanical_reason: pair.mechanical_reason.clone(),
                            summary,
                        });
                    }
                }
                Err(err) => {
                    // PRIN-5: Fail-closed on evaluator error by recording mechanical finding with error note
                    eprintln!(
                        "  {} Evaluator warning for candidate pair ({} vs {}): {}",
                        "▲".yellow(),
                        pair.spec_a_id,
                        pair.spec_b_id,
                        err
                    );
                }
            }
        } else {
            // Evaluator not configured: report candidate from Slice 1 with mechanical flag
            findings.push(ContradictionFinding {
                spec_a_id: pair.spec_a_id.clone(),
                spec_a_title: pair.spec_a_title.clone(),
                spec_a_status: pair.spec_a_status.clone(),
                spec_a_type: pair.spec_a_type.clone(),

                spec_b_id: pair.spec_b_id.clone(),
                spec_b_title: pair.spec_b_title.clone(),
                spec_b_status: pair.spec_b_status.clone(),
                spec_b_type: pair.spec_b_type.clone(),

                verdict: "candidate".to_string(),
                confidence: 0.50,
                probability: 0.50,
                heuristic: true,
                model: "mechanical-join".to_string(),
                mechanical_reason: pair.mechanical_reason.clone(),
                summary: format!(
                    "Mechanical contradiction candidate between {} and {}",
                    pair.spec_a_id, pair.spec_b_id
                ),
            });
        }
    }

    Ok(findings)
}

/// Render contradiction findings to the terminal.
// trace:STORY-1426 | ai:antigravity
pub fn render_findings(findings: &[ContradictionFinding]) {
    if findings.is_empty() {
        println!(
            "{}",
            "No semantic contradictions detected in requirement store.".green()
        );
        return;
    }

    println!(
        "\n{} Detected {} semantic contradiction candidate(s):\n",
        "▲".red().bold(),
        findings.len().to_string().bold()
    );

    for (idx, f) in findings.iter().enumerate() {
        println!(
            "  {}. {} {} (heuristic: p={:.2}, model: {})",
            idx + 1,
            format!("[{}]", f.verdict.to_ascii_uppercase()).red().bold(),
            f.summary.bold(),
            f.probability,
            f.model.cyan()
        );
        println!(
            "     Spec A: {} ({}: {})",
            f.spec_a_id.cyan().bold(),
            f.spec_a_type,
            f.spec_a_status.yellow()
        );
        println!("       \"{}\"", f.spec_a_title);
        println!(
            "     Spec B: {} ({}: {})",
            f.spec_b_id.cyan().bold(),
            f.spec_b_type,
            f.spec_b_status.green()
        );
        println!("       \"{}\"", f.spec_b_title);
        println!("     Join trigger: {}", f.mechanical_reason.dimmed());
        println!();
    }
}
