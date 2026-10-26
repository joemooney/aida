//! BUG-2221 slice 3: the trace walk behind `aida show`'s `files` linkage is
//! streaming and deadline-aware. Injected expiry probes spend the budget at
//! exact traversal points (no wall-clock races); real symlink fixtures cover
//! cycles, in-root aliases and external targets.
// trace:BUG-2221 | ai:claude
use super::*;
use std::collections::HashSet;
use std::path::Path;

fn wanted(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn files_of(hits: &std::collections::HashMap<String, Vec<TraceHit>>, id: &str) -> Vec<String> {
    let mut f: Vec<String> = hits
        .get(id)
        .map(|v| v.iter().map(|h| h.file.clone()).collect())
        .unwrap_or_default();
    f.sort();
    f.dedup();
    f
}

/// The budget is spent while ENUMERATING (before the first content file is
/// ever yielded): the walk stops promptly, reports incomplete, and never
/// claims a complete empty file set. The old shape enumerated the whole tree
/// unbounded before the first budget check.
#[test]
fn trace_deadline_expires_during_enumeration() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    // 40 empty directories, then (lexically and in practice anywhere) one
    // matching file nested deep.
    for i in 0..40 {
        std::fs::create_dir_all(root.join(format!("d{i:02}/e/f"))).unwrap();
    }
    write(
        &root.join("d39/e/f/hit.rs"),
        "// trace:TASK-7 | ai:test\nfn wanted() {}\n",
    );

    // Sanity: an unexpiring walk finds the file and is complete.
    let (hits, complete) = scan_trace_graph_streaming(root, &wanted(&["TASK-7"]), &mut || false);
    assert!(complete);
    assert_eq!(
        files_of(&hits, "TASK-7"),
        vec!["d39/e/f/hit.rs".to_string()]
    );

    // Expire after a handful of enumeration steps: no content file has been
    // read yet (the tree has 120 directories to walk first, in any order
    // fewer than 10 steps cannot reach a file three levels down in 40).
    let mut calls = 0usize;
    let (hits, complete) = scan_trace_graph_streaming(root, &wanted(&["TASK-7"]), &mut || {
        calls += 1;
        calls > 3
    });
    assert!(!complete, "expiry during enumeration must read incomplete");
    assert!(hits.is_empty(), "no content was reached before expiry");
    assert_eq!(calls, 4, "the walk stops at the first expired check");
}

/// Symlinks: an in-root directory alias is followed and keeps its displayed
/// relative path; a link back to an open ancestor is a cycle (skipped, scan
/// marked incomplete — it terminates instead of looping); a link out of the
/// project root is never descended (incomplete, not silently complete).
#[cfg(unix)]
#[test]
fn trace_symlink_cycle_and_alias_preserve_truth() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    write(
        &root.join("src/a.rs"),
        "// trace:TASK-7 | ai:test\nfn a() {}\n",
    );
    std::os::unix::fs::symlink(root.join("src"), root.join("alias")).unwrap();

    // Alias only: complete, both display paths present.
    let (hits, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(complete, "an in-root alias is ordinary coverage");
    assert_eq!(
        files_of(&hits, "TASK-7"),
        vec!["alias/a.rs".to_string(), "src/a.rs".to_string()]
    );

    // Cycle: src/loop -> src. Terminates, keeps the real hits, incomplete.
    std::os::unix::fs::symlink(root.join("src"), root.join("src/loop")).unwrap();
    let (hits, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(!complete, "a symlink cycle is a recorded gap");
    let files = files_of(&hits, "TASK-7");
    assert!(files.contains(&"src/a.rs".to_string()), "{files:?}");
    assert!(
        files.iter().all(|f| !f.contains("loop/loop")),
        "the cycle is never re-entered: {files:?}"
    );
    std::fs::remove_file(root.join("src/loop")).unwrap();

    // External target: another tree with its own trace hit is NOT scanned.
    let outside = tmp.path().join("other-repo");
    write(
        &outside.join("b.rs"),
        "// trace:TASK-7 | ai:test\nfn b() {}\n",
    );
    std::os::unix::fs::symlink(&outside, root.join("ext")).unwrap();
    let (hits, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(
        !complete,
        "an external link is refused, not silently complete"
    );
    assert!(
        files_of(&hits, "TASK-7")
            .iter()
            .all(|f| !f.starts_with("ext/")),
        "the scan must not widen into another tree"
    );

    // A dangling link is an unresolved gap too.
    std::fs::remove_file(root.join("ext")).unwrap();
    std::os::unix::fs::symlink(tmp.path().join("missing"), root.join("dangling")).unwrap();
    let (_, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(!complete, "an unresolved link is a recorded gap");
}

/// Expiry in the middle of one file's content pass keeps the hits already
/// parsed (partial truth) and reports incomplete — a huge file cannot carry
/// the walk arbitrarily past the deadline.
#[test]
fn trace_partial_read_is_incomplete() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let mut body = String::from("// trace:TASK-7 | ai:test\nfn early() {}\n");
    for i in 0..1000 {
        body.push_str(&format!("let filler_{i} = {i};\n"));
    }
    body.push_str("// trace:TASK-7 | ai:test\nfn late() {}\n");
    write(&root.join("big.rs"), &body);

    let (hits, complete) = scan_trace_graph_streaming(root, &wanted(&["TASK-7"]), &mut || false);
    assert!(complete);
    let syms: Vec<Option<String>> = hits["TASK-7"].iter().map(|h| h.symbol.clone()).collect();
    assert_eq!(syms, vec![Some("early".into()), Some("late".into())]);

    // Checks: walk start (1), entry (2), before reading the file (3), then
    // the first in-file chunk check (4) — expire there.
    let mut calls = 0usize;
    let (hits, complete) = scan_trace_graph_streaming(root, &wanted(&["TASK-7"]), &mut || {
        calls += 1;
        calls >= 4
    });
    assert!(!complete, "a partially parsed file is incomplete coverage");
    let syms: Vec<Option<String>> = hits["TASK-7"].iter().map(|h| h.symbol.clone()).collect();
    assert_eq!(
        syms,
        vec![Some("early".into())],
        "hits before expiry are kept"
    );
}

/// FILE symlinks get the same external-target fence as directory links: an
/// in-root file alias is read under its displayed path, a file link out of
/// the project root is never read (incomplete, not silently complete).
#[cfg(unix)]
#[test]
fn trace_file_symlink_outside_root_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    write(
        &root.join("src/a.rs"),
        "// trace:TASK-7 | ai:test\nfn a() {}\n",
    );
    std::os::unix::fs::symlink(root.join("src/a.rs"), root.join("alias.rs")).unwrap();
    let (hits, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(complete, "an in-root file alias is ordinary coverage");
    assert_eq!(
        files_of(&hits, "TASK-7"),
        vec!["alias.rs".to_string(), "src/a.rs".to_string()]
    );

    let outside = tmp.path().join("other-repo/b.rs");
    write(&outside, "// trace:TASK-7 | ai:test\nfn b() {}\n");
    std::os::unix::fs::symlink(&outside, root.join("ext.rs")).unwrap();
    let (hits, complete) = scan_trace_graph_streaming(&root, &wanted(&["TASK-7"]), &mut || false);
    assert!(!complete, "an external file link is a recorded gap");
    assert!(
        !files_of(&hits, "TASK-7").contains(&"ext.rs".to_string()),
        "the scan must not read a file outside the project"
    );
}
