//! BUG-1627: follow-ups from BUG-1624's security re-review of the
//! `.aida/session-env.sh` filter. A NUL byte in an allowlisted value is
//! dropped instead of panicking `set_var`, and `AIDA_AGENT_TYPE` is taken
//! only when it names a known agent type.
//!
//! Tests use scratch dirs only and restore every env var they may touch;
//! nothing reads or writes `~/.aida` or a live store.
// trace:BUG-1627 | ai:claude

use super::*;

/// A stand-in for the running binary: an absolute, existing file.
fn fake_running_exe(dir: &std::path::Path) -> std::path::PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("aida");
    std::fs::write(&exe, "").unwrap();
    exe
}

const SESSION_ENV_KEYS: [&str; 3] = ["CARGO_TARGET_DIR", "AIDA_AGENT_TYPE", "AIDA_BIN"];

/// A value carrying a NUL is dropped from the eval'd lines; the other
/// names still come through.
#[test]
fn bug_1627_nul_value_is_dropped_from_eval_lines() {
    let tree = tempfile::TempDir::new().unwrap();
    let exe = fake_running_exe(tree.path());
    let body = "export CARGO_TARGET_DIR='/w/tar\0get'\nexport AIDA_AGENT_TYPE='cla\0ude'\n\
                export AIDA_BIN='/w/bin/aida'\n";
    let lines = session_env_eval_lines(body, &exe);
    assert!(!lines.contains('\0'), "{lines:?}");
    assert!(!lines.contains("CARGO_TARGET_DIR"), "{lines}");
    assert!(!lines.contains("AIDA_AGENT_TYPE"), "{lines}");
    assert!(lines.contains("export AIDA_BIN="), "{lines}");

    // A later clean value for a dropped name is still taken.
    let pairs = trusted_session_env(
        "export CARGO_TARGET_DIR='/w/a\0b'\nexport CARGO_TARGET_DIR='/w/target'\n",
        &exe,
    );
    assert_eq!(
        pairs,
        vec![("CARGO_TARGET_DIR".to_string(), "/w/target".to_string())]
    );
}

/// `session start --launch` / `queue work`: a NUL value used to panic in
/// `std::env::set_var`. Now it is skipped and the process env is untouched.
#[test]
fn bug_1627_apply_session_env_to_process_skips_nul_values() {
    let _restore = crate::test_env::EnvVarsGuard::snapshot(&SESSION_ENV_KEYS);
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("AIDA_AGENT_TYPE");
    }
    let applied = std::panic::catch_unwind(|| {
        apply_session_env_to_process(
            "export CARGO_TARGET_DIR='/w/tar\0get'\nexport AIDA_AGENT_TYPE='codex\0'\n",
        )
    })
    .expect("a NUL value must not panic set_var");
    assert!(
        !applied
            .iter()
            .any(|n| n == "CARGO_TARGET_DIR" || n == "AIDA_AGENT_TYPE"),
        "{applied:?}"
    );
    assert!(std::env::var_os("CARGO_TARGET_DIR").is_none());
    assert!(std::env::var_os("AIDA_AGENT_TYPE").is_none());
}

/// Only a known agent type survives, in its canonical spelling; anything
/// else (which would switch on the advisor code-gate agent carve-out and add
/// a mailbox identity) is dropped.
#[test]
fn bug_1627_agent_type_accepts_only_known_types() {
    let tree = tempfile::TempDir::new().unwrap();
    let exe = fake_running_exe(tree.path());
    for (raw, want) in [
        ("claude", Some("claude")),
        ("Claude-Code", Some("claude")),
        ("codex", Some("codex")),
        ("gemini", Some("antigravity")),
        ("antigravity", Some("antigravity")),
        ("shell", Some("shell")),
        ("web", Some("web")),
        ("other", None),
        ("advisor", None),
        ("root", None),
        ("", None),
        ("   ", None),
        ("claude $(touch pwned)", None),
        ("cl'aude", None),
    ] {
        let body = format!("export AIDA_AGENT_TYPE={}\n", shell_single_quote(raw));
        let got: Vec<(String, String)> = trusted_session_env(&body, &exe);
        let want: Vec<(String, String)> = want
            .map(|w| vec![("AIDA_AGENT_TYPE".to_string(), w.to_string())])
            .unwrap_or_default();
        assert_eq!(got, want, "raw {raw:?}");
        let lines = session_env_eval_lines(&body, &exe);
        match want.first() {
            Some((_, w)) => assert_eq!(lines, format!("export AIDA_AGENT_TYPE='{w}'\n")),
            None => assert!(lines.is_empty(), "raw {raw:?}: {lines}"),
        }
    }
    // The generated file (`render_session_env_file`) round-trips.
    let rendered = render_session_env_file(std::path::Path::new("/w/target"), Some("codex"), None);
    assert_eq!(
        trusted_session_env(&rendered, &exe),
        vec![
            ("CARGO_TARGET_DIR".to_string(), "/w/target".to_string()),
            ("AIDA_AGENT_TYPE".to_string(), "codex".to_string()),
        ]
    );
}

/// The in-process path applies the same agent-type filter.
#[test]
fn bug_1627_apply_session_env_to_process_drops_unknown_agent_type() {
    let _restore = crate::test_env::EnvVarsGuard::snapshot(&SESSION_ENV_KEYS);
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var("AIDA_AGENT_TYPE");
    }
    let applied = apply_session_env_to_process("export AIDA_AGENT_TYPE='not-an-agent'\n");
    assert!(
        !applied.iter().any(|n| n == "AIDA_AGENT_TYPE"),
        "{applied:?}"
    );
    assert!(std::env::var_os("AIDA_AGENT_TYPE").is_none());

    let applied = apply_session_env_to_process("export AIDA_AGENT_TYPE='Codex'\n");
    assert!(
        applied.iter().any(|n| n == "AIDA_AGENT_TYPE"),
        "{applied:?}"
    );
    assert_eq!(std::env::var("AIDA_AGENT_TYPE").as_deref(), Ok("codex"));
}

/// `queue work --no-launch` points at the filtered `worktree enter`, never a
/// raw `source` of the branch-controllable file.
#[test]
fn bug_1627_no_launch_hint_does_not_source_session_env() {
    let src = include_str!("../queue_cmd.rs");
    let needle = concat!("\"source .aida/", "session-env.sh\"");
    assert!(!src.contains(needle), "queue_cmd.rs still prints {needle}");
    assert!(src.contains("format!(\"aida worktree enter {}\", lease.scope)"));
}
