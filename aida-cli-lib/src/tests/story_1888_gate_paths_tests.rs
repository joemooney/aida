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
    /// Root handovers (`use crate::*;`, `use crate as c;`), public or not.
    handovers: Vec<Named>,
    /// Modules the source declares, which shadow a root glob's names.
    local_mods: Vec<String>,
    /// Every `#[path]` target (and `path = "..."` in attribute or macro
    /// tokens), escapes decoded.
    path_targets: Vec<String>,
    /// How many times the `include` macro is named.
    includes: usize,
    level: usize,
    /// Skip root handovers through a private `use` (a test module's
    /// `use super::*;`), which no other module can reach through; they
    /// still root the module's own relative paths.
    public_handovers_only: bool,
    in_use: bool,
    private_use: bool,
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
    /// Named by a non-private `use`, so other modules can reach it.
    public: bool,
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
            public: self.in_use && !self.private_use,
        };
        if crate_root || supers >= self.level {
            self.root(named);
        } else if supers == 0 && !tail.is_empty() {
            self.unrooted.push(named);
        }
    }

    /// Records a path that resolves to the crate root.
    fn root(&mut self, named: Named) {
        if named.segs.is_empty() {
            self.handovers.push(named.clone());
            if self.public_handovers_only && !named.public {
                return;
            }
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

    fn path(&mut self, p: &syn::Path) {
        if p.leading_colon.is_none() {
            let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
            self.note(&segs, false, false);
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for Reach {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.in_use = true;
        self.private_use = matches!(u.vis, syn::Visibility::Inherited);
        self.tree(&mut Vec::new(), &u.tree);
        self.in_use = false;
        self.private_use = false;
    }
    fn visit_item_extern_crate(&mut self, e: &'ast syn::ItemExternCrate) {
        // `extern crate self as me;` renames the crate root.
        if e.ident == "self" {
            self.rooted.push(Named {
                segs: Vec::new(),
                glob: false,
                whole: true,
                rename: e.rename.as_ref().map(|(_, r)| r.to_string()),
                public: true,
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

/// What a source `gate/dispatch_*` may reach (an allowlisted definer, or a
/// crate-root file) hands on from `crate::dispatch`: like
/// [`dispatch_items_at`], but a private `use super::*;` is not a handover.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn launders_at(text: &str, level: usize) -> Vec<String> {
    let reach = Reach {
        level,
        public_handovers_only: true,
        ..Reach::default()
    };
    dispatch_items(scan_reach(text, reach))
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

/// What a source `gate/dispatch_*` may reach re-exports from another
/// crate-root module (`pub(crate) use crate::util::*;`). The gate could
/// then reach `util` through an allowlisted name without `util` ever being
/// scanned, so an allowlisted definer re-exports nothing from the root.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn root_reexports_at(text: &str, level: usize) -> Vec<String> {
    let reach = Reach {
        level,
        public_handovers_only: true,
        ..Reach::default()
    };
    scan_reach(text, reach)
        .rooted
        .into_iter()
        .filter(|n| n.public && !n.segs.is_empty())
        .map(|n| n.segs.join("::") + if n.glob { "::*" } else { "" })
        .collect()
}

/// How each root-level `use` and item in a crate-root source binds `name`:
/// the paths of explicit `use`s that bind it (leading `crate`/`self`
/// dropped), the modules globbed into the root, and whether an item there
/// defines it.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
fn root_bindings(text: &str, name: &str) -> (Vec<Vec<String>>, Vec<Vec<String>>, bool) {
    fn walk(
        tree: &syn::UseTree,
        prefix: &mut Vec<String>,
        name: &str,
        explicit: &mut Vec<Vec<String>>,
        globs: &mut Vec<Vec<String>>,
    ) {
        let path = |prefix: &[String], leaf: &syn::Ident| {
            let mut p: Vec<String> = prefix
                .iter()
                .skip_while(|s| *s == "crate" || *s == "self")
                .cloned()
                .collect();
            if leaf != "self" {
                p.push(leaf.to_string());
            }
            p
        };
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                walk(&p.tree, prefix, name, explicit, globs);
                prefix.pop();
            }
            syn::UseTree::Group(g) => {
                g.items
                    .iter()
                    .for_each(|t| walk(t, prefix, name, explicit, globs));
            }
            syn::UseTree::Glob(_) => globs.push(
                prefix
                    .iter()
                    .skip_while(|s| *s == "crate" || *s == "self")
                    .cloned()
                    .collect(),
            ),
            syn::UseTree::Name(n) if n.ident == name => explicit.push(path(prefix, &n.ident)),
            syn::UseTree::Rename(r) if r.rename == name => explicit.push(path(prefix, &r.ident)),
            _ => {}
        }
    }
    let file = syn::parse_file(text).unwrap_or_else(|e| panic!("source must parse: {e}"));
    let (mut explicit, mut globs, mut defines) = (Vec::new(), Vec::new(), false);
    for item in &file.items {
        let ident = match item {
            syn::Item::Use(u) => {
                walk(&u.tree, &mut Vec::new(), name, &mut explicit, &mut globs);
                continue;
            }
            syn::Item::Fn(f) => &f.sig.ident,
            syn::Item::Const(c) => &c.ident,
            syn::Item::Static(s) => &s.ident,
            syn::Item::Mod(m) => &m.ident,
            syn::Item::Struct(s) => &s.ident,
            syn::Item::Enum(e) => &e.ident,
            syn::Item::Type(t) => &t.ident,
            syn::Item::Trait(t) => &t.ident,
            syn::Item::Macro(m) => match &m.ident {
                Some(i) => i,
                None => continue,
            },
            _ => continue,
        };
        defines |= ident == name;
    }
    (explicit, globs, defines)
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
    let parsed = syn::parse_file(text).unwrap_or_else(|e| panic!("{file} must parse: {e}"));
    let mut out = Vec::new();
    if !is_cfg_test(&parsed.attrs) {
        walk(&parsed.items, dir, &owned, &mut Vec::new(), false, &mut out);
    }
    out
}

/// The files each top-level out-of-line `mod name;` in a crate-root source
/// may load from. An allowlisted module is scanned as `name.rs`, so any
/// `#[path]` here (`#[path = "forge2.rs"] mod forge;`) would let the gate's
/// `crate::name` resolve to an unscanned file; an inline `mod name { .. }`
/// is not reported at all.
// trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
fn root_module_mounts(text: &str, name: &str) -> Vec<Vec<String>> {
    mounts("lib.rs", true, text)
        .into_iter()
        .filter(|m| m.depth == 1 && m.name == name)
        .map(|m| m.candidates.into_iter().map(|(c, _)| c).collect())
        .collect()
}

/// What a crate-root source puts in the textual macro scope of every
/// module declared after it: each `#[macro_use]` module (bare or under
/// `cfg_attr`) as `mod x`, and each top-level `macro_rules!` by name. A
/// `#[macro_use] mod util;` or root `macro_rules! format` shadows the
/// prelude macro `gate/dispatch_*` calls bare, so its expansion runs
/// unscanned code inside the gate.
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
fn root_textual_macros(text: &str) -> (Vec<String>, Vec<String>) {
    let macro_use = regex::Regex::new(r"\bmacro_use\b").unwrap();
    let file = syn::parse_file(text).unwrap_or_else(|e| panic!("source must parse: {e}"));
    let (mut mods, mut rules) = (Vec::new(), Vec::new());
    for item in &file.items {
        match item {
            syn::Item::Mod(m)
                if m.attrs.iter().any(|a| {
                    a.path().is_ident("macro_use")
                        || matches!(&a.meta, syn::Meta::List(l)
                            if a.path().is_ident("cfg_attr")
                                && macro_use.is_match(&l.tokens.to_string()))
                }) =>
            {
                mods.push(format!("mod {}", m.ident));
            }
            syn::Item::Macro(m) if m.mac.path.is_ident("macro_rules") => {
                if let Some(ident) = &m.ident {
                    rules.push(ident.to_string().trim_start_matches("r#").to_string());
                }
            }
            _ => {}
        }
    }
    (mods, rules)
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

/// Crate-root names `gate/dispatch_*` may reach, each with the unprotected
/// `aida-cli-lib/src` file that defines it (`""` for `gate` itself and for
/// `dispatch`, whose items [`GATE_DISPATCH_IMPORT_ALLOWLIST`] governs).
/// Any other root module could re-export a `crate::dispatch` primitive
/// under a new path (`pub use crate::dispatch::sha256_hex;` in `util`, then
/// `crate::util::sha256_hex`), so a new entry needs gate review.
const GATE_DISPATCH_ROOT_ALLOWLIST: &[(&str, &str)] = &[
    ("gate", ""),
    ("dispatch", ""),
    ("agent_registry", "agent_registry.rs"),
    ("forge", "forge.rs"),
    ("mandate_drive", "mandate_drive.rs"),
    ("process_probe", "process_probe.rs"),
    ("canonical_role_name", "kernel.rs"),
    ("load_store_for_lookup", "lib_part3.rs"),
];

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

/// `gate/dispatch_*` reaches the crate root only through
/// [`GATE_DISPATCH_ROOT_ALLOWLIST`], and neither those names' defining
/// sources nor the crate-root sources (`lib.rs` and what it `include!`s,
/// where a `use` could shadow an allowlisted name) touch `crate::dispatch`.
/// Without this, a mandate-shipped `pub use crate::dispatch::sha256_hex;` in
/// any unprotected module launders the primitive past the import ratchet.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn gate_dispatch_reaches_the_crate_only_through_allowlisted_names() {
    let allowed = |n: &str| GATE_DISPATCH_ROOT_ALLOWLIST.iter().any(|(a, _)| *a == n);
    let mut bad = Vec::new();
    for (name, text) in gate_dispatch_sources() {
        for root in crate_root_names(&text) {
            if !allowed(&root) {
                bad.push(format!("  gate/{name}: crate::{root}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "gate/dispatch_* reaches crate-root names off the allowlist:\n{}\nUse \
         the item through a gate module, or add its module to \
         GATE_DISPATCH_ROOT_ALLOWLIST with gate review.",
        bad.join("\n")
    );

    let src = workspace_root().join("aida-cli-lib/src");
    let read =
        |f: &str| std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let lib = read("lib.rs");
    let include = regex::Regex::new(r#"include!\s*\(\s*"([^"]+)""#).unwrap();
    let root_files: Vec<String> = std::iter::once("lib.rs".to_string())
        .chain(include.captures_iter(&lib).map(|c| c[1].to_string()))
        .collect();
    // (file, levels below the root its top-level code sits)
    let mut scanned: Vec<(String, usize)> = root_files.iter().map(|f| (f.clone(), 0)).collect();
    // Allowlisted definers and every non-test module they mount, however
    // deep: (file, level, read mod.rs-style).
    let mut definers: Vec<(String, usize, bool)> = Vec::new();
    for (name, file) in GATE_DISPATCH_ROOT_ALLOWLIST {
        if file.is_empty() || root_files.iter().any(|f| f == file) {
            continue;
        }
        let text = read(file);
        let defines = regex::Regex::new(&format!(r"\b(fn|mod)\s+{name}\b")).unwrap();
        assert!(
            *file == format!("{name}.rs") || defines.is_match(&text),
            "{file} no longer defines allowlisted `{name}`"
        );
        definers.push((file.to_string(), 1, false));
    }
    let mut next = 0;
    while next < definers.len() {
        let (file, level, mod_rs) = definers[next].clone();
        next += 1;
        for mount in mounts(&file, mod_rs, &read(&file)) {
            if mount.test {
                continue;
            }
            let found = mount
                .candidates
                .into_iter()
                .find(|(c, _)| src.join(c).is_file());
            if let Some((target, target_mod_rs)) = found {
                if !definers.iter().any(|(f, ..)| *f == target) {
                    definers.push((target, level + mount.depth, target_mod_rs));
                }
            }
        }
    }
    scanned.extend(definers.iter().map(|(f, level, _)| (f.clone(), *level)));

    // The root binds each allowlisted name to the allowlisted definition:
    // no root-level `use` rebinds it from elsewhere (an explicit
    // `use util::canonical_role_name;` shadows `use kernel::*;`), no other
    // root item defines it, and a fn entry is bound from its own file.
    for (name, file) in GATE_DISPATCH_ROOT_ALLOWLIST {
        let in_root = root_files.iter().any(|f| f == file);
        let fn_entry = !file.is_empty() && *file != format!("{name}.rs");
        let home: Vec<String> = if fn_entry && !in_root {
            vec![file.trim_end_matches(".rs").to_string(), name.to_string()]
        } else {
            Vec::new()
        };
        let (mut bound, mut defined) = (false, 0);
        for root in &root_files {
            let (explicit, globs, defines) = root_bindings(&read(root), name);
            for path in &explicit {
                assert!(
                    *path == home,
                    "{root} binds allowlisted `{name}` to crate::{}",
                    path.join("::")
                );
                bound = true;
            }
            bound |= !home.is_empty() && globs.iter().any(|g| *g == home[..1]);
            defined += usize::from(defines);
        }
        if in_root {
            let defines = regex::Regex::new(&format!(r"\bfn\s+{name}\s*[(<]")).unwrap();
            assert!(
                defines.is_match(&read(file)),
                "{file} no longer defines `fn {name}`"
            );
        } else if fn_entry {
            assert!(bound, "the crate root no longer binds `{name}` from {file}");
            assert_eq!(defined, 0, "a crate-root item shadows allowlisted `{name}`");
        } else if !file.is_empty() {
            // A module entry is scanned as `name.rs`, so the root mounts it
            // from there: no `#[path]`, no inline body.
            // trace:TASK-b6b1278890774ef993d2ebc327862470-014 | ai:claude
            let plain = vec![file.to_string(), format!("{name}/mod.rs")];
            let found: Vec<Vec<String>> = root_files
                .iter()
                .flat_map(|root| root_module_mounts(&read(root), name))
                .collect();
            assert!(
                !found.is_empty() && found.iter().all(|c| *c == plain),
                "the crate root must mount allowlisted `{name}` as a plain \
                 `mod {name};` loading {file}, found {found:?}"
            );
        }
    }

    // No crate-root source shadows a macro the gate calls bare: rustc
    // resolves `format!` through textual scope first, so a `#[macro_use]`
    // module or root `macro_rules! format` would expand unscanned code
    // inside the gate.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
    let gate_macros: std::collections::BTreeSet<String> = gate_dispatch_sources()
        .iter()
        .flat_map(|(_, text)| bare_macro_names(text))
        .collect();
    let mut shadows = Vec::new();
    for root in &root_files {
        let (mods, rules) = root_textual_macros(&read(root));
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

    let mut launders: Vec<String> = scanned
        .iter()
        .flat_map(|(file, level)| {
            launders_at(&read(file), *level)
                .into_iter()
                .map(move |item| format!("  {file}: crate::dispatch::{item}"))
        })
        .collect();
    for (file, level, _) in &definers {
        for path in root_reexports_at(&read(file), *level) {
            launders.push(format!("  {file}: re-exports crate::{path}"));
        }
    }
    assert!(
        launders.is_empty(),
        "a source gate/dispatch_* may reach names `crate::dispatch` or \
         re-exports another crate-root module, so it could hand a dispatch \
         primitive to the gate:\n{}",
        launders.join("\n")
    );
}

// The laundering forms an allowlisted (or crate-root) source could use are
// seen at that source's level.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn dispatch_item_detection_sees_laundering_at_each_level() {
    for (src, level) in [
        ("pub use crate::dispatch::sha256_hex;", 1),
        ("pub use super::dispatch::sha256_hex;", 1),
        ("pub use super::super::dispatch::sha256_hex;", 2),
        (
            "pub fn h(b: &[u8]) -> String { crate::dispatch::sha256_hex(b) }",
            1,
        ),
        ("pub use dispatch::sha256_hex as canonical_role_name;", 0),
        (
            "pub fn load_store_for_lookup() { dispatch::sha256_hex(b); }",
            0,
        ),
        ("pub use self::dispatch::*;", 0),
        ("pub use super::*;", 1),
        ("pub(crate) use super::{self as root};", 1),
        ("mod tests { pub(crate) use super::super::*; }", 1),
        ("use crate::dispatch::sha256_hex;", 1),
        // A private root handover roots the module's relative paths.
        (
            "use crate::*;\npub fn h(b: &[u8]) -> String { dispatch::sha256_hex(b) }",
            1,
        ),
        (
            "pub(crate) fn canonical_role_name(raw: &str) -> String { \
             dispatch::sha256_hex(raw.as_bytes()) }\nuse crate::*;",
            1,
        ),
        ("use crate::*;\npub(crate) use dispatch::sha256_hex;", 1),
        ("use crate::*;\npub(crate) use dispatch;", 1),
        ("use super::*;\nfn f() { dispatch::sha256_hex(b); }", 1),
        ("use crate as c;\nfn f() { c::dispatch::sha256_hex(b); }", 1),
        ("use super as c;\npub use c::dispatch::sha256_hex;", 1),
        ("use crate::{self as c};\nfn f() { c::dispatch::x(); }", 1),
        ("use crate as c;\nuse c::*;\nfn f() { dispatch::x(); }", 1),
        (
            "use crate as c;\nuse c as d;\nfn f() { d::dispatch::x(); }",
            1,
        ),
        (
            "use super::super as c;\nfn f() { m!(c::dispatch::sha256_hex); }",
            2,
        ),
    ] {
        assert!(
            !launders_at(src, level).is_empty(),
            "`{src}` at level {level} must be seen"
        );
    }
    for (src, level) in [
        ("mod dispatch;\nuse kernel::*;\n", 0),
        ("fn f() { let _ = \"dispatch\"; }", 0),
        ("mod tests { use super::*; }", 0),
        ("use super::*;", 1),
        ("use crate::*;\nfn f() { forge::x(); }", 1),
        ("use crate::*;\nmod dispatch;\nfn f() { dispatch::x(); }", 1),
        ("fn f() { dispatch::x(); }", 1),
        ("use crate as c;\nfn f() { c::forge::x(); }", 1),
    ] {
        assert!(launders_at(src, level).is_empty(), "{src}");
    }
}

// A definer that re-exports another crate-root module hands the gate a
// name nobody scans.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn root_reexport_detection_sees_two_hop_laundering() {
    for (src, level) in [
        ("pub(crate) use crate::util::*;", 1),
        ("pub(crate) use crate::util::CiState;", 1),
        ("pub use super::util::sha256_hex as canonical_role_name;", 1),
        ("pub(crate) use super::super::util::*;", 2),
        ("use crate::*;\npub(crate) use util::*;", 1),
        ("use crate as c;\npub(crate) use c::util::x;", 1),
        ("mod inner { pub(crate) use crate::util::x; }", 1),
    ] {
        assert!(
            !root_reexports_at(src, level).is_empty(),
            "`{src}` at level {level} must be seen"
        );
    }
    for (src, level) in [
        (
            "#[path = \"forge_commit_ci.rs\"]\nmod commit_ci;\npub(crate) use commit_ci::*;",
            1,
        ),
        (
            "use crate::*;\nmod commit_ci;\npub(crate) use commit_ci::*;",
            1,
        ),
        ("pub use aida_core::liveness::ProcTree;", 1),
        ("use crate::util::x;", 1),
        ("mod tests { use super::*; }", 1),
    ] {
        assert_eq!(root_reexports_at(src, level), Vec::<String>::new(), "{src}");
    }
}

// The crate root's binding of an allowlisted fn name is seen, including an
// explicit `use` that would shadow the glob it comes through.
// trace:TASK-b6b1278890774ef993d2ebc327862470-010 | ai:claude
#[test]
fn root_binding_detection_sees_shadowing() {
    let n = "canonical_role_name";
    let kernel = || vec!["kernel".to_string(), n.to_string()];
    let util = || vec!["util".to_string(), n.to_string()];
    let (e, g, d) = root_bindings("mod kernel;\nuse kernel::*;\n", n);
    assert!(e.is_empty() && g == vec![vec!["kernel".to_string()]] && !d);
    for src in [
        "pub(crate) use util::canonical_role_name;",
        "use crate::util::canonical_role_name;",
        "use util::{x, canonical_role_name};",
        "use util::helper as canonical_role_name;",
    ] {
        let (e, ..) = root_bindings(src, n);
        assert!(
            !e.is_empty() && e.iter().all(|p| *p != kernel()),
            "{src}: {e:?}"
        );
    }
    assert_eq!(
        root_bindings("use util::canonical_role_name;", n).0,
        vec![util()]
    );
    assert_eq!(
        root_bindings("pub use kernel::canonical_role_name;", n).0,
        vec![kernel()]
    );
    assert!(root_bindings("pub(crate) fn canonical_role_name() {}", n).2);
    assert!(root_bindings("macro_rules! canonical_role_name { () => {} }", n).2);
    assert!(!root_bindings("mod m { fn canonical_role_name() {} }", n).2);
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
    ] {
        let found = root_module_mounts(src, "forge");
        assert!(
            found.iter().any(|c| *c != plain()),
            "`{src}` mounts forge from another file unseen: {found:?}"
        );
    }
    // Inline, nested or absent: nothing loads forge.rs as `crate::forge`.
    for src in [
        "mod forge { pub fn f() {} }",
        "mod a { mod forge; }",
        "mod forge_profiles;",
        "",
    ] {
        assert!(root_module_mounts(src, "forge").is_empty(), "{src}");
    }
}

// Each way a crate-root source widens textual macro scope is seen.
// trace:TASK-b6b1278890774ef993d2ebc327862470-015 | ai:claude
#[test]
fn root_textual_macro_detection_sees_evasive_forms() {
    for src in [
        "#[macro_use] mod util;",
        "#[macro_use]\npub(crate) mod util;",
        "#[cfg_attr(all(), macro_use)] mod util;",
        "#[cfg_attr(unix, cfg_attr(all(), macro_use))] mod util;",
        "#[macro_use] mod util { macro_rules! format { () => {} } }",
        "#[cfg(test)]\n#[macro_use]\nmod util;",
    ] {
        assert_eq!(root_textual_macros(src).0, vec!["mod util"], "{src}");
    }
    for (src, want) in [
        ("macro_rules! format { () => {} }", "format"),
        (
            "#[macro_export]\nmacro_rules! matches { () => {} }",
            "matches",
        ),
        ("macro_rules! r#bail { () => {} }", "bail"),
    ] {
        assert_eq!(root_textual_macros(src).1, vec![want], "{src}");
    }
    for src in [
        "mod util;",
        "mod util { macro_rules! format { () => {} } }",
        "fn f() { macro_rules! format { () => {} } }",
        "#[allow(unused_macros)] mod util;",
    ] {
        let (mods, rules) = root_textual_macros(src);
        assert!(mods.is_empty() && rules.is_empty(), "{src}");
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
