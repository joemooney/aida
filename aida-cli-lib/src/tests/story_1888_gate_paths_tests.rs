//! STORY-1888: operator-added protected paths (tighten-only) and the drift
//! ratchet that keeps [`GATE_PROTECTED_PATHS`] complete. This file is on
//! the floor itself, so a mandate-shipped PR cannot weaken the ratchet.
// trace:STORY-1888 | ai:claude
// mandate-gate

use super::*;
use std::path::PathBuf;

fn layer(toml: &str) -> GateLayer {
    parse_layer(toml, "test").unwrap()
}

fn paths(ps: &[&str]) -> Vec<String> {
    ps.iter().map(|p| p.to_string()).collect()
}

// ── tighten-only config layer ───────────────────────────────────────────────

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn protected_paths_parse_exact_and_prefix_entries() {
    let l = layer(
        "[mandate.human_gates]\nprotected_paths = [\"aida-cli-lib/src/role.rs\", \"./gate/\"]\n",
    );
    assert_eq!(
        l.protected_paths,
        ["aida-cli-lib/src/role.rs", "gate/"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn malformed_protected_entries_are_config_errors() {
    for bad in [
        "\"\"",
        "\"/etc/passwd\"",
        "\"a/../mandate.rs\"",
        "\"src/*.rs\"",
        "\"src/[ab].rs\"",
        "\"a\\\\b.rs\"",
        "\"a//b.rs\"",
        "\"a//\"",
        "\"C:/x.rs\"",
        "1",
    ] {
        let text = format!("[mandate.human_gates]\nprotected_paths = [{bad}]\n");
        assert!(
            parse_layer(&text, "test").is_err(),
            "`{bad}` must not parse (an error gates every op)"
        );
    }
    assert!(parse_layer(
        "[mandate.human_gates]\nprotected_paths = \"x.rs\"\n",
        "test"
    )
    .is_err());
}

// trace:STORY-1888.acd4bc79 | ai:claude
#[test]
fn a_directory_prefix_matches_nested_files_on_a_component_boundary() {
    let mut set = ProtectedPaths::floor();
    set.extend(PathSource::Operator, ["foo/".to_string()]);
    let hit = touches_gates(&paths(&["README.md", "./foo/a/b.rs"]), &set).unwrap();
    assert_eq!(
        hit,
        GateMatch {
            path: "foo/a/b.rs".into(),
            entry: "foo/".into(),
            source: PathSource::Operator,
        }
    );
    assert_eq!(touches_gates(&paths(&["foobar/x.rs", "foo"]), &set), None);
    // An exact entry never matches as a prefix.
    set.extend(PathSource::Operator, ["bar.rs".to_string()]);
    assert_eq!(touches_gates(&paths(&["bar.rs.bak"]), &set), None);
}

// trace:TASK-2168 | ai:agy
#[test]
fn a_directory_entry_without_a_trailing_slash_matches_files_under_it() {
    let mut set = ProtectedPaths::floor();
    let l = layer("[mandate.human_gates]\nprotected_paths = [\"./aida-cli-lib/src/gate\"]\n");
    set.extend(PathSource::Operator, l.protected_paths);
    for file in [
        "aida-cli-lib/src/gate/mod.rs",
        "./aida-cli-lib/src/gate/nested/check.rs",
        "aida-cli-lib/src/gate",
    ] {
        let hit = touches_gates(&paths(&["README.md", file]), &set)
            .unwrap_or_else(|| panic!("{file} must match directory entry without trailing '/'"));
        assert_eq!(
            hit,
            GateMatch {
                path: file.trim_start_matches("./").into(),
                entry: "aida-cli-lib/src/gate".into(),
                source: PathSource::Operator,
            }
        );
    }
    assert_eq!(
        touches_gates(
            &paths(&[
                "aida-cli-lib/src/gates.rs",
                "aida-cli-lib/src/gate_extra/x.rs"
            ]),
            &set
        ),
        None
    );
}

// trace:STORY-1888.acd4bc79 | ai:claude
#[test]
fn no_layer_can_shrink_the_floor() {
    let committed = layer(
        "[mandate.human_gates]\nops = []\nprotected_paths = [\"aida-cli-lib/src/mandate.rs\", \"a.rs\"]\n",
    );
    let operator = layer("[mandate.human_gates]\nprotected_paths = [\"a.rs\", \"b/\"]\n");
    for g in [
        union(committed, operator),
        union(GateLayer::default(), GateLayer::default()),
    ] {
        for floor in GATE_PROTECTED_PATHS {
            let hit = touches_gates(&paths(&[floor]), &g.protected)
                .unwrap_or_else(|| panic!("{floor} fell off the floor"));
            assert_eq!(hit.source, PathSource::BuiltIn, "{floor}");
        }
    }
    let g = union(
        layer("[mandate.human_gates]\nprotected_paths = [\"a.rs\"]\n"),
        layer("[mandate.human_gates]\nprotected_paths = [\"a.rs\", \"b/\"]\n"),
    );
    let sources: Vec<_> = g
        .protected
        .entries()
        .iter()
        .skip(GATE_PROTECTED_PATHS.len())
        .map(|e| (e.pattern.as_str(), e.source))
        .collect();
    assert_eq!(
        sources,
        vec![
            ("a.rs", PathSource::Committed),
            ("b/", PathSource::Operator)
        ]
    );
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn an_unreadable_load_still_checks_the_floor() {
    let gates: std::result::Result<HumanGates, String> = Err("no origin/main".into());
    let set = ProtectedPaths::of(&gates);
    assert!(touches_gates(&paths(&["aida-cli-lib/src/pr_ship.rs"]), &set).is_some());
    assert!(gate_reason(&gates, GateOp::Ship).is_some());
}

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// trace:STORY-1888.acd4bc79 | ai:claude
#[test]
fn a_branch_only_edit_to_the_committed_layer_has_no_effect() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.path().join(".aida")).unwrap();
    std::fs::write(
        repo.path().join(".aida/config.toml"),
        "[mandate.human_gates]\nprotected_paths = [\"kept.rs\"]\n",
    )
    .unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "gates"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    // A branch adds an entry and drops one; origin/main still decides.
    std::fs::write(
        repo.path().join(".aida/config.toml"),
        "[mandate.human_gates]\nprotected_paths = [\"branch.rs\"]\n",
    )
    .unwrap();
    git(repo.path(), &["commit", "-qam", "branch edit"]);
    let g = union(committed_layer(repo.path()).unwrap(), GateLayer::default());
    let hit = touches_gates(&paths(&["kept.rs"]), &g.protected).unwrap();
    assert_eq!(hit.source, PathSource::Committed);
    assert_eq!(touches_gates(&paths(&["branch.rs"]), &g.protected), None);
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn refusal_text_names_the_entry_and_its_source() {
    let hit = GateMatch {
        path: "gate/x.rs".into(),
        entry: "gate/".into(),
        source: PathSource::Operator,
    };
    assert_eq!(
        hit.describe(),
        "`gate/x.rs`, protected by `gate/` (~/.aida/config.toml)"
    );
    let refusal = crate::mandate::MandateRefusal::TouchesGates(hit);
    let msg = refusal.message(7);
    assert!(
        msg.contains("`gate/`") && msg.contains("~/.aida/config.toml"),
        "{msg}"
    );
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn status_json_lists_each_entry_with_its_source() {
    let mut set = ProtectedPaths::floor();
    set.extend(PathSource::Committed, ["x/".to_string()]);
    let json = set.to_json();
    assert_eq!(json[0]["source"], "built-in");
    assert_eq!(json[GATE_PROTECTED_PATHS.len()]["path"], "x/");
    assert_eq!(
        json[GATE_PROTECTED_PATHS.len()]["source"],
        "origin/main:.aida/config.toml"
    );
}

/// A repo whose `origin/main` carries `config` as `.aida/config.toml`.
fn repo_with_committed_config(config: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.path().join(".aida")).unwrap();
    std::fs::write(repo.path().join(".aida/config.toml"), config).unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "gates"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    repo
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn mandate_status_lists_every_protected_entry_with_its_source() {
    let repo = repo_with_committed_config(
        "[mandate.human_gates]\nprotected_paths = [\"gate/\", \"kept.rs\"]\n",
    );
    let state = crate::mandate::MandateState::None;
    let now = chrono::Utc::now();
    let text = crate::mandate::status_report(&state, now, Some(repo.path()), false);
    assert!(text.contains("  protected paths:\n"), "{text}");
    for floor in GATE_PROTECTED_PATHS {
        assert!(
            text.contains(&format!("    {floor}  [built-in]\n")),
            "{floor} missing:\n{text}"
        );
    }
    for entry in ["gate/", "kept.rs"] {
        assert!(
            text.contains(&format!("    {entry}  [origin/main:.aida/config.toml]\n")),
            "{entry} missing:\n{text}"
        );
    }
    let json: serde_json::Value = serde_json::from_str(&crate::mandate::status_report(
        &state,
        now,
        Some(repo.path()),
        true,
    ))
    .unwrap();
    let entries = json["protected_paths"].as_array().unwrap();
    assert!(entries.contains(&serde_json::json!({
        "path": "gate/",
        "source": "origin/main:.aida/config.toml",
    })));
    assert_eq!(entries[0]["source"], "built-in");
}

// trace:STORY-1888.ac4d90ed | ai:claude
#[test]
fn mandate_status_with_an_unreadable_committed_layer_shows_the_floor_only() {
    let repo = repo_with_committed_config("[mandate.human_gates]\nprotected_paths = [\"a//\"]\n");
    let state = crate::mandate::MandateState::None;
    let now = chrono::Utc::now();
    let text = crate::mandate::status_report(&state, now, Some(repo.path()), false);
    assert!(
        text.contains("  protected paths (gate config unreadable, built-in floor only: "),
        "{text}"
    );
    let listed: Vec<&str> = text.lines().filter(|l| l.starts_with("    ")).collect();
    assert_eq!(listed.len(), GATE_PROTECTED_PATHS.len(), "{text}");
    assert!(listed.iter().all(|l| l.ends_with("  [built-in]")), "{text}");
    let json: serde_json::Value = serde_json::from_str(&crate::mandate::status_report(
        &state,
        now,
        Some(repo.path()),
        true,
    ))
    .unwrap();
    let pp = &json["protected_paths"];
    assert!(pp["error"].as_str().unwrap().contains("a//"), "{pp}");
    assert_eq!(pp["floor"], ProtectedPaths::floor().to_json());
}

// ── drift ratchet ───────────────────────────────────────────────────────────

/// The Rust source roots the ratchet scans, relative to the workspace root.
const SCANNED_ROOTS: &[&str] = &[
    "aida-cli-lib/src",
    "aida-cli/src",
    "aida-core/src",
    "aida-server/src",
    "aida-tui/src",
];

const MARKER: &str = "// mandate-gate";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "tests" {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            out.push(path);
        }
    }
}

/// `{` minus `}` on one line, ignoring line comments, string literals and
/// `'{'` / `'}'` char literals.
fn brace_delta(line: &str) -> i32 {
    let line = line.replace("'{'", "").replace("'}'", "");
    let mut depth = 0;
    let mut in_str = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_str => {
                chars.next();
            }
            '"' => in_str = !in_str,
            '/' if !in_str && chars.peek() == Some(&'/') => break,
            '{' if !in_str => depth += 1,
            '}' if !in_str => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// The lines of `text` outside `#[cfg(test)]` and `#[test]` items. An item
/// ends at its `;` (before any brace opens) or where its braces balance, so
/// a one-line item or one closed by `});` never swallows the next item.
fn non_test_lines(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed == "#[cfg(test)]" || trimmed == "#[test]" {
            i += 1;
            while i < lines.len() && lines[i].trim_start().starts_with("#[") {
                i += 1;
            }
            let mut depth = 0;
            let mut opened = false;
            while i < lines.len() {
                let item = lines[i];
                opened |= item.contains('{');
                depth += brace_delta(item);
                i += 1;
                if (opened && depth <= 0) || (!opened && item.trim_end().ends_with(';')) {
                    break;
                }
            }
            continue;
        }
        out.push(line);
        i += 1;
    }
    out
}

/// Why a file is gate authority: its marker, or the first function whose
/// name reads as a ship, merge, hold, review or seat authority decision.
fn gate_authority(text: &str) -> Option<String> {
    let name = regex::Regex::new(
        r"^(\w*_)?(approval|authority|hold|ship|merge|verdict|seat|mandate|signoff|review)\w*_(gate|gated|authority|authorized|decision|refusal)$|_authority_from$",
    )
    .unwrap();
    let fn_def = regex::Regex::new(
        r#"^\s*(pub(\([^)]*\))?\s+)?((const|async|unsafe)\s+)*(extern\s+("[^"]*"\s+)?)?fn\s+(?P<name>\w+)"#,
    )
    .unwrap();
    let lines = non_test_lines(text);
    if lines.iter().any(|l| l.trim() == MARKER) {
        return Some(format!("carries `{MARKER}`"));
    }
    lines.iter().find_map(|l| {
        let f = fn_def.captures(l)?.name("name")?.as_str();
        name.is_match(f).then(|| format!("defines `fn {f}`"))
    })
}

fn scan() -> std::collections::BTreeMap<String, String> {
    let root = workspace_root();
    let mut files = Vec::new();
    for r in SCANNED_ROOTS {
        rust_files(&root.join(r), &mut files);
    }
    files
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(&f).ok()?;
            let rel = f
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            gate_authority(&text).map(|why| (rel, why))
        })
        .collect()
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn ratchet_skips_test_items() {
    let text = "fn ok() {}\n#[cfg(test)]\nmod tests {\n    fn ship_gate() {}\n}\n    #[test]\n    fn merge_decision() {\n    }\n";
    assert_eq!(gate_authority(text), None);
    // A one-line test item ends on its own line ...
    let one_line = "#[cfg(test)]\nfn fixture() -> u8 { 1 }\npub(crate) fn ship_gate() {}\n";
    assert!(gate_authority(one_line).is_some(), "{one_line}");
    // ... and so does one closed by `});`.
    let closure = "#[cfg(test)]\nstatic FIXTURE: Lazy<u8> = Lazy::new(|| {\n    1\n});\nfn merge_decision() {}\n";
    assert!(gate_authority(closure).is_some(), "{closure}");
    let braces =
        "#[test]\nfn t() {\n    let _ = \"}\"; // }\n    let _ = '}';\n}\nfn ship_gate() {}\n";
    assert!(gate_authority(braces).is_some(), "{braces}");
    assert!(gate_authority("pub(crate) fn hold_authorized() {}\n").is_some());
    assert!(gate_authority("fn advisor_authority_from() {}\n").is_some());
    assert!(gate_authority("//! notes\n// mandate-gate\n").is_some());
    assert_eq!(gate_authority("// see `// mandate-gate` markers\n"), None);
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn ratchet_sees_qualified_fns() {
    for def in [
        "pub(crate) const fn ship_gate() {}",
        "unsafe fn merge_decision() {}",
        "pub extern \"C\" fn hold_authorized() {}",
        "pub(crate) const unsafe fn verdict_gate() {}",
        "extern fn seat_authority() {}",
        "async fn review_decision() {}",
    ] {
        assert!(gate_authority(&format!("{def}\n")).is_some(), "{def}");
    }
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn gate_authority_code_is_on_the_floor() {
    let missing: Vec<String> = scan()
        .into_iter()
        .filter(|(path, _)| {
            !GATE_PROTECTED_PATHS.contains(&path.as_str())
                && !KNOWN_UNPROTECTED.contains(&path.as_str())
        })
        .map(|(path, why)| format!("  {path} ({why})"))
        .collect();
    assert!(
        missing.is_empty(),
        "gate-authority code outside the mandate's protected floor:\n{}\n\
         Move the function into a file already on GATE_PROTECTED_PATHS in \
         aida-cli-lib/src/mandate_gates.rs, or add its path there (either change \
         then needs a human merge). Do not remove the marker, rename the function, \
         or widen KNOWN_UNPROTECTED to get past this check.",
        missing.join("\n")
    );
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn every_floor_source_file_carries_the_marker() {
    let root = workspace_root();
    let unmarked: Vec<&str> = GATE_PROTECTED_PATHS
        .iter()
        .copied()
        .filter(|p| p.ends_with(".rs"))
        .filter(|p| {
            let text = std::fs::read_to_string(root.join(p))
                .unwrap_or_else(|e| panic!("floor file {p} is unreadable: {e}"));
            !text.lines().any(|l| l.trim() == MARKER)
        })
        .collect();
    assert!(
        unmarked.is_empty(),
        "floor files without a `{MARKER}` line: {unmarked:?}. Add the marker; \
         do not drop the file from GATE_PROTECTED_PATHS."
    );
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn known_unprotected_only_shrinks() {
    let found = scan();
    let stale: Vec<&str> = KNOWN_UNPROTECTED
        .iter()
        .copied()
        .filter(|p| !found.contains_key(*p) || GATE_PROTECTED_PATHS.contains(p))
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_UNPROTECTED entries that no longer hold gate authority outside the \
         floor: {stale:?}. Delete them from KNOWN_UNPROTECTED in mandate_gates.rs; \
         the list only shrinks."
    );
}
