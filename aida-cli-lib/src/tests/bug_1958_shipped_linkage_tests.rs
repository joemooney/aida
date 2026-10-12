//! BUG-1958: STORY-1649 review follow-ups for the default-branch linkage
//! scan — skip a local ref the remote already covers, `--` after the ref, the
//! whitespace-free `detail=` token, the shallow-clone suffix, and a ':'-safe
//! trace-hit parser. Pure git fixtures, no gh.
// trace:BUG-1958 | ai:claude
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

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "chore: init"]);
    (tmp, root)
}

fn commit(root: &std::path::Path, file: &str, body: &str, subject: &str) {
    let path = root.join(file);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", subject]);
}

/// Bare `origin` inside the fixture tempdir, with `main` pushed.
fn add_bare_origin(tmp: &tempfile::TempDir, root: &std::path::Path) {
    let origin = tmp.path().join("origin.git");
    git(
        tmp.path(),
        &[
            "init",
            "--bare",
            "--quiet",
            "--initial-branch=main",
            "origin.git",
        ],
    );
    git(root, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(root, &["push", "-q", "-u", "origin", "main"]);
}

fn wanted(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

// ---- item 2: skip a local ref the remote already covers --------------------

#[test]
fn local_ref_equal_to_remote_is_not_scanned() {
    let (tmp, root) = init_repo();
    add_bare_origin(&tmp, &root);
    let refs = default_branch_refs(&root);
    assert_eq!(refs.in_scan_order(), vec!["origin/main", "main"]);
    assert_eq!(refs_worth_scanning(&root, &refs), vec!["origin/main"]);

    let scan = scan_default_branch_linkage(&root, &["TASK-1".to_string()]);
    assert_eq!(scan.scanned_refs, vec!["origin/main"], "{scan:?}");
}

#[test]
fn local_ref_behind_remote_is_not_scanned() {
    let (tmp, root) = init_repo();
    add_bare_origin(&tmp, &root);
    commit(&root, "a.txt", "x", "feat: merged remotely (TASK-2)");
    git(&root, &["push", "-q", "origin", "main"]);
    git(&root, &["reset", "-q", "--hard", "HEAD~1"]);

    let scan = scan_default_branch_linkage(&root, &["TASK-1".to_string()]);
    assert_eq!(scan.scanned_refs, vec!["origin/main"], "{scan:?}");
}

#[test]
fn local_ref_ahead_of_remote_is_still_scanned() {
    let (tmp, root) = init_repo();
    add_bare_origin(&tmp, &root);
    commit(&root, "a.txt", "x", "feat: local only (TASK-7)");

    let scan = scan_default_branch_linkage(&root, &["TASK-7".to_string()]);
    assert_eq!(scan.scanned_refs, vec!["origin/main", "main"], "{scan:?}");
    assert_eq!(scan.found_on.as_deref(), Some("main"), "{scan:?}");

    let unshipped = scan_default_branch_linkage(&root, &["TASK-1".to_string()]);
    assert_eq!(unshipped.scanned_refs, vec!["origin/main", "main"]);
    let warning = completion_linkage_warning(&root, &{
        let mut r = aida_core::Requirement::new("w".into(), String::new());
        r.spec_id = Some("TASK-1".into());
        r.req_type = aida_core::RequirementType::Task;
        r.status = aida_core::RequirementStatus::Completed;
        r
    })
    .expect("warns");
    assert!(warning.contains("origin/main or main"), "{warning}");
}

// ---- item 3: `--` after the ref ---------------------------------------------

/// A tracked file named `main` (and a directory `origin/` holding a file
/// `main`) makes the bare ref ambiguous to git; without `--` the scan's
/// swallowed error read as "nothing shipped".
#[test]
fn path_named_like_the_branch_does_not_break_the_scan() {
    let (tmp, root) = init_repo();
    commit(
        &root,
        "main",
        "// trace:TASK-5 | ai:claude\n",
        "feat: file named main",
    );
    commit(
        &root,
        "origin/main",
        "x\n",
        "feat: a path named origin/main",
    );
    commit(&root, "b.txt", "x", "feat: ship it (TASK-6) (#4)");
    add_bare_origin(&tmp, &root);
    commit(&root, "c.txt", "x", "feat: local ahead (TASK-8)");

    let ids = wanted(&["TASK-6"]);
    assert_eq!(referencing_commits_on_ref(&root, "main", &ids).len(), 1);
    assert_eq!(
        referencing_commits_on_ref(&root, "origin/main", &ids).len(),
        1
    );
    assert_eq!(trace_files_on_ref(&root, "main", &wanted(&["TASK-5"])), 1);

    let scan = scan_default_branch_linkage(&root, &["TASK-6".to_string()]);
    assert_eq!(scan.found_on.as_deref(), Some("origin/main"), "{scan:?}");
    assert_eq!(scan.shipped_pr, Some(4));
    let traced = scan_default_branch_linkage(&root, &["TASK-5".to_string()]);
    assert!(traced.corroborated(), "{traced:?}");
    assert_eq!(traced.trace_files, 1);
    let local = scan_default_branch_linkage(&root, &["TASK-8".to_string()]);
    assert_eq!(local.found_on.as_deref(), Some("main"), "{local:?}");
}

// ---- item 4: whitespace-free detail token -----------------------------------

#[test]
fn unshipped_linkage_line_splits_cleanly_on_whitespace() {
    let linkage = DefaultBranchLinkage {
        scanned_refs: vec!["origin/main".into(), "main".into()],
        ..Default::default()
    };
    let line = render_toon_linkage(&linkage);
    assert_eq!(
        line,
        "shipped=false commits=0 files=0 checked=origin/main+main detail=--format=json"
    );
    assert!(
        line.split_whitespace().all(|t| t.contains('=')),
        "every token is key=value: {line}"
    );
}

// ---- item 6: shallow clone suffix --------------------------------------------

#[test]
fn completion_warning_marks_shallow_clone() {
    let refs = vec!["origin/main".to_string()];
    let deep = format_completion_linkage_warning("TASK-1", &refs, false);
    assert!(!deep.contains("shallow clone"), "{deep}");
    assert!(deep.contains("no commit referencing it was found on origin/main"));
    let shallow = format_completion_linkage_warning("TASK-1", &refs, true);
    assert!(shallow.starts_with(&deep), "suffix is appended: {shallow}");
    assert!(shallow.contains("(shallow clone"), "{shallow}");
}

#[test]
fn shallow_clone_probe_reads_git() {
    let (tmp, root) = init_repo();
    commit(&root, "a.txt", "x", "feat: second");
    assert!(!is_shallow_repository(&root));
    let clone = tmp.path().join("shallow");
    let url = format!("file://{}", root.display());
    git(
        tmp.path(),
        &["clone", "-q", "--depth", "1", &url, clone.to_str().unwrap()],
    );
    assert!(is_shallow_repository(&clone));
}

// ---- item 7: ':'-safe trace-hit parser ---------------------------------------

#[test]
fn trace_hit_parser_handles_colon_in_path() {
    let ids = wanted(&["TASK-9"]);
    let hits = "main:dir/a:b.rs\0// trace:TASK-9 | ai:claude\n\
                main:c.rs\0// trace:TASK-9 | ai:claude\n\
                main:dir/a:b.rs\0// trace:TASK-9 again\n\
                main:d:e.rs\0// trace:TASK-90 | ai:claude";
    assert_eq!(count_trace_hit_files(hits, "main", &ids), 2);
}

#[test]
fn trace_files_on_ref_counts_colon_path() {
    let (_tmp, root) = init_repo();
    commit(
        &root,
        "a:b.rs",
        "// trace:TASK-9 | ai:claude\n",
        "feat: colon path",
    );
    commit(
        &root,
        "c.rs",
        "// trace:TASK-9 | ai:claude\n",
        "feat: plain path",
    );
    assert_eq!(trace_files_on_ref(&root, "main", &wanted(&["TASK-9"])), 2);
}
