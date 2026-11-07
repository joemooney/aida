//! TASK-2712: the native Codex write set's pure parts, and the ratchet that
//! every Codex spawn path gets it from the one shared helper. The write set
//! against a real seat clone is tested beside the BUG-2169 clone fixture
//! (`bug_2169_contained_clone_bwrap_tests.rs`), the publication end to end
//! beside the seat-publish fixture (`bug_home_038_seat_publish_tests.rs`).
//! trace:TASK-2712 | ai:claude

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Outside a git repository there are no write roots, so the posture carries
/// no `--add-dir` at all: the write set fails closed.
#[test]
fn no_write_roots_means_the_bare_posture() {
    let flags = codex_native_sandbox_flags_for_roots(None, &[], Some("reviewer")).unwrap();
    assert_eq!(flags, s(&CODEX_NATIVE_POSTURE));
    assert!(carries_native_posture(&flags));
}

/// A tree outside git rescopes to the bare posture; the `--add-dir` run the
/// helper emitted is replaced, the arguments around it are kept, and an
/// argv without the native posture is never touched.
#[test]
fn rescope_replaces_only_the_emitted_add_dir_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mut args = s(&[
        "--no-daemon",
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--add-dir",
        "/old/main/.aida-store",
        "--add-dir",
        "/old/main/.aida/cache-db",
        "--model",
        "m",
    ]);
    rescope_codex_native_flags(None, &mut args, tmp.path(), None).unwrap();
    assert_eq!(
        args,
        s(&[
            "--no-daemon",
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "--model",
            "m",
        ])
    );

    let mut other = s(&[
        "--dangerously-bypass-approvals-and-sandbox",
        "--add-dir",
        "/x",
    ]);
    let before = other.clone();
    rescope_codex_native_flags(None, &mut other, tmp.path(), None).unwrap();
    assert_eq!(other, before);
}

/// Advisor F7: a rescope strips EVERY `--add-dir`, not only the run after
/// the posture: a user `--add-dir` elsewhere (either spelling) is gone.
// trace:TASK-2712 | ai:claude
#[test]
fn rescope_strips_every_user_add_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let mut args = s(&[
        "--add-dir",
        "/early",
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--add-dir",
        "/old/main/.aida-store",
        "--model",
        "m",
        "--add-dir",
        "/",
        "--add-dir=/etc",
    ]);
    rescope_codex_native_flags(None, &mut args, tmp.path(), Some("reviewer")).unwrap();
    assert_eq!(
        args,
        s(&[
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "--model",
            "m",
        ])
    );
}

/// Advisor F7: the `--bg` override check refuses a foreign `--add-dir`
/// (outside AIDA's own run, `=`-spelled, `/` itself, or with no native
/// posture at all) and accepts AIDA's own run.
// trace:TASK-2712 | ai:claude
#[test]
fn foreign_add_dir_is_refused_and_the_own_run_is_accepted() {
    let posture = s(&CODEX_NATIVE_POSTURE);
    let mut own = posture.clone();
    own.extend(s(&[
        "--add-dir",
        "/m/.aida-store",
        "--add-dir",
        "/m/.aida/cache-db",
    ]));
    refuse_foreign_add_dir(&own).unwrap();
    crate::agent_launch::agent_codex_background::refuse_posture_overrides(&own).unwrap();

    let mut cases: Vec<Vec<String>> = Vec::new();
    let mut later = own.clone();
    later.extend(s(&["--model", "m", "--add-dir", "/"]));
    cases.push(later);
    let mut adjacent_root = own.clone();
    adjacent_root.extend(s(&["--add-dir", "/"]));
    cases.push(adjacent_root);
    let mut relative = posture.clone();
    relative.extend(s(&["--add-dir", "etc"]));
    cases.push(relative);
    let mut equals = own.clone();
    equals.push("--add-dir=/etc".into());
    cases.push(equals);
    cases.push(s(&["--add-dir", "/m/.aida-store", "exec", "p"]));
    for args in cases {
        let err = refuse_foreign_add_dir(&args).unwrap_err().to_string();
        assert!(err.contains("outside AIDA's own"), "{args:?}: {err}");
        let err = crate::agent_launch::agent_codex_background::refuse_posture_overrides(&args)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--bg"), "{args:?}: {err}");
    }
}

/// Advisor F7: the agents.toml / `--extra-flag` passthrough never forwards
/// a write root.
// trace:TASK-2712 | ai:claude
#[test]
fn user_passthrough_add_dir_is_refused() {
    refuse_user_add_dir(&s(&["--model", "m"]), "--extra-flag").unwrap();
    for flags in [
        s(&["--add-dir", "/"]),
        s(&["--model", "m", "--add-dir=/home"]),
    ] {
        let err = refuse_user_add_dir(&flags, "[agents.codex] default_flags")
            .unwrap_err()
            .to_string();
        assert!(err.contains("default_flags"), "{err}");
        assert!(err.contains("outside AIDA's own write set"), "{err}");
    }
}

/// Advisor F8: a native resume takes the write set of the RESUMED lane's
/// tree, not this process's cwd: a lane outside git gets the bare posture
/// even though this test process runs inside a git checkout.
// trace:TASK-2712 | ai:claude
#[test]
fn a_native_resume_uses_the_resumed_lanes_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let lane = crate::session::CodexLane {
        tree: tmp.path(),
        role: Some("reviewer"),
        host_main: None,
    };
    let args =
        crate::session::codex_interactive_resume_args("t-1", Some(lane), false, false).unwrap();
    let mut want = s(&CODEX_NATIVE_POSTURE);
    want.extend(s(&["resume", "t-1"]));
    assert_eq!(args, want);
}

/// A plan lane (BUG-2394 read-only) drops the write set with the posture.
#[test]
fn a_read_only_plan_lane_drops_every_add_dir() {
    let args = crate::session::codex_read_only_args(s(&[
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--add-dir",
        "/m/.aida-store",
        "--add-dir=/m/.aida/cache-db",
        "exec",
        "p",
    ]));
    assert_eq!(
        args,
        s(&[
            "--sandbox",
            "read-only",
            "--ask-for-approval",
            "never",
            "exec",
            "p"
        ])
    );
}

/// N1 with the N3 fallback: only a native Codex lane is refused publication,
/// and outside a seat clone the refusal names the reason and the way out.
/// (The in-clone refusal, which points at the host commit, is tested beside
/// the seat-publish fixture.)
#[test]
fn publication_outside_a_clone_is_refused_only_for_native_codex() {
    let tmp = tempfile::tempdir().unwrap();
    let err = refuse_native_publication(Some("native"), tmp.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("native Codex sandbox"), "{err}");
    assert!(err.contains("seat clone"), "{err}");
    assert!(err.contains("untouched"), "{err}");
    for posture in [None, Some("aida"), Some("none")] {
        refuse_native_publication(posture, tmp.path()).unwrap();
    }
}

/// Advisor ruling 2026-11-03 18:32: the gate ledger is a reviewer grant
/// only. The one role constant is `reviewer`.
#[test]
fn the_gate_ledger_role_is_the_reviewer() {
    assert_eq!(GATE_LEDGER_ROLE, "reviewer");
}

/// The source of `src/<file>`.
fn production_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// N4 ratchet: the native posture is spelled once, here; every Codex spawn
/// path gets its argv from this module, directly or through
/// `tool_contained_flags("codex")`, which delegates to it; and the detached
/// `--bg` lane (`agent_codex_background.rs`) rescopes the set to its own tree
/// and asks for a seat clone.
#[test]
fn every_codex_spawn_path_gets_the_shared_write_set() {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&src_dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if rel == "codex_native_sandbox.rs" || rel.ends_with("_tests.rs") {
                continue;
            }
            let src = production_source(&rel);
            // trace:STORY-2539 | ai:codex
            // Inline test fixtures can spell argv directly; only production
            // builders must delegate to the shared write-set helper.
            let lines: Vec<_> = src.lines().collect();
            let tests = crate::seat_clone::clone_git_ratchet_tests::test_module_lines(&lines);
            let flat: String = lines
                .iter()
                .enumerate()
                .filter(|(line, _)| !tests.contains(line))
                .flat_map(|(_, text)| text.split_whitespace())
                .collect();
            // An argv that spells the posture itself instead of asking the
            // helper. `config_cmd.rs` renders it for display only.
            let builds = flat.contains("\"--sandbox\".to_string(),\"workspace-write\"")
                || (rel != "config_cmd.rs"
                    && flat.contains("\"--sandbox\".into(),\"workspace-write\""));
            if builds {
                offenders.push(rel);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these files build the native Codex posture without the shared write set \
         (use crate::codex_native_sandbox): {offenders:?}"
    );

    let launch: String = production_source("agent_launch.rs")
        .split_whitespace()
        .collect();
    assert!(
        launch.contains(
            "\"codex\"=>crate::codex_native_sandbox::codex_native_sandbox_flags(host_main)?"
        ),
        "tool_contained_flags(\"codex\") must delegate to the shared helper"
    );
    assert!(
        launch.contains("crate::codex_native_sandbox::rescope_codex_native_flags("),
        "rescope_contained_settings must rescope a Codex launch"
    );
    let bg: String = production_source("agent_codex_background.rs")
        .split_whitespace()
        .collect();
    assert!(
        bg.contains(
            "crate::agent_launch::rescope_contained_settings(&mutconfig,&plan.launch_cwd,plan.role.as_deref(),)?"
        ),
        "the --bg lane must rescope its write set to its own tree and role"
    );
    assert!(
        bg.contains(
            "prepare_agent_launch_attempt_on_branch(&project_root,role,spec,\"codex\",name,cwd_ref,true,"
        ),
        "the --bg lane must ask for a seat clone"
    );
    // Every agent-new launch passes the plan's role, so the gate ledger
    // follows the role (reviewer only).
    assert_eq!(
        launch
            .matches(
                "rescope_contained_settings(&mutconfig,&plan.launch_cwd,plan.role.as_deref())?"
            )
            .count(),
        2,
        "both agent-new paths must rescope with the plan's role"
    );

    // Advisor F2: every reviewer-capable Codex path passes the lane's role
    // to the one helper: headless spawns and coordinators by seat,
    // `exec_codex_session` and the queue-work resume by the lane's role, the
    // interactive reviewer plan and the reviewer resume as `reviewer`.
    let session: String = production_source("session.rs").split_whitespace().collect();
    assert!(
        session.contains(
            "rescope_native_codex_for_seat(host_main,vendor,&mutargs,&headless_worktree_root(),seat.as_str(),)?"
        ),
        "the headless spawn must rescope by its seat"
    );
    assert!(
        session.contains("rescope_native_codex_for_seat(host_main,vendor,&mutargs,cwd,seat)?"),
        "the headless resume must rescope by its seat, in the resumed tree"
    );
    assert!(
        session.contains(
            "crate::codex_native_sandbox::codex_native_sandbox_flags_in(host_main,&headless_worktree_root(),Some(\"reviewer\"),)?"
        ),
        "the interactive reviewer plan must ask for the reviewer write set"
    );
    assert!(
        session.contains(
            "crate::codex_native_sandbox::codex_native_sandbox_flags_in(host_main,&headless_worktree_root(),role,)?"
        ),
        "exec_codex_session must pass its role"
    );
    assert_eq!(
        session
            .matches("crate::tool_contained_flags(None,\"codex\")")
            .count()
            + session
                .matches("crate::tool_contained_flags(host_main,\"codex\")")
                .count(),
        1,
        "only the interactive session config (rescoped by agent new) may take \
         the cwd-resolved, role-less set in session.rs"
    );
    // trace:STORY-2539 | ai:codex
    // Headless composition moved behind the vendor profile. Keep the same
    // role-less-helper count and seat-rescoping requirement across that move.
    let headless: String = production_source("vendors/headless.rs")
        .split_whitespace()
        .collect();
    assert_eq!(headless.matches("crate::tool_contained_flags(").count(), 1);
    assert!(
        headless
            .contains("Some(crate::tool_contained_flags(host_main,vendor.profile().executable,)?)"),
        "headless_vendor_args must obtain its profile's posture from the shared helper"
    );
    let queue: String = production_source("queue_cmd.rs")
        .split_whitespace()
        .collect();
    assert!(queue.contains("child_grant_id.as_deref(),Some(&crate::canonical_role_name(&role)),);"));
    // TASK-2032: the reviewer resume carries the pinned host main.
    assert!(queue.contains("role:Some(\"reviewer\"),host_main,"));
}

/// Advisor F2: a composed native Codex argv rescoped by seat. Outside git
/// (no write roots) the posture survives and nothing else is granted; a
/// non-Codex argv is untouched. The ledger itself is asserted against a real
/// seat clone beside the BUG-2169 fixture.
// trace:TASK-2712 | ai:claude
#[test]
fn rescope_by_seat_touches_only_native_codex() {
    let tmp = tempfile::tempdir().unwrap();
    let mut codex = s(&CODEX_NATIVE_POSTURE);
    codex.extend(s(&["--add-dir", "/x", "exec", "p"]));
    crate::session::rescope_native_codex_for_seat(
        None,
        crate::session::HeadlessVendor::Codex,
        &mut codex,
        tmp.path(),
        "reviewer",
    )
    .unwrap();
    let mut want = s(&CODEX_NATIVE_POSTURE);
    want.extend(s(&["exec", "p"]));
    assert_eq!(codex, want);
    let mut claude = s(&["--add-dir", "/x", "-p"]);
    let before = claude.clone();
    crate::session::rescope_native_codex_for_seat(
        None,
        crate::session::HeadlessVendor::Claude,
        &mut claude,
        tmp.path(),
        "reviewer",
    )
    .unwrap();
    assert_eq!(claude, before);
}
