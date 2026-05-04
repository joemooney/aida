use super::*;

/// Claude Code's statusbar command. `aida statusline` produces the AIDA
/// one-liner (project · role · queue · cache); the `printf` fallback runs
/// when `aida` is not on PATH or the cwd is outside an AIDA project, so
/// the user always gets *something* useful.
/// trace:FR-1-013 | ai:claude
const STATUSLINE_COMMAND: &str =
    "aida statusline 2>/dev/null || printf '%s' \"$(pwd)\"";

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

        // Status bar one-liner. Always emitted — it costs nothing when AIDA
        // is unavailable (the printf fallback prints cwd) and surfaces the
        // active role / project the moment the user opens Claude Code.
        // trace:FR-1-013 | ai:claude
        let status_line_block = format!(
            r#"  "statusLine": {{
    "type": "command",
    "command": {}
  }}"#,
            json_string_literal(STATUSLINE_COMMAND)
        );

        let hooks_block = if hooks.is_empty() {
            r#"  "hooks": {}"#.to_string()
        } else {
            format!("  \"hooks\": {{\n{}\n  }}", hooks.join(",\n"))
        };

        format!("{{\n{},\n{}\n}}", hooks_block, status_line_block)
    }
}

/// Render a Rust string as a JSON string literal (with surrounding quotes).
/// Avoids hand-rolling escape rules for embedded `"` and `\`.
fn json_string_literal(s: &str) -> String {
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c if (c as u32) < 0x20 => format!("\\u{:04x}", c as u32).chars().collect(),
            c => vec![c],
        })
        .collect();
    format!("\"{}\"", escaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaffolding::{ScaffoldConfig, Scaffolder};
    use std::path::PathBuf;

    fn build(config: ScaffoldConfig) -> String {
        let scaffolder = Scaffolder::new(PathBuf::from("/tmp/aida-test-scaffold"), config);
        scaffolder.generate_claude_settings_json()
    }

    fn parse_json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap_or_else(|e| {
            panic!("settings.json output is not valid JSON: {}\n\n{}", e, s)
        })
    }

    /// Default config produces both hooks AND statusLine.
    /// trace:FR-1-013 | ai:claude
    #[test]
    fn default_config_emits_status_line_and_hooks() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);

        let cmd = v["statusLine"]["command"]
            .as_str()
            .expect("statusLine.command missing");
        assert_eq!(cmd, STATUSLINE_COMMAND);
        assert_eq!(v["statusLine"]["type"], "command");

        // Hooks block still populated.
        assert!(v["hooks"]["PreToolUse"].is_array());
        assert!(v["hooks"]["PostToolUse"].is_array());
    }

    /// Even when no hooks are configured the statusLine is still emitted
    /// and the JSON parses cleanly. trace:FR-1-013 | ai:claude
    #[test]
    fn no_hooks_still_emits_status_line() {
        let mut config = ScaffoldConfig::default();
        config.include_validate_commit_hook = false;
        config.include_track_commits_hook = false;

        let json = build(config);
        let v = parse_json(&json);

        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["command"].as_str().unwrap(), STATUSLINE_COMMAND);
        // hooks still present as an empty object — Claude Code accepts both
        // shapes, but a present-but-empty object signals "we considered it".
        assert!(v["hooks"].is_object());
    }

    /// Sanity: the fallback in STATUSLINE_COMMAND survives JSON escaping
    /// (the embedded `"$(pwd)"` shouldn't break parsing).
    /// trace:FR-1-013 | ai:claude
    #[test]
    fn status_line_command_round_trips_through_json() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);
        let cmd = v["statusLine"]["command"].as_str().unwrap();
        assert!(cmd.contains("aida statusline"));
        assert!(cmd.contains("$(pwd)"));
    }
}
