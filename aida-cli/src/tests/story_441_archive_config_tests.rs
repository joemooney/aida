use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

// trace:STORY-441 | ai:claude
#[test]
fn read_archive_config_absent_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_archive_auto_after_days(tmp.path()), None);
}

// trace:STORY-441 | ai:claude
#[test]
fn read_archive_config_returns_configured_days() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[archive]\nauto_after_days = 30\n");
    assert_eq!(read_archive_auto_after_days(tmp.path()), Some(30));
}

/// Clamps below 7 to 7 with a stderr warning (warning is fire-and-
// forget; we just verify the return value). trace:STORY-441 | ai:claude
#[test]
fn read_archive_config_clamps_below_seven_days() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[archive]\nauto_after_days = 1\n");
    assert_eq!(read_archive_auto_after_days(tmp.path()), Some(7));
}

// trace:STORY-441 | ai:claude
#[test]
fn read_archive_config_at_seven_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[archive]\nauto_after_days = 7\n");
    assert_eq!(read_archive_auto_after_days(tmp.path()), Some(7));
}

// trace:STORY-441 | ai:claude
#[test]
fn read_archive_config_missing_section_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[store.sync]\nauto_push = \"manual\"\n");
    assert_eq!(read_archive_auto_after_days(tmp.path()), None);
}

/// STORY-717: `[focus] out_of_scope` config read — default + each mode.
// trace:STORY-717 | ai:claude
#[test]
fn read_focus_policy_defaults_to_warn_when_absent_or_unset() {
    let tmp = tempfile::tempdir().unwrap();
    // No config file at all → default warn.
    assert_eq!(
        read_focus_out_of_scope_policy(tmp.path()),
        focus::OutOfScopePolicy::Warn
    );
    // Config present but no [focus] block → default warn.
    write_config(tmp.path(), "[archive]\nauto_after_days = 30\n");
    assert_eq!(
        read_focus_out_of_scope_policy(tmp.path()),
        focus::OutOfScopePolicy::Warn
    );
}

// trace:STORY-717 | ai:claude
#[test]
fn read_focus_policy_reads_each_mode() {
    for (body, expected) in [
        (
            "[focus]\nout_of_scope = \"off\"\n",
            focus::OutOfScopePolicy::Off,
        ),
        (
            "[focus]\nout_of_scope = \"warn\"\n",
            focus::OutOfScopePolicy::Warn,
        ),
        (
            "[focus]\nout_of_scope = \"block\"\n",
            focus::OutOfScopePolicy::Block,
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write_config(tmp.path(), body);
        assert_eq!(read_focus_out_of_scope_policy(tmp.path()), expected);
    }
}

/// Mirrors `auto_bump_env_flag_respects_opt_out` shape — one test that
/// saves/restores the env var so parallel tests don't race on it.
// trace:STORY-441 | ai:claude
#[test]
fn auto_archive_enabled_env_flag_respects_opt_out() {
    let saved = std::env::var("AIDA_AUTO_ARCHIVE").ok();

    // Unset → on (default).
    std::env::remove_var("AIDA_AUTO_ARCHIVE");
    assert!(auto_archive_enabled());

    for off in &["false", "0", "no", "off", "FALSE", "Off"] {
        std::env::set_var("AIDA_AUTO_ARCHIVE", off);
        assert!(
            !auto_archive_enabled(),
            "AIDA_AUTO_ARCHIVE={off:?} should disable"
        );
    }
    for on in &["true", "1", "", "yes", "anything-else"] {
        std::env::set_var("AIDA_AUTO_ARCHIVE", on);
        assert!(
            auto_archive_enabled(),
            "AIDA_AUTO_ARCHIVE={on:?} should stay on"
        );
    }

    match saved {
        Some(v) => std::env::set_var("AIDA_AUTO_ARCHIVE", v),
        None => std::env::remove_var("AIDA_AUTO_ARCHIVE"),
    }
}
