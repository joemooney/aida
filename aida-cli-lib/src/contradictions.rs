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
use std::path::Path;

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

/// Stable category used for grouping, ordering, and bounded continuation.
// trace:STORY-1426 | ai:codex
pub fn finding_category(finding: &ContradictionFinding) -> &'static str {
    let reason = finding.mechanical_reason.as_str();
    if reason.starts_with("Accepted ADR ") {
        "adr-overlap"
    } else if reason.starts_with("Terminal ") && reason.contains(" plan promises Followups") {
        "plan-followup"
    } else if reason.starts_with("Terminal spec ") {
        "post-terminal-acceptance-edit"
    } else if reason.starts_with("Epic ") {
        "completed-epic-blocks-open"
    } else {
        "approved-reference"
    }
}

/// Deterministic ordering makes offset pagination stable for a fixed store.
// trace:STORY-1426 | ai:codex
pub fn sort_findings(findings: &mut [ContradictionFinding]) {
    findings.sort_by(|a, b| {
        finding_category(a)
            .cmp(finding_category(b))
            .then_with(|| a.spec_a_id.cmp(&b.spec_a_id))
            .then_with(|| a.spec_b_id.cmp(&b.spec_b_id))
            .then_with(|| a.mechanical_reason.cmp(&b.mechanical_reason))
    });
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ContradictionPage {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub returned: usize,
    pub remaining: usize,
    pub next_offset: Option<usize>,
    pub category_counts: std::collections::BTreeMap<String, usize>,
    pub findings: Vec<ContradictionFinding>,
}

/// Sort and page without dropping information: `next_offset` walks every
/// finding, while category totals let an operator size and prioritize the
/// complete scan before opening later pages.
// trace:STORY-1426 | ai:codex
pub fn paginate_findings(
    mut findings: Vec<ContradictionFinding>,
    limit: usize,
    offset: usize,
    all: bool,
) -> ContradictionPage {
    sort_findings(&mut findings);
    let total = findings.len();
    let mut category_counts = std::collections::BTreeMap::<String, usize>::new();
    for finding in &findings {
        *category_counts
            .entry(finding_category(finding).to_string())
            .or_default() += 1;
    }
    let effective_offset = if all { 0 } else { offset.min(total) };
    let effective_limit = if all { total } else { limit.max(1) };
    let page = findings
        .into_iter()
        .skip(effective_offset)
        .take(effective_limit)
        .collect::<Vec<_>>();
    let returned = page.len();
    let next = effective_offset + returned;
    let remaining = total.saturating_sub(next);
    ContradictionPage {
        total,
        offset: effective_offset,
        limit: effective_limit,
        returned,
        remaining,
        next_offset: (remaining > 0).then_some(next),
        category_counts,
        findings: page,
    }
}

/// Helper to get a stable display ID for a requirement.
fn req_id(req: &Requirement) -> String {
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| req.id.to_string())
}

fn is_terminal_status(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Completed
            | RequirementStatus::Done
            | RequirementStatus::Rejected
            | RequirementStatus::Superseded
    )
}

/// A terminal plan owner can temporarily present an earlier cached/canonical
/// status while every explicitly linked child already records the completed
/// roll-up. Treat that fully terminal child set as equivalent evidence for the
/// reporting-only Followups join; it cannot mutate or close the parent.
fn plan_owner_is_terminal(req: &Requirement, reqs: &[Requirement]) -> bool {
    if is_terminal_status(&req.status) {
        return true;
    }
    !req.relationships.is_empty()
        && req
            .relationships
            .iter()
            .filter_map(|rel| reqs.iter().find(|candidate| candidate.id == rel.target_id))
            .all(|child| is_terminal_status(&child.status))
        && req
            .relationships
            .iter()
            .all(|rel| reqs.iter().any(|candidate| candidate.id == rel.target_id))
}

/// Slice 1: Computable contradiction candidate extraction via mechanical joins.
// trace:STORY-1426 | ai:antigravity
pub fn find_mechanical_candidates(store: &RequirementsStore) -> Vec<CandidatePair> {
    find_mechanical_candidates_at(store, Path::new("."))
}

pub fn find_mechanical_candidates_at(
    store: &RequirementsStore,
    project_root: &Path,
) -> Vec<CandidatePair> {
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

    // Mechanical Join 3: accepted ADRs with no supersession and a later
    // accepted ADR sharing a tag or parent relationship.
    for a in reqs.iter().filter(|r| {
        r.req_type == RequirementType::Decision
            && matches!(
                r.status,
                RequirementStatus::Approved | RequirementStatus::Completed
            )
    }) {
        let a_superseded = a.relationships.iter().any(|rel| {
            format!("{:?}", rel.rel_type)
                .to_ascii_lowercase()
                .contains("supersed")
        });
        if a_superseded {
            continue;
        }
        for b in reqs.iter().filter(|r| {
            r.req_type == RequirementType::Decision
                && matches!(
                    r.status,
                    RequirementStatus::Approved | RequirementStatus::Completed
                )
                && r.created_at > a.created_at
        }) {
            let shared_tag = a.tags.iter().any(|tag| b.tags.contains(tag));
            let shared_parent = a
                .relationships
                .iter()
                .filter(|rel| format!("{:?}", rel.rel_type).eq_ignore_ascii_case("parent"))
                .any(|left| {
                    b.relationships.iter().any(|right| {
                        format!("{:?}", right.rel_type).eq_ignore_ascii_case("parent")
                            && left.target_id == right.target_id
                    })
                });
            if shared_tag || shared_parent {
                candidates.push(pair(
                    a,
                    b,
                    format!(
                        "Accepted ADR {} overlaps later accepted ADR {} by {}",
                        req_id(a),
                        req_id(b),
                        if shared_parent { "parent" } else { "tag" }
                    ),
                ));
            }
        }
    }

    // Mechanical Join 4: acceptance text changed after terminal transition.
    for req in reqs.iter().filter(|r| {
        matches!(
            r.status,
            RequirementStatus::Completed
                | RequirementStatus::Done
                | RequirementStatus::Rejected
                | RequirementStatus::Superseded
        )
    }) {
        let terminal_at = req
            .history
            .iter()
            .filter(|h| {
                h.changes.iter().any(|c| {
                    c.field_name == "status"
                        && matches!(
                            c.new_value.to_ascii_lowercase().as_str(),
                            "completed" | "done" | "rejected" | "superseded"
                        )
                })
            })
            .map(|h| h.timestamp)
            .min();
        let acceptance_edit = terminal_at.and_then(|terminal| {
            req.history.iter().find(|h| {
                h.timestamp > terminal
                    && h.changes.iter().any(|c| {
                        c.field_name == "description"
                            && (c.old_value.to_ascii_lowercase().contains("acceptance")
                                || c.new_value.to_ascii_lowercase().contains("acceptance"))
                    })
            })
        });
        if let Some(edit) = acceptance_edit {
            let mut candidate = pair(
                req,
                req,
                format!(
                    "Terminal spec {} had acceptance text edited at {} after terminal transition",
                    req_id(req),
                    edit.timestamp
                ),
            );
            candidate.spec_b_id = format!("{}@acceptance-edit", req_id(req));
            candidate.spec_b_title = "post-terminal acceptance edit".into();
            candidates.push(candidate);
        }
    }

    // Mechanical Join 5: terminal plan Followups with no matching child.
    for req in reqs.iter().filter(|r| plan_owner_is_terminal(r, reqs)) {
        let Some(plan_rel) = req
            .description
            .split_whitespace()
            .find(|word| {
                word.trim_matches(|c: char| {
                    c == '.' || c == ',' || c == ')' || c == '(' || c == '`'
                })
                .starts_with("docs/plans/")
            })
            .map(|s| {
                s.trim_matches(|c: char| c == '.' || c == ',' || c == ')' || c == '(' || c == '`')
            })
        else {
            continue;
        };
        let Ok(plan) = std::fs::read_to_string(project_root.join(plan_rel)) else {
            continue;
        };
        let Some(followups) = plan.split("## Followups").nth(1) else {
            continue;
        };
        for bullet in followups
            .lines()
            .take_while(|line| !line.starts_with("## "))
            .filter_map(|line| line.trim().strip_prefix("- "))
        {
            let terms: Vec<String> = bullet
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| s.len() >= 5)
                .map(|s| s.to_ascii_lowercase())
                .collect();
            let has_child = req
                .relationships
                .iter()
                .filter_map(|rel| reqs.iter().find(|child| child.id == rel.target_id))
                .any(|child| {
                    let haystack =
                        format!("{} {}", child.title, child.description).to_ascii_lowercase();
                    terms.iter().take(3).all(|term| haystack.contains(term))
                });
            if !has_child {
                candidates.push(CandidatePair {
                    spec_a_id: req_id(req), spec_a_title: req.title.clone(), spec_a_status: format!("{:?}", req.status), spec_a_type: format!("{:?}", req.req_type), spec_a_description: req.description.clone(),
                    spec_b_id: format!("{}#Followups", plan_rel), spec_b_title: bullet.to_string(), spec_b_status: "MissingChild".into(), spec_b_type: "PlanFollowup".into(), spec_b_description: bullet.to_string(),
                    mechanical_reason: format!("Terminal {} plan promises Followups bullet with no corresponding child spec", req_id(req)),
                });
            }
        }
    }

    candidates
}

fn pair(a: &Requirement, b: &Requirement, mechanical_reason: String) -> CandidatePair {
    CandidatePair {
        spec_a_id: req_id(a),
        spec_a_title: a.title.clone(),
        spec_a_status: format!("{:?}", a.status),
        spec_a_type: format!("{:?}", a.req_type),
        spec_a_description: a.description.clone(),
        spec_b_id: req_id(b),
        spec_b_title: b.title.clone(),
        spec_b_status: format!("{:?}", b.status),
        spec_b_type: format!("{:?}", b.req_type),
        spec_b_description: b.description.clone(),
        mechanical_reason,
    }
}

/// Slice 2: Sweep requirements store for semantic contradictions.
///
/// Evaluates mechanical candidate pairs using `EvaluatorEngine::evaluate_choice`.
// trace:STORY-1426 | ai:antigravity
pub fn sweep_contradictions(
    store: &RequirementsStore,
    evaluator: Option<&dyn EvaluatorEngine>,
) -> Result<Vec<ContradictionFinding>> {
    sweep_contradictions_at(store, evaluator, Path::new("."))
}

pub fn sweep_contradictions_at(
    store: &RequirementsStore,
    evaluator: Option<&dyn EvaluatorEngine>,
    project_root: &Path,
) -> Result<Vec<ContradictionFinding>> {
    let candidates = find_mechanical_candidates_at(store, project_root);
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

                    // Slice 1 is an independently actionable mechanical sweep.
                    // A Rung-3.5 heuristic may enrich a candidate, but a
                    // `compatible` answer is not evidence sufficient to erase
                    // the deterministic join that discovered it.
                    let is_semantic_conflict = (resp.choice == "contradicts"
                        || resp.choice == "supersedes")
                        && prob >= 0.50;
                    let summary = if is_semantic_conflict {
                        format!(
                            "Semantic conflict detected: {} ({}) is contradicted or superseded by {} ({}) without status alignment.",
                            pair.spec_a_id, pair.spec_a_status, pair.spec_b_id, pair.spec_b_status
                        )
                    } else {
                        format!(
                            "Mechanical contradiction candidate remains unresolved: {} vs {}; heuristic classified it as {}.",
                            pair.spec_a_id, pair.spec_b_id, resp.choice
                        )
                    };
                    findings.push(ContradictionFinding {
                        spec_a_id: pair.spec_a_id.clone(),
                        spec_a_title: pair.spec_a_title.clone(),
                        spec_a_status: pair.spec_a_status.clone(),
                        spec_a_type: pair.spec_a_type.clone(),
                        spec_b_id: pair.spec_b_id.clone(),
                        spec_b_title: pair.spec_b_title.clone(),
                        spec_b_status: pair.spec_b_status.clone(),
                        spec_b_type: pair.spec_b_type.clone(),
                        verdict: if is_semantic_conflict {
                            resp.choice
                        } else {
                            "candidate".into()
                        },
                        confidence: resp.confidence,
                        probability: prob,
                        heuristic: resp.heuristic,
                        model: resp.model,
                        mechanical_reason: pair.mechanical_reason.clone(),
                        summary,
                    });
                }
                Err(err) => {
                    // PRIN-5: an unavailable heuristic cannot erase a mechanical candidate.
                    findings.push(ContradictionFinding {
                        spec_a_id: pair.spec_a_id.clone(),
                        spec_a_title: pair.spec_a_title.clone(),
                        spec_a_status: pair.spec_a_status.clone(),
                        spec_a_type: pair.spec_a_type.clone(),
                        spec_b_id: pair.spec_b_id.clone(),
                        spec_b_title: pair.spec_b_title.clone(),
                        spec_b_status: pair.spec_b_status.clone(),
                        spec_b_type: pair.spec_b_type.clone(),
                        verdict: "evaluation-unavailable".into(),
                        confidence: 0.0,
                        probability: 0.0,
                        heuristic: true,
                        model: "unavailable".into(),
                        mechanical_reason: pair.mechanical_reason.clone(),
                        summary: format!(
                            "Evaluator failed closed for {} vs {}: {}",
                            pair.spec_a_id, pair.spec_b_id, err
                        ),
                    });
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
pub fn render_findings_page(
    findings: &[ContradictionFinding],
    total: usize,
    offset: usize,
    limit: usize,
    category_counts: &std::collections::BTreeMap<String, usize>,
) {
    if total == 0 {
        println!(
            "{}",
            "No semantic contradictions detected in requirement store.".green()
        );
        return;
    }

    println!(
        "\n{} Detected {} semantic contradiction candidate(s):\n",
        "▲".red().bold(),
        total.to_string().bold()
    );

    println!(
        "  categories: {}",
        category_counts
            .iter()
            .map(|(category, count)| format!("{category}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  showing {}..{} of {}\n",
        offset.min(total),
        (offset + findings.len()).min(total),
        total
    );

    for (idx, f) in findings.iter().enumerate() {
        println!(
            "  {}. {} {} (heuristic: p={:.2}, model: {})",
            offset + idx + 1,
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

    let next = offset.saturating_add(findings.len());
    if next < total {
        println!(
            "  {} {} finding(s) remain; continue with `aida doctor --contradictions --contradictions-limit {} --contradictions-offset {}` or use `--all`.",
            "…".yellow(),
            total - next,
            limit,
            next
        );
    }
}
