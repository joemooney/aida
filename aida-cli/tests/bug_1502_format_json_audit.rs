// End-to-end conformance for the global JSON pin and its checked-in command
// audit. These tests drive the real binary, because dispatch gaps are the
// defect class unit render tests repeatedly missed.
// trace:BUG-1502 | ai:codex
#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};

/// Build a real-binary command that is incapable of escaping its fixture.
///
/// Review reproductions often run with live AIDA coordination variables in
/// the parent shell. Cwd isolation alone is too easy to omit when translating
/// this helper into an ad-hoc command, so after init every child is also pinned
/// to the fixture's canonical store. Root/drive and git overrides are removed
/// before either resolver can observe them.
// trace:BUG-1588 | ai:codex
fn aida_command(repo: &Path, home: &Path) -> Command {
    aida_command_with_inherited(repo, home, &[])
}

fn aida_command_with_inherited(repo: &Path, home: &Path, inherited: &[(&str, &Path)]) -> Command {
    assert!(
        repo.join(".git").exists(),
        "fixture repo must be a git checkout"
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor");
    for (name, value) in inherited {
        command.env(name, value);
    }
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
    let fixture_store = repo.join(".aida-store");
    if fixture_store.join("objects").is_dir() {
        command.env("AIDA_STORE", fixture_store);
    }
    command
}

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    aida_command(repo, home)
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1502 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    std::fs::write(
        tmp.path().join(".aida-workspace"),
        "name = \"fixture\"\nstore_path = \"repo/.aida-store\"\n\n[[repos]]\npath = \"repo\"\nname = \"fixture-repo\"\n",
    )
    .unwrap();

    let description =
        "quote: \"hello\"\nbackslash: C:\\\\tmp\\file\nblank follows\n\nUnicode: café 東京 🧭";
    let description_path = tmp.path().join("description.txt");
    std::fs::write(&description_path, description).unwrap();
    let add = aida(
        &repo,
        &home,
        &[
            "add",
            "--title",
            "format json fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--description-from-file",
            description_path.to_str().unwrap(),
            "--queue",
        ],
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap()
        .to_string();

    let edit = aida(
        &repo,
        &home,
        &[
            "edit",
            &spec,
            "--implementation-summary",
            "implemented fixture",
            "--risk-notes",
            "residual fixture risk",
            "--test-coverage-notes",
            "fixture coverage",
            "--mode",
            "drive",
            "--origin",
            "fixture-repo/component",
            "--add-ref",
            "github:owner/repo#42",
        ],
    );
    assert!(
        edit.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&edit.stdout),
        String::from_utf8_lossy(&edit.stderr)
    );
    let assign = aida(&repo, &home, &["assign", &spec, "--to", "review-fixture"]);
    assert!(
        assign.status.success(),
        "{}",
        String::from_utf8_lossy(&assign.stderr)
    );
    let done = aida(
        &repo,
        &home,
        &[
            "queue",
            "done",
            &spec,
            "--interface-cli",
            "fixture command changed",
        ],
    );
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    (tmp, repo, home, spec)
}

fn json(out: &Output, label: &str) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!("{label}: {error}\n{}", String::from_utf8_lossy(&out.stdout))
    })
}

fn normalize_volatile(mut value: serde_json::Value) -> serde_json::Value {
    // Separate process invocations can cross a one-second boundary while
    // reporting the same session. The contract is the field/schema and all
    // stable values, not equality of a live elapsed-time sample.
    if let Some(object) = value.as_object_mut() {
        object.remove("idle_secs");
    }
    value
}

#[test]
fn real_binary_fixture_cannot_write_an_inherited_caller_store() {
    let (_tmp, repo, home, _spec) = fixture();
    let decoy = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(decoy.path().join("objects")).unwrap();

    // Model a reviewer shell carrying live-store/root anchors. The helper's
    // boundary must replace/remove them before the child starts.
    let output = aida_command_with_inherited(
        &repo,
        &home,
        &[
            ("AIDA_STORE", decoy.path()),
            ("AIDA_PROJECT_ROOT", decoy.path()),
            ("AIDA_DRIVE_ROOT", decoy.path()),
        ],
    )
    .args([
        "add",
        "--title",
        "BUG-1588 isolation sentinel",
        "--type",
        "task",
        "--status",
        "approved",
    ])
    .output()
    .expect("run isolated aida add");
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let fixture_objects = repo.join(".aida-store/objects");
    assert!(tree_contains(
        &fixture_objects,
        "BUG-1588 isolation sentinel"
    ));
    assert!(
        !tree_contains(&decoy.path().join("objects"), "BUG-1588 isolation sentinel"),
        "fixture command escaped into the caller store"
    );
}

fn tree_contains(root: &Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            tree_contains(&path, needle)
        } else {
            std::fs::read_to_string(path).is_ok_and(|body| body.contains(needle))
        }
    })
}

#[test]
fn show_format_json_round_trips_description_and_matches_json_flag() {
    let (_tmp, repo, home, spec) = fixture();
    let via_format = aida(
        &repo,
        &home,
        &["show", &spec, "--no-git", "--format", "json"],
    );
    let via_flag = aida(&repo, &home, &["show", &spec, "--no-git", "--json"]);
    let format_value = json(&via_format, "show --format json");
    let flag_value = json(&via_flag, "show --json");
    assert_eq!(format_value, flag_value);
    assert_eq!(
        format_value["description"],
        "quote: \"hello\"\nbackslash: C:\\\\tmp\\file\nblank follows\n\nUnicode: café 東京 🧭"
    );
    assert!(format_value["git_linkage"].is_null());
    assert!(format_value["opened"].is_string());
    assert!(format_value["modified"].is_string());
    assert_eq!(format_value["assignee"], "review-fixture");
    assert_eq!(format_value["origin"]["repo"], "fixture-repo");
    assert_eq!(format_value["origin"]["component"], "component");
    assert_eq!(format_value["external_refs"][0], "github:owner/repo#42");
    assert_eq!(
        format_value["implementation_summary"],
        "implemented fixture"
    );
    assert_eq!(format_value["risk_notes"], "residual fixture risk");
    assert_eq!(format_value["test_coverage_notes"], "fixture coverage");
    assert_eq!(format_value["execution_mode"], "drive");
    assert_eq!(
        format_value["interface_changes"]["cli"][0],
        "fixture command changed"
    );
    assert_eq!(format_value["queue_membership"][0]["role"], "general");
    assert_eq!(format_value["queue_membership"][0]["position"], 1);
}

#[test]
fn show_json_uses_type_aware_status_display_and_preserves_stored_status() {
    let (_tmp, repo, home, _task_spec) = fixture();
    let add = aida(
        &repo,
        &home,
        &[
            "add",
            "--title",
            "status display fixture",
            "--type",
            "decision",
            "--status",
            "approved",
        ],
    );
    assert!(
        add.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&add.stdout),
        String::from_utf8_lossy(&add.stderr)
    );
    let spec = String::from_utf8_lossy(&add.stdout)
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("ADR-") && token[4..].chars().all(|c| c.is_ascii_digit()))
        .unwrap()
        .to_string();
    let value = json(
        &aida(
            &repo,
            &home,
            &["show", &spec, "--no-git", "--format", "json"],
        ),
        "decision show --format json",
    );
    assert_eq!(value["status"], "Accepted");
    assert_eq!(value["stored_status"], "Approved");
}

#[test]
fn show_json_includes_git_linkage_unless_suppressed() {
    let (_tmp, repo, home, spec) = fixture();
    std::fs::write(
        repo.join("trace.rs"),
        format!("// trace:{spec} | ai:test\nfn linked() {{}}\n"),
    )
    .unwrap();
    git(&repo, &["add", "trace.rs"]);
    git(
        &repo,
        &[
            "commit",
            "-q",
            "-m",
            &format!("test: link fixture ({spec})"),
        ],
    );
    let value = json(
        &aida(&repo, &home, &["show", &spec, "--format", "json"]),
        "show linkage",
    );
    assert!(value["git_linkage"]["commits"]
        .as_array()
        .is_some_and(|v| !v.is_empty()));
    assert!(value["git_linkage"]["files"]
        .as_array()
        .is_some_and(|v| !v.is_empty()));
    assert_eq!(value["git_linkage"]["shipped"], true);
}

#[test]
fn unsupported_global_json_pin_errors_before_human_fallback() {
    let (_tmp, repo, home, _spec) = fixture();
    let out = aida(&repo, &home, &["focus", "show", "--format", "json"]);
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("has no JSON projection"), "{combined}");
    assert!(
        !combined.contains("No focus set"),
        "unsupported JSON fell through to the human renderer: {combined}"
    );
}

#[test]
fn checked_in_audit_is_exhaustive_and_every_honoured_probe_parses() {
    let (_tmp, repo, home, spec) = fixture();
    let catalog = json(
        &aida(&repo, &home, &["help", "commands", "--json", "--hidden"]),
        "command catalog",
    );
    let catalog_paths: BTreeSet<String> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            entry["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let audit = include_str!("../../docs/cli-format-json-audit.md");
    let audit_paths: BTreeSet<String> = audit
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split_once('`').map(|(path, _)| path.to_string()))
        .collect();
    assert_eq!(catalog_paths, audit_paths, "regenerate the BUG-1502 audit");

    let audit_outcomes: BTreeMap<String, String> = audit
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| {
            let (path, rest) = line.split_once('`')?;
            let outcome = rest.split('|').nth(1)?.trim();
            Some((path.to_string(), outcome.to_string()))
        })
        .collect();

    let probes: &[(&str, &[&str])] = &[
        ("aida fasttrack status", &["fasttrack", "status"]),
        ("aida show", &["show", &spec, "--no-git"]),
        ("aida list", &["list", "--limit", "1"]),
        ("aida status", &["status", &spec]),
        ("aida ps", &["ps"]),
        ("aida queue", &["queue"]),
        ("aida queue list", &["queue", "list"]),
        ("aida queue progress", &["queue", "progress"]),
        ("aida findings list", &["findings", "list"]),
        // trace:BUG-1631 | ai:claude
        ("aida history", &["history", "--full"]),
        ("aida history events", &["history", "events"]),
    ];
    for (path, args) in probes {
        let mut via_format = args.to_vec();
        via_format.extend(["--format", "json"]);
        let format_value = json(&aida(&repo, &home, &via_format), path);
        let mut via_flag = args.to_vec();
        via_flag.push("--json");
        let flag_value = json(&aida(&repo, &home, &via_flag), path);
        assert_eq!(
            normalize_volatile(format_value),
            normalize_volatile(flag_value),
            "{path} spellings diverged"
        );
        assert!(audit.contains(&format!("| `{path}` | JSON honoured |")));
    }

    let verified: BTreeSet<&str> = probes.iter().map(|(path, _)| *path).collect();
    for entry in catalog.as_array().unwrap() {
        let path = entry["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        let has_local_json = entry["flags"]
            .as_array()
            .is_some_and(|flags| flags.iter().any(|flag| flag["name"] == "json"));
        let expected = if verified.contains(path.as_str()) {
            "JSON honoured"
        } else if has_local_json {
            "COULD NOT VERIFY"
        } else {
            "JSON not honoured"
        };
        assert_eq!(
            audit_outcomes.get(&path).map(String::as_str),
            Some(expected),
            "audit outcome drifted for {path}; regenerate the BUG-1502 audit"
        );

        // Every unsupported catalog path is driven through the real binary.
        // Missing required operands may make clap reject first; either way the
        // global JSON pin can never reach a successful human/TOON fallback.
        if expected == "JSON not honoured" {
            let mut args: Vec<&str> = path.split_whitespace().skip(1).collect();
            args.extend(["--format", "json"]);
            let out = aida(&repo, &home, &args);
            assert!(
                !out.status.success(),
                "unsupported catalog path silently succeeded: {path}\n{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}

// BUG-1631: `aida history --json` stdout is pure JSON even where the human
// `Window: …` line would otherwise print (human mode forced with
// AIDA_AGENT_OUTPUT=0, or `--format human --json`).
// trace:BUG-1631 | ai:claude
#[test]
fn bug_1631_history_json_stdout_is_pure_json_outside_agent_mode() {
    let (_tmp, repo, home, _spec) = fixture();
    let cases: &[(&[&str], Option<&str>)] = &[
        (&["history", "--json", "--since", "7d"], Some("0")),
        (&["history", "events", "--json", "--since", "7d"], Some("0")),
        (
            &["history", "--format", "human", "--json", "--since", "7d"],
            None,
        ),
    ];
    for (args, agent_env) in cases {
        let mut command = aida_command(&repo, &home);
        command.args(*args);
        if let Some(v) = agent_env {
            command.env("AIDA_AGENT_OUTPUT", v);
        }
        let out = command.output().expect("run aida");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!stdout.contains("Window:"), "{args:?} leaked: {stdout}");
        let value: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("{args:?} stdout is not JSON ({e}): {stdout}"));
        assert!(value["events"].is_array(), "{args:?}: {value}");
    }
}
