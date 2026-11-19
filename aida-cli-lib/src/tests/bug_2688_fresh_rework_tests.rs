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

    let moved =
        move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688).unwrap();
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
        move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688)
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
    let moved =
        move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688).unwrap();
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
    let original = std::fs::metadata(&fx.root).unwrap().permissions();
    std::fs::set_permissions(&fx.root, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A privileged runner ignores directory permissions; nothing to prove.
    let privileged = std::fs::write(fx.root.join("probe"), "").is_ok();
    let result = (!privileged)
        .then(|| move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688));
    std::fs::set_permissions(&fx.root, original).unwrap();
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
    assert!(gate.contains("Some((pr,branch))iffresh_rework=>"), "{gate}");
    let flag = &compact[compact.find("letfresh_rework=").expect("fresh_rework gate")..];
    assert!(
        flag.starts_with("letfresh_rework=rework_pr.is_some()&&fresh&&adopt_orphan.is_none()"),
        "{flag}"
    );
    assert!(
        compact.contains("rework_seat_for(adopt_orphan.as_ref(),fresh),*pr,&lease.id,&plan.scope,")
    );
    assert!(compact.contains("prompt.push_str(&moved_seats_note(&moved_seats));"));
    // Both branch preflights carry the same `--fresh` gate, so a retained
    // linked worktree reaches the move-aside step. trace:BUG-2688 | ai:claude
    assert!(compact.contains("force_claim,false,fresh_rework,)?;"));
    assert!(compact.contains("preflight_queue_work_branch_fresh(&project_root,"));
    assert!(compact.contains("force_claim,fresh_rework,)?;"));
    let note = crate::queue_cmd::moved_seats_note(&[MovedSeat {
        from: "/w/aida-seat-x".into(),
        to: "/w/aida-seat-x.retained-1".into(),
        detached_at: None,
    }]);
    assert!(note.contains(
        "moved the retained seat /w/aida-seat-x aside intact to /w/aida-seat-x.retained-1"
    ));
}

fn move_aside_with_seam(main: &Path, seam: &dyn Fn(&Path)) -> anyhow::Result<Vec<MovedSeat>> {
    move_aside_retained_rework_seats_with(
        main,
        "BUG-2688",
        "bug-2688",
        "feat",
        2688,
        std::time::Duration::from_millis(300),
        seam,
    )
}

/// Review P1 (TOCTOU): a seat replaced between discovery and the rename is
/// never moved. The replacement stays where it is, the inspected seat is
/// untouched, and no pin changes.
#[test]
fn fresh_refuses_a_seat_replaced_after_discovery() {
    let (fx, seat, _) = retained_clone_seat();
    std::fs::write(seat.join("notes.txt"), "old lane\n").unwrap();
    let before = snapshot(&seat);
    let pins = seat_pins(&fx.main);
    let displaced = fx.root.join("displaced-by-other-pickup");
    let error = format!(
        "{:#}",
        move_aside_with_seam(&fx.main, &|path| {
            // Another pickup moves the old seat and provisions a new one
            // at the same path.
            std::fs::rename(path, &displaced).unwrap();
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("new-lane.txt"), "running lane\n").unwrap();
        })
        .unwrap_err()
    );
    assert!(
        error.contains("is no longer the directory that was inspected"),
        "{error}"
    );
    assert!(error.contains("nothing was moved or changed"), "{error}");
    assert_eq!(
        std::fs::read_to_string(seat.join("new-lane.txt")).unwrap(),
        "running lane\n"
    );
    assert_eq!(snapshot(&displaced), before);
    assert_eq!(seat_pins(&fx.main), pins);
}

/// Review P1 (TOCTOU): a seat leased between discovery and the rename is
/// never moved.
#[test]
fn fresh_refuses_a_seat_leased_after_discovery() {
    let (fx, seat, _) = retained_clone_seat();
    let before = snapshot(&seat);
    let pins = seat_pins(&fx.main);
    let main = fx.main.clone();
    let error = format!(
        "{:#}",
        move_aside_with_seam(&fx.main, &|path| {
            record_test_publication_lease(&main, path, "feat");
        })
        .unwrap_err()
    );
    assert!(error.contains("it is now recorded by lease"), "{error}");
    assert_eq!(snapshot(&seat), before);
    assert_eq!(seat_pins(&fx.main), pins);
}

/// Review P1 (serialization): relocation waits for a seat placement in
/// progress (session start holds the lock shared until its lease is
/// written), and refuses with nothing moved if it does not finish.
#[test]
fn fresh_waits_for_seat_placement_in_progress() {
    let (fx, seat, _) = retained_clone_seat();
    let before = snapshot(&seat);
    let placing = SeatPlacementLock::shared(&fx.main).unwrap();
    let error = format!("{:#}", move_aside_with_seam(&fx.main, &|_| {}).unwrap_err());
    assert!(error.contains("seat placement lock"), "{error}");
    assert!(error.contains("nothing was moved or changed"), "{error}");
    assert_eq!(snapshot(&seat), before);
    drop(placing);
    assert_eq!(move_aside_with_seam(&fx.main, &|_| {}).unwrap().len(), 1);
}

/// Review P1 (fail closed): an unparseable lease is unknown ownership, never
/// "unleased": the move refuses with seat bytes and pins unchanged.
#[test]
fn fresh_refuses_on_malformed_lease_evidence() {
    let (fx, seat, _) = retained_clone_seat();
    std::fs::write(seat.join("notes.txt"), "untracked\n").unwrap();
    let before = snapshot(&seat);
    let pins = seat_pins(&fx.main);
    let broken = crate::lease_path(&fx.main, "broken-lease");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(
        &broken,
        "id = \"broken-lease\"\nworktree_path = [unterminated\n",
    )
    .unwrap();
    let error = format!(
        "{:#}",
        move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688)
            .unwrap_err()
    );
    assert!(error.contains("seat ownership is unknown"), "{error}");
    assert!(
        error.contains("retry `aida queue work BUG-2688 --fresh`"),
        "{error}"
    );
    assert_eq!(snapshot(&seat), before);
    assert_eq!(seat_pins(&fx.main), pins);
    std::fs::remove_file(&broken).unwrap();
    assert_eq!(
        move_aside_retained_rework_seats(&fx.main, "BUG-2688", "bug-2688", "feat", 2688)
            .unwrap()
            .len(),
        1
    );
}

/// Review P2 (AC acc34a6d): an unleased, idle linked worktree retained on the
/// PR branch with an unfinished merge and its journal entry passes the
/// pickup's `--fresh` branch preflight, moves aside intact (files, index and
/// MERGE_HEAD unchanged; HEAD detached at the same commit), and a fresh
/// checkout of the PR branch then restarts the conflict merge from origin.
#[test]
fn fresh_moves_retained_linked_worktree_aside_and_restarts_from_origin() {
    let c = conflict_seat();
    assert_eq!(
        prepare(&c, ReworkSeat::New, "old-lane").unwrap(),
        ["README.md"]
    );
    std::fs::write(c.seat.join("README.md"), "half resolved\n").unwrap();
    std::fs::write(c.seat.join("scratch.txt"), "untracked\n").unwrap();
    let admin = c.fx.main.join(".git/worktrees/repair-seat");
    let merge_head = std::fs::read(admin.join("MERGE_HEAD")).unwrap();
    let index = std::fs::read(admin.join("index")).unwrap();
    let before = snapshot(&c.seat);

    // The pickup's branch preflight refuses without --fresh, and lets the
    // unleased idle worktree through to the move-aside step with it.
    let held = format!(
        "{:#}",
        crate::queue_cmd::preflight_queue_work_branch_fresh(
            &c.fx.main,
            Some("feat"),
            None,
            false,
            false
        )
        .unwrap_err()
    );
    assert!(held.contains("is held by worktree"), "{held}");
    crate::queue_cmd::preflight_queue_work_branch_fresh(
        &c.fx.main,
        Some("feat"),
        None,
        false,
        true,
    )
    .unwrap();
    // A leased worktree still refuses under --fresh.
    unproven_lease(&c.fx.main, "old-lane", &c.seat);
    assert!(crate::queue_cmd::preflight_queue_work_branch_fresh(
        &c.fx.main,
        Some("feat"),
        None,
        false,
        true
    )
    .is_err());
    let leased = format!(
        "{:#}",
        move_aside_retained_rework_seats(&c.fx.main, "BUG-2688", "bug-2688", "feat", 2688)
            .unwrap_err()
    );
    assert!(leased.contains("is recorded by lease old-lane"), "{leased}");
    assert_eq!(snapshot(&c.seat), before);
    std::fs::remove_file(crate::lease_path(&c.fx.main, "old-lane")).unwrap();

    let moved =
        move_aside_retained_rework_seats(&c.fx.main, "BUG-2688", "bug-2688", "feat", 2688).unwrap();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0].from, c.seat);
    assert_eq!(moved[0].detached_at.as_deref(), Some(c.head.as_str()));
    let to = moved[0].to.clone();
    assert!(!c.seat.exists());
    assert_eq!(snapshot(&to), before, "moved worktree changed");
    assert_eq!(std::fs::read(admin.join("MERGE_HEAD")).unwrap(), merge_head);
    assert_eq!(std::fs::read(admin.join("index")).unwrap(), index);
    assert_eq!(git_ok(&to, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&to, &["rev-parse", "MERGE_HEAD"]), c.base);
    assert!(
        git_ok(&c.fx.main, &["worktree", "list", "--porcelain"]).contains(&format!(
            "worktree {}\nHEAD {}\ndetached",
            to.display(),
            c.head
        ))
    );

    // A fresh checkout takes the branch at the original path and restarts
    // the merge from the verified remote head; the dead lane's reservation
    // is released.
    git_ok(
        &c.fx.main,
        &["worktree", "add", c.seat.to_str().unwrap(), "feat"],
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(
        prepare(&c, ReworkSeat::Fresh, "new-lane").unwrap(),
        ["README.md"]
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
    assert_eq!(
        std::fs::read_to_string(to.join("README.md")).unwrap(),
        "half resolved\n"
    );
}

/// Review P2 (AC aca42931): a failed remote head fetch on a new seat with no
/// unfinished merge names the missing heads and the one command.
#[test]
fn fetch_failure_names_missing_heads_and_command() {
    let c = conflict_seat();
    git_ok(
        &c.fx.main,
        &["remote", "set-url", "origin", "/nonexistent/origin.git"],
    );
    let error = format!("{:#}", prepare(&c, ReworkSeat::New, "lane").unwrap_err());
    for needle in [
        format!("rework preparation failed for seat {} on branch feat", c.seat.display()),
        "Missing evidence: the verified remote heads origin/main and origin/feat".into(),
        format!(
            "Recovery: once `git -C {} fetch origin main feat` succeeds, retry `aida queue work BUG-2688`",
            c.fx.main.display()
        ),
        "remote head fetch failed".into(),
    ] {
        assert!(error.contains(&needle), "missing {needle:?} in {error}");
    }
    assert!(!error.contains("diagnose"), "{error}");
}

/// Review P2 (AC aca42931): a failed host merge observation names the
/// observation it lacks and the one command.
#[test]
fn merge_observation_failure_names_evidence_and_command() {
    let c = conflict_seat();
    // Drop the base side's README blob: merge-tree cannot read it, while
    // the fetch (already up to date) and the seat's own checkout still work.
    let blob = git_ok(&c.fx.main, &["rev-parse", "origin/main:README.md"]);
    let object =
        c.fx.main
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]);
    std::fs::remove_file(&object).unwrap();
    let error = format!("{:#}", prepare(&c, ReworkSeat::New, "lane").unwrap_err());
    for needle in [
        "host merge observation failed".to_string(),
        format!(
            "Missing evidence: a host merge observation of origin/main {} into feat at {}",
            c.base, c.head
        ),
        format!(
            "Recovery: once `git -C {} merge-tree --write-tree {} {}` reports a tree or conflicts, retry `aida queue work BUG-2688`",
            c.fx.main.display(),
            c.head,
            c.base
        ),
    ] {
        assert!(error.contains(&needle), "missing {needle:?} in {error}");
    }
    // No reservation was created and the seat is unchanged.
    assert!(crate::repair_journal::RepairGuard::lock(&c.fx.main)
        .unwrap()
        .retained_for(&c.seat, "feat", 2688, &c.head)
        .is_empty());
    assert!(git_ok(&c.seat, &["status", "--porcelain"]).is_empty());
}

/// Every tagged preparation stage has evidence and a command; an untagged
/// failure still names one.
#[test]
fn every_preparation_failure_carries_a_recovery_command() {
    let main = Path::new("/w/main");
    let retained = RetainedRework {
        main,
        tree: Path::new("/w/seat"),
        branch: "feat",
        pr: 7,
        lane_id: "lane",
        scope: "BUG-1",
        clone: false,
    };
    let heads = ("b0".to_string(), "h0".to_string());
    for stage in [
        Some(PrepStage::BaseDiscovery),
        Some(PrepStage::HeadsFetch),
        Some(PrepStage::PendingChanges),
        Some(PrepStage::Alignment),
        Some(PrepStage::MergeObservation),
        Some(PrepStage::Merge),
        Some(PrepStage::Regeneration),
        Some(PrepStage::Publication),
        None,
    ] {
        let error = anyhow::anyhow!("inner");
        let error = match stage {
            Some(stage) => error.context(stage),
            None => error,
        };
        let text = retained.preparation_recovery(&error, Some("main"), Some(&heads));
        assert!(text.starts_with("Missing evidence: "), "{stage:?}: {text}");
        assert!(text.contains("Recovery: "), "{stage:?}: {text}");
        assert!(text.contains("`aida queue work BUG-1"), "{stage:?}: {text}");
        assert!(!text.contains("diagnose"), "{stage:?}: {text}");
    }
}
