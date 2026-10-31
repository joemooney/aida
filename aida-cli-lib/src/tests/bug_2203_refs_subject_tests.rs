// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
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
    let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
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
        number: None,
    };
    let (subject, trailer) =
        derive_pr_ship_squash_subject_from(root, &pr, "claude/bug-2169", false).unwrap();
    assert!(trailer.ids.is_empty(), "{trailer:?}");
    assert_eq!(trailer.source, pr_ship::TrailerSource::None);
    // The exact squash subject: the title, unchanged.
    assert_eq!(subject.as_deref(), Some(title));
    // ...and the `--subject` the ship's merge step passes (pr_cmd step 3 →
    // `merge_args`) is that title, with no bare `(BUG-2169)` appended.
    let (ship_subject, _) = derive_pr_ship_subject_for_method_from(
        root,
        &pr,
        "claude/bug-2169",
        false,
        crate::forge::MergeMethod::Squash,
        false,
    )
    .unwrap();
    assert_eq!(ship_subject.as_deref(), Some(title));
    let argv = pr_ship::merge_args(2787, false, ship_subject.as_deref(), None);
    let at = argv
        .iter()
        .position(|a| a == "--subject")
        .expect("--subject");
    assert_eq!(argv[at + 1], title);
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

/// BUG-2203 review: a squash body lists the branch's commits as bullets, and a
/// slice bullet `* … (Refs BUG-999)` must not suppress the subject's completing
/// `(BUG-999)` (real merges: fe70a874c6, c43d5444ad, b381e35218). Completion
/// intent comes from the subject line; this holds in the live auto-bump scan
/// and in the `reconcile-status` replay.
// trace:BUG-2203 | ai:claude
#[test]
fn bug_2203_refs_body_bullet_never_suppresses_a_subject_trailer() {
    for replay in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "t@t.t"]);
        git(root, &["config", "user.name", "t"]);
        std::fs::write(root.join("a"), "1").unwrap();
        git(root, &["add", "a"]);
        git(root, &["commit", "-q", "-m", "chore: initial"]);
        let mut done = aida_core::Requirement::new("t".to_string(), String::new());
        done.spec_id = Some("BUG-999".to_string());
        done.req_type = aida_core::RequirementType::Bug;
        done.status = aida_core::RequirementStatus::Done;
        let store_dir = root.join(".aida-store");
        let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
        aida_core::DatabaseBackend::save(
            &backend,
            &aida_core::RequirementsStore {
                requirements: vec![done],
                ..Default::default()
            },
        )
        .unwrap();
        std::fs::write(root.join("a"), "2").unwrap();
        git(
            root,
            &[
                "commit",
                "-q",
                "-am",
                "fix: final slice (BUG-999) (#2800)\n\n\
                 * fix: slice one (Refs BUG-999)",
            ],
        );

        let storage = crate::Storage::new(&store_dir);
        if replay {
            crate::db_reconcile::handle_db_reconcile_status(
                &store_dir,
                None,
                Some("BUG-999"),
                false,
            )
            .unwrap();
        } else {
            let flips =
                crate::db_reconcile::auto_bump_done_to_completed(root, &store_dir, None, &storage)
                    .expect("auto_bump runs");
            assert!(
                flips.iter().any(|f| f.spec_id == "BUG-999"),
                "a Refs body bullet must not suppress the subject trailer: {flips:?}"
            );
        }
        assert_eq!(
            spec_status(&store_dir, "BUG-999"),
            aida_core::RequirementStatus::Completed,
            "replay={replay}"
        );
    }
}

/// BUG-2203: the per-line Refs rule the completion scans share.
// trace:BUG-2203 | ai:claude
#[test]
fn bug_2203_refs_filter_is_scoped_to_the_carrying_line() {
    let f = |subject: &str, body: &str| {
        crate::db_reconcile::refs_completion_filter(subject, &format!("{subject}\n\n{body}"))
    };
    // A Refs body bullet suppresses nothing in the subject.
    let x = f("fix: x (BUG-1) (#9)", "* slice (Refs BUG-1)");
    assert!(!x.subject("BUG-1"));
    // The subject's Refs group suppresses its own bare repeat, and the body.
    let x = f("fix: x (Refs BUG-1) (BUG-1) (#9)", "* y (BUG-1)");
    assert!(x.subject("BUG-1") && x.body("BUG-1"));
    // A body line `(Refs X) (X)` does not complete X from the body...
    let x = f("fix: x (#9)", "* y (Refs BUG-2) (BUG-2)");
    assert!(x.body("BUG-2"));
    // ...unless another body line carries X as a plain trailer.
    let x = f("fix: x (#9)", "* y (Refs BUG-2) (BUG-2)\n* z (BUG-2)");
    assert!(!x.body("BUG-2"));
}
