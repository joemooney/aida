use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn spec_status(store_dir: &std::path::Path, id: &str) -> aida_core::RequirementStatus {
    crate::Storage::new(store_dir)
        .load()
        .unwrap()
        .get_requirement_unambiguous(id)
        .unwrap()
        .unwrap()
        .status
        .clone()
}

/// BUG-2203: shipping a PR whose title ends in `(Refs BUG-2169)` from a branch
/// named for BUG-2169 must squash with the title unchanged — no bare
/// `(BUG-2169)` from the branch-name fallback — and the merge must leave
/// BUG-2169's status alone. The historical polluted subject
/// `… (Refs BUG-2169) (BUG-2169)` (main b66e4d93b8) must not complete it either.
// trace:BUG-2203 | ai:claude
#[test]
fn bug_2203_refs_subject_ship_appends_no_trailer_and_completes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "a"]);
    git(root, &["commit", "-q", "-m", "chore: initial"]);

    // BUG-2169 is Done, so a completion trailer WOULD complete it; BUG-999 is
    // the control proving the scan ran.
    let mut referenced = aida_core::Requirement::new("t".to_string(), String::new());
    referenced.spec_id = Some("BUG-2169".to_string());
    referenced.req_type = aida_core::RequirementType::Bug;
    referenced.status = aida_core::RequirementStatus::Done;
    let mut control = aida_core::Requirement::new("c".to_string(), String::new());
    control.spec_id = Some("BUG-999".to_string());
    control.req_type = aida_core::RequirementType::Bug;
    control.status = aida_core::RequirementStatus::Done;
    let store_dir = root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_dir).unwrap();
    aida_core::DatabaseBackend::save(
        &backend,
        &aida_core::RequirementsStore {
            requirements: vec![referenced, control],
            ..Default::default()
        },
    )
    .unwrap();

    let title = "[AI:claude] feat(contained): sandbox boundary, slice 2 (Refs BUG-2169)";
    git(root, &["checkout", "-q", "-b", "claude/bug-2169"]);
    std::fs::write(root.join("a"), "2").unwrap();
    git(root, &["commit", "-q", "-am", title]);

    let pr = PrShipMetadata {
        title: title.to_string(),
        body: String::new(),
    };
    let (subject, trailer) =
        derive_pr_ship_squash_subject_from(root, &pr, "claude/bug-2169", false).unwrap();
    assert!(trailer.ids.is_empty(), "{trailer:?}");
    assert_eq!(trailer.source, pr_ship::TrailerSource::None);
    // The exact squash subject: the title, unchanged.
    assert_eq!(subject.as_deref(), Some(title));
    // AC4: what ship prints before the merge says nothing is completed.
    let printed = pr_ship::format_trailer_provenance(&trailer);
    assert!(
        printed.starts_with("completion trailer: none — this merge completes no spec"),
        "{printed}"
    );
    assert!(printed.contains("(Refs BUG-2169)"), "{printed}");

    // Merge it as ship would (squash subject + GitHub's `(#N)`), plus the
    // historical polluted subject and a control completion.
    git(root, &["checkout", "-q", "main"]);
    for (content, msg) in [
        ("3", format!("{title} (#2787)")),
        (
            "4",
            "[AI:claude] feat(contained): slice 1 (Refs BUG-2169) (BUG-2169) (#2780)".to_string(),
        ),
        ("5", "fix: unrelated (BUG-999) (#2791)".to_string()),
    ] {
        std::fs::write(root.join("a"), content).unwrap();
        git(root, &["commit", "-q", "-am", &msg]);
    }

    let storage = crate::Storage::new(&store_dir);
    let flips = crate::db_reconcile::auto_bump_done_to_completed(root, &store_dir, None, &storage)
        .expect("auto_bump runs");
    assert!(
        flips.iter().any(|f| f.spec_id == "BUG-999"),
        "control spec must still complete: {flips:?}"
    );
    assert!(
        !flips.iter().any(|f| f.spec_id == "BUG-2169"),
        "a Refs id must never complete: {flips:?}"
    );
    assert_eq!(
        spec_status(&store_dir, "BUG-2169"),
        aida_core::RequirementStatus::Done
    );
}
