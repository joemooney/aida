//! Vendor-aware next-step hints (BUG-2396).
//!
//! CLI hints, recovery lines, status nudges and advisor mail used to hardcode
//! `claude` and `/aida-*` slash commands. A Codex-first project
//! (`[agents] vendor = "codex"`) was then told to run commands its agent
//! cannot run. Every such surface renders through these helpers, so the
//! vendor rule lives in one place:
//!
//! - Claude invokes a skill as `/aida-<name>`.
//! - Codex invokes a skill shipped in `.agents/skills/` as `$aida-<name>`.
//! - Anything else (a Claude-only skill on Codex, or a vendor with no known
//!   invocation syntax) gets `aida skill render aida-<name>`, which prints the
//!   workflow for any agent to follow. The hint stays truthful.
//!
//! The vendor comes from `[agents] vendor` (project over global, the
//! STORY-761 knob), defaulting to Claude. A surface that already resolved a
//! vendor (e.g. `aida queue work --vendor`) passes it in instead, so an
//! explicit override is preserved.
// trace:BUG-2396 | ai:claude

use std::path::Path;

use crate::session::HeadlessVendor;

/// The hint vendor for `project_root`: `[agents] vendor`, else Claude.
// trace:BUG-2396 | ai:claude
pub(crate) fn hint_vendor(project_root: &Path) -> HeadlessVendor {
    aida_core::agents_config::resolve_default_vendor(project_root)
        .as_deref()
        .and_then(HeadlessVendor::parse)
        .unwrap_or(HeadlessVendor::Claude)
}

/// [`hint_vendor`] for the current project, or Claude outside a project.
// trace:BUG-2396 | ai:claude
pub(crate) fn hint_vendor_here() -> HeadlessVendor {
    crate::find_project_root()
        .map(|root| hint_vendor(&root))
        .unwrap_or(HeadlessVendor::Claude)
}

/// Parse a resolved vendor token (`--vendor` value), falling back to Claude.
// trace:BUG-2396 | ai:claude
pub(crate) fn vendor_from_token(token: &str) -> HeadlessVendor {
    HeadlessVendor::parse(token).unwrap_or(HeadlessVendor::Claude)
}

/// `true` when `skill` ships in the portable `.agents/skills/` pack that Codex
/// reads (not Claude-only, not kept out of every pack, and has a body).
fn skill_is_portable(skill: &str) -> bool {
    use aida_core::scaffolding::inventory;
    skill.starts_with(inventory::AIDA_SKILL_PREFIX)
        && !inventory::claude_only_skills().contains_key(skill)
        && !inventory::is_not_a_skill(skill)
        && inventory::portable_skill_md_source(skill).is_some()
}

fn with_args(head: String, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        head
    } else {
        format!("{head} {args}")
    }
}

/// How `vendor` invokes skill `skill` (`aida-pr`, no leading slash) with
/// `args`: `/aida-pr`, `$aida-pr`, or the `aida skill render` fallback.
// trace:BUG-2396 | ai:claude
pub(crate) fn skill_ref(vendor: HeadlessVendor, skill: &str, args: &str) -> String {
    let skill = skill.trim_start_matches('/');
    match vendor {
        HeadlessVendor::Claude => with_args(format!("/{skill}"), args),
        HeadlessVendor::Codex if skill_is_portable(skill) => with_args(format!("${skill}"), args),
        _ => with_args(format!("aida skill render {skill}"), args),
    }
}

/// The `aida agent new <vendor>` subcommand token for `vendor`.
fn agent_new_token(vendor: HeadlessVendor) -> &'static str {
    match vendor {
        HeadlessVendor::Claude => "claude",
        HeadlessVendor::Codex => "codex",
        HeadlessVendor::Agy => "antigravity",
    }
}

/// `aida agent new <vendor> <flags>`. `background` adds `--bg`, which only
/// the Claude lane supports; other vendors get the foreground form.
// trace:BUG-2396 | ai:claude
pub(crate) fn agent_new_cmd(vendor: HeadlessVendor, flags: &str, background: bool) -> String {
    let mut cmd = with_args(format!("aida agent new {}", agent_new_token(vendor)), flags);
    if background && vendor == HeadlessVendor::Claude {
        cmd.push_str(" --bg");
    }
    cmd
}

/// Shell line that starts `vendor` with `skill args` as its first prompt.
/// The prompt is single-quoted so a Codex `$aida-*` mention is not expanded
/// by the shell. A vendor without a skill syntax gets `aida agent new`.
// trace:BUG-2396 | ai:claude
pub(crate) fn launch_with_skill(vendor: HeadlessVendor, skill: &str, args: &str) -> String {
    match vendor {
        HeadlessVendor::Claude | HeadlessVendor::Codex => {
            format!("{} '{}'", vendor.program(), skill_ref(vendor, skill, args))
        }
        HeadlessVendor::Agy => agent_new_cmd(vendor, &format!("--spec {}", args.trim()), false),
    }
}

/// The interactive agent CLI to start in a worktree (`claude` / `codex`),
/// with a trailing `# then <skill ref>` comment.
// trace:BUG-2396 | ai:claude
pub(crate) fn start_then_skill(vendor: HeadlessVendor, skill: &str, args: &str) -> String {
    format!(
        "{}    # then {}",
        vendor.program(),
        skill_ref(vendor, skill, args)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_keeps_slash_commands() {
        assert_eq!(skill_ref(HeadlessVendor::Claude, "aida-pr", ""), "/aida-pr");
        assert_eq!(
            skill_ref(HeadlessVendor::Claude, "/aida-implement", " TASK-1 "),
            "/aida-implement TASK-1"
        );
    }

    #[test]
    fn codex_uses_dollar_mentions_for_portable_skills() {
        assert_eq!(skill_ref(HeadlessVendor::Codex, "aida-pr", ""), "$aida-pr");
        assert_eq!(
            skill_ref(HeadlessVendor::Codex, "aida-pickup", "BUG-1"),
            "$aida-pickup BUG-1"
        );
    }

    #[test]
    fn codex_claude_only_and_unknown_vendor_skills_fall_back_truthfully() {
        // aida-burndown is Claude-only: Codex has no `$aida-burndown`.
        assert_eq!(
            skill_ref(HeadlessVendor::Codex, "aida-burndown", ""),
            "aida skill render aida-burndown"
        );
        assert_eq!(
            skill_ref(HeadlessVendor::Agy, "aida-pr", ""),
            "aida skill render aida-pr"
        );
    }

    #[test]
    fn agent_new_bg_is_claude_only() {
        assert_eq!(
            agent_new_cmd(HeadlessVendor::Claude, "--spec X", true),
            "aida agent new claude --spec X --bg"
        );
        assert_eq!(
            agent_new_cmd(HeadlessVendor::Codex, "--spec X", true),
            "aida agent new codex --spec X"
        );
        assert_eq!(
            agent_new_cmd(HeadlessVendor::Agy, "--role advisor", false),
            "aida agent new antigravity --role advisor"
        );
    }

    #[test]
    fn launch_lines_quote_the_prompt_for_the_shell() {
        assert_eq!(
            launch_with_skill(HeadlessVendor::Codex, "aida-pickup", "BUG-1"),
            "codex '$aida-pickup BUG-1'"
        );
        assert_eq!(
            launch_with_skill(HeadlessVendor::Claude, "aida-pickup", "BUG-1"),
            "claude '/aida-pickup BUG-1'"
        );
        assert_eq!(
            start_then_skill(HeadlessVendor::Codex, "aida-implement", "BUG-1"),
            "codex    # then $aida-implement BUG-1"
        );
    }

    #[test]
    fn hint_vendor_reads_agents_vendor_from_project_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[agents]\nvendor = \"codex\"\n",
        )
        .unwrap();
        assert_eq!(hint_vendor(dir.path()), HeadlessVendor::Codex);
        assert_eq!(vendor_from_token("bogus"), HeadlessVendor::Claude);
    }
}
