//! BUG-2169: the private clone a contained seat works in.
//!
//! A contained seat in a linked worktree can only run git by writing the main
//! checkout's shared common gitdir (`objects/`, `refs/`, `logs/`). Read-only
//! file binds inside a writable directory do not hold under bwrap (rename the
//! parent aside; a host lock-then-rename detaches the bind), so the advisor's
//! ruling is that a contained seat gets a PRIVATE CLONE instead:
//! `git clone --reference <main> <origin>` into a seat-owned directory. Main's
//! `.git` stays wholly read-only to the seat, which pushes to origin. See
//! `docs/plans/2026-10-16-bug-2169-contained-clone.md`.
//!
//! This module is the ONLY code that may run git with a seat clone as the
//! repository or working directory (advisor signoff, condition 1). A clone's
//! `.git/config`, `hooks/` and `info/` are seat-writable, so host git there
//! would run seat-planted configuration outside the sandbox. Everything else
//! learns about a clone from plain files: the host-owned marker, the
//! alternates line and the lease.
//! trace:BUG-2169 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Where `create` writes the marker naming the clone's main checkout,
/// relative to the clone root. `.aida/*` is excluded from git in the clone.
// trace:BUG-2169 | ai:claude
pub(crate) use aida_core::seat_clone_detect::{CLONE_DIR_PREFIX, MARKER_RELATIVE};

/// Where [`crate::worktree_gc::main_worktree_root_from`] points a seat clone
/// that does not verify: a path under `/dev/null`, so every read, write or
/// `git -C` aimed at it fails instead of treating the clone as main.
// trace:BUG-2169 | ai:claude
pub(crate) const UNVERIFIED_MAIN_SENTINEL: &str = "/dev/null/aida-unverified-seat-clone";

/// The namespace of the base pins `create` writes in the MAIN checkout, so
/// main's `gc` never prunes objects a clone borrows through alternates.
// trace:BUG-2169 | ai:claude
pub(crate) const PIN_PREFIX: &str = "refs/aida/seats/";

/// Set on every git command this module spawns. It only tags the invocation
/// so the condition-1 test can tell the accessor's own git from anyone
/// else's; it grants nothing.
// trace:BUG-2169 | ai:claude
pub(crate) const ACCESSOR_ENV: &str = "AIDA_SEAT_CLONE_ACCESSOR";

/// What kind of tree a session lease's `worktree_path` is. Leases written
/// before BUG-2169 carry no key and read as `Worktree`.
// trace:BUG-2169 | ai:claude
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TreeKind {
    /// A linked worktree of the main checkout (`git worktree add`, the pool).
    #[default]
    Worktree,
    /// A contained seat's private clone, made by [`create`].
    Clone,
}

impl TreeKind {
    /// For `skip_serializing_if`: a worktree lease is written exactly as it
    /// was before BUG-2169.
    pub(crate) fn is_worktree(&self) -> bool {
        *self == TreeKind::Worktree
    }
}

/// The `[contained] seat_tree` knob: which tree a CONTAINED seat gets. The
/// default is `clone` since slice S4c (TASK-2053); `worktree` opts out.
// trace:BUG-2169 | trace:TASK-2053 | ai:claude
pub(crate) fn seat_tree_knob(project_root: &Path) -> Result<TreeKind> {
    let cfg = crate::read_project_config_value(project_root);
    parse_seat_tree_knob(crate::config_lookup(cfg.as_ref(), "contained", "seat_tree"))
}

/// Whether `[contained] seat_tree` is set at all, rather than defaulted.
// trace:TASK-2053 | ai:claude
pub(crate) fn seat_tree_is_explicit(project_root: &Path) -> bool {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "seat_tree").is_some()
}

/// Parse a raw `[contained] seat_tree` value. Absent means `clone`; an
/// unknown value is an error, so a typo never silently picks a posture.
// trace:BUG-2169 | trace:TASK-2053 | ai:claude
pub(crate) fn parse_seat_tree_knob(raw: Option<&toml::Value>) -> Result<TreeKind> {
    let Some(raw) = raw else {
        return Ok(TreeKind::Clone);
    };
    match raw
        .as_str()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("worktree") => Ok(TreeKind::Worktree),
        Some("clone") => Ok(TreeKind::Clone),
        _ => anyhow::bail!(
            "[contained] seat_tree value `{raw}` is not recognised — expected `worktree` (a \
             linked worktree of the main checkout) or `clone` (a private clone per contained seat)"
        ),
    }
}

/// The tree a session gets: a clone only when the seat is contained AND the
/// knob says `clone`. Uncontained seats always keep worktrees, and their
/// launches never read the knob.
// trace:BUG-2169 | ai:claude
pub(crate) fn tree_kind_for_launch(project_root: &Path, contained_seat: bool) -> Result<TreeKind> {
    if !contained_seat {
        return Ok(TreeKind::Worktree);
    }
    seat_tree_knob(project_root)
}

/// What a contained launch asks of its tree, for [`default_clone_fallback`].
// trace:TASK-2053 | ai:claude
pub(crate) struct CloneLaunch {
    pub(crate) review: bool,
    pub(crate) reentry: bool,
    /// An explicit --path that is not an `aida-seat-*` directory directly in
    /// the worktree parent ([`is_seat_clone_location`]).
    pub(crate) path_not_seat_location: bool,
    pub(crate) has_origin: bool,
}

/// The tree a contained launch gets once the default is `clone`, plus a
/// one-line note when it falls back. With the knob unset, a launch a clone
/// cannot serve keeps a worktree and says why; never silently. With an
/// explicit `seat_tree = "clone"` a missing origin refuses here, and the
/// other cases refuse later in session start with their own reasons.
// trace:TASK-2053 | ai:claude
pub(crate) fn default_clone_fallback(
    kind: TreeKind,
    explicit: bool,
    launch: &CloneLaunch,
) -> Result<(TreeKind, Option<String>)> {
    if kind != TreeKind::Clone {
        return Ok((kind, None));
    }
    if explicit {
        if !launch.has_origin {
            anyhow::bail!(
                "[contained] seat_tree = \"clone\" needs an `origin` remote to clone from, and \
                 this checkout has none: add an origin remote or set seat_tree = \"worktree\""
            );
        }
        return Ok((kind, None));
    }
    let why = if !launch.has_origin {
        "this checkout has no `origin` remote to clone from"
    } else if launch.review {
        "a review session runs in a worktree"
    } else if launch.reentry {
        "it re-enters an existing worktree"
    } else if launch.path_not_seat_location {
        "its --path is not an `aida-seat-*` directory directly in the worktree parent"
    } else {
        return Ok((kind, None));
    };
    Ok((
        TreeKind::Worktree,
        Some(format!(
            "this contained seat gets a linked worktree, not a private clone: {why}"
        )),
    ))
}

/// A fresh directory for a seat clone of `project_root` working `slug`:
/// `aida-seat-<project>-<slug>[-N]` under `[worktree_pool] worktree_parent`
/// (or beside the project), the first name that does not exist yet.
// trace:BUG-2169 | ai:claude
pub(crate) fn clone_path(project_root: &Path, slug: &str) -> Result<PathBuf> {
    clone_paths(project_root, slug)
        .find(|path| !path.exists())
        .with_context(|| format!(
            "no free seat-clone directory for `{slug}` next to {} (aida-seat-<project>-{slug} through -50 all exist); remove stale seat clones first",
            project_root.display()
        ))
}

// A manually preserved clone may leave its original pin behind. Allocate
// another slot without using, moving, or deleting that host evidence.
pub(crate) fn pinned_rework_clone_path(main: &Path, slug: &str) -> Result<PathBuf> {
    let pins = linked_rework_git(main, &["for-each-ref", "--format=%(refname)", PIN_PREFIX])?;
    clone_paths(main, slug)
        .find(|path| {
            matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
                && pin_ref(path).is_some_and(|pin| !pins.lines().any(|reference| reference == pin))
        })
        .context("no free seat-clone slot with an unused host pin")
}

fn clone_paths<'a>(project_root: &'a Path, slug: &str) -> impl Iterator<Item = PathBuf> + 'a {
    let project = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("project");
    let stem = format!("{CLONE_DIR_PREFIX}{project}-{slug}");
    let parent = crate::worktree_gc::worktree_pool_config_worktree_parent(project_root);
    (1..=50).map(move |n| {
        let name = if n == 1 {
            stem.clone()
        } else {
            format!("{stem}-{n}")
        };
        aida_core::worktree_pool::worktree_placement_path(project_root, &name, parent.as_deref())
    })
}

/// The base-pin ref for the clone at `clone_root`: `refs/aida/seats/<dir>`.
// trace:BUG-2169 | ai:claude
pub(crate) fn pin_ref(clone_root: &Path) -> Option<String> {
    let name = clone_root.file_name()?.to_str()?;
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    valid.then(|| format!("{PIN_PREFIX}{name}"))
}

/// The host-owned record `create` writes at [`MARKER_RELATIVE`]. A seat can
/// rewrite it, so it is a HINT: [`main_for`] trusts it only when the
/// alternates line and a lease in that main agree.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SeatCloneMarker {
    pub(crate) main: PathBuf,
    pub(crate) branch: String,
    pub(crate) base_sha: String,
    pub(crate) origin_url: String,
    pub(crate) pin: String,
}

/// What [`create`] made.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct CreatedClone {
    pub(crate) path: PathBuf,
    pub(crate) base_sha: String,
    pub(crate) pin: String,
}

/// The refusal for a path inside a seat clone that does not verify.
// trace:BUG-2169 | ai:claude
pub(crate) fn unverified_refusal(clone_root: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} is a contained seat's private clone, but its marker, alternates and lease do not \
         agree on a main checkout, so it is not used as one. Run this from the main checkout.",
        clone_root.display()
    )
}

/// A git command tagged as this module's own. `dir` is passed with `-C`.
fn git(dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.env(ACCESSOR_ENV, "1").arg("-C").arg(dir);
    cmd
}

/// Run `cmd`; its trimmed stdout on success, otherwise an error carrying
/// git's stderr.
fn run(mut cmd: std::process::Command, what: &str) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("could not run git to {what}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git could not {what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Only failure of the origin-clone command permits the unset knob to
/// fall back. Destination preparation, pinning and checkout failures still refuse.
// trace:TASK-2300 | ai:codex
#[derive(Debug)]
struct OriginCloneFailure(String);

impl std::fmt::Display for OriginCloneFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for OriginCloneFailure {}

/// An explicit clone never downgrades; the default names the origin failure.
// trace:TASK-2300 | ai:codex
pub(crate) fn origin_failure_fallback(explicit: bool, error: &anyhow::Error) -> Option<String> {
    if explicit || error.downcast_ref::<OriginCloneFailure>().is_none() {
        return None;
    }
    Some("this contained seat gets a linked worktree because cloning from `origin` failed or timed out; check origin reachability and credentials".into())
}

/// The loud refusal when a contained seat cannot get its clone. Callers may
/// fall back only for a typed origin failure with the knob unset.
// trace:TASK-2300 | ai:codex
fn refusal(dest: &Path, error: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot create the private clone this contained seat needs at {}: {error:#}\n\
         This clone refusal never falls back to a shared worktree. Fix the cause and retry, set \
         `[contained] seat_tree = \"worktree\"`, or launch from an uncontained shell.",
        dest.display()
    )
}

/// Fetch the explicit base before allocating a seat. Resolve our own ref so
/// concurrent fetches cannot replace the selected commit via a tracking ref
/// or FETCH_HEAD.
// trace:BUG-2673 | ai:codex
pub(crate) fn fetch_explicit_base(main: &Path, branch: &str) -> Result<String> {
    crate::git_arg_guard::reject_option_like("--base", branch)?;
    let source = format!("refs/heads/{branch}");
    let mut validate = git(main);
    validate.args(["check-ref-format", &source]);
    run(validate, &format!("validate --base branch `{branch}`"))?;

    let target = format!("refs/aida/seat-base-fetch/{}", uuid::Uuid::new_v4());
    let mut fetch = aida_core::git_ops::host_remote_git_command(main, "origin");
    fetch
        .env(ACCESSOR_ENV, "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            crate::forge::batch_mode_ssh_command(main),
        )
        .stdin(std::process::Stdio::null())
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "origin",
        ])
        .arg(format!("{source}:{target}"));
    const CAP: u64 = 1024 * 1024;
    let result = (|| {
        match crate::command_output_with_timeout_capped(
            fetch,
            std::time::Duration::from_secs(30),
            CAP,
        ) {
            crate::BoundedCommandOutput::Completed(out)
                if out.stdout.len() as u64 > CAP || out.stderr.len() as u64 > CAP =>
            {
                anyhow::bail!("output exceeded the capture limit")
            }
            crate::BoundedCommandOutput::Completed(out) if out.status.success() => {}
            crate::BoundedCommandOutput::Completed(out) => {
                anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim())
            }
            crate::BoundedCommandOutput::TimedOut => anyhow::bail!("git fetch timed out"),
            crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!("could not start git fetch"),
        }
        let mut resolve = git(main);
        resolve.args(["rev-parse", "--verify", &format!("{target}^{{commit}}")]);
        run(resolve, "resolve the fetched commit")
    })();
    let mut cleanup = git(main);
    cleanup.args(["update-ref", "-d", &target]);
    let cleaned = run(cleanup, "remove the temporary seat base ref");
    let sha = result.map_err(|error| {
        anyhow::anyhow!("could not fetch --base `{branch}` from origin: {error:#}")
    })?;
    cleaned?;
    Ok(sha)
}

/// Create the private clone for a contained seat. Runs HOST side, before any
/// sandbox exists:
///
/// 1. resolve `base_ref` to a commit in `main` (the base is main's view);
/// 2. create the empty `dest`, then pin the base at `refs/aida/seats/<dir>`
///    in `main`;
/// 3. `git clone --reference <main> --no-checkout -- <origin> <dest>`;
/// 4. check out `branch` at that commit (present through alternates);
/// 5. exclude AIDA runtime files and write the marker.
///
/// Every failure refuses clone creation and attempts to remove its directory
/// and pin. Cleanup failure retains recovery evidence and prevents fallback.
/// This function never creates a worktree.
// trace:BUG-2169 | ai:claude
pub(crate) fn create(
    main: &Path,
    dest: &Path,
    branch: &str,
    base_ref: &str,
) -> Result<CreatedClone> {
    if !main.join(".git").is_dir() {
        return Err(refusal(
            dest,
            anyhow::anyhow!(
                "{} is not a main checkout with its own .git directory",
                main.display()
            ),
        ));
    }
    if dest.exists() {
        return Err(refusal(
            dest,
            anyhow::anyhow!("the directory already exists"),
        ));
    }
    // Host code recognises a clone by this name even after the seat deletes
    // its marker, so no clone is ever made under another name.
    let named = dest
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(CLONE_DIR_PREFIX));
    if !named {
        return Err(refusal(
            dest,
            anyhow::anyhow!("a seat clone's directory name must start with `{CLONE_DIR_PREFIX}`"),
        ));
    }
    let Some(pin) = pin_ref(dest) else {
        return Err(refusal(
            dest,
            anyhow::anyhow!("the directory name cannot be used in a git ref"),
        ));
    };
    crate::git_arg_guard::reject_option_like("--branch", branch).map_err(|e| refusal(dest, e))?;
    crate::git_arg_guard::reject_option_like("--base", base_ref).map_err(|e| refusal(dest, e))?;

    let mut resolve = git(main);
    resolve.args([
        "rev-parse",
        "--verify",
        "--quiet",
        crate::git_arg_guard::END_OF_OPTIONS,
        &format!("{base_ref}^{{commit}}"),
    ]);
    let base_sha = run(
        resolve,
        &format!("resolve `{base_ref}` in the main checkout"),
    )
    .map_err(|e| refusal(dest, e))?;
    let mut origin = git(main);
    origin.args(["config", "--get", "remote.origin.url"]);
    let origin_url = run(origin, "read the main checkout's origin URL")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            refusal(
                dest,
                anyhow::anyhow!("the main checkout has no `origin` remote to clone from"),
            )
        })?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))
            .map_err(|e| refusal(dest, e))?;
    }

    // Claim the (empty) directory BEFORE the pin, so a pin never exists
    // without its clone directory: reap releases a pin with no clone lease
    // only when the directory is gone, and the lease is written after
    // `create` returns. `create_dir` also fails when another launch claimed
    // the same path first. `git clone` accepts an empty directory.
    // trace:TASK-2053 | ai:claude
    std::fs::create_dir(dest)
        .with_context(|| format!("create {}", dest.display()))
        .map_err(|e| refusal(dest, e))?;
    let mut pin_cmd = git(main);
    // An empty old value makes `update-ref` refuse when the pin already exists.
    pin_cmd.args(["update-ref", &pin, &base_sha, ""]);
    if let Err(error) = run(pin_cmd, &format!("pin {pin} in the main checkout")) {
        let _ = std::fs::remove_dir(dest);
        return Err(refusal(dest, error));
    }

    let made = clone_and_checkout(main, dest, branch, &base_sha, &origin_url, &pin);
    if let Err(error) = made {
        return Err(cleanup_failed_clone(main, dest, &pin, error));
    }
    Ok(CreatedClone {
        path: dest.to_path_buf(),
        base_sha,
        pin,
    })
}

/// Retire the mail of a clone this process is still creating, before it is
/// removed. No lease exists yet, so there is no host-recorded binding to
/// look up; the binding is this process's own: it cloned `dest` from `main`
/// moments ago, and no seat has run in it. The clone is fenced under main's
/// anchor like any bound clone, so a refusal (or a `main` that cannot be
/// resolved) keeps it; it is never removed unfenced because it looks empty
/// (C SKETCH section 5).
// trace:STORY-1843 | ai:claude
fn retire_unfinished_clone(
    main: &Path,
    dest: &Path,
    reason: &str,
) -> aida_core::mailbox_lifecycle::guard::Gate {
    let main = main.canonicalize().ok();
    aida_core::mailbox_lifecycle::guard::retire_clone_checkout(main.as_deref(), dest, reason)
}

/// Preserve origin-failure eligibility only after removing both allocations.
// trace:TASK-2300 | ai:codex
fn cleanup_failed_clone(
    main: &Path,
    dest: &Path,
    pin: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    if dest.exists() {
        // Fenced under main's anchor through the creator's own binding (see
        // `retire_unfinished_clone`). trace:STORY-1843 | ai:claude
        let retirement = match retire_unfinished_clone(main, dest, "failed clone") {
            Ok(retirement) => retirement,
            Err(kept) => return refusal(dest, anyhow::anyhow!("{error:#}; {kept}")),
        };
        // The quiet re-check runs right before the removal. trace:STORY-1843 | ai:claude
        if let Err(cleanup) = aida_core::mailbox_lifecycle::guard::remove_retired(
            dest,
            retirement,
            || std::fs::remove_dir_all(dest).map_err(anyhow::Error::from),
            aida_core::mailbox_lifecycle::guard::Retirement::complete,
        ) {
            return refusal(
                dest,
                anyhow::anyhow!("{error:#}; clone cleanup failed: {cleanup}"),
            );
        }
    }
    if let Err(cleanup) = release_pin(main, pin) {
        return refusal(
            dest,
            anyhow::anyhow!("{error:#}; base-pin cleanup failed: {cleanup:#}"),
        );
    }
    let origin_failed = error.downcast_ref::<OriginCloneFailure>().is_some();
    let refused = refusal(dest, error);
    if origin_failed {
        anyhow::Error::new(OriginCloneFailure(refused.to_string()))
    } else {
        refused
    }
}

// trace:TASK-2300 | ai:codex
fn run_origin_clone(cmd: std::process::Command, timeout: std::time::Duration) -> Result<()> {
    const CAP: u64 = 1024 * 1024;
    match crate::command_output_with_timeout_capped(cmd, timeout, CAP) {
        crate::BoundedCommandOutput::Completed(out)
            if out.stdout.len() as u64 > CAP || out.stderr.len() as u64 > CAP =>
        {
            anyhow::bail!("origin clone output exceeded the capture limit")
        }
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(()),
        crate::BoundedCommandOutput::Completed(out) => {
            Err(anyhow::Error::new(OriginCloneFailure(format!(
                "git could not clone origin: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))))
        }
        crate::BoundedCommandOutput::TimedOut => Err(anyhow::Error::new(OriginCloneFailure(
            "git clone from origin timed out".into(),
        ))),
        crate::BoundedCommandOutput::SpawnFailed => {
            anyhow::bail!("could not start git to clone origin")
        }
    }
}

fn clone_and_checkout(
    main: &Path,
    dest: &Path,
    branch: &str,
    base_sha: &str,
    origin_url: &str,
    pin: &str,
) -> Result<()> {
    let mut clone = std::process::Command::new("git");
    clone
        .env(ACCESSOR_ENV, "1")
        // `--no-hardlinks`: a local-path origin is otherwise hardlinked into the
        // seat's objects, so the seat could rewrite origin's packs in place.
        .args([
            "clone",
            "--quiet",
            "--no-checkout",
            "--no-hardlinks",
            "--reference",
        ]);
    clone.arg(main).arg("--").arg(origin_url).arg(dest);
    // Bound unreachable transports and disable terminal credential prompts.
    // Clone stderr may contain credentials from the URL, so the fallback note
    // is generic; explicit clone refusals retain the existing diagnostics.
    // trace:TASK-2300 | ai:codex
    clone
        .env("GIT_TERMINAL_PROMPT", "0")
        // ssh ignores GIT_TERMINAL_PROMPT and the null stdin: it opens
        // /dev/tty for passphrase and host-key prompts, and in the timeout
        // runner's own process group it stops on SIGTTIN until the full 30s
        // timeout. Batch mode makes an ssh origin that needs interaction fail
        // at once, keeping any configured ssh command.
        // trace:TASK-2272 | ai:claude (TASK-2318)
        .env(
            "GIT_SSH_COMMAND",
            crate::forge::batch_mode_ssh_command(main),
        )
        .stdin(std::process::Stdio::null());
    run_origin_clone(clone, std::time::Duration::from_secs(30))?;

    let mut checkout = git(dest);
    checkout.args(["checkout", "--quiet", "--no-track", "-b", branch, base_sha]);
    run(checkout, &format!("check out `{branch}` at {base_sha}"))?;

    let exclude = dest.join(".git").join("info").join("exclude");
    if let Some(dir) = exclude.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    // Read the exclude from git's resolved common dir rather than a literal
    // `.git/info/exclude`, so it stays correct if `create` ever accepts a
    // primary whose `.git` is a gitfile (today it refuses one).
    // trace:BUG-b6b1278890774ef993d2ebc327862470-003 | ai:claude
    let mut common = git(main);
    common.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let common = PathBuf::from(run(common, "resolve the primary checkout's git dir")?);
    let primary = std::fs::read_to_string(common.join("info").join("exclude")).unwrap_or_default();
    std::fs::write(&exclude, seat_exclude_body(&existing, &primary))
        .with_context(|| format!("write {}", exclude.display()))?;

    let marker = SeatCloneMarker {
        main: main.canonicalize().unwrap_or_else(|_| main.to_path_buf()),
        branch: branch.to_string(),
        base_sha: base_sha.to_string(),
        origin_url: origin_url.to_string(),
        pin: pin.to_string(),
    };
    let marker_path = dest.join(MARKER_RELATIVE);
    if let Some(dir) = marker_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&marker_path, toml::to_string(&marker)?)
        .with_context(|| format!("write {}", marker_path.display()))?;
    Ok(())
}

/// The seat clone's `info/exclude`: the clone's own body, the AIDA runtime
/// entries, then every entry of the primary's `info/exclude` the clone lacks.
/// A seat clone is a separate clone, so without this it sees what the primary
/// ignores (the Claude Code sandbox stand-ins among them) as untracked files a
/// lane commit picks up; a stand-in the primary does not ignore stays visible,
/// and ship's zero-byte read-only check is the backstop. Only primary entries
/// that would shadow the clone's `/.aida/*` are skipped, which leaves tracked
/// `.aida/` config re-includable.
// trace:BUG-b6b1278890774ef993d2ebc327862470-003 | ai:claude
pub(crate) fn seat_exclude_body(existing: &str, primary: &str) -> String {
    let mut body = existing.to_string();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    // trace:STORY-2630 | ai:codex
    body.push_str(
        "# AIDA runtime state (BUG-2169 seat clone)\n/.aida/*\n.aida-store\n.codegraph\n",
    );
    let mut present: std::collections::HashSet<String> =
        body.lines().map(|l| l.trim().to_string()).collect();
    let inherited: Vec<&str> = primary
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !shadows_aida_dir(l))
        .filter(|l| present.insert(l.to_string()))
        .collect();
    if !inherited.is_empty() {
        body.push_str("# Inherited from the primary checkout's info/exclude\n");
        for line in inherited {
            body.push_str(line);
            body.push('\n');
        }
    }
    body
}

/// Whether an exclude pattern names the `.aida` directory itself (`.aida`,
/// `/.aida/`, `.aida/*`, ...), which would shadow the clone's `/.aida/*`
/// re-include of tracked config. `.aida-store` or `.aida-compete-*.log` do not.
fn shadows_aida_dir(pattern: &str) -> bool {
    let p = pattern.strip_prefix('/').unwrap_or(pattern);
    let p = p
        .strip_suffix("/**")
        .or_else(|| p.strip_suffix("/*"))
        .or_else(|| p.strip_suffix('/'))
        .unwrap_or(p);
    p == ".aida"
}

/// Removes a just-made clone and releases its pin when dropped armed, so a
/// `session_start` that fails after [`create`] (runtime links, env shim,
/// lease write) leaves neither behind. Disarm once the lease is written.
// trace:BUG-2169 | ai:claude
pub(crate) struct CloneCleanupGuard {
    main: PathBuf,
    clone: PathBuf,
    pin: String,
    armed: bool,
}

impl CloneCleanupGuard {
    pub(crate) fn new(main: &Path, made: &CreatedClone) -> Self {
        Self {
            main: main.to_path_buf(),
            clone: made.path.clone(),
            pin: made.pin.clone(),
            armed: true,
        }
    }

    /// The session now owns the clone; keep it.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CloneCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // trace:STORY-1843 | ai:claude
        let retirement = match retire_unfinished_clone(&self.main, &self.clone, "unfinished clone")
        {
            Ok(retirement) => retirement,
            Err(kept) => {
                eprintln!(
                    "kept the unfinished seat clone {}: {kept}",
                    self.clone.display()
                );
                return;
            }
        };
        if let Err(error) = aida_core::mailbox_lifecycle::guard::remove_retired(
            &self.clone,
            retirement,
            || std::fs::remove_dir_all(&self.clone).map_err(anyhow::Error::from),
            aida_core::mailbox_lifecycle::guard::Retirement::complete,
        ) {
            eprintln!(
                "could not remove the unfinished seat clone {}: {error}",
                self.clone.display()
            );
        }
        if let Err(error) = release_pin(&self.main, &self.pin) {
            eprintln!("{error:#}");
        }
    }
}

/// Test-only failure injection for the steps after [`create`].
// trace:BUG-2169 | ai:claude
#[cfg(test)]
pub(crate) mod failpoint {
    use std::cell::Cell;

    thread_local! {
        static FAIL_BEFORE_LEASE: Cell<bool> = const { Cell::new(false) };
    }

    /// Make the next `session_start` on this thread fail just before it
    /// writes the lease.
    pub(crate) fn fail_before_lease(on: bool) {
        FAIL_BEFORE_LEASE.with(|f| f.set(on));
    }

    pub(crate) fn check_before_lease() -> anyhow::Result<()> {
        if FAIL_BEFORE_LEASE.with(|f| f.replace(false)) {
            anyhow::bail!("injected failure before the lease write");
        }
        Ok(())
    }
}

/// Production no-op twin of [`failpoint::check_before_lease`].
#[cfg(not(test))]
pub(crate) mod failpoint {
    #[inline]
    pub(crate) fn check_before_lease() -> anyhow::Result<()> {
        Ok(())
    }
}

/// Delete a base pin in `main`. Only `refs/aida/seats/*` is ever touched.
// trace:BUG-2169 | ai:claude
pub(crate) fn release_pin(main: &Path, pin: &str) -> Result<()> {
    if !pin.starts_with(PIN_PREFIX) || pin.len() == PIN_PREFIX.len() {
        anyhow::bail!("`{pin}` is not a seat-clone pin");
    }
    let mut cmd = git(main);
    cmd.args(["update-ref", "-d", pin]);
    run(cmd, &format!("release {pin}")).map(|_| ())
}

/// The root of the seat clone `start` sits in, found from plain files: the
/// nearest ancestor whose `.git` is a DIRECTORY and that carries the marker.
/// Runs no git. `None` for a worktree, the main checkout or any other repo.
// trace:BUG-2169 | ai:claude
pub(crate) fn seat_clone_root(start: &Path) -> Option<PathBuf> {
    aida_core::seat_clone_detect::seat_clone_root(start)
}

/// How [`main_for`] resolved a path.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MainFor {
    /// Not inside a seat clone; ask git as usual.
    NotAClone,
    /// A seat clone whose marker, alternates and lease agree on this main.
    Verified(PathBuf),
    /// A seat clone that does not verify. Callers refuse: it is neither a
    /// main checkout nor safe to run git in.
    Unverified(PathBuf),
}

/// Resolve the main checkout for `start` without running git. A seat clone
/// counts as verified only when all three agree:
///
/// - the marker names a main checkout with a `.git` directory;
/// - the clone's first alternates line is that main's `.git/objects`;
/// - that main holds a `clone` lease for exactly this clone.
// trace:BUG-2169 | ai:claude
pub(crate) fn main_for(start: &Path) -> MainFor {
    let Some(clone_root) = seat_clone_root(start) else {
        return MainFor::NotAClone;
    };
    match verified_main(&clone_root) {
        Some(main) => MainFor::Verified(main),
        None => MainFor::Unverified(clone_root),
    }
}

fn verified_main(clone_root: &Path) -> Option<PathBuf> {
    let main = clone_marker_main(clone_root)?;
    let leased = crate::list_leases(&main).into_iter().any(|lease| {
        lease.tree_kind == TreeKind::Clone
            && lease
                .worktree_path
                .canonicalize()
                .is_ok_and(|p| p == clone_root)
    });
    leased.then_some(main)
}

// Project identity for fresh recovery of an unleased clone. Launch and
// containment authority still require verified_main or host_recorded_clone.
// trace:BUG-2688 | ai:codex
fn clone_marker_main(clone_root: &Path) -> Option<PathBuf> {
    // Seat-controlled files: read only regular files (a FIFO would hang).
    // A `.git` the seat turned into a symlink or `gitdir:` file never verifies.
    if !aida_core::seat_clone_detect::has_real_git_dir(clone_root) {
        return None;
    }
    let read = aida_core::seat_clone_detect::read_regular_file;
    let marker: SeatCloneMarker = toml::from_str(&read(&clone_root.join(MARKER_RELATIVE))?).ok()?;
    let main = marker.main.canonicalize().ok()?;
    if !main.join(".git").is_dir() || main == clone_root {
        return None;
    }
    let alternates = read(&clone_root.join(".git/objects/info/alternates"))?;
    let first = alternates.lines().map(str::trim).find(|l| !l.is_empty())?;
    let borrowed = Path::new(first).canonicalize().ok()?;
    if borrowed != main.join(".git").join("objects").canonicalize().ok()? {
        return None;
    }
    Some(main)
}

/// A seat clone the HOST recorded, as the S2 sandbox boundary sees it.
// trace:BUG-2169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostRecordedClone {
    /// The clone root, canonical.
    pub(crate) root: PathBuf,
    /// The main checkout whose `.aida/sessions` holds the clone's lease.
    pub(crate) main: PathBuf,
    /// The seat's own cargo target directory, `<main>/target/worktrees/<dir>`,
    /// from the lease; `None` when main has no `target/`.
    pub(crate) cargo_target_dir: Option<PathBuf>,
}

/// `root` as a seat clone the host itself created, or `None`. This is the
/// only thing that lets a contained boundary treat a write root's own `.git`
/// as the seat's (BUG-2012 otherwise denies a real `.git` under a write root
/// as a main checkout's).
///
/// The trust comes from the `clone` lease in `<main>/.aida/sessions`, which
/// the host writes and which sits outside every root a contained seat may
/// write. `main` is the launcher's main checkout, passed in by the caller
/// (resolved once at the launch entry point, see [`launcher_main`]); the
/// clone's marker must name exactly it.
/// [`main_for`] reads main from the seat-writable marker, so without that
/// anchor a seat could point its marker at a forged "main" in a directory it
/// can write and plant a lease there. TASK-2032: the anchor is the one main
/// this launch is for, not every main the process ever resolved, so a
/// long-lived process does not widen it. The trust rests on that caller
/// contract: `main` is host-resolved, never read from a seat-writable file.
/// As defense in depth `main` must also pass [`plausible_host_main`], which
/// refuses a clone or a repository under a `target/worktrees` directory. That
/// is a partial filter, not a forgery guarantee: a repository a seat
/// `git init`s in another root it writes (its sandbox TMPDIR, say) passes it,
/// so only the caller contract keeps such a path out. On top of that,
/// `root` must be the clone root itself (not a directory inside one), be
/// named `aida-seat-*`, and neither contain the main checkout nor sit inside
/// it. The cargo target directory is accepted only at exactly
/// `<main>/target/worktrees/<clone dir>`, as a real directory, never a
/// symlink.
// trace:BUG-2169 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn host_recorded_clone(main: &Path, root: &Path) -> Option<HostRecordedClone> {
    let launcher_main = main.canonicalize().ok()?;
    if !plausible_host_main(&launcher_main) {
        return None;
    }
    let root = root.canonicalize().ok()?;
    if seat_clone_root(&root).as_deref() != Some(root.as_path()) {
        return None;
    }
    let named = root
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(CLONE_DIR_PREFIX));
    if !named {
        return None;
    }
    let MainFor::Verified(main) = main_for(&root) else {
        return None;
    };
    if main != launcher_main {
        return None;
    }
    if main.starts_with(&root) || root.starts_with(&main) {
        return None;
    }
    let lease = crate::list_leases(&main).into_iter().find(|lease| {
        lease.tree_kind == TreeKind::Clone
            && lease.worktree_path.canonicalize().is_ok_and(|p| p == root)
    })?;
    // The target directory is the seat's alone, at the one place the host
    // puts it, and a real directory (a symlink there would grant its target).
    let expected = main
        .join("target")
        .join("worktrees")
        .join(root.file_name()?);
    let cargo_target_dir = lease.cargo_target_dir.filter(|dir| {
        *dir == expected && std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir())
    });
    Some(HostRecordedClone {
        root,
        main,
        cargo_target_dir,
    })
}

/// Whether `main` may anchor a seat clone at all: a checkout with a real
/// `.git` directory that is not itself, nor inside, a seat clone (by name or
/// marker; no lease or process-wide state is read, so the answer never
/// depends on what else the process resolved), and that does not sit under a
/// `target/worktrees` directory, where every seat's cargo target directory
/// lives and a seat could `git init` a forged main (TASK-2032 round 2).
// trace:TASK-2032 | ai:claude
pub(crate) fn plausible_host_main(main: &Path) -> bool {
    let under_seat_targets = main.ancestors().skip(1).any(|dir| {
        dir.file_name().is_some_and(|n| n == "worktrees")
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "target")
    });
    aida_core::seat_clone_detect::has_real_git_dir(main)
        && !aida_core::seat_clone_detect::is_named_or_marked_clone(main)
        && !under_seat_targets
}

/// The host main for a launch that starts in `start` (a host directory, never
/// a seat's): the main checkout host git resolves from it. `None` when
/// `start` is inside a seat clone, whose main comes only from its
/// seat-writable marker and so can anchor nothing.
// trace:TASK-2032 | ai:claude
pub(crate) fn host_main_from(start: &Path) -> Option<PathBuf> {
    if main_for(start) != MainFor::NotAClone {
        return None;
    }
    let main = crate::main_worktree_root_from(start).canonicalize().ok()?;
    plausible_host_main(&main).then_some(main)
}

/// The launcher's main checkout: [`host_main_from`] the project this process
/// runs in (its cwd, or a test's pinned project root). A launch entry point
/// calls this ONCE, before it changes directory into a seat, and threads the
/// result explicitly through every boundary builder: after a chdir into a
/// seat clone the cwd names the clone, and this returns `None`. Boundary
/// builders never call it: they take the host main as a parameter. The
/// functions allowed to call it are listed in
/// `tests/task_2032_launcher_main_ratchet_tests.rs`, which fails on any new
/// caller.
// trace:TASK-2032 | ai:claude
pub(crate) fn launcher_main() -> Option<PathBuf> {
    host_main_from(&crate::find_project_root().ok()?)
}

/// Test seam: pins this thread's project root (and so [`launcher_main`]) to
/// `main` for the guard's lifetime, without the env lock an
/// `AmbientGuard` holds, so a test may still take one beside it.
// trace:TASK-2032 | ai:claude
#[cfg(test)]
pub(crate) struct LauncherMainPin(Option<crate::test_ambient::Ambient>);

#[cfg(test)]
impl LauncherMainPin {
    pub(crate) fn new(main: &Path) -> Self {
        Self(crate::test_ambient::replace(Some(
            crate::test_ambient::Ambient {
                project_root: main.to_path_buf(),
                stdin_is_terminal: false,
                stdout_is_terminal: false,
                stderr_is_terminal: false,
            },
        )))
    }
}

#[cfg(test)]
impl Drop for LauncherMainPin {
    fn drop(&mut self) {
        crate::test_ambient::replace(self.0.take());
    }
}

/// [`host_recorded_clone`] for a launch whose entry point resolved
/// `host_main`; `None` (no host main) recognises no clone, so every boundary
/// fails closed.
// trace:TASK-2032 | ai:claude
pub(crate) fn recorded_clone_for(
    host_main: Option<&Path>,
    root: &Path,
) -> Option<HostRecordedClone> {
    host_recorded_clone(host_main?, root)
}

/// The extra paths a host-recorded clone seat writes beyond its own root: its
/// cargo target directory, as recorded and canonical. sccache's shared cache
/// is never one of them (advisor ruling O2: a compile cache a contained seat
/// can write poisons every other seat and the host).
// trace:BUG-2169 | ai:claude
pub(crate) fn seat_extra_write_paths(clone: &HostRecordedClone) -> Vec<PathBuf> {
    clone
        .cargo_target_dir
        .as_deref()
        .map(crate::contained_boundary::contained_write_roots)
        .unwrap_or_default()
}

/// The environment a clone seat's tools run with: cargo builds into the
/// seat's own target directory, and with no compiler wrapper, so a configured
/// `rustc-wrapper = sccache` never reaches the host's shared sccache server
/// (ruling O2). An empty `RUSTC_WRAPPER` overrides `build.rustc-wrapper`.
// trace:BUG-2169 | ai:claude
pub(crate) fn seat_build_env(clone: &HostRecordedClone) -> Vec<(&'static str, String)> {
    let mut env = vec![("RUSTC_WRAPPER", String::new())];
    if let Some(dir) = &clone.cargo_target_dir {
        env.push(("CARGO_TARGET_DIR", dir.display().to_string()));
    }
    env
}

/// How long one sandboxed git run in a seat clone may take. A seat-planted
/// filter or hook that never exits is killed and the probe reads "unknown".
// trace:TASK-2053 | ai:claude
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// What the host learned about a seat clone from [`probe`].
// trace:TASK-2053 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// The sandboxed git runs succeeded.
    Known(ProbeFacts),
    /// No answer (no bwrap, a timeout, a git failure). Every caller treats
    /// the clone as holding unshipped work and keeps it: fail closed.
    Unknown(String),
    /// The clone has submodules: work in them is neither counted nor
    /// salvaged, so every caller keeps the clone for the operator. The text
    /// is the full reason.
    HasSubmodules(String),
}

/// A seat clone's state, parsed from the output of git run inside the probe
/// sandbox. The clone is seat-controlled, so this is untrusted data: it can
/// only make the host keep or salvage MORE, never reach outside the clone.
// trace:TASK-2053 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProbeFacts {
    /// HEAD's commit; `None` for an unborn HEAD.
    pub(crate) head: Option<String>,
    /// The checked-out branch; `None` when HEAD is detached.
    pub(crate) branch: Option<String>,
    /// `git status --porcelain=v2` entries: changed and untracked files.
    pub(crate) dirty: Vec<String>,
    /// Commits reachable from any ref or stash entry that are on no
    /// `origin/*` ref, not in the clone's base and not reachable from a ref of
    /// the main checkout.
    // Clean, published heads exempt slice checkpoint tags; teardown salvages them.
    // trace:BUG-2491 | ai:codex
    pub(crate) unpushed: u64,
    /// The refs those commits are on, as `<count> commit(s) on <ref>` lines
    /// (seat-written names, escaped for display), for the refusal message.
    // trace:BUG-2224 | ai:claude
    pub(crate) unpushed_refs: Vec<String>,
}

impl ProbeFacts {
    /// Whether removing the clone now would lose work origin does not have.
    pub(crate) fn unshipped(&self) -> bool {
        !self.dirty.is_empty() || self.unpushed > 0
    }
}

/// The most a probe run may print. `git status` of a clone with very many
/// changed files stays far below this; more means a seat-planted program is
/// streaming, and the probe reads "unknown".
// trace:TASK-2053 | ai:claude
const PROBE_OUTPUT_CAP: u64 = 16 * 1024 * 1024;

/// The most [`salvage`] writes. A larger patch is cut off, says so, and the
/// clone is kept.
// trace:TASK-2053 | ai:claude
const SALVAGE_OUTPUT_CAP: u64 = 64 * 1024 * 1024;

/// The repository config git reads inside the probe sandbox, bound read-only
/// over the clone's own `.git/config`. It defines no filter, diff, textconv
/// or fsmonitor driver and no include, so attributes a seat plants in its
/// clone name drivers that do not exist and run nothing in the clone itself.
/// It does not replace a submodule's own `.git/modules/<name>/config`: git
/// run in a submodule reads that seat-written config, and its drivers run,
/// inside the sandbox only (hooks and fsmonitor stay off). [`probe`] keeps
/// any clone with submodules before running anything that recurses into
/// one, so only a submodule that appears after the probe is reached. The
/// object format comes from MAIN (the clone borrows main's objects). The
/// one other repository extension a clone can be made with, `refStorage` (a
/// user's `init.defaultRefFormat`), is carried over from the clone's config
/// only as an allowlisted value, `reftable`; anything else is left out,
/// which can only make the probe fail and the host keep the clone.
// trace:TASK-2053 | ai:claude
fn probe_repo_config(main: &Path, clone: &Path) -> String {
    let mut cmd = git(main);
    cmd.args(["rev-parse", "--show-object-format"]);
    let mut extensions = Vec::new();
    if run(cmd, "read the object format").is_ok_and(|format| format == "sha256") {
        extensions.push("\tobjectformat = sha256\n");
    }
    if clone_ref_storage(main, clone).as_deref() == Some("reftable") {
        extensions.push("\trefstorage = reftable\n");
    }
    if extensions.is_empty() {
        return "[core]\n\trepositoryformatversion = 0\n\tbare = false\n".to_string();
    }
    format!(
        "[core]\n\trepositoryformatversion = 1\n\tbare = false\n[extensions]\n{}",
        extensions.concat()
    )
}

/// `extensions.refStorage` from the clone's own `.git/config`, lowercased.
/// The seat-written file is read only as a regular file (never a FIFO or
/// through a symlink) and parsed by `git config --file` on a host-side copy,
/// which follows no include.
// trace:TASK-2053 | ai:claude
fn clone_ref_storage(main: &Path, clone: &Path) -> Option<String> {
    let body = aida_core::seat_clone_detect::read_regular_file(&clone.join(".git").join("config"))?;
    let copy = tempfile::NamedTempFile::new().ok()?;
    std::fs::write(copy.path(), body).ok()?;
    let mut cmd = git(main);
    cmd.args(["config", "--file"])
        .arg(copy.path())
        .args(["--get", "extensions.refstorage"]);
    run(cmd, "read the clone's ref storage")
        .ok()
        .map(|value| value.to_ascii_lowercase())
}

/// The bwrap argv that runs a program against `clone` for the host: no
/// network, no other namespace, a fresh session, a private `/tmp`, torn down
/// with the host process, and never more of the file system than the seat
/// itself may read. With `[contained] read_allowlist` (read from MAIN's
/// config) only the essential system paths and the allowlist are visible, as
/// for the seat; without it the file system is read-only. The clone and
/// main's `.git` (which the clone borrows objects from) are bound read-only
/// after the `/tmp` mount, and `repo_config` (see [`probe_repo_config`]) over
/// the clone's own config. `GIT_DIR` and `GIT_COMMON_DIR` are set, so a
/// seat-planted `.git/commondir` cannot point git at another config. The
/// environment is cleared: no user or system git config, and command-scope
/// config that disables hooks and fsmonitor.
// trace:TASK-2053 | ai:claude
pub(crate) fn probe_sandbox_args(
    clone: &Path,
    main: &Path,
    read_allowlist: &[String],
    repo_config: &Path,
) -> Vec<std::ffi::OsString> {
    sandbox_args(clone, main, read_allowlist, repo_config, &[])
}

/// TASK-2712: [`probe_sandbox_args`] for the host's commit of a native
/// Codex seat's pending work. Identical, except the clone (its `.git`
/// included) is bound read-WRITE and the commit identity is set in the
/// cleared environment. Main's `.git` stays read-only and `repo_config`
/// stays bound read-only over the clone's own config, so no seat-written
/// config, hook, filter or fsmonitor runs, and there is still no network.
/// When the clone's read-only bind cannot be found (the probe sandbox's
/// shape changed), this is an error: a commit sandbox that silently kept the
/// clone read-only would fail later with a misleading git error.
// trace:TASK-2712 | ai:claude
pub(crate) fn commit_sandbox_args(
    clone: &Path,
    main: &Path,
    read_allowlist: &[String],
    repo_config: &Path,
    identity: &[(&'static str, std::ffi::OsString)],
) -> Result<Vec<std::ffi::OsString>> {
    let args = sandbox_args(clone, main, read_allowlist, repo_config, identity);
    writable_clone_bind(args, clone)
}

/// `args` with the clone's `--ro-bind <clone> <clone>` turned into a
/// writable `--bind`: exactly one such bind must exist.
// trace:TASK-2712 | ai:claude
pub(crate) fn writable_clone_bind(
    mut args: Vec<std::ffi::OsString>,
    clone: &Path,
) -> Result<Vec<std::ffi::OsString>> {
    let clone_os = clone.as_os_str();
    let binds: Vec<usize> = args
        .windows(3)
        .enumerate()
        .filter(|(_, w)| w[0] == "--ro-bind" && w[1] == *clone_os && w[2] == *clone_os)
        .map(|(at, _)| at)
        .collect();
    anyhow::ensure!(
        binds.len() == 1,
        "refusing to commit the seat's pending work: the commit sandbox found {} read-only \
         binds of {} (expected exactly one), so it cannot make the clone writable. Nothing \
         was committed",
        binds.len(),
        clone.display()
    );
    args[binds[0]] = "--bind".into();
    Ok(args)
}

/// TASK-2712: `(device, inode)` of `dir`, which must be a real directory
/// (never a symbolic link).
// trace:TASK-2712 | ai:claude
pub(crate) fn dir_identity(dir: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("could not read the seat clone {}", dir.display()))?;
    anyhow::ensure!(
        meta.file_type().is_dir(),
        "refusing to commit the seat's pending work: {} is not a directory (a symbolic link \
         or a replaced path). Nothing was committed",
        dir.display()
    );
    Ok((meta.dev(), meta.ino()))
}

/// TASK-2712: refuse when `dir` is no longer the directory pinned as
/// `pinned` (it was replaced or swapped for a symbolic link since it was
/// checked).
// trace:TASK-2712 | ai:claude
pub(crate) fn ensure_dir_identity(dir: &Path, pinned: (u64, u64)) -> Result<()> {
    anyhow::ensure!(
        dir_identity(dir)? == pinned,
        "refusing to commit the seat's pending work: {} was replaced after it was checked. \
         Nothing was committed",
        dir.display()
    );
    Ok(())
}

// trace:TASK-2053 trace:TASK-2712 | ai:claude
fn sandbox_args(
    clone: &Path,
    main: &Path,
    read_allowlist: &[String],
    repo_config: &Path,
    extra_env: &[(&'static str, std::ffi::OsString)],
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["--unshare-all", "--die-with-parent", "--new-session"]
        .iter()
        .map(Into::into)
        .collect();
    if read_allowlist.is_empty() {
        args.extend(["--ro-bind".into(), "/".into(), "/".into()]);
    } else {
        let paths = crate::session::strict_read_essential_paths()
            .iter()
            .map(|p| p.to_string())
            .chain(read_allowlist.iter().cloned());
        for path in paths {
            args.extend(["--ro-bind-try".into(), path.clone().into(), path.into()]);
        }
    }
    args.extend(
        ["--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]
            .iter()
            .map(Into::into),
    );
    let git_dir = clone.join(".git");
    for path in [clone.to_path_buf(), main.join(".git")] {
        args.extend(["--ro-bind".into(), path.clone().into(), path.into()]);
    }
    args.extend([
        "--ro-bind".into(),
        repo_config.as_os_str().to_os_string(),
        git_dir.join("config").into(),
        "--chdir".into(),
        clone.as_os_str().to_os_string(),
        "--clearenv".into(),
    ]);
    let host_path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let env: [(&str, std::ffi::OsString); 16] = [
        ("PATH", host_path),
        ("HOME", "/nonexistent".into()),
        ("LC_ALL", "C".into()),
        ("GIT_DIR", git_dir.clone().into()),
        ("GIT_COMMON_DIR", git_dir.into()),
        ("GIT_WORK_TREE", clone.into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_TERMINAL_PROMPT", "0".into()),
        ("GIT_OPTIONAL_LOCKS", "0".into()),
        ("GIT_CONFIG_COUNT", "2".into()),
        ("GIT_CONFIG_KEY_0", "core.hooksPath".into()),
        ("GIT_CONFIG_VALUE_0", "/dev/null".into()),
        ("GIT_CONFIG_KEY_1", "core.fsmonitor".into()),
        ("GIT_CONFIG_VALUE_1", "false".into()),
        (ACCESSOR_ENV, "1".into()),
    ];
    for (name, value) in env.into_iter().chain(extra_env.iter().cloned()) {
        args.extend(["--setenv".into(), name.into(), value]);
    }
    args.push("--".into());
    args
}

/// Why a sandboxed run gave no usable answer.
// trace:TASK-2053 | ai:claude
#[derive(Debug)]
enum SandboxFailure {
    /// No bwrap, a spawn failure, a timeout or a non-zero exit.
    Failed(String),
    /// More than `cap` bytes on stdout: the first `cap` bytes.
    Truncated(Vec<u8>),
}

impl std::fmt::Display for SandboxFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxFailure::Failed(why) => f.write_str(why),
            SandboxFailure::Truncated(kept) => {
                write!(
                    f,
                    "the sandboxed output passed {} bytes and was cut off",
                    kept.len()
                )
            }
        }
    }
}

/// Operator diagnostics for a refused force-claim, through the same bounded
/// read-only sandbox as the state probe. Include staged and unstaged changes
/// against HEAD; untracked paths are named by the accompanying dirty listing.
// trace:BUG-2455 | ai:codex
pub(crate) fn pending_diff_stat(main: &Path, clone: &Path) -> Result<String> {
    anyhow::ensure!(
        host_recorded_clone(main, clone).is_some(),
        "the host does not recognise this seat clone"
    );
    let args: Vec<&std::ffi::OsStr> = [
        "diff",
        "--stat",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
        "--",
    ]
    .iter()
    .map(std::ffi::OsStr::new)
    .collect();
    let out = run_sandboxed(clone, main, "git", &args, PROBE_OUTPUT_CAP)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let text = String::from_utf8_lossy(&out).trim().replace('\n', "; ");
    Ok(if text.is_empty() {
        "no tracked changes".into()
    } else {
        text
    })
}

/// Run `program args…` in the probe sandbox for `clone`, keeping at most
/// `cap` bytes of its stdout.
// trace:TASK-2053 | ai:claude
fn run_sandboxed(
    clone: &Path,
    main: &Path,
    program: &str,
    args: &[&std::ffi::OsStr],
    cap: u64,
) -> std::result::Result<Vec<u8>, SandboxFailure> {
    run_sandboxed_until(clone, main, program, args, cap, None)
}

// trace:TASK-2053 | ai:codex
fn run_sandboxed_until(
    clone: &Path,
    main: &Path,
    program: &str,
    args: &[&std::ffi::OsStr],
    cap: u64,
    deadline: Option<std::time::Instant>,
) -> std::result::Result<Vec<u8>, SandboxFailure> {
    run_sandboxed_as(clone, main, program, args, cap, deadline, None)
}

/// TASK-2712: [`run_sandboxed`] in the commit sandbox
/// ([`commit_sandbox_args`]): the clone writable, `identity` set, hooks off.
// trace:TASK-2712 | ai:claude
fn run_commit_sandboxed(
    clone: &Path,
    main: &Path,
    args: &[&std::ffi::OsStr],
    identity: &[(&'static str, std::ffi::OsString)],
) -> std::result::Result<Vec<u8>, SandboxFailure> {
    run_sandboxed_as(
        clone,
        main,
        "git",
        args,
        PROBE_OUTPUT_CAP,
        None,
        Some(identity),
    )
}

/// The one sandbox runner: the probe sandbox when `commit_identity` is
/// `None`, the commit sandbox with that identity otherwise.
// trace:TASK-2053 trace:TASK-2712 | ai:claude
fn run_sandboxed_as(
    clone: &Path,
    main: &Path,
    program: &str,
    args: &[&std::ffi::OsStr],
    cap: u64,
    deadline: Option<std::time::Instant>,
    commit_identity: Option<&[(&'static str, std::ffi::OsString)]>,
) -> std::result::Result<Vec<u8>, SandboxFailure> {
    let failed = |why: String| SandboxFailure::Failed(why);
    let repo_config = tempfile::NamedTempFile::new()
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(probe_repo_config(main, clone).as_bytes())?;
            Ok(file)
        })
        .map_err(|error| failed(format!("could not write the probe's git config: {error}")))?;
    let read_allowlist = crate::session::contained_read_allowlist(main);
    let sandbox = match commit_identity {
        None => probe_sandbox_args(clone, main, &read_allowlist, repo_config.path()),
        Some(identity) => {
            commit_sandbox_args(clone, main, &read_allowlist, repo_config.path(), identity)
                .map_err(|e| failed(format!("{e:#}")))?
        }
    };
    let timeout = deadline
        .map(|end| {
            end.saturating_duration_since(std::time::Instant::now())
                .min(PROBE_TIMEOUT)
        })
        .unwrap_or(PROBE_TIMEOUT);
    if timeout.is_zero() {
        return Err(failed("the clone scan deadline elapsed".into()));
    }
    let output = retry_readonly_mount_race(commit_identity.is_none(), timeout, |remaining| {
        let mut cmd = std::process::Command::new("bwrap");
        cmd.args(&sandbox)
            .arg(program)
            .args(args)
            .stdin(std::process::Stdio::null());
        crate::command_output_with_timeout_capped(cmd, remaining, cap)
    });
    match output {
        crate::BoundedCommandOutput::Completed(mut out) if out.stdout.len() as u64 > cap => {
            out.stdout.truncate(cap as usize);
            Err(SandboxFailure::Truncated(out.stdout))
        }
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(out.stdout),
        crate::BoundedCommandOutput::Completed(out)
            if program == "git"
                && repair_merge_tree_conflict(
                    &args
                        .iter()
                        .map(|arg| arg.to_str().unwrap_or(""))
                        .collect::<Vec<_>>(),
                    out.status.code(),
                    &out.stdout,
                ) =>
        {
            Ok(out.stdout)
        }
        crate::BoundedCommandOutput::Completed(out) => Err(failed(format!(
            "the sandboxed {program} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
        crate::BoundedCommandOutput::SpawnFailed => Err(failed(
            "bwrap is not available to sandbox the probe".to_string(),
        )),
        crate::BoundedCommandOutput::TimedOut => Err(failed(format!(
            "the sandboxed {program} did not finish within {timeout:?}"
        ))),
    }
}

// Bubblewrap can race a disappearing systemd credentials mount while making
// the root bind read-only. Retry setup only, with the original time budget;
// commit runs and arbitrary command failures must never be replayed.
// trace:STORY-2493 | ai:codex
fn retry_readonly_mount_race(
    readonly: bool,
    timeout: std::time::Duration,
    mut run: impl FnMut(std::time::Duration) -> crate::BoundedCommandOutput,
) -> crate::BoundedCommandOutput {
    let start = std::time::Instant::now();
    for attempt in 0..3 {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return crate::BoundedCommandOutput::TimedOut;
        }
        let output = run(remaining);
        let retry = match &output {
            crate::BoundedCommandOutput::Completed(out) if readonly && out.stdout.is_empty() => {
                // Bubblewrap exposes this pre-exec mount failure only through stderr.
                // external-prose-classifier: seat_clone::retry_readonly_mount_race
                let stderr = String::from_utf8_lossy(&out.stderr);
                out.status.code() == Some(1)
                    && stderr.starts_with("bwrap: Can't bind mount /oldroot/ on /newroot/:")
                    && stderr.contains(
                        "Unable to apply mount flags: remount \"/newroot/run/credentials/",
                    )
                    && stderr.trim_end().ends_with("No such file or directory")
            }
            _ => false,
        };
        if !retry || attempt == 2 {
            return output;
        }
    }
    unreachable!()
}

/// [`run_sandboxed`] for a probe: text, at most [`PROBE_OUTPUT_CAP`].
// trace:TASK-2053 | ai:claude
fn probe_run(
    clone: &Path,
    main: &Path,
    args: &[&std::ffi::OsStr],
    program: &str,
) -> std::result::Result<String, String> {
    run_sandboxed(clone, main, program, args, PROBE_OUTPUT_CAP)
        .map(|out| String::from_utf8_lossy(&out).into_owned())
        .map_err(|failure| failure.to_string())
}

/// Dispatch inspection uses the same read-only sandbox even for old MCP
/// claims that did not record tree_kind or a branch. This grants no teardown
/// authority and never runs seat-written Git configuration on the host.
// trace:BUG-2473 | ai:codex
pub(crate) fn dispatch_snapshot(main: &Path, clone: &Path) -> Result<Vec<u8>> {
    anyhow::ensure!(
        aida_core::seat_clone_detect::has_real_git_dir(clone),
        "seat clone .git is not a real directory"
    );
    let script = r#"set -eu
branch=$(git symbolic-ref --quiet --short HEAD || true)
printf '%s\0' "$branch"
if git rev-parse --verify --quiet HEAD >/dev/null; then
  git log -1 --format='%at%x09%s'
else
  # Only an unborn symbolic HEAD is allowed to have no last commit.
  [ -n "$branch" ]
  if git show-ref --verify --quiet "refs/heads/$branch"; then exit 1; else
    [ "$?" -eq 1 ]
  fi
fi
printf '\0'
ahead=$(git rev-list --count origin/main..HEAD 2>/dev/null || git rev-list --count main..HEAD 2>/dev/null || printf '0')
printf '%s\0' "$ahead"
git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=all
"#;
    run_sandboxed_until(
        clone,
        main,
        "sh",
        &["-c".as_ref(), script.as_ref()],
        PROBE_OUTPUT_CAP,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

const PENDING_MERGE_PATH: &str = r#"set -eu
merge=$(git rev-parse --git-path MERGE_HEAD)
test -x "$(dirname -- "$merge")"
if [ -e "$merge" ] || [ -L "$merge" ]; then
    if [ "${1:-}" = parent ]; then
        test ! -L "$merge" && test -f "$merge" || exit 2
        printf "present:"
        cat -- "$merge"
    else
        printf pending
    fi
fi"#;

/// Read the merge pseudoref by path. Porcelain can be empty when the
/// staged resolution matches HEAD, and normal refs can also be named MERGE_HEAD.
// trace:BUG-2679 | ai:codex
pub(crate) fn has_pending_merge(main: &Path, tree: &Path) -> Result<bool> {
    let timeout = std::time::Duration::from_secs(5);
    if let Some(seat) = host_recorded_clone(main, tree) {
        let out = run_sandboxed_until(
            &seat.root,
            &seat.main,
            "sh",
            &["-c".as_ref(), PENDING_MERGE_PATH.as_ref()],
            PROBE_OUTPUT_CAP,
            Some(std::time::Instant::now() + timeout),
        )
        .map_err(|why| anyhow::anyhow!("could not inspect seat merge state: {why}"))?;
        return Ok(!out.is_empty());
    }
    let mut cmd = aida_core::git_ops::host_git_command(tree);
    cmd.args([
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "MERGE_HEAD",
    ]);
    let path = match crate::command_output_with_timeout_detail(cmd, timeout) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => out.stdout,
        crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
            "could not inspect merge state: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        _ => anyhow::bail!("could not inspect merge state within the Git probe budget"),
    };
    use std::os::unix::ffi::OsStrExt;
    let path = path.strip_suffix(b"\n").context("missing merge path")?;
    let path = Path::new(std::ffi::OsStr::from_bytes(path));
    anyhow::ensure!(path.is_absolute(), "invalid merge path");
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("could not inspect merge state"),
    }
}

/// Read only HEAD reachability for final lane reporting, inside the clone sandbox.
// trace:BUG-2464 | ai:codex
pub(crate) fn head_has_unpushed_commits(main: &Path, clone: &Path) -> Option<bool> {
    let out = run_sandboxed_until(
        clone,
        main,
        "git",
        &[
            "rev-list".as_ref(),
            "--max-count=1".as_ref(),
            "HEAD".as_ref(),
            "--not".as_ref(),
            "--remotes".as_ref(),
        ],
        PROBE_OUTPUT_CAP,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
    )
    .ok()?;
    Some(!out.is_empty())
}

/// Whether the leased branch contains work beyond the host-pinned seat base.
// trace:BUG-2488 | ai:codex
pub(crate) fn has_commits_beyond_base(main: &Path, clone: &Path) -> Result<bool> {
    let root = clone.canonicalize()?;
    let lease = crate::list_leases(main)
        .into_iter()
        .find(|lease| {
            lease.tree_kind == TreeKind::Clone
                && lease
                    .worktree_path
                    .canonicalize()
                    .is_ok_and(|path| path == root)
        })
        .context("could not read seat branch lease")?;
    let seat = host_recorded_clone(main, clone).context("could not validate seat clone")?;
    branch_has_commits_beyond_base(&seat, &lease.branch)
}

// trace:BUG-2488 | ai:codex
pub(crate) fn branch_has_commits_beyond_base(
    seat: &HostRecordedClone,
    branch: &str,
) -> Result<bool> {
    let main = &seat.main;
    let clone = &seat.root;
    let base = pinned_base(main, clone).context("could not read seat base pin")?;
    let branch = format!("refs/heads/{branch}");
    let out = run_sandboxed_until(
        clone,
        main,
        "git",
        &[
            "rev-list".as_ref(),
            "--max-count=1".as_ref(),
            branch.as_ref(),
            "--not".as_ref(),
            base.as_ref(),
        ],
        PROBE_OUTPUT_CAP,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
    )
    .map_err(|why| anyhow::anyhow!("could not inspect seat commits: {why}"))?;
    Ok(!out.is_empty())
}

/// Count commits held by the seat but by no remote: HEAD and the lease branch
/// for worktrees, all local branches for private clones. Unlike salvage
/// accounting, a local host ref or base pin never excuses a commit.
/// Clone Git stays inside the existing read-only probe sandbox.
// trace:BUG-2450 | ai:codex
pub(crate) fn remote_missing_commits(
    main: &Path,
    seat: &Path,
    is_clone: bool,
    lease_branch: &str,
) -> Result<u64> {
    remote_missing_commits_with_review_head(main, seat, is_clone, lease_branch, None)
}

/// A freshly fetched forge head can cover a linked reviewer checkout too.
/// Private clones retain their ordinary remote-only accounting.
// trace:BUG-2469 | ai:codex
pub(crate) fn remote_missing_commits_with_review_head(
    main: &Path,
    seat: &Path,
    is_clone: bool,
    lease_branch: &str,
    review_head: Option<&str>,
) -> Result<u64> {
    anyhow::ensure!(
        !is_clone || review_head.is_none(),
        "review coverage requires a linked worktree"
    );
    remote_missing_commits_via(main, seat, is_clone, lease_branch, review_head, "--remotes")
}

/// GC requires origin coverage, rather than a secondary backup remote.
// trace:TASK-1222 | ai:codex
pub(crate) fn origin_missing_commits(main: &Path, seat: &Path, branch: &str) -> Result<u64> {
    remote_missing_commits_via(main, seat, true, branch, None, "--remotes=origin")
}

fn remote_missing_commits_via(
    main: &Path,
    seat: &Path,
    is_clone: bool,
    lease_branch: &str,
    review_head: Option<&str>,
    remotes: &str,
) -> Result<u64> {
    let deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
    // A remote tip absent from the clone cannot excuse anything there. Git
    // failures (including a broken HEAD) refuse rather than guessing zero.
    let script = r#"set -eu
head=''
if git rev-parse --verify HEAD >/dev/null 2>&1; then
  head=HEAD
else
  branch=$(git symbolic-ref -q HEAD) || exit 1
  if git show-ref --verify --quiet "$branch"; then exit 1; else
    code=$?
    [ "$code" -eq 1 ] || exit 1
  fi
fi
negative=''
if [ "$#" -gt 0 ]; then
  objects=$(printf '%s\n' "$@" | git cat-file --batch-check='%(objectname) %(objecttype)')
  negative=$(printf '%s\n' "$objects" | awk '$2 == "commit" {print "^" $1}')
fi
printf '%s\n' "$negative" | git rev-list --stdin --count $head --branches --not --remotes
"#;
    let script = if remotes == "--remotes=origin" {
        // Inherited host tags are not seat work. Exclude only an exact
        // name/object match from positive roots, never from negative coverage:
        // a seat branch at the same unpublished commit must still retain it.
        // trace:TASK-1222 | ai:codex
        let mut tags = aida_core::git_ops::host_git_command(main);
        tags.args([
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/tags/",
        ]);
        let host_tags = run_bounded(tags, "", deadline).context("cannot read host tags")?;
        let host_tags = host_tags.replace('\'', "'\"'\"'");
        let positives = format!(
            r#"host_tags='{host_tags}'
positives=''
refs=$(git for-each-ref --format='%(objectname) %(refname)')
while read -r oid name; do
  [ -n "$oid" ] || continue
  case "$name" in
    refs/tags/*)
      if printf '%s\n' "$host_tags" | grep -Fqx -- "$oid $name"; then continue; fi ;;
  esac
  positives="$positives $oid"
done <<EOF
$refs
EOF
"#
        );
        // Preserve seat-created/moved tags, other refs and every stash entry.
        // Secondary remotes never grant origin coverage to GC.
        format!(
            "set -eu\n{}\n{}\n{}",
            GC_STASH_ENTRIES,
            positives,
            script
                .replace("--branches", "$positives $s")
                // Clone tracking refs are seat-writable. Only the negative
                // OIDs corroborated through host origin refs/advertisement
                // above may excuse a commit in the GC path.
                .replace("--not --remotes", "")
        )
    } else {
        script.to_string()
    };
    let output = if is_clone {
        let out = run_sandboxed_until(
            seat,
            main,
            "sh",
            &["-c".as_ref(), REF_TIPS_SCRIPT.as_ref()],
            PROBE_OUTPUT_CAP,
            deadline,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
        let out = String::from_utf8_lossy(&out);
        let (tips, candidates) = split_tips(&out);
        let remote_tips =
            main_reachable_commits_via(main, tips.into_iter().chain(candidates), deadline, remotes);
        let count_with = |tips: &[String]| -> Result<String> {
            let mut args: Vec<&std::ffi::OsStr> = vec![
                "-c".as_ref(),
                script.as_ref(),
                "aida-remote-missing".as_ref(),
            ];
            args.extend(tips.iter().map(|s| std::ffi::OsStr::new(s)));
            let out = run_sandboxed_until(seat, main, "sh", &args, PROBE_OUTPUT_CAP, deadline)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(String::from_utf8(out)?)
        };
        let first = count_with(&remote_tips)?;
        if first.trim() == "0" {
            first
        } else {
            // Explicit-refspec pushes need not leave tracking refs in either
            // checkout. Use the same bounded origin-tip evidence as the
            // existing clone probe; host-only refs still excuse nothing.
            // trace:BUG-2450 | ai:codex
            let remaining = deadline
                .unwrap()
                .saturating_duration_since(std::time::Instant::now());
            let timeout = (remaining / 2).min(ORIGIN_REFRESH_CAP);
            let heads = if remotes == "--remotes=origin" {
                origin_gc_oids(main, timeout)
            } else {
                origin_head_oids(main, timeout)
            };
            if heads.is_empty() {
                first
            } else {
                let tips: Vec<String> = remote_tips.into_iter().chain(heads).collect();
                count_with(&tips)?
            }
        }
    } else {
        let mut cmd = aida_core::git_ops::host_git_command(seat);
        // Use Git directly for ordinary worktrees; the negative tips are
        // already in their shared object database and remote namespace.
        let mut head = aida_core::git_ops::host_git_command(seat);
        head.args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
        let has_head = run_bounded(head, "", deadline).is_some();
        if !has_head {
            let mut symbolic = aida_core::git_ops::host_git_command(seat);
            symbolic.args(["symbolic-ref", "--quiet", "HEAD"]);
            let branch = run_bounded(symbolic, "", deadline).context("cannot read HEAD")?;
            let mut exists = aida_core::git_ops::host_git_command(seat);
            exists.args(["show-ref", "--verify", "--quiet", branch.trim()]);
            let out = crate::command_output_with_timeout(exists, std::time::Duration::from_secs(5))
                .context("cannot validate unborn HEAD")?;
            anyhow::ensure!(out.status.code() == Some(1), "HEAD is unreadable");
        }
        // Worktrees share refs/heads with every other seat. Only this
        // seat's current HEAD and its recorded branch belong to this lease.
        // HEAD also covers a different branch currently checked out here.
        // trace:BUG-2450 | ai:codex
        let lease_ref = format!("refs/heads/{lease_branch}");
        let mut exists = aida_core::git_ops::host_git_command(seat);
        exists.args(["show-ref", "--verify", "--quiet", &lease_ref]);
        let status = crate::command_output_with_timeout(exists, std::time::Duration::from_secs(5))
            .context("cannot validate lease branch")?
            .status;
        anyhow::ensure!(
            matches!(status.code(), Some(0 | 1)),
            "cannot read lease branch"
        );
        let has_branch = status.success();
        if !has_head && !has_branch {
            return Ok(0);
        }
        cmd.args(["rev-list", "--count"]);
        if has_branch {
            cmd.arg(&lease_ref);
        }
        if has_head {
            cmd.arg("HEAD");
        }
        cmd.args(["--not", remotes]);
        if let Some(head) = review_head {
            cmd.arg(head);
        }
        run_bounded(cmd, "", deadline).context("cannot count commits absent from remotes")?
    };
    output
        .trim()
        .parse::<u64>()
        .context("invalid remote-missing commit count")
}

/// The commit `refs/aida/seats/<dir>` pins in `main`: the clone's base, read
/// from main (host-owned), never from the clone.
// trace:TASK-2053 | ai:claude
pub(crate) fn pinned_base(main: &Path, clone: &Path) -> Option<String> {
    pinned_base_until(main, clone, None)
}

// trace:BUG-2384 | ai:codex
fn pinned_base_until(
    main: &Path,
    clone: &Path,
    deadline: Option<std::time::Instant>,
) -> Option<String> {
    let pin = pin_ref(clone)?;
    let mut cmd = git(main);
    cmd.args([
        "rev-parse",
        "--verify",
        "--quiet",
        crate::git_arg_guard::END_OF_OPTIONS,
        &format!("{pin}^{{commit}}"),
    ]);
    run_bounded(cmd, "", deadline)
        .map(|s| s.trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// Read a seat clone's state for the host. Git runs only inside the probe
/// sandbox ([`probe_sandbox_args`]); a seat-planted filter or driver that
/// runs there (see [`probe_repo_config`]) can read but not write, and has no
/// network. A clone with any submodule evidence ([`SUBMODULE_EVIDENCE_SCRIPT`])
/// reads as [`Probe::HasSubmodules`] before any other git runs.
/// `main` is the main checkout from the LEASE, never from the clone's marker.
// trace:TASK-2053 | ai:claude
pub(crate) fn probe(main: &Path, clone: &Path) -> Probe {
    probe_with_shipped(main, clone, &[])
}

/// [`probe`], with `shipped` commits (the heads of the clone branch's merged
/// PRs, from the forge) counted as on origin. A squash merge puts none of the
/// branch's commits on origin, and once the seat prunes `origin/<branch>`
/// every one of them would otherwise read as unpushed. Only full hex ids the
/// clone has as commits are used; work on other refs, or past a merged head,
/// still counts.
// trace:TASK-2053 trace:TASK-2110 | ai:claude
pub(crate) fn probe_with_shipped(main: &Path, clone: &Path, shipped: &[String]) -> Probe {
    probe_until(main, clone, shipped, None)
}

/// Environment variable through which `aida pr ship`'s cleanup worker hands
/// its per-clone scan budget (whole seconds) to the `aida session end` it runs.
// trace:BUG-2384 | ai:claude
pub(crate) const SHIP_CLEANUP_PROBE_ENV: &str = "AIDA_SHIP_CLEANUP_PROBE_SECS";

/// Per-clone scan budget during `aida pr ship` post-merge cleanup.
// trace:BUG-2384 | ai:claude
pub(crate) const SHIP_CLEANUP_PROBE_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(10);

static SHIP_CLEANUP_SCOPE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Called by the ship cleanup worker: its in-process reap and prune scans get
/// the ship cleanup budget. Every other probe caller (doctor, seat destroy and
/// list, salvage) keeps its own deadline and stays silent.
// trace:BUG-2384 | ai:claude
pub(crate) fn scope_probe_budget_to_ship_cleanup() {
    SHIP_CLEANUP_SCOPE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The ship cleanup scan budget, when this process runs inside ship cleanup
/// (the worker itself, or a `session end` it launched); `None` otherwise.
// trace:BUG-2384 | ai:claude
fn ship_cleanup_probe_budget() -> Option<std::time::Duration> {
    if SHIP_CLEANUP_SCOPE.load(std::sync::atomic::Ordering::Relaxed) {
        return Some(SHIP_CLEANUP_PROBE_BUDGET);
    }
    let secs = std::env::var(SHIP_CLEANUP_PROBE_ENV).ok()?;
    secs.trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
}

/// `deadline`, tightened to the ship cleanup budget when inside ship cleanup.
// trace:BUG-2384 | ai:claude
fn ship_cleanup_cap(deadline: Option<std::time::Instant>) -> Option<std::time::Instant> {
    let Some(budget) = ship_cleanup_probe_budget() else {
        return deadline;
    };
    let scan_end = std::time::Instant::now() + budget;
    Some(deadline.map_or(scan_end, |end| end.min(scan_end)))
}

/// [`ship_cleanup_cap`], plus the progress line ship cleanup prints per clone.
// trace:BUG-2384 | ai:claude
fn ship_cleanup_deadline(
    deadline: Option<std::time::Instant>,
    clone: &Path,
) -> Option<std::time::Instant> {
    if ship_cleanup_probe_budget().is_none() {
        return deadline;
    }
    eprintln!("  inspecting seat clone {} (bounded scan)", clone.display());
    ship_cleanup_cap(deadline)
}

// One deadline covers all four sandbox calls and the host git that checks the
// clone's ref tips against main, rather than a fresh budget per call.
// trace:TASK-2053 | ai:codex
// trace:BUG-2224 | ai:claude
// trace:BUG-2454 | ai:codex
pub(crate) fn probe_until(
    main: &Path,
    clone: &Path,
    shipped: &[String],
    deadline: Option<std::time::Instant>,
) -> Probe {
    // trace:BUG-2384 | ai:claude
    let deadline = ship_cleanup_deadline(deadline, clone);
    let probe_run = |clone: &Path, main: &Path, args: &[&std::ffi::OsStr], program: &str| {
        run_sandboxed_until(clone, main, program, args, PROBE_OUTPUT_CAP, deadline)
            .map(|out| String::from_utf8_lossy(&out).into_owned())
            .map_err(|failure| failure.to_string())
    };
    if !aida_core::seat_clone_detect::has_real_git_dir(clone) {
        return Probe::Unknown(format!(
            "{} has no .git directory of its own",
            clone.display()
        ));
    }
    match probe_run(
        clone,
        main,
        &["-c".as_ref(), SUBMODULE_EVIDENCE_SCRIPT.as_ref()],
        "sh",
    ) {
        Ok(found) if found.trim().is_empty() => {}
        Ok(found) => {
            return Probe::HasSubmodules(format!(
                "the clone has submodules ({}); automatic salvage does not cover them, so it is \
                 kept: inspect {} and remove it by hand",
                found.trim(),
                clone.display()
            ))
        }
        Err(why) => return Probe::Unknown(format!("submodule evidence: {why}")),
    }
    // `--ignore-submodules=none`: a seat's `.gitmodules` `ignore = dirty`
    // must not hide anything, should a submodule appear after the check.
    // trace:TASK-2053 | ai:claude
    let status = match probe_run(
        clone,
        main,
        &[
            "status".as_ref(),
            "--porcelain=v2".as_ref(),
            "--branch".as_ref(),
            "--untracked-files=all".as_ref(),
            "--ignore-submodules=none".as_ref(),
            // Match commit_pending: the host marker is never seat work.
            // trace:TASK-3749 | ai:codex
            "--".as_ref(),
            ".".as_ref(),
            format!(":(exclude){MARKER_RELATIVE}").as_ref(),
        ],
        "git",
    ) {
        Ok(out) => out,
        Err(why) => return Probe::Unknown(format!("working tree status: {why}")),
    };
    let mut facts = parse_status_v2(&status);
    // The sandbox's empty `.gitmodules` placeholder holds no work, so it
    // does not make a clean clone read as dirty.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-047 | ai:claude
    if is_empty_gitmodules_placeholder(clone) {
        facts.dirty.retain(|line| line != "? .gitmodules");
    }
    // The session's runtime links and build/cache output are not work.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
    facts.dirty.retain(|line| {
        !is_runtime_noise_entry(clone, line) && !is_mirrored_handshake_entry(main, clone, line)
    });
    let base = pinned_base_until(main, clone, deadline).unwrap_or_default();
    // BUG-2224: a seat clone inherits refs (tags, `wip/*` branches) whose
    // commits the main checkout already holds; removing the clone loses none
    // of them. The clone's ref tips main reaches from its own refs (host git,
    // in main only) are passed on like shipped heads; a seat-only commit on
    // top of one still counts. trace:BUG-2224 | ai:claude
    let tips = match probe_run(
        clone,
        main,
        &["-c".as_ref(), REF_TIPS_SCRIPT.as_ref()],
        "sh",
    ) {
        Ok(out) => out,
        Err(why) => return Probe::Unknown(format!("local ref tips: {why}")),
    };
    // trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
    let (ref_tips, candidates) = split_tips(&tips);
    let mut in_main = main_reachable_commits(main, ref_tips.into_iter(), deadline);
    in_main.extend(main_reachable_commits(
        main,
        candidates.into_iter(),
        deadline,
    ));
    let count = probe_run(
        clone,
        main,
        &[
            "-c".as_ref(),
            UNPUSHED_COUNT_SCRIPT.as_ref(),
            "aida-seat-unpushed".as_ref(),
            if facts.dirty.is_empty() {
                "ignore-slice"
            } else {
                "keep-slice"
            }
            .as_ref(),
            base.as_ref(),
        ]
        .into_iter()
        .chain(
            shipped
                .iter()
                .chain(in_main.iter())
                .filter(|oid| is_full_oid(oid))
                .map(|oid| oid.as_ref()),
        )
        .collect::<Vec<&std::ffi::OsStr>>(),
        "sh",
    );
    match count {
        Ok(out) => match parse_unpushed_count(&out) {
            Some((n, refs)) => {
                facts.unpushed = n;
                facts.unpushed_refs = refs;
            }
            None => return Probe::Unknown(format!("unreadable commit count `{}`", out.trim())),
        },
        Err(why) => return Probe::Unknown(format!("unpushed commit count: {why}")),
    }
    Probe::Known(facts)
}

/// [`REF_TIPS_SCRIPT`]'s output as (ref tips, candidate commits), split at
/// the [`TIPS_SEPARATOR`] line; output without one is all ref tips.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn split_tips(out: &str) -> (Vec<&str>, Vec<&str>) {
    let mut lines = out.lines().map(str::trim);
    let tips = lines
        .by_ref()
        .take_while(|line| *line != TIPS_SEPARATOR)
        .collect();
    (tips, lines.collect())
}

/// Why a clone with `facts` is kept: the work origin does not have, by ref.
// trace:BUG-2224 | ai:claude
pub(crate) fn unshipped_reason(facts: &ProbeFacts) -> String {
    // BUG-051: name every item, never an empty list.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
    let mut parts: Vec<String> = facts
        .dirty
        .iter()
        .map(|line| describe_status_entry(line))
        .collect();
    if facts.unpushed > 0 && facts.unpushed_refs.is_empty() {
        parts.push(format!("{} commit(s) on no listed ref", facts.unpushed));
    }
    parts.extend(facts.unpushed_refs.iter().cloned());
    if parts.is_empty() {
        "it holds work origin does not have".to_string()
    } else {
        format!("it holds work origin does not have: {}", parts.join("; "))
    }
}

/// Every item that makes a clone with `facts` unshipped, one per entry: each
/// changed or untracked path ([`describe_status_entry`]), then the commits
/// origin lacks with the refs holding them. Empty exactly when
/// [`ProbeFacts::unshipped`] is false.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn unshipped_items(facts: &ProbeFacts) -> Vec<String> {
    let mut items: Vec<String> = facts
        .dirty
        .iter()
        .map(|line| describe_status_entry(line))
        .collect();
    if facts.unpushed > 0 {
        items.push(format!(
            "{} commit(s) that origin does not have, on:",
            facts.unpushed
        ));
        if facts.unpushed_refs.is_empty() {
            items.push("  (no listed ref holds them)".to_string());
        } else {
            items.extend(facts.unpushed_refs.iter().map(|line| format!("  {line}")));
        }
    }
    items
}

/// A `git status --porcelain=v2` entry as `<kind>: <path>`, so a refusal
/// names the file rather than printing modes and object ids. A line that does
/// not parse is shown as it is. The path is seat-written, so it is escaped.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn describe_status_entry(line: &str) -> String {
    let field = |n: usize| line.splitn(n + 1, ' ').nth(n);
    let xy = line.split(' ').nth(1).unwrap_or("");
    let described = match line.as_bytes().first() {
        Some(b'?') => field(1).map(|p| format!("untracked: {}", p.escape_debug())),
        Some(b'!') => field(1).map(|p| format!("ignored: {}", p.escape_debug())),
        Some(b'1') => field(8).map(|p| format!("changed ({xy}): {}", p.escape_debug())),
        Some(b'2') => field(9).map(|p| match p.split_once('\t') {
            Some((to, from)) => format!(
                "renamed ({xy}): {} -> {}",
                from.escape_debug(),
                to.escape_debug()
            ),
            None => format!("renamed ({xy}): {}", p.escape_debug()),
        }),
        Some(b'u') => field(10).map(|p| format!("unmerged ({xy}): {}", p.escape_debug())),
        _ => None,
    };
    described.unwrap_or_else(|| line.escape_debug().to_string())
}

/// Whether an untracked probe entry is disposable runtime state: shared
/// store/codegraph links, agent/cache/build output, Python bytecode, notice
/// markers, empty sandbox placeholders, Claude runtime links, session-env.sh,
/// and the sessions symlink or direct regular `*.toml` lease files. Other
/// session files remain work; tracked entries never qualify.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn is_runtime_noise_entry(clone: &Path, line: &str) -> bool {
    let Some(path) = line.strip_prefix("? ") else {
        return false;
    };
    let path = path.trim_end_matches('/');
    // Only known runtime links/caches and empty sandbox placeholders qualify.
    // Genuine ignored state (holds, verdicts, credentials, notes) stays work.
    // trace:TASK-1222 | ai:codex
    // Python bytecode is regenerated by the gates. Keep arbitrary files even
    // inside __pycache__, and never excuse tracked status entries (above).
    if path.split('/').any(|part| part == "__pycache__") && path.ends_with(".pyc") {
        return true;
    }
    // Presentation/onboarding receipts are disposable; brief.md and events.jsonl
    // carry work context/audit evidence and intentionally remain protected.
    if matches!(
        path,
        ".aida/team-onboarded"
            | ".aida/upstream-report-notice-version"
            | ".aida/.format-hint-shown"
    ) {
        return true;
    }
    if matches!(path, ".bashrc" | ".gitconfig" | ".gitmodules") {
        return std::fs::symlink_metadata(clone.join(path))
            .is_ok_and(|m| m.is_file() && m.len() == 0);
    }
    if path.starts_with(".claude/") {
        let mut component = clone.join(path);
        while component != clone.join(".claude") {
            if std::fs::symlink_metadata(&component).is_ok_and(|m| m.is_symlink()) {
                return true;
            }
            if !component.pop() {
                break;
            }
        }
    }
    if path == ".aida/session-env.sh" {
        return true;
    }
    // A real sessions directory can contain operator notes. Only the shared
    // link or direct regular lease files are disposable; unknown state stays.
    // trace:TASK-1222 | ai:codex
    if path == ".aida/sessions" {
        return std::fs::symlink_metadata(clone.join(path)).is_ok_and(|meta| meta.is_symlink());
    }
    if let Some(lease) = path.strip_prefix(".aida/sessions/") {
        return !lease.contains('/')
            && lease.ends_with(".toml")
            && std::fs::symlink_metadata(clone.join(path)).is_ok_and(|meta| meta.is_file());
    }
    // trace:STORY-2630 | ai:codex
    if matches!(path, ".aida-store" | ".codegraph") {
        return std::fs::symlink_metadata(clone.join(path))
            .is_ok_and(|meta| meta.file_type().is_symlink());
    }
    // trace:TASK-2707 | ai:claude
    path.starts_with(".aida/cache.db")
        || path == ".aida/cache-db"
        || path.starts_with(".aida/cache-db/")
        || path == ".aida/agents"
        || path.starts_with(".aida/agents/")
        || path == "target"
        || path.starts_with("target/")
}

/// Handshakes are disposable only when the host already retains their evidence.
// trace:BUG-2488 | ai:codex
pub(crate) fn is_mirrored_handshake_entry(main: &Path, clone: &Path, line: &str) -> bool {
    let Some(path) = line.strip_prefix("? ") else {
        return false;
    };
    if path != ".aida/punts.jsonl"
        && !path.starts_with(".aida/punt-signals/")
        && !path.starts_with(".aida/pr-holds/")
    {
        return false;
    }
    // Keep cleanup probes bounded even if a seat swaps a FIFO or symlink.
    let read = aida_core::seat_clone_detect::read_regular_file;
    let Ok(Some(text)) = crate::punt::seat_handshake_text(clone, &clone.join(path)) else {
        return false;
    };
    if path.starts_with(".aida/punt-signals/") || path.starts_with(".aida/pr-holds/") {
        let Some(spec) = Path::new(path).file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let host = if path.starts_with(".aida/punt-signals/") {
            crate::punt::signal_path(main, spec)
        } else {
            crate::punt::hold_signal_path(main, spec)
        };
        return read(&host).is_some_and(|host| host == text);
    }
    let Ok(host) = std::fs::read_to_string(crate::punt::ledger_path(main)) else {
        return text.is_empty();
    };
    let existing: Vec<crate::punt::PuntRecord> = host
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    text.lines().all(|line| {
        serde_json::from_str::<crate::punt::PuntRecord>(line).is_ok_and(|record| {
            existing.iter().any(|r| {
                r.spec.eq_ignore_ascii_case(&record.spec)
                    && r.timestamp == record.timestamp
                    && r.category == record.category
            })
        })
    })
}

/// Bring origin's copy of `branch` into `main`'s `refs/remotes/origin/<branch>`
/// (host git in main only, bounded, no prompts), so a clone whose branch head
/// is an ancestor of it reads as pushed: [`main_reachable_commits`] excuses
/// tips main reaches. A seat publishes through main ([`crate::ship_from_seat`])
/// without a tracking ref in the clone, and origin's branch may since have
/// gained commits (an update-branch merge). Best effort: a failure, an absent
/// branch or a timeout changes nothing and the count stays conservative.
/// Returns whether main's tracking ref moved (only then can a re-read differ).
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn refresh_origin_branch(
    main: &Path,
    branch: &str,
    timeout: std::time::Duration,
) -> bool {
    let valid = !timeout.is_zero()
        && !branch.is_empty()
        && !branch.starts_with('-')
        && git(main)
            .args(["check-ref-format", "--branch", branch])
            .output()
            .is_ok_and(|out| out.status.success());
    if !valid {
        return false;
    }
    let tracking = format!("refs/remotes/origin/{branch}");
    let tip = || {
        let mut cmd = git(main);
        cmd.args(["rev-parse", "--verify", "--quiet", &tracking]);
        run(cmd, "read origin's branch in the main checkout").ok()
    };
    let before = tip();
    let mut fetch = aida_core::git_ops::host_remote_git_command(main, "origin");
    fetch
        .env(ACCESSOR_ENV, "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            crate::forge::batch_mode_ssh_command(main),
        )
        .stdin(std::process::Stdio::null())
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "origin",
        ])
        .arg(format!("+refs/heads/{branch}:{tracking}"));
    let fetched =
        crate::command_output_with_timeout(fetch, timeout).is_some_and(|out| out.status.success());
    fetched && tip() != before
}

/// The most one [`probe_counting_origin`] refresh fetch may take; it never
/// takes more than half the check's remaining budget either.
const ORIGIN_REFRESH_CAP: std::time::Duration = std::time::Duration::from_secs(20);

/// BUG-051: [`probe_until`] under the ancestor-of-origin rule: a branch head
/// that is an ancestor of origin's copy of the branch counts as pushed. Every
/// host reading that acts on whether a clone holds commits origin lacks
/// (session end, reap, the lease classifiers, salvage, phase 2) goes through
/// here, so they agree. The read-only status and awaiting surfaces call
/// [`probe_until`] directly: they apply the same rule against main's
/// last-fetched origin refs but never fetch or move them. A clone whose first reading
/// counts no such commits costs no network. Otherwise origin's copy of
/// `branch` (the host-recorded lease branch; the clone's checked-out branch
/// when there is none) is fetched into main, bounded by half the remaining
/// budget (`deadline`, else one [`PROBE_TIMEOUT`]) and [`ORIGIN_REFRESH_CAP`],
/// and the clone is read again under the same deadline. A fetch that moves
/// nothing, or a re-read with no answer, leaves the first (conservative)
/// reading standing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn probe_counting_origin(
    main: &Path,
    clone: &Path,
    branch: Option<&str>,
    shipped: &[String],
    deadline: Option<std::time::Instant>,
) -> Probe {
    // Inside ship cleanup, share the clone budget with an optional origin
    // refresh and second probe; other callers keep their own deadline.
    // trace:BUG-2384 | ai:claude
    let deadline = ship_cleanup_cap(deadline);
    let first = probe_until(main, clone, shipped, deadline);
    let Probe::Known(facts) = &first else {
        return first;
    };
    if facts.unpushed == 0 {
        return first;
    }
    let budget = || {
        let remaining = deadline.map_or(PROBE_TIMEOUT, |end| {
            end.saturating_duration_since(std::time::Instant::now())
        });
        (remaining / 2).min(ORIGIN_REFRESH_CAP)
    };
    let branch = branch
        .filter(|b| !b.is_empty())
        .map(str::to_owned)
        .or_else(|| facts.branch.clone());
    let mut best = first;
    if let Some(branch) = branch {
        if refresh_origin_branch(main, &branch, budget()) {
            if let again @ Probe::Known(_) = probe_until(main, clone, shipped, deadline) {
                best = again;
            }
        }
    }
    // BUG-2415: a seat that pushed with an explicit refspec
    // (`HEAD:refs/heads/<name>`) may have published under a name other than
    // the lease branch, and neither the clone nor main tracks it. Ask origin
    // for its real branch tips: a clone ref tip equal to one is on origin, and
    // so is everything it reaches. Any failure keeps the reading standing.
    // trace:BUG-2415 | ai:claude
    if matches!(&best, Probe::Known(facts) if facts.unpushed > 0) {
        let heads = origin_head_oids(main, budget());
        if !heads.is_empty() {
            let excused: Vec<String> = shipped.iter().cloned().chain(heads).collect();
            if let again @ Probe::Known(_) = probe_until(main, clone, &excused, deadline) {
                best = again;
            }
        }
    }
    best
}

/// The most origin branch tips [`origin_head_oids`] hands the probe; past it
/// none are used, and the count stays conservative.
const ORIGIN_HEADS_CAP: usize = 5_000;

/// The commit ids origin's branches point at right now (`git ls-remote
/// --heads origin`, host git in main only, bounded, no prompts), for
/// [`probe_counting_origin`] to excuse. Empty on any failure, a timeout, or
/// more than [`ORIGIN_HEADS_CAP`] branches.
// trace:BUG-2415 | ai:claude
pub(crate) fn origin_head_oids(main: &Path, timeout: std::time::Duration) -> Vec<String> {
    origin_oids(
        main,
        timeout,
        &["ls-remote", "--heads", "origin"],
        ORIGIN_HEADS_CAP,
    )
}

/// GC may corroborate a published source head through origin's PR/MR refs
/// after squash-merge branch deletion. These are host-observed advertisements,
/// never clone-written tracking refs; the separate lifecycle gate still applies.
// trace:TASK-1222 | ai:codex
fn origin_gc_oids(main: &Path, timeout: std::time::Duration) -> Vec<String> {
    origin_oids(
        main,
        timeout,
        &[
            "ls-remote",
            "--refs",
            "origin",
            "refs/heads/*",
            "refs/pull/*/head",
            "refs/merge-requests/*/head",
        ],
        50_000,
    )
}

fn origin_oids(
    main: &Path,
    timeout: std::time::Duration,
    args: &[&str],
    cap: usize,
) -> Vec<String> {
    if timeout.is_zero() {
        return Vec::new();
    }
    let mut cmd = aida_core::git_ops::host_remote_git_command(main, "origin");
    cmd.env(ACCESSOR_ENV, "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            crate::forge::batch_mode_ssh_command(main),
        )
        .stdin(std::process::Stdio::null())
        .args(args);
    let Some(out) = crate::command_output_with_timeout(cmd, timeout) else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut oids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|oid| is_full_oid(oid))
        .map(str::to_string)
        .collect();
    oids.sort_unstable();
    oids.dedup();
    if oids.len() > cap {
        return Vec::new();
    }
    oids
}

/// Parse [`UNPUSHED_COUNT_SCRIPT`]'s output: the total on the first line,
/// then `<count> <ref>` per ref holding any of those commits. The ref names
/// are seat-written, so they are escaped and the list is capped.
// trace:BUG-2224 | ai:claude
pub(crate) fn parse_unpushed_count(out: &str) -> Option<(u64, Vec<String>)> {
    const SHOWN: usize = 50;
    let mut lines = out.lines().map(str::trim).filter(|line| !line.is_empty());
    let total = lines.next()?.parse().ok()?;
    let mut refs = Vec::new();
    let mut more = 0usize;
    for line in lines {
        let (count, name) = line.split_once(' ')?;
        let count: u64 = count.parse().ok()?;
        if refs.len() < SHOWN {
            refs.push(format!("{count} commit(s) on {}", name.escape_debug()));
        } else {
            more += 1;
        }
    }
    if more > 0 {
        refs.push(format!("... and {more} more ref(s)"));
    }
    Some((total, refs))
}

/// The most clone ref tips [`main_reachable_commits`] checks; past it none
/// are excused, and the count stays conservative.
const MAIN_TIP_CAP: usize = 20_000;

/// Of the `tips` (commit ids a clone's refs point at, as the probe read them:
/// untrusted), those that are commits reachable from a ref of `main`. Git runs
/// in main only, bounded by `deadline` (and [`PROBE_TIMEOUT`]). Anything that
/// cannot be checked in time is left out, so it counts.
// trace:BUG-2224 | ai:claude
pub(crate) fn main_reachable_commits<'a>(
    main: &Path,
    tips: impl Iterator<Item = &'a str>,
    deadline: Option<std::time::Instant>,
) -> Vec<String> {
    main_reachable_commits_via(main, tips, deadline, "--all")
}

// Only remote refs grant teardown coverage; ordinary salvage accounting
// continues to use all host refs through the wrapper above.
// trace:BUG-2450 | ai:codex
fn main_reachable_commits_via<'a>(
    main: &Path,
    tips: impl Iterator<Item = &'a str>,
    deadline: Option<std::time::Instant>,
    refs: &str,
) -> Vec<String> {
    let mut tips: Vec<&str> = tips.filter(|oid| is_full_oid(oid)).collect();
    tips.sort_unstable();
    tips.dedup();
    if tips.is_empty() || tips.len() > MAIN_TIP_CAP {
        return Vec::new();
    }
    let input: String = tips.iter().map(|oid| format!("{oid}\n")).collect();
    let mut check = git(main);
    check.args(["cat-file", "--batch-check=%(objectname) %(objecttype)"]);
    let Some(present) = run_bounded(check, &input, deadline) else {
        return Vec::new();
    };
    let commits: Vec<&str> = present
        .lines()
        .filter_map(|line| line.strip_suffix(" commit"))
        .filter(|oid| tips.binary_search(oid).is_ok())
        .collect();
    if commits.is_empty() {
        return Vec::new();
    }
    // `rev-list <tips> --not --all` lists what main's refs do not reach; a
    // tip missing from it is reachable.
    let mut unreached = git(main);
    unreached.args(["rev-list", "--stdin", "--not", refs]);
    let input: String = commits.iter().map(|oid| format!("{oid}\n")).collect();
    let Some(out) = run_bounded(unreached, &input, deadline) else {
        return Vec::new();
    };
    let unreached: std::collections::HashSet<&str> = out.lines().map(str::trim).collect();
    commits
        .into_iter()
        .filter(|oid| !unreached.contains(oid))
        .map(str::to_string)
        .collect()
}

/// Run host git `cmd` with `input` on stdin (from a temp file, so a large
/// reply cannot deadlock it), killed once `deadline` or [`PROBE_TIMEOUT`]
/// passes; its stdout on success.
// trace:BUG-2224 | ai:claude
fn run_bounded(
    mut cmd: std::process::Command,
    input: &str,
    deadline: Option<std::time::Instant>,
) -> Option<String> {
    use std::io::{Seek, Write};
    let timeout = deadline
        .map(|end| {
            end.saturating_duration_since(std::time::Instant::now())
                .min(PROBE_TIMEOUT)
        })
        .unwrap_or(PROBE_TIMEOUT);
    if timeout.is_zero() {
        return None;
    }
    let mut file = tempfile::tempfile().ok()?;
    file.write_all(input.as_bytes()).ok()?;
    file.rewind().ok()?;
    cmd.stdin(file);
    let out = crate::command_output_with_timeout(cmd, timeout)?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `git status --porcelain=v2 --branch`. Only a full 40/64-hex
/// `branch.oid` counts as a HEAD.
// trace:TASK-2053 | ai:claude
pub(crate) fn parse_status_v2(out: &str) -> ProbeFacts {
    let mut facts = ProbeFacts::default();
    for line in out.lines() {
        if let Some(oid) = line.strip_prefix("# branch.oid ") {
            let oid = oid.trim();
            let hex =
                (oid.len() == 40 || oid.len() == 64) && oid.chars().all(|c| c.is_ascii_hexdigit());
            facts.head = hex.then(|| oid.to_string());
        } else if let Some(head) = line.strip_prefix("# branch.head ") {
            let head = head.trim();
            facts.branch = (head != "(detached)" && !head.is_empty()).then(|| head.to_string());
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            facts.dirty.push(line.to_string());
        }
    }
    facts
}

/// Whether `clone/.gitmodules` is a 0-byte regular file (not a symlink): the
/// placeholder the Claude Code sandbox creates for a protected path. Only
/// file metadata is read; nothing in the clone is opened or run.
// trace:BUG-B6B1278890774EF993D2EBC327862470-047 | ai:claude
pub(crate) fn is_empty_gitmodules_placeholder(clone: &Path) -> bool {
    std::fs::symlink_metadata(clone.join(".gitmodules"))
        .is_ok_and(|meta| meta.file_type().is_file() && meta.len() == 0)
}

/// Inside the probe sandbox: set `s` to every stash entry's commit (the
/// reflog of `refs/stash`, not only its tip). Shared by the scripts below.
// trace:TASK-2053 | ai:claude
macro_rules! stash_entries_sh {
    () => {
        "s=''\nif git rev-parse -q --verify refs/stash >/dev/null; then\n  \
         s=$(git log -g --format=%H refs/stash) || exit 1\nfi\n"
    };
}

/// Inside the probe sandbox: what shows the clone has, or had, a submodule,
/// on stdout (nothing when there is none): a `.gitmodules` in the worktree
/// that defines a submodule (or is tracked, not a regular file, or cannot be
/// parsed), one at HEAD, a gitlink in the index or in HEAD's tree, or
/// anything under `.git/modules` (a deinit'd or removed submodule keeps its
/// commits there). An empty, untracked `.gitmodules` is not evidence: the
/// Claude Code sandbox creates one as a placeholder for a protected path
/// (see [`is_empty_gitmodules_placeholder`]). Submodule work is outside what
/// the salvage captures, so the clone is kept whatever the submodules hold;
/// nothing here enters a submodule.
// trace:TASK-2053 trace:BUG-B6B1278890774EF993D2EBC327862470-047 | ai:claude
const SUBMODULE_EVIDENCE_SCRIPT: &str = r##"set -u
if [ -L .gitmodules ] || { [ -e .gitmodules ] && [ ! -f .gitmodules ]; }; then echo '.gitmodules in the worktree is not a regular file'; exit 0; fi
g=$(git ls-files -s -- .gitmodules) || exit 1
if [ -n "$g" ]; then echo '.gitmodules in the index'; exit 0; fi
if [ -s .gitmodules ]; then
  if [ ! -r .gitmodules ]; then echo '.gitmodules in the worktree cannot be read'; exit 0; fi
  rc=0
  git config --file .gitmodules --name-only --get-regexp '^submodule\.' >/dev/null 2>&1 || rc=$?
  if [ "$rc" -eq 0 ]; then echo '.gitmodules in the worktree defines a submodule'; exit 0; fi
  if [ "$rc" -ne 1 ]; then echo '.gitmodules in the worktree cannot be read'; exit 0; fi
fi
if git rev-parse -q --verify HEAD^{commit} >/dev/null; then
  if git cat-file -e HEAD:.gitmodules 2>/dev/null; then echo '.gitmodules at HEAD'; exit 0; fi
  t=$(git ls-tree -r --full-tree HEAD) || exit 1
  if printf '%s\n' "$t" | grep -q '^160000 '; then echo 'a gitlink in HEAD'; exit 0; fi
fi
i=$(git ls-files -s) || exit 1
if printf '%s\n' "$i" | grep -q '^160000 '; then echo 'a gitlink in the index'; exit 0; fi
d="$GIT_DIR/modules"
if [ -L "$d" ] || { [ -e "$d" ] && [ ! -d "$d" ]; }; then echo '.git/modules is not a directory'; exit 0; fi
if [ -d "$d" ]; then
  l=$(ls -A "$d") || exit 1
  if [ -n "$l" ]; then echo 'repositories in .git/modules'; exit 0; fi
fi
exit 0
"##;

/// Inside the probe sandbox: the commit every ref of the clone points at
/// (tags peeled), one per line, for [`main_reachable_commits`]; then a
/// [`TIPS_SEPARATOR`] line and the newest 5000 commits the clone's refs
/// reach that no `origin/*` ref does (older ones are not excused, so count). BUG-051: a seat commit on
/// top of one that main now reaches (origin's branch gained a merge after
/// the seat pushed) leaves no tip main reaches, so the candidates are checked
/// too: each one main reaches is excused, and with it its ancestors.
// trace:BUG-2224 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
// Clean, published branch heads may retire lane checkpoint tags even when
// pre-squash checkpoints are unreachable from origin. Salvage still counts
// every ref so those checkpoints are bundled before teardown.
// trace:BUG-2491 | ai:codex
const REF_TIPS_SCRIPT: &str = concat!(
    "set -u\n",
    "git for-each-ref --format='%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)' || exit 1\n",
    "echo --\n",
    "exec git rev-list --max-count=5000 --all --not --remotes=origin\n"
);

/// The line [`REF_TIPS_SCRIPT`] prints between the ref tips and the
/// candidate commits.
const TIPS_SEPARATOR: &str = "--";

// trace:TASK-1222 | ai:codex
const GC_STASH_ENTRIES: &str = stash_entries_sh!();

/// Inside the probe sandbox: how many commits the clone holds that origin
/// does not: reachable from ANY ref (every branch, tag, HEAD) or stash entry,
/// and not from `origin/*`, the pinned base (first positional argument, or empty) or an excused
/// head (remaining full hex ids: a shipped head, or a tip main reaches; one the
/// clone lacks is skipped). Submodules are not counted: [`probe`] keeps any
/// clone that has one. The total is the first line; then, when it is not 0,
/// `<count> <ref>` for each ref (a detached HEAD, every ref outside
/// `origin/*`, the stash) holding any.
// trace:TASK-2053 trace:BUG-2224 | ai:claude
const UNPUSHED_COUNT_SCRIPT: &str = concat!(
    "set -u\n",
    stash_entries_sh!(),
    // One batch checks all excused heads. Only commit objects become negative
    // revisions; absent objects and tags never excuse work. Keep the large set
    // on stdin to rev-list rather than expanding it into each process's argv.
    // trace:BUG-2384 | ai:codex
    // Only the clean-tree probe requests this exemption; salvage keeps all refs.
    // trace:BUG-2491 | ai:codex
    "ignore_slice=0\nslice_only=0\ncase $1 in ignore-slice) ignore_slice=1; shift;; keep-slice) shift;; slice-only) slice_only=1; shift;; esac\n",
    "base=$1\nshift\nshipped=''\n",
    "if [ \"$#\" -gt 0 ]; then\n",
    "  objects=$(printf '%s\\n' \"$@\" | git cat-file --batch-check='%(objectname) %(objecttype)') || exit 1\n",
    "  shipped=$(printf '%s\\n' \"$objects\" | awk '$2 == \"commit\" {print \"^\" $1}') || exit 1\n",
    "fi\n",
    "refs=$(git for-each-ref --format='%(refname)') || exit 1\n",
    "head=\"\"\nif git rev-parse -q --verify HEAD^{commit} >/dev/null; then head=HEAD; fi\n",
    "if [ \"$ignore_slice\" -eq 1 ] && [ -n \"$head\" ]; then\n",
    "  n=$(printf '%s\\n' \"$shipped\" | git rev-list --stdin --count HEAD --not --remotes=origin ${base:+\"$base\"}) || exit 1\n",
    "  if [ \"$n\" -eq 0 ]; then refs=$(printf '%s\\n' \"$refs\" | awk 'index($0, \"refs/tags/slice-\") != 1') || exit 1; fi\n",
    "fi\n",
    // Teardown only needs salvage for checkpoint history not already published.
    // trace:BUG-2491 | ai:codex
    "if [ \"$slice_only\" -eq 1 ]; then\n",
    "  refs=$(printf '%s\\n' \"$refs\" | awk 'index($0, \"refs/tags/slice-\") == 1') || exit 1\n",
    "  head=''; s=''\nfi\n",
    // Blank lines terminate rev-list stdin, including an unborn HEAD.
    // trace:BUG-2491 | ai:codex
    "revisions=$(printf '%s\\n' \"$head\" \"$refs\" \"$shipped\" | sed '/^$/d') || exit 1\n",
    "total=$(printf '%s\\n' \"$revisions\" | git rev-list --stdin --count $s --not --remotes=origin ${base:+\"$base\"}) || exit 1\n",
    "printf '%s\\n' \"$total\"\n",
    "if [ \"$total\" -eq 0 ]; then exit 0; fi\n",
    "if ! git symbolic-ref -q HEAD >/dev/null && git rev-parse -q --verify HEAD >/dev/null; then\n",
    "  refs=\"HEAD\n$refs\"\n",
    "fi\n",
    "printf '%s\\n' \"$refs\" | while IFS= read -r r; do\n",
    "  case $r in ''|refs/remotes/origin/*|refs/stash) continue;; esac\n",
    "  n=$(printf '%s\\n' \"$shipped\" | git rev-list --stdin --count \"$r\" --not --remotes=origin ${base:+\"$base\"}) || exit 1\n",
    "  if [ \"$n\" -gt 0 ]; then printf '%s %s\\n' \"$n\" \"$r\"; fi\n",
    "done || exit 1\n",
    "if [ -n \"$s\" ]; then\n",
    "  n=$(printf '%s\\n' \"$shipped\" | git rev-list --stdin --count $s --not --remotes=origin ${base:+\"$base\"}) || exit 1\n",
    "  if [ \"$n\" -gt 0 ]; then printf '%s refs/stash\\n' \"$n\"; fi\n",
    "fi\n"
);

/// A full 40- or 64-hex object id.
fn is_full_oid(oid: &str) -> bool {
    (oid.len() == 40 || oid.len() == 64) && oid.chars().all(|c| c.is_ascii_hexdigit())
}

/// The head commits of `branch`'s merged PRs on GitHub (`gh pr list --head
/// <branch> --state merged`), for [`probe_with_shipped`]. Empty when gh is
/// missing, fails, or the forge is not GitHub: the count then stays
/// conservative.
// trace:TASK-2110 | ai:claude
pub(crate) fn merged_pr_heads(main: &Path, branch: &str) -> Vec<String> {
    merged_pr_heads_with_timeout(main, branch, std::time::Duration::from_secs(20))
}

// trace:TASK-2053 | ai:codex
pub(crate) fn merged_pr_heads_with_timeout(
    main: &Path,
    branch: &str,
    timeout: std::time::Duration,
) -> Vec<String> {
    if branch.is_empty() {
        return Vec::new();
    }
    let mut cmd = std::process::Command::new("gh");
    cmd.current_dir(main)
        .args(["pr", "list", "--state", "merged", "--head", branch])
        .args(["--json", "headRefOid", "--jq", ".[].headRefOid"])
        .stdin(std::process::Stdio::null());
    let Some(out) = crate::command_output_with_timeout(cmd, timeout) else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|oid| is_full_oid(oid))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
// trace:STORY-2493 | ai:codex
mod mount_race_tests {
    use super::*;
    use crate::BoundedCommandOutput::{Completed, TimedOut};
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;

    const RACE: &str = "bwrap: Can't bind mount /oldroot/ on /newroot/: Unable to apply mount flags: remount \"/newroot/run/credentials/systemd-tmpfiles-clean.service\": No such file or directory\n";

    fn output(code: i32, stdout: &[u8], stderr: &str) -> crate::BoundedCommandOutput {
        Completed(std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn readonly_mount_race_retries_setup_with_shared_budget() {
        let mut calls = 0;
        let mut previous = Duration::from_secs(1);
        let result = retry_readonly_mount_race(true, previous, |remaining| {
            assert!(remaining <= previous);
            previous = remaining;
            calls += 1;
            if calls == 1 {
                output(1, b"", RACE)
            } else {
                output(0, b"probe facts", "")
            }
        });
        assert_eq!(calls, 2);
        assert!(
            matches!(result, Completed(out) if out.status.success() && out.stdout == b"probe facts")
        );
    }

    #[test]
    fn readonly_mount_race_is_bounded_and_retains_failure() {
        let mut calls = 0;
        let result = retry_readonly_mount_race(true, Duration::from_secs(1), |_| {
            calls += 1;
            output(1, b"", RACE)
        });
        assert_eq!(calls, 3);
        assert!(matches!(result, Completed(out) if out.stderr == RACE.as_bytes()));
        let result = retry_readonly_mount_race(true, Duration::ZERO, |_| {
            panic!("expired budget must not spawn")
        });
        assert!(matches!(result, TimedOut));
    }

    #[test]
    fn mount_race_never_replays_commits_or_other_failures() {
        for (readonly, code, stdout, stderr) in [
            (false, 1, b"".as_slice(), RACE),
            (true, 1, b"command ran".as_slice(), RACE),
            (true, 2, b"".as_slice(), RACE),
            (true, 1, b"".as_slice(), "bwrap: permission denied"),
            (true, 1, b"".as_slice(), "git: command failed"),
        ] {
            let mut calls = 0;
            let _ = retry_readonly_mount_race(readonly, Duration::from_secs(1), |_| {
                calls += 1;
                output(code, stdout, stderr)
            });
            assert_eq!(calls, 1, "{readonly}: {stderr}");
        }
    }
}

/// Inside the probe sandbox: a git bundle of every commit
/// [`UNPUSHED_COUNT_SCRIPT`] counts, with all of the clone's refs, on stdout.
// trace:TASK-2053 | ai:claude
const SALVAGE_BUNDLE_SCRIPT: &str = concat!(
    "set -u\n",
    stash_entries_sh!(),
    "exec git bundle create - --all $s --not --remotes=origin ${1:+\"$1\"}\n"
);

/// Inside the probe sandbox: the HEAD commits origin does not have (as
/// `format-patch`, for reading; the bundle beside the patch holds every
/// ref), the stash entries' commits, the uncommitted changes and every
/// untracked file's content (as new-file diffs), all on stdout. Untracked
/// names travel NUL-separated, so no name is quoted or dropped, and a file
/// that cannot be diffed fails the whole salvage. So does an untracked
/// nested git repository (listed as `dir/`) or anything else that is not a
/// regular file or symlink: `diff --no-index` exits 1 for "Could not access"
/// just as for "files differ", so such an entry would otherwise vanish with
/// the clone. So does an untracked symlink to a directory (diff follows it)
/// and any file whose diff comes back empty. A submodule with changes or
/// with commits its remote lacks fails the salvage up front: the patch and
/// the bundle hold the superproject only, so the clone must be kept. (The
/// probe already keeps a clone with submodules; this catches one that
/// appeared since.)
/// `$1` is the pinned base, or empty. `$2` is `complete` for a salvage a
/// lease is released on: the patch holds one HEAD-to-worktree diff, so a
/// path whose staged content differs from both HEAD and the working tree
/// (staged and changed again, or staged then restored) fails it up front.
// trace:TASK-2053 trace:TASK-3796 | ai:claude
const SALVAGE_SCRIPT: &str = concat!(
    "set -u\n",
    stash_entries_sh!(),
    r##"st=$(git status --porcelain=v2 --ignore-submodules=none) || exit 1
if printf '%s\n' "$st" | grep -q '^[12] .. S'; then
  printf 'aida: a submodule has changes or commits, which this salvage does not capture\n' >&2
  exit 1
fi
git submodule --quiet foreach --recursive 'n=$(git rev-list --count --all --not --remotes) || exit 1
if [ "$n" -gt 0 ]; then
  printf "aida: submodule %s has %s commit(s) its remote lacks, which this salvage does not capture\n" "$displaypath" "$n" >&2
  exit 1
fi' || exit 1
if [ "${2:-}" = complete ]; then
  git diff --cached --name-only -z --no-renames HEAD > /tmp/aida-staged || exit 1
  xargs -0 -r -n 1 sh -c 'git --literal-pathspecs diff --quiet --no-ext-diff --no-textconv -- "$1"
rc=$?
if [ $rc -eq 1 ]; then
  printf "aida: %s has staged content that differs from both HEAD and the working tree, which one salvage patch cannot hold\n" "$1" >&2; exit 255
fi
if [ $rc -ne 0 ]; then
  printf "aida: could not compare staged %s with the working tree\n" "$1" >&2; exit 255
fi' aida-staged < /tmp/aida-staged || exit 1
fi
printf '# AIDA salvage patch (contained seat clone)\n# clone: %s\n' "$PWD"
printf '# Every commit origin lacks, on every ref and stash entry, is in the .bundle beside this patch.\n'
for c in $s; do printf '# stash entry: %s\n' "$c"; done
printf '\n# Commits on HEAD not on origin:\n'
git format-patch --stdout --binary HEAD --not --remotes=origin ${1:+"$1"} || exit 1
printf '\n# Uncommitted changes:\n'
git diff --binary --no-ext-diff --no-textconv HEAD || exit 1
printf '\n# Untracked files:\n'
git ls-files -z --others --exclude-standard > /tmp/aida-untracked || exit 1
xargs -0 -r -n 1 sh -c 'case "$1" in */)
  printf "aida: %s is a nested git repository, which a patch cannot hold\n" "$1" >&2; exit 255;;
esac
if [ -L "$1" ] && [ -d "$1" ]; then
  printf "aida: %s is a symlink to a directory, which a patch cannot hold\n" "$1" >&2; exit 255
fi
if [ ! -L "$1" ] && [ ! -f "$1" ]; then
  printf "aida: %s is not a regular file or symlink\n" "$1" >&2; exit 255
fi
printf "#   %s\n" "$1"
git diff --no-index --binary --no-ext-diff --no-textconv -- /dev/null "$1" > /tmp/aida-file-diff
rc=$?
if [ $rc -gt 1 ] || [ ! -s /tmp/aida-file-diff ]; then
  printf "aida: could not record untracked %s\n" "$1" >&2; exit 255
fi
cat /tmp/aida-file-diff' aida-untracked < /tmp/aida-untracked || exit 1
exit 0
"##
);

/// Write what [`SALVAGE_SCRIPT`] captures to
/// `<main>/.aida/salvage/<clone dir>/<UTC stamp>.patch`. The host never fetches
/// from the clone: that would run upload-pack in a repository the seat
/// controls.
// trace:TASK-2053 | ai:claude
pub(crate) fn salvage(main: &Path, clone: &Path) -> Result<PathBuf> {
    salvage_capped(main, clone, SALVAGE_OUTPUT_CAP, false)
}

/// [`salvage`] for a caller that gives up the lane's lease once the patch
/// exists: a staged version that neither HEAD nor the working tree holds
/// fails it, checked inside the sandbox, so no patch claims to be the whole
/// work.
// trace:TASK-3796 | ai:claude
pub(crate) fn salvage_complete(main: &Path, clone: &Path) -> Result<PathBuf> {
    salvage_capped(main, clone, SALVAGE_OUTPUT_CAP, true)
}

/// [`salvage`] keeping at most `cap` bytes: a longer patch is written cut
/// off, with a closing `# TRUNCATED` line, and returned as an error so the
/// caller keeps the clone.
// trace:TASK-2053 | ai:claude
fn salvage_capped(main: &Path, clone: &Path, cap: u64, complete: bool) -> Result<PathBuf> {
    let base = pinned_base(main, clone).unwrap_or_default();
    let mode = if complete { "complete" } else { "" };
    let ran = run_sandboxed(
        clone,
        main,
        "sh",
        &[
            "-c".as_ref(),
            SALVAGE_SCRIPT.as_ref(),
            "aida-seat-salvage".as_ref(),
            base.as_ref(),
            mode.as_ref(),
        ],
        cap,
    );
    let (mut body, truncated) = match ran {
        Ok(body) => (body, false),
        Err(SandboxFailure::Truncated(kept)) => (kept, true),
        Err(failure) => anyhow::bail!("could not salvage {}: {failure}", clone.display()),
    };
    let name = clone
        .file_name()
        .and_then(|n| n.to_str())
        .context("a seat clone path has a directory name")?;
    let dir = main.join(".aida").join("salvage").join(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let stamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H%M%S%.3fZ")
        .to_string();
    let path = dir.join(format!("{stamp}.patch"));
    // Every commit origin lacks, on any ref or stash entry, goes into a
    // bundle beside the patch (`git fetch <bundle>` restores the refs). A
    // bundle that cannot be made whole keeps the clone.
    // trace:TASK-2053 | ai:claude
    let script_args = |script: &'static str| -> [std::ffi::OsString; 4] {
        [
            "-c".into(),
            script.into(),
            "aida-seat-salvage".into(),
            base.clone().into(),
        ]
    };
    let count_args = script_args(UNPUSHED_COUNT_SCRIPT);
    let count_args: Vec<&std::ffi::OsStr> = count_args.iter().map(|a| a.as_os_str()).collect();
    let unpushed: u64 = probe_run(clone, main, &count_args, "sh")
        .ok()
        .and_then(|out| parse_unpushed_count(&out))
        .map(|(total, _)| total)
        .with_context(|| {
            format!(
                "could not count the unpushed commits of {}",
                clone.display()
            )
        })?;
    if unpushed > 0 {
        let bundle_args = script_args(SALVAGE_BUNDLE_SCRIPT);
        let bundle_args: Vec<&std::ffi::OsStr> =
            bundle_args.iter().map(|a| a.as_os_str()).collect();
        let bundle = run_sandboxed(clone, main, "sh", &bundle_args, cap).map_err(|failure| {
            anyhow::anyhow!(
                "could not bundle the {unpushed} unpushed commit(s) of {}: {failure}",
                clone.display()
            )
        })?;
        let bundle_path = dir.join(format!("{stamp}.bundle"));
        std::fs::write(&bundle_path, bundle)
            .with_context(|| format!("write {}", bundle_path.display()))?;
    }
    if truncated {
        body.extend_from_slice(
            format!("\n# TRUNCATED: the salvage passed {cap} bytes; the clone was kept\n")
                .as_bytes(),
        );
    }
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    if truncated {
        anyhow::bail!(
            "the salvage of {} passed {cap} bytes and was cut off (partial patch at {}), so the \
             clone is kept",
            clone.display(),
            path.display()
        );
    }
    Ok(path)
}

/// Resolve the actual PR base before any fetch or merge. Unknown forge
/// evidence refuses preparation rather than guessing the default branch.
// trace:BUG-2603 | ai:codex
pub(crate) fn rework_base_branch(main: &Path, pr: u64, branch: &str) -> Result<String> {
    Ok(observe_rework_change(main, pr, branch)?.base_ref)
}

fn observe_rework_change(
    main: &Path,
    pr: u64,
    branch: &str,
) -> Result<crate::forge::ChangeMetadata> {
    verify_rework_metadata(
        branch,
        crate::forge::forge_for(main)
            .observe_change_metadata(pr, &mut crate::network_retry::StderrSink),
    )
}

// trace:BUG-2603 BUG-2688 | ai:codex
fn verify_rework_metadata(
    branch: &str,
    observation: crate::forge_observed::Observed<
        std::result::Result<crate::forge::ChangeMetadata, String>,
    >,
) -> Result<crate::forge::ChangeMetadata> {
    use crate::forge_observed::Observed;
    let metadata = match observation {
        Observed::Known(Ok(metadata)) => metadata,
        Observed::Known(Err(why)) | Observed::Pending(why) | Observed::Transient(why) => {
            anyhow::bail!("could not determine the rework PR base: {why}")
        }
    };
    anyhow::ensure!(
        metadata.state == crate::forge::ChangeState::Open
            && !metadata.is_cross_repository
            && metadata.head_ref == branch,
        "open rework PR no longer names the local origin branch {branch}"
    );
    validate_rework_base(&metadata.base_ref)?;
    Ok(metadata)
}

// Local origins have no forge service. Supply deterministic metadata while
// retaining the production validation and preparation path.
// trace:BUG-2603 | ai:codex
#[cfg(test)]
pub(crate) fn prepare_rework_merge_fixture(
    main: &Path,
    tree: &Path,
    branch: &str,
    resume: bool,
    pr: u64,
    lane_id: &str,
) -> Result<Vec<String>> {
    prepare_rework_merge_fixture_admitted(main, tree, branch, resume, pr, lane_id, &|| Ok(()))
}

// trace:BUG-2688 | ai:claude
#[cfg(test)]
pub(crate) fn prepare_rework_merge_fixture_admitted(
    main: &Path,
    tree: &Path,
    branch: &str,
    resume: bool,
    pr: u64,
    lane_id: &str,
    admit_conflict: &dyn Fn() -> Result<()>,
) -> Result<Vec<String>> {
    prepare_rework_merge_fixture_for_seat(
        main,
        tree,
        branch,
        if resume {
            ReworkSeat::Adopted
        } else {
            ReworkSeat::Replacement
        },
        pr,
        lane_id,
        admit_conflict,
    )
}

#[cfg(test)]
pub(crate) fn prepare_rework_merge_fixture_for_seat(
    main: &Path,
    tree: &Path,
    branch: &str,
    seat: ReworkSeat,
    pr: u64,
    lane_id: &str,
    admit_conflict: &dyn Fn() -> Result<()>,
) -> Result<Vec<String>> {
    prepare_rework_merge_with_base(
        main,
        tree,
        branch,
        seat,
        pr,
        lane_id,
        "BUG-2688",
        admit_conflict,
        || {
            let metadata = verify_rework_metadata(branch, fixture_rework_metadata(branch, "main"))?;
            fetch_rework_heads(main, &metadata.base_ref, branch)
        },
    )
}

// trace:BUG-2603 | ai:codex
#[cfg(test)]
fn fixture_rework_metadata(
    branch: &str,
    base: &str,
) -> crate::forge_observed::Observed<std::result::Result<crate::forge::ChangeMetadata, String>> {
    use crate::forge::{ChangeMetadata, ChangeState};
    crate::forge_observed::Observed::Known(Ok(ChangeMetadata {
        state: ChangeState::Open,
        title: String::new(),
        author: None,
        merged_at: None,
        base_ref: base.into(),
        head_ref: branch.into(),
        head_sha: String::new(),
        is_draft: false,
        is_cross_repository: false,
        head_repo: None,
    }))
}

// trace:BUG-2603 | ai:codex
fn validate_rework_base(base: &str) -> Result<()> {
    crate::git_arg_guard::reject_option_like("rework base", base)?;
    let mut cmd = aida_core::git_ops::host_git_command(Path::new("."));
    cmd.args(["check-ref-format", &format!("refs/heads/{base}")]);
    anyhow::ensure!(
        !base.is_empty() && cmd.status()?.success(),
        "invalid rework PR base"
    );
    Ok(())
}

// A stage with its own recovery keeps that command through pickup context.
// trace:BUG-2688 | ai:codex
#[derive(Debug)]
struct ReworkRecovery(String);

impl std::fmt::Display for ReworkRecovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// A failed preparation observed user files or retained merge state. Keep
// the pickup receipt and lease so its owner can preserve those files.
#[derive(Debug)]
pub(crate) struct PreserveReworkSeat(String);

impl std::fmt::Display for PreserveReworkSeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Fetch only the actual base and PR head through the trusted host origin.
// trace:BUG-2603 | ai:codex
pub(crate) fn fetch_rework_heads(
    main: &Path,
    base: &str,
    branch: &str,
) -> Result<(String, String)> {
    validate_rework_base(base)?;
    validate_rework_base(branch)?;
    let base_ref = format!("+refs/heads/{base}:refs/remotes/origin/{base}");
    let branch_ref = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    let recovery = format!(
        "`git -C {} fetch origin {} {}`",
        crate::shell_quote(&main.to_string_lossy()),
        crate::shell_quote(&base_ref),
        crate::shell_quote(&branch_ref),
    );
    let mut fetch = aida_core::git_ops::host_remote_git_command(main, "origin");
    fetch
        .args([
            "fetch",
            "--no-write-fetch-head",
            "--atomic",
            "origin",
            &base_ref,
            &branch_ref,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(fetch, std::time::Duration::from_secs(20)) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => {}
        crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(ReworkRecovery(format!(
            "could not fetch rework heads origin/{base} and origin/{branch}: {}; retry {recovery}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
        _ => anyhow::bail!(ReworkRecovery(format!(
            "could not complete fetch of origin/{base} and origin/{branch}; retry {recovery}"
        ))),
    }
    let resolve = |name: &str| -> Result<String> {
        let mut cmd = git(main);
        cmd.args([
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{name}^{{commit}}"),
        ]);
        run(cmd, "pin remote commit").with_context(|| {
            ReworkRecovery(format!(
                "missing commit evidence for origin/{name}; refresh it with {recovery}"
            ))
        })
    };
    Ok((resolve(base)?, resolve(branch)?))
}

// Keep the exact fetched commits for both archive and preparation. Later
// observations can veto pickup, but cannot replace either commit.
// trace:BUG-2688 | ai:codex
pub(crate) struct ReworkHeads {
    base_ref: String,
    base: String,
    pub(crate) head: String,
}

impl ReworkHeads {
    pub(crate) fn observe(main: &Path, pr: u64, branch: &str) -> Result<Self> {
        let metadata = observe_rework_change(main, pr, branch)?;
        Self::fetch(main, metadata, branch)
    }

    fn fetch(main: &Path, metadata: crate::forge::ChangeMetadata, branch: &str) -> Result<Self> {
        let (base, head) = fetch_rework_heads(main, &metadata.base_ref, branch)?;
        anyhow::ensure!(
            head == metadata.head_sha,
            "rework origin/{branch} at {head} differs from the observed PR head {}; inspect origin with `git -C {} ls-remote --refs origin {}` and retry pickup after forge and origin agree",
            metadata.head_sha,
            crate::shell_quote(&main.to_string_lossy()),
            crate::shell_quote(&format!("refs/heads/{branch}")),
        );
        Ok(Self {
            base_ref: metadata.base_ref,
            base,
            head,
        })
    }

    fn verify_unchanged(&self, main: &Path, pr: u64, branch: &str) -> Result<()> {
        self.verify_origin(main, branch)?;
        let metadata = observe_rework_change(main, pr, branch).context(
            "could not recheck the pinned rework PR; retry pickup once the forge is reachable",
        )?;
        self.require_same_change(&metadata)
    }

    fn require_same_change(&self, metadata: &crate::forge::ChangeMetadata) -> Result<()> {
        anyhow::ensure!(
            metadata.head_sha == self.head && metadata.base_ref == self.base_ref,
            "rework PR head or base branch changed since it was pinned; retry pickup from a new observation"
        );
        Ok(())
    }

    pub(crate) fn verify_origin(&self, main: &Path, branch: &str) -> Result<()> {
        let base_ref = format!("refs/heads/{}", self.base_ref);
        let head_ref = format!("refs/heads/{branch}");
        let mut cmd = aida_core::git_ops::host_remote_git_command(main, "origin");
        cmd.args(["ls-remote", "--refs", "origin", &base_ref, &head_ref])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null());
        let verify = || -> Result<()> {
            let crate::BoundedCommandOutput::Completed(out) =
                crate::command_output_with_timeout_detail(cmd, std::time::Duration::from_secs(20))
            else {
                anyhow::bail!("origin ref verification timed out");
            };
            anyhow::ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            let refs = std::str::from_utf8(&out.stdout)?;
            for (name, expected) in [(&base_ref, &self.base), (&head_ref, &self.head)] {
                let mut observed = refs
                    .lines()
                    .filter_map(|line| line.split_once('\t'))
                    .filter_map(|(oid, reference)| (reference == name).then_some(oid));
                anyhow::ensure!(
                    observed.next() == Some(expected.as_str()) && observed.next().is_none(),
                    "origin {name} no longer matches pinned {expected}"
                );
            }
            Ok(())
        };
        verify().with_context(|| {
            format!(
                "could not verify the pinned rework refs; inspect origin with `git -C {} ls-remote --refs origin {} {}` and retry pickup after origin and the forge agree",
                crate::shell_quote(&main.to_string_lossy()),
                crate::shell_quote(&base_ref),
                crate::shell_quote(&head_ref),
            )
        })
    }
}

/// Host preparation of a rework. Clone Git runs only in AIDA's writable
/// sandbox with the host identity; ordinary linked worktrees use host Git.
/// Conflicts retain MERGE_HEAD and journaled paths; the lane edits files only.
/// `admit_conflict` is the launch's route and eligibility check for a
/// conflicted repair. It runs once a conflict is observed and before the
/// repair journal is touched, so a refused route reserves nothing and its
/// error is returned as is.
// trace:STORY-2475 | ai:codex
// trace:BUG-2688 | ai:claude
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReworkSeat {
    Adopted,
    Replacement,
    Fresh,
}

pub(crate) fn prepare_rework_merge(
    main: &Path,
    tree: &Path,
    branch: &str,
    seat: ReworkSeat,
    pr: u64,
    lane_id: &str,
    scope: &str,
    pinned_heads: Option<&ReworkHeads>,
    admit_conflict: &dyn Fn() -> Result<()>,
) -> Result<Vec<String>> {
    prepare_rework_merge_with_base(
        main,
        tree,
        branch,
        seat,
        pr,
        lane_id,
        scope,
        admit_conflict,
        || {
            let observed;
            let heads = match pinned_heads {
                Some(heads) => {
                    heads.verify_unchanged(main, pr, branch)?;
                    heads
                }
                None => {
                    observed = ReworkHeads::observe(main, pr, branch)?;
                    &observed
                }
            };
            Ok((heads.base.clone(), heads.head.clone()))
        },
    )
}

// Inject the forge observation at the boundary; all Git and repair preparation
// below this point is shared by production and local-origin fixtures.
// trace:BUG-2603 | ai:codex
fn prepare_rework_merge_with_base(
    main: &Path,
    tree: &Path,
    branch: &str,
    seat: ReworkSeat,
    pr: u64,
    lane_id: &str,
    scope: &str,
    admit_conflict: &dyn Fn() -> Result<()>,
    fetch_heads: impl FnOnce() -> Result<(String, String)>,
) -> Result<Vec<String>> {
    use std::ffi::{OsStr, OsString};
    let execution_environment = aida_core::exec_context::Environment::capture();
    let clone = host_recorded_clone(main, tree);
    let pinned = dir_identity(tree)?;
    let host_value = |key: &str| -> Result<String> {
        let mut cmd = git(main);
        cmd.args(["config", "--get", key]);
        let recovery = || {
            format!(
                "missing host {key}; set it with `git -C {} config {key} <value>` and retry pickup",
                crate::shell_quote(&main.to_string_lossy())
            )
        };
        let value = run(cmd, "read host merge identity").with_context(recovery)?;
        anyhow::ensure!(!value.is_empty(), "{}", recovery());
        Ok(value)
    };
    let name = host_value("user.name")?;
    let email = host_value("user.email")?;
    let identity: [(&'static str, OsString); 4] = [
        ("GIT_AUTHOR_NAME", name.clone().into()),
        ("GIT_AUTHOR_EMAIL", email.clone().into()),
        ("GIT_COMMITTER_NAME", name.into()),
        ("GIT_COMMITTER_EMAIL", email.into()),
    ];
    let decode = |args: &[&str], bytes: Vec<u8>| -> Result<String> {
        let text = String::from_utf8(bytes).context("rework Git returned non-UTF-8 evidence")?;
        Ok(if args.contains(&"-z") {
            text
        } else {
            text.trim().to_owned()
        })
    };
    let execute = |args: &[&str]| -> Result<String> {
        ensure_dir_identity(tree, pinned)?;
        if clone.is_some() {
            let os_args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            run_commit_sandboxed(tree, main, &os_args, &identity)
                .map_err(|why| anyhow::anyhow!("rework Git failed: {why}"))
                .and_then(|out| decode(args, out))
        } else {
            let mut cmd = aida_core::git_ops::host_git_command(tree);
            cmd.args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(args)
            .envs(identity.iter().map(|(key, value)| (*key, value)))
            .stdin(std::process::Stdio::null());
            match crate::command_output_with_timeout_detail(cmd, std::time::Duration::from_secs(30))
            {
                crate::BoundedCommandOutput::Completed(out)
                    if out.status.success()
                        || repair_merge_tree_conflict(args, out.status.code(), &out.stdout) =>
                {
                    decode(args, out.stdout)
                }
                crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
                    "rework Git failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ),
                _ => anyhow::bail!("rework Git timed out"),
            }
        }
    };
    let quote_tree = crate::shell_quote(&tree.to_string_lossy());
    let preserve_pending = if clone.is_some() {
        let subject = format!("[AI:codex] fix(rework): preserve pending work (Refs {scope})");
        format!(
            "aida ship --from-seat {quote_tree} --commit-pending {}",
            crate::shell_quote(&subject)
        )
    } else {
        format!("git -C {quote_tree} stash push --include-untracked --message aida-rework-recovery")
    };
    let repair_key = std::cell::RefCell::new(None);
    // A refused route is not a preparation failure: no reservation exists, no
    // refusal notice is owed, and the recovery text below does not apply.
    // trace:BUG-2688 | ai:claude
    let route_refused = std::cell::Cell::new(false);
    prepare_host_repair_admitted(
        main,
        tree,
        pr,
        lane_id,
        &execute,
        branch,
        seat,
        scope,
        &preserve_pending,
        clone.is_some(),
        &|| admit_conflict().inspect_err(|_| route_refused.set(true)),
        |tip| publish_rework_merge(main, tree, branch, tip),
        |base, restore_only| {
            let head = execute(&["rev-parse", "HEAD"])?;
            *repair_key.borrow_mut() = Some(crate::repair_journal::RepairKey {
                repository: main.into(),
                pr,
                original_head: head,
            });
            regenerate_repair_outputs(
                tree,
                main,
                base,
                &identity,
                &execution_environment,
                restore_only,
            )
        },
        fetch_heads,
    )
    .map_err(|error| {
        if route_refused.get() {
            return error;
        }
        if let Some(key) = repair_key.into_inner() {
            report_repair_refusal(main, &key, &error);
        }
        if error.downcast_ref::<ReworkRecovery>().is_some() {
            return error;
        }
        error.context(format!(
            "rework preparation refused for seat {} on {branch}",
            tree.display()
        ))
    })
}

// A session holds this lock shared from placement/adoption through the lease
// write. Fresh rework moves seats under the exclusive side.
// trace:BUG-2688 | ai:codex
pub(crate) fn lock_seat_placement(main: &Path, exclusive: bool) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let dir = main.join(".aida");
    std::fs::create_dir_all(&dir)?;
    anyhow::ensure!(
        std::fs::symlink_metadata(&dir)?.is_dir(),
        "unsafe seat placement directory {}",
        dir.display()
    );
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("seat-placement.lock"))?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "seat placement lock is not a regular file"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let result = if exclusive {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        };
        match result {
            Ok(()) => return Ok(file),
            Err(error)
                if aida_core::file_lock::is_lock_contended(&error)
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => return Err(error).context(
                "seat placement lock unavailable; retry after the other pickup finishes placement",
            ),
        }
    }
}

// Inspect the retained state without running Git in a private clone. For
// linked worktrees the Git path is resolved by the registered worktree.
// trace:BUG-2688 | ai:codex
fn rework_merge_head(execute: &dyn Fn(&[&str]) -> Result<String>) -> Result<Option<String>> {
    let path = execute(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "MERGE_HEAD",
    ])?;
    match aida_core::seat_clone_detect::read_regular_file_result(Path::new(&path)) {
        Ok(raw) => {
            let head = raw.trim();
            anyhow::ensure!(
                is_full_oid(head),
                "invalid MERGE_HEAD evidence at {path}; inspect with `od -c -- {}`",
                crate::shell_quote(&path)
            );
            Ok(Some(head.to_owned()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot inspect MERGE_HEAD at {path}")),
    }
}

fn linked_rework_command(tree: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = aida_core::git_ops::host_git_command(tree);
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
    ])
    .args(args);
    cmd
}

fn linked_rework_git(tree: &Path, args: &[&str]) -> Result<String> {
    run(
        linked_rework_command(tree, args),
        "inspect or preserve registered rework worktree",
    )
}

pub(crate) struct ReworkWorktree<'a> {
    pub path: &'a Path,
    pub branch: Option<&'a str>,
    pub locked: bool,
    prunable: bool,
}

impl ReworkWorktree<'_> {
    pub(crate) fn absent_prunable(&self) -> bool {
        self.prunable
            && !self.locked
            && matches!(
                std::fs::symlink_metadata(self.path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
    }
}

pub(crate) fn rework_worktree_inventory(main: &Path) -> Result<Vec<u8>> {
    let out = linked_rework_command(main, &["worktree", "list", "--porcelain", "-z"])
        .output()
        .context("could not inspect branch worktree ownership")?;
    anyhow::ensure!(
        out.status.success(),
        "could not inspect branch worktree ownership: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

pub(crate) fn rework_worktrees(inventory: &[u8]) -> impl Iterator<Item = ReworkWorktree<'_>> {
    use std::os::unix::ffi::OsStrExt;
    let mut fields = inventory.split(|&b| b == 0);
    std::iter::from_fn(move || {
        let path = Path::new(std::ffi::OsStr::from_bytes(
            fields.next()?.strip_prefix(b"worktree ")?,
        ));
        let mut worktree = ReworkWorktree {
            path,
            branch: None,
            locked: false,
            prunable: false,
        };
        for field in fields.by_ref().take_while(|field| !field.is_empty()) {
            if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
                worktree.branch = std::str::from_utf8(branch).ok();
            }
            worktree.locked |= field == b"locked" || field.starts_with(b"locked ");
            worktree.prunable |= field == b"prunable" || field.starts_with(b"prunable ");
        }
        Some(worktree)
    })
}

pub(crate) fn linked_rework_has_merge(tree: &Path) -> Result<bool> {
    Ok(rework_merge_head(&|args| linked_rework_git(tree, args))?.is_some())
}

fn admit_pinned_rework(
    main: &Path,
    heads: &ReworkHeads,
    admit_conflict: &dyn Fn() -> Result<()>,
) -> Result<()> {
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "-z",
        &heads.head,
        &heads.base,
    ];
    let probe = || -> Result<Vec<String>> {
        let mut cmd = linked_rework_command(main, &args);
        cmd.stdin(std::process::Stdio::null());
        let crate::BoundedCommandOutput::Completed(out) =
            crate::command_output_with_timeout_detail(cmd, std::time::Duration::from_secs(30))
        else {
            anyhow::bail!("merge-tree timed out");
        };
        anyhow::ensure!(
            out.status.success()
                || repair_merge_tree_conflict(&args, out.status.code(), &out.stdout),
            "{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        repair_merge_tree_paths(std::str::from_utf8(&out.stdout)?)
            .context("invalid merge-tree evidence")
    };
    let paths = probe().with_context(|| format!(
        "cannot verify the pinned merge; inspect with `git -C {} merge-tree --write-tree --name-only -z {} {}` before retrying fresh pickup",
        crate::shell_quote(&main.to_string_lossy()), heads.head, heads.base
    ))?;
    if !paths.is_empty() {
        admit_conflict()?;
    }
    Ok(())
}

// Free only this absent registration's branch, retaining its Git admin
// directory (including its index, merge metadata and the old tip).
// trace:BUG-2688 | ai:codex
fn detach_absent_rework(main: &Path, path: &Path, branch: &str) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let common = linked_rework_git(
        main,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let mut matched = None;
    let mut target = path.join(".git").as_os_str().as_bytes().to_vec();
    target.push(b'\n');
    for entry in std::fs::read_dir(Path::new(&common).join("worktrees"))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let gitdir = entry.path().join("gitdir");
        let gitdir = aida_core::seat_clone_detect::read_regular_file_result(&gitdir)?;
        if gitdir.as_bytes() == target {
            anyhow::ensure!(
                matched.is_none(),
                "ambiguous absent worktree registration at {}",
                path.display()
            );
            matched = Some(entry.file_name());
        }
    }
    let id = matched.context("missing Git registration for absent rework seat")?;
    let head_ref = format!(
        "worktrees/{}/HEAD",
        id.to_str().context("non-UTF-8 worktree registration ID")?
    );
    let branch_ref = format!("refs/heads/{branch}");
    let tip = linked_rework_git(main, &["rev-parse", "--verify", &head_ref])?;
    anyhow::ensure!(
        linked_rework_git(main, &["symbolic-ref", &head_ref])? == branch_ref
            && rework_worktrees(&rework_worktree_inventory(main)?).any(|entry| entry.path == path
                && entry.branch == Some(branch)
                && entry.absent_prunable()),
        "absent worktree registration at {} changed; retry fresh pickup after checking ownership",
        path.display()
    );
    linked_rework_git(main, &["update-ref", "--no-deref", &head_ref, &tip, &tip])?;
    Ok(())
}

// Under the exclusive placement lock and after branch occupancy checks,
// keep the previous local tip independently before moving the branch.
pub(crate) fn align_pinned_rework_branch(main: &Path, branch: &str, head: &str) -> Result<()> {
    let reference = format!("refs/heads/{branch}");
    let old = linked_rework_git(main, &["rev-parse", "--verify", &reference])?;
    if old != head {
        let backup = format!("refs/heads/aida-recovery/{}", uuid::Uuid::new_v4());
        linked_rework_git(main, &["update-ref", &backup, &old, ""])?;
        linked_rework_git(main, &["update-ref", &reference, head, &old]).with_context(|| {
            format!("local tip preserved at {backup}; retry fresh pickup after checking {branch}")
        })?;
        eprintln!("  fresh rework: preserved previous {branch} tip {old} at {backup}");
    }
    Ok(())
}

// Explicit fresh pickup archives only the retained merges of this PR in
// managed clone locations or registered, unlocked linked worktrees. It
// never reclaims a lease. All file/index changes stay in the old directory.
// trace:BUG-2688 | ai:codex
pub(crate) fn archive_retained_rework(
    main: &Path,
    scope: &str,
    pr: u64,
    branch: &str,
    heads: &ReworkHeads,
    admit_conflict: impl Fn() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    archive_retained_rework_with(main, scope, pr, branch, heads, admit_conflict, |path| {
        aida_core::worktree_holders::check_worktree_removal(path).map_err(Into::into)
    })
}

fn archive_retained_rework_with(
    main: &Path,
    scope: &str,
    pr: u64,
    branch: &str,
    heads: &ReworkHeads,
    admit_conflict: impl Fn() -> Result<()>,
    check_holders: impl Fn(&Path) -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let _placement = lock_seat_placement(main, true)?;
    let reserved = crate::repair_journal::RepairGuard::lock(main)?.reserved_seats(pr, branch);
    let candidate_clones = clone_paths(main, &crate::slugify(scope))
        .chain(reserved.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let inventory = rework_worktree_inventory(main)?;
    let pr_head = &heads.head;
    let project_main = main.canonicalize()?;
    let mut retained = Vec::new();
    for path in candidate_clones {
        if reserved.contains_key(&path)
            && matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            && !rework_worktrees(&inventory).any(|entry| entry.path == path)
        {
            retained.push((path, None));
            continue;
        }
        if !is_seat_clone_location(main, &path)
            || checked_out_branch(&path).as_deref() != Some(branch)
        {
            continue;
        }
        let identity = dir_identity(&path)?;
        let merge_path = path.join(".git/MERGE_HEAD");
        match aida_core::seat_clone_detect::read_regular_file_result(&merge_path) {
            Ok(merge) => {
                anyhow::ensure!(
                    is_full_oid(merge.trim()),
                    "invalid MERGE_HEAD evidence; inspect with `od -c -- {}`",
                    crate::shell_quote(&merge_path.to_string_lossy())
                );
                require_retained_clone_main(&project_main, &path)?;
                retained.push((path, Some((identity, None, merge.trim().to_owned()))));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot inspect MERGE_HEAD at {}", merge_path.display())
                })
            }
        }
    }
    for entry in rework_worktrees(&inventory) {
        if entry.path == project_main || entry.branch != Some(branch) {
            continue;
        }
        anyhow::ensure!(
            entry.path.to_str().is_some(),
            "non-UTF-8 path holds branch {branch}"
        );
        if entry.absent_prunable() {
            retained.push((entry.path.to_owned(), None));
            continue;
        }
        let admin = linked_registration_identity(main, entry.path)?;
        let identity = dir_identity(entry.path)?;
        if let Some(merge) = rework_merge_head(&|args| linked_rework_git(entry.path, args))? {
            anyhow::ensure!(!entry.locked, "retained worktree {} is locked; after its owner exits run `git -C {} worktree unlock {}`", entry.path.display(), crate::shell_quote(&main.to_string_lossy()), crate::shell_quote(&entry.path.to_string_lossy()));
            retained.push((entry.path.to_owned(), Some((identity, Some(admin), merge))));
        }
    }

    if !retained.is_empty() {
        admit_pinned_rework(main, heads, &admit_conflict)?;
    }
    let mut preserved = Vec::new();
    for (from, state) in retained {
        let from = if let Some((identity, _, _)) = &state {
            ensure_dir_identity(&from, *identity)?;
            let from = from.canonicalize()?;
            ensure_dir_identity(&from, *identity)?;
            from
        } else {
            from
        };
        let cwd = std::env::current_dir()?;
        anyhow::ensure!(
            !cwd.starts_with(&from),
            "retained seat is the current checkout; run `cd {}` before retrying fresh pickup",
            crate::shell_quote(&main.to_string_lossy())
        );
        let mut journal = crate::repair_journal::RepairGuard::lock(main)?;
        let (leases, locks) =
            crate::leases::list_seat_ownership_strict(main).map_err(anyhow::Error::msg)?;
        let matches_seat =
            |path: &Path| path == from || path.canonicalize().is_ok_and(|path| path == from);
        if let Some(lock) = locks
            .iter()
            .find(|lock| matches_seat(Path::new(&lock.worktree_path)))
        {
            anyhow::bail!("retained seat {} is locked by {}; ask the advisor to release it with `aida lock release {}` before retrying fresh pickup", from.display(), lock.authorized_by, crate::shell_quote(&from.to_string_lossy()));
        }
        if let Some(lease) = leases.iter().find(|lease| {
            matches_seat(&lease.worktree_path)
                || (!matches!(&state, Some((_, None, _))) && lease.branch == branch)
        }) {
            anyhow::bail!("retained seat {} is leased; once the owner exits, release it with `aida session end {}`", from.display(), lease.id);
        }
        let reservation =
            journal.reservation_for_relocation(main, &from, pr, branch, &leases, scope)?;
        let reservation_id = reservation.map(crate::repair_journal::reservation_id);
        check_holders(&from)
            .with_context(|| format!("retained seat {} has process holders; retry `aida queue work {} --fresh` after they exit", from.display(), crate::shell_quote(scope)))?;
        let Some((identity, admin, merge)) = state else {
            anyhow::ensure!(
                matches!(std::fs::symlink_metadata(&from), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
                "absent seat reappeared"
            );
            if rework_worktrees(&inventory).any(|entry| entry.path == from) {
                detach_absent_rework(main, &from, branch)?;
            } else {
                anyhow::ensure!(
                    reservation_id.as_ref() == reserved.get(&from)
                        && reservation_id.is_some()
                        && !rework_worktrees(&rework_worktree_inventory(main)?)
                            .any(|entry| entry.path == from),
                    "absent seat ownership changed"
                );
            }
            release_preserved_reservation(main, &mut journal, reservation_id.as_deref())?;
            continue;
        };
        let current_merge = if let Some(expected) = admin {
            anyhow::ensure!(
                linked_registration_identity(main, &from)? == expected,
                "worktree registration changed"
            );
            rework_merge_head(&|args| linked_rework_git(&from, args))?
        } else {
            Some(
                aida_core::seat_clone_detect::read_regular_file_result(
                    &from.join(".git/MERGE_HEAD"),
                )?
                .trim()
                .to_owned(),
            )
        };
        anyhow::ensure!(
            current_merge.as_deref() == Some(merge.as_str()),
            "retained MERGE_HEAD at {} changed; retry `aida queue work {} --fresh`",
            from.display(),
            crate::shell_quote(scope),
        );
        let name = from
            .file_name()
            .and_then(|n| n.to_str())
            .context("invalid retained seat name")?;
        let to = from.with_file_name(format!("{name}.retained-{}", uuid::Uuid::new_v4()));
        ensure_dir_identity(&from, identity)?;
        if let Some(admin) = admin {
            move_rework_worktree(main, &from, &to, branch, pr_head, identity, admin)?;
        } else {
            let scoped_path = clone_paths(&project_main, &crate::slugify(scope)).any(|path| {
                path.file_name() == from.file_name()
                    && path.parent().and_then(|p| p.canonicalize().ok()).as_deref() == from.parent()
            });
            let journal_bound = reservation.is_some_and(|entry| {
                entry.key.repository == project_main
                    && entry.tree_path == from
                    && entry.base == merge
                    && retained_clone_tip_matches(&from, branch, &entry.key.original_head)
            });
            move_rework_clone(
                &project_main,
                &from,
                &to,
                branch,
                identity,
                scoped_path,
                journal_bound,
                scope,
            )?;
        }
        eprintln!("  fresh rework: preserved {} with MERGE_HEAD {merge} at {}; verified origin/{branch} at {}", from.display(), to.display(), pr_head);
        release_preserved_reservation(main, &mut journal, reservation_id.as_deref())?;
        preserved.push(to);
    }
    Ok(preserved)
}

fn release_preserved_reservation(
    main: &Path,
    journal: &mut crate::repair_journal::RepairGuard,
    id: Option<&str>,
) -> Result<()> {
    if let Some(id) = id {
        let backup = journal
            .heal_dead_lane_reservation(main, id)?
            .map_err(anyhow::Error::msg)?;
        eprintln!(
            "  fresh rework: released the previous lane's reservation (backup {})",
            backup.display()
        );
    }
    Ok(())
}

// trace:BUG-2688 | ai:codex
fn require_retained_clone_main(main: &Path, clone: &Path) -> Result<()> {
    anyhow::ensure!(
        clone_marker_main(clone).as_deref() == Some(main),
        "retained clone {} lacks matching marker and object alternates for {}; left in place. Inspect its alternates with `cat -- {}`",
        clone.display(),
        main.display(), crate::shell_quote(&clone.join(".git/objects/info/alternates").to_string_lossy())
    );
    Ok(())
}

// The journal binds the old generation, which can predate the current PR
// head. Check its tip using plain files, without executing clone Git.
// trace:BUG-2688 | ai:codex
fn retained_clone_tip_matches(clone: &Path, branch: &str, expected: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    let git_dir = clone.join(".git");
    let read = aida_core::seat_clone_detect::read_regular_file_result;
    match read(&git_dir.join(&reference)) {
        Ok(tip) => tip.trim() == expected,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            read(&git_dir.join("packed-refs")).is_ok_and(|refs| {
                refs.lines()
                    .any(|line| line.split_once(' ') == Some((expected, reference.as_str())))
            })
        }
        Err(_) => false,
    }
}

// trace:BUG-2688 | ai:codex
fn move_rework_clone(
    main: &Path,
    from: &Path,
    to: &Path,
    branch: &str,
    identity: (u64, u64),
    scoped_path: bool,
    journal_bound: bool,
    scope: &str,
) -> Result<()> {
    require_retained_clone_main(main, from)?;
    anyhow::ensure!(
        checked_out_branch(from).as_deref() == Some(branch),
        "retained clone changed branch"
    );
    let pin = pin_ref(from).context("invalid retained clone pin")?;
    let new_pin = pin_ref(to).context("invalid archive pin")?;
    let refs = linked_rework_git(
        main,
        &["for-each-ref", "--format=%(refname) %(objectname)", &pin],
    )?;
    let oid = refs
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find_map(|(name, oid)| (name == pin).then_some(oid));
    let marker: SeatCloneMarker = toml::from_str(
        &aida_core::seat_clone_detect::read_regular_file_result(&from.join(MARKER_RELATIVE))?,
    )?;
    let pin_matches_marker =
        marker.branch == branch && marker.pin == pin && oid == Some(marker.base_sha.as_str());
    // The placement pin identifies this clone's creation, even if either
    // the seat's local tip or the remote PR head has since moved.
    let manual_archive = from.with_file_name(format!("retained-seat-{}", uuid::Uuid::new_v4()));
    anyhow::ensure!(
        (pin_matches_marker && scoped_path)
            || (journal_bound && (oid.is_none() || pin_matches_marker)),
        "retained clone {} has no matching host pin or exact repair reservation; left in place. Expected marker branch {branch} and pin {pin} at the recorded creation SHA {:?}; marker names branch {:?} and pin {:?}, host pin is {} (scope path matches: {scoped_path}). Inspect from the host with `git -C {} show-ref --verify -- {}` and `cd {} && aida doctor check repair-journal`. If you can independently verify this is your seat and it has no lock, lease or process holder, preserve the entire directory from the host with `mv -T --no-clobber -- {} {}`. Verify it is at the new path, then retry `cd {} && aida queue work {} --fresh`",
        from.display(),
        marker.base_sha,
        marker.branch,
        marker.pin,
        oid.unwrap_or("missing"),
        crate::shell_quote(&main.to_string_lossy()),
        crate::shell_quote(&pin),
        crate::shell_quote(&main.to_string_lossy()),
        crate::shell_quote(&from.to_string_lossy()),
        crate::shell_quote(&manual_archive.to_string_lossy()),
        crate::shell_quote(&main.to_string_lossy()),
        crate::shell_quote(scope),
    );
    if let Some(oid) = oid {
        linked_rework_git(main, &["update-ref", &new_pin, oid, ""])?;
    }
    if let Err(error) = relocate_rework_directory(from, to, identity) {
        if let Some(oid) = oid {
            let _ = linked_rework_git(main, &["update-ref", "-d", &new_pin, oid]);
        }
        return Err(error);
    }
    if let Some(oid) = oid {
        linked_rework_git(main, &["update-ref", "-d", &pin, oid]).with_context(|| {
            format!(
                "retained clone is at {}, but its old pin could not be released",
                to.display()
            )
        })?;
    }
    Ok(())
}

// trace:BUG-2688 | ai:codex
fn linked_registration_identity(main: &Path, tree: &Path) -> Result<(u64, u64)> {
    let common = PathBuf::from(linked_rework_git(
        main,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?)
    .canonicalize()?;
    let read = aida_core::seat_clone_detect::read_regular_file_result;
    let pointer = tree.join(".git");
    let forward = read(&pointer)?;
    let admin = tree
        .join(
            forward
                .trim_end_matches('\n')
                .strip_prefix("gitdir: ")
                .context("invalid worktree pointer")?,
        )
        .canonicalize()?;
    anyhow::ensure!(
        admin.parent() == Some(common.join("worktrees").as_path())
            && admin
                .join(read(&admin.join("commondir"))?.trim_end_matches('\n'))
                .canonicalize()?
                == common
            && admin
                .join(read(&admin.join("gitdir"))?.trim_end_matches('\n'))
                .canonicalize()?
                == pointer.canonicalize()?,
        "retained worktree {} does not match host registration",
        tree.display()
    );
    dir_identity(&admin)
}

fn move_rework_worktree(
    main: &Path,
    from: &Path,
    to: &Path,
    branch: &str,
    pr_head: &str,
    identity: (u64, u64),
    expected_admin: (u64, u64),
) -> Result<()> {
    anyhow::ensure!(
        linked_rework_git(from, &["symbolic-ref", "--short", "HEAD"])? == branch,
        "retained worktree changed branch"
    );
    let head = linked_rework_git(from, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let repair = |path: &Path| {
        let mut cmd = aida_core::git_ops::host_git_command(main);
        cmd.args(["-c", "core.hooksPath=/dev/null", "worktree", "repair", "--"])
            .arg(path);
        run(cmd, "re-register retained worktree")
    };
    let registration = rework_worktree_inventory(main)?;
    anyhow::ensure!(
        rework_worktrees(&registration)
            .any(|entry| entry.path == from && entry.branch == Some(branch) && !entry.locked),
        "retained worktree registration changed or became locked"
    );
    anyhow::ensure!(
        linked_registration_identity(main, from)? == expected_admin,
        "worktree registration changed before relocation"
    );
    relocate_rework_directory(from, to, identity)?;
    let verify_repair = || {
        repair(to)?;
        let registration = rework_worktree_inventory(main)?;
        anyhow::ensure!(
            rework_worktrees(&registration).any(|entry| entry.path == to),
            "retained worktree registration was not moved"
        );
        Ok::<_, anyhow::Error>(())
    };
    if let Err(error) = verify_repair() {
        let restored = aida_core::mailbox_lifecycle::rename_noreplace(to, from).is_ok()
            && repair(from).is_ok();
        return Err(error).context(if restored {
            format!("retained worktree restored at {}", from.display())
        } else {
            format!(
                "check {} and {} before retrying `git worktree repair`",
                from.display(),
                to.display()
            )
        });
    }

    // Only move the local branch after the archived worktree independently
    // anchors its old tip. No checkout, reset, or index write in the archive.
    linked_rework_git(to, &["update-ref", "--no-deref", "HEAD", &head])
        .with_context(|| format!("could not detach the retained worktree at {}", to.display()))?;
    if head != pr_head {
        linked_rework_git(
            main,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                pr_head,
                &head,
            ],
        )
        .with_context(|| {
            format!(
                "previous branch tip is preserved at {}; cannot align local {branch}",
                to.display()
            )
        })?;
    }
    Ok(())
}

fn relocate_rework_directory(from: &Path, to: &Path, identity: (u64, u64)) -> Result<()> {
    ensure_dir_identity(from, identity)?;
    aida_core::mailbox_lifecycle::rename_noreplace(from, to).with_context(|| {
        format!(
            "cannot move retained seat {} to {}",
            from.display(),
            to.display()
        )
    })?;
    if dir_identity(to).ok() != Some(identity) {
        let restored = aida_core::mailbox_lifecycle::rename_noreplace(to, from).is_ok();
        anyhow::bail!(
            "retained seat changed during relocation; inspect {}",
            if restored { from } else { to }.display()
        );
    }
    Ok(())
}

/// Publish the prepared tip using only the trusted host origin. Clone objects
/// cross the existing sandboxed bundle boundary; seat hooks never run.
// trace:BUG-2456 | ai:codex
fn publish_rework_merge(main: &Path, tree: &Path, branch: &str, tip: &str) -> Result<()> {
    crate::ship_from_seat::require_publication_branch(main, tree, branch)?;
    let imported = host_recorded_clone(main, tree)
        .map(|seat| crate::ship_from_seat::import_seat_branch(&seat, branch))
        .transpose()?;
    // Keep the result until cleanup has run, including tip mismatch, rejection
    // and timeout. Compare-delete only the exact object imported by this call.
    let result = (|| {
        if let Some((_, imported_tip)) = &imported {
            anyhow::ensure!(
                imported_tip == tip,
                "rework branch changed before publication"
            );
        }
        crate::ship_from_seat::require_publication_branch(main, tree, branch)?;
        // trace:BUG-2618 | ai:codex — queue rework is a head publication too.
        crate::pr_head_guard::check_branch(main, branch)?;
        let mut push = aida_core::git_ops::host_remote_git_command(main, "origin");
        push.args([
            "-c",
            "core.hooksPath=/dev/null",
            "push",
            "origin",
            &format!("{tip}:refs/heads/{branch}"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
        match crate::command_output_with_timeout_detail(push, std::time::Duration::from_secs(30)) {
            crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(()),
            crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
                "could not publish prepared rework merge: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
            _ => anyhow::bail!("rework merge publication timed out; no vendor launched"),
        }
    })();
    if let Some((reference, imported_tip)) = imported {
        let mut cleanup = git(main);
        cleanup.args(["update-ref", "-d", &reference, &imported_tip]);
        let cleanup = run(cleanup, "remove rework import");
        if let Err(why) = cleanup {
            anyhow::bail!("rework import cleanup failed: {why:#}; publication result: {result:?}");
        }
    }
    result
}

/// Merge a host-fetched remote PR head after committing the child's edits.
/// Objects are available through the clone's host-object alternates. Every
/// clone command stays inside the existing writable Git sandbox, with the
/// host identity and hooks disabled. Conflicts abort and refuse publication.
// trace:TASK-2879 | ai:codex
pub(crate) fn merge_publication_head(
    main: &Path,
    tree: &Path,
    branch: &str,
    remote: &str,
    message: &str,
    pinned: (u64, u64),
) -> Result<()> {
    use std::ffi::{OsStr, OsString};
    let value = |key: &str| -> Result<OsString> {
        let mut cmd = git(main);
        cmd.args(["config", "--get", key]);
        let value = run(cmd, "read host publication identity")?;
        anyhow::ensure!(!value.is_empty(), "missing host {key}");
        Ok(value.into())
    };
    let name = value("user.name")?;
    let email = value("user.email")?;
    let identity = [
        ("GIT_AUTHOR_NAME", name.clone()),
        ("GIT_AUTHOR_EMAIL", email.clone()),
        ("GIT_COMMITTER_NAME", name),
        ("GIT_COMMITTER_EMAIL", email),
    ];
    merge_publication_head_with(
        &|args| {
            ensure_dir_identity(tree, pinned)?;
            let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            run_commit_sandboxed(tree, main, &args, &identity)
                .map(|out| String::from_utf8_lossy(&out).trim().to_string())
                .map_err(|e| anyhow::anyhow!("publication Git failed: {e}"))
        },
        branch,
        remote,
        message,
    )
}

// trace:TASK-2879 | ai:codex
fn merge_publication_head_with(
    execute: &dyn Fn(&[&str]) -> Result<String>,
    branch: &str,
    remote: &str,
    message: &str,
) -> Result<()> {
    anyhow::ensure!(
        execute(&["symbolic-ref", "--short", "HEAD"])? == branch,
        "publication seat is not on its recorded branch {branch}"
    );
    anyhow::ensure!(
        execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err(),
        "publication seat already has an unfinished merge"
    );
    anyhow::ensure!(
        execute(&["status", "--porcelain", "--untracked-files=all"])?.is_empty(),
        "publication seat has pending changes; commit them before merging the remote PR head"
    );
    if let Err(why) = execute(&["merge", "--no-edit", "--no-verify", "-m", message, remote]) {
        if execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
            execute(&["merge", "--abort"]).context("could not abort publication merge")?;
        }
        anyhow::bail!("could not merge the remote PR head; nothing pushed: {why:#}");
    }
    Ok(())
}

/// Restore with host Git, then run pinned generators with Git metadata read-only.
/// The generator driver is embedded in the host image; command implementations
/// come from the pinned base, not lane edits. No host process executes seat code.
// trace:STORY-2710 | ai:codex
fn regenerate_repair_outputs(
    tree: &Path,
    main: &Path,
    base: &str,
    identity: &[(&'static str, std::ffi::OsString)],
    parent: &aida_core::exec_context::Environment,
    restore_only: bool,
) -> Result<()> {
    use std::ffi::OsStr;
    // Repositories without registered outputs need no generator (including
    // minimal worktree fixtures). Inspect the pinned host object, not the seat.
    let mut inventory = git(main);
    inventory.args([
        "ls-tree",
        "-r",
        "--name-only",
        base,
        "--",
        "scripts/exec-context-legacy.json",
        "aida-cli-lib/src/tests/golden/cli_help_snapshot.txt", // trace:TASK-3803 | ai:codex
        ".claude/skills",
        ".claude/commands",
        ".claude/hooks",
        ".agents/skills",
        ".aida/discipline",
        "aida-core/templates/skills",
    ]);
    if run(inventory, "inspect pinned regeneration outputs")?.is_empty() {
        return Ok(());
    }
    let pack_outputs = serde_json::to_string(
        &aida_core::scaffolding::agent_pack::rendered_portable_pack(
            aida_core::scaffolding::inventory::PORTABLE_PACK,
            &aida_core::scaffolding::ScaffoldConfig::default(),
        )
        .keys()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>(),
    )?;
    let driver = include_str!("../../scripts/repair-generated.py");
    let restore = [
        OsStr::new("-c"),
        OsStr::new(driver),
        OsStr::new(base),
        OsStr::new("restore"),
        OsStr::new(&pack_outputs),
    ];
    let config = tempfile::NamedTempFile::new()?;
    std::fs::write(config.path(), probe_repo_config(main, tree))?;
    let mut sandbox = commit_sandbox_args(
        tree,
        main,
        &crate::session::contained_read_allowlist(main),
        config.path(),
        identity,
    )?;
    sandbox.pop(); // append mounts before the command separator
    let clone_git = tree.join(".git");
    let git_dir = if host_recorded_clone(main, tree).is_some() {
        clone_git.clone()
    } else {
        // Only this accessor may run host Git in a linked worktree. Its index
        // lives outside the worktree, under main's otherwise read-only .git.
        let mut cmd = aida_core::git_ops::host_git_command(tree);
        cmd.args(["rev-parse", "--absolute-git-dir"]);
        PathBuf::from(run(cmd, "locate linked repair index")?)
    };
    let linked_git = git_dir != clone_git;
    if linked_git {
        let common = main.join(".git");
        let clone_config = clone_git.join("config");
        for arg in &mut sandbox {
            if *arg == clone_git.as_os_str() {
                *arg = git_dir.clone().into();
            } else if *arg == clone_config.as_os_str() {
                *arg = common.join("config").into();
            }
        }
        // GIT_DIR selects the private linked index; GIT_COMMON_DIR selects
        // the trusted common objects/config. Neither may be inferred by Git.
        for i in 0..sandbox.len().saturating_sub(2) {
            if sandbox[i] == "--setenv" && sandbox[i + 1] == "GIT_COMMON_DIR" {
                sandbox[i + 2] = common.clone().into();
            }
        }
        sandbox.extend([
            "--bind".into(),
            git_dir.clone().into(),
            git_dir.clone().into(),
        ]);
    }
    let execute_driver = |sandbox: &[std::ffi::OsString],
                          args: &[&OsStr],
                          timeout: std::time::Duration|
     -> Result<()> {
        let mut cmd = std::process::Command::new("bwrap");
        cmd.args(sandbox)
            .arg("--")
            .arg("python3")
            .args(args)
            .stdin(std::process::Stdio::null());
        match crate::command_output_with_timeout_capped(cmd, timeout, PROBE_OUTPUT_CAP) {
            crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(()),
            crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
                "repair regeneration refused: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
            _ => {
                anyhow::bail!("repair regeneration failed or timed out; merge and journal retained")
            }
        }
    };
    // The embedded restore driver runs no checkout-authored programs.
    if restore_only {
        return execute_driver(&sandbox, &restore, PROBE_TIMEOUT);
    }
    sandbox.extend(["--ro-bind".into(), git_dir.clone().into(), git_dir.into()]);
    if linked_git {
        sandbox.extend([
            "--ro-bind".into(),
            clone_git.clone().into(),
            clone_git.into(),
        ]);
    }
    // Parent metadata mounts hide earlier child mounts. Restore the trusted
    // config last, including the common config used by linked worktrees.
    // trace:STORY-2710 | ai:codex
    sandbox.extend([
        "--ro-bind".into(),
        config.path().into(),
        if linked_git {
            main.join(".git/config").into()
        } else {
            tree.join(".git/config").into()
        },
    ]);
    let trusted = tempfile::tempdir()?;
    for (i, path) in [
        "Makefile",
        "scripts/check-exec-context.py",
        "scripts/check-template-hooks.py",
    ]
    .iter()
    .enumerate()
    {
        let mut cmd = git(main);
        cmd.args(["show", &format!("{base}:{path}")]);
        let out = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(10))
            .context("cannot read pinned generator")?;
        // Small fixture repositories may have no registered outputs or generators.
        if !out.status.success() {
            continue;
        }
        let file = trusted.path().join(i.to_string());
        std::fs::write(&file, out.stdout)?;
        sandbox.extend(["--ro-bind".into(), file.into(), tree.join(path).into()]);
    }
    // Locate installed toolchains explicitly while HOME remains private and
    // all host caches remain read-only. Downloads are forbidden in this phase.
    use aida_core::launch_policy::{EnvPolicy, EnvRecipient, LaunchKind};
    let mut tools = parent.clone();
    for (key, leaf) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if tools.get(key).is_none() {
            if let Some(home) = parent.get("HOME") {
                tools = tools.with_value(key, Some(Path::new(home).join(leaf).into()));
            }
        }
    }
    tools = tools
        .with_value("CARGO_NET_OFFLINE", Some("true".into()))
        .with_value("PYTHONDONTWRITEBYTECODE", Some("1".into()));
    let policy = EnvPolicy::new(
        crate::session::resolve_headless_vendor(main).launch_vendor(),
        LaunchKind::Fresh,
        EnvRecipient::ToolShell,
        [
            "CARGO_HOME",
            "RUSTUP_HOME",
            "CARGO_NET_OFFLINE",
            "PYTHONDONTWRITEBYTECODE",
        ]
        .map(Into::into),
    )?;
    // Fixed build decisions, as in `seat_build_env`: an empty RUSTC_WRAPPER
    // overrides a CARGO_HOME or repository `build.rustc-wrapper` (sccache),
    // whose shared cache is never writable here. The renderer builds into the
    // seat's own validated target (writable, outside host caches) so a
    // fresh-seat regeneration reuses the seeded artifacts; without one, cargo
    // falls back to a checkout-local target within the same bounded budget.
    // trace:STORY-2710 | ai:claude
    let seat_target = repair_seat_target(main, tree);
    if let Some(dir) = &seat_target {
        sandbox.extend(["--bind".into(), dir.clone().into(), dir.clone().into()]);
    }
    let decisions = [
        ("RUSTC_WRAPPER".into(), Some(std::ffi::OsString::new())),
        (
            "CARGO_TARGET_DIR".into(),
            seat_target.map(std::ffi::OsString::from),
        ),
    ];
    for (key, value) in policy.project(&tools, decisions)?.pairs() {
        sandbox.extend(["--setenv".into(), key.to_owned(), value.to_owned()]);
    }
    // The driver bounds every generation phase (ledger, template renderer
    // build and run, final ledger validation) inside one budget below the
    // host's kill bound, so a slow or refused run rolls its own writes back.
    // trace:STORY-2710 | ai:claude
    let budget = REPAIR_GENERATION_TIMEOUT
        .saturating_sub(std::time::Duration::from_secs(20))
        .as_secs()
        .to_string();
    execute_driver(
        &sandbox,
        &[
            OsStr::new("-c"),
            OsStr::new(driver),
            OsStr::new(base),
            OsStr::new("generate"),
            OsStr::new(&pack_outputs),
            OsStr::new(&budget),
        ],
        REPAIR_GENERATION_TIMEOUT,
    )
}

/// The host kill bound for publication-time regeneration. Unlike a Git probe,
/// it may build the pinned template renderer from a cold or partly seeded
/// target, so it is sized like a warm lane build rather than [`PROBE_TIMEOUT`].
/// One budget covers every generation phase.
// trace:STORY-2710 | ai:claude
pub(crate) const REPAIR_GENERATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(900);

/// The seat's own cargo target for repair regeneration: the host-recorded
/// clone's validated target, else the linked worktree lease's target. Either
/// is accepted only at `<main>/target/worktrees/<seat dir>` as a real
/// directory (never a symlink), so the writable bind cannot reach host caches.
// trace:STORY-2710 | ai:claude
pub(crate) fn repair_seat_target(main: &Path, tree: &Path) -> Option<PathBuf> {
    if let Some(clone) = host_recorded_clone(main, tree) {
        return clone.cargo_target_dir;
    }
    let main = main.canonicalize().ok()?;
    let tree = tree.canonicalize().ok()?;
    let expected = main
        .join("target")
        .join("worktrees")
        .join(tree.file_name()?);
    crate::list_leases(&main)
        .into_iter()
        .find(|lease| {
            lease.tree_kind != TreeKind::Clone
                && lease.worktree_path.canonicalize().is_ok_and(|p| p == tree)
        })?
        .cargo_target_dir
        .filter(|dir| {
            std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir())
                && dir.canonicalize().is_ok_and(|real| real == expected)
        })
        .map(|_| expected)
}

// merge-tree -z emits a tree OID, then conflicted paths up to the empty
// separator. Exit 1 is usable evidence only with this structured conflict list.
// Other failures (including timeouts and unsupported options) never reserve.
// trace:STORY-2710 | ai:codex
fn repair_merge_tree_paths(output: &str) -> Option<Vec<String>> {
    let (oid, rest) = output.split_once('\0')?;
    // Require the path-list terminator as well as the OID. Truncated probe
    // output must not become positive conflict evidence.
    let paths = if rest.is_empty() {
        ""
    } else {
        rest.split_once("\0\0")?.0
    };
    if ![40, 64].contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        paths
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn repair_merge_tree_conflict(args: &[&str], code: Option<i32>, stdout: &[u8]) -> bool {
    args.starts_with(&["merge-tree", "--write-tree", "--name-only", "-z"])
        && code == Some(1)
        && std::str::from_utf8(stdout)
            .ok()
            .and_then(repair_merge_tree_paths)
            .is_some_and(|paths| !paths.is_empty())
}

// Host-staged conflict preparation. The shared reservation precedes the merge;
// any ambiguous failure keeps it. Only proof of lane death may release it.
// trace:STORY-2710 | ai:codex
// `admit_conflict` refuses a conflicted repair this launch may not run. It is
// asked before the reservation and before any other journal write, so a
// refused route or ineligible launch leaves the journal and the seat as found.
// trace:BUG-2688 | ai:claude
fn prepare_host_repair_admitted(
    main: &Path,
    tree: &Path,
    pr: u64,
    lane_id: &str,
    execute: &dyn Fn(&[&str]) -> Result<String>,
    branch: &str,
    seat: ReworkSeat,
    scope: &str,
    preserve_pending: &str,
    private_clone: bool,
    admit_conflict: &dyn Fn() -> Result<()>,
    publish: impl FnOnce(&str) -> Result<()>,
    regenerate: impl FnOnce(&str, bool) -> Result<()>,
    fetch_heads: impl FnOnce() -> Result<(String, String)>,
) -> Result<Vec<String>> {
    use crate::repair_journal::{RepairEntry, RepairGuard, RepairKey, SpawnOutcome};
    let fresh_command = format!("aida queue work {} --fresh", crate::shell_quote(scope));
    let resume = seat == ReworkSeat::Adopted;
    if execute(&["symbolic-ref", "--short", "HEAD"])? != branch {
        anyhow::bail!(ReworkRecovery(format!(
            "seat {} is not on the PR head branch {branch}; once the seat is unleased run `{fresh_command}`",
            tree.display()
        )));
    }
    let (base, pr_head) = fetch_heads()?;
    if let Some(merge) = rework_merge_head(execute)? {
        let head = execute(&["rev-parse", "HEAD"])?;
        let guard = RepairGuard::lock(main)?;
        let entry = guard.for_tree(tree, branch, &head, &merge).with_context(|| {
            ReworkRecovery(format!(
                "unfinished merge lacks host repair journal evidence for seat {}, HEAD {head}, MERGE_HEAD {merge}; once the seat is unleased run `{fresh_command}`",
                tree.display()
            ))
        })?;
        if !(resume
            && entry.lane_id == lane_id
            && entry.key.pr == pr
            && pr_head == head
            && merge == base)
        {
            anyhow::bail!(ReworkRecovery(format!(
                "pending repair does not match adopted lane {lane_id} for PR #{pr} at fetched head {pr_head} and base {base} (retained MERGE_HEAD {merge}); once the seat is unleased run `{fresh_command}`"
            )));
        }
        let paths = entry.original_conflict_paths.clone();
        let key = entry.key.clone();
        let prepared = entry.preparation_success == Some(true);
        let failed_generation = entry.regeneration_success == Some(false);
        drop(guard);
        admit_conflict()?;
        if !prepared || failed_generation {
            RepairGuard::lock(main)?.record_preparation(&key, false)?;
            regenerate(&merge, !failed_generation)?;
            RepairGuard::lock(main)?.record_preparation(&key, true)?;
            if failed_generation {
                RepairGuard::lock(main)?.record_regeneration(&key, true)?;
            }
        }
        RepairGuard::lock(main)?
            .for_tree(tree, branch, &head, &merge)?
            .require_prepared()?;
        return Ok(paths);
    }
    let dirty = !execute(&["status", "--porcelain", "--untracked-files=all"])?.is_empty();
    if dirty && !resume {
        return Err(anyhow::anyhow!(PreserveReworkSeat(format!(
            "rework seat {} has pending changes; merge refused. After its owner exits, save the edits with `{preserve_pending}`, release the seat with `aida session end {lane_id}`, and retry `{fresh_command}`",
            tree.display()
        ))));
    }
    let current = execute(&["rev-parse", "HEAD"])?;
    let behind = current != pr_head
        && !dirty
        && execute(&["merge-base", "--is-ancestor", &current, &pr_head]).is_ok();
    if !behind {
        execute(&["merge-base", "--is-ancestor", &pr_head, &current]).with_context(|| {
            ReworkRecovery(if dirty {
                format!("seat {} is behind or diverged from origin/{branch} at {pr_head}; edits preserved; after the lane exits save them with `{preserve_pending}`", tree.display())
            } else {
                let recovery = if private_clone && resume {
                    format!("once the lane exits, reconcile and publish its commits from the host with `aida ship --from-seat {}`", crate::shell_quote(&tree.to_string_lossy()))
                } else {
                    let backup = format!("aida-recovery/{}", uuid::Uuid::new_v4());
                    format!(
                        "once the lane exits, preserve the local branch and free its name with `git -C {} branch -m -- {} {}`; after releasing any lease, run `{fresh_command}`",
                        crate::shell_quote(&main.to_string_lossy()), crate::shell_quote(branch), backup
                    )
                };
                format!("seat {} cannot be fast-forwarded to origin/{branch} at {pr_head}; local commits preserved; {recovery}", tree.display())
            })
        })?;
        if seat == ReworkSeat::Fresh && current != pr_head {
            anyhow::bail!(ReworkRecovery(format!(
                "local {branch} has commits ahead of origin/{branch} at {pr_head}; commits preserved at {current}. Once the seat is unleased run `{fresh_command}` to place a seat at the verified PR head",
            )));
        }
    }
    if dirty {
        execute(&["merge-tree", "--write-tree", "HEAD", &base])
            .with_context(|| ReworkRecovery(format!("could not prove a clean merge for dirty resume; edits preserved at {}; after the lane exits save them with `{preserve_pending}`", tree.display())))?;
        return Ok(vec![]);
    }
    let before = if behind { pr_head.clone() } else { current };
    let key = RepairKey {
        repository: main.into(),
        pr,
        original_head: before.clone(),
    };
    // Reserve only a positively observed conflict; merge-tree changes no index.
    let merge_observation_recovery = || {
        let inspect = format!(
            "git -C {} merge-tree --write-tree --name-only -z {} {}",
            crate::shell_quote(&main.to_string_lossy()),
            crate::shell_quote(&before),
            crate::shell_quote(&base)
        );
        ReworkRecovery(format!(
            "could not verify merge-tree for HEAD {before} and base {base}; no reservation created; inspect the pinned merge from the host with `{inspect}`"
        ))
    };
    let observation = execute(&[
        "merge-tree",
        "--write-tree",
        "--name-only",
        "-z",
        &before,
        &base,
    ])
    .with_context(merge_observation_recovery)?;
    let observed_paths =
        repair_merge_tree_paths(&observation).with_context(merge_observation_recovery)?;
    if !observed_paths.is_empty() {
        if before != pr_head {
            anyhow::bail!(ReworkRecovery(format!(
                "repair requires exact PR head {pr_head}; local commits at {before} preserved; once the seat is unleased run `{fresh_command}`"
            )));
        }
        admit_conflict()?;
    }
    // An unadmitted route never modifies even a stale replacement seat.
    let protect_ignored = |target: &str| -> Result<()> {
        let ignored = execute(&[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ])?;
        if ignored.is_empty() {
            return Ok(());
        }
        let changes = execute(&[
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--diff-filter=ACMT",
            "--name-only",
            "-z",
            "HEAD",
            target,
            "--",
        ])?;
        let paths: std::collections::BTreeSet<_> =
            changes.split_terminator('\0').map(Path::new).collect();
        let ancestors: std::collections::BTreeSet<_> =
            paths.iter().flat_map(|path| path.ancestors()).collect();
        for name in ignored.split_terminator('\0') {
            let path = Path::new(name);
            anyhow::ensure!(
                !ancestors.contains(path) && !path.ancestors().any(|parent| paths.contains(parent)),
                "incoming change would overwrite ignored path {name:?}"
            );
        }
        Ok(())
    };
    if behind {
        protect_ignored(&pr_head).with_context(|| PreserveReworkSeat(format!("cannot safely advance seat; preserve its ignored files before retrying `{fresh_command}`")))?;
        execute(&["merge", "--ff-only", "--no-overwrite-ignore", "--no-verify", &pr_head])
            .with_context(|| PreserveReworkSeat(format!("could not advance clean seat to fetched origin/{branch} at {pr_head}; retry `{fresh_command}` after clearing the named obstruction")))?;
        anyhow::ensure!(
            execute(&["rev-parse", "HEAD"])? == pr_head,
            "seat HEAD changed while advancing to {pr_head}; retry `{fresh_command}` after checking ownership"
        );
    }
    if seat != ReworkSeat::Adopted {
        eprintln!(
            "  rework: preparing {} from fetched origin/{branch} at {before}",
            tree.display()
        );
    }
    if !observed_paths.is_empty() {
        let mut journal = RepairGuard::lock(main)?;
        journal
            .reserve(RepairEntry {
                key: key.clone(),
                dispatched_by: "direct-rework".into(),
                reserved_at: chrono::Utc::now().to_rfc3339(),
                lane_id: lane_id.into(),
                tree_path: tree.into(),
                branch: branch.into(),
                base: base.clone(),
                spawn: SpawnOutcome::Unknown,
                original_conflict_paths: observed_paths,
                regeneration_success: None,
                regenerated_tree: None,
                preparation_success: None,
                validation: None,
                refusal_notice: None,
            })
            .map_err(|error| anyhow::anyhow!(ReworkRecovery(format!("{error:#}"))))?;
        drop(journal);
        match execute(&[
            "merge",
            "--no-edit",
            "--no-overwrite-ignore",
            "--no-verify",
            &base,
        ]) {
            Ok(_) => {
                anyhow::bail!("conflict observation changed; reservation retained for recovery")
            }
            Err(why) => {
                if rework_merge_head(execute)
                    .with_context(|| PreserveReworkSeat(format!("cannot inspect MERGE_HEAD after merge failed: {why:#}; preserve the seat and inspect `aida doctor check repair-journal` before retrying")))?
                    .as_deref() != Some(base.as_str())
                {
                    return Err(why).context(PreserveReworkSeat(format!(
                        "merge failed without the expected MERGE_HEAD; after the lane exits, correct the Git error and preserve any obstructing files outside the seat, then run `aida session end {lane_id}`, `aida doctor heal repair-journal`, and retry `{fresh_command}`"
                    )));
                }
                let paths = execute(&["diff", "--name-only", "--diff-filter=U", "-z"])?;
                let paths: Vec<String> = paths
                    .split('\0')
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect();
                anyhow::ensure!(!paths.is_empty(), "merge failed without conflicts: {why:#}");
                RepairGuard::lock(main)?.record_paths(&key, &paths)?;
                RepairGuard::lock(main)?.record_preparation(&key, false)?;
                regenerate(&base, true)?;
                RepairGuard::lock(main)?.record_preparation(&key, true)?;
                // Return the persisted list, including generated files already staged.
                let journal = RepairGuard::lock(main)?;
                Ok(journal
                    .for_tree(tree, branch, &before, &base)?
                    .original_conflict_paths
                    .clone())
            }
        }
    } else {
        // Git's ort strategy can replace ignored files even with --no-overwrite-ignore.
        // The validated first merge-tree field identifies the incoming result tree.
        let merged_tree = observation.split('\0').next().unwrap_or_default();
        protect_ignored(merged_tree).with_context(|| {
            PreserveReworkSeat(format!(
                "cannot safely merge; preserve ignored files before retrying `{fresh_command}`"
            ))
        })?;
        execute(&[
            "merge",
            "--no-edit",
            "--no-overwrite-ignore",
            "--no-verify",
            &base,
        ])
        .with_context(|| {
            PreserveReworkSeat(format!(
                "merge stopped in {}; clear the obstruction before retrying `{fresh_command}`",
                tree.display()
            ))
        })?;
        let tip = execute(&["rev-parse", "HEAD"])?;
        if tip != before && before == pr_head {
            let parents = execute(&["show", "-s", "--format=%P", &tip])?;
            if parents.split_whitespace().collect::<Vec<_>>() == [pr_head.as_str(), base.as_str()] {
                publish(&tip)?;
            }
        }
        Ok(vec![])
    }
}

// The preparation with every conflicted repair admitted.
// trace:BUG-2688 | ai:claude
#[cfg(test)]
fn prepare_host_repair_with(
    main: &Path,
    tree: &Path,
    pr: u64,
    lane_id: &str,
    execute: &dyn Fn(&[&str]) -> Result<String>,
    branch: &str,
    resume: bool,
    publish: impl FnOnce(&str) -> Result<()>,
    regenerate: impl FnOnce(&str, bool) -> Result<()>,
    fetch_heads: impl FnOnce() -> Result<(String, String)>,
) -> Result<Vec<String>> {
    prepare_host_repair_admitted(
        main,
        tree,
        pr,
        lane_id,
        execute,
        branch,
        if resume {
            ReworkSeat::Adopted
        } else {
            ReworkSeat::Replacement
        },
        "BUG-2688",
        "git stash push --include-untracked",
        false,
        &|| Ok(()),
        publish,
        regenerate,
        fetch_heads,
    )
}

// Keep launch admission and the Git operations together: any failed fetch,
// ancestry check, merge or abort propagates before the vendor can start.
// trace:STORY-2475 | ai:codex
#[cfg(test)]
fn prepare_rework_merge_with(
    execute: &dyn Fn(&[&str]) -> Result<String>,
    branch: &str,
    resume: bool,
    publish: impl FnOnce(&str) -> Result<()>,
    fetch_heads: impl FnOnce() -> Result<(String, String)>,
) -> Result<Vec<String>> {
    anyhow::ensure!(
        execute(&["symbolic-ref", "--short", "HEAD"])? == branch,
        "rework seat is not on the PR head branch {branch}"
    );
    anyhow::ensure!(
        execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err(),
        "rework seat already has an unfinished merge"
    );
    let dirty = !execute(&["status", "--porcelain", "--untracked-files=all"])?.is_empty();
    anyhow::ensure!(
        resume || !dirty,
        "rework seat has pending changes; merge refused"
    );
    let (base, pr_head) = fetch_heads()?;
    // Dirty seats need preservation; clean/fresh pickups need only a current head.
    // trace:BUG-2504 | ai:codex
    execute(&["merge-base", "--is-ancestor", &pr_head, "HEAD"]).context(if resume && dirty {
        "seated rework branch is behind the PR head; edits preserved"
    } else {
        "seated rework branch is behind the PR head; retry pickup with its current head"
    })?;
    // Probe committed trees without touching pending edits, the index or HEAD.
    // A clean merge needs no host preparation before continuing pending work.
    // trace:BUG-2504 | ai:codex
    if resume && dirty {
        execute(&["merge-tree", "--write-tree", "HEAD", &base]).context(
            "could not prove a clean main merge for the dirty rework resume; edits preserved",
        )?;
        return Ok(Vec::new());
    }
    let before = execute(&["rev-parse", "HEAD"])?;
    let files = finish_rework_merge(execute, &base)?;
    // Only publish the host-created merge of the fetched PR head and base.
    // Existing local lane commits retain the post-exit trailer and punt/hold
    // gates, including when this merge reports "Already up to date".
    // trace:BUG-2456 | ai:codex
    if files.is_empty() && before == pr_head {
        let tip = execute(&["rev-parse", "HEAD"])?;
        if tip != before {
            let parents = execute(&["show", "-s", "--format=%P", &tip])?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            if parents == [pr_head.as_str(), base.as_str()] {
                publish(&tip)?;
            }
        }
    }
    Ok(files)
}

// trace:STORY-2475 | ai:codex
#[cfg(test)]
fn finish_rework_merge(
    execute: &dyn Fn(&[&str]) -> Result<String>,
    base: &str,
) -> Result<Vec<String>> {
    match execute(&["merge", "--no-edit", "--no-verify", base]) {
        Ok(_) => Ok(Vec::new()),
        Err(why) => {
            let files = execute(&["diff", "--name-only", "--diff-filter=U", "-z"]);
            // Conflict paths always require a successful abort. Even a
            // timed-out merge with no paths may have a MERGE_HEAD to clear.
            if files.as_ref().is_ok_and(|paths| !paths.is_empty())
                || execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
            {
                execute(&["merge", "--abort"])
                    .context("could not abort rework merge; no vendor launched")?;
            }
            let files: Vec<String> = files?
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            anyhow::ensure!(
                !files.is_empty(),
                "rework merge failed without conflict paths: {why:#}"
            );
            Ok(files)
        }
    }
}

#[cfg(test)]
mod rework_merge_tests {
    use super::*;
    // trace:BUG-2603 | ai:codex
    #[test]
    fn bug_2603_integration_rework_fetches_and_merges_only_actual_base() {
        let fx = tests::fixture();
        let g = |args: &[&str]| tests::git_ok(&fx.main, args);
        g(&["branch", "integration"]);
        g(&["checkout", "-b", "rework"]);
        std::fs::write(fx.main.join("lane"), "lane").unwrap();
        g(&["add", "lane"]);
        g(&["commit", "-qm", "lane"]);
        g(&["push", "origin", "rework"]);
        g(&["checkout", "integration"]);
        std::fs::write(fx.main.join("integration-only"), "integration").unwrap();
        g(&["add", "integration-only"]);
        g(&["commit", "-qm", "integration"]);
        let integration = g(&["rev-parse", "HEAD"]);
        g(&["push", "origin", "integration"]);
        g(&["checkout", "main"]);
        std::fs::write(fx.main.join("main-only"), "main").unwrap();
        g(&["add", "main-only"]);
        g(&["commit", "-qm", "main"]);
        let main = g(&["rev-parse", "HEAD"]);
        g(&["push", "origin", "main"]);
        g(&["checkout", "rework"]);
        let execute = |args: &[&str]| {
            let mut cmd = git(&fx.main);
            cmd.args(args);
            run(cmd, "test rework Git")
        };
        let before = execute(&["rev-parse", "HEAD"]).unwrap();
        let status = execute(&["status", "--porcelain"]).unwrap();
        let unknown = prepare_rework_merge_with_base(
            &fx.main,
            &fx.main,
            "rework",
            ReworkSeat::Replacement,
            2603,
            "integration-lane",
            "BUG-2603",
            &|| Ok(()),
            || {
                let metadata = verify_rework_metadata(
                    "rework",
                    crate::forge_observed::Observed::Transient("metadata unavailable".into()),
                )?;
                fetch_rework_heads(&fx.main, &metadata.base_ref, "rework")
            },
        )
        .unwrap_err();
        assert!(format!("{unknown:#}").contains("could not determine the rework PR base"));
        assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(execute(&["status", "--porcelain"]).unwrap(), status);
        assert!(execute(&["rev-parse", "--verify", "MERGE_HEAD"]).is_err());
        assert!(!fx.main.join(".aida/repair-journal.json").exists());
        // Publication requires host ownership evidence; main is the seat here,
        // so keep the lease file from reading as pending work.
        // trace:BUG-2633 | ai:claude
        record_test_publication_lease(&fx.main, &fx.main, "rework");
        std::fs::write(fx.main.join(".git/info/exclude"), ".aida/sessions/\n").unwrap();
        prepare_rework_merge_with_base(
            &fx.main,
            &fx.main,
            "rework",
            ReworkSeat::Replacement,
            2603,
            "integration-lane",
            "BUG-2603",
            &|| Ok(()),
            || {
                let metadata = verify_rework_metadata(
                    "rework",
                    fixture_rework_metadata("rework", "integration"),
                )?;
                fetch_rework_heads(&fx.main, &metadata.base_ref, "rework")
            },
        )
        .unwrap();
        assert!(execute(&["merge-base", "--is-ancestor", &integration, "HEAD"]).is_ok());
        assert!(execute(&["merge-base", "--is-ancestor", &main, "HEAD"]).is_err());
        assert!(fx.main.join("integration-only").exists());
        assert!(!fx.main.join("main-only").exists());
        let error = fetch_rework_heads(&fx.main, "missing-base", "rework")
            .unwrap_err()
            .to_string();
        assert!(error.contains("origin/missing-base"), "{error}");
        assert!(error.contains(" fetch origin "), "{error}");
        assert!(
            error.contains("refs/heads/rework:refs/remotes/origin/rework"),
            "{error}"
        );
    }

    // Run the actual registered library renderer in a child selected by the
    // fixture Makefile. No Cargo, fake symlinks, or handwritten pack layout.
    // trace:STORY-2710 | ai:codex
    #[test]
    #[ignore = "child generator entry point; invoked by the real portable renderer regression"]
    fn portable_renderer_child() {
        assert!(Path::new("repair-generator-fixture").is_file());
        aida_core::scaffolding::agent_pack::sync_portable_pack(
            Path::new("."),
            aida_core::scaffolding::inventory::PORTABLE_PACK,
            &aida_core::scaffolding::ScaffoldConfig::default(),
        )
        .unwrap();
    }

    #[test]
    fn story_2710_real_portable_renderer_outputs_are_registered() {
        let fx = tests::fixture();
        let config = aida_core::scaffolding::ScaffoldConfig::default();
        let expected = aida_core::scaffolding::agent_pack::rendered_portable_pack(
            aida_core::scaffolding::inventory::PORTABLE_PACK,
            &config,
        );
        assert!(expected.len() > 10);
        let inventory = serde_json::to_string(
            &expected
                .keys()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        std::fs::create_dir_all(fx.main.join("aida-core/templates/skills")).unwrap();
        std::fs::write(
            fx.main.join("aida-core/templates/skills/aida-fixture.md"),
            "fixture",
        )
        .unwrap();
        // Stage the actual renderer's template masters as well. An empty host
        // inventory then models a host older than the resolved template pack.
        for skill in aida_core::scaffolding::inventory::portable_skill_inventory(&config).values() {
            // Portable overrides replace the body, but the renderer still
            // requires the ordinary master to define the skill itself.
            let templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/templates");
            let flat = format!("skills/{}.md", skill.name);
            let folder = format!("skills/{}/SKILL.md", skill.name);
            let master = if templates.join(&flat).is_file() {
                flat
            } else {
                folder
            };
            let target = fx.main.join("aida-core/templates").join(&master);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(templates.join(master), target).unwrap();
            for file in &skill.files {
                let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../aida-core/templates")
                    .join(&file.source_key);
                let target = fx.main.join("aida-core/templates").join(&file.source_key);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(source, target).unwrap();
            }
        }
        std::fs::write(fx.main.join("repair-generator-fixture"), "fixture").unwrap();
        std::fs::write(fx.main.join(".gitignore"), "/.agents/\n").unwrap();
        let executable = crate::aida_bin::current_executable_path().unwrap();
        let quoted = format!(
            "'{}'",
            executable.display().to_string().replace('\'', "'\"'\"'")
        );
        let makefile = format!("sync-templates:\n\t{quoted} --exact seat_clone::rework_merge_tests::portable_renderer_child --ignored\n");
        std::fs::write(fx.main.join("Makefile"), &makefile).unwrap();
        tests::git_ok(&fx.main, &["add", "."]);
        tests::git_ok(
            &fx.main,
            &[
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "commit",
                "-qm",
                "registered real renderer fixture",
            ],
        );
        let base = tests::git_ok(&fx.main, &["rev-parse", "HEAD"]);
        assert!(tests::git_ok(
            &fx.main,
            &["ls-tree", "-r", "--name-only", &base, "--", ".agents"]
        )
        .is_empty());
        let generate = |inventory: &str| {
            std::process::Command::new("python3")
                .current_dir(&fx.main)
                .args([
                    "-c",
                    include_str!("../../scripts/repair-generated.py"),
                    &base,
                    "generate",
                    inventory,
                ])
                .output()
                .unwrap()
        };
        // Fresh and stale ignored packs also work with no host-known paths:
        // their staged real masters form the closed expected inventory.
        for (changed, inventory) in [(false, "[]"), (true, "[]"), (true, inventory.as_str())] {
            if changed {
                std::fs::write(fx.main.join(expected.keys().next().unwrap()), "stale pack")
                    .unwrap();
            }
            let out = generate(inventory);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            for (path, body) in &expected {
                assert_eq!(std::fs::read_to_string(fx.main.join(path)).unwrap(), *body);
            }
        }
        // The real renderer removes a retired skill's orphaned pack files.
        // trace:STORY-2710 | ai:claude
        let retired = fx.main.join(".agents/skills/aida-retired/SKILL.md");
        std::fs::create_dir_all(retired.parent().unwrap()).unwrap();
        std::fs::write(&retired, "retired").unwrap();
        let out = generate(&inventory);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!retired.exists());
        // A renderer compiled from merged sources may ship a skill this host
        // binary does not know; its staged master authorizes the pack file.
        let merged_skill = ".agents/skills/aida-fixture/SKILL.md";
        std::fs::write(
            fx.main.join("Makefile"),
            format!("{makefile}\tmkdir -p .agents/skills/aida-fixture\n\techo merged > {merged_skill}\n"),
        )
        .unwrap();
        let out = generate(&inventory);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(fx.main.join(merged_skill).is_file());
        // The closed inventory still catches an extra write under the pack,
        // including one whose would-be master is not a staged source, and the
        // refused run leaves none of its writes behind.
        std::fs::write(
            fx.main.join("aida-core/templates/skills/aida-unstaged.md"),
            "unstaged",
        )
        .unwrap();
        std::fs::write(
            fx.main.join("Makefile"),
            format!(
                "{makefile}\techo unexpected > .agents/skills/aida-invented.md\n\
                 \tmkdir -p .agents/skills/aida-unstaged\n\
                 \techo unexpected > .agents/skills/aida-unstaged/SKILL.md\n"
            ),
        )
        .unwrap();
        let out = generate(&inventory);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("unregistered outputs"), "{stderr}");
        assert!(stderr.contains("aida-unstaged/SKILL.md"), "{stderr}");
        assert!(stderr.contains("rolled back"), "{stderr}");
        assert!(!fx.main.join(".agents/skills/aida-invented.md").exists());
        assert!(!fx
            .main
            .join(".agents/skills/aida-unstaged/SKILL.md")
            .exists());
    }

    // trace:STORY-2710 | ai:codex
    #[test]
    fn story_2710_merge_tree_requires_structured_conflict_exit() {
        let args = [
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            "HEAD",
            "base",
        ];
        let output = format!("{}\0path with\nnewline\0\0details\0", "a".repeat(40));
        assert!(repair_merge_tree_conflict(
            &args,
            Some(1),
            output.as_bytes()
        ));
        assert_eq!(
            repair_merge_tree_paths(&output).unwrap(),
            ["path with\nnewline"]
        );
        for code in [None, Some(0), Some(128), Some(129)] {
            assert!(!repair_merge_tree_conflict(&args, code, output.as_bytes()));
        }
        for output in [
            "",
            "unknown option",
            "a tree without paths",
            &format!("{}\0unterminated-path", "a".repeat(40)),
            &format!("{}\0unterminated-list\0", "a".repeat(40)),
            &format!("{}\0", "a".repeat(40)),
        ] {
            assert!(!repair_merge_tree_conflict(
                &args,
                Some(1),
                output.as_bytes()
            ));
        }
        assert_eq!(
            repair_merge_tree_paths(&format!("{}\0", "a".repeat(40))).unwrap(),
            Vec::<String>::new()
        );
    }

    // trace:STORY-2710 | ai:codex
    #[test]
    fn story_2710_clean_merge_probe_does_not_reserve() {
        let fx = tests::fixture();
        tests::git_ok(&fx.main, &["checkout", "-qb", "repair"]);
        let head = tests::git_ok(&fx.main, &["rev-parse", "HEAD"]);
        let execute = |args: &[&str]| -> Result<String> {
            let out = crate::test_git::git_command()
                .current_dir(&fx.main)
                .args(args)
                .output()?;
            anyhow::ensure!(out.status.success(), "Git refused");
            let text = String::from_utf8(out.stdout)?;
            Ok(if args.contains(&"-z") {
                text
            } else {
                text.trim().to_owned()
            })
        };
        // Failed observations never reserve this head, and a later successful
        // probe can retry without requiring a dead-lane recovery operation.
        // trace:STORY-2710 | ai:codex
        for reason in ["probe timed out", "I/O failure", "unsupported option"] {
            let failed_probe = |args: &[&str]| -> Result<String> {
                if args.first() == Some(&"merge-tree") {
                    anyhow::bail!("{reason}");
                }
                execute(args)
            };
            let error = prepare_host_repair_with(
                &fx.main,
                &fx.main,
                3813,
                "lease-1",
                &failed_probe,
                "repair",
                false,
                |_| panic!("failed probe must not publish"),
                |_, _| panic!("failed probe must not regenerate"),
                || Ok((head.clone(), head.clone())),
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(reason));
            assert!(!fx.main.join(".aida/repair-journal.json").exists());
        }
        let paths = prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "lease-1",
            &execute,
            "repair",
            false,
            |_| panic!("unchanged tip must not publish"),
            |_, _| panic!("clean merge must not regenerate"),
            || Ok((head.clone(), head.clone())),
        )
        .unwrap();
        assert!(paths.is_empty());
        assert!(!fx.main.join(".aida/repair-journal.json").exists());
    }

    // A real staged merge, with the index left untouched while the lane edits
    // files. This exercises the marker gate without requiring bubblewrap.
    // trace:STORY-2710 | ai:codex
    #[test]
    fn story_2710_marker_gate_refuses_unmerged_and_staged_resolutions() {
        let fx = tests::fixture();
        let g = |args: &[&str]| tests::git_ok(&fx.main, args);
        let path = "conflict [literal]\n$(touch sentinel).rs";
        // Existing documentation examples remain legitimate ordinary edits.
        let guide = fx.main.join("conflict-guide.md");
        std::fs::write(&guide, "<<<<<<< example\n>>>>>>> example\n").unwrap();
        g(&["add", "conflict-guide.md"]);
        g(&["commit", "-qm", "marker examples"]);
        std::fs::write(&guide, "<<<<<<< example\n>>>>>>> example\nexplanation\n").unwrap();
        let ordinary_probe = |program: &str, args: &[&std::ffi::OsStr]| -> Result<Vec<u8>> {
            let out = std::process::Command::new(program)
                .current_dir(&fx.main)
                .args(args)
                .output()?;
            anyhow::ensure!(out.status.success(), "probe failed: {:?}", out);
            Ok(out.stdout)
        };
        g(&["branch", "MERGE_HEAD"]);
        refuse_pending_conflict_markers(&ordinary_probe, None).unwrap();
        g(&["add", "conflict-guide.md"]);
        refuse_pending_conflict_markers(&ordinary_probe, None).unwrap();
        g(&["branch", "-D", "MERGE_HEAD"]);
        g(&["commit", "-qm", "explain markers"]);
        // Even absent a merge, an unreadable probe must fail closed.
        assert!(
            refuse_pending_conflict_markers(&|_, _| anyhow::bail!("probe failed"), None).is_err()
        );
        std::fs::write(fx.main.join(path), "base\n").unwrap();
        g(&["add", "--", path]);
        g(&["commit", "-qm", "base"]);
        g(&["checkout", "-qb", "repair"]);
        std::fs::write(fx.main.join(path), "lane\n").unwrap();
        g(&["commit", "-qam", "lane"]);
        let head = g(&["rev-parse", "HEAD"]);
        g(&["checkout", "main"]);
        std::fs::write(fx.main.join(path), "main\n").unwrap();
        g(&["commit", "-qam", "main"]);
        g(&["checkout", "repair"]);
        let base = g(&["rev-parse", "main"]);
        let git_execute = |args: &[&str]| -> Result<String> {
            let out = crate::test_git::git_command()
                .current_dir(&fx.main)
                .args(args)
                .output()?;
            anyhow::ensure!(
                out.status.success()
                    || repair_merge_tree_conflict(args, out.status.code(), &out.stdout),
                "Git refused"
            );
            let text = String::from_utf8(out.stdout)?;
            Ok(if args.contains(&"-z") {
                text
            } else {
                text.trim().to_owned()
            })
        };
        // Failed observations happen before reservation or mutation. Recovery
        // on the same head must still reach the real conflict below.
        // trace:STORY-2710 | ai:codex
        let index_before = g(&["ls-files", "--stage"]);
        for failure in [
            "probe timed out",
            "probe I/O failure",
            "unknown option: write-tree",
            "fatal: unable to read tree",
            "could not spawn git",
        ] {
            let failed_probe = |args: &[&str]| -> Result<String> {
                if args.first() == Some(&"merge-tree") {
                    anyhow::bail!("{failure}");
                }
                git_execute(args)
            };
            let why = prepare_host_repair_with(
                &fx.main,
                &fx.main,
                3813,
                "lease-1",
                &failed_probe,
                "repair",
                false,
                |_| panic!("failed probe cannot publish"),
                |_, _| panic!("failed probe cannot regenerate"),
                || Ok((base.clone(), head.clone())),
            )
            .unwrap_err();
            assert!(format!("{why:#}").contains(failure));
            assert!(!fx.main.join(".aida/repair-journal.json").exists());
            assert_eq!(g(&["rev-parse", "HEAD"]), head);
            assert_eq!(g(&["ls-files", "--stage"]), index_before);
            assert!(git_execute(&["rev-parse", "--verify", "MERGE_HEAD"]).is_err());
        }
        let files = prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "lease-1",
            &git_execute,
            "repair",
            false,
            |_| panic!("conflicts must not publish"),
            |_, _| anyhow::bail!("generator timeout after partial write"),
            || Ok((base.clone(), head.clone())),
        );
        assert!(files.is_err());
        // Preparation failure retains the merge, but neither resume nor direct
        // publication may infer success from the retained original path list.
        let journal = crate::repair_journal::RepairGuard::lock(&fx.main).unwrap();
        assert_eq!(
            journal
                .for_tree(&fx.main, "repair", &head, &base)
                .unwrap()
                .preparation_success,
            Some(false)
        );
        drop(journal);
        let resumed = prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "lease-1",
            &git_execute,
            "repair",
            true,
            |_| panic!("failed preparation cannot publish"),
            |_, _| anyhow::bail!("generator still fails"),
            || Ok((base.clone(), head.clone())),
        );
        assert!(resumed.is_err());
        // This is the same transfer the verified session-start orphan adoption
        // performs after writing its freshly minted successor lease.
        crate::repair_journal::RepairGuard::lock(&fx.main)
            .unwrap()
            .transfer_adopted_lane(&fx.main, "repair", "lease-1", "successor-lease")
            .unwrap();
        assert!(prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "unrelated-lease",
            &git_execute,
            "repair",
            true,
            |_| panic!("unrelated lane cannot publish"),
            |_, _| panic!("unrelated lane cannot regenerate"),
            || Ok((base.clone(), head.clone())),
        )
        .is_err());
        let files = prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "successor-lease",
            &git_execute,
            "repair",
            true,
            |_| panic!("conflicts cannot publish"),
            |_, restore_only| {
                assert!(restore_only);
                Ok(())
            },
            || Ok((base.clone(), head.clone())),
        )
        .unwrap();
        assert_eq!(files, [path]);
        let execute = |program: &str, args: &[&std::ffi::OsStr]| -> Result<Vec<u8>> {
            let mut command = if program == "git" {
                crate::test_git::git_command()
            } else {
                std::process::Command::new(program)
            };
            let out = command.current_dir(&fx.main).args(args).output()?;
            anyhow::ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(out.stdout)
        };
        let journal = crate::repair_journal::RepairGuard::lock(&fx.main).unwrap();
        let original_paths: Vec<std::ffi::OsString> = journal
            .for_tree(&fx.main, "repair", &head, &base)
            .unwrap()
            .original_conflict_paths
            .iter()
            .map(Into::into)
            .collect();
        drop(journal);
        let index = g(&["ls-files", "--stage"]);
        let merge_head = g(&["rev-parse", "MERGE_HEAD"]);
        for contents in [
            "<<<<<<< HEAD\nlane\n=======\nmain\n>>>>>>> main\n",
            "resolved except opening\n<<<<<<< HEAD\n",
            "resolved except closing\n>>>>>>> main\n",
        ] {
            std::fs::write(fx.main.join(path), contents).unwrap();
            let why = refuse_pending_conflict_markers(&execute, Some(&original_paths))
                .unwrap_err()
                .to_string();
            assert!(why.contains("refusing to publish"), "{why}");
            assert!(why.contains("conflict [literal]"), "{why}");
            assert_eq!(g(&["ls-files", "--stage"]), index);
            assert_eq!(g(&["rev-parse", "HEAD"]), head);
            assert_eq!(g(&["rev-parse", "MERGE_HEAD"]), merge_head);
        }
        // A failed prior host commit may have staged the resolution: scan it
        // on retry as well, rather than trusting absence of unmerged entries.
        g(&["add", "--", path]);
        assert!(g(&["ls-files", "--unmerged"]).is_empty());
        let resumed = prepare_host_repair_with(
            &fx.main,
            &fx.main,
            3813,
            "successor-lease",
            &git_execute,
            "repair",
            true,
            |_| panic!("retry must not publish"),
            |_, _| panic!("retry must not overwrite evidence"),
            || Ok((base.clone(), head.clone())),
        )
        .unwrap();
        assert_eq!(resumed, [path]);
        assert!(refuse_pending_conflict_markers(&execute, Some(&original_paths)).is_err());
        // A failed staging attempt may have removed the path from the index.
        // Recreated/untracked working-tree bytes still belong to the original
        // host conflict list and must not bypass the gate.
        g(&["rm", "--cached", "-f", "--", path]);
        assert!(refuse_pending_conflict_markers(&execute, Some(&original_paths)).is_err());
        g(&["add", "--", path]);
        // Unrelated changed files may contain marker examples even during a
        // repair. They were never in the host's original conflict list.
        std::fs::write(
            &guide,
            "<<<<<<< example\n>>>>>>> example\nmore explanation\n",
        )
        .unwrap();
        std::fs::write(fx.main.join(path), "lane and main\n").unwrap();
        refuse_pending_conflict_markers(&execute, Some(&original_paths)).unwrap();
        g(&["add", "--", path, "conflict-guide.md"]);
        refuse_pending_conflict_markers(&execute, Some(&original_paths)).unwrap();
        // Missing evidence must not fall back to a changed-path/index scan.
        let why = refuse_pending_conflict_markers(&execute, None)
            .unwrap_err()
            .to_string();
        assert!(
            why.contains("host-recorded original conflict paths"),
            "{why}"
        );
        assert!(refuse_pending_conflict_markers(&execute, Some(&[])).is_err());
        for invalid in ["", "../outside", "/absolute", "a/../../outside"] {
            assert!(refuse_pending_conflict_markers(&execute, Some(&[invalid.into()])).is_err());
        }
        assert!(!fx.main.join("sentinel").exists());
        // A deletion is also a valid resolution; no file must be opened.
        std::fs::remove_file(fx.main.join(path)).unwrap();
        refuse_pending_conflict_markers(&execute, Some(&original_paths)).unwrap();
        assert!(refuse_pending_conflict_markers(
            &|_, _| anyhow::bail!("read failed"),
            Some(&original_paths)
        )
        .is_err());
    }

    // trace:BUG-2456 | ai:codex
    #[test]
    fn bug_2456_publication_failure_refuses_launch_and_current_head_skips_push() {
        for current in [false, true] {
            let merged = std::cell::Cell::new(false);
            let execute = |args: &[&str]| -> Result<String> {
                match args {
                    ["symbolic-ref", "--short", "HEAD"] => Ok("rework".into()),
                    ["rev-parse", "-q", "--verify", "MERGE_HEAD"] => anyhow::bail!("absent"),
                    ["rev-parse", "HEAD"] => Ok(if current || merged.get() {
                        "prepared"
                    } else {
                        "old"
                    }
                    .into()),
                    ["show", "-s", "--format=%P", "prepared"] => Ok("old base".into()),
                    ["merge", "--no-edit", "--no-verify", "base"] => {
                        merged.set(true);
                        Ok(String::new())
                    }
                    _ => Ok(String::new()),
                }
            };
            let result = prepare_rework_merge_with(
                &execute,
                "rework",
                false,
                |tip| {
                    assert!(!current, "an already published head needs no push");
                    assert_eq!(tip, "prepared");
                    anyhow::bail!("push rejected")
                },
                || {
                    Ok((
                        "base".into(),
                        if current { "prepared" } else { "old" }.into(),
                    ))
                },
            );
            if current {
                assert!(result.unwrap().is_empty());
            } else {
                assert_eq!(result.unwrap_err().to_string(), "push rejected");
            }
        }
    }

    // trace:BUG-2456 | ai:codex
    #[test]
    fn bug_2456_only_exact_host_merge_parents_publish() {
        for parents in ["old base", "base old", "old", "old other", "old base third"] {
            let merged = std::cell::Cell::new(false);
            let published = std::cell::Cell::new(false);
            let execute = |args: &[&str]| -> Result<String> {
                match args {
                    ["symbolic-ref", "--short", "HEAD"] => Ok("rework".into()),
                    ["rev-parse", "-q", "--verify", "MERGE_HEAD"] => anyhow::bail!("absent"),
                    ["rev-parse", "HEAD"] => {
                        Ok(if merged.get() { "prepared" } else { "old" }.into())
                    }
                    ["show", "-s", "--format=%P", "prepared"] => Ok(parents.into()),
                    ["merge", "--no-edit", "--no-verify", "base"] => {
                        merged.set(true);
                        Ok(String::new())
                    }
                    _ => Ok(String::new()),
                }
            };
            prepare_rework_merge_with(
                &execute,
                "rework",
                false,
                |_| {
                    published.set(true);
                    Ok(())
                },
                || Ok(("base".into(), "old".into())),
            )
            .unwrap();
            assert_eq!(published.get(), parents == "old base", "{parents}");
        }
    }

    // trace:BUG-2456 | ai:codex
    #[test]
    fn bug_2456_local_commits_never_publish_before_resume() {
        for base_ahead in [false, true] {
            let fx = tests::fixture();
            let g = |args: &[&str]| tests::git_ok(&fx.main, args);
            let pr_head = g(&["rev-parse", "HEAD"]);
            g(&["checkout", "-qb", "rework"]);
            g(&["commit", "--allow-empty", "-qm", "unpublished lane work"]);
            let local = g(&["rev-parse", "HEAD"]);
            let base = if base_ahead {
                g(&["checkout", "main"]);
                g(&["commit", "--allow-empty", "-qm", "base advances"]);
                let base = g(&["rev-parse", "HEAD"]);
                g(&["checkout", "rework"]);
                base
            } else {
                pr_head.clone()
            };
            let execute = |args: &[&str]| -> Result<String> {
                if args == ["rev-parse", "-q", "--verify", "MERGE_HEAD"] {
                    anyhow::bail!("absent");
                }
                Ok(g(args))
            };
            assert!(prepare_rework_merge_with(
                &execute,
                "rework",
                true,
                |_| panic!("local lane commits must retain post-exit publication gates"),
                || Ok((base.clone(), pr_head.clone())),
            )
            .unwrap()
            .is_empty());
            if base_ahead {
                assert_eq!(g(&["rev-parse", "HEAD^1"]), local);
            } else {
                assert_eq!(g(&["rev-parse", "HEAD"]), local);
            }
        }
    }

    // trace:BUG-2618 | ai:codex
    #[test]
    fn bug2618_rework_publication_refuses_unreadable_policy_before_push() {
        let fx = tests::fixture();
        let tip = tests::git_ok(&fx.main, &["rev-parse", "HEAD"]);
        let config = crate::config_path_for_project(&fx.main);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "[pr-head]\nprotect = 'invalid'\n").unwrap();
        // Own the branch so the BUG-2633 ownership check passes and the
        // refusal under test is the policy read, not a missing lease.
        record_test_publication_lease(&fx.main, &fx.main, "protected");
        let err = publish_rework_merge(&fx.main, &fx.main, "protected", &tip).unwrap_err();
        assert!(
            format!("{err:#}").contains("pr-head.protect must be a boolean"),
            "{err:#}"
        );
        let remote = tests::git_ok(&fx.main, &["ls-remote", "origin", "refs/heads/protected"]);
        assert!(remote.is_empty(), "guard refusal published {remote}");
    }

    // No bwrap dependency: move the real remote after fetch, then exercise
    // the production push and prove refusal stops the launch continuation.
    // trace:BUG-2456 | ai:codex
    #[test]
    fn bug_2456_real_remote_race_refuses_before_vendor_launch() {
        let fx = tests::fixture();
        let g = |args: &[&str]| tests::git_ok(&fx.main, args);
        g(&["checkout", "-qb", "rework"]);
        g(&["commit", "--allow-empty", "-qm", "PR head"]);
        let pr_head = g(&["rev-parse", "HEAD"]);
        g(&["push", "-q", "origin", "rework"]);
        g(&["checkout", "main"]);
        g(&["commit", "--allow-empty", "-qm", "main advances"]);
        let base = g(&["rev-parse", "HEAD"]);
        g(&["push", "-q", "origin", "main"]);
        g(&["checkout", "-qb", "racer", &pr_head]);
        g(&["commit", "--allow-empty", "-qm", "concurrent PR work"]);
        let raced = g(&["rev-parse", "HEAD"]);
        g(&["checkout", "rework"]);
        record_test_publication_lease(&fx.main, &fx.main, "rework");
        // This fixture uses main itself as the seat. Host coordination files
        // must not turn the otherwise clean source checkout into pending work.
        std::fs::write(fx.main.join(".git/info/exclude"), ".aida/sessions/\n").unwrap();
        let execute = |args: &[&str]| -> Result<String> {
            if args == ["rev-parse", "-q", "--verify", "MERGE_HEAD"] {
                anyhow::bail!("absent");
            }
            // This happens after fetch but before publication. A real push
            // advances origin while preparation retains its fetched PR SHA.
            if args.first() == Some(&"merge") {
                g(&["push", "-q", "origin", "racer:refs/heads/rework"]);
            }
            Ok(g(args))
        };
        let launched = std::cell::Cell::new(false);
        let result = (|| -> Result<()> {
            prepare_rework_merge_with(
                &execute,
                "rework",
                false,
                |tip| publish_rework_merge(&fx.main, &fx.main, "rework", tip),
                || {
                    g(&["fetch", "-q", "origin"]);
                    assert_eq!(g(&["rev-parse", "origin/rework"]), pr_head);
                    Ok((base.clone(), pr_head.clone()))
                },
            )?;
            launched.set(true);
            Ok(())
        })();
        let why = result.unwrap_err().to_string();
        assert!(
            why.contains("could not publish prepared rework merge"),
            "{why}"
        );
        assert!(why.contains("rejected"), "{why}");
        assert!(!launched.get());
        assert_eq!(
            tests::git_ok(&fx.origin, &["rev-parse", "refs/heads/rework"]),
            raced
        );
    }

    // Exercise admission against real Git state; the fetch seam pins heads
    // without a network dependency and must never run for local refusals.
    // trace:STORY-2475 | ai:codex
    #[test]
    fn story_2475_prepare_refuses_wrong_dirty_merging_and_behind_seats() {
        let fx = tests::fixture();
        let execute = |args: &[&str]| -> Result<String> {
            let out = crate::test_git::git_command()
                .current_dir(&fx.main)
                .args(args)
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let no_fetch =
            || -> Result<(String, String)> { panic!("local refusal must precede fetch") };
        let why = prepare_rework_merge_with(
            &execute,
            "wrong",
            false,
            |_| panic!("must not publish"),
            no_fetch,
        )
        .unwrap_err();
        assert!(why.to_string().contains("PR head branch"));
        std::fs::write(fx.main.join("pending"), "edits").unwrap();
        let why = prepare_rework_merge_with(
            &execute,
            "main",
            false,
            |_| panic!("must not publish"),
            no_fetch,
        )
        .unwrap_err();
        assert!(why.to_string().contains("pending changes"));
        // A dirty resume still checks fresh remote heads before admission.
        // trace:BUG-2504 | ai:codex
        let current = execute(&["rev-parse", "HEAD"]).unwrap();
        assert!(prepare_rework_merge_with(
            &execute,
            "main",
            true,
            |_| panic!("must not publish"),
            || Ok((current.clone(), current.clone())),
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            std::fs::read_to_string(fx.main.join("pending")).unwrap(),
            "edits"
        );
        let before = execute(&["rev-parse", "HEAD"]).unwrap();
        std::fs::write(fx.main.join(".git/MERGE_HEAD"), format!("{before}\n")).unwrap();
        for resume in [false, true] {
            let why = prepare_rework_merge_with(
                &execute,
                "main",
                resume,
                |_| panic!("must not publish"),
                no_fetch,
            )
            .unwrap_err();
            assert!(why.to_string().contains("unfinished merge"));
        }
        std::fs::remove_file(fx.main.join(".git/MERGE_HEAD")).unwrap();
        std::fs::remove_file(fx.main.join("pending")).unwrap();
        execute(&["checkout", "-b", "pr"]).unwrap();
        execute(&["commit", "--allow-empty", "-m", "new PR head"]).unwrap();
        let ahead = execute(&["rev-parse", "HEAD"]).unwrap();
        execute(&["checkout", "main"]).unwrap();
        let why = prepare_rework_merge_with(
            &execute,
            "main",
            false,
            |_| panic!("must not publish"),
            || Ok((before.clone(), ahead)),
        )
        .unwrap_err();
        assert!(why.to_string().contains("behind the PR head"));
        assert!(!why.to_string().contains("commit-pending"));
        assert!(!why.to_string().contains("--resume"));
        assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), before);
        let why = prepare_rework_merge_with(
            &execute,
            "main",
            false,
            |_| panic!("must not publish"),
            || anyhow::bail!("fetch refused"),
        )
        .unwrap_err();
        assert_eq!(why.to_string(), "fetch refused");
        assert!(!fx.main.join(".git/MERGE_HEAD").exists());
    }

    // trace:BUG-2504 | ai:codex
    #[test]
    fn bug_2504_dirty_resume_checks_fetched_heads_without_touching_edits() {
        let fx = tests::fixture();
        let execute = |args: &[&str]| -> Result<String> {
            let out = crate::test_git::git_command()
                .current_dir(&fx.main)
                .args(args)
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let head = execute(&["rev-parse", "HEAD"]).unwrap();
        execute(&["checkout", "-b", "new-main"]).unwrap();
        std::fs::write(fx.main.join("main-update"), "main\n").unwrap();
        execute(&["add", "."]).unwrap();
        execute(&["commit", "-m", "main moved"]).unwrap();
        let base = execute(&["rev-parse", "HEAD"]).unwrap();
        execute(&["checkout", "main"]).unwrap();
        std::fs::write(fx.main.join("pending"), "keep these edits\n").unwrap();
        let status = execute(&["status", "--porcelain"]).unwrap();
        prepare_rework_merge_with(
            &execute,
            "main",
            true,
            |_| panic!("must not publish"),
            || Ok((base.clone(), head.clone())),
        )
        .expect("non-conflicting main advance admits pending work");
        {
            let why = prepare_rework_merge_with(
                &execute,
                "main",
                true,
                |_| panic!("must not publish"),
                || Ok((head.clone(), base.clone())),
            )
            .unwrap_err();
            assert!(format!("{why:#}").contains("behind the PR head"), "{why:#}");
            assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), head);
            assert_eq!(execute(&["status", "--porcelain"]).unwrap(), status);
            assert_eq!(
                std::fs::read_to_string(fx.main.join("pending")).unwrap(),
                "keep these edits\n"
            );
            assert!(execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err());
        }
        let why = prepare_rework_merge_with(
            &execute,
            "main",
            true,
            |_| panic!("must not publish"),
            || anyhow::bail!("fetch unavailable"),
        )
        .unwrap_err();
        assert!(why.to_string().contains("fetch unavailable"));
        std::fs::write(fx.main.join("main-update"), "PR change\n").unwrap();
        execute(&["add", "main-update"]).unwrap();
        execute(&["commit", "-m", "conflicting PR change"]).unwrap();
        let pr_head = execute(&["rev-parse", "HEAD"]).unwrap();
        let status = execute(&["status", "--porcelain"]).unwrap();
        let why = prepare_rework_merge_with(
            &execute,
            "main",
            true,
            |_| panic!("must not publish"),
            || Ok((base, pr_head.clone())),
        )
        .unwrap_err();
        let guidance = format!("{why:#}");
        assert!(guidance.contains("edits preserved"));
        assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), pr_head);
        assert_eq!(execute(&["status", "--porcelain"]).unwrap(), status);
        assert_eq!(
            std::fs::read_to_string(fx.main.join("pending")).unwrap(),
            "keep these edits\n"
        );
        assert!(execute(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err());
    }

    // trace:STORY-2475 | ai:codex
    #[test]
    fn story_2475_merge_and_abort_failures_refuse() {
        for conflicts in [false, true] {
            let execute = |args: &[&str]| -> Result<String> {
                match args[0] {
                    "merge" if args[1] == "--abort" => anyhow::bail!("abort failed"),
                    "merge" => anyhow::bail!("merge failed"),
                    "diff" => Ok(if conflicts {
                        "conflict\0".into()
                    } else {
                        String::new()
                    }),
                    "rev-parse" => anyhow::bail!("no merge in progress"),
                    _ => panic!("unexpected command {args:?}"),
                }
            };
            let why = finish_rework_merge(&execute, "base")
                .unwrap_err()
                .to_string();
            assert!(
                why.contains(if conflicts {
                    "could not abort"
                } else {
                    "without conflict paths"
                }),
                "{why}"
            );
        }
    }

    #[test]
    fn story_2475_real_merge_commits_clean_or_restores_conflicting_branch() {
        for conflicting in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let execute = |args: &[&str]| -> Result<String> {
                let out = crate::test_git::git_command()
                    .arg("-C")
                    .arg(root.path())
                    .args([
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.test",
                        "-c",
                        "core.hooksPath=/dev/null",
                        "-c",
                        "commit.gpgsign=false",
                    ])
                    .args(args)
                    .output()?;
                anyhow::ensure!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            };
            execute(&["init", "-b", "main"]).unwrap();
            std::fs::write(root.path().join("shared"), "base\n").unwrap();
            execute(&["add", "."]).unwrap();
            execute(&["commit", "-m", "base"]).unwrap();
            execute(&["checkout", "-b", "rework"]).unwrap();
            std::fs::write(root.path().join("shared"), "rework\n").unwrap();
            execute(&["commit", "-am", "rework"]).unwrap();
            let before = execute(&["rev-parse", "HEAD"]).unwrap();
            execute(&["checkout", "main"]).unwrap();
            std::fs::write(
                root.path()
                    .join(if conflicting { "shared" } else { "other" }),
                "main\n",
            )
            .unwrap();
            execute(&["add", "."]).unwrap();
            execute(&["commit", "-m", "main update"]).unwrap();
            let base = execute(&["rev-parse", "HEAD"]).unwrap();
            execute(&["checkout", "rework"]).unwrap();
            let files = finish_rework_merge(&execute, &base).unwrap();
            assert_eq!(execute(&["status", "--porcelain"]).unwrap(), "");
            assert!(execute(&["rev-parse", "--verify", "MERGE_HEAD"]).is_err());
            if conflicting {
                assert_eq!(files, ["shared"]);
                assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), before);
                assert_eq!(
                    std::fs::read_to_string(root.path().join("shared")).unwrap(),
                    "rework\n"
                );
            } else {
                assert!(files.is_empty());
                assert_eq!(execute(&["rev-parse", "HEAD^1"]).unwrap(), before);
                assert_eq!(execute(&["rev-parse", "HEAD^2"]).unwrap(), base);
            }
        }
    }

    // trace:TASK-2879 | ai:codex
    #[test]
    fn task_2879_publication_merges_remote_and_aborts_conflicts() {
        for conflicting in [false, true] {
            let fx = tests::fixture();
            let execute = |args: &[&str]| {
                let mut cmd = crate::test_git::git_command();
                cmd.current_dir(&fx.main).args(args);
                run(cmd, "publication merge test")
            };
            execute(&["checkout", "-b", "pr"]).unwrap();
            std::fs::write(fx.main.join("README.md"), "lane edit\n").unwrap();
            execute(&["commit", "-am", "lane edits (Refs TASK-2879)"]).unwrap();
            let lane_head = execute(&["rev-parse", "HEAD"]).unwrap();
            execute(&["checkout", "main"]).unwrap();
            std::fs::write(
                fx.main.join(if conflicting {
                    "README.md"
                } else {
                    "remote.txt"
                }),
                "remote edit\n",
            )
            .unwrap();
            execute(&["add", "."]).unwrap();
            execute(&["commit", "-m", "remote update"]).unwrap();
            let remote = execute(&["rev-parse", "HEAD"]).unwrap();
            execute(&["checkout", "pr"]).unwrap();
            let message = "merge remote PR head (Refs TASK-2879)";
            let result = merge_publication_head_with(&execute, "pr", &remote, message);
            assert_eq!(execute(&["status", "--porcelain"]).unwrap(), "");
            assert!(!fx.main.join(".git/MERGE_HEAD").exists());
            if conflicting {
                assert!(result.unwrap_err().to_string().contains("nothing pushed"));
                assert_eq!(execute(&["rev-parse", "HEAD"]).unwrap(), lane_head);
                assert_eq!(
                    std::fs::read_to_string(fx.main.join("README.md")).unwrap(),
                    "lane edit\n"
                );
            } else {
                result.unwrap();
                assert_eq!(execute(&["rev-parse", "HEAD^1"]).unwrap(), lane_head);
                assert_eq!(execute(&["rev-parse", "HEAD^2"]).unwrap(), remote);
                assert_eq!(execute(&["log", "-1", "--format=%s"]).unwrap(), message);
                // An already integrated remote head is a harmless no-op.
                merge_publication_head_with(&execute, "pr", &remote, message).unwrap();
            }
            assert!(merge_publication_head_with(&execute, "wrong", &remote, message).is_err());
            std::fs::write(fx.main.join("pending"), "keep me").unwrap();
            assert!(
                merge_publication_head_with(&execute, "pr", &remote, message)
                    .unwrap_err()
                    .to_string()
                    .contains("pending changes")
            );
        }
    }

    #[test]
    fn story_2475_conflicts_abort_before_return_and_failures_refuse() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let files = finish_rework_merge(
            &|args| {
                calls.borrow_mut().push(args.join(" "));
                match args[0] {
                    "merge" if args[1] != "--abort" => anyhow::bail!("conflict"),
                    "diff" => Ok("a.rs\0b.rs\0".into()),
                    _ => Ok("head".into()),
                }
            },
            "base",
        )
        .unwrap();
        assert_eq!(files, ["a.rs", "b.rs"]);
        assert_eq!(calls.borrow().last().unwrap(), "merge --abort");
        assert!(finish_rework_merge(&|_| anyhow::bail!("unavailable"), "base").is_err());
        assert!(finish_rework_merge(
            &|args| if args == ["merge", "--abort"] {
                anyhow::bail!("abort failed")
            } else if args[0] == "merge" {
                anyhow::bail!("conflict")
            } else {
                Ok("path\0".into())
            },
            "base"
        )
        .is_err());
    }
}

/// TASK-2712 (the advisor's N3 fallback): commit the seat's pending work for
/// a native Codex seat, which cannot commit inside its own sandbox (Codex
/// re-protects `<clone>/.git`). Every git run is AIDA's own bwrap job: the
/// checks in the read-only probe sandbox, `git add -A` and `git commit` in
/// the commit sandbox ([`commit_sandbox_args`]: the clone writable, main's
/// `.git` read-only, the clone's config replaced, hooks and fsmonitor off, no
/// network). The clone's HEAD must be the lease's `branch`. The identity is
/// the host's own `user.name`/`user.email` from MAIN's config, never the
/// seat's, read before any sandbox runs. `Ok(None)` when there is nothing to
/// commit; `Ok(Some(tip))` with the new commit otherwise. The clone root is
/// pinned (device and inode) at the start and re-checked before staging and
/// before committing, so a path replaced mid-run is refused. On a failure
/// the working tree is untouched, and a failure after staging restores the
/// index to the exact tree it held before (`git write-tree` snapshot, then
/// `git read-tree`), so nothing is left staged, but only while HEAD has not
/// moved ([`index_restore_allowed`]).
// trace:TASK-2712 | ai:claude
pub(crate) fn commit_pending(
    main: &Path,
    clone: &Path,
    branch: &str,
    message: &str,
) -> Result<Option<String>> {
    commit_pending_with(
        main,
        clone,
        branch,
        message,
        &crate::lane_job::check_repair_tree,
    )
}

/// Compile a snapshot of the staged repair tree: `(main, seat, lease,
/// snapshot)` -> the policy command that ran and its outcome.
// trace:TASK-3804 | ai:claude
pub(crate) type RepairCompile<'a> =
    &'a dyn Fn(&Path, &Path, &str, &Path) -> (Vec<String>, Result<()>);

// trace:TASK-3804 | ai:codex
fn commit_pending_with(
    main: &Path,
    clone: &Path,
    branch: &str,
    message: &str,
    compile: RepairCompile<'_>,
) -> Result<Option<String>> {
    anyhow::ensure!(
        !message.trim().is_empty(),
        "refusing to commit the seat's pending work with an empty message"
    );
    let pinned = dir_identity(clone)?;
    let read_identity = |key: &str| -> Result<String> {
        let mut cmd = git(main);
        cmd.args(["config", "--get", key]);
        run(cmd, &format!("read the host's {key}"))
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .with_context(|| {
                format!(
                    "refusing to commit the seat's pending work: the main checkout has no \
                     `{key}`. Set it with `git config {key} …` in {}; nothing was committed",
                    main.display()
                )
            })
    };
    let (name, email) = (read_identity("user.name")?, read_identity("user.email")?);
    let os = |s: &'static str| -> &'static std::ffi::OsStr { s.as_ref() };
    let probe_text = |args: &[&std::ffi::OsStr]| {
        run_sandboxed(clone, main, "git", args, PROBE_OUTPUT_CAP)
            .map(|out| String::from_utf8_lossy(&out).trim().to_string())
            .map_err(|failure| {
                anyhow::anyhow!(
                    "could not read the seat {} inside the seat sandbox: {failure}",
                    clone.display()
                )
            })
    };
    let head = probe_text(&[os("symbolic-ref"), os("-q"), os("HEAD")])?;
    let want = format!("refs/heads/{branch}");
    anyhow::ensure!(
        head == want,
        "refusing to commit the seat's pending work: {} is on `{}`, not its lease's branch \
         `{branch}`. Nothing was committed; the working tree is untouched",
        clone.display(),
        head.escape_debug()
    );
    // The host's own seat marker is never seat work, ignored or not.
    let exclude_marker = format!(
        ":(exclude){}",
        aida_core::seat_clone_detect::MARKER_RELATIVE
    );
    let pathspec: [&std::ffi::OsStr; 3] = [os("--"), os("."), exclude_marker.as_ref()];
    let mut status_args = vec![
        os("status"),
        os("--porcelain=v1"),
        os("--untracked-files=all"),
    ];
    status_args.extend(pathspec);
    let status = probe_text(&status_args)?;
    // Resolve the pseudoref only after proving the file exists. A corrupt
    // pseudoref or a failed Git/sandbox probe must stop even a clean seat.
    // trace:BUG-2558 TASK-3804 | ai:codex
    let inspect_merge = || {
        pending_merge_parent(&|program, args| {
            run_sandboxed_until(
                clone,
                main,
                program,
                args,
                PROBE_OUTPUT_CAP,
                Some(std::time::Instant::now() + std::time::Duration::from_secs(5)),
            )
            .map_err(|why| anyhow::anyhow!("{why}"))
        })
        .context("could not inspect seat merge state")
    };
    let merge_base = inspect_merge()?;
    if status.is_empty() && merge_base.is_none() {
        return Ok(None);
    }
    let repair = if let Some(base) = merge_base.as_deref() {
        let original_head = probe_text(&[os("rev-parse"), os("HEAD")])?;
        let journal = crate::repair_journal::RepairGuard::lock(main)?;
        let entry = journal.for_tree(clone, branch, &original_head, base)?;
        if let Err(error) = entry.require_prepared() {
            let key = entry.key.clone();
            drop(journal);
            report_repair_refusal(main, &key, &error);
            return Err(error);
        }
        Some((
            entry.key.clone(),
            entry.lane_id.clone(),
            entry
                .original_conflict_paths
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        ))
    } else {
        None
    };
    // Both gates use the same host-recorded paths, including after generators
    // replace outputs whose template masters were never conflicted.
    // trace:STORY-2710 | ai:codex
    let check_repair_markers = || {
        refuse_pending_conflict_markers(
            &|program, args| {
                run_sandboxed(clone, main, program, args, PROBE_OUTPUT_CAP)
                    .map_err(|why| anyhow::anyhow!("could not inspect conflict markers: {why}"))
            },
            repair.as_ref().map(|(_, _, paths)| paths.as_slice()),
        )
    };
    let report_refusal = |error: anyhow::Error| {
        if let Some((key, _, _)) = &repair {
            report_repair_refusal(main, key, &error);
        }
        error
    };
    check_repair_markers().map_err(report_refusal)?;
    let identity: [(&'static str, std::ffi::OsString); 4] = [
        ("GIT_AUTHOR_NAME", name.clone().into()),
        ("GIT_AUTHOR_EMAIL", email.clone().into()),
        ("GIT_COMMITTER_NAME", name.into()),
        ("GIT_COMMITTER_EMAIL", email.into()),
    ];
    let recovery = |step: &str, failure: SandboxFailure| {
        anyhow::anyhow!(
            "could not {step} the seat's pending work in {} inside AIDA's sandbox: {failure}. \
             The working tree is untouched; fix the cause and rerun `aida ship --from-seat {} \
             --commit-pending …`",
            clone.display(),
            clone.display()
        )
    };
    // HEAD before anything is staged: the index is restored only while HEAD
    // still points here (F5), never behind a commit that did land.
    // trace:TASK-2712 | ai:claude
    let head_before = probe_text(&[os("rev-parse"), os("--verify"), os("HEAD")]).ok();
    // The index as it is now, so a failure after staging can put it back.
    ensure_dir_identity(clone, pinned)?;
    let index_tree = if merge_base.is_some() {
        None
    } else {
        Some(
            run_commit_sandboxed(clone, main, &[os("write-tree")], &identity)
                .map(|out| String::from_utf8_lossy(&out).trim().to_string())
                .map_err(|failure| recovery("snapshot the index before staging", failure))?,
        )
    };
    anyhow::ensure!(
        index_tree
            .as_ref()
            .is_none_or(|tree| tree.len() >= 40 && tree.bytes().all(|b| b.is_ascii_hexdigit())),
        "refusing to commit the seat's pending work: `git write-tree` in {} printed `{}`, not \
         a tree id. Nothing was committed",
        clone.display(),
        format!("{index_tree:?}")
    );
    let unstage_marker = || -> Result<()> {
        // Should the seat's exclude no longer ignore the marker, unstage it.
        let staged_marker = probe_text(&[
            os("diff"),
            os("--cached"),
            os("--name-only"),
            os("--"),
            os(aida_core::seat_clone_detect::MARKER_RELATIVE),
        ])?;
        if !staged_marker.is_empty() {
            run_commit_sandboxed(
                clone,
                main,
                &[
                    os("reset"),
                    os("-q"),
                    os("--"),
                    os(aida_core::seat_clone_detect::MARKER_RELATIVE),
                ],
                &identity,
            )
            .map_err(|failure| recovery("unstage the seat marker from", failure))?;
        }
        Ok(())
    };
    let staged = (|| -> Result<String> {
        // Stage everything `.gitignore`/`info/exclude` allow. No explicit
        // marker exclude here: git refuses an add whose pathspec names an
        // ignored path ("paths are ignored"), and the marker normally IS
        // ignored. Never `-f`.
        // trace:TASK-2712 | ai:claude
        run_commit_sandboxed(
            clone,
            main,
            &[os("add"), os("-A"), os("--"), os(".")],
            &identity,
        )
        .map_err(|failure| recovery("stage", failure))?;
        if let Some(base) = merge_base.as_deref() {
            let head = head_before.as_deref().context("missing repair head")?;
            let key = crate::repair_journal::RepairGuard::lock(main)?
                .for_tree(clone, branch, head, base)?
                .key
                .clone();
            // Invalidate prior success before any generator can write. Every
            // publication attempt regenerates; stale success never grants credit.
            crate::repair_journal::RepairGuard::lock(main)?.record_regeneration(&key, false)?;
            regenerate_repair_outputs(
                clone,
                main,
                base,
                &identity,
                &aida_core::exec_context::Environment::capture(),
                false,
            )?;
            // Registered generators can reintroduce markers into a resolved
            // original conflict. Refuse before staging or recording success.
            // trace:STORY-2710 | ai:codex
            check_repair_markers()?;
            run_commit_sandboxed(
                clone,
                main,
                &[os("add"), os("-A"), os("--"), os(".")],
                &identity,
            )
            .map_err(|failure| recovery("stage regenerated outputs", failure))?;
            unstage_marker()?;
            // Record exactly what successful generation produced, after staging.
            let generated_tree = String::from_utf8(
                run_commit_sandboxed(clone, main, &[os("write-tree")], &identity)
                    .map_err(|failure| recovery("record regenerated tree", failure))?,
            )?
            .trim()
            .to_owned();
            crate::repair_journal::RepairGuard::lock(main)?
                .record_regenerated_tree(&key, &generated_tree)?;
        }
        if merge_base.is_none() {
            unstage_marker()?;
        }
        if let Some((key, lane, _)) = &repair {
            let write_tree = || -> Result<String> {
                Ok(String::from_utf8(
                    run_commit_sandboxed(clone, main, &[os("write-tree")], &identity)
                        .map_err(|failure| recovery("verify staged repair tree", failure))?,
                )?
                .trim()
                .to_owned())
            };
            let staged_tree = write_tree()?;
            crate::repair_journal::RepairGuard::lock(main)?
                .for_tree(
                    clone,
                    branch,
                    &key.original_head,
                    merge_base.as_deref().unwrap(),
                )?
                .require_regenerated_tree(&staged_tree)?;
            let snapshot = tempfile::tempdir()?;
            let mut command = Vec::new();
            let checked = snapshot_repair_tree(main, clone, &staged_tree, snapshot.path())
                .and_then(|()| {
                    let (ran, result) = compile(main, clone, lane, snapshot.path());
                    command = ran;
                    result
                })
                .and_then(|()| {
                    anyhow::ensure!(
                        write_tree()? == staged_tree
                            && inspect_merge()? == merge_base
                            && probe_text(&[os("rev-parse"), os("HEAD")])? == key.original_head
                            && probe_text(&[os("symbolic-ref"), os("-q"), os("HEAD")])? == want,
                        "staged repair tree, branch or parent changed during compile validation; retry publication"
                    );
                    Ok(())
                });
            let recorded =
                crate::repair_journal::RepairGuard::lock(main).and_then(|mut journal| {
                    journal.record_validation(key, &staged_tree, &command, checked.is_ok())
                });
            match (checked, recorded) {
                (Err(error), Err(record_error)) => {
                    return Err(error.context(format!(
                        "could not record repair validation: {record_error:#}"
                    )));
                }
                (Err(error), _) | (_, Err(error)) => return Err(error),
                (Ok(()), Ok(())) => {}
            }
        }
        ensure_dir_identity(clone, pinned)?;
        run_commit_sandboxed(
            clone,
            main,
            &[
                os("commit"),
                os("--no-verify"),
                // trace:BUG-2493 | ai:codex
                os("--cleanup=verbatim"),
                os("-q"),
                os("-m"),
                message.as_ref(),
            ],
            &identity,
        )
        .map_err(|failure| recovery("commit", failure))?;
        probe_text(&[os("rev-parse"), os("--verify"), os("HEAD")])
    })();
    match staged {
        Ok(tip) => Ok(Some(tip)),
        Err(error) => {
            let Some(index_tree) = index_tree else {
                return Err(report_refusal(error).context("pending repair merge retained with its staged index; retry uses the same host-recorded original conflict paths"));
            };
            // Put the index back exactly, but only while HEAD has not moved:
            // after a commit that landed, the index matches the new HEAD and
            // rolling it back would stage a reversal. Never touch the tree.
            let head_now = probe_text(&[os("rev-parse"), os("--verify"), os("HEAD")]).ok();
            if !index_restore_allowed(head_before.as_deref(), head_now.as_deref()) {
                return Err(error.context(format!(
                    "HEAD of {} moved or could not be read after the commit step, so its index \
                     was left as is (not rolled back to tree {index_tree}); check `git log -1` \
                     in the seat",
                    clone.display()
                )));
            }
            let restored = ensure_dir_identity(clone, pinned).is_ok()
                && run_commit_sandboxed(
                    clone,
                    main,
                    &[os("read-tree"), index_tree.as_ref()],
                    &identity,
                )
                .is_ok();
            if restored {
                Err(error)
            } else {
                Err(error.context(format!(
                    "the index of {} could not be restored to tree {index_tree}; restore it \
                     with `git read-tree {index_tree}` in the seat",
                    clone.display()
                )))
            }
        }
    }
}

// trace:TASK-3804 | ai:codex
fn report_repair_refusal(
    main: &Path,
    key: &crate::repair_journal::RepairKey,
    error: &anyhow::Error,
) {
    if let Err(mail) = crate::repair_journal::notify_refusal(main, key, &format!("{error:#}")) {
        eprintln!("could not record or deliver repair refusal: {mail:#}");
    }
}

// trace:TASK-3804 | ai:codex
fn pending_merge_parent(
    execute: &dyn Fn(&str, &[&std::ffi::OsStr]) -> Result<Vec<u8>>,
) -> Result<Option<String>> {
    let pending = execute(
        "sh",
        &[
            "-c".as_ref(),
            PENDING_MERGE_PATH.as_ref(),
            "merge-state".as_ref(),
            "parent".as_ref(),
        ],
    )?;
    if pending.is_empty() {
        return Ok(None);
    }
    let parent = std::str::from_utf8(&pending)?
        .strip_prefix("present:")
        .context("unexpected merge state response")?
        .trim();
    anyhow::ensure!(
        matches!(parent.len(), 40 | 64) && parent.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid MERGE_HEAD commit id"
    );
    let revision = format!("{parent}^{{commit}}");
    let resolved = execute(
        "git",
        &["rev-parse".as_ref(), "--verify".as_ref(), revision.as_ref()],
    )?;
    anyhow::ensure!(
        std::str::from_utf8(&resolved)?.trim() == parent,
        "MERGE_HEAD does not name a commit"
    );
    Ok(Some(parent.to_owned()))
}

// Materialize the pinned tree with a private index and the host Git config.
// The checkout cannot observe unstaged files or export-ignore attributes.
// trace:TASK-3804 | ai:codex
fn snapshot_repair_tree(main: &Path, clone: &Path, tree: &str, dest: &Path) -> Result<()> {
    use std::io::Write;
    let mut config = tempfile::NamedTempFile::new()?;
    config.write_all(probe_repo_config(main, clone).as_bytes())?;
    let mut args = probe_sandbox_args(
        clone,
        main,
        &crate::session::contained_read_allowlist(main),
        config.path(),
    );
    let scratch = tempfile::tempdir()?;
    args.pop(); // Move the command delimiter after the snapshot mounts.
    args.extend(
        [
            "--bind".as_ref(),
            scratch.path().as_os_str(),
            "/tmp/aida-index".as_ref(),
            "--bind".as_ref(),
            dest.as_os_str(),
            "/tmp/aida-tree".as_ref(),
            "--".as_ref(),
            "sh".as_ref(),
            "-ec".as_ref(),
            r#"export GIT_INDEX_FILE=/tmp/aida-index/index GIT_WORK_TREE=/tmp/aida-tree
git read-tree "$1"
git checkout-index --all --prefix=/tmp/aida-tree/"#
                .as_ref(),
            "snapshot-repair".as_ref(),
            tree.as_ref(),
        ]
        .into_iter()
        .map(std::ffi::OsStr::to_os_string),
    );
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(args).stdin(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(cmd, PROBE_TIMEOUT) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(()),
        crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
            "could not materialize staged repair tree: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        _ => anyhow::bail!("could not materialize staged repair tree within the sandbox budget"),
    }
}

/// Scan only the host-recorded original conflicts, including on staged retries.
/// The caller must obtain these from the shared repair journal, never reconstruct
/// them from changed paths or the current index. Missing evidence refuses pending
/// merge publication; ordinary publication needs no journal entry.
/// Literal pathspecs preserve unusual names without selecting additional files.
/// Deleted resolutions have no bytes to scan. Git grep does not follow worktree
/// symlinks or run textconv filters. Probe errors refuse publication.
// trace:STORY-2710 | ai:codex
fn refuse_pending_conflict_markers(
    execute: &dyn Fn(&str, &[&std::ffi::OsStr]) -> Result<Vec<u8>>,
    original_conflict_paths: Option<&[std::ffi::OsString]>,
) -> Result<()> {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;
    if pending_merge_parent(execute)?.is_none() {
        return Ok(());
    }
    let original = original_conflict_paths.context(
        "refusing to publish a pending merge: host-recorded original conflict paths are unavailable; shared repair journal integration is required; nothing was staged or committed",
    )?;
    anyhow::ensure!(
        !original.is_empty(),
        "refusing to publish a pending merge: host-recorded conflict path list is empty"
    );
    let paths: Vec<OsString> = original
        .iter()
        .map(|p| {
            use std::os::unix::ffi::OsStrExt;
            anyhow::ensure!(
                !p.is_empty()
                    && !p.as_bytes().contains(&0)
                    && std::path::Path::new(p)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "refusing to publish: invalid host-recorded conflict path {p:?}"
            );
            let mut literal = b":(literal)".to_vec();
            literal.extend_from_slice(p.as_bytes());
            Ok(OsString::from_vec(literal))
        })
        .collect::<Result<_>>()?;
    // grep's 1 means no matches; every other failure remains a failure. The
    // shell program is fixed, with filenames passed only as argv, never code.
    let mut args: Vec<&OsStr> = [
        "-c",
        "git grep --no-index --no-textconv -a -l -z -e '^<<<<<<<' -e '^>>>>>>>' -- \"$@\"; result=$?; if [ \"$result\" -eq 1 ]; then exit 0; else exit \"$result\"; fi",
        "aida-conflict-marker-scan",
    ].iter().map(OsStr::new).collect();
    args.extend(paths.iter().map(OsString::as_os_str));
    let matches = execute("sh", &args)?;
    anyhow::ensure!(
        matches.is_empty(),
        "refusing to publish: unresolved conflict markers in {}; resolve <<<<<<< / >>>>>>> markers before retrying; nothing was staged or committed",
        matches.split(|b| *b == 0).filter(|p| !p.is_empty())
            .map(|p| format!("{:?}", String::from_utf8_lossy(p)))
            .collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

/// [`commit_pending`]'s failure path may roll the index back only when HEAD
/// was read before staging and still reads the same now. A HEAD that moved
/// (the commit landed) or could not be read leaves the index alone.
// trace:TASK-2712 | ai:claude
pub(crate) fn index_restore_allowed(head_before: Option<&str>, head_now: Option<&str>) -> bool {
    matches!((head_before, head_now), (Some(before), Some(now)) if before == now)
}

/// A bundle of `refs/heads/<branch>` alone, made by git inside the probe
/// sandbox ([`probe_sandbox_args`]: hooks and fsmonitor off, no system or
/// global config, the clone's own config replaced) and returned as bytes.
/// Its prerequisites are the clone's pinned base (host-owned) or, without a
/// pin, the clone's `origin/*` refs. The caller verifies what comes back: it
/// is seat-made data.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn bundle_branch(main: &Path, clone: &Path, branch: &str) -> Result<Vec<u8>> {
    let head = format!("refs/heads/{branch}");
    let base = pinned_base(main, clone);
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "bundle".as_ref(),
        "create".as_ref(),
        "-".as_ref(),
        head.as_ref(),
        "--not".as_ref(),
    ];
    match &base {
        Some(base) => args.push(base.as_ref()),
        None => args.push("--remotes=origin".as_ref()),
    }
    run_sandboxed(clone, main, "git", &args, SALVAGE_OUTPUT_CAP).map_err(|failure| {
        anyhow::anyhow!(
            "could not bundle {head} of {} inside the seat sandbox: {failure}",
            clone.display()
        )
    })
}

/// The clone's git config and hooks as report lines for the operator: every
/// config key (read from a host-side copy by `git config --file`, which
/// follows no include) and every hook file that is not a `.sample`. Seat
/// files are read FIFO-safe; nothing here runs in the clone.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn config_hooks_report(main: &Path, clone: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let read = aida_core::seat_clone_detect::read_regular_file;
    match read(&clone.join(".git").join("config")) {
        None => lines.push("config: unreadable (not a regular file under 64 KiB)".to_string()),
        Some(body) => {
            let keys = tempfile::NamedTempFile::new()
                .ok()
                .and_then(|copy| {
                    std::fs::write(copy.path(), body).ok()?;
                    let mut cmd = git(main);
                    cmd.args(["config", "--file"])
                        .arg(copy.path())
                        .args(["--name-only", "--list"]);
                    run(cmd, "list the clone's config keys").ok()
                })
                .unwrap_or_default();
            if keys.is_empty() {
                lines.push("config: no keys could be listed".to_string());
            } else {
                // Seat-written names: escaped for display.
                let keys: Vec<String> =
                    keys.lines().map(|k| k.escape_debug().to_string()).collect();
                lines.push(format!("config: {}", keys.join(", ")));
            }
        }
    }
    let hooks: Vec<String> = std::fs::read_dir(clone.join(".git").join("hooks"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.ends_with(".sample"))
                .map(|name| name.escape_debug().to_string())
                .collect()
        })
        .unwrap_or_default();
    if hooks.is_empty() {
        lines.push("hooks: none (and none run: the bundle is made with hooks off)".to_string());
    } else {
        let mut hooks = hooks;
        hooks.sort();
        lines.push(format!(
            "hooks: {} (not run: the bundle is made with hooks off)",
            hooks.join(", ")
        ));
    }
    lines
}

/// Whether `clone` is where [`clone_path`] puts a seat clone of `main`: an
/// `aida-seat-*` directory that can be pinned, directly under main's
/// `[worktree_pool] worktree_parent`, and neither inside main nor holding it.
/// The lease is seat-writable through the store, so [`destroy`] refuses any
/// other path rather than delete what a forged lease names.
// trace:TASK-2053 | ai:claude
pub(crate) fn is_seat_clone_location(main: &Path, clone: &Path) -> bool {
    let Some(name) = clone.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !name.starts_with(CLONE_DIR_PREFIX) || pin_ref(clone).is_none() {
        return false;
    }
    let parent = crate::worktree_gc::worktree_pool_config_worktree_parent(main);
    let expected = aida_core::worktree_pool::worktree_placement_path(main, name, parent.as_deref());
    let canon_dir = |p: &Path| p.parent().and_then(|d| d.canonicalize().ok());
    let Some(dir) = canon_dir(clone) else {
        return false;
    };
    if canon_dir(&expected).as_ref() != Some(&dir) {
        return false;
    }
    let clone = dir.join(name);
    let main = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    !clone.starts_with(&main) && !main.starts_with(&clone)
}

/// The branch a seat clone has checked out, read from its `.git/HEAD` as a
/// plain file: host git never runs in a clone. `None` for a detached HEAD, a
/// rewired `.git` or anything that is not a regular `ref: refs/heads/` line.
// trace:BUG-2223 | ai:claude
pub(crate) fn checked_out_branch(clone_root: &Path) -> Option<String> {
    if !aida_core::seat_clone_detect::has_real_git_dir(clone_root) {
        return None;
    }
    let head = aida_core::seat_clone_detect::read_regular_file(&clone_root.join(".git/HEAD"))?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .filter(|b| !b.is_empty())
        .map(str::to_string)
}

/// How [`destroy`] treats a clone holding work origin does not have.
// trace:TASK-2053 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestroyMode {
    /// Reap: salvage a patch and KEEP the clone; also keep a clone a live
    /// process is using.
    KeepUnshipped,
    /// Session end or automatic lease cleanup: retain unpublished work;
    /// the caller already checked holders.
    // trace:BUG-2606 | ai:codex
    RetainUnshipped,
    /// Explicit discard and dismiss, after their own confirmation: salvage a
    /// patch, then remove the clone.
    SalvageUnshipped,
}

/// What [`destroy`] did.
// trace:TASK-2053 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destroyed {
    /// The clone, its cargo target directory and its base pin are gone.
    Removed { salvage: Option<PathBuf> },
    /// Nothing was removed; `reason` says why.
    Kept {
        reason: String,
        salvage: Option<PathBuf>,
        /// The mail-retention refusal that kept it, if that was the reason:
        /// the caller then keeps the lease for recovery ownership (R3-F1).
        retention: Option<aida_core::mailbox_lifecycle::guard::RetentionRefusal>,
    },
}

/// Tear down the seat clone at `clone` of the main checkout `main`:
///
/// 1. refuse unless `clone` is at a seat-clone location of `main`;
/// 2. [`probe`] it: unknown, or any submodule, keeps it (fail closed), and
///    so does a HEAD other than `expect_head` when the caller pinned one;
/// 3. salvage unpushed or uncommitted work, then keep or go on per `mode`;
/// 4. `remove_dir_all` the clone (it does not follow symlinks);
/// 5. remove `<main>/target/worktrees/<clone dir>`;
/// 6. release `refs/aida/seats/<clone dir>` in main.
///
/// A clone directory that is already gone skips straight to steps 5 and 6.
/// The caller releases the lease.
// trace:TASK-2053 | ai:claude
pub(crate) fn destroy(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
) -> Result<Destroyed> {
    destroy_with_shipped(main, clone, mode, expect_head, &[])
}

/// [`destroy`], reading the clone with [`probe_counting_origin`]: commits that
/// only a merged PR's head reaches also count as on origin.
// trace:TASK-2110 | ai:claude
pub(crate) fn destroy_with_shipped(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
    shipped: &[String],
) -> Result<Destroyed> {
    destroy_retained(main, clone, mode, expect_head, shipped, None)
}

/// [`destroy_with_shipped`] for a caller that already retired the clone's
/// mail (`session end` retires before it deletes the lease the binding
/// needs). With `retired == None` the clone is retired here, through the
/// verified host binding, right before the removal; a refusal keeps it.
/// Every path that keeps the clone reopens the retirement.
// trace:STORY-1843 | ai:claude
pub(crate) fn destroy_retained(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
    shipped: &[String],
    retired: Option<aida_core::mailbox_lifecycle::guard::Retirement>,
) -> Result<Destroyed> {
    destroy_retained_policy(main, clone, mode, expect_head, shipped, retired, true)
}

// Lifecycle GC reports build outputs without deleting them.
// trace:BUG-2587 | ai:codex
pub(crate) fn destroy_retained_preserving_target(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
    shipped: &[String],
    retired: Option<aida_core::mailbox_lifecycle::guard::Retirement>,
) -> Result<Destroyed> {
    destroy_retained_policy(main, clone, mode, expect_head, shipped, retired, false)
}

fn destroy_retained_policy(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
    shipped: &[String],
    retired: Option<aida_core::mailbox_lifecycle::guard::Retirement>,
    remove_target: bool,
) -> Result<Destroyed> {
    let mut retired = retired;
    let outcome = destroy_inner(
        main,
        clone,
        mode,
        expect_head,
        shipped,
        &mut retired,
        remove_target,
    );
    // Anything not consumed by the removal was kept: reopen it.
    crate::mail_retention::reopen_quietly(retired);
    outcome
}

fn destroy_inner(
    main: &Path,
    clone: &Path,
    mode: DestroyMode,
    expect_head: Option<&str>,
    shipped: &[String],
    retired: &mut Option<aida_core::mailbox_lifecycle::guard::Retirement>,
    remove_target: bool,
) -> Result<Destroyed> {
    if !is_seat_clone_location(main, clone) {
        anyhow::bail!(
            "{} is not a seat clone of {} (it must be an `{CLONE_DIR_PREFIX}*` directory in the \
             worktree parent); it was not removed",
            clone.display(),
            main.display()
        );
    }
    let mut salvaged = None;
    if std::fs::symlink_metadata(clone).is_ok() {
        // Recheck publication against origin, including explicit refspec pushes
        // that left no tracking ref. Keep fresh dirty/unpublished work protected.
        // trace:BUG-2630 | ai:codex
        let facts = match probe_counting_origin(main, clone, None, shipped, None) {
            Probe::Known(facts) => facts,
            Probe::Unknown(why) => {
                return Ok(Destroyed::Kept {
                    reason: format!("its state could not be read ({why}), so it is kept"),
                    salvage: None,
                    retention: None,
                })
            }
            Probe::HasSubmodules(reason) => {
                return Ok(Destroyed::Kept {
                    reason,
                    salvage: None,
                    retention: None,
                })
            }
        };
        if let Some(expected) = expect_head {
            if facts.head.as_deref() != Some(expected) {
                return Ok(Destroyed::Kept {
                    reason: "its HEAD moved since the scan".to_string(),
                    salvage: None,
                    retention: None,
                });
            }
        }
        // Unpublished checkpoint history needs a recoverable bundle before removal.
        // Probe inside the same sandbox; unreadable evidence retains the clone.
        // trace:BUG-2491 | ai:codex
        let base = pinned_base(main, clone).unwrap_or_default();
        let checkpoint_args: Vec<&std::ffi::OsStr> = [
            "-c".as_ref(),
            UNPUSHED_COUNT_SCRIPT.as_ref(),
            "aida-seat-checkpoints".as_ref(),
            "slice-only".as_ref(),
            base.as_ref(),
        ]
        .into_iter()
        .chain(
            shipped
                .iter()
                .filter(|oid| is_full_oid(oid))
                .map(|oid| oid.as_ref()),
        )
        .collect();
        let checkpoints = match probe_run(clone, main, &checkpoint_args, "sh").and_then(|out| {
            parse_unpushed_count(&out)
                .map(|(n, _)| n > 0)
                .ok_or_else(|| "invalid checkpoint count".to_string())
        }) {
            Ok(unpublished) => unpublished,
            Err(why) => {
                return Ok(Destroyed::Kept {
                    reason: format!("checkpoint refs could not be read ({why})"),
                    salvage: None,
                    retention: None,
                })
            }
        };
        if facts.unshipped() || checkpoints {
            // A salvage that failed or was cut off keeps the clone: removing
            // it would lose what the patch does not hold.
            // trace:TASK-2053 | ai:claude
            match salvage(main, clone) {
                Ok(path) => salvaged = Some(path),
                Err(error) => {
                    return Ok(Destroyed::Kept {
                        reason: format!("{error:#}"),
                        salvage: None,
                        retention: None,
                    })
                }
            }
            if mode != DestroyMode::SalvageUnshipped && facts.unshipped() {
                return Ok(Destroyed::Kept {
                    reason: unshipped_reason(&facts),
                    salvage: salvaged,
                    retention: None,
                });
            }
        }
        // Reap acts on a scan that may be stale (`aida session resume` into
        // the clone), so it re-checks for live processes right before the
        // removal and keeps the clone in use or unverified, as
        // `teardown_worktree_path` does for worktrees (BUG-1947). Session end
        // and dismiss ran their own holder checks, with their own overrides.
        // trace:TASK-2053 | ai:claude
        if mode == DestroyMode::KeepUnshipped {
            if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(clone) {
                return Ok(Destroyed::Kept {
                    reason: format!("it is in use: {in_use}"),
                    salvage: salvaged,
                    retention: None,
                });
            }
        }
        // Retire the clone's mail into main's anchor before the removal.
        // trace:STORY-1843 | ai:claude
        let retirement = match retired.take() {
            Some(retirement) => Some(retirement),
            None => {
                match crate::mail_retention::retire_tree(main, clone, true, "seat clone teardown") {
                    Ok(retirement) => retirement,
                    Err(refusal) => {
                        return Ok(Destroyed::Kept {
                            reason: crate::mail_retention::refusal_lines(&refusal).join("; "),
                            salvage: salvaged,
                            retention: Some(refusal),
                        })
                    }
                }
            }
        };
        // Re-verified right before the removal: mail that landed since the
        // commit keeps the clone. trace:STORY-1843 | ai:claude
        let removed = aida_core::mailbox_lifecycle::guard::remove_retired(
            clone,
            retirement,
            || {
                std::fs::remove_dir_all(clone)
                    .with_context(|| format!("remove the seat clone {}", clone.display()))
            },
            aida_core::mailbox_lifecycle::guard::Retirement::complete,
        );
        if let Err(error) = removed {
            if let Some(refusal) = crate::mail_retention::retention_refusal(&error) {
                return Ok(Destroyed::Kept {
                    reason: crate::mail_retention::refusal_lines(refusal).join("; "),
                    salvage: salvaged,
                    retention: Some(refusal.clone()),
                });
            }
            return Err(error);
        }
    }
    let name = clone.file_name().context("a seat clone has a name")?;
    let target = main.join("target").join("worktrees").join(name);
    if remove_target {
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_dir() => {
                if let Err(error) = std::fs::remove_dir_all(&target) {
                    eprintln!(
                        "could not remove the seat's cargo target {}: {error}",
                        target.display()
                    );
                }
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&target);
            }
            Err(_) => {}
        }
    }
    if let Some(pin) = pin_ref(clone) {
        if let Err(error) = release_pin(main, &pin) {
            eprintln!("{error:#}");
        }
    }
    // BUG-049: drop the seat's Claude Code trust entry with the seat, so
    // ~/.claude.json does not grow without bound. Teardown never fails on it.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
    if let Err(error) = crate::claude_trust::remove_seat_trust(main, clone) {
        eprintln!("could not remove the seat's Claude Code trust entry: {error:#}");
    }
    Ok(Destroyed::Removed { salvage: salvaged })
}

/// A base pin in main that no `clone` lease accounts for.
// trace:TASK-2053 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct OrphanPin {
    /// `refs/aida/seats/<clone dir>`.
    pub(crate) pin: String,
    /// The commit the pin named when it was listed; release deletes the pin
    /// only while it still names this commit.
    pub(crate) oid: String,
    /// Where that clone would be.
    pub(crate) clone: PathBuf,
    /// The directory still exists: an unleased clone, kept for the operator
    /// (`aida worktree dismiss <path>`). A pin whose clone is gone is safe to
    /// release.
    pub(crate) clone_exists: bool,
}

/// The directory names of every clone a `clone` lease in `main` names.
// trace:TASK-2053 | ai:claude
fn leased_clone_names(main: &Path) -> Vec<std::ffi::OsString> {
    crate::list_leases(main)
        .into_iter()
        .filter(|lease| lease.tree_kind == TreeKind::Clone)
        .filter_map(|lease| lease.worktree_path.file_name().map(|n| n.to_os_string()))
        .collect()
}

/// Release an orphan pin whose clone is gone. Re-checks right before the
/// delete that the directory is still absent and that no clone lease names
/// it, then deletes the pin only while it still names the listed commit
/// (`update-ref -d <pin> <oid>`), so a pass racing a launch or a re-lease
/// never drops a live clone's pin. `Ok(false)`: the state changed; kept.
// trace:TASK-2053 | ai:claude
pub(crate) fn release_orphan_pin(main: &Path, orphan: &OrphanPin) -> Result<bool> {
    let name = orphan.pin.strip_prefix(PIN_PREFIX).unwrap_or_default();
    if name.is_empty()
        || std::fs::symlink_metadata(&orphan.clone).is_ok()
        || leased_clone_names(main)
            .iter()
            .any(|l| l.as_os_str() == name)
    {
        return Ok(false);
    }
    let mut cmd = git(main);
    cmd.args(["update-ref", "-d", &orphan.pin, &orphan.oid]);
    run(cmd, &format!("release {}", orphan.pin)).map(|_| true)
}

/// Every `refs/aida/seats/*` pin in `main` with no `clone` lease naming its
/// clone (signoff condition 4: reap enumerates orphan pins).
// trace:TASK-2053 | ai:claude
pub(crate) fn orphan_pins(main: &Path) -> Vec<OrphanPin> {
    let mut cmd = git(main);
    cmd.args([
        "for-each-ref",
        "--format=%(objectname) %(refname)",
        PIN_PREFIX,
    ]);
    let Ok(listing) = run(cmd, "list seat-clone pins") else {
        return Vec::new();
    };
    let leased = leased_clone_names(main);
    let parent = crate::worktree_gc::worktree_pool_config_worktree_parent(main);
    listing
        .lines()
        .filter_map(|line| {
            let (oid, pin) = line.split_once(' ')?;
            let name = pin.strip_prefix(PIN_PREFIX)?;
            if name.is_empty() || leased.iter().any(|l| l.as_os_str() == name) {
                return None;
            }
            let clone =
                aida_core::worktree_pool::worktree_placement_path(main, name, parent.as_deref());
            Some(OrphanPin {
                pin: pin.to_string(),
                oid: oid.to_string(),
                clone_exists: std::fs::symlink_metadata(&clone).is_ok(),
                clone,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/bug_2169_seat_clone_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/task_2053_seat_clone_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "tests/bug_2169_contained_clone_bwrap_tests.rs"]
mod bwrap_tests;

#[cfg(test)]
#[path = "tests/task_2032_launcher_main_ratchet_tests.rs"]
mod launcher_main_ratchet_tests;

#[cfg(test)]
#[path = "tests/task_2053_s4_host_git_tests.rs"]
mod s4_host_git_tests;

#[cfg(test)]
#[path = "tests/task_2143_clone_git_ratchet_tests.rs"]
pub(crate) mod clone_git_ratchet_tests;

#[cfg(test)]
#[path = "tests/task_2053_s4b_seat_trees_tests.rs"]
mod s4b_seat_trees_tests;

#[cfg(test)]
#[path = "tests/bug_2223_orphan_seat_reentry_tests.rs"]
mod orphan_reentry_tests;

// trace:TASK-2823 | ai:claude
#[cfg(test)]
#[path = "tests/task_2823_lane_worktree_reentry_tests.rs"]
mod lane_worktree_reentry_tests;

// The queue-work launch rollback against a real setup; mounted here for the
// git fixture. trace:TASK-2807 | ai:claude
#[cfg(test)]
#[path = "tests/task_2807_pickup_launch_rollback_tests.rs"]
mod pickup_launch_rollback_tests;

// The conflict-repair reservation loop against real Git seats and a real
// pickup rollback. trace:BUG-2688 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2688_repair_reservation_tests.rs"]
mod repair_reservation_tests;

// The lane disk deferral against a real setup and a git-canonical queue.
// trace:BUG-2684 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2684_disk_deferral_tests.rs"]
mod disk_deferral_tests;

#[cfg(test)]
#[path = "tests/story_1843_c2_seat_clone_retention_tests.rs"]
mod retention_tests;

// trace:BUG-B6B1278890774EF993D2EBC327862470-031 | ai:claude
#[cfg(test)]
#[path = "tests/bug_031_seat_clone_mailbox_tests.rs"]
mod seat_clone_mailbox_tests;

// trace:BUG-HOME-038 | ai:claude
#[cfg(test)]
#[path = "tests/bug_home_038_seat_publish_tests.rs"]
pub(crate) mod seat_publish_tests;

// trace:BUG-2705 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2705_seat_clone_leftover_tests.rs"]
mod bug_2705_seat_clone_leftover_tests;

// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
#[cfg(test)]
#[path = "tests/story_b6b_100_seat_clone_handshake_tests.rs"]
mod seat_clone_handshake_tests;

#[cfg(test)]
#[path = "tests/bug_2455_force_claim_clone_tests.rs"]
mod bug_2455_force_claim_clone_tests;
// trace:BUG-2451 | ai:codex
#[cfg(test)]
#[path = "tests/bug_2451_pickup_entry_tests.rs"]
mod pickup_entry_tests;

// trace:BUG-2457 | ai:codex
#[cfg(test)]
#[path = "tests/bug_2457_review_repickup_tests.rs"]
mod review_repickup_tests;

// Rework fixtures must carry the same host ownership evidence as queue pickup.
// trace:BUG-2633 | ai:codex
#[cfg(test)]
pub(crate) fn record_test_publication_lease(main: &Path, tree: &Path, branch: &str) {
    let tree = tree.canonicalize().unwrap();
    if crate::list_leases(main).iter().any(|lease| {
        lease
            .worktree_path
            .canonicalize()
            .is_ok_and(|path| path == tree)
    }) {
        return;
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = crate::lease_path(main, &id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!(
        "id = {id:?}\nscope = \"BUG-2633\"\nslug = \"bug-2633\"\nowner = \"test\"\nworktree_path = {}\nbranch = {}\nstarted_at = \"2026-11-16T00:00:00Z\"\nhostname = \"h\"\nrole = \"implementer\"\n",
        aida_core::toml_quote::toml_string(&tree.display().to_string()),
        aida_core::toml_quote::toml_string(branch),
    )).unwrap();
}
