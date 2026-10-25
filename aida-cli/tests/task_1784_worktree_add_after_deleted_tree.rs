#![cfg(target_os = "linux")]
//! TASK-1784: `aida worktree add <SPEC>` after the spec's worktree was deleted
//! by hand (`rm -rf`) must end the dead lease and mint a fresh worktree, not
//! refuse with "spec is InProgress but no local lease holds it". A lease that
//! still points somewhere real but is not re-enterable (the project root) is
//! named in the refusal together with `aida session end <id>`.
//!
//! Driven through the real `aida` binary, so the mint runs through the real
//! `session_start` (status bump, lease save, preflight) — the BUG-2143 core
//! test stubbed the mint and used a repo-wide prune.
// trace:TASK-1784 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

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
        "AIDA_SESSION_GRANT",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = test_git::git_command()
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

fn describe(out: &Output) -> String {
    format!(
        "exit {:?}\n--- stderr ---\n{}\n--- stdout ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

/// A distributed AIDA project (pool disabled, so the worktree lands at the
/// deterministic per-spec path) with one Approved spec.
fn project(base_dir: &Path) -> (PathBuf, PathBuf, String) {
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    // A local bare origin: pure-git treats the work branch as the change and
    // refreshes it from origin when the re-mint reuses it.
    let origin = base_dir.join("origin.git");
    git(
        base_dir,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main"]);

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
    assert!(init.status.success(), "aida init: {}", describe(&init));

    let cfg = repo.join(".aida").join("config.toml");
    let body = std::fs::read_to_string(&cfg).expect("read scaffolded config");
    const SCAFFOLD: &str = "[worktree_pool]\nenabled = true\n";
    assert!(
        body.contains(SCAFFOLD),
        "init no longer scaffolds {SCAFFOLD:?}"
    );
    // Pure-git forge: the reused branch must never be looked up on a real
    // forge (the fixture has no origin).
    let patched: String = body
        .replace(SCAFFOLD, "[worktree_pool]\nenabled = false\n")
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("provider =") {
                "provider = \"pure-git\"".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        patched.contains("provider = \"pure-git\""),
        "no [forge] provider line"
    );
    std::fs::write(&cfg, patched).unwrap();

    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(
            "AIDA_SESSION_GRANT",
            support::ensure_seat(&home, &repo, "advisor", &[])
                .expect("fixture repo is initialized"),
        )
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "re-mint me",
        ])
        .output()
        .expect("run aida add");
    assert!(add.status.success(), "aida add: {}", describe(&add));
    let out = String::from_utf8_lossy(&add.stdout);
    let spec = out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("no spec id in:\n{out}"))
        .to_string();
    (repo, home, spec)
}

fn worktree_add(repo: &Path, home: &Path, spec: &str) -> Output {
    aida(repo, home)
        .args(["worktree", "add", spec])
        .output()
        .expect("run aida worktree add")
}

/// Linked worktrees git lists for `repo`, excluding the primary checkout and
/// the store worktree.
fn linked_worktrees(repo: &Path) -> Vec<PathBuf> {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git worktree list");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|p| p != repo && !p.ends_with(".aida-store"))
        .collect()
}

/// Lease files under `.aida/sessions/` whose scope is `spec`.
fn spec_lease_files(repo: &Path, spec: &str) -> Vec<PathBuf> {
    let dir = repo.join(".aida").join("sessions");
    let scope_line = format!("scope = \"{spec}\"");
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                // `<id>.activity.toml` and friends are companions, not leases.
                .filter(|p| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| !s.contains('.'))
                })
                .filter(|p| {
                    std::fs::read_to_string(p)
                        .is_ok_and(|b| b.lines().any(|l| l.trim() == scope_line))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn single_worktree(repo: &Path) -> PathBuf {
    let trees = linked_worktrees(repo);
    assert_eq!(
        trees.len(),
        1,
        "expected one linked worktree, got {trees:?}"
    );
    trees.into_iter().next().unwrap()
}

#[test]
fn worktree_add_after_rm_rf_ends_the_dead_lease_and_mints_fresh() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(&base_dir);

    let first = worktree_add(&repo, &home, &spec);
    assert!(first.status.success(), "first add: {}", describe(&first));
    let tree = single_worktree(&repo);
    // The work was pushed before the tree was deleted.
    git(&tree, &["push", "-q", "origin", "HEAD"]);
    let leases = spec_lease_files(&repo, &spec);
    assert_eq!(leases.len(), 1, "first add saves one lease: {leases:?}");
    let dead_lease = leases[0].clone();

    std::fs::remove_dir_all(&tree).expect("rm -rf the worktree");

    let again = worktree_add(&repo, &home, &spec);
    assert!(
        again.status.success(),
        "re-add after rm -rf must re-mint, not refuse: {}",
        describe(&again)
    );
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(
        !stderr.contains("no local lease holds it"),
        "the misleading refusal is gone: {stderr}"
    );
    assert!(tree.join(".git").exists(), "a real worktree was re-minted");
    assert_eq!(single_worktree(&repo), tree);
    assert!(!dead_lease.exists(), "the dead lease was ended");
    assert_eq!(
        spec_lease_files(&repo, &spec).len(),
        1,
        "exactly one live lease after the re-mint"
    );
}

#[test]
fn worktree_add_names_a_project_root_lease_and_how_to_end_it() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(&base_dir);

    let first = worktree_add(&repo, &home, &spec);
    assert!(first.status.success(), "first add: {}", describe(&first));
    let lease_file = spec_lease_files(&repo, &spec).pop().expect("one lease");
    let lease_id = lease_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap()
        .to_string();

    // Point the lease at the project root — a path that exists but is never
    // re-entered.
    let body = std::fs::read_to_string(&lease_file).unwrap();
    let mut lease: toml::Table = toml::from_str(&body).unwrap();
    assert!(
        lease.contains_key("worktree_path"),
        "lease records a worktree_path"
    );
    lease.insert(
        "worktree_path".to_string(),
        toml::Value::String(repo.to_string_lossy().into_owned()),
    );
    std::fs::write(&lease_file, toml::to_string(&lease).unwrap()).unwrap();

    let again = worktree_add(&repo, &home, &spec);
    assert!(!again.status.success(), "must refuse: {}", describe(&again));
    let stderr = describe(&again);
    let short = &lease_id[..lease_id.len().min(8)];
    assert!(
        stderr.contains(&format!("aida session end {short}")),
        "names the lease and how to end it: {stderr}"
    );
    assert!(
        !stderr.contains("no local lease holds it"),
        "not the misleading refusal: {stderr}"
    );
    assert!(
        lease_file.exists(),
        "a live-path lease is never ended implicitly"
    );
}

/// The common case: the branch was never pushed before the tree was deleted.
/// The re-mint reuses the local-only branch (origin confirms it has no such
/// ref, so there is nothing to refresh from), keeping the unpushed commit, and
/// the dead lease is ended only once that mint succeeded.
#[test]
fn worktree_add_after_rm_rf_of_an_unpushed_branch_re_mints_from_the_local_branch() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(&base_dir);

    let first = worktree_add(&repo, &home, &spec);
    assert!(first.status.success(), "first add: {}", describe(&first));
    let tree = single_worktree(&repo);
    // Local-only work: committed, never pushed.
    git(
        &tree,
        &["commit", "-q", "--allow-empty", "-m", "unpushed work"],
    );
    let dead_lease = spec_lease_files(&repo, &spec).pop().expect("one lease");

    std::fs::remove_dir_all(&tree).expect("rm -rf the worktree");

    let again = worktree_add(&repo, &home, &spec);
    let text = describe(&again);
    assert!(again.status.success(), "re-add must re-mint: {text}");
    assert!(
        !text.contains("no local lease holds it"),
        "never the misleading refusal: {text}"
    );
    assert!(tree.join(".git").exists(), "a real worktree was re-minted");
    let log = test_git::git_command()
        .current_dir(&tree)
        .args(["log", "-1", "--format=%s"])
        .output()
        .expect("git log");
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "unpushed work",
        "the unpushed commit survives the re-mint"
    );
    assert!(!dead_lease.exists(), "the superseded lease was ended");
    assert_eq!(spec_lease_files(&repo, &spec).len(), 1);
}

/// A failed mint must never strand the spec: the dead lease stays until a mint
/// succeeds, so a retry sees the same lease rather than "InProgress but no
/// local lease holds it". The mint is made to fail by an unreachable origin.
#[test]
fn a_failed_re_mint_keeps_the_dead_lease() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize");
    let (repo, home, spec) = project(&base_dir);

    let first = worktree_add(&repo, &home, &spec);
    assert!(first.status.success(), "first add: {}", describe(&first));
    let tree = single_worktree(&repo);
    let dead_lease = spec_lease_files(&repo, &spec).pop().expect("one lease");
    std::fs::remove_dir_all(&tree).expect("rm -rf the worktree");
    let gone = base_dir.join("no-such-origin.git");
    git(
        &repo,
        &["remote", "set-url", "origin", gone.to_str().unwrap()],
    );

    for attempt in 1..=2 {
        let again = worktree_add(&repo, &home, &spec);
        let text = describe(&again);
        assert!(
            !again.status.success(),
            "attempt {attempt} must fail: {text}"
        );
        assert!(
            text.contains("claiming it anyway"),
            "attempt {attempt}: the failure is the mint itself: {text}"
        );
        assert!(
            !text.contains("no local lease holds it"),
            "attempt {attempt}: never the misleading refusal: {text}"
        );
        assert!(
            dead_lease.exists(),
            "attempt {attempt}: a failed mint keeps the dead lease: {text}"
        );
        assert_eq!(
            spec_lease_files(&repo, &spec).len(),
            1,
            "no half-minted lease"
        );
    }
}
