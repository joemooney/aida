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
    fn release(&mut self, lane: &DeadLane) -> Result<()> {
        self.released.push(lane.lease.clone());
        Ok(())
    }
    fn reset(&mut self, spec: &str) -> Result<()> {
        self.reset.push(spec.into());
        Ok(())
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
    let notes = keep(&[Step::Salvage(lane.clone())], &mut fake, false);
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
        let notes = keep(&[Step::Salvage(lane.clone())], &mut fake, false);
        assert_eq!(fake.salvaged.len(), 1);
        assert!(fake.released.is_empty(), "{notes:?}");
        assert!(fake.noted.is_empty(), "{notes:?}");
        assert!(notes[0].contains("lease is kept"), "{notes:?}");
    }
}

#[test]
fn a_patch_that_cannot_be_recorded_is_reported_and_the_lease_still_goes() {
    let dir = tempfile::tempdir().unwrap();
    let patch = patch_in(dir.path());
    let mut fake = Fake {
        patch: Some(patch.clone()),
        note_fails: true,
        ..Fake::default()
    };
    let lane = lane(dir.path(), "bug-7-fix", Some("BUG-7"));
    let notes = keep(&[Step::Salvage(lane.clone())], &mut fake, false);
    assert_eq!(fake.released, [lane.lease]);
    assert!(notes[0].contains("not recorded on BUG-7"), "{notes:?}");
    assert!(notes[0].contains(&patch.display().to_string()), "{notes:?}");
}

#[test]
fn a_reset_records_the_branch_to_resume_first_and_resets_once() {
    let mut fake = Fake::default();
    let steps = [
        Step::Reset {
            spec: "BUG-5".into(),
            resume: Some("bug-5-unfinished".into()),
        },
        Step::Reset {
            spec: "BUG-6".into(),
            resume: None,
        },
    ];
    let notes = keep(&steps, &mut fake, false);
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
    let notes = keep(&steps[..1], &mut fake, false);
    assert!(fake.reset.is_empty(), "{notes:?}");
    assert!(notes[0].contains("not reset"), "{notes:?}");
}

#[test]
fn a_dry_run_and_the_reported_steps_call_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let steps = [
        Step::Salvage(lane(dir.path(), "bug-7-fix", Some("BUG-7"))),
        Step::Reset {
            spec: "BUG-5".into(),
            resume: Some("bug-5-unfinished".into()),
        },
    ];
    let mut fake = Fake::default();
    let notes = keep(&steps, &mut fake, true);
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
    let notes = keep(&reported, &mut fake, false);
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
