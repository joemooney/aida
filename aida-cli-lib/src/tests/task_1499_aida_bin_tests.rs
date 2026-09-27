//! TASK-1499: Portable AIDA binary resolution across local installations.
//!
//! Acceptance criteria & test coverage:
//! 1. Precedence: explicit override (AIDA_BIN env var > ~/.aida/agents.toml aida_bin)
//!    > running executable (current executable with (deleted) stripped)
//!    > PATH (aida) > bare aida with PathUnverified fallback.
//! 2. Directory override:
//!    - Explicit request (AIDA_BUILD_PROFILE / aida_build_profile) wins; fails if absent.
//!    - Otherwise release, unless debug is newer by mtime (freshest-wins); only one present -> that one.
//! 3. Strict executable validation:
//!    - Non-executable or missing path errors immediately with exact format:
//!      "aida executable {path} (from {source}) is not executable; set AIDA_BIN to a built binary or run cargo build --release"
//! 4. Profile & checkout root detection + newer_alternate detection.
//! 5. Stripping of Linux ` (deleted)` suffix from running executable path.
//! 6. Diagnostics in render_agent_launch_noexec:
//!    - aida_executable: <path> (source: ..., profile: ..., checkout: ...)
//!    - aida_build_stale: <alt> is newer than <curr> (<age>); run cargo build --release or set AIDA_BUILD_PROFILE
//!    - aida_executable: aida (not found on PATH; child will fail to exec)

// trace:TASK-1499 | ai:antigravity

use super::*;
use crate::aida_bin::{
    detect_profile_and_checkout, ensure_executable, is_executable, resolve_aida_executable,
    AidaBinSource, BuildProfile, ResolveInputs,
};
use crate::test_env::EnvVarsGuard;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn create_dummy_file(path: &Path, executable: bool) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"#!/bin/sh\necho aida-test\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(if executable { 0o755 } else { 0o644 });
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn set_mtime(path: &Path, time: SystemTime) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(time).unwrap();
}

#[test]
fn test_is_executable_and_ensure_executable() {
    let temp = tempfile::tempdir().unwrap();
    let exec_file = temp.path().join("exec");
    let non_exec_file = temp.path().join("non_exec");
    create_dummy_file(&exec_file, true);
    create_dummy_file(&non_exec_file, false);

    assert!(is_executable(&exec_file));
    #[cfg(unix)]
    assert!(!is_executable(&non_exec_file));

    assert!(ensure_executable(&exec_file, AidaBinSource::OverrideEnv).is_ok());
    #[cfg(unix)]
    {
        let err = ensure_executable(&non_exec_file, AidaBinSource::OverrideConfig).unwrap_err();
        assert!(format!("{err:#}").contains("is not executable; set AIDA_BIN to a built binary or run cargo build --release"));
    }
}

#[test]
fn test_precedence_override_env_over_config_and_exe() {
    let temp = tempfile::tempdir().unwrap();
    let env_bin = temp.path().join("env_aida");
    let current_bin = temp.path().join("current_aida");
    let path_bin = temp.path().join("path_aida");
    create_dummy_file(&env_bin, true);
    create_dummy_file(&current_bin, true);
    create_dummy_file(&path_bin, true);

    let inputs = ResolveInputs {
        override_path: Some(env_bin.clone()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: None,
        current_exe: Some(current_bin),
        path_env: Some(temp.path().into()),
    };

    let resolved = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(resolved.source, AidaBinSource::OverrideEnv);
    assert_eq!(resolved.path, env_bin);
}

#[test]
fn test_precedence_config_over_exe_and_path() {
    let temp = tempfile::tempdir().unwrap();
    let config_bin = temp.path().join("config_aida");
    let current_bin = temp.path().join("current_aida");
    let path_bin = temp.path().join("path_aida");
    create_dummy_file(&config_bin, true);
    create_dummy_file(&current_bin, true);
    create_dummy_file(&path_bin, true);

    let inputs = ResolveInputs {
        override_path: Some(config_bin.clone()),
        override_source: Some(AidaBinSource::OverrideConfig),
        profile_request: None,
        current_exe: Some(current_bin),
        path_env: Some(temp.path().into()),
    };

    let resolved = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(resolved.source, AidaBinSource::OverrideConfig);
    assert_eq!(resolved.path, config_bin);
}

#[test]
fn test_precedence_exe_over_path() {
    let temp = tempfile::tempdir().unwrap();
    let current_bin = temp.path().join("current_aida");
    let path_dir = temp.path().join("bin");
    let path_bin = path_dir.join("aida");
    create_dummy_file(&current_bin, true);
    create_dummy_file(&path_bin, true);

    let inputs = ResolveInputs {
        override_path: None,
        override_source: None,
        profile_request: None,
        current_exe: Some(current_bin.clone()),
        path_env: Some(path_dir.into()),
    };

    let resolved = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(resolved.source, AidaBinSource::RunningExe);
    assert_eq!(resolved.path, current_bin);
}

#[test]
fn test_precedence_path_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path_dir = temp.path().join("bin");
    let path_bin = path_dir.join("aida");
    create_dummy_file(&path_bin, true);

    let inputs = ResolveInputs {
        override_path: None,
        override_source: None,
        profile_request: None,
        current_exe: None,
        path_env: Some(path_dir.into()),
    };

    let resolved = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(resolved.source, AidaBinSource::Path);
    assert_eq!(resolved.path, path_bin);
}

#[test]
fn test_precedence_path_unverified_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let empty_dir = temp.path().join("empty");
    std::fs::create_dir_all(&empty_dir).unwrap();

    let inputs = ResolveInputs {
        override_path: None,
        override_source: None,
        profile_request: None,
        current_exe: None,
        path_env: Some(empty_dir.into()),
    };

    let resolved = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(resolved.source, AidaBinSource::PathUnverified);
    assert_eq!(resolved.path, PathBuf::from("aida"));
    assert_eq!(resolved.profile, None);
    assert_eq!(resolved.checkout_root, None);
    assert_eq!(resolved.newer_alternate, None);
}

#[test]
fn test_directory_override_explicit_profile_wins() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let rel_bin = repo.join("target/release/aida");
    let dbg_bin = repo.join("target/debug/aida");
    create_dummy_file(&rel_bin, true);
    create_dummy_file(&dbg_bin, true);

    // Make debug newer
    let now = SystemTime::now();
    set_mtime(&rel_bin, now - Duration::from_secs(100));
    set_mtime(&dbg_bin, now);

    // Explicit release request overrides the fact that debug is newer
    let inputs_rel = ResolveInputs {
        override_path: Some(repo.to_path_buf()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: Some(BuildProfile::Release),
        current_exe: None,
        path_env: None,
    };
    let resolved_rel = resolve_aida_executable(&inputs_rel).unwrap();
    assert_eq!(resolved_rel.path, rel_bin);
    assert_eq!(resolved_rel.profile, Some(BuildProfile::Release));
    assert_eq!(resolved_rel.checkout_root, Some(repo.to_path_buf()));
    // Alternate build (debug) is newer!
    assert!(resolved_rel.newer_alternate.is_some());
    let (alt_prof, alt_path) = resolved_rel.newer_alternate.unwrap();
    assert_eq!(alt_prof, BuildProfile::Debug);
    assert_eq!(alt_path, dbg_bin);

    // Explicit debug request picks debug
    let inputs_dbg = ResolveInputs {
        override_path: Some(repo.to_path_buf()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: Some(BuildProfile::Debug),
        current_exe: None,
        path_env: None,
    };
    let resolved_dbg = resolve_aida_executable(&inputs_dbg).unwrap();
    assert_eq!(resolved_dbg.path, dbg_bin);
    assert_eq!(resolved_dbg.profile, Some(BuildProfile::Debug));
    assert_eq!(resolved_dbg.newer_alternate, None);

    // If requested profile is absent, fails with exact suggestion
    std::fs::remove_file(&dbg_bin).unwrap();
    let err_dbg = resolve_aida_executable(&inputs_dbg).unwrap_err();
    let msg_dbg = format!("{err_dbg:#}");
    assert!(
        msg_dbg.contains("no debug build at"),
        "expected debug error, got: {msg_dbg}"
    );
    assert!(
        msg_dbg.contains("Run `cargo build` first."),
        "expected advice, got: {msg_dbg}"
    );

    std::fs::remove_file(&rel_bin).unwrap();
    create_dummy_file(&dbg_bin, true);
    let err_rel = resolve_aida_executable(&inputs_rel).unwrap_err();
    let msg_rel = format!("{err_rel:#}");
    assert!(
        msg_rel.contains("no release build at"),
        "expected release error, got: {msg_rel}"
    );
    assert!(
        msg_rel.contains("Run `cargo build --release` first."),
        "expected advice, got: {msg_rel}"
    );
}

#[test]
fn test_directory_override_freshest_wins() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let rel_bin = repo.join("target/release/aida");
    let dbg_bin = repo.join("target/debug/aida");
    create_dummy_file(&rel_bin, true);
    create_dummy_file(&dbg_bin, true);

    let inputs = ResolveInputs {
        override_path: Some(repo.to_path_buf()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: None,
        current_exe: None,
        path_env: None,
    };

    // Case 1: debug is newer -> debug wins
    let now = SystemTime::now();
    set_mtime(&rel_bin, now - Duration::from_secs(60));
    set_mtime(&dbg_bin, now);
    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.profile, Some(BuildProfile::Debug));
    assert_eq!(res.path, dbg_bin);

    // Case 2: release is newer -> release wins
    set_mtime(&dbg_bin, now - Duration::from_secs(60));
    set_mtime(&rel_bin, now);
    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.profile, Some(BuildProfile::Release));
    assert_eq!(res.path, rel_bin);

    // Case 3: only release exists -> release wins
    std::fs::remove_file(&dbg_bin).unwrap();
    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.profile, Some(BuildProfile::Release));
    assert_eq!(res.path, rel_bin);

    // Case 4: only debug exists -> debug wins
    std::fs::remove_file(&rel_bin).unwrap();
    create_dummy_file(&dbg_bin, true);
    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.profile, Some(BuildProfile::Debug));
    assert_eq!(res.path, dbg_bin);
}

#[test]
fn test_directory_override_missing_binary_fails() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    std::fs::create_dir_all(repo.join("target")).unwrap();

    let inputs = ResolveInputs {
        override_path: Some(repo.to_path_buf()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: None,
        current_exe: None,
        path_env: None,
    };

    let err = resolve_aida_executable(&inputs).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no aida binary found at"),
        "expected missing binary message, got: {msg}"
    );
}

#[test]
fn test_override_non_executable_fails_with_exact_message() {
    let temp = tempfile::tempdir().unwrap();
    let non_exec = temp.path().join("aida_non_exec");
    create_dummy_file(&non_exec, false);

    let inputs_non_exec = ResolveInputs {
        override_path: Some(non_exec.clone()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: None,
        current_exe: None,
        path_env: None,
    };

    let err = resolve_aida_executable(&inputs_non_exec).unwrap_err();
    let msg = format!("{err:#}");
    let expected = format!(
        "aida executable {} (from override-env) is not executable; set AIDA_BIN to a built binary or run cargo build --release",
        non_exec.display()
    );
    assert_eq!(msg, expected);

    // Missing path
    let missing_path = temp.path().join("does_not_exist");
    let inputs_missing = ResolveInputs {
        override_path: Some(missing_path.clone()),
        override_source: Some(AidaBinSource::OverrideConfig),
        profile_request: None,
        current_exe: None,
        path_env: None,
    };
    let err_missing = resolve_aida_executable(&inputs_missing).unwrap_err();
    let msg_missing = format!("{err_missing:#}");
    let expected_missing = format!(
        "aida executable {} (from override-config) does not exist",
        missing_path.display()
    );
    assert_eq!(msg_missing, expected_missing);
}

#[test]
fn test_profile_and_checkout_detection_and_newer_alternate() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let rel_bin = repo.join("target/release/aida");
    let dbg_bin = repo.join("target/debug/aida");
    create_dummy_file(&rel_bin, true);
    create_dummy_file(&dbg_bin, true);

    let (prof_rel, root_rel) = detect_profile_and_checkout(&rel_bin);
    assert_eq!(prof_rel, Some(BuildProfile::Release));
    assert_eq!(root_rel, Some(repo.to_path_buf()));

    let (prof_dbg, root_dbg) = detect_profile_and_checkout(&dbg_bin);
    assert_eq!(prof_dbg, Some(BuildProfile::Debug));
    assert_eq!(root_dbg, Some(repo.to_path_buf()));

    // Arbitrary file outside target/<profile>/aida
    let other_file = temp.path().join("other_bin");
    create_dummy_file(&other_file, true);
    let (prof_other, root_other) = detect_profile_and_checkout(&other_file);
    assert_eq!(prof_other, None);
    assert_eq!(root_other, None);

    // Resolving as file when alternate is newer
    let now = SystemTime::now();
    set_mtime(&rel_bin, now - Duration::from_secs(50));
    set_mtime(&dbg_bin, now);

    let inputs = ResolveInputs {
        override_path: Some(rel_bin.clone()),
        override_source: Some(AidaBinSource::OverrideEnv),
        profile_request: None,
        current_exe: None,
        path_env: None,
    };
    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.profile, Some(BuildProfile::Release));
    assert_eq!(res.checkout_root, Some(repo.to_path_buf()));
    assert_eq!(
        res.newer_alternate,
        Some((BuildProfile::Debug, dbg_bin.clone()))
    );

    // Resolving as file when alternate is older
    set_mtime(&dbg_bin, now - Duration::from_secs(50));
    set_mtime(&rel_bin, now);
    let res2 = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res2.newer_alternate, None);
}

#[test]
fn test_deleted_suffix_stripping() {
    let temp = tempfile::tempdir().unwrap();
    let live = temp.path().join("aida");
    create_dummy_file(&live, true);
    let deleted = PathBuf::from(format!("{} (deleted)", live.display()));

    let inputs = ResolveInputs {
        override_path: None,
        override_source: None,
        profile_request: None,
        current_exe: Some(deleted),
        path_env: None,
    };

    let res = resolve_aida_executable(&inputs).unwrap();
    assert_eq!(res.source, AidaBinSource::RunningExe);
    assert_eq!(res.path, live);
}

#[test]
fn test_resolve_inputs_from_process_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let dot_aida = home.join(".aida");
    std::fs::create_dir_all(&dot_aida).unwrap();

    let env_bin = temp.path().join("bin_from_env");
    let config_bin = temp.path().join("bin_from_config");
    create_dummy_file(&env_bin, true);
    create_dummy_file(&config_bin, true);

    std::fs::write(
        dot_aida.join("agents.toml"),
        format!(
            "[agents]\naida_bin = \"{}\"\naida_build_profile = \"debug\"\n",
            config_bin.display()
        ),
    )
    .unwrap();

    // 1. Both AIDA_BIN and config are present: AIDA_BIN wins
    {
        let _env = EnvVarsGuard::apply(&[
            ("AIDA_BIN", Some(env_bin.to_str().unwrap())),
            ("AIDA_BUILD_PROFILE", Some("release")),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        let inputs = ResolveInputs::from_process();
        assert_eq!(inputs.override_source, Some(AidaBinSource::OverrideEnv));
        assert_eq!(inputs.override_path, Some(env_bin.clone()));
        assert_eq!(inputs.profile_request, Some(BuildProfile::Release));
    }

    // 2. AIDA_BIN is absent, config is present: config wins
    {
        let _env = EnvVarsGuard::apply(&[
            ("AIDA_BIN", None),
            ("AIDA_BUILD_PROFILE", None),
            ("HOME", Some(home.to_str().unwrap())),
        ]);
        let inputs = ResolveInputs::from_process();
        assert_eq!(inputs.override_source, Some(AidaBinSource::OverrideConfig));
        assert_eq!(inputs.override_path, Some(config_bin.clone()));
        assert_eq!(inputs.profile_request, Some(BuildProfile::Debug));
    }

    // 3. Both are absent
    {
        let empty_home = temp.path().join("empty_home");
        std::fs::create_dir_all(&empty_home).unwrap();
        let _env = EnvVarsGuard::apply(&[
            ("AIDA_BIN", None),
            ("AIDA_BUILD_PROFILE", None),
            ("HOME", Some(empty_home.to_str().unwrap())),
        ]);
        let inputs = ResolveInputs::from_process();
        assert_eq!(inputs.override_source, None);
        assert_eq!(inputs.override_path, None);
        assert_eq!(inputs.profile_request, None);
    }
}

#[test]
fn test_noexec_preview_includes_aida_executable_and_stale_warning() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let rel_bin = repo.join("target/release/aida");
    let dbg_bin = repo.join("target/debug/aida");
    create_dummy_file(&rel_bin, true);
    create_dummy_file(&dbg_bin, true);

    let now = SystemTime::now();
    set_mtime(&rel_bin, now - Duration::from_secs(120));
    set_mtime(&dbg_bin, now);

    let _env = EnvVarsGuard::apply(&[
        ("AIDA_BIN", Some(rel_bin.to_str().unwrap())),
        ("AIDA_BUILD_PROFILE", None),
    ]);

    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: repo.to_path_buf(),
        launch_cwd: repo.to_path_buf(),
        role: Some("implementer".into()),
        role_instance: RoleInstanceKind::Driver,
        current_spec: Some("TASK-1499".into()),
        name: "test-preview".to_string(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    let prompt = AgentPromptOptions::new(None, false);
    let preview = render_agent_launch_noexec(
        std::path::Path::new("/bin/echo"),
        &config,
        &plan,
        &prompt,
        &["work TASK-1499".to_string()],
        false,
        false,
    )
    .unwrap();

    assert!(
        preview.contains(&format!(
            "aida_executable: {} (source: override-env, profile: release, checkout: {})",
            rel_bin.display(),
            repo.display()
        )),
        "preview: {preview}"
    );
    assert!(
        preview.contains("aida_build_stale: debug is newer than release"),
        "preview: {preview}"
    );
    assert!(
        preview.contains("run cargo build --release or set AIDA_BUILD_PROFILE"),
        "preview: {preview}"
    );
}

#[test]
fn test_noexec_preview_fails_on_broken_override() {
    let temp = tempfile::tempdir().unwrap();
    let broken_bin = temp.path().join("broken_aida");
    create_dummy_file(&broken_bin, false); // Not executable

    let _env = EnvVarsGuard::apply(&[
        ("AIDA_BIN", Some(broken_bin.to_str().unwrap())),
    ]);

    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: temp.path().to_path_buf(),
        launch_cwd: temp.path().to_path_buf(),
        role: Some("implementer".into()),
        role_instance: RoleInstanceKind::Driver,
        current_spec: Some("TASK-1499".into()),
        name: "test-preview".to_string(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    let prompt = AgentPromptOptions::new(None, false);
    let err = render_agent_launch_noexec(
        std::path::Path::new("/bin/echo"),
        &config,
        &plan,
        &prompt,
        &["work TASK-1499".to_string()],
        false,
        false,
    )
    .unwrap_err();

    let msg = format!("{err:#}");
    assert!(
        msg.contains("is not executable; set AIDA_BIN to a built binary or run cargo build --release"),
        "expected strict validation error, got: {msg}"
    );
}
