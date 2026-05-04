use super::*;

impl Scaffolder {
    /// Generate Claude Code settings.json content. Hook commands use
    /// `$CLAUDE_PROJECT_DIR/...` so they resolve regardless of CWD when
    /// Claude Code invokes them — relative `.claude/hooks/...` paths
    /// silently failed when the Bash tool was used from any cwd other
    /// than the project root (e.g., inside .aida-store/).
    /// trace:EPIC-1-001 | ai:claude
    pub(super) fn generate_claude_settings_json(&self) -> String {
        let mut hooks = Vec::new();

        if self.config.include_validate_commit_hook {
            hooks.push(
                r#"    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-validate-commit.sh",
            "timeout": 10
          }
        ]
      }
    ]"#,
            );
        }

        if self.config.include_track_commits_hook {
            hooks.push(
                r#"    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-track-commits.sh",
            "timeout": 15
          }
        ]
      }
    ]"#,
            );
        }

        if hooks.is_empty() {
            return r#"{
  "hooks": {}
}"#
            .to_string();
        }

        format!(
            r#"{{
  "hooks": {{
{}
  }}
}}"#,
            hooks.join(",\n")
        )
    }
}
