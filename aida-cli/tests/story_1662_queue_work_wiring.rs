// Binary-driving e2e suite; Linux-only like its siblings (BUG-688).
#![cfg(target_os = "linux")]
//! STORY-1662 (TASK-2154): the standalone implementer launch exercised
//! through `aida queue work`, not by calling the launcher module directly —
//! the `is_standalone_implementer` dispatch, the concurrency slot taken
//! before any setup, the launch-authority record, the outcome line and exit
//! code, and no manual-continuation hint for an outcome.
// trace:STORY-1662 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn aida(repo: &Path, home: &Path, bin_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env_remove("AIDA_OS_WRAP");
    cmd.env_remove("AIDA_CONTAINED");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    cmd.env("PATH", path);
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
        "AIDA_SESSION_ID",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    spec: String,
}

/// A Claude-enabled project with one approved spec. The fake `claude` on
/// PATH runs `claude_body`. `config_extra` is appended to the project config.
fn fixture(claude_body: &str, config_extra: &str) -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    let bin = base.join("bin");
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    support::contained_lane_host(&home, &bin);
    let claude = bin.join("claude");
    std::fs::write(&claude, format!("#!/bin/sh\n{claude_body}\n")).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Hermetic forge: PR metadata lookups fail fast instead of reaching GitHub.
    let gh = bin.join("gh");
    std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    // The allocation reader refuses shared writes, independently of runner umask.
    // trace:STORY-2660 | ai:codex
    let common = aida_core::allocation_authority::common_dir(&repo).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(&repo, &home, &bin)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    std::fs::write(
        repo.join(".aida/config.toml"),
        format!("[agents]\nenabled = [\"claude\"]\n{config_extra}"),
    )
    .unwrap();

    let advisor = support::ensure_seat(&home, &repo, "advisor", &["implementer", "reviewer"])
        .expect("initialized");
    let add = aida(&repo, &home, &bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &advisor)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--title",
            "standalone implementer wiring",
        ])
        .output()
        .unwrap();
    assert!(add.status.success(), "{add:?}");
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.starts_with("STORY-"))
        .unwrap_or_else(|| panic!("no spec id in {stdout}"))
        .to_string();
    // Settle the per-command bookkeeping every invocation writes, so the
    // snapshots compare pickup state only.
    let settle = aida(&repo, &home, &bin)
        .args(["queue", "list", "--all"])
        .output()
        .unwrap();
    assert!(settle.status.success(), "{settle:?}");
    Fixture {
        _tmp: tmp,
        base,
        repo,
        home,
        bin,
        spec,
    }
}

/// Every file under the fixture (outside git's object database, whose
/// externally visible effects — refs and worktrees — are captured separately),
/// plus refs and the worktree list. Byte-identical before and after a refusal
/// means no status, queue, branch, worktree, lease, manifest or grant write.
fn snapshot(fx: &Fixture) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                if entry.file_name() == ".git" {
                    continue;
                }
                walk(root, &path, out);
            } else if ft.is_file() {
                // SQLite's `-shm` is the volatile WAL index (reader marks,
                // rebuilt from the WAL), not state. The cache keeps it on disk
                // (SQLITE_FCNTL_PERSIST_WAL), so any reader rewrites it. The
                // `.db` and `-wal` files stay in the snapshot.
                // trace:BUG-1929 | ai:claude
                if path.extension().is_some_and(|e| e == "db-shm") {
                    continue;
                }
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&fx.base, &fx.base, &mut out);
    out.insert(
        "<git refs>".into(),
        git(&fx.repo, &["for-each-ref"]).into_bytes(),
    );
    out.insert(
        "<git worktrees>".into(),
        git(&fx.repo, &["worktree", "list", "--porcelain"]).into_bytes(),
    );
    out
}

fn assert_unchanged(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) {
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .collect();
    assert!(changed.is_empty(), "refusal mutated state: {changed:?}");
}

fn launch(fx: &Fixture, grant: &str) -> (Option<i32>, String, String) {
    let out = aida(&fx.repo, &fx.home, &fx.bin)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_PURPOSE", "advise")
        .env("AIDA_SESSION_GRANT", grant)
        .args([
            "queue",
            "work",
            &fx.spec,
            "--role",
            "implementer",
            "--no-human",
            "--no-pull",
        ])
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A launch past the concurrency cap exits 34 before any setup: no lease,
/// worktree, queue row, status change or manifest.
// trace:STORY-1662.AC7 | ai:claude
// trace:STORY-1662.AC11 | ai:claude
#[test]
fn cap_refusal_exits_34_and_leaves_no_lease_worktree_or_queue_row() {
    let fx = fixture("exit 97", "[queue]\nstandalone_implementer_cap = 1\n");
    // One live launcher (this test process) already holds the only slot.
    let slots = fx.home.join(".aida/standalone-implementers");
    std::fs::create_dir_all(&slots).unwrap();
    std::fs::write(slots.join(".lock"), "").unwrap();
    std::fs::write(
        slots.join(format!("{}.json", std::process::id())),
        format!(
            "{{\"pid\":{},\"spec\":\"STORY-OTHER\"}}",
            std::process::id()
        ),
    )
    .unwrap();
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["reviewer"]);
    let before = snapshot(&fx);
    let (code, stdout, stderr) = launch(&fx, &grant);
    assert_eq!(code, Some(34), "exit {code:?}: {stdout}{stderr}");
    // Piped stdout is agent mode, so the refusal may land on either channel.
    let text = format!("{stdout}{stderr}");
    assert!(text.contains("the cap is 1"), "{text}");
    assert!(
        !text.contains("manual continuation"),
        "a cap refusal set nothing up to continue: {text}"
    );
    let mut after = snapshot(&fx);
    // Admission receipts are the only permitted refusal write. Validate them
    // before excluding them from the setup-state comparison. trace:STORY-2690 | ai:codex
    let receipts: Vec<_> = after
        .keys()
        .filter(|path| {
            path.contains("/.aida/lane-results/") || path.starts_with(".aida/lane-results/")
        })
        .cloned()
        .collect();
    assert_eq!(receipts.len(), 1, "one admission receipt: {receipts:?}");
    for path in receipts {
        let receipt: serde_json::Value =
            serde_json::from_slice(&after.remove(&path).unwrap()).unwrap();
        assert_eq!(receipt["spec"], fx.spec);
        assert_eq!(receipt["result"], "refused-before-start");
        assert_eq!(receipt["started"], false);
    }
    assert_unchanged(&before, &after);
}

/// The full wiring: dispatch to the standalone launcher, the outcome line
/// and its exit code, the launch-authority record naming why a granted
/// advisor's child runs at baseline, and no manual-continuation hint.
// trace:STORY-1662.AC5 | ai:claude
// trace:STORY-1662.AC11 | ai:claude
#[test]
fn punting_child_exits_32_through_queue_work_with_authority_recorded() {
    let fx = fixture(
        "printf '%s' '{\"spec\":\"X\",\"category\":\"design-fork\",\"detail\":\"pick a schema\"}' \
         > \"$AIDA_PUNT_SIGNAL_FILE\"\nexit 0",
        "",
    );
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["reviewer"]);
    let (code, stdout, stderr) = launch(&fx, &grant);
    assert_eq!(code, Some(32), "{stdout}{stderr}");
    assert!(
        stdout.lines().any(|l| l.starts_with("implementer: punted")),
        "{stdout}"
    );
    assert!(
        !format!("{stdout}{stderr}").contains("manual continuation"),
        "an outcome is not a failed setup: {stdout}{stderr}"
    );
    let ledger = std::fs::read_to_string(fx.repo.join(".aida/review-authorship.jsonl")).unwrap();
    assert!(
        ledger.contains("\"grant\":\"baseline\"") && ledger.contains("launcher-did-not-delegate"),
        "{ledger}"
    );
    // The slot is released when the launcher returns.
    let held = std::fs::read_dir(fx.home.join(".aida/standalone-implementers"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        })
        .expect("host-wide slot directory must have been created");
    assert_eq!(held, 0, "the concurrency slot outlived the launch");
}

// trace:STORY-2660 | ai:codex
#[test]
fn host_capacity_doctor_reads_real_host_and_project_config() {
    let fx = fixture("exit 97", "[queue]\nstandalone_implementer_cap = 4\n");
    let observe = || {
        let out = aida(&fx.repo, &fx.home, &fx.bin)
            .args(["doctor", "--lane-capacity", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    let project = observe();
    assert_eq!(project["cap"], 4);
    assert_eq!(project["cap_source"], "project");
    std::fs::create_dir_all(fx.home.join(".aida")).unwrap();
    std::fs::write(
        fx.home.join(".aida/host.toml"),
        "[queue]\nstandalone_implementer_cap = 8\n[lane_host]\nallow_ungranted = true\n",
    )
    .unwrap();
    let host = observe();
    assert_eq!(host["schema_version"], 1);
    assert_eq!(host["cap"], 8);
    assert_eq!(host["cap_source"], "host");
    assert!(host["nproc"].as_u64().unwrap() > 0);
    assert!(host["warnings"][0]
        .as_str()
        .unwrap()
        .contains("Legacy ungranted"));
    assert_eq!(host["occupied"], 0);
    assert_eq!(host["free"], 8);
}

// trace:STORY-2660 | ai:codex
#[test]
fn live_legacy_slot_refuses_before_setup() {
    let fx = fixture("exit 97", "");
    let slots = fx.repo.join(".aida/standalone-implementers");
    std::fs::create_dir_all(&slots).unwrap();
    std::fs::write(
        slots.join("live.json"),
        format!(
            "{{\"pid\":{},\"spec\":\"STORY-OTHER\"}}",
            std::process::id()
        ),
    )
    .unwrap();
    let grant = support::grant_seat(&fx.home, &fx.repo, "advisor", &["reviewer"]);
    let before = snapshot(&fx);
    let (code, stdout, stderr) = launch(&fx, &grant);
    assert_ne!(code, Some(0));
    assert!(format!("{stdout}{stderr}").contains("legacy checkout-local launcher"));
    let mut after = snapshot(&fx);
    // Admission receipts are the only permitted refusal write. Validate them
    // before excluding them from the setup-state comparison. trace:STORY-2690 | ai:codex
    let receipts: Vec<_> = after
        .keys()
        .filter(|path| {
            path.contains("/.aida/lane-results/") || path.starts_with(".aida/lane-results/")
        })
        .cloned()
        .collect();
    assert_eq!(receipts.len(), 1, "one admission receipt: {receipts:?}");
    for path in receipts {
        let receipt: serde_json::Value =
            serde_json::from_slice(&after.remove(&path).unwrap()).unwrap();
        assert_eq!(receipt["spec"], fx.spec);
        assert_eq!(receipt["result"], "refused-before-start");
        assert_eq!(receipt["started"], false);
    }
    assert_unchanged(&before, &after);
}
