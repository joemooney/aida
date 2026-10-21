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
            "// trace:TASK-1\n#![allow(unused)]\n// trace:TASK-2\nfn f() {}",
            1..2,
        ),
        (
            "a.rs",
            "// trace:TASK-1\nuse x::*;\n// trace:TASK-2\nfn f() {}",
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
fn source_symlinks_and_invalid_text_refuse_and_all_text_paths_supply_evidence() {
    let r = Repo::new();
    r.put("README.md", "trace:TASK-999");
    let base = r.commit();
    r.put("notes.md", "trace:TASK-998");
    r.put("target/fake.rs", "// trace:TASK-997");
    let docs = r.commit();
    assert_eq!(
        collect(r.path(), &base, &docs, Scope::FullFile)
            .unwrap()
            .ids,
        ids(&["TASK-998", "TASK-997"])
    );
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

// trace:TASK-1790 | ai:codex
#[test]
fn bare_cr_diff_lines_cannot_hide_guarded_trace() {
    let r = Repo::new();
    let source = concat!("// pad\r\r\r\r\r\r\r\r\n",
        "// trace:SUP-1\nfn guarded() { let x = 1; }\n\n",
        "// trace:OK-2\nfn benign() {\n let a = 0;\n let b = 0;\n let c = 0;\n let d = 0;\n let e = 0;\n}\n");
    r.put("a.rs", source);
    let base = r.commit();
    r.put("a.rs", &source.replace("x = 1", "x = 2"));
    let head = r.commit();
    let evidence = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    assert!(evidence.ids.contains("SUP-1"), "{evidence:?}");
}

// trace:TASK-1790 | ai:codex
#[test]
fn every_changed_text_path_and_case_insensitive_marker_is_scanned() {
    let r = Repo::new();
    let paths = [
        "README.md",
        "Cargo.toml",
        "ci.yml",
        "config.yaml",
        "plugin.lua",
        "api.proto",
        "Makefile",
        ".hooks/pre-commit",
        "src/build/mod.rs",
        "src/dist/mod.rs",
        "src/target/mod.rs",
        "src/.cache/mod.rs",
        "src/venv/mod.rs",
    ];
    for (i, path) in paths.iter().enumerate() {
        r.put(path, &format!("// TrAcE:SUP-{}\n", i + 1));
    }
    let base = r.commit();
    for (i, path) in paths.iter().enumerate() {
        r.put(path, &format!("// TRACE:NEW-{}\n", i + 1));
    }
    let head = r.commit();
    let evidence = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    for i in 1..=paths.len() {
        assert!(evidence.ids.contains(&format!("SUP-{i}")), "{evidence:?}");
        assert!(evidence.ids.contains(&format!("NEW-{i}")), "{evidence:?}");
    }
}

// trace:TASK-1800 | ai:codex
#[test]
fn hunk_scope_always_adds_header_traces_before_any_first_item() {
    for first in [
        "use std::fmt;",
        "struct First;",
        "fn first() {}",
        "mod sibling {}",
    ] {
        let source = format!(
            "//! trace:SUP-1\n/* trace:SUP-2 */\n// trace:SUP-3\n{first}\n\nfn changed() {{\n let value = 1;\n}}\n\n// trace:OTHER-9\nfn unrelated() {{}}\n"
        );
        let (got, fallback) = source_ids("a.rs", &source, &[6..7], Scope::ChangedItems);
        assert!(!fallback, "{first}");
        assert_eq!(got, ids(&["SUP-1", "SUP-2", "SUP-3"]), "{first}");
    }
}

// trace:TASK-1800 | ai:codex
#[test]
fn hunk_scope_unions_both_committed_headers_and_retains_item_traces() {
    let r = Repo::new();
    let source = "//! trace:SUP-1\nuse std::fmt;\n\n// trace:LOCAL-2\nfn changed() {\n let value = 1;\n}\n\n// trace:OTHER-9\nfn unrelated() {}\n";
    r.put("a.rs", source);
    let base = r.commit();
    r.put(
        "a.rs",
        &source
            .replace("SUP-1", "SUP-3")
            .replace("value = 1", "value = 2"),
    );
    let head = r.commit();
    let got = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    assert!(got.full_file_fallbacks.contains("a.rs")); // edited header remains fail closed
    assert!(ids(&["SUP-1", "SUP-3", "LOCAL-2"]).is_subset(&got.ids));
    // An unchanged header also applies to a body-only hunk, without fallback.
    let base = head;
    r.put(
        "a.rs",
        &source
            .replace("SUP-1", "SUP-3")
            .replace("value = 1", "value = 4"),
    );
    let head = r.commit();
    let got = collect(r.path(), &base, &head, Scope::ChangedItems).unwrap();
    assert!(got.full_file_fallbacks.is_empty());
    assert_eq!(got.ids, ids(&["SUP-3", "LOCAL-2"]));
}

// trace:TASK-1800 | ai:codex
#[test]
fn header_traces_include_outer_docs_attributes_and_same_line_prefixes() {
    for header in [
        "/* trace:SUP-1 */ use std::fmt;\n",
        "/// trace:SUP-1\n/// continued\nuse std::fmt;\n",
        "#![allow(unused)]\n// trace:SUP-1\nuse std::fmt;\n",
    ] {
        let source = format!("{header}\nfn changed() {{\n let value = 1;\n}}\n");
        let line = source
            .lines()
            .position(|line| line.contains("let value"))
            .unwrap();
        let (got, fallback) = source_ids("a.rs", &source, &[line..line + 1], Scope::ChangedItems);
        assert!(!fallback, "{header}");
        assert_eq!(got, ids(&["SUP-1"]), "{header}");
    }
}

// trace:TASK-1800 | ai:codex
#[test]
fn hunk_scope_adds_unowned_module_comments_without_sibling_function_traces() {
    for module_comments in [
        "// trace:SUP-1\nuse std::io;",
        "// trace:SUP-1\nconst X: u8 = 1;",
        "/// trace:SUP-1\nstruct Other;",
        "/* trace:SUP-1 */ static X: u8 = 1;",
        "macro_rules! noop { () => {} }\n// trace:SUP-1",
        "mod untouched { //! trace:SUP-1\n fn other() {} }",
    ] {
        let source = format!("use std::fmt;\n{module_comments}\nfn changed() {{\n let x = 1;\n}}\n// trace:OTHER-9\nfn sibling() {{}}\n// trace:TAIL-2\n");
        let line = source.lines().position(|s| s.contains("let x")).unwrap();
        let (got, fallback) = source_ids("a.rs", &source, &[line..line + 1], Scope::ChangedItems);
        assert!(!fallback, "{module_comments}");
        assert_eq!(got, ids(&["SUP-1", "TAIL-2"]), "{module_comments}");
    }
}

// trace:TASK-1800 | ai:codex
#[test]
fn inline_module_headers_apply_even_when_attached_to_unchanged_first_function() {
    for header in [
        "//! trace:SUP-1\n",
        "/* trace:SUP-1 */ ",
        "/// trace:SUP-1\n",
        "#![allow(unused)]\n// trace:SUP-1\n#[inline]\n",
    ] {
        let source = format!("use std::fmt;\nmod outer {{\n mod inner {{\n {header}fn first() {{}}\n // trace:LOCAL-2\n fn changed() {{\n let x = 1;\n }}\n // trace:OTHER-9\n fn sibling() {{}}\n }}\n}}\n");
        let line = source.lines().position(|s| s.contains("let x")).unwrap();
        let (got, fallback) = source_ids("a.rs", &source, &[line..line + 1], Scope::ChangedItems);
        assert!(!fallback, "{header}");
        assert_eq!(got, ids(&["SUP-1", "LOCAL-2"]), "{header}");
    }
}

/// TASK-1800 AC2 shape fixture: the IDs and fallback of the item-level
/// collector for a change on the lines containing `marker`, checked against
/// the fn/impl collector and the full file. The new set is always a subset of
/// the full file, and a superset of the old set whenever the old one did not
/// fall back.
// trace:TASK-1800 | ai:claude
fn shape(source: &str, marker: &str) -> (BTreeSet<String>, Option<Fallback>) {
    let changes: Vec<_> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(marker))
        .map(|(i, _)| i..i + 1)
        .collect();
    assert!(!changes.is_empty(), "no line contains {marker:?}");
    let (full, _) = source_evidence("a.rs", source, &changes, Scope::FullFile);
    let (old, old_fallback) = source_evidence("a.rs", source, &changes, Scope::ChangedItemsV1);
    let (new, fallback) = source_evidence("a.rs", source, &changes, Scope::ChangedItems);
    assert!(new.is_subset(&full), "{new:?} not within {full:?}");
    if old_fallback.is_none() {
        assert!(old.is_subset(&new), "{old:?} lost from {new:?}");
    }
    if fallback.is_some() {
        assert_eq!(new, full);
    }
    (new, fallback)
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_struct_field_change_selects_the_traced_fns_that_use_it() {
    let source = "use std::fmt;\n// trace:S-1\nstruct Config {\n    port: u16,\n}\n\n// trace:USE-2\nfn reads(c: &Config) -> u16 {\n    c.port\n}\n\n// trace:CALL-3\nfn caller() -> u16 {\n    reads(&Config { port: 1 })\n}\n\n// trace:OTHER-9\nfn unrelated() {}\n";
    assert_eq!(
        source_evidence("a.rs", source, &[3..4], Scope::ChangedItemsV1).1,
        Some(Fallback::Uncovered)
    );
    assert_eq!(
        shape(source, "port: u16"),
        (ids(&["S-1", "USE-2", "CALL-3"]), None)
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_const_change_selects_the_traced_fn_that_reads_it() {
    let source = "use std::fmt;\n// trace:C-1\nconst LIMIT: u32 = 3;\n// trace:USE-2\nfn check(n: u32) -> bool {\n    n < LIMIT\n}\n// trace:OTHER-9\nfn other() {}\n";
    assert_eq!(shape(source, "LIMIT: u32"), (ids(&["C-1", "USE-2"]), None));
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_use_line_change_falls_back() {
    let source = "use std::fmt;\n// trace:U-1\nuse std::collections::{BTreeMap as Map, btree_set::{self}};\n// trace:USE-2\nfn build() -> Map<u8, u8> {\n    Map::new()\n}\n// trace:SET-3\nfn set() -> btree_set::BTreeSet<u8> {\n    Default::default()\n}\n// trace:OTHER-9\nfn other() {}\n";
    // A changed import can change trait-mediated method resolution that no
    // name mention shows (a trait, `as _`): it fails closed.
    assert_eq!(
        shape(source, "BTreeMap as Map").1,
        Some(Fallback::ChangedUse)
    );
    assert_eq!(
        shape(
            "use std::fmt;\nuse std::io::Write as _;\n// trace:F-1\nfn f() {}\n",
            "as _"
        )
        .1,
        Some(Fallback::ChangedUse)
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_change_inside_a_mod_tests_block_stays_within_its_users() {
    let source = "use std::fmt;\n// trace:LIB-1\nfn lib() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    // trace:T-2\n    const FIXTURE: u8 = 1;\n    // trace:T-3\n    #[test]\n    fn uses_fixture() {\n        let _ = FIXTURE;\n    }\n    // trace:T-4\n    #[test]\n    fn other() {\n        lib()\n    }\n}\n";
    assert_eq!(shape(source, "FIXTURE: u8"), (ids(&["T-2", "T-3"]), None));
    // The module's own brace lines are its shell, not every item inside it.
    // T-2 sits on a const, which no fn/impl owns: module evidence, as before.
    assert_eq!(shape(source, "mod tests {"), (ids(&["T-2"]), None));
    // A glob import's names are unknowable: changing it fails closed.
    assert_eq!(shape(source, "use super::*;").1, Some(Fallback::ChangedUse));
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_attribute_change_falls_back() {
    let source = "use std::fmt;\n// trace:A-1\n#[derive(Debug)]\nstruct Shape;\n// trace:USE-2\nfn make() -> Shape {\n    Shape\n}\n// trace:OTHER-9\nfn other() {}\n";
    // A derive or attribute macro expands to code the closure cannot see.
    assert_eq!(
        shape(source, "#[derive(Debug)]").1,
        Some(Fallback::ChangedAttribute)
    );
    // The item itself is still attributable when its attributes are untouched.
    assert_eq!(
        shape(source, "struct Shape;"),
        (ids(&["A-1", "USE-2"]), None)
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_closure_name_inside_a_non_std_macro_invocation_falls_back() {
    let source = "use std::fmt;\n// trace:C-1\nconst LIMIT: u32 = 3;\n// trace:USE-2\nfn show() -> String {\n    format!(\"{}\", LIMIT)\n}\n// trace:OTHER-9\nfn other() {}\n";
    // A std macro only expands its arguments, which the closure sees.
    assert_eq!(shape(source, "LIMIT: u32"), (ids(&["C-1", "USE-2"]), None));
    // Any other macro may be crate-local and expand to unseen references,
    // including one that shadows a std name.
    for call in ["my_macro!(LIMIT)", "crate::log!(LIMIT)"] {
        let source = source.replace("format!(\"{}\", LIMIT)", call);
        assert_eq!(
            shape(&source, "LIMIT: u32"),
            (ids(&["C-1", "USE-2", "OTHER-9"]), Some(Fallback::MacroName))
        );
    }
    let shadow = format!("macro_rules! format {{ ($e:expr) => {{ $e }} }}\n{source}");
    assert_eq!(shape(&shadow, "LIMIT: u32").1, Some(Fallback::MacroName));
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_item_macros_path_attributes_and_include_fall_back() {
    for (source, marker, why) in [
        (
            "use std::fmt;\n// trace:M-1\nmacro_rules! twice {\n    ($e:expr) => { $e * 2 };\n}\n// trace:OTHER-9\nfn other() {}\n",
            "$e * 2",
            Fallback::Opaque,
        ),
        (
            "use std::fmt;\n// trace:M-1\nthread_local! { static X: u8 = 1; }\n// trace:F-2\nfn f() {\n    let a = 1;\n}\n",
            "thread_local!",
            Fallback::Opaque,
        ),
        (
            "use std::fmt;\n// trace:F-1\nfn f() {\n    macro_rules! local { () => {} }\n    let a = 1;\n}\n// trace:OTHER-9\nfn other() {}\n",
            "let a = 1",
            Fallback::Opaque,
        ),
        (
            "use std::fmt;\n#[path = \"other.rs\"]\nmod other;\n// trace:F-1\nfn f() {\n    let a = 1;\n}\n// trace:OTHER-9\nfn g() {}\n",
            "let a = 1",
            Fallback::ExternalSource,
        ),
        (
            "use std::fmt;\n#[cfg_attr(unix, path = \"u.rs\")]\nmod other;\n// trace:F-1\nfn f() {\n    let a = 1;\n}\n// trace:OTHER-9\nfn g() {}\n",
            "let a = 1",
            Fallback::ExternalSource,
        ),
        (
            "use std::fmt;\nmod other;\n// trace:F-1\nfn f() {\n    let a = 1;\n}\n// trace:OTHER-9\nfn g() {}\n",
            "let a = 1",
            Fallback::ExternalSource,
        ),
        (
            "use std::fmt;\ninclude!(\"gen.rs\");\n// trace:F-1\nfn f() {\n    let a = 1;\n}\n// trace:OTHER-9\nfn g() {}\n",
            "let a = 1",
            Fallback::ExternalSource,
        ),
    ] {
        let (_, fallback) = shape(source, marker);
        assert_eq!(fallback, Some(why), "{source}");
    }
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_unchanged_old_scope_is_kept_and_only_grown_by_references() {
    // A change inside an impl method keeps the whole impl as before; the
    // methods it defines pull in a free fn that calls one of them.
    let source = "use std::fmt;\n// trace:I-1\nimpl Thing {\n    // trace:M-2\n    fn get(&self) -> u8 {\n        1\n    }\n}\n// trace:CALL-3\nfn caller(t: &Thing) -> u8 {\n    t.get()\n}\n// trace:OTHER-9\nfn other() {}\nstruct Thing;\n";
    let (old, old_fallback) = source_evidence("a.rs", source, &[5..6], Scope::ChangedItemsV1);
    assert_eq!((old, old_fallback), (ids(&["I-1", "M-2"]), None));
    assert_eq!(
        shape(source, "        1"),
        (ids(&["I-1", "M-2", "CALL-3"]), None)
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn ac2_test_only_out_of_line_modules_are_ordinary_units() {
    for module in [
        "#[cfg(test)]\n#[path = \"tests/a_tests.rs\"]\nmod tests;",
        "#[cfg(test)]\nmod tests;",
    ] {
        let source = format!("use std::fmt;\n// trace:F-1\nfn f() {{\n    let a = 1;\n}}\n// trace:OTHER-9\nfn g() {{}}\n{module}\n");
        assert_eq!(
            shape(&source, "let a = 1"),
            (ids(&["F-1"]), None),
            "{module}"
        );
    }
    // Test-only, but under a cfg_attr path the condition can re-path it.
    let source = "use std::fmt;\n// trace:F-1\nfn f() {\n    let a = 1;\n}\n#[cfg(all(test, unix))]\n#[path = \"t.rs\"]\nmod tests;\n";
    assert_eq!(shape(source, "let a = 1").1, Some(Fallback::ExternalSource));
}
