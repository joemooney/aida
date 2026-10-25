//! STORY-1662 PR1: the standalone implementer launch — its selection, outcome
//! contract and exit codes, child env scrub, stop-contract refusals,
//! concurrency cap, and authorship record.
// trace:STORY-1662 | ai:claude

// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use super::*;
use crate::punt::{HoldSignal, PuntSignal};
use std::collections::BTreeMap;

const LEASE: &str = "0123456789ab";

// ── Selection ─────────────────────────────────────────────────────────────

// trace:STORY-1662.AC11 | ai:claude
#[test]
fn is_standalone_implementer_truth_table() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (crate::orchestrator::PHASE_ENV, None),
        (crate::orchestrator::AUTO_COMPLETE_ENV, None),
    ]);
    for (role, scope_is_spec, no_human, want) in [
        ("implementer", true, true, true),
        ("Implementer", true, true, true),
        ("implementer", false, true, false),
        ("implementer", true, false, false),
        ("reviewer", true, true, false),
        ("advisor", true, true, false),
    ] {
        assert_eq!(
            is_standalone_implementer(role, scope_is_spec, no_human),
            want,
            "role={role} spec={scope_is_spec} no_human={no_human}"
        );
    }
}

// trace:STORY-1662.AC11 | ai:claude
#[test]
fn a_drain_child_is_never_a_standalone_implementer() {
    for key in [
        crate::orchestrator::PHASE_ENV,
        crate::orchestrator::AUTO_COMPLETE_ENV,
    ] {
        let other = if key == crate::orchestrator::PHASE_ENV {
            crate::orchestrator::AUTO_COMPLETE_ENV
        } else {
            crate::orchestrator::PHASE_ENV
        };
        let _env = crate::test_env::EnvVarsGuard::apply(&[(key, Some("1")), (other, None)]);
        assert!(
            !is_standalone_implementer("implementer", true, true),
            "{key} marks a drain child"
        );
    }
}

// ── Outcome contract and exit codes ───────────────────────────────────────

fn punt() -> PuntSignal {
    PuntSignal {
        spec: "STORY-9".into(),
        category: aida_core::models::PuntCategory::DesignFork,
        detail: "two valid\nschemas".into(),
        lean: None,
    }
}

fn hold() -> HoldSignal {
    HoldSignal {
        spec: "STORY-9".into(),
        branch: "claude/story-9".into(),
        reason: None,
    }
}

// trace:STORY-1662.AC8 | ai:claude
// trace:STORY-1662.AC12 | ai:claude
#[test]
fn outcome_classifier_gives_one_line_and_exit_code_per_outcome() {
    let pr = || Some((42, Some("abc123".to_string())));
    let none = || None;
    let cases: Vec<(Outcome, Option<i32>, &str, i32)> = vec![
        (
            classify(None, None, pr),
            Some(0),
            "implementer: pr-opened pr=42 head=abc123",
            0,
        ),
        (
            classify(Some(punt()), None, none),
            Some(0),
            "implementer: punted reason=[design-fork] two valid schemas",
            PUNTED_EXIT_CODE,
        ),
        (
            classify(None, Some(hold()), none),
            Some(0),
            "implementer: held branch=claude/story-9",
            HELD_EXIT_CODE,
        ),
        (
            classify(None, None, none),
            Some(0),
            "implementer: no-pr",
            NO_PR_EXIT_CODE,
        ),
        // The vendor's nonzero code passes through only for a PR-less run.
        (classify(None, None, none), Some(7), "implementer: no-pr", 7),
        (
            classify(None, None, pr),
            Some(1),
            "implementer: pr-opened pr=42 head=abc123",
            0,
        ),
        (
            classify(Some(punt()), None, none),
            Some(1),
            "implementer: punted reason=[design-fork] two valid schemas",
            PUNTED_EXIT_CODE,
        ),
        // Killed by a signal: no code, so the no-PR code.
        (
            classify(None, None, none),
            None,
            "implementer: no-pr",
            NO_PR_EXIT_CODE,
        ),
    ];
    for (outcome, vendor, line, code) in cases {
        let (got_line, got_code) = report(&outcome, vendor);
        assert_eq!(got_line, line, "{outcome:?} vendor={vendor:?}");
        assert!(!got_line.contains('\n'), "one stdout line: {got_line:?}");
        assert_eq!(got_code, code, "{outcome:?} vendor={vendor:?}");
    }
}

// trace:STORY-1662.AC12 | ai:claude
#[test]
fn a_pure_git_change_zero_is_not_an_opened_pr() {
    let change = |number| crate::OpenPrInfo {
        number,
        title: String::new(),
        url: String::new(),
        head_branch: Some("claude/story-9".into()),
    };
    assert!(!is_real_pr(&change(0)), "pure-git's branch-as-change");
    assert!(is_real_pr(&change(42)));
}

// trace:STORY-1662.AC12 | ai:claude
#[test]
fn only_a_pr_on_the_lease_branch_is_this_runs_outcome() {
    let pr = |number, head: &str| crate::OpenPrInfo {
        number,
        title: String::new(),
        url: String::new(),
        head_branch: Some(head.into()),
    };
    assert!(is_run_pr(&pr(42, "claude/story-9"), "claude/story-9"));
    assert!(
        !is_run_pr(&pr(43, "claude/story-9-slice-2"), "claude/story-9"),
        "a sibling PR of the same spec is not this run's PR"
    );
    assert!(!is_run_pr(&pr(0, "claude/story-9"), "claude/story-9"));
    let headless = crate::OpenPrInfo {
        head_branch: None,
        ..pr(44, "x")
    };
    assert!(!is_run_pr(&headless, "claude/story-9"));
}

// trace:STORY-1662.AC12 | ai:claude
#[test]
fn a_signal_wins_over_the_pr_lookup_which_is_then_skipped() {
    let called = std::cell::Cell::new(false);
    let outcome = classify(Some(punt()), Some(hold()), || {
        called.set(true);
        Some((1, None))
    });
    assert!(matches!(outcome, Outcome::Punted { .. }));
    assert!(!called.get(), "a punted run needs no forge lookup");
    let outcome = classify(None, Some(hold()), || {
        called.set(true);
        Some((1, None))
    });
    assert!(matches!(outcome, Outcome::Held { .. }));
    assert!(!called.get());
}

// trace:STORY-1662.AC8 | ai:claude
#[test]
fn outcome_exit_codes_are_distinct_and_mapped_by_exit_code_for_error() {
    // TASK-2154: distinctness comes from the ONE exit-code table, not a
    // hand-kept list here.
    let table = crate::orchestrator_leases::EXIT_CODES;
    for (i, (code, meaning)) in table.iter().enumerate() {
        assert!(
            !table[..i].iter().any(|(c, _)| c == code),
            "exit code {code} ({meaning}) has two rows; merge them into one"
        );
    }
    for code in [3, 124] {
        assert!(
            table.iter().any(|(c, _)| *c == code),
            "exit code {code} is missing from the table"
        );
    }
    for code in [
        NO_PR_EXIT_CODE,
        PUNTED_EXIT_CODE,
        HELD_EXIT_CODE,
        CAP_EXIT_CODE,
    ] {
        assert_eq!(
            table.iter().filter(|(c, _)| *c == code).count(),
            1,
            "exit code {code} must have exactly one row"
        );
        let err = anyhow::Error::new(StandaloneImplementerExit {
            code,
            message: "x".into(),
            vendor_exit: false,
        })
        .context("queue work failed");
        assert_eq!(crate::exit_code_for_error(&err), code);
    }
}

/// TASK-2215: `exit_code_for_error` reads the table. A named code with no
/// row exits 1; only a vendor pass-through skips the table.
// trace:STORY-1662.AC8 | ai:claude
#[test]
fn exit_code_for_error_reads_the_exit_code_table() {
    let unlisted = 77;
    assert!(!crate::orchestrator_leases::EXIT_CODES
        .iter()
        .any(|(c, _)| *c == unlisted));
    let exit = |vendor_exit| {
        crate::exit_code_for_error(&anyhow::Error::new(StandaloneImplementerExit {
            code: unlisted,
            message: "x".into(),
            vendor_exit,
        }))
    };
    assert_eq!(exit(false), 1, "an unregistered named code must not escape");
    assert_eq!(exit(true), unlisted, "the vendor's own code passes through");
    let rebase = anyhow::Error::new(crate::pr_rebase::RebaseFailureExit {
        code: unlisted,
        message: "x".into(),
    });
    assert_eq!(crate::exit_code_for_error(&rebase), 1);
    for (code, _) in crate::orchestrator_leases::EXIT_CODES {
        assert_eq!(crate::orchestrator_leases::named_exit_code(*code), *code);
    }
}

// ── Child env (signoff C4) ────────────────────────────────────────────────

/// The env a child sees: `parent` overlaid with `cmd`'s explicit sets and
/// removals.
fn effective_env(
    cmd: &std::process::Command,
    parent: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = parent.clone();
    for (key, value) in cmd.get_envs() {
        let key = key.to_string_lossy().into_owned();
        match value {
            Some(v) => {
                env.insert(key, v.to_string_lossy().into_owned());
            }
            None => {
                env.remove(&key);
            }
        }
    }
    env
}

/// An advisor launcher's env: its role and grant, every launcher-only
/// variable, and an unrelated variable that must pass through.
fn advisor_env() -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = LAUNCHER_ONLY_ENV
        .iter()
        .map(|k| (k.to_string(), format!("launcher-{k}")))
        .collect();
    env.insert("AIDA_SESSION_ROLE".into(), "advisor".into());
    env.insert(
        crate::seat_authority::GRANT_ENV.into(),
        "advisor-grant".into(),
    );
    env.insert("KEEP_ME".into(), "1".into());
    env
}

/// The command the standalone launch builds for its child: the shared
/// headless seat env, then the standalone overlay.
fn child_command(grant_env: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("true");
    crate::session::apply_headless_child_seat_env(&mut cmd, "implementer", grant_env);
    apply_standalone_child_env(
        &mut cmd,
        LEASE,
        std::path::Path::new("/main/.aida/punt-signals/STORY-9.json"),
        std::path::Path::new("/main/.aida/pr-holds/STORY-9.json"),
    );
    cmd
}

// trace:STORY-1662.AC9 | ai:claude
#[test]
fn child_env_from_an_advisor_env_has_the_implementer_seat_and_no_launcher_variable() {
    for grant in ["child-grant-id", ""] {
        let env = effective_env(&child_command(grant), &advisor_env());
        assert_eq!(env["AIDA_SESSION_ROLE"], "implementer");
        assert_eq!(env[crate::seat_authority::GRANT_ENV], grant);
        assert_eq!(env["KEEP_ME"], "1");
        for key in LAUNCHER_ONLY_ENV {
            let launcher = format!("launcher-{key}");
            assert_ne!(
                env.get(*key),
                Some(&launcher),
                "{key} leaked from the launcher into the child"
            );
        }
        // Only the launch's own values come back after the scrub.
        assert_eq!(env[MARKER_ENV], LEASE);
        assert_eq!(
            env[crate::punt::SIGNAL_FILE_ENV],
            "/main/.aida/punt-signals/STORY-9.json"
        );
        assert_eq!(
            env[crate::punt::HOLD_SIGNAL_FILE_ENV],
            "/main/.aida/pr-holds/STORY-9.json"
        );
        let reset = [
            MARKER_ENV,
            crate::punt::SIGNAL_FILE_ENV,
            crate::punt::HOLD_SIGNAL_FILE_ENV,
        ];
        for key in LAUNCHER_ONLY_ENV.iter().filter(|k| !reset.contains(k)) {
            assert!(!env.contains_key(*key), "{key} must be absent: {env:?}");
        }
    }
}

// trace:STORY-1662.AC9 | ai:claude
#[test]
fn launcher_only_list_names_the_drain_context_the_grant_file_and_the_marker() {
    for key in [
        "AIDA_SESSION_PURPOSE",
        "AIDA_AUTO_COMPLETE",
        "AIDA_AUTO_COMPLETE_PHASE",
        "AIDA_AUTO_COMPLETE_TOKEN",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_NO_HUMAN_MODE",
        "AIDA_SESSION_GRANT_FILE",
        MARKER_ENV,
    ] {
        assert!(LAUNCHER_ONLY_ENV.contains(&key), "{key} is missing");
    }
}

// trace:STORY-1662.AC9 | ai:claude
#[test]
fn every_headless_child_drops_the_marker_so_a_grandchild_is_unbound() {
    let mut cmd = std::process::Command::new("true");
    crate::session::scrub_launcher_session_env(&mut cmd);
    let mut parent = BTreeMap::new();
    parent.insert(MARKER_ENV.to_string(), LEASE.to_string());
    assert!(!effective_env(&cmd, &parent).contains_key(MARKER_ENV));
}

// ── Advisor code guard and commit gate ────────────────────────────────────

/// Run the advisor code guard hook on a `.rs` Edit inside a fresh git repo,
/// with exactly `env` (no ambient `aida`, so solo mode reads as off).
#[cfg(unix)]
fn guard_exit(env: &BTreeMap<String, String>) -> i32 {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let out = crate::test_git::git_command()
        .args(["init", "-q"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let file = repo.join("src").join("lib.rs");
    std::fs::write(&file, "").unwrap();
    let hook = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aida-core/templates/hooks/aida-advisor-code-guard.sh");
    let input = format!(
        r#"{{"session_id":"story-1662","tool_name":"Edit","tool_input":{{"file_path":"{}"}}}}"#,
        file.display()
    );
    let mut child = std::process::Command::new("bash")
        .arg(&hook)
        .env_clear()
        .envs(env)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", tmp.path())
        .env("CLAUDE_PROJECT_DIR", &repo)
        .current_dir(&repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait().unwrap().code().unwrap_or(-1)
}

// trace:STORY-1662.AC1 | ai:claude
#[cfg(unix)]
#[test]
fn advisor_code_guard_passes_the_child_env_and_blocks_the_parent_env() {
    let mut parent = advisor_env();
    // An advisor shell is not inside a drain.
    parent.remove(crate::orchestrator::AUTO_COMPLETE_ENV);
    assert_eq!(guard_exit(&parent), 2, "the advisor's own edit is blocked");
    let child = effective_env(&child_command(""), &parent);
    assert_eq!(guard_exit(&child), 0, "the implementer child edits freely");
}

// trace:STORY-1662.AC1 | ai:claude
#[test]
fn commit_gate_passes_the_child_env_and_refuses_the_parent_env() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let out = crate::test_git::git_command()
        .args(["init", "-q"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    std::fs::write(repo.join("src").join("lib.rs"), "fn main() {}\n").unwrap();
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(&repo)
        .args(["add", "src/lib.rs"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    // The gate keys on the validated grant's seat: the advisor launcher
    // holds an advisor grant, its baseline child the empty grant.
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_TYPE", None),
        ("CLAUDE_CODE_CHILD_SESSION", None),
        (crate::advisor_code_gate::ALLOW_ENV, None),
        (crate::orchestrator::AUTO_COMPLETE_ENV, None),
        ("AIDA_SESSION_ROLE", None),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    struct RestoreAmbient(Option<crate::test_ambient::Ambient>);
    impl Drop for RestoreAmbient {
        fn drop(&mut self) {
            crate::test_ambient::replace(self.0.take());
        }
    }
    let _ambient = RestoreAmbient(crate::test_ambient::replace(Some(
        crate::test_ambient::Ambient {
            project_root: repo.clone(),
            stdin_is_terminal: false,
            stdout_is_terminal: false,
            stderr_is_terminal: false,
        },
    )));
    let advisor_grant = crate::seat_authority::test_support::mint_grant_for(
        &repo,
        &crate::current_user_id(None),
        "advisor",
        &[],
    );
    let mut parent = advisor_env();
    parent.remove(crate::orchestrator::AUTO_COMPLETE_ENV);
    parent.insert(crate::seat_authority::GRANT_ENV.into(), advisor_grant);
    let child = effective_env(&child_command(""), &parent);
    let mut gate = |vars: &BTreeMap<String, String>| {
        for key in ["AIDA_SESSION_ROLE", crate::seat_authority::GRANT_ENV]
            .into_iter()
            .chain(LAUNCHER_ONLY_ENV.iter().copied())
        {
            match vars.get(key) {
                Some(v) => env.set_key(key, v),
                None => env.unset_key(key),
            }
        }
        crate::advisor_code_gate::enforce_at_commit(&repo, false)
    };
    assert!(
        gate(&parent).is_err(),
        "an advisor committing code is refused"
    );
    gate(&child).expect("the implementer child commits");
}

// ── Stop contract (signoff C1) ────────────────────────────────────────────

struct MarkerGuard;
impl MarkerGuard {
    fn set() -> Self {
        TEST_MARKER.with(|m| *m.borrow_mut() = Some(LEASE.to_string()));
        MarkerGuard
    }
}
impl Drop for MarkerGuard {
    fn drop(&mut self) {
        TEST_MARKER.with(|m| *m.borrow_mut() = None);
    }
}

// trace:STORY-1662.AC6 | ai:claude
#[test]
fn pr_ship_refuses_under_the_marker() {
    let cli = crate::command_registry::try_parse_from(["aida", "pr", "ship", "42"]).unwrap();
    let args = cli.command.registered::<crate::pr_cmd::PrArgs>();
    let _marker = MarkerGuard::set();
    let err = crate::pr_cmd::handle_pr_command(&args.inner).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("`aida pr ship` is refused"), "{text}");
    assert!(text.contains(MARKER_ENV), "{text}");
}

/// The refusal sits in the shared merge path, so every caller of it (the
/// `aida ship` finish tail included) is covered, not only the CLI arm.
// trace:STORY-1662.AC6 | ai:claude
#[test]
fn the_shared_ship_path_refuses_under_the_marker() {
    let _marker = MarkerGuard::set();
    let err = crate::pr_cmd::pr_ship_with_method(
        Some(42),
        true,
        true,
        false,
        false,
        None,
        None,
        true,
        false,
        false,
        None,
        false,
        crate::forge::MergeMethod::Squash,
        false,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("is refused"), "{err:#}");
}

// trace:STORY-1662.AC6 | ai:claude
#[test]
fn top_level_ship_refuses_to_merge_under_the_marker() {
    let _marker = MarkerGuard::set();
    let err = crate::pr_cmd::run_human_finish_ceremony(crate::pr_cmd::HumanFinishOptions {
        spec: Some("STORY-9".into()),
        message: None,
        no_merge: false,
        no_pr: false,
        keep_worktree: false,
        dry_run: false,
        no_trailer_check: false,
    })
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("`aida ship` is refused"),
        "{err:#}"
    );
}

// trace:STORY-1662.AC6 | ai:claude
#[test]
fn burndown_and_release_merges_refuse_under_the_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let _marker = MarkerGuard::set();
    let pr = crate::burndown::ResidualPr {
        number: 42,
        spec: "STORY-9".into(),
        title: "Widget (STORY-9)".into(),
        branch: "claude/story-9".into(),
    };
    assert!(
        !crate::burndown_run::merge_wave_pr(tmp.path(), &pr),
        "a wave merge does not run"
    );
    let err =
        crate::upgrade_release::handle_release(true, false, false, false, None, true).unwrap_err();
    assert!(
        format!("{err:#}").contains("`aida release` is refused"),
        "{err:#}"
    );
}

// trace:STORY-1662.AC6 | ai:claude
#[test]
fn review_record_refuses_under_the_marker_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let _marker = MarkerGuard::set();
    let err = crate::handle_review_record_at(
        tmp.path().to_path_buf(),
        "STORY-9",
        "approved",
        Some("abc123"),
        None,
        None,
        &[],
        &[],
        Some(42),
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("`aida review record` is refused"),
        "{err:#}"
    );
    assert!(
        !tmp.path().join(".aida").exists(),
        "a refused record writes nothing"
    );
}

// trace:STORY-1662.AC6 | ai:claude
#[test]
fn the_prompt_carries_the_stop_contract() {
    for phrase in [
        "stop",
        "aida pr ship",
        "aida review record",
        "aida punt",
        "aida pr hold",
    ] {
        assert!(STOP_CONTRACT.contains(phrase), "{phrase}");
    }
}

// ── Concurrency cap (signoff C2) ──────────────────────────────────────────

fn write_slot(dir: &std::path::Path, pid: u32, spec: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"start":"t","spec":"{spec}"}}"#),
    )
    .unwrap();
}

// trace:STORY-1662.AC7 | ai:claude
#[test]
fn cap_refuses_past_the_live_count_with_its_own_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida").join("standalone-implementers");
    for (pid, spec) in [(11, "STORY-1"), (12, "STORY-2"), (13, "STORY-3")] {
        write_slot(&dir, pid, spec);
    }
    write_slot(&dir, 99, "STORY-DEAD");
    let alive = |pid: u32, _: Option<&str>| pid != 99;

    let err = acquire_slot_with(tmp.path(), "STORY-4", 3, 500, alive)
        .err()
        .expect("three live launchers fill a cap of 3");
    assert_eq!(crate::exit_code_for_error(&err), CAP_EXIT_CODE);
    let text = format!("{err:#}");
    assert!(text.contains("STORY-1, STORY-2, STORY-3"), "{text}");
    assert!(text.contains("standalone_implementer_cap"), "{text}");
    assert!(
        !dir.join("500.json").exists(),
        "a refused launch takes no slot"
    );
    assert!(
        !dir.join("99.json").exists(),
        "a dead launcher's slot is pruned"
    );

    let guard = acquire_slot_with(tmp.path(), "STORY-4", 4, 500, alive).unwrap();
    assert!(dir.join("500.json").exists());
    drop(guard);
    assert!(
        !dir.join("500.json").exists(),
        "the slot is released on drop"
    );
}

// trace:STORY-1662.AC7 | ai:claude
#[test]
fn cap_comes_from_config_with_a_default_of_three() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    assert_eq!(configured_cap(tmp.path()), DEFAULT_CAP);
    assert_eq!(DEFAULT_CAP, 3);
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[queue]\nstandalone_implementer_cap = 5\n",
    )
    .unwrap();
    assert_eq!(configured_cap(tmp.path()), 5);
    std::fs::write(
        tmp.path().join(".aida/config.toml"),
        "[queue]\nstandalone_implementer_cap = 0\n",
    )
    .unwrap();
    assert_eq!(configured_cap(tmp.path()), DEFAULT_CAP);
}

// ── Authorship and the Q1 ledger note ─────────────────────────────────────

fn implementer_lease(worktree: std::path::PathBuf) -> SessionLease {
    let mut table: toml::Table = toml::from_str(&format!(
        r#"
id = "{LEASE}"
scope = "STORY-9"
slug = "story-9"
owner = "tester"
branch = "claude/story-9"
started_at = "2026-10-21T00:00:00Z"
hostname = "h"
role = "implementer"
"#
    ))
    .unwrap();
    table.insert(
        "worktree_path".into(),
        toml::Value::String(worktree.to_string_lossy().into_owned()),
    );
    let mut lease: SessionLease = table.try_into().unwrap();
    // The caller's Bash shell ran `aida queue work`.
    lease.creator_pid = Some(200);
    lease.creator_pid_start_time = Some("s".into());
    lease
}

fn proc_entry(pid: u32, name: &str, start: &str) -> crate::review_authority::ProcEntry {
    crate::review_authority::ProcEntry {
        pid,
        name: name.into(),
        cmd: vec![name.into()],
        start: Some(start.into()),
    }
}

// trace:STORY-1662.AC2 | ai:claude
#[test]
fn the_launch_record_never_names_the_orchestrator_that_ran_it() {
    let root = tempfile::tempdir().unwrap();
    let lease = implementer_lease(root.path().join("wt"));
    let path = crate::lease_path(root.path(), &lease.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(&lease).unwrap()).unwrap();

    // `aida queue work` (300) run by the orchestrator's shell (200) under
    // its claude harness (90).
    let orchestrator = crate::review_authority::IdentitySource {
        session_ids: vec!["orchestrator-session".into()],
        chain: vec![
            proc_entry(300, "aida", "a"),
            proc_entry(200, "bash", "s"),
            proc_entry(90, "claude", "h"),
        ],
        session_leader: None,
    };
    crate::review_authority::with_identity_source(orchestrator.clone(), || {
        crate::review_authority::as_launcher(|| {
            crate::review_authority::record_authoring_lease(root.path(), &lease);
            record_child_authority(
                root.path(),
                &lease.id,
                &crate::seat_authority::HeadlessChildGrant::Baseline {
                    cause: "the launcher's grant does not delegate `implementer`".into(),
                },
                Some("g-advisor"),
            );
        })
    });
    // The spawned claude child (400) is recorded as the worker.
    crate::review_authority::record_launched_worker(root.path(), &lease.id, 400);

    let ledger = crate::review_authority::load_ledger(root.path());
    let tokens: Vec<String> = ledger
        .iter()
        .filter(|r| r.lease_id == lease.id)
        .flat_map(|r| r.tokens.clone())
        .collect();
    assert!(!tokens.is_empty());
    let recorder = orchestrator.recorder_tokens();
    assert!(
        !crate::review_authority::identities_overlap(&tokens, &recorder),
        "the orchestrator stays independent of the worker it launched: \
         ledger {tokens:?} vs recorder {recorder:?}"
    );

    // Q1: the baseline fallback is written down for an audit.
    let note = ledger
        .iter()
        .find_map(|r| r.launch.clone())
        .expect("a launch-authority record");
    assert_eq!(note.seat, "implementer");
    assert_eq!(note.grant, "baseline");
    assert_eq!(note.reason, "launcher-did-not-delegate");
    assert!(note.detail.unwrap().contains("does not delegate"));
}

/// TASK-2154: the audit note tells a launcher with no grant apart from a
/// granted launcher that did not delegate `implementer`.
// trace:STORY-1662.AC5 | ai:claude
#[test]
fn baseline_reason_distinguishes_no_grant_from_not_delegated() {
    assert_eq!(baseline_reason(None), BASELINE_NO_GRANT);
    assert_eq!(baseline_reason(Some("")), BASELINE_NO_GRANT);
    assert_eq!(baseline_reason(Some("  ")), BASELINE_NO_GRANT);
    assert_eq!(baseline_reason(Some("g-advisor")), BASELINE_NOT_DELEGATED);
    assert_ne!(BASELINE_NO_GRANT, BASELINE_NOT_DELEGATED);

    let root = tempfile::tempdir().unwrap();
    let lease = implementer_lease(root.path().join("wt"));
    let path = crate::lease_path(root.path(), &lease.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string_pretty(&lease).unwrap()).unwrap();
    record_child_authority(
        root.path(),
        LEASE,
        &crate::seat_authority::HeadlessChildGrant::Baseline {
            cause: "no valid parent seat grant is active".into(),
        },
        None,
    );
    let note = crate::review_authority::load_ledger(root.path())
        .into_iter()
        .find_map(|r| r.launch)
        .expect("a launch-authority record");
    assert_eq!(note.grant, "baseline");
    assert_eq!(note.reason, BASELINE_NO_GRANT);
}

// ── End to end, with a stub vendor ────────────────────────────────────────

/// The real launch against a stub vendor that punts, from a granted advisor
/// that does not delegate `implementer`. While the stub runs (signoff C6;
/// BUG-2207): `aida ps` shows the lane as a live implementer on its spec, the
/// claim set the owner and moved the spec to In Progress, the cargo-slot WHO
/// column (the child's `AIDA_SESSION_ROLE`) reads implementer, and no
/// advisor-role lease exists. Afterwards: exit 32 and nothing of the
/// launcher's reached the child.
// trace:STORY-1662.AC5 | ai:claude
// trace:STORY-1662.AC10 | ai:claude
// trace:STORY-1662.AC11 | ai:claude
#[cfg(unix)]
#[test]
fn stub_vendor_that_punts_exits_32_as_a_live_implementer_lane_with_no_grant_leaked() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let worktree = tmp.path().join("wt");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let dump = tmp.path().join("child-env.txt");
    let started = tmp.path().join("started");
    let release = tmp.path().join("release");
    let mock = tmp.path().join("mock-claude.sh");
    crate::test_exec::write_executable(
        &mock,
        format!(
            "#!/bin/sh\nenv > \"{dump}\"\necho $$ > \"{started}\"\n\
             i=0; while [ ! -f \"{release}\" ] && [ $i -lt 300 ]; do sleep 0.1; i=$((i+1)); done\n\
             printf '%s' '{{\"spec\":\"STORY-9\",\"category\":\"design-fork\",\"detail\":\"pick a schema\"}}' > \"$AIDA_PUNT_SIGNAL_FILE\"\nexit 0\n",
            dump = dump.display(),
            started = started.display(),
            release = release.display(),
        ),
    );
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
        ("AIDA_HEADLESS_VENDOR", None),
        ("AIDA_CONTAINED", None),
        ("AIDA_TEE_HEADLESS", Some("0")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_SESSION_PURPOSE", Some("advise")),
        ("AIDA_REVIEW_VERDICT_FILE", Some("/launcher/verdict.json")),
        (crate::orchestrator::NO_HUMAN_MODE_ENV, Some("both")),
        (crate::seat_authority::GRANT_ENV, None),
    ]);
    // A granted advisor that does not delegate `implementer`: the child
    // falls back to baseline (the Q1 ruling).
    let launcher_grant = crate::seat_authority::test_support::mint_grant_for(
        &project,
        &crate::current_user_id(None),
        "advisor",
        &["reviewer"],
    );
    env.set_key(crate::seat_authority::GRANT_ENV, &launcher_grant);
    let child_grant = crate::seat_authority::issue_headless_child(
        &project,
        "implementer",
        &crate::current_user_id(None),
    )
    .unwrap();
    assert!(matches!(
        child_grant,
        crate::seat_authority::HeadlessChildGrant::Baseline { .. }
    ));

    // What the pickup set up before the launch: the Approved, unowned spec
    // and the implementer lease `queue work` minted for it.
    let store_path = crate::detect_distributed_store_from(&project).unwrap();
    let storage = crate::test_env::home_fixture_storage(&store_path);
    let mut store = storage.load().unwrap_or_default();
    let mut req = aida_core::Requirement::new("Widget".into(), "".into());
    req.spec_id = Some("STORY-9".into());
    req.status = aida_core::RequirementStatus::Approved;
    req.owner = String::new();
    store.requirements.push(req);
    storage.save(&store).unwrap();
    let lease = implementer_lease(worktree.clone());
    let lease = SessionLease {
        creator_pid: None,
        creator_pid_start_time: None,
        ..lease
    };
    let lease_file = crate::lease_path(&project, &lease.id);
    std::fs::create_dir_all(lease_file.parent().unwrap()).unwrap();
    std::fs::write(&lease_file, toml::to_string_pretty(&lease).unwrap()).unwrap();

    let result = std::thread::scope(|scope| {
        let launch = scope.spawn(|| {
            run(Launch {
                project_root: &project,
                vendor: crate::session::HeadlessVendor::Claude,
                prompt: "/aida-pickup STORY-9",
                session_id: "019e0000-0000-7000-8000-000000001662",
                spec: "STORY-9",
                lease_id: LEASE,
                branch: "claude/story-9",
                contained: false,
                child_grant: &child_grant,
            })
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let child_pid: u32 = loop {
            if let Some(pid) = std::fs::read_to_string(&started)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the stub never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let live_checks = || {
            let leases = crate::list_leases(&project);
            let ours = leases
                .iter()
                .find(|l| l.id == LEASE)
                .expect("the lane's lease");
            assert_eq!(
                ours.active_pid,
                Some(child_pid),
                "the child is stamped on its lease"
            );
            assert!(
                leases.iter().all(|l| l.role.as_deref() != Some("advisor")),
                "no advisor-role lease: {leases:?}"
            );
            let (rows, _) = crate::gather_running_work(&project);
            let row = rows
                .iter()
                .find(|r| r.lease.id == LEASE)
                .expect("an `aida ps` row");
            assert_eq!(row.role.as_deref(), Some("implementer"));
            assert!(matches!(row.state, LeaseState::Live), "{:?}", row.state);
            assert_eq!(row.spec.as_deref(), Some("STORY-9"));
            let store = crate::test_env::home_fixture_storage(&store_path)
                .load()
                .unwrap();
            let spec = store
                .requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some("STORY-9"))
                .unwrap();
            assert_eq!(spec.status, aida_core::RequirementStatus::InProgress);
            assert_eq!(spec.owner, crate::get_default_author());
        };
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(live_checks));
        std::fs::write(&release, "").unwrap();
        let result = launch.join().unwrap();
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
        result
    });
    let err = result.expect_err("a punted run is not an open PR");
    assert_eq!(
        crate::exit_code_for_error(&err),
        PUNTED_EXIT_CODE,
        "{err:#}"
    );

    let child: BTreeMap<String, String> = std::fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // The cargo-slot wrapper's WHO column reads this variable.
    assert_eq!(child["AIDA_SESSION_ROLE"], "implementer");
    assert_eq!(
        child[crate::seat_authority::GRANT_ENV],
        "",
        "baseline child"
    );
    assert_eq!(child[MARKER_ENV], LEASE);
    for key in [
        "AIDA_SESSION_PURPOSE",
        "AIDA_REVIEW_VERDICT_FILE",
        crate::orchestrator::NO_HUMAN_MODE_ENV,
    ] {
        assert!(!child.contains_key(key), "{key} leaked: {child:?}");
    }
    assert!(
        !child.values().any(|v| v.contains(&launcher_grant)),
        "the launcher's grant reached the child"
    );
}
