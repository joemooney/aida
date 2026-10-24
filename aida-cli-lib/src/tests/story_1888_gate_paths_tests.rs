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

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn gate_authority_code_is_on_the_floor() {
    let missing: Vec<String> = scan()
        .into_iter()
        .filter(|(path, _)| !on_floor(path) && !KNOWN_UNPROTECTED.contains(&path.as_str()))
        .map(|(path, why)| format!("  {path} ({why})"))
        .collect();
    assert!(
        missing.is_empty(),
        "gate-authority code outside the mandate's protected floor:\n{}\n\
         Move it into aida-cli-lib/src/gate/ (the change then needs a human merge). \
         Do not remove the marker, rename the function, or widen \
         KNOWN_UNPROTECTED to get past this check.",
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

// trace:STORY-1888.aca70d99 | ai:claude
#[test]
fn known_unprotected_only_shrinks() {
    let found = scan();
    let stale: Vec<&str> = KNOWN_UNPROTECTED
        .iter()
        .copied()
        .filter(|p| !found.contains_key(*p) || on_floor(p))
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_UNPROTECTED entries that no longer hold gate authority outside the \
         floor: {stale:?}. Delete them from KNOWN_UNPROTECTED in mandate_gates.rs; \
         the list only shrinks."
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
fn gate_reexports(text: &str) -> Vec<String> {
    let reexport = regex::Regex::new(
        r"^\s*pub(\([^)]*\))?\s+use\s+(crate::|self::|super::)*(\{[^}]*\b)?gate(::|\s*;|\s*\})",
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

#[test]
fn no_unprotected_file_reexports_gate_items() {
    let found: std::collections::BTreeMap<String, Vec<String>> = sources()
        .into_iter()
        .filter(|(path, _)| !on_floor(path))
        .filter_map(|(path, text)| {
            let lines = gate_reexports(&text);
            (!lines.is_empty()).then_some((path, lines))
        })
        .collect();
    let unlisted: Vec<String> = found
        .iter()
        .filter(|(path, _)| !TRANSITIONAL_GATE_REEXPORTS.contains(&path.as_str()))
        .map(|(path, lines)| format!("  {path}: {}", lines.join(" | ")))
        .collect();
    assert!(
        unlisted.is_empty(),
        "unprotected files re-export crate::gate items:\n{}\n\
         A re-export outside the floor can be repointed at a non-gate function \
         without a human merge. Call `crate::gate::...` directly instead.",
        unlisted.join("\n")
    );
    let stale: Vec<&str> = TRANSITIONAL_GATE_REEXPORTS
        .iter()
        .copied()
        .filter(|p| !found.contains_key(*p))
        .collect();
    assert!(
        stale.is_empty(),
        "TRANSITIONAL_GATE_REEXPORTS entries that no longer re-export: {stale:?}. \
         Delete them from mandate_gates.rs; the list only shrinks."
    );
}
