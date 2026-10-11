//! STORY-1649: the default-branch linkage scan behind the TOON `linkage:`
//! line and the `edit --status completed` warning. Pure git fixtures, no gh.
// trace:STORY-1649 | ai:claude
use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fresh repo on `main` with one initial commit and no remote.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "chore: init"]);
    (tmp, root)
}

fn commit(root: &std::path::Path, file: &str, body: &str, subject: &str) {
    std::fs::write(root.join(file), body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", subject]);
}

/// Add a bare `origin` and push `main` so `origin/main` exists.
fn add_bare_origin(root: &std::path::Path) -> tempfile::TempDir {
    let origin = tempfile::TempDir::new().unwrap();
    git(
        origin.path(),
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );
    git(
        root,
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );
    git(root, &["push", "-q", "-u", "origin", "main"]);
    origin
}

fn completed(spec_id: &str, req_type: aida_core::RequirementType) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(format!("work for {spec_id}"), String::new());
    r.spec_id = Some(spec_id.to_string());
    r.req_type = req_type;
    r.status = aida_core::RequirementStatus::Completed;
    r
}

fn task(spec_id: &str) -> aida_core::Requirement {
    completed(spec_id, aida_core::RequirementType::Task)
}

// ---- TOON `linkage:` line (AC1 / AC2 shape) -------------------------------

#[test]
fn toon_linkage_line_names_pr_and_commit() {
    let (_tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: ship the thing (TASK-801) (#53)");
    let short = git(&root, &["rev-parse", "--short", "HEAD"]);

    let scan = scan_default_branch_linkage(&root, &["TASK-801".to_string()]);
    assert_eq!(scan.found_on.as_deref(), Some("main"));
    assert_eq!(scan.shipped_pr, Some(53));
    assert_eq!(scan.commits.len(), 1);
    assert_eq!(scan.commits[0].0, short);
    assert!(scan.corroborated());

    let line = toon_show_linkage_line(&root, &["TASK-801".to_string()]);
    assert!(line.starts_with("linkage: "), "{line}");
    let commit_token = format!("commit={short}");
    for token in [
        "shipped=true",
        "shipped_pr=53",
        commit_token.as_str(),
        "commits=1",
        "files=0",
        "on=main",
    ] {
        assert!(line.contains(token), "missing `{token}` in {line}");
    }
    // Bare tokens only: the scalar stays unquoted so agents can split on
    // whitespace.
    assert!(!line.contains('"'), "{line}");
}

#[test]
fn toon_linkage_line_not_shipped_is_honest() {
    let (_tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: unrelated work (TASK-802)");

    let line = toon_show_linkage_line(&root, &["TASK-801".to_string()]);
    assert!(line.starts_with("linkage: "), "{line}");
    assert!(line.contains("shipped=false"), "{line}");
    assert!(line.contains("commits=0"), "{line}");
    assert!(line.contains("checked=main"), "{line}");
    assert!(!line.contains("shipped_pr"), "{line}");
    assert!(!line.contains("commit="), "{line}");
    assert!(!line.contains('"'), "{line}");
}

/// `-F --grep TASK-1` also matches `TASK-10`; the exact-token post-filter
/// (the doctor's regex) must reject that.
#[test]
fn toon_linkage_prefilter_does_not_match_longer_id() {
    let (_tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: other spec (TASK-10)");
    commit(
        &root,
        "b.rs",
        "// trace:TASK-10 | ai:claude\n",
        "feat: trace for other",
    );

    let scan = scan_default_branch_linkage(&root, &["TASK-1".to_string()]);
    assert!(!scan.corroborated(), "{scan:?}");
    assert_eq!(scan.trace_files, 0);
}

#[test]
fn toon_linkage_counts_trace_files_without_commit() {
    let (_tmp, root) = init_repo();
    commit(
        &root,
        "src.rs",
        "fn x() {}\n// trace:TASK-900 | ai:claude\n",
        "feat: no id in subject",
    );
    commit(
        &root,
        "other.rs",
        "// trace:TASK-900.A1 | ai:codex\n",
        "feat: criterion-suffixed trace",
    );

    let scan = scan_default_branch_linkage(&root, &["TASK-900".to_string()]);
    assert!(scan.corroborated(), "{scan:?}");
    assert_eq!(scan.commits.len(), 0);
    assert_eq!(scan.trace_files, 2, "{scan:?}");
    let line = render_toon_linkage(&scan);
    assert!(
        line.contains("shipped=true commits=0 files=2 on=main"),
        "{line}"
    );
}

#[test]
fn toon_linkage_without_default_branch_says_unknown() {
    let tmp = tempfile::TempDir::new().unwrap();
    let scan = scan_default_branch_linkage(tmp.path(), &["TASK-1".to_string()]);
    assert!(scan.scanned_refs.is_empty());
    let line = render_toon_linkage(&scan);
    assert!(line.contains("unknown"), "{line}");
}

#[test]
fn default_branch_refs_prefer_remote_then_local() {
    let (_tmp, root) = init_repo();
    assert_eq!(
        default_branch_refs(&root),
        DefaultBranchRefs {
            remote: None,
            local: Some("main".to_string()),
        }
    );
    let _origin = add_bare_origin(&root);
    let refs = default_branch_refs(&root);
    assert_eq!(refs.remote.as_deref(), Some("origin/main"));
    assert_eq!(refs.local.as_deref(), Some("main"));
    assert_eq!(refs.in_scan_order(), vec!["origin/main", "main"]);
}

// ---- completion warning (AC3 / AC4 / A1) ----------------------------------

#[test]
fn completed_without_shipped_commit_warns() {
    let (_tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: some other spec (TASK-2)");

    let warning = completion_linkage_warning(&root, &task("TASK-1"))
        .expect("code spec with nothing shipped warns");
    assert!(warning.contains("TASK-1"), "{warning}");
    assert!(warning.contains("main"), "{warning}");
    assert!(warning.contains("doctor:no-code"), "{warning}");
    assert!(warning.contains("git fetch"), "{warning}");
    assert!(warning.contains("completed-without-commit"), "{warning}");
}

#[test]
fn completed_with_shipped_commit_does_not_warn() {
    let (_tmp, root) = init_repo();
    commit(
        &root,
        "a.txt",
        "x",
        "[AI:claude] feat(x): ship it (TASK-1) (#9)",
    );
    assert_eq!(completion_linkage_warning(&root, &task("TASK-1")), None);
}

#[test]
fn completed_with_trace_comment_does_not_warn() {
    let (_tmp, root) = init_repo();
    commit(
        &root,
        "src.rs",
        "// trace:TASK-1 | ai:claude\n",
        "feat: subject names nothing",
    );
    assert_eq!(completion_linkage_warning(&root, &task("TASK-1")), None);
}

/// A1: `gh pr merge` lands on the remote first. A commit present on
/// `origin/main` but not yet on the stale local `main` counts as shipped.
#[test]
fn completed_with_commit_on_origin_only_does_not_warn() {
    let (_tmp, root) = init_repo();
    let _origin = add_bare_origin(&root);
    commit(
        &root,
        "a.txt",
        "x",
        "feat: merged on the forge (TASK-1) (#12)",
    );
    git(&root, &["push", "-q", "origin", "main"]);
    // Rewind the local branch: the merge now exists only on origin/main.
    git(&root, &["reset", "-q", "--hard", "HEAD~1"]);
    assert_eq!(
        git(&root, &["log", "main", "--oneline", "--grep=TASK-1"]),
        "",
        "fixture: local main must not carry the commit"
    );

    let scan = scan_default_branch_linkage(&root, &["TASK-1".to_string()]);
    assert_eq!(scan.found_on.as_deref(), Some("origin/main"), "{scan:?}");
    assert_eq!(scan.shipped_pr, Some(12));
    assert_eq!(completion_linkage_warning(&root, &task("TASK-1")), None);

    // And the TOON line says where it found it.
    let line = toon_show_linkage_line(&root, &["TASK-1".to_string()]);
    assert!(line.contains("on=origin/main"), "{line}");
}

/// A1: with a remote present and the spec on neither ref, the warning names
/// both refs and points at `git fetch` instead of asserting nothing shipped.
#[test]
fn completed_missing_on_both_refs_names_both_and_hints_fetch() {
    let (_tmp, root) = init_repo();
    let _origin = add_bare_origin(&root);
    let warning = completion_linkage_warning(&root, &task("TASK-1")).expect("warns");
    assert!(warning.contains("origin/main or main"), "{warning}");
    assert!(warning.contains("git fetch"), "{warning}");
}

#[test]
fn completed_doc_spec_does_not_warn() {
    let (_tmp, root) = init_repo();
    for req_type in [
        aida_core::RequirementType::Decision,
        aida_core::RequirementType::Doc,
        aida_core::RequirementType::Faq,
        aida_core::RequirementType::Term,
        aida_core::RequirementType::Principle,
    ] {
        let req = completed("ADR-1", req_type.clone());
        assert_eq!(
            completion_linkage_warning(&root, &req),
            None,
            "{req_type:?} is not code"
        );
    }
}

#[test]
fn completed_opted_out_tag_does_not_warn() {
    let (_tmp, root) = init_repo();
    let mut req = task("TASK-1");
    req.tags.insert(COMPLETED_NO_CODE_OPT_OUT_TAG.to_string());
    assert_eq!(completion_linkage_warning(&root, &req), None);
}

#[test]
fn completed_without_any_git_repo_does_not_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    assert_eq!(
        completion_linkage_warning(tmp.path(), &task("TASK-1")),
        None
    );
}

/// AC5: the completion warning and the doctor's `completed-without-commit`
/// scan apply one rule. Every kind of evidence the doctor accepts or
/// rejects — a real commit, a trace comment, a plan commit, a bare
/// `update SPEC` bookkeeping commit, nothing at all, a non-code type, the
/// opt-out tag — must produce the same verdict from both on one fixture.
#[test]
fn warning_and_doctor_scan_agree_on_same_fixture() {
    let (_tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: real work (TASK-100)");
    commit(
        &root,
        "b.rs",
        "// trace:TASK-200 | ai:claude\n",
        "feat: trace only",
    );
    commit(&root, "c.txt", "x", "docs(plans): plan the work (TASK-300)");
    commit(&root, "d.txt", "x", "update TASK-400");
    // TASK-500: nothing at all.

    let mut opted_out = task("TASK-600");
    opted_out.tags.insert("doctor:no-code".to_string());

    let specs = vec![
        task("TASK-100"),
        task("TASK-200"),
        task("TASK-300"),
        task("TASK-400"),
        task("TASK-500"),
        opted_out,
        completed("ADR-700", aida_core::RequirementType::Decision),
    ];
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = specs.clone();

    let doctor: Vec<String> = scan_completed_without_commit(&root, &store, None)
        .into_iter()
        .map(|f| f.id)
        .collect();
    let warned: Vec<String> = specs
        .iter()
        .filter(|r| completion_linkage_warning(&root, r).is_some())
        .map(|r| r.display_id())
        .collect();

    assert_eq!(
        warned, doctor,
        "warning and doctor must flag the same specs"
    );
    assert_eq!(
        warned,
        vec!["TASK-300", "TASK-400", "TASK-500"],
        "plan-only, bookkeeping-only and unreferenced specs are the violations"
    );
}

#[test]
fn spec_lookup_ids_include_agreed_and_origin_ids_once() {
    let mut req = task("TASK-1");
    req.agreed_id = Some("BUG-7".to_string());
    // display_id prefers the agreed id; the origin id still rides along.
    assert_eq!(spec_lookup_ids(&req), vec!["BUG-7", "TASK-1"]);
    let plain = task("TASK-2");
    assert_eq!(spec_lookup_ids(&plain), vec!["TASK-2"]);
}
