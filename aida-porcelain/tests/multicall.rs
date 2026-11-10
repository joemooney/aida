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
    assert_eq!(before, snapshot(dir.path()));
}

#[test]
fn reader_fallback_and_failure_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("installed");
    let path_bin = dir.path().join("path");
    fs::create_dir(&installed).unwrap();
    fs::create_dir(&path_bin).unwrap();
    let companion = installed.join("aida-porcelain");
    fs::copy(env!("CARGO_BIN_EXE_aida-porcelain"), &companion).unwrap();
    let write_reader = |path: &Path, body: &str| {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    let path_reader = path_bin.join("aida");
    let sibling = installed.join("aida");
    let explicit = dir.path().join("explicit");
    write_reader(
        &path_reader,
        "printf 'path-out'; printf 'path-err' >&2; exit 23",
    );
    write_reader(
        &sibling,
        "printf 'sibling-out'; printf 'sibling-err' >&2; exit 24",
    );
    write_reader(
        &explicit,
        "printf 'explicit-out'; printf 'explicit-err' >&2; exit 25",
    );
    let run = |selection: Option<&Path>, format: &str| {
        let mut cmd = Command::new(&companion);
        cmd.args([
            "flow",
            "report",
            "--since",
            "2026-11-09",
            "--format",
            format,
        ])
        .env_remove("AIDA_BIN")
        .env("PATH", &path_bin);
        if let Some(selection) = selection {
            cmd.env("AIDA_BIN", selection);
        }
        cmd.output().unwrap()
    };
    for (selection, label, code) in [
        (Some(explicit.as_path()), "explicit", 25),
        (None, "sibling", 24),
    ] {
        let output = run(selection, "json");
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(output.stdout, format!("{label}-out").as_bytes());
        assert_eq!(output.stderr, format!("{label}-err").as_bytes());
    }
    for format in ["human", "toon", "json"] {
        let output = run(Some(&explicit), format);
        assert_eq!(output.status.code(), Some(25));
        assert_eq!(output.stdout, b"explicit-out");
        assert_eq!(output.stderr, b"explicit-err");
    }
    fs::remove_file(sibling).unwrap();
    let output = run(None, "json");
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(output.stdout, b"path-out");
    assert_eq!(output.stderr, b"path-err");
    // An explicit missing binary must not silently select the PATH reader.
    assert_eq!(
        run(Some(&dir.path().join("missing")), "json").status.code(),
        Some(1)
    );
}

// Environment aliases must select the same error channels as their canonical
// explicit formats, regardless of piped stdout or the agent-output fallback.
// The explicit reference also verifies that the flag overrides an unknown env pin.
#[test]
fn environment_alias_error_parity() {
    let dir = tempfile::tempdir().unwrap();
    let reader = dir.path().join("reader");
    fs::write(
        &reader,
        "#!/bin/sh\nprintf 'reader-out'; printf 'reader-err' >&2; exit 23\n",
    )
    .unwrap();
    fs::set_permissions(&reader, fs::Permissions::from_mode(0o755)).unwrap();
    let run = |format: &str, explicit: Option<&str>, agent: &str, since: &str, reader: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aida-porcelain"));
        if let Some(explicit) = explicit {
            command.args(["--format", explicit]);
        }
        command
            .args(["flow", "report", "--since", since])
            .current_dir(dir.path())
            .env("AIDA_OUTPUT_FORMAT", format)
            .env("AIDA_AGENT_OUTPUT", agent)
            .env("AIDA_BIN", reader)
            .output()
            .unwrap()
    };
    for (alias, canonical, agent) in [
        ("table", "human", "0"),
        ("table", "human", "1"),
        (" TABLE ", "human", "1"),
        ("agent", "toon", "0"),
        ("agent", "toon", "1"),
        (" AgEnT ", "toon", "0"),
    ] {
        for (since, selected_reader, code) in [
            ("bad", reader.clone(), 1),
            ("2026-11-09", dir.path().join("missing-reader"), 1),
            ("2026-11-09", reader.clone(), 23),
        ] {
            let aliased = run(alias, None, agent, since, &selected_reader);
            let expected = run("unsupported", Some(canonical), agent, since, &selected_reader);
            assert_eq!(aliased.status.code(), Some(code));
            assert_eq!(aliased.status.code(), expected.status.code());
            assert_eq!(aliased.stdout, expected.stdout, "{alias}: {since}");
            assert_eq!(aliased.stderr, expected.stderr, "{alias}: {since}");
            if code == 23 {
                assert_eq!(aliased.stdout, b"reader-out");
                assert_eq!(aliased.stderr, b"reader-err");
            } else if canonical == "human" {
                assert!(aliased.stdout.is_empty());
                assert!(String::from_utf8_lossy(&aliased.stderr).starts_with("Error: "));
            } else {
                assert!(aliased.stderr.is_empty());
                assert!(String::from_utf8_lossy(&aliased.stdout).starts_with("error: \""));
            }
        }
    }
}
