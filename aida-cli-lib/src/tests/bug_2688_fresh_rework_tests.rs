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
        Some(PrepStage::Alignment(SeatDrift::Behind)),
        Some(PrepStage::Alignment(SeatDrift::Ahead)),
        Some(PrepStage::Alignment(SeatDrift::Diverged)),
        Some(PrepStage::Alignment(SeatDrift::Obstructed)),
        Some(PrepStage::Alignment(SeatDrift::Moved)),
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

/// Publish a commit adding `file` to origin/feat, then return the seat's
/// branch to where it was: the seat is clean and strictly behind origin.
fn advance_origin(c: &ConflictSeat, file: &str) -> String {
    std::fs::write(c.seat.join(file), "origin moved\n").unwrap();
    git_ok(&c.seat, &["add", file]);
    git_ok(&c.seat, &["commit", "-qm", "origin moves"]);
    git_ok(&c.seat, &["push", "-q", "origin", "feat"]);
    let pr_head = git_ok(&c.seat, &["rev-parse", "HEAD"]);
    git_ok(&c.seat, &["reset", "-q", "--hard", &c.head]);
    pr_head
}

fn commit_local(c: &ConflictSeat, file: &str) -> String {
    std::fs::write(c.seat.join(file), "local work\n").unwrap();
    git_ok(&c.seat, &["add", file]);
    git_ok(&c.seat, &["commit", "-qm", "local work"]);
    git_ok(&c.seat, &["rev-parse", "HEAD"])
}

/// The `--fresh` recovery as `queue work` runs it: move the unleased seat
/// aside intact, check the PR branch out again, prepare a fresh seat.
fn recover_fresh(c: &ConflictSeat) -> (PathBuf, Vec<String>) {
    let moved =
        move_aside_retained_rework_seats(&c.fx.main, "BUG-2688", "bug-2688", "feat", 2688).unwrap();
    assert_eq!(moved.len(), 1, "{moved:?}");
    git_ok(
        &c.fx.main,
        &["worktree", "add", c.seat.to_str().unwrap(), "feat"],
    );
    let files = prepare(c, ReworkSeat::Fresh, "recovery-lane").unwrap();
    (moved[0].to.clone(), files)
}

fn assert_alignment(error: &str, state: &str, needles: &[String]) {
    assert!(
        error.contains(&format!("seat alignment to the PR head failed: {state}")),
        "{error}"
    );
    for needle in needles {
        assert!(
            error.contains(needle.as_str()),
            "missing {needle:?} in {error}"
        );
    }
}

/// Review finding 1: a CLEAN adopted seat strictly BEHIND the fetched PR
/// head has nothing to publish. Its refusal says so and names `--fresh`,
/// never a push; that recovery then aligns a new seat to the PR head.
#[test]
fn clean_adopted_seat_behind_origin_recovers_with_fresh_not_push() {
    let c = conflict_seat();
    let pr_head = advance_origin(&c, "extra.txt");
    let error = format!(
        "{:#}",
        prepare(&c, ReworkSeat::Adopted, "successor").unwrap_err()
    );
    assert_alignment(
        &error,
        "the seat is behind it",
        &[
            format!("origin/feat {pr_head}; the seat is behind it and carries no commits origin lacks"),
            "Recovery: after this lane exits, `aida queue work BUG-2688 --fresh` (without --resume)".into(),
        ],
    );
    assert!(!error.contains("push origin"), "{error}");
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&c.seat, &["status", "--porcelain"]), "");

    let (_, files) = recover_fresh(&c);
    assert_eq!(files, ["README.md"]);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), pr_head);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "MERGE_HEAD"]), c.base);
}

/// Review finding 1: a fast-forward refused because the PR head adds a file
/// the seat holds as IGNORED is an obstruction, not unpublished work. The
/// ignored file is never overwritten, the refusal names `--fresh`, and that
/// recovery keeps the ignored file in the moved seat and aligns a new one.
#[test]
fn ignored_file_obstruction_recovers_with_fresh_and_keeps_the_file() {
    let c = conflict_seat();
    let pr_head = advance_origin(&c, "build.log");
    let exclude = c.fx.main.join(".git/info/exclude");
    let mut rules = std::fs::read_to_string(&exclude).unwrap_or_default();
    rules.push_str("build.log\n");
    std::fs::write(&exclude, rules).unwrap();
    std::fs::write(c.seat.join("build.log"), "local ignored\n").unwrap();
    assert_eq!(
        git_ok(&c.seat, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    let error = format!("{:#}", prepare(&c, ReworkSeat::New, "lane").unwrap_err());
    assert_alignment(
        &error,
        "the fast-forward is obstructed",
        &[
            format!("Missing evidence: a fast-forward of clean seat {} to origin/feat {pr_head}", c.seat.display()),
            "would overwrite ignored or untracked files".into(),
            "Recovery: after this lane exits, `aida queue work BUG-2688 --fresh` moves an unleased retained seat aside intact, ignored files included".into(),
        ],
    );
    assert!(!error.contains("push origin"), "{error}");
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(
        std::fs::read_to_string(c.seat.join("build.log")).unwrap(),
        "local ignored\n"
    );

    let (moved, files) = recover_fresh(&c);
    assert_eq!(files, ["README.md"]);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), pr_head);
    assert_eq!(
        std::fs::read_to_string(moved.join("build.log")).unwrap(),
        "local ignored\n"
    );
}

/// Review finding 1: only a seat strictly AHEAD of origin is told to push,
/// and that push clears the refusal.
#[test]
fn seat_ahead_of_origin_recovers_with_push() {
    let c = conflict_seat();
    let local = commit_local(&c, "local.txt");
    let error = format!("{:#}", prepare(&c, ReworkSeat::New, "lane").unwrap_err());
    let push = format!("git -C {} push origin refs/heads/feat", c.fx.main.display());
    assert_alignment(
        &error,
        "the seat is ahead of it",
        &[
            format!(
                "origin/feat {}; its feat is ahead of it with commits origin lacks",
                c.head
            ),
            format!("Recovery: `{push}` publishes them to the PR as a fast-forward"),
        ],
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), local);
    git_ok(&c.fx.main, &["push", "-q", "origin", "refs/heads/feat"]);
    assert_eq!(prepare(&c, ReworkSeat::New, "lane").unwrap(), ["README.md"]);
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), local);
}

/// Review finding 1: a DIVERGED adopted seat is told neither to push (it
/// would be rejected) nor to fast-forward; merging origin into it keeps
/// both sides and the adopted pickup then resumes.
#[test]
fn diverged_adopted_seat_recovers_by_merging_origin_then_resuming() {
    let c = conflict_seat();
    let pr_head = advance_origin(&c, "extra.txt");
    let local = commit_local(&c, "local.txt");
    let error = format!(
        "{:#}",
        prepare(&c, ReworkSeat::Adopted, "successor").unwrap_err()
    );
    assert_alignment(
        &error,
        "the seat has diverged from it",
        &[
            format!("origin/feat {pr_head}; its feat and origin/feat each carry commits the other lacks"),
            format!("`git -C {} merge origin/feat` keeps both sides", c.seat.display()),
            format!(
                "`git -C {} push origin refs/heads/feat` publishes the merge as a fast-forward",
                c.fx.main.display()
            ),
            "then retry `aida queue work BUG-2688 --resume`".into(),
        ],
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), local);

    git_ok(&c.seat, &["merge", "-q", "--no-edit", "origin/feat"]);
    git_ok(&c.fx.main, &["push", "-q", "origin", "refs/heads/feat"]);
    assert_eq!(
        prepare(&c, ReworkSeat::Adopted, "successor").unwrap(),
        ["README.md"]
    );
    for kept in [&local, &pr_head] {
        git_ok(&c.seat, &["merge-base", "--is-ancestor", kept, "HEAD"]);
    }
}

/// A seat clone that is ahead or diverged is never told to run host Git.
#[test]
fn clone_ahead_or_diverged_recovery_runs_no_host_git() {
    let retained = RetainedRework {
        main: Path::new("/w/main"),
        tree: Path::new("/w/seat"),
        branch: "feat",
        pr: 7,
        lane_id: "lane",
        scope: "BUG-1",
        clone: true,
    };
    let heads = ("b0".to_string(), "h0".to_string());
    for drift in [SeatDrift::Ahead, SeatDrift::Diverged] {
        let error = anyhow::anyhow!("inner").context(PrepStage::Alignment(drift));
        let text = retained.preparation_recovery(&error, Some("main"), Some(&heads));
        assert!(!text.contains("git -C"), "{drift:?}: {text}");
        assert!(
            text.contains("`aida queue work BUG-1 --fresh` moves the seat clone aside intact"),
            "{drift:?}: {text}"
        );
    }
}

/// Point contained seats at private clones under `<root>/seats`.
fn clone_seats(main: &Path, root: &Path) {
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&root.join("seats").display().to_string())
        ),
    )
    .unwrap();
}

/// The production session start `queue work` runs for a contained seat.
fn start_seat(
    owns: &str,
    branch: &str,
    path: Option<&Path>,
    reentry: crate::session_start::PathReentry,
) -> anyhow::Result<()> {
    crate::session_start::session_start_with_receipt(
        owns,
        Some(branch),
        None,
        false,
        path.and_then(|p| p.to_str()),
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        /* force_claim */ false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
        /* rollback_failed_setup */ false,
        reentry,
        None,
        /* done_rework_admitted */ false,
    )
    .map(|mut receipt| receipt.disarm())
}

fn relocate(main: &Path, scope: &str, branch: &str) -> anyhow::Result<Vec<MovedSeat>> {
    move_aside_retained_rework_seats_with(
        main,
        scope,
        &crate::slugify(scope),
        branch,
        2688,
        std::time::Duration::from_millis(200),
        &|_| {},
    )
}

/// Attempt a `--fresh` relocation inside the next session start, after its
/// tree is placed or adopted and immediately before its lease is written.
fn relocate_before_lease(
    main: &Path,
    scope: &str,
    branch: &str,
) -> std::rc::Rc<std::cell::RefCell<Option<String>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (main, scope, branch, out) = (
        main.to_path_buf(),
        scope.to_owned(),
        branch.to_owned(),
        seen.clone(),
    );
    failpoint::run_before_lease(move || {
        *out.borrow_mut() = Some(match relocate(&main, &scope, &branch) {
            Ok(moved) => format!("relocation proceeded: {moved:?}"),
            Err(error) => format!("{error:#}"),
        });
    });
    seen
}

fn scope_lease(main: &Path, scope: &str) -> crate::SessionLease {
    let leases: Vec<_> = crate::list_leases(main)
        .into_iter()
        .filter(|l| l.scope == scope)
        .collect();
    assert_eq!(leases.len(), 1, "{leases:?}");
    leases.into_iter().next().unwrap()
}

fn assert_relocation_waited(attempt: &std::cell::RefCell<Option<String>>) {
    let seen = attempt
        .borrow_mut()
        .take()
        .expect("relocation ran between placement and lease publication");
    assert!(seen.contains("seat placement lock"), "{seen}");
    assert!(seen.contains("nothing was moved or changed"), "{seen}");
}

/// Review finding 2 (new placement): production session start holds seat
/// placement from creating the seat until its lease is published. A
/// relocation attempted in that interval waits and refuses with nothing
/// moved; afterwards it sees the published lease and keeps the seat.
/// Removing the shared acquisition, or releasing it before the lease write,
/// lets the relocation proceed and fails this test.
#[test]
fn session_start_holds_placement_until_a_new_seats_lease_is_published() {
    let fx = fixture();
    clone_seats(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let attempt = relocate_before_lease(&fx.main, "TASK-2688", "feat");
    start_seat(
        "TASK-2688",
        "feat",
        None,
        crate::session_start::PathReentry::None,
    )
    .unwrap();
    assert_relocation_waited(&attempt);
    let lease = scope_lease(&fx.main, "TASK-2688");
    let seat = lease.worktree_path.clone();
    assert_eq!(lease.tree_kind, TreeKind::Clone);
    assert!(seat.join(".git").is_dir());
    let before = snapshot(&seat);

    assert!(relocate(&fx.main, "TASK-2688", "feat").unwrap().is_empty());
    assert_eq!(snapshot(&seat), before, "a leased seat was changed");
    // Non-vacuous: this seat is a relocation candidate once unleased.
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();
    let moved = relocate(&fx.main, "TASK-2688", "feat").unwrap();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0].from, seat);
}

/// Review finding 2 (adoption): the same interval for an adopted orphan's
/// seat, from adoption until the successor lease is published.
#[test]
fn session_start_holds_placement_until_an_adopted_seats_lease_is_published() {
    let fx = fixture();
    clone_seats(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    start_seat(
        "TASK-2688",
        "feat",
        None,
        crate::session_start::PathReentry::None,
    )
    .unwrap();
    let orphan = scope_lease(&fx.main, "TASK-2688");
    let seat = orphan.worktree_path.clone();
    orphan_owner(&fx.main, &orphan);
    assert_eq!(
        crate::leases::adoptable_orphan_lease(&fx.main, "TASK-2688").map(|l| l.id),
        Some(orphan.id.clone())
    );
    std::fs::write(seat.join("wip.txt"), "orphan's work\n").unwrap();
    let before = snapshot(&seat);

    let attempt = relocate_before_lease(&fx.main, "TASK-2688", "feat");
    start_seat(
        "TASK-2688",
        "feat",
        Some(&seat),
        crate::session_start::PathReentry::AdoptOrphan,
    )
    .unwrap();
    assert_relocation_waited(&attempt);
    let successor = scope_lease(&fx.main, "TASK-2688");
    assert_ne!(successor.id, orphan.id);
    assert_eq!(successor.worktree_path, seat);
    assert_eq!(snapshot(&seat), before, "the adopted seat was changed");
    assert!(relocate(&fx.main, "TASK-2688", "feat").unwrap().is_empty());
    assert_eq!(snapshot(&seat), before, "a leased seat was changed");
}

/// Record `lease`'s owner as an exited process, through both the legacy PID
/// fields and the canonical ownership heartbeat, so it is an adoptable orphan.
fn orphan_owner(main: &Path, lease: &crate::SessionLease) {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let mut lease = lease.clone();
    lease.active_pid = Some(pid);
    lease.active_pid_start_time = crate::process_probe::process_start_identity(pid);
    std::fs::write(
        crate::lease_path(main, &lease.id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();
    use aida_core::db::DatabaseBackend as _;
    let backend = crate::test_env::home_fixture_backend(&main.join(".aida-store")).unwrap();
    let store = backend.load().unwrap();
    let req = store
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(&lease.scope))
        .unwrap_or_else(|| {
            let mut req = aida_core::Requirement::new("Adoption fixture".into(), String::new());
            req.spec_id = Some(lease.scope.clone());
            req.status = aida_core::RequirementStatus::Approved;
            backend.add_requirement(req).unwrap()
        });
    crate::ownership::reader::seed_lease_heartbeat(
        main,
        &lease,
        crate::ownership::WorkKey::Spec {
            spec: crate::gate::canon::SpecKey(req.id),
        },
        false,
    );
}

/// The pickup fixture: a forge whose only open change is PR-2688 on `feat`
/// at `head`, the queued spec, and a PATH whose only `claude` is a
/// non-executable file. A launching pickup then runs its whole setup and
/// rework preparation and fails at the `exec` of the agent, so the test
/// process is never replaced and no real agent runs.
fn rework_pickup(
    c: &ConflictSeat,
    head: &str,
    ambient: &mut crate::test_env::AmbientGuard,
) -> crate::Storage {
    let main = &c.fx.main;
    crate::test_env::write_project_config_fixture(
        main,
        "store_path = \".aida-store\"\n\n[forge]\nprovider = \"github\"\n",
    );
    let metadata = serde_json::json!({
        "state": "OPEN",
        "title": "fix (BUG-2688)",
        "baseRefName": "main",
        "headRefName": "feat",
        "headRefOid": head,
        "isCrossRepository": false,
    });
    let gh = c.fx.root.join("fake-gh");
    crate::test_exec::write_executable(
        &gh,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\n\
             if [ \"$1 $2\" = 'pr list' ]; then printf '%s\\n' \
             '2688\tfix (BUG-2688)\thttps://example.invalid/2688\tfeat'; exit 0; fi\n\
             if [ \"$1 $2\" = 'pr view' ]; then printf '%s\\n' '{metadata}'; exit 0; fi\n\
             exit 1\n"
        ),
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let bin = c.fx.root.join("no-agent");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("claude"), "not an agent\n").unwrap();
    let env = aida_core::exec_context::Environment::capture();
    let inherited = env.get("PATH").unwrap_or_default().to_os_string();
    let dirs = std::env::split_paths(&inherited).filter(|d| !d.join("claude").exists());
    let path = std::env::join_paths(std::iter::once(bin).chain(dirs)).unwrap();
    ambient.set_key("PATH", &path);
    git_ok(main, &["init", "-q", ".aida-store"]);
    super::orphan_reentry_tests::queued_scope(main, "BUG-2688").0
}

/// `aida queue work BUG-2688 [--resume] [--fresh]` as the CLI runs a
/// launching pickup.
fn queue_work_launch(storage: &crate::Storage, resume: bool, fresh: bool) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some("BUG-2688"),
        None,
        /* sandbox */ false,
        /* no_launch */ false,
        crate::queue_cmd::QueueWorkRoleInput::direct(Some("implementer")),
        /* no_pull */ true,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        false,
        /* force_claim */ false,
        resume.then_some(""),
        fresh,
        false,
        None,
        "claude",
        false,
        crate::phase_driver::AutonomyMode::Default,
        None,
        false,
        false,
        false,
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        None,
        /* over_wip */ false,
        None,
    )
}

fn hermetic(main: &Path) -> crate::test_env::AmbientGuard {
    crate::test_env::AmbientGuard::hermetic_with_seat_and(
        main,
        "implementer",
        &["implementer"],
        &[],
    )
}

/// Round-9 finding (AC1): the behind-origin recovery, run exactly as printed
/// (`aida queue work BUG-2688 --fresh`, without --resume) through
/// `handle_queue_work`, works from the state a failed adoption leaves: the
/// ORIGINAL dead orphan lease restored on the clean, behind-origin seat. The
/// pickup ends that exited lease (only the lease) before branch occupancy is
/// judged, moves the seat aside intact, and prepares a new seat from the PR
/// head. Without the early reclaim the branch preflight refuses the leased
/// worktree and this test fails at the `failed to exec claude` assertion.
#[test]
fn restored_dead_orphan_lease_recovers_through_queue_work_fresh() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let c = conflict_seat();
    let main = c.fx.main.clone();
    let mut ambient = hermetic(&main);
    let pr_head = advance_origin(&c, "extra.txt");
    let storage = rework_pickup(&c, &pr_head, &mut ambient);
    unproven_lease(&main, "01a1deadbeef", &c.seat);
    let orphan = scope_lease(&main, "BUG-2688");
    orphan_owner(&main, &orphan);

    // The adopted pickup of the behind-origin seat refuses, and its rollback
    // restores the original dead orphan lease.
    let refusal = format!(
        "{:#}",
        queue_work_launch(&storage, true, false).unwrap_err()
    );
    assert_eq!(
        crate::leases::adoptable_orphan_lease(&main, "BUG-2688").map(|l| l.id),
        Some(orphan.id.clone()),
        "{refusal}"
    );
    assert_eq!(git_ok(&c.seat, &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&c.seat, &["status", "--porcelain"]), "");

    // The exact recovery command, through the production pickup path. It
    // fails only at the agent exec, after setup and rework preparation.
    let outcome = format!(
        "{:#}",
        queue_work_launch(&storage, false, true).unwrap_err()
    );
    assert!(outcome.contains("failed to exec claude"), "{outcome}");
    assert!(
        !crate::lease_path(&main, &orphan.id).exists(),
        "the exited orphan lease was ended: {outcome}"
    );
    let moved: Vec<PathBuf> = std::fs::read_dir(&c.fx.root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p != &c.seat && p.to_string_lossy().contains("repair-seat"))
        .collect();
    assert_eq!(moved.len(), 1, "{moved:?}: {outcome}");
    assert_eq!(git_ok(&moved[0], &["rev-parse", "HEAD"]), c.head);
    assert_eq!(git_ok(&moved[0], &["status", "--porcelain"]), "");
    // The failed exec keeps the pickup's setup (its cleanup is refused in
    // this fixture): the new lane's seat is at the PR head, merge restarted.
    let new_lease = scope_lease(&main, "BUG-2688");
    assert_ne!(new_lease.id, orphan.id);
    let seat = &new_lease.worktree_path;
    assert_eq!(git_ok(seat, &["rev-parse", "HEAD"]), pr_head, "{outcome}");
    assert_eq!(git_ok(seat, &["rev-parse", "MERGE_HEAD"]), c.base);
}

/// The same recovery never ends a lease whose owner is live or unknown: the
/// pickup refuses before any agent, and the lease and the seat are kept.
#[test]
fn fresh_recovery_keeps_an_unproven_orphan_lease() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let c = conflict_seat();
    let main = c.fx.main.clone();
    let mut ambient = hermetic(&main);
    let pr_head = advance_origin(&c, "extra.txt");
    let storage = rework_pickup(&c, &pr_head, &mut ambient);
    unproven_lease(&main, "01a1unproven0", &c.seat);
    let before = snapshot(&c.seat);

    let error = format!(
        "{:#}",
        queue_work_launch(&storage, false, true).unwrap_err()
    );
    assert!(
        error.contains("ownership reclaim could not end lease 01a1unproven0"),
        "{error}"
    );
    assert!(
        crate::lease_path(&main, "01a1unproven0").exists(),
        "{error}"
    );
    assert_eq!(snapshot(&c.seat), before, "{error}");
}
