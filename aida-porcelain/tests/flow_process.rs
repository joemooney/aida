//! Exercise the installed process boundary, including lexical symlink dispatch.
// trace:STORY-2704 | ai:codex
use std::{collections::BTreeMap, fs, os::unix::fs::symlink, path::Path, process::Command};

fn snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn report_process_alias_help_and_errors_preserve_fixture() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // Empty history exercises the real Git adapter without needing a forge or
    // network; metric-bearing ledger fixtures live beside the report itself.
    fs::write(root.join("tracked.txt"), "tracked sentinel\n").unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["add", "tracked.txt"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    for name in [
        ".aida-store/.git/refs/heads/aida-store",
        ".aida/cache-db/state",
        ".aida/queue.json",
        ".aida/sessions/lease.toml",
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "sentinel\n").unwrap();
    }
    let links = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_aida-porcelain");
    symlink(binary, links.path().join("aida-porcelain")).unwrap();
    symlink("aida-porcelain", links.path().join("aida-flow")).unwrap();
    symlink("aida-porcelain", links.path().join("aida-unknown")).unwrap();
    let before = snapshot(root);
    let report = [
        "report",
        "--since",
        "2000-01-01",
        "--until",
        "2000-01-02",
        "--json",
    ];
    let run = |name: &str, args: &[&str]| {
        let output = Command::new(links.path().join(name))
            .current_dir(root)
            .env_remove("AIDA_OUTPUT_FORMAT")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            snapshot(root),
            before,
            "{name} {args:?} changed fixture state"
        );
        output
    };
    let alias = run("aida-flow", &report);
    let mut explicit = vec!["flow"];
    explicit.extend(report);
    let canonical = run("aida-porcelain", &explicit);
    assert!(
        alias.status.success(),
        "{}",
        String::from_utf8_lossy(&alias.stderr)
    );
    assert_eq!(alias.stdout, canonical.stdout);
    assert_eq!(alias.status.code(), canonical.status.code());
    let json: serde_json::Value = serde_json::from_slice(&alias.stdout).unwrap();
    assert_eq!(json["prs"], serde_json::json!([]));
    let help = run("aida-flow", &["report", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--since"));
    let invalid = run(
        "aida-flow",
        &["report", "--since", "bad", "--format", "human"],
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("expected RFC3339"));
    assert_eq!(run("aida-flow", &["report"]).status.code(), Some(2));
    let unknown = run("aida-unknown", &["--help"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("usage:"));
}
