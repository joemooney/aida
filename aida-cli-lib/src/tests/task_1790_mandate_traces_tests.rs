// trace:TASK-1790 | ai:codex
use super::*;

fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rust_comments_exclude_strings_and_include_doc_block_and_unicode() {
    let source = concat!(
        "\n// trace:TASK-1\n",
        "/// trace:TASK-2\n",
        "/** trace:TASK-3 */\n",
        "fn f() {\n",
        " let s = \"// trace:TASK-999\";\n",
        " let raw = r###\"/* trace:TASK-998 */\"###;\n",
        " let c = 'é'; /* trace:TASK-4 */\n",
        " // é trace:TASK-5.AC1\n",
        "}\n",
    );
    let (got, fallback) = source_ids("src/lib.rs", source, &[], Scope::FullFile);
    assert!(!fallback);
    assert_eq!(
        got,
        ids(&["TASK-1", "TASK-2", "TASK-3", "TASK-4", "TASK-5"])
    );
}

#[test]
fn changed_item_keeps_leading_and_inside_traces_without_sibling_function() {
    let source = "// trace:TASK-1\nfn one() {\n // trace:TASK-2\n let a = 1;\n}\n\n// trace:TASK-3\nfn two() {}\n";
    let (got, fallback) = source_ids("a.rs", source, &[3..4], Scope::ChangedItems);
    assert!(!fallback);
    assert_eq!(got, ids(&["TASK-1", "TASK-2"]));
}

#[test]
fn impl_scope_keeps_impl_and_enclosed_traces() {
    let source = "// trace:TASK-1\nimpl A {\n // trace:TASK-2\n fn a() { let x = 1; }\n // trace:TASK-3\n fn b() {}\n}\n";
    let (got, fallback) = source_ids("a.rs", source, &[3..4], Scope::ChangedItems);
    assert!(!fallback);
    assert_eq!(got, ids(&["TASK-1", "TASK-2", "TASK-3"]));
}

#[test]
fn unattributable_and_malformed_sources_use_full_file_fallback() {
    for (path, text, change) in [
        (
            "a.rs",
            "// trace:TASK-1\nuse x::y;\n// trace:TASK-2\nfn f() {}",
            1..2,
        ),
        ("a.rs", "// trace:TASK-1\nfn broken( {", 1..2),
        ("a.py", "# trace:TASK-1\nprint(1)", 1..2),
    ] {
        let (got, fallback) = source_ids(path, text, &[change], Scope::ChangedItems);
        assert!(fallback, "{path}: {text}");
        assert!(got.contains("TASK-1"));
    }
}

#[test]
fn trace_token_forms_and_criteria_resolve_to_specs() {
    assert_eq!(
        trace_ids("trace:TASK-1.AC1,BUG-2 trace:relates:STORY-3-4 | ai:codex"),
        ids(&["TASK-1", "BUG-2", "STORY-3-4"])
    );
    assert!(trace_ids("trace:word trace:TASK-X").is_empty());
    assert_eq!(
        trace_ids("trace:FR-1-042.AC3 trace:TASK-4*/ trace:BUG-2]"),
        ids(&["FR-1-042", "TASK-4", "BUG-2"])
    );
}

struct Repo(tempfile::TempDir);
impl Repo {
    fn new() -> Self {
        let r = Self(tempfile::tempdir().unwrap());
        r.run(&["init", "-q"]);
        r.run(&["config", "user.name", "Test"]);
        r.run(&["config", "user.email", "test@example.invalid"]);
        r.run(&["config", "core.hooksPath", "/dev/null"]);
        r
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn run(&self, args: &[&str]) -> String {
        String::from_utf8(git(self.path(), args).unwrap())
            .unwrap()
            .trim()
            .to_owned()
    }
    fn put(&self, path: &str, body: &str) {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    fn commit(&self) -> String {
        self.run(&["add", "-A"]);
        self.run(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
            "--allow-empty",
        ]);
        self.run(&["rev-parse", "HEAD"])
    }
}

#[test]
fn base_head_union_keeps_removed_traces_deleted_files_and_added_sources() {
    let r = Repo::new();
    r.put("a.rs", "// trace:TASK-1\nfn a() {}\n");
    r.put("deleted.rs", "// trace:TASK-2\nfn d() {}\n");
    let base = r.commit();
    r.put("a.rs", "// trace:TASK-3\nfn a() {}\n");
    std::fs::remove_file(r.path().join("deleted.rs")).unwrap();
    r.put("new.rs", "// trace:TASK-4\nfn n() {}\n");
    let head = r.commit();
    for scope in [Scope::FullFile, Scope::ChangedItems] {
        let evidence = collect(r.path(), &base, &head, scope).unwrap();
        assert_eq!(evidence.ids, ids(&["TASK-1", "TASK-2", "TASK-3", "TASK-4"]));
    }
    // Dirty checkout content supplies no evidence and cannot erase evidence.
    r.put("a.rs", "// trace:TASK-999\n");
    assert_eq!(
        collect(r.path(), &base, &head, Scope::FullFile)
            .unwrap()
            .ids,
        ids(&["TASK-1", "TASK-2", "TASK-3", "TASK-4"])
    );
}

#[test]
fn rename_across_source_boundary_reads_original_source() {
    let r = Repo::new();
    r.put("old.rs", "// trace:TASK-1\nfn a() {}\n");
    let base = r.commit();
    std::fs::rename(r.path().join("old.rs"), r.path().join("renamed.txt")).unwrap();
    let head = r.commit();
    assert_eq!(
        collect(r.path(), &base, &head, Scope::FullFile)
            .unwrap()
            .ids,
        ids(&["TASK-1"])
    );
}

#[test]
fn unchanged_sibling_is_excluded_by_item_scope_but_present_in_full_file() {
    let r = Repo::new();
    let body = "// trace:TASK-1\nfn one() { let x = 1; }\n\n// trace:TASK-2\nfn two() {}\n";
    r.put("a.rs", body);
    let base = r.commit();
    r.put("a.rs", &body.replace("x = 1", "x = 2"));
    let head = r.commit();
    assert_eq!(
        collect(r.path(), &base, &head, Scope::FullFile)
            .unwrap()
            .ids,
        ids(&["TASK-1", "TASK-2"])
    );
    let scoped = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    assert_eq!(scoped.ids, ids(&["TASK-1"]));
    assert!(scoped.full_file_fallbacks.is_empty());
}

#[test]
fn missing_object_unpinned_revision_and_noncommit_refuse() {
    let r = Repo::new();
    let base = r.commit();
    for head in ["HEAD", "--help", "0000000000000000000000000000000000000000"] {
        assert!(collect(r.path(), &base, head, Scope::FullFile).is_err());
    }
    r.put("a.rs", "fn a() {}\n");
    let head = r.commit();
    let tree = r.run(&["rev-parse", "HEAD^{tree}"]);
    assert!(collect(r.path(), &base, &tree, Scope::FullFile).is_err());
    let blob = r.run(&["rev-parse", "HEAD:a.rs"]);
    // Delete only a fixture object's loose blob to prove required reads refuse.
    std::fs::remove_file(
        r.path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    assert!(collect(r.path(), &base, &head, Scope::FullFile).is_err());
}

#[test]
fn source_symlinks_and_invalid_text_refuse_but_docs_are_not_source() {
    let r = Repo::new();
    r.put("README.md", "trace:TASK-999");
    let base = r.commit();
    r.put("notes.md", "trace:TASK-998");
    r.put("target/fake.rs", "// trace:TASK-997");
    let docs = r.commit();
    assert!(collect(r.path(), &base, &docs, Scope::FullFile)
        .unwrap()
        .ids
        .is_empty());
    std::os::unix::fs::symlink("notes.md", r.path().join("a.rs")).unwrap();
    let link = r.commit();
    assert!(collect(r.path(), &docs, &link, Scope::FullFile).is_err());
    std::fs::remove_file(r.path().join("a.rs")).unwrap();
    std::fs::write(r.path().join("a.rs"), [0xff]).unwrap();
    let invalid = r.commit();
    assert!(collect(r.path(), &docs, &invalid, Scope::FullFile).is_err());
}

#[test]
fn pr_evidence_requires_matching_head_and_unique_available_base() {
    let r = Repo::new();
    r.put("a.rs", "// trace:TASK-1\nfn a() { let x = 1; }\n");
    let base = r.commit();
    r.run(&["update-ref", "refs/remotes/origin/main", &base]);
    r.put("a.rs", "// trace:TASK-1\nfn a() { let x = 2; }\n");
    let head = r.commit();
    assert_eq!(
        collect_pr(r.path(), "main", &head, &head).unwrap().ids,
        ids(&["TASK-1"])
    );
    assert!(collect_pr(r.path(), "main", &base, &head).is_err());
    assert!(collect_pr(r.path(), "missing", &head, &head).is_err());
    assert!(collect_pr(r.path(), "bad ref", &head, &head).is_err());
}

#[test]
fn store_evidence_detects_dirty_working_objects_and_ref_changes() {
    let r = Repo::new();
    r.put("objects/a.yaml", "title: first\n");
    let base = r.commit();
    let snap = store_snapshot(r.path()).unwrap();
    assert_eq!(snap, store_snapshot(r.path()).unwrap());
    r.put("objects/a.yaml", "title: changed\n");
    assert!(store_snapshot(r.path()).is_err());
    let head = r.commit();
    assert_ne!(snap, store_snapshot(r.path()).unwrap());
    r.run(&["update-ref", "refs/heads/sticky-history", &base]);
    let refs = store_snapshot(r.path()).unwrap();
    r.run(&["update-ref", "refs/heads/sticky-history", &head]);
    assert_ne!(refs, store_snapshot(r.path()).unwrap());
}

#[test]
fn mode_only_change_uses_full_file_fallback() {
    let r = Repo::new();
    r.put("a.rs", "// trace:TASK-1\nfn a() {}\n");
    let base = r.commit();
    crate::test_exec::write_executable(&r.path().join("a.rs"), "// trace:TASK-1\nfn a() {}\n");
    let head = r.commit();
    let got = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    assert_eq!(got.ids, ids(&["TASK-1"]));
    assert_eq!(got.full_file_fallbacks, ids(&["a.rs"]));
}

#[test]
fn snapshot_does_not_execute_repository_fsmonitor() {
    let r = Repo::new();
    r.put("objects/a.yaml", "title: fixture\n");
    let watcher = r.path().join("watcher.sh");
    crate::test_exec::write_executable(&watcher, "#!/bin/sh\nprintf called > fsmonitor-ran\n");
    r.commit();
    r.run(&["config", "core.fsmonitor", watcher.to_str().unwrap()]);
    // Positive control: this same repository setting executes without the
    // host builder. The controlled hook writes only in this fixture repository.
    let raw = std::process::Command::new("git")
        .current_dir(r.path())
        .args(["status", "--porcelain=v1"])
        .output()
        .unwrap();
    assert!(raw.status.success());
    let marker = r.path().join("fsmonitor-ran");
    assert!(
        marker.exists(),
        "positive control did not execute the fixture"
    );
    std::fs::remove_file(&marker).unwrap();
    assert!(store_snapshot(r.path()).is_ok());
    assert!(
        !marker.exists(),
        "mandate snapshot executed repository code"
    );
}

#[test]
fn unsupported_source_gitlink_cannot_be_hidden_by_host_diff_defaults() {
    let r = Repo::new();
    let base = r.commit();
    r.run(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{base},module.rs"),
    ]);
    r.run(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "gitlink fixture",
    ]);
    let head = r.run(&["rev-parse", "HEAD"]);
    let err = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap_err();
    assert!(
        err.to_string().contains("not an ordinary regular file"),
        "{err:#}"
    );
}
