/// BUG-1971: the write roots of a contained session launched from this
/// process: the git root of cwd, which `queue work` has `set_current_dir`'d to
/// the lease's worktree. Both the contained `--settings` rules
/// ([`claude_contained_flags`]) and the `queue work` boundary announcement
/// ([`contained_write_boundary_announcement`]) read this one list, so the two
/// cannot disagree on a symlinked path. Outside a git repository there is no
/// worktree, so there are no roots and `dontAsk` denies every edit: the
/// boundary fails closed instead of widening to cwd (possibly `$HOME`).
// trace:BUG-1971 | trace:BUG-2012 | ai:antigravity
pub(crate) fn contained_session_write_roots() -> Vec<PathBuf> {
    contained_session_cwd()
        .as_deref()
        .map(contained_session_write_roots_from)
        .unwrap_or_default()
}

/// The write roots of a contained session whose process cwd is `cwd`: the git
/// root `cwd` sits in, or none outside a git repository (no cwd fallback).
// trace:BUG-1971 | ai:claude
pub(crate) fn contained_session_write_roots_from(cwd: &Path) -> Vec<PathBuf> {
    write_roots_for_git_root(crate::find_project_root_from(cwd).ok().as_deref())
}

// trace:BUG-2012 | ai:antigravity
fn contained_session_cwd() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(cwd) = contained_cwd_seam::cwd() {
        return Some(cwd);
    }
    #[cfg(test)]
    if let Some(root) = crate::test_ambient::project_root() {
        return Some(root);
    }
    std::env::current_dir().ok().map(ambient_session_cwd)
}

/// The process cwd as a contained session resolves it. Production: as is.
// trace:BUG-2381 | ai:claude
#[cfg(not(test))]
fn ambient_session_cwd(cwd: PathBuf) -> PathBuf {
    cwd
}

/// BUG-2381: an unpinned test must not resolve the operator's own checkout
/// (from a seat clone its main is the LIVE main); see
/// [`contained_cwd_seam::hermetic_unless_fixture`].
// trace:BUG-2381 | ai:claude
#[cfg(test)]
fn ambient_session_cwd(cwd: PathBuf) -> PathBuf {
    contained_cwd_seam::hermetic_unless_fixture(cwd)
}

/// The git root a contained session launched from this process writes in:
/// the git root of the process cwd, or `None` outside a git repository.
// trace:BUG-1971 | trace:BUG-2012 | ai:antigravity
fn contained_session_git_root() -> Option<PathBuf> {
    contained_session_write_roots().into_iter().next()
}

/// BUG-1971: test-only, per-thread stand-in for the process cwd that
/// [`contained_session_write_roots`] resolves from, so a test can drive the
/// production function from a directory outside git without a process-global
/// `chdir`. Takes precedence over the hermetic project-root pin.
// trace:BUG-1971 | ai:claude
#[cfg(test)]
pub(crate) mod contained_cwd_seam {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static CWD: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(crate) fn cwd() -> Option<PathBuf> {
        CWD.with(|c| c.borrow().clone())
    }

    /// Pins the cwd until the guard drops, restoring the previous pin.
    pub(crate) struct CwdGuard(Option<PathBuf>);

    pub(crate) fn pin(cwd: &Path) -> CwdGuard {
        CwdGuard(CWD.with(|c| c.replace(Some(cwd.to_path_buf()))))
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let prev = self.0.take();
            CWD.with(|c| *c.borrow_mut() = prev);
        }
    }

    /// BUG-2381: `cwd`, unless its git root is this crate's own workspace
    /// checkout — the directory libtest starts in — in which case a
    /// process-wide hermetic git repository stands in for it. In a seat clone
    /// or linked worktree that checkout's main is the operator's live main, so
    /// an unpinned test resolved the live store, its lock and its seat leases
    /// (and was refused as an unrecorded seat clone). A fixture directory a
    /// launch `chdir`s into has its own git root and is kept as given.
    // trace:BUG-2381 | ai:claude
    pub(crate) fn hermetic_unless_fixture(cwd: PathBuf) -> PathBuf {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent();
        let canon = |p: &Path| std::fs::canonicalize(p).ok();
        let git_root = crate::find_project_root_from(&cwd).ok();
        match (
            git_root.as_deref().and_then(canon),
            workspace.and_then(canon),
        ) {
            (Some(root), Some(ws)) if root == ws => hermetic_checkout().clone(),
            _ => cwd,
        }
    }

    /// The process-wide hermetic checkout, once created.
    static HERMETIC_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

    /// A plain `git init -b main` checkout under the system temp dir, created
    /// once per test process: a main checkout of its own, the shape a CI clone
    /// has, with no store, leases or seat marker. A test binary shares it
    /// across threads, so no test's `TempDir` can own it; it is removed when
    /// the process exits (an `atexit` hook, the way `drain_lock` cleans up),
    /// so test runs leave no `aida-bug-2381-checkout-*` directory behind.
    // trace:BUG-2381 | ai:claude
    pub(crate) fn hermetic_checkout() -> &'static PathBuf {
        HERMETIC_ROOT.get_or_init(|| {
            let dir = tempfile::Builder::new()
                .prefix("aida-bug-2381-checkout-")
                .tempdir()
                .expect("BUG-2381: create the hermetic checkout")
                .keep();
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            register_hermetic_cleanup();
            let status = hermetic_git_init(&dir)
                .status()
                .expect("BUG-2381: run git init for the hermetic checkout");
            assert!(status.success(), "BUG-2381: git init {}", dir.display());
            dir
        })
    }

    /// The `git init` for the hermetic checkout, with no system or global
    /// git config (an operator's `init.templateDir` hooks or
    /// `init.defaultBranch` never reach it) and an explicit `main` branch.
    // trace:BUG-2381 | ai:claude
    pub(crate) fn hermetic_git_init(dir: &Path) -> std::process::Command {
        let mut command = crate::test_git::git_command();
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["init", "-q", "-b", "main"])
            .arg(dir);
        command
    }

    /// Remove the hermetic checkout at process exit. Unix only (`libc` is a
    /// unix-only dependency); elsewhere it stays in the temp dir.
    // trace:BUG-2381 | ai:claude
    fn register_hermetic_cleanup() {
        #[cfg(unix)]
        // SAFETY: `remove_hermetic_checkout` is a no-arg extern "C" fn that
        // only reads a OnceLock and does best-effort fs; safe to run at exit.
        let _registered = unsafe { libc::atexit(remove_hermetic_checkout) };
    }

    #[cfg(unix)]
    extern "C" fn remove_hermetic_checkout() {
        if let Some(dir) = HERMETIC_ROOT.get() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The write roots for a discovered git root; none when there is no git root.
// trace:BUG-1971 | ai:claude
pub(crate) fn write_roots_for_git_root(git_root: Option<&Path>) -> Vec<PathBuf> {
    git_root
        .map(crate::contained_boundary::contained_write_roots)
        .unwrap_or_default()
}

/// BUG-1971: the `write boundary: …` line `queue work` prints for a contained
/// claude launch, built from the same roots as the launch's settings.
// trace:BUG-1971 | ai:claude
pub(crate) fn contained_write_boundary_announcement() -> String {
    // trace:BUG-1971 | ai:claude
    if contained_session_git_root().is_none() {
        return crate::contained_boundary::OUTSIDE_GIT_BOUNDARY_ANNOUNCEMENT.to_string();
    }
    crate::contained_boundary::write_boundary_announcement(&contained_session_write_roots())
}

/// BUG-1971: the write roots of a contained headless RESUME running in `cwd`.
/// `cwd` must itself be a git root (the worktree being resumed in): a `cwd`
/// that is a subdirectory of a checkout, or sits outside git (`$HOME`), grants
/// nothing, so a caller passing the wrong directory fails closed.
// trace:BUG-1971 | ai:claude
pub(crate) fn contained_resume_write_roots(cwd: &Path) -> Vec<PathBuf> {
    let Ok(canonical) = std::fs::canonicalize(cwd) else {
        return Vec::new();
    };
    let git_root = crate::find_project_root_from(&canonical)
        .ok()
        .filter(|root| *root == canonical);
    write_roots_for_git_root(git_root.as_deref())
}

/// Applied credit is launch evidence, never a configured vendor preference.
/// ClaudeNative preserves the legacy Claude behavior until PR3.
// trace:TASK-2707 | ai:codex
pub fn containment_applied(
    vendor: HeadlessVendor,
    contained: bool,
    receipt: Option<&crate::containment::launch::ContainmentReceipt>,
) -> bool {
    enum Evidence<'a> {
        ClaudeNative,
        Receipt(&'a crate::containment::launch::ContainmentReceipt),
        None,
    }
    let evidence = match (vendor, contained, receipt) {
        // trace:TASK-2708 | ai:claude — a Claude launch through the shared
        // wrapper is credited by its accepted receipt, like Codex.
        (HeadlessVendor::Claude, true, Some(receipt)) => Evidence::Receipt(receipt),
        (HeadlessVendor::Claude, true, None) => Evidence::ClaudeNative,
        (HeadlessVendor::Codex, true, Some(receipt)) => Evidence::Receipt(receipt),
        _ => Evidence::None,
    };
    match evidence {
        Evidence::ClaudeNative => true,
        Evidence::Receipt(receipt) => receipt.is_accepted(),
        Evidence::None => false,
    }
}

/// Whether a launch child's grant is minted (and its key preflighted) as a
/// sandboxed grant. A Codex child under the `aida` posture is credited before
/// its receipt exists, since the receipt is only produced at spawn; every mint
/// and preflight site asks this one question so they cannot disagree.
/// Only Codex consults the posture, so other vendors never fail here.
// trace:TASK-2708 | ai:claude
pub fn sandboxed_child_grant(vendor: HeadlessVendor, contained: bool) -> Result<bool> {
    Ok(containment_applied(vendor, contained, None)
        || (vendor == HeadlessVendor::Codex
            && containment_posture(vendor, contained)? == ContainmentPosture::Aida))
}

/// Whether the trusted default-branch config selects the shared contained
/// wrapper for Claude (`[contained.vendors] claude = "aida"`, written out).
/// Unset keeps the Claude-native boundary (Claude's own sandbox under AIDA's
/// generated settings) until the operator opts in. A malformed trusted
/// value refuses (fail closed).
// trace:TASK-2708 | ai:claude
pub(crate) fn claude_uses_contained_wrapper(root: &Path) -> Result<bool> {
    Ok(crate::agent_config::load_contained_vendor(root, "claude")?.as_deref() == Some("aida"))
}

/// Whether this headless launch runs inside the shared contained wrapper
/// (`launch::contained::PreparedLaunch`): Codex under the `aida` posture,
/// and Claude under the `aida` posture once the trusted config selects it.
// trace:TASK-2708 | ai:claude
pub(crate) fn contained_wrapper_launch(vendor: HeadlessVendor, contained: bool) -> Result<bool> {
    Ok(match vendor {
        HeadlessVendor::Codex => {
            containment_posture(vendor, contained)? == ContainmentPosture::Aida
        }
        HeadlessVendor::Claude => {
            contained
                && containment_posture(vendor, contained)? == ContainmentPosture::Aida
                && claude_uses_contained_wrapper(&headless_worktree_root())?
        }
        HeadlessVendor::Agy => false,
    })
}

thread_local! {
    /// Set only while a wrapper-owned launch composes its Claude argv.
    // trace:TASK-2708 | ai:claude
    static CLAUDE_WRAPPER_COMPOSITION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The scope in which a wrapper-owned launch composes its Claude
/// argv: the contained Claude flags are permitted, and the legacy os_wrap is
/// not applied (the wrapper owns the boundary). Outside it, a Claude AIDA
/// composition under `claude = "aida"` refuses by name
/// ([`CLAUDE_WRAPPER_UNWIRED`]): every surface not yet wired to the wrapper
/// fails closed rather than run the Claude-native boundary instead.
// trace:TASK-2708 | ai:claude
pub(crate) struct ClaudeWrapperComposition(bool);

impl ClaudeWrapperComposition {
    pub(crate) fn enter() -> Self {
        Self(CLAUDE_WRAPPER_COMPOSITION.with(|c| c.replace(true)))
    }
}

impl Drop for ClaudeWrapperComposition {
    fn drop(&mut self) {
        CLAUDE_WRAPPER_COMPOSITION.with(|c| c.set(self.0));
    }
}

pub(crate) fn composing_for_claude_wrapper() -> bool {
    CLAUDE_WRAPPER_COMPOSITION.with(std::cell::Cell::get)
}

/// The named refusal for a Claude AIDA launch surface the wrapper does not
/// own yet, under `[contained.vendors] claude = "aida"`.
// trace:TASK-2708 | ai:claude
pub(crate) const CLAUDE_WRAPPER_UNWIRED: &str = "[contained.vendors] claude = \"aida\" selects AIDA's shared contained wrapper, which supports foreground headless Claude launches, coordinators, private-PTY agent-new and AIDA-supervised background launches so far; this launch surface (resume) refuses rather than fall back. Remove `claude` from [contained.vendors] to use the Claude-native boundary.";

/// Effective launch boundary. Native never grants AIDA containment authority.
// trace:TASK-2631 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainmentPosture {
    Aida,
    Native,
    None,
}

// trace:TASK-2631 | ai:codex
impl ContainmentPosture {
    pub fn label(self) -> &'static str {
        match self {
            Self::Aida => "AIDA-contained",
            Self::Native => "native sandbox, not AIDA-contained",
            Self::None => "not contained",
        }
    }
}

/// Resolve the project opt-in, failing closed on malformed or unsupported policy.
// trace:TASK-2631 | ai:codex
pub(crate) fn containment_posture_in(
    root: &Path,
    vendor: &str,
    requested: bool,
) -> Result<ContainmentPosture> {
    if !requested {
        return Ok(ContainmentPosture::None);
    }
    let setting = crate::agent_config::load_contained_vendor(root, vendor)?;
    vendor_containment_posture(vendor, setting.as_deref())
}

// trace:TASK-3047 | ai:codex
const SUPPORTED_CONTAINMENT_VENDORS: &str = "Supported [contained.vendors] settings: claude = \"aida\" (AIDA containment), codex = \"native\" (explicit opt-in in the trusted default-branch config; native sandbox, not AIDA-contained). Use --vendor claude or AIDA_HEADLESS_VENDOR=claude for AIDA containment.";

// trace:TASK-2631 | ai:codex
// trace:TASK-3047 | ai:codex
fn vendor_containment_posture(vendor: &str, setting: Option<&str>) -> Result<ContainmentPosture> {
    // trace:TASK-3048 | ai:codex
    if HeadlessVendor::parse(vendor) == Some(HeadlessVendor::Agy) {
        return crate::launch::agy::AGY
            .require_conformance()
            .map(|()| ContainmentPosture::Aida)
            .map_err(|error| anyhow::anyhow!("{error}. {SUPPORTED_CONTAINMENT_VENDORS}"));
    }
    match (HeadlessVendor::parse(vendor), setting) {
        (Some(HeadlessVendor::Claude), None | Some("aida")) => Ok(ContainmentPosture::Aida),
        (Some(HeadlessVendor::Codex), Some("aida")) => Ok(ContainmentPosture::Aida),
        (Some(HeadlessVendor::Codex), Some("native")) => Ok(ContainmentPosture::Native),
        (_, Some("native")) => anyhow::bail!(
            "[contained.vendors] {vendor} = \"native\" is unsupported: `{vendor}` has no supported native sandbox; refusing to launch it. {SUPPORTED_CONTAINMENT_VENDORS}"
        ),
        (_, None | Some("aida")) => anyhow::bail!(
            "`{vendor}` cannot run an AIDA-contained session; refusing to launch it. {SUPPORTED_CONTAINMENT_VENDORS}"
        ),
        (_, Some(value)) => anyhow::bail!(
            "[contained.vendors] {vendor} = {} must be \"aida\" or \"native\"; refusing to launch it. {SUPPORTED_CONTAINMENT_VENDORS}", aida_core::toml_quote::toml_string(value)
        ),
    }
}

/// Runtime resolver shared by preflight and argv builders.
// trace:TASK-2631 | ai:codex
pub fn containment_posture(vendor: HeadlessVendor, requested: bool) -> Result<ContainmentPosture> {
    // Ambient launch-policy tests must not inherit this repository's opt-in.
    // Tests with policy fixtures pin the existing per-thread cwd/root seam.
    #[cfg(test)]
    if contained_cwd_seam::cwd().is_none() && crate::kernel::test_ambient::project_root().is_none()
    {
        return if requested {
            vendor_containment_posture(vendor.as_str(), None)
        } else {
            Ok(ContainmentPosture::None)
        };
    }
    containment_posture_in(&headless_worktree_root(), vendor.as_str(), requested)
}

/// BUG-2133: containment is monotonic — a contained drain contains every
/// agent child it spawns. Resolves whether the drain running in this process
/// is contained, the same way its phase-1 `aida queue work` children do
/// (`AIDA_CONTAINED` in the environment, or the `[agents] contained` knob), so
/// the children the drain spawns directly (CI-fix, advisor, harvest) can never
/// run with less confinement than their implementer sibling.
// trace:BUG-2133 | ai:claude
///
/// BUG-2146: an unreadable or malformed `agents.toml` is an error, not
/// "uncontained" — the caller refuses to launch, naming the file, rather than
/// failing open.
// trace:BUG-2146 | ai:claude
pub(crate) fn drain_contained(project_root: &Path) -> Result<bool> {
    if aida_contained_env()? {
        return Ok(true);
    }
    crate::agent_config::load_agents_contained(project_root).context(
        "[contained] could not read the contained posture — refusing to launch rather than \
         run uncontained. Fix the file named below and retry",
    )
}

/// The vendor and containment of a headless launch rooted at `project_root`:
/// the configured vendor, containment from the env OR the project/user
/// configuration ([`drain_contained`]), and a refusal when that vendor cannot
/// run contained. `what` names the launch in the refusal. Shared by the
/// burndown and intake coordinators and `questions remedy`/`clarify`, so none
/// of them can drift back to an env-only read.
// trace:BUG-2393 | ai:claude
pub(crate) fn resolve_headless_launch_posture(
    project_root: &Path,
    what: &str,
) -> Result<(HeadlessVendor, bool)> {
    let contained = drain_contained(project_root)?;
    let vendor = resolve_headless_vendor(project_root);
    require_vendor_containment(vendor, contained, what)?;
    Ok((vendor, contained))
}

/// A headless coordinator's launch (`burndown run`, `autopilot intake`):
/// [`resolve_headless_launch_posture`] plus its permission mode.
// trace:BUG-2393 | ai:claude
pub(crate) struct CoordinatorLaunch {
    pub vendor: HeadlessVendor,
    pub contained: bool,
    pub mode: String,
}

// trace:BUG-2393 | ai:claude
pub(crate) fn resolve_coordinator_launch(
    project_root: &Path,
    permission_mode: Option<&str>,
) -> Result<CoordinatorLaunch> {
    let (vendor, contained) =
        resolve_headless_launch_posture(project_root, "headless coordinator")?;
    let mode = coordinator_permission_mode(vendor, permission_mode, contained)?;
    Ok(CoordinatorLaunch {
        vendor,
        contained,
        mode,
    })
}

/// BUG-2146 (#2665 review): parse `AIDA_CONTAINED`. Case-insensitive
/// `1`/`true`/`yes`/`on` is contained; unset, empty, or `0`/`false`/`no`/`off`
/// is not. Any other value is an ERROR: a typo such as `AIDA_CONTAINED=ture`
/// must not read as "uncontained" (fail closed, never open).
// trace:BUG-2146 | ai:claude
pub(crate) fn aida_contained_env() -> Result<bool> {
    let Some(raw) = std::env::var_os("AIDA_CONTAINED") else {
        return Ok(false);
    };
    parse_aida_contained(&raw.to_string_lossy())
}

/// What `AIDA_CONTAINED` held before this process changed it for its
/// children, and the value it set: `(inherited, applied)`.
static CONTAINED_ENV_APPLIED: std::sync::Mutex<
    Option<(Option<std::ffi::OsString>, Option<std::ffi::OsString>)>,
> = std::sync::Mutex::new(None);

/// Set or clear `AIDA_CONTAINED` for the children this process launches.
/// The value the process inherited is kept, so [`process_contained`] still
/// answers for this process rather than for the children it posts.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn set_contained_env_for_children(contained: bool) {
    let mut applied = CONTAINED_ENV_APPLIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // A second export keeps the first one's inherited value, unless the env
    // was changed in between: then the current value is what was inherited.
    let current = std::env::var_os("AIDA_CONTAINED");
    let inherited = match applied.take() {
        Some((inherited, set)) if set == current => inherited,
        _ => current,
    };
    if contained {
        std::env::set_var("AIDA_CONTAINED", "1");
    } else {
        std::env::remove_var("AIDA_CONTAINED");
    }
    *applied = Some((inherited, std::env::var_os("AIDA_CONTAINED")));
}

/// Drop the remembered export (test guards that reset `AIDA_CONTAINED`).
#[cfg(test)]
pub(crate) fn forget_contained_env_for_children() {
    *CONTAINED_ENV_APPLIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Whether THIS process runs contained. A launcher resolves containment
/// from the `[agents] contained` knob and exports `AIDA_CONTAINED=1` for its
/// children ([`set_contained_env_for_children`]); that export must not make
/// the launcher itself refuse its primary-side writes (collecting a
/// reviewer's verdict onto the gate ledger). The inherited value answers
/// while the env still holds what this process set; any later change to
/// the env is read as it stands. A malformed value is contained (BUG-2146:
/// fail closed).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn process_contained() -> bool {
    // In the test binary every test shares one process env: a test may only
    // set AIDA_CONTAINED under the env lock, so a test that does not hold it
    // runs uncontained, as the primary side does, instead of racing a
    // sibling's contained window.
    #[cfg(test)]
    if !crate::test_env::holds_env_lock() && !crate::test_env::is_path_isolated_process() {
        return false;
    }
    let current = std::env::var_os("AIDA_CONTAINED");
    let applied = CONTAINED_ENV_APPLIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let raw = match applied.as_ref() {
        Some((inherited, set)) if *set == current => inherited.clone(),
        _ => current,
    };
    raw.is_some_and(|raw| parse_aida_contained(&raw.to_string_lossy()).unwrap_or(true))
}

/// Capture launch inputs before resolution. Preserve the parent's inherited
/// containment when this process has exported a different child posture.
// trace:STORY-2538 | ai:codex
pub(crate) fn capture_launch_exec(
    project_root: &Path,
    authority_root: Option<&Path>,
) -> Result<aida_core::exec_context::ExecContext> {
    use aida_core::exec_context::{Environment, ExecContext, SystemClock, ToolPaths};
    let environment = Environment::capture();
    if let Some(raw) = environment.get("AIDA_CONTAINED") {
        parse_aida_contained(&raw.to_string_lossy())?;
    }
    let current = environment
        .get("AIDA_CONTAINED")
        .map(std::ffi::OsStr::to_os_string);
    let inherited = {
        let applied = CONTAINED_ENV_APPLIED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match applied.as_ref() {
            Some((inherited, set)) if *set == current => inherited.clone(),
            _ => current,
        }
    };
    // Match process_contained's legacy shared-test-process isolation. Production
    // always uses the captured inherited marker above.
    #[cfg(test)]
    let inherited =
        if !crate::test_env::holds_env_lock() && !crate::test_env::is_path_isolated_process() {
            None
        } else {
            inherited
        };
    let tools = ToolPaths::new(
        environment.get("PATH").map(std::ffi::OsStr::to_os_string),
        [],
    );
    Ok(ExecContext::new(
        environment.with_value("AIDA_CONTAINED", inherited),
        crate::home_dir(),
        project_root.to_path_buf(),
        authority_root.unwrap_or(project_root).to_path_buf(),
        tools,
        std::sync::Arc::new(SystemClock::default()),
    ))
}

/// The pure parse behind [`aida_contained_env`].
// trace:BUG-2146 | ai:claude
pub(crate) fn parse_aida_contained(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "no" | "off" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        other => anyhow::bail!(
            "[contained] AIDA_CONTAINED={other:?} is not recognised — refusing to launch \
             rather than guess. Use 1/true/yes/on to contain, or 0/false/no/off (or unset it)."
        ),
    }
}

/// BUG-2144: the vendor half of [`child_containment_preflight`] for a launch
/// that is not necessarily a drain child (an interactive `aida queue work
/// --sandbox`, a headless implementer or reviewer). Containment is monotonic:
/// a contained request on a vendor that cannot apply it (only Claude can) is
/// refused, never silently downgraded to an uncontained launch.
// trace:BUG-2144 | ai:claude
// trace:TASK-3047 | ai:codex
pub(crate) fn require_vendor_containment(
    vendor: HeadlessVendor,
    contained: bool,
    what: &str,
) -> Result<()> {
    // trace:TASK-2631 | ai:codex
    if contained {
        crate::refuse_uncontainable_agent_tool(vendor.as_str()).with_context(|| format!(
            "the `{}` vendor cannot run a contained session: {SUPPORTED_CONTAINMENT_VENDORS}\n  [contained] refusing to launch the {what} uncontained", vendor.as_str()
        ))?;
    }
    Ok(())
}

/// BUG-2133: fail closed. A child launch that was asked to run contained must
/// be able to: the vendor must apply containment (only Claude does), and on
/// Linux the sandbox the contained posture turns on needs `bwrap`. Otherwise
/// refuse to launch rather than silently run the child uncontained.
// trace:BUG-2133 | ai:claude
pub(crate) fn require_child_containment(
    vendor: HeadlessVendor,
    contained: bool,
    child: &str,
) -> Result<()> {
    let bwrap_needed = cfg!(target_os = "linux");
    child_containment_preflight(
        vendor,
        contained,
        !bwrap_needed || which_on_path("bwrap").is_some(),
        child,
    )
}

/// The pure decision behind [`require_child_containment`]; `sandbox_available`
/// is whether the host can provide the contained sandbox.
// trace:BUG-2133 | ai:claude
// trace:TASK-3047 | ai:codex
pub(crate) fn child_containment_preflight(
    vendor: HeadlessVendor,
    contained: bool,
    sandbox_available: bool,
    child: &str,
) -> Result<()> {
    if !contained {
        return Ok(());
    }
    // trace:BUG-2223 | ai:claude
    // trace:TASK-2631 | ai:codex
    let posture = containment_posture(vendor, true).with_context(|| format!(
        "the `{}` vendor cannot run a contained session: {SUPPORTED_CONTAINMENT_VENDORS}\n  [contained] refusing to launch the {child} uncontained", vendor.as_str()
    ))?;
    if posture == ContainmentPosture::Native {
        return Ok(());
    }
    if !sandbox_available {
        anyhow::bail!(
            "[contained] this drain is contained, but `bwrap` (bubblewrap) was not found on \
             PATH — refusing to launch the {child} uncontained. Install it ({BWRAP_INSTALL_DEBIAN}) \
             or turn containment off for this drain."
        );
    }
    Ok(())
}

/// STORY-263: build the argv (after the `claude` program name) for a headless
/// `claude -p` launch. The flag set is SPIKE-7's mandatory list — see
/// `docs/internal/spikes/2026-05-16-claude-headless.md`:
///   - `-p` — print mode: single turn, exits on its own, no Ctrl+D.
///   - `--permission-mode bypassPermissions` — mandatory; `acceptEdits`
///     leaves `Bash` gated and `default` auto-denies silently (spike Q2).
///   - `--output-format stream-json --verbose` — newline-delimited JSON
///     events, so a watchdog (TASK-298) can tail liveness (spike Q6).
///   - `--disallowed-tools AskUserQuestion` — structural programmatic gate:
///     under headless mode there is no human to answer the prompt, so the
///     tool must be unavailable rather than advisory. BUG-280 added skill-
///     template instructions saying "don't call AskUserQuestion under
///     `--no-human`"; BUG-327 found a reviewer reasoning past those
///     instructions and bailing without a verdict file. The flag is
///     variadic on the claude side, so we wedge it between `--verbose`
///     and `--session-id` so the next `--` token terminates its value
///     list and the prompt positional at the tail stays the prompt.
///     trace:BUG-327 | ai:claude
///   - `--session-id <uuid>` — persistence stays ON, so a killed run stays
///     resumable (spike Q9).
///
/// Never `--bare`: it strips OAuth/keychain auth and breaks login (spike Q1).
/// Pure — the flag set is unit-tested without spawning claude.
/// trace:STORY-263 | ai:claude

pub fn claude_contained_flags(host_main: Option<&Path>) -> Result<Vec<String>> {
    // BUG-1928: the write boundary is the worktree this process runs in — the
    // git root of cwd, which `queue work` has already `set_current_dir`'d to the
    // lease's worktree (pool or named), and the same root the STORY-612 OS
    // wrapper binds rw. Never the orchestrator's requested path, which the warm
    // pool may have replaced. trace:BUG-1928 | ai:claude
    // BUG-1971: no git root means no write roots (fail closed), never a cwd
    // fallback. trace:BUG-1971 | ai:claude
    claude_contained_flags_for_roots(host_main, &contained_session_write_roots())
}

/// BUG-1928: the contained launch flags with the write boundary scoped to
/// `worktree` (see `crate::contained_boundary`).
// trace:BUG-1928 | ai:claude
pub fn claude_contained_flags_for(
    host_main: Option<&Path>,
    worktree: &Path,
) -> Result<Vec<String>> {
    claude_contained_flags_for_roots(
        host_main,
        &crate::contained_boundary::contained_write_roots(worktree),
    )
}

/// The contained launch flags for an explicit list of write roots. Errors when
/// the launch must be refused ([`claude_contained_settings_json`]).
// trace:BUG-1971 | ai:claude
// trace:BUG-1982 | ai:claude
pub(crate) fn claude_contained_flags_for_roots(
    host_main: Option<&Path>,
    write_roots: &[PathBuf],
) -> Result<Vec<String>> {
    // trace:TASK-2708 | ai:claude — every Claude AIDA composition passes
    // here: under `claude = "aida"`, only a wrapper-owned launch may compose.
    anyhow::ensure!(
        composing_for_claude_wrapper()
            || !claude_uses_contained_wrapper(&headless_worktree_root())?,
        CLAUDE_WRAPPER_UNWIRED
    );
    Ok(vec![
        "--setting-sources".to_string(),
        "project".to_string(),
        "--settings".to_string(),
        claude_contained_settings_json(host_main, write_roots)?,
    ])
}

/// Which AIDA surface a supervised seat gets. TASK-1558 default is `Off`: SPIKE-73 measured MCP at
/// ~1.8-2x the CLI's cost for identical or worse success over a 72-cell matrix, and the result is
/// structural — on-demand schema loading does not rescue it. No shipped skill references the MCP
/// surface, and every generated seat brief already speaks in `aida ...` CLI verbs.
///
/// Claude enforces it with launch-time MCP flags; Codex with per-server `-c` overrides
/// ([`codex_mcp_flags`]), where an unset value means `aida` (BUG-2394). Antigravity registers MCP
/// in its own config files, so this cannot be enforced at launch for it.
// trace:TASK-1558 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentMcpSurface {
    /// No MCP servers at all; the `aida` CLI is the surface.
    #[default]
    Off,
    /// Only AIDA's own server definition (BUG-1698).
    Aida,
    /// Inject nothing; the vendor's own MCP configuration applies, trust prompt included.
    Native,
}

impl AgentMcpSurface {
    // trace:TASK-1558 | ai:claude
    pub(crate) fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "cli" => Ok(Self::Off),
            "aida" => Ok(Self::Aida),
            "native" | "vendor" => Ok(Self::Native),
            other => anyhow::bail!(
                "[agents] mcp value `{other}` is not recognised — expected `off` (no MCP, the \
                 `aida` CLI is the surface), `aida` (attach AIDA's own server) or `native` (leave \
                 the vendor's MCP configuration alone)"
            ),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Aida => "aida",
            Self::Native => "native",
        }
    }
}

/// The MCP launch flags for a claude seat under `surface`.
///
/// BUG-1698: every `aida agent new claude` launch in an AIDA repo used to stop on Claude Code's
/// project-MCP trust modal —
///
///   New MCP server found in this project: aida
///   > Continue without using this MCP server
///
/// in a PTY nobody was watching, blocking the caller until it was killed. `--permission-mode
/// bypassPermissions` does NOT dismiss it; project-MCP trust is a separate gate from tool
/// permission mode (probed on claude v2.1.283). `--strict-mcp-config` closes the gate under both
/// `Off` and `Aida` because nothing from `.mcp.json` is consulted — so a checkout cannot smuggle a
/// command in under the name `aida`.
// trace:BUG-1698 | ai:claude
// trace:TASK-1558 | ai:claude
pub(crate) fn claude_mcp_flags(surface: AgentMcpSurface) -> Vec<String> {
    match surface {
        AgentMcpSurface::Off => vec!["--strict-mcp-config".to_string()],
        AgentMcpSurface::Aida => vec![
            "--mcp-config".to_string(),
            aida_mcp_server_config_json(),
            "--strict-mcp-config".to_string(),
        ],
        AgentMcpSurface::Native => Vec::new(),
    }
}

/// The `aida` server definition AIDA vouches for, built here rather than read from the
/// repository.
// trace:BUG-1698 | ai:claude
pub(crate) fn aida_mcp_server_config_json() -> String {
    serde_json::json!({
        "mcpServers": {
            "aida": {
                "command": "aida",
                "args": ["mcp-serve"]
            }
        }
    })
    .to_string()
}

/// The paths a contained lane on `write_roots` writes beyond the roots
/// themselves, and the paths it must never write. Claude renders them into its
/// `--settings`; native Codex passes the grants as `--add-dir`
/// ([`crate::codex_native_sandbox`]), so both vendors get ONE write set.
/// Errors refuse the launch: the main git directory's deny cannot be
/// expressed exactly (BUG-1982), or the tree's `.git` points somewhere the
/// host does not vouch for (BUG-2016).
///
/// `host_main` (TASK-2032) is the only anchor that can make the first write
/// root a host-recorded seat clone; `None` recognises no clone.
// trace:TASK-2712 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn contained_write_set(
    host_main: Option<&Path>,
    write_roots: &[PathBuf],
) -> Result<ContainedWriteSet> {
    contained_write_set_with(host_main, write_roots, WriteSetMode::Launch)
}

/// BUG-2439: the write set a lane will get once it runs in a linked worktree
/// of `project_root` that does not exist yet (a `compete --dry-run` arm).
/// Such a worktree resolves the same main as `project_root`, so its grants
/// are derived from `project_root`'s write roots. Nothing is written: no
/// legacy cache migration, no store hook install, no store marking. A linked
/// worktree is never a host-recorded seat clone, so no clone grants either.
/// A legacy cache layout the launch would migrate first is the one case
/// where the preview can show one grant fewer than the launch.
// trace:BUG-2439 | ai:claude
pub(crate) fn contained_write_set_for_new_linked_worktree(
    host_main: Option<&Path>,
    project_root: &Path,
) -> Result<ContainedWriteSet> {
    contained_write_set_with(
        host_main,
        &contained_session_write_roots_from(project_root),
        WriteSetMode::PreviewNewLinkedWorktree,
    )
}

/// Whether [`contained_write_set_with`] prepares the host for a launch or
/// only reads it to preview one.
// trace:BUG-2439 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteSetMode {
    /// A real launch in `write_roots[0]`: migrate, install hooks, mark.
    Launch,
    /// A read-only preview for a not-yet-created linked worktree of
    /// `write_roots[0]` (see [`contained_write_set_for_new_linked_worktree`]).
    PreviewNewLinkedWorktree,
}

// trace:TASK-2712 trace:BUG-2439 | ai:claude
fn contained_write_set_with(
    host_main: Option<&Path>,
    write_roots: &[PathBuf],
    mode: WriteSetMode,
) -> Result<ContainedWriteSet> {
    let launch = mode == WriteSetMode::Launch;
    // trace:BUG-1928 | ai:claude
    // BUG-1929: the main checkout's store and cache, so the session's `aida`
    // writes (claim, comment, queue done) land. Priming first makes the
    // cache's persistent sidecars exist, since the sandbox skips a missing path.
    // BUG-1971: the main checkout is resolved from the session's own write
    // root, so no write roots (outside git) grants no store paths either: the
    // boundary stays fail closed.
    // trace:BUG-1929 | ai:claude
    // BUG-1982: `<main>/.git` is denied whatever the store grant, so a launch
    // that grants no store paths (the git version check below failing) still
    // keeps the main git directory read-only. trace:BUG-1982.ace25510
    // trace:BUG-1982 | ai:claude
    let mut main_git_deny: Vec<PathBuf> = Vec::new();
    let store_paths = match write_roots.first() {
        Some(worktree) => {
            let main = crate::main_worktree_root_from(worktree);
            // trace:TASK-1952 | trace:BUG-2016 | ai:claude
            verify_linked_worktree_main(host_main, worktree, &main)?;
            // Review round 1 F3: every spelling, from every write root (the
            // as-given and canonical roots may resolve to differently spelled
            // mains). trace:BUG-1982 | ai:claude
            for root in write_roots {
                let root_main = crate::main_worktree_root_from(root);
                for spelling in main_spellings_via(root, &root_main) {
                    for path in main_git_deny_paths(&spelling) {
                        if !main_git_deny.contains(&path) {
                            main_git_deny.push(path);
                        }
                    }
                }
            }
            refuse_unexpressible_main_git_deny(&main_git_deny)?;
            // TASK-2707: the sandbox binds the cache DIRECTORY; move a legacy
            // cache into it before launch. Nothing is created here (a missing
            // directory is a doctor gap, as a missing cache file was).
            // trace:TASK-2707 | ai:claude
            if launch {
                aida_core::db::migrate_legacy_cache_layout(&main.join(".aida"));
            }
            // Host-owned hooks must exist before the sandbox makes the common
            // git directory read-only. Every later store write re-verifies them
            // and refuses on failure; the write boundary is never widened.
            // trace:BUG-1929 | ai:codex
            let store = main.join(".aida-store");
            if launch && store.is_dir() {
                if let Err(error) = aida_core::git_ops::resolve_store_hooks_dir(&store) {
                    eprintln!("{error:#}");
                }
            }
            // BUG-2131: mark the store before the git version check, so host
            // store git on an older git refuses even without contained config.
            // Launchers refuse contained mode on git < 2.40; should any path
            // reach here anyway, no store write paths are granted, but a write
            // root that is the primary checkout still covers `.aida-store`, so
            // the marker (not the empty grant) is what keeps host git off it.
            // trace:BUG-2131 | ai:claude
            // trace:TASK-1775 | ai:claude
            // A preview assumes the launch's marking succeeds and writes nothing.
            // trace:BUG-2439 | ai:claude
            let mut paths = if launch {
                contained_store_grant(&main, &store, aida_core::git_ops::mark_store_contained)
            } else {
                contained_store_grant(&main, &store, |_| Ok(()))
            };
            // trace:BUG-2218 | ai:codex
            paths.extend(crate::contained_boundary::contained_mailbox_write_paths(
                &main,
            )?);
            // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
            paths.extend(crate::contained_boundary::contained_deputy_write_paths(
                &main,
            )?);
            paths
        }
        None => Vec::new(),
    };
    // BUG-2169 S2: a seat clone the host recorded also writes its own cargo
    // target directory, builds with no compiler wrapper, and may never
    // rewrite the host leases that vouch for it.
    // trace:BUG-2169 | ai:claude
    // A new linked worktree is never a recorded seat clone, even when the
    // root it is previewed from is one. trace:BUG-2439 | ai:claude
    let clone = write_roots
        .first()
        .filter(|_| launch)
        .and_then(|root| crate::seat_clone::recorded_clone_for(host_main, root));
    let (mut store_paths, mut extra_deny) = (store_paths, main_git_deny);
    // TASK-2772: sandboxed Bash in a lane may write the cargo slot locks and
    // build log, so the LIGHT gate gets a build slot inside the sandbox. Never
    // the shared sccache (ruling O2); the gate runner falls back to
    // `RUSTC_WRAPPER=`. Only beside a store grant (a fail-closed launch stays
    // empty), and never for a seat clone, which has its own target directory.
    // trace:TASK-2772 | ai:claude
    if clone.is_none() && !store_paths.is_empty() {
        for path in crate::contained_boundary::lane_build_write_paths() {
            if !store_paths.contains(&path) {
                store_paths.push(path);
            }
        }
    }
    if let Some(clone) = &clone {
        store_paths.extend(crate::seat_clone::seat_extra_write_paths(clone));
        for main in crate::contained_boundary::contained_write_roots(&clone.main) {
            let sessions = main.join(".aida").join("sessions");
            if !extra_deny.contains(&sessions) {
                extra_deny.push(sessions);
            }
        }
    }
    Ok(ContainedWriteSet {
        grant_paths: store_paths,
        deny_paths: extra_deny,
        clone,
    })
}

/// [`contained_write_set`]'s result.
// trace:TASK-2712 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct ContainedWriteSet {
    /// Store, cache, mailbox, deputy inbox, and the build slots or the seat
    /// clone's own cargo target.
    pub(crate) grant_paths: Vec<PathBuf>,
    /// Every spelling of the main git directory and, for a seat clone, main's
    /// `.aida/sessions`.
    pub(crate) deny_paths: Vec<PathBuf>,
    /// The host-recorded seat clone the first write root is, if any.
    pub(crate) clone: Option<crate::seat_clone::HostRecordedClone>,
}

/// The contained launch settings for `write_roots`. Errors (the launch is
/// refused) when the main git directory's deny cannot be expressed exactly as
/// a sandbox and tool rule (BUG-1982): Claude Code's own sandbox would leave
/// it writable to a linked-worktree session, so a launch without that deny
/// fails open.
///
/// `host_main` (TASK-2032) is the only anchor that can make the first write
/// root a host-recorded seat clone; `None` recognises no clone.
// trace:BUG-1982 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn claude_contained_settings_json(
    host_main: Option<&Path>,
    write_roots: &[PathBuf],
) -> Result<String> {
    // Egress allowlist is strictly opt-in via `[contained] allowed_hosts`. When
    // the project root or config can't be resolved we fall back to an empty
    // allowlist — i.e. the pre-STORY-605 behavior. trace:STORY-605 | ai:claude
    let root = crate::find_project_root().ok();
    let allowed_hosts = root
        .as_ref()
        .map(|r| contained_allowed_hosts(r))
        .unwrap_or_default();
    let managed_only = root
        .as_ref()
        .map(|r| contained_managed_domains_only(r))
        .unwrap_or(false);
    // trace:TASK-2712 | ai:claude
    let ContainedWriteSet {
        grant_paths: store_paths,
        deny_paths: extra_deny,
        clone,
    } = contained_write_set(host_main, write_roots)?;
    let json = contained_settings_json_with_boundary(
        host_main,
        &allowed_hosts,
        managed_only,
        write_roots,
        &store_paths,
        &extra_deny,
    );
    let Some(clone) = clone else {
        return Ok(json);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&json)?;
    // The clone's `.aida/sessions` is a symlink into main. A tool write to
    // that spelling follows it, so the tool rules deny it too. The sandbox
    // layer already denies main's real directory, and a symlink is not a
    // path it can bind.
    if let Some(deny) = settings["permissions"]["deny"].as_array_mut() {
        for root in write_roots {
            let sessions = root.join(".aida").join("sessions");
            // trace:BUG-b6b1278890774ef993d2ebc327862470-029 | ai:claude
            for rule in crate::contained_boundary::path_deny_rules(&sessions) {
                let rule = serde_json::Value::String(rule);
                if !deny.contains(&rule) {
                    deny.push(rule);
                }
            }
        }
    }
    let env: serde_json::Map<String, serde_json::Value> = crate::seat_clone::seat_build_env(&clone)
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
        .collect();
    settings["env"] = serde_json::Value::Object(env);
    Ok(settings.to_string())
}

/// The store write paths a contained launch grants for `main`. `mark` writes
/// the contained-store marker first (BUG-2131); when it fails the launch
/// grants NO store write path, so host store git on an older git is never left
/// to contained config alone (BUG-2154). A git without contained support also
/// grants none.
// trace:TASK-1952 | trace:BUG-2154 | ai:claude
pub(crate) fn contained_store_grant(
    main: &Path,
    store: &Path,
    mark: impl FnOnce(&Path) -> Result<()>,
) -> Vec<PathBuf> {
    if store.is_dir() {
        if let Err(error) = mark(store) {
            eprintln!("{error:#}");
            eprintln!("contained launch: the store is not marked, so no store write is granted");
            return Vec::new();
        }
    }
    if let Err(error) = aida_core::git_ops::ensure_git_supports_contained() {
        eprintln!("{error:#}");
        Vec::new()
    } else {
        crate::contained_boundary::contained_store_write_paths(main)
    }
}

/// BUG-2016: `main` was resolved through `worktree/.git`, a pointer file inside
/// the granted write root. Accept it only when the host-owned side agrees:
/// the pointer names a linked-worktree gitdir directly under `main`'s common
/// git directory (`<common>/worktrees/<name>` with a `commondir` back to it),
/// and that gitdir's own `gitdir` file names this worktree's `.git`. Both live
/// in `main`'s git directory, which the sandbox cannot write. A rewritten
/// pointer therefore refuses the launch instead of granting another
/// repository's store and cache. A tree that is its own main (a main
/// checkout, a seat clone) resolved its main without a pointer, and a tree
/// with no `.git` (a subdirectory) resolves through its parents.
///
/// TASK-2111: a `.git` directory (holding a `commondir` that names another
/// repository) or a `.git` symlink also redirects the resolved main, with no
/// pointer file to check. When such a tree resolves to a main other than
/// itself, it is accepted only as a host-recorded seat clone of that main.
// trace:TASK-1952 | trace:BUG-2016 | ai:claude
// trace:TASK-1952 | trace:TASK-2111 | ai:claude
pub(crate) fn verify_linked_worktree_main(
    host_main: Option<&Path>,
    worktree: &Path,
    main: &Path,
) -> Result<()> {
    let git_entry = worktree.join(".git");
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    if canon(worktree) == canon(main) {
        return Ok(());
    }
    let Ok(meta) = std::fs::symlink_metadata(&git_entry) else {
        return Ok(());
    };
    if !meta.file_type().is_file() {
        let recorded = crate::seat_clone::recorded_clone_for(host_main, worktree)
            .is_some_and(|clone| canon(&clone.main).is_some() && canon(&clone.main) == canon(main));
        if recorded {
            return Ok(());
        }
        anyhow::bail!(
            "contained launch refused: {} has a .git directory or symlink that resolves to \
             {}, and it is not a seat clone the host recorded for that checkout",
            worktree.display(),
            main.display()
        );
    }
    let refuse = || {
        anyhow::anyhow!(
            "contained launch refused: {} does not point at a worktree of {} \
             (its .git pointer may have been rewritten); repair it with \
             `git worktree repair` from the main checkout",
            worktree.display(),
            main.display()
        )
    };
    let gitdir = aida_core::git_ops::worktree_gitdir_path(worktree)
        .map(|raw| crate::contained_boundary::normalize_lexical(&raw))
        .ok_or_else(refuse)?;
    let main_common = if main.join(".git").is_dir() {
        Some(main.join(".git"))
    } else {
        aida_core::git_ops::worktree_gitdir_path(main)
    };
    let common_ok = crate::contained_boundary::is_linked_worktree_gitdir(&gitdir)
        && canon(&gitdir).and_then(|g| g.parent().and_then(Path::parent).map(Path::to_path_buf))
            == main_common.as_deref().and_then(canon);
    // `worktree.useRelativePaths` (git 2.48+) writes the back-pointer
    // relative to the gitdir itself.
    let back_ok = std::fs::read_to_string(gitdir.join("gitdir"))
        .ok()
        .and_then(|body| canon(&gitdir.join(body.trim())))
        == canon(&git_entry);
    if common_ok && back_ok {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// Every spelling of `<main>/.git` a tool or sandbox path may resolve to: the
/// main checkout as given and canonical ([`contained_write_roots`]), each
/// joined with `.git`, plus the canonical `.git` itself when it is a symlink,
/// and the `gitdir:` target when `<main>/.git` is a main-checkout gitfile.
///
/// [`contained_write_roots`]: crate::contained_boundary::contained_write_roots
// trace:BUG-1982 | ai:claude
// trace:BUG-2012 | ai:antigravity
pub(crate) fn main_git_deny_paths(main: &Path) -> Vec<PathBuf> {
    let roots = crate::contained_boundary::contained_write_roots(main);
    let mut paths: Vec<PathBuf> = roots.iter().map(|root| root.join(".git")).collect();
    if paths.is_empty() {
        paths.push(main.join(".git"));
    }
    if let Ok(canonical) = std::fs::canonicalize(main.join(".git")) {
        if !paths.contains(&canonical) {
            paths.push(canonical);
        }
    }
    for root in std::iter::once(main).chain(roots.iter().map(PathBuf::as_path)) {
        if let Some(raw_target) = aida_core::git_ops::worktree_gitdir_path(root) {
            let target = crate::contained_boundary::normalize_lexical(&raw_target);
            if !crate::contained_boundary::is_linked_worktree_gitdir(&target) {
                if !paths.contains(&target) {
                    paths.push(target.clone());
                }
                if let Ok(canon_target) = std::fs::canonicalize(&target) {
                    if !paths.contains(&canon_target) {
                        paths.push(canon_target);
                    }
                }
            }
        }
    }
    paths
}

/// The spellings of the main checkout `main` (as git resolved it, usually
/// canonical) that the write root `root` implies: `main` itself, plus, when
/// `root` lies inside the main checkout, the same directory spelled the way
/// `root` spells it (a symlinked checkout given as `/link/sub` yields `/link`).
// trace:BUG-1982 | ai:claude
fn main_spellings_via(root: &Path, main: &Path) -> Vec<PathBuf> {
    let mut spellings = vec![main.to_path_buf()];
    let (Ok(root_canonical), Ok(main_canonical)) =
        (std::fs::canonicalize(root), std::fs::canonicalize(main))
    else {
        return spellings;
    };
    let Ok(rel) = root_canonical.strip_prefix(&main_canonical) else {
        return spellings;
    };
    let mut given = root.to_path_buf();
    for _ in rel.components() {
        if !given.pop() {
            return spellings;
        }
    }
    if given.is_absolute() && !spellings.contains(&given) {
        spellings.push(given);
    }
    spellings
}

/// Refuse a contained launch when some spelling of `<main>/.git` cannot be
/// written as an exact deny rule (a rule metacharacter such as `( [ ! {` in
/// the path, or a non-UTF-8 segment).
// trace:BUG-1982 | ai:claude
fn refuse_unexpressible_main_git_deny(deny: &[PathBuf]) -> Result<()> {
    if let Some(path) = deny
        .iter()
        .find(|path| crate::contained_boundary::root_rule_path(path).is_none())
    {
        anyhow::bail!(
            "refusing to launch a contained session: the main git directory {} cannot be \
             denied exactly in the sandbox and tool rules (its path holds a character such \
             as `( ) [ ] {{ }} * ? ! \\`), so the session could rewrite the host's git \
             metadata. Move the checkout to a path without those characters.",
            path.display()
        );
    }
    Ok(())
}

/// Read `[contained] allowed_hosts` (a string array) from the project config.
/// Empty when unset, absent, or malformed — the network-egress restriction is
/// strictly OPT-IN, so an absent/typo'd key never silently restricts a drain.
/// Section is `[contained]` (not `[sandbox]`) to avoid colliding with the
/// `aida sandbox` throwaway-store command's vocabulary. trace:STORY-605 | ai:claude
pub(crate) fn contained_allowed_hosts(project_root: &std::path::Path) -> Vec<String> {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "allowed_hosts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// STORY-615: read `[contained] managed_domains_only` (bool, default false).
/// When true, the contained settings add `sandbox.network.allowManagedDomainsOnly`
/// so a HEADLESS drain default-DENIES egress (to the managed set + any
/// `allowed_hosts`) WITHOUT the approval prompt the allowlist-only path (STORY-605)
/// hits — which a `claude -p` drain can't answer. Default OFF so a building drain
/// that needs crates.io / github.com / npm isn't silently cut off; opt in only
/// when the drain's egress is fully covered by the managed set + allowed_hosts.
/// trace:STORY-615 | ai:claude
pub(crate) fn contained_managed_domains_only(project_root: &std::path::Path) -> bool {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "managed_domains_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// STORY-617: read `[contained] read_allowlist` (a string array of absolute
/// paths) from the project config. EMPTY when unset, absent, or malformed — the
/// strict read-confinement is strictly OPT-IN (default-ABSENT key), so an
/// absent/typo'd key never silently narrows the readable filesystem. Mirrors the
/// slice-1 `allowed_hosts` rule. When non-empty the os_wrap path binds ONLY
/// these paths (+ the essential system/toolchain paths + the worktree) ro,
/// instead of `--ro-bind / /`. trace:STORY-617 | ai:claude
pub(crate) fn contained_read_allowlist(project_root: &std::path::Path) -> Vec<String> {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "read_allowlist")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// TASK-809: build the Claude Code MANAGED-settings JSON for a headless drain.
/// STORY-615 already emits `sandbox.network.allowManagedDomainsOnly` in the
/// project `--settings` JSON, but Claude Code only HARD-BLOCKS (deny without a
/// prompt) when that flag arrives via the MANAGED settings tier — the project /
/// `--settings` tier still PROMPTS for a non-allowlisted domain, which a headless
/// `claude -p` drain cannot answer. This is the remaining slice: a managed-tier
/// settings document (delivered by bind-mounting it over the wrapped process's
/// `/etc/claude-code/managed-settings.json` inside the bwrap namespace — no host
/// root, no host pollution) so the headless drain's egress is truly default-deny.
///
/// Per Claude Code's docs, when `allowManagedDomainsOnly` is set in managed
/// settings, ONLY `allowedDomains` from the MANAGED settings are honored — so the
/// operator's `allowed_hosts` are mirrored here too. Pure + total so the shape is
/// unit-tested without spawning. trace:TASK-809 | ai:claude
pub(crate) fn managed_settings_json(allowed_hosts: &[String]) -> String {
    let mut network = serde_json::Map::new();
    network.insert(
        "allowManagedDomainsOnly".to_string(),
        serde_json::json!(true),
    );
    if !allowed_hosts.is_empty() {
        network.insert(
            "allowedDomains".to_string(),
            serde_json::json!(allowed_hosts),
        );
    }
    serde_json::json!({
        "sandbox": {
            "network": serde_json::Value::Object(network)
        }
    })
    .to_string()
}

/// Build the contained-mode `--settings` JSON. With a NON-EMPTY `allowed_hosts`
/// egress allowlist, a `sandbox.network.allowedDomains` key is added so Claude
/// Code's own sandbox (bubblewrap + an out-of-sandbox proxy on Linux) default-
/// denies network egress except to those hosts (STORY-605, SPIKE-61). With an
/// EMPTY allowlist the `network` key is OMITTED entirely — the output is the
/// same shape as the pre-STORY-605 contained settings, so the posture is
/// unchanged unless the operator opts in via `[contained] allowed_hosts`.
///
/// NOTE (slice-1 limitation): a non-allowlisted domain PROMPTS for approval by
/// default; a headless `claude -p` drain can't answer that prompt. True block-
/// without-prompt needs `network.allowManagedDomainsOnly` via MANAGED settings
/// (not this project `--settings`) and is a follow-up — see STORY-605. Pure +
/// total so the "empty → unchanged" invariant is unit-tested. trace:STORY-605
///
/// BUG-1928: `permissions.allow` is the WRITE boundary under `dontAsk` — one
/// absolute `//<root>/**` rule per file-writing tool for each of `write_roots`
/// (see `crate::contained_boundary`). It replaces `Edit(/**)`, whose single
/// leading `/` Claude Code anchors at the project root, so it never covered the
/// session's worktree and every contained edit was denied. Empty roots → no
/// allow rule → every edit denied (fails closed).
// trace:BUG-1928 | ai:claude
pub(crate) fn contained_settings_json(
    allowed_hosts: &[String],
    managed_domains_only: bool,
    write_roots: &[PathBuf],
) -> String {
    contained_settings_json_with_store(allowed_hosts, managed_domains_only, write_roots, &[])
}

/// [`contained_settings_json`] plus BUG-1929's `sandbox.filesystem`: the main
/// checkout's store and cache paths in `allowWrite`, which widen only what
/// sandboxed Bash (the `aida` binary) may write, and the store's `.git`
/// pointer in `denyWrite`. The Edit/Write tool boundary stays at
/// `write_roots`. The key is omitted when no path can be expressed.
// trace:BUG-1929 | ai:claude
pub(crate) fn contained_settings_json_with_store(
    allowed_hosts: &[String],
    managed_domains_only: bool,
    write_roots: &[PathBuf],
    store_write_paths: &[PathBuf],
) -> String {
    contained_settings_json_with_boundary(
        None,
        allowed_hosts,
        managed_domains_only,
        write_roots,
        store_write_paths,
        &[],
    )
}

/// [`contained_settings_json_with_store`] plus `extra_deny`: paths denied in
/// both the sandbox (`denyWrite`) and the direct tool rules even when no store
/// path is granted (BUG-1982: the main git directory). When an `extra_deny`
/// path cannot be expressed exactly (a rule metacharacter in it), the settings
/// grant no writes at all: no write-root allow rules and no store paths. A
/// missing deny never sits beside a write grant (fails closed).
// trace:BUG-1982 | ai:claude
pub(crate) fn contained_settings_json_with_boundary(
    host_main: Option<&Path>,
    allowed_hosts: &[String],
    managed_domains_only: bool,
    write_roots: &[PathBuf],
    store_write_paths: &[PathBuf],
    extra_deny: &[PathBuf],
) -> String {
    // trace:BUG-1982 | ai:claude
    // TASK-2037: the main-checkout `.git` denies computed from `write_roots`
    // must be expressible too; an unexpressible one would otherwise be
    // dropped from both layers while the write roots stay granted.
    // trace:TASK-1952 | trace:TASK-2037 | ai:claude
    let main_git_deny =
        crate::contained_boundary::contained_main_checkout_git_deny_paths(host_main, write_roots);
    let deny_expressible = extra_deny
        .iter()
        .chain(&main_git_deny)
        .all(|path| crate::contained_boundary::root_rule_path(path).is_some());
    let (write_roots, store_write_paths, main_git_deny): (&[PathBuf], &[PathBuf], &[PathBuf]) =
        if deny_expressible {
            (write_roots, store_write_paths, &main_git_deny)
        } else {
            (&[], &[], &[])
        };
    let mut deny_paths =
        crate::contained_boundary::contained_store_deny_write_paths(store_write_paths);
    for path in extra_deny {
        if !deny_paths.contains(path) {
            deny_paths.push(path.clone());
        }
    }
    let mut sandbox = serde_json::json!({
        "enabled": true,
        "failIfUnavailable": true,
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false
    });
    // STORY-615: the `network` key is added when EITHER an allowlist is set OR
    // managed-domains-only is on; with neither it's omitted entirely so the
    // posture is unchanged (the pre-STORY-605 shape). trace:STORY-615
    if !allowed_hosts.is_empty() || managed_domains_only {
        let mut network = serde_json::Map::new();
        if !allowed_hosts.is_empty() {
            network.insert(
                "allowedDomains".to_string(),
                serde_json::json!(allowed_hosts),
            );
        }
        if managed_domains_only {
            network.insert(
                "allowManagedDomainsOnly".to_string(),
                serde_json::json!(true),
            );
        }
        sandbox["network"] = serde_json::Value::Object(network);
    }
    let extra_entries = {
        // TASK-2007: the BUG-2012 main-checkout `.git` denies reach the OS
        // sandbox's `denyWrite` explicitly, as BUG-1982's `extra_deny` does,
        // not only `permissions.deny` via Claude Code's implicit merge.
        // trace:TASK-1952 | trace:TASK-2007 | ai:antigravity
        let mut layer_deny = extra_deny.to_vec();
        for path in main_git_deny {
            if !layer_deny.contains(path) {
                layer_deny.push(path.clone());
            }
        }
        crate::contained_boundary::sandbox_allow_write_entries(&layer_deny)
    };
    match crate::contained_boundary::sandbox_filesystem(store_write_paths) {
        Some(mut filesystem) => {
            if let Some(entries) = filesystem["denyWrite"].as_array_mut() {
                for entry in extra_entries {
                    let entry = serde_json::Value::String(entry);
                    if !entries.contains(&entry) {
                        entries.push(entry);
                    }
                }
            }
            sandbox["filesystem"] = filesystem;
        }
        // trace:BUG-1982 | ai:claude
        None if !extra_entries.is_empty() => {
            sandbox["filesystem"] = serde_json::json!({ "denyWrite": extra_entries });
        }
        None => {}
    }
    // Tool writes do not pass through Bash's OS sandbox, so deny the host
    // git authority paths in both layers (including exact pointer files).
    // BUG-2012: also deny `<root>/.git` and `<root>/.git/**` for any main
    // checkout in `write_roots` even when `store_write_paths` is empty.
    // trace:BUG-1929 | trace:BUG-2012 | ai:antigravity
    let mut deny: Vec<String> = destructive_command_deny_rules()
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in main_git_deny {
        if !deny_paths.contains(path) {
            deny_paths.push(path.clone());
        }
    }
    // trace:BUG-b6b1278890774ef993d2ebc327862470-029 | ai:claude
    for path in &deny_paths {
        for rule in crate::contained_boundary::path_deny_rules(path) {
            if !deny.contains(&rule) {
                deny.push(rule);
            }
        }
    }
    let mut allow = crate::contained_boundary::write_allow_rules(write_roots);
    // BUG-2201: `--setting-sources project` drops the operator's user-level
    // allows, and a downstream project's `.claude/settings.json` never
    // receives the template's (`permissions` is not a managed slot), so the
    // session's own `aida`/git calls and its reads of the main checkout's
    // store would be denied under `dontAsk`. The launcher grants them itself.
    // Only beside a write grant: a fail-closed launch stays empty.
    // trace:BUG-2201 | ai:claude
    if !allow.is_empty() {
        allow.extend(contained_baseline_allow_rules(
            write_roots,
            store_write_paths,
        ));
    }
    serde_json::json!({
        "permissions": {
            "allow": allow,
            "deny": deny
        },
        "sandbox": sandbox
    })
    .to_string()
}

/// BUG-2201: the tool rules every contained AIDA session gets whatever its
/// project's `.claude/settings.json` says: the `aida` CLI through Bash, and
/// `Read` of the session's worktree and the main checkout's store and cache.
/// No write tool appears here; the write boundary stays
/// [`crate::contained_boundary::write_allow_rules`].
///
/// Deliberately no git rule. Plain git reads already run under
/// `autoAllowBashIfSandboxed`, confined by the OS sandbox. An allow rule only
/// adds the commands that auto-allow would refuse, and a `Bash(git …)` prefix
/// rule cannot exclude git's option-borne exec and ref writes
/// (`grep -O<cmd>`, `fetch --upload-pack=` or a `+src:dst` refspec,
/// `diff --output=`/`--ext-diff`, `-c core.pager=`), which the deny list
/// cannot enumerate either (TASK-2058).
// trace:BUG-2201 | ai:claude
pub(crate) fn contained_baseline_allow_rules(
    write_roots: &[PathBuf],
    store_paths: &[PathBuf],
) -> Vec<String> {
    let mut rules: Vec<String> = vec!["Bash(aida)".to_string(), "Bash(aida *)".to_string()];
    let mut push = |rule: String| {
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    };
    for root in write_roots {
        if let Some(abs) = crate::contained_boundary::root_rule_path(root) {
            push(format!("Read(/{abs}/**)"));
        }
    }
    for path in store_paths {
        if let Some(abs) = crate::contained_boundary::root_rule_path(path) {
            push(format!("Read(/{abs})"));
            push(format!("Read(/{abs}/**)"));
        }
    }
    rules
}

fn destructive_command_deny_rules() -> Vec<&'static str> {
    vec![
        "Bash(rm -rf / *)",
        "Bash(rm -rf /)",
        "Bash(rm -rf ~ *)",
        "Bash(rm -rf ~)",
        "Bash(rm -rf .. *)",
        "Bash(rm -rf ../*)",
        "Bash(git reset --hard *)",
        "Bash(git reset --hard)",
        "Bash(git clean -fd *)",
        "Bash(git clean -fd)",
        "Bash(git clean -fx *)",
        "Bash(git clean -fx)",
        "Bash(git push --force *)",
        "Bash(git push -f *)",
        "Bash(git branch -D *)",
        "Bash(git checkout -- *)",
        "Bash(git restore --source * -- *)",
    ]
}

// ───────────────────────── STORY-612: OS-boundary wrapper ─────────────────────
//
// Slice 2 of the sandbox-execution work (SPIKE-61). The `contained` posture
// (STORY-567/605) turns on Claude Code's *own* Bash sandbox, but Edit/Write/MCP
// run unconfined and there is no OS boundary around the `claude` process itself.
// This wraps the headless `claude -p` spawn in **bubblewrap** so the WHOLE
// process — every tool it drives — is confined at the OS level.
//
// Model = WRITE-confinement (operator decision 2026-06-13): `--ro-bind / /`
// makes the whole filesystem READABLE but read-only, then rw-binds ONLY the code
// worktree, the `.aida-store` worktree, and the build/auth caches. So every
// OS-level WRITE by Edit/Write/MCP/Bash lands inside the worktree; a rogue or
// injected drain cannot `rm -rf ~`, tamper with `~/.ssh`, or scribble outside
// its tree. Network stays SHARED — we never `--unshare-net`, because `claude`
// itself must reach `api.anthropic.com`. Egress is therefore bounded by the
// slice-1 `[contained] allowed_hosts` allowlist (Claude's Bash-sandbox proxy),
// NOT by this FS confinement; READS stay broad. That read-exfil gap is the
// documented limitation a slice-3 strict read-confinement follow-up closes.
//
// Strictly opt-in via `[contained] os_wrap = true` (the slice-1 allowlist lives
// in the same `[contained]` block). Fail-CLOSED: if os_wrap is requested but
// `bwrap` is not on PATH we error rather than silently run unconfined — asking
// for an OS boundary and not getting one must never pass silently.
// trace:STORY-612 | ai:claude

/// Resolve the opt-in `[contained] os_wrap` flag from the project config rooted
/// at `worktree_root`. Defaults to `false` (today's behavior) when unset,
/// absent, malformed, or the config can't be read — the OS boundary is strictly
/// opt-in, mirroring the slice-1 `allowed_hosts` rule. trace:STORY-612 | ai:claude
/// trace:TASK-866 | ai:claude
///
/// The `AIDA_OS_WRAP` environment variable takes PRECEDENCE over the config
/// value (TASK-876): `bwrap` availability is a per-MACHINE property, so its
/// enable switch must be per-machine too. Committing `os_wrap = true` to the
/// tracked `.aida/config.toml` would enable it repo-wide and, being fail-closed,
/// would ERROR on every clone whose host lacks working bwrap. The env override
/// lets an operator enable confinement per-host (`export AIDA_OS_WRAP=1` in a
/// shell / `.bashrc`) with NO shared-config change and NO risk to other clones.
/// Accepts `1`/`true`/`yes` (on) and `0`/`false`/`no` (off), case-insensitive;
/// an unrecognized value is ignored (falls through to config).
/// trace:TASK-876 | ai:claude
pub(crate) fn os_wrap_enabled(worktree_root: &Path) -> bool {
    os_wrap_explicit(worktree_root).unwrap_or(false)
}

/// The `[contained] os_wrap` value someone actually chose: the `AIDA_OS_WRAP`
/// override, else the config key. `None` when neither says anything, which is
/// what lets a headless implementer lane default it on
/// ([`default_os_wrap_for_implementer_lane`]). trace:BUG-2434 | ai:claude
pub(crate) fn os_wrap_explicit(worktree_root: &Path) -> Option<bool> {
    if let Some(over) = os_wrap_env_override() {
        return Some(over);
    }
    let cfg = crate::read_project_config_value(worktree_root);
    crate::config_lookup(cfg.as_ref(), "contained", "os_wrap").and_then(|v| v.as_bool())
}

/// How a headless implementer lane's `[contained] os_wrap` resolves.
// trace:BUG-2434 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneOsWrap {
    /// Set by `AIDA_OS_WRAP` or the config: honoured as given.
    Explicit(bool),
    /// Unset, and bubblewrap works on this host: the lane turns it on.
    DefaultOn,
    /// Unset, and bubblewrap does not work here: it stays off.
    Unavailable,
}

/// BUG-2434: decide a headless implementer lane's os_wrap. The lane job
/// runner runs a job only inside bubblewrap and only with os_wrap on, and
/// os_wrap is opt-in, so on a host that never opted in every lane reported
/// `untested=true`. When nobody chose a value and bubblewrap works here, the
/// lane defaults it on; an explicit choice, either way, wins. Pure.
// trace:BUG-2434 | ai:claude
pub(crate) fn lane_os_wrap(explicit: Option<bool>, bwrap_live: bool) -> LaneOsWrap {
    match explicit {
        Some(on) => LaneOsWrap::Explicit(on),
        None if bwrap_live => LaneOsWrap::DefaultOn,
        None => LaneOsWrap::Unavailable,
    }
}

impl LaneOsWrap {
    /// Whether the lane's jobs may run (each inside its own bubblewrap
    /// profile). trace:BUG-2434 | ai:claude
    pub(crate) fn on(self) -> bool {
        matches!(self, LaneOsWrap::Explicit(true) | LaneOsWrap::DefaultOn)
    }
}

/// Resolve [`lane_os_wrap`] for a headless implementer lane at `seat`.
/// `bwrap_live` is the host probe, asked only when nobody chose a value (the
/// launcher passes [`bwrap_availability`]; tests inject it). Mutates nothing:
/// the decision travels on the lane's own job runner
/// ([`crate::lane_job::Sandbox::BwrapDefaultOn`]), never through this
/// process's environment, so no other launch in the process inherits it.
// trace:BUG-2434 | ai:claude
pub(crate) fn implementer_lane_os_wrap(
    seat: &Path,
    bwrap_live: impl FnOnce() -> bool,
) -> LaneOsWrap {
    let explicit = os_wrap_explicit(seat);
    lane_os_wrap(explicit, explicit.is_none() && bwrap_live())
}

/// Parse the `AIDA_OS_WRAP` per-host override. Returns `Some(true)`/`Some(false)`
/// for a recognized truthy/falsey value, `None` when the var is unset or holds
/// an unrecognized value (so the config value is consulted). Case-insensitive;
/// surrounding whitespace tolerated. trace:TASK-876 | ai:claude
fn os_wrap_env_override() -> Option<bool> {
    let raw = std::env::var("AIDA_OS_WRAP").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Build the bubblewrap argv that goes BETWEEN the `bwrap` program name and the
/// wrapped `claude` command — i.e. the confinement flags only. Pure + total so
/// the write-confinement invariants (ro root, rw worktree, never `--unshare-net`)
/// are unit-tested without spawning anything.
///
/// `rw_paths` are additional read-WRITE bind-mounts (the store worktree, cargo /
/// npm caches, the `~/.claude` auth dir + `~/.claude.json`). They use the
/// `--bind-try` form so a missing path is skipped rather than erroring — only
/// the worktree itself is a hard `--bind` (it always exists; the drain runs in
/// it). trace:STORY-612 | ai:claude
// STORY-617: the default (no read-allowlist) convenience entry — preserved as
// the STORY-612 public signature; production now calls the `_inner` variant
// directly to thread the opt-in allowlist. Still used by the bwrap unit tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn bwrap_confinement_args(worktree: &Path, rw_paths: &[PathBuf]) -> Vec<String> {
    bwrap_confinement_args_inner(worktree, rw_paths, &[])
}

/// STORY-617: the essential system paths a strict read-confinement allowlist
/// always binds (read-only) on top of the operator's `read_allowlist`, because
/// `claude` / `node` / `cargo` cannot run without them. Kept minimal and
/// READ-ONLY: the toolchains live under `/usr` and `/nix`, dynamic linking +
/// shared libs under `/lib*`, system config (incl. resolv.conf and the
/// `/etc/claude-code` managed-settings drop the TASK-809 path mounts over) and
/// TLS roots under `/etc`. `/dev`, `/proc`, `/tmp` are provided separately by
/// the bwrap `--dev`/`--proc`/`--tmpfs` flags. trace:STORY-617 | ai:claude
pub(crate) fn strict_read_essential_paths() -> &'static [&'static str] {
    &[
        "/usr", "/bin", "/sbin", "/lib", "/lib64", "/lib32", "/etc", "/opt", "/nix", "/run", "/var",
    ]
}

/// Build the bubblewrap confinement argv. STORY-617 adds an OPTIONAL strict
/// read-confinement allowlist: when `read_allowlist` is NON-EMPTY the whole-
/// filesystem `--ro-bind / /` base is REPLACED by an enumerated set of ro
/// binds — the essential system/toolchain paths (`strict_read_essential_paths`)
/// plus the operator's allowlist plus the worktree — so host secrets outside
/// the allowlist (`~/.ssh`, `~/.aws`, browser cookies) are simply NOT PRESENT
/// in the sandbox, not merely read-only. When `read_allowlist` is EMPTY the
/// output is byte-for-byte the pre-STORY-617 write-confinement base (ro root),
/// so the default posture is unchanged. The allowlist entries use `--ro-bind-try`
/// so a listed-but-absent path is skipped rather than aborting the launch.
/// Pure + total so both arms are unit-tested without spawning. trace:STORY-617
pub(crate) fn bwrap_confinement_args_inner(
    worktree: &Path,
    rw_paths: &[PathBuf],
    read_allowlist: &[String],
) -> Vec<String> {
    let wt = worktree.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    if read_allowlist.is_empty() {
        // Default (unchanged) posture: whole filesystem readable but READ-ONLY —
        // the STORY-612 write-confinement base.
        args.push("--ro-bind".into());
        args.push("/".into());
        args.push("/".into());
    } else {
        // STORY-617 strict read-confinement: default-ABSENT filesystem. Bind only
        // the essential system/toolchain paths + the operator allowlist (all ro
        // via `--ro-bind-try` so a missing path is skipped, not fatal). The
        // worktree itself is bound rw below.
        for p in strict_read_essential_paths() {
            args.push("--ro-bind-try".into());
            args.push((*p).to_string());
            args.push((*p).to_string());
        }
        for p in read_allowlist {
            args.push("--ro-bind-try".into());
            args.push(p.clone());
            args.push(p.clone());
        }
    }
    args.extend([
        // Fresh /dev, /proc, writable /tmp so toolchains have a sane env.
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        // The code worktree is the one always-present rw surface.
        "--bind".into(),
        wt.clone(),
        wt.clone(),
    ]);
    for p in rw_paths {
        // `--bind-try`: skip silently if the path is absent (caches/auth dirs
        // may not exist on a fresh machine) rather than aborting the launch.
        args.push("--bind-try".into());
        let s = p.to_string_lossy().into_owned();
        args.push(s.clone());
        args.push(s);
    }
    // Run inside the worktree (the parent's cwd may differ, e.g. resume), and
    // tear the sandbox down with the parent so a killed drain leaves nothing.
    args.push("--chdir".into());
    args.push(wt);
    args.push("--die-with-parent".into());
    args
}

/// The read-WRITE binds `os_wrap` adds beside the code worktree: the store
/// worktree, the npm cache and Claude Code's own state. The npm cache must
/// stay writable or npm fails mid-install; CARGO_HOME is read-only (BUG-2418:
/// cargo fetches new crates on the host); `~/.claude` + `~/.claude.json`
/// hold Claude Code's session state and auth, which it writes during a run.
///
/// BUG-2169 S2: for a seat clone the host recorded, the main checkout's
/// store and cache files and the seat's own cargo target directory instead
/// of `<clone>/.aida-store` (a symlink into main). Main's `.git` is never
/// bound: the seat commits in its own clone and pushes to origin.
// trace:STORY-612 | ai:claude
// trace:BUG-2169 | ai:claude
pub(crate) fn os_wrap_rw_paths(
    host_main: Option<&Path>,
    worktree_root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut rw_paths = match crate::seat_clone::recorded_clone_for(host_main, worktree_root) {
        Some(clone) => {
            let mut paths = crate::contained_boundary::contained_store_write_paths(&clone.main);
            paths.extend(crate::seat_clone::seat_extra_write_paths(&clone));
            paths
        }
        // TASK-2772: the same validated cargo slot pair the contained
        // settings grant, or the outer bwrap keeps them read-only and the
        // inner sandbox cannot reopen them. Never for a seat clone.
        // trace:TASK-2772 trace:BUG-2418 | ai:claude
        None => {
            let mut paths = vec![worktree_root.join(".aida-store")];
            for path in crate::contained_boundary::lane_build_write_paths() {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            paths
        }
    };
    // trace:BUG-2218 | ai:codex
    let main = crate::main_worktree_root_from(worktree_root);
    // Match the contained settings path: refuse the launch on an unsafe path.
    // trace:BUG-2218 TASK-2327 | ai:codex
    rw_paths.extend(crate::contained_boundary::contained_mailbox_write_paths(
        &main,
    )?);
    // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
    rw_paths.extend(crate::contained_boundary::contained_deputy_write_paths(
        &main,
    )?);
    // BUG-2418: CARGO_HOME is deliberately absent. The host compiles its
    // registry sources and runs `~/.cargo/bin` from PATH, so a lane that could
    // write there could plant code the host later runs. It is bound read-only
    // instead ([`os_wrap_cargo_home_ro_args`]).
    if let Some(home) = crate::home_dir() {
        rw_paths.extend(os_wrap_home_rw_paths(&home));
    }
    Ok(rw_paths)
}

/// The read-write binds under the operator's home directory: the npm cache
/// and Claude Code's own state. Never CARGO_HOME (BUG-2418).
// trace:BUG-2418 | ai:claude
pub(crate) fn os_wrap_home_rw_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".npm"),
        home.join(".claude"),
        home.join(".claude.json"),
    ]
}

/// Cargo's home directory as the host's cargo resolves it: `$CARGO_HOME`, or
/// `~/.cargo` when unset.
// trace:BUG-2418 | ai:claude
pub(crate) fn cargo_home_dir() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home_dir().map(|home| home.join(".cargo")))
}

/// BUG-2418: bind `cargo_home` READ-ONLY. Appended after every read-write
/// bind, so it wins even when a broader read-write root covers it, and in the
/// strict read-allowlist mode it keeps the toolchain and registry readable.
/// `--ro-bind-try`: a host without the directory skips it.
// trace:BUG-2418 | ai:claude
pub(crate) fn os_wrap_cargo_home_ro_args(cargo_home: &Path) -> Vec<String> {
    let path = cargo_home.to_string_lossy().into_owned();
    vec!["--ro-bind-try".to_string(), path.clone(), path]
}

/// BUG-2418: the one line a contained lane sees when cargo fails because
/// CARGO_HOME is read-only there (a new crate cannot be downloaded).
// trace:BUG-2418 | ai:claude
pub(crate) const CARGO_HOME_READ_ONLY_NOTE: &str =
    "new dependencies are fetched on the host: run cargo fetch outside the lane";

/// [`CARGO_HOME_READ_ONLY_NOTE`] when `cargo_home` exists but this process
/// cannot write to it (a read-only bind, or no write permission); `None` when
/// it is writable or absent. Probes by creating and removing a temporary file,
/// so call it only after cargo has already failed.
// trace:BUG-2418 | ai:claude
pub(crate) fn cargo_home_read_only_note(cargo_home: &Path) -> Option<&'static str> {
    if !cargo_home.is_dir() {
        return None;
    }
    match tempfile::Builder::new()
        .prefix(".aida-write-probe")
        .tempfile_in(cargo_home)
    {
        Ok(_) => None,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            Some(CARGO_HOME_READ_ONLY_NOTE)
        }
        Err(_) => None,
    }
}

/// Explicit runtime preparation; never called by path/settings renderers.
// trace:BUG-2218 TASK-2327 TASK-2328 | ai:codex
pub(crate) fn prepare_contained_mailbox_for_launch(
    host_main: Option<&Path>,
    worktree: &Path,
) -> Result<()> {
    if let Some(root) = contained_session_write_roots_from(worktree).first() {
        let main = crate::main_worktree_root_from(root);
        verify_linked_worktree_main(host_main, root, &main)?;
        crate::contained_boundary::prepare_mailbox(&main)?;
        // Best-effort: a launcher that cannot create it (a contained parent
        // whose own sandbox did not bind it) leaves the path missing, which
        // the sandbox skips; `aida deputy submit` then says no deputy is set
        // up. It never widens the boundary and never blocks the launch.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
        let _ = crate::contained_boundary::prepare_deputy_incoming(&main);
    }
    Ok(())
}

/// `--setenv` pairs for a seat clone the host recorded: the same build
/// environment its `--settings` carry ([`crate::seat_clone::seat_build_env`]),
/// so a build the OS layer runs never reaches the shared sccache either.
/// Empty for any other tree.
// trace:BUG-2169 | ai:claude
pub(crate) fn os_wrap_seat_env_args(host_main: Option<&Path>, worktree_root: &Path) -> Vec<String> {
    let Some(clone) = crate::seat_clone::recorded_clone_for(host_main, worktree_root) else {
        return Vec::new();
    };
    crate::seat_clone::seat_build_env(&clone)
        .into_iter()
        .flat_map(|(key, value)| ["--setenv".to_string(), key.to_string(), value])
        .collect()
}

/// The single resolver for the PROGRAM a HEADLESS agent spawn actually invokes.
/// Every headless spawn site (drain phase, headless resume, advisor tier, the
/// `claude agents --json` liveness query) passes the vendor binary name it would
/// otherwise hardcode (`claude` / `codex`) through here.
///
/// - `AIDA_AGENT_CMD` unset (or empty/whitespace) → returns the vendor program
///   unchanged, so an un-configured drain is byte-identical to today. This is the
///   faithful-launcher invariant — the override is the ONE explicit opt-in, no
///   hidden behavior change.
/// - `AIDA_AGENT_CMD` set → returns that program in place of the vendor binary.
///   The caller passes the ORIGINAL argv (all flags + the brief/prompt positional)
///   through unchanged, so a mock substitute receives exactly what the real vendor
///   CLI would. Per-vendor overrides are intentionally NOT supported — one redirect
///   is all the mock substrate needs (PRIN-2).
///
/// Interactive TTY launches are out of scope — they resolve their program
/// independently and are not routed through this resolver.
// trace:TASK-1081
pub(crate) fn resolve_agent_program(vendor_program: &str) -> String {
    match std::env::var("AIDA_AGENT_CMD") {
        Ok(cmd) if !cmd.trim().is_empty() => cmd,
        _ => vendor_program.to_string(),
    }
}

/// Compose the program + argv to actually exec for a headless `claude` launch,
/// applying the STORY-612 OS-boundary wrapper when `[contained] os_wrap` is on.
///
/// - os_wrap OFF (default) → `("claude", claude_args)` unchanged.
/// - os_wrap ON  → `("bwrap", [confinement-flags…, "claude", claude_args…])`,
///   binding the worktree + store + cargo/npm/claude caches rw, everything else
///   ro. Errors (fail-closed) if `bwrap` is not on PATH.
///
/// TASK-1081: both callers are HEADLESS (`exec_claude_headless`,
/// `spawn_claude_headless_resume`), so the wrapped program is routed through
/// `resolve_agent_program` — an `AIDA_AGENT_CMD` override swaps the vendor binary
/// for a mock while the argv is unchanged. Unset → the native `claude` as before.
///
/// `worktree_root` is the code worktree the drain runs in (its `.aida-store`
/// sibling is bound rw).
// trace:STORY-612 trace:TASK-1081 | ai:claude
fn claude_program_and_args(
    host_main: Option<&Path>,
    worktree_root: &Path,
    claude_args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    os_wrapped_program_and_args(
        host_main,
        worktree_root,
        &resolve_agent_program("claude"),
        claude_args,
    )
}

/// Generalized form of `claude_program_and_args` that wraps an ARBITRARY program
/// (the headless paths pass the bare `"claude"`; the interactive `aida agent new`
/// path passes the PATH-resolved claude binary). Same fail-closed contract:
///
/// - os_wrap OFF (default, incl. no `AIDA_OS_WRAP`) → `(program, args)` unchanged,
///   so a normal launch is byte-identical to today's behavior.
/// - os_wrap ON  → `("bwrap", [confinement-flags…, program, args…])`. Errors
///   (fail-closed) if `bwrap` is not on PATH or the userns preflight fails —
///   never launches unconfined when the OS boundary was requested.
///
/// trace:TASK-864 | ai:claude
pub(crate) fn os_wrapped_program_and_args(
    host_main: Option<&Path>,
    worktree_root: &Path,
    program: &str,
    program_args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    // trace:TASK-2707 | ai:codex — AIDA Codex is owned by containment::launch,
    // independently of the legacy optional os_wrap switch.
    if program_args
        .windows(2)
        .any(|p| p[0] == "--sandbox" && p[1] == "workspace-write")
        && Path::new(program).file_name().is_some_and(|n| n == "codex")
        && containment_posture_in(worktree_root, "codex", true)? == ContainmentPosture::Aida
    {
        return Ok((program.to_string(), program_args));
    }
    // trace:TASK-2708 | ai:claude — likewise a wrapper-owned Claude launch.
    if composing_for_claude_wrapper()
        && Path::new(program)
            .file_name()
            .is_some_and(|n| n == "claude")
    {
        return Ok((program.to_string(), program_args));
    }
    if !os_wrap_enabled(worktree_root) {
        return Ok((program.to_string(), program_args));
    }
    // BUG-2001: fail closed when no git root resolves for `worktree_root`
    // (e.g. `headless_worktree_root()` fell back to cwd / `$HOME` outside a git
    // repository). Never bind a non-repository directory read-write in bwrap.
    // trace:BUG-2001 | ai:antigravity
    let worktree_root_buf = crate::find_project_root_from(worktree_root).with_context(|| {
        format!(
            "[contained] os_wrap is enabled, but {} is not inside a git repository — \
             refusing to bind a non-repository directory read-write in bubblewrap",
            worktree_root.display()
        )
    })?;
    let worktree_root = worktree_root_buf.as_path();
    let claude_args = program_args;
    if which_on_path("bwrap").is_none() {
        anyhow::bail!(
            "[contained] os_wrap is enabled but `bwrap` (bubblewrap) was not found on PATH — \
             install bubblewrap or unset os_wrap. Refusing to launch the drain unconfined."
        );
    }
    // Fail-closed preflight: `bwrap` can be installed yet unable to set up a user
    // namespace (the Ubuntu 23.10+/24.04 AppArmor `apparmor_restrict_unprivileged_userns`
    // restriction — SPIKE-61 §7.7). Catch it here with an actionable message
    // instead of letting the drain die on a cryptic `uid map: Permission denied`.
    bwrap_preflight()?;
    let rw_paths = os_wrap_rw_paths(host_main, worktree_root)?;
    // TASK-2772: `--bind-try` skips a missing directory, which would leave a
    // first-use slot read-only inside the wrapper. Create the validated slot
    // pair (0700) first; best-effort, it never widens the boundary.
    // trace:TASK-2772 | ai:claude
    if crate::seat_clone::recorded_clone_for(host_main, worktree_root).is_none() {
        for slot in crate::contained_boundary::lane_build_write_paths() {
            if rw_paths.contains(&slot) {
                let _ = create_private_dir_all(&slot);
            }
        }
    }
    // STORY-617: opt-in strict read-confinement. Default-ABSENT => empty =>
    // `bwrap_confinement_args_inner` falls back to the unchanged `--ro-bind / /`.
    let read_allowlist = contained_read_allowlist(worktree_root);
    let mut full = bwrap_confinement_args_inner(worktree_root, &rw_paths, &read_allowlist);
    // trace:BUG-2418 | ai:claude
    if let Some(cargo_home) = cargo_home_dir() {
        full.extend(os_wrap_cargo_home_ro_args(&cargo_home));
    }
    // trace:BUG-2169 | ai:claude
    full.extend(os_wrap_seat_env_args(host_main, worktree_root));
    // TASK-809: when `[contained] managed_domains_only` is on, deliver the hard
    // default-deny egress via the MANAGED-settings tier — Claude Code only
    // blocks-without-prompt when the flag arrives there, not via the project
    // `--settings` STORY-615 emits. We write the managed doc under `.aida/`
    // (gitignored runtime state) and bind it READ-ONLY over the wrapped
    // process's `/etc/claude-code/managed-settings.json`, so the host's `/etc`
    // is never touched and the policy can never be overridden from inside the
    // sandbox. trace:TASK-809 | ai:claude
    if contained_managed_domains_only(worktree_root) {
        let allowed_hosts = contained_allowed_hosts(worktree_root);
        let json = managed_settings_json(&allowed_hosts);
        let aida_dir = worktree_root.join(".aida");
        std::fs::create_dir_all(&aida_dir).with_context(|| {
            format!(
                "failed to create {} for managed settings",
                aida_dir.display()
            )
        })?;
        let managed_path = aida_dir.join("contained-managed-settings.json");
        std::fs::write(&managed_path, &json).with_context(|| {
            format!(
                "failed to write contained managed settings {}",
                managed_path.display()
            )
        })?;
        // `--ro-bind` (hard, not -try): if managed_domains_only is requested we
        // MUST get the policy in front of claude — fail-closed if the bind can't
        // be set up rather than silently launching with egress un-hard-blocked.
        full.push("--ro-bind".to_string());
        full.push(managed_path.to_string_lossy().into_owned());
        full.push("/etc/claude-code/managed-settings.json".to_string());
    }
    // Pin the executable boundary for bwrap and the Codex child-env adapter.
    // Keep this separator for every vendor, including native Codex launches
    // when an outer OS wrapper was requested.
    // trace:BUG-2440 | ai:codex
    full.push("--".to_string());
    full.push(program.to_string());
    full.extend(claude_args);
    Ok(("bwrap".to_string(), full))
}

/// BUG-2383: wrap one lane job (a seat's `cargo test|check|build`) in its own
/// minimal bubblewrap profile. A job runs code the seat wrote (build scripts,
/// tests, a seat-written `.cargo/config.toml` runner), so it never runs
/// unsandboxed and never gets the seat's wider launch profile (`~/.claude`,
/// the store, the mailbox): the filesystem is read-only except `seat` and the
/// `rw` paths the caller names (the seat's cargo target and the slot
/// limiter's state; never cargo home). Fails closed, with the remediation, when `[contained]
/// os_wrap` is off for the seat or bubblewrap cannot run. `os_wrap_on` is the
/// lane's own decision (the seat's setting, or the BUG-2434 lane default),
/// passed in rather than read back from this process's environment.
// trace:BUG-2383 | ai:claude
// trace:BUG-2434 | ai:claude
pub(crate) fn lane_job_wrapped_program_and_args(
    seat: &Path,
    os_wrap_on: bool,
    rw: &[PathBuf],
    program: &str,
    args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    if !os_wrap_on {
        anyhow::bail!(
            "a lane job runs only inside bubblewrap, and `[contained] os_wrap` is off for this \
             seat. Turn it on (`[contained] os_wrap = true` in .aida/config.toml, or \
             AIDA_OS_WRAP=1 for the launcher), with bubblewrap installed ({BWRAP_INSTALL_DEBIAN}). \
             Nothing ran; report the work as untested (untested=true)."
        );
    }
    let root = crate::find_project_root_from(seat).with_context(|| {
        format!(
            "{} is not inside a git repository; refusing to bind it read-write in bubblewrap",
            seat.display()
        )
    })?;
    if which_on_path("bwrap").is_none() {
        anyhow::bail!(
            "a lane job runs only inside bubblewrap, and `bwrap` is not on PATH \
             ({BWRAP_INSTALL_DEBIAN}). Nothing ran; report the work as untested (untested=true)."
        );
    }
    bwrap_preflight()?;
    let read_allowlist = contained_read_allowlist(&root);
    Ok((
        "bwrap".to_string(),
        lane_job_bwrap_argv(&root, rw, &read_allowlist, program, args),
    ))
}

/// The pure argv of [`lane_job_wrapped_program_and_args`], after `bwrap`.
// trace:BUG-2383 | ai:claude
pub(crate) fn lane_job_bwrap_argv(
    seat: &Path,
    rw: &[PathBuf],
    read_allowlist: &[String],
    program: &str,
    args: Vec<String>,
) -> Vec<String> {
    let mut full = bwrap_confinement_args_inner(seat, rw, read_allowlist);
    // Keep host PID identity: the shared Cargo slot queue checks ticket
    // liveness with kill -0 and names tickets using the wrapper's PID.
    // trace:STORY-2641 | ai:codex
    full.push(program.to_string());
    full.extend(args);
    full
}

/// `mkdir -p` with mode 0700 for every directory it creates.
// trace:TASK-2772 | ai:claude
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Run a trivial `bwrap … true` to confirm bubblewrap can actually create the
/// user namespace + uid map on this host BEFORE wrapping the real drain. On
/// failure (typically the AppArmor unprivileged-userns restriction) bail with
/// the remediation rather than letting the drain hit a cryptic bwrap error —
/// and never fall through to an unconfined launch. trace:STORY-612 | ai:claude
fn bwrap_preflight() -> Result<()> {
    use std::process::{Command, Stdio};
    let ran = Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "true",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match ran {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!(
            "[contained] os_wrap is enabled and `bwrap` is installed, but a sandbox self-test \
             failed — the kernel is refusing the unprivileged user namespace bwrap needs. On \
             Ubuntu 23.10+/24.04 this is AppArmor's unprivileged-userns restriction. Remediate \
             with ONE of:\n  \
             - sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   (host-wide)\n  \
             - install an AppArmor profile granting bwrap userns (see /etc/apparmor.d)\n\
             Refusing to launch the drain unconfined — unset [contained] os_wrap to opt out."
        ),
    }
}

/// Availability of the bubblewrap (`bwrap`) OS sandbox on this host, as
/// reported by `aida doctor` / `aida init`. A read-only probe — it does NOT
/// enable `os_wrap` (the `[contained] os_wrap` config knob, exposed separately
/// by TASK-866); it only tells the user whether userns confinement *could* be
/// used. trace:TASK-865 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BwrapAvailability {
    /// `bwrap` is on PATH and the userns self-test passes.
    Ok,
    /// `bwrap` is not installed / not on PATH.
    NotInstalled,
    /// `bwrap` is installed but the kernel refuses the unprivileged user
    /// namespace it needs — carries the remediation hint from `bwrap_preflight`.
    UsernsBlocked { hint: String },
}

/// Probe whether the bubblewrap OS sandbox is available on this host: is
/// `bwrap` on PATH, and does the trivial userns self-test pass? Reuses
/// `which_on_path` + `bwrap_preflight` so the doctor/init report matches what
/// the launch path actually checks. Read-only — never enables confinement.
/// trace:TASK-865 | ai:claude
pub(crate) fn bwrap_availability() -> BwrapAvailability {
    if which_on_path("bwrap").is_none() {
        return BwrapAvailability::NotInstalled;
    }
    match bwrap_preflight() {
        Ok(()) => BwrapAvailability::Ok,
        Err(e) => BwrapAvailability::UsernsBlocked {
            hint: bwrap_userns_remediation_hint(&e.to_string()),
        },
    }
}

/// Distil `bwrap_preflight`'s long fail-closed message down to the one-line
/// remediation a doctor/init status row wants. Falls back to the full text if
/// the expected `Remediate with ONE of:` marker is absent. trace:TASK-865 | ai:claude
fn bwrap_userns_remediation_hint(full: &str) -> String {
    // The preflight message lists remediations after a "Remediate with ONE of:"
    // marker; surface the first concrete `sysctl` line as the short hint.
    if let Some(rest) = full.split("Remediate with ONE of:").nth(1) {
        for line in rest.lines() {
            // The source lines are `  - sudo sysctl … (host-wide)\n  ` — strip
            // the leading bullet, the trailing soft-wrap whitespace, and any
            // parenthetical aside so the one-liner is a clean copy-paste hint.
            let trimmed = line.trim().trim_start_matches('-').trim();
            let trimmed = trimmed.split("   ").next().unwrap_or(trimmed).trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "the kernel is refusing the unprivileged user namespace bwrap needs (on \
     Ubuntu 23.10+/24.04, AppArmor's apparmor_restrict_unprivileged_userns); \
     run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` or \
     install an AppArmor profile granting bwrap userns"
        .to_string()
}

/// The exact command that lifts the kernel's unprivileged-userns restriction
/// for the CURRENT boot (does not survive a reboot). Single source of truth so
/// the doctor remediation, the guided setup printer, and the docs can't drift.
/// trace:STORY-665 | ai:claude
pub(crate) const BWRAP_USERNS_SYSCTL_RUNTIME: &str =
    "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0";

/// The command that PERSISTS the userns sysctl across reboots via a
/// `/etc/sysctl.d` drop-in. trace:STORY-665 | ai:claude
pub(crate) const BWRAP_USERNS_SYSCTL_PERSIST: &str =
    "echo 'kernel.apparmor_restrict_unprivileged_userns=0' \
     | sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf";

/// The Debian/Ubuntu install command for bubblewrap. trace:STORY-665 | ai:claude
pub(crate) const BWRAP_INSTALL_DEBIAN: &str = "sudo apt install bubblewrap";

/// Minimal PATH lookup for an executable name (avoids pulling in a `which`
/// crate). Returns the first matching path, or `None`. trace:STORY-612 | ai:claude
fn which_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

// trace:TASK-2631 | ai:codex
pub(crate) fn stamp_containment_posture(
    cmd: &mut std::process::Command,
    posture: ContainmentPosture,
) {
    cmd.env(
        "AIDA_CONTAINMENT_POSTURE",
        match posture {
            ContainmentPosture::Aida => "aida",
            ContainmentPosture::Native => "native",
            ContainmentPosture::None => "none",
        },
    );
}
