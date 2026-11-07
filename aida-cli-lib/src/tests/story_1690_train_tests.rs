//! STORY-1690: the merge train. Builder fixtures use a real git repository;
//! the drive engine runs against scripted rails; the regression demo runs the
//! #2599/#2597 shape (a whole-repo guard plus an offender, each green alone)
//! through the dry-run rails and shows main never goes red.
// trace:STORY-1690 | ai:claude

// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
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
        release: None,
    }
}

/// BUG-2214: the cover a fake gate hands a held member.
fn fake_cover() -> crate::gate::mandate_ship::MandateCover {
    crate::gate::mandate_ship::MandateCover {
        id: "6f1c1d6e-0000-4000-8000-00000000fake".into(),
        principal: "joe".into(),
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
        let want = crate::gate::review_carryover::patch_id(r, &fork, head).unwrap();
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
    assert_eq!(member_label_refusal(&c(false, false, false), false), None);
    assert_eq!(member_label_refusal(&c(false, true, true), false), None);
    assert!(member_label_refusal(&c(true, true, false), false).is_some());
    assert!(member_label_refusal(&c(true, true, true), false).is_some());
    assert!(member_label_refusal(&c(false, true, false), false).is_some());
    assert!(member_label_refusal(&c(false, false, true), false).is_some());
    // BUG-2214: with a typed marker under an active mandate the hold label
    // defers to the cover; `recorded` without `cleared` is a live hold's
    // normal state, while `cleared` beside a live hold is inconsistent, and
    // the pair rules still bind once the hold label is gone.
    assert_eq!(member_label_refusal(&c(true, true, false), true), None);
    assert_eq!(member_label_refusal(&c(true, false, false), true), None);
    assert!(member_label_refusal(&c(true, true, true), true).is_some());
    assert!(member_label_refusal(&c(false, true, false), true).is_some());
    assert!(member_label_refusal(&c(false, false, true), true).is_some());
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
    let why = crate::gate::ci_freshness::stale_gate_decision(7, "main", warning.as_ref(), false)
        .expect_err("a head behind its base must not ship");
    assert!(why.contains("aida pr train 7"), "{why}");
    assert!(why.contains("1 commit(s) behind main"), "{why}");
    // No override: the flag that once shipped a stale definition does not
    // ship a behind head.
    let why = crate::gate::ci_freshness::stale_gate_decision(7, "main", warning.as_ref(), true)
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
        crate::gate::ci_freshness::stale_gate_decision(7, "main", warning.as_ref(), false),
        Ok(())
    );
}

/// A clone of a bare origin that has a `pr` branch. Returns (seed repo,
/// the dir holding origin and clone, origin path, clone path).
fn origin_and_clone() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = repo();
    let seed = tmp.path().to_path_buf();
    pr_branch(&seed, "pr", |r| write(r, "pr.txt", "pr\n"));
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    git(
        &seed,
        &["clone", "--quiet", "--bare", ".", &origin.to_string_lossy()],
    );
    let clone = origin.with_file_name("clone");
    git(
        &seed,
        &[
            "clone",
            "--quiet",
            &origin.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    );
    (tmp, dir, origin, clone)
}

// trace:STORY-1690.acb4ffd6 | ai:claude
#[test]
fn the_behind_base_gate_fetches_so_a_main_moved_elsewhere_still_refuses() {
    let (tmp, _dir, origin, clone) = origin_and_clone();
    // The clone's origin/main is current: the gate passes.
    assert_eq!(
        crate::gate::ci_freshness::behind_base_gate_on(&clone, "pr", 7, "main", false),
        Ok(())
    );
    // Another machine moves main on origin; the clone's ref is now stale.
    let seed = tmp.path();
    write(seed, "tests/guard.txt", "a new whole-repo guard\n");
    commit(seed, "main moves on elsewhere");
    git(
        seed,
        &["push", "--quiet", &origin.to_string_lossy(), "main"],
    );
    let (kind, why) =
        crate::gate::ci_freshness::behind_base_gate_on(&clone, "pr", 7, "main", false)
            .expect_err("the gate must fetch and see main moved");
    assert_eq!(kind, crate::gate::ship::ShipRefusal::BehindBase);
    assert!(why.contains("1 commit(s) behind main"), "{why}");
    assert!(why.contains("aida pr train 7"), "{why}");
}

// trace:STORY-1690.acb4ffd6 | ai:claude
#[test]
fn the_behind_base_gate_fails_closed_when_staleness_cannot_be_computed() {
    // No origin at all: nothing can be fetched or compared.
    let tmp = behind_repo();
    let r = tmp.path();
    git(
        r,
        &[
            "remote",
            "add",
            "origin",
            &r.join("missing.git").to_string_lossy(),
        ],
    );
    let (kind, why) = crate::gate::ci_freshness::behind_base_gate_on(r, "pr", 7, "main", false)
        .expect_err("an unknown staleness must refuse");
    assert_eq!(kind, crate::gate::ship::ShipRefusal::BehindBase);
    assert!(why.contains("could not be computed"), "{why}");
    // A missing ref (the head branch never existed) refuses too.
    assert!(crate::pr_stale_check(r, "no-such-branch", "origin/main").is_err());
    assert!(
        crate::gate::ci_freshness::behind_base_refusal(7, "main", Err("boom".into()), true)
            .is_err()
    );
}

// trace:STORY-1690 | ai:claude
#[test]
fn a_behind_head_with_no_definition_change_refuses_as_behind_base() {
    let warning = |definition: Vec<String>| {
        Ok(Some(crate::StaleCheckWarning {
            behind_commits: 2,
            definition_files: definition,
            test_files: Vec::new(),
        }))
    };
    let (kind, _) =
        crate::gate::ci_freshness::behind_base_refusal(7, "main", warning(vec![]), false)
            .unwrap_err();
    assert_eq!(kind, crate::gate::ship::ShipRefusal::BehindBase);
    let (kind, why) = crate::gate::ci_freshness::behind_base_refusal(
        7,
        "main",
        warning(vec![".github/workflows/ci.yml".into()]),
        false,
    )
    .unwrap_err();
    assert_eq!(kind, crate::gate::ship::ShipRefusal::StaleDefinition);
    assert!(why.contains("OLDER definition"), "{why}");
    assert_eq!(
        crate::gate::ci_freshness::behind_base_refusal(7, "main", Ok(None), false),
        Ok(())
    );
}

// trace:STORY-1690 | ai:claude
#[test]
fn the_train_refuses_gitlab_and_pure_git_with_a_reason() {
    use crate::forge::ForgeKind;
    assert_eq!(forge_refusal(ForgeKind::GitHub), None);
    let why = forge_refusal(ForgeKind::GitLab).unwrap();
    assert!(
        why.contains("GitHub only") && why.contains("aida pr ship N"),
        "{why}"
    );
    assert!(forge_refusal(ForgeKind::None).is_some());
}

// trace:STORY-1690 | ai:claude
#[test]
fn naming_more_prs_than_max_is_refused_up_front() {
    let why = explicit_over_max_refusal(3, 2, false).unwrap();
    assert!(why.contains("3 PRs named but --max is 2"), "{why}");
    assert_eq!(explicit_over_max_refusal(2, 2, false), None);
    // --all-ready leaves the rest for the next train instead.
    assert_eq!(explicit_over_max_refusal(30, 2, true), None);
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
    let protocol = std::fs::read_to_string(root.join(".aida/discipline/two-seat-protocol.md"))
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        protocol.contains("Drain-to-train handoff") && protocol.contains("`behind-base`"),
        "two-seat-protocol does not document the drain-to-train handoff"
    );
    assert!(
        protocol.contains("That line is advisory"),
        "two-seat-protocol does not say the members line is advisory"
    );
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
    drive: Option<String>,
    records: Vec<crate::pr_cmd::ShipGateSpecRecord>,
    tty: bool,
    mandate: std::result::Result<crate::gate::mandate_ship::MandateCover, Option<String>>,
    /// BUG-2214: whether the gate defers a held member to the mandate cover.
    mandate_active: bool,
    /// The IDs the spec-policy gates were asked about.
    asked: Vec<String>,
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
            drive: None,
            records: Vec::new(),
            tty: false,
            mandate: Err(None),
            mandate_active: false,
            asked: Vec::new(),
        }
    }

    /// A member shipping `id`, with `tags` and execution `mode`.
    fn shipping(id: &str, tags: &[&str], mode: Option<aida_core::ExecutionMode>) -> Self {
        let mut c = FakeChecks::new();
        c.records = vec![(
            id.to_string(),
            tags.iter().map(|t| t.to_string()).collect(),
            mode,
        )];
        c
    }
}

impl MemberChecks for FakeChecks {
    fn metadata(&mut self, _pr: u64) -> std::result::Result<crate::forge::ChangeMetadata, String> {
        self.calls.push("metadata");
        Ok(self.meta.clone())
    }
    fn covered_ids(&mut self, _pr: u64, _head_ref: &str, ids: Vec<String>) -> Vec<String> {
        ids
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
        Ok((
            format!("feat: x (TASK-9) (#{pr})"),
            "feat: y (TASK-8)\n".into(),
        ))
    }
    fn drive_block(&mut self, _pr: u64) -> Option<String> {
        self.calls.push("drive");
        self.drive.clone()
    }
    fn spec_records(&mut self, ids: &[String]) -> Vec<crate::pr_cmd::ShipGateSpecRecord> {
        self.calls.push("specs");
        self.asked = ids.to_vec();
        self.records.clone()
    }
    fn interactive_tty(&self) -> bool {
        self.tty
    }
    fn mandate_cover(
        &mut self,
        _pr: u64,
        _ids: &[String],
        _head_ref: &str,
        _head: &str,
    ) -> std::result::Result<crate::gate::mandate_ship::MandateCover, Option<String>> {
        self.calls.push("mandate");
        self.mandate.clone()
    }
    fn mandate_active(&self) -> bool {
        self.mandate_active
    }
}

fn claim_for_review(root: &Path, pr: u64, head: &str) {
    let m =
        crate::gate::review_marker::Marker::for_this_process(pr, Some(head), None, "reviewer seat");
    crate::gate::review_marker::write(root, &m).unwrap();
}

#[test]
fn live_gate_passes_a_clean_member_with_ships_message() {
    let mut c = FakeChecks::new();
    let m = gate_member_with(&mut c, "main", 41).unwrap();
    assert_eq!(m.head, "a".repeat(40));
    assert_eq!(m.subject, "feat: x (TASK-9) (#41)");
    assert_eq!(
        c.calls,
        vec![
            "metadata", "drive", "hold", "labels", "fetch", "approval", "corpus", "message",
            "specs"
        ]
    );
}

// ─────────────── ship's spec-policy and drive gates, per member ───────────────

#[test]
fn live_gate_reads_specs_from_the_branch_title_subject_and_commit_subjects() {
    let mut c = FakeChecks::new();
    c.meta.head_ref = "claude/story-77".into();
    c.meta.title = "feat: train (BUG-5)".into();
    gate_member_with(&mut c, "main", 41).unwrap();
    for id in ["STORY-77", "BUG-5", "TASK-9", "TASK-8"] {
        assert!(
            c.asked.iter().any(|a| a.eq_ignore_ascii_case(id)),
            "{id} not in {:?}",
            c.asked
        );
    }
}

#[test]
fn live_gate_refuses_a_pr_a_live_drive_owns() {
    let mut c = FakeChecks::new();
    c.drive = Some("live drive PID 7 (wave w1); TASK-9 phase review".into());
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("live drive owns it"), "{why}");
    assert!(why.contains("PID 7"), "{why}");
    assert_eq!(c.calls, vec!["metadata", "drive"]);
}

#[test]
fn live_gate_refuses_a_review_draft_only_spec_even_for_a_human() {
    let mut c = FakeChecks::shipping(
        "TASK-9",
        &[crate::gate::ship::DRAFT_ONLY_TAG],
        Some(aida_core::ExecutionMode::Drain),
    );
    c.tty = true;
    c.mandate = Ok(fake_cover());
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("TASK-9"), "{why}");
    assert!(why.contains(crate::gate::ship::DRAFT_ONLY_TAG), "{why}");
}

#[test]
fn live_gate_refuses_a_supervised_spec_without_a_human_or_a_mandate() {
    for mode in [
        Some(aida_core::ExecutionMode::Guided),
        Some(aida_core::ExecutionMode::Drive),
        None,
    ] {
        let mut c = FakeChecks::shipping("TASK-9", &[], mode);
        let why = gate_member_with(&mut c, "main", 41).unwrap_err();
        assert!(why.contains("TASK-9 is marked"), "{mode:?}: {why}");
        assert!(why.contains("never auto-merges a supervised spec"), "{why}");
        assert!(c.calls.contains(&"mandate"), "{:?}", c.calls);
    }
}

#[test]
fn live_gate_lands_a_drain_spec_unattended() {
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Drain));
    assert!(gate_member_with(&mut c, "main", 41).is_ok());
    assert!(
        !c.calls.contains(&"mandate"),
        "no mandate read for a drain spec"
    );
}

#[test]
fn live_gate_lets_a_human_at_a_terminal_land_a_supervised_spec() {
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.tty = true;
    assert!(gate_member_with(&mut c, "main", 41).is_ok());
    assert!(!c.calls.contains(&"mandate"), "a human needs no mandate");
}

#[test]
fn live_gate_honours_the_mandate_cover_for_a_supervised_spec() {
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.mandate = Ok(fake_cover());
    assert!(gate_member_with(&mut c, "main", 41).is_ok());

    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.mandate = Err(Some("the mandate excludes TASK-9".into()));
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(
        why.contains("The mandate does not cover it: the mandate excludes TASK-9"),
        "{why}"
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
    // trace:STORY-2401 | ai:codex
    crash_marker: Option<(String, std::path::PathBuf)>,
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
    /// Fail the open with this number (1-based), or every merge.
    open_fails_at: Option<u64>,
    merge_fails: bool,
    /// BUG-2433: what the re-reads after a failed merge call prove, how often
    /// they ran, and a refusal raised before any forge merge call.
    after_merge_error: MergeErrorVerdict,
    rechecks: usize,
    merge_refused: bool,
    ci_unreadable: bool,
    // trace:STORY-2401 | ai:codex
    confirmation: Option<crate::forge_observed::Confirmed>,
    // trace:BUG-2423 | ai:codex
    ci_states: Option<VecDeque<TrainCiReadiness>>,
    /// A non-transient CI-wait error (e.g. head/state changed), once.
    // trace:BUG-2423 | ai:claude
    ci_error: Option<String>,
    // BUG-2214: the land step's hold releases.
    released: Vec<u64>,
    rearmed: Vec<u64>,
    release_fails_for: Option<u64>,
    /// What `hold_gate_green_after_release` answers (fail closed by default).
    gate_green: bool,
    gate_checked_covers: Vec<String>,
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
    fn recover_change(
        &mut self,
        entry: &journal::Entry,
    ) -> Result<(crate::forge::ChangeRef, crate::forge::ChangeState)> {
        Ok((
            crate::forge::ChangeRef {
                id: entry.change_id.unwrap_or(901),
                url: String::new(),
                branch: entry.branch.clone(),
                base: "main".into(),
                title: None,
            },
            if self.confirmation == Some(crate::forge_observed::Confirmed::Positive) {
                crate::forge::ChangeState::Merged
            } else {
                crate::forge::ChangeState::Open
            },
        ))
    }
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
        if self.open_fails_at == Some(self.opened + 1) {
            anyhow::bail!("git push: connection reset");
        }
        self.opened += 1;
        Ok(crate::forge::ChangeRef {
            id: 900 + self.opened,
            url: String::new(),
            branch: branch.to_string(),
            base: "main".into(),
            title: None,
        })
    }
    fn wait_ci(&mut self, _change: &crate::forge::ChangeRef, _train: &Built) -> Result<CiVerdict> {
        if self.ci_unreadable {
            return Err(TrainCiReadError(anyhow::anyhow!("no CI registered within 600s")).into());
        }
        // trace:BUG-2423 | ai:claude
        if let Some(error) = self.ci_error.take() {
            anyhow::bail!(error);
        }
        if let Some(states) = self.ci_states.as_mut() {
            let clock = std::cell::Cell::new(0);
            return wait_train_ci(
                _change.id,
                0,
                || Ok(states.pop_front().unwrap_or(TrainCiReadiness::Unregistered)),
                || clock.get(),
                || clock.set(clock.get() + TRAIN_CI_POLL_SECS),
            );
        }
        Ok(CiVerdict::Green)
    }
    // trace:BUG-2214 | ai:claude
    fn release_hold(
        &mut self,
        member: &Member,
    ) -> std::result::Result<crate::gate::merge_hold_core::MergeHoldRecord, String> {
        if self.release_fails_for == Some(member.pr) {
            return Err("the mandate is no longer active".into());
        }
        self.released.push(member.pr);
        Ok(crate::gate::merge_hold_core::typed_hold(
            member.pr,
            crate::gate::merge_hold_core::HoldReasonKind::Supervision,
            "supervised spec",
            None,
        ))
    }
    fn rearm_hold(&mut self, record: &crate::gate::merge_hold_core::MergeHoldRecord) -> String {
        self.rearmed.push(record.pr);
        format!(" PR-{}'s merge-hold is re-armed.", record.pr)
    }
    fn hold_gate_green_after_release(
        &mut self,
        _change: &crate::forge::ChangeRef,
        covers: &[String],
    ) -> Result<bool> {
        self.gate_checked_covers = covers.to_vec();
        Ok(self.gate_green)
    }
    fn confirm_ci_refusal(&mut self, _change: &crate::forge::ChangeRef, _train: &Built) -> bool {
        true
    }
    fn confirm_not_landed(
        &mut self,
        _change: &crate::forge::ChangeRef,
        _train: &Built,
    ) -> crate::forge_observed::Confirmed {
        if let Some((stage, marker)) = &self.crash_marker {
            if stage == "retire"
                && journal::entries(marker.parent().unwrap())
                    .unwrap()
                    .iter()
                    .any(|e| matches!(e.step, journal::Step::Retire { .. }))
            {
                crash_ready(marker, "eject_decided");
            }
        }
        self.confirmation
            .unwrap_or(crate::forge_observed::Confirmed::Negative)
    }
    fn confirm_member_head(&mut self, _pr: u64, _head: &str) -> bool {
        true
    }
    fn retire(&mut self, change: &crate::forge::ChangeRef, why: &str) {
        self.retired.push((change.id, why.to_string()));
    }
    fn merge(&mut self, change: &crate::forge::ChangeRef, tip: &str) -> Result<()> {
        // trace:BUG-2433 | ai:claude
        if self.merge_refused {
            anyhow::bail!("train is under review");
        }
        if self.merge_fails {
            return Err(anyhow::anyhow!("gh pr merge: HTTP 502").context(ForgeMergeCallFailed));
        }
        if let Some((stage, marker)) = &self.crash_marker {
            if stage == "merge" {
                crash_ready(marker, "merged");
            }
        }
        self.merged.push((change.id, tip.to_string()));
        Ok(())
    }
    // trace:BUG-2433 | ai:claude
    fn merged_despite_error(
        &mut self,
        change: &crate::forge::ChangeRef,
        train: &Built,
    ) -> MergeErrorVerdict {
        self.rechecks += 1;
        if self.after_merge_error == MergeErrorVerdict::Landed {
            self.merged.push((change.id, train.tip.clone()));
        }
        self.after_merge_error
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

#[test]
fn a_merge_error_retires_the_open_train_and_tells_every_member() {
    let mut f = FakeForge::mains(&["m0"]);
    f.merge_fails = true;
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    let error = out.error.clone().expect("the error is in the outcome");
    assert!(error.contains("HTTP 502"), "{error}");
    assert!(out.landed.is_empty());
    assert_eq!(
        out.ejected.iter().map(|e| e.pr).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        out.ejected[0].reason.contains("stopped"),
        "{:?}",
        out.ejected
    );
    assert_eq!(f.retired.len(), 1, "{:?}", f.retired);
    assert_eq!(f.retired[0].0, 901);
    assert!(f.retired[0].1.contains("HTTP 502"), "{:?}", f.retired);
    assert_eq!(f.comments, vec![1, 2]);
}

#[test]
fn story_2401_open_error_retains_members_for_ambiguous_publication() {
    let mut f = FakeForge::mains(&["m0"]);
    f.open_fails_at = Some(1);
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1])).unwrap();
    assert!(out.error.as_deref().unwrap().contains("connection reset"));
    assert!(out.ejected.is_empty());
    assert!(f.retired.is_empty(), "nothing was open");
    assert!(f.comments.is_empty(), "no member ejection notice");
}

#[test]
fn an_unreadable_ci_stops_the_run_instead_of_bisecting_it_as_red() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_unreadable = true;
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2, 3])).unwrap();
    assert!(out.landed.is_empty());
    let error = out.error.clone().expect("the CI error is in the outcome");
    assert!(error.contains("no CI registered"), "{error}");
    assert_eq!(f.opened, 1, "no rerun or bisect on an infrastructure error");
    assert!(
        out.ejected.is_empty(),
        "unreadable CI does not incriminate members"
    );
    assert!(f.retired.is_empty(), "the train stays open for inspection");
    assert_eq!(f.comments, [901], "only the train receives the pause note");
}

// ─────────────────────────── round-4 review fixes ───────────────────────────

/// The bash gate in merge-hold-gate.yml is the binding members-line parser;
/// `parse_members_line` must accept and refuse exactly the cases it does.
/// tests/test_merge_hold_gate.sh runs the same fixture through the gate.
#[test]
fn parse_members_line_agrees_with_the_bash_gate_on_the_shared_fixture() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/train-members-lines.txt");
    let text = std::fs::read_to_string(fixture).unwrap();
    let mut cases = 0;
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (want, value) = line.split_once('\t').expect("<want>\\t<value>");
        let value = value.replace("<CR>", "\r");
        let body = format!("Train.\n\n{MEMBERS_LINE} {value}");
        let accepted = parse_members_line(&body).is_some();
        assert_eq!(accepted, want == "0", "members line {value:?}");
        cases += 1;
    }
    assert!(cases >= 10, "the fixture read only {cases} cases");
}

/// The reviewer's case: title `feat: X` with no ID, branch `claude/story-1`,
/// head commit `feat: X (STORY-1)`. Ship's derivation equals the commit
/// subject and returns `None`; the car must keep `(STORY-1)`.
#[test]
fn a_car_keeps_the_head_commits_spec_id_when_ship_derives_no_subject() {
    let commit = "feat: X (STORY-1)\n\nthe work";
    let tmp = repo_with_specs(&["STORY-1"]);
    // Ship's own derivation (TASK-2272: driven, not re-implemented): equal to
    // the commit's subject, so it returns None and leaves it to the merge.
    let (returned, _) = crate::pr_cmd::derive_pr_ship_squash_subject_from_message(
        tmp.path(),
        &ship_meta("feat: X"),
        "claude/story-1",
        commit,
        false,
    )
    .unwrap();
    assert_eq!(returned, None, "the scenario must reach the fallback");
    let subject = member_squash_subject(returned, commit, "feat: X");
    assert_eq!(subject, "feat: X (STORY-1)");
    let car = car_message(&Member {
        pr: 9,
        head: "a".repeat(40),
        subject,
        body: String::new(),
        release: None,
    });
    let first = car.lines().next().unwrap();
    assert!(
        first.contains("(STORY-1)") && first.ends_with("(#9)"),
        "{first}"
    );
    // A derived subject still wins, and an unreadable head falls to the title.
    assert_eq!(
        member_squash_subject(Some("s (TASK-1)".into()), commit, "t"),
        "s (TASK-1)"
    );
    assert_eq!(member_squash_subject(None, "", "feat: X"), "feat: X");
}

/// `main` plus an `.aida-store` holding `ids`, so ship's derivation can
/// recognise them.
fn repo_with_specs(ids: &[&str]) -> tempfile::TempDir {
    let tmp = repo();
    let requirements = ids
        .iter()
        .map(|id| {
            let mut req = aida_core::Requirement::new(id.to_string(), String::new());
            req.spec_id = Some(id.to_string());
            req.req_type = aida_core::RequirementType::Story;
            req.status = aida_core::RequirementStatus::InProgress;
            req
        })
        .collect();
    let backend = crate::test_env::home_fixture_backend(&tmp.path().join(".aida-store")).unwrap();
    aida_core::DatabaseBackend::save(
        &backend,
        &aida_core::RequirementsStore {
            requirements,
            ..Default::default()
        },
    )
    .unwrap();
    tmp
}

fn ship_meta(title: &str) -> crate::pr_cmd::PrShipMetadata {
    crate::pr_cmd::PrShipMetadata {
        title: title.to_string(),
        body: String::new(),
        number: None,
    }
}

/// TASK-2272 (TASK-2271): the merge seat's checkout holds a stale local
/// branch of the same name. With no usable title (the GitLab metadata path)
/// the subject comes from the commit, and that commit is the gated head sha,
/// so the car carries its `(STORY-1)`, never the stale `(STORY-2)`.
// trace:TASK-2272 | ai:claude
#[test]
fn a_car_reads_its_subject_at_the_approved_head_not_a_stale_local_branch() {
    let tmp = repo_with_specs(&["STORY-1", "STORY-2"]);
    let r = tmp.path();
    git(r, &["checkout", "--quiet", "-b", "feature-x", "main"]);
    write(r, "f.txt", "old\n");
    let stale = commit(r, "feat: old (STORY-2)");
    write(r, "f.txt", "new\n");
    let head = commit(r, "feat: X (STORY-1)");
    git(r, &["checkout", "--quiet", "main"]);
    git(r, &["branch", "--quiet", "-f", "feature-x", &stale]);

    let subject = member_subject_at_head(r, r, &ship_meta(""), "feature-x", &head).unwrap();
    assert!(subject.contains("(STORY-1)"), "{subject}");
    assert!(!subject.contains("STORY-2"), "{subject}");
    // The branch-based read this replaces lands on the stale commit.
    let (derived, _) =
        crate::pr_cmd::derive_pr_ship_squash_subject_from(r, &ship_meta(""), "feature-x", false)
            .unwrap();
    let branch_msg = crate::pr_cmd::branch_head_commit_message(r, "feature-x").unwrap();
    let old = member_squash_subject(derived, &branch_msg, "");
    assert!(
        old.contains("STORY-2"),
        "the fixture must reproduce the bug: {old}"
    );

    // An unreadable head fails closed instead of falling back to the branch.
    let err = member_subject_at_head(r, r, &ship_meta("feat: X"), "feature-x", &"0".repeat(40))
        .unwrap_err();
    assert!(err.contains("could not be read"), "{err}");
}

/// TASK-2272 (TASK-1-468): the PRs named on the command line are the ones a
/// terminal's consent covers; `--all-ready` names none.
// trace:TASK-2272 | ai:claude
#[test]
fn only_prs_named_on_the_command_line_get_terminal_consent() {
    let opts = |all_ready| TrainOptions {
        prs: &[1],
        all_ready,
        dry_run: true,
        local_ci: None,
        max: 10,
        no_pull: true,
    };
    assert!(prs_named_on_command_line(&opts(false)));
    assert!(!prs_named_on_command_line(&opts(true)));
    assert!(tty_consents(true, prs_named_on_command_line(&opts(false))));
    assert!(!tty_consents(true, prs_named_on_command_line(&opts(true))));
}

#[test]
fn the_train_refuses_inside_a_standalone_implementer() {
    crate::standalone_implementer::TEST_MARKER
        .with(|m| *m.borrow_mut() = Some("01lease".to_string()));
    let err = pr_train_handler(&TrainOptions {
        prs: &[1],
        all_ready: false,
        dry_run: true,
        local_ci: None,
        max: 10,
        no_pull: true,
    })
    .unwrap_err();
    crate::standalone_implementer::TEST_MARKER.with(|m| *m.borrow_mut() = None);
    let text = format!("{err:#}");
    assert!(text.contains("`aida pr train` is refused"), "{text}");
    assert!(
        text.contains(crate::standalone_implementer::MARKER_ENV),
        "{text}"
    );
}

/// A human at a terminal consents only to the PRs they named; one swept in
/// by `--all-ready` needs a mandate for a supervised spec.
#[test]
fn the_terminal_bypass_covers_named_prs_only() {
    assert!(tty_consents(true, true));
    assert!(
        !tty_consents(true, false),
        "--all-ready gets no TTY consent"
    );
    assert!(!tty_consents(false, true));
    // What the gate sees for a swept-in supervised member with a human at
    // the terminal and no mandate: refused.
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.tty = tty_consents(true, false);
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("TASK-9 is marked"), "{why}");
    // The same member, swept in under a covering mandate, lands.
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.tty = tty_consents(true, false);
    c.mandate = Ok(fake_cover());
    assert!(gate_member_with(&mut c, "main", 41).is_ok());
}

// ── BUG-2214: the train releases a member's hold under the mandate ──────────

/// A member admitted with a hold to release at land time.
fn covered_member(pr: u64) -> Member {
    let mut m = member(pr, &format!("head{pr}"), "s");
    m.release = Some(fake_cover());
    m
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_gate_defers_a_held_member_to_the_mandate_cover() {
    // No active mandate: the refusal is byte-identical to before.
    let mut c = FakeChecks::new();
    c.hold = Some("supervised spec needs a human".into());
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert_eq!(
        why,
        "it is under a merge hold (supervised spec needs a human); the train never releases a hold"
    );
    assert!(
        !c.calls.contains(&"mandate"),
        "no cover read without a mandate"
    );

    // Active covering mandate: admitted, carrying the cover for the land step.
    let mut c = FakeChecks::new();
    c.hold = Some("supervised spec needs a human".into());
    c.mandate_active = true;
    c.mandate = Ok(fake_cover());
    let m = gate_member_with(&mut c, "main", 41).unwrap();
    assert_eq!(m.release, Some(fake_cover()));

    // Active mandate whose cover refuses: the hold stays, the reason names both.
    let mut c = FakeChecks::new();
    c.hold = Some("supervised spec needs a human".into());
    c.mandate_active = true;
    c.mandate = Err(Some("TASK-9 is in an excluded class".into()));
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("it is under a merge hold"), "{why}");
    assert!(why.contains("TASK-9 is in an excluded class"), "{why}");
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_one_cover_read_serves_the_supervised_gate_and_the_hold() {
    let mut c = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    c.hold = Some("supervised spec".into());
    c.mandate_active = true;
    c.mandate = Ok(fake_cover());
    let m = gate_member_with(&mut c, "main", 41).unwrap();
    assert_eq!(m.release, Some(fake_cover()));
    assert_eq!(c.calls.iter().filter(|s| **s == "mandate").count(), 1);
}

// trace:BUG-2214.acd6c45c | ai:claude
#[test]
fn bug_2214_a_label_only_hold_stays_human_even_under_a_mandate() {
    let mut c = FakeChecks::new();
    c.mandate_active = true;
    c.mandate = Ok(fake_cover());
    c.labels = Ok(Some(crate::merge_hold::PinnedChange {
        merged: false,
        state: crate::merge_hold::ChangeState::Open,
        head_sha: None,
        hold_label: true,
        recorded_label: false,
        cleared_label: false,
    }));
    let why = gate_member_with(&mut c, "main", 41).unwrap_err();
    assert!(why.contains("the train never releases a hold"), "{why}");
}

// trace:BUG-2214.acd6c45c | ai:claude
#[test]
fn bug_2214_train_ci_tolerates_only_a_covered_hold_gate_red() {
    use crate::ci_gate::CheckRow;
    let row = |name: &str, bucket: &str| CheckRow {
        name: name.into(),
        workflow: "w".into(),
        bucket: bucket.into(),
        run_id: None,
    };
    let pinned = |hold: bool, recorded: bool, cleared: bool| crate::merge_hold::PinnedChange {
        merged: false,
        state: crate::merge_hold::ChangeState::Open,
        head_sha: None,
        hold_label: hold,
        recorded_label: recorded,
        cleared_label: cleared,
    };
    let car = |m: &Member| Car {
        member: m.clone(),
        commit: "c".into(),
        patch_id: "p".into(),
    };
    let covered = vec![car(&covered_member(7))];
    let plain = vec![car(&member(7, "head7", "s"))];
    let red = vec![
        row("build", "pass"),
        row(crate::ci_gate::HOLD_GATE_CHECK, "fail"),
    ];
    // The one tolerated shape: only the hold gate is red, the member is
    // admitted for release, and its live labels show exactly a live hold.
    assert!(hold_gate_tolerance(&red, &covered, |_| Ok(Some(pinned(
        true, true, false
    )))));
    // A held member the mandate does not cover stays red.
    assert!(!hold_gate_tolerance(&red, &plain, |_| Ok(Some(pinned(
        true, true, false
    )))));
    // Any other red check stays red.
    let two_red = vec![
        row("build", "fail"),
        row(crate::ci_gate::HOLD_GATE_CHECK, "fail"),
    ];
    assert!(!hold_gate_tolerance(&two_red, &covered, |_| Ok(Some(
        pinned(true, true, false)
    ))));
    // A BUG-1693 pair-rule failure stays red.
    assert!(!hold_gate_tolerance(&red, &covered, |_| Ok(Some(pinned(
        false, true, false
    )))));
    // Unreadable or missing labels fail closed.
    assert!(!hold_gate_tolerance(&red, &covered, |_| Err(
        "api down".into()
    )));
    assert!(!hold_gate_tolerance(&red, &covered, |_| Ok(None)));
    // A green hold gate is not this tolerance's business.
    let green = vec![row(crate::ci_gate::HOLD_GATE_CHECK, "pass")];
    assert!(!hold_gate_tolerance(&green, &covered, |_| Ok(Some(
        pinned(false, false, false)
    ))));
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_land_releases_covered_holds_then_merges() {
    let covered = covered_member(7);
    let mut f = FakeForge::mains(&["base"]);
    f.gates.insert(7, Ok(covered.clone()));
    f.gate_green = true;
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![covered]).unwrap();
    assert_eq!(
        out.landed.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        [7]
    );
    assert_eq!(f.released, [7]);
    assert_eq!(f.gate_checked_covers, [fake_cover().id]);
    assert!(f.rearmed.is_empty(), "{:?}", f.rearmed);
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_a_failed_release_rearms_the_already_released_and_ejects() {
    let (seven, eight) = (covered_member(7), covered_member(8));
    let mut f = FakeForge::mains(&["base"]);
    f.gates.insert(7, Ok(seven.clone()));
    f.gates.insert(8, Ok(eight.clone()));
    f.release_fails_for = Some(8);
    f.gate_green = true;
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![seven, eight]).unwrap();
    let ejected: Vec<_> = out.ejected.iter().filter(|e| e.pr == 8).collect();
    assert_eq!(ejected.len(), 1, "{:?}", out.ejected);
    assert!(
        ejected[0]
            .reason
            .contains("its merge-hold was not released"),
        "{}",
        ejected[0].reason
    );
    assert!(
        ejected[0].reason.contains("re-armed"),
        "{}",
        ejected[0].reason
    );
    // PR-7's hold was released before PR-8 failed, re-armed, then released
    // again when PR-7 landed alone.
    assert_eq!(
        out.landed.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        [7]
    );
    assert!(f.rearmed.contains(&7), "{:?}", f.rearmed);
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_a_red_hold_gate_after_release_rearms_and_rebuilds() {
    let covered = covered_member(7);
    let mut f = FakeForge::mains(&["base"]);
    f.gates.insert(7, Ok(covered.clone()));
    // gate_green stays false: the gate never re-runs green after the release.
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![covered]).unwrap();
    assert!(out.landed.is_empty(), "{:?}", out.landed);
    assert!(!f.released.is_empty());
    assert_eq!(f.rearmed, f.released, "every release was re-armed");
    assert!(f.merged.is_empty(), "{:?}", f.merged);
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_main_moving_after_the_release_rearms_then_lands_on_rebuild() {
    let covered = covered_member(7);
    let mut f = FakeForge::mains(&["base", "base", "moved"]);
    f.gates.insert(7, Ok(covered.clone()));
    f.gate_green = true;
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![covered]).unwrap();
    assert_eq!(
        out.landed.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        [7]
    );
    assert_eq!(f.released, [7, 7], "released, re-armed, released again");
    assert_eq!(f.rearmed, [7]);
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_a_merge_error_after_the_forge_merged_lands_every_member() {
    let mut f = FakeForge::mains(&["m0"]);
    f.merge_fails = true;
    f.after_merge_error = MergeErrorVerdict::Landed;
    f.on_main = vec![
        ("Lcar1".into(), "m0".into()),
        ("Lcar2".into(), "Lcar1".into()),
    ];
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(out.ejected.is_empty(), "{:?}", out.ejected);
    // drive() records each car's built commit; the rails record what main got.
    assert_eq!(
        out.landed,
        vec![(1, "car1".to_string()), (2, "car2".to_string())]
    );
    assert_eq!(
        rails.landed_on_main,
        vec![(1, "Lcar1".to_string()), (2, "Lcar2".to_string())]
    );
    assert!(
        rails
            .warnings
            .iter()
            .any(|w| w.contains("HTTP 502") && w.contains("landed")),
        "{:?}",
        rails.warnings
    );
    drop(rails);
    assert_eq!(f.landed_members, vec![1, 2], "members close with pointers");
    assert!(f.retired.is_empty(), "{:?}", f.retired);
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_a_merged_hold_release_is_not_rearmed() {
    let covered = covered_member(7);
    let mut f = FakeForge::mains(&["base"]);
    f.gates.insert(7, Ok(covered.clone()));
    f.gate_green = true;
    f.merge_fails = true;
    f.after_merge_error = MergeErrorVerdict::Landed;
    f.on_main = vec![("Lcar7".into(), "base".into())];
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![covered]).unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(
        out.landed.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        [7]
    );
    assert!(
        f.rearmed.is_empty(),
        "a landed member's hold stays released"
    );
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_merged_at_tip_reads_the_post_error_state() {
    use crate::forge::ChangeState::*;
    assert_eq!(merged_at_tip(Some((&Merged, "tip")), "tip"), Some(true));
    assert_eq!(merged_at_tip(Some((&Merged, "other")), "tip"), Some(false));
    assert_eq!(merged_at_tip(Some((&Closed, "tip")), "tip"), Some(false));
    assert_eq!(merged_at_tip(Some((&Open, "tip")), "tip"), None);
    assert_eq!(merged_at_tip(None, "tip"), None);
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_main_carries_tip_needs_a_moved_main_with_the_tip_tree() {
    // The forge-silent proof: main moved off the base and holds the tip tree.
    assert!(main_carries_tip("m1", "m0", Some("T"), Some("T")));
    // Main never moved: nothing landed, whatever the trees say.
    assert!(!main_carries_tip("m0", "m0", Some("T"), Some("T")));
    // Main moved, but to a different tree.
    assert!(!main_carries_tip("m1", "m0", Some("T"), Some("U")));
    // An unreadable tree proves nothing, even on both sides.
    assert!(!main_carries_tip("m1", "m0", None, None));
    assert!(!main_carries_tip("m1", "m0", None, Some("T")));
}

/// BUG-2433: run `judge_merge_error` over scripted reads; report the verdict,
/// how many reads it took, and whether main was consulted.
fn judge(
    script: Vec<Option<(crate::forge::ChangeState, &str)>>,
    on_main: bool,
) -> (MergeErrorVerdict, u32, bool) {
    let mut reads = 0;
    let mut consulted = false;
    let verdict = judge_merge_error(
        3,
        "tip",
        |attempt| {
            reads += 1;
            script[attempt as usize].map(|(s, h)| (s, h.to_string()))
        },
        || {
            consulted = true;
            on_main
        },
    );
    (verdict, reads, consulted)
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_judge_merge_error_trusts_the_forge_and_fails_closed() {
    use crate::forge::ChangeState::*;
    use MergeErrorVerdict::*;
    // A definite forge answer stops the reads; main is never consulted.
    assert_eq!(
        judge(vec![None, Some((Merged, "tip")), None], false),
        (Landed, 2, false)
    );
    assert_eq!(
        judge(vec![Some((Merged, "other")), None, None], true),
        (NotLanded, 1, false)
    );
    assert_eq!(
        judge(vec![Some((Closed, "tip")), None, None], true),
        (NotLanded, 1, false)
    );
    // Forge silent on every read: main's tree is the evidence.
    assert_eq!(judge(vec![None, None, None], true), (Landed, 3, true));
    assert_eq!(judge(vec![None, None, None], false), (NotLanded, 3, true));
    // The forge answered open: main never overrides it. A matching tree is
    // a contradiction for the advisor, not a landing.
    assert_eq!(
        judge(vec![Some((Open, "tip")); 3], true),
        (OpenButOnMain, 3, true)
    );
    assert_eq!(
        judge(vec![None, Some((Open, "tip")), None], true),
        (OpenButOnMain, 3, true)
    );
    assert_eq!(
        judge(vec![Some((Open, "tip")); 3], false),
        (NotLanded, 3, true)
    );
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_open_but_on_main_is_reported_and_not_landed() {
    let mut f = FakeForge::mains(&["m0"]);
    f.merge_fails = true;
    f.after_merge_error = MergeErrorVerdict::OpenButOnMain;
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    let error = out.error.clone().expect("the contradiction stops the run");
    assert!(
        error.contains("still reports train PR") && error.contains("advisor"),
        "{error}"
    );
    assert!(out.landed.is_empty(), "{:?}", out.landed);
    drop(rails);
    assert!(f.landed_members.is_empty(), "members stay open");
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_a_pre_merge_refusal_surfaces_without_rereads() {
    let mut f = FakeForge::mains(&["m0"]);
    f.merge_refused = true;
    f.after_merge_error = MergeErrorVerdict::Landed;
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1])).unwrap();
    let error = out.error.clone().expect("the refusal stops the run");
    assert!(error.contains("under review"), "{error}");
    assert!(out.landed.is_empty(), "{:?}", out.landed);
    drop(rails);
    assert_eq!(
        f.rechecks, 0,
        "a refusal before the merge call is not re-read"
    );
}

// trace:BUG-2433 | ai:claude
#[test]
fn bug_2433_the_forge_merge_call_marker_survives_context() {
    let tagged = anyhow::anyhow!("HTTP 502")
        .context(ForgeMergeCallFailed)
        .context("train PR #9 was not merged");
    assert!(is_forge_merge_call_error(&tagged));
    assert!(!is_forge_merge_call_error(&anyhow::anyhow!("lease lost")));
}

// trace:BUG-2214.ac2ec1b2 | ai:claude
#[test]
fn bug_2214_a_merge_failure_after_the_release_rearms() {
    let covered = covered_member(7);
    let mut f = FakeForge::mains(&["base"]);
    f.gates.insert(7, Ok(covered.clone()));
    f.gate_green = true;
    f.merge_fails = true;
    let mut rails = ForgeRails::new(&mut f, "s".into());
    let out = drive(&mut rails, vec![covered]).unwrap();
    assert!(out.error.is_some(), "the merge failure stops the run");
    assert!(out.landed.is_empty());
    assert_eq!(f.rearmed, f.released, "every release was re-armed");
}

// ── BUG-2220: a PR rides one train at a time ───────────────────────────────

// trace:BUG-2220 | ai:claude
#[test]
fn bug_2220_a_pr_riding_an_open_train_is_refused_naming_it() {
    let open = vec![(
        2890,
        Some(vec![(2864, "aaaa".to_string()), (2869, "bbbb".to_string())]),
    )];
    let why = train_rider_refusal(2869, &open, &[]).unwrap();
    assert!(why.contains("#2890"), "{why}");
    assert!(
        train_rider_refusal(2899, &open, &[]).is_none(),
        "a PR on no train is free to board"
    );
}

// trace:BUG-2220 | ai:claude
#[test]
fn bug_2220_a_pr_on_a_leased_train_is_refused_naming_the_lease() {
    let leased = vec![(2891, Some(vec![(2864, "aaaa".to_string())]))];
    let why = train_rider_refusal(2864, &[], &leased).unwrap();
    assert!(why.contains("#2891") && why.contains("lease"), "{why}");
}

/// A real members line round-trips into the refusal; a malformed line names
/// nobody (blocking every candidate on one bad body would stop all trains).
// trace:BUG-2220 | ai:claude
#[test]
fn bug_2220_members_lines_parse_into_the_gate_and_malformed_names_nobody() {
    let body = format!("CI body\n{MEMBERS_LINE} #12@abc1234, #13@def5678\n");
    let open = vec![(77, parse_members_line(&body))];
    assert!(train_rider_refusal(12, &open, &[]).is_some());
    assert!(train_rider_refusal(13, &open, &[]).is_some());
    assert!(train_rider_refusal(14, &open, &[]).is_none());
    let malformed = vec![(78, parse_members_line("aida-train-members: nonsense"))];
    assert!(train_rider_refusal(12, &malformed, &[]).is_none());
}

/// TASK-1865 fixture: a private live train lease on PR #2891 that is not an
/// open `train/*` PR, and a fake `gh` whose `pr list` returns no open train
/// and whose `pr view 2891` runs `view_script`. Returns what the real
/// `live_trains` lookup made of it.
// trace:TASK-1865 | ai:claude
#[cfg(unix)]
#[allow(clippy::type_complexity)]
fn task_1865_leased_lookup(
    view_script: &str,
) -> (
    Vec<(u64, Option<Vec<(u64, String)>>)>,
    Vec<(u64, Option<Vec<(u64, String)>>)>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("main");
    write_retry_config(&root);
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::test_exec::write_executable(
        &bin.join("gh"),
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = pr ] && [ \"$2\" = list ]; then echo '[]'; exit 0; fi\n\
             if [ \"$1\" = pr ] && [ \"$2\" = view ] && [ \"$3\" = 2891 ]; then\n{view_script}\nfi\n\
             echo unexpected gh args: \"$@\" >&2\n\
             exit 1\n"
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(path.as_str()))]);
    let _lease = crate::merge_lock::acquire(
        &root,
        "main",
        Some(2891),
        TRAIN_LEASE_NOTE,
        std::time::Duration::ZERO,
    )
    .unwrap();
    let (open, leased, _abandoned) = live_trains(&root).unwrap();
    (open, leased)
}

/// An unreadable leased train body still names nobody (the rider gate fails
/// open, as before), but the failed probe is said on stderr naming the train.
// trace:TASK-1865 | ai:claude
#[cfg(unix)]
#[test]
fn task_1865_an_unreadable_leased_train_warns_and_still_fails_open() {
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("warning: could not read leased train PR #2891's members"),
            "{stderr}"
        );
        assert!(
            stderr.contains("gh pr view 2891") && stderr.contains("could not resolve PR 2891"),
            "the warning must name the failed probe: {stderr}"
        );
        return;
    }
    let (open, leased) =
        task_1865_leased_lookup("  echo 'GraphQL: could not resolve PR 2891' >&2; exit 1");
    assert!(open.is_empty(), "{open:?}");
    assert_eq!(leased, vec![(2891, None)]);
    assert!(train_rider_refusal(2864, &open, &leased).is_none());
}

/// A readable leased train body keeps refusing its known member, with no
/// failure warning.
// trace:TASK-1865 | ai:claude
#[cfg(unix)]
#[test]
fn task_1865_a_readable_leased_train_refuses_its_member_without_warning() {
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("could not read leased train"), "{stderr}");
        return;
    }
    let (open, leased) =
        task_1865_leased_lookup(&format!("  echo '{MEMBERS_LINE} #2864@abc1234'; exit 0"));
    assert!(open.is_empty(), "{open:?}");
    assert_eq!(
        leased,
        vec![(2891, Some(vec![(2864, "abc1234".to_string())]))]
    );
    let why = train_rider_refusal(2864, &open, &leased).unwrap();
    assert!(why.contains("#2891") && why.contains("lease"), "{why}");
    assert!(train_rider_refusal(2899, &open, &leased).is_none());
}

// ── BUG-2219: train network steps retry transient failures ────────────────

/// Fast retry config for the temp repo: 3 attempts, ~1ms backoff.
// trace:BUG-2219 | ai:claude
#[cfg(unix)]
fn write_retry_config(root: &Path) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[orchestrator]\nretry_max_attempts = 3\nretry_base_delay_ms = 1\n",
    )
    .unwrap();
}

/// A command that fails `fail_n` times with `msg` on stderr, then succeeds,
/// counting attempts in `counter`.
// trace:BUG-2219 | ai:claude
#[cfg(unix)]
fn flaky_cmd(counter: &Path, fail_n: u32, msg: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(format!(
        "n=$(cat {c} 2>/dev/null || echo 0); n=$((n+1)); printf %s $n > {c}; \
         if [ $n -le {fail_n} ]; then echo '{msg}' >&2; exit 1; fi; echo ok",
        c = counter.display()
    ));
    cmd
}

#[cfg(unix)]
fn attempts(counter: &Path) -> String {
    std::fs::read_to_string(counter).unwrap_or_default()
}

/// An injected transient failure (the BUG-2219 symptom text) retries and the
/// step then succeeds.
// trace:BUG-2219 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2219_a_transient_network_failure_retries_then_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    write_retry_config(dir.path());
    let counter = dir.path().join("count");
    let out = net_output(dir.path(), "git fetch origin", || {
        flaky_cmd(&counter, 2, "Could not resolve host: github.com")
    })
    .unwrap();
    assert!(out.status.success(), "the retried step must succeed");
    assert_eq!(attempts(&counter), "3", "two failures, then the success");
}

/// A non-transient failure (auth) is returned at once — exactly one attempt.
// trace:BUG-2219 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2219_a_non_transient_failure_never_retries() {
    let dir = tempfile::tempdir().unwrap();
    write_retry_config(dir.path());
    let counter = dir.path().join("count");
    let out = net_output(dir.path(), "gh pr view 1", || {
        flaky_cmd(&counter, 99, "authentication failed")
    })
    .unwrap();
    assert!(!out.status.success());
    assert_eq!(attempts(&counter), "1", "no retry on a permanent failure");
}

/// Retries are bounded; when they run out the failing output comes back so
/// the caller's `ensure!` names the step that failed.
// trace:BUG-2219 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2219_exhausted_retries_return_the_final_failure() {
    let dir = tempfile::tempdir().unwrap();
    write_retry_config(dir.path());
    let counter = dir.path().join("count");
    let out = net_output(dir.path(), "git push origin", || {
        flaky_cmd(&counter, 99, "Connection reset by peer")
    })
    .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        attempts(&counter),
        "3",
        "bounded at the configured attempts"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Connection reset by peer"));
}

/// The train's two network helpers route every fetch/push/gh call through
/// the retry runner.
// trace:BUG-2219 | ai:claude
#[test]
fn bug_2219_train_network_helpers_route_through_the_retry_runner() {
    let train = include_str!("../pr_train.rs");
    let forge = include_str!("../forge.rs");
    // trace:STORY-2401 | ai:codex — inspect the actual delegated body,
    // rather than accidentally matching a neighbouring helper in 700 bytes.
    for (src, name, callee) in [
        (train, "fn gh_stdout(", "crate::forge::train_gh_stdout("),
        (
            train,
            "fn train_policy_read(",
            "crate::forge::train_gh_api_read(",
        ),
        (train, "fn origin_git(", "net_output("),
        (forge, "fn train_gh_stdout(", "crate::pr_train::net_output("),
        (
            forge,
            "fn train_gh_api_read(",
            "crate::pr_train::net_output(",
        ),
    ] {
        let start = src.find(name).expect(name);
        let body = src[start..].split("\n}").next().unwrap();
        assert!(body.contains(callee), "{name} must route through {callee}");
    }
}

// trace:BUG-2401 | ai:codex
fn required_train_rows() -> Vec<crate::ci_gate::CheckRow> {
    (0..7)
        .map(|i| crate::ci_gate::CheckRow {
            name: format!("Build lane {i}"),
            bucket: "pass".into(),
            workflow: "CI".into(),
            run_id: None,
        })
        .chain(std::iter::once(crate::ci_gate::CheckRow {
            name: crate::ci_gate::HOLD_GATE_CHECK.into(),
            bucket: "pass".into(),
            workflow: "hold".into(),
            run_id: None,
        }))
        .collect()
}

#[test]
// trace:BUG-2401 | ai:codex
fn train_required_aggregate_registration_and_precedence() {
    use crate::ci_gate::{CheckRow, CiGateConfig};
    let required = vec![
        "Build (ubuntu-latest)".into(),
        crate::ci_gate::HOLD_GATE_CHECK.into(),
    ];
    let cfg = CiGateConfig {
        informational_checks: vec!["Build*".into()],
        informational_workflows: vec![],
    };
    let mut rows = required_train_rows();
    // Exact #3202 shape: seven passing lanes plus hold, no protected aggregate.
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Missing(_)
    ));
    // Preserve genuine red-train rerun/bisection even before registration.
    rows[0].bucket = "fail".into();
    assert!(matches!(
        train_ci_readiness(&rows, &required, &CiGateConfig::default(), false),
        TrainCiReadiness::Red(_)
    ));
    rows[0].bucket = "pass".into();
    // A held member must never make an absent aggregate green either.
    rows.last_mut().unwrap().bucket = "fail".into();
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, true),
        TrainCiReadiness::Missing(_)
    ));
    rows.push(CheckRow {
        name: required[0].clone(),
        bucket: "pending".into(),
        workflow: "CI".into(),
        run_id: None,
    });
    for bucket in ["pending", "unknown"] {
        rows.last_mut().unwrap().bucket = bucket.into();
        assert!(matches!(
            train_ci_readiness(&rows, &required, &cfg, true),
            TrainCiReadiness::Pending(_)
        ));
    }
    // A skipped required check is a terminal refusal naming it, never a wait
    // until the CI ceiling. trace:BUG-2401 | ai:claude
    rows.last_mut().unwrap().bucket = "skipping".into();
    match train_ci_readiness(&rows, &required, &cfg, true) {
        TrainCiReadiness::Refused(why) => {
            assert!(why.contains("skipped: Build (ubuntu-latest)"), "{why}")
        }
        other => panic!("a skipped required check must refuse, got {other:?}"),
    }
    rows.last_mut().unwrap().bucket = "fail".into();
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, true),
        TrainCiReadiness::Red(_)
    ));
    rows.last_mut().unwrap().bucket = "pass".into();
    assert_eq!(
        train_ci_readiness(&rows, &required, &cfg, true),
        TrainCiReadiness::Green
    );
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Red(_)
    ));
    rows[7].bucket = "pass".into();
    rows.push(CheckRow {
        name: "Build informational".into(),
        bucket: "pending".into(),
        workflow: "other".into(),
        run_id: None,
    });
    assert_eq!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Green
    );
    assert!(matches!(
        train_ci_readiness(&[], &[], &cfg, false),
        TrainCiReadiness::Missing(_)
    ));
}

#[test]
// trace:BUG-2401 | ai:codex
fn train_required_ci_live_ports_refuse_stale_and_final_missing() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    // Fast transient retries for the policy-read case (BUG-2219 runner).
    write(
        dir,
        ".aida/config.toml",
        "[orchestrator]\nretry_max_attempts = 3\nretry_base_delay_ms = 1\n",
    );
    let gh = dir.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
case "$*" in
  'pr view '*)
    if [ -f moving ] && [ -f viewed ]; then cat moved.json; else cat head.json; fi
    touch viewed ;;
  'api repos/{owner}/{repo}/branches/release%2Fnext/protection')
    if [ -f flaky ]; then rm flaky; echo 'error connecting to api.github.com' >&2; exit 1; fi
    if [ -f unprotected ]; then echo '{"message":"Branch not protected"}'; echo 'gh: Branch not protected (HTTP 404)' >&2; exit 1; fi
    if [ -f unavailable ]; then echo 'HTTP 403' >&2; exit 1; fi; cat protection.json ;;
  'api repos/{owner}/{repo}/rules/branches/release%2Fnext?per_page=100&page='*)
    p=${2##*page=}
    if [ -f "rules-$p.json" ]; then cat "rules-$p.json"; elif [ "$p" = 1 ]; then cat rules.json; else echo '[]'; fi ;;
  'pr checks '*) cat rows.json ;;
  *) echo "unexpected forge call: $*" >&2; exit 1 ;;
esac
"#,
    );
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", path.as_str()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);
    let tip = "a".repeat(40);
    let change = crate::forge::ChangeRef {
        id: 3202,
        url: "fixture".into(),
        branch: "train".into(),
        base: "release/next".into(),
        title: None,
    };
    write(
        dir,
        "head.json",
        &serde_json::json!({"headRefOid":tip,"baseRefName":change.base,"state":"OPEN"}).to_string(),
    );
    write(
        dir,
        "moved.json",
        &serde_json::json!({"headRefOid":"b".repeat(40),"baseRefName":change.base,"state":"OPEN"})
            .to_string(),
    );
    write(
        dir,
        "protection.json",
        r#"{"required_status_checks":{"contexts":["Build (ubuntu-latest)","merge-hold-gate"]}}"#,
    );
    write(dir, "rules.json", "[]");
    let rows_json = |aggregate: Option<&str>| {
        let mut rows: Vec<_> = required_train_rows()
            .into_iter()
            .map(|r| serde_json::json!({"name":r.name,"bucket":r.bucket,"workflow":r.workflow}))
            .collect();
        if let Some(bucket) = aggregate {
            rows.push(
                serde_json::json!({"name":"Build (ubuntu-latest)","bucket":bucket,"workflow":"CI"}),
            );
        }
        serde_json::to_string(&rows).unwrap()
    };
    let forge = crate::forge::forge_for_kind(dir, crate::forge::ForgeKind::GitHub);
    let mut host = LiveTrainForge {
        project_root: dir.into(),
        main_worktree: dir.into(),
        base: change.base.clone(),
        forge: forge.as_ref(),
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    // Delayed registration and settlement at the exact protected train head.
    for (bucket, expected) in [
        (None, "missing"),
        (Some("pending"), "pending"),
        (Some("pass"), "green"),
        (Some("fail"), "red"),
    ] {
        write(dir, "rows.json", &rows_json(bucket));
        let state = host.train_ci_snapshot(&change, &tip, &[]).unwrap();
        assert!(match expected {
            "missing" => matches!(state, TrainCiReadiness::Missing(_)),
            "pending" => matches!(state, TrainCiReadiness::Pending(_)),
            "green" => state == TrainCiReadiness::Green,
            _ => matches!(state, TrainCiReadiness::Red(_)),
        });
    }
    // Initial success does not authorize landing after a later row disappears.
    write(dir, "rows.json", &rows_json(Some("pass")));
    let train = Built {
        base: "base".into(),
        tip: tip.clone(),
        cars: vec![],
        dropped: vec![],
    };
    assert!(matches!(
        host.wait_ci(&change, &train).unwrap(),
        CiVerdict::Green
    ));
    write(dir, "rows.json", &rows_json(None));
    assert!(host
        .merge(&change, &tip)
        .unwrap_err()
        .to_string()
        .contains("final CI recheck"));
    write(dir, "unavailable", "");
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    std::fs::remove_file(dir.join("unavailable")).unwrap();
    write(
        dir,
        "rules.json",
        r#"[{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"ruleset-only"}]}}]"#,
    );
    write(dir, "rows.json", &rows_json(Some("pass")));
    assert!(matches!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Missing(_)
    ));
    write(dir, "rules.json", "{}");
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    write(dir, "rules.json", r#"[{"type":"workflows"}]"#);
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    write(dir, "rules.json", "[]");
    // A full first page is followed: a required context or an unsupported
    // workflow rule on a later page is never dropped (GitHub pages rules).
    // trace:BUG-2401 | ai:claude
    let full_page = serde_json::to_string(&vec![
        serde_json::json!({"type":"deletion"});
        crate::status_prs::TRAIN_RULE_PAGE_SIZE
    ])
    .unwrap();
    write(dir, "rules-1.json", &full_page);
    write(
        dir,
        "rules-2.json",
        r#"[{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"page-two-only"}]}}]"#,
    );
    match host.train_ci_snapshot(&change, &tip, &[]).unwrap() {
        TrainCiReadiness::Missing(names) => assert_eq!(names, vec!["page-two-only".to_string()]),
        other => panic!("later-page required check must be missing, got {other:?}"),
    }
    write(dir, "rules-2.json", r#"[{"type":"workflows"}]"#);
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    write(dir, "rules-2.json", r#"{"message":"Server Error"}"#);
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    // More full pages than the bound: incomplete policy refuses.
    for page in 1..=crate::status_prs::TRAIN_RULE_PAGE_LIMIT {
        write(dir, &format!("rules-{page}.json"), &full_page);
    }
    assert!(host
        .train_ci_snapshot(&change, &tip, &[])
        .unwrap_err()
        .to_string()
        .contains("policy incomplete"));
    for page in 1..=crate::status_prs::TRAIN_RULE_PAGE_LIMIT {
        std::fs::remove_file(dir.join(format!("rules-{page}.json"))).unwrap();
    }
    write(
        dir,
        "protection.json",
        r#"{"required_status_checks":{"contexts":[42]}}"#,
    );
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    write(dir, "protection.json", r#"{"required_status_checks":null}"#);
    write(dir, "rows.json", "[]");
    assert!(matches!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Unregistered
    ));
    write(dir, "rows.json", &rows_json(Some("pass")));
    assert_eq!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Green
    );
    // GitHub omits `required_status_checks` on a protected branch with no
    // status-check requirement: absent reads as nothing required, like null;
    // a present but malformed section still refuses. trace:BUG-2401 | ai:claude
    write(
        dir,
        "protection.json",
        r#"{"url":"fixture","enforce_admins":{"enabled":true}}"#,
    );
    assert_eq!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Green
    );
    write(
        dir,
        "protection.json",
        r#"{"required_status_checks":{"contexts":"Build"}}"#,
    );
    assert!(host.train_ci_snapshot(&change, &tip, &[]).is_err());
    // The 'Branch not protected' 404 is a positive "nothing required" (rules
    // still apply); every other protection failure stays unknown above.
    write(dir, "unprotected", "");
    assert_eq!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Green
    );
    std::fs::remove_file(dir.join("unprotected")).unwrap();
    // One transient policy-read failure is retried (BUG-2219) and the CI
    // wait carries on to green instead of stopping the run.
    write(dir, "protection.json", r#"{"required_status_checks":null}"#);
    write(dir, "flaky", "");
    assert!(matches!(
        host.wait_ci(&change, &train).unwrap(),
        CiVerdict::Green
    ));
    assert!(!dir.join("flaky").exists(), "the transient failure was hit");
    std::fs::remove_file(dir.join("viewed")).unwrap();
    write(dir, "moving", "");
    assert!(host
        .train_ci_snapshot(&change, &tip, &[])
        .unwrap_err()
        .to_string()
        .contains("changed"));
    assert!(host.merge(&change, &tip).is_err());
}

// trace:BUG-2401 | ai:claude
fn bug2401_row(name: &str, bucket: &str) -> crate::ci_gate::CheckRow {
    crate::ci_gate::CheckRow {
        name: name.into(),
        bucket: bucket.into(),
        workflow: "CI".into(),
        run_id: None,
    }
}

/// An aggregate that registers only after its lanes finish is pending while
/// they run, never missing (and never green); names share one comparator.
#[test]
// trace:BUG-2401 | ai:claude
fn bug2401_absent_aggregate_behind_running_lanes_is_pending() {
    let cfg = crate::ci_gate::CiGateConfig::default();
    let required = vec!["Build (ubuntu-latest)".to_string()];
    let mut rows = required_train_rows();
    rows[3].bucket = "pending".into();
    match train_ci_readiness(&rows, &required, &cfg, false) {
        TrainCiReadiness::Pending(names) => {
            assert!(names[0].starts_with("Build (ubuntu-latest)"), "{names:?}")
        }
        other => panic!("absent aggregate behind a running lane: {other:?}"),
    }
    rows[3].bucket = "pass".into();
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Missing(_)
    ));
    // Case differences neither leave a present check "missing" nor skip it.
    rows.push(bug2401_row("build (UBUNTU-latest)", "pending"));
    assert!(matches!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Pending(_)
    ));
    rows.last_mut().unwrap().bucket = "pass".into();
    assert_eq!(
        train_ci_readiness(&rows, &required, &cfg, false),
        TrainCiReadiness::Green
    );
}

/// The wait loop: a long-lane build (aggregate absent behind pending lanes
/// for well over 600s) still ends green; the 600s registration bound starts
/// only once every observed row has settled.
#[test]
// trace:BUG-2401 | ai:claude
fn bug2401_wait_loop_bounds_registration_only_after_lanes_settle() {
    let pending = || TrainCiReadiness::Pending(vec!["Build (ubuntu-latest)".into()]);
    let missing = || TrainCiReadiness::Missing(vec!["Build (ubuntu-latest)".into()]);
    let run = |script: Vec<TrainCiReadiness>, settle_limit: u64| {
        let mut script = script.into_iter();
        let clock = std::cell::Cell::new(0u64);
        wait_train_ci(
            3206,
            settle_limit,
            || Ok(script.next().expect("the loop must not outlive its script")),
            || clock.get(),
            || clock.set(clock.get() + TRAIN_CI_POLL_SECS),
        )
    };
    // ~17 minutes of lanes with no aggregate row, then it registers green.
    let mut long = (0..100).map(|_| pending()).collect::<Vec<_>>();
    long.push(TrainCiReadiness::Green);
    assert_eq!(run(long, 3600).unwrap(), CiVerdict::Green);
    // Lanes ran 1000s; the aggregate then stays absent: refused 600s later,
    // not at once (the clock started when the lanes settled).
    let mut late = (0..100).map(|_| pending()).collect::<Vec<_>>();
    late.extend((0..70).map(|_| missing()));
    let err = run(late, 0).unwrap_err().to_string();
    assert!(err.contains("no CI registered within 600s"), "{err}");
    // A run of Missing interrupted by Pending restarts the bound.
    let mut reset = (0..50).map(|_| missing()).collect::<Vec<_>>();
    reset.push(pending());
    reset.extend((0..50).map(|_| missing()));
    reset.push(TrainCiReadiness::Green);
    assert_eq!(run(reset, 0).unwrap(), CiVerdict::Green);
    // Absence never turns green, and the ordinary ceiling still bounds a wait.
    let stuck = (0..100).map(|_| pending()).collect::<Vec<_>>();
    assert!(run(stuck, 500)
        .unwrap_err()
        .to_string()
        .contains("not ready"));
    assert!(matches!(
        run(vec![TrainCiReadiness::Red("x".into())], 0).unwrap(),
        CiVerdict::Red(_)
    ));
    // A refusal stops on the first poll with its reason, well inside the ceiling.
    // trace:BUG-2401 | ai:claude
    let err = run(
        vec![TrainCiReadiness::Refused(
            "required checks were skipped: X".into(),
        )],
        3600,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("refused: required checks were skipped: X"),
        "{err}"
    );
}

/// Composed live snapshot + wait loop with a mandate-admitted held member:
/// the hold gate fails within seconds while lanes run and the aggregate is
/// unregistered; the train waits, then ends green once the lanes and the
/// aggregate pass (BUG-2214 tolerance). An unadmitted hold is red at once.
#[test]
// trace:BUG-2401 | ai:claude
fn bug2401_held_member_train_waits_through_pending_lanes_to_green() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    let gh = dir.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
case "$*" in
  'pr view '*) cat head.json ;;
  'api repos/{owner}/{repo}/branches/main/protection') cat protection.json ;;
  'api repos/{owner}/{repo}/rules/branches/main?per_page=100&page='*) echo '[]' ;;
  'pr checks '*) n=$(cat poll); cat "rows-$n.json" ;;
  *) echo "unexpected forge call: $*" >&2; exit 1 ;;
esac
"#,
    );
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", path.as_str()),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);
    let tip = "c".repeat(40);
    let change = crate::forge::ChangeRef {
        id: 3206,
        url: "fixture".into(),
        branch: "train".into(),
        base: "main".into(),
        title: None,
    };
    write(
        dir,
        "head.json",
        &serde_json::json!({"headRefOid":tip,"baseRefName":"main","state":"OPEN"}).to_string(),
    );
    write(
        dir,
        "protection.json",
        r#"{"required_status_checks":{"contexts":["Build (ubuntu-latest)","merge-hold-gate"]}}"#,
    );
    let rows = |lanes: &str, aggregate: Option<&str>| {
        let mut rows: Vec<_> = (0..7)
            .map(|i| serde_json::json!({"name":format!("Build lane {i}"),"bucket":lanes,"workflow":"CI"}))
            .collect();
        rows.push(serde_json::json!({"name":"merge-hold-gate","bucket":"fail","workflow":"hold"}));
        if let Some(b) = aggregate {
            rows.push(
                serde_json::json!({"name":"Build (ubuntu-latest)","bucket":b,"workflow":"CI"}),
            );
        }
        serde_json::to_string(&rows).unwrap()
    };
    write(dir, "rows-0.json", &rows("pending", None));
    write(dir, "rows-1.json", &rows("pending", None));
    write(dir, "rows-2.json", &rows("pass", Some("pending")));
    write(dir, "rows-3.json", &rows("pass", Some("pass")));
    let forge = crate::forge::forge_for_kind(dir, crate::forge::ForgeKind::GitHub);
    let mut host = LiveTrainForge {
        project_root: dir.into(),
        main_worktree: dir.into(),
        base: "main".into(),
        forge: forge.as_ref(),
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    let car = |m: Member| Car {
        member: m,
        commit: "c".into(),
        patch_id: "p".into(),
    };
    let held_labels =
        |_: u64| -> std::result::Result<Option<crate::merge_hold::PinnedChange>, String> {
            Ok(Some(crate::merge_hold::PinnedChange {
                merged: false,
                state: crate::merge_hold::ChangeState::Open,
                head_sha: None,
                hold_label: true,
                recorded_label: true,
                cleared_label: false,
            }))
        };
    let run = |host: &mut LiveTrainForge<'_>, cars: &[Car]| {
        write(dir, "poll", "0");
        let polls = std::cell::Cell::new(0u32);
        wait_train_ci(
            change.id,
            0,
            || host.train_ci_snapshot_with(&change, &tip, cars, None, held_labels),
            || u64::from(polls.get()) * 400,
            || {
                polls.set(polls.get() + 1);
                write(dir, "poll", &polls.get().to_string());
            },
        )
        .map(|v| (v, polls.get()))
    };
    // Admitted held member: waits through pending lanes (and 1200s of no
    // aggregate), then green once lanes + aggregate pass.
    let covered = vec![car(covered_member(7))];
    assert_eq!(run(&mut host, &covered).unwrap(), (CiVerdict::Green, 3));
    // A held member the mandate does not cover: the hold-gate red is real.
    let plain = vec![car(member(7, "head7", "s"))];
    let (verdict, polls) = run(&mut host, &plain).unwrap();
    assert!(matches!(verdict, CiVerdict::Red(_)), "{verdict:?}");
    assert_eq!(
        polls, 0,
        "an unexplained hold-gate red is red on the first poll"
    );
    // The final merge recheck carries no tolerance: a still-red hold gate
    // (not re-run green after release) refuses.
    assert!(matches!(
        host.train_ci_snapshot(&change, &tip, &[]).unwrap(),
        TrainCiReadiness::Red(_)
    ));
}

// ── BUG-2412: a train PR whose owner died does not block its riders ───────

/// An owner recorded in this process's PID namespace and boot.
// trace:BUG-2412 | ai:claude
fn bug_2412_local_owner(host: &str, pid: u32, start: &str) -> TrainOwner {
    TrainOwner {
        host: host.into(),
        pid,
        start: Some(start.into()),
        pid_ns: aida_core::local_pid_ns(),
        boot: aida_core::local_boot_id(),
    }
}

/// Whether this platform reports a PID namespace and boot id, without which
/// no owner is ever judged dead (so the abandon paths cannot be exercised).
// trace:BUG-2412 | ai:claude
fn bug_2412_identity_readable() -> bool {
    aida_core::local_pid_ns().is_some() && aida_core::local_boot_id().is_some()
}

/// The owner line round-trips, and a body without one (a pre-BUG-2412
/// train, or a hand edit) names no owner.
// trace:BUG-2412 | ai:claude
#[test]
fn bug_2412_owner_line_round_trips_and_a_missing_line_names_nobody() {
    let owner = TrainOwner {
        host: "box".into(),
        pid: 4242,
        start: Some("linux-starttime:77".into()),
        pid_ns: Some("4026531836".into()),
        boot: Some("boot-a".into()),
    };
    let body = format!(
        "Train.\n\n{MEMBERS_LINE} #1@abc1234\n{}\n",
        owner_line(&owner)
    );
    assert_eq!(parse_owner_line(&body), Some(owner.clone()));
    assert_eq!(
        parse_members_line(&body),
        Some(vec![(1, "abc1234".to_string())]),
        "the owner line leaves the members line the merge-hold gate reads intact"
    );
    let no_start = TrainOwner {
        start: None,
        pid_ns: None,
        boot: None,
        ..owner
    };
    assert_eq!(parse_owner_line(&owner_line(&no_start)), Some(no_start));
    assert_eq!(parse_owner_line("Train.\n"), None);
    assert_eq!(
        parse_owner_line(&format!("{OWNER_LINE} host=box pid=x")),
        None
    );
}

/// Only a dead owner in this host's boot and PID namespace makes a train
/// abandoned: this process's own train stays live, and another host's, an
/// ownerless, a foreign-namespace, an unrecorded-namespace or another boot's
/// train keeps blocking (its liveness cannot be read here).
// trace:BUG-2412 | ai:claude
#[test]
fn bug_2412_only_a_dead_same_host_owner_abandons_a_train() {
    let alive = aida_core::liveness::process_identity_is_alive;
    let ours = format!("x\n{}\n", owner_line_for_self());
    let host = crate::coordination::hostname();
    assert_eq!(train_abandoned(&ours, &host, alive), None, "a live owner");
    if !bug_2412_identity_readable() {
        return;
    }
    let dead = bug_2412_local_owner(&host, 999_999_999, "linux-starttime:1");
    let dead_body = format!("x\n{}\n", owner_line(&dead));
    assert_eq!(
        train_abandoned(&dead_body, &host, alive),
        Some(dead.clone())
    );
    assert_eq!(
        train_abandoned(&dead_body, "another-host", alive),
        None,
        "another host's train is never judged from here"
    );
    assert_eq!(train_abandoned("x\n", &host, alive), None, "no owner line");
    // The owner pid was recorded in another sandboxed call's PID namespace:
    // probing it here says nothing, so the train is never closed.
    let foreign_ns = TrainOwner {
        pid_ns: Some("1".into()),
        ..dead.clone()
    };
    assert_eq!(
        train_abandoned(&owner_line(&foreign_ns), &host, alive),
        None,
        "an owner in a foreign PID namespace is unreadable, not dead"
    );
    let unrecorded_ns = TrainOwner {
        pid_ns: None,
        ..dead.clone()
    };
    assert_eq!(
        train_abandoned(&owner_line(&unrecorded_ns), &host, alive),
        None,
        "an owner with no recorded PID namespace is unreadable"
    );
    let other_boot = TrainOwner {
        boot: Some("another-boot".into()),
        ..dead.clone()
    };
    assert_eq!(
        train_abandoned(&owner_line(&other_boot), &host, alive),
        None,
        "an owner from another boot is unreadable"
    );
    let unrecorded_boot = TrainOwner {
        boot: None,
        ..dead.clone()
    };
    assert_eq!(
        train_abandoned(&owner_line(&unrecorded_boot), &host, alive),
        None,
        "an owner with no recorded boot id is unreadable"
    );
    // A recycled pid (alive, but a different start identity) is dead.
    let recycled = bug_2412_local_owner(&host, std::process::id(), "linux-starttime:1");
    if aida_core::liveness::process_start_identity(std::process::id()).is_some() {
        assert!(train_abandoned(&format!("{}\n", owner_line(&recycled)), &host, alive).is_some());
    }
}

/// The observed failure: `aida pr train 3214 3240` was killed mid-run and
/// left train PR #3248 (members #3240) open with no owning process; the next
/// train refused #3240 as "already riding live train PR #3248". Now the
/// lookup reports #3248 abandoned. Unknown cleanup proof stops new admission
/// and leaves #3248 open. An ownerless legacy train, a train owned from another PID
/// namespace, and a dead-owner train still named by a live train lease all
/// keep blocking their riders and are never closed.
// trace:BUG-2412 | ai:claude
#[cfg(unix)]
#[test]
fn story_2401_abandoned_cleanup_unknown_blocks_new_train() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !bug_2412_identity_readable() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("main");
    write_retry_config(&root);
    let dead = bug_2412_local_owner("train-host", 999_999_999, "linux-starttime:1");
    let foreign = TrainOwner {
        pid_ns: Some("1".into()),
        ..dead.clone()
    };
    let rows = serde_json::json!([
        {
            "number": 3248,
            "headRefName": "train/killed-1",
            "body": format!("Train.\n\n{MEMBERS_LINE} #3240@abc1234\n{}\n", owner_line(&dead)),
        },
        {
            "number": 3249,
            "headRefName": "train/legacy-1",
            "body": format!("Train.\n\n{MEMBERS_LINE} #3214@def5678\n"),
        },
        {
            "number": 3250,
            "headRefName": "train/sandboxed-1",
            "body": format!("Train.\n\n{MEMBERS_LINE} #3260@0123abc\n{}\n", owner_line(&foreign)),
        },
        {"number": 3240, "headRefName": "bug-1-work", "body": "member"},
    ]);
    let list = tmp.path().join("list.json");
    std::fs::write(&list, rows.to_string()).unwrap();
    let log = tmp.path().join("gh.log");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::test_exec::write_executable(
        &bin.join("gh"),
        format!(
            "#!/bin/sh\n\
             echo \"$@\" >> '{log}'\n\
             if [ \"$1\" = pr ] && [ \"$2\" = list ]; then cat '{list}'; exit 0; fi\n\
             if [ \"$1\" = pr ] && [ \"$2\" = close ]; then exit 0; fi\n\
             echo unexpected gh args: \"$@\" >&2\n\
             exit 1\n",
            log = log.display(),
            list = list.display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(path.as_str())),
        ("AIDA_HOST_OVERRIDE", Some("train-host")),
    ]);
    let (open, leased, abandoned) = live_trains(&root).unwrap();
    assert_eq!(
        abandoned,
        vec![(3248, "train/killed-1".to_string(), dead.clone())]
    );
    assert_eq!(
        open,
        vec![
            (3249, Some(vec![(3214, "def5678".to_string())])),
            (3250, Some(vec![(3260, "0123abc".to_string())])),
        ]
    );
    assert!(
        train_rider_refusal(3260, &open, &leased).is_some(),
        "a train owned from a foreign PID namespace is live as far as we can tell"
    );
    assert!(
        train_rider_refusal(3240, &open, &leased).is_none(),
        "#3240 boards: its old train's owner is dead"
    );
    assert!(
        train_rider_refusal(3214, &open, &leased).is_some(),
        "an ownerless train is not judged dead and still blocks"
    );
    close_abandoned_train(&root, 3248, "train/killed-1", &dead, true).unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        !calls.contains("pr close"),
        "a dry run closes nothing: {calls}"
    );
    assert!(close_abandoned_train(&root, 3248, "train/killed-1", &dead, false).is_err());
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("pr close"), "{calls}");
    // A live train merge-lease still naming #3248 means a train is landing
    // it: it is not abandoned, it keeps blocking #3240, and nothing closes it.
    let _lease = crate::merge_lock::acquire(
        &root,
        "main",
        Some(3248),
        TRAIN_LEASE_NOTE,
        std::time::Duration::ZERO,
    )
    .unwrap();
    let (open, leased, abandoned) = live_trains(&root).unwrap();
    assert!(
        abandoned.is_empty(),
        "a leased train is never closed: {abandoned:?}"
    );
    assert!(
        open.iter().any(|(n, _)| *n == 3248),
        "the leased train stays live: {open:?}"
    );
    assert!(train_rider_refusal(3240, &open, &leased).is_some());
}

// ── BUG-2413: a bundled spec superseded into a drain carrier ──────────────

// trace:BUG-2413 | ai:claude
fn bug_2413_spec(mode: Option<aida_core::ExecutionMode>) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new("t".into(), "d".into());
    r.execution_mode = mode;
    r
}

// trace:BUG-2413 | ai:claude
fn bug_2413_supersede(bundled: &mut aida_core::Requirement, carrier: &aida_core::Requirement) {
    bundled.status = aida_core::RequirementStatus::Superseded;
    bundled.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::SupersededBy,
        target_id: carrier.id,
        created_at: None,
        created_by: None,
    });
}

/// The observed failure: carrier TASK-2731 is drain, bundled TASK-2737 was
/// superseded into it with its mode unset, and the train refused "TASK-2737
/// is marked unset (supervised)". It now gates on the carrier's drain.
// trace:BUG-2413 | ai:claude
#[test]
fn bug_2413_a_superseded_unset_spec_gates_on_its_drain_carrier() {
    use aida_core::ExecutionMode;
    let carrier = bug_2413_spec(Some(ExecutionMode::Drain));
    let mut bundled = bug_2413_spec(None);
    bug_2413_supersede(&mut bundled, &carrier);
    let store = vec![carrier.clone(), bundled.clone()];
    let mode = crate::pr_cmd::gating_execution_mode(&store, &bundled);
    assert_eq!(mode, Some(ExecutionMode::Drain));
    let records = vec![
        ("TASK-2731".to_string(), Vec::new(), carrier.execution_mode),
        ("TASK-2737".to_string(), Vec::new(), mode),
    ];
    assert_eq!(
        member_spec_policy_refusal(records, false, || Err(None)),
        None,
        "a drain carrier with its bundled spec needs no human"
    );
}

/// The inheritance never loosens the gate: a bundled spec's own mode wins, a
/// supervised carrier still supervises, an unset spec with no superseded-by
/// edge stays supervised, and a cycle terminates.
// trace:BUG-2413 | ai:claude
#[test]
fn bug_2413_inheritance_never_loosens_the_supervised_gate() {
    use aida_core::ExecutionMode;
    let guided = bug_2413_spec(Some(ExecutionMode::Guided));
    let mut bundled = bug_2413_spec(None);
    bug_2413_supersede(&mut bundled, &guided);
    let store = vec![guided.clone(), bundled.clone()];
    assert_eq!(
        crate::pr_cmd::gating_execution_mode(&store, &bundled),
        Some(ExecutionMode::Guided)
    );
    let drain = bug_2413_spec(Some(ExecutionMode::Drain));
    let mut own = bug_2413_spec(Some(ExecutionMode::Guided));
    bug_2413_supersede(&mut own, &drain);
    assert_eq!(
        crate::pr_cmd::gating_execution_mode(&[drain, own.clone()], &own),
        Some(ExecutionMode::Guided),
        "a spec's own mode is never overridden"
    );
    let lone = bug_2413_spec(None);
    assert_eq!(
        crate::pr_cmd::gating_execution_mode(std::slice::from_ref(&lone), &lone),
        None
    );
    let mut a = bug_2413_spec(None);
    let mut b = bug_2413_spec(None);
    bug_2413_supersede(&mut a, &b);
    bug_2413_supersede(&mut b, &a);
    assert_eq!(
        crate::pr_cmd::gating_execution_mode(&[a.clone(), b], &a),
        None
    );
}

#[test]
// trace:BUG-2423 | ai:codex
fn bug2423_delayed_ci_registration_lands_the_same_train() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_states = Some(VecDeque::from([
        TrainCiReadiness::Unregistered,
        TrainCiReadiness::Unregistered,
        TrainCiReadiness::Unregistered,
        TrainCiReadiness::Pending(vec!["Build".into()]),
        TrainCiReadiness::Green,
    ]));
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    assert_eq!(out.landed.len(), 2);
    assert!(out.ejected.is_empty());
    assert!(out.error.is_none());
    assert_eq!(f.opened, 1);
    assert_eq!(f.merged.len(), 1);
    assert!(f.retired.is_empty());
}

#[test]
// trace:BUG-2423 | ai:codex
fn bug2423_ci_never_started_retains_train_and_members() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_states = Some(VecDeque::new());
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    assert!(out.error.unwrap().contains("CI never started within 180s"));
    assert!(out.ejected.is_empty());
    assert!(out.landed.is_empty());
    assert_eq!(f.opened, 1);
    assert!(f.merged.is_empty());
    assert!(f.retired.is_empty());
    assert_eq!(f.comments, [901]);
}

/// Drive two members through a train whose CI wait fails deterministically;
/// the members must be ejected with `reason`, never paused as unreadable CI.
// trace:BUG-2423 | ai:claude
fn bug2423_assert_deterministic_ci_failure_ejects(mut f: FakeForge, reason: &str) {
    let mut rails = ForgeRails::new(&mut f, "t".into());
    let out = drive(&mut rails, fms(&[1, 2])).unwrap();
    let error = out.error.clone().expect("the CI error is in the outcome");
    assert!(error.contains(reason), "{error}");
    assert!(!error.contains("could not be read"), "{error}");
    assert!(out.landed.is_empty());
    assert_eq!(out.ejected.len(), 2, "{:?}", out.ejected);
    for e in &out.ejected {
        assert!(e.reason.contains(reason), "{e:?}");
        assert!(!e.reason.contains("could not be read"), "{e:?}");
    }
    assert_eq!(f.opened, 1, "no rerun or bisect");
    assert_eq!(f.retired.len(), 1, "the train PR is retired, not left open");
    assert_eq!(f.retired[0].0, 901);
    assert!(f.merged.is_empty());
}

#[test]
// trace:BUG-2423 | ai:claude
fn bug2423_refused_ci_ejects_members_with_the_refusal_reason() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_states = Some(VecDeque::from([TrainCiReadiness::Refused(
        "required checks were skipped: Build".into(),
    )]));
    bug2423_assert_deterministic_ci_failure_ejects(f, "required checks were skipped: Build");
}

#[test]
// trace:BUG-2423 | ai:claude
fn bug2423_head_or_state_change_ejects_members_with_the_reason() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_error = Some("train PR #901 head/base/state changed during CI readiness".into());
    bug2423_assert_deterministic_ci_failure_ejects(f, "head/base/state changed");
}

#[test]
// trace:BUG-2423 | ai:claude
fn story_2401_missing_required_check_timeout_ejects_members() {
    let mut f = FakeForge::mains(&["m0"]);
    f.ci_states = Some(
        std::iter::repeat_with(|| TrainCiReadiness::Missing(vec!["Build".into()]))
            .take(100)
            .collect(),
    );
    bug2423_assert_deterministic_ci_failure_ejects(f, "missing: Build");
}

#[test]
// trace:BUG-2423 | ai:codex
fn bug2423_transient_ci_reads_retry_but_persistent_errors_stop() {
    for recovers in [true, false] {
        let attempts = std::cell::Cell::new(0);
        let clock = std::cell::Cell::new(0);
        let result = wait_train_ci(
            901,
            0,
            || {
                attempts.set(attempts.get() + 1);
                if recovers && attempts.get() == 3 {
                    Ok(TrainCiReadiness::Green)
                } else {
                    Err(TrainCiReadError(anyhow::anyhow!("HTTP 503")).into())
                }
            },
            || clock.get(),
            || clock.set(clock.get() + TRAIN_CI_POLL_SECS),
        );
        assert_eq!(attempts.get(), 3);
        if recovers {
            assert_eq!(result.unwrap(), CiVerdict::Green);
        } else {
            assert!(result.unwrap_err().to_string().contains("HTTP 503"));
        }
    }
    let result = wait_train_ci(
        901,
        0,
        || anyhow::bail!("authority revoked"),
        || 0,
        || panic!("authority refusals must not retry"),
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("authority revoked"));
}

#[test]
// trace:BUG-2423 | ai:codex
fn bug2423_gh_no_checks_reported_is_empty_but_api_errors_refuse() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let gh = root.path().join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
if test -f fail; then echo 'HTTP 503: Service Unavailable' >&2; exit 1; fi
if test -f malformed; then echo '[broken'; exit 1; fi
if test -f pending; then echo '[{"name":"Build","bucket":"pending","workflow":"CI"}]'; exit 8; fi
echo "no checks reported on the 'train/fixture' branch" >&2
exit 1
"#,
    );
    let path = format!(
        "{}:{}",
        root.path().display(),
        std::env::var("PATH").unwrap()
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
    let forge = crate::forge::forge_for_kind(root.path(), crate::forge::ForgeKind::GitHub);
    let change = crate::forge::ChangeRef {
        id: 901,
        url: "fixture".into(),
        branch: "train/fixture".into(),
        base: "main".into(),
        title: None,
    };
    for _ in 0..3 {
        assert!(forge.check_rows(&change).unwrap().is_empty());
    }
    write(root.path(), "pending", "");
    assert_eq!(forge.check_rows(&change).unwrap()[0].bucket, "pending");
    write(root.path(), "fail", "");
    assert!(forge
        .check_rows(&change)
        .unwrap_err()
        .to_string()
        .contains("HTTP 503"));
    std::fs::remove_file(root.path().join("fail")).unwrap();
    write(root.path(), "malformed", "");
    assert!(forge.check_rows(&change).is_err());
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_train_unconfirmed_failure_suspends_without_ejection_or_close() {
    let mut f = FakeForge::mains(&["base"]);
    f.ci_error = Some("train state could not be read".into());
    f.confirmation = Some(crate::forge_observed::Confirmed::Unconfirmed);
    let mut rails = ForgeRails::new(&mut f, "confirm".into());
    let outcome = drive(&mut rails, vec![member(1, "head1", "s")]).unwrap();
    assert!(outcome.error.is_some());
    assert!(outcome.ejected.is_empty());
    assert!(f.retired.is_empty());
    assert!(f.landed_members.is_empty());
    assert!(!f.comments.is_empty());
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_train_transient_then_merged_finishes_landing() {
    let mut f = FakeForge::mains(&["base"]);
    f.ci_unreadable = true;
    f.confirmation = Some(crate::forge_observed::Confirmed::Positive);
    let mut rails = ForgeRails::new(&mut f, "confirm".into());
    let outcome = drive(&mut rails, vec![member(1, "head1", "s")]).unwrap();
    assert!(outcome.error.is_none());
    assert!(outcome.ejected.is_empty());
    assert_eq!(outcome.landed.len(), 1);
    assert_eq!(f.landed_members, vec![1]);
    assert!(f.retired.is_empty());
    assert!(
        f.merged.is_empty(),
        "reconciled landing must not issue another merge"
    );
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_train_confirmed_closed_failure_can_retire() {
    let mut f = FakeForge::mains(&["base"]);
    f.ci_error = Some("train is closed".into());
    f.confirmation = Some(crate::forge_observed::Confirmed::Negative);
    let mut rails = ForgeRails::new(&mut f, "confirm".into());
    let outcome = drive(&mut rails, vec![member(1, "head1", "s")]).unwrap();
    assert!(outcome.error.is_some());
    assert_eq!(outcome.ejected.len(), 1);
    assert_eq!(f.retired.len(), 1);
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_git_confirmation_fetches_private_ref_and_refuses_unknown() {
    use crate::forge_observed::Observed;
    let tmp = repo();
    let r = tmp.path();
    git(r, &["remote", "add", "origin", r.to_str().unwrap()]);
    let head = pr_branch(r, "candidate", |r| write(r, "new.txt", "candidate"));
    assert_eq!(
        crate::forge_confirmation::on_base(r, "main", &head, 42),
        Observed::Known(false)
    );
    git(r, &["merge", "--quiet", "--ff-only", "candidate"]);
    assert_eq!(
        crate::forge_confirmation::on_base(r, "main", &head, 42),
        Observed::Known(true)
    );
    assert!(git(r, &["for-each-ref", "refs/aida/confirm/"]).is_empty());
    // A stale local origin/main must never substitute for a failed fetch.
    git(
        r,
        &["remote", "set-url", "origin", "/no-such-story-2401-origin"],
    );
    assert!(matches!(
        crate::forge_confirmation::on_base(r, "main", &head, 42),
        Observed::Transient(_)
    ));
    assert!(git(r, &["for-each-ref", "refs/aida/confirm/"]).is_empty());
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_positive_during_abort_keeps_landing_credit() {
    let mut f = FakeForge::mains(&["base"]);
    f.ci_error = Some("train is no longer open".into());
    f.confirmation = Some(crate::forge_observed::Confirmed::Positive);
    let mut rails = ForgeRails::new(&mut f, "confirm".into());
    let outcome = drive(&mut rails, vec![member(1, "head1", "s")]).unwrap();
    assert_eq!(outcome.landed.len(), 1);
    assert!(
        outcome.error.is_none(),
        "a fully reconciled landing is not a failed train"
    );
    assert!(outcome.ejected.is_empty());
    assert_eq!(f.landed_members, vec![1]);
    assert!(f.retired.is_empty());
}

// Settled live policy refusals eject the member while preserving its hold.
// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_live_preland_gate_refusal_ejects() {
    let tmp = repo();
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.metadata_state = crate::forge::ChangeState::Open;
    forge.is_draft = true;
    let r = tmp.path();
    git(r, &["remote", "add", "origin", r.to_str().unwrap()]);
    let base = git(r, &["rev-parse", "main"]);
    git(r, &["checkout", "-b", "train"]);
    std::fs::write(r.join("train-change"), "train").unwrap();
    git(r, &["add", "train-change"]);
    git(r, &["commit", "-m", "train fixture"]);
    let tip = git(r, &["rev-parse", "HEAD"]);
    forge.metadata_head = tip.clone();
    crate::gate::merge_hold_core::write_hold(tmp.path(), 42, "new merge hold").unwrap();
    let mut host = LiveTrainForge {
        project_root: tmp.path().into(),
        main_worktree: tmp.path().into(),
        base: "main".into(),
        forge: &forge,
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    assert!(
        matches!(host.observe_gate(42), crate::forge_observed::Observed::Known(Err(why))
        if why.contains("it is a draft"))
    );
    let train = Built {
        base: base.clone(),
        tip,
        dropped: vec![],
        cars: vec![Car {
            member: member(42, "approved-head", "member"),
            commit: base.clone(),
            patch_id: "patch".into(),
        }],
    };
    let change = crate::forge::ChangeRef {
        id: 100,
        url: "fixture".into(),
        branch: "codex/task".into(),
        base: "main".into(),
        title: None,
    };
    let mut rails = ForgeRails::new(&mut host, "live-refusal".into());
    rails.opened.push((train.tip.clone(), change.clone()));
    rails.built.insert(change.id, train.clone());
    rails.live.push(change);
    let Landing::Refused(ejected) = rails.land(&train).unwrap() else {
        panic!("settled draft refusal must eject");
    };
    assert_eq!(ejected.len(), 1);
    assert_eq!(ejected[0].pr, 42);
    assert!(ejected[0].reason.contains("it is a draft"));
    assert!(rails.live.is_empty(), "refused train is retired");
    assert!(rails.landed_on_main.is_empty());
    drop(rails);
    assert_eq!(forge.closed.lock().unwrap()[0].0, 100);
    assert!(crate::gate::merge_hold_core::hold_is_open(tmp.path(), 42));
}

// trace:STORY-2401 | ai:codex
#[test]
fn story_2401_typed_member_gate_distinguishes_refusals_from_unavailable_reads() {
    let mut checks = FakeChecks::new();
    checks.labels = Err("HTTP 503".into());
    assert!(matches!(typed_gate_member_with(&mut checks, "main", 42),
        Err(MemberGateError::Unavailable(why)) if why.contains("HTTP 503")));
    checks.labels = Ok(None);
    checks.pin = None;
    assert!(matches!(typed_gate_member_with(&mut checks, "main", 42),
        Err(MemberGateError::Refused(why)) if why.contains("no approval")));
    checks.hold = Some("new hold".into());
    assert!(matches!(typed_gate_member_with(&mut checks, "main", 42),
        Err(MemberGateError::Refused(why)) if why.contains("merge hold")));
}

// trace:STORY-2401 BUG-2412 | ai:codex
fn crash_ready(marker: &std::path::Path, truth: &str) -> ! {
    // This file represents the forge receipt, outside the killed train's
    // journal. The parent SIGKILLs only after the fixture reaches the seam.
    std::fs::write(marker, truth).unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn pr_train_journal_killed_merge_and_eject_resume_without_replay() {
    let environment = aida_core::exec_context::Environment::capture();
    const CHILD: &str = "AIDA_TRAIN_JOURNAL_CRASH_CHILD";
    if let Ok(stage) = environment.var(CHILD).map(str::to_owned) {
        let dir = std::path::PathBuf::from(environment.var("AIDA_TRAIN_JOURNAL_TEST_DIR").unwrap());
        let _lock = journal::lock(&dir).unwrap();
        let mut forge = FakeForge::mains(&["main"]);
        forge.crash_marker = Some((stage.clone(), dir.join("ready")));
        if stage == "retire" {
            forge.gates.insert(1, Err("approval withdrawn".into()));
        }
        let mut rails = ForgeRails::new(&mut forge, "crash-fixture".into()).with_journal(&dir);
        let _ = drive(&mut rails, vec![member(1, "head1", "s")]);
        panic!("crash seam was not reached");
    }
    let name = std::thread::current()
        .name()
        .expect("libtest thread name")
        .to_string();
    for stage in ["merge", "retire"] {
        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env(CHILD, stage)
            .env("AIDA_TRAIN_JOURNAL_TEST_DIR", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dir.path().join("ready").exists() && std::time::Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("crash child exited early: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let reached = dir.path().join("ready").exists();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(reached, "child never reached {stage} seam");
        assert!(!status.success());
        let entries = journal::entries(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].train.tip, "car1");
        assert_eq!(entries[0].train.members()[0].head, "head1");
        let _lock = journal::lock(dir.path()).unwrap();
        let mut forge = FakeForge::mains(&["main"]);
        let receipt = std::fs::read_to_string(dir.path().join("ready")).unwrap();
        if receipt == "eject_decided" {
            forge.gates.insert(1, Err("approval withdrawn".into()));
        }
        forge.confirmation = Some(if receipt == "merged" {
            crate::forge_observed::Confirmed::Positive
        } else {
            crate::forge_observed::Confirmed::Negative
        });
        let mut rails = ForgeRails::new(&mut forge, "resume".into()).with_journal(dir.path());
        let out = rails.resume().unwrap();
        if stage == "merge" {
            assert_eq!(out.landed.len(), 1);
            assert!(out.ejected.is_empty());
            assert_eq!(
                rails.ports.recover_change(&entries[0]).unwrap().1,
                crate::forge::ChangeState::Merged
            );
        } else {
            assert_eq!(out.ejected.len(), 1);
            assert_eq!(out.ejected[0].pr, 1);
            assert!(out.landed.is_empty());
        }
        assert!(journal::entries(dir.path()).unwrap().is_empty());
        assert!(
            rails.resume().unwrap().landed.is_empty(),
            "completed journal must not replay"
        );
        drop(rails);
        assert_eq!(forge.opened, 0, "resume must not open another train");
        assert!(
            forge.merged.is_empty(),
            "completed merge must not run twice"
        );
        assert_eq!(forge.retired.len(), usize::from(stage == "retire"));
        assert_eq!(forge.landed_members.len(), usize::from(stage == "merge"));
    }
}

#[test]
fn pr_train_journal_unknown_recovery_and_write_failure_preserve_members() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = FakeForge::mains(&["main"]);
    {
        let mut rails = ForgeRails::new(&mut forge, "unknown".into()).with_journal(dir.path());
        rails
            .test(&[train_of("main", &[member(1, "head1", "s")])])
            .unwrap();
    }
    forge.confirmation = Some(crate::forge_observed::Confirmed::Unconfirmed);
    {
        let mut rails = ForgeRails::new(&mut forge, "resume".into()).with_journal(dir.path());
        assert!(rails.resume().is_err());
    }
    assert!(forge.retired.is_empty());
    assert!(forge.landed_members.is_empty());
    let missing = dir.path().join("missing");
    let mut rails = ForgeRails::new(&mut forge, "write-fails".into()).with_journal(&missing);
    assert!(rails
        .test(&[train_of("main", &[member(2, "head2", "s")])])
        .is_err());
    drop(rails);
    assert_eq!(forge.opened, 1, "failed intent write must precede open");
}

#[test]
fn pr_train_journal_completed_member_is_skipped_and_authority_is_not_restored() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = FakeForge::mains(&["main"]);
    let mut m = member(1, "head1", "s");
    m.release = Some(crate::gate::mandate_ship::MandateCover {
        id: "not-authority".into(),
        principal: "operator".into(),
    });
    let train = train_of("main", &[m, member(2, "head2", "s")]);
    let mut rails = ForgeRails::new(&mut forge, "member-progress".into()).with_journal(dir.path());
    rails.test(std::slice::from_ref(&train)).unwrap();
    journal::complete_member(1).unwrap();
    drop(rails);
    assert!(journal::entries(dir.path()).unwrap()[0].train.cars[0]
        .member
        .release
        .is_none());
    forge.confirmation = Some(crate::forge_observed::Confirmed::Positive);
    let mut rails = ForgeRails::new(&mut forge, "resume".into()).with_journal(dir.path());
    assert_eq!(rails.resume().unwrap().landed.len(), 2);
    drop(rails);
    assert_eq!(forge.landed_members, vec![2]);
}

// Exercise production checked cleanup, including a close that happened before
// the killed writer could save its completion receipt.
// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_live_cleanup_skips_closed_and_refuses_failed_or_moved() {
    let root = tempfile::tempdir().unwrap();
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.metadata_head = "head1".into();
    forge.metadata_state = crate::forge::ChangeState::Closed;
    let mut train = train_of("main", &[member(1, "head1", "s")]);
    train.tip = "head1".into();
    let change = crate::forge::ChangeRef {
        id: 901,
        url: String::new(),
        branch: "codex/task".into(),
        base: "main".into(),
        title: None,
    };
    let mut host = LiveTrainForge {
        project_root: root.path().into(),
        main_worktree: root.path().into(),
        base: "main".into(),
        forge: &forge,
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    host.retire_checked(&change, &train, "retire").unwrap();
    host.member_landed_checked(&change, &train, &train.cars[0], Some("landed1"))
        .unwrap();
    assert!(
        forge.closed.lock().unwrap().is_empty(),
        "a completed close must not be replayed"
    );
    forge.metadata_state = crate::forge::ChangeState::Open;
    forge.close_error = Some("HTTP 503 after close".into());
    let mut host = LiveTrainForge {
        project_root: root.path().into(),
        main_worktree: root.path().into(),
        base: "main".into(),
        forge: &forge,
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    assert!(host.retire_checked(&change, &train, "retire").is_err());
    assert!(host
        .member_landed_checked(&change, &train, &train.cars[0], None)
        .is_err());
    let attempts = forge.closed.lock().unwrap().len();
    forge.metadata_head = "new-head".into();
    let mut host = LiveTrainForge {
        project_root: root.path().into(),
        main_worktree: root.path().into(),
        base: "main".into(),
        forge: &forge,
        named: true,
        dispatch_ctx: None,
        dispatch_abort: None,
    };
    assert!(host.retire_checked(&change, &train, "retire").is_err());
    assert!(host
        .member_landed_checked(&change, &train, &train.cars[0], None)
        .is_err());
    assert_eq!(
        forge.closed.lock().unwrap().len(),
        attempts,
        "changed heads must stop before closing"
    );
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_recovery_has_no_terminal_consent_for_old_members() {
    let dir = tempfile::tempdir().unwrap();
    let recording = crate::forge::fake::RecordingForge::new();
    // The current command names #50; the old train carries supervised #41.
    let opts = TrainOptions {
        prs: &[50],
        all_ready: false,
        dry_run: false,
        local_ci: None,
        max: 10,
        no_pull: true,
    };
    let host = LiveTrainForge {
        project_root: dir.path().into(),
        main_worktree: dir.path().into(),
        base: "main".into(),
        forge: &recording,
        named: prs_named_on_command_line(&opts),
        dispatch_ctx: None,
        dispatch_abort: None,
    }
    .for_recovery();
    let mut checks = FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    checks.tty = tty_consents(true, host.named);
    let refusal = gate_member_with(&mut checks, "main", 41).unwrap_err();
    assert!(refusal.contains("TASK-9 is marked"), "{refusal}");
    assert!(checks.calls.contains(&"mandate"));
    let train = train_of("main", &fms(&[41]));
    let mut entry = journal::Entry::new(train, "train/old-all-ready-1".into());
    entry.change_id = Some(901);
    entry.step = journal::Step::Merge;
    journal::activate(dir.path(), entry).unwrap();
    journal::clear();
    let mut forge = FakeForge::mains(&["main"]);
    forge.gates.insert(41, Err(refusal));
    let mut rails = ForgeRails::new(&mut forge, "new".into()).with_journal(dir.path());
    assert_eq!(rails.resume().unwrap().ejected[0].pr, 41);
    drop(rails);
    assert!(forge.merged.is_empty());
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_recovered_ci_waits_and_retires_confirmed_red() {
    for step in [journal::Step::Gate, journal::Step::Merge] {
        for red in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/ci-1".into());
            entry.change_id = Some(901);
            entry.step = step.clone();
            journal::activate(dir.path(), entry).unwrap();
            journal::clear();
            let mut forge = FakeForge::mains(&["main"]);
            let settled = || {
                if red {
                    TrainCiReadiness::Red("failed check".into())
                } else {
                    TrainCiReadiness::Green
                }
            };
            forge.ci_states = Some(VecDeque::from([
                TrainCiReadiness::Pending(vec!["Build".into()]),
                settled(),
                settled(),
            ]));
            let mut rails = ForgeRails::new(&mut forge, "resume".into()).with_journal(dir.path());
            let out = rails.resume().unwrap();
            assert_eq!(out.ci_runs, 1);
            assert!(journal::entries(dir.path()).unwrap().is_empty());
            assert!(rails.resume().unwrap().landed.is_empty());
            drop(rails);
            assert_eq!(forge.retired.len(), usize::from(red));
            assert_eq!(forge.merged.len(), usize::from(!red));
            assert_eq!(forge.opened, 0);
        }
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_recovered_candidate_filter_and_empty_guard() {
    let recovered = Outcome {
        landed: vec![(1, "landed".into())],
        ejected: vec![Ejected {
            pr: 2,
            reason: "refused".into(),
        }],
        ..Outcome::default()
    };
    let mut candidates = vec![1, 2, 3, 3];
    filter_recovered_candidates(&mut candidates, &recovered).unwrap();
    assert_eq!(candidates, vec![3]);
    let mut candidates = vec![1, 2];
    filter_recovered_candidates(&mut candidates, &recovered).unwrap();
    assert!(candidates.is_empty());
    filter_recovered_candidates(&mut vec![], &recovered).unwrap();
    assert!(filter_recovered_candidates(&mut vec![], &Outcome::default()).is_err());
}

// Live recovery uses the real gh branch-search argv and real local push, with
// RecordingForge supplying only the forge create/metadata boundary.
// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_live_recover_reopens_ambiguity_and_pin_mismatches() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    let root = repo();
    let r = root.path();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        r,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let gh = r.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
case "$*" in
  'pr list --state all --head train/recover-1 --limit 1000 --json number') cat rows.json ;;
  *) echo "unexpected gh: $*" >&2; exit 1 ;;
esac
"#,
    );
    let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", &path),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);
    let tip = git(r, &["rev-parse", "HEAD"]);
    let mut train = train_of(&tip, &fms(&[1]));
    train.tip = tip.clone();
    let entry = journal::Entry::new(train, "train/recover-1".into());
    let mut forge = crate::forge::fake::RecordingForge::new();
    forge.opens_as = Some(901);
    forge.metadata_head = tip.clone();
    forge.metadata_branch = entry.branch.clone();
    write(r, "rows.json", "[]");
    let run = |forge: &crate::forge::fake::RecordingForge| {
        let mut host = LiveTrainForge {
            project_root: r.into(),
            main_worktree: r.into(),
            base: "main".into(),
            forge,
            named: false,
            dispatch_ctx: None,
            dispatch_abort: None,
        };
        host.recover_change(&entry)
    };
    let (change, state) = run(&forge).unwrap();
    assert_eq!(change.id, 901);
    assert_eq!(state, crate::forge::ChangeState::Open);
    assert_eq!(
        std::fs::read_to_string(r.join("gh-calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(forge.opened.lock().unwrap().len(), 1);
    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/train/recover-1"]),
        tip
    );
    write(r, "rows.json", r#"[{"number":901},{"number":902}]"#);
    assert!(run(&forge)
        .unwrap_err()
        .to_string()
        .contains("no unique forge change"));
    write(r, "rows.json", r#"[{"number":901}]"#);
    assert!(run(&forge).is_ok());
    forge.metadata_head = "different-head".into();
    assert!(run(&forge)
        .unwrap_err()
        .to_string()
        .contains("head/base changed"));
    forge.metadata_head = tip;
    forge.metadata_base = "other-base".into();
    assert!(run(&forge)
        .unwrap_err()
        .to_string()
        .contains("head/base changed"));
    forge.metadata_base = "main".into();
    forge.metadata_branch = "other-branch".into();
    assert!(run(&forge)
        .unwrap_err()
        .to_string()
        .contains("head/base changed"));
    assert_eq!(
        forge.opened.lock().unwrap().len(),
        1,
        "only two empty reads may reopen"
    );
    assert!(forge.closed.lock().unwrap().is_empty());
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_confirmation_fetches_refuse_when_checkpoint_fails() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    let root = repo();
    let r = root.path();
    crate::test_exec::write_executable(
        &r.join("git"),
        r#"#!/bin/sh
for arg in "$@"; do
    case "$arg" in
        fetch)
            printf '%s\n' "$*" >> fetch-calls
            mv intent intent-saved || exit 2
            printf 'unavailable' > intent
            exit 0 ;;
        rev-parse) printf '%040d\n' 1; exit 0 ;;
        cat-file) exit 1 ;;
    esac
done
exit 0
"#,
    );
    let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
    for fail_before_base in [true, false] {
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/fetch-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Merge;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if fail_before_base {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let observed = crate::forge_confirmation::rebase_landed(r, "main", &"b".repeat(40));
        assert!(
            matches!(observed, crate::forge_observed::Observed::Transient(ref why)
                if why.contains("could not persist train intent")),
            "{observed:?}"
        );
        let calls = std::fs::read_to_string(r.join("fetch-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(!fail_before_base));
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Merge);
        assert_eq!(saved[0].train.tip, "car1");
        std::fs::remove_file(&dir).unwrap();
        std::fs::remove_dir_all(r.join("intent-saved")).unwrap();
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_nested_member_reads_refuse_when_checkpoint_fails() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    use crate::forge::Forge;
    let root = repo();
    let r = root.path();
    git(
        r,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/test/train.git",
        ],
    );
    let gh = r.join("gh");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> gh-calls\nexit 1\n",
    );
    let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
    let forge = crate::forge::fake::RecordingForge::new();
    let meta = forge
        .change_metadata(1, &mut crate::network_retry::NoopSink)
        .unwrap();
    let mut checks = LiveChecks {
        project_root: r,
        main_worktree: r,
        forge: &forge,
        base: "main",
        named: false,
    };
    let dir = r.join("intent");
    std::fs::create_dir(&dir).unwrap();
    let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/nested-1".into());
    entry.step = journal::Step::Gate;
    journal::activate(&dir, entry).unwrap();
    let _scope = journal::Scope;
    // An earlier metadata checkpoint succeeded, but its response arrived after
    // the journal became unavailable. Each following remote read must refuse.
    std::fs::rename(&dir, r.join("intent-saved")).unwrap();
    std::fs::write(&dir, "unavailable").unwrap();
    assert!(checks
        .labels(1)
        .unwrap_err()
        .contains("could not persist train intent"));
    assert!(checks
        .message(1, &meta, "head1")
        .unwrap_err()
        .contains("could not persist train intent"));
    assert!(
        !r.join("gh-calls").exists(),
        "nested reads must not spawn gh"
    );
    let saved = journal::entries(&r.join("intent-saved")).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].step, journal::Step::Gate);
    assert_eq!(saved[0].train.tip, "car1");
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_hold_label_calls_refuse_when_checkpoint_fails() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    for seam in ["read", "write", "retry"] {
        let root = repo();
        let r = root.path();
        git(
            r,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/test/train.git",
            ],
        );
        crate::test_exec::write_executable(
            &r.join("gh"),
            r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
mv intent intent-saved || exit 2
printf 'unavailable' > intent
case "$*" in
  'pr view '*)
    printf '%s\n' '{"url":"https://github.com/test/train/pull/1","state":"OPEN","headRefOid":"head1","labels":[{"name":"aida:merge-hold-recorded"}]}'
    ;;
  *) echo 'error connecting to api.github.com' >&2; exit 1 ;;
esac
"#,
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/labels-1".into());
        entry.step = journal::Step::Gate;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        let error = if seam == "read" {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
            crate::merge_hold::fetch_pinned_change(r, crate::forge::ForgeKind::GitHub, 1)
                .unwrap_err()
        } else {
            // Unhold probes first; re-arm writes directly. Either response
            // makes the journal unavailable before the next call/retry.
            crate::merge_hold::sync_label(r, 1, seam == "retry")
                .unwrap_err()
                .to_string()
        };
        assert!(
            error.contains("could not persist train intent"),
            "{seam}: {error}"
        );
        let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap_or_default();
        assert_eq!(
            calls.lines().count(),
            usize::from(seam != "read"),
            "{seam}: {calls}"
        );
        if seam == "write" {
            assert!(calls.starts_with("pr view "), "no edit after the probe");
        }
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Gate);
        assert_eq!(saved[0].train.tip, "car1");
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_metadata_retry_refuses_when_checkpoint_fails() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    use crate::forge::Forge;
    #[derive(Default)]
    struct Retries(usize);
    impl crate::network_retry::RetrySink for Retries {
        fn on_retry(&mut self, _: &crate::network_retry::RetryEvent) {
            self.0 += 1;
        }
    }
    let root = repo();
    let r = root.path();
    std::fs::create_dir_all(r.join(".aida")).unwrap();
    write(
        r,
        ".aida/config.toml",
        "[orchestrator]\nretry_max_attempts = 3\nretry_base_delay_ms = 0\n",
    );
    let gh = r.join("gh");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
mv intent intent-saved || exit 2
printf 'unavailable' > intent
echo 'error connecting to api.github.com' >&2
exit 1
"#,
    );
    let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("PATH", &path),
        ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
    ]);
    for fail_before_first in [true, false] {
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/retry-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Merge;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if fail_before_first {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let forge = crate::forge::GitHubForge::new(r);
        let mut sink = Retries::default();
        let error = forge.change_metadata(901, &mut sink).unwrap_err();
        assert!(format!("{error:#}").contains("could not persist train intent"));
        assert_eq!(sink.0, usize::from(!fail_before_first));
        let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(!fail_before_first));
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(
            saved.len(),
            1,
            "failed checkpoint must retain recovery evidence"
        );
        assert_eq!(saved[0].step, journal::Step::Merge);
        assert_eq!(saved[0].train.tip, "car1");
        std::fs::remove_file(&dir).unwrap();
        std::fs::remove_dir_all(r.join("intent-saved")).unwrap();
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_hold_gate_poll_does_not_swallow_checkpoint_failure() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    for fail_before_first in [true, false] {
        let root = repo();
        let r = root.path();
        crate::test_exec::write_executable(
            &r.join("gh"),
            r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
mv intent intent-saved || exit 2
printf 'unavailable' > intent
printf '%s\n' '[{"name":"merge-hold-gate","bucket":"pending","workflow":"hold"}]'
"#,
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry =
            journal::Entry::new(train_of("main", &fms(&[1])), "train/hold-poll-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Gate;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if fail_before_first {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let change = crate::forge::ChangeRef {
            id: 901,
            url: String::new(),
            branch: "train/hold-poll-1".into(),
            base: "main".into(),
            title: None,
        };
        let error = crate::ci_gate::wait_hold_gate_green(
            &crate::forge::GitHubForge::new(r),
            &change,
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
        )
        .expect_err("journal refusal must not take the unsupported-rows success fallback");
        assert!(format!("{error:#}").contains("could not persist train intent"));
        let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(!fail_before_first));
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Gate);
        assert_eq!(saved[0].train.tip, "car1");
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_carryover_fetches_require_durable_intent() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    for fail_before_base in [true, false] {
        let root = repo();
        let r = root.path();
        crate::test_exec::write_executable(
            &r.join("git"),
            r#"#!/bin/sh
repo="$2"
case "$3" in
    fetch)
        printf '%s\n' "$*" >> "$repo/fetch-calls"
        mv "$repo/intent" "$repo/intent-saved" || exit 2
        printf 'unavailable' > "$repo/intent"
        exit 0 ;;
    rev-parse)
        case "$*" in
            *refs/remotes/origin/main*) printf '%040d\n' 1; exit 0 ;;
            *) exit 1 ;;
        esac ;;
    check-ref-format) exit 0 ;;
esac
exit 2
"#,
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/proof-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Gate;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if fail_before_base {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let error = crate::gate::review_carryover::prove_carry_over(
            r,
            1,
            "main",
            &"a".repeat(40),
            &"b".repeat(40),
        )
        .unwrap_err();
        if fail_before_base {
            assert!(error.reason().contains("could not persist train intent"));
            assert!(!error.is_deterministic());
        }
        // The approval policy also fetches a missing reviewed commit to read
        // its date. A failed checkpoint must leave that evidence unknown.
        let approval = crate::gate::review_verdict::RecordedVerdict {
            kind: crate::gate::review_verdict::VerdictKind::Approved,
            reviewed_sha: Some("a".repeat(40)),
            recorded_at: Some("2026-11-06T00:00:00Z".into()),
            ..Default::default()
        };
        let policy = crate::gate::approval::ship_approval_policy(r, &[], None, &[approval]);
        assert!(policy.reviewed_committed_at.is_none());
        let calls = std::fs::read_to_string(r.join("fetch-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(!fail_before_base));
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Gate);
        assert_eq!(saved[0].train.tip, "car1");
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_prunes_old_done_and_reports_corrupt_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/old-1".into());
    entry.step = journal::Step::Done;
    write(
        dir.path(),
        "old-1.json",
        &serde_json::to_string(&entry).unwrap(),
    );
    assert!(journal::entries(dir.path()).unwrap().is_empty());
    assert!(!dir.path().join("old-1.json").exists());
    write(dir.path(), "broken.json", "{");
    let err = journal::entries(dir.path()).unwrap_err().to_string();
    assert!(err.contains("broken.json"), "{err}");
    // The documented discard moves the file outside the live scan.
    std::fs::create_dir(dir.path().join("archive")).unwrap();
    std::fs::rename(
        dir.path().join("broken.json"),
        dir.path().join("archive/broken.json"),
    )
    .unwrap();
    assert!(journal::entries(dir.path()).unwrap().is_empty());
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_gitlab_ci_calls_require_durable_intent() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    use crate::forge::Forge;
    // Refuse before registration, watch, fallback rows, or the nested jobs read.
    for seam in ["initial", "registration", "watch", "jobs", "fallback-jobs"] {
        let root = repo();
        let r = root.path();
        let glab = r.join("glab");
        crate::test_exec::write_executable(
            &glab,
            r#"#!/bin/sh
printf '%s\n' "$*" >> glab-calls
seam=$(cat seam)
case "$*" in
  'api -X GET projects/:id/pipelines'*)
    if [ "$seam" = registration ] || [ "$seam" = jobs ] || [ -f watched ]; then
      mv intent intent-saved || exit 2
      printf unavailable > intent
    fi
    printf '%s\n' '[{"id":42,"status":"success"}]'
    ;;
  'ci status -b '*)
    if [ "$seam" = fallback-jobs ]; then
      touch watched
    else
      mv intent intent-saved || exit 2
      printf unavailable > intent
    fi
    exit 1
    ;;
  *) echo 'unexpected call' >&2; exit 2 ;;
esac
"#,
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("PATH", &path),
            ("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap()),
        ]);
        write(r, "seam", seam);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry = journal::Entry::new(train_of("main", &fms(&[1])), "train/gitlab-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Ci;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if seam == "initial" {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let change = crate::forge::ChangeRef {
            id: 901,
            url: String::new(),
            branch: "train/gitlab-1".into(),
            base: "main".into(),
            title: None,
        };
        let forge = crate::forge::GitLabForge::new(r);
        let error = if seam == "jobs" {
            forge.check_rows(&change).unwrap_err()
        } else {
            let mut host = LiveTrainForge {
                project_root: r.into(),
                main_worktree: r.into(),
                base: "main".into(),
                forge: &forge,
                named: true,
                dispatch_ctx: None,
                dispatch_abort: None,
            };
            host.wait_ci(&change, &train_of("main", &fms(&[1])))
                .expect_err("intent failure must suspend CI waiting")
        };
        assert!(format!("{error:#}").contains("could not persist train intent"));
        let calls = std::fs::read_to_string(r.join("glab-calls")).unwrap_or_default();
        let expected = match seam {
            "initial" => 0,
            "watch" => 2,
            "fallback-jobs" => 3,
            _ => 1,
        };
        assert_eq!(calls.lines().count(), expected, "{seam}: {calls}");
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Ci);
        assert_eq!(saved[0].train.tip, "car1");
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_verified_required_checks_require_durable_intent() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    for fail_before_first in [true, false] {
        let root = repo();
        let r = root.path();
        crate::test_exec::write_executable(
            &r.join("gh"),
            r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
cp intent/required-1.json checkpoint.json || exit 2
mv intent intent-saved || exit 2
printf unavailable > intent
printf '%s\n' '[{"name":"Build","bucket":"pass","workflow":"CI","link":""}]'
"#,
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut entry =
            journal::Entry::new(train_of("main", &fms(&[1])), "train/required-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Merge;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        let change = crate::forge::ChangeRef {
            id: 901,
            url: String::new(),
            branch: "train/required-1".into(),
            base: "main".into(),
            title: None,
        };
        if fail_before_first {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        } else {
            assert_eq!(
                crate::forge::github_required_checks(r, &change).unwrap(),
                vec!["Build".to_string()]
            );
            // Inspect the receipt captured by the forge stub at call time:
            // the nested read must have persisted its own call checkpoint.
            let checkpoint: serde_json::Value =
                serde_json::from_slice(&std::fs::read(r.join("checkpoint.json")).unwrap()).unwrap();
            assert_eq!(checkpoint["call"], "GitHub required checks");
        }
        let error = crate::forge::github_required_checks(r, &change).unwrap_err();
        assert!(format!("{error:#}").contains("could not persist train intent"));
        let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(!fail_before_first));
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Merge);
        assert_eq!(saved[0].change_id, Some(901));
        assert_eq!(saved[0].train.tip, "car1");
        assert_eq!(saved[0].train.members()[0].head, "head1");
    }
}

// trace:STORY-2401 | ai:codex
#[test]
fn pr_train_journal_commit_ci_pages_require_durable_intent() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    use crate::forge::Forge;
    let head = "a".repeat(40);
    for seam in ["initial", "page", "status"] {
        let root = repo();
        let r = root.path();
        let gh = r.join("gh");
        crate::test_exec::write_executable(
            &gh,
            r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
cp intent/commit-ci-1.json checkpoint.json || exit 2
mv intent intent-saved || exit 2
printf unavailable > intent
cat check-runs.json
"#,
        );
        // A full first page forces another page; an empty complete listing
        // forces the separate commit-status read. Neither may reuse the first
        // request's successful checkpoint after its destination is lost.
        let runs: Vec<_> = if seam == "page" {
            (0..100)
                .map(|i| {
                    serde_json::json!({
                        "name": format!("check-{i}"), "head_sha": head,
                        "status": "completed", "conclusion": "success"
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        write(
            r,
            "check-runs.json",
            &serde_json::json!({
                "total_count": if seam == "page" { 101 } else { 0 },
                "check_runs": runs
            })
            .to_string(),
        );
        let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("PATH", &path),
            ("AIDA_TEST_GH_BINARY", gh.to_str().unwrap()),
        ]);
        let dir = r.join("intent");
        std::fs::create_dir(&dir).unwrap();
        let mut train = train_of("main", &fms(&[1]));
        train.tip = head.clone();
        let mut entry = journal::Entry::new(train, "train/commit-ci-1".into());
        entry.change_id = Some(901);
        entry.step = journal::Step::Merge;
        journal::activate(&dir, entry).unwrap();
        let _scope = journal::Scope;
        if seam == "initial" {
            std::fs::rename(&dir, r.join("intent-saved")).unwrap();
            std::fs::write(&dir, "unavailable").unwrap();
        }
        let forge = crate::forge::GitHubForge::new(r);
        let error = forge.commit_checks(&head).unwrap_err();
        assert!(format!("{error:#}").contains("could not persist train intent"));
        let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap_or_default();
        assert_eq!(calls.lines().count(), usize::from(seam != "initial"));
        if seam != "initial" {
            assert!(calls.contains("check-runs?per_page=100&page=1"));
            let checkpoint: serde_json::Value =
                serde_json::from_slice(&std::fs::read(r.join("checkpoint.json")).unwrap()).unwrap();
            assert_eq!(checkpoint["call"], "commit CI API read");
        }
        let saved = journal::entries(&r.join("intent-saved")).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].step, journal::Step::Merge);
        assert_eq!(saved[0].change_id, Some(901));
        assert_eq!(saved[0].train.tip, head);
        assert_eq!(saved[0].train.members()[0].head, "head1");
    }
}
