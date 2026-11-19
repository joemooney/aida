// trace:TASK-3428 | ai:codex
use crate::{
    agent_new_claude, agent_new_codex, session, slugify, AgentContextOptions,
    AgentDefaultFlagOptions, AgentPromptOptions, AgentResumeOptions,
};
use anyhow::Result;
use std::io::IsTerminal;

pub(crate) fn guided_review_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         aida show {spec}\n\
         aida review {spec} --no-agent\n\n\
         You are the guided-review advisor for {spec}. Keep this interactive and bounded: \
         inspect the linked review surface, compare the diff to the spec acceptance criteria, \
         surface consequential review forks to the operator with concrete options and a \
         recommendation, and finish by telling the operator whether to approve, request changes, \
         inspect the diff manually, or defer. Do not auto-merge."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuidedReviewLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

pub(crate) fn guided_review_launch_request(spec: &str) -> GuidedReviewLaunchRequest {
    GuidedReviewLaunchRequest {
        role: Some("advisor".to_string()),
        // A guided review shell is an advisor reviewing an already-held surface,
        // not a new implementation claim. Keep the spec in the prompt/name only
        // so `agent new` does not run `session_start` for a Done spec.
        // trace:STORY-818 | ai:codex
        spec: None,
        prompt: guided_review_prompt(spec),
        name: Some(format!("guided-review-{}", slugify(spec))),
        description: Some(format!("guided review for {spec}")),
    }
}

pub(crate) fn derisk_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         /aida-derisk {spec}\n\n\
         You are the de-risk advisor for {spec}. Follow the /aida-derisk skill: \
         read the spec and graph, classify decision/blast-radius risk, surface \
         load-bearing forks to the operator, record decisions as ADRs, fold \
         decisions and testable safety gates into acceptance, lint the result, \
         and only write a new execution_mode after explicit operator confirmation. \
         Do not implement the spec."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeriskLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

// trace:TASK-1235 | ai:codex
pub(crate) fn derisk_launch_request(spec: &str) -> DeriskLaunchRequest {
    DeriskLaunchRequest {
        role: Some("advisor".to_string()),
        // De-risking is an advisor disposition session, not an implementation
        // claim. Keep the target in the prompt/name so the launcher does not
        // acquire an implementer-style spec worktree or mutate status.
        spec: None,
        prompt: derisk_prompt(spec),
        name: Some(format!("derisk-{}", slugify(spec))),
        description: Some(format!("de-risk {spec}")),
    }
}

/// `aida derisk <SPEC>` — vendor-aware launcher for the `/aida-derisk` skill.
/// The Rust command is deliberately thin: it starts an advisor session seeded
/// with the skill prompt and leaves all mode decisions inside the skill's
/// explicit-confirmation workflow.
// trace:TASK-1235 | ai:codex
pub(crate) fn handle_derisk_command(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("aida derisk needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = derisk_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "aida derisk does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// STORY-818: `aida human review <SPEC> --guided` — spawn an interactive
/// advisor shell for review, using the existing `aida agent new` launch-context
/// machinery instead of inventing a second session substrate. The launched
/// agent deliberately has no `--spec` scope: review normally runs while the spec
/// is Done, and a spec-scoped `agent new` means implementation lease/worktree
/// semantics.
// trace:STORY-818 | ai:codex
pub(crate) fn handle_guided_human_review(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("guided review needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = guided_review_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "guided review does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}
