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
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    ts_rs_forge::TS,
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
    let project = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("project");
    let parent = crate::worktree_gc::worktree_pool_config_worktree_parent(project_root);
    for n in 1..=50 {
        let name = if n == 1 {
            format!("{CLONE_DIR_PREFIX}{project}-{slug}")
        } else {
            format!("{CLONE_DIR_PREFIX}{project}-{slug}-{n}")
        };
        let path = aida_core::worktree_pool::worktree_placement_path(
            project_root,
            &name,
            parent.as_deref(),
        );
        if !path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!(
        "no free seat-clone directory for `{slug}` next to {} (aida-seat-{project}-{slug} through \
         -50 all exist); remove stale seat clones first",
        project_root.display()
    )
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
    body.push_str("# AIDA runtime state (BUG-2169 seat clone)\n/.aida/*\n.aida-store\n");
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
    let leased = crate::list_leases(&main).into_iter().any(|lease| {
        lease.tree_kind == TreeKind::Clone
            && lease
                .worktree_path
                .canonicalize()
                .is_ok_and(|p| p == clone_root)
    });
    leased.then_some(main)
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
/// write. The main itself must be one THIS host process resolved
/// ([`aida_core::seat_clone_detect::is_host_main`]): [`main_for`] reads main
/// from the seat-writable marker, so without that anchor a seat could point
/// its marker at a forged "main" in a directory it can write and plant a
/// lease there. On top of that, `root` must be the clone root itself (not a
/// directory inside one), be named `aida-seat-*`, and neither contain the
/// main checkout nor sit inside it. The cargo target directory is accepted
/// only at exactly `<main>/target/worktrees/<clone dir>`, as a real
/// directory, never a symlink.
// trace:BUG-2169 | ai:claude
pub(crate) fn host_recorded_clone(root: &Path) -> Option<HostRecordedClone> {
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
    if !aida_core::seat_clone_detect::is_host_main(&main) {
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
    for (name, value) in env {
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
    let failed = |why: String| SandboxFailure::Failed(why);
    let repo_config = tempfile::NamedTempFile::new()
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(probe_repo_config(main, clone).as_bytes())?;
            Ok(file)
        })
        .map_err(|error| failed(format!("could not write the probe's git config: {error}")))?;
    let read_allowlist = crate::session::contained_read_allowlist(main);
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(probe_sandbox_args(
        clone,
        main,
        &read_allowlist,
        repo_config.path(),
    ))
    .arg(program)
    .args(args)
    .stdin(std::process::Stdio::null());
    let timeout = deadline
        .map(|end| {
            end.saturating_duration_since(std::time::Instant::now())
                .min(PROBE_TIMEOUT)
        })
        .unwrap_or(PROBE_TIMEOUT);
    if timeout.is_zero() {
        return Err(failed("the clone scan deadline elapsed".into()));
    }
    match crate::command_output_with_timeout_capped(cmd, timeout, cap) {
        crate::BoundedCommandOutput::Completed(mut out) if out.stdout.len() as u64 > cap => {
            out.stdout.truncate(cap as usize);
            Err(SandboxFailure::Truncated(out.stdout))
        }
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(out.stdout),
        crate::BoundedCommandOutput::Completed(out) => Err(failed(format!(
            "the sandboxed {program} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
        crate::BoundedCommandOutput::SpawnFailed => Err(failed(
            "bwrap is not available to sandbox the probe".to_string(),
        )),
        crate::BoundedCommandOutput::TimedOut => Err(failed(format!(
            "the sandboxed {program} did not finish within {}s",
            timeout.as_secs()
        ))),
    }
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

/// The commit `refs/aida/seats/<dir>` pins in `main`: the clone's base, read
/// from main (host-owned), never from the clone.
// trace:TASK-2053 | ai:claude
pub(crate) fn pinned_base(main: &Path, clone: &Path) -> Option<String> {
    let pin = pin_ref(clone)?;
    let mut cmd = git(main);
    cmd.args([
        "rev-parse",
        "--verify",
        "--quiet",
        crate::git_arg_guard::END_OF_OPTIONS,
        &format!("{pin}^{{commit}}"),
    ]);
    run(cmd, "read the seat's base pin")
        .ok()
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

// One deadline covers all four sandbox calls and the host git that checks the
// clone's ref tips against main, rather than a fresh budget per call.
// trace:TASK-2053 | ai:codex
// trace:BUG-2224 | ai:claude
pub(crate) fn probe_until(
    main: &Path,
    clone: &Path,
    shipped: &[String],
    deadline: Option<std::time::Instant>,
) -> Probe {
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
        Err(why) => return Probe::Unknown(why),
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
        ],
        "git",
    ) {
        Ok(out) => out,
        Err(why) => return Probe::Unknown(why),
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
    facts
        .dirty
        .retain(|line| !is_runtime_noise_entry(clone, line));
    let base = pinned_base(main, clone).unwrap_or_default();
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
        Err(why) => return Probe::Unknown(why),
    };
    let in_main = main_reachable_commits(main, tips.lines().map(str::trim), deadline);
    let count = probe_run(
        clone,
        main,
        &[
            "-c".as_ref(),
            UNPUSHED_COUNT_SCRIPT.as_ref(),
            "aida-seat-unpushed".as_ref(),
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
        Err(why) => return Probe::Unknown(why),
    }
    Probe::Known(facts)
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

/// Whether a probe `git status` entry is the session's runtime state or
/// build/cache output rather than work: the `.aida-store` link to the
/// primary's store, `.aida/cache.db*`, `.aida/agents`, and `target/`. Only
/// untracked entries qualify; `.aida-store` only while it is a symlink (read
/// with metadata only; nothing in the clone is opened).
// trace:BUG-B6B1278890774EF993D2EBC327862470-051 | ai:claude
pub(crate) fn is_runtime_noise_entry(clone: &Path, line: &str) -> bool {
    let Some(path) = line.strip_prefix("? ") else {
        return false;
    };
    let path = path.trim_end_matches('/');
    if path == ".aida-store" {
        return std::fs::symlink_metadata(clone.join(path))
            .is_ok_and(|meta| meta.file_type().is_symlink());
    }
    path.starts_with(".aida/cache.db")
        || path == ".aida/agents"
        || path.starts_with(".aida/agents/")
        || path == "target"
        || path.starts_with("target/")
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
/// host reading that decides whether a clone holds commits origin lacks
/// (session end, reap, status and awaiting, the lease classifiers, salvage,
/// phase 2) goes through here, so they agree. A clone whose first reading
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
    let first = probe_until(main, clone, shipped, deadline);
    let Probe::Known(facts) = &first else {
        return first;
    };
    if facts.unpushed == 0 {
        return first;
    }
    let Some(branch) = branch
        .filter(|b| !b.is_empty())
        .map(str::to_owned)
        .or_else(|| facts.branch.clone())
    else {
        return first;
    };
    let remaining = deadline.map_or(PROBE_TIMEOUT, |end| {
        end.saturating_duration_since(std::time::Instant::now())
    });
    if !refresh_origin_branch(main, &branch, (remaining / 2).min(ORIGIN_REFRESH_CAP)) {
        return first;
    }
    match probe_until(main, clone, shipped, deadline) {
        again @ Probe::Known(_) => again,
        _ => first,
    }
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
    unreached.args(["rev-list", "--stdin", "--not", "--all"]);
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
fn is_empty_gitmodules_placeholder(clone: &Path) -> bool {
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
/// (tags peeled), one per line, for [`main_reachable_commits`].
// trace:BUG-2224 | ai:claude
const REF_TIPS_SCRIPT: &str =
    "set -u\nexec git for-each-ref --format='%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)'\n";

/// Inside the probe sandbox: how many commits the clone holds that origin
/// does not: reachable from ANY ref (every branch, tag, HEAD) or stash entry,
/// and not from `origin/*`, the pinned base (`$1`, or empty) or an excused
/// head (`$2...`, full hex ids: a shipped head, or a tip main reaches; one the
/// clone lacks is skipped). Submodules are not counted: [`probe`] keeps any
/// clone that has one. The total is the first line; then, when it is not 0,
/// `<count> <ref>` for each ref (a detached HEAD, every ref outside
/// `origin/*`, the stash) holding any.
// trace:TASK-2053 trace:BUG-2224 | ai:claude
const UNPUSHED_COUNT_SCRIPT: &str = concat!(
    "set -u\n",
    stash_entries_sh!(),
    "base=$1\nshift\nshipped=''\n",
    "for t in \"$@\"; do\n  if git cat-file -e \"$t^{commit}\" 2>/dev/null; then shipped=\"$shipped $t\"; fi\ndone\n",
    "total=$(git rev-list --count --all $s --not --remotes=origin ${base:+\"$base\"} $shipped) || exit 1\n",
    "printf '%s\\n' \"$total\"\n",
    "if [ \"$total\" -eq 0 ]; then exit 0; fi\n",
    "refs=$(git for-each-ref --format='%(refname)') || exit 1\n",
    "if ! git symbolic-ref -q HEAD >/dev/null && git rev-parse -q --verify HEAD >/dev/null; then\n",
    "  refs=\"HEAD\n$refs\"\n",
    "fi\n",
    "printf '%s\\n' \"$refs\" | while IFS= read -r r; do\n",
    "  case $r in ''|refs/remotes/origin/*|refs/stash) continue;; esac\n",
    "  n=$(git rev-list --count \"$r\" --not --remotes=origin ${base:+\"$base\"} $shipped) || exit 1\n",
    "  if [ \"$n\" -gt 0 ]; then printf '%s %s\\n' \"$n\" \"$r\"; fi\n",
    "done || exit 1\n",
    "if [ -n \"$s\" ]; then\n",
    "  n=$(git rev-list --count $s --not --remotes=origin ${base:+\"$base\"} $shipped) || exit 1\n",
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
/// `$1` is the pinned base, or empty.
// trace:TASK-2053 | ai:claude
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
    salvage_capped(main, clone, SALVAGE_OUTPUT_CAP)
}

/// [`salvage`] keeping at most `cap` bytes: a longer patch is written cut
/// off, with a closing `# TRUNCATED` line, and returned as an error so the
/// caller keeps the clone.
// trace:TASK-2053 | ai:claude
fn salvage_capped(main: &Path, clone: &Path, cap: u64) -> Result<PathBuf> {
    let base = pinned_base(main, clone).unwrap_or_default();
    let ran = run_sandboxed(
        clone,
        main,
        "sh",
        &[
            "-c".as_ref(),
            SALVAGE_SCRIPT.as_ref(),
            "aida-seat-salvage".as_ref(),
            base.as_ref(),
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
    /// Session end and dismiss, after their own confirmation: salvage a
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

/// [`destroy`], reading the clone with [`probe_with_shipped`]: commits that
/// only a merged PR's head reaches count as on origin.
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
    let mut retired = retired;
    let outcome = destroy_inner(main, clone, mode, expect_head, shipped, &mut retired);
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
        let facts = match probe_with_shipped(main, clone, shipped) {
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
        if facts.unshipped() {
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
            if mode == DestroyMode::KeepUnshipped {
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
    if let Some(pin) = pin_ref(clone) {
        if let Err(error) = release_pin(main, &pin) {
            eprintln!("{error:#}");
        }
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
#[path = "tests/task_2053_s4_host_git_tests.rs"]
mod s4_host_git_tests;

#[cfg(test)]
#[path = "tests/task_2143_clone_git_ratchet_tests.rs"]
mod clone_git_ratchet_tests;

#[cfg(test)]
#[path = "tests/task_2053_s4b_seat_trees_tests.rs"]
mod s4b_seat_trees_tests;

#[cfg(test)]
#[path = "tests/bug_2223_orphan_seat_reentry_tests.rs"]
mod orphan_reentry_tests;

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
mod seat_publish_tests;

// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
#[cfg(test)]
#[path = "tests/story_b6b_100_seat_clone_handshake_tests.rs"]
mod seat_clone_handshake_tests;
