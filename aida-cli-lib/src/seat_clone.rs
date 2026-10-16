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

/// Until slice S3 moves the host's status, reap and end paths off plain git,
/// `seat_tree = "clone"` is a development switch: it works under test or with
/// this variable set to `1`, and refuses otherwise (advisor ruling on #2699).
// trace:BUG-2169 | ai:claude
pub(crate) const DEV_ENABLE_ENV: &str = "AIDA_DEV_SEAT_CLONE";

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
/// default stays `worktree` until slice S4 is approved (condition 5).
// trace:BUG-2169 | ai:claude
pub(crate) fn seat_tree_knob(project_root: &Path) -> Result<TreeKind> {
    let cfg = crate::read_project_config_value(project_root);
    parse_seat_tree_knob(crate::config_lookup(cfg.as_ref(), "contained", "seat_tree"))
}

/// Parse a raw `[contained] seat_tree` value. Absent means `worktree`; an
/// unknown value is an error, so a typo never silently picks a posture.
// trace:BUG-2169 | ai:claude
pub(crate) fn parse_seat_tree_knob(raw: Option<&toml::Value>) -> Result<TreeKind> {
    let Some(raw) = raw else {
        return Ok(TreeKind::Worktree);
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
    let kind = seat_tree_knob(project_root)?;
    let dev_switch = std::env::var(DEV_ENABLE_ENV).ok();
    if kind == TreeKind::Clone && !clone_seats_enabled(cfg!(test), dev_switch.as_deref()) {
        anyhow::bail!(
            "[contained] seat_tree = \"clone\" is not available yet: host status, reap and \
             session-end paths still run git in a seat's tree, which is unsafe for a private \
             clone. Set seat_tree = \"worktree\", or {DEV_ENABLE_ENV}=1 for development only."
        );
    }
    Ok(kind)
}

/// Whether clone seats may be created: under test, or with the development
/// switch set to exactly `1`.
// trace:BUG-2169 | ai:claude
pub(crate) fn clone_seats_enabled(under_test: bool, dev_switch: Option<&str>) -> bool {
    under_test || dev_switch.map(str::trim) == Some("1")
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

/// The loud refusal when a contained seat cannot get its clone. There is no
/// worktree fallback: a silent fallback is how a seat ends up with a shared
/// gitdir again (advisor ruling O1).
fn refusal(dest: &Path, error: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot create the private clone this contained seat needs at {}: {error:#}\n\
         A contained seat never falls back to a shared worktree. Fix the cause and retry, set \
         `[contained] seat_tree = \"worktree\"`, or launch from an uncontained shell.",
        dest.display()
    )
}

/// Create the private clone for a contained seat. Runs HOST side, before any
/// sandbox exists:
///
/// 1. resolve `base_ref` to a commit in `main` (the base is main's view);
/// 2. pin it at `refs/aida/seats/<dir>` in `main`;
/// 3. `git clone --reference <main> --no-checkout -- <origin> <dest>`;
/// 4. check out `branch` at that commit (present through alternates);
/// 5. exclude AIDA runtime files and write the marker.
///
/// Every failure is a [`refusal`]; anything made before it (pin, directory)
/// is removed, and `dest` is never a worktree afterwards.
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

    let mut pin_cmd = git(main);
    // An empty old value makes `update-ref` refuse when the pin already exists.
    pin_cmd.args(["update-ref", &pin, &base_sha, ""]);
    run(pin_cmd, &format!("pin {pin} in the main checkout")).map_err(|e| refusal(dest, e))?;

    let made = clone_and_checkout(main, dest, branch, &base_sha, &origin_url, &pin);
    if let Err(error) = made {
        if dest.exists() {
            let _ = std::fs::remove_dir_all(dest);
        }
        let _ = release_pin(main, &pin);
        return Err(refusal(dest, error));
    }
    Ok(CreatedClone {
        path: dest.to_path_buf(),
        base_sha,
        pin,
    })
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
    run(clone, &format!("clone {origin_url}"))?;

    let mut checkout = git(dest);
    checkout.args(["checkout", "--quiet", "--no-track", "-b", branch, base_sha]);
    run(checkout, &format!("check out `{branch}` at {base_sha}"))?;

    let exclude = dest.join(".git").join("info").join("exclude");
    if let Some(dir) = exclude.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("# AIDA runtime state (BUG-2169 seat clone)\n/.aida/*\n.aida-store\n");
    std::fs::write(&exclude, body).with_context(|| format!("write {}", exclude.display()))?;

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
        if let Err(error) = std::fs::remove_dir_all(&self.clone) {
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

#[cfg(test)]
#[path = "tests/bug_2169_seat_clone_tests.rs"]
mod tests;
