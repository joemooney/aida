//! Executable entrypoint and filesystem contract. trace:STORY-2704 | ai:codex
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(snapshot(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}
#[test]
fn report_multicall_parity_and_no_writes() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        ".git/refs/heads/main",
        ".aida-store/.git/refs/heads/store",
        "tracked.txt",
        ".aida/cache-db/state",
        ".aida/queue.json",
        ".aida/sessions/lease.toml",
    ] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, name).unwrap();
    }
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(&git,"#!/bin/sh\n[ \"$1\" = log ] || exit 19\nprintf '2026-11-09T04:00:00Z\\tsubject (#42)\\n'\n").unwrap();
    let reader = bin.join("aida");
    fs::write(&reader, "#!/bin/sh\n[ \"$1 $2\" = \"flow report\" ] || exit 19\necho '{\"prs\":[{\"open_hours\":4.0,\"ledger_present\":false}],\"aggregate\":{},\"notes\":[]}'\n").unwrap();
    for path in [&git, &reader] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let before = snapshot(dir.path());
    let invoke = |name: &str, prefix: &[&str], extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_aida-porcelain"))
            .arg0(name)
            .args(prefix)
            .args(extra)
            .current_dir(dir.path())
            .env("PATH", &bin)
            .env("AIDA_BIN", &reader)
            .output()
            .unwrap()
    };
    let args = [
        "report",
        "--since",
        "2026-11-09",
        "--until",
        "2026-11-10",
        "--json",
    ];
    let explicit = invoke("/somewhere/aida-porcelain", &["flow"], &args);
    let alias = invoke("/somewhere/aida-flow", &[], &args);
    let global = invoke("aida-porcelain", &["--format", "json", "flow"], &args[..5]);
    assert!(
        global.status.success(),
        "{}",
        String::from_utf8_lossy(&global.stderr)
    );
    assert_eq!(explicit.stdout, global.stdout);
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(explicit.stdout, alias.stdout);
    assert_eq!(explicit.status.code(), alias.status.code());
    let report: serde_json::Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(report["prs"][0]["open_hours"], 4.0);
    assert_eq!(report["prs"][0]["ledger_present"], false);
    assert!(invoke("aida-flow", &[], &["--help"]).status.success());
    assert_eq!(invoke("unknown", &[], &args).status.code(), Some(2));
    assert_eq!(
        invoke("aida-flow", &[], &["report", "--since", "bad"])
            .status
            .code(),
        Some(1)
    );
    let missing_reader = Command::new(env!("CARGO_BIN_EXE_aida-porcelain"))
        .args(["flow", "report", "--since", "2026-11-09"])
        .env_remove("AIDA_BIN")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .env("AIDA_AGENT_OUTPUT", "1")
        .output()
        .unwrap();
    assert_eq!(missing_reader.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing_reader.stdout).contains("AIDA_BIN is required"));
    assert_eq!(before, snapshot(dir.path()));
}
