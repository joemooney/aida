use anyhow::Result;
use serde::Serialize;

use aida_core::{Comment, Requirement, RequirementStatus, RequirementType, Storage};

use crate::{events, findings};

const ATTEMPTS_FIELD: &str = "supervisor.attempts";
const LAST_KIND_FIELD: &str = "supervisor.last_kind";
const NEEDS_HUMAN_TAG: &str = "needs-human";

#[derive(Debug, Clone, Copy)]
pub(crate) struct SuperviseOpts {
    pub(crate) execute: bool,
    pub(crate) max_attempts: u32,
    pub(crate) max: Option<usize>,
    pub(crate) json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum ParkClass {
    Transient,
    NeedsHuman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SuperviseDecision {
    pub(crate) spec: String,
    pub(crate) class: ParkClass,
    pub(crate) reason: String,
    pub(crate) attempts: u32,
    pub(crate) action: String,
}

/// STORY-1051: top-level re-drive supervisor. It scans parked
/// `NeedsAttention` specs, retries only typed transient parks, and escalates
/// repeated transient parks to human triage after a capped attempt count.
pub(crate) fn handle_supervise_command(
    storage: &Storage,
    project_root: &std::path::Path,
    opts: SuperviseOpts,
) -> Result<()> {
    let mut store = storage.load()?;
    let mut decisions = Vec::new();
    let mut launches = Vec::new();
    let mut cap_findings = Vec::new();
    let mut changed = false;
    let mut launched = 0usize;

    let mut parked: Vec<usize> = store
        .requirements
        .iter()
        .enumerate()
        .filter_map(|(idx, r)| matches!(r.status, RequirementStatus::NeedsAttention).then_some(idx))
        .collect();
    parked.sort_by(|a, b| {
        store.requirements[*a]
            .modified_at
            .cmp(&store.requirements[*b].modified_at)
    });

    for idx in parked {
        let req = &mut store.requirements[idx];
        let spec = req.display_id();
        let decision = classify_requirement(req);
        match decision.class {
            ParkClass::NeedsHuman => decisions.push(decision),
            ParkClass::Transient => {
                if decision.attempts >= opts.max_attempts {
                    if opts.execute {
                        reclassify_needs_human(req, opts.max_attempts);
                        cap_findings.push((
                            spec.clone(),
                            decision.reason.clone(),
                            decision.attempts,
                        ));
                        events::emit(
                            project_root,
                            &events::Event::new(
                                Some(spec.clone()),
                                "",
                                events::EventKind::ReclassifiedNeedsHuman {
                                    kind: decision.reason.clone(),
                                    attempts: decision.attempts,
                                },
                            ),
                        );
                        changed = true;
                    }
                    decisions.push(SuperviseDecision {
                        action: "reclassify-needs-human".to_string(),
                        ..decision
                    });
                    continue;
                }

                if opts.max.is_some_and(|max| launched >= max) {
                    decisions.push(SuperviseDecision {
                        action: "skip-max-this-run".to_string(),
                        ..decision
                    });
                    continue;
                }

                let next_attempt = decision.attempts + 1;
                if opts.execute {
                    let phase = req
                        .failure_reason
                        .as_ref()
                        .map(|fr| fr.phase.clone())
                        .unwrap_or_else(|| "supervisor".to_string());
                    req.status = RequirementStatus::Approved;
                    req.failure_reason = None;
                    req.custom_fields
                        .insert(ATTEMPTS_FIELD.to_string(), next_attempt.to_string());
                    req.custom_fields
                        .insert(LAST_KIND_FIELD.to_string(), decision.reason.clone());
                    req.modified_at = chrono::Utc::now();
                    changed = true;
                    launches.push(spec.clone());
                    launched += 1;
                    events::emit(
                        project_root,
                        &events::Event::new(
                            Some(spec.clone()),
                            "",
                            events::EventKind::SpecRetried {
                                phase,
                                cause: decision.reason.clone(),
                                attempt: next_attempt,
                                max: opts.max_attempts,
                            },
                        ),
                    );
                }
                decisions.push(SuperviseDecision {
                    attempts: next_attempt,
                    action: if opts.execute {
                        "re-drive".to_string()
                    } else {
                        "would-re-drive".to_string()
                    },
                    ..decision
                });
            }
        }
    }

    for (spec, kind, attempts) in cap_findings {
        file_cap_finding(&mut store, &spec, &kind, attempts);
    }

    if changed {
        storage.save(&store)?;
    }

    render_decisions(&decisions, opts.json)?;

    if opts.execute {
        for spec in launches {
            launch_redrive(project_root, &spec)?;
        }
    }

    Ok(())
}

pub(crate) fn classify_requirement(req: &Requirement) -> SuperviseDecision {
    let spec = req.display_id();
    let attempts = supervisor_attempts(req);
    if req.tags.contains(NEEDS_HUMAN_TAG) {
        return SuperviseDecision {
            spec,
            class: ParkClass::NeedsHuman,
            reason: NEEDS_HUMAN_TAG.to_string(),
            attempts,
            action: "leave-for-human".to_string(),
        };
    }
    if let Some(attention) = &req.attention_reason {
        return SuperviseDecision {
            spec,
            class: ParkClass::NeedsHuman,
            reason: attention.category.to_string(),
            attempts,
            action: "leave-for-human".to_string(),
        };
    }
    let Some(fr) = &req.failure_reason else {
        return SuperviseDecision {
            spec,
            class: ParkClass::NeedsHuman,
            reason: "missing-failure-reason".to_string(),
            attempts,
            action: "leave-for-human".to_string(),
        };
    };
    let class = if is_transient_failure_kind(&fr.kind) {
        ParkClass::Transient
    } else {
        ParkClass::NeedsHuman
    };
    let action = match class {
        ParkClass::Transient => "would-re-drive",
        ParkClass::NeedsHuman => "leave-for-human",
    };
    SuperviseDecision {
        spec,
        class,
        reason: fr.kind.clone(),
        attempts,
        action: action.to_string(),
    }
}

// trace:STORY-1051 | ai:codex
pub(crate) fn is_transient_failure_kind(kind: &str) -> bool {
    matches!(
        normalize_kind(kind).as_str(),
        "watchdog"
            | "no-verdict"
            | "no-pr"
            | "tool-exit"
            | "lease-conflict"
            | "cache-locked"
            | "headless-wait"
            | "spawn"
            | "inconclusive"
    )
}

fn normalize_kind(kind: &str) -> String {
    kind.trim()
        .chars()
        .map(|c| match c {
            '_' | ' ' => '-',
            c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
            c => c,
        })
        .collect()
}

fn supervisor_attempts(req: &Requirement) -> u32 {
    req.custom_fields
        .get(ATTEMPTS_FIELD)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
}

fn reclassify_needs_human(req: &mut Requirement, max_attempts: u32) {
    req.tags.insert(NEEDS_HUMAN_TAG.to_string());
    req.add_comment(Comment::new(
        "supervisor".to_string(),
        format!(
            "Supervisor retry cap exhausted after {max_attempts} transient re-drive attempt(s). \
             Leaving this parked for human triage."
        ),
    ));
    req.modified_at = chrono::Utc::now();
}

fn file_cap_finding(
    store: &mut aida_core::RequirementsStore,
    spec: &str,
    kind: &str,
    attempts: u32,
) {
    let mut finding = Requirement::new(
        format!("Supervisor retry cap exhausted for {spec}"),
        format!(
            "{spec} remained parked after {attempts} supervised re-drive attempt(s) \
             for transient failure `{kind}`. Inspect the latest run and decide whether \
             the failure is environmental, a real product/code issue, or needs a new spec."
        ),
    );
    finding.req_type = RequirementType::Task;
    finding.status = RequirementStatus::Draft;
    finding.owner = "supervisor".to_string();
    finding
        .tags
        .insert(format!("{}{}", findings::FROM_ADVISOR_PREFIX, spec));
    finding.tags.insert("kind:supervisor-cap".to_string());
    finding.tags.insert("severity:major".to_string());
    finding.tags.insert(format!("linked:{spec}"));
    store.add_requirement_with_id(finding, None, Some("TASK"));
}

fn render_decisions(decisions: &[SuperviseDecision], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(decisions)?);
        return Ok(());
    }
    if decisions.is_empty() {
        println!("No parked specs to supervise.");
        return Ok(());
    }
    println!("Supervisor decisions");
    for d in decisions {
        println!(
            "  {:<14} {:<10} {:<22} attempts={} action={}",
            d.spec,
            match d.class {
                ParkClass::Transient => "transient",
                ParkClass::NeedsHuman => "human",
            },
            d.reason,
            d.attempts,
            d.action
        );
    }
    Ok(())
}

fn launch_redrive(project_root: &std::path::Path, spec: &str) -> Result<()> {
    let status = std::process::Command::new(std::env::current_exe()?)
        .current_dir(project_root)
        .args([
            "queue",
            "work",
            spec,
            "--auto-complete",
            "--force-claim",
            "--no-human",
            "both",
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "supervisor re-drive for {spec} exited {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "with a signal".to_string())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{FailureReason, PuntCategory};

    fn req_with_failure(kind: &str) -> Requirement {
        let mut req = Requirement::new("parked".to_string(), "parked".to_string());
        req.spec_id = Some("STORY-1".to_string());
        req.status = RequirementStatus::NeedsAttention;
        req.failure_reason = Some(FailureReason {
            phase: "implementer".to_string(),
            phase_index: 1,
            kind: kind.to_string(),
            detail: "failed".to_string(),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        });
        req
    }

    #[test]
    fn classifies_transient_failure_kinds() {
        for kind in [
            "watchdog",
            "no-verdict",
            "no-pr",
            "tool-exit",
            "lease-conflict",
            "cache-locked",
            "headless-wait",
        ] {
            assert!(is_transient_failure_kind(kind), "{kind}");
        }
        assert!(!is_transient_failure_kind("ci-red"));
        assert!(!is_transient_failure_kind("request-changes"));
    }

    #[test]
    fn design_fork_attention_stays_human() {
        let mut req = req_with_failure("watchdog");
        req.attention_reason = Some(aida_core::AttentionReason {
            category: PuntCategory::DesignFork,
            detail: "pick an architecture".to_string(),
            lean: None,
            raised_by: None,
            raised_at: chrono::Utc::now(),
        });
        let decision = classify_requirement(&req);
        assert_eq!(decision.class, ParkClass::NeedsHuman);
        assert_eq!(decision.reason, "design-fork");
    }

    #[test]
    fn attempt_count_reads_custom_field() {
        let mut req = req_with_failure("watchdog");
        req.custom_fields
            .insert(ATTEMPTS_FIELD.to_string(), "2".to_string());
        let decision = classify_requirement(&req);
        assert_eq!(decision.class, ParkClass::Transient);
        assert_eq!(decision.attempts, 2);
    }

    #[test]
    fn needs_human_tag_prevents_duplicate_reclassification() {
        let mut req = req_with_failure("watchdog");
        req.tags.insert(NEEDS_HUMAN_TAG.to_string());
        let decision = classify_requirement(&req);
        assert_eq!(decision.class, ParkClass::NeedsHuman);
        assert_eq!(decision.reason, NEEDS_HUMAN_TAG);
    }
}
