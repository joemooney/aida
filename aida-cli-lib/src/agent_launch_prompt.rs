//! Role-aware initial prompts for `aida agent new`.
//!
//! Every launch gets a short, role-specific first message: a spec launch gets
//! that role's contract for the spec (implementer builds, reviewer evaluates,
//! advisor judges, product grooms, integrator lands), and a launch without a
//! spec gets a brief orientation naming the role, its boundary, and where to
//! find work. The body comes from the role file (`launch_prompt` /
//! `launch_prompt_spec` in `~/.aida/roles/<role>.toml` or the project role)
//! when set, otherwise from the embedded defaults below, the same lookup
//! order the launch-context snapshot uses for role guidance. The launch-context
//! read commands are always prepended so every vendor starts from the same
//! machine-readable context.
//!
//! These are startup pointers, not the discipline pack: keep each one short.
// trace:STORY-1471 | ai:claude

use crate::{canonical_role_name, load_role, seat_next_command, RoleState};

/// First line of every launch-context preamble. The Codex interactive renderer
/// keys on it to find the invocation line that follows the preamble.
// trace:BUG-2387 | ai:claude
pub(crate) const LAUNCH_CONTEXT_PREAMBLE: &str = "Read your AIDA launch context first:";

/// Resolve and render the automatic initial prompt for a launch, reading the
/// role file from `project_root` when one exists.
pub(crate) fn role_launch_prompt(
    project_root: &std::path::Path,
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
    ship_instruction: &str,
) -> String {
    let role_file = role
        .filter(|r| !r.trim().is_empty())
        .and_then(|r| load_role(project_root, r).ok())
        .map(|(state, _path)| state);
    let prompt =
        render_role_launch_prompt(agent_type, role, spec, role_file.as_ref(), ship_instruction);
    // trace:BUG-2512 | ai:codex
    match crate::project_checks::load(project_root) {
        Ok(checks) => format!("{prompt}\n\n{}", checks.prompt()),
        Err(error) => format!("{prompt}\n\nProject validation config unavailable: {error:#}"),
    }
}

/// Pure renderer: the role file (when given) overrides the embedded body.
pub(crate) fn render_role_launch_prompt(
    agent_type: &str,
    role: Option<&str>,
    spec: Option<&str>,
    role_file: Option<&RoleState>,
    ship_instruction: &str,
) -> String {
    // A launch with a spec but no role keeps the historical implementer
    // contract; a launch with neither is asked to establish its role first.
    let seat = match (role.map(str::trim).filter(|r| !r.is_empty()), spec) {
        (Some(r), _) => canonical_role_name(r),
        (None, Some(_)) => "implementer".to_string(),
        (None, None) => "unspecified".to_string(),
    };

    let custom = role_file.and_then(|state| {
        let text = if spec.is_some() {
            state.launch_prompt_spec.as_deref()
        } else {
            state.launch_prompt.as_deref()
        };
        text.map(str::trim).filter(|t| !t.is_empty())
    });

    let body = match custom {
        Some(template) => template
            .replace("{role}", &seat)
            .replace("{agent}", agent_type)
            .replace("{spec}", spec.unwrap_or("")),
        None => default_body(agent_type, &seat, spec, ship_instruction),
    };

    format!("{}\n\n{body}", context_header(agent_type, spec))
}

fn context_header(agent_type: &str, spec: Option<&str>) -> String {
    let mut out = format!("{LAUNCH_CONTEXT_PREAMBLE}\ncat \"$AIDA_AGENT_CONTEXT_FILE\"\n");
    if let Some(spec) = spec {
        out.push_str(&format!("aida show {spec}\n"));
    }
    out.push_str(&format!("aida brief list --for-agent {agent_type}"));
    out
}

fn default_body(agent_type: &str, seat: &str, spec: Option<&str>, ship: &str) -> String {
    match (seat, spec) {
        // trace:TASK-2879 | ai:codex
        ("implementer", Some(spec)) => format!(
            "Implement {spec} per its acceptance criteria. Stay in single-spec scope for {spec}. \
             Standard cadence: inspect context, make bounded changes, run relevant tests and project-configured formatting, and add trace:{spec}. In a native sandbox \
             (AIDA_CONTAINMENT_POSTURE=native), leave tested edits uncommitted and exit with a \
             handoff listing files, validation and blockers; the host commits after exit and \
             publishes separately. Do not commit, push, open a PR or run queue done in that \
             seat. Otherwise, commit with trailer ({spec}), {ship}"
        ),
        ("implementer", None) => "You are the implementer seat: you build one spec at a time, \
             bounded to its acceptance criteria. No spec was assigned at launch. Find work with \
             `aida queue next --for implementer` or a pending brief, then claim it with \
             `aida worktree add <ID>` (it takes the lease and prints the worktree path), cd into \
             that printed path and work only there, never in the primary checkout. Do not \
             review or merge your own work."
            .to_string(),
        ("advisor", Some(spec)) => format!(
            "You are the advisor seat: independent judgment and gates, not implementation. \
             Assess {spec}: disposition, design forks, acceptance sufficiency and merge readiness. \
             Record the decision on the spec (`aida comment add {spec} \"...\"`) and route build \
             work to an implementer. Never implement or merge code you authored."
        ),
        ("advisor", None) => "You are the advisor seat: independent judgment and gates, not \
             implementation. No spec was assigned at launch. Run `aida advisor` for what is \
             waiting at your gate and `aida awaiting` for your inbox. Wait for new work with a \
             shell/event watcher, never a model-side polling loop."
            .to_string(),
        // trace:BUG-2508 | ai:codex
        // trace:STORY-2697 | ai:codex
        ("reviewer", Some(spec)) => format!(
            "You are the reviewer seat: evaluate, do not implement. Review the open PR for {spec} \
             against its acceptance criteria, prioritising correctness and regression risk; run \
             targeted tests where useful. Record the verdict with `aida review record {spec} --pr \
             <N> --verdict approved|request-changes|rejected|waiting --summary \"<why>\"`. When there are no actionable defects, record APPROVED even if CI is pending or running; state \"CI pending at review time\" in the rationale. CI alone is not a reason for WAITING: readiness still requires exact-head CI, and landing still requires ship CI or stacked-tip CI. WAITING is reserved for independent external evidence (such as an operator-only canary), or a specific named test result on which the reviewer's judgment depends. Name the dependency, responsible actor and resume condition, with no substantive findings. A red merge-hold-gate or supervised human/advisor clearance that this review's own fresh APPROVED verdict can release is not external evidence: record approved when there are no actionable defects. Approval does not release the hold or authorize merge; leave clearance to the authorized actor. Outstanding independent evidence, such as an operator-only canary, still requires waiting even on a held PR. Resume requires a fresh review at the current head. Code defects require request-changes. Do not push fixes."
        ),
        ("reviewer", None) => "You are the reviewer seat: evaluate, do not implement. No spec was \
             assigned at launch. Find the next PR awaiting review with \
             `aida queue next --for reviewer`, then record your verdict with \
             `aida review record <ID> --pr <N> --verdict ...`. When there are no actionable defects, record APPROVED even if CI is pending or running; state \"CI pending at review time\" in the rationale. CI alone is not a reason for WAITING: readiness still requires exact-head CI, and landing still requires ship CI or stacked-tip CI. WAITING is reserved for independent external evidence (such as an operator-only canary), or a specific named test result on which the reviewer's judgment depends. Name the dependency, responsible actor and resume condition, with no substantive findings. A red merge-hold-gate or supervised human/advisor clearance that this review's own fresh APPROVED verdict can release is not external evidence: record approved when there are no actionable defects. Approval does not release the hold or authorize merge; leave clearance to the authorized actor. Outstanding independent evidence, such as an operator-only canary, still requires waiting even on a held PR. Resume requires a fresh review at the current head. Code defects require request-changes. Do not push fixes."
            .to_string(),
        ("product", Some(spec)) => format!(
            "You are the product seat: intake, requirement quality, ordering and wave continuity. \
             Groom {spec}: sharpen its acceptance criteria, check its type, priority and \
             relationships, and place it in the right queue order. Leave disposition calls to the \
             advisor and implementation to implementers."
        ),
        ("product", None) => "You are the product seat: intake, requirement quality, ordering and \
             wave continuity. No spec was assigned at launch. Start with \
             `aida queue next --for product` and the draft inbox (`aida list --status draft`); \
             groom and order work so the current wave stays fed. Leave disposition calls to the \
             advisor and implementation to implementers."
            .to_string(),
        ("integrator", Some(spec)) => format!(
            "You are the integrator seat: mechanical integration only. Land the open PR for \
             {spec} on the default branch: rebase if stale, resolve mechanical conflicts only, \
             merge once CI is green and a review verdict is recorded, then run `aida pull`. \
             Escalate any conflict that needs a design call to the advisor."
        ),
        ("integrator", None) => "You are the integrator seat: mechanical integration only. No \
             spec was assigned at launch. Run `aida integrate` to see finished work with open \
             PRs, then land them one at a time in dependency order. Escalate any conflict that \
             needs a design call to the advisor."
            .to_string(),
        ("unspecified", _) => format!(
            "No role was provided at launch. Establish whether you are acting as product, \
             advisor, implementer, reviewer or integrator before changing anything; pending \
             briefs for `{agent_type}` are the first place to look."
        ),
        (other, Some(spec)) => format!(
            "You are launched in the `{other}` role. Examine {spec} through that role's lens as \
             described in the launch context; do not take implementation ownership unless the \
             role says so."
        ),
        (other, None) => format!(
            "You are launched in the `{other}` role. No spec was assigned at launch. Follow the \
             role guidance in the launch context and find work with `{}`.",
            seat_next_command(other)
        ),
    }
}

#[cfg(test)]
mod native_finish_tests {
    // trace:TASK-2879 | ai:codex
    #[test]
    fn task_2879_native_role_prompt_defers_git_and_queue_to_host() {
        let prompt = super::render_role_launch_prompt(
            "codex",
            Some("implementer"),
            Some("TASK-2879"),
            None,
            "open a PR and stop",
        );
        assert!(prompt.contains("AIDA_CONTAINMENT_POSTURE=native"));
        assert!(prompt.contains("leave tested edits uncommitted"));
        assert!(prompt.contains("host commits after exit"));
        assert!(prompt.contains("Do not commit, push, open a PR or run queue done"));
        assert!(prompt.contains("Otherwise, commit with trailer (TASK-2879)"));
    }
}

// trace:BUG-2508 | ai:codex
#[cfg(test)]
mod bug_2508_tests {
    #[test]
    fn bug_2508_reviewer_prompts_distinguish_hold_from_external_evidence() {
        let mut prompts = vec![crate::role::default_role_guidance("reviewer")];
        for spec in [None, Some("BUG-2508")] {
            prompts.push(super::render_role_launch_prompt(
                "codex",
                Some("reviewer"),
                spec,
                None,
                "",
            ));
        }
        for prompt in prompts {
            assert!(
                prompt.contains("own fresh APPROVED verdict can release is not external evidence")
            );
            assert!(prompt.contains("record approved when there are no actionable defects"));
            assert!(prompt.contains("CI pending at review time"));
            assert!(prompt.contains("specific named test result"));
            assert!(!prompt.contains("external CI or host evidence"));
            assert!(prompt.contains("operator-only canary, still requires waiting"));
            assert!(prompt.contains("Approval does not release the hold or authorize merge"));
        }
    }

    // trace:STORY-2697 | ai:codex
    #[test]
    fn bug_2508_review_templates_carry_both_held_pr_fixtures() {
        for template in [
            include_str!("../../aida-core/templates/skills/aida-review.md"),
            include_str!("../../aida-core/templates/commands/aida-review.md"),
        ] {
            let rows: Vec<_> = template
                .lines()
                .filter(|line| line.starts_with("| Green except merge-hold-gate |"))
                .collect();
            assert_eq!(rows.len(), 2);
            assert!(rows[0].contains("fresh APPROVED verdict"));
            assert!(rows[0].ends_with("| None | APPROVED |"));
            assert!(rows[1].contains("fresh APPROVED verdict"));
            assert!(rows[1].ends_with("| Operator-only canary result | WAITING |"));
            assert!(template.contains("Approval does not release the hold or authorize merge"));
            assert!(template.contains("CI pending at review time"));
            assert!(!template.contains("external CI or host evidence"));
            assert!(!template.contains("before Approving"));
            assert!(!template.contains("Do **not** proceed to the Approved verdict"));
            assert!(template.contains("| Pending or running | Fresh APPROVED verdict may release Rework hold | None | APPROVED |"));
            assert!(template.contains("| Pending or running | Any | Specific named test result needed for judgment | WAITING |"));
        }
    }
}
