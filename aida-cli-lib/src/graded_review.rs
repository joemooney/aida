//! Graded Review Engine (STORY-1424).
//!
//! Bridges deterministic executable acceptance checks (Rung 2) with
//! calibrated System One evaluation (Rung 3.5) and Phase 3 conversational review.
//!
//! Complies with:
//! - PRIN-5: Fail closed on execution or evaluation errors
//! - PRIN-6: Currency & provenance (binds reviewed_sha and exit codes)
//! - PRIN-7: Dual predicates (CI + explicit review verdict)
//! - PRIN-8: Calibrated heuristic labeling (`heuristic: true`)
//
// trace:STORY-1424 | ai:antigravity

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

use crate::evaluator::EvaluatorEngine;
use crate::review_verdict::VerdictKind;

/// Categorized acceptance criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CriterionKind {
    /// Deterministic executable command check (Rung 2).
    Executable {
        command: String,
        description: String,
    },
    /// Residual prose criterion requiring semantic evaluation (Rung 3.5 or Phase 3 seat).
    Prose { text: String },
}

/// Execution status of a criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CriterionStatus {
    Passed,
    Failed,
    Escalated,
}

/// Result of evaluating an individual criterion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CriterionResult {
    pub criterion: CriterionKind,
    pub status: CriterionStatus,
    pub output: Option<String>,
    pub exit_code: Option<i32>,
    pub probability: Option<f64>,
    pub confidence: Option<f64>,
    pub heuristic: bool,
    pub model: Option<String>,
    pub question_payload_hash: Option<String>,
}

/// Aggregate graded review verdict for a specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GradedReviewVerdict {
    pub spec_id: String,
    pub reviewed_sha: String,
    pub overall_verdict: String,
    pub verdict_kind: String,
    pub machine_verified_count: usize,
    pub machine_passed_count: usize,
    pub prose_count: usize,
    pub residual_prose_count: usize,
    pub escalated_to_seat: bool,
    pub results: Vec<CriterionResult>,
    pub summary: String,
}

/// Parse acceptance criteria into executable commands vs residual prose.
// trace:STORY-1424 | ai:antigravity
pub fn parse_acceptance_criteria(description: &str) -> Vec<CriterionKind> {
    let mut criteria = Vec::new();

    let lines: Vec<&str> = description.lines().collect();
    let mut in_acceptance = false;
    let mut in_verify = false;
    let mut in_fenced_block = false;

    for line in lines {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") {
            let heading = trimmed
                .strip_prefix("## ")
                .unwrap()
                .trim()
                .to_ascii_lowercase();
            if heading.starts_with("acceptance") {
                in_acceptance = true;
                in_verify = false;
                continue;
            } else if heading.starts_with("verify") || heading.starts_with("verification") {
                in_verify = true;
                in_acceptance = false;
                continue;
            } else {
                in_acceptance = false;
                in_verify = false;
                continue;
            }
        }

        if in_verify {
            if trimmed.starts_with("```") {
                in_fenced_block = !in_fenced_block;
                continue;
            }

            if in_fenced_block && !trimmed.is_empty() && !trimmed.starts_with('#') {
                criteria.push(CriterionKind::Executable {
                    command: trimmed.to_string(),
                    description: format!("Verification check: `{}`", trimmed),
                });
                continue;
            }
        }

        if in_acceptance || in_verify {
            let bullet = if let Some(rest) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                rest.trim()
            } else if let Some((_, rest)) = trimmed.split_once(". ") {
                rest.trim()
            } else {
                continue;
            };

            // Clean checkboxes [ ] or [x]
            let clean_bullet = if let Some(rest) = bullet
                .strip_prefix("[ ]")
                .or_else(|| bullet.strip_prefix("[x]"))
                .or_else(|| bullet.strip_prefix("[X]"))
            {
                rest.trim()
            } else {
                bullet
            };

            if clean_bullet.is_empty() {
                continue;
            }

            // Detect if bullet specifies an executable check
            if let Some(cmd) = extract_executable_command(clean_bullet) {
                criteria.push(CriterionKind::Executable {
                    command: cmd,
                    description: clean_bullet.to_string(),
                });
            } else {
                criteria.push(CriterionKind::Prose {
                    text: clean_bullet.to_string(),
                });
            }
        }
    }

    // Fallback: If no section heading was found, check whole body for explicit backtick commands or bullets
    if criteria.is_empty() {
        for line in description.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                let clean = rest
                    .trim_start_matches("[ ]")
                    .trim_start_matches("[x]")
                    .trim();
                if let Some(cmd) = extract_executable_command(clean) {
                    criteria.push(CriterionKind::Executable {
                        command: cmd,
                        description: clean.to_string(),
                    });
                } else if !clean.is_empty() {
                    criteria.push(CriterionKind::Prose {
                        text: clean.to_string(),
                    });
                }
            }
        }
    }

    criteria
}

/// Helper to detect if a criterion line contains an executable shell command.
fn extract_executable_command(text: &str) -> Option<String> {
    let text = text.trim();
    // 1. Check if line starts with a backtick command, e.g. `cargo test -p aida-cli-lib`: ...
    if text.starts_with('`') {
        if let Some(end) = text[1..].find('`') {
            let cmd = &text[1..=end];
            if is_shell_command(cmd)
                || text.len() == end + 2
                || text[end + 1..].trim_start().starts_with(':')
            {
                return Some(cmd.trim().to_string());
            }
        }
    }

    // 2. Check if line has explicit command prefix
    for prefix in &[
        "$ ", "run: ", "Run: ", "verify: ", "Verify: ", "cmd: ", "check: ", "Check: ",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let trimmed = rest.trim();
            if trimmed.starts_with('`') {
                if let Some(end) = trimmed[1..].find('`') {
                    return Some(trimmed[1..=end].trim().to_string());
                }
            } else if let Some((cmd, _)) = trimmed.split_once(':') {
                if is_shell_command(cmd.trim()) {
                    return Some(cmd.trim().to_string());
                }
            } else if is_shell_command(trimmed) {
                return Some(trimmed.to_string());
            }
        }
    }

    // 3. Check if there is an embedded backticked command matching shell tools
    if let Some(start) = text.find('`') {
        if let Some(end) = text[start + 1..].find('`') {
            let cmd = &text[start + 1..start + 1 + end];
            if is_shell_command(cmd) {
                return Some(cmd.trim().to_string());
            }
        }
    }

    None
}

/// Check if a string looks like an executable command (cargo, git, pytest, bash, etc.).
fn is_shell_command(candidate: &str) -> bool {
    let trimmed = candidate.trim();
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let first = words[0];
    matches!(
        first,
        "cargo"
            | "git"
            | "bash"
            | "sh"
            | "pytest"
            | "python"
            | "python3"
            | "make"
            | "npm"
            | "pnpm"
            | "yarn"
            | "true"
            | "false"
            | "echo"
            | "test"
            | "grep"
            | "cat"
            | "curl"
            | "jq"
            | "aida"
    ) || first.starts_with("./")
        || first.starts_with("tests/")
        || first.ends_with(".sh")
}

/// Execute deterministic acceptance checks and evaluate residual prose criteria.
// trace:STORY-1424 | ai:antigravity
pub fn execute_graded_review(
    spec_id: &str,
    spec_title: &str,
    description: &str,
    diff_text: &str,
    reviewed_sha: &str,
    worktree_path: &Path,
    evaluator: Option<&dyn EvaluatorEngine>,
) -> Result<GradedReviewVerdict> {
    let criteria = parse_acceptance_criteria(description);

    let mut results = Vec::new();
    let mut machine_verified_count = 0;
    let mut machine_passed_count = 0;
    let mut machine_failed_count = 0;

    let mut residual_prose = Vec::new();

    // Rung 2: Execute deterministic machine checks
    for crit in &criteria {
        match crit {
            CriterionKind::Executable {
                command,
                description: _,
            } => {
                machine_verified_count += 1;

                let output_res = Command::new("bash")
                    .arg("-c")
                    .arg(command)
                    .current_dir(worktree_path)
                    .output();

                match output_res {
                    Ok(out) => {
                        let exit_code = out.status.code().unwrap_or(-1);
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let combined = format!("{}\n{}", stdout, stderr).trim().to_string();

                        if out.status.success() {
                            machine_passed_count += 1;
                            results.push(CriterionResult {
                                criterion: crit.clone(),
                                status: CriterionStatus::Passed,
                                output: Some(combined),
                                exit_code: Some(exit_code),
                                probability: Some(1.0),
                                confidence: Some(1.0),
                                heuristic: false,
                                model: None,
                                question_payload_hash: None,
                            });
                        } else {
                            machine_failed_count += 1;
                            results.push(CriterionResult {
                                criterion: crit.clone(),
                                status: CriterionStatus::Failed,
                                output: Some(combined),
                                exit_code: Some(exit_code),
                                probability: Some(0.0),
                                confidence: Some(1.0),
                                heuristic: false,
                                model: None,
                                question_payload_hash: None,
                            });
                        }
                    }
                    Err(e) => {
                        // PRIN-5: Fail closed on execution error
                        machine_failed_count += 1;
                        results.push(CriterionResult {
                            criterion: crit.clone(),
                            status: CriterionStatus::Failed,
                            output: Some(format!("Failed to execute command: {}", e)),
                            exit_code: Some(-1),
                            probability: Some(0.0),
                            confidence: Some(1.0),
                            heuristic: false,
                            model: None,
                            question_payload_hash: None,
                        });
                    }
                }
            }
            CriterionKind::Prose { text } => {
                residual_prose.push(text.clone());
            }
        }
    }

    let prose_count = residual_prose.len();

    // If any machine check failed, fast-fail immediately (Rung 2 veto)
    if machine_failed_count > 0 {
        return Ok(GradedReviewVerdict {
            spec_id: spec_id.to_string(),
            reviewed_sha: reviewed_sha.to_string(),
            overall_verdict: "rejected".to_string(),
            verdict_kind: format!("{:?}", VerdictKind::Rejected),
            machine_verified_count,
            machine_passed_count,
            prose_count,
            residual_prose_count: prose_count,
            escalated_to_seat: false,
            results,
            summary: format!(
                "{} of {} deterministic acceptance checks failed.",
                machine_failed_count, machine_verified_count
            ),
        });
    }

    // If 100% of criteria were machine-verified and all passed: auto-approve (Rung 2)
    if prose_count == 0 && machine_verified_count > 0 {
        return Ok(GradedReviewVerdict {
            spec_id: spec_id.to_string(),
            reviewed_sha: reviewed_sha.to_string(),
            overall_verdict: "approved".to_string(),
            verdict_kind: format!("{:?}", VerdictKind::Approved),
            machine_verified_count,
            machine_passed_count,
            prose_count: 0,
            residual_prose_count: 0,
            escalated_to_seat: false,
            results,
            summary: format!(
                "All {} acceptance criteria verified deterministically via executable checks.",
                machine_verified_count
            ),
        });
    }

    // Rung 3.5: Evaluate residual prose criteria via Jev System One
    let mut min_probability = 1.0f64;
    let mut min_confidence = 1.0f64;
    let mut confident_failure: Option<(f64, f64)> = None;
    let mut evaluation_failed = false;

    if let Some(eval) = evaluator {
        for text in &residual_prose {
            let context = format!(
                "Specification: {} - {}\nAcceptance Criterion: {}\n\nImplementation Diff:\n{}",
                spec_id,
                spec_title,
                text,
                diff_text.lines().take(40).collect::<Vec<_>>().join("\n")
            );
            let instruction = format!(
                "Does the provided implementation diff satisfy the acceptance criterion: '{}'?",
                text
            );

            match eval.evaluate_noul_sync(&context, &instruction) {
                Ok(resp) => {
                    if resp.noul < min_probability {
                        min_probability = resp.noul;
                    }
                    min_confidence = min_confidence.min(resp.confidence);
                    if resp.noul <= 0.20 && resp.confidence >= 0.85 {
                        confident_failure = Some((resp.noul, resp.confidence));
                    }
                    // A probability at either fast-decision boundary is only
                    // settled when its confidence also satisfies the ADR-55
                    // predicate.  Otherwise this criterion is residual work
                    // for Phase 3; marking it Passed/Failed here would make
                    // aggregate escalation silently drop it from the prompt.
                    let status = if resp.noul >= 0.95 && resp.confidence >= 0.90 {
                        CriterionStatus::Passed
                    } else if resp.noul <= 0.20 && resp.confidence >= 0.85 {
                        CriterionStatus::Failed
                    } else {
                        CriterionStatus::Escalated
                    };

                    results.push(CriterionResult {
                        criterion: CriterionKind::Prose { text: text.clone() },
                        status,
                        output: None,
                        exit_code: None,
                        probability: Some(resp.noul),
                        confidence: Some(resp.confidence),
                        heuristic: true,
                        model: Some(resp.model),
                        question_payload_hash: Some(resp.payload_hash),
                    });
                }
                Err(e) => {
                    // PRIN-5: Fail closed on evaluator error
                    evaluation_failed = true;
                    results.push(CriterionResult {
                        criterion: CriterionKind::Prose { text: text.clone() },
                        status: CriterionStatus::Escalated,
                        output: Some(format!("Evaluator error: {}", e)),
                        exit_code: None,
                        probability: None,
                        confidence: None,
                        heuristic: true,
                        model: None,
                        question_payload_hash: None,
                    });
                }
            }
        }
    } else {
        // No evaluator configured: all prose criteria remain escalated to Phase 3 seat
        for text in &residual_prose {
            results.push(CriterionResult {
                criterion: CriterionKind::Prose { text: text.clone() },
                status: CriterionStatus::Escalated,
                output: None,
                exit_code: None,
                probability: None,
                confidence: None,
                heuristic: false,
                model: None,
                question_payload_hash: None,
            });
        }
    }

    // Tri-state confidence escalation model (ADR-55):
    // 1. Fast-Pass: p >= 0.95 on all residual criteria AND no evaluator failure
    // 2. Fast-Fail: p <= 0.20 on any residual criterion
    // 3. Escalation Zone (0.20 < p < 0.95 or evaluator failure or offline): escalate to Phase 3 reviewer seat
    if !evaluation_failed
        && evaluator.is_some()
        && min_probability >= 0.95
        && min_confidence >= 0.90
    {
        Ok(GradedReviewVerdict {
            spec_id: spec_id.to_string(),
            reviewed_sha: reviewed_sha.to_string(),
            overall_verdict: "approved".to_string(),
            verdict_kind: format!("{:?}", VerdictKind::Approved),
            machine_verified_count,
            machine_passed_count,
            prose_count,
            residual_prose_count: 0,
            escalated_to_seat: false,
            results,
            summary: format!(
                "Passed {} machine check(s); residual prose criteria satisfied (p={:.2}, heuristic: true).",
                machine_passed_count, min_probability
            ),
        })
    } else if !evaluation_failed && evaluator.is_some() && confident_failure.is_some() {
        let (failure_probability, failure_confidence) = confident_failure.unwrap();
        Ok(GradedReviewVerdict {
            spec_id: spec_id.to_string(),
            reviewed_sha: reviewed_sha.to_string(),
            overall_verdict: "request-changes".to_string(),
            verdict_kind: format!("{:?}", VerdictKind::RequestChanges),
            machine_verified_count,
            machine_passed_count,
            prose_count,
            residual_prose_count: prose_count,
            escalated_to_seat: false,
            results,
            summary: format!(
                "Residual prose criterion failed evaluation (p={:.2} <= 0.20, confidence={:.2} >= 0.85, heuristic: true).",
                failure_probability, failure_confidence
            ),
        })
    } else {
        // Escalation zone
        Ok(GradedReviewVerdict {
            spec_id: spec_id.to_string(),
            reviewed_sha: reviewed_sha.to_string(),
            overall_verdict: "escalated".to_string(),
            verdict_kind: format!("{:?}", VerdictKind::Other),
            machine_verified_count,
            machine_passed_count,
            prose_count,
            residual_prose_count: prose_count,
            escalated_to_seat: true,
            results,
            summary: format!(
                "Passed {} machine check(s); residual prose criteria escalated to Phase 3 conversational reviewer seat.",
                machine_passed_count
            ),
        })
    }
}

/// Generate prompt for Phase 3 conversational reviewer seat when review escalates.
///
/// In accordance with STORY-1424:
/// - Settled machine checks are listed as passed and excluded from judgment.
/// - The reviewer is asked to evaluate ONLY the residual prose criteria.
// trace:STORY-1424 | ai:antigravity
pub fn generate_graded_reviewer_prompt(
    spec_id: &str,
    pr_number: Option<u64>,
    verdict: &GradedReviewVerdict,
) -> String {
    let mut prompt = match pr_number {
        Some(n) => format!("/aida-review --pr {n}"),
        None => format!("/aida-review --spec {spec_id}"),
    };

    if verdict.machine_passed_count > 0 {
        prompt.push_str("\n\n[GRADED REVIEW CONTEXT — STORY-1424]");
        prompt.push_str(&format!(
            "\nThe following criteria were MACHINE-VERIFIED at commit {} and are already SETTLED (do not re-evaluate):\n",
            &verdict.reviewed_sha[..std::cmp::min(10, verdict.reviewed_sha.len())]
        ));
        for r in &verdict.results {
            if r.status == CriterionStatus::Passed {
                if let CriterionKind::Executable { command, .. } = &r.criterion {
                    prompt.push_str(&format!("  - [PASSED (exit 0)] `{}`\n", command));
                }
            }
        }

        prompt.push_str(
            "\nPlease focus your evaluation SOLELY on the remaining residual prose criteria:\n",
        );
        for r in &verdict.results {
            if r.status == CriterionStatus::Escalated {
                if let CriterionKind::Prose { text } = &r.criterion {
                    prompt.push_str(&format!("  - {}\n", text));
                }
            }
        }
    }

    prompt
}
