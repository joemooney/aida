//! BUG-2208: a Refs-only slice PR of a multi-PR spec (commit subjects end in
//! `(Refs SPEC-ID)`, never `(SPEC-ID)`) must still get a reviewer: the
//! auto-queue files a review story that covers the spec without completing
//! it, `queue work PR-N --role reviewer` resolves the spec ahead of a stray
//! headline mention or a superseded member, and a fallback that still cannot
//! resolve says why.
// trace:TASK-1955 | ai:claude (BUG-2208)

// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use super::*;
use aida_core::{Relationship, RelationshipType, Requirement, RequirementsStore};

fn spec(id: &str) -> Requirement {
    let mut req = Requirement::new(format!("{id} title"), String::new());
    req.spec_id = Some(id.to_string());
    req
}

fn rel(rel_type: RelationshipType, target: &Requirement) -> Relationship {
    Relationship {
        rel_type,
        target_id: target.id,
        created_at: None,
        created_by: None,
    }
}

/// The PR-2826 shape: every subject is `(Refs TASK-1955)` and one headline
/// mentions BUG-2130 in prose.
fn slice_messages() -> Vec<String> {
    vec![
        "[AI:claude] fix(review): slice 7 sweep items, part 1 (Refs TASK-1955)\n\nbody".into(),
        "[AI:claude] style(test): rustfmt the BUG-2130 identity tests (Refs TASK-1955)".into(),
        "[AI:claude] fix(test): ratchet (Refs TASK-1955)\n\n* trace line (TASK-2017)".into(),
    ]
}

#[test]
fn refs_only_commits_are_a_slice_of_their_spec_not_zero_coverage() {
    let cov = review_coverage_from_commits(&slice_messages());
    assert!(cov.delivered.is_empty(), "{cov:?}");
    assert_eq!(cov.slice_refs, vec!["TASK-1955".to_string()]);
    // The body trailer stays informational; the prose mention is not a spec.
    assert_eq!(cov.referenced, vec!["TASK-2017".to_string()]);
    // The BUG-776 gate counts the slice, so the story is filed.
    assert_eq!(
        auto_queue_skip_reason(
            true,
            Some(2826),
            Some(cov.delivered.len() + cov.slice_refs.len())
        ),
        None
    );
}

#[test]
fn a_delivered_trailer_wins_over_a_refs_trailer_for_the_same_spec() {
    let msgs = vec![
        "[AI:claude] fix: part 1 (Refs TASK-7)".to_string(),
        "[AI:claude] fix: last part (TASK-7)".to_string(),
        "[AI:claude] fix: side work (Refs TASK-8)".to_string(),
    ];
    let cov = review_coverage_from_commits(&msgs);
    assert_eq!(cov.delivered, vec!["TASK-7".to_string()]);
    assert_eq!(cov.slice_refs, vec!["TASK-8".to_string()]);
    assert!(cov.referenced.is_empty());
}

#[test]
fn a_spec_superseded_into_another_candidate_is_dropped() {
    let mut store = RequirementsStore::new();
    let mut sweep = spec("TASK-1955");
    let mut folded = spec("BUG-2130");
    let unrelated = spec("BUG-9");
    sweep
        .relationships
        .push(rel(RelationshipType::Supersedes, &folded));
    store.requirements.push(sweep.clone());
    store.requirements.push(folded.clone());
    store.requirements.push(unrelated);
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["BUG-2130".into(), "TASK-1955".into()]
        ),
        vec!["TASK-1955".to_string()]
    );
    // The reciprocal edge alone is enough too.
    sweep.relationships.clear();
    folded
        .relationships
        .push(rel(RelationshipType::SupersededBy, &sweep));
    store.requirements[0] = sweep;
    store.requirements[1] = folded;
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["TASK-1955".into(), "BUG-2130".into()]
        ),
        vec!["TASK-1955".to_string()]
    );
    // Unrelated specs are still both kept (real ambiguity).
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["TASK-1955".into(), "BUG-9".into()]
        )
        .len(),
        2
    );
}

/// A fake `gh` whose `pr view --json title,commits` answer is `tsv`.
fn fake_gh(dir: &std::path::Path, tsv: &str) -> std::path::PathBuf {
    let path = dir.join("gh");
    crate::test_exec::write_executable(&path, format!("#!/bin/sh\nprintf '%s\\n' '{tsv}'\n"));
    path
}

#[test]
fn a_refs_only_pr_resolves_to_its_spec_over_a_headline_mention() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(
        dir.path(),
        "[AI:claude] fix(review): slice 7 (Refs TASK-1955)\t\
         [AI:claude] style(test): rustfmt the BUG-2130 identity tests (Refs TASK-1955)",
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let store = RequirementsStore::new();
    assert_eq!(
        crate::pr_lookup::resolve_pr_to_spec(dir.path(), 2826, &store).unwrap(),
        "TASK-1955"
    );
}

#[test]
fn reviewer_fallback_that_cannot_resolve_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(
        dir.path(),
        "[AI:claude] fix: two things\t[AI:claude] fix: a (TASK-1)\t[AI:claude] fix: b (TASK-2)",
    );
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    crate::test_env::home_fixture_backend(&root)
        .unwrap()
        .save(&RequirementsStore::default())
        .unwrap();
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let err = crate::queue_cmd::resolve_queue_work_plan(
        &storage,
        "u",
        Some("PR-77"),
        None,
        false,
        true,
        false,
        Some("reviewer"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("no queued review story for PR-77"), "{err}");
    assert!(
        err.contains("reviewing without a story failed too")
            && err.contains("multiple backing specs: TASK-1, TASK-2"),
        "{err}"
    );
    // BUG-2217: the refusal names the fix.
    assert!(err.contains("aida pr auto-queue-review"), "{err}");
}

/// TASK-2255: the whole auto-queue path on a Refs-only range files a review
/// story linked to the spec with `covers` and never with `implements`, so
/// merging the slice cannot complete the spec. `gh` and `aida` are recording
/// fakes; the commit range is a real repository's `main..feat`.
// trace:TASK-1955 | ai:claude (TASK-2255)
#[test]
fn a_refs_only_range_files_a_story_that_covers_and_never_implements() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&["checkout", "-q", "-b", "feat"]);
    git(&[
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "[AI:claude] fix(review): slice 1 (Refs TASK-77)",
    ]);
    git(&[
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "[AI:claude] test: slice 1 tests (Refs TASK-77)",
    ]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/widget.git",
    ]);
    let store = repo.join(".aida-store");
    std::fs::create_dir_all(repo.join(".aida")).unwrap();
    std::fs::write(
        repo.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    crate::test_env::home_fixture_backend(&store)
        .unwrap()
        .save(&RequirementsStore::default())
        .unwrap();

    let gh = dir.path().join("gh");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\n\
         case \"$1 $2\" in\n\
         'pr list') printf '42\\tslice 1\\thttps://github.com/acme/widget/pull/42\\tfeat\\n' ;;\n\
         'pr view') printf '{\"state\":\"OPEN\",\"title\":\"slice 1\",\"baseRefName\":\"main\",\"headRefName\":\"feat\"}\\n' ;;\n\
         *) exit 1 ;;\n\
         esac\n",
    );
    let log = dir.path().join("aida.log");
    let aida = dir.path().join("aida");
    crate::test_exec::write_executable(
        &aida,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n\
             if [ \"$1\" = add ]; then echo 'Added: STORY-900 - review'; fi\n",
            log.display()
        ),
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let _aida = crate::kernel::test_aida_exe::set(aida);

    let outcome = try_auto_queue_pr_review_unbumped(
        &repo,
        "feat",
        "session-task-2255",
        AutoQueueOrigin::PrSkill,
    );
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        outcome.review_spec.as_deref(),
        Some("STORY-900"),
        "{}\n{calls}",
        outcome.summary
    );
    assert!(
        calls
            .lines()
            .any(|l| l.starts_with("add ") && l.contains("Review PR-42: slice 1")),
        "{calls}"
    );
    assert!(
        calls
            .lines()
            .any(|l| l == "rel add STORY-900 TASK-77 --type covers"),
        "{calls}"
    );
    assert!(
        !calls.contains("implements") && !calls.contains("implemented-by"),
        "a Refs-only slice must never implement its spec:\n{calls}"
    );
    assert!(
        calls
            .lines()
            .any(|l| l.starts_with("queue add STORY-900 --for reviewer")),
        "{calls}"
    );
}
