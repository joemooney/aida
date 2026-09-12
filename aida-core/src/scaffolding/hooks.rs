use super::*;

// The git hook bodies below are pulled straight out of their master templates in
// `aida-core/templates/hooks/` with `include_str!`, so each template is the ONLY
// source of that hook's text.
//
// They previously did a runtime `EMBEDDED_TEMPLATES` lookup with a full inline
// copy of an older hook body as an "impossible in practice" fallback. Those
// copies silently rotted: the pre-commit fallback had lost the TASK-917
// sentinel, the BUG-651 robust binary resolution, the TASK-984 glyph lint and
// the TASK-144 move detection. `include_str!` deletes the second copy outright
// and turns a missing template into a COMPILE error rather than a scaffolded but
// stale hook — a build.rs `embed_directory` miss is silent, so the old shape
// could have shipped a years-old hook to a user without any signal.
//
// `trim_end()` mirrors how `build.rs` embeds the same files, so scaffolded
// output stays byte-identical to the previous embedded-template path (pinned by
// `hook_bodies_match_master_templates` below).
// trace:TASK-147 trace:BUG-26 trace:TASK-917 | ai:claude
impl Scaffolder {
    /// Generate commit-msg git hook content — the master template carries the
    /// strict REQ_ID_PATTERN plus conventional-commit checks.
    pub(super) fn generate_commit_msg_hook(&self) -> String {
        include_str!("../../templates/hooks/aida-commit-msg")
            .trim_end()
            .to_string()
    }

    /// Generate pre-commit git hook content.
    pub(super) fn generate_pre_commit_hook(&self) -> String {
        include_str!("../../templates/hooks/aida-pre-commit.sh")
            .trim_end()
            .to_string()
    }

    /// Generate the post-commit git hook content — direct `--no-verify` bypass
    /// detection, paired with the pre-commit sentinel.
    pub(super) fn generate_post_commit_hook(&self) -> String {
        include_str!("../../templates/hooks/aida-post-commit.sh")
            .trim_end()
            .to_string()
    }

    /// Generate Claude Code validate-commit hook content
    // trace:BUG-1092 | ai:codex
    pub(super) fn generate_validate_commit_hook(&self) -> String {
        include_str!("../../templates/hooks/aida-validate-commit.sh")
            .trim_end()
            .to_string()
    }

    /// Generate Claude Code track-commits hook content
    // trace:BUG-1092 | ai:codex
    pub(super) fn generate_track_commits_hook(&self) -> String {
        include_str!("../../templates/hooks/aida-track-commits.sh")
            .trim_end()
            .to_string()
    }

    /// Generate the SessionStart role-context hook content. Sourced from
    /// the embedded template (`templates/hooks/aida-role-context.sh`) so
    /// the binary, the master template, and the scaffolded copy stay in
    /// sync — unlike the older hardcoded hooks above which predate the
    /// embedded-template system. trace:TASK-27 | ai:claude
    pub(super) fn generate_role_context_hook(&self) -> String {
        crate::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-role-context.sh")
            .copied()
            .unwrap_or("")
            .to_string()
    }

    /// Generate the SubagentStart harness worktree lease hook content.
    /// trace:TASK-702 | ai:claude
    pub(super) fn generate_subagent_start_hook(&self) -> String {
        crate::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-subagent-start.sh")
            .copied()
            .unwrap_or("")
            .to_string()
    }

    /// Generate the SubagentStop harness worktree lease hook content.
    /// trace:TASK-702 | ai:claude
    pub(super) fn generate_subagent_stop_hook(&self) -> String {
        crate::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-subagent-stop.sh")
            .copied()
            .unwrap_or("")
            .to_string()
    }

    /// Generate the PreToolUse git-guardrails hook content. Same model as
    /// `generate_role_context_hook` — reads from EMBEDDED_TEMPLATES.
    /// trace:TASK-27 | ai:claude
    pub(super) fn generate_git_guardrails_hook(&self) -> String {
        crate::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-git-guardrails.sh")
            .copied()
            .unwrap_or("")
            .to_string()
    }

    /// Advisor-code-guard PreToolUse hook (Write|Edit|MultiEdit) — soft-blocks
    /// the first code edit in an advisor session. trace:STORY-670 | ai:claude
    pub(super) fn generate_advisor_code_guard_hook(&self) -> String {
        crate::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-advisor-code-guard.sh")
            .copied()
            .unwrap_or("")
            .to_string()
    }
}

// trace:TASK-147 | ai:claude
#[cfg(test)]
mod tests {
    use super::*;

    // The git hook generators `include_str!` their master templates while the
    // rest of the codebase (scaffold drift checks, hook tests) reads the same
    // files through build.rs's `EMBEDDED_TEMPLATES`. Pin the two paths together
    // so scaffolded hooks stay byte-identical to the embedded template and a
    // second, drifting copy of a hook body cannot be reintroduced.
    #[test]
    fn hook_bodies_match_master_templates() {
        let scaffolder = Scaffolder::new(
            std::path::PathBuf::from("/nonexistent-scaffolder-root"),
            ScaffoldConfig::default(),
        );

        let cases = [
            (
                "hooks/aida-commit-msg",
                scaffolder.generate_commit_msg_hook(),
            ),
            (
                "hooks/aida-pre-commit.sh",
                scaffolder.generate_pre_commit_hook(),
            ),
            (
                "hooks/aida-post-commit.sh",
                scaffolder.generate_post_commit_hook(),
            ),
            (
                "hooks/aida-validate-commit.sh",
                scaffolder.generate_validate_commit_hook(),
            ),
            (
                "hooks/aida-track-commits.sh",
                scaffolder.generate_track_commits_hook(),
            ),
        ];

        for (key, generated) in cases {
            let embedded = crate::templates::EMBEDDED_TEMPLATES
                .get(key)
                .copied()
                .unwrap_or_else(|| panic!("master template {key} is not embedded"));
            assert!(!generated.is_empty(), "{key} generated an empty hook body");
            assert_eq!(
                generated, embedded,
                "{key} generated body drifted from its master template"
            );
        }
    }

    // The worktree-scope guard has to reach a project through `aida init`'s
    // hook scaffolding, not just exist as a binary subcommand — a downstream
    // repo never types `aida internal ...` by hand. Pin BOTH halves of its
    // contract into the scaffolded pre-commit body: it is invoked, and it is
    // invoked warn-only (`|| true`), so no future edit can quietly turn an
    // advisory isolation check into a commit blocker.
    // trace:TASK-1178 | ai:claude
    #[test]
    fn pre_commit_hook_scaffolds_the_warn_only_worktree_scope_guard() {
        let scaffolder = Scaffolder::new(
            std::path::PathBuf::from("/nonexistent-scaffolder-root"),
            ScaffoldConfig::default(),
        );
        let body = scaffolder.generate_pre_commit_hook();
        assert!(
            body.contains("internal worktree-scope-gate"),
            "scaffolded pre-commit hook must invoke the worktree-scope guard"
        );
        assert!(
            body.contains("internal worktree-scope-gate || true"),
            "the worktree-scope guard must be invoked warn-only (`|| true`), never as a gate"
        );
    }
}
