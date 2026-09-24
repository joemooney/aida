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
    render_role_launch_prompt(agent_type, role, spec, role_file.as_ref(), ship_instruction)
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
    let mut out =
        String::from("Read your AIDA launch context first:\ncat \"$AIDA_AGENT_CONTEXT_FILE\"\n");
    if let Some(spec) = spec {
        out.push_str(&format!("aida show {spec}\n"));
    }
    out.push_str(&format!("aida brief list --for-agent {agent_type}"));
    out
}

fn default_body(agent_type: &str, seat: &str, spec: Option<&str>, ship: &str) -> String {
    match (seat, spec) {
        ("implementer", Some(spec)) => format!(
            "Implement {spec} per its acceptance criteria. Stay in single-spec scope for {spec}. \
             Standard cadence: inspect context, make bounded changes, run relevant tests, run \
             cargo fmt --all --check, commit with trailer ({spec}) and trace:{spec}, {ship}"
        ),
        ("implementer", None) => "You are the implementer seat: you build one spec at a time, \
             bounded to its acceptance criteria. No spec was assigned at launch. Find work with \
             `aida queue next --for implementer` or a pending brief, then claim it with \
             `aida worktree enter <ID>` so it gets its own worktree and lease. Do not review or \
             merge your own work."
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
        ("reviewer", Some(spec)) => format!(
            "You are the reviewer seat: evaluate, do not implement. Review the open PR for {spec} \
             against its acceptance criteria, prioritising correctness and regression risk; run \
             targeted tests where useful. Record the verdict with `aida review record {spec} --pr \
             <N> --verdict approved|request-changes|rejected --summary \"<why>\"`. Do not push fixes."
        ),
        ("reviewer", None) => "You are the reviewer seat: evaluate, do not implement. No spec was \
             assigned at launch. Find the next PR awaiting review with \
             `aida queue next --for reviewer`, then record your verdict with \
             `aida review record <ID> --pr <N> --verdict ...`. Do not push fixes."
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
