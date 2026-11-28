//! BUG-035: a contained lane pushes its PR branch from a seat clone, so the
//! primary checkout running `aida pr auto-queue-review` has no local branch
//! by the forge's bare head name. `git_log_messages` must still find the
//! commits (via `origin/<head>`, fetching it if needed), and a range nothing
//! resolves must come back as an error naming it, never as "no commits".
//! trace:BUG-B6B1278890774EF993D2EBC327862470-035 | ai:claude

use super::*;
use crate::{git_log_messages, resolve_log_range};
use aida_core::DatabaseBackend;

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@localhost",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A bare origin with `main`, and a primary clone of it. Returns
/// `(tmp, origin, primary, seat)`; the seat clone has not pushed anything yet.
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let origin = tmp.path().join("origin.git");
    let seed = tmp.path().join("seed");
    let primary = tmp.path().join("primary");
    let seat = tmp.path().join("seat");
    git(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(tmp.path(), &["clone", "-q", "origin.git", "seed"]);
    git(
        &seed,
        &["commit", "-q", "--allow-empty", "--no-verify", "-m", "base"],
    );
    git(&seed, &["push", "-q", "origin", "main"]);
    git(tmp.path(), &["clone", "-q", "origin.git", "primary"]);
    git(tmp.path(), &["clone", "-q", "origin.git", "seat"]);
    (tmp, origin, primary, seat)
}

fn push_lane_branch(seat: &std::path::Path) {
    git(seat, &["checkout", "-q", "-b", "lane-branch"]);
    git(
        seat,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "--no-verify",
            "-m",
            "[AI:claude] fix(x): lane work (BUG-HOME-035)",
        ],
    );
    git(seat, &["push", "-q", "origin", "lane-branch"]);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-035.acab779a | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-035.Test | ai:claude
#[test]
fn head_branch_only_on_origin_is_fetched_and_trailers_found() {
    let (_tmp, _origin, primary, seat) = fixture();
    push_lane_branch(&seat);

    // The primary has neither `lane-branch` nor `origin/lane-branch`.
    let messages =
        git_log_messages(&primary, "main", "lane-branch").expect("range resolves via origin");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("(BUG-HOME-035)"), "{messages:?}");
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-035.acab779a | ai:claude
#[test]
fn head_branch_present_as_remote_tracking_ref_resolves_without_local_branch() {
    let (_tmp, _origin, primary, seat) = fixture();
    push_lane_branch(&seat);
    git(&primary, &["fetch", "-q", "origin"]);

    assert_eq!(
        resolve_log_range(&primary, "main", "lane-branch"),
        ("origin/main".to_string(), "origin/lane-branch".to_string())
    );
    let messages = git_log_messages(&primary, "main", "lane-branch").expect("resolves");
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// A remote-tracking ref fetched before the lane's latest push is refreshed,
/// so trailers on the newer commits are not missed.
// trace:BUG-B6B1278890774EF993D2EBC327862470-035.acab779a | ai:claude
#[test]
fn stale_remote_tracking_head_is_refreshed_before_logging() {
    let (_tmp, _origin, primary, seat) = fixture();
    push_lane_branch(&seat);
    git(&primary, &["fetch", "-q", "origin"]);
    git(
        &seat,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "--no-verify",
            "-m",
            "[AI:claude] fix(x): later lane work (BUG-HOME-036)",
        ],
    );
    git(&seat, &["push", "-q", "origin", "lane-branch"]);

    let messages = git_log_messages(&primary, "main", "lane-branch").expect("resolves");
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages.iter().any(|m| m.contains("(BUG-HOME-036)")),
        "{messages:?}"
    );
}

#[test]
fn local_head_branch_is_used_as_is() {
    let (_tmp, _origin, primary, _seat) = fixture();
    git(&primary, &["branch", "local-only"]);
    assert_eq!(
        resolve_log_range(&primary, "main", "local-only"),
        ("main".to_string(), "local-only".to_string())
    );
}

/// TASK-3518: `aida ship` rebases the seat's branch onto a newer
/// `origin/main` but leaves the seat's local `main` behind. Lands
/// `(BUG-7001)` on origin, then rebases the seat's `(BUG-7002)` branch onto it.
fn rebased_seat_with_stale_local_main(primary: &std::path::Path, seat: &std::path::Path) {
    let commit = |dir: &std::path::Path, msg: &str| {
        git(
            dir,
            &["commit", "-q", "--allow-empty", "--no-verify", "-m", msg],
        );
    };
    commit(primary, "[AI:claude] fix(x): landed work (BUG-7001) (#1)");
    git(primary, &["push", "-q", "origin", "main"]);
    git(seat, &["checkout", "-q", "-b", "lane-branch"]);
    commit(seat, "[AI:claude] fix(x): lane work (BUG-7002)");
    git(seat, &["fetch", "-q", "origin"]);
    git(seat, &["rebase", "-q", "origin/main"]);
}

// trace:TASK-3518 | ai:claude
#[test]
fn rebased_seat_with_stale_local_main_logs_only_its_own_commits() {
    let (_tmp, _origin, primary, seat) = fixture();
    rebased_seat_with_stale_local_main(&primary, &seat);

    assert_eq!(
        resolve_log_range(&seat, "main", "lane-branch"),
        ("origin/main".to_string(), "lane-branch".to_string())
    );
    let messages = git_log_messages(&seat, "main", "lane-branch").expect("resolves");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("(BUG-7002)"), "{messages:?}");
}

/// A local base with commits origin lacks is kept, so those commits stay
/// outside the range rather than being pulled into it.
// trace:TASK-3518 | ai:claude
#[test]
fn local_base_ahead_of_origin_is_kept() {
    let (_tmp, _origin, primary, _seat) = fixture();
    git(
        &primary,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "--no-verify",
            "-m",
            "local-only main work (BUG-7003)",
        ],
    );
    git(&primary, &["checkout", "-q", "-b", "topic"]);
    assert_eq!(
        resolve_log_range(&primary, "main", "topic"),
        ("main".to_string(), "topic".to_string())
    );
    assert!(git_log_messages(&primary, "main", "topic")
        .expect("resolves")
        .is_empty());
}

/// The PR-4685 shape end to end: the review story filed from a rebased seat
/// links only the PR's own spec, never one whose commit already landed.
// trace:TASK-3518 | ai:claude
#[test]
fn auto_queue_from_rebased_seat_links_only_the_prs_own_spec() {
    let (tmp, _origin, primary, seat) = fixture();
    rebased_seat_with_stale_local_main(&primary, &seat);
    std::fs::create_dir_all(seat.join(".aida")).unwrap();
    std::fs::write(
        seat.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    crate::test_env::home_fixture_backend(&seat.join(".aida-store"))
        .unwrap()
        .save(&aida_core::RequirementsStore::default())
        .unwrap();
    let gh = tmp.path().join("gh");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\n\
         case \"$1 $2\" in\n\
         'pr list') printf '42\\tlane\\thttps://github.com/acme/widget/pull/42\\tlane-branch\\n' ;;\n\
         'pr view') printf '{\"state\":\"OPEN\",\"title\":\"lane\",\"baseRefName\":\"main\",\"headRefName\":\"lane-branch\"}\\n' ;;\n\
         *) exit 1 ;;\n\
         esac\n",
    );
    let log = tmp.path().join("aida.log");
    let aida = tmp.path().join("aida");
    crate::test_exec::write_executable(
        &aida,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n\
             if [ \"$1\" = add ]; then echo 'Added: STORY-900 - review'; fi\n",
            log.display()
        ),
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let _aida = crate::kernel::test_aida_exe::set(aida);

    let outcome = crate::review_gates::try_auto_queue_pr_review_unbumped(
        &seat,
        "lane-branch",
        "session-task-3518",
        crate::review_gates::AutoQueueOrigin::PrSkill,
    );
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        outcome.review_spec.as_deref(),
        Some("STORY-900"),
        "{}\n{calls}",
        outcome.summary
    );
    assert!(
        calls
            .lines()
            .any(|l| l == "rel add STORY-900 BUG-7002 --type implements"),
        "{calls}"
    );
    assert!(
        !calls.contains("BUG-7001"),
        "a spec that already landed must not be linked or listed:\n{calls}"
    );
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-035.acbcb0eb | ai:claude
#[test]
fn unresolvable_head_is_an_error_naming_the_range() {
    let (_tmp, _origin, primary, _seat) = fixture();
    let err = git_log_messages(&primary, "main", "nowhere-branch")
        .expect_err("a branch that exists nowhere must not read as zero commits");
    assert!(
        format!("{err:#}").contains("main..nowhere-branch"),
        "error must name the range: {err:#}"
    );
}

/// AC2 at the auto-queue layer: a PR whose head branch exists nowhere this
/// checkout or its origin can see is reported as needing attention with the
/// unreadable range named, never as "no `(REQ-ID)` trailers", and no
/// reviewer story is filed.
// trace:BUG-B6B1278890774EF993D2EBC327862470-035.acbcb0eb | ai:claude
#[test]
fn auto_queue_names_an_unreadable_range_instead_of_reporting_no_trailers() {
    let (tmp, _origin, primary, _seat) = fixture();
    let store = primary.join(".aida-store");
    std::fs::create_dir_all(primary.join(".aida")).unwrap();
    std::fs::write(
        primary.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .save(&aida_core::RequirementsStore::default())
        .unwrap();

    let gh = tmp.path().join("gh");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\n\
         case \"$1 $2\" in\n\
         'pr list') printf '42\\tlane\\thttps://github.com/acme/widget/pull/42\\tnowhere-branch\\n' ;;\n\
         'pr view') printf '{\"state\":\"OPEN\",\"title\":\"lane\",\"baseRefName\":\"main\",\"headRefName\":\"nowhere-branch\"}\\n' ;;\n\
         *) exit 1 ;;\n\
         esac\n",
    );
    let log = tmp.path().join("aida.log");
    let aida = tmp.path().join("aida");
    crate::test_exec::write_executable(
        &aida,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display()),
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let _aida = crate::kernel::test_aida_exe::set(aida);

    let outcome = crate::review_gates::try_auto_queue_pr_review_unbumped(
        &primary,
        "nowhere-branch",
        "session-bug-035",
        crate::review_gates::AutoQueueOrigin::PrSkill,
    );
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        matches!(
            outcome.status,
            crate::review_gates::AutoQueueStatus::SkippedNeedsAttention
        ),
        "{}\n{calls}",
        outcome.summary
    );
    assert!(
        outcome.summary.contains("main..nowhere-branch"),
        "{}",
        outcome.summary
    );
    assert!(
        !outcome.summary.contains("trailers"),
        "an unreadable range is not a trailer problem: {}",
        outcome.summary
    );
    assert!(
        !calls.lines().any(|l| l.starts_with("add ")),
        "no reviewer story may be filed:\n{calls}"
    );
}
