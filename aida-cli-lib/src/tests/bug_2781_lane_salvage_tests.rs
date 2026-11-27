//! BUG-2781 AC4: a lane that ends non-zero after committing is salvage-pushed
//! from host git; uncommitted-only work and a non-fast-forward are retained.
// trace:BUG-2781 | ai:claude

use super::seat_publish_tests::{git_ok, seat_with_commit, test_bundle};
use super::tests::{fixture, ref_exists};
use crate::headless_background::{OriginHead, SeatState};
use crate::lane_salvage::{on_failed_exit, Lane, UNVERIFIED};
use crate::standalone_implementer::Outcome;
use std::cell::RefCell;
use std::path::Path;

fn lane<'a>(
    main: &'a Path,
    clone: &'a Path,
    branch: &'a str,
    id: &'a str,
    pr: Option<u64>,
) -> Lane<'a> {
    Lane {
        main,
        clone,
        spec: "BUG-1",
        branch,
        lane: id,
        launch_head: None,
        open_pr: pr,
    }
}

fn committed(tip: &str) -> SeatState {
    SeatState {
        head: Some(tip.into()),
        origin: OriginHead::Absent,
        uncommitted: Some(0),
        head_has_unpushed: Some(true),
    }
}

fn no_imports(main: &Path) {
    let refs = git_ok(
        main,
        &[
            "for-each-ref",
            "--format=%(refname)",
            crate::ship_from_seat::QUARANTINE_PREFIX,
            "refs/aida/publication/",
        ],
    );
    assert_eq!(refs, "", "host import refs outlived the salvage");
}

#[cfg(unix)]
fn sigterm() -> String {
    use std::os::unix::process::ExitStatusExt as _;
    crate::lane_salvage::exit_cause(std::process::ExitStatus::from_raw(15))
}

#[cfg(unix)]
#[test]
fn bug_2781_sigterm_after_commit_salvage_pushes_to_a_salvage_ref() {
    let fx = fixture();
    let id = "c0ffee278101";
    let (clone, tip) = seat_with_commit(&fx, "salvage", id, "seat-salvage", "fix: x (BUG-1)");
    let said = RefCell::new(Vec::new());
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "seat-salvage", id, None),
        &Outcome::NoPr,
        &committed(&tip),
        &sigterm(),
        &test_bundle,
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
    )
    .expect("salvaged");
    let target = format!("refs/heads/salvage/{id}");
    assert_eq!(git_ok(&fx.origin, &["rev-parse", &target]), tip);
    assert!(
        !ref_exists(&fx.origin, "refs/heads/seat-salvage"),
        "no PR: the claimed branch stays untouched"
    );
    assert!(note.starts_with(UNVERIFIED), "{note}");
    let said = said.into_inner();
    assert_eq!(said.len(), 1);
    assert!(
        said[0].0.contains("SIGTERM") && said[0].0.contains(UNVERIFIED),
        "{:?}",
        said[0]
    );
    no_imports(&fx.main);
}

#[cfg(unix)]
#[test]
fn bug_2781_open_pr_branch_is_fast_forwarded() {
    let fx = fixture();
    let id = "c0ffee278102";
    let (clone, tip) = seat_with_commit(&fx, "prff", id, "seat-prff", "fix: y (BUG-1)");
    git_ok(
        &fx.main,
        &[
            "push",
            "-q",
            "origin",
            "refs/remotes/origin/main:refs/heads/seat-prff",
        ],
    );
    let said = RefCell::new(Vec::new());
    on_failed_exit(
        &lane(&fx.main, &clone, "seat-prff", id, Some(7)),
        &Outcome::PrOpened { pr: 7, head: None },
        &committed(&tip),
        &sigterm(),
        &test_bundle,
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
    )
    .expect("salvaged");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-prff"]),
        tip
    );
    assert_eq!(said.into_inner()[0].1, Some(7), "announced on the PR");
    no_imports(&fx.main);
}

#[cfg(unix)]
#[test]
fn bug_2781_uncommitted_only_pushes_nothing() {
    let fx = fixture();
    let id = "c0ffee278103";
    let (clone, tip) = seat_with_commit(&fx, "dirty", id, "seat-dirty", "fix: z (BUG-1)");
    std::fs::write(clone.join("edit.txt"), "edit\n").unwrap();
    let edits = SeatState {
        head: Some(tip.clone()),
        origin: OriginHead::Absent,
        uncommitted: Some(1),
        head_has_unpushed: Some(true),
    };
    let mut at_launch = lane(&fx.main, &clone, "seat-dirty", id, None);
    at_launch.launch_head = Some(&tip);
    let note = on_failed_exit(
        &at_launch,
        &Outcome::NoPr,
        &edits,
        &sigterm(),
        &|_, _, _| panic!("bundled a seat with nothing committed"),
        &|_, _| panic!("announced a salvage that did not happen"),
    );
    assert_eq!(note, None);
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    assert_eq!(
        git_ok(&clone, &["status", "--porcelain", "edit.txt"]),
        "?? edit.txt",
        "retained"
    );
}

#[cfg(unix)]
#[test]
fn bug_2781_non_fast_forward_is_refused_and_retained() {
    let fx = fixture();
    let id = "c0ffee278104";
    let (clone, tip) = seat_with_commit(&fx, "diverged", id, "seat-diverged", "fix: w (BUG-1)");
    let other = git_ok(
        &fx.main,
        &[
            "-c",
            "user.name=Other",
            "-c",
            "user.email=other@example.com",
            "commit-tree",
            "origin/main^{tree}",
            "-p",
            "origin/main",
            "-m",
            "someone else's push",
        ],
    );
    git_ok(
        &fx.main,
        &[
            "push",
            "-q",
            "origin",
            &format!("{other}:refs/heads/seat-diverged"),
        ],
    );
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "seat-diverged", id, Some(8)),
        &Outcome::PrOpened { pr: 8, head: None },
        &committed(&tip),
        &sigterm(),
        &test_bundle,
        &|_, _| panic!("announced a refused salvage"),
    )
    .expect("a refusal note");
    assert!(
        note.contains("refused") && note.contains("fast-forward"),
        "{note}"
    );
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-diverged"]),
        other
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "HEAD"]),
        tip,
        "the commits stay in the seat"
    );
    no_imports(&fx.main);
}
