use crate::statusline_cmd::{
    antigravity_statusline_fragment, claude_statusline_block, codex_statusline_setup_text,
    install_antigravity_statusline, install_claude_statusline, osc_terminal_title,
    print_statusline_setup_text_for_test, STATUSLINE_SETUP_COMMAND,
};

/// The Claude Code statusLine block uses the same command string the
/// scaffolder writes (so init-scaffolded and setup-installed config
/// agree) and contains no bashisms — Claude Code runs it under
/// /bin/sh (dash).
#[test]
fn claude_block_command_is_posix_and_canonical() {
    let block = claude_statusline_block();
    let cmd = block["command"]
        .as_str()
        .expect("command should be a string");
    assert_eq!(cmd, STATUSLINE_SETUP_COMMAND);
    assert_eq!(block["type"], "command");
    // POSIX printf fallback + 2>/dev/null redirect; no bash-only `[[`,
    // `&>`, or `function` keyword.
    assert!(cmd.contains("aida statusline --color=always"));
    assert!(cmd.contains("printf"));
    assert!(!cmd.contains("[["));
    assert!(!cmd.contains("&>"));
}

/// Installing into a fresh project creates settings.json with only the
/// statusLine key, and reports creation.
#[test]
fn install_creates_settings_when_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir.path().join(".claude").join("settings.json");

    let created = install_claude_statusline(&settings).expect("install should succeed");
    assert!(created, "should report file creation");
    assert!(settings.exists());

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["statusLine"]["type"], "command");
    assert_eq!(v["statusLine"]["command"], STATUSLINE_SETUP_COMMAND);
}

/// Installing into an existing settings.json MERGES the statusLine key
/// and preserves every pre-existing key (hooks, custom fields, etc.).
#[test]
fn install_merges_and_preserves_existing_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks": {"PreToolUse": []}, "custom": "keep-me"}"#,
    )
    .unwrap();

    let created = install_claude_statusline(&settings).expect("install should succeed");
    assert!(!created, "should report a merge, not a creation");

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    // Pre-existing keys survive.
    assert_eq!(v["custom"], "keep-me");
    assert!(v["hooks"]["PreToolUse"].is_array());
    // statusLine added.
    assert_eq!(v["statusLine"]["command"], STATUSLINE_SETUP_COMMAND);
}

/// A corrupt (non-JSON) settings.json is reported, not silently
/// clobbered — the user's hand-edited file is never overwritten blind.
#[test]
fn install_refuses_invalid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = claude.join("settings.json");
    std::fs::write(&settings, "{ not json").unwrap();

    let err = install_claude_statusline(&settings).expect_err("invalid JSON should error");
    assert!(err.to_string().contains("valid JSON"));
    // The original bytes are untouched.
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ not json");
}

// trace:TASK-1199 | ai:codex
#[test]
fn antigravity_fragment_uses_claude_command_and_stacks_by_default() {
    let fragment = antigravity_statusline_fragment(true);

    assert_eq!(fragment["statusLine"]["type"], "command");
    assert_eq!(fragment["statusLine"]["command"], STATUSLINE_SETUP_COMMAND);
    assert_eq!(fragment["statusLine"]["stack_with_default"], true);
    assert_eq!(fragment["title"]["type"], "command");
    assert_eq!(fragment["title"]["command"], "aida statusline title");
}

// trace:TASK-1199 | ai:codex
#[test]
fn antigravity_fragment_pretty_json_golden() {
    let fragment = antigravity_statusline_fragment(true);
    let pretty = serde_json::to_string_pretty(&fragment).unwrap();

    assert_eq!(
        pretty,
        r#"{
  "statusLine": {
    "command": "aida statusline --color=always 2>/dev/null || printf '%s' \"$(pwd)\"",
    "stack_with_default": true,
    "type": "command"
  },
  "title": {
    "command": "aida statusline title",
    "type": "command"
  }
}"#
    );
}

// trace:TASK-1199 | ai:codex
#[test]
fn antigravity_fragment_replace_default_emits_false() {
    let fragment = antigravity_statusline_fragment(false);

    assert_eq!(fragment["statusLine"]["stack_with_default"], false);
}

// trace:TASK-1199 | ai:codex
#[test]
fn antigravity_install_merges_preserves_keys_and_writes_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"theme": "solarized", "mcpServers": {"aida": {"command": "aida"}}}"#,
    )
    .unwrap();

    let (created, backup) =
        install_antigravity_statusline(&settings, true).expect("install should succeed");
    assert!(!created, "should merge existing settings");
    let backup = backup.expect("existing settings should be backed up");
    assert!(
        backup.exists(),
        "backup should exist at {}",
        backup.display()
    );
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        r#"{"theme": "solarized", "mcpServers": {"aida": {"command": "aida"}}}"#
    );

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["theme"], "solarized");
    assert_eq!(v["mcpServers"]["aida"]["command"], "aida");
    assert_eq!(v["statusLine"]["command"], STATUSLINE_SETUP_COMMAND);
    assert_eq!(v["statusLine"]["stack_with_default"], true);
    assert_eq!(v["title"]["command"], "aida statusline title");
}

// trace:TASK-1199 | ai:codex
#[test]
fn antigravity_install_refuses_invalid_json_without_backup_or_overwrite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = dir
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, "{ not json").unwrap();

    let err =
        install_antigravity_statusline(&settings, true).expect_err("invalid JSON should error");
    assert!(err.to_string().contains("valid JSON"));
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ not json");
    let backups: Vec<_> = std::fs::read_dir(settings.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains("aida-bak"))
        .collect();
    assert!(
        backups.is_empty(),
        "invalid JSON must not be backed up/written"
    );
}

// trace:TASK-1199 | ai:codex
#[test]
fn setup_all_mentions_claude_codex_and_antigravity() {
    let out = print_statusline_setup_text_for_test("all", true);

    assert!(out.contains("Claude Code"));
    assert!(out.contains("Codex CLI"));
    assert!(out.contains("Antigravity CLI"));
    assert!(out.contains("\"stack_with_default\": true"));
}

// trace:TASK-1188 | ai:codex — Codex now writes its own terminal title, so
// setup guidance must disable it and steer full AIDA status to tmux.
#[test]
fn codex_setup_disables_terminal_title_and_recommends_tmux_status_right() {
    let out = codex_statusline_setup_text();

    assert!(out.contains("[tui]"));
    assert!(out.contains("status_line = [\"model-with-reasoning\""));
    assert!(out.contains("terminal_title = null"));
    assert!(out.contains("Codex releases overwrite shell prompt title hooks"));
    assert!(out.contains("set -g status-right '#(aida statusline --color=never)'"));
    assert!(out.contains("set -g status-interval 15"));
    assert!(out.contains("aida statusbar --plain"));
    assert!(out.contains("command-backed `[tui] status_line` item"));
    assert!(!out.contains("PROMPT_COMMAND"));
    assert!(!out.contains("precmd()"));
}

// trace:TASK-896 — `--title` parity surface for clients (e.g. Codex CLI)
// whose footer cannot run `aida statusline` as a command.

/// `osc_terminal_title` wraps the line in `ESC ] 2 ; <text> BEL` so the
/// AIDA segment lands in the terminal title bar / tmux window name.
#[test]
fn osc_title_wraps_in_set_window_title_escape() {
    let out = osc_terminal_title("aida \u{3b1}\u{3b9}\u{3b4}\u{3b1} role:advisor q:4 inbox:43");
    assert!(out.starts_with("\x1b]2;"), "must open with OSC 2: {out:?}");
    assert!(out.ends_with('\x07'), "must terminate with BEL: {out:?}");
    // The payload survives intact between the markers.
    assert!(out.contains("role:advisor q:4 inbox:43"));
    // No trailing newline — a prompt hook updates only the title.
    assert!(!out.ends_with('\n'));
}

/// Control chars (newlines, embedded ESC/BEL) are stripped so a crafted
/// project name can't break out of the OSC string or corrupt the terminal.
#[test]
fn osc_title_strips_control_chars() {
    let out = osc_terminal_title("safe\nname\x07\x1b]0;evil\x07tail");
    // Exactly one opening OSC and one closing BEL — no smuggled escapes.
    assert_eq!(
        out.matches('\x07').count(),
        1,
        "only the closing BEL: {out:?}"
    );
    assert_eq!(
        out.matches('\x1b').count(),
        1,
        "only the opening ESC: {out:?}"
    );
    assert!(!out.contains('\n'));
    // Printable text is preserved (concatenated, control chars removed).
    assert!(out.contains("safename"));
    assert!(out.contains("0;eviltail"));
}
