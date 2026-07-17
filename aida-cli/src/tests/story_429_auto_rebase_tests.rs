use super::*;

fn driver(
    no_human: Option<auto_complete::NoHumanMode>,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> RealPhaseDriver {
    RealPhaseDriver::new(
        std::env::temp_dir().join(format!("aida-story-429-{}", uuid::Uuid::now_v7())),
        "STORY-429".to_string(),
        None,
        true,
        no_human,
        AutonomyMode::Default,
        "test-run".to_string(),
        false,
        false,
        allow_stale_base,
        no_auto_rebase,
        auto_complete::LifecycleSkip::none(),
    )
}

#[test]
fn opt_out_preserves_story_281_refuse_and_park() {
    let driver = driver(Some(auto_complete::NoHumanMode::Both), false, true);
    assert_eq!(driver.should_auto_rebase_stale_base(false), Err("disabled"));
}

#[test]
fn allow_stale_base_preempts_auto_rebase() {
    let driver = driver(Some(auto_complete::NoHumanMode::Both), true, false);
    assert_eq!(
        driver.should_auto_rebase_stale_base(false),
        Err("allow-stale-base")
    );
}

#[test]
fn recursive_stale_base_hits_retry_limit() {
    let driver = driver(Some(auto_complete::NoHumanMode::Both), false, false);
    assert_eq!(
        driver.should_auto_rebase_stale_base(true),
        Err("retry-limit")
    );
}

#[test]
fn fully_headless_first_stale_base_attempts_auto_rebase() {
    let driver = driver(Some(auto_complete::NoHumanMode::Both), false, false);
    assert_eq!(driver.should_auto_rebase_stale_base(false), Ok(()));
}

#[test]
fn human_permitted_mode_does_not_auto_rebase() {
    let driver = driver(Some(auto_complete::NoHumanMode::ReviewerOnly), false, false);
    assert_eq!(
        driver.should_auto_rebase_stale_base(false),
        Err("not-fully-headless")
    );
}

#[cfg(unix)]
#[test]
fn clean_auto_rebase_proceeds_without_retrying_preflight() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let fake_aida = tmp.path().join("aida");
    std::fs::write(
        &fake_aida,
        "#!/bin/sh\n\
             if [ \"$1\" = pr ] && [ \"$2\" = rebase ] && [ \"$4\" = --no-smoke ]; then\n\
               exit 0\n\
             fi\n\
             echo unexpected aida args: \"$@\" >&2\n\
             exit 1\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_aida).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_aida, perms).unwrap();

    // BUG-409: make the fixture a fresh, fully-isolated git repo (the spec's
    // prescribed fix). The driver runs git/aida against `project_root`; an
    // un-init'd tempdir left any stray git op to fall through to whatever
    // working tree the process happened to inherit — the source of the
    // "untracked working tree files would be overwritten by merge" CI flake
    // on unrelated (docs-only) PRs. A real repo here contains every git
    // operation to this throwaway dir. trace:BUG-409 | ai:claude
    let g = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(args)
            .output()
            .expect("git spawn in fixture");
    };
    g(&["init", "-q", "-b", "main"]);
    g(&["config", "user.email", "t@t.t"]);
    g(&["config", "user.name", "t"]);
    g(&["commit", "-q", "--allow-empty", "-m", "init"]);

    let mut driver = driver(Some(auto_complete::NoHumanMode::Both), false, false);
    driver.aida_exe = fake_aida;
    driver.project_root = tmp.path().to_path_buf();
    let mut attempted = false;
    let action = driver.resolve_phase3_stale_overlap(
        250,
        &mut attempted,
        "stale cache should not be rechecked".to_string(),
    );

    assert_eq!(action, Phase3StaleOverlapAction::Proceed);
    assert!(attempted);
    assert_eq!(driver.auto_rebase_events.len(), 1);
    assert_eq!(driver.auto_rebase_events[0].outcome, "clean");
}
