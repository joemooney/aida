//! STORY-1690: the merge train. Builder fixtures use a real git repository;
//! the drive engine runs against scripted rails; the regression demo runs the
//! #2599/#2597 shape (a whole-repo guard plus an offender, each green alone)
//! through the dry-run rails and shows main never goes red.
// trace:STORY-1690 | ai:claude

use super::*;
use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(repo: &Path, file: &str, body: &str) {
    let path = repo.join(file);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn commit(repo: &Path, msg: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", msg]);
    git(repo, &["rev-parse", "HEAD"])
}

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn set_line(repo: &Path, file: &str, n: usize, text: &str) {
    let body = std::fs::read_to_string(repo.join(file)).unwrap();
    let mut ls: Vec<String> = body.lines().map(str::to_string).collect();
    ls[n - 1] = text.to_string();
    write(repo, file, &(ls.join("\n") + "\n"));
}

/// A repo whose `main` holds `f.txt` (20 lines).
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    git(r, &["init", "--quiet", "-b", "main"]);
    git(r, &["config", "user.email", "train@example.com"]);
    git(r, &["config", "user.name", "Train Test"]);
    git(r, &["config", "commit.gpgsign", "false"]);
    write(r, "f.txt", &lines(20));
    commit(r, "base");
    tmp
}

/// Branch `name` off main, apply `edit`, commit; back on main. Returns head.
fn pr_branch(r: &Path, name: &str, edit: impl FnOnce(&Path)) -> String {
    git(r, &["checkout", "--quiet", "-b", name, "main"]);
    edit(r);
    let head = commit(r, &format!("work on {name}"));
    git(r, &["checkout", "--quiet", "main"]);
    head
}

fn member(pr: u64, head: &str, subject: &str) -> Member {
    Member {
        pr,
        head: head.to_string(),
        subject: subject.to_string(),
        body: format!("* {subject}"),
    }
}

#[test]
fn build_stacks_one_car_per_member_with_the_ship_subject_and_a_patch_id_proof() {
    let tmp = repo();
    let r = tmp.path();
    let a = pr_branch(r, "a", |r| write(r, "a.txt", "a\n"));
    let b = pr_branch(r, "b", |r| set_line(r, "f.txt", 3, "b was here"));
    let built = build(
        r,
        "main",
        &[
            member(11, &a, "feat: a (TASK-1)"),
            member(12, &b, "fix: b (BUG-2)"),
        ],
    )
    .unwrap();
    assert!(built.dropped.is_empty(), "{:?}", built.dropped);
    assert_eq!(built.cars.len(), 2);
    assert_eq!(built.base, git(r, &["rev-parse", "main"]));
    assert_eq!(built.tip, built.cars[1].commit);
    // One commit per member, oldest first, each subject ending in (#N).
    let log = git(
        r,
        &[
            "log",
            "--format=%s",
            &format!("{}..{}", built.base, built.tip),
        ],
    );
    assert_eq!(log, "fix: b (BUG-2) (#12)\nfeat: a (TASK-1) (#11)");
    let msg = git(r, &["log", "-1", "--format=%B", &built.cars[0].commit]);
    assert!(
        msg.contains(&format!("Aida-Train-Member: #11 at {a}")),
        "{msg}"
    );
    // The tip carries both changes.
    assert_eq!(git(r, &["show", &format!("{}:a.txt", built.tip)]), "a");
    assert!(git(r, &["show", &format!("{}:f.txt", built.tip)]).contains("b was here"));
    // Each car's patch-id is the member's own approved diff.
    for (car, head) in built.cars.iter().zip([&a, &b]) {
        let fork = git(r, &["merge-base", "main", head]);
        let want = crate::review_carryover::patch_id(r, &fork, head).unwrap();
        assert_eq!(car.patch_id, want);
    }
    // Main itself never moved.
    assert_eq!(git(r, &["rev-parse", "main"]), built.base);
}

#[test]
fn build_drops_a_member_that_conflicts_with_an_earlier_one_and_keeps_the_rest() {
    let tmp = repo();
    let r = tmp.path();
    let a = pr_branch(r, "a", |r| set_line(r, "f.txt", 5, "a's line 5"));
    let b = pr_branch(r, "b", |r| set_line(r, "f.txt", 5, "b's line 5"));
    let c = pr_branch(r, "c", |r| write(r, "c.txt", "c\n"));
    let built = build(
        r,
        "main",
        &[member(1, &a, "a"), member(2, &b, "b"), member(3, &c, "c")],
    )
    .unwrap();
    assert_eq!(
        built.cars.iter().map(|c| c.member.pr).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(built.dropped.len(), 1);
    assert_eq!(built.dropped[0].pr, 2);
    assert!(
        built.dropped[0].reason.contains("conflicts"),
        "{:?}",
        built.dropped
    );
}

#[test]
fn build_drops_a_member_whose_diff_changes_when_stacked_even_without_a_conflict() {
    // a edits line 5, b edits line 7: git merges them cleanly, but b's
    // context now differs, so the reviewed diff is not what would land.
    let tmp = repo();
    let r = tmp.path();
    let a = pr_branch(r, "a", |r| set_line(r, "f.txt", 5, "a's line 5"));
    let b = pr_branch(r, "b", |r| set_line(r, "f.txt", 7, "b's line 7"));
    let built = build(r, "main", &[member(1, &a, "a"), member(2, &b, "b")]).unwrap();
    assert_eq!(built.cars.len(), 1);
    assert_eq!(built.dropped[0].pr, 2);
    assert!(
        built.dropped[0].reason.contains("patch-id"),
        "{:?}",
        built.dropped
    );
}

#[test]
fn build_drops_an_empty_member_and_an_unknown_head() {
    let tmp = repo();
    let r = tmp.path();
    let main = git(r, &["rev-parse", "main"]);
    let built = build(
        r,
        "main",
        &[
            member(1, &main, "nothing"),
            member(2, "0123456789abcdef0123456789abcdef01234567", "gone"),
        ],
    )
    .unwrap();
    assert!(built.cars.is_empty());
    assert_eq!(built.tip, built.base);
    assert_eq!(built.dropped.len(), 2);
    assert!(built.dropped[1].reason.contains("not available"));
}

#[test]
fn build_keeps_a_member_behind_main_when_its_own_diff_is_unchanged() {
    let tmp = repo();
    let r = tmp.path();
    let a = pr_branch(r, "a", |r| set_line(r, "f.txt", 2, "a"));
    // main moves on, far from a's hunk.
    set_line(r, "f.txt", 18, "main moved");
    commit(r, "main moves");
    let built = build(r, "main", &[member(1, &a, "a")]).unwrap();
    assert_eq!(built.cars.len(), 1, "{:?}", built.dropped);
    let f = git(r, &["show", &format!("{}:f.txt", built.tip)]);
    assert!(f.contains("main moved") && f.contains("\na\n"), "{f}");
}

#[test]
fn members_line_round_trips_and_malformed_lines_fail_closed() {
    let cars = vec![
        Car {
            member: member(2599, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "x"),
            commit: String::new(),
            patch_id: String::new(),
        },
        Car {
            member: member(2597, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "y"),
            commit: String::new(),
            patch_id: String::new(),
        },
    ];
    let line = members_line(&cars);
    let body = format!("Train of two.\n\n{line}\n");
    assert_eq!(
        parse_members_line(&body).unwrap(),
        vec![(2599, "a".repeat(40)), (2597, "b".repeat(40)),]
    );
    assert_eq!(parse_members_line("no members here"), None);
    assert_eq!(parse_members_line(&format!("{MEMBERS_LINE} ")), None);
    assert_eq!(parse_members_line(&format!("{MEMBERS_LINE} 12@zz")), None);
    assert_eq!(
        parse_members_line(&format!("{MEMBERS_LINE} 12@abcdef0,x@abcdef0")),
        None
    );
}

#[test]
fn car_message_appends_the_pr_number_once() {
    let m = member(7, "abc", "feat: x (TASK-1)");
    assert!(car_message(&m).starts_with("feat: x (TASK-1) (#7)\n\n* feat: x"));
    let m = member(7, "abc", "feat: x (TASK-1) (#7)");
    assert!(car_message(&m).starts_with("feat: x (TASK-1) (#7)\n"));
}

// ───────────────────────────── drive engine ─────────────────────────────

/// Scripted rails: a set of "bad" PRs makes any train containing one red;
/// `flaky_first` makes the very first test red regardless; landing appends
/// to a simulated main. No git.
#[derive(Default)]
struct Script {
    bad: Vec<u64>,
    flaky_first: bool,
    stale_once: bool,
    refuse_at_land: Vec<u64>,
    main: u64,
    tested: Vec<Vec<u64>>,
    landed: Vec<Vec<u64>>,
    told: Vec<u64>,
}

fn prs(ms: &[Member]) -> Vec<u64> {
    ms.iter().map(|m| m.pr).collect()
}

impl Rails for Script {
    fn main_tip(&mut self) -> Result<String> {
        Ok(format!("main{}", self.main))
    }
    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        Ok(Built {
            base: base.to_string(),
            tip: format!("{base}+{:?}", prs(members)),
            cars: members
                .iter()
                .map(|m| Car {
                    member: m.clone(),
                    commit: format!("c{}", m.pr),
                    patch_id: String::new(),
                })
                .collect(),
            dropped: Vec::new(),
        })
    }
    fn test(&mut self, trains: &[Built]) -> Result<Vec<CiVerdict>> {
        Ok(trains
            .iter()
            .map(|t| {
                let ids = prs(&t.members());
                self.tested.push(ids.clone());
                if std::mem::take(&mut self.flaky_first) || ids.iter().any(|p| self.bad.contains(p))
                {
                    CiVerdict::Red("run".into())
                } else {
                    CiVerdict::Green
                }
            })
            .collect())
    }
    fn land(&mut self, train: &Built) -> Result<Landing> {
        if std::mem::take(&mut self.stale_once) {
            self.main += 1;
            return Ok(Landing::Stale);
        }
        let refused: Vec<Ejected> = train
            .members()
            .iter()
            .filter(|m| self.refuse_at_land.contains(&m.pr))
            .map(|m| Ejected {
                pr: m.pr,
                reason: "a hold appeared".into(),
            })
            .collect();
        if !refused.is_empty() {
            self.refuse_at_land.clear();
            return Ok(Landing::Refused(refused));
        }
        if train.base != format!("main{}", self.main) {
            return Ok(Landing::Stale);
        }
        self.main += 1;
        self.landed.push(prs(&train.members()));
        Ok(Landing::Landed(format!("main{}", self.main)))
    }
    fn ejected(&mut self, e: &Ejected) {
        self.told.push(e.pr);
    }
}

fn ms(ids: &[u64]) -> Vec<Member> {
    ids.iter().map(|&p| member(p, "h", "s")).collect()
}

#[test]
fn drive_lands_an_all_green_train_in_one_ci_run() {
    let mut s = Script::default();
    let out = drive(&mut s, ms(&[1, 2, 3, 4])).unwrap();
    assert_eq!(out.ci_runs, 1);
    assert_eq!(s.landed, vec![vec![1, 2, 3, 4]]);
    assert!(out.ejected.is_empty());
}

#[test]
fn drive_reruns_a_first_red_once_before_bisecting() {
    let mut s = Script {
        flaky_first: true,
        ..Default::default()
    };
    let out = drive(&mut s, ms(&[1, 2, 3])).unwrap();
    assert_eq!(s.tested, vec![vec![1, 2, 3], vec![1, 2, 3]]);
    assert_eq!(s.landed, vec![vec![1, 2, 3]]);
    assert_eq!(out.ci_runs, 2);
}

#[test]
fn drive_bisects_a_red_train_and_ejects_only_the_bad_member() {
    let mut s = Script {
        bad: vec![3],
        ..Default::default()
    };
    let out = drive(&mut s, ms(&[1, 2, 3, 4])).unwrap();
    // full, rerun, then halves [1,2] and [3,4] in parallel; [1,2] lands, so
    // [3,4]'s verdict is red and it splits; [3] is ejected, [4] lands.
    assert_eq!(s.tested[0], vec![1, 2, 3, 4]);
    assert_eq!(s.tested[1], vec![1, 2, 3, 4]);
    assert_eq!(s.tested[2..4], [vec![1, 2], vec![3, 4]]);
    assert_eq!(s.landed, vec![vec![1, 2], vec![4]]);
    assert_eq!(out.ejected.len(), 1);
    assert_eq!(out.ejected[0].pr, 3);
    assert!(out.ejected[0].reason.contains("CI is red"));
    assert_eq!(s.told, vec![3]);
}

/// Rails whose CI is red exactly when PRs 1 and 4 ride together (a
/// semantic conflict between them).
struct Pair(Script);

impl Rails for Pair {
    fn main_tip(&mut self) -> Result<String> {
        self.0.main_tip()
    }
    fn build(&mut self, b: &str, m: &[Member]) -> Result<Built> {
        self.0.build(b, m)
    }
    fn test(&mut self, trains: &[Built]) -> Result<Vec<CiVerdict>> {
        Ok(trains
            .iter()
            .map(|t| {
                let ids = prs(&t.members());
                self.0.tested.push(ids.clone());
                if ids.contains(&1) && ids.contains(&4) {
                    CiVerdict::Red("pair".into())
                } else {
                    CiVerdict::Green
                }
            })
            .collect())
    }
    fn land(&mut self, t: &Built) -> Result<Landing> {
        self.0.land(t)
    }
}

#[test]
fn drive_retests_a_green_sibling_after_the_other_half_lands() {
    // Both halves are green in the same wave. The first lands, which makes
    // the second's base stale: it is rebuilt on the new main and tested
    // again, so main only ever receives a combination CI has seen.
    let mut p = Pair(Script::default());
    let out = drive(&mut p, ms(&[1, 2, 3, 4])).unwrap();
    let s = p.0;
    assert_eq!(
        s.tested,
        vec![
            vec![1, 2, 3, 4],
            vec![1, 2, 3, 4],
            vec![1, 2],
            vec![3, 4],
            vec![3, 4],
        ]
    );
    assert_eq!(s.landed, vec![vec![1, 2], vec![3, 4]]);
    assert!(out.ejected.is_empty());
    assert_eq!(out.ci_runs, 5);
}

#[test]
fn drive_rebuilds_a_stale_train_and_drops_members_refused_at_land() {
    let mut s = Script {
        stale_once: true,
        refuse_at_land: vec![2],
        ..Default::default()
    };
    let out = drive(&mut s, ms(&[1, 2, 3])).unwrap();
    // stale → rebuilt and retested; then 2 refused → [1,3] rebuilt, landed.
    assert_eq!(s.tested, vec![vec![1, 2, 3], vec![1, 2, 3], vec![1, 3]]);
    assert_eq!(s.landed, vec![vec![1, 3]]);
    assert_eq!(out.ejected.len(), 1);
    assert_eq!(out.ejected[0].pr, 2);
}

#[test]
fn drive_ejects_a_red_one_member_train_after_its_rerun() {
    let mut s = Script {
        bad: vec![9],
        ..Default::default()
    };
    let out = drive(&mut s, ms(&[9])).unwrap();
    assert_eq!(s.tested.len(), 2);
    assert!(s.landed.is_empty());
    assert_eq!(out.ejected[0].pr, 9);
}

// ───────────────────────────── member gate ──────────────────────────────

fn meta() -> crate::forge::ChangeMetadata {
    crate::forge::ChangeMetadata {
        state: crate::forge::ChangeState::Open,
        title: "t".into(),
        author: None,
        merged_at: None,
        base_ref: "main".into(),
        head_ref: "feature".into(),
        head_sha: "a".repeat(40),
        is_draft: false,
        is_cross_repository: false,
        head_repo: None,
    }
}

#[test]
fn member_gate_refuses_a_draft_with_a_reason_instead_of_waiting() {
    // TASK-1378: a draft never runs Build.
    let m = crate::forge::ChangeMetadata {
        is_draft: true,
        ..meta()
    };
    let why = member_metadata_refusal(&m, "main").unwrap();
    assert!(
        why.contains("draft") && why.contains("gh pr ready"),
        "{why}"
    );
}

#[test]
fn member_gate_refuses_closed_merged_forked_off_base_and_train_prs() {
    use crate::forge::ChangeState;
    assert_eq!(member_metadata_refusal(&meta(), "main"), None);
    for (m, needle) in [
        (
            crate::forge::ChangeMetadata {
                state: ChangeState::Closed,
                ..meta()
            },
            "closed",
        ),
        (
            crate::forge::ChangeMetadata {
                state: ChangeState::Merged,
                ..meta()
            },
            "merged",
        ),
        (
            crate::forge::ChangeMetadata {
                is_cross_repository: true,
                ..meta()
            },
            "fork",
        ),
        (
            crate::forge::ChangeMetadata {
                base_ref: "release".into(),
                ..meta()
            },
            "release",
        ),
        (
            crate::forge::ChangeMetadata {
                head_ref: "train/20261022-1".into(),
                ..meta()
            },
            "train PR",
        ),
    ] {
        let why = member_metadata_refusal(&m, "main").unwrap();
        assert!(why.contains(needle), "{why}");
    }
}

#[test]
fn member_gate_applies_the_merge_hold_gate_label_rules() {
    let c = |hold, recorded, cleared| crate::merge_hold::PinnedChange {
        merged: false,
        state: crate::merge_hold::ChangeState::Open,
        head_sha: None,
        hold_label: hold,
        recorded_label: recorded,
        cleared_label: cleared,
    };
    assert_eq!(member_label_refusal(&c(false, false, false)), None);
    assert_eq!(member_label_refusal(&c(false, true, true)), None);
    assert!(member_label_refusal(&c(true, true, false)).is_some());
    assert!(member_label_refusal(&c(true, true, true)).is_some());
    assert!(member_label_refusal(&c(false, true, false)).is_some());
    assert!(member_label_refusal(&c(false, false, true)).is_some());
}

// ─────────────────────────── regression demo ────────────────────────────

/// The #2599/#2597 shape: main runs every script under `guards/` as CI. PR A
/// adds a whole-repo guard (no file may contain `FORBIDDEN`); PR B adds a
/// file containing it. Each is green alone. Shipped one after the other on
/// their own greens, main goes red; the train lands A, ejects B, and every
/// main it produces is green.
fn guard_ci(dir: &Path) -> std::result::Result<(), String> {
    let guards = dir.join("guards");
    let Ok(entries) = std::fs::read_dir(&guards) else {
        return Ok(());
    };
    for e in entries {
        let guard = std::fs::read_to_string(e.unwrap().path()).unwrap();
        if let Some(word) = guard.trim().strip_prefix("forbid ") {
            for f in std::fs::read_dir(dir.join("src")).unwrap() {
                let p = f.unwrap().path();
                if std::fs::read_to_string(&p).unwrap().contains(word) {
                    return Err(format!("{} contains {word}", p.display()));
                }
            }
        }
    }
    Ok(())
}

// trace:STORY-1690.ac22c696 | ai:claude
#[test]
fn regression_demo_two_green_prs_that_conflict_semantically_never_turn_main_red() {
    let tmp = repo();
    let r = tmp.path();
    write(r, "src/lib.txt", "fine\n");
    commit(r, "src");
    let a = pr_branch(r, "guard", |r| {
        write(r, "guards/no_forbidden", "forbid FORBIDDEN\n")
    });
    let b = pr_branch(r, "offender", |r| write(r, "src/new.txt", "FORBIDDEN\n"));
    let main = git(r, &["rev-parse", "main"]);
    let mut rails = LocalRails {
        repo: r.to_path_buf(),
        main: main.clone(),
        check: Box::new(guard_ci),
        history: Vec::new(),
    };
    // Each PR is green on its own (its CI ran against a main without the other).
    assert_eq!(rails.check_rev(&a).unwrap(), CiVerdict::Green);
    assert_eq!(rails.check_rev(&b).unwrap(), CiVerdict::Green);
    // Today's failure: both merge on their own greens and main goes red.
    let both = build(
        r,
        &main,
        &[member(2597, &a, "guard"), member(2599, &b, "offender")],
    )
    .unwrap();
    assert_eq!(both.cars.len(), 2);
    assert!(matches!(
        rails.check_rev(&both.tip).unwrap(),
        CiVerdict::Red(_)
    ));
    // The train: red, rerun, bisect — A lands, B is ejected before main moves
    // to a red state.
    let out = drive(
        &mut rails,
        vec![member(2597, &a, "guard"), member(2599, &b, "offender")],
    )
    .unwrap();
    assert_eq!(
        out.landed.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        vec![2597]
    );
    assert_eq!(out.ejected.len(), 1);
    assert_eq!(out.ejected[0].pr, 2599);
    assert!(out.ejected[0].reason.contains("CI is red"));
    assert!(!rails.history.is_empty());
    let history = rails.history.clone();
    for state in &history {
        assert_eq!(
            rails.check_rev(state).unwrap(),
            CiVerdict::Green,
            "main went red at {state}"
        );
    }
}

// ───────────────────── ship refuses a stale green ───────────────────────

/// `pr` branched off main, then main moved on; `origin/main` mirrors main.
fn behind_repo() -> tempfile::TempDir {
    let tmp = repo();
    let r = tmp.path();
    pr_branch(r, "pr", |r| write(r, "pr.txt", "pr\n"));
    write(r, "tests/guard.txt", "a new whole-repo guard\n");
    commit(r, "main moves on");
    git(r, &["update-ref", "refs/remotes/origin/main", "main"]);
    tmp
}

// trace:STORY-1690.acb4ffd6 | ai:claude
#[test]
fn ship_refuses_a_head_behind_its_base_and_points_at_the_train() {
    let tmp = behind_repo();
    let r = tmp.path();
    let warning = crate::pr_stale_check_warning(r, "pr", "origin/main");
    let w = warning.as_ref().expect("the head is behind origin/main");
    // Only a test file moved on main: before STORY-1690 this only warned.
    assert!(w.definition_files.is_empty(), "{w:?}");
    let why = crate::pr_cmd::stale_gate_decision(7, "main", warning.as_ref(), false)
        .expect_err("a head behind its base must not ship");
    assert!(why.contains("aida pr train 7"), "{why}");
    assert!(why.contains("1 commit(s) behind main"), "{why}");
    // No override: the flag that once shipped a stale definition does not
    // ship a behind head.
    let why = crate::pr_cmd::stale_gate_decision(7, "main", warning.as_ref(), true)
        .expect_err("--override-stale-check must not ship a behind head");
    assert!(why.contains("aida pr train 7"), "{why}");
}

// trace:STORY-1690.acb4ffd6 | ai:claude
#[test]
fn ship_still_ships_a_head_that_contains_its_base() {
    let tmp = behind_repo();
    let r = tmp.path();
    git(r, &["checkout", "--quiet", "pr"]);
    git(r, &["merge", "--quiet", "--no-edit", "main"]);
    git(r, &["checkout", "--quiet", "main"]);
    let warning = crate::pr_stale_check_warning(r, "pr", "origin/main");
    assert!(warning.is_none(), "{warning:?}");
    assert_eq!(
        crate::pr_cmd::stale_gate_decision(7, "main", warning.as_ref(), false),
        Ok(())
    );
}

// trace:STORY-1690.ac453d5b | ai:claude
#[test]
fn the_template_masters_document_the_train_and_the_ship_refusal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/templates");
    for doc in [
        ".aida/discipline/two-seat-protocol.md",
        ".aida/discipline/git-sync-and-review.md",
    ] {
        let body = std::fs::read_to_string(root.join(doc)).unwrap();
        // Prose is re-wrapped freely; compare it as one line.
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(body.contains("aida pr train"), "{doc} names no train");
        assert!(
            body.contains("refuses a PR whose") && body.contains("behind main"),
            "{doc} does not say ship refuses a behind head"
        );
    }
}

// ───────────────────────── member gate, faked ───────────────────────────

/// Scripted [`MemberChecks`]: every probe passes unless told otherwise, and
/// each records that it ran.
struct FakeChecks {
    meta: crate::forge::ChangeMetadata,
    hold: Option<String>,
    labels: std::result::Result<Option<crate::merge_hold::PinnedChange>, String>,
    pin: Option<String>,
    root: tempfile::TempDir,
    calls: Vec<&'static str>,
}

impl FakeChecks {
    fn new() -> Self {
        FakeChecks {
            meta: meta(),
            hold: None,
            labels: Ok(None),
            pin: Some("a".repeat(40)),
            root: tempfile::tempdir().unwrap(),
            calls: Vec::new(),
        }
    }
}

impl MemberChecks for FakeChecks {
    fn metadata(&mut self, _pr: u64) -> std::result::Result<crate::forge::ChangeMetadata, String> {
        self.calls.push("metadata");
        Ok(self.meta.clone())
    }
    fn hold_marker(&mut self, _pr: u64) -> Option<String> {
        self.calls.push("hold");
        self.hold.clone()
    }
    fn labels(
        &mut self,
        _pr: u64,
    ) -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String> {
        self.calls.push("labels");
        self.labels.clone()
    }
    fn fetch_head(&mut self, _head_ref: &str) -> std::result::Result<(), String> {
        self.calls.push("fetch");
        Ok(())
    }
    fn approval_pin(
        &mut self,
        _pr: u64,
        _head_ref: &str,
        _head: &str,
    ) -> std::result::Result<Option<String>, String> {
        self.calls.push("approval");
        Ok(self.pin.clone())
    }
    fn marker_root(&self) -> &Path {
        self.root.path()
    }
    fn corpus(
        &mut self,
        _pr: u64,
        _hints: &[String],
        _head: &str,
    ) -> std::result::Result<(), String> {
        self.calls.push("corpus");
        Ok(())
    }
    fn message(
        &mut self,
        pr: u64,
        _meta: &crate::forge::ChangeMetadata,
        _head: &str,
    ) -> std::result::Result<(String, String), String> {
        self.calls.push("message");
        Ok((format!("feat: x (#{pr})"), "body".into()))
    }
}

fn claim_for_review(root: &Path, pr: u64, head: &str) {
    let m = crate::review_marker::Marker::for_this_process(pr, Some(head), None, "reviewer seat");
    crate::review_marker::write(root, &m).unwrap();
}

#[test]
fn live_gate_passes_a_clean_member_with_ships_message() {
    let mut c = FakeChecks::new();
    let m = gate_member_with(&mut c, "main", 41).unwrap();
    assert_eq!(m.head, "a".repeat(40));
    assert_eq!(m.subject, "feat: x (#41)");
    assert_eq!(
        c.calls,
        vec!["metadata", "hold", "labels", "fetch", "approval", "corpus", "message"]
    );
}

#[test]
fn live_gate_refuses_a_member_a_reviewer_has_claimed_at_its_head() {
    let mut c = FakeChecks::new();
    claim_for_review(c.root.path(), 41, &"a".repeat(40));
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("under review"), "{why}");
    assert!(!c.calls.contains(&"approval"), "{:?}", c.calls);
}

#[test]
fn live_gate_ignores_a_review_claim_on_another_head() {
    let mut c = FakeChecks::new();
    claim_for_review(c.root.path(), 41, &"b".repeat(40));
    assert!(gate_member_with(&mut c, "main", 41).is_ok());
}

#[test]
fn live_gate_fails_closed_on_an_unknown_head() {
    let mut c = FakeChecks::new();
    c.meta.head_sha = String::new();
    // Even a claim with no recorded head must not be skipped.
    claim_for_review(c.root.path(), 41, "");
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("failing closed"), "{why}");
    assert_eq!(c.calls, vec!["metadata"]);
}

#[test]
fn live_gate_refuses_a_hold_marker_unreadable_labels_and_a_stale_pin() {
    let mut c = FakeChecks::new();
    c.hold = Some("waiting on the operator".into());
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("merge hold"), "{why}");

    let mut c = FakeChecks::new();
    c.labels = Err("gh: 502".into());
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("failing closed"), "{why}");

    let mut c = FakeChecks::new();
    c.pin = Some("c".repeat(40));
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("pinned to"), "{why}");

    let mut c = FakeChecks::new();
    c.pin = None;
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("no approval"), "{why}");
}

// ───────────────────────── land, on a fake forge ────────────────────────

/// Scripted [`TrainForge`]. `mains` answers successive main reads (the last
/// repeats); `gates` overrides a member's re-gate at land.
#[derive(Default)]
struct FakeForge {
    mains: VecDeque<String>,
    gates: std::collections::BTreeMap<u64, std::result::Result<Member, String>>,
    lease_busy: usize,
    /// What main received, as (commit, parent), oldest first.
    on_main: Vec<(String, String)>,
    opened: u64,
    retired: Vec<(u64, String)>,
    merged: Vec<(u64, String)>,
    landed_members: Vec<u64>,
    comments: Vec<u64>,
}

impl FakeForge {
    fn mains(list: &[&str]) -> Self {
        FakeForge {
            mains: list.iter().map(|m| m.to_string()).collect(),
            ..FakeForge::default()
        }
    }
}

impl TrainForge for FakeForge {
    fn fetch_main(&mut self) -> Result<String> {
        Ok(if self.mains.len() > 1 {
            self.mains.pop_front().unwrap()
        } else {
            self.mains.front().cloned().unwrap_or_default()
        })
    }
    fn lease(&mut self, _train_pr: u64) -> std::result::Result<Box<dyn std::any::Any>, String> {
        if self.lease_busy > 0 {
            self.lease_busy -= 1;
            return Err("held by aida pr ship for PR #77".into());
        }
        Ok(Box::new(()))
    }
    fn build(&mut self, base: &str, members: &[Member]) -> Result<Built> {
        Ok(train_of(base, members))
    }
    fn gate(&mut self, pr: u64) -> std::result::Result<Member, String> {
        self.gates
            .get(&pr)
            .cloned()
            .unwrap_or_else(|| Ok(member(pr, &format!("head{pr}"), "s")))
    }
    fn open(&mut self, _train: &Built, branch: &str) -> Result<crate::forge::ChangeRef> {
        self.opened += 1;
        Ok(crate::forge::ChangeRef {
            id: 900 + self.opened,
            url: String::new(),
            branch: branch.to_string(),
            base: "main".into(),
            title: None,
        })
    }
    fn wait_ci(&mut self, _change: &crate::forge::ChangeRef) -> Result<CiVerdict> {
        Ok(CiVerdict::Green)
    }
    fn retire(&mut self, change: &crate::forge::ChangeRef, why: &str) {
        self.retired.push((change.id, why.to_string()));
    }
    fn merge(&mut self, change: &crate::forge::ChangeRef, tip: &str) -> Result<()> {
        self.merged.push((change.id, tip.to_string()));
        Ok(())
    }
    fn landed(&mut self, _base: &str, _main: &str, cars: &[Car]) -> Vec<Option<String>> {
        cars.iter()
            .map(|c| {
                self.on_main
                    .iter()
                    .find(|(sha, _)| sha.ends_with(&c.commit))
                    .map(|(sha, _)| sha.clone())
            })
            .collect()
    }
    fn parent(&mut self, commit: &str) -> Option<String> {
        self.on_main
            .iter()
            .find(|(sha, _)| sha == commit)
            .map(|(_, p)| p.clone())
    }
    fn member_landed(
        &mut self,
        _change: &crate::forge::ChangeRef,
        _train: &Built,
        car: &Car,
        _sha: Option<&str>,
    ) {
        self.landed_members.push(car.member.pr);
    }
    fn comment(&mut self, pr: u64, _note: &str) {
        self.comments.push(pr);
    }
}

fn train_of(base: &str, members: &[Member]) -> Built {
    let cars: Vec<Car> = members
        .iter()
        .map(|m| Car {
            member: m.clone(),
            commit: format!("car{}", m.pr),
            patch_id: format!("pid{}", m.pr),
        })
        .collect();
    Built {
        base: base.to_string(),
        tip: cars.last().map(|c| c.commit.clone()).unwrap_or_default(),
        cars,
        dropped: Vec::new(),
    }
}

fn fms(ids: &[u64]) -> Vec<Member> {
    ids.iter()
        .map(|&p| member(p, &format!("head{p}"), "s"))
        .collect()
}

/// Open `train`'s PR through the rails, as `drive` does before landing.
fn opened(rails: &mut ForgeRails<'_>, train: &Built) {
    assert_eq!(
        rails.test(std::slice::from_ref(train)).unwrap(),
        vec![CiVerdict::Green]
    );
}

#[test]
fn land_merges_a_clean_train_pinned_and_closes_each_member() {
    let mut f = FakeForge::mains(&["m0"]);
    f.on_main = vec![
        ("Lcar1".into(), "m0".into()),
        ("Lcar2".into(), "Lcar1".into()),
    ];
    let train = train_of("m0", &fms(&[1, 2]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    assert_eq!(rails.land(&train).unwrap(), Landing::Landed("m0".into()));
    assert!(rails.warnings.is_empty(), "{:?}", rails.warnings);
    assert_eq!(
        rails.landed_on_main,
        vec![(1, "Lcar1".to_string()), (2, "Lcar2".to_string())]
    );
    assert_eq!(f.merged, vec![(901, "car2".to_string())]);
    assert_eq!(f.landed_members, vec![1, 2]);
    assert!(f.retired.is_empty());
}

#[test]
fn land_refuses_a_member_whose_head_moved_and_retires_the_train() {
    let mut f = FakeForge::mains(&["m0"]);
    f.gates.insert(2, Ok(member(2, "pushed-after-build", "s")));
    let train = train_of("m0", &fms(&[1, 2]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    let Landing::Refused(refused) = rails.land(&train).unwrap() else {
        panic!("a moved head must refuse the land");
    };
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].pr, 2);
    assert!(refused[0].reason.contains("head moved"), "{refused:?}");
    assert!(f.merged.is_empty());
    assert_eq!(f.retired.len(), 1);
    assert!(f.retired[0].1.contains("pre-land recheck"));
}

#[test]
fn land_refuses_a_member_whose_hold_appeared_after_the_build() {
    let mut f = FakeForge::mains(&["m0"]);
    f.gates.insert(
        1,
        Err("it is under a merge hold (operator); the train never releases a hold".into()),
    );
    let train = train_of("m0", &fms(&[1, 2]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    let Landing::Refused(refused) = rails.land(&train).unwrap() else {
        panic!("a new hold must refuse the land");
    };
    assert_eq!(refused[0].pr, 1);
    assert!(refused[0].reason.contains("merge hold"));
    assert!(f.merged.is_empty());
    assert_eq!(f.retired.len(), 1);
}

#[test]
fn land_retires_a_stale_train_when_main_moved_since_the_build() {
    let mut f = FakeForge::mains(&["m1"]);
    let train = train_of("m0", &fms(&[1]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    assert_eq!(rails.land(&train).unwrap(), Landing::Stale);
    assert!(f.merged.is_empty());
    assert_eq!(f.retired.len(), 1);
    assert!(f.retired[0].1.contains("Main moved"));
}

#[test]
fn land_rechecks_main_right_before_the_merge() {
    // Main is the base at the lease, then moves during the re-gate.
    let mut f = FakeForge::mains(&["m0", "m1"]);
    let train = train_of("m0", &fms(&[1, 2]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    assert_eq!(rails.land(&train).unwrap(), Landing::Stale);
    assert!(
        f.merged.is_empty(),
        "a train must not merge onto an untested main"
    );
    assert!(f.retired[0].1.contains("re-checked"));
}

#[test]
fn land_warns_when_the_landed_train_does_not_sit_on_its_tested_base() {
    let mut f = FakeForge::mains(&["m0"]);
    // An interloper landed underneath between the last check and the merge.
    f.on_main = vec![
        ("Lcar1".into(), "interloper".into()),
        ("Lcar2".into(), "Lcar1".into()),
    ];
    let train = train_of("m0", &fms(&[1, 2]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    opened(&mut rails, &train);
    assert!(matches!(rails.land(&train).unwrap(), Landing::Landed(_)));
    assert_eq!(rails.warnings.len(), 1, "{:?}", rails.warnings);
    assert!(
        rails.warnings[0].contains("interloper"),
        "{:?}",
        rails.warnings
    );
}

#[test]
fn lease_contention_rebuilds_the_train_instead_of_aborting_the_run() {
    let mut f = FakeForge::mains(&["m0"]);
    f.lease_busy = 1;
    f.on_main = vec![("Lcar1".into(), "m0".into())];
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1])).unwrap();
    assert_eq!(out.landed, vec![(1, "car1".to_string())]);
    assert!(out.ejected.is_empty(), "{:?}", out.ejected);
    // The contended train PR was retired; a fresh one landed.
    assert_eq!(f.retired.len(), 1);
    assert!(f.retired[0].1.contains("merge lease"));
    assert_eq!(f.merged, vec![(902, "car1".to_string())]);
}
