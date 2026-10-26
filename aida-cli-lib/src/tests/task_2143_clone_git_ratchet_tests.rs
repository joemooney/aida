//! TASK-2143 (TASK-2053 S4a): the ratchet that replaces S3's "grep gate
//! against host git in clones". A grep cannot tell git aimed at a lease's
//! clone from git aimed at a worktree, so the gate counts instead. In every
//! non-test source file that reads a lease's `worktree_path`, it counts the
//! raw `Command::new("git")` builders. Runtime refusal of a seat clone lives
//! in `aida_core::git_ops::host_git_command`, and a raw builder skips it.
//! trace:TASK-2143 | trace:TASK-2053 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Raw-git builders per file. COUNTS MAY ONLY GO DOWN. When you move a call
/// onto `host_git_command`, lower its count here in the same change: the
/// test fails on a count that no longer matches, in either direction. A file
/// that drops to zero leaves this list. Never raise a count; build the new
/// call with `host_git_command`, or justify a call that can never see a seat
/// clone with `// raw-git-ok(TASK-2053): <why>` in the three lines above it.
const RAW_GIT_ALLOWLIST: &[(&str, usize)] = &[
    ("aida-cli-lib/src/agent_launch.rs", 1),
    ("aida-cli-lib/src/doctor_cmd.rs", 18),
    ("aida-cli-lib/src/doctor_root.rs", 4),
    ("aida-cli-lib/src/leases.rs", 2),
    ("aida-cli-lib/src/lib_part5.rs", 6),
    ("aida-cli-lib/src/mcp.rs", 1),
    ("aida-cli-lib/src/orchestrator_leases.rs", 6),
    ("aida-cli-lib/src/phase_driver.rs", 1),
    ("aida-cli-lib/src/pr_cmd.rs", 53),
    ("aida-cli-lib/src/queue_cmd.rs", 4),
    ("aida-cli-lib/src/seat_clone.rs", 2),
    ("aida-cli-lib/src/session_end.rs", 1),
    ("aida-cli-lib/src/session_reap.rs", 7),
    ("aida-cli-lib/src/session_start.rs", 18),
    ("aida-cli-lib/src/status_awaiting.rs", 1),
    ("aida-cli-lib/src/status_cmd.rs", 1),
    // S4b: status_prs.rs and trace.rs began reading leases (seat_trees);
    // every raw site in them runs in the main checkout (`git -C project_root`).
    ("aida-cli-lib/src/status_prs.rs", 12),
    ("aida-cli-lib/src/status_sections.rs", 5),
    ("aida-cli-lib/src/trace.rs", 12),
    ("aida-cli-lib/src/worktree_dismiss.rs", 4),
    ("aida-cli-lib/src/worktree_gc.rs", 14),
    ("aida-cli-lib/src/worktree_scope_gate.rs", 1),
    ("aida-core/src/git_ops.rs", 5),
    ("aida-cli-lib/src/linkage_branch.rs", 0),
];

/// Files that run git against a tree handed to them by a caller, so they
/// never name `worktree_path` themselves; the gate scans them anyway.
/// `linkage_branch.rs` holds `aida show`'s in-flight branch search, moved out
/// of `trace.rs` (a lease reader) by BUG-2221 P2; without this entry the move
/// would take its two git builders out of the gate.
// trace:BUG-2221 | ai:claude
const ALWAYS_SCANNED: &[&str] = &["aida-cli-lib/src/linkage_branch.rs"];

/// Lexical state across lines: braces inside Rust literals and comments do
/// not close a test module. Raw strings close only with their full delimiter.
// trace:TASK-2190 | ai:codex
#[derive(Default)]
struct BraceState {
    string: bool,
    raw_hashes: Option<usize>,
    block_depth: usize,
}

fn brace_delta(line: &str, state: &mut BraceState) -> i64 {
    let b = line.as_bytes();
    let (mut i, mut delta) = (0, 0);
    while i < b.len() {
        if let Some(hashes) = state.raw_hashes {
            if b[i] == b'"'
                && b.get(i + 1..i + 1 + hashes)
                    .is_some_and(|tail| tail.iter().all(|c| *c == b'#'))
            {
                state.raw_hashes = None;
                i += 1 + hashes;
            } else {
                i += 1;
            }
            continue;
        }
        if state.block_depth > 0 {
            if b[i..].starts_with(b"/*") {
                state.block_depth += 1;
                i += 2;
            } else if b[i..].starts_with(b"*/") {
                state.block_depth -= 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if state.string {
            match b[i] {
                b'\\' => i += 2,
                b'"' => {
                    state.string = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if b[i..].starts_with(b"//") {
            break;
        }
        if b[i..].starts_with(b"/*") {
            state.block_depth = 1;
            i += 2;
            continue;
        }
        if b[i] == b'r' {
            let mut j = i + 1;
            while b.get(j) == Some(&b'#') {
                j += 1;
            }
            if b.get(j) == Some(&b'"') {
                state.raw_hashes = Some(j - i - 1);
                i = j + 1;
                continue;
            }
        }
        match b[i] {
            b'"' => state.string = true,
            // Skip character literals, while leaving lifetimes alone.
            b'\'' => {
                let mut j = i + 1;
                if b.get(j..j + 3) == Some(b"\\u{") {
                    j += 3;
                    while b
                        .get(j)
                        .is_some_and(|c| c.is_ascii_hexdigit() || *c == b'_')
                    {
                        j += 1;
                    }
                    if b.get(j) == Some(&b'}') {
                        j += 1;
                    }
                } else if b.get(j) == Some(&b'\\') {
                    j += 2;
                } else if let Some(c) = line.get(j..).and_then(|s| s.chars().next()) {
                    j += c.len_utf8();
                }
                if b.get(j) == Some(&b'\'') {
                    i = j + 1;
                    continue;
                }
            }
            b'{' => delta += 1,
            b'}' => delta -= 1,
            _ => {}
        }
        i += 1;
    }
    delta
}

/// The 0-based lines of `#[cfg(test)] mod name { ... }` blocks, at any
/// indentation and with the attribute on its own line or the module's,
/// matched by brace balance.
fn test_module_lines(lines: &[&str]) -> std::collections::HashSet<usize> {
    static MODULE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\s*(pub(\(crate\))? )?mod \w+\s*\{").unwrap()
    });
    let module = &*MODULE;
    let mut skip = std::collections::HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        let attr = lines[i].trim_start();
        let start = match attr.strip_prefix("#[cfg(test)]") {
            Some(rest) if module.is_match(rest) => Some(i),
            Some(rest)
                if rest.trim().is_empty()
                    && lines.get(i + 1).is_some_and(|l| module.is_match(l)) =>
            {
                Some(i + 1)
            }
            _ => None,
        };
        let Some(start) = start else {
            i += 1;
            continue;
        };
        let (mut depth, mut state) = (0, BraceState::default());
        let mut j = start;
        while j < lines.len() {
            skip.insert(j);
            let line = if j == start {
                lines[j]
                    .split_once("#[cfg(test)]")
                    .map_or(lines[j], |(_, rest)| rest)
            } else {
                lines[j]
            };
            depth += brace_delta(line, &mut state);
            if depth <= 0 {
                break;
            }
            j += 1;
        }
        i = j + 1;
    }
    skip
}

/// For one source file: `None` when its non-test code never names the
/// lease field `worktree_path`, else the number of raw git builders
/// (`<Type>::new("git")`) without a `raw-git-ok(TASK-2053):` justification
/// in the three lines above.
fn raw_git_in_lease_reader(src: &str) -> Option<usize> {
    raw_git_builders(src, true)
}

/// [`raw_git_in_lease_reader`], or with `require_field` false the raw-git
/// count of any file (`Some` always), for [`ALWAYS_SCANNED`].
fn raw_git_builders(src: &str, require_field: bool) -> Option<usize> {
    static RAW: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\b\w+::new\("git"\)"#).unwrap());
    static FIELD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\bworktree_path\b").unwrap());
    // Cheap pre-filter: most files never name the field.
    if require_field && !src.contains("worktree_path") {
        return None;
    }
    let (raw, field) = (&*RAW, &*FIELD);
    let lines: Vec<&str> = src.lines().collect();
    let tests = test_module_lines(&lines);
    let code = |i: usize| !tests.contains(&i) && !lines[i].trim_start().starts_with("//");
    if require_field && !(0..lines.len()).any(|i| code(i) && field.is_match(lines[i])) {
        return None;
    }
    let count = (0..lines.len())
        .filter(|&i| code(i) && raw.is_match(lines[i]))
        .filter(|&i| {
            !lines[i.saturating_sub(3)..i]
                .iter()
                .any(|l| l.contains("raw-git-ok(TASK-2053):"))
        })
        .count();
    Some(count)
}

/// The non-test `.rs` sources of every workspace crate (skipping `tests`
/// directories, `*_tests.rs` and `tests.rs`), keyed by workspace-relative path.
fn workspace_sources(workspace: &Path) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();
    let crates: Vec<PathBuf> = std::fs::read_dir(workspace)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|dir| dir.join("Cargo.toml").is_file() && dir.join("src").is_dir())
        .collect();
    for krate in crates {
        let mut stack = vec![krate.join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "tests" {
                        stack.push(path);
                    }
                    continue;
                }
                if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                    let rel = path.strip_prefix(workspace).unwrap().display().to_string();
                    sources.insert(rel, std::fs::read_to_string(&path).unwrap());
                }
            }
        }
    }
    sources
}

/// Every mismatch between the measured counts and the allowlist.
fn ratchet_violations(
    measured: &BTreeMap<String, usize>,
    allowed: &[(&str, usize)],
) -> Vec<String> {
    let allowed: BTreeMap<&str, usize> = allowed.iter().copied().collect();
    let mut violations = Vec::new();
    for (file, &count) in measured {
        match allowed.get(file.as_str()) {
            None if count > 0 => violations.push(format!(
                "{file}: {count} new raw git builder(s) in a file that reads a lease \
                 worktree_path; use aida_core::git_ops::host_git_command"
            )),
            Some(&max) if count > max => violations.push(format!(
                "{file}: {count} raw git builders, allowlist says {max}; use \
                 aida_core::git_ops::host_git_command for the new one"
            )),
            Some(&max) if count < max => violations.push(format!(
                "{file}: {count} raw git builders, allowlist says {max}; lower the \
                 allowlist to {count} (the ratchet only tightens)"
            )),
            _ => {}
        }
    }
    for (file, max) in allowed {
        if !measured.contains_key(file) {
            violations.push(format!(
                "{file}: allowlisted for {max} but no longer reads worktree_path or is gone; \
                 remove it from the allowlist"
            ));
        }
    }
    violations
}

#[test]
fn task_2143_raw_git_scanner_counts_only_lease_readers() {
    let reader = "fn a(lease: &Lease) {\n    let _ = std::process::Command::new(\"git\")\n        \
                  .arg(&lease.worktree_path)\n        .output();\n}\n\
                  fn b(dir: &Path) {\n    // raw-git-ok(TASK-2053): never a clone.\n    \
                  let _ = Command::new(\"git\").arg(dir).output();\n}\n\
                  // Command::new(\"git\") in a comment\n\
                  #[cfg(test)]\nmod t {\n    fn c() { Command::new(\"git\"); }\n}\n";
    assert_eq!(raw_git_in_lease_reader(reader), Some(1));
    let other = "fn a(dir: &Path) { let _ = Command::new(\"git\").arg(dir).output(); }\n";
    assert_eq!(raw_git_in_lease_reader(other), None);
    // The field named only in a test module does not make a lease reader.
    let test_only = "fn a() { let _ = Command::new(\"git\"); }\n#[cfg(test)]\nmod t {\n    \
                     fn c(l: &Lease) { let _ = &l.worktree_path; }\n}\n";
    assert_eq!(raw_git_in_lease_reader(test_only), None);
}

/// Review round 1, item 5: a one-line test module and a nested, indented
/// one are skipped by brace balance, and the code after each still counts.
#[test]
fn task_2143_test_modules_are_skipped_by_brace_balance() {
    let one_line = "fn a(l: &Lease) { let _ = &l.worktree_path; }\n\
                    #[cfg(test)] mod t { fn c() { Command::new(\"git\"); } }\n\
                    fn b() { let _ = Command::new(\"git\"); }\n";
    assert_eq!(raw_git_in_lease_reader(one_line), Some(1));
    let nested = "mod outer {\n    fn a(l: &Lease) { let _ = &l.worktree_path; }\n    \
                  #[cfg(test)]\n    mod t {\n        fn c() {\n            \
                  let _ = format!(\"{{\");\n            let _ = '}';\n            \
                  let _ = \"} \\\n            }}\";\n            \
                  Command::new(\"git\");\n        }\n    }\n    \
                  fn b() { let _ = Command::new(\"git\"); }\n}\n\
                  fn d() { let _ = Command::new(\"git\"); }\n";
    assert_eq!(raw_git_in_lease_reader(nested), Some(2));
    let lines: Vec<&str> = nested.lines().collect();
    let skipped = test_module_lines(&lines);
    assert_eq!(
        skipped
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        (3..=11).collect(),
        "{lines:#?}"
    );
}

#[test]
fn task_2143_ratchet_fails_up_down_new_and_stale() {
    let measured: BTreeMap<String, usize> = [("a.rs", 2), ("b.rs", 1), ("c.rs", 1), ("d.rs", 0)]
        .into_iter()
        .map(|(f, n)| (f.to_string(), n))
        .collect();
    let allowed = [("a.rs", 1), ("b.rs", 2), ("e.rs", 3)];
    let violations = ratchet_violations(&measured, &allowed);
    assert_eq!(violations.len(), 4, "{violations:#?}");
    assert!(violations[0].starts_with("a.rs: 2 raw git builders, allowlist says 1"));
    assert!(violations[1].contains("lower the allowlist to 1"));
    assert!(violations[2].starts_with("c.rs: 1 new raw git builder"));
    assert!(violations[3].starts_with("e.rs: allowlisted for 3"));
    assert!(ratchet_violations(&measured, &[("a.rs", 2), ("b.rs", 1), ("c.rs", 1)]).is_empty());
}

/// The gate itself, over the whole workspace.
#[test]
fn task_2143_no_new_raw_git_where_a_lease_tree_is_read() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let sources = workspace_sources(workspace);
    for required in [
        "aida-core/src/git_ops.rs",
        "aida-cli-lib/src/session_end.rs",
    ] {
        assert!(
            sources.contains_key(required),
            "the gate must scan {required}"
        );
    }
    let measured: BTreeMap<String, usize> = sources
        .iter()
        .filter_map(|(file, src)| {
            raw_git_builders(src, !ALWAYS_SCANNED.contains(&file.as_str()))
                .map(|n| (file.clone(), n))
        })
        .collect();
    for always in ALWAYS_SCANNED {
        assert!(
            measured.contains_key(*always),
            "the gate must scan {always}"
        );
    }
    let violations = ratchet_violations(&measured, RAW_GIT_ALLOWLIST);
    assert!(
        violations.is_empty(),
        "raw git where a lease tree (possibly a seat clone) is read:\n  {}",
        violations.join("\n  ")
    );
}

// trace:TASK-2190 | ai:codex
#[test]
fn task_2190_raw_string_braces_do_not_swallow_production_git() {
    for literal in [
        "r\"{\"",
        "r\"\\\"",
        "r#\"a\"b\"#",
        "'\\u{7b}'",
        "'\\u{007D}'",
        "'\\\''",
        "'é'",
        "r#\"\"{\n}\"#",
        "r###\"\"# {{ /* \n } \"## }\"###",
        "br##\"\" { } }\"##",
        "cr#\"\" {\"#",
        "/* { /* } */ { */ 0",
    ] {
        let src = format!(
            "fn lease(l: &Lease) {{ let _ = &l.worktree_path; }}\n\
             #[cfg(test)] mod t {{ fn test() {{ let _ = {literal}; }} }}\n\
             fn production() {{ Command::new(\"git\"); }}\n"
        );
        assert_eq!(raw_git_in_lease_reader(&src), Some(1), "{src}");
    }
}
