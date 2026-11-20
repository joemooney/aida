//! STORY-2768 slice 3a: the housekeeping step of `aida land run`.
// trace:STORY-2768 | ai:claude

use super::*;

fn change(id: u64, branch: &str, title: &str) -> Pr {
    Pr {
        id,
        head: "1111111111111111111111111111111111111111".into(),
        base: "main".into(),
        branch: Some(branch.into()),
        title: title.into(),
        updated_at: None,
        hold_label: None,
        ci: None,
    }
}

fn lane(dir: &Path, branch: &str, spec: Option<&str>) -> DeadLane {
    DeadLane {
        lease: format!("lease-{branch}"),
        worktree: dir.to_path_buf(),
        branch: branch.into(),
        spec: spec.map(String::from),
        agent: Some("implementer".into()),
        unread: None,
    }
}

fn orphan(spec: &str) -> Orphan {
    Orphan {
        spec: spec.into(),
        ids: vec![spec.into()],
        ..Orphan::default()
    }
}

/// Counts every act and fails the ones a test asks it to.
#[derive(Default)]
struct Fake {
    patch: Option<PathBuf>,
    salvage_fails: bool,
    note_fails: bool,
    salvaged: Vec<String>,
    noted: Vec<(String, String)>,
    released: Vec<String>,
    reset: Vec<String>,
}

impl Keeper for Fake {
    fn salvage(&mut self, lane: &DeadLane) -> Result<Option<PathBuf>> {
        self.salvaged.push(lane.lease.clone());
        anyhow::ensure!(!self.salvage_fails, "the worktree could not be read");
        Ok(self.patch.clone())
    }
    fn note(&mut self, spec: &str, text: &str) -> Result<()> {
        anyhow::ensure!(!self.note_fails, "the store did not answer");
        self.noted.push((spec.into(), text.into()));
        Ok(())
    }
    fn already_saved(&mut self, _lane: &DeadLane, marker: &str) -> bool {
        self.noted.iter().any(|(_, text)| text.starts_with(marker))
    }
    fn release(&mut self, lane: &DeadLane) -> Result<()> {
        self.released.push(lane.lease.clone());
        Ok(())
    }
    fn reset(&mut self, spec: &str) -> Result<()> {
        self.reset.push(spec.into());
        Ok(())
    }
}

/// The tests of release and reset run with them turned on.
const ON: Unattended = Unattended::ReleaseAndReset;

fn reset(spec: &str, resume: Option<&str>) -> Step {
    Step::Reset {
        spec: spec.into(),
        ids: vec![spec.into()],
        resume: resume.map(String::from),
    }
}

fn patch_in(dir: &Path) -> PathBuf {
    let patch = dir.join("saved.patch");
    std::fs::write(&patch, "diff\n").unwrap();
    patch
}

// ---- the plan --------------------------------------------------------------

#[test]
fn a_dead_lane_is_salvaged_and_a_landing_branch_never_is() {
    let dir = Path::new("/w");
    let steps = plan(
        vec![
            lane(dir, "bug-7-fix", Some("BUG-7")),
            lane(dir, "train/20261114-1", None),
            lane(dir, "promote/integration-9", None),
        ],
        Vec::new(),
        &[],
    );
    assert_eq!(
        steps[0],
        Step::Salvage(lane(dir, "bug-7-fix", Some("BUG-7")))
    );
    for step in &steps[1..] {
        let Step::Skip { why, .. } = step else {
            panic!("a landing branch must be left alone: {step:?}")
        };
        assert!(why.contains("never salvaged"), "{why}");
    }
    assert_eq!(steps.len(), 3);
}

#[test]
fn a_spec_an_open_change_names_is_not_orphaned_whatever_its_branch() {
    let changes = [
        change(1, "bug-1-fix", "fix(x): a fix (BUG-1)"),
        change(2, "train/20261114-1", "train: 2 changes (STORY-2 BUG-3)"),
        change(
            3,
            "promote/integration-9",
            "chore(integration): promotion 9 (TASK-4)",
        ),
    ];
    let orphans = ["bug-1", "STORY-2", "BUG-3", "TASK-4"].map(orphan).to_vec();
    assert_eq!(plan(Vec::new(), orphans, &changes), Vec::new());
}

#[test]
fn an_orphan_with_no_change_goes_back_to_the_queue() {
    let steps = plan(Vec::new(), vec![orphan("BUG-5")], &[]);
    assert_eq!(
        steps,
        [Step::Reset {
            spec: "BUG-5".into(),
            ids: vec!["BUG-5".into()],
            resume: None
        }]
    );
}

#[test]
fn an_orphan_resumes_its_origin_branch_unless_a_change_or_the_landing_owns_it() {
    let mut spec = orphan("BUG-5");
    spec.branches = vec![
        "train/bug-5".into(),
        "bug-5-open".into(),
        "bug-5-unfinished".into(),
    ];
    // The open change is on one of the spec's branches but names another
    // spec, so the spec itself is still orphaned.
    let changes = [change(9, "bug-5-open", "fix(x): something else (BUG-6)")];
    let steps = plan(Vec::new(), vec![spec], &changes);
    assert_eq!(
        steps,
        [Step::Reset {
            spec: "BUG-5".into(),
            ids: vec!["BUG-5".into()],
            resume: Some("bug-5-unfinished".into())
        }]
    );
}

#[test]
fn an_orphan_held_on_another_host_or_possibly_worked_is_left_and_reported() {
    let mut remote = orphan("BUG-5");
    remote.elsewhere = Some("dell1".into());
    // The other host wins even over a tag a human would have to look at.
    remote.tags = vec!["supervised".into()];
    let mut busy = orphan("BUG-6");
    busy.maybe_worked = Some("a live fan-out names it".into());
    let steps = plan(Vec::new(), vec![remote, busy], &[]);
    let [Step::Skip { what, why }, Step::Skip { why: busy, .. }] = &steps[..] else {
        panic!("{steps:?}")
    };
    assert_eq!(what, "BUG-5");
    assert!(why.contains("dell1"), "{why}");
    assert!(busy.contains("fan-out"), "{busy}");
}

#[test]
fn a_spec_merged_on_integration_and_not_promoted_is_never_reset() {
    let mut merged = orphan("BUG-25");
    merged.awaits_promotion = true;
    // Merged work wins over a tag a human would look at and over a branch.
    merged.tags = vec!["architecture".into()];
    merged.branches = vec!["bug-25-work".into()];
    let steps = plan(Vec::new(), vec![merged], &[]);
    let [Step::Skip { what, why }] = &steps[..] else {
        panic!("{steps:?}")
    };
    assert_eq!(what, "BUG-25");
    assert!(why.contains("waits for a promotion"), "{why}");

    let ids = vec!["BUG-25".to_string(), "bug-b6b1-349".to_string()];
    let subjects = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(named_in(
        &ids,
        &subjects(&["[AI:claude] chore(integration): merge PR #9 a fix (BUG-25)"])
    ));
    assert!(named_in(&ids, &subjects(&["fix(x): y (Refs bug-25)"])));
    assert!(named_in(&ids, &subjects(&["fix(x): y (BUG-B6B1-349)"])));
    assert!(!named_in(
        &ids,
        &subjects(&["fix(x): y (BUG-255)", "BUG-2 and BUG-5"])
    ));
    assert!(!named_in(&ids, &[]));
}

#[test]
fn only_a_live_driver_keeps_an_orphan_from_being_reset() {
    use aida_core::driver_registry::DerivedDriver as Driver;
    // Idle and queued say nothing is working the spec: a true orphan.
    for none in [
        None,
        Some(Driver::Idle { since_secs: None }),
        Some(Driver::Idle {
            since_secs: Some(18042),
        }),
        Some(Driver::Queued {
            batch: "night".into(),
        }),
    ] {
        assert_eq!(worked_by(false, false, none.as_ref()), None, "{none:?}");
    }
    for (live, label) in [
        (Driver::Relay { run: "r1".into() }, "relay:r1"),
        (Driver::Drain { pid: 42 }, "drain:42"),
        (
            Driver::Agent {
                who: "codex".into(),
            },
            "agent:codex",
        ),
    ] {
        let why = worked_by(false, false, Some(&live)).expect("a live driver");
        assert_eq!(why, format!("{label} is working it"));
    }
    let idle = Driver::Idle { since_secs: None };
    assert!(worked_by(true, false, Some(&idle))
        .unwrap()
        .contains("fan-out"));
    assert!(worked_by(false, true, Some(&idle))
        .unwrap()
        .contains("subagent"));
}

#[test]
fn an_orphan_a_human_must_reset_is_listed_and_never_reset() {
    for tag in ["supervised", "operator-only", "architecture"] {
        let mut spec = orphan("STORY-8");
        spec.tags = vec!["effort:open:1h".into(), tag.into()];
        let steps = plan(Vec::new(), vec![spec], &[]);
        let [Step::Attention { what, why }] = &steps[..] else {
            panic!("{tag}: {steps:?}")
        };
        assert_eq!(what, "STORY-8");
        assert!(why.contains(tag), "{why}");
    }
}

// ---- the acts --------------------------------------------------------------

#[test]
fn a_salvage_saves_records_and_releases_once_each() {
    let dir = tempfile::tempdir().unwrap();
    let patch = patch_in(dir.path());
    let mut fake = Fake {
        patch: Some(patch.clone()),
        ..Fake::default()
    };
    let lane = lane(dir.path(), "bug-7-fix", Some("BUG-7"));
    let notes = keep(&[Step::Salvage(lane.clone())], &mut fake, false, ON);
    assert_eq!(fake.salvaged, [lane.lease.clone()]);
    assert_eq!(fake.released, [lane.lease]);
    let [(spec, text)] = &fake.noted[..] else {
        panic!("{:?}", fake.noted)
    };
    assert_eq!(spec, "BUG-7");
    assert!(text.contains(&patch.display().to_string()), "{text}");
    assert!(
        notes[0].contains("lease released, worktree left alone"),
        "{notes:?}"
    );
    assert!(fake.reset.is_empty());
}

#[test]
fn a_failed_or_missing_patch_releases_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let lane = lane(dir.path(), "bug-7-fix", Some("BUG-7"));
    let cases = [
        Fake {
            salvage_fails: true,
            ..Fake::default()
        },
        // The salvage named a patch that is not on disk.
        Fake {
            patch: Some(dir.path().join("absent.patch")),
            ..Fake::default()
        },
        // Nothing uncommitted is left.
        Fake::default(),
    ];
    for mut fake in cases {
        let notes = keep(&[Step::Salvage(lane.clone())], &mut fake, false, ON);
        assert_eq!(fake.salvaged.len(), 1);
        assert!(fake.released.is_empty(), "{notes:?}");
        assert!(fake.noted.is_empty(), "{notes:?}");
        assert!(notes[0].contains("lease is kept"), "{notes:?}");
    }
}

#[test]
fn a_patch_that_cannot_be_recorded_keeps_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let patch = patch_in(dir.path());
    let mut fake = Fake {
        patch: Some(patch.clone()),
        note_fails: true,
        ..Fake::default()
    };
    let lane = lane(dir.path(), "bug-7-fix", Some("BUG-7"));
    let notes = keep(&[Step::Salvage(lane)], &mut fake, false, ON);
    // The lease is the only thing that still leads to the saved work.
    assert!(fake.released.is_empty(), "{notes:?}");
    assert!(
        notes[0].contains("could not be recorded on BUG-7") && notes[0].contains("lease is kept"),
        "{notes:?}"
    );
    assert!(notes[0].contains(&patch.display().to_string()), "{notes:?}");
}

#[test]
fn a_lane_with_no_spec_to_record_on_keeps_its_lease_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let patch = patch_in(dir.path());
    let mut fake = Fake {
        patch: Some(patch.clone()),
        ..Fake::default()
    };
    let notes = keep(
        &[Step::Salvage(lane(dir.path(), "scratch-work", None))],
        &mut fake,
        false,
        ON,
    );
    assert_eq!(fake.salvaged.len(), 1);
    assert!(
        fake.released.is_empty() && fake.noted.is_empty(),
        "{notes:?}"
    );
    assert!(
        notes[0].contains("names no spec") && notes[0].contains("a human releases it"),
        "{notes:?}"
    );
    assert!(notes[0].contains(&patch.display().to_string()), "{notes:?}");
}

// A stale dirty lane and its In-Progress spec arrive in one pass. The spec
// goes back to the queue only when the lane's work was saved and recorded.
#[test]
fn a_spec_is_not_reset_when_its_dead_lanes_work_was_not_saved_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let patch = patch_in(dir.path());
    // The lane's lease is scoped to an alias of the spec the orphan names.
    let steps = [
        Step::Salvage(lane(dir.path(), "bug-7-fix", Some("bug-b6b1-7"))),
        Step::Reset {
            spec: "BUG-7".into(),
            ids: vec!["BUG-7".into(), "BUG-B6B1-7".into()],
            resume: None,
        },
        reset("BUG-8", None),
    ];
    let failing = [
        // The work could not be captured.
        Fake {
            salvage_fails: true,
            ..Fake::default()
        },
        // The salvage named a patch that is not on disk.
        Fake {
            patch: Some(dir.path().join("absent.patch")),
            ..Fake::default()
        },
        // The patch is on disk and its path could not be recorded.
        Fake {
            patch: Some(patch.clone()),
            note_fails: true,
            ..Fake::default()
        },
    ];
    for mut fake in failing {
        let notes = keep(&steps, &mut fake, false, ON);
        assert!(fake.released.is_empty(), "{notes:?}");
        // The unrelated orphan is still reset; the lane's spec is not.
        assert_eq!(fake.reset, ["BUG-8"], "{notes:?}");
        assert!(
            notes[1].starts_with("BUG-7: not reset") && notes[1].contains("was not saved"),
            "{notes:?}"
        );
    }

    // Saved and recorded: the lease goes and the spec is reset.
    let mut fake = Fake {
        patch: Some(patch),
        ..Fake::default()
    };
    let notes = keep(&steps, &mut fake, false, ON);
    assert_eq!(fake.released.len(), 1, "{notes:?}");
    assert_eq!(fake.reset, ["BUG-7", "BUG-8"], "{notes:?}");
}

#[test]
fn a_reset_records_the_branch_to_resume_first_and_resets_once() {
    let mut fake = Fake::default();
    let steps = [
        reset("BUG-5", Some("bug-5-unfinished")),
        reset("BUG-6", None),
    ];
    let notes = keep(&steps, &mut fake, false, ON);
    assert_eq!(fake.reset, ["BUG-5", "BUG-6"]);
    let [(spec, text)] = &fake.noted[..] else {
        panic!("{:?}", fake.noted)
    };
    assert_eq!(spec, "BUG-5");
    assert!(text.contains("--branch bug-5-unfinished"), "{text}");
    assert!(text.contains("not a fresh start"), "{text}");
    assert_eq!(notes.len(), 2);

    // A branch that could not be recorded would be picked up as fresh work.
    let mut fake = Fake {
        note_fails: true,
        ..Fake::default()
    };
    let notes = keep(&steps[..1], &mut fake, false, ON);
    assert!(fake.reset.is_empty(), "{notes:?}");
    assert!(notes[0].contains("not reset"), "{notes:?}");
}

#[test]
fn a_dry_run_and_the_reported_steps_call_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let steps = [
        Step::Salvage(lane(dir.path(), "bug-7-fix", Some("BUG-7"))),
        reset("BUG-5", Some("bug-5-unfinished")),
    ];
    let mut fake = Fake::default();
    let notes = keep(&steps, &mut fake, true, ON);
    assert_eq!(notes.len(), 2);
    assert!(
        notes.iter().all(|note| note.contains("(dry run)")),
        "{notes:?}"
    );

    let reported = [
        Step::Skip {
            what: "BUG-1".into(),
            why: "its latest session is on another host (dell1)".into(),
        },
        Step::Attention {
            what: "STORY-8".into(),
            why: "tagged `supervised`".into(),
        },
    ];
    let notes = keep(&reported, &mut fake, false, ON);
    assert!(notes[0].contains("left alone") && notes[1].contains("needs a human"));
    assert!(fake.salvaged.is_empty() && fake.released.is_empty());
    assert!(fake.noted.is_empty() && fake.reset.is_empty());
}

#[test]
fn spec_branches_match_the_id_alone_or_followed_by_a_dash() {
    let remote: Vec<String> = [
        "bug-5",
        "bug-5-fix",
        "bug-50-other",
        "bug-b6b1278890774ef993d2ebc327862470-314-x",
        "main",
    ]
    .map(String::from)
    .to_vec();
    let ids = [
        "BUG-5".to_string(),
        "BUG-b6b1278890774ef993d2ebc327862470-314".to_string(),
    ];
    assert_eq!(
        spec_branches(&ids, &remote),
        [
            "bug-5",
            "bug-5-fix",
            "bug-b6b1278890774ef993d2ebc327862470-314-x"
        ]
    );
}

// ---- what the plan is decided from ------------------------------------------

fn claim(scope: &str, node: &str, host: &str) -> crate::coordination::Claim {
    serde_json::from_value(serde_json::json!({
        "scope": scope,
        "node_id": node,
        "clone_path": "/home/other/aida",
        "host": host,
        "pid": 4242,
        "agent": "implementer",
        "started_at": "2026-11-14T10:00:00Z",
        "heartbeat_at": "2026-11-14T10:00:00Z",
        "ttl_secs": 3600,
    }))
    .unwrap()
}

// A live claim another host took on the spec, with no lease here: the spec
// is held there. The claim's scope is the spec; its node id is the clone.
#[test]
fn a_live_claim_on_another_host_holds_the_spec_by_its_scope() {
    let ids = vec!["BUG-5".to_string(), "bug-b6b1-5".to_string()];
    let held = |claims: &[crate::coordination::Claim]| held_elsewhere(&ids, &[], claims, "here");
    assert_eq!(
        held(&[claim("BUG-5", "node-dell1", "dell1")]).as_deref(),
        Some("dell1")
    );
    // By any id the spec answers to, whatever the case.
    assert_eq!(
        held(&[claim("BUG-B6B1-5", "node-dell1", "dell1")]).as_deref(),
        Some("dell1")
    );
    // A clone whose node id happens to read like the spec holds nothing.
    assert_eq!(held(&[claim("STORY-9", "BUG-5", "dell1")]), None);
    // A claim on this host is this host's own lease table to answer for.
    assert_eq!(held(&[claim("BUG-5", "node-here", "here")]), None);
    assert_eq!(held(&[]), None);

    // Through the plan: the held spec is left alone, never reset.
    let mut spec = orphan("BUG-5");
    spec.elsewhere = held(&[claim("BUG-5", "node-dell1", "dell1")]);
    let steps = plan(Vec::new(), vec![spec], &[]);
    assert!(
        matches!(&steps[..], [Step::Skip { why, .. }] if why.contains("dell1")),
        "{steps:?}"
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare origin holding `main`, and a clone of it with one commit.
fn origin_and_clone(dir: &Path) -> (PathBuf, PathBuf) {
    let origin = dir.join("origin.git");
    let clone = dir.join("clone");
    std::fs::create_dir_all(&clone).unwrap();
    git(dir, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(&clone, &["init", "-q", "-b", "main"]);
    git(&clone, &["config", "user.email", "t@example.invalid"]);
    git(&clone, &["config", "user.name", "T"]);
    git(&clone, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &clone,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    git(&clone, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);
    (origin, clone)
}

/// Commits made in the bare origin itself, as another clone's push would.
fn commit_on_origin(origin: &Path, branch: &str, from: &str, subjects: &[String]) {
    use std::io::Write as _;
    let mut stream = String::new();
    for (n, subject) in subjects.iter().enumerate() {
        stream.push_str(&format!(
            "commit refs/heads/{branch}\ncommitter T <t@example.invalid> {} +0000\ndata {}\n{subject}\n",
            1_700_000_000 + n,
            subject.len()
        ));
        if n == 0 {
            stream.push_str(&format!("from refs/heads/{from}\n"));
        }
        stream.push('\n');
    }
    let mut child = crate::test_git::git_command()
        .current_dir(origin)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

// A branch pushed to origin after this clone last fetched is still found:
// origin is asked, the clone's remembered refs are not.
#[test]
fn origin_is_asked_for_its_branches_not_the_clones_last_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = origin_and_clone(dir.path());
    commit_on_origin(&origin, "bug-5-unfinished", "main", &["wip".to_string()]);
    let remembered = git(
        &clone,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=3)",
            "refs/remotes/origin",
        ],
    );
    assert!(!remembered.contains("bug-5-unfinished"), "{remembered}");

    let evidence = origin_evidence(&clone).unwrap();
    assert!(
        evidence.branches.contains(&"bug-5-unfinished".to_string()),
        "{evidence:?}"
    );
    // Origin answered and has no integration branch: nothing is waiting.
    assert_eq!(evidence.unpromoted, Vec::<String>::new());
    assert_eq!(
        spec_branches(&["BUG-5".to_string()], &evidence.branches),
        ["bug-5-unfinished"]
    );
}

// The whole of what integration carries is read, however long it is, and
// from origin as it is now.
#[test]
fn integration_history_is_read_whole_past_two_thousand_commits() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = origin_and_clone(dir.path());
    let mut subjects = vec!["fix(x): the oldest merged work (BUG-25)".to_string()];
    subjects.extend((0..2100).map(|n| format!("chore: filler {n}")));
    commit_on_origin(&origin, "integration", "main", &subjects);

    let evidence = origin_evidence(&clone).unwrap();
    assert_eq!(evidence.unpromoted.len(), 2101);
    assert!(named_in(&["BUG-25".to_string()], &evidence.unpromoted));
    assert!(!named_in(&["BUG-26".to_string()], &evidence.unpromoted));
}

// An origin that cannot be read is an error, never "no branches" or
// "nothing merged": the caller then resets nothing.
#[test]
fn an_unreadable_origin_is_an_error_and_not_an_empty_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = origin_and_clone(dir.path());
    commit_on_origin(
        &origin,
        "integration",
        "main",
        &["fix(x): merged (BUG-25)".to_string()],
    );
    assert_eq!(origin_evidence(&clone).unwrap().unpromoted.len(), 1);

    // Integration is there and main is not: nothing to compare it with.
    git(&origin, &["update-ref", "-d", "refs/heads/main"]);
    let error = origin_evidence(&clone).unwrap_err();
    assert!(format!("{error:#}").contains("no `main`"), "{error:#}");

    // Origin is gone altogether.
    std::fs::rename(&origin, dir.path().join("moved.git")).unwrap();
    let error = origin_evidence(&clone).unwrap_err();
    assert!(
        format!("{error:#}").contains("origin's branches could not be listed"),
        "{error:#}"
    );
}

/// Whether this process can be refused a read by file mode (root cannot).
fn modes_are_enforced(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let probe = dir.join("mode-probe");
    std::fs::write(&probe, "x").unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o000)).unwrap();
    let enforced = std::fs::read(&probe).is_err();
    std::fs::remove_file(&probe).unwrap();
    enforced
}

// The salvage the lease is released on captures everything or writes no
// patch. Real git, failing on a file it cannot read.
#[test]
fn a_salvage_that_cannot_capture_every_file_writes_no_patch() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    if !modes_are_enforced(dir.path()) {
        return;
    }
    let root = dir.path().join("root");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.email", "t@example.invalid"]);
    git(&tree, &["config", "user.name", "T"]);
    std::fs::write(tree.join("tracked.txt"), "one\n").unwrap();
    git(&tree, &["add", "tracked.txt"]);
    git(&tree, &["commit", "-q", "-m", "first"]);
    let salvage = || {
        crate::worktree_gc::salvage_worktree_patch_complete(
            &root,
            "BUG-7",
            Some("implementer"),
            &tree,
        )
    };
    let patches = || {
        std::fs::read_dir(root.join(".aida").join("salvage"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    };
    let mode = |name: &str, mode: u32| {
        std::fs::set_permissions(tree.join(name), std::fs::Permissions::from_mode(mode)).unwrap()
    };

    // A tracked edit git cannot read.
    std::fs::write(tree.join("tracked.txt"), "one\ntwo\n").unwrap();
    mode("tracked.txt", 0o000);
    let error = salvage().unwrap_err();
    assert!(
        format!("{error:#}").contains("no patch was written"),
        "{error:#}"
    );
    assert_eq!(patches(), 0);
    mode("tracked.txt", 0o644);

    // An untracked file git cannot read, beside a readable tracked edit.
    std::fs::write(tree.join("new.txt"), "new\n").unwrap();
    mode("new.txt", 0o000);
    let error = salvage().unwrap_err();
    assert!(
        format!("{error:#}").contains("no patch was written"),
        "{error:#}"
    );
    assert_eq!(patches(), 0);
    // The lossy form writes a patch that looks whole and is missing the file.
    let partial = crate::worktree_gc::salvage_worktree_patch(&root, "BUG-7", None, &tree)
        .unwrap()
        .expect("the lossy salvage still writes a patch");
    assert!(!std::fs::read_to_string(&partial).unwrap().contains("+new"));
    std::fs::remove_file(&partial).unwrap();
    mode("new.txt", 0o644);

    // Everything readable: one patch, holding both.
    let patch = salvage().unwrap().expect("a patch");
    let text = std::fs::read_to_string(&patch).unwrap();
    assert!(text.contains("+two") && text.contains("+new"), "{text}");
    assert_eq!(patches(), 1);
}

// The patch a lease is released on recovers the work: applied to a clean
// checkout of HEAD it reproduces staged additions, staged edits, an
// unstaged edit and an untracked file.
// trace:STORY-2768 | ai:claude
#[test]
fn a_complete_salvage_patch_applies_back_to_the_same_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let tree = dir.path().join("tree");
    let clean = dir.path().join("clean");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.email", "t@example.invalid"]);
    git(&tree, &["config", "user.name", "T"]);
    for name in ["edited.txt", "mixed.txt", "kept.txt"] {
        std::fs::write(tree.join(name), "one\n").unwrap();
    }
    git(&tree, &["add", "."]);
    git(&tree, &["commit", "-q", "-m", "first"]);
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            tree.to_str().unwrap(),
            clean.to_str().unwrap(),
        ],
    );

    // A staged new file, a staged edit, an unstaged edit, and an untracked
    // file.
    std::fs::write(tree.join("added.txt"), "added\n").unwrap();
    std::fs::write(tree.join("edited.txt"), "one\nstaged\n").unwrap();
    git(&tree, &["add", "added.txt", "edited.txt"]);
    std::fs::write(tree.join("mixed.txt"), "one\nunstaged\n").unwrap();
    std::fs::write(tree.join("untracked.txt"), "untracked\n").unwrap();

    let patch = crate::worktree_gc::salvage_worktree_patch_complete(&root, "BUG-7", None, &tree)
        .unwrap()
        .expect("a patch");
    git(&clean, &["apply", "--check", patch.to_str().unwrap()]);
    git(&clean, &["apply", patch.to_str().unwrap()]);
    for name in [
        "added.txt",
        "edited.txt",
        "mixed.txt",
        "kept.txt",
        "untracked.txt",
    ] {
        assert_eq!(
            std::fs::read_to_string(clean.join(name)).unwrap(),
            std::fs::read_to_string(tree.join(name)).unwrap(),
            "{name} after applying the salvage patch"
        );
    }
}

// F1 (round 7): a staged version that is in neither HEAD nor the working
// tree is not held by the one HEAD-to-worktree diff, so the complete salvage
// refuses rather than report a patch as the whole work. A path staged and
// then reverted in the working tree is the sharpest case: the patch would be
// empty for it.
// trace:TASK-3796 | ai:claude
#[test]
fn a_complete_salvage_refuses_staged_content_the_worktree_no_longer_has() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.email", "t@example.invalid"]);
    git(&tree, &["config", "user.name", "T"]);
    for name in ["reverted.txt", "mixed.txt", "plain.txt"] {
        std::fs::write(tree.join(name), "one\n").unwrap();
    }
    git(&tree, &["add", "."]);
    git(&tree, &["commit", "-q", "-m", "first"]);
    let salvage =
        || crate::worktree_gc::salvage_worktree_patch_complete(&root, "BUG-7", None, &tree);
    let patches = || {
        std::fs::read_dir(root.join(".aida").join("salvage"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    };

    // Staged, then the working tree put back to HEAD.
    std::fs::write(tree.join("reverted.txt"), "one\nstaged only\n").unwrap();
    git(&tree, &["add", "reverted.txt"]);
    std::fs::write(tree.join("reverted.txt"), "one\n").unwrap();
    let error = format!("{:#}", salvage().unwrap_err());
    assert!(
        error.contains("reverted.txt") && error.contains("no patch was written"),
        "{error}"
    );
    assert_eq!(patches(), 0);
    git(&tree, &["reset", "-q", "--", "reverted.txt"]);

    // Staged, then changed again.
    std::fs::write(tree.join("mixed.txt"), "one\nstaged\n").unwrap();
    git(&tree, &["add", "mixed.txt"]);
    std::fs::write(tree.join("mixed.txt"), "one\nstaged\nagain\n").unwrap();
    let error = format!("{:#}", salvage().unwrap_err());
    assert!(error.contains("mixed.txt"), "{error}");
    assert_eq!(patches(), 0);

    // Staged and unstaged edits to different paths are held whole.
    git(&tree, &["reset", "-q", "--", "mixed.txt"]);
    std::fs::write(tree.join("plain.txt"), "one\nstaged\n").unwrap();
    git(&tree, &["add", "plain.txt"]);
    salvage().unwrap().expect("a patch");
    assert_eq!(patches(), 1);

    // The lossy form, used where nothing is released on it, still writes.
    std::fs::write(tree.join("plain.txt"), "one\nstaged\nagain\n").unwrap();
    crate::worktree_gc::salvage_worktree_patch(&root, "BUG-7", None, &tree)
        .unwrap()
        .expect("the lossy salvage still writes a patch");
}

// A lease file that cannot be removed is reported as not released.
#[test]
fn a_lease_that_cannot_be_removed_is_not_reported_released() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    if !modes_are_enforced(dir.path()) {
        return;
    }
    let root = dir.path();
    let path = crate::lease_path(root, "lease-1");
    let sessions = path.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(&path, "id = \"lease-1\"\n").unwrap();
    let mode = |mode: u32| {
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(mode)).unwrap()
    };

    mode(0o555);
    let error = super::super::run::release_lease(root, "lease-1").unwrap_err();
    mode(0o755);
    assert!(
        format!("{error:#}").contains("could not be removed"),
        "{error:#}"
    );
    assert!(path.exists());

    super::super::run::release_lease(root, "lease-1").unwrap();
    assert!(!path.exists());
}

// F5 (TASK-3796): a lease that is not live whose worktree could not be read
// (Unknown) is reported, nothing is salvaged or released, and its spec, In
// Progress with no session, is kept and not reset. A live lane is not a lane
// of this step at all. From the ps rows through the plan to the acts.
// trace:TASK-3796 | ai:claude
#[test]
fn an_unreadable_lane_keeps_its_spec_from_survey_to_keep() {
    use crate::dispatch_health_ps::DispatchState;
    use crate::leases::bug_1680_salvage_main_tests::mock_ps_row;
    let dir = tempfile::tempdir().unwrap();
    let dispatch = |state| crate::ps_integrate::PsDispatch {
        state,
        hint: Some("worktree state could not be read: git status failed".into()),
        dirty: false,
        ahead_of_main: 0,
        untracked_only: false,
        commit_movement_from: None,
    };
    let mut unread = mock_ps_row("lease-unread", "BUG-9", dir.path().join("unread"));
    unread.dispatch = Some(dispatch(DispatchState::Unknown));
    let mut live = mock_ps_row("lease-live", "BUG-10", dir.path().join("live"));
    live.state = aida_core::liveness::LeaseState::Live;
    live.dispatch = Some(dispatch(DispatchState::Unknown));
    let mut dead = mock_ps_row("lease-dead", "BUG-11", dir.path().join("dead"));
    dead.dispatch = Some(dispatch(DispatchState::Salvageable));

    let lanes = lanes_of(&[unread, live, dead]);
    assert_eq!(
        lanes
            .iter()
            .map(|lane| lane.lease.as_str())
            .collect::<Vec<_>>(),
        ["lease-unread", "lease-dead"]
    );
    assert!(lanes[0]
        .unread
        .as_deref()
        .unwrap()
        .contains("could not be read"));
    assert_eq!(lanes[1].unread, None);

    let steps = plan(lanes, vec![orphan("bug-9"), orphan("BUG-12")], &[]);
    let mut fake = Fake {
        patch: Some(patch_in(dir.path())),
        ..Fake::default()
    };
    let notes = keep(&steps, &mut fake, false, ON);
    assert_eq!(fake.salvaged, ["lease-dead"], "{notes:#?}");
    assert_eq!(fake.released, ["lease-dead"], "{notes:#?}");
    assert_eq!(fake.reset, ["BUG-12"], "{notes:#?}");
    let joined = notes.join("\n");
    assert!(
        joined.contains("lane BUG-9") && joined.contains("its spec is kept"),
        "{joined}"
    );
    assert!(
        joined.contains("bug-9: needs a human") && joined.contains("not reset"),
        "{joined}"
    );
}

// ---- the knob ----------------------------------------------------------------

// Release and reset are on only for the boolean `true`.
// trace:TASK-3796 | ai:claude
#[test]
fn automatic_release_and_reset_are_off_unless_set_to_true() {
    let read = |text: &str| Unattended::from_config(Some(&toml::from_str(text).unwrap()));
    assert_eq!(Unattended::from_config(None), Unattended::ReportOnly);
    assert_eq!(read(""), Unattended::ReportOnly);
    assert_eq!(read("[land]\nmax_lanes = 2\n"), Unattended::ReportOnly);
    for off in ["false", "\"true\"", "1", "\"on\""] {
        assert_eq!(
            read(&format!("[land]\nhousekeeping_auto_release = {off}\n")),
            Unattended::ReportOnly,
            "{off}"
        );
    }
    assert_eq!(
        read("[land]\nhousekeeping_auto_release = true\n"),
        Unattended::ReleaseAndReset
    );
    // A project with no config file is off.
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Unattended::load(dir.path()), Unattended::ReportOnly);
}

/// Salvages for real, releases a real lease file, and counts what it does.
struct Real<'a> {
    root: &'a Path,
    noted: Vec<(String, String)>,
    salvaged: usize,
    released: usize,
    reset: Vec<String>,
}

impl Keeper for Real<'_> {
    fn salvage(&mut self, lane: &DeadLane) -> Result<Option<PathBuf>> {
        self.salvaged += 1;
        crate::worktree_gc::salvage_worktree_patch_complete(
            self.root,
            lane.spec.as_deref().unwrap_or(&lane.branch),
            lane.agent.as_deref(),
            &lane.worktree,
        )
    }
    fn note(&mut self, spec: &str, text: &str) -> Result<()> {
        self.noted.push((spec.into(), text.into()));
        Ok(())
    }
    fn already_saved(&mut self, _lane: &DeadLane, marker: &str) -> bool {
        self.noted.iter().any(|(_, text)| text.starts_with(marker))
    }
    fn release(&mut self, lane: &DeadLane) -> Result<()> {
        self.released += 1;
        super::super::run::release_lease(self.root, &lane.lease)
    }
    fn reset(&mut self, spec: &str) -> Result<()> {
        self.reset.push(spec.into());
        Ok(())
    }
}

// The scope ruling of 2026-11-15: with the knob off, a fully salvageable dead
// lane is saved and recorded once, and nothing else happens: its lease file
// stays, release (which removes the file and revokes the grant) is never
// called, and no spec, the lane's own or an unrelated orphan's, is reset or
// requeued. A later pass does not save it again. Turned on, the same steps
// release and reset.
// trace:TASK-3796 | ai:claude
#[test]
fn with_the_knob_off_nothing_is_released_or_reset_even_for_a_salvageable_lane() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let tree = dir.path().join("tree");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.email", "t@example.invalid"]);
    git(&tree, &["config", "user.name", "T"]);
    std::fs::write(tree.join("tracked.txt"), "one\n").unwrap();
    git(&tree, &["add", "."]);
    git(&tree, &["commit", "-q", "-m", "first"]);
    std::fs::write(tree.join("tracked.txt"), "one\ntwo\n").unwrap();
    std::fs::write(tree.join("new.txt"), "new\n").unwrap();

    let lease = crate::lease_path(&root, "lease-bug-7-fix");
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(&lease, "id = \"lease-bug-7-fix\"\n").unwrap();

    let lanes = vec![lane(&tree, "bug-7-fix", Some("BUG-7"))];
    let orphans = vec![orphan("BUG-7"), orphan("BUG-8")];
    let steps = plan(lanes, orphans, &[]);
    let unattended = Unattended::load(&root);
    assert_eq!(unattended, Unattended::ReportOnly);

    let mut keeper = Real {
        root: &root,
        noted: Vec::new(),
        salvaged: 0,
        released: 0,
        reset: Vec::new(),
    };
    for pass in 0..3 {
        let notes = keep(&steps, &mut keeper, false, unattended);
        assert_eq!(keeper.salvaged, 1, "pass {pass}: {notes:#?}");
        assert_eq!(keeper.released, 0, "pass {pass}: {notes:#?}");
        assert!(keeper.reset.is_empty(), "pass {pass}: {notes:#?}");
        assert!(lease.exists(), "pass {pass}: the lease file was removed");
        let joined = notes.join("\n");
        assert!(
            joined.contains("lease is kept") && joined.contains("housekeeping_auto_release"),
            "{joined}"
        );
        assert!(
            joined.contains("BUG-8: In Progress with no session")
                && joined.contains("not sent back to the queue"),
            "{joined}"
        );
    }
    // One breadcrumb, naming the patch, which holds the work.
    let [(spec, text)] = &keeper.noted[..] else {
        panic!("{:?}", keeper.noted)
    };
    assert_eq!(spec, "BUG-7");
    assert!(text.contains("Its lease was kept"), "{text}");
    let patches: Vec<_> = std::fs::read_dir(root.join(".aida").join("salvage"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let [patch] = &patches[..] else {
        panic!("{patches:?}")
    };
    assert!(text.contains(&patch.display().to_string()), "{text}");
    let body = std::fs::read_to_string(patch).unwrap();
    assert!(body.contains("+two") && body.contains("+new"), "{body}");
    // The worktree is untouched.
    assert_eq!(
        std::fs::read_to_string(tree.join("tracked.txt")).unwrap(),
        "one\ntwo\n"
    );

    // A dry run with the knob off says it would keep the lease.
    let mut fake = Fake::default();
    let notes = keep(&steps, &mut fake, true, unattended);
    assert!(fake.salvaged.is_empty() && fake.released.is_empty() && fake.reset.is_empty());
    assert!(
        notes.iter().all(|note| note.contains("(dry run)")),
        "{notes:?}"
    );
    assert!(notes[0].contains("keep its lease"), "{notes:?}");

    // Turned on, the same steps release the lease and reset both specs.
    let notes = keep(&steps, &mut keeper, false, ON);
    assert_eq!(keeper.released, 1, "{notes:#?}");
    assert!(!lease.exists(), "{notes:#?}");
    assert_eq!(keeper.reset, ["BUG-7", "BUG-8"], "{notes:#?}");
}
