// trace:FR-0227 | ai:claude:high
//! Build script for aida-cli — compiles gRPC client code when the remote
//! feature is enabled, and stamps every build with build time + git SHA so
//! `aida --version` can show "0.4.0 (built 2026-05-03T01:23:45Z, sha abc1234)".
//! Lets `aida upgrade` distinguish two binaries at the same version number.

use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "remote")]
    {
        // Compile proto for client with protox (pure Rust) — no external `protoc`
        // binary; tonic-build generates the client from the descriptor set.
        // trace:FR-0227 | ai:claude
        println!("cargo:rerun-if-changed=../proto/aida.proto");
        let fds = protox::compile(["../proto/aida.proto"], ["../proto"])?;
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .out_dir("src/generated")
            .compile_fds(fds)?;
    }

    // ---- Build-time stamps (EPIC-1-001) -------------------------------------

    // Unix epoch seconds at build time. Formatted at runtime to keep the
    // build-script deps minimal (no chrono in build.rs).
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=AIDA_BUILD_UNIX_TIME={}", now_secs);

    // Short git SHA, or "unknown" if we can't run git or aren't in a repo.
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AIDA_BUILD_GIT_SHA={}", sha);

    // Mark dirty if working tree has uncommitted changes (best-effort).
    let dirty = Command::new("git")
        // `--no-optional-locks`: without it `git status` opportunistically
        // rewrites a racy index (fresh after `git worktree add`), and since
        // the index is a rerun trigger the next build would be Dirty.
        // trace:BUG-1630 | ai:claude
        .args(["--no-optional-locks", "status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!(
        "cargo:rustc-env=AIDA_BUILD_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );

    // Re-run the build script when the git HEAD or the index changes so the
    // SHA and dirty flag stay accurate. (Pure timestamp updates won't trigger
    // a rebuild on their own, which is fine — `cargo build` after a code
    // change will pick up the new timestamp; pristine builds get cached.)
    emit_git_rerun_triggers();

    Ok(())
}

// trace:BUG-1630 | ai:claude
/// Emit `rerun-if-changed` for the files that move when HEAD moves.
///
/// A linked worktree has a `.git` *file* (`gitdir: ...`), so a hard-coded
/// `../.git/HEAD` never exists — and cargo treats a missing watched path as
/// always stale, rerunning this script (and recompiling the crate) on every
/// build. Resolve the real per-worktree git dir and the shared common dir
/// instead, and only ever emit paths that exist. With no git metadata at all
/// (tarball build) nothing is emitted, so cargo's default package-file
/// tracking applies.
fn emit_git_rerun_triggers() {
    let Some((git_dir, common_dir)) = resolve_git_dirs() else {
        return;
    };
    let mut watched: Vec<PathBuf> = Vec::new();

    // Per-worktree state: HEAD, index (dirty flag), and HEAD's reflog, which
    // is appended on every commit/checkout/reset in this worktree even when
    // HEAD is a symref whose own file content never changes.
    let head = git_dir.join("HEAD");
    let reflog = git_dir.join("logs").join("HEAD");
    watched.push(head.clone());
    watched.push(git_dir.join("index"));
    watched.push(reflog.clone());

    // The branch HEAD points to: its loose ref lives in the common dir.
    //
    // `packed-refs` is deliberately NOT watched. The files ref backend never
    // moves a branch by rewriting packed-refs — an update always writes a
    // loose ref — so packed-refs only changes on `pack-refs`/branch deletion
    // anywhere in the repo. In a worktree-per-agent fleet that is constant
    // churn unrelated to this checkout, and watching it would bring back the
    // rebuild-every-time cost this function exists to remove.
    if let Some(refname) = std::fs::read_to_string(&head)
        .ok()
        .and_then(|h| h.trim().strip_prefix("ref:").map(|r| r.trim().to_string()))
    {
        let loose = common_dir.join(&refname);
        if loose.is_file() {
            watched.push(loose);
        } else if !reflog.exists() {
            // Packed ref and no reflog to catch the next commit: watch the
            // nearest existing ancestor of where the loose ref will be created
            // (for a nested `a/b` whose `refs/heads/a` doesn't exist yet, its
            // creation bumps `refs/heads`). Spurious reruns are safe; a missed
            // stamp update is not.
            // Never climb above `refs/`: a recursive watch of the whole git
            // dir (objects, logs) would rerun on nearly every git operation.
            let refs_root = common_dir.join("refs");
            if let Some(dir) = loose
                .ancestors()
                .skip(1)
                .take_while(|d| d.starts_with(&refs_root))
                .find(|d| d.is_dir())
            {
                watched.push(dir.to_path_buf());
            }
        }
    }

    for path in watched {
        // Never emit a path that doesn't exist: that is exactly the
        // rerun-every-build bug this function exists to avoid.
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// Resolve `(git_dir, common_dir)` for the checkout containing this crate.
/// Prefers `git rev-parse`; falls back to parsing `.git` / `commondir` by
/// hand when the git binary is unavailable. Returns `None` outside a repo.
fn resolve_git_dirs() -> Option<(PathBuf, PathBuf)> {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);

    let rev_parse = |arg: &str| -> Option<PathBuf> {
        let out = Command::new("git")
            .current_dir(&manifest_dir)
            .args(["rev-parse", arg])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let s = String::from_utf8(out.stdout).ok()?;
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // `--git-common-dir` may be relative to the working directory.
        Some(manifest_dir.join(s))
    };
    if let (Some(git_dir), Some(common_dir)) = (
        rev_parse("--absolute-git-dir"),
        rev_parse("--git-common-dir"),
    ) {
        return Some((git_dir, common_dir));
    }

    // Manual fallback: walk up to the first `.git` entry.
    let dot_git = manifest_dir
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let contents = std::fs::read_to_string(&dot_git).ok()?;
        let target = contents.trim().strip_prefix("gitdir:")?.trim();
        dot_git.parent()?.join(target)
    };
    let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|c| git_dir.join(c.trim()))
        .unwrap_or_else(|| git_dir.clone());
    Some((git_dir, common_dir))
}
