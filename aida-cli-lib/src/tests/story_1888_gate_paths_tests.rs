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

// trace:TASK-2299 | ai:agy
#[test]
fn a_directory_entry_without_a_trailing_slash_matches_files_under_it() {
    let mut set = ProtectedPaths::floor();
    let l = layer("[mandate.human_gates]\nprotected_paths = [\"./aida-cli-lib/src/gate_dir\"]\n");
    set.extend(PathSource::Operator, l.protected_paths);
    for file in [
        "aida-cli-lib/src/gate_dir/mod.rs",
        "./aida-cli-lib/src/gate_dir/nested/check.rs",
        "aida-cli-lib/src/gate_dir",
    ] {
        let hit = touches_gates(&paths(&["README.md", file]), &set)
            .unwrap_or_else(|| panic!("{file} must match directory entry without trailing '/'"));
        assert_eq!(
            hit,
            GateMatch {
                path: file.trim_start_matches("./").into(),
                entry: "aida-cli-lib/src/gate_dir".into(),
                source: PathSource::Operator,
            }
        );
    }
    assert_eq!(
        touches_gates(
            &paths(&[
                "aida-cli-lib/src/gate_dirs.rs",
                "aida-cli-lib/src/gate_dir_extra/x.rs"
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
        "[mandate.human_gates]\nops = []\nprotected_paths = [\"aida-cli-lib/src/gate/\", \"a.rs\"]\n",
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
    assert!(touches_gates(&paths(&["aida-cli-lib/src/gate/ship.rs"]), &set).is_some());
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
    let refusal = crate::gate::mandate::MandateRefusal::TouchesGates(hit);
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
    let state = crate::gate::mandate::MandateState::None;
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
    let state = crate::gate::mandate::MandateState::None;
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

fn authority_name() -> regex::Regex {
    regex::Regex::new(
        r"^(\w*_)?(approval|authority|hold|ship|merge|verdict|seat|mandate|signoff|review)\w*_(gate|gated|authority|authorized|decision|refusal)$|_authority_from$",
    )
    .unwrap()
}

/// The functions `text` defines outside test items.
fn defined_fns(text: &str) -> Vec<String> {
    let fn_def = regex::Regex::new(
        r#"^\s*(pub(\([^)]*\))?\s+)?((const|async|unsafe)\s+)*(extern\s+("[^"]*"\s+)?)?fn\s+(?P<name>\w+)"#,
    )
    .unwrap();
    non_test_lines(text)
        .iter()
        .filter_map(|l| Some(fn_def.captures(l)?.name("name")?.as_str().to_string()))
        .collect()
}

/// STORY-1889: is `(path, f)` a confirmed name-only match?
fn exempt(path: &str, f: &str) -> bool {
    NOT_GATE_AUTHORITY
        .iter()
        .any(|(p, name, _)| *p == path && *name == f)
}

/// Why a file is gate authority: its marker, or the first function whose
/// name reads as a ship, merge, hold, review or seat authority decision and
/// is not a confirmed name-only match in [`NOT_GATE_AUTHORITY`].
fn gate_authority(path: &str, text: &str) -> Option<String> {
    if non_test_lines(text).iter().any(|l| l.trim() == MARKER) {
        return Some(format!("carries `{MARKER}`"));
    }
    let name = authority_name();
    defined_fns(text)
        .into_iter()
        .find(|f| name.is_match(f) && !exempt(path, f))
        .map(|f| format!("defines `fn {f}`"))
}

/// Every scanned non-test source file, workspace-relative, with its text.
fn sources() -> Vec<(String, String)> {
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
            Some((rel, text))
        })
        .collect()
}

fn scan() -> std::collections::BTreeMap<String, String> {
    sources()
        .into_iter()
        .filter_map(|(rel, text)| gate_authority(&rel, &text).map(|why| (rel, why)))
        .collect()
}

/// STORY-1889: on the floor as an exact path or under a prefix entry.
fn on_floor(path: &str) -> bool {
    touches_gates(&[path.to_string()], ProtectedPaths::floor_ref()).is_some()
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn ratchet_skips_test_items() {
    let text = "fn ok() {}\n#[cfg(test)]\nmod tests {\n    fn ship_gate() {}\n}\n    #[test]\n    fn merge_decision() {\n    }\n";
    assert_eq!(gate_authority("x.rs", text), None);
    // A one-line test item ends on its own line ...
    let one_line = "#[cfg(test)]\nfn fixture() -> u8 { 1 }\npub(crate) fn ship_gate() {}\n";
    assert!(gate_authority("x.rs", one_line).is_some(), "{one_line}");
    // ... and so does one closed by `});`.
    let closure = "#[cfg(test)]\nstatic FIXTURE: Lazy<u8> = Lazy::new(|| {\n    1\n});\nfn merge_decision() {}\n";
    assert!(gate_authority("x.rs", closure).is_some(), "{closure}");
    let braces =
        "#[test]\nfn t() {\n    let _ = \"}\"; // }\n    let _ = '}';\n}\nfn ship_gate() {}\n";
    assert!(gate_authority("x.rs", braces).is_some(), "{braces}");
    assert!(gate_authority("x.rs", "pub(crate) fn hold_authorized() {}\n").is_some());
    assert!(gate_authority("x.rs", "fn advisor_authority_from() {}\n").is_some());
    assert!(gate_authority("x.rs", "//! notes\n// mandate-gate\n").is_some());
    assert_eq!(
        gate_authority("x.rs", "// see `// mandate-gate` markers\n"),
        None
    );
}

#[test]
fn an_exemption_covers_only_its_own_function() {
    let (path, f, _) = NOT_GATE_AUTHORITY[0];
    assert_eq!(gate_authority(path, &format!("pub fn {f}() {{}}\n")), None);
    // The same name elsewhere, or a second match in the same file, is caught.
    assert!(gate_authority("x.rs", &format!("pub fn {f}() {{}}\n")).is_some());
    assert!(gate_authority(path, &format!("pub fn {f}() {{}}\nfn ship_gate() {{}}\n")).is_some());
}

#[test]
fn a_prefix_floor_entry_covers_the_gate_module() {
    assert!(on_floor("aida-cli-lib/src/gate/role_authority.rs"));
    assert!(on_floor(
        "aida-cli-lib/src/gate/tests/seat_authority_tests.rs"
    ));
    assert!(!on_floor("aida-cli-lib/src/gateway.rs"));
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
        assert!(
            gate_authority("x.rs", &format!("{def}\n")).is_some(),
            "{def}"
        );
    }
}

// trace:STORY-1888.aca70d99 trace:STORY-1889 | ai:claude
#[test]
fn gate_authority_code_is_on_the_floor() {
    let missing: Vec<String> = scan()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .map(|(path, why)| format!("  {path} ({why})"))
        .collect();
    assert!(
        missing.is_empty(),
        "gate-authority code outside the mandate's protected floor:\n{}\n\
         Move it into aida-cli-lib/src/gate/ (the change then needs a human merge). \
         Do not remove the marker or rename the function to get past this check.",
        missing.join("\n")
    );
}

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn every_floor_source_file_carries_the_marker() {
    let root = workspace_root();
    let mut files: Vec<String> = Vec::new();
    for entry in GATE_PROTECTED_PATHS {
        if entry.ends_with('/') {
            // STORY-1889: every non-test source file under a prefix entry.
            let mut found = Vec::new();
            rust_files(&root.join(entry), &mut found);
            assert!(!found.is_empty(), "floor prefix {entry} holds no source");
            files.extend(found.into_iter().map(|f| {
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            }));
        } else if entry.ends_with(".rs") {
            files.push((*entry).to_string());
        }
    }
    let unmarked: Vec<String> = files
        .into_iter()
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

#[test]
fn not_gate_authority_only_shrinks() {
    let root = workspace_root();
    let name = authority_name();
    let stale: Vec<String> = NOT_GATE_AUTHORITY
        .iter()
        .filter(|(path, f, reason)| {
            let defined = std::fs::read_to_string(root.join(path))
                .map(|text| defined_fns(&text).iter().any(|d| d == f))
                .unwrap_or(false);
            !defined || !name.is_match(f) || on_floor(path) || reason.trim().is_empty()
        })
        .map(|(path, f, _)| format!("{path}::{f}"))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_GATE_AUTHORITY entries that no longer name a matching function outside \
         the floor (or carry no reason): {stale:?}. Delete them from \
         NOT_GATE_AUTHORITY in mandate_gates.rs; the list only shrinks."
    );
}

/// The non-test lines of `text` that re-export a `crate::gate` item.
// trace:TASK-2319 | ai:agy
fn gate_reexports(text: &str) -> Vec<String> {
    let reexport = regex::Regex::new(
        r"^\s*pub(\([^)]*\))?\s+use\s+(crate::|self::|super::)*(\{[^}]*\b)?gate(::|\s*;|\s*\}|\s*,|\s+as\b)",
    )
    .unwrap();
    non_test_lines(text)
        .into_iter()
        .filter(|l| reexport.is_match(l))
        .map(|l| l.trim().to_string())
        .collect()
}

#[test]
fn reexport_detection() {
    assert_eq!(
        gate_reexports("pub(crate) use gate::seat_authority;\n").len(),
        1
    );
    assert_eq!(
        gate_reexports("pub(crate) use crate::gate::role_authority::{\n    a,\n};\n").len(),
        1
    );
    assert_eq!(gate_reexports("pub use crate::{gate::x, y};\n").len(), 1);
    assert!(gate_reexports("use crate::gate::seat_authority::GrantCheck;\n").is_empty());
    assert!(gate_reexports("pub(crate) use crate::gateway::x;\n").is_empty());
}

// trace:TASK-2319 | ai:agy
#[test]
fn reexport_detection_sees_aliases() {
    for line in [
        "pub(crate) use crate::gate as g;\n",
        "pub use self::gate as _;\n",
        "pub use crate::{gate, other};\n",
        "pub use crate::{other, gate as g};\n",
    ] {
        assert_eq!(gate_reexports(line).len(), 1, "{line}");
    }
    assert!(gate_reexports("pub(crate) use crate::gateway as g;\n").is_empty());
}

/// Every `#[path = "..."]` target in `text`, wherever the attribute sits on
/// its line (`#[cfg(test)] #[path = "..."] mod t;` is one valid line), in raw
/// string form (`r"..."`, `r#"..."#`), split across lines, or as
/// `#[cfg_attr(..., path = "...")]`.
// trace:TASK-2335 trace:STORY-1889 | ai:claude
fn path_attr_targets(text: &str) -> Vec<String> {
    let path_attr =
        regex::Regex::new(r#"#\s*\[\s*(?:cfg_attr\s*\([^\]]*?,\s*)?path\s*=\s*(?:r#*)?"([^"]*)""#)
            .unwrap();
    path_attr
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect()
}

/// The `#[path = "..."]` targets in `text` that point into `gate/`.
// trace:TASK-2335 | ai:agy
fn gate_path_mounts(text: &str) -> Vec<String> {
    path_attr_targets(text)
        .into_iter()
        .filter(|p| p.starts_with("gate/") || p.contains("/gate/"))
        .collect()
}

// trace:TASK-2335 | ai:agy
#[test]
fn gate_path_mount_detection() {
    assert_eq!(
        gate_path_mounts(
            "#[cfg(test)]\n#[path = \"gate/tests/story_1405_review_marker_tests.rs\"]\nmod t;\n"
        ),
        vec!["gate/tests/story_1405_review_marker_tests.rs".to_string()]
    );
    assert_eq!(
        gate_path_mounts("#[path = \"../gate/tests/x.rs\"]\nmod x;\n"),
        vec!["../gate/tests/x.rs".to_string()]
    );
    assert!(gate_path_mounts("#[path = \"tests/review_marker_tests.rs\"]\nmod t;\n").is_empty());
}

/// The mount ratchets see a `#[path]` that does not start its line, raw
/// strings, multi-line attributes and `cfg_attr` forms.
// trace:STORY-1889 | ai:claude
#[test]
fn gate_path_mount_detection_sees_inline_and_raw_forms() {
    let want = vec!["gate/tests/story_1917_dispatch_exec_tests.rs".to_string()];
    for text in [
        "#[cfg(test)] #[path = \"gate/tests/story_1917_dispatch_exec_tests.rs\"] mod t;\n",
        "#[cfg(test)] #[path = r\"gate/tests/story_1917_dispatch_exec_tests.rs\"] mod t;\n",
        "#[cfg(test)] #[path = r#\"gate/tests/story_1917_dispatch_exec_tests.rs\"#] mod t;\n",
        "#[cfg(test)]\n#[path =\n    \"gate/tests/story_1917_dispatch_exec_tests.rs\"]\nmod t;\n",
        "#[cfg_attr(test, path = \"gate/tests/story_1917_dispatch_exec_tests.rs\")] mod t;\n",
    ] {
        assert_eq!(gate_path_mounts(text), want, "{text}");
        assert!(
            mounts_dispatch_suite(text),
            "the stray dispatch-suite check misses: {text}"
        );
    }
    assert!(!mounts_dispatch_suite(
        "#[cfg(test)] #[path = \"tests/other_tests.rs\"] mod t;\n"
    ));
    assert!(!mounts_dispatch_suite(
        "let path = \"gate/tests/story_1917_dispatch_exec_tests.rs\";\n"
    ));
}

// trace:TASK-2335 | ai:agy
#[test]
fn no_unprotected_file_mounts_a_gate_module() {
    let bad: Vec<String> = sources()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .filter_map(|(path, text)| {
            let mounts = gate_path_mounts(&text);
            (!mounts.is_empty()).then(|| format!("  {path}: {}", mounts.join(", ")))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "unprotected files mount `gate/` modules via `#[path = ...]`:\n{}\n\
         A `#[path]` declaration in an unprotected file can be repointed or \
         removed without a human merge. Mount `gate/tests/*` inside a protected \
         `aida-cli-lib/src/gate/` module instead.",
        bad.join("\n")
    );

    let root = workspace_root();
    let gate_dir = root.join("aida-cli-lib/src/gate");
    let mut gate_sources = Vec::new();
    for entry in std::fs::read_dir(&gate_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                gate_sources.push(text);
            }
        }
    }
    let mut unmounted = Vec::new();
    for entry in std::fs::read_dir(gate_dir.join("tests")).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let rel = format!("tests/{}", path.file_name().unwrap().to_string_lossy());
            let needle = format!("\"{rel}\"");
            if !gate_sources.iter().any(|t| t.contains(&needle)) {
                unmounted.push(rel);
            }
        }
    }
    assert!(
        unmounted.is_empty(),
        "`aida-cli-lib/src/gate/tests/` files not mounted by any `gate/*.rs` module: {unmounted:?}"
    );
}

#[test]
fn no_unprotected_file_reexports_gate_items() {
    let unlisted: Vec<String> = sources()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .filter_map(|(path, text)| {
            let lines = gate_reexports(&text);
            (!lines.is_empty()).then(|| format!("  {path}: {}", lines.join(" | ")))
        })
        .collect();
    assert!(
        unlisted.is_empty(),
        "unprotected files re-export crate::gate items:\n{}\n\
         A re-export outside the floor can be repointed at a non-gate function \
         without a human merge. Call `crate::gate::...` directly instead.",
        unlisted.join("\n")
    );
}

// ── authority inputs bound inside the gate (STORY-1889 review F1-F3) ───────

/// The non-test source of the gate file `name` (under `aida-cli-lib/src/gate/`).
fn gate_source(name: &str) -> String {
    let text =
        std::fs::read_to_string(workspace_root().join("aida-cli-lib/src/gate").join(name)).unwrap();
    non_test_lines(&text).join("\n")
}

/// The body of `fn name` in `text` (from its signature to the next item).
fn fn_body<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("fn {name}("))
        .or_else(|| text.find(&format!("fn {name}<")))
        .unwrap_or_else(|| panic!("fn {name} is defined"));
    let rest = &text[start..];
    let end = rest[1..]
        .find("\nfn ")
        .or_else(|| rest[1..].find("\npub"))
        .unwrap_or(rest.len() - 1);
    &rest[..end + 1]
}

/// Names whose use decides gate authority. Outside the floor nothing may
/// name them: a caller that picks the human floor, the ship decision or the
/// supervised default could pick a permissive one without a human merge.
const GATE_ONLY_NAMES: &[&str] = &[
    "require_mandate_human",
    "ship_decision",
    "rework_release_decision",
    "drive_includes_supervised",
    "collect_held_prs_with",
    "verify_fresh_restart_plan",
    "append_restart",
    // trace:STORY-1889 | ai:claude
    "activate_dispatch_v2_lock_quiescent",
];

// trace:STORY-1889 | ai:claude
#[test]
fn no_unprotected_file_names_a_gate_only_authority_input() {
    let word = |n: &str| regex::Regex::new(&format!(r"\b{n}\b")).unwrap();
    let names: Vec<(&str, regex::Regex)> = GATE_ONLY_NAMES.iter().map(|n| (*n, word(n))).collect();
    let bad: Vec<String> = sources()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .filter_map(|(path, text)| {
            let body = non_test_lines(&text).join("\n");
            let hits: Vec<&str> = names
                .iter()
                .filter(|(_, re)| re.is_match(&body))
                .map(|(n, _)| *n)
                .collect();
            (!hits.is_empty()).then(|| format!("  {path}: {}", hits.join(", ")))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "unprotected files name gate-only authority inputs:\n{}\n\
         Call the gate entry that binds them instead (`start_as_human`, \
         `stop_as_human`, `mandate_ship_cover_for_ship`, `with_live_tick_inputs`, \
         `record_fresh_restart`, `activate_lock_as_human`).",
        bad.join("\n")
    );
}

// F1: mandate start/stop bind the human floor in the gate; the injectable
// seams exist only under `cfg(test)`.
// trace:STORY-1889 | ai:claude
#[test]
fn mandate_start_and_stop_bind_the_human_floor_in_the_gate() {
    let src = gate_source("mandate.rs");
    let fns = defined_fns(&src);
    for seam in ["start", "start_with", "start_with_scopes", "stop"] {
        assert!(
            !fns.iter().any(|f| f == seam),
            "gate::mandate::{seam} takes an injectable human floor; it must be #[cfg(test)]"
        );
    }
    for private in ["mint", "tombstone_all"] {
        assert!(
            src.contains(&format!("\nfn {private}(")),
            "gate::mandate::{private} must stay private to the gate module"
        );
    }
    assert!(fn_body(&src, "start_as_human").contains("&require_mandate_human,"));
    assert!(fn_body(&src, "start_as_human").contains("drive_includes_supervised("));
    assert!(fn_body(&src, "stop_as_human").contains("&require_mandate_human,"));
}

// F2: the drive tick's authority inputs are private to the gate, built only
// by `with_live_tick_inputs`.
// trace:STORY-1889 | ai:claude
#[test]
fn drive_tick_authority_inputs_are_private_to_the_gate() {
    let src = gate_source("mandate_drive.rs");
    let body = src
        .split("pub(crate) struct TickInputs<'a> {")
        .nth(1)
        .and_then(|s| s.split("\n}").next())
        .expect("TickInputs is defined in gate/mandate_drive.rs");
    for field in [
        "mandate_dir",
        "principal",
        "clock",
        "gates",
        "lanes",
        "held_prs",
        "spec_always_human",
        "spec_signoff",
        "store_readable",
    ] {
        let decl = body
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&format!("{field}:")) || l.contains(&format!(" {field}:")))
            .unwrap_or_else(|| panic!("TickInputs::{field} is declared"));
        assert!(
            !decl.starts_with("pub"),
            "TickInputs::{field} must be private to the gate: {decl}"
        );
    }
    let live = fn_body(&src, "with_live_tick_inputs");
    for input in [
        "load_store_for_lookup(",
        "read_exclusions(",
        "spec_always_human_from(",
        "spec_signoff_from(",
        "load_human_gates(",
        "collect_held_prs(",
        "current_user_id(",
    ] {
        assert!(
            live.contains(input),
            "with_live_tick_inputs must call {input}"
        );
    }
    assert!(fn_body(&src, "collect_held_prs").contains("&changed_paths)"));
    assert!(
        src.contains("\nfn changed_paths("),
        "changed_paths stays gate-private"
    );
    let restart = fn_body(&src, "record_fresh_restart");
    let verify = restart.find("verify_fresh_restart_plan(").expect("verify");
    let append = restart.find("append_restart(").expect("append");
    assert!(
        verify < append,
        "the fresh re-check runs before the lineage line"
    );
    for (path, text) in sources() {
        if !on_floor(&path) {
            assert!(
                !non_test_lines(&text).join("\n").contains("TickInputs {"),
                "{path} constructs TickInputs outside the gate"
            );
        }
    }
}

// F3: the ship cover's decision is bound in the gate.
// trace:STORY-1889 | ai:claude
#[test]
fn the_ship_cover_decision_is_bound_in_the_gate() {
    let src = gate_source("mandate_ship.rs");
    assert!(
        src.contains("pub(in crate::gate) fn mandate_ship_cover("),
        "the decision-taking cover must be gate-private"
    );
    let ship = fn_body(&src, "mandate_ship_cover_for_ship");
    assert!(ship.contains("crate::gate::mandate::ship_decision,\n        true,"));
}

// ── dispatch authority on the floor (STORY-1889 final slice) ───────────────

/// The floor is the gate prefix plus `.aida/config.toml` and protected test
/// files (STORY-1889 acceptance 3); no other production file is listed.
// trace:STORY-1889 | ai:claude
#[test]
fn the_floor_is_the_gate_prefix_config_and_protected_tests() {
    assert!(GATE_PROTECTED_PATHS.contains(&"aida-cli-lib/src/gate/"));
    for entry in GATE_PROTECTED_PATHS {
        assert!(
            *entry == ".aida/config.toml"
                || entry.starts_with("aida-cli-lib/src/gate/")
                || entry.ends_with("_tests.rs"),
            "{entry} is a production path outside aida-cli-lib/src/gate/; move its \
             authority into the gate instead of listing the file"
        );
    }
}

/// Dispatch decisions (request records and admission, claims and caps, the
/// `tick.lock`, the recovery ledger, the flow ship selectors) defined only
/// under `gate/`.
const DISPATCH_AUTHORITY_FNS: &[&str] = &[
    "check_request_authority",
    "revalidate_request",
    "validate_spec_in_store",
    "verify_frozen_brief",
    "check_serve_grant",
    "verify_claim_locked",
    "probe_worker_identity",
    "pass_admission",
    "run_admission",
    "verify_running_claim",
    "admit_execution",
    "submission_collision",
    "verdict_at_head",
    "ci_state_is_green",
    "flow_ship_decision",
    "dead_claims",
    "pending_result",
    "activate_lock_as_human",
    "activate_dispatch_v2_lock_quiescent",
    "acquire_dispatch_lock",
    "reconcile_inventory_locked",
    "inspect_dispatch_inventory_readonly",
    "check_recovery_index_bounds",
    "remove_reviewer_worktree_preserving_hold",
    "preserve_reviewer_worktree_hold",
];

const DISPATCH_AUTHORITY_TYPES: &[&str] = &[
    "DispatchRequest",
    "DispatchVerb",
    "DispatchStatus",
    "DispatchResult",
    "WorkerClaim",
    "AuthorizedDispatchCaller",
    "ClaimCaps",
    "RecoveryFenceEntry",
    "RecoveryIndex",
    "DispatchInventory",
    "DispatchLock",
    "FlowVerdict",
    "ClaimAdmission",
    "ExecutionAdmission",
    "ExecutionEntry",
    "SubmissionCollision",
];

fn defined_types(text: &str) -> Vec<String> {
    let ty = regex::Regex::new(r"^\s*(pub(\([^)]*\))?\s+)?(struct|enum)\s+(?P<name>\w+)").unwrap();
    non_test_lines(text)
        .iter()
        .filter_map(|l| Some(ty.captures(l)?.name("name")?.as_str().to_string()))
        .collect()
}

/// Matches any definition of a dispatch authority name in raw text, so the
/// ratchets parse test items only in the few files that can define one.
fn dispatch_authority_definition() -> regex::Regex {
    let names = DISPATCH_AUTHORITY_FNS
        .iter()
        .chain(DISPATCH_AUTHORITY_TYPES)
        .copied()
        .collect::<Vec<_>>()
        .join("|");
    regex::Regex::new(&format!(r"\b(fn|struct|enum)\s+({names})\b")).unwrap()
}

// trace:STORY-1889 | ai:claude
#[test]
fn dispatch_authority_is_defined_only_in_the_gate() {
    let mut in_gate: Vec<String> = Vec::new();
    let mut outside: Vec<String> = Vec::new();
    let candidate = dispatch_authority_definition();
    for (path, text) in sources() {
        if !candidate.is_match(&text) {
            continue;
        }
        let fns = defined_fns(&text);
        let types = defined_types(&text);
        let names = fns.iter().chain(types.iter()).filter(|n| {
            DISPATCH_AUTHORITY_FNS.contains(&n.as_str())
                || DISPATCH_AUTHORITY_TYPES.contains(&n.as_str())
        });
        for name in names {
            if path.starts_with("aida-cli-lib/src/gate/dispatch_") {
                in_gate.push(name.clone());
            } else {
                outside.push(format!("  {path}: {name}"));
            }
        }
    }
    assert!(
        outside.is_empty(),
        "dispatch authority defined outside aida-cli-lib/src/gate/dispatch_*.rs:\n{}",
        outside.join("\n")
    );
    for name in DISPATCH_AUTHORITY_FNS
        .iter()
        .chain(DISPATCH_AUTHORITY_TYPES)
    {
        assert!(
            in_gate.iter().any(|n| n == name),
            "{name} is not defined in aida-cli-lib/src/gate/dispatch_*.rs"
        );
    }
}

// The activate-lock human floor and the admitted record's authority fields
// are bound in the gate.
// trace:STORY-1889 | ai:claude
#[test]
fn dispatch_authority_inputs_are_bound_in_the_gate() {
    let lock = gate_source("dispatch_lock.rs");
    let activate = fn_body(&lock, "activate_lock_as_human");
    let human = activate.find("require_direct_human(").expect("human check");
    let quiescent = activate
        .find("activate_dispatch_v2_lock_quiescent(")
        .expect("activation");
    assert!(human < quiescent, "the human check runs before activation");

    let auth = gate_source("dispatch_authority.rs");
    let unprotected: Vec<(String, String)> = sources()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .collect();
    // No file outside the floor writes a gate-built record as a literal.
    let constructs_outside = |ty: &str| {
        let literal = format!("{ty} {{");
        unprotected
            .iter()
            .filter(|(_, text)| text.contains(&literal))
            .filter(|(_, text)| non_test_lines(text).join("\n").contains(&literal))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>()
    };
    for ty in [
        "AuthorizedDispatchCaller",
        "ClaimCaps",
        "ClaimAdmission",
        "ExecutionAdmission",
    ] {
        let body = auth
            .split(&format!("pub(crate) struct {ty} {{"))
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .unwrap_or_else(|| panic!("{ty} is defined in gate/dispatch_authority.rs"));
        for field in body.lines().map(str::trim).filter(|l| l.contains(':')) {
            assert!(
                !field.starts_with("pub"),
                "{ty} field must be private: {field}"
            );
        }
        let bad = constructs_outside(ty);
        assert!(bad.is_empty(), "{bad:?} construct {ty} outside the gate");
    }
    // The request and result records: the request's fields and the result's
    // identity, mandate, grant, claim and status fields are gate-private, and
    // no file outside the floor writes either record as a literal.
    for (ty, fields) in [
        (
            "DispatchRequest",
            &[
                "schema_version",
                "id",
                "requested_at",
                "principal",
                "requester_seat",
                "requester_grant_id",
                "matched_binding",
                "mandate_id",
                "dedupe_key",
                "target_key",
                "action",
            ][..],
        ),
        (
            "DispatchResult",
            &[
                "id",
                "requested_at",
                "requester_grant_prefix",
                "mandate_id",
                "worker_pid",
                "worker_start",
                "claim_token",
                "status",
                "started_at",
                "finished_at",
                "summary",
            ][..],
        ),
    ] {
        let body = auth
            .split(&format!("pub(crate) struct {ty} {{"))
            .nth(1)
            .and_then(|s| s.split("\n}").next())
            .unwrap_or_else(|| panic!("{ty} is defined in gate/dispatch_authority.rs"));
        for field in fields {
            let decl = body
                .lines()
                .map(str::trim)
                .find(|l| l.contains(&format!(" {field}:")) || l.starts_with(&format!("{field}:")))
                .unwrap_or_else(|| panic!("{ty}::{field} is declared"));
            assert!(
                decl.starts_with("pub(in crate::gate) ") || !decl.starts_with("pub"),
                "{ty}::{field} must be private to the gate: {decl}"
            );
        }
        let bad = constructs_outside(ty);
        assert!(bad.is_empty(), "{bad:?} construct {ty} outside the gate");
    }
    let pending = fn_body(&auth, "pending_result");
    for bound in [
        "id: req.id.clone(),",
        "requested_at: req.requested_at,",
        "requester_grant_prefix: self.grant_id.chars().take(8).collect(),",
        "mandate_id: self.mandate.id.clone(),",
        "worker_pid: None,",
        "claim_token: None,",
    ] {
        assert!(pending.contains(bound), "pending_result must set `{bound}`");
    }

    let record = fn_body(&auth, "admitted_request");
    for bound in [
        "schema_version: DISPATCH_SCHEMA_VERSION,",
        "requester_seat: crate::gate::mandate::MANDATE_SEAT.to_string(),",
        "requester_grant_id: self.grant_id.clone(),",
        "matched_binding: self.matched_binding.clone(),",
        "mandate_id: self.mandate.id.clone(),",
    ] {
        assert!(
            record.contains(bound),
            "admitted_request must set `{bound}`"
        );
    }
}

// The dispatch orchestration reaches every admission point through the gate.
// trace:STORY-1889 | ai:claude
#[test]
fn dispatch_orchestration_calls_the_gate_at_each_admission_point() {
    let text =
        std::fs::read_to_string(workspace_root().join("aida-cli-lib/src/dispatch.rs")).unwrap();
    let src = non_test_lines(&text).join("\n");
    for (f, calls) in [
        (
            "submit_request",
            &[
                "check_request_authority(",
                "check_submission_fence(",
                "submission_collision(",
                "caller.admitted_request(",
                "revalidate_request(",
            ][..],
        ),
        (
            "claim_and_dispatch_pass",
            &[
                "acquire_dispatch_lock(",
                "ClaimCaps::from_inventory(",
                "pass_admission(",
                "PassAdmission::Claim(admission)",
                "res.begin_running(",
                "res.publish_worker(",
                "res.fail_running(",
            ][..],
        ),
        (
            "execute_one_request_with_token",
            &[
                "acquire_dispatch_lock(",
                "res.execution_entry(",
                "verify_running_claim(",
                "let admission = run_admission(",
                "res.begin_running(",
                "admit_execution(",
            ][..],
        ),
        (
            "reap_dead_workers_locked",
            &["dead_claims(", "res.fail_running("][..],
        ),
        (
            "finalize_request",
            &["verify_claim_locked(", "res.finish("][..],
        ),
        (
            "record_tampered_refusal",
            &["verify_claim_locked(", "res.finish("][..],
        ),
        ("cancel_request", &["res.cancel("][..]),
        ("run_dispatch_serve", &["check_serve_grant("][..]),
        (
            "run_dispatch_serve_once_sync",
            &["check_serve_grant(", "res.revert_unstarted_claim("][..],
        ),
        (
            "cleanup_dead_reviewer_worktree",
            &["remove_reviewer_worktree_preserving_hold("][..],
        ),
        ("handle_dispatch", &["activate_lock_as_human("][..]),
    ] {
        let body = fn_body(&src, f);
        for call in calls {
            assert!(body.contains(call), "dispatch.rs `{f}` must call `{call}`");
        }
    }
    // Every verb runner takes the gate's `ExecutionAdmission`, which only
    // `admit_execution` builds, so a runner cannot run on a discarded or
    // skipped admission.
    let flow_text =
        std::fs::read_to_string(workspace_root().join("aida-cli-lib/src/dispatch_flow.rs"))
            .unwrap();
    let flow_src = non_test_lines(&flow_text).join("\n");
    for (text, runner) in [
        (&src, "run_review_verb"),
        (&src, "run_take_verb"),
        (&src, "run_impl_verb"),
        (&flow_src, "run_flow_verb"),
        (&flow_src, "run_train_verb"),
    ] {
        let body = fn_body(text, runner);
        let signature = &body[..body.find('{').unwrap()];
        assert!(
            signature.contains("&ExecutionAdmission"),
            "`{runner}` must take the gate's `&ExecutionAdmission`"
        );
    }
    // No record-status write outside the gate's transition methods.
    for write in [".status =", ".started_at =", ".finished_at =", ".summary ="] {
        assert!(
            !src.contains(write) && !flow_src.contains(write),
            "dispatch orchestration writes a record field (`{write}`) outside the gate"
        );
    }
    // A reviewer worktree is removed only by the gate, after its merge-hold
    // marker is preserved.
    for (name, text) in [("dispatch.rs", &src), ("dispatch_flow.rs", &flow_src)] {
        assert!(
            !text.contains("\"worktree\", \"remove\""),
            "{name} removes a worktree itself; call \
             `crate::gate::dispatch_reviewer_hold::remove_reviewer_worktree_preserving_hold`"
        );
        assert!(
            !text.contains("fn preserve_reviewer_worktree_hold"),
            "{name} defines the merge-hold preservation outside the gate"
        );
    }
    let hold = gate_source("dispatch_reviewer_hold.rs");
    let remove = fn_body(&hold, "remove_reviewer_worktree_preserving_hold");
    let preserve = remove
        .find("preserve_reviewer_worktree_hold(main_worktree, rev_wt, pr_scope)?;")
        .expect("the removal preserves the hold and stops on failure");
    let git_remove = remove
        .find("\"worktree\", \"remove\"")
        .expect("the removal runs git worktree remove");
    assert!(
        preserve < git_remove,
        "the merge-hold is preserved before `git worktree remove`"
    );
    assert_eq!(
        hold.matches("\"worktree\", \"remove\"").count(),
        1,
        "gate/dispatch_reviewer_hold.rs removes a worktree only in \
         `remove_reviewer_worktree_preserving_hold`"
    );

    // The reaper only writes what the gate's `dead_claims` released; it never
    // probes a worker or reads the probe's tri-state itself.
    let reap = fn_body(&src, "reap_dead_workers_locked");
    for decision in ["probe_worker_identity(", "Probe::"] {
        assert!(
            !reap.contains(decision),
            "dispatch.rs `reap_dead_workers_locked` decides the reap itself (`{decision}`); \
             call `crate::gate::dispatch_authority::dead_claims` instead"
        );
    }

    // `flow` ships or stops on a verdict through the gate's decision only.
    let text = std::fs::read_to_string(workspace_root().join("aida-cli-lib/src/dispatch_flow.rs"))
        .unwrap();
    let flow = non_test_lines(&text).join("\n");
    let run = fn_body(&flow, "run_flow_verb");
    assert_eq!(
        run.matches("flow_ship_decision(").count(),
        2,
        "run_flow_verb decides the fast path and the post-review ship through \
         `flow_ship_decision`"
    );
    assert!(
        !flow.contains("VerdictKind"),
        "dispatch_flow.rs reads a review verdict kind outside the gate"
    );
}

/// The STORY-1917 / STORY-1889 behavioural suites of the dispatch gate.
const DISPATCH_POLICY_SUITES: &[&str] = &[
    "story_1917_dispatch_request_tests",
    "story_1917_dispatch_exec_tests",
    "story_1917_dispatch_flow_schedule_tests",
    "story_1889_dispatch_authority_tests",
];

/// The dispatch gate's behavioural tests live under `gate/tests/` and are
/// compiled from a protected `gate/dispatch_*.rs` mount, so a mandate-shipped
/// diff can neither drop the mount nor repoint it at a weaker copy.
// trace:STORY-1889 | ai:claude
#[test]
fn dispatch_policy_suites_are_mounted_on_the_floor() {
    let root = workspace_root();
    let gate_dir = root.join("aida-cli-lib/src/gate");
    let mut mounted: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&gate_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("dispatch_") && name.ends_with(".rs")) {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            for target in path_attr_targets(line) {
                let next = lines.get(i + 1).map(|l| l.trim()).unwrap_or("");
                let prev = i.checked_sub(1).map(|j| lines[j].trim()).unwrap_or("");
                let same_line = line.contains("#[cfg(test)]") && line.contains(" mod ");
                if same_line || (prev == "#[cfg(test)]" && next.starts_with("mod ")) {
                    mounted.push(target);
                }
            }
        }
    }
    for suite in DISPATCH_POLICY_SUITES {
        let rel = format!("tests/{suite}.rs");
        assert!(
            gate_dir.join(&rel).is_file(),
            "aida-cli-lib/src/gate/{rel} is missing"
        );
        assert!(
            mounted.contains(&rel),
            "aida-cli-lib/src/gate/{rel} is not mounted by a gate/dispatch_*.rs `#[cfg(test)] #[path]` module"
        );
    }
    // No copy of a dispatch suite outside the floor, and no unprotected file
    // mounts one.
    let mut stray: Vec<String> = Vec::new();
    let mut all = Vec::new();
    for r in SCANNED_ROOTS {
        all_rust_files(&root.join(r), &mut all);
    }
    for f in all {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        if DISPATCH_POLICY_SUITES.contains(&stem.as_str()) && !on_floor(&rel) {
            stray.push(format!("  {rel} (a dispatch suite outside the floor)"));
        }
    }
    for (path, text) in sources() {
        if on_floor(&path) || !mounts_dispatch_suite(&text) {
            continue;
        }
        stray.push(format!("  {path} mounts a dispatch suite via `#[path]`"));
    }
    assert!(
        stray.is_empty(),
        "dispatch gate tests outside the protected floor:\n{}",
        stray.join("\n")
    );
}

/// Items `gate/dispatch_*` may take from `crate::dispatch`: the clap value
/// enums of `dispatch_args.rs`. They are argument shapes the gate maps itself;
/// none of them reads, hashes, locates or writes a record the gate trusts.
const GATE_DISPATCH_IMPORT_ALLOWLIST: &[&str] = &[
    "DispatchRequestVerb",
    "DispatchStatusFilter",
    "DispatchVendor",
];

/// The `crate::dispatch` items `text` names, each as written after the
/// module path (`{a, b}` groups are split; a glob is reported as `*`).
// trace:STORY-1889 | ai:claude
fn dispatch_items_named(text: &str) -> Vec<String> {
    let flat: String = text.split_whitespace().collect();
    let path = regex::Regex::new(
        r"(?:crate|super::super|super)::(?:\{)?dispatch(?:::dispatch_args)?::(\{[^}]*\}|\*|\w+)",
    )
    .unwrap();
    let mut out = Vec::new();
    for c in path.captures_iter(&flat) {
        let body = c[1].trim_start_matches('{').trim_end_matches('}');
        for item in body.split(',').filter(|i| !i.is_empty()) {
            let name = item.split("::").next().unwrap_or(item);
            out.push(name.split("as").next().unwrap_or(name).to_string());
        }
    }
    // `use crate::{dispatch, ...}` / `crate::dispatch as d` hand over the
    // whole module.
    let whole =
        regex::Regex::new(r"(?:crate|super)::(?:\{[^}]*\bdispatch\b[,}]|dispatch(?:as\w+)?;)")
            .unwrap();
    if whole.is_match(&flat) {
        out.push("dispatch".to_string());
    }
    out
}

// trace:STORY-1889 | ai:claude
#[test]
fn dispatch_item_detection() {
    assert_eq!(
        dispatch_items_named("use crate::dispatch::{sha256_hex, DispatchVendor as V};\n"),
        vec!["sha256_hex", "DispatchVendor"]
    );
    assert_eq!(
        dispatch_items_named("let x = crate::dispatch::result_path(a, b);\n"),
        vec!["result_path"]
    );
    assert_eq!(dispatch_items_named("use crate::dispatch::*;\n"), vec!["*"]);
    assert_eq!(
        dispatch_items_named("use crate::dispatch::dispatch_args::DispatchVendor;\n"),
        vec!["DispatchVendor"]
    );
    assert_eq!(
        dispatch_items_named("use crate::dispatch;\n"),
        vec!["dispatch"]
    );
    assert_eq!(
        dispatch_items_named("use crate::{dispatch, gate};\n"),
        vec!["dispatch"]
    );
    assert!(dispatch_items_named("use crate::gate::dispatch_io::sha256_hex;\n").is_empty());
    assert!(dispatch_items_named("use crate::dispatch_flow::x;\n").is_empty());
}

/// `gate/dispatch_*` reads, hashes, locates and pins dispatch records only
/// through primitives on the floor (`gate/dispatch_io.rs`). Importing them
/// from the unprotected `crate::dispatch` would let a mandate-shipped edit
/// there disable brief-tamper detection, claim verification or hold
/// preservation without touching `gate/`.
// trace:STORY-1889 | ai:claude
#[test]
fn gate_dispatch_imports_nothing_authority_bearing_from_dispatch() {
    let gate_dir = workspace_root().join("aida-cli-lib/src/gate");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&gate_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with("dispatch_") && name.ends_with(".rs")) {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        for item in dispatch_items_named(&text) {
            if !GATE_DISPATCH_IMPORT_ALLOWLIST.contains(&item.as_str()) {
                bad.push(format!("  gate/{name}: crate::dispatch::{item}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "gate/dispatch_* takes authority-bearing items from the unprotected \
         `crate::dispatch`:\n{}\nMove the primitive into `gate/dispatch_io.rs` \
         (or another gate module) and have `dispatch.rs` call it from there.",
        bad.join("\n")
    );
    for primitive in [
        "fn sha256_hex",
        "fn read_bounded_regular_file",
        "fn write_private_atomic",
        "fn open_pinned_dispatch_subdir",
        "fn dispatch_root",
        "fn request_path",
        "fn result_path",
        "fn brief_path",
        "fn recovery_path",
    ] {
        let io = std::fs::read_to_string(gate_dir.join("dispatch_io.rs")).unwrap();
        assert!(
            io.contains(primitive),
            "gate/dispatch_io.rs lost `{primitive}`"
        );
    }
}

/// Whether `text` mounts one of the [`DISPATCH_POLICY_SUITES`] through any
/// `#[path]` form [`path_attr_targets`] recognises.
// trace:STORY-1889 | ai:claude
fn mounts_dispatch_suite(text: &str) -> bool {
    path_attr_targets(text)
        .iter()
        .any(|t| DISPATCH_POLICY_SUITES.iter().any(|s| t.contains(s)))
}

/// Every `.rs` file under `dir`, test files included.
fn all_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            all_rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}
