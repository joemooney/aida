use super::*;
use crate::statusline_cmd::{
    claude_statusline_block, install_claude_statusline, osc_terminal_title,
    STATUSLINE_SETUP_COMMAND,
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
