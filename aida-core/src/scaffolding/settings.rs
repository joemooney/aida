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

/// The `[contained]` config section `aida init` scaffolds, from its template
/// master. trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
/// trace:BUG-2392 | ai:codex
pub const CONTAINED_CONFIG_TEMPLATE: &str = include_str!("../../templates/contained.toml");

/// The allow entries a contained reviewer needs to run `aida` at all. Every
/// other reviewer command (`git log`, `gh pr diff`) runs under the contained
/// sandbox; `aida` is what the permission layer denied in an adopted repo.
/// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
pub const REVIEWER_ALLOW_ENTRIES: &[&str] = &["Bash(aida)", "Bash(aida *)"];

/// `[contained] allowed_hosts` from the template master.
/// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
/// trace:BUG-2392 | ai:codex
pub fn template_contained_allowed_hosts() -> Vec<String> {
    toml::from_str::<toml::Value>(CONTAINED_CONFIG_TEMPLATE)
        .ok()
        .and_then(|v| {
            v.get("contained")?
                .get("allowed_hosts")?
                .as_array()
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|h| h.as_str().map(str::to_string))
        .collect()
}

/// The AIDA repository's own development entries: its dev binaries and the
/// operator's on/off aliases. Emitted only when the scaffolded repository IS
/// AIDA ([`is_aida_repository`]) — in an adopted repo `./target/debug/aida`
/// is an agent-writable path, so allowing it would run anything written
/// there unprompted. trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
pub const AIDA_DEV_ALLOW_ENTRIES: &[&str] = &[
    "Bash(aida-on)",
    "Bash(aida-off)",
    "Bash(./target/debug/aida)",
    "Bash(./target/debug/aida *)",
    "Bash(./target/release/aida)",
    "Bash(./target/release/aida *)",
];

/// Whether `project_root` is the AIDA workspace itself: its root
/// `Cargo.toml` lists an `aida-core` member whose package is `aida-core`.
/// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
pub fn is_aida_repository(project_root: &std::path::Path) -> bool {
    let read = |path: std::path::PathBuf| -> Option<toml::Value> {
        toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let lists_member = read(project_root.join("Cargo.toml"))
        .and_then(|v| v.get("workspace")?.get("members")?.as_array().cloned())
        .is_some_and(|members| members.iter().any(|m| m.as_str() == Some("aida-core")));
    lists_member
        && read(project_root.join("aida-core").join("Cargo.toml"))
            .and_then(|v| Some(v.get("package")?.get("name")?.as_str()? == "aida-core"))
            .unwrap_or(false)
}

/// The `permissions.allow` entries the scaffold emits for `project_root`:
/// the reviewer entries, plus the dev entries only in the AIDA repository.
/// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
pub fn scaffold_permissions_allow(project_root: &std::path::Path) -> Vec<&'static str> {
    let mut allow = REVIEWER_ALLOW_ENTRIES.to_vec();
    if is_aida_repository(project_root) {
        allow.extend_from_slice(AIDA_DEV_ALLOW_ENTRIES);
    }
    allow
}

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
        // trace:STORY-1854 | ai:claude
        if self.config.include_cargo_timeout_hook {
            pre_entries.push(
                r#"          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-cargo-timeout-guard.sh",
            "timeout": 5
          }"#,
            );
        }
        // PreToolUse holds one or more matcher objects: the Bash matcher
        // (validate-commit + git-guardrails) and, separately, the
        // Write|Edit|MultiEdit advisor-code-guard. trace:STORY-670 | ai:claude
        let mut pre_matchers: Vec<String> = Vec::new();
        if !pre_entries.is_empty() {
            let entries = pre_entries.join(",\n");
            pre_matchers.push(format!(
                r#"      {{
        "matcher": "Bash",
        "hooks": [
{entries}
        ]
      }}"#,
                entries = entries
            ));
        }
        if self.config.include_advisor_code_guard_hook {
            pre_matchers.push(
                r#"      {
        "matcher": "Write|Edit|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-advisor-code-guard.sh",
            "timeout": 5
          }
        ]
      }"#
                .to_string(),
            );
        }
        if !pre_matchers.is_empty() {
            hooks.push(format!(
                "    \"PreToolUse\": [\n{}\n    ]",
                pre_matchers.join(",\n")
            ));
        }

        // PostToolUse holds the Bash-matched track-commits hook and,
        // separately, the pending-approval clear hook (no matcher — every
        // tool, not just Bash, so the marker never outlives the turn it was
        // raised on). trace:TASK-1461 | ai:claude
        let mut post_matchers: Vec<String> = Vec::new();
        if self.config.include_track_commits_hook {
            post_matchers.push(
                r#"      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-track-commits.sh",
            "timeout": 15
          }
        ]
      }"#
                .to_string(),
            );
        }
        if self.config.include_pending_approval_hooks {
            post_matchers.push(
                r#"      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-clear-pending-approval.sh",
            "timeout": 5
          }
        ]
      }"#
                .to_string(),
            );
        }
        if !post_matchers.is_empty() {
            hooks.push(format!(
                "    \"PostToolUse\": [\n{}\n    ]",
                post_matchers.join(",\n")
            ));
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

        // UserPromptSubmit: clears the pending-approval marker (the other
        // half of the pair below on PostToolUse) once a human sends the next
        // prompt. trace:TASK-1461 | ai:claude
        if self.config.include_pending_approval_hooks {
            hooks.push(
                r#"    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-clear-pending-approval.sh",
            "timeout": 5
          }
        ]
      }
    ]"#
                .to_string(),
            );
        }

        // Notification(permission_prompt): records the pending-approval
        // marker Claude Code is waiting on the operator's approval — ground
        // truth for `aida ps` / `aida awaiting`, not a heuristic.
        // trace:TASK-1454 trace:TASK-1461 | ai:claude
        if self.config.include_pending_approval_hooks {
            hooks.push(
                r#"    "Notification": [
      {
        "matcher": "permission_prompt",
        "hooks": [
          {
            "type": "command",
            "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/aida-notification.sh",
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

        // BUG-014: without these a contained reviewer's `aida` calls are
        // denied and it exits with no verdict.
        // trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
        let allow = scaffold_permissions_allow(&self.project_root)
            .iter()
            .map(|entry| format!("      {}", json_string_literal(entry)))
            .collect::<Vec<_>>()
            .join(",\n");
        let permissions_block =
            format!("  \"permissions\": {{\n    \"allow\": [\n{allow}\n    ]\n  }}");

        format!(
            "{{\n{},\n{},\n{}\n}}",
            hooks_block, status_line_block, permissions_block
        )
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

    // trace:BUG-2392 | ai:codex
    #[test]
    fn contained_template_includes_both_vendors_and_development_hosts() {
        let template: toml::Value = toml::from_str(CONTAINED_CONFIG_TEMPLATE).unwrap();
        let hosts = template["contained"]["allowed_hosts"].as_array().unwrap();
        let expected = [
            "github.com",
            "api.github.com",
            "objects.githubusercontent.com",
            "codeload.github.com",
            "api.anthropic.com",
            "statsig.anthropic.com",
            "api.openai.com",
            "chatgpt.com",
            "auth.openai.com",
            "static.crates.io",
            "index.crates.io",
            "crates.io",
            "registry.npmjs.org",
        ];
        assert_eq!(hosts.len(), expected.len());
        assert_eq!(template_contained_allowed_hosts(), expected);
    }

    /// BUG-014: an adopted repo gets only the reviewer entries; the AIDA
    /// repository also gets its dev entries, and together they are exactly
    /// the template master's list. The contained template parses.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
    #[test]
    fn bug_014_scaffold_allows_only_reviewer_entries_outside_aida() {
        let allow = |json: &str| -> Vec<String> {
            parse_json(json)["permissions"]["allow"]
                .as_array()
                .expect("permissions.allow emitted")
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        };
        assert_eq!(
            allow(&build(ScaffoldConfig::default())),
            REVIEWER_ALLOW_ENTRIES
        );

        let aida = tempfile::tempdir().unwrap();
        std::fs::write(
            aida.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"aida-core\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(aida.path().join("aida-core")).unwrap();
        std::fs::write(
            aida.path().join("aida-core/Cargo.toml"),
            "[package]\nname = \"aida-core\"\n",
        )
        .unwrap();
        let dev = allow(
            &Scaffolder::new(aida.path().to_path_buf(), ScaffoldConfig::default())
                .generate_claude_settings_json(),
        );
        let template: Vec<String> = serde_json::from_str::<serde_json::Value>(
            crate::templates::EMBEDDED_TEMPLATES
                .get("settings.json")
                .unwrap(),
        )
        .unwrap()["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
        assert_eq!(dev, template);

        let hosts = template_contained_allowed_hosts();
        assert!(hosts.iter().any(|h| h == "api.github.com"), "{hosts:?}");
        assert!(hosts.iter().any(|h| h == "api.anthropic.com"), "{hosts:?}");
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
        assert_eq!(entries.len(), 3);
        let cmds: Vec<&str> = entries
            .iter()
            .map(|e| e["command"].as_str().unwrap())
            .collect();
        assert!(cmds.iter().any(|c| c.ends_with("/aida-validate-commit.sh")));
        assert!(cmds.iter().any(|c| c.ends_with("/aida-git-guardrails.sh")));
        // trace:STORY-1854 | ai:claude
        assert!(cmds
            .iter()
            .any(|c| c.ends_with("/aida-cargo-timeout-guard.sh")));
    }

    /// The cargo time-budget hint is opt-out like the other Bash hooks.
    // trace:STORY-1854 | ai:claude
    #[test]
    fn pre_tool_use_omits_cargo_timeout_hook_when_disabled() {
        let cfg = ScaffoldConfig {
            include_cargo_timeout_hook: false,
            ..ScaffoldConfig::default()
        };
        let v = parse_json(&build(cfg));
        let entries = v["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("PreToolUse hooks array");
        assert_eq!(entries.len(), 2);
        assert!(!v.to_string().contains("aida-cargo-timeout-guard.sh"));
    }

    /// The advisor-code-guard rides as its own PreToolUse matcher
    /// (Write|Edit|MultiEdit), default-on. trace:STORY-670 | ai:claude
    #[test]
    fn pre_tool_use_includes_advisor_code_guard_by_default() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);
        let matchers = v["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse array");
        let guard = matchers
            .iter()
            .find(|m| m["matcher"] == "Write|Edit|MultiEdit")
            .expect("advisor-code-guard matcher present");
        let cmd = guard["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("/aida-advisor-code-guard.sh"));
    }

    /// `--no-hooks`-style opt-out drops the advisor-code-guard matcher.
    /// trace:STORY-670 | ai:claude
    #[test]
    fn pre_tool_use_omits_advisor_code_guard_when_disabled() {
        let cfg = ScaffoldConfig {
            include_advisor_code_guard_hook: false,
            ..ScaffoldConfig::default()
        };
        let json = build(cfg);
        let v = parse_json(&json);
        let matchers = v["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse array");
        assert!(matchers
            .iter()
            .all(|m| m["matcher"] != "Write|Edit|MultiEdit"));
    }

    /// Default config wires the Notification(permission_prompt) marker hook,
    /// the UserPromptSubmit clear, and a no-matcher PostToolUse clear entry
    /// alongside the Bash-matched track-commits entry — matching this repo's
    /// own template settings.json exactly.
    // trace:TASK-1461 | ai:claude
    #[test]
    fn pending_approval_hooks_wired_by_default() {
        let json = build(ScaffoldConfig::default());
        let v = parse_json(&json);

        let notif = &v["hooks"]["Notification"][0];
        assert_eq!(notif["matcher"], "permission_prompt");
        assert!(notif["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("/aida-notification.sh"));

        let ups = &v["hooks"]["UserPromptSubmit"][0];
        assert!(ups.get("matcher").is_none());
        assert!(ups["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("/aida-clear-pending-approval.sh"));

        let post_matchers = v["hooks"]["PostToolUse"]
            .as_array()
            .expect("PostToolUse array");
        assert_eq!(
            post_matchers.len(),
            2,
            "Bash track-commits + no-matcher clear"
        );
        let bash_entry = post_matchers
            .iter()
            .find(|m| m["matcher"] == "Bash")
            .expect("Bash-matched track-commits entry present");
        assert!(bash_entry["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("/aida-track-commits.sh"));
        let clear_entry = post_matchers
            .iter()
            .find(|m| m["matcher"].is_null())
            .expect("no-matcher clear entry present");
        assert!(clear_entry["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("/aida-clear-pending-approval.sh"));
    }

    /// Disabling the flag drops Notification, UserPromptSubmit, and the
    /// no-matcher PostToolUse entry, leaving the Bash track-commits entry
    /// untouched.
    // trace:TASK-1461 | ai:claude
    #[test]
    fn pending_approval_hooks_omitted_when_disabled() {
        let cfg = ScaffoldConfig {
            include_pending_approval_hooks: false,
            ..ScaffoldConfig::default()
        };
        let json = build(cfg);
        let v = parse_json(&json);

        assert!(v["hooks"]["Notification"].is_null());
        assert!(v["hooks"]["UserPromptSubmit"].is_null());

        let post_matchers = v["hooks"]["PostToolUse"]
            .as_array()
            .expect("PostToolUse array still present for track-commits");
        assert_eq!(post_matchers.len(), 1);
        assert_eq!(post_matchers[0]["matcher"], "Bash");
    }
}
