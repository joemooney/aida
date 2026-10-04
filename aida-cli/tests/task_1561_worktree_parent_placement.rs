// BUG-688: gated to Linux like every other binary-driving e2e suite here — the
// nightly macOS/Windows matrix fails `aida init` for reasons undetermined
// without platform access. trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! TASK-1561: a worktree minted by the warm-pool FALLBACK must land under
//! `[worktree_pool] worktree_parent`, and the `--dry-run` preview must print the
//! path the real run creates.
//!
//! Driven through the real `aida` binary against a throwaway repo, because the
//! defect was never in the config reader (BUG-1700 tested that thoroughly) — it
//! was that the fallback CREATION path never asked it. Only an on-disk assertion
//! distinguishes those two.
//!
//! The pool is disabled in the fixture, which is what puts `session_start` in the
//! exact `else` branch a pool MISS takes: both arrive with `worktree_path` still
//! holding `pickup_worktree_path`'s value and both run the same fresh
//! `git worktree add` (lib.rs, the `if let Some(acquired) = pooled` fork). Ending
//! up there by configuration rather than by sabotaging a pool tree makes the test
//! deterministic without changing the code under test.
// trace:TASK-1561 | ai:claude

use std::path::{Path, PathBuf};
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    for k in [
        "AIDA_SESSION_ROLE",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_SESSION_ID",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn is_spec_id(token: &str) -> bool {
    token.split_once('-').is_some_and(|(kind, num)| {
        !kind.is_empty()
            && !num.is_empty()
            && kind.chars().all(|c| c.is_ascii_uppercase())
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// A distributed AIDA project with one Approved spec. `pool_keys` REPLACES the
/// body of the `[worktree_pool]` table `aida init` scaffolds.
///
/// Replaces rather than appends: `aida init` already writes `[worktree_pool]`
/// with `enabled = true`, and a second table of the same name is a TOML
/// duplicate-key error. That made the whole config unreadable, which surfaced as
/// `aida add` failing with EMPTY stderr — so the substitution is asserted below,
/// and a template rename breaks this test loudly instead of silently testing a
/// config that was never applied.
fn project(base_dir: &Path, pool_keys: &str) -> (PathBuf, PathBuf, String) {
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

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
    assert!(
        init.status.success(),
        "aida init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let cfg = repo.join(".aida").join("config.toml");
    let body = std::fs::read_to_string(&cfg).expect("read scaffolded config");
    const SCAFFOLD: &str = "[worktree_pool]\nenabled = true\n";
    assert!(
        body.contains(SCAFFOLD),
        "`aida init` no longer scaffolds {SCAFFOLD:?}; this fixture must be \
         updated or it will test an unapplied config:\n{body}"
    );
    let patched = body.replace(SCAFFOLD, &format!("[worktree_pool]\n{pool_keys}"));
    std::fs::write(&cfg, &patched).unwrap();
    // Prove the config AIDA will read actually parses, so a fixture typo cannot
    // masquerade as a placement failure.
    toml::from_str::<toml::Value>(&patched).expect("patched config must be valid TOML");

    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "add", "--type", "task", "--status", "approved", "--title", "place me",
        ])
        .output()
        .expect("run aida add");
    assert!(
        add.status.success(),
        "aida add failed (exit {:?}):\n--- stderr ---\n{}\n--- stdout ---\n{}",
        add.status.code(),
        String::from_utf8_lossy(&add.stderr),
        String::from_utf8_lossy(&add.stdout)
    );
    let out = String::from_utf8_lossy(&add.stdout);
    let spec = out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("no spec id in:\n{out}"))
        .to_string();
    (repo, home, spec)
}

/// The `worktree:` path the `--dry-run` preview prints.
fn previewed_worktree(repo: &Path, home: &Path, spec: &str) -> String {
    let dry = aida(repo, home)
        .args(["queue", "work", spec, "--dry-run", "--no-pull"])
        .output()
        .expect("run queue work --dry-run");
    let plan = format!(
        "{}{}",
        String::from_utf8_lossy(&dry.stderr),
        String::from_utf8_lossy(&dry.stdout)
    );
    assert!(dry.status.success(), "dry-run exited non-zero:\n{plan}");
    plan.lines()
        .find_map(|l| l.trim().strip_prefix("worktree:"))
        .map(|p| p.trim().to_string())
        .unwrap_or_else(|| panic!("dry-run printed no `worktree:` line:\n{plan}"))
}

/// The worktree `aida queue work` actually registers, read back from git rather
/// than from AIDA's own report — the report is what the defect made plausible.
fn registered_worktrees(repo: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

fn create_worktree(repo: &Path, home: &Path, spec: &str) {
    let run = aida(repo, home)
        .args(["queue", "work", spec, "--no-pull", "--no-launch"])
        .output()
        .expect("run queue work");
    assert!(
        run.status.success(),
        "queue work exited non-zero ({:?}):\n--- stderr ---\n{}\n--- stdout ---\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run.stdout)
    );
}

// AC1 + AC3 — the defect itself. With `worktree_parent` set and the pool not
// serving, the created worktree lands under the configured directory, and the
// preview printed that same path. Asserted on disk and against git's own
// registry, not against AIDA's report.
#[test]
fn fallback_worktree_lands_under_the_configured_parent_and_the_preview_agrees() {
    let base = tempfile::tempdir().expect("tempdir");
    // BUG-671: canonicalize once so the path the test compares matches the one
    // `aida` records internally.
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(
        &base_dir,
        "enabled = false\nworktree_parent = \"../aida-worktrees\"\n",
    );

    let previewed = previewed_worktree(&repo, &home, &spec);
    create_worktree(&repo, &home, &spec);

    let created: Vec<PathBuf> = registered_worktrees(&repo)
        .into_iter()
        .filter(|p| p != &repo && !p.ends_with(".aida-store"))
        .collect();
    assert_eq!(
        created.len(),
        1,
        "expected exactly one new worktree, got {created:?}"
    );
    let created = &created[0];

    let trusted_parent = base_dir.join("aida-worktrees");
    assert!(
        created.starts_with(&trusted_parent),
        "the worktree escaped the configured parent {}: {}",
        trusted_parent.display(),
        created.display()
    );
    assert!(created.is_dir(), "{} is not a directory", created.display());

    // The historical sibling location must be EMPTY — proving the key captured
    // the placement rather than merely being read somewhere.
    let sibling = base_dir.join(format!(
        "repo-{}",
        spec.to_ascii_lowercase().replace('_', "-")
    ));
    assert!(
        !sibling.exists(),
        "a worktree still landed at the old sibling path {}",
        sibling.display()
    );

    // BUG-1628's invariant: preview == reality. A divergence here is the exact
    // defect that consolidated these paths onto one resolver.
    assert_eq!(
        PathBuf::from(&previewed),
        *created,
        "`--dry-run` previewed {previewed} but the real run created {}",
        created.display()
    );
}

// AC2 — the regression that must not happen. With the key unset the worktree
// lands at the historical sibling path, byte for byte, so no existing project's
// layout moves.
#[test]
fn unset_key_still_lands_at_the_historical_sibling_path() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(&base_dir, "enabled = false\n");

    let previewed = previewed_worktree(&repo, &home, &spec);
    create_worktree(&repo, &home, &spec);

    let created: Vec<PathBuf> = registered_worktrees(&repo)
        .into_iter()
        .filter(|p| p != &repo && !p.ends_with(".aida-store"))
        .collect();
    assert_eq!(created.len(), 1, "expected one worktree, got {created:?}");
    let created = &created[0];

    assert_eq!(
        created.parent(),
        Some(base_dir.as_path()),
        "unset worktree_parent must keep the sibling layout: {}",
        created.display()
    );
    assert_eq!(PathBuf::from(&previewed), *created);
}
