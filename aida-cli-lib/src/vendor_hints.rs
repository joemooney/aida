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
//!   workflow for any agent to follow (args go in a trailing note: the
//!   command takes one NAME). The hint stays truthful.
//!
//! The vendor comes from `[agents] vendor` (project over global, the
//! STORY-761 knob), then a Claude-less `[agents] enabled` profile, read from
//! the main checkout first, defaulting to Claude. A surface that already
//! resolved a vendor (e.g. `aida queue work`'s launch vendor) passes it in
//! instead, so an explicit override is preserved.
// trace:BUG-2396 | ai:claude

use std::path::Path;

use crate::session::HeadlessVendor;

/// The hint vendor configured at `root`: `[agents] vendor`, else the first
/// enabled profile when `[agents] enabled` leaves Claude out (the codex-only
/// profile the launcher auto-picks, BUG-898). `None` when neither says.
fn configured_hint_vendor(root: &Path) -> Option<HeadlessVendor> {
    if let Some(v) = aida_core::agents_config::resolve_default_vendor(root)
        .as_deref()
        .and_then(HeadlessVendor::parse)
    {
        return Some(v);
    }
    let selection = crate::init_cmd::read_enabled_agent_selection(root)?;
    let enabled = crate::session::enabled_headless_vendors(selection);
    if enabled.contains(&HeadlessVendor::Claude) {
        return None;
    }
    enabled.first().copied()
}

/// The hint vendor for a path in the project (main checkout or a linked
/// worktree). Resolved from the main checkout first, because the
/// `.aida/agents.toml` knob is gitignored and absent from worktrees, then
/// from `start` itself, else Claude.
// trace:BUG-2396 | ai:claude
pub(crate) fn hint_vendor(start: &Path) -> HeadlessVendor {
    let main = crate::worktree_gc::main_worktree_root_from(start);
    if let Some(v) = configured_hint_vendor(&main) {
        return v;
    }
    if main != start {
        if let Some(v) = configured_hint_vendor(start) {
            return v;
        }
    }
    HeadlessVendor::Claude
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

/// Render from the shared descriptor after resolving project skill inventory.
// trace:BUG-2396 STORY-2539 | ai:codex
pub(crate) fn skill_ref(vendor: HeadlessVendor, skill: &str, args: &str) -> String {
    crate::vendors::skill_ref(
        vendor.profile(),
        skill,
        args,
        skill_is_portable(skill.trim_start_matches('/')),
    )
}

// trace:STORY-2539 | ai:codex
pub(crate) fn agent_new_cmd(vendor: HeadlessVendor, flags: &str, background: bool) -> String {
    crate::vendors::agent_new_cmd(vendor.profile(), flags, background)
}

// trace:STORY-2539 | ai:codex
pub(crate) fn launch_with_skill(vendor: HeadlessVendor, skill: &str, args: &str) -> String {
    crate::vendors::launch_with_skill(
        vendor.profile(),
        skill,
        args,
        skill_is_portable(skill.trim_start_matches('/')),
    )
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
    }

    /// A codex-only enabled profile (no `vendor` key) gets Codex hints, as
    /// the launcher auto-picks Codex for it (BUG-898).
    #[test]
    fn hint_vendor_follows_a_codex_only_enabled_profile() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(home.join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        assert_eq!(hint_vendor(&project), HeadlessVendor::Claude, "no config");

        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();
        assert_eq!(hint_vendor(&project), HeadlessVendor::Codex);

        // Claude still enabled: the Claude default stands.
        std::fs::write(
            project.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\", \"claude\"]\n",
        )
        .unwrap();
        assert_eq!(hint_vendor(&project), HeadlessVendor::Claude);
    }

    #[test]
    fn render_fallback_keeps_args_out_of_the_command() {
        assert_eq!(
            skill_ref(HeadlessVendor::Agy, "aida-pr", "BUG-1"),
            "aida skill render aida-pr  # then follow it for BUG-1"
        );
        assert_eq!(
            skill_ref(HeadlessVendor::Codex, "aida-burndown", " X "),
            "aida skill render aida-burndown  # then follow it for X"
        );
    }
}
