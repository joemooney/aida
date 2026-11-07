//! Lossless canonical-store migration and registered CLI contract.
// trace:STORY-2402 | ai:codex
use super::*;

#[test]
fn story2402_s3_migration_heuristics_and_conservative_fallback() {
    let mut snapshot = RequirementsStore::default();
    for id in [
        "STORY-123",
        "STORY-2492",
        "STORY-1469",
        "TASK-42",
        "BUG-1-234",
        "EPIC-42",
        "SPIKE-42",
        "BUG-42",
        "STORY-1-234",
        "STORY-42",
    ] {
        let mut row = Requirement::new(id.into(), "".into());
        row.spec_id = Some(id.into());
        snapshot.requirements.push(row);
    }
    for (raw, expected) in [
        ("STORY-123 completes", "spec:STORY-123=completed"),
        ("STORY-2492 completed", "spec:STORY-2492=completed"),
        ("STORY-1469 approved", "spec:STORY-1469=approved"),
        ("TASK-42 shipped", "spec:TASK-42=completed"),
        ("BUG-1-234 landed", "spec:BUG-1-234=completed"),
        ("after EPIC-42 merged.", "spec:EPIC-42=completed"),
        ("SPIKE-42 rejected", "spec:SPIKE-42=rejected"),
        ("TASK-42 In Progress", "spec:TASK-42=in-progress"),
        ("BUG-42 needs-attention", "spec:BUG-42=needs-attention"),
        ("after story-1-234 ships", "spec:STORY-1-234=completed"),
        ("when STORY-42 lands.", "spec:STORY-42=completed"),
        ("#3491 merges", "pr:3491=merged"),
        ("after v0.16.2 ships", "release:>=0.16.2"),
        ("0.16.2", "release:>=0.16.2"),
        ("2026-12-01", "date:2026-12-01"),
        ("until 2026-12-01", "date:2026-12-01"),
    ] {
        assert_eq!(infer(raw, &snapshot).as_deref(), Some(expected), "{raw}");
        let typed = infer(raw, &snapshot).unwrap();
        assert_eq!(typed.parse::<Condition>().unwrap().to_string(), expected);
    }
    for raw in [
        "#0 merges",
        "follow-up done",
        "after re-review completes",
        "post-merge lands",
        "sign-off approved",
        "STORY-999999 completes",
        "spec:STORY-999999=completed",
        "2026-02-30",
        "date:bad",
        "spec:",
        "STORY-42 lands and testing finishes",
        "STORY-2492 completed and STORY-1469 approved",
        "TASK-42 approved after testing",
        "BUG-42 unknown",
        "unrelated words TASK-42 completed",
        "not before 0.16.2",
    ] {
        assert!(infer(raw, &snapshot).is_none(), "{raw}");
    }
    // Resolve both native/agreed IDs, and keep ambiguous targets manual.
    let mut first = Requirement::new("first".into(), "".into());
    first.spec_id = Some("STORY-100".into());
    first.agreed_id = Some("STORY-200".into());
    snapshot.requirements.push(first.clone());
    assert_eq!(
        infer("STORY-100 completes", &snapshot).as_deref(),
        Some("spec:STORY-100=completed")
    );
    assert_eq!(
        infer("STORY-200 completes", &snapshot).as_deref(),
        Some("spec:STORY-200=completed")
    );
    let mut second = Requirement::new("second".into(), "".into());
    second.spec_id = Some("STORY-101".into());
    second.agreed_id = first.agreed_id;
    snapshot.requirements.push(second);
    assert!(infer("STORY-200 completes", &snapshot).is_none());
    let mut row = Requirement::new("fixture".into(), "unchanged".into());
    row.deferred = true;
    row.deferred_until = Some("needs judgement".into());
    row.owner = "joe@example.com".into();
    assert_eq!(
        plan_spec(&row, &snapshot).unwrap().condition,
        "manual:joe@example.com"
    );
    for raw in ["follow-up done", "STORY-999999 completes"] {
        row.deferred_until = Some(raw.into());
        assert_eq!(
            plan_spec(&row, &snapshot).unwrap().condition,
            "manual:joe@example.com"
        );
    }
    for owner in ["", "  ", "Joe Mooney"] {
        row.owner = owner.into();
        assert_eq!(
            plan_spec(&row, &snapshot).unwrap().condition,
            "manual:operator"
        );
    }
    row.deferred_until = Some("date:bad".into());
    assert!(plan_spec(&row, &snapshot).unwrap().malformed_typed);
    row.deferred_until = None;
    assert_eq!(
        plan_spec(&row, &snapshot).unwrap().condition,
        "manual:operator"
    );
    row.tags.insert("deferred:2026-12-01".into());
    row.tags.insert("deferred:#42 merges".into());
    assert_eq!(
        plan_spec(&row, &snapshot).unwrap().condition,
        "manual:operator"
    );
    row.tags.clear();
    row.tags.insert("deferred:date:bad".into());
    assert!(plan_spec(&row, &snapshot).unwrap().malformed_typed);
    row.tags.clear();
    row.tags.insert("deferred:spec:STORY-42=completed".into());
    assert_eq!(
        plan_spec(&row, &snapshot).unwrap().condition,
        "spec:STORY-42=completed"
    );
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, CachedGitBackend) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    std::fs::create_dir_all(&root).unwrap();
    aida_core::git_ops::init(&root).unwrap();
    aida_core::git_ops::configure_user(&root, "fixture", "fixture@localhost").unwrap();
    crate::test_env::enroll_synthetic_home(&root).unwrap();
    let backend =
        CachedGitBackend::open(&root, &aida_core::db::project_cache_path(dir.path())).unwrap();
    (dir, root, backend)
}

fn objects(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn collect(root: &Path, paths: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, paths);
            } else {
                paths.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    collect(&root.join("objects"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect()
}

#[test]
fn story2402_s3_migration_round_trip_idempotent_nothing_lost() {
    let (_dir, root, backend) = fixture();
    let mut target = Requirement::new("target".into(), "".into());
    target.spec_id = Some("STORY-42".into());
    backend.add_requirement(target).unwrap();
    let cases = [
        Some("  STORY-42 ships\n"),
        Some("unknown\n\"quoted\" reason"),
        Some("date:bad"),
        None,
        Some("date:2026-12-01"),
        Some("manual:joe"),
        Some("STORY-999999 completes"),
        Some("follow-up done"),
    ];
    let mut original = Vec::new();
    for (index, raw) in cases.iter().enumerate() {
        let mut row = Requirement::new(format!("fixture {index}"), "description survives".into());
        row.spec_id = Some(format!("FR-{}", index + 1));
        row.deferred = index != 3;
        row.deferred_until = raw.map(str::to_string);
        row.owner = "joe".into();
        row.deferred_at = if matches!(index, 1 | 3) {
            None
        } else {
            Some(row.created_at)
        };
        row.deferred_reason = Some("operator decision must survive".into());
        row.tags.insert("operator-held".into());
        row.tags.insert("other-tag".into());
        row.add_comment(Comment::new("joe".into(), "existing comment".into()));
        if index == 3 {
            row.tags.insert("deferred:#3491 merges".into());
        }
        if index == 4 {
            row.tags.insert("deferred:old text".into());
        }
        original.push(backend.add_requirement(row).unwrap());
    }
    let before = objects(&root);
    let preview = run_migration(&backend, &root, false).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.specs.len(), 7);
    assert_eq!(preview.specs_written, 0);
    assert_eq!(
        objects(&root),
        before,
        "dry-run leaves exact canonical bytes alone"
    );
    let applied = run_migration(&backend, &root, true).unwrap();
    assert_eq!(applied.specs_written, 7);
    let expected = [
        "spec:STORY-42=completed",
        "manual:joe",
        "manual:joe",
        "pr:3491=merged",
        "date:2026-12-01",
        "manual:joe",
        "manual:joe",
        "manual:joe",
    ];
    for (index, old) in original.iter().enumerate() {
        let new = backend.get_requirement(&old.id).unwrap().unwrap();
        assert_eq!(new.deferred_until.as_deref(), Some(expected[index]));
        assert!(new.deferred);
        if index == 5 {
            assert_eq!(
                serde_yaml::to_value(&new).unwrap(),
                serde_yaml::to_value(old).unwrap()
            );
            continue;
        }
        assert_eq!(
            new.deferred_at,
            Some(old.deferred_at.unwrap_or(old.modified_at))
        );
        let audit = new.comments.last().unwrap();
        assert!(audit.content.starts_with("[aida:defer-migration]\n"));
        let source: serde_json::Value =
            serde_json::from_str(audit.content.lines().nth(1).unwrap()).unwrap();
        assert_eq!(
            source["deferred_until"],
            serde_json::json!(old.deferred_until)
        );
        let tags: Vec<String> = serde_json::from_value(source["legacy_tags"].clone()).unwrap();
        let mut restored = new.clone();
        restored.deferred_until = old.deferred_until.clone();
        restored.deferred = source["deferred"].as_bool().unwrap();
        restored.deferred_at = serde_json::from_value(source["deferred_at"].clone()).unwrap();
        restored.tags.extend(tags);
        restored.comments.pop();
        restored.modified_at = old.modified_at;
        assert_eq!(
            serde_yaml::to_value(restored).unwrap(),
            serde_yaml::to_value(old).unwrap(),
            "all unrelated fields survive for {index}"
        );
    }
    let snapshot = crate::condition_cmd::read_store(&root, Some(&root)).unwrap();
    let facts = crate::condition_cmd::CliConditionFacts::from_store(&root, snapshot);
    for old in &original {
        let new = backend.get_requirement(&old.id).unwrap().unwrap();
        let condition: Condition = new.deferred_until.unwrap().parse().unwrap();
        if matches!(condition, Condition::Spec { .. } | Condition::Manual { .. }) {
            assert!(
                condition.evaluate(&facts).is_ok(),
                "migrated target must evaluate safely"
            );
        }
    }
    let after = objects(&root);
    let second = run_migration(&backend, &root, true).unwrap();
    assert_eq!(second.specs_written, 0);
    assert!(second.specs.is_empty());
    assert_eq!(
        objects(&root),
        after,
        "re-apply changes no bytes or comments"
    );
}

#[test]
fn story2402_s3_migration_registered_cli_defaults_to_preview() {
    use clap::FromArgMatches;
    for flags in [vec![], vec!["--apply", "--json"]] {
        let mut argv = vec!["aida", "db", "migrate-defer-conditions"];
        argv.extend(&flags);
        let matches = crate::command_registry::built_command()
            .try_get_matches_from(argv)
            .unwrap();
        let args =
            crate::db_cmd::DbArgs::from_arg_matches(matches.subcommand_matches("db").unwrap())
                .unwrap();
        let crate::cli::DbCommand::MigrateDeferConditions(args) = args.inner else {
            panic!("wrong command");
        };
        assert_eq!(args.apply, !flags.is_empty());
        assert_eq!(args.json, !flags.is_empty());
    }
}

#[test]
fn story2402_s3_migration_corrupt_canonical_input_refuses_before_writes() {
    let (_dir, root, backend) = fixture();
    let mut row = Requirement::new("fixture".into(), "".into());
    row.deferred = true;
    backend.add_requirement(row).unwrap();
    let broken = aida_core::object_store::object_path(&root.join("objects"), "FR-9999").unwrap();
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(broken, "id: [invalid").unwrap();
    let before = objects(&root);
    assert!(run_migration(&backend, &root, true).is_err());
    assert_eq!(objects(&root), before);
}

#[test]
fn story2402_s3_migration_replans_live_row_preserving_new_trigger_and_hold() {
    let (_dir, root, backend) = fixture();
    let mut row = Requirement::new("fixture".into(), "".into());
    row.deferred = true;
    row.deferred_until = Some("STORY-42 ships".into());
    let observed = backend.add_requirement(row).unwrap();
    let mut edited = observed.clone();
    edited.deferred_until = Some("  DATE:2026-12-01  ".into());
    edited.deferred_reason = Some("new human hold".into());
    edited.tags.insert("operator-held".into());
    backend.update_requirement(&edited).unwrap();
    let before = objects(&root);
    assert!(
        migrate_spec(&backend, &observed, &RequirementsStore::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        objects(&root),
        before,
        "stale candidates cannot overwrite current data"
    );

    edited.tags.insert("deferred:unrecognised".into());
    edited.deferred_at = None;
    backend.update_requirement(&edited).unwrap();
    let live = backend.get_requirement(&edited.id).unwrap().unwrap();
    let plan = migrate_spec(&backend, &observed, &RequirementsStore::default())
        .unwrap()
        .unwrap();
    assert_eq!(plan.condition, "  DATE:2026-12-01  ");
    let written = backend.get_requirement(&edited.id).unwrap().unwrap();
    assert_eq!(written.deferred_reason, live.deferred_reason);
    assert!(written.tags.contains("operator-held"));
    assert_eq!(written.deferred_at, Some(live.modified_at));
}

// trace:STORY-2402 | ai:codex
#[test]
fn story2402_s3_migration_inactive_prose_untouched_and_manual_age_reported() {
    let (_dir, root, backend) = fixture();
    let now = chrono::Utc::now();
    let mut original = Vec::new();
    for (name, deferred, tag, age, archived) in [
        ("inactive", false, false, 30, false),
        ("old prose", true, false, 30, false),
        ("recent prose", true, false, 1, false),
        ("old tag", false, true, 30, false),
        ("archived", true, false, 30, true),
    ] {
        let mut row = Requirement::new(name.into(), "".into());
        row.spec_id = Some(format!("FR-{}", original.len() + 1));
        row.deferred = deferred;
        row.archived = archived;
        row.deferred_until = Some("needs human judgement".into());
        row.deferred_at = None;
        row.modified_at = now - chrono::Duration::days(age);
        if tag {
            row.tags.insert("deferred:ask joe".into());
        }
        original.push(backend.add_requirement(row).unwrap());
    }
    let preview = run_migration(&backend, &root, false).unwrap();
    assert_eq!(preview.specs.len(), 4);
    assert_eq!(
        preview
            .specs
            .iter()
            .filter(|plan| plan.manual_overdue)
            .count(),
        2
    );
    let report = render_outcome(&preview);
    assert!(
        report.contains("2 resulting manual condition(s) older than the default 14-day threshold")
    );
    assert!(report.contains(r#"trigger="needs human judgement" tags=[] -> manual:operator"#));
    assert!(!report.contains("Some("));
    run_migration(&backend, &root, true).unwrap();
    let inactive = backend.get_requirement(&original[0].id).unwrap().unwrap();
    assert_eq!(
        serde_yaml::to_value(inactive).unwrap(),
        serde_yaml::to_value(&original[0]).unwrap()
    );
    assert_eq!(
        run_migration(&backend, &root, true).unwrap().specs_written,
        0
    );
}
