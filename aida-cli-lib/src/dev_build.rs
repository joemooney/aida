//! `aida dev build` and `aida dev rollback`: the single dev-binary pipeline.
//!
//! Build -> smoke test -> promote. The promoted pair (`aida`, `aida-server`)
//! lives in a directory OUTSIDE `target/` (default
//! `~/.local/share/aida/live/`), so `cargo clean`, a raw `cargo build` or
//! `make build-fast` can never change or remove the binary every session runs.
//! The previous pair is kept in `live/prev/` for a one-step rollback.
//!
//! Every side-effecting step that would touch the real toolchain or host
//! (git probe, live-wave guard, cargo build, `cargo clean`, smoke test, MCP
//! restart, the link confirmation prompt) goes through [`BuildSteps`], so the
//! pipeline is tested with fakes and temp directories; the filesystem moves
//! (promote, rollback, `--clean`, link install) take explicit paths in a
//! [`DevLayout`] and are tested against temp dirs directly.
// trace:STORY-1646 | ai:claude

use crate::cli::DevCleanMode;
use crate::process_retry::RetryEtxtbsy;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The binaries that make up one live build, promoted and rolled back as a pair.
pub(crate) const LIVE_BINARIES: [&str; 2] = ["aida", "aida-server"];

/// Cargo profile `aida dev build` compiles (same as `make build-fast`).
const PROFILE: &str = "agent";

/// Remedy appended to every build / smoke-test failure.
const CLEAN_HINT: &str = "if this looks like incremental-cache trouble, retry with \
     `aida dev build --clean` (or `aida dev build --clean=full` to rebuild dependencies too)";

/// Where the pipeline reads builds from and writes the live pair to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevLayout {
    /// The AIDA checkout being built.
    pub repo: PathBuf,
    /// Cargo's target dir for `repo` (`$CARGO_TARGET_DIR` or `<repo>/target`).
    pub target_dir: PathBuf,
    /// The live directory the PATH links point at.
    pub live_dir: PathBuf,
    /// Where `--install-links` puts the `aida` / `aida-server` links.
    pub link_dir: PathBuf,
}

impl DevLayout {
    /// Layout for `repo` with the default (or env-overridden) live and link dirs.
    pub(crate) fn for_repo(repo: &Path) -> Result<Self> {
        let target_dir = env_path("CARGO_TARGET_DIR")
            .map(|p| if p.is_absolute() { p } else { repo.join(p) })
            .unwrap_or_else(|| repo.join("target"));
        let layout = Self {
            repo: repo.to_path_buf(),
            target_dir,
            live_dir: default_live_dir()?,
            link_dir: default_link_dir()?,
        };
        layout.check_live_outside_target()?;
        Ok(layout)
    }

    /// Refuse a live dir inside the target dir: `cargo clean` would delete
    /// the live pair, which is the whole point of keeping it separate.
    // trace:STORY-1646 | ai:claude
    pub(crate) fn check_live_outside_target(&self) -> Result<()> {
        let target = canonical_or_self(&self.target_dir);
        let live = canonical_or_self(&self.live_dir);
        if live.starts_with(&target) {
            bail!(
                "the live dir {} is inside cargo's target dir {}, where `cargo clean` would delete \
                 the live binaries. Point AIDA_DEV_LIVE_DIR somewhere outside it (the default is \
                 ~/.local/share/aida/live). Nothing was changed.",
                self.live_dir.display(),
                self.target_dir.display()
            );
        }
        Ok(())
    }

    pub(crate) fn built(&self, name: &str) -> PathBuf {
        self.target_dir.join(PROFILE).join(name)
    }

    pub(crate) fn live(&self, name: &str) -> PathBuf {
        self.live_dir.join(name)
    }

    pub(crate) fn prev_dir(&self) -> PathBuf {
        self.live_dir.join("prev")
    }

    pub(crate) fn prev(&self, name: &str) -> PathBuf {
        self.prev_dir().join(name)
    }

    pub(crate) fn incremental_dir(&self) -> PathBuf {
        self.target_dir.join(PROFILE).join("incremental")
    }

    pub(crate) fn link(&self, name: &str) -> PathBuf {
        self.link_dir.join(name)
    }
}

/// `path` canonicalized; when it does not exist yet, its nearest existing
/// ancestor canonicalized with the remaining components re-appended.
// trace:STORY-1646 | ai:claude
fn canonical_or_self(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(real) = cur.canonicalize() {
            return tail.iter().rev().fold(real, |acc, c| acc.join(c));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `$AIDA_DEV_LIVE_DIR`, else `~/.local/share/aida/live`.
pub(crate) fn default_live_dir() -> Result<PathBuf> {
    if let Some(dir) = env_path("AIDA_DEV_LIVE_DIR") {
        return Ok(dir);
    }
    let home = crate::home_dir()
        .context("cannot locate the home directory for the live dir; set AIDA_DEV_LIVE_DIR")?;
    Ok(home.join(".local").join("share").join("aida").join("live"))
}

/// `$AIDA_DEV_LINK_DIR`, else `~/.local/bin`.
pub(crate) fn default_link_dir() -> Result<PathBuf> {
    if let Some(dir) = env_path("AIDA_DEV_LINK_DIR") {
        return Ok(dir);
    }
    let home = crate::home_dir()
        .context("cannot locate the home directory for the link dir; set AIDA_DEV_LINK_DIR")?;
    Ok(home.join(".local").join("bin"))
}

// ---------------------------------------------------------------------------
// Promotion gate
// ---------------------------------------------------------------------------

/// What git says about the checkout being built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CheckoutState {
    /// A `git worktree add` checkout rather than the primary one.
    pub is_linked_worktree: bool,
    /// Current branch, `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Paths with tracked modifications (staged or not).
    pub tracked_changes: Vec<String>,
    /// Count of untracked (non-ignored) files.
    pub untracked: usize,
    /// Whether `refs/remotes/origin/main` exists.
    pub has_origin_main: bool,
    /// Commits in HEAD that origin/main lacks.
    pub ahead: usize,
    /// Commits in origin/main that HEAD lacks.
    pub behind: usize,
    /// `git rev-parse HEAD`, when readable.
    // trace:STORY-1646 | ai:claude
    pub head: Option<String>,
}

/// Why a build stops after the smoke test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildOnlyReason {
    /// `--no-promote` was passed.
    NoPromote,
    /// Built in a linked worktree; worktree builds never promote.
    Worktree,
}

/// The promotion decision, made before building and re-checked before promoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromotionPlan {
    Promote { warnings: Vec<String> },
    BuildOnly(BuildOnlyReason),
    Refuse { reasons: Vec<String> },
}

/// Pure promotion gate: primary checkout, on `main`, no tracked
/// modifications, HEAD == origin/main. Untracked files only warn.
/// `--allow-dirty` turns refusals into warnings; it never makes a worktree
/// build promote.
pub(crate) fn plan_promotion(
    state: &CheckoutState,
    no_promote: bool,
    allow_dirty: bool,
) -> PromotionPlan {
    if no_promote {
        return PromotionPlan::BuildOnly(BuildOnlyReason::NoPromote);
    }
    if state.is_linked_worktree {
        return PromotionPlan::BuildOnly(BuildOnlyReason::Worktree);
    }
    let mut reasons = Vec::new();
    match state.branch.as_deref() {
        Some("main") => {}
        Some(other) => reasons.push(format!("on branch `{other}`, not `main`")),
        None => reasons.push("HEAD is detached, not on `main`".to_string()),
    }
    if !state.tracked_changes.is_empty() {
        let shown: Vec<&str> = state
            .tracked_changes
            .iter()
            .take(5)
            .map(String::as_str)
            .collect();
        let more = state.tracked_changes.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        };
        reasons.push(format!(
            "{} tracked file(s) have uncommitted changes: {}{tail}",
            state.tracked_changes.len(),
            shown.join(", ")
        ));
    }
    if !state.has_origin_main {
        reasons.push("there is no `origin/main` to compare HEAD against".to_string());
    } else if state.ahead > 0 && state.behind > 0 {
        reasons.push(format!(
            "HEAD has diverged from origin/main ({} ahead, {} behind)",
            state.ahead, state.behind
        ));
    } else if state.behind > 0 {
        reasons.push(format!(
            "HEAD is {} commit(s) behind origin/main",
            state.behind
        ));
    } else if state.ahead > 0 {
        reasons.push(format!(
            "HEAD is {} commit(s) ahead of origin/main (unpushed work)",
            state.ahead
        ));
    }
    let mut warnings = Vec::new();
    if state.untracked > 0 {
        warnings.push(format!(
            "{} untracked file(s) in the checkout (not blocking)",
            state.untracked
        ));
    }
    if reasons.is_empty() {
        return PromotionPlan::Promote { warnings };
    }
    if allow_dirty {
        warnings.extend(
            reasons
                .into_iter()
                .map(|r| format!("--allow-dirty: promoting anyway, {r}")),
        );
        return PromotionPlan::Promote { warnings };
    }
    PromotionPlan::Refuse { reasons }
}

fn refusal_message(reasons: &[String]) -> String {
    let mut msg = String::from(
        "refusing to promote: the live binary is only built from a clean `main` that matches `origin/main`.\n",
    );
    for r in reasons {
        msg.push_str(&format!("  - {r}\n"));
    }
    msg.push_str(
        "Nothing was built and the live binaries are untouched.\n\
         HEAD is compared with your local `origin/main` ref; this command does not fetch, \
         so run `git fetch` first if that ref may be stale.\n\
         Options: commit/push or sync with `origin/main` and retry; `aida dev build --no-promote` \
         to build and smoke-test only; or `aida dev build --allow-dirty` to promote anyway.",
    );
    msg
}

fn git_out(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        // A hook-exported GIT_DIR would silently redirect `-C`.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn git_path(repo: &Path, arg: &str) -> Option<PathBuf> {
    let raw = PathBuf::from(git_out(repo, &["rev-parse", arg])?);
    let abs = if raw.is_absolute() {
        raw
    } else {
        repo.join(raw)
    };
    abs.canonicalize().ok()
}

/// Read the checkout state from git (local refs only; no fetch).
pub(crate) fn probe_checkout(repo: &Path) -> Result<CheckoutState> {
    let git_dir = git_path(repo, "--git-dir")
        .with_context(|| format!("{} is not a git checkout", repo.display()))?;
    let common_dir = git_path(repo, "--git-common-dir").unwrap_or_else(|| git_dir.clone());
    let branch =
        git_out(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).filter(|b| !b.is_empty());
    let status = git_out(
        repo,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .context("`git status` failed")?;
    let mut tracked_changes = Vec::new();
    let mut untracked = 0;
    for line in status.lines().filter(|l| l.len() > 3) {
        if line.starts_with("??") {
            untracked += 1;
        } else if !line.starts_with("!!") {
            tracked_changes.push(line[3..].to_string());
        }
    }
    let has_origin_main = git_out(
        repo,
        &["rev-parse", "--verify", "-q", "refs/remotes/origin/main"],
    )
    .is_some();
    let (mut ahead, mut behind) = (0, 0);
    if has_origin_main {
        if let Some(counts) = git_out(
            repo,
            &[
                "rev-list",
                "--left-right",
                "--count",
                "HEAD...refs/remotes/origin/main",
            ],
        ) {
            let mut it = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
            ahead = it.next().unwrap_or(0);
            behind = it.next().unwrap_or(0);
        }
    }
    Ok(CheckoutState {
        is_linked_worktree: git_dir != common_dir,
        branch,
        tracked_changes,
        untracked,
        has_origin_main,
        ahead,
        behind,
        head: git_out(repo, &["rev-parse", "HEAD"]).filter(|h| !h.is_empty()),
    })
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// The side-effecting steps of the pipeline. [`RealSteps`] runs the real
/// toolchain; tests substitute fakes so no unit test runs a cargo build or
/// touches the host's live binaries or MCP servers.
pub(crate) trait BuildSteps {
    fn probe(&mut self, repo: &Path) -> Result<CheckoutState>;
    /// Live-wave guard: refuse (or wait, with `after_wave`) when a live drain
    /// wave is pinned to `binary`, which this run would replace.
    fn guard(&mut self, repo: &Path, binary: &Path, after_wave: bool) -> Result<()>;
    /// Where `aida` on `$PATH` currently resolves to, if anywhere.
    fn path_aida(&mut self) -> Option<PathBuf>;
    /// `cargo clean --profile agent`.
    fn clean_full(&mut self, layout: &DevLayout) -> Result<()>;
    /// Incremental `--profile agent` workspace build.
    fn build(&mut self, layout: &DevLayout) -> Result<()>;
    /// Run `aida` / `server` (in `repo`); `Err` names what failed. When
    /// promoting these are the staged copies that will be renamed live.
    fn smoke(&mut self, repo: &Path, aida: &Path, server: &Path) -> Result<()>;
    /// The `sha ...` token of `aida --version` (may end in `+dirty`).
    // trace:STORY-1646 | ai:claude
    fn version_sha(&mut self, aida: &Path) -> Option<String>;
    /// Restart MCP servers still running a replaced binary.
    fn restart_mcp(&mut self, layout: &DevLayout) -> Result<()>;
    /// Ask before replacing existing files at the link location.
    fn confirm(&mut self, prompt: &str) -> bool;
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DevBuildOptions {
    pub clean: Option<DevCleanMode>,
    pub no_promote: bool,
    pub allow_dirty: bool,
    pub install_links: bool,
    pub yes: bool,
    pub after_wave: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildOutcome {
    Promoted,
    BuiltOnly(BuildOnlyReason),
}

/// Run build -> smoke -> (promote) for `layout`. A failure at any step before
/// the promote leaves the live directory untouched and returns `Err` naming
/// the step.
pub(crate) fn run_dev_build(
    layout: &DevLayout,
    opts: &DevBuildOptions,
    steps: &mut dyn BuildSteps,
    out: &mut dyn Write,
) -> Result<BuildOutcome> {
    let state = steps.probe(&layout.repo)?;
    let plan = plan_promotion(&state, opts.no_promote, opts.allow_dirty);
    match &plan {
        PromotionPlan::Refuse { reasons } => bail!("{}", refusal_message(reasons)),
        PromotionPlan::Promote { warnings } => {
            for w in warnings {
                writeln!(out, "warning: {w}")?;
            }
        }
        PromotionPlan::BuildOnly(_) => {}
    }
    let promoting = matches!(plan, PromotionPlan::Promote { .. });
    steps.guard(&layout.repo, &layout.built("aida"), opts.after_wave)?;
    if promoting {
        steps.guard(&layout.repo, &layout.live("aida"), opts.after_wave)?;
    }

    // A PATH binary that still resolves into this target dir is the binary
    // every session runs: cargo overwrites it before any smoke test, and a
    // full clean would delete it outright.
    let mut candidates: Vec<PathBuf> = LIVE_BINARIES.iter().map(|n| layout.link(n)).collect();
    candidates.extend(steps.path_aida());
    let exposed = live_binaries_in_target(layout, &candidates);
    if !exposed.is_empty() {
        let list = exposed
            .iter()
            .map(|(link, real)| format!("  {} -> {}", link.display(), real.display()))
            .collect::<Vec<_>>()
            .join("\n");
        if opts.clean == Some(DevCleanMode::Full) {
            bail!(
                "refusing --clean=full: the binary on your PATH lives in this build's target dir, \
                 and `cargo clean` would delete it for every session until the build finishes \
                 (or for good if the build fails):\n{list}\n\
                 Run `aida dev build --install-links` first (from a clean `main` in the primary \
                 checkout) so the PATH points at the live directory, then retry. Nothing was changed."
            );
        }
        writeln!(
            out,
            "warning: the binary on your PATH lives in this build's target dir, so cargo replaces \
             it before the smoke test runs:\n{list}\n\
             Run `aida dev build --install-links` to point the PATH at the live directory instead."
        )?;
    }

    match opts.clean {
        Some(DevCleanMode::Incremental) => {
            let removed = clean_incremental(layout)?;
            writeln!(
                out,
                "--clean: {} {}",
                if removed {
                    "deleted"
                } else {
                    "nothing to delete at"
                },
                layout.incremental_dir().display()
            )?;
        }
        Some(DevCleanMode::Full) => {
            writeln!(out, "--clean=full: cargo clean --profile {PROFILE}")?;
            steps
                .clean_full(layout)
                .map_err(|e| anyhow!("`cargo clean --profile {PROFILE}` failed: {e:#}"))?;
        }
        None => {}
    }

    steps.build(layout).map_err(|e| {
        anyhow!("build failed: {e:#}\nThe live binaries were not touched.\nHint: {CLEAN_HINT}")
    })?;
    for name in LIVE_BINARIES {
        let built = layout.built(name);
        if !built.is_file() {
            bail!(
                "build finished but {} is missing.\nThe live binaries were not touched.\nHint: {CLEAN_HINT}",
                built.display()
            );
        }
    }
    let smoke_failed = |e: anyhow::Error| {
        anyhow!(
            "smoke test of the new build failed: {e:#}\nThe live binaries were not touched.\nHint: {CLEAN_HINT}"
        )
    };

    let outcome = match plan {
        PromotionPlan::BuildOnly(reason) => {
            steps
                .smoke(
                    &layout.repo,
                    &layout.built("aida"),
                    &layout.built("aida-server"),
                )
                .map_err(smoke_failed)?;
            writeln!(out, "Smoke test passed: {}", layout.built("aida").display())?;
            match reason {
                BuildOnlyReason::Worktree => writeln!(
                    out,
                    "Not promoted: this is a worktree, and worktree builds never replace the live binary.\n\
                     Try this build in your shell: cd {} && aida dev activate auto\n\
                     Or run it directly: {}",
                    layout.repo.display(),
                    layout.built("aida").display()
                )?,
                BuildOnlyReason::NoPromote => writeln!(
                    out,
                    "Not promoted (--no-promote). The build is at {}",
                    layout.built("aida").display()
                )?,
            }
            BuildOutcome::BuiltOnly(reason)
        }
        PromotionPlan::Promote { .. } => {
            // Everything from staging to the final rename happens under the
            // live-dir lock, and the smoke test runs against the staged
            // copies, so the pair that goes live is exactly the pair that
            // was tested even if another build rewrites target/ meanwhile.
            let lock = lock_live_dir(layout)?;
            for note in recover_live_dir(layout)? {
                writeln!(out, "note: {note}")?;
            }
            let incoming = stage_incoming(layout)?;
            let smoked = steps.smoke(
                &layout.repo,
                &incoming.join("aida"),
                &incoming.join("aida-server"),
            );
            if let Err(e) = smoked {
                let _ = std::fs::remove_dir_all(&incoming);
                return Err(smoke_failed(e));
            }
            writeln!(
                out,
                "Smoke test passed: {}",
                incoming.join("aida").display()
            )?;
            // The build and smoke test can take minutes; re-check that the
            // checkout still qualifies before replacing the live pair.
            let state = steps.probe(&layout.repo)?;
            if let PromotionPlan::Refuse { reasons } =
                plan_promotion(&state, false, opts.allow_dirty)
            {
                let _ = std::fs::remove_dir_all(&incoming);
                bail!(
                    "the checkout changed during the build, so the new build was not promoted.\n{}",
                    refusal_message(&reasons)
                );
            }
            // trace:STORY-1646 | ai:claude
            let head = state.head.clone().unwrap_or_default();
            if !opts.allow_dirty {
                let staged = steps.version_sha(&incoming.join("aida"));
                if let Err(why) = check_staged_sha(staged.as_deref(), &head) {
                    let _ = std::fs::remove_dir_all(&incoming);
                    bail!(
                        "the staged build does not match HEAD, so it was not promoted: {why}.\n\
                         The live binaries were not touched. Re-run `aida dev build` (or pass \
                         --allow-dirty to promote it anyway)."
                    );
                }
            }
            let meta = PromotionMeta {
                sha: head,
                allow_dirty: opts.allow_dirty
                    && matches!(
                        plan_promotion(&state, false, false),
                        PromotionPlan::Refuse { .. }
                    ),
                promoted_at: chrono::Utc::now().to_rfc3339(),
            };
            commit_staged(layout, &meta)?;
            drop(lock);
            writeln!(
                out,
                "Promoted to {} (previous build kept in {}; `aida dev rollback` restores it)",
                layout.live_dir.display(),
                layout.prev_dir().display()
            )?;
            if let Err(e) = steps.restart_mcp(layout) {
                writeln!(
                    out,
                    "warning: could not restart stale MCP servers ({e:#}); they respawn on their next \
                     request, or restart your MCP client"
                )?;
            }
            BuildOutcome::Promoted
        }
        PromotionPlan::Refuse { .. } => unreachable!("refusal returned above"),
    };

    if opts.install_links {
        // trace:STORY-1646 | ai:claude
        install_links(layout, opts.yes, steps, out).map_err(|e| {
            if outcome == BuildOutcome::Promoted {
                anyhow!(
                    "{e:#}\nThe build WAS promoted to {}; only the PATH links were left alone. \
                     Run `aida dev build --install-links --yes` to replace them.",
                    layout.live_dir.display()
                )
            } else {
                e
            }
        })?;
    }
    Ok(outcome)
}

/// The `candidates` (PATH entries) that resolve to a file inside
/// `layout.target_dir`, as `(candidate, resolved)` pairs.
pub(crate) fn live_binaries_in_target(
    layout: &DevLayout,
    candidates: &[PathBuf],
) -> Vec<(PathBuf, PathBuf)> {
    let target = layout
        .target_dir
        .canonicalize()
        .unwrap_or_else(|_| layout.target_dir.clone());
    let mut found: Vec<(PathBuf, PathBuf)> = Vec::new();
    for cand in candidates {
        let Ok(real) = cand.canonicalize() else {
            continue;
        };
        if real.starts_with(&target) && !found.iter().any(|(_, r)| *r == real) {
            found.push((cand.clone(), real));
        }
    }
    found
}

/// `--clean`: delete `target/agent/incremental` only. Returns whether it existed.
pub(crate) fn clean_incremental(layout: &DevLayout) -> Result<bool> {
    let dir = layout.incremental_dir();
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("deleting {}", dir.display()))?;
    Ok(true)
}

/// The `cargo clean --profile agent` command `--clean=full` runs, scoped to
/// `layout`'s manifest and target dir.
pub(crate) fn full_clean_command(layout: &DevLayout) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&layout.repo)
        .arg("clean")
        .arg("--profile")
        .arg(PROFILE)
        .arg("--manifest-path")
        .arg(layout.repo.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&layout.target_dir);
    cmd
}

fn run_checked(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("could not run {what}"))?;
    if !status.success() {
        bail!("{what} exited with {status}");
    }
    Ok(())
}

/// Hard-link `from` to `to` (cheap, same inode), falling back to a copy.
fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn fresh_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}

/// Exclusive lock on `<live>/.lock`, held across staging, the smoke test,
/// prev rotation and the final renames (and across a rollback). A second
/// promote or rollback refuses instead of interleaving with this one.
pub(crate) fn lock_live_dir(layout: &DevLayout) -> Result<std::fs::File> {
    use fs2::FileExt;
    let live = &layout.live_dir;
    std::fs::create_dir_all(live).with_context(|| format!("creating {}", live.display()))?;
    let path = live.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // trace:STORY-1646 | ai:claude
    // Retry briefly: a child forked by this process (any thread) shares the
    // lock's file description until it execs, which reads as "held" for a
    // few microseconds even though no other promote is running.
    let deadline = Instant::now() + Duration::from_millis(500);
    while file.try_lock_exclusive().is_err() {
        if Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        bail!(
            "another `aida dev build` or `aida dev rollback` is updating {} right now; \
             retry when it finishes. Nothing was changed.",
            live.display()
        );
    }
    Ok(file)
}

/// Finish or undo whatever an interrupted promote or rollback left behind.
/// Must run under [`lock_live_dir`]. Returns a note per repair made.
///
/// Repaired: a promote that died between moving `prev/` aside and moving
/// the new `prev/` in (the old `prev/` is restored); a rollback that died
/// mid-swap (the parked live binary is moved into `prev/`); leftover staging
/// dirs. Not repairable here: a crash between the two per-binary renames
/// leaves a mixed live pair (new `aida`, old `aida-server` or the reverse);
/// re-run `aida dev build` to fix that. A rollback that crashes between its
/// two binary swaps leaves a mixed pair the same way, and its `.promoted`
/// records follow `aida`. Running `aida dev rollback` again only swaps that
/// mixed pair with the equally mixed `prev/`; `aida dev build` is the fix.
/// If an old-format rollback crashed after the first binary swap while live had no
/// record, there is no `.rollback/.promoted`, so prev's record stays beside
/// the wrong pair.
pub(crate) fn recover_live_dir(layout: &DevLayout) -> Result<Vec<String>> {
    recover_live_dir_with_steps(layout, &mut |_| Ok(()))
}

pub(crate) fn recover_live_dir_with_steps(
    layout: &DevLayout,
    step: &mut dyn FnMut(&'static str) -> Result<()>,
) -> Result<Vec<String>> {
    let live = &layout.live_dir;
    let mut notes = Vec::new();
    let prev_old = live.join(".prev-old");
    if prev_old.exists() {
        if layout.prev_dir().exists() {
            std::fs::remove_dir_all(&prev_old)?;
        } else {
            std::fs::rename(&prev_old, layout.prev_dir())?;
            notes.push(format!(
                "restored {} after an interrupted promote",
                layout.prev_dir().display()
            ));
        }
    }
    let swap = live.join(".rollback");
    if swap.exists() {
        // trace:STORY-1646 | ai:claude
        // trace:BUG-1959 | ai:claude
        // trace:BUG-1999 | ai:antigravity
        // In STORY-1646 an older binary parked the live record directly at
        // `.rollback/.promoted` without writing `.swapping` or parking prev's
        // record. Normalize it into the current layout: write the marker first,
        // park prev's record when live has none, and move `.rollback/.promoted` last.
        let old_promoted = swap.join(PROMOTED_FILE);
        let had_old_promoted = old_promoted.is_file();
        if had_old_promoted {
            std::fs::File::create(swap.join(ROLLBACK_MARKER))?;
            step("marker")?;
            if !layout.live_dir.join(PROMOTED_FILE).exists() {
                move_record(
                    &layout.prev_dir().join(PROMOTED_FILE),
                    &swap.join(PARKED_PREV_RECORD),
                )?;
                step("parked_prev")?;
            }
            move_record(&old_promoted, &swap.join(PARKED_LIVE_RECORD))?;
            step("parked_live")?;
        }
        // A binary counts as swapped once its prev slot has moved live, or,
        // after parking finished, once its parked copy has moved on into
        // `prev/`. The `.swapping` marker notes that parking completed before
        // any binary swaps began (or was normalized from an older format above).
        // Both `.promoted` records were parked before the marker, so settling
        // them (forward when swapped, back otherwise) only ever moves files
        // out of `.rollback/` and is a no-op once done.
        let marked = swap.join(ROLLBACK_MARKER).exists();
        let swapped = LIVE_BINARIES
            .iter()
            .any(|n| !layout.prev(n).exists() || (marked && !swap.join(n).is_file()));
        settle_records(layout, &swap, swapped, &mut |_| Ok(()))?;
        for name in LIVE_BINARIES {
            let parked = swap.join(name);
            if parked.is_file() && !layout.prev(name).exists() {
                std::fs::create_dir_all(layout.prev_dir())?;
                std::fs::rename(&parked, layout.prev(name))?;
                notes.push(format!(
                    "finished an interrupted rollback of {}",
                    layout.live(name).display()
                ));
            }
        }
        // Marker first: a partly deleted `.rollback/` must never look like
        // "parking finished, binaries gone".
        let marker = swap.join(ROLLBACK_MARKER);
        if marker.exists() {
            std::fs::remove_file(&marker)?;
        }
        std::fs::remove_dir_all(&swap)?;
    }
    for leftover in [".prev-new", ".incoming"] {
        let dir = live.join(leftover);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    Ok(notes)
}

/// Copy the built pair into `<live>/.incoming` (same filesystem as the live
/// pair, so the final rename is atomic) and return that dir. Run under
/// [`lock_live_dir`].
pub(crate) fn stage_incoming(layout: &DevLayout) -> Result<PathBuf> {
    let incoming = layout.live_dir.join(".incoming");
    fresh_dir(&incoming)?;
    for name in LIVE_BINARIES {
        std::fs::copy(layout.built(name), incoming.join(name)).with_context(|| {
            format!(
                "staging {} into {}",
                layout.built(name).display(),
                incoming.display()
            )
        })?;
    }
    Ok(incoming)
}

/// Marker `rollback` writes into `.rollback/` once the live pair and both
/// records are parked, before the first binary swap.
// trace:BUG-1959 | ai:claude
pub(crate) const ROLLBACK_MARKER: &str = ".swapping";

/// Where `rollback` parks the live pair's and `prev/`'s `.promoted` records
/// inside `.rollback/` before the marker goes down.
// trace:BUG-1959 | ai:claude
pub(crate) const PARKED_LIVE_RECORD: &str = "live.promoted";
// trace:BUG-1959 | ai:claude
pub(crate) const PARKED_PREV_RECORD: &str = "prev.promoted";

/// Name of the promotion record kept beside each live pair (and in `prev/`).
// trace:STORY-1646 | ai:claude
pub(crate) const PROMOTED_FILE: &str = ".promoted";

/// What `commit_staged` records about a promotion. `aida dev status` uses it,
/// not the `+dirty` in the version banner (which build.rs also sets for
/// untracked files the gate allows), to decide whether to warn.
// trace:STORY-1646 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PromotionMeta {
    /// `git rev-parse HEAD` at promotion.
    pub sha: String,
    /// `--allow-dirty` overrode a gate refusal for this promotion.
    pub allow_dirty: bool,
    /// RFC 3339 promotion time.
    pub promoted_at: String,
}

/// Read a `.promoted` record from `dir` (the live dir or `prev/`).
// trace:STORY-1646 | ai:claude
pub(crate) fn read_promotion_meta(dir: &Path) -> Option<PromotionMeta> {
    let raw = std::fs::read_to_string(dir.join(PROMOTED_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// `Ok` when the staged banner's sha (minus `+dirty`) is a prefix of `head`.
// trace:STORY-1646 | ai:claude
pub(crate) fn check_staged_sha(
    staged: Option<&str>,
    head: &str,
) -> std::result::Result<(), String> {
    let Some(staged) = staged else {
        return Err("could not read a sha from the staged `aida --version`".into());
    };
    if head.is_empty() {
        return Err("could not read HEAD with `git rev-parse HEAD`".into());
    }
    let sha = staged.trim_end_matches("+dirty");
    if sha.len() < 7 || !head.starts_with(sha) {
        return Err(format!(
            "the staged `aida --version` reports sha {staged}, but HEAD is {head}"
        ));
    }
    Ok(())
}

/// Rotate the live pair into `live/prev/` and rename the staged pair live.
/// Each binary is replaced with a single `rename`, so `live/aida` never
/// disappears; the outgoing pair is hard-linked into a fresh `prev/` first.
/// Run under [`lock_live_dir`] after [`stage_incoming`].
pub(crate) fn commit_staged(layout: &DevLayout, meta: &PromotionMeta) -> Result<()> {
    let live = &layout.live_dir;
    let incoming = live.join(".incoming");
    // trace:STORY-1646 | ai:claude
    let record = serde_json::to_string_pretty(meta)?;
    std::fs::write(incoming.join(PROMOTED_FILE), record)
        .with_context(|| format!("writing {}", incoming.join(PROMOTED_FILE).display()))?;
    if LIVE_BINARIES.iter().any(|n| layout.live(n).is_file()) {
        let prev_new = live.join(".prev-new");
        fresh_dir(&prev_new)?;
        for name in LIVE_BINARIES {
            if layout.live(name).is_file() {
                link_or_copy(&layout.live(name), &prev_new.join(name))?;
            }
        }
        // The outgoing pair's record goes with it, so rollback can swap it back.
        let live_meta = live.join(PROMOTED_FILE);
        if live_meta.is_file() {
            link_or_copy(&live_meta, &prev_new.join(PROMOTED_FILE))?;
        }
        let prev_old = live.join(".prev-old");
        if prev_old.exists() {
            std::fs::remove_dir_all(&prev_old)?;
        }
        if layout.prev_dir().exists() {
            std::fs::rename(layout.prev_dir(), &prev_old)?;
        }
        std::fs::rename(&prev_new, layout.prev_dir())?;
        let _ = std::fs::remove_dir_all(&prev_old);
    }
    for name in LIVE_BINARIES {
        std::fs::rename(incoming.join(name), layout.live(name))
            .with_context(|| format!("promoting {}", layout.live(name).display()))?;
    }
    std::fs::rename(incoming.join(PROMOTED_FILE), live.join(PROMOTED_FILE))
        .with_context(|| format!("writing {}", live.join(PROMOTED_FILE).display()))?;
    let _ = std::fs::remove_dir(&incoming);
    Ok(())
}

/// Move a `.promoted` record when it is still at `from` (else a no-op).
// trace:BUG-1959 | ai:claude
fn move_record(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(from, to)
            .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
    }
    Ok(())
}

/// Put the two parked `.promoted` records in place: swapped when the
/// binaries were swapped (each record follows its pair), back where they
/// came from otherwise. Every source is in `swap`, so re-running after a
/// crash is a no-op for the moves already made.
// trace:BUG-1959 | ai:claude
fn settle_records(
    layout: &DevLayout,
    swap: &Path,
    swapped: bool,
    step: &mut dyn FnMut(&'static str) -> Result<()>,
) -> Result<()> {
    let (to_live, to_prev) = if swapped {
        (PARKED_PREV_RECORD, PARKED_LIVE_RECORD)
    } else {
        (PARKED_LIVE_RECORD, PARKED_PREV_RECORD)
    };
    move_record(&swap.join(to_live), &layout.live_dir.join(PROMOTED_FILE))?;
    step("settled-live-record")?;
    move_record(&swap.join(to_prev), &layout.prev_dir().join(PROMOTED_FILE))?;
    step("settled-prev-record")
}

/// `aida dev rollback`: swap `live/prev/` and the live pair, under the
/// live-dir lock. Checks every precondition before changing anything, so "no
/// previous build" is a clean no-op error. Running it twice rolls forward.
/// Returns notes about any interrupted-state repairs.
pub(crate) fn rollback(layout: &DevLayout) -> Result<Vec<String>> {
    rollback_with_steps(layout, &mut |_| Ok(()))
}

/// [`rollback`], calling `step` after every on-disk change so tests can
/// inspect each intermediate state or stop there (an `Err` simulates a
/// crash at that point).
// trace:BUG-1959 | ai:claude
pub(crate) fn rollback_with_steps(
    layout: &DevLayout,
    step: &mut dyn FnMut(&'static str) -> Result<()>,
) -> Result<Vec<String>> {
    let _lock = lock_live_dir(layout)?;
    let notes = recover_live_dir(layout)?;
    for name in LIVE_BINARIES {
        if !layout.prev(name).is_file() {
            bail!(
                "no previous build to roll back to ({} does not exist); nothing was changed",
                layout.prev(name).display()
            );
        }
        if !layout.live(name).is_file() {
            bail!(
                "no live build to roll back from ({} does not exist); nothing was changed",
                layout.live(name).display()
            );
        }
    }
    // Park the live pair and both records, mark parking done, then per
    // binary prev -> live and the parked copy -> prev, then settle the
    // records. [`recover_live_dir`] finishes or undoes a crash at any step.
    let swap = layout.live_dir.join(".rollback");
    fresh_dir(&swap)?;
    step("fresh")?;
    for name in LIVE_BINARIES {
        link_or_copy(&layout.live(name), &swap.join(name))?;
        step("parked-binary")?;
    }
    // trace:STORY-1646 | ai:claude
    // trace:BUG-1959 | ai:claude
    move_record(
        &layout.live_dir.join(PROMOTED_FILE),
        &swap.join(PARKED_LIVE_RECORD),
    )?;
    step("parked-live-record")?;
    move_record(
        &layout.prev_dir().join(PROMOTED_FILE),
        &swap.join(PARKED_PREV_RECORD),
    )?;
    step("parked-prev-record")?;
    // Parking is complete; from here a parked binary missing from `.rollback/`
    // means it was swapped into `prev/` (see [`recover_live_dir`]).
    std::fs::write(swap.join(ROLLBACK_MARKER), b"")?;
    step("marked")?;
    for name in LIVE_BINARIES {
        std::fs::rename(layout.prev(name), layout.live(name))
            .with_context(|| format!("restoring {}", layout.live(name).display()))?;
        step("restored-binary")?;
        std::fs::rename(swap.join(name), layout.prev(name))?;
        step("swapped-binary")?;
    }
    settle_records(layout, &swap, true, step)?;
    std::fs::remove_file(swap.join(ROLLBACK_MARKER))?;
    step("unmarked")?;
    let _ = std::fs::remove_dir(&swap);
    Ok(notes)
}

// ---------------------------------------------------------------------------
// PATH links
// ---------------------------------------------------------------------------

/// What `--install-links` will do at one link path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkAction {
    Create,
    AlreadyCorrect,
    /// Something else is there; `existing` describes it for the user.
    Replace {
        existing: String,
        regular_file: bool,
    },
    /// A directory (or unreadable entry); never replaced.
    Blocked {
        why: String,
    },
}

pub(crate) fn plan_link(link: &Path, target: &Path) -> LinkAction {
    let Ok(meta) = std::fs::symlink_metadata(link) else {
        return LinkAction::Create;
    };
    if meta.file_type().is_symlink() {
        return match std::fs::read_link(link) {
            Ok(cur) if cur == target => LinkAction::AlreadyCorrect,
            Ok(cur) => LinkAction::Replace {
                existing: format!("a symlink to {}", cur.display()),
                regular_file: false,
            },
            Err(e) => LinkAction::Blocked {
                why: format!("cannot read the symlink: {e}"),
            },
        };
    }
    if meta.is_dir() {
        return LinkAction::Blocked {
            why: "it is a directory".to_string(),
        };
    }
    LinkAction::Replace {
        existing: format!("a regular file (kept as {})", backup_path(link).display()),
        regular_file: true,
    }
}

/// Where a replaced regular file at `link` is kept: `<name>.pre-live`, or
/// `<name>.pre-live.<unix-secs>[.<n>]` when that name is taken. An existing
/// backup is never overwritten.
pub(crate) fn backup_path(link: &Path) -> PathBuf {
    let name = link.file_name().unwrap_or_default().to_string_lossy();
    let plain = link.with_file_name(format!("{name}.pre-live"));
    if std::fs::symlink_metadata(&plain).is_err() {
        return plain;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut n = 0u32;
    loop {
        let cand = if n == 0 {
            link.with_file_name(format!("{name}.pre-live.{secs}"))
        } else {
            link.with_file_name(format!("{name}.pre-live.{secs}.{n}"))
        };
        if std::fs::symlink_metadata(&cand).is_err() {
            return cand;
        }
        n += 1;
    }
}

/// Point `<link_dir>/{aida,aida-server}` at the live pair. Anything already
/// there is listed first and replaced only with `yes` or a confirmation;
/// regular files are kept as `<name>.pre-live`.
pub(crate) fn install_links(
    layout: &DevLayout,
    yes: bool,
    steps: &mut dyn BuildSteps,
    out: &mut dyn Write,
) -> Result<()> {
    for name in LIVE_BINARIES {
        if !layout.live(name).is_file() {
            bail!(
                "nothing to link: {} does not exist yet. Run `aida dev build` in the primary checkout \
                 (on a clean main) first, or add --install-links to that run.",
                layout.live(name).display()
            );
        }
    }
    let plans: Vec<(&str, LinkAction)> = LIVE_BINARIES
        .iter()
        .map(|n| (*n, plan_link(&layout.link(n), &layout.live(n))))
        .collect();
    for (name, action) in &plans {
        if let LinkAction::Blocked { why } = action {
            bail!(
                "cannot link {}: {why}. Nothing was changed.",
                layout.link(name).display()
            );
        }
    }
    let replacing: Vec<String> = plans
        .iter()
        .filter_map(|(name, a)| match a {
            LinkAction::Replace { existing, .. } => {
                Some(format!("  {}: {existing}", layout.link(name).display()))
            }
            _ => None,
        })
        .collect();
    if !replacing.is_empty() {
        writeln!(out, "--install-links will replace:")?;
        for line in &replacing {
            writeln!(out, "{line}")?;
        }
        if !yes && !steps.confirm("Replace these with links to the live directory?") {
            bail!("links left unchanged; re-run with --yes to replace them");
        }
    }
    std::fs::create_dir_all(&layout.link_dir)
        .with_context(|| format!("creating {}", layout.link_dir.display()))?;
    for (name, action) in &plans {
        let link = layout.link(name);
        let target = layout.live(name);
        match action {
            LinkAction::AlreadyCorrect => {
                writeln!(
                    out,
                    "{} already points at {}",
                    link.display(),
                    target.display()
                )?;
                continue;
            }
            LinkAction::Replace {
                regular_file: true, ..
            } => {
                let backup = backup_path(&link);
                link_or_copy(&link, &backup)?;
                writeln!(
                    out,
                    "kept the old {} as {}",
                    link.display(),
                    backup.display()
                )?;
            }
            _ => {}
        }
        // Create beside the destination, then rename over it, so the PATH
        // entry is never missing.
        let tmp = layout.link_dir.join(format!(".{name}.live-link"));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&target, &tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        std::fs::rename(&tmp, &link).with_context(|| format!("installing {}", link.display()))?;
        writeln!(out, "{} -> {}", link.display(), target.display())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Real steps
// ---------------------------------------------------------------------------

/// Run `cmd` with a deadline, capturing stdout/stderr on reader threads so a
/// chatty child cannot fill a pipe and stall.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<(bool, String, String)> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_retrying_etxtbsy()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    match status {
        Some(s) => Ok((s.success(), out, err)),
        None => bail!("timed out after {}s", timeout.as_secs()),
    }
}

/// One smoke check: run, require exit 0 and a stdout predicate.
fn smoke_check(
    label: &str,
    cmd: &mut Command,
    timeout: Duration,
    stdout_ok: impl Fn(&str) -> bool,
) -> Result<()> {
    let (ok, out, err) = run_with_timeout(cmd, timeout).with_context(|| format!("`{label}`"))?;
    if !ok {
        let detail = err
            .lines()
            .chain(out.lines())
            .take(8)
            .collect::<Vec<_>>()
            .join("\n  ");
        bail!("`{label}` exited non-zero:\n  {detail}");
    }
    if !stdout_ok(&out) {
        bail!("`{label}` printed unexpected output: {}", out.trim());
    }
    Ok(())
}

/// A smoke-test [`Command`] for `bin`, with the git env vars a hook may
/// export removed: a stray GIT_DIR would point the read-only check at
/// another repo.
// trace:STORY-1646 | ai:claude
// trace:BUG-1959 | ai:claude
pub(crate) fn smoke_command(bin: &Path) -> Command {
    let mut c = Command::new(bin);
    c.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    c
}

/// The production [`BuildSteps`].
pub(crate) struct RealSteps;

impl BuildSteps for RealSteps {
    fn probe(&mut self, repo: &Path) -> Result<CheckoutState> {
        probe_checkout(repo)
    }

    fn guard(&mut self, repo: &Path, binary: &Path, after_wave: bool) -> Result<()> {
        crate::dev_cmd::guard_binary_change(repo, binary, after_wave)
    }

    fn path_aida(&mut self) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("aida"))
            .find(|cand| cand.is_file())
    }

    fn clean_full(&mut self, layout: &DevLayout) -> Result<()> {
        run_checked(&mut full_clean_command(layout), "cargo clean")
    }

    fn build(&mut self, layout: &DevLayout) -> Result<()> {
        // Parity with `make build-fast`: best-effort hook install, then the
        // incremental agent-profile build with sccache disabled (BUG-1684).
        let hooks = layout.repo.join("scripts/install-agent-skill-hooks.sh");
        if hooks.is_file() {
            let _ = Command::new("bash")
                .arg(&hooks)
                .arg("--best-effort")
                .current_dir(&layout.repo)
                .status();
        }
        run_checked(
            Command::new("cargo")
                .current_dir(&layout.repo)
                .env("RUSTC_WRAPPER", "")
                .env("CARGO_INCREMENTAL", "1")
                .args(["build", "--workspace", "--profile", PROFILE]),
            "cargo build --workspace --profile agent",
        )
    }

    fn smoke(&mut self, repo: &Path, aida: &Path, server: &Path) -> Result<()> {
        let cmd = smoke_command;
        smoke_check(
            "aida --version",
            cmd(aida).arg("--version"),
            Duration::from_secs(60),
            |o| o.starts_with("aida ") && o.contains("sha "),
        )?;
        smoke_check(
            "aida list --limit 1",
            cmd(aida).args(["list", "--limit", "1"]).current_dir(repo),
            Duration::from_secs(300),
            |_| true,
        )?;
        smoke_check(
            "aida-server --version",
            cmd(server).arg("--version"),
            Duration::from_secs(60),
            |o| o.starts_with("aida-server"),
        )
    }

    fn version_sha(&mut self, aida: &Path) -> Option<String> {
        binary_sha_label(aida)
    }

    fn restart_mcp(&mut self, layout: &DevLayout) -> Result<()> {
        let script = layout.repo.join("scripts/restart-mcp-servers.sh");
        if !script.is_file() {
            bail!("{} not found", script.display());
        }
        run_checked(
            Command::new("bash")
                .arg(&script)
                .arg(layout.live("aida"))
                .current_dir(&layout.repo),
            "scripts/restart-mcp-servers.sh",
        )
    }

    fn confirm(&mut self, prompt: &str) -> bool {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            eprintln!("stdin is not a terminal, so not replacing without --yes");
            return false;
        }
        eprint!("{prompt} [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return false;
        }
        matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes")
    }
}

/// Locate the checkout `aida dev build` should build: the enclosing AIDA
/// checkout of the cwd first (so a worktree builds itself even when
/// `$AIDA_DEV_REPO` names the main checkout), then `$AIDA_DEV_REPO`.
pub(crate) fn resolve_build_repo() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    if let Some(found) = cwd.ancestors().find(|p| crate::is_aida_repo(p)) {
        return Ok(found.canonicalize()?);
    }
    if let Some(repo) = env_path("AIDA_DEV_REPO") {
        if crate::is_aida_repo(&repo) {
            return Ok(repo.canonicalize()?);
        }
    }
    bail!(
        "`aida dev build` must run inside an AIDA checkout (or with $AIDA_DEV_REPO pointing at one)"
    )
}

pub(crate) fn handle_dev_build(opts: &DevBuildOptions) -> Result<()> {
    let repo = resolve_build_repo()?;
    let layout = DevLayout::for_repo(&repo)?;
    let mut stdout = std::io::stdout();
    run_dev_build(&layout, opts, &mut RealSteps, &mut stdout)?;
    Ok(())
}

/// `aida dev rollback` with the live-wave guard: refuses (or waits, with
/// `after_wave`) while a live drain wave is pinned to the live binary,
/// unless `force`.
pub(crate) fn run_dev_rollback(
    layout: &DevLayout,
    after_wave: bool,
    force: bool,
    steps: &mut dyn BuildSteps,
    out: &mut dyn Write,
) -> Result<()> {
    if force {
        writeln!(out, "--force: skipping the live-wave guard")?;
    } else {
        steps
            .guard(&layout.repo, &layout.live("aida"), after_wave)
            .map_err(|e| anyhow!("{e:#}\n(or `aida dev rollback --force` to swap anyway)"))?;
    }
    for note in rollback(layout)? {
        writeln!(out, "note: {note}")?;
    }
    writeln!(
        out,
        "Rolled back: {} now holds the previous build ({}); run `aida dev rollback` again to undo.",
        layout.live_dir.display(),
        binary_sha_label(&layout.live("aida")).unwrap_or_else(|| "unknown sha".into())
    )?;
    if let Err(e) = steps.restart_mcp(layout) {
        writeln!(
            out,
            "warning: could not restart stale MCP servers ({e:#}); restart your MCP clients \
             (or their `aida mcp-serve` processes) to pick up the restored build"
        )?;
    }
    Ok(())
}

pub(crate) fn handle_dev_rollback(after_wave: bool, force: bool) -> Result<()> {
    let live_dir = default_live_dir()?;
    let repo = resolve_build_repo().ok();
    let layout = DevLayout {
        repo: repo.unwrap_or_else(|| live_dir.clone()),
        target_dir: PathBuf::new(),
        live_dir,
        link_dir: default_link_dir()?,
    };
    let mut stdout = std::io::stdout();
    run_dev_rollback(&layout, after_wave, force, &mut RealSteps, &mut stdout)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// The `sha ...` token of a `--version` banner, keeping a `+dirty` suffix.
pub(crate) fn banner_sha_token(banner: &str) -> Option<String> {
    let idx = banner.find("sha ")?;
    let tok: String = banner[idx + 4..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '+')
        .collect();
    (!tok.is_empty()).then_some(tok)
}

fn binary_sha_label(bin: &Path) -> Option<String> {
    if !bin.is_file() {
        return None;
    }
    let out = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output_retrying_etxtbsy()
        .ok()?;
    banner_sha_token(&String::from_utf8_lossy(&out.stdout))
}

/// Pure: what `aida dev status` should flag about the live / prev / built SHAs.
///
/// `live_meta` is the live pair's `.promoted` record; when present it alone
/// decides the dirty warning (the banner's `+dirty` also counts untracked
/// files, which the gate allows). Without it, fall back to the banner.
pub(crate) fn live_status_notes(
    live: Option<&str>,
    live_meta: Option<&PromotionMeta>,
    prev: Option<&str>,
    built: Option<&str>,
) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(live) = live else {
        notes.push(
            "no live binary yet: run `aida dev build --install-links` in the primary checkout"
                .to_string(),
        );
        return notes;
    };
    // trace:STORY-1646 | ai:claude
    match live_meta {
        Some(meta) if meta.allow_dirty => notes.push(format!(
            "the live binary was promoted with --allow-dirty from a dirty checkout ({live}, HEAD {})",
            meta_sha(meta)
        )),
        Some(_) => {}
        None if live.ends_with("+dirty") => notes.push(format!(
            "the live binary was built from a dirty checkout ({live})"
        )),
        None => {}
    }
    if let Some(built) = built {
        if built != live {
            notes.push(format!(
                "the latest build ({built}) is not the live one ({live}); `aida dev build` promotes it"
            ));
        }
    }
    if prev.is_some_and(|p| p == live) {
        notes.push("the previous build has the same SHA as the live one".to_string());
    }
    notes
}

/// The recorded HEAD sha, or `unknown` when the record has none (an
/// `--allow-dirty` promotion where `git rev-parse HEAD` failed).
// trace:BUG-1959 | ai:claude
fn meta_sha(meta: &PromotionMeta) -> &str {
    let sha = meta.sha.trim();
    if sha.is_empty() {
        "unknown"
    } else {
        sha
    }
}

/// The `Promoted:` line of `aida dev status`.
// trace:STORY-1646 | ai:claude
// trace:BUG-1959 | ai:claude
pub(crate) fn promoted_line(meta: &PromotionMeta) -> String {
    format!(
        "Promoted:     HEAD {} at {}{}",
        meta_sha(meta),
        meta.promoted_at,
        if meta.allow_dirty {
            " (--allow-dirty)"
        } else {
            ""
        }
    )
}

/// The live-binary block of `aida dev status`.
pub(crate) fn print_live_status() {
    let Ok(live_dir) = default_live_dir() else {
        return;
    };
    let mut layout_err = None;
    let layout = match resolve_build_repo() {
        Ok(repo) => DevLayout::for_repo(&repo)
            .map_err(|e| layout_err = Some(format!("{e:#}")))
            .ok(),
        Err(_) => None,
    };
    // Outside an AIDA checkout, only report when a live dir exists.
    if layout.is_none() && !live_dir.exists() {
        return;
    }
    let live = binary_sha_label(&live_dir.join("aida"));
    let prev = binary_sha_label(&live_dir.join("prev").join("aida"));
    let built = layout
        .as_ref()
        .and_then(|l| binary_sha_label(&l.built("aida")));
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".to_string());
    println!("Live binary:  {} ({})", show(&live), live_dir.display());
    // trace:STORY-1646 | ai:claude
    let live_meta = read_promotion_meta(&live_dir);
    if let Some(meta) = &live_meta {
        println!("{}", promoted_line(meta));
    }
    println!("Previous:     {}", show(&prev));
    if let Some(l) = &layout {
        println!(
            "Latest built: {} ({})",
            show(&built),
            l.built("aida").display()
        );
    }
    if let Some(e) = &layout_err {
        println!("  ! {e}");
    }
    for note in live_status_notes(
        live.as_deref(),
        live_meta.as_ref(),
        prev.as_deref(),
        built.as_deref(),
    ) {
        println!("  ! {note}");
    }
    if live.is_some() {
        if let Ok(link_dir) = default_link_dir() {
            let link = link_dir.join("aida");
            if !matches!(
                plan_link(&link, &live_dir.join("aida")),
                LinkAction::AlreadyCorrect
            ) {
                println!(
                    "  ! {} does not point at the live binary: run `aida dev build --install-links`",
                    link.display()
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/dev_build_tests.rs"]
mod dev_build_tests;
