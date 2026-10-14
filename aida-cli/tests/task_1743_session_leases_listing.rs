// Gated to Linux like every other binary-driving e2e suite here.
#![cfg(target_os = "linux")]
//! TASK-1743 / TASK-1368: drive the real `aida session leases` renderer, not
//! just `select_lease_listing`. A stale spec-scoped lease must appear in the
//! default view (it still blocks dispatch) with a state column, while a stale
//! harness lease is hidden until `--all`.
// trace:TASK-1743 | ai:claude
// trace:TASK-1368 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1");
    for k in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_ID",
        "AIDA_STORE_PATH",
        "AIDA_PROJECT_ROOT",
        "AIDA_USER",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    )
}

fn git(repo: &Path, home: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Write a lease whose worktree does not exist, which classifies as stale.
fn write_stale_lease(repo: &Path, id: &str, scope: &str) {
    let started = "2020-01-01T00:00:00Z";
    let missing = repo.join("missing-worktrees").join(id);
    let mut lease = toml::map::Map::new();
    for (k, v) in [
        ("id", id.to_string()),
        ("scope", scope.to_string()),
        ("slug", id.to_string()),
        ("owner", "tester".to_string()),
        ("worktree_path", missing.display().to_string()),
        ("branch", format!("{id}-branch")),
        ("started_at", started.to_string()),
        ("hostname", "test".to_string()),
        ("role", "implementer".to_string()),
    ] {
        lease.insert(k.to_string(), toml::Value::String(v));
    }
    let body = toml::to_string(&toml::Value::Table(lease)).unwrap();
    let dir = repo.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.toml")), body).unwrap();
}

fn json_ids(o: &Output) -> Vec<(String, String)> {
    assert!(o.status.success(), "{}", show(o));
    let rows: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json rows");
    rows.as_array()
        .expect("array")
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_string(),
                r["state"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn session_leases_renders_stale_spec_lease_and_hides_stale_harness_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &home, &["init", "-q", "-b", "main"]);
    git(&repo, &home, &["config", "user.email", "t@t.t"]);
    git(&repo, &home, &["config", "user.name", "t"]);
    git(&repo, &home, &["config", "commit.gpgsign", "false"]);
    git(
        &repo,
        &home,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(init.status.success(), "aida init failed: {}", show(&init));

    write_stale_lease(&repo, "specstale0001", "TASK-9001");
    write_stale_lease(&repo, "harnessstale01", "harness-worktree");

    // JSON default: only the dispatch-blocking spec lease, reported stale.
    let default_json = aida(&repo, &home)
        .args(["session", "leases", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        json_ids(&default_json),
        vec![("specstale0001".to_string(), "stale".to_string())]
    );

    // JSON --all: both leases.
    let all_json = aida(&repo, &home)
        .args(["session", "leases", "--all", "--json"])
        .output()
        .unwrap();
    let mut all_ids: Vec<String> = json_ids(&all_json).into_iter().map(|(i, _)| i).collect();
    all_ids.sort();
    assert_eq!(all_ids, vec!["harnessstale01", "specstale0001"]);

    // Human default: state column always present, spec row shown as stale,
    // harness row hidden behind the --all hint.
    let human = aida(&repo, &home)
        .args(["session", "leases"])
        .output()
        .unwrap();
    assert!(human.status.success(), "{}", show(&human));
    let out = String::from_utf8_lossy(&human.stdout);
    let header = out
        .lines()
        .find(|l| l.trim_start().starts_with("id "))
        .unwrap_or_else(|| panic!("no header row:\n{out}"));
    assert!(header.contains("state"), "state column missing:\n{out}");
    let spec_row = out
        .lines()
        .find(|l| l.contains("TASK-9001"))
        .unwrap_or_else(|| panic!("stale spec lease missing from default view:\n{out}"));
    assert!(spec_row.contains("stale"), "spec row lacks state:\n{out}");
    assert!(
        !out.contains("harness-worktree"),
        "harness lease leaked:\n{out}"
    );
    assert!(
        out.contains("1 stale lease hidden"),
        "no hidden hint:\n{out}"
    );

    // Human --all: harness lease now shown.
    let human_all = aida(&repo, &home)
        .args(["session", "leases", "--all"])
        .output()
        .unwrap();
    assert!(human_all.status.success(), "{}", show(&human_all));
    let out_all = String::from_utf8_lossy(&human_all.stdout);
    assert!(out_all.contains("harness-worktree"), "{out_all}");
    assert!(!out_all.contains("stale lease hidden"), "{out_all}");
}
