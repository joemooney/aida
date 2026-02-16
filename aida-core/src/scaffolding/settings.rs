use super::*;

impl Scaffolder {
    /// Generate Claude Code settings.json content
    pub(super) fn generate_claude_settings_json(&self) -> String {
        let mut hooks = Vec::new();

        if self.config.include_validate_commit_hook {
            hooks.push(r#"    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": ".claude/hooks/aida-validate-commit.sh",
            "timeout": 10
          }
        ]
      }
    ]"#);
        }

        if self.config.include_track_commits_hook {
            hooks.push(r#"    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": ".claude/hooks/aida-track-commits.sh",
            "timeout": 15
          }
        ]
      }
    ]"#);
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
