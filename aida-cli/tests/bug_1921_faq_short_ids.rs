#![cfg(target_os = "linux")]
//! Regression coverage for BUG-1921: `faq` (and the other types added after
//! the original block-allocation list — `doc`, change requests) got
//! node-aware ids (`FAQ-1-001`) because no block was ever allocated for
//! them, and acquire-time allocation never re-runs on an existing clone.
//!
//! The fixture is an already-acquired per-type node whose block registry
//! predates the type (no FAQ/DOC/CR block). `aida add --type faq` must
//! yield an agreed short `FAQ-N` id directly — no `aida db merge-gate`.
// trace:BUG-1921 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

mod support;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    aida_env(fixture, args, &[])
}

fn aida_env(fixture: &Fixture, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.envs(envs.iter().copied());
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .args(args);
    if let Some(grant) = support::ensure_seat(&fixture.home, &fixture.repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.output().expect("run aida")
}

fn ok(label: &str, out: Output) -> String {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// An initialised, acquired node switched to per-type counters with an
/// empty block registry — the shape of a clone acquired before FAQ, DOC
/// and CR joined the block-allocated set.
fn legacy_per_type_node() -> Fixture {
    node_with(true, "blocks: []\n")
}

/// An initialised node whose block registry is replaced by `blocks_yaml`,
/// with per-type counters when `per_type` (else init's global scope).
fn node_with(per_type: bool, blocks_yaml: &str) -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(git(&repo, &["init", "-q", "-b", "main"]).status.success());
    assert!(git(&repo, &["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&repo, &["config", "user.name", "Faq Ids Test"])
        .status
        .success());
    assert!(git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
        .status
        .success());
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(
        "aida init",
        aida(
            &fixture,
            &[
                "init",
                "--force",
                "--no-skills",
                "--no-hooks",
                "--no-agent-config",
                "--no-roles",
            ],
        ),
    );

    let config_path = fixture.repo.join(".aida").join("config.toml");
    let config = std::fs::read_to_string(&config_path).expect("read config.toml");
    assert!(
        config.contains("counter_scope = \"global\""),
        "fixture expects init to write a global counter scope:\n{config}"
    );
    if per_type {
        std::fs::write(
            &config_path,
            config.replace("counter_scope = \"global\"", "counter_scope = \"per-type\""),
        )
        .expect("write config.toml");
    }

    let store = fixture.repo.join(".aida-store");
    std::fs::write(store.join("registry").join("blocks.yaml"), blocks_yaml)
        .expect("reset blocks.yaml");
    assert!(
        git(&store, &["commit", "-q", "-am", "test: pre-FAQ registry"])
            .status
            .success()
    );
    fixture
}

fn added_id(stdout: &str) -> String {
    let line = stdout
        .lines()
        .find(|l| l.starts_with("Added: "))
        .unwrap_or_else(|| panic!("no `Added:` line in {stdout}"));
    line["Added: ".len()..]
        .split_whitespace()
        .next()
        .expect("id after Added:")
        .to_string()
}

fn is_short(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

#[test]
fn faq_on_an_existing_node_gets_a_short_id_without_merge_gate() {
    let fixture = legacy_per_type_node();

    let first = added_id(&ok(
        "aida add --type faq",
        aida(
            &fixture,
            &["add", "--type", "faq", "--title", "How do I x?"],
        ),
    ));
    let second = added_id(&ok(
        "aida add --type faq",
        aida(
            &fixture,
            &["add", "--type", "faq", "--title", "How do I y?"],
        ),
    ));
    assert_eq!(first, "FAQ-1", "first FAQ must get a short agreed id");
    assert_eq!(second, "FAQ-2", "second FAQ dispenses from the same block");

    // DOC and CR had the same gap.
    for (ty, prefix) in [("doc", "DOC"), ("change-request", "CR")] {
        let id = added_id(&ok(
            "aida add",
            aida(&fixture, &["add", "--type", ty, "--title", "gap check"]),
        ));
        assert!(is_short(&id, prefix), "{ty} got non-short id {id}");
    }

    // The lazily-claimed block is recorded for this node in the registry.
    let blocks = std::fs::read_to_string(
        fixture
            .repo
            .join(".aida-store")
            .join("registry")
            .join("blocks.yaml"),
    )
    .expect("read blocks.yaml");
    assert!(
        blocks.contains("type_prefix: FAQ"),
        "FAQ block not recorded:\n{blocks}"
    );
}

fn is_node_aware(id: &str, prefix: &str) -> bool {
    let Some(rest) = id.strip_prefix(prefix).and_then(|r| r.strip_prefix('-')) else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn blocks_yaml(fixture: &Fixture) -> String {
    std::fs::read_to_string(
        fixture
            .repo
            .join(".aida-store")
            .join("registry")
            .join("blocks.yaml"),
    )
    .expect("read blocks.yaml")
}

fn store_log(fixture: &Fixture) -> String {
    String::from_utf8_lossy(
        &git(
            &fixture.repo.join(".aida-store"),
            &["log", "--format=%s", "-20"],
        )
        .stdout,
    )
    .to_string()
}

/// A first-block claim is best-effort: when it cannot reach the remote (or
/// remote sync is switched off) the add still succeeds with a node-aware
/// id, and no unpushed claim commit is left behind to collide later.
fn assert_offline_add_falls_back(envs: &[(&str, &str)]) {
    let fixture = legacy_per_type_node();
    let store = fixture.repo.join(".aida-store");
    let nowhere = fixture.repo.parent().unwrap().join("nowhere.git");
    assert!(git(
        &store,
        &["remote", "add", "origin", nowhere.to_str().unwrap()]
    )
    .status
    .success());

    let out = aida_env(
        &fixture,
        &["add", "--type", "faq", "--title", "Offline question?"],
        envs,
    );
    let stdout = ok("offline aida add --type faq", out);
    let id = added_id(&stdout);
    assert!(
        is_node_aware(&id, "FAQ"),
        "expected node-aware id, got {id}"
    );

    let blocks = blocks_yaml(&fixture);
    assert!(
        !blocks.contains("type_prefix: FAQ"),
        "unpushed FAQ claim left in blocks.yaml:\n{blocks}"
    );
    let log = store_log(&fixture);
    assert!(
        !log.contains("claim first"),
        "unpushed claim commit left in the store:\n{log}"
    );
}

#[test]
fn unreachable_remote_falls_back_to_node_aware_id() {
    assert_offline_add_falls_back(&[]);
}

#[test]
fn remote_sync_disabled_skips_first_block_claim() {
    assert_offline_add_falls_back(&[("AIDA_ADD_NO_REMOTE_SYNC", "1")]);
}

#[test]
fn excluded_type_keeps_node_aware_id_and_claims_nothing() {
    let fixture = legacy_per_type_node();
    let id = added_id(&ok(
        "aida add --type non-functional",
        aida(
            &fixture,
            &["add", "--type", "non-functional", "--title", "Fast enough"],
        ),
    ));
    assert!(
        is_node_aware(&id, "NFR"),
        "expected node-aware NFR id, got {id}"
    );
    let blocks = blocks_yaml(&fixture);
    assert!(
        !blocks.contains("type_prefix: NFR"),
        "excluded type claimed a block:\n{blocks}"
    );
}

#[test]
fn custom_prefix_claims_nothing() {
    let fixture = legacy_per_type_node();
    let id = added_id(&ok(
        "aida add --prefix SEC",
        aida(
            &fixture,
            &[
                "add",
                "--type",
                "task",
                "--prefix",
                "SEC",
                "--title",
                "Harden it",
            ],
        ),
    ));
    assert!(
        is_node_aware(&id, "SEC"),
        "expected node-aware SEC id, got {id}"
    );
    let blocks = blocks_yaml(&fixture);
    assert!(
        !blocks.contains("type_prefix: SEC"),
        "custom prefix claimed a block:\n{blocks}"
    );
}

#[test]
fn exhausted_block_is_not_reclaimed() {
    let fixture = node_with(
        true,
        "blocks:\n\
         - node_id: '1'\n  owner: redacted@node.invalid\n  hostname: test\n  \
         type_prefix: FAQ\n  range_start: 1\n  range_end: 2\n  next: 3\n  \
         allocated_at: 2026-01-01T00:00:00Z\n",
    );
    let id = added_id(&ok(
        "aida add --type faq",
        aida(
            &fixture,
            &["add", "--type", "faq", "--title", "After roll-off?"],
        ),
    ));
    assert!(
        is_node_aware(&id, "FAQ"),
        "expected node-aware id, got {id}"
    );
    let blocks = blocks_yaml(&fixture);
    assert_eq!(
        blocks.matches("type_prefix: FAQ").count(),
        1,
        "exhausted FAQ block was re-claimed:\n{blocks}"
    );
}

#[test]
fn global_scope_without_shared_block_claims_nothing() {
    let fixture = node_with(false, "blocks: []\n");
    let id = added_id(&ok(
        "aida add --type faq",
        aida(&fixture, &["add", "--type", "faq", "--title", "Global?"]),
    ));
    assert!(
        is_node_aware(&id, "FAQ"),
        "expected node-aware id, got {id}"
    );
    let blocks = blocks_yaml(&fixture);
    assert!(
        !blocks.contains("type_prefix"),
        "global scope claimed a block on add:\n{blocks}"
    );
}

/// The allocator writes blocks.yaml before committing it. When the commit
/// itself fails (here: a store commit-msg hook rejects the claim commit),
/// the claim must not survive on disk to be dispensed from.
#[test]
fn failed_claim_commit_leaves_no_block_behind() {
    let fixture = legacy_per_type_node();
    let store = fixture.repo.join(".aida-store");
    // Fault injection belongs to a test-owned git executable, since project
    // hooks intentionally never run on host store commits. Forward the verified
    // hook pin unchanged and fail exactly the first-block claim commit.
    // trace:BUG-1929 | ai:codex
    let bin = fixture.repo.parent().unwrap().join("claim-git");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = bin.join("claim-refused");
    let shim = bin.join("git");
    std::fs::write(&shim, format!(
        "#!/bin/bash\nis_commit=0\nfor arg in \"$@\"; do [ \"$arg\" = commit ] && is_commit=1; done\nif [ \"$is_commit\" = 1 ]; then\n  for arg in \"$@\"; do\n    case \"$arg\" in *'claim first'*) printf refused > '{}'; echo 'rejected claim' >&2; exit 1;; esac\n  done\nfi\nexec /usr/bin/git \"$@\"\n", marker.display()
    )).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let stdout = ok(
        "aida add --type faq with failing claim commit",
        aida_env(
            &fixture,
            &["add", "--type", "faq", "--title", "Hook said no?"],
            &[("PATH", &path)],
        ),
    );
    assert!(
        marker.exists(),
        "claim-commit fault must actually be exercised"
    );
    let id = added_id(&stdout);
    assert!(
        is_node_aware(&id, "FAQ"),
        "expected node-aware id, got {id}"
    );
    let blocks = blocks_yaml(&fixture);
    assert!(
        !blocks.contains("type_prefix: FAQ"),
        "uncommitted FAQ claim left in blocks.yaml:\n{blocks}"
    );
    let staged = git(&store, &["diff", "--cached", "--name-only"]);
    assert!(
        !String::from_utf8_lossy(&staged.stdout).contains("blocks.yaml"),
        "claim left staged"
    );
}

/// Happy path with a real (bare) remote: the first-block claim is pushed,
/// so the remote's registry records it.
#[test]
fn first_block_claim_is_pushed_to_the_remote() {
    let fixture = legacy_per_type_node();
    let store = fixture.repo.join(".aida-store");
    let bare = fixture.repo.parent().unwrap().join("remote.git");
    assert!(git(
        fixture.repo.parent().unwrap(),
        &["init", "-q", "--bare", bare.to_str().unwrap()]
    )
    .status
    .success());
    let branch =
        String::from_utf8_lossy(&git(&store, &["rev-parse", "--abbrev-ref", "HEAD"]).stdout)
            .trim()
            .to_string();
    assert!(
        git(&store, &["remote", "add", "origin", bare.to_str().unwrap()])
            .status
            .success()
    );
    let pushed = git(&store, &["push", "-q", "-u", "origin", &branch]);
    assert!(
        pushed.status.success(),
        "seed push failed: {}",
        String::from_utf8_lossy(&pushed.stderr)
    );

    let id = added_id(&ok(
        "aida add --type faq",
        aida(&fixture, &["add", "--type", "faq", "--title", "Online?"]),
    ));
    assert_eq!(id, "FAQ-1");

    let remote_blocks = git(&bare, &["show", &format!("{branch}:registry/blocks.yaml")]);
    let remote_blocks = String::from_utf8_lossy(&remote_blocks.stdout);
    assert!(
        remote_blocks.contains("type_prefix: FAQ"),
        "FAQ claim not on the remote:\n{remote_blocks}"
    );
}
