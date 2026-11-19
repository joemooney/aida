//! BUG-2688: conflict rework preparation names the exact missing evidence and
//! its one recovery command, and `queue work --fresh` never resets a retained
//! seat in place: it moves the seat aside intact and starts a fresh checkout
//! of the PR head at the original path.
// trace:BUG-2688 | ai:claude

use super::tests::{fixture, Fixture};
use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=T"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every entry under `root` (Git metadata included) with its exact bytes, or
/// its link target for a symlink.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                out.insert(rel, target.into_os_string().into_encoded_bytes());
            } else if meta.is_dir() {
                out.insert(rel, b"<dir>".to_vec());
                walk(root, &path, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn seat_pins(main: &Path) -> String {
    git_ok(
        main,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            PIN_PREFIX,
        ],
    )
}

/// A PR branch `feat` on origin, and the scope's default seat clone of it.
fn retained_clone_seat() -> (Fixture, PathBuf, String) {
    let fx = fixture();
    git_ok(&fx.main, &["checkout", "-qb", "feat"]);
    std::fs::write(fx.main.join("README.md"), "PR version\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "PR version"]);
    git_ok(&fx.main, &["push", "-q", "origin", "feat"]);
    git_ok(&fx.main, &["checkout", "-q", "main"]);
    git_ok(&fx.main, &["fetch", "-q", "origin"]);
    let pr_head = git_ok(&fx.main, &["rev-parse", "origin/feat"]);
    let seat = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-bug-2688"));
    create(&fx.main, &seat, "feat", "origin/feat").unwrap();
    (fx, seat, pr_head)
}

/// Regressions (a) and (b): committed, staged, unstaged, untracked and ignored
/// content survives byte-for-byte in the moved-aside seat (its whole `.git`
/// included), and the new checkout at the original path is at the PR head.
#[test]
fn fresh_moves_retained_seat_aside_intact_then_checks_out_pr_head() {
    let (fx, seat, pr_head) = retained_clone_seat();
    std::fs::write(seat.join("local.txt"), "unpublished commit\n").unwrap();
    git_ok(&seat, &["add", "local.txt"]);
    git_ok(&seat, &["commit", "-qm", "local work"]);
    std::fs::write(seat.join("README.md"), "staged\n").unwrap();
    git_ok(&seat, &["add", "README.md"]);
    std::fs::write(seat.join("README.md"), "unstaged\n").unwrap();
    std::fs::write(seat.join("notes.txt"), "untracked\n").unwrap();
    let exclude = seat.join(".git/info/exclude");
    let mut rules = std::fs::read_to_string(&exclude).unwrap_or_default();
    rules.push_str("secret.log\nbuild/\n");
    std::fs::write(&exclude, rules).unwrap();
    std::fs::write(seat.join("secret.log"), "ignored file\n").unwrap();
    std::fs::create_dir_all(seat.join("build")).unwrap();
    std::fs::write(seat.join("build/out.bin"), [0u8, 159, 146, 150, 255]).unwrap();
    let status = git_ok(&seat, &["status", "--porcelain", "--ignored"]);
    for line in ["MM README.md", "?? notes.txt", "!! secret.log", "!! build/"] {
        assert!(status.contains(line), "{line:?} in {status}");
    }
    let before = snapshot(&seat);
    let pin = pin_ref(&seat).unwrap();
    let pinned = git_ok(&fx.main, &["rev-parse", &pin]);

    let moved = move_aside_retained_rework_seats(&fx.main, "bug-2688", "feat", 2688).unwrap();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0].from, seat);
    let to = moved[0].to.clone();
    assert_eq!(to.parent(), seat.parent());
    let name = to.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with(&format!(
            "{CLONE_DIR_PREFIX}aida-bug-2688{RETAINED_SEAT_INFIX}"
        )),
        "{name}"
    );
    assert!(!seat.exists());
    assert_eq!(snapshot(&to), before, "moved seat changed");
    // The base pin follows the moved clone, so reap keeps it for the operator.
    assert_eq!(
        seat_pins(&fx.main),
        format!("{pinned} {}", pin_ref(&to).unwrap())
    );
    assert!(orphan_pins(&fx.main)
        .iter()
        .any(|o| o.clone == to && o.clone_exists));

    // A fresh checkout of the PR head takes the original path.
    create(&fx.main, &seat, "feat", "origin/feat").unwrap();
    assert_eq!(git_ok(&seat, &["rev-parse", "HEAD"]), pr_head);
    assert_eq!(git_ok(&seat, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(seat.join("README.md")).unwrap(),
        "PR version\n"
    );
    // A leased seat is never moved; the moved-aside seat stays untouched.
    record_test_publication_lease(&fx.main, &seat, "feat");
    assert!(
        move_aside_retained_rework_seats(&fx.main, "bug-2688", "feat", 2688)
            .unwrap()
            .is_empty()
    );
    assert!(seat.join(".git").is_dir());
    assert_eq!(snapshot(&to), before, "moved seat changed");
}

/// A seat of another branch is not this rework's, and stays in place.
#[test]
fn fresh_moves_only_seats_of_the_pr_branch() {
    let (fx, seat, _) = retained_clone_seat();
    let other = fx.root.join(format!("{CLONE_DIR_PREFIX}aida-bug-2688-2"));
    create(&fx.main, &other, "other", "origin/main").unwrap();
    let moved = move_aside_retained_rework_seats(&fx.main, "bug-2688", "feat", 2688).unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].from, seat);
    assert!(other.join(".git").is_dir());
}

/// Regression (d): a failed rename refuses, and nothing changes: the seat,
/// its bytes and its pin are exactly as before.
#[cfg(unix)]
#[test]
fn failed_move_aside_refuses_with_no_changes() {
    use std::os::unix::fs::PermissionsExt as _;
    let (fx, seat, _) = retained_clone_seat();
    std::fs::write(seat.join("notes.txt"), "untracked\n").unwrap();
    let before = snapshot(&seat);
    let pins = seat_pins(&fx.main);
    std::fs::set_permissions(&fx.root, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A privileged runner ignores directory permissions; nothing to prove.
    let privileged = std::fs::write(fx.root.join("probe"), "").is_ok();
    let result =
        (!privileged).then(|| move_aside_retained_rework_seats(&fx.main, "bug-2688", "feat", 2688));
    std::fs::set_permissions(&fx.root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let Some(result) = result else {
        return;
    };
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains(&format!(
            "could not move retained seat {} aside",
            seat.display()
        )),
        "{error}"
    );
    assert!(error.contains("nothing was moved or changed"), "{error}");
    assert_eq!(snapshot(&seat), before);
    assert_eq!(seat_pins(&fx.main), pins);
}

// A worktree seat whose PR branch conflicts with main on README.md, pushed.
struct ConflictSeat {
    fx: Fixture,
    seat: PathBuf,
    head: String,
    base: String,
}

fn conflict_seat() -> ConflictSeat {
    let fx = fixture();
    git_ok(&fx.main, &["branch", "feat"]);
    let seat = fx.root.join("repair-seat");
    git_ok(
        &fx.main,
        &["worktree", "add", seat.to_str().unwrap(), "feat"],
    );
    std::fs::write(seat.join("README.md"), "PR version\n").unwrap();
    git_ok(&seat, &["commit", "-qam", "PR version"]);
    git_ok(&seat, &["push", "-q", "origin", "feat"]);
    std::fs::write(fx.main.join("README.md"), "main version\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "main version"]);
    git_ok(&fx.main, &["push", "-q", "origin", "main"]);
    let head = git_ok(&seat, &["rev-parse", "HEAD"]);
    let base = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    ConflictSeat {
        fx,
        seat,
        head,
        base,
    }
}

fn prepare(c: &ConflictSeat, seat: ReworkSeat, lane: &str) -> anyhow::Result<Vec<String>> {
    prepare_rework_merge_fixture_seat(&c.fx.main, &c.seat, "feat", seat, 2688, lane, "BUG-2688")
}

/// Replace the seat with a clean checkout of the PR branch, as a new pickup
/// would after the old seat was removed or moved aside.
fn reseat(c: &ConflictSeat) {
    let path = c.seat.to_str().unwrap();
    git_ok(&c.fx.main, &["worktree", "remove", "--force", path]);
    git_ok(&c.fx.main, &["worktree", "add", path, "feat"]);
}

// A lease with no recorded owner process: its lane cannot be proven dead.
fn unproven_lease(main: &Path, id: &str, tree: &Path) {
    let path = crate::lease_path(main, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "id = {id:?}\nscope = \"BUG-2688\"\nslug = \"bug-2688\"\nowner = \"test\"\nworktree_path = {}\nbranch = \"feat\"\nstarted_at = \"2026-11-16T00:00:00Z\"\nhostname = \"h\"\nrole = \"implementer\"\n",
            aida_core::toml_quote::toml_string(&tree.display().to_string()),
        ),
    )
    .unwrap();
}

/// AC 4: conflict rework from a clean remote branch, then seat gone and no
/// journal: a new seat starts fresh from origin, not a hard refusal.
#[test]
fn clean_remote_branch_and_seat_gone_without_journal_start_fresh() {
    let c = conflict_seat();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "first").unwrap(),
        ["README.md"]
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
    // The seat and its journal disappear.
    reseat(&c);
    std::fs::remove_file(c.fx.main.join(".aida/repair-journal.json")).unwrap();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "second").unwrap(),
        ["README.md"]
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
    assert_eq!(git_ok(&c.seat, &["status", "--porcelain"]), "UU README.md");
}

/// AC 1-3: a dead lane's reservation of the PR head refuses a new seat with
/// the exact evidence and the one command; `--fresh` releases it (logged) and
/// restarts the conflict merge from the verified remote head.
#[test]
fn dead_lane_reservation_names_fresh_and_fresh_restarts_from_origin() {
    let c = conflict_seat();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "old-lane").unwrap(),
        ["README.md"]
    );
    reseat(&c);
    let refusal = format!(
        "{:#}",
        prepare(&c, ReworkSeat::New, "new-lane").unwrap_err()
    );
    for needle in [
        format!(
            "repair head {} of PR-2688 is already reserved by lane old-lane",
            c.head
        ),
        format!("host repair journal: lane old-lane reserves head {}", c.head),
        "releasable: its lease has ended".into(),
        "0 local commit(s) not on origin; 0 pending path(s)".into(),
        "Recovery: `aida queue work BUG-2688 --fresh` moves an unleased retained seat of feat aside intact".into(),
        format!("restarts the conflict merge in a new seat from origin/feat at {}", c.head),
    ] {
        assert!(refusal.contains(&needle), "missing {needle:?} in {refusal}");
    }
    assert!(!refusal.contains("diagnose"), "{refusal}");
    assert!(git_ok(&c.seat, &["status", "--porcelain"]).is_empty());

    let fresh = crate::queue_cmd::rework_seat_for(None, true);
    assert_eq!(fresh, ReworkSeat::Fresh);
    assert_eq!(prepare(&c, fresh, "new-lane").unwrap(), ["README.md"]);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
    let journal = crate::repair_journal::RepairGuard::lock(&c.fx.main).unwrap();
    assert_eq!(
        journal
            .for_tree(&c.seat, "feat", &c.head, &c.base)
            .unwrap()
            .lane_id,
        "new-lane"
    );
}

/// `--fresh` never takes over a reservation whose lane is live or unproven.
#[test]
fn fresh_keeps_an_unproven_lanes_reservation() {
    let c = conflict_seat();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "live-lane").unwrap(),
        ["README.md"]
    );
    reseat(&c);
    unproven_lease(&c.fx.main, "live-lane", &c.seat);
    let refusal = format!(
        "{:#}",
        prepare(&c, ReworkSeat::Fresh, "new-lane").unwrap_err()
    );
    for needle in [
        "a fresh restart would take over a reservation whose lane is not proven dead",
        "retained: its lease is live or its owner cannot be proven dead",
        "lane live-lane keeps its reservation because its lease is live or its owner cannot be proven dead; `aida session show live-lane` reports its liveness. After that lane exits, `aida session end live-lane` releases it; then retry `aida queue work BUG-2688 --fresh`",
    ] {
        assert!(refusal.contains(needle), "missing {needle:?} in {refusal}");
    }
    assert!(git_ok(&c.seat, &["status", "--porcelain"]).is_empty());
    let lanes: Vec<String> = crate::repair_journal::RepairGuard::lock(&c.fx.main)
        .unwrap()
        .retained_for(&c.seat, "feat", 2688, &c.head)
        .into_iter()
        .map(|e| e.lane_id)
        .collect();
    assert_eq!(lanes, ["live-lane"]);
}

/// AC 4: a genuinely retained conflict with its journal resumes, edits kept,
/// under verified adoption; the queue seam decides by adoption alone.
#[test]
fn adopted_seat_resumes_journaled_conflict_with_edits() {
    let c = conflict_seat();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "orphan-lane").unwrap(),
        ["README.md"]
    );
    std::fs::write(c.seat.join("README.md"), "keep both\n").unwrap();
    unproven_lease(&c.fx.main, "orphan-lane", &c.seat);
    let orphan = crate::leases::read_exact_lease(&c.fx.main, "orphan-lane")
        .unwrap()
        .unwrap();
    let adopted = crate::queue_cmd::rework_seat_for(Some(&orphan), true);
    assert_eq!(adopted, ReworkSeat::Adopted);
    assert_eq!(
        crate::queue_cmd::rework_seat_for(None, false),
        ReworkSeat::New
    );
    crate::repair_journal::RepairGuard::lock(&c.fx.main)
        .unwrap()
        .transfer_adopted_lane(&c.seat, "feat", "orphan-lane", "successor")
        .unwrap();
    assert_eq!(prepare(&c, adopted, "successor").unwrap(), ["README.md"]);
    assert_eq!(
        std::fs::read_to_string(c.seat.join("README.md")).unwrap(),
        "keep both\n"
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
    // A non-adopted seat with that conflict refuses with the evidence.
    let refusal = format!(
        "{:#}",
        prepare(&c, ReworkSeat::New, "successor").unwrap_err()
    );
    assert!(
        refusal.contains("cannot resume: only a verified orphan adoption resumes it"),
        "{refusal}"
    );
}

/// The production path: `queue work` moves retained seats aside only under
/// an explicit `--fresh`, BEFORE session start creates the new seat, and
/// carries the same intent into rework preparation. Reverting either wiring
/// fails here; the behavior each wires is proven by the tests above.
#[test]
fn queue_work_wires_fresh_into_move_aside_and_rework_prep() {
    let source = include_str!("../queue_cmd/work.rs")
        .split_once("pub(crate) fn handle_queue_work(")
        .expect("handle_queue_work exists")
        .1
        .split_once("\n}\n")
        .expect("handle_queue_work ends")
        .0;
    let compact: String = source.split_whitespace().collect();
    let move_aside = compact
        .find("crate::seat_clone::move_aside_retained_rework_seats(")
        .expect("queue work moves retained seats aside");
    let start = compact
        .find("crate::session_start::session_start_with_receipt(")
        .expect("queue work starts a session");
    assert!(move_aside < start, "move-aside must precede session start");
    let gate = &compact[..move_aside];
    let gate = &gate[gate.rfind("letmoved_seats=").expect("move-aside gate")..];
    assert!(gate.contains("iffresh&&adopt_orphan.is_none()"), "{gate}");
    assert!(
        compact.contains("rework_seat_for(adopt_orphan.as_ref(),fresh),*pr,&lease.id,&plan.scope,")
    );
    assert!(compact.contains("prompt.push_str(&moved_seats_note(&moved_seats));"));
    let note = crate::queue_cmd::moved_seats_note(&[MovedSeat {
        from: "/w/aida-seat-x".into(),
        to: "/w/aida-seat-x.retained-1".into(),
    }]);
    assert!(note.contains(
        "moved the retained seat /w/aida-seat-x aside intact to /w/aida-seat-x.retained-1"
    ));
}
