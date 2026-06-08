use super::*;

/// Claude Code's statusbar command. `aida statusline` produces the AIDA
/// one-liner (project · role · @SPEC · queue · cache); the `printf`
/// fallback runs when `aida` is not on PATH or the cwd is outside an
/// AIDA project, so the user always gets *something* useful.
///
/// `--color=always` is set deliberately: Claude Code's statusLine
/// consumer runs the command via shell pipe (no TTY), so `aida
/// statusline`'s default `--color=auto` would emit plain text. Claude
/// Code DOES render ANSI escape codes from the consumer's output, so
/// forcing color here gives users the intended colored statusline.
/// trace:FR-1-013, FR-1-041 | ai:claude
const STATUSLINE_COMMAND: &str =
    "aida statusline --color=always 2>/dev/null || printf '%s' \"$(pwd)\"";

impl Scaffolder {
    /// Generate Claude Code settings.json content. Hook commands use
    /// `$CLAUDE_PROJECT_DIR/...` so they resolve regardless of CWD when
    /// Claude Code invokes them — relative `.claude/hooks/...` paths
    /// silently failed when the Bash tool was used from any cwd other
    /// than the project root (e.g., inside .aida-store/).
    /// trace:EPIC-1-001 | ai:claude
    pub(super) fn generate_claude_settings_json(&self) -> String {
        let mut hooks = Vec::new();

        // PreToolUse: validate-commit + git-guardrails ride together (both
        // match Bash). When neither is enabled the block is omitted.
        // trace:TASK-20 | ai:claude
        let mut pre_entries: Vec<&str> = Vec::new();
        if self.config.include_validate_commit_hook {
            pre_entries.push(
                r#"          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-validate-commit.sh",
            "timeout": 10
          }"#,
            );
        }
        if self.config.include_git_guardrails_hook {
            pre_entries.push(
                r#"          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-git-guardrails.sh",
            "timeout": 5
          }"#,
            );
        }
        if !pre_entries.is_empty() {
            let entries = pre_entries.join(",\n");
            // Owned String — Vec<&str> can't borrow it directly, so we leak
            // through a Box::leak-free path by collecting into String later.
            hooks.push(format!(
                r#"    "PreToolUse": [
      {{
        "matcher": "Bash",
        "hooks": [
{entries}
        ]
      }}
    ]"#,
                entries = entries
            ));
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
    ]"#
                .to_string(),
            );
        }

        // SessionStart: role-context hook surfaces (role:<name>) state when
        // a Claude Code session starts. trace:TASK-20 | ai:claude
        if self.config.include_role_context_hook {
            hooks.push(
                r#"    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-role-context.sh",
            "timeout": 5
          }
        ]
      }
    ]"#
                .to_string(),
            );
        }

        // SubagentStart/Stop: passive-observe harness worktree leases.
        // trace:TASK-702 | ai:claude
        hooks.push(
            r#"    "SubagentStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-subagent-start.sh",
            "timeout": 5
          }
        ]
      }
    ]"#
            .to_string(),
        );
        hooks.push(
            r#"    "SubagentStop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-subagent-stop.sh",
            "timeout": 5
          }
        ]
      }
    ]"#
            .to_string(),
        );

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
        serde_json::from_str(s)
            .unwrap_or_else(|e| panic!("settings.json output is not valid JSON: {}\n\n{}", e, s))
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
        let config = ScaffoldConfig {
            include_validate_commit_hook: false,
            include_track_commits_hook: false,
            ..Default::default()
        };

        let json = build(config);
        let v = parse_json(&json);

        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(
            v["statusLine"]["command"].as_str().unwrap(),
            STATUSLINE_COMMAND
        );
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

    /// Default config now emits the SessionStart block pointing at
    /// aida-role-context.sh. trace:TASK-20 | ai:claude
    #[test]
    fn session_start_role_context_hook_present_by_default() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);
        let entries = v["hooks"]["SessionStart"]
            .as_array()
            .expect("SessionStart should be an array");
        assert!(!entries.is_empty(), "SessionStart should not be empty");
        let cmd = entries[0]["hooks"][0]["command"]
            .as_str()
            .expect("command should be a string");
        assert!(
            cmd.ends_with("/aida-role-context.sh"),
            "expected role-context hook, got {}",
            cmd
        );
        assert_eq!(entries[0]["hooks"][0]["timeout"], 5);
    }

    /// Disabling the role-context flag drops the SessionStart block
    /// entirely (no empty array left behind). trace:TASK-20 | ai:claude
    #[test]
    fn session_start_omitted_when_role_context_hook_disabled() {
        let config = ScaffoldConfig {
            include_role_context_hook: false,
            ..Default::default()
        };
        let json = build(config);
        let v = parse_json(&json);
        assert!(
            v["hooks"]["SessionStart"].is_null(),
            "SessionStart should be absent when the flag is off"
        );
    }

    /// SubagentStart/Stop hooks capture harness worktree leases.
    /// trace:TASK-702 | ai:claude
    #[test]
    fn subagent_hooks_present_by_default() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);

        let start = v["hooks"]["SubagentStart"][0]["hooks"][0]["command"]
            .as_str()
            .expect("SubagentStart command should be a string");
        assert!(
            start.ends_with("/aida-subagent-start.sh"),
            "expected subagent start hook, got {}",
            start
        );
        assert_eq!(v["hooks"]["SubagentStart"][0]["hooks"][0]["timeout"], 5);

        let stop = v["hooks"]["SubagentStop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("SubagentStop command should be a string");
        assert!(
            stop.ends_with("/aida-subagent-stop.sh"),
            "expected subagent stop hook, got {}",
            stop
        );
        assert_eq!(v["hooks"]["SubagentStop"][0]["hooks"][0]["timeout"], 5);
    }

    /// PreToolUse merges validate-commit + git-guardrails into one
    /// matcher block. trace:TASK-20 | ai:claude
    #[test]
    fn pre_tool_use_combines_validate_and_guardrails() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);
        let entries = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("PreToolUse hooks array");
        assert_eq!(entries.len(), 2);
        let cmds: Vec<&str> = entries
            .iter()
            .map(|e| e["command"].as_str().unwrap())
            .collect();
        assert!(cmds.iter().any(|c| c.ends_with("/aida-validate-commit.sh")));
        assert!(cmds.iter().any(|c| c.ends_with("/aida-git-guardrails.sh")));
    }
}
