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
