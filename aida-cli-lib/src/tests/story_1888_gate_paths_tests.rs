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
    // STORY-1843 C2: the removal goes through the mail-retention guard,
    // which retires the reviewer's mail before `git worktree remove`.
    let removal = "aida_core::mailbox_lifecycle::guard::remove_worktree_retained(";
    let git_remove = remove
        .find(removal)
        .expect("the removal runs the guarded git worktree remove");
    assert!(
        preserve < git_remove,
        "the merge-hold is preserved before the worktree is removed"
    );
    assert_eq!(
        hold.matches(removal).count(),
        1,
        "gate/dispatch_reviewer_hold.rs removes a worktree only in \
         `remove_reviewer_worktree_preserving_hold`"
    );
    assert!(
        !hold.contains("\"worktree\", \"remove\""),
        "gate/dispatch_reviewer_hold.rs runs no raw `git worktree remove`"
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

/// What a source file reaches outside itself, scanned as module code
/// `level` steps below the crate root (`gate/dispatch_*.rs` sits at 2).
/// Use trees are walked structurally, so groups, nesting, renames and
/// `self` all resolve to the path they import; paths in code, attribute
/// arguments and macro tokens (string literals included) are seen too.
// trace:STORY-1889 | trace:TASK-b6b1278890774ef993d2ebc327862470-009 | trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[derive(Default)]
struct Reach {
    /// Every path that resolves to the crate root, as the segments after it.
    rooted: Vec<Named>,
    /// Paths spelled relative to the module itself (no `crate`, no
    /// `super`), which [`Reach::resolve`] roots when a private `use` hands
    /// the module the crate root (`use crate::*;`, `use crate as c;`).
    unrooted: Vec<Named>,
    /// Root handovers (`use crate::*;`, `use crate as c;`).
    handovers: Vec<Named>,
    /// Paths that climb with `super` to the module one below the crate
    /// root (`gate` for a gate file), as the segments after it.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
    top: Vec<Named>,
    /// Modules the source declares, which shadow a root glob's names.
    local_mods: Vec<String>,
    /// Every `#[path]` target (and `path = "..."` in attribute or macro
    /// tokens), escapes decoded.
    path_targets: Vec<String>,
    /// How many times the `include` macro is named.
    includes: usize,
    level: usize,
    in_use: bool,
    rename: Option<String>,
}

/// A path a source names, as its segments (after the crate root once
/// rooted).
#[derive(Clone)]
struct Named {
    segs: Vec<String>,
    glob: bool,
    /// Hands over the module it names whole (renamed or globbed rather
    /// than named).
    whole: bool,
    /// The name a renaming `use` binds it to.
    rename: Option<String>,
}

impl Reach {
    /// Records `segs` when it resolves to the crate root: it starts at
    /// `crate`, climbs out of `level` with `super`, or is spelled at the
    /// root itself (where a bare `dispatch::x` names the module). Other
    /// relative paths are kept for [`Reach::resolve`].
    fn note(&mut self, segs: &[String], glob: bool, whole: bool) {
        let mut crate_root = false;
        let mut supers = 0;
        let mut i = 0;
        for s in segs {
            match s.as_str() {
                "crate" if i == 0 => crate_root = true,
                "super" => supers += 1,
                "self" => {}
                _ => break,
            }
            i += 1;
        }
        let tail = &segs[i..];
        // A lone unrooted name outside a `use` is a local, not a path.
        if i == 0 && tail.len() == 1 && !glob && !whole && !self.in_use {
            return;
        }
        let named = Named {
            segs: tail.to_vec(),
            glob,
            whole,
            rename: self.rename.clone(),
        };
        if crate_root || supers >= self.level {
            self.root(named);
        } else if supers == 0 && !tail.is_empty() {
            self.unrooted.push(named);
        } else if supers + 1 == self.level {
            self.top.push(named);
        }
    }

    /// Records a path that resolves to the crate root.
    fn root(&mut self, named: Named) {
        if named.segs.is_empty() {
            self.handovers.push(named.clone());
        }
        self.rooted.push(named);
    }

    /// Roots the relative paths a private root handover makes reach the
    /// crate root: anything after `use crate::*;` that no local module
    /// shadows, and anything through an alias (`use crate as c;` then
    /// `c::dispatch::x`). Repeats until no new handover appears.
    fn resolve(&mut self) {
        loop {
            let globbed = self.handovers.iter().any(|h| h.glob);
            let aliases: Vec<String> = self
                .handovers
                .iter()
                .filter_map(|h| h.rename.clone())
                .collect();
            let mut rooted = Vec::new();
            let mut kept = Vec::new();
            for n in std::mem::take(&mut self.unrooted) {
                let first = &n.segs[0];
                if aliases.contains(first) {
                    rooted.push(Named {
                        segs: n.segs[1..].to_vec(),
                        ..n
                    });
                } else if globbed && !self.local_mods.contains(first) {
                    rooted.push(n);
                } else {
                    kept.push(n);
                }
            }
            self.unrooted = kept;
            if rooted.is_empty() {
                return;
            }
            rooted.into_iter().for_each(|n| self.root(n));
        }
    }

    fn tree(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.tree(prefix, &p.tree);
                prefix.pop();
            }
            syn::UseTree::Group(g) => g.items.iter().for_each(|t| self.tree(prefix, t)),
            syn::UseTree::Glob(_) => self.note(prefix, true, true),
            syn::UseTree::Name(n) => self.leaf(prefix, &n.ident, None),
            syn::UseTree::Rename(r) => self.leaf(prefix, &r.ident, Some(&r.rename)),
        }
    }

    /// A named (or renamed) leaf; `self` names the prefix itself.
    fn leaf(&mut self, prefix: &mut Vec<String>, ident: &syn::Ident, rename: Option<&syn::Ident>) {
        self.ident(ident);
        let name = ident.to_string();
        let pushed = name != "self" || prefix.is_empty();
        if pushed {
            prefix.push(name);
        }
        self.rename = rename.map(|r| r.to_string());
        self.note(prefix, false, rename.is_some());
        self.rename = None;
        if pushed {
            prefix.pop();
        }
    }

    fn ident(&mut self, ident: &impl std::fmt::Display) {
        if ident.to_string() == "include" {
            self.includes += 1;
        }
    }

    /// Paths spelled inside unparsed tokens (macro bodies, attribute
    /// arguments), including string literals that parse as a path, and
    /// `path = "..."` pairs.
    fn tokens(&mut self, ts: proc_macro2::TokenStream) {
        use proc_macro2::{Spacing, TokenTree};
        let tts: Vec<TokenTree> = ts.into_iter().collect();
        for w in tts.windows(3) {
            if let [TokenTree::Ident(i), TokenTree::Punct(p), TokenTree::Literal(l)] = w {
                if i == "path" && p.as_char() == '=' {
                    if let Ok(s) = syn::parse_str::<syn::LitStr>(&l.to_string()) {
                        self.path_targets.push(s.value());
                    }
                }
            }
        }
        let mut cur: Vec<String> = Vec::new();
        let mut joined = false;
        let mut it = tts.into_iter().peekable();
        while let Some(tt) = it.next() {
            match tt {
                TokenTree::Ident(i) => {
                    self.ident(&i);
                    if !joined {
                        self.flush(&mut cur);
                    }
                    cur.push(i.to_string());
                    joined = false;
                    continue;
                }
                TokenTree::Punct(p)
                    if p.as_char() == ':'
                        && p.spacing() == Spacing::Joint
                        && matches!(it.peek(), Some(TokenTree::Punct(q)) if q.as_char() == ':') =>
                {
                    it.next();
                    joined = true;
                    continue;
                }
                TokenTree::Group(g) => {
                    self.flush(&mut cur);
                    self.tokens(g.stream());
                }
                TokenTree::Literal(l) => {
                    self.flush(&mut cur);
                    let s = syn::parse_str::<syn::LitStr>(&l.to_string()).map(|s| s.value());
                    if let Ok(path) = s.and_then(|s| syn::parse_str::<syn::Path>(&s)) {
                        self.path(&path);
                    }
                }
                TokenTree::Punct(_) => {}
            }
            self.flush(&mut cur);
            joined = false;
        }
        self.flush(&mut cur);
    }

    fn flush(&mut self, cur: &mut Vec<String>) {
        if cur.len() > 1 {
            self.note(cur, false, false);
        }
        cur.clear();
    }

    /// A path in code. A leading `::` names an extern crate
    /// (`::aida_core::x` is `aida_core::x`), so it is noted the same way.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-053 | ai:claude
    fn path(&mut self, p: &syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        self.note(&segs, false, false);
    }
}

impl<'ast> syn::visit::Visit<'ast> for Reach {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.in_use = true;
        self.tree(&mut Vec::new(), &u.tree);
        self.in_use = false;
    }
    fn visit_item_extern_crate(&mut self, e: &'ast syn::ItemExternCrate) {
        // `extern crate self as me;` renames the crate root.
        if e.ident == "self" {
            self.rooted.push(Named {
                segs: Vec::new(),
                glob: false,
                whole: true,
                rename: e.rename.as_ref().map(|(_, r)| r.to_string()),
            });
        } else if let Some((_, rename)) = &e.rename {
            // `extern crate aida_core as x;` hands the crate over whole
            // under `x`, like `use aida_core as x;`.
            // trace:TASK-b6b1278890774ef993d2ebc327862470-053 | ai:claude
            self.unrooted.push(Named {
                segs: vec![e.ident.to_string()],
                glob: false,
                whole: true,
                rename: Some(rename.to_string()),
            });
        }
    }
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        self.local_mods.push(m.ident.to_string());
        // An inline `mod m { .. }` is one level further from the root.
        let inline = m.content.is_some();
        self.level += usize::from(inline);
        syn::visit::visit_item_mod(self, m);
        self.level -= usize::from(inline);
    }
    fn visit_ident(&mut self, i: &'ast proc_macro2::Ident) {
        self.ident(i);
    }
    fn visit_path(&mut self, p: &'ast syn::Path) {
        self.path(p);
        syn::visit::visit_path(self, p);
    }
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        self.tokens(m.tokens.clone());
        syn::visit::visit_macro(self, m);
    }
    fn visit_meta_list(&mut self, m: &'ast syn::MetaList) {
        self.tokens(m.tokens.clone());
        syn::visit::visit_meta_list(self, m);
    }
    fn visit_meta_name_value(&mut self, m: &'ast syn::MetaNameValue) {
        if m.path.is_ident("path") {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &m.value
            {
                self.path_targets.push(s.value());
            }
        }
        syn::visit::visit_meta_name_value(self, m);
    }
}

fn reach(text: &str, level: usize) -> Reach {
    scan_reach(
        text,
        Reach {
            level,
            ..Reach::default()
        },
    )
}

fn scan_reach(text: &str, mut reach: Reach) -> Reach {
    use syn::visit::Visit;
    let file = syn::parse_file(text).unwrap_or_else(|e| panic!("source must parse: {e}"));
    reach.visit_file(&file);
    reach.resolve();
    reach
}

/// One parse of a source. The crate-root files run to ~0.5MB, so the
/// ratchet parses each once and hands the tree to every check.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn parse_source(text: &str) -> syn::File {
    syn::parse_file(text).unwrap_or_else(|e| panic!("source must parse: {e}"))
}

/// The `crate::dispatch` items a `gate/dispatch_*` source names, each as
/// written after the module path. A glob is reported as `*`, and anything
/// that hands over the module itself (or the crate root it hangs off) as
/// `dispatch`.
// trace:STORY-1889 | trace:TASK-b6b1278890774ef993d2ebc327862470-009 | ai:claude
fn dispatch_items_named(text: &str) -> Vec<String> {
    dispatch_items_at(text, 2)
}

/// [`dispatch_items_named`] for code `level` steps below the crate root.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn dispatch_items_at(text: &str, level: usize) -> Vec<String> {
    dispatch_items(reach(text, level))
}

fn dispatch_items(reach: Reach) -> Vec<String> {
    reach
        .rooted
        .into_iter()
        .filter_map(
            |Named {
                 segs, glob, whole, ..
             }| {
                // `use crate as c`, `use crate::*`: the root hands over `dispatch` too.
                let Some((first, tail)) = segs.split_first() else {
                    return (glob || whole).then(|| "dispatch".to_string());
                };
                if first != "dispatch" {
                    return None;
                }
                let tail = match tail.split_first() {
                    Some((m, t)) if m == "dispatch_args" && !t.is_empty() => t,
                    _ => tail,
                };
                Some(match tail.first() {
                    Some(item) => item.clone(),
                    None if glob => "*".to_string(),
                    None => "dispatch".to_string(),
                })
            },
        )
        .collect()
}

/// The crate-root names a `gate/dispatch_*` source reaches: `*` for a glob
/// of the root and `crate` for the root handed over under another name.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn crate_root_names(text: &str) -> Vec<String> {
    reach(text, 2)
        .rooted
        .into_iter()
        .filter_map(
            |Named {
                 segs, glob, whole, ..
             }| match segs.first() {
                Some(name) => Some(name.clone()),
                None if glob => Some("*".to_string()),
                None if whole => Some("crate".to_string()),
                None => None,
            },
        )
        .collect()
}

/// An out-of-line module a source mounts (`mod x;`, with or without
/// `#[path]`): the files it may load from, relative to `aida-cli-lib/src`
/// and each with whether rustc reads it mod.rs-style; how many levels below
/// the source's top level it sits; and whether `#[cfg(test)]` (on it or an
/// enclosing inline module) keeps it out of non-test builds.
struct Mount {
    name: String,
    candidates: Vec<(String, bool)>,
    depth: usize,
    test: bool,
}

/// The modules `file` (read mod.rs-style when `mod_rs`) mounts. Paths
/// follow rustc: a top-level `#[path]` is relative to the file's
/// directory; a plain `mod x;` (and any `#[path]` inside an inline module)
/// is relative to the directory the file owns (`forge.rs` owns `forge/`)
/// plus the inline modules around it.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn mounts(file: &str, mod_rs: bool, text: &str) -> Vec<Mount> {
    let parsed = syn::parse_file(text).unwrap_or_else(|e| panic!("{file} must parse: {e}"));
    mounts_in(file, mod_rs, &parsed)
}

fn mounts_in(file: &str, mod_rs: bool, parsed: &syn::File) -> Vec<Mount> {
    fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(l) if l.tokens.to_string() == "test")
        })
    }
    fn walk(
        items: &[syn::Item],
        dir: &str,
        owned: &str,
        inline: &mut Vec<String>,
        test: bool,
        out: &mut Vec<Mount>,
    ) {
        let join = |base: &str, rest: &str| {
            if base.is_empty() {
                rest.to_string()
            } else {
                format!("{base}/{rest}")
            }
        };
        for item in items {
            let syn::Item::Mod(m) = item else { continue };
            let test = test || is_cfg_test(&m.attrs);
            let name = m.ident.to_string();
            if let Some((_, content)) = &m.content {
                inline.push(name);
                walk(content, dir, owned, inline, test, out);
                inline.pop();
                continue;
            }
            let nested = inline.iter().fold(owned.to_string(), |b, s| join(&b, s));
            let mut targets = Reach::default();
            for attr in &m.attrs {
                use syn::visit::Visit;
                targets.visit_attribute(attr);
            }
            let base = if inline.is_empty() { dir } else { &nested };
            let mut candidates: Vec<(String, bool)> = targets
                .path_targets
                .iter()
                .map(|t| (join(base, t), true))
                .collect();
            if !m.attrs.iter().any(|a| a.path().is_ident("path")) {
                candidates.push((join(&nested, &format!("{name}.rs")), false));
                candidates.push((join(&nested, &format!("{name}/mod.rs")), true));
            }
            out.push(Mount {
                name,
                candidates,
                depth: inline.len() + 1,
                test,
            });
        }
    }
    let dir = file.rsplit_once('/').map_or("", |(d, _)| d);
    let owned = if mod_rs {
        dir.to_string()
    } else {
        file.trim_end_matches(".rs").to_string()
    };
    let mut out = Vec::new();
    if !is_cfg_test(&parsed.attrs) {
        walk(&parsed.items, dir, &owned, &mut Vec::new(), false, &mut out);
    }
    out
}

/// The files each top-level out-of-line `mod name;` in a crate-root source
/// may load from. An allowlisted module is scanned as `name.rs`, so any
/// `#[path]` here (`#[path = "forge2.rs"] mod forge;`) would let the gate's
/// `crate::name` resolve to an unscanned file. A top-level inline
/// `mod name { .. }` is reported as `["<inline>"]`, so a cfg-gated inline
/// twin of a plain mount (`#[cfg(not(test))] mod forge { .. }`) is seen.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn root_module_mounts(text: &str, name: &str) -> Vec<Vec<String>> {
    root_module_mounts_in(&parse_source(text), name)
        .into_iter()
        .map(|(candidates, _)| candidates)
        .collect()
}

/// [`root_module_mounts`] over a parsed source, each mount with every
/// outer attribute on it but doc comments. Any such attribute can change
/// where, or whether, rustc loads the module (`cfg`, `cfg_attr(not(test),
/// cfg(any()))`, `path`, `macro_use`, ...), so an allowlisted mount carries
/// none: there is no per-attribute reading to get wrong.
// trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
fn root_module_mounts_in(file: &syn::File, name: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let attrs = |attrs: &[syn::Attribute]| -> Vec<String> {
        attrs
            .iter()
            .filter(|a| !a.path().is_ident("doc"))
            .map(attr_text)
            .collect()
    };
    let mods = |out_of_line: bool| {
        file.items.iter().filter_map(move |item| match item {
            syn::Item::Mod(m) if m.content.is_none() == out_of_line && m.ident == name => Some(m),
            _ => None,
        })
    };
    mounts_in("lib.rs", true, file)
        .into_iter()
        .filter(|m| m.depth == 1 && m.name == name)
        .zip(mods(true))
        .map(|(m, item)| {
            let candidates = m.candidates.into_iter().map(|(c, _)| c).collect();
            (candidates, attrs(&item.attrs))
        })
        .chain(mods(false).map(|item| (vec!["<inline>".to_string()], attrs(&item.attrs))))
        .collect()
}

/// An attribute as written, enough to name it in a refusal.
// trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
fn attr_text(a: &syn::Attribute) -> String {
    let path = a
        .path()
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    match &a.meta {
        syn::Meta::Path(_) => format!("#[{path}]"),
        syn::Meta::List(l) => format!("#[{path}({})]", l.tokens),
        syn::Meta::NameValue(_) => format!("#[{path} = ..]"),
    }
}

/// Why the crate-root mounts of `name` are not exactly one bare
/// `mod name;` loading `name.rs` or `name/mod.rs`, or `None` when they are.
/// Fails closed: a missing mount, a second mount, an inline body, a
/// `#[path]` target or any attribute but a doc comment is refused.
// trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
fn mount_not_bare(found: &[(Vec<String>, Vec<String>)], name: &str) -> Option<String> {
    let plain = [format!("{name}.rs"), format!("{name}/mod.rs")];
    match found {
        [] => Some(format!("no top-level `mod {name};`")),
        [(_, attrs)] if !attrs.is_empty() => Some(format!(
            "`mod {name};` carries {}; only doc comments are allowed",
            attrs.join(" ")
        )),
        [(candidates, _)] if *candidates != plain => {
            Some(format!("`mod {name}` loads {candidates:?}, not {plain:?}"))
        }
        [_] => None,
        _ => Some(format!("{} mounts of `{name}`: {found:?}", found.len())),
    }
}

/// The names a crate-root source binds besides its modules: each `use`
/// leaf or rename (a `self` leaf binds its parent) and each `extern crate`
/// (under its alias when renamed). A root binding of an allowlisted name
/// could stand in for its mount if a future cfg gap dropped the `mod`.
// trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
fn root_bindings(file: &syn::File) -> Vec<String> {
    fn tree(prefix: Option<&syn::Ident>, t: &syn::UseTree, out: &mut Vec<String>) {
        match t {
            syn::UseTree::Path(p) => tree(Some(&p.ident), &p.tree, out),
            syn::UseTree::Name(n) if n.ident == "self" => {
                out.extend(prefix.map(|p| p.to_string()));
            }
            syn::UseTree::Name(n) => out.push(n.ident.to_string()),
            syn::UseTree::Rename(r) => out.push(r.rename.to_string()),
            syn::UseTree::Group(g) => g.items.iter().for_each(|i| tree(prefix, i, out)),
            syn::UseTree::Glob(_) => {}
        }
    }
    let mut out = Vec::new();
    for item in &file.items {
        match item {
            syn::Item::Use(u) => tree(None, &u.tree, &mut out),
            syn::Item::ExternCrate(e) => out.push(
                e.rename
                    .as_ref()
                    .map_or(&e.ident, |(_, alias)| alias)
                    .to_string(),
            ),
            _ => {}
        }
    }
    out.into_iter()
        .map(|n| n.trim_start_matches("r#").to_string())
        .collect()
}

/// What a crate-root source puts in the textual macro scope of every
/// module declared after it: each `#[macro_use]` module (bare or under
/// `cfg_attr`, on the item or as an inner `#![macro_use]` in any file
/// `load` returns for a top-level `mod x;`) as `mod x`, and each top-level
/// `macro_rules!` by name. A `#[macro_use] extern crate x;` is reported as
/// `extern crate x`: it puts `x`'s exported macros in the `macro_use`
/// prelude, ahead of std's. A `#[macro_use] mod util;` or root
/// `macro_rules! format` shadows the prelude macro `gate/dispatch_*` calls
/// bare, so its expansion runs unscanned code inside the gate.
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
fn root_textual_macros(
    text: &str,
    load: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<String>) {
    root_textual_macros_in(&parse_source(text), load)
}

fn root_textual_macros_in(
    file: &syn::File,
    load: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<String>) {
    let macro_use = regex::Regex::new(r"\bmacro_use\b").unwrap();
    let widens = |attrs: &[syn::Attribute]| {
        attrs.iter().any(|a| {
            a.path().is_ident("macro_use")
                || matches!(&a.meta, syn::Meta::List(l)
                    if a.path().is_ident("cfg_attr")
                        && macro_use.is_match(&l.tokens.to_string()))
        })
    };
    let (mut mods, mut rules) = (Vec::new(), Vec::new());
    for item in &file.items {
        match item {
            syn::Item::Mod(m) if widens(&m.attrs) => mods.push(format!("mod {}", m.ident)),
            // trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
            syn::Item::ExternCrate(e) if widens(&e.attrs) => {
                mods.push(format!("extern crate {}", e.ident));
            }
            syn::Item::Macro(m) if m.mac.path.is_ident("macro_rules") => {
                if let Some(ident) = &m.ident {
                    rules.push(ident.to_string().trim_start_matches("r#").to_string());
                }
            }
            _ => {}
        }
    }
    // An out-of-line module's own file can widen the scope from inside.
    for mount in mounts_in("lib.rs", true, file) {
        let named = format!("mod {}", mount.name);
        if mount.depth != 1 || mods.contains(&named) {
            continue;
        }
        let inner = mount.candidates.iter().any(|(candidate, _)| {
            // Most modules never say `macro_use`; skip parsing those.
            load(candidate).is_some_and(|body| {
                if !macro_use.is_match(&body) {
                    return false;
                }
                let parsed = syn::parse_file(&body)
                    .unwrap_or_else(|e| panic!("{candidate} must parse: {e}"));
                widens(&parsed.attrs)
            })
        });
        if inner {
            mods.push(named);
        }
    }
    (mods, rules)
}

/// The root-level item macro invocations a crate-root source makes beyond
/// `include!` of one of `parts` and `thread_local!`. syn sees only the
/// unexpanded call, so `crate::m!();` expanding to `#[path = "gate2.rs"]
/// mod gate;` would remount an allowlisted module past
/// [`root_module_mounts`]; its tokens need not name it, and its definition
/// may sit in any module, so every other invocation is refused.
/// `macro_rules!` definitions expand nothing themselves.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn root_item_macros(file: &syn::File, parts: &[String]) -> Vec<String> {
    file.items
        .iter()
        .filter_map(|item| {
            let syn::Item::Macro(m) = item else {
                return None;
            };
            let path = &m.mac.path;
            if path.is_ident("macro_rules") {
                return None;
            }
            let allowed = if path.is_ident("include") {
                syn::parse2::<syn::LitStr>(m.mac.tokens.clone())
                    .is_ok_and(|lit| parts.contains(&lit.value()))
            } else {
                path.is_ident("thread_local")
            };
            let shown = path.segments.iter().map(|s| s.ident.to_string());
            let lead = if path.leading_colon.is_some() {
                "::"
            } else {
                ""
            };
            (!allowed).then(|| format!("{lead}{}!", shown.collect::<Vec<_>>().join("::")))
        })
        .collect()
}

/// Whether `text` defines or renames a macro to a name [`root_item_macros`]
/// lets the crate root invoke bare: a `#[macro_export] macro_rules!
/// include` (or `pub(crate) use m as include;` reached by a root glob)
/// would shadow the builtin and run its own expansion at the root. Read
/// from the source's raw tokens, macro bodies and use trees included, as
/// `macro_rules ! name` or `as name`, so comments between tokens hide
/// nothing (and a cast `x as include` fails closed too). Every syn item
/// that defines or renames either name lexes to one of those two shapes,
/// so tokens alone suffice and the ~33MB crate is lexed, not parsed. An
/// unlexable source fails closed.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn shadows_root_item_macro(text: &str) -> bool {
    use syn::ext::IdentExt;
    fn shadowed(ident: &proc_macro2::Ident) -> bool {
        matches!(
            ident.unraw().to_string().as_str(),
            "include" | "thread_local"
        )
    }
    fn in_tokens(tokens: proc_macro2::TokenStream) -> bool {
        use proc_macro2::TokenTree;
        let tokens: Vec<TokenTree> = tokens.into_iter().collect();
        tokens.iter().enumerate().any(|(i, t)| match t {
            TokenTree::Group(g) => in_tokens(g.stream()),
            TokenTree::Ident(id) if shadowed(id) => match (i.checked_sub(1), i.checked_sub(2)) {
                (Some(p), _) if matches!(&tokens[p], TokenTree::Ident(a) if a == "as") => true,
                (Some(p), Some(q)) => {
                    matches!(&tokens[p], TokenTree::Punct(b) if b.as_char() == '!')
                        && matches!(&tokens[q], TokenTree::Ident(m) if m == "macro_rules")
                }
                _ => false,
            },
            _ => false,
        })
    }
    if !may_shadow_root_item_macro(text) {
        return false;
    }
    text.parse::<proc_macro2::TokenStream>()
        .map_or(true, in_tokens)
}

/// Whether `text` could name `include` or `thread_local` at all: any
/// definition or rename spells the identifier (raw `r#include` too) in the
/// raw bytes, and no comment can split an identifier, so a source where
/// neither occurs between ASCII non-identifier bytes is skipped unlexed.
/// A non-ASCII neighbour counts as a boundary, which only lets more files
/// through.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn may_shadow_root_item_macro(text: &str) -> bool {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    ["include", "thread_local"].iter().any(|name| {
        text.match_indices(name).any(|(at, _)| {
            let end = at + name.len();
            (at == 0 || !ident(bytes[at - 1])) && bytes.get(end).map_or(true, |&b| !ident(b))
        })
    })
}

/// The macros a source invokes by a bare name (`format!`, not
/// `std::format!`), which textual scope resolves before the prelude.
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
fn bare_macro_names(text: &str) -> std::collections::BTreeSet<String> {
    regex::Regex::new(r"(?:^|[^:\w])(?:r#)?([A-Za-z_]\w*)\s*!\s*[(\[{]")
        .unwrap()
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect()
}

/// The ways a `gate/dispatch_*` source compiles code from outside `gate/`
/// into itself: any `include!` (however it is spelled or renamed), and any
/// `#[path]` target that is absolute, climbs with `..` or carries a
/// separator or escape that could hide either.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn source_inclusions(text: &str) -> Vec<String> {
    let reach = reach(text, 2);
    let mut found = vec!["include!".to_string(); reach.includes.min(1)];
    let escapes = |t: &str| {
        t.starts_with('/')
            || t.contains('\\')
            || t.contains(':')
            || t.split('/').any(|seg| seg == "..")
    };
    // The raw spellings catch what an escape decodes to; the decoded ones
    // catch attributes the regex cannot see.
    for target in path_attr_targets(text)
        .into_iter()
        .chain(reach.path_targets)
    {
        if escapes(&target) {
            found.push(format!("#[path = {target:?}]"));
        }
    }
    found
}

// trace:STORY-1889 | trace:TASK-b6b1278890774ef993d2ebc327862470-009 | ai:claude
#[test]
fn dispatch_item_detection() {
    assert_eq!(
        dispatch_items_named("use crate::dispatch::{sha256_hex, DispatchVendor as V};\n"),
        vec!["sha256_hex", "DispatchVendor"]
    );
    assert_eq!(
        dispatch_items_named("fn f() { let x = crate::dispatch::result_path(a, b); }\n"),
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
    assert!(dispatch_items_named("use super::*;\nuse self::x::y;\n").is_empty());
    assert!(dispatch_items_named("/// `crate::dispatch::sha256_hex`\nfn f() {}\n").is_empty());
}

// Each evasive spelling of an import from `crate::dispatch` is still seen.
// trace:TASK-b6b1278890774ef993d2ebc327862470-009 | ai:claude
#[test]
fn dispatch_item_detection_sees_evasive_forms() {
    for (src, want) in [
        // Aliases.
        ("use crate::{dispatch as d};", "dispatch"),
        ("use crate::dispatch as d;", "dispatch"),
        ("use crate::dispatch::sha256_hex as h;", "sha256_hex"),
        ("use crate as c;", "dispatch"),
        ("use crate::{self as c};", "dispatch"),
        ("use super::super as c;", "dispatch"),
        ("extern crate self as me;", "dispatch"),
        // Nested groups.
        ("use crate::{gate, dispatch::{sha256_hex}};", "sha256_hex"),
        (
            "use crate::{gate::{x}, {dispatch::result_path}};",
            "result_path",
        ),
        (
            "use crate::{dispatch::{dispatch_args::{DispatchVendor}}};",
            "DispatchVendor",
        ),
        (
            "use super::super::{dispatch::{brief_path as b}};",
            "brief_path",
        ),
        ("use crate::{dispatch::*};", "*"),
        ("use crate::*;", "dispatch"),
        ("use super::super::*;", "dispatch"),
        // Self imports.
        ("use crate::dispatch::{self};", "dispatch"),
        ("use crate::dispatch::{self as d};", "dispatch"),
        ("use crate::{dispatch::{self, sha256_hex}};", "dispatch"),
        (
            "use self::super::super::dispatch::request_path;",
            "request_path",
        ),
        // Whitespace, comments and line breaks inside the tree.
        (
            "use crate::{\n    dispatch /* x */ as\n    d,\n};",
            "dispatch",
        ),
        // Non-`use` spellings.
        (
            "fn f() { crate::dispatch::dispatch_root(); }",
            "dispatch_root",
        ),
        ("fn f() -> crate::dispatch::Rec { todo!() }", "Rec"),
        (
            "fn f() { let _ = format!(\"{}\", crate::dispatch::sha256_hex(b)); }",
            "sha256_hex",
        ),
        ("fn f() { m!(super::super::dispatch as d); }", "dispatch"),
        (
            "#[serde(with = \"crate::dispatch::codec\")]\nstruct S;",
            "codec",
        ),
        ("fn f() { use crate::{dispatch as d}; }", "dispatch"),
        (
            "mod m { use crate::dispatch::{recovery_path}; }",
            "recovery_path",
        ),
    ] {
        let got = dispatch_items_named(src);
        assert!(
            got.iter().any(|g| g == want),
            "`{src}` must report `{want}`, got {got:?}"
        );
    }
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

/// Crate-root names a `gate/dispatch_*` source may reach: `gate` itself and
/// `dispatch`, whose items [`GATE_DISPATCH_IMPORT_ALLOWLIST`] governs.
/// Everything else outside `gate/` goes through the facade
/// [`DISPATCH_DEPS`] (`crate::canonical_role_name` in `kernel.rs` could call
/// any root module, and that module `crate::dispatch`).
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
const GATE_DISPATCH_ROOT_ALLOWLIST: &[&str] = &["gate", "dispatch"];

/// The gate-owned facade: the one `gate/dispatch_*` source that names
/// anything outside `gate/`.
const DISPATCH_DEPS: &str = "dispatch_deps.rs";

/// The paths in the facade's own `FACADE_EXTERNAL_LEAVES`.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
fn facade_leaves() -> Vec<&'static str> {
    crate::gate::dispatch_deps::FACADE_EXTERNAL_LEAVES
        .iter()
        .map(|(path, _)| *path)
        .collect()
}

/// Every path outside `gate/` that `text` (a gate file) names, spelled
/// from `crate::` (and, with `sibling`, from `aida_core::`). A path spelled
/// through an imported name (`use crate::events;` then `events::emit`) is
/// reported in full as well as the import. A glob or a handover of a whole
/// crate root is reported as `::*` or the bare root: no leaf can list it.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
fn outside_paths(text: &str, sibling: bool) -> Vec<String> {
    type Found = std::collections::BTreeSet<String>;
    fn take(path: String, n: &Named, found: &mut Found, bound: &mut Vec<(String, String)>) {
        if n.glob {
            found.insert(format!("{path}::*"));
            return;
        }
        if let Some(name) = n.rename.clone().or_else(|| n.segs.last().cloned()) {
            bound.push((name, path.clone()));
        }
        found.insert(path);
    }
    let reach = reach(text, 2);
    let mut found = Found::new();
    let mut bound = Vec::new();
    for n in &reach.rooted {
        match n.segs.first() {
            Some(first) if first == "gate" => {}
            Some(_) => take(
                format!("crate::{}", n.segs.join("::")),
                n,
                &mut found,
                &mut bound,
            ),
            // `pub(crate)` names the root and hands nothing over.
            None if !n.glob && !n.whole => {}
            None => {
                found.insert(if n.glob { "crate::*" } else { "crate" }.to_string());
            }
        }
    }
    for n in &reach.unrooted {
        if sibling && n.segs[0] == "aida_core" {
            take(n.segs.join("::"), n, &mut found, &mut bound);
        }
    }
    // Follow imported names until nothing new appears.
    let mut seen = 0;
    while seen < bound.len() {
        let (name, path) = bound[seen].clone();
        seen += 1;
        for n in &reach.unrooted {
            if n.segs[0] == name && n.segs.len() > 1 {
                let full = format!("{path}::{}", n.segs[1..].join("::"));
                if !found.contains(&full) {
                    take(full, n, &mut found, &mut bound);
                }
            }
        }
    }
    found.into_iter().collect()
}

/// Where `gate/{name}` reaches outside `gate/` other than through the
/// facade. Every source but [`DISPATCH_DEPS`] stays inside
/// [`GATE_DISPATCH_ROOT_ALLOWLIST`]; every outside path the facade names,
/// crate or `aida_core`, is in `leaves` by its full path.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
fn gate_dispatch_external_reach(name: &str, text: &str, leaves: &[&str]) -> Vec<String> {
    if name != DISPATCH_DEPS {
        return crate_root_names(text)
            .into_iter()
            .filter(|root| !GATE_DISPATCH_ROOT_ALLOWLIST.contains(&root.as_str()))
            .map(|root| format!("crate::{root}"))
            .collect();
    }
    outside_paths(text, true)
        .into_iter()
        .filter(|p| !leaves.contains(&p.as_str()))
        .collect()
}

/// The gate modules `text` (a `gate/dispatch_*` source) names, one hop:
/// `crate::gate::m`, `super::m`. `Err` carries a form that hands over
/// `gate` itself (a glob, the bare module), which cannot be followed.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
fn gate_modules_named(text: &str) -> Result<Vec<String>, String> {
    let reach = reach(text, 2);
    // `use crate::gate;` then `gate::m::x` names `m` through the import.
    let imported = reach
        .unrooted
        .into_iter()
        .filter(|n| n.segs[0] == "gate" && n.segs.len() > 1);
    let gate_rel = reach
        .rooted
        .into_iter()
        .filter(|n| n.segs.first().is_some_and(|s| s == "gate"))
        .chain(imported)
        .map(|n| Named {
            segs: n.segs[1..].to_vec(),
            ..n
        })
        .chain(reach.top);
    let mut out = std::collections::BTreeSet::new();
    for n in gate_rel {
        match n.segs.first() {
            Some(m) => {
                out.insert(m.clone());
            }
            // `pub(in crate::gate)` and a plain `use crate::gate;` hand
            // nothing over; a glob or a rename does.
            None if !n.glob && n.rename.is_none() => {}
            None => {
                return Err(format!(
                    "crate::gate{}",
                    if n.glob { "::*" } else { " as _" }
                ))
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// The outside reach of each non-`dispatch_*` gate module the `sources`
/// name, one hop: each outside path must be in `leaves` (calls through the
/// facade are gate paths and pass). `module` reads `gate/<m>.rs`. Returns
/// the refusals and the leaves the hop used.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
fn gate_dispatch_one_hop(
    sources: &[(String, String)],
    module: impl Fn(&str) -> String,
    leaves: &[&str],
) -> (Vec<String>, Vec<String>) {
    let mut bad = Vec::new();
    let mut modules = std::collections::BTreeSet::new();
    for (name, text) in sources {
        match gate_modules_named(text) {
            Ok(ms) => modules.extend(ms.into_iter().filter(|m| !m.starts_with("dispatch_"))),
            Err(how) => bad.push(format!("  gate/{name}: {how} cannot be followed")),
        }
    }
    let mut used = Vec::new();
    for m in modules {
        for path in outside_paths(&module(&m), false) {
            if leaves.contains(&path.as_str()) {
                used.push(path);
            } else {
                bad.push(format!(
                    "  gate/{m}.rs (named from gate/dispatch_*): {path}"
                ));
            }
        }
    }
    (bad, used)
}

/// The source of gate module `m`: `gate/<m>.rs`, else `gate/<m>/mod.rs`,
/// else `gate/mod.rs` (an item `crate::gate::x` defined there).
fn gate_module_source(m: &str) -> String {
    let gate_dir = workspace_root().join("aida-cli-lib/src/gate");
    [
        gate_dir.join(format!("{m}.rs")),
        gate_dir.join(m).join("mod.rs"),
        gate_dir.join("mod.rs"),
    ]
    .iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .unwrap()
}

/// Every `gate/dispatch_*.rs` source, by file name.
fn gate_dispatch_sources() -> Vec<(String, String)> {
    let gate_dir = workspace_root().join("aida-cli-lib/src/gate");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&gate_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("dispatch_") && n.ends_with(".rs"))
        .map(|n| {
            let text = std::fs::read_to_string(gate_dir.join(&n)).unwrap();
            (n, text)
        })
        .collect();
    out.sort();
    out
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn source_inclusion_detection_sees_evasive_forms() {
    for src in [
        "include!(\"../dispatch.rs\");",
        "fn f() { include!(\"../dispatch.rs\") }",
        "std::include!(\"../dispatch.rs\");",
        "fn f() { core::include!(\"../dispatch.rs\") }",
        "use std::include as inc;\nfn f() { inc!(\"../dispatch.rs\") }",
        "use std::{include};",
        "macro_rules! m { () => { include!(\"../dispatch.rs\"); } }",
        "#[path = \"../dispatch.rs\"] mod x;",
        "#[path = \"tests/../../dispatch.rs\"] mod x;",
        "#[path = \"/home/u/aida/aida-cli-lib/src/dispatch.rs\"] mod x;",
        "#[path = \"\\x2e\\x2e/dispatch.rs\"] mod x;",
        "#[path = r\"..\\dispatch.rs\"] mod x;",
        "#[path = r#\"../dispatch.rs\"#] mod x;",
        "#[path =\n    \"../dispatch.rs\"]\nmod x;",
        "#[cfg_attr(test, path = \"../dispatch.rs\")] mod x;",
        "#[cfg_attr(all(), cfg_attr(all(), path = \"../dispatch.rs\"))] mod x;",
        "mod outer { #[path = \"../../dispatch.rs\"] mod x; }",
        "macro_rules! m { () => { #[path = \"../dispatch.rs\"] mod x; } }",
    ] {
        assert!(
            !source_inclusions(src).is_empty(),
            "`{src}` compiles outside source into the gate unseen"
        );
    }
    for src in [
        "#[cfg(test)]\n#[path = \"tests/story_1917_dispatch_exec_tests.rs\"]\nmod t;",
        "mod inline { fn f() {} }",
        "fn f() { let _ = \"start with --include-supervised\"; }",
        "fn f() { let _ = include_str!(\"tests/fixture.txt\"); }",
    ] {
        assert_eq!(source_inclusions(src), Vec::<String>::new(), "{src}");
    }
}

/// `gate/dispatch_*` compiles no source from outside `gate/`:
/// `include!("../dispatch.rs")` or `#[path = "../dispatch.rs"] mod x;`
/// would put unprotected `dispatch.rs` code inside the gate module.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn gate_dispatch_includes_no_source_from_outside_the_gate() {
    let bad: Vec<String> = gate_dispatch_sources()
        .into_iter()
        .flat_map(|(name, text)| {
            source_inclusions(&text)
                .into_iter()
                .map(move |how| format!("  gate/{name}: {how}"))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "gate/dispatch_* compiles source from outside `gate/`:\n{}\nMove the \
         code into a gate module and mount it with a plain relative `#[path]`.",
        bad.join("\n")
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn crate_root_name_detection_sees_evasive_forms() {
    for (src, want) in [
        ("use crate::util::sha256_hex;", "util"),
        ("use crate::util::sha256_hex as h;", "util"),
        ("use crate::{gate, util::{sha256_hex}};", "util"),
        ("use super::super::util::sha256_hex;", "util"),
        ("use self::super::super::util::sha256_hex;", "util"),
        ("use super::super::{util};", "util"),
        ("mod m { use super::super::super::util::x; }", "util"),
        (
            "mod m { mod n { fn f() { super::super::super::super::util::x(); } } }",
            "util",
        ),
        ("fn f() { crate::util::sha256_hex(b); }", "util"),
        ("fn f() -> crate::util::Rec { todo!() }", "util"),
        ("fn f() { m!(crate::util::sha256_hex); }", "util"),
        (
            "fn f() { let _ = format!(\"{}\", crate::util::sha256_hex(b)); }",
            "util",
        ),
        ("#[serde(with = \"crate::util::codec\")]\nstruct S;", "util"),
        ("macro_rules! m { () => { $crate::util::x() } }", "util"),
        ("fn f() { use crate::{util as u}; }", "util"),
        ("use crate as c;", "crate"),
        ("use crate::{self as c};", "crate"),
        ("use super::super as c;", "crate"),
        ("extern crate self as me;", "crate"),
        ("use crate::*;", "*"),
        ("use super::super::*;", "*"),
    ] {
        let got = crate_root_names(src);
        assert!(
            got.iter().any(|g| g == want),
            "`{src}` must report `{want}`, got {got:?}"
        );
    }
    for src in [
        "use super::*;",
        "use super::dispatch_io::sha256_hex;",
        "use self::inner::x;",
        "mod tests { use super::*; }",
        "mod tests { use super::super::dispatch_io::x; }",
        "fn f() { std::fs::read(\"x\").ok(); }",
        "fn f() { let _ = \"reviewer\"; }",
    ] {
        assert_eq!(crate_root_names(src), Vec::<String>::new(), "{src}");
    }
    assert_eq!(crate_root_names("use crate::gate::mandate;"), vec!["gate"]);
}

// Each two-hop form from the sketch, and each way around the facade, is
// refused; listed leaves and gate-internal paths are not.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[test]
fn gate_dispatch_external_reach_sees_two_hop_forms() {
    let gate = "dispatch_authority.rs";
    let leaves = &[
        "crate::events::emit",
        "aida_core::liveness::process_start_identity",
    ];
    for (name, src) in [
        // The old allowlisted definers, however they are spelled.
        (gate, "fn f() { crate::canonical_role_name(s); }"),
        (gate, "fn f() { crate::load_store_for_lookup(p); }"),
        (gate, "use crate::kernel::canonical_role_name;"),
        (gate, "use super::super::lib_part3::load_store_for_lookup;"),
        (gate, "fn f() -> crate::forge::CiState { todo!() }"),
        (gate, "use crate::agent_registry::PidNamespaceProbe;"),
        (
            gate,
            "fn f() { crate::process_probe::process_start_identity(1); }",
        ),
        (gate, "use crate::mandate_drive::{ActionRow, Outcome};"),
        // A definer body's next hop.
        (gate, "fn f() { crate::util::wrap(raw); }"),
        // A listed leaf is the facade's, not every dispatch source's.
        (gate, "fn f() { crate::events::emit(r, e); }"),
        // A bare name resolved through a root glob or alias.
        (gate, "use crate::*;\nfn f() { canonical_role_name(s); }"),
        (
            gate,
            "use super::super::*;\nfn f() { canonical_role_name(s); }",
        ),
        (
            gate,
            "use crate as c;\nfn f() { c::canonical_role_name(s); }",
        ),
        // The facade: no definer, no glob or handover, no unlisted path.
        (
            DISPATCH_DEPS,
            "pub(crate) use crate::kernel::canonical_role_name;",
        ),
        (DISPATCH_DEPS, "pub(crate) use crate::util::wrap;"),
        (DISPATCH_DEPS, "use crate::*;"),
        (DISPATCH_DEPS, "use super::super::*;"),
        (DISPATCH_DEPS, "use crate as c;"),
        (DISPATCH_DEPS, "pub(crate) use crate::events::*;"),
        (
            DISPATCH_DEPS,
            "use crate::events;\nfn f() { events::forward(r); }",
        ),
        (DISPATCH_DEPS, "pub(crate) use crate::dispatch::sha256_hex;"),
        (DISPATCH_DEPS, "fn f() { crate::canonical_role_name(s); }"),
        (DISPATCH_DEPS, "fn f() -> crate::forge::CiState { todo!() }"),
        (
            DISPATCH_DEPS,
            "mod m { pub(crate) use crate::forge::CiState; }",
        ),
        (
            DISPATCH_DEPS,
            "pub(crate) fn h(raw: &str) -> String { crate::util::wrap(raw) }",
        ),
        // An unlisted sibling-crate call, or the sibling crate handed over.
        (
            DISPATCH_DEPS,
            "fn f() { aida_core::liveness::pid_is_alive(1); }",
        ),
        (DISPATCH_DEPS, "use aida_core::liveness::*;"),
        (DISPATCH_DEPS, "use aida_core as core2;"),
        (
            DISPATCH_DEPS,
            "use aida_core::liveness;\nfn f() { liveness::pid_is_alive(1); }",
        ),
    ] {
        assert!(
            !gate_dispatch_external_reach(name, src, leaves).is_empty(),
            "gate/{name}: `{src}` must be refused"
        );
    }
    for (name, src) in [
        (gate, "use crate::gate::dispatch_deps::canonical_role_name;"),
        (
            gate,
            "fn f() { crate::gate::dispatch_deps::emit_action(r, m, a, w); }",
        ),
        (
            gate,
            "use super::dispatch_deps::{CiState, PidNamespaceProbe};",
        ),
        (gate, "use crate::dispatch::DispatchVendor;"),
        (
            gate,
            "fn f() { aida_core::liveness::process_start_identity(1); }",
        ),
        (DISPATCH_DEPS, "fn f() { crate::events::emit(r, e); }"),
        (
            DISPATCH_DEPS,
            "pub(crate) use aida_core::liveness::process_start_identity;",
        ),
        (
            DISPATCH_DEPS,
            "pub(crate) use super::dispatch_types::CiState;",
        ),
        (DISPATCH_DEPS, "use crate::gate::mandate::MANDATE_SEAT;"),
        (
            DISPATCH_DEPS,
            "pub(crate) fn f(raw: &str) -> bool { raw.eq_ignore_ascii_case(HUMAN_ROUTE) }",
        ),
    ] {
        assert_eq!(
            gate_dispatch_external_reach(name, src, leaves),
            Vec::<String>::new(),
            "gate/{name}: {src}"
        );
    }
}

// One hop into the gate modules `gate/dispatch_*` names: an outside path
// there must be listed too, including one re-exported under a gate path.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[test]
fn gate_dispatch_one_hop_sees_outside_reach_in_named_gate_modules() {
    let leaves = &["crate::events::emit"];
    let hop = |dispatch: &str, module: &'static str| {
        let sources = vec![("dispatch_authority.rs".to_string(), dispatch.to_string())];
        gate_dispatch_one_hop(&sources, |_| module.to_string(), leaves)
    };
    for (dispatch, module) in [
        // A protected module re-exporting outside code under a gate path.
        (
            "fn f() { crate::gate::mandate::X::new(); }",
            "pub(crate) use crate::forge::X;",
        ),
        ("use super::mandate::X;", "pub(crate) use crate::forge::X;"),
        (
            "use crate::gate::{mandate::{X}};",
            "pub(crate) use super::super::forge::X;",
        ),
        // A protected module calling an unprotected definer.
        (
            "use crate::gate::seat_authority::grant_check_at;",
            "fn grant_check_at() { crate::grant_keyring::read(); }",
        ),
        (
            "use crate::gate::seat_authority::f;",
            "use crate::grant_keyring;\nfn f() { grant_keyring::read(); }",
        ),
        (
            "use crate::gate::seat_authority::f;",
            "use crate::*;\nfn f() {}",
        ),
        // A form that hands over `gate` itself cannot be followed.
        ("use crate::gate::*;", ""),
        ("use crate::gate as g;", ""),
        (
            "use crate::gate;\nfn f() { gate::mandate::X::new(); }",
            "pub(crate) use crate::forge::X;",
        ),
        ("use super::*;", ""),
    ] {
        assert!(
            !hop(dispatch, module).0.is_empty(),
            "`{dispatch}` -> `{module}` must be refused"
        );
    }
    for (dispatch, module) in [
        (
            "use crate::gate::mandate::f;",
            "fn f() { crate::events::emit(r, e); }",
        ),
        (
            "use crate::gate::seat_authority::f;",
            "fn f() { crate::gate::dispatch_deps::canonical_role_name(s); }",
        ),
        (
            "use crate::gate::mandate::f;",
            "fn f() { aida_core::liveness::process_start_identity(1); }",
        ),
        // A visibility path names no module.
        ("pub(in crate::gate) struct S;", "use crate::*;"),
        // Only one hop: a `dispatch_*` module is scanned on its own.
        (
            "use super::dispatch_io::f;",
            "fn f() { crate::util::wrap(raw); }",
        ),
    ] {
        assert_eq!(
            hop(dispatch, module).0,
            Vec::<String>::new(),
            "`{dispatch}` -> `{module}`"
        );
    }
}

/// `gate/dispatch_*` reaches outside `gate/` only through the facade
/// `gate/dispatch_deps.rs`, and the gate modules they name reach outside
/// only through the facade or a listed leaf. Every outside path the facade
/// and those modules name is in the facade's `FACADE_EXTERNAL_LEAVES`, and
/// every entry there is still used, so the list can only shrink without a
/// human merge. This checks direct reach from `gate/dispatch_*` and one hop
/// into the gate modules it names; it does not follow a leaf's own body. The
/// decision kernels' entries count as used here too; their own rule is
/// [`gate_kernels_reach_outside_only_through_listed_leaves`].
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[test]
fn gate_dispatch_reaches_outside_only_through_the_facade_or_listed_leaves() {
    let sources = gate_dispatch_sources();
    let leaves = facade_leaves();
    let facade = sources
        .iter()
        .find(|(name, _)| name == DISPATCH_DEPS)
        .unwrap_or_else(|| panic!("gate/{DISPATCH_DEPS} is missing"));
    let mut bad: Vec<String> = sources
        .iter()
        .flat_map(|(name, text)| {
            gate_dispatch_external_reach(name, text, &leaves)
                .into_iter()
                .map(move |how| format!("  gate/{name}: {how}"))
        })
        .collect();
    let (hop_bad, mut used) = gate_dispatch_one_hop(&sources, gate_module_source, &leaves);
    bad.extend(hop_bad);
    used.extend(outside_paths(&facade.1, true));
    // trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
    for (_, text) in gate_kernel_sources() {
        used.extend(outside_paths(&text, true));
    }
    let mut seen = std::collections::BTreeSet::new();
    let duplicate: Vec<&str> = leaves
        .iter()
        .copied()
        .filter(|l| !seen.insert(*l))
        .collect();
    let stale: Vec<&str> = leaves
        .iter()
        .copied()
        .filter(|l| !used.iter().any(|u| u == l))
        .collect();
    assert!(
        bad.is_empty(),
        "gate/dispatch_* (or a gate module it names) reaches outside `gate/` \
         past the facade:\n{}\nCall the item through `gate/{DISPATCH_DEPS}`; if \
         it feeds a gate decision, implement it on the floor. A leaf that must \
         stay outside goes in `FACADE_EXTERNAL_LEAVES` with its reason.",
        bad.join("\n")
    );
    assert!(
        stale.is_empty() && duplicate.is_empty(),
        "`FACADE_EXTERNAL_LEAVES` lists paths nothing reaches (stale: {stale:?}, \
         duplicate: {duplicate:?}); remove them."
    );
}

/// The crate root mounts each [`GATE_DISPATCH_ROOT_ALLOWLIST`] module from
/// the file the ratchet scans, invokes no item macro that could expand to
/// an unseen mount or binding, and shadows no macro `gate/dispatch_*` calls
/// bare.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
#[test]
fn crate_root_mounts_the_gate_plainly_and_hides_no_macros() {
    let src = workspace_root().join("aida-cli-lib/src");
    let read =
        |f: &str| std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let lib = read("lib.rs");
    let include = regex::Regex::new(r#"include!\s*\(\s*"([^"]+)""#).unwrap();
    let root_files: Vec<String> = std::iter::once("lib.rs".to_string())
        .chain(include.captures_iter(&lib).map(|c| c[1].to_string()))
        .collect();
    // Parse each crate-root file once: every check below walks the same
    // trees, and re-parsing ~0.5MB per check blew the test time budget.
    let roots: Vec<(&String, syn::File)> = root_files
        .iter()
        .map(|f| (f, parse_source(&read(f))))
        .collect();

    // `gate` and `dispatch` are scanned as `gate/` and `dispatch.rs`, so the
    // root mounts each from there as a bare `mod name;` and binds the name
    // no other way.
    for name in GATE_DISPATCH_ROOT_ALLOWLIST {
        let found: Vec<(Vec<String>, Vec<String>)> = roots
            .iter()
            .flat_map(|(_, parsed)| root_module_mounts_in(parsed, name))
            .collect();
        if let Some(why) = mount_not_bare(&found, name) {
            panic!(
                "the crate root must mount allowlisted `{name}` as one bare \
                 `mod {name};` (doc comments only) loading {name}.rs or \
                 {name}/mod.rs: {why}"
            );
        }
        let binders: Vec<&String> = roots
            .iter()
            .filter(|(_, parsed)| root_bindings(parsed).iter().any(|b| b == name))
            .map(|(root, _)| *root)
            .collect();
        assert!(
            binders.is_empty(),
            "{binders:?} bind allowlisted `{name}` at the crate root by `use` or \
             `extern crate`; only its bare `mod {name};` may"
        );
    }

    // A root macro could expand to a mount or binding syn never sees, so
    // the root invokes none but `include!` of its parts and `thread_local!`,
    // and no crate source shadows either name.
    let parts: Vec<String> = root_files[1..].to_vec();
    for (root, parsed) in &roots {
        let macros = root_item_macros(parsed, &parts);
        assert!(
            macros.is_empty(),
            "{root}: root-level {macros:?} could expand to a mount or binding \
             of an allowlisted name unscanned; write the items out instead"
        );
    }
    let mut sources = Vec::new();
    all_rust_files(&src, &mut sources);
    let shadowing: Vec<_> = sources
        .iter()
        .filter(|p| !p.ends_with("tests/story_1888_gate_paths_tests.rs"))
        .filter(|p| shadows_root_item_macro(&std::fs::read_to_string(p).unwrap_or_default()))
        .collect();
    assert!(
        shadowing.is_empty(),
        "these sources define or rename a macro to `include`/`thread_local`, \
         shadowing the crate root's builtin call: {shadowing:?}"
    );

    // No crate-root source shadows a macro the gate calls bare: rustc
    // resolves `format!` through textual scope first, so a `#[macro_use]`
    // module or root `macro_rules! format` would expand unscanned code
    // inside the gate.
    let gate_macros: std::collections::BTreeSet<String> = gate_dispatch_sources()
        .iter()
        .flat_map(|(_, text)| bare_macro_names(text))
        .collect();
    let mut shadows = Vec::new();
    for (root, parsed) in &roots {
        let (mods, rules) =
            root_textual_macros_in(parsed, &|f| std::fs::read_to_string(src.join(f)).ok());
        shadows.extend(mods.iter().map(|m| format!("  {root}: #[macro_use] {m}")));
        shadows.extend(
            rules
                .iter()
                .filter(|r| gate_macros.contains(*r))
                .map(|r| format!("  {root}: macro_rules! {r}")),
        );
    }
    assert!(
        shadows.is_empty(),
        "a crate-root source shadows a macro gate/dispatch_* calls bare:\n{}",
        shadows.join("\n")
    );
}

// An allowlisted module mounted from any file but its own is seen.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
#[test]
fn root_module_mount_detection_sees_redirected_modules() {
    let plain = || vec!["forge.rs".to_string(), "forge/mod.rs".to_string()];
    for src in [
        "mod forge;",
        "pub(crate) mod forge;",
        "#[cfg(unix)]\nmod forge;",
    ] {
        assert_eq!(root_module_mounts(src, "forge"), vec![plain()], "{src}");
    }
    for src in [
        "#[path = \"forge2.rs\"] mod forge;",
        "#[path = \"forge2.rs\"]\npub mod forge;",
        "#[path = \"forge.rs\"] mod forge_x;\n#[path = \"forge2.rs\"] mod forge;",
        "#[cfg_attr(unix, path = \"forge2.rs\")] mod forge;",
        "#[cfg_attr(all(), cfg_attr(all(), path = \"forge2.rs\"))] mod forge;",
        "mod forge;\n#[cfg(windows)]\n#[path = \"forge2.rs\"]\nmod forge;",
        "mod forge { pub fn f() {} }",
        "#[cfg(test)]\nmod forge;\n#[cfg(not(test))]\nmod forge { pub use crate::x::*; }",
    ] {
        let found = root_module_mounts(src, "forge");
        assert!(
            found.iter().any(|c| *c != plain()),
            "`{src}` mounts forge from another file unseen: {found:?}"
        );
    }
    // Only one bare `mod forge;` counts; every other attribute fails closed
    // and is named.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-023 | ai:claude
    let not_bare =
        |src: &str| mount_not_bare(&root_module_mounts_in(&parse_source(src), "forge"), "forge");
    for src in [
        "mod forge;",
        "pub(crate) mod forge;",
        "/// The forge.\nmod forge;",
        "#[doc = \"The forge.\"]\n#[doc(hidden)]\nmod forge;",
    ] {
        assert_eq!(not_bare(src), None, "{src}");
    }
    for (src, named) in [
        ("#[cfg_attr(not(test), cfg(any()))]\nmod forge;", "cfg_attr"),
        (
            "#[cfg_attr(all(), path = \"forge.rs\")]\nmod forge;",
            "cfg_attr",
        ),
        ("#[cfg(test)]\nmod forge;", "#[cfg(test)]"),
        ("#[cfg(not(test))]\nmod forge;", "#[cfg(not (test))]"),
        ("#[cfg(unix)]\nmod forge;", "#[cfg(unix)]"),
        ("#[allow(dead_code)]\nmod forge;", "#[allow(dead_code)]"),
        ("#[macro_use]\nmod forge;", "#[macro_use]"),
        (
            "/// Doc.\n#[cfg_attr(test, doc = \"x\")]\nmod forge;",
            "cfg_attr",
        ),
        ("#[path = \"forge.rs\"]\nmod forge;", "#[path = ..]"),
        ("#[cfg(test)]\nmod forge;\nmod forge;", "2 mounts"),
        ("mod forge { pub fn f() {} }", "<inline>"),
        ("#![cfg(test)]\nmod forge;", "no top-level"),
        ("", "no top-level"),
    ] {
        let why = not_bare(src).unwrap_or_else(|| panic!("`{src}` passed as bare"));
        assert!(why.contains(named), "`{src}`: {why}");
    }
    // Any root `use` or `extern crate` that binds the name is seen.
    for src in [
        "use crate::shadow as forge;",
        "pub(crate) use crate::x::{y, forge};",
        "use crate::x::forge::{self};",
        "extern crate forge;",
        "extern crate core as forge;",
        "use crate::x::r#forge;",
    ] {
        assert!(
            root_bindings(&parse_source(src)).contains(&"forge".to_string()),
            "{src}"
        );
    }
    for src in ["use crate::forge::f;", "use crate::forge::*;", "mod forge;"] {
        assert!(
            !root_bindings(&parse_source(src)).contains(&"forge".to_string()),
            "{src}"
        );
    }
    // Nested or absent: nothing mounts `crate::forge`.
    for src in [
        "mod a { mod forge; }",
        "mod a { mod forge { pub fn f() {} } }",
        "mod forge_profiles;",
        "",
    ] {
        assert!(root_module_mounts(src, "forge").is_empty(), "{src}");
    }
}

// Every root item macro that could expand to a mount or binding is seen,
// though syn never expands it and its tokens need not name what it binds.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
#[test]
fn root_macro_detection_sees_hidden_mounts_and_bindings() {
    let parts = ["lib_part1.rs".to_string()];
    let invoked = |src: &str| root_item_macros(&parse_source(src), &parts);
    for (src, shown) in [
        (
            "macro_rules! m { () => { #[cfg(not(test))] #[path = \"forge2.rs\"] mod forge; } }\n\
             m!();\n#[cfg(test)]\nmod forge;",
            "m!",
        ),
        (
            "macro_rules! m { ($p:ident, $n:ident) => { use $p::$n; } }\n\
             m!(util, canonical_role_name);",
            "m!",
        ),
        ("crate::reexport! { r#forge }", "crate::reexport!"),
        // The PR-2999 review repro (rustc-confirmed): util.rs holds
        // `#[macro_export] macro_rules! m { () => { #[cfg(not(test))]
        // #[path = "forge2.rs"] mod forge; } }`, so nothing here names
        // forge and the only visible mount is test-only.
        (
            "mod util;\n#[cfg(test)]\nmod forge;\ncrate::m!();",
            "crate::m!",
        ),
        ("::aida::m!();", "::aida::m!"),
        ("include!(\"lib_part9.rs\");", "include!"),
        ("include!(concat!(\"lib_part1\", \".rs\"));", "include!"),
        ("std::include!(\"lib_part1.rs\");", "std::include!"),
        (
            "std::thread_local! { static X: u8 = 0; }",
            "std::thread_local!",
        ),
    ] {
        assert_eq!(invoked(src), vec![shown], "{src}");
    }
    for src in [
        "mod forge;",
        "macro_rules! m { () => { mod forge; } }",
        "include!(\"lib_part1.rs\");",
        "thread_local! { static X: u8 = 0; }",
        "fn f() { m!(forge); }",
        "mod a { m!(); }",
    ] {
        assert!(invoked(src).is_empty(), "{src}");
    }
    for text in [
        "#[macro_export]\nmacro_rules! include { ($f:literal) => {} }",
        "macro_rules! r#thread_local { () => {} }",
        "pub(crate) use m as include;",
        "pub(crate) use crate::m as\n  thread_local;",
        // PR-2999 round 2: comments are whitespace to rustc.
        "#[macro_export]\nmacro_rules!/**/include { ($f:literal) => {} }",
        "macro_rules! /* x */ r#thread_local { () => {} }",
        "use crate::m as /**/ include;",
        "use crate::{a, m as // x\n include};",
        "macro_rules! gen { () => { macro_rules! /**/ include { () => {} } } }",
        "fn f() { macro_rules!/**/include { () => {} } }",
    ] {
        assert!(may_shadow_root_item_macro(text), "prefilter skips {text}");
        assert!(shadows_root_item_macro(text), "{text}");
    }
    for text in [
        "use std::include_str as inc;\nconst INCLUDED: u8 = 0;",
        "fn r#includes() {} // thread_locals",
    ] {
        assert!(!may_shadow_root_item_macro(text), "prefilter keeps {text}");
    }
    for text in [
        "include!(\"lib_part1.rs\");",
        "thread_local! { static X: u8 = 0; }",
        "use std::include_str as inc;",
        "use x as included;",
        "// macro_rules! include\nfn f() {}",
        "const S: &str = \"use m as include;\";",
    ] {
        assert!(!shadows_root_item_macro(text), "{text}");
    }
}

// Each way a crate-root source widens textual macro scope is seen.
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
#[test]
fn root_textual_macro_detection_sees_evasive_forms() {
    let none = |_: &str| None;
    for src in [
        "#[macro_use] mod util;",
        "#[macro_use]\npub(crate) mod util;",
        "#[cfg_attr(all(), macro_use)] mod util;",
        "#[cfg_attr(unix, cfg_attr(all(), macro_use))] mod util;",
        "#[macro_use] mod util { macro_rules! format { () => {} } }",
        "#[cfg(test)]\n#[macro_use]\nmod util;",
    ] {
        assert_eq!(root_textual_macros(src, &none).0, vec!["mod util"], "{src}");
    }
    // A `#[macro_use] extern crate` fills the `macro_use` prelude.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
    for src in [
        "#[macro_use] extern crate util;",
        "#[cfg_attr(all(), macro_use)]\nextern crate util;",
        "#[macro_use] extern crate util as other;",
    ] {
        assert_eq!(
            root_textual_macros(src, &none).0,
            vec!["extern crate util"],
            "{src}"
        );
    }
    assert!(root_textual_macros("extern crate util;", &none)
        .0
        .is_empty());
    for (src, want) in [
        ("macro_rules! format { () => {} }", "format"),
        (
            "#[macro_export]\nmacro_rules! matches { () => {} }",
            "matches",
        ),
        ("macro_rules! r#bail { () => {} }", "bail"),
    ] {
        assert_eq!(root_textual_macros(src, &none).1, vec![want], "{src}");
    }
    for src in [
        "mod util;",
        "mod util { macro_rules! format { () => {} } }",
        "fn f() { macro_rules! format { () => {} } }",
        "#[allow(unused_macros)] mod util;",
    ] {
        let (mods, rules) = root_textual_macros(src, &none);
        assert!(mods.is_empty() && rules.is_empty(), "{src}");
    }
    // An inner `#![macro_use]` in the module's own file widens it too,
    // wherever rustc would load that file from.
    for (src, file, body) in [
        (
            "mod util;",
            "util.rs",
            "#![macro_use]\nmacro_rules! format { () => {} }",
        ),
        ("mod util;", "util/mod.rs", "#![cfg_attr(all(), macro_use)]"),
        ("#[path = \"u2.rs\"] mod util;", "u2.rs", "#![macro_use]"),
        ("#[cfg(test)]\nmod util;", "util.rs", "#![macro_use]"),
    ] {
        let load = |f: &str| (f == file).then(|| body.to_string());
        assert_eq!(root_textual_macros(src, &load).0, vec!["mod util"], "{src}");
    }
    for (src, file, body) in [
        (
            "mod util;",
            "util.rs",
            "#![allow(unused_macros)]\nmacro_rules! format { () => {} }",
        ),
        ("mod util;", "other.rs", "#![macro_use]"),
        ("mod util;", "util.rs", "#[macro_use]\nmod inner;"),
        ("mod a { mod util; }", "a/util.rs", "#![macro_use]"),
    ] {
        let load = |f: &str| (f == file).then(|| body.to_string());
        assert!(
            root_textual_macros(src, &load).0.is_empty(),
            "{src} + {file}"
        );
    }
    let names = bare_macro_names(
        "fn f() { let _ = format!(\"{}\", matches!(x, Some(_))); bail! {\"x\"}; \
         std::println!(\"q\"); ::core::assert!(a != b); vec![1]; }",
    );
    let want: std::collections::BTreeSet<String> = ["bail", "format", "matches", "vec"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(names, want);
}

// Mounts are followed wherever rustc would load them, and judged test-only
// by `#[cfg(test)]`, not by where the file lives.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn mount_detection_follows_path_and_plain_modules() {
    let live = |file: &str, mod_rs: bool, src: &str| -> Vec<Vec<String>> {
        mounts(file, mod_rs, src)
            .into_iter()
            .filter(|m| !m.test)
            .map(|m| m.candidates.into_iter().map(|(c, _)| c).collect())
            .collect()
    };
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // A non-test `#[path]` mount under tests/ is still scanned.
    assert_eq!(
        live(
            "forge.rs",
            false,
            "#[path = \"tests/helpers.rs\"]\npub mod helpers;"
        ),
        vec![strs(&["tests/helpers.rs"])]
    );
    assert_eq!(
        live(
            "forge.rs",
            false,
            "#[cfg(not(test))]\n#[path = \"tests/h.rs\"]\nmod h;"
        ),
        vec![strs(&["tests/h.rs"])]
    );
    // An out-of-line `pub mod sub;` loads forge/sub.rs or forge/sub/mod.rs.
    assert_eq!(
        live("forge.rs", false, "pub mod sub;"),
        vec![strs(&["forge/sub.rs", "forge/sub/mod.rs"])]
    );
    assert_eq!(
        live("forge.rs", false, "mod a { pub mod sub; }"),
        vec![strs(&["forge/a/sub.rs", "forge/a/sub/mod.rs"])]
    );
    assert_eq!(
        live("forge.rs", false, "mod a { #[path = \"x.rs\"] mod sub; }"),
        vec![strs(&["forge/a/x.rs"])]
    );
    // A `#[path]`-loaded (or mod.rs) file owns its own directory.
    assert_eq!(
        live("forge_commit_ci.rs", true, "mod sub;"),
        vec![strs(&["sub.rs", "sub/mod.rs"])]
    );
    assert_eq!(
        live("forge/mod.rs", true, "mod sub;"),
        vec![strs(&["forge/sub.rs", "forge/sub/mod.rs"])]
    );
    assert_eq!(
        live(
            "forge.rs",
            false,
            "#[cfg_attr(unix, path = \"tests/u.rs\")]\nmod u;"
        ),
        vec![strs(&["tests/u.rs", "forge/u.rs", "forge/u/mod.rs"])]
    );
    // Test-only mounts are skipped, however they are spelled.
    for src in [
        "#[cfg(test)]\n#[path = \"tests/story_1689_commit_ci_status_tests.rs\"]\nmod t;",
        "#[cfg(test)]\nmod tests;",
        "#[cfg(test)]\nmod tests { mod inner; }",
        "#![cfg(test)]\nmod sub;",
        "mod inline { fn f() {} }",
    ] {
        assert!(live("forge.rs", false, src).is_empty(), "{src}");
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

// ── Every gate module (TASK-b6b1278890774ef993d2ebc327862470-024) ──────────

/// The gate's decision kernels: modules that decide, and reach outside
/// `gate/` only through a gate path or a `FACADE_EXTERNAL_LEAVES` entry
/// (`aida_core` paths included: `aida-core/` is not gate-protected). Every
/// other non-`dispatch_*` gate module is an orchestrator with a frozen
/// inventory in `gate::orchestrator_leaves`.
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
const GATE_KERNELS: &[&str] = &[
    "branch_policy.rs",
    "fleet_lanes.rs",
    "hold_collect.rs",
    "kernel_deps.rs",
    "mandate.rs",
    "mandate_drive.rs",
    "mandate_gates.rs",
    "mandate_traces.rs",
    "mod.rs",
    "operator_hold.rs",
    "orchestrator_leaves.rs",
    "relay_launch.rs",
    "review_author.rs",
    "review_approval.rs",
    "review_marker.rs",
    "review_verdict.rs",
    "role_authority.rs",
    "seat_authority.rs",
    "solo_review.rs",
];

/// Every `gate/*.rs` source, by file name.
fn gate_sources() -> Vec<(String, String)> {
    let gate_dir = workspace_root().join("aida-cli-lib/src/gate");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&gate_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".rs"))
        .map(|n| {
            let text = std::fs::read_to_string(gate_dir.join(&n)).unwrap();
            (n, text)
        })
        .collect();
    out.sort();
    out
}

fn gate_kernel_sources() -> Vec<(String, String)> {
    gate_sources()
        .into_iter()
        .filter(|(n, _)| GATE_KERNELS.contains(&n.as_str()))
        .collect()
}

/// The orchestrators' frozen inventories, by module file.
fn orchestrator_inventory() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    crate::gate::orchestrator_leaves::ORCHESTRATOR_OUTSIDE_PATHS
        .iter()
        .map(|(m, why, paths)| (*m, *why, paths.to_vec()))
        .collect()
}

/// The glob imports in `text` (a gate file) that reach outside `gate/`: a
/// glob of the crate root or of any root module, however spelled.
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
fn outside_globs(text: &str) -> Vec<String> {
    outside_paths(text, false)
        .into_iter()
        .filter(|p| p.ends_with("::*"))
        .collect()
}

/// Where a decision kernel reaches outside `gate/` past `leaves`: every
/// outside crate or `aida_core` path it names (direct, through an import,
/// re-exported or handed over whole) must be listed.
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-041 | ai:claude
fn kernel_outside_reach(text: &str, leaves: &[&str]) -> Vec<String> {
    outside_paths(text, true)
        .into_iter()
        .filter(|p| !leaves.contains(&p.as_str()))
        .collect()
}

/// How an orchestrator's outside reach differs from its frozen list: paths
/// it names that are not listed (`new`), and listed paths it no longer names
/// (`stale`).
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-041 | ai:claude
fn orchestrator_drift(text: &str, frozen: &[&str]) -> Vec<String> {
    let named = outside_paths(text, true);
    let mut out: Vec<String> = named
        .iter()
        .filter(|p| !frozen.contains(&p.as_str()))
        .map(|p| format!("new {p}"))
        .collect();
    out.extend(
        frozen
            .iter()
            .filter(|f| !named.iter().any(|n| n == *f))
            .map(|f| format!("stale {f}")),
    );
    out
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn every_gate_module_is_a_dispatch_module_a_kernel_or_an_orchestrator() {
    let inventory = orchestrator_inventory();
    let orchestrators: Vec<&str> = inventory.iter().map(|(m, _, _)| *m).collect();
    let names: Vec<String> = gate_sources().into_iter().map(|(n, _)| n).collect();
    let mut bad = Vec::new();
    for n in &names {
        let classes = [
            n.starts_with("dispatch_"),
            GATE_KERNELS.contains(&n.as_str()),
            orchestrators.contains(&n.as_str()),
        ]
        .iter()
        .filter(|c| **c)
        .count();
        if classes != 1 {
            bad.push(format!("  gate/{n}: in {classes} classes"));
        }
    }
    for listed in GATE_KERNELS.iter().chain(&orchestrators) {
        if !names.iter().any(|n| n == listed) {
            bad.push(format!("  gate/{listed}: listed but missing"));
        }
    }
    for (m, why, _) in &inventory {
        if why.trim().is_empty() {
            bad.push(format!("  gate/{m}: orchestrator without a reason"));
        }
    }
    assert!(
        bad.is_empty(),
        "every gate module is exactly one of gate/dispatch_*, a decision kernel \
         (GATE_KERNELS) or an orchestrator (gate::orchestrator_leaves):\n{}",
        bad.join("\n")
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn no_gate_module_glob_imports_outside_the_gate() {
    let bad: Vec<String> = gate_sources()
        .into_iter()
        .flat_map(|(name, text)| {
            outside_globs(&text)
                .into_iter()
                .map(move |g| format!("  gate/{name}: {g}"))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "gate modules glob-import code from outside `gate/`:\n{}\nName each \
         item: a glob hides which outside code the gate runs.",
        bad.join("\n")
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn gate_kernels_reach_outside_only_through_listed_leaves() {
    let leaves = facade_leaves();
    let bad: Vec<String> = gate_kernel_sources()
        .into_iter()
        .flat_map(|(name, text)| {
            kernel_outside_reach(&text, &leaves)
                .into_iter()
                .map(move |p| format!("  gate/{name}: {p}"))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "a decision-kernel gate module reaches outside `gate/` past the \
         listed leaves:\n{}\nIf it feeds a gate decision, implement it on the \
         floor (`gate/kernel_deps.rs`); a leaf that must stay outside goes in \
         `FACADE_EXTERNAL_LEAVES` with its reason.",
        bad.join("\n")
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn gate_orchestrators_reach_outside_only_through_their_frozen_inventory() {
    let sources = gate_sources();
    let mut bad = Vec::new();
    for (m, _, frozen) in orchestrator_inventory() {
        let Some((_, text)) = sources.iter().find(|(n, _)| n == m) else {
            continue;
        };
        let mut seen = std::collections::BTreeSet::new();
        bad.extend(
            frozen
                .iter()
                .filter(|p| !seen.insert(**p))
                .map(|p| format!("  gate/{m}: duplicate {p}")),
        );
        bad.extend(
            orchestrator_drift(text, &frozen)
                .into_iter()
                .map(|d| format!("  gate/{m}: {d}")),
        );
    }
    assert!(
        bad.is_empty(),
        "an orchestrating gate module's outside reach drifted from its frozen \
         inventory in `gate/orchestrator_leaves.rs`:\n{}\nRemove stale entries. \
         A new outside path needs a human merge of that file; prefer reaching \
         it through a gate module instead.",
        bad.join("\n")
    );
}

// Each refusal form per module class: a new outside path, a glob, a
// re-export, and the two-hop spellings (an import, an alias of the root).
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn every_gate_module_class_refuses_new_outside_reach() {
    let leaves = &[
        "crate::grant_keyring::read",
        "aida_core::team::canonical_role",
    ];
    let frozen = &["crate::forge::forge_for", "aida_core::team::canonical_role"];
    let refused = [
        // A new outside path.
        "fn f() { crate::forge::resolve_forge_kind(r); }",
        "fn f() -> crate::forge::ChangeRef { todo!() }",
        // A re-export under a gate path.
        "pub(crate) use crate::forge::ChangeRef;",
        "pub(crate) use super::super::forge::ChangeRef;",
        // Two hops: through an import, or the root handed over.
        "use crate::grant_keyring;\nfn f() { grant_keyring::wipe(); }",
        "use crate::forge;\nfn f() { forge::resolve_forge_kind(r); }",
        "use crate as c;\nfn f() { c::grant_keyring::read(); }",
        "use super::super as c;",
        "extern crate self as me;",
        // A glob of outside code.
        "use crate::*;",
        "use crate::forge::*;",
        // A new sibling-crate path, direct or through an import
        // (TASK-b6b1278890774ef993d2ebc327862470-041).
        "fn f() { aida_core::liveness::pid_is_alive(1); }",
        "use aida_core::liveness;\nfn f() { liveness::pid_is_alive(1); }",
        "use aida_core::team::TeamRoster;",
        // A leading `::` or an `extern crate` alias of the sibling crate
        // (TASK-b6b1278890774ef993d2ebc327862470-053).
        "fn f() { ::aida_core::liveness::pid_is_alive(1); }",
        "fn f() -> ::aida_core::team::TeamRoster { todo!() }",
        "extern crate aida_core as core2;\nfn f() { core2::liveness::pid_is_alive(1); }",
    ];
    for src in refused {
        assert!(
            !kernel_outside_reach(src, leaves).is_empty(),
            "kernel: `{src}` must be refused"
        );
        assert!(
            orchestrator_drift(src, frozen)
                .iter()
                .any(|d| d.starts_with("new ")),
            "orchestrator: `{src}` must be refused"
        );
    }
    for src in [
        "use crate::*;",
        "use super::super::*;",
        "use crate::merge_hold::*;",
        "mod mh { pub(super) use crate::merge_hold::*; }",
        "fn f() { use crate::forge::*; }",
    ] {
        assert!(
            !outside_globs(src).is_empty(),
            "glob: `{src}` must be refused"
        );
    }
    for src in [
        "use super::*;",
        "mod tests { use super::*; }",
        "mod mh { pub(super) use crate::gate::merge_hold_core::*; }",
        "use crate::gate::kernel_deps::*;",
        "use std::path::*;",
    ] {
        assert_eq!(outside_globs(src), Vec::<String>::new(), "glob: {src}");
    }
    for src in [
        "fn f() { crate::grant_keyring::read(); }",
        "use crate::grant_keyring::read;",
        "fn f() { crate::gate::kernel_deps::current_user_id(None); }",
        "use super::kernel_deps::is_hex_sha;",
        "fn f() { aida_core::team::canonical_role(s); }",
        "fn f() { ::aida_core::team::canonical_role(s); }",
        "pub(crate) fn f() {}",
    ] {
        assert_eq!(
            kernel_outside_reach(src, leaves),
            Vec::<String>::new(),
            "kernel: {src}"
        );
    }
    assert_eq!(
        orchestrator_drift(
            "fn f() { crate::forge::forge_for(r); aida_core::team::canonical_role(s); }",
            frozen
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        orchestrator_drift("fn f() {}", frozen),
        vec![
            "stale crate::forge::forge_for".to_string(),
            "stale aida_core::team::canonical_role".to_string()
        ]
    );
}
