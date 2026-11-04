//! STORY-2324 PR-B: every gate-ledger writer stamps the canonical key its
//! spec label resolves to in the committed store (E2/E3), driven against one
//! fixture project whose store holds one requirement.
// trace:STORY-2324 | ai:claude
// mandate-gate

use super::*;
use crate::gate::canon::SpecKey;

/// A project at `<tmp>/primary` whose `.aida-store` commits one requirement
/// (`STORY-1`); returns the project root and the requirement's key.
fn project_with_one_requirement(tmp: &Path) -> (PathBuf, SpecKey) {
    let project = tmp.join("primary");
    let store = project.join(".aida-store");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida").join("config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    let mut req = aida_core::Requirement::new("fixture".into(), "fixture".into());
    req.spec_id = Some("STORY-1".into());
    let object = store.join("objects/STORY/000/STORY-1.yaml");
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, serde_yaml::to_string(&req).unwrap()).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "-q", "-m", "seed"],
    ] {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&store)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
    let key = SpecKey(req.id);
    assert_eq!(
        GateSpec::resolve(&project, "story-1").key,
        Some(key),
        "the fixture store resolves"
    );
    (project, key)
}

/// `hold::append` (the event and its hold record), the `aida pr hold`
/// signal, and `record_verdict_at` (unbound, then change-bound) each carry
/// the requirement's UUID.
#[test]
fn hold_signal_and_record_verdict_at_stamp_the_requirement_uuid() {
    let (_tmp, root) = root();
    let (project, key) = project_with_one_requirement(&root);
    let _env = uncontained();

    // hold::append: the event and the hold record it carries.
    let hold = crate::gate::merge_hold_core::typed_hold(
        41,
        crate::gate::merge_hold_core::HoldReasonKind::Rework,
        "rework",
        Some(HEAD.to_string()),
    )
    .with_spec("STORY-1");
    let event = hold::append(
        &project,
        41,
        EventBody::Hold {
            hold,
            rearmed: false,
        },
        Some(HEAD),
        Some("story-1"),
    )
    .unwrap();
    assert_eq!(event.spec_key, Some(key));
    let on_ledger = store::read_file(&project, ChangeKey::pr(41));
    let held = on_ledger
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Hold { hold, .. } => Some((e.spec_key, hold.spec_key)),
            _ => None,
        })
        .unwrap();
    assert_eq!(held, (Some(key), Some(key)), "event and hold record");

    // `aida pr hold`'s signal.
    let signal = crate::pr_cmd::hold_signal(&project, "STORY-1", "story-1", None);
    assert_eq!(signal.spec_key, Some(key));
    let json = serde_json::to_string(&signal).unwrap();
    assert!(json.contains(&key.to_string()), "{json}");

    // record_verdict_at, spec-keyed path: no change yet, so the unbound entry.
    let rec = record("approved", &ts(1), None);
    record_verdict_at(&rv::verdict_path(&project, "STORY-1"), &rec, None).unwrap();
    let unbound = store::read_unbound(&project, "STORY-1");
    assert_eq!(unbound.len(), 1);
    assert_eq!(unbound[0].spec_key, Some(key));

    // record_verdict_at, change-keyed path with the credited spec.
    let rec = record("request-changes", &ts(2), None);
    record_verdict_at(
        &rv::verdict_path(&project, "PR-42"),
        &rec,
        Some(&GateSpec::resolve(&project, "STORY-1")),
    )
    .unwrap();
    let bound: Vec<GateEvent> = store::read_file(&project, ChangeKey::pr(42))
        .into_iter()
        .filter(|e| e.verdict_record().is_some())
        .collect();
    assert_eq!(bound.len(), 1);
    assert_eq!(
        (bound[0].spec.as_deref(), bound[0].spec_key),
        (Some("STORY-1"), Some(key))
    );
}

/// `collect_standalone_verdict` resolves each credited label once and
/// `record_collected_verdict` writes the spec's record with its UUID; a
/// direct `record_collected_verdict` does the same.
#[test]
fn collected_verdicts_stamp_the_requirement_uuid() {
    let (_tmp, root) = root();
    let (project, key) = project_with_one_requirement(&root);
    let worktree = root.join("seat");
    let (_guard, a) = granted_reviewer(&project);
    let rec = record("approved", &ts(1), Some(&a));
    reviewer_wrote(&worktree, "STORY-1", &rec);
    let child = a.grant_id.clone().unwrap();

    rv::collect_standalone_verdict(
        &project,
        &worktree,
        &rv::verdict_path(&project, "PR-43"),
        &["story-1".to_string()],
        since(),
        &checks(&project, 43, &child),
    )
    .unwrap();
    let spec_events: Vec<GateEvent> = store::read_file(&project, ChangeKey::pr(43))
        .into_iter()
        .filter(|e| e.verdict_record().is_some() && e.spec.is_some())
        .collect();
    assert_eq!(spec_events.len(), 1, "{spec_events:?}");
    assert_eq!(spec_events[0].spec_key, Some(key));

    let n = record_collected_verdict(
        &project,
        ChangeKey::pr(44),
        &rec,
        &[(GateSpec::resolve(&project, "STORY-1"), rec.clone())],
    )
    .unwrap();
    assert_eq!(n, 2);
    let keys: Vec<Option<SpecKey>> = store::read_file(&project, ChangeKey::pr(44))
        .into_iter()
        .filter(|e| e.verdict_record().is_some() && e.spec.is_some())
        .map(|e| e.spec_key)
        .collect();
    assert_eq!(keys, vec![Some(key)]);
}
