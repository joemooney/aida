//! Scheduler drivers for `aida schedule tick`: the systemd user timer, the
//! rules for switching between it and the crontab entry, and the two-driver
//! status `aida doctor` and `aida shift status` read (STORY-1218 slice 2).
//!
//! The binding advisor rules this module implements:
//!
//! - A12 (unit shape): `Type=oneshot`, `KillMode=process`, `OnActiveSec` +
//!   `OnBootSec` first-fire triggers (`OnUnitInactiveSec` alone never fires
//!   the first time), no `Persistent=` (it only applies to `OnCalendar`),
//!   `TimeoutStartSec=15min` as a backstop, and NO resource limits on the
//!   TICK unit (they would silently constrain a wave left behind in the
//!   cgroup; a wave's OWN unit does carry limits — see "Wave limits"). Under this
//!   driver a drain wave normally runs in its OWN transient unit
//!   (`aida-wave-*.service`, started with `systemd-run --user`; see "Wave
//!   units" below), outside the tick's cgroup. `KillMode=process` remains for
//!   the detached fallback: when the wave cannot get its own unit it is
//!   launched in the tick's cgroup, and without it systemd would kill that
//!   wave when the oneshot exits. The tick unit files are unchanged by the
//!   wave units.
//! - A13 (switch): install and verify the new driver before removing the old
//!   one, so a failure part-way leaves both (harmless under the tick lock,
//!   flagged by doctor) and never none; cron removal goes through
//!   `crontab_after_driver_switch` (this repo's ACTIVE marked and legacy lines
//!   only); systemd removal only runs `disable --now` on the `.timer`, never
//!   touches the `.service` unit's run state, and deletes a unit file only if
//!   it carries our marker; a same-named file without the marker is never
//!   overwritten; never sudo, only a linger reminder.
//!
//! Every process and filesystem side effect goes through [`DriverHost`], so
//! tests run against an in-memory crontab, a fake `systemctl` and a unit
//! directory under a temporary HOME. The real host refuses to call
//! `systemctl` or `systemd-run`, or resolve the real unit directory, under
//! `cfg(test)`.
//!
//! Wave units: the builders here are pure. They never add a timer flag
//! (`--on-calendar`, `--on-active`, ...), a `Restart=` or any other way for
//! systemd to start a wave on its own; only a guarded tick launches one.
//!
//! Wave limits: a wave unit carries `MemoryHigh`/`MemoryMax`, a lowered
//! `CPUWeight`/`IOWeight` and `TasksMax`, so a runaway wave is throttled and
//! then killed inside its own cgroup instead of pushing the whole login
//! session into a systemd-oomd kill. The limits only constrain a wave: they
//! add no way to start one. Bad configuration fails closed (the wave-limits
//! guard refuses the launch); missing cgroup delegation fails open (a doctor
//! warning, and the wave still runs).
//!
//! trace:TASK-1491 | ai:claude
//! trace:TASK-1510 | ai:claude
//! trace:TASK-1517 | ai:claude

use crate::maintenance_schedule::{
    classify_cron_driver, crontab_after_driver_switch, crontab_after_install,
    crontab_has_repair_target, render_tick_cron_line, tick_cron_marker, tick_invocation,
    CronDriverStatus, DriverInstallOutcome, TickInvocation,
};
use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long after the previous tick finished the timer fires the next one.
pub(crate) const SYSTEMD_TICK_EVERY: &str = "10min";

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// Result of one `systemctl --user` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Every side effect a driver install, removal or status read performs.
// trace:TASK-1491 | ai:claude
pub(crate) trait DriverHost {
    /// `crontab -l`: `Ok(None)` when the user has no crontab.
    fn read_crontab(&mut self) -> Result<Option<String>>;
    /// `crontab -` with `body`.
    fn write_crontab(&mut self, body: &str) -> Result<()>;
    /// `systemctl --user <args>`. `Err` only when it could not be run.
    fn systemctl_user(&mut self, args: &[&str]) -> Result<CommandOutput>;
    /// The user unit directory (`$XDG_CONFIG_HOME/systemd/user`).
    fn unit_dir(&self) -> Result<PathBuf>;
    /// Whether the user has linger on. `None` when it cannot be told.
    fn linger_enabled(&mut self) -> Option<bool>;
    /// Whether systemd user timers exist on this platform at all.
    fn systemd_supported(&self) -> bool;
    /// `systemd-run <args>` (the args start with `--user`), killed after
    /// `timeout`. Only the night-shift wave launch calls it. A host that
    /// does not implement it never runs anything: the wave falls back to
    /// the detached launch.
    // trace:TASK-1510 | ai:claude
    fn systemd_run_user(&mut self, _args: &[String], _timeout: Duration) -> BoundedRun {
        BoundedRun::SpawnFailed("this host does not run systemd-run".to_string())
    }
    /// `systemctl --user <args>`, killed after `timeout`. The wave launch
    /// uses it only for the read-only `show` probe.
    // trace:TASK-1510 | ai:claude
    fn systemctl_user_bounded(&mut self, args: &[&str], _timeout: Duration) -> BoundedRun {
        match self.systemctl_user(args) {
            Ok(out) => BoundedRun::Exited(out),
            Err(e) => BoundedRun::SpawnFailed(format!("{e:#}")),
        }
    }
    /// The contents of `/proc/self/cgroup`; `None` when it cannot be read.
    // trace:TASK-1510 | ai:claude
    fn self_cgroup(&self) -> Option<String> {
        None
    }
    /// The `cgroup.controllers` of the systemd user manager's own cgroup:
    /// what a transient wave unit can actually be limited by. `None` when it
    /// cannot be read, which doctor reports as unknown, never as ok.
    // trace:TASK-1517 | ai:claude
    fn cgroup_controllers(&self) -> Option<String> {
        None
    }
}

/// How a bounded `systemd-run` / `systemctl` call ended.
// trace:TASK-1510 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundedRun {
    /// The command ran and exited.
    Exited(CommandOutput),
    /// The command ran past its timeout and was killed. What it did before
    /// that is unknown.
    TimedOut,
    /// The command could not be started at all (missing binary, refused).
    SpawnFailed(String),
}

/// The real host: the user's crontab, `systemctl --user`, and the unit
/// directory under the real HOME. Linux-only parts sit behind `cfg`.
pub(crate) struct RealDriverHost;

impl DriverHost for RealDriverHost {
    fn read_crontab(&mut self) -> Result<Option<String>> {
        if cfg!(windows) {
            anyhow::bail!("no crontab on Windows");
        }
        crate::maintenance_schedule::read_crontab()
    }

    fn write_crontab(&mut self, body: &str) -> Result<()> {
        if cfg!(test) {
            anyhow::bail!("refusing to write the real crontab from a test");
        }
        crate::maintenance_schedule::write_crontab(body)
    }

    fn systemctl_user(&mut self, args: &[&str]) -> Result<CommandOutput> {
        real_systemctl_user(args)
    }

    fn unit_dir(&self) -> Result<PathBuf> {
        if cfg!(test) {
            anyhow::bail!("the real systemd user unit directory is not used from a test");
        }
        user_unit_dir_from(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            crate::home_dir(),
        )
        .context("could not resolve the home directory for the systemd user unit directory")
    }

    fn linger_enabled(&mut self) -> Option<bool> {
        real_linger_enabled()
    }

    fn systemd_supported(&self) -> bool {
        cfg!(target_os = "linux")
    }

    // trace:TASK-1510 | ai:claude
    fn systemd_run_user(&mut self, args: &[String], timeout: Duration) -> BoundedRun {
        if cfg!(test) {
            return BoundedRun::SpawnFailed(
                "refusing to run the real `systemd-run` from a test".to_string(),
            );
        }
        if !cfg!(target_os = "linux") {
            return BoundedRun::SpawnFailed("systemd-run is only available on Linux".to_string());
        }
        if args.first().map(String::as_str) != Some("--user") {
            return BoundedRun::SpawnFailed(
                "refusing a systemd-run call that is not `--user`".to_string(),
            );
        }
        let mut cmd = std::process::Command::new("systemd-run");
        cmd.args(args);
        run_bounded(cmd, timeout)
    }

    // trace:TASK-1510 | ai:claude
    fn systemctl_user_bounded(&mut self, args: &[&str], timeout: Duration) -> BoundedRun {
        if cfg!(test) {
            return BoundedRun::SpawnFailed(
                "refusing to run the real `systemctl --user` from a test".to_string(),
            );
        }
        if !cfg!(target_os = "linux") {
            return BoundedRun::SpawnFailed("systemctl is only available on Linux".to_string());
        }
        let mut cmd = std::process::Command::new("systemctl");
        cmd.arg("--user").args(args);
        run_bounded(cmd, timeout)
    }

    // trace:TASK-1510 | ai:claude
    fn self_cgroup(&self) -> Option<String> {
        if cfg!(target_os = "linux") {
            std::fs::read_to_string("/proc/self/cgroup").ok()
        } else {
            None
        }
    }

    // trace:TASK-1517 | ai:claude — a read-only look at two files under
    // /sys/fs/cgroup; it never writes and never starts anything.
    fn cgroup_controllers(&self) -> Option<String> {
        let path = user_manager_cgroup_path(&self.self_cgroup()?)?;
        std::fs::read_to_string(format!("/sys/fs/cgroup{path}/cgroup.controllers")).ok()
    }
}

/// Run `cmd` with piped output, killing it after `timeout`. A spawn failure
/// and a timeout are told apart: the first proves nothing ran.
// trace:TASK-1510 | ai:claude
fn run_bounded(mut cmd: std::process::Command, timeout: Duration) -> BoundedRun {
    use std::io::Read;
    use std::process::Stdio;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return BoundedRun::SpawnFailed(e.to_string()),
    };
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .map(|pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).to_string()
        })
    })
    .collect();
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let mut texts = readers.into_iter().map(|h| h.join().unwrap_or_default());
    let stdout = texts.next().unwrap_or_default();
    let stderr = texts.next().unwrap_or_default();
    match status {
        Some(s) => BoundedRun::Exited(CommandOutput {
            success: s.success(),
            stdout,
            stderr,
        }),
        None => BoundedRun::TimedOut,
    }
}

#[cfg(target_os = "linux")]
fn real_systemctl_user(args: &[&str]) -> Result<CommandOutput> {
    if cfg!(test) {
        anyhow::bail!("refusing to run the real `systemctl --user` from a test");
    }
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("failed to run `systemctl --user {}`", args.join(" ")))?;
    Ok(CommandOutput {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

#[cfg(not(target_os = "linux"))]
fn real_systemctl_user(_args: &[&str]) -> Result<CommandOutput> {
    anyhow::bail!("systemd user timers are only available on Linux")
}

#[cfg(target_os = "linux")]
fn real_linger_enabled() -> Option<bool> {
    if cfg!(test) {
        return None;
    }
    // SAFETY: getuid never fails and has no preconditions.
    let uid = unsafe { libc::getuid() };
    let out = std::process::Command::new("loginctl")
        .args([
            "show-user",
            &uid.to_string(),
            "--property=Linger",
            "--value",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
fn real_linger_enabled() -> Option<bool> {
    None
}

/// PURE: `$XDG_CONFIG_HOME/systemd/user`, else `~/.config/systemd/user`.
pub(crate) fn user_unit_dir_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match xdg.filter(|p| p.is_absolute()) {
        Some(x) => x,
        None => home?.join(".config"),
    };
    Some(base.join("systemd").join("user"))
}

// ---------------------------------------------------------------------------
// Unit builders (A12)
// ---------------------------------------------------------------------------

/// The two unit files for one repo, plus the marker they both carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemdUnits {
    pub service_name: String,
    pub timer_name: String,
    pub service: String,
    pub timer: String,
    pub marker: String,
}

/// `aida-tick-<first 8 hex of sha256(canonical repo path)>`. Stable across
/// builds and platforms (not `DefaultHasher`), and distinct per clone.
// trace:TASK-1491 | ai:claude
pub(crate) fn unit_stem(repo: &str) -> String {
    format!("aida-tick-{}", repo_hash8(repo))
}

/// First 8 hex digits of sha256(canonical repo path), shared by the tick
/// unit and the wave units of one repo.
// trace:TASK-1510 | ai:claude
pub(crate) fn repo_hash8(repo: &str) -> String {
    let digest = Sha256::digest(repo.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// `(service, timer)` file names for a canonical repo path.
pub(crate) fn unit_names(repo: &str) -> (String, String) {
    let stem = unit_stem(repo);
    (format!("{stem}.service"), format!("{stem}.timer"))
}

/// Quote one value for `ExecStart=` / `Environment=`: double quotes with
/// C-style escapes for `\` and `"`. `ExecStart=` also expands `$VAR`, so
/// there a literal `$` is written `$$`. `%` never reaches here
/// (`tick_invocation` refuses it).
fn systemd_quote(value: &str, in_exec: bool) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' if in_exec => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// PURE: render the `.service` + `.timer` for the shared tick invocation.
/// Same repo, PATH, binary, argv and marker as the crontab line
/// (`render_tick_cron_line`); only the invoker tag differs.
// trace:TASK-1491 | ai:claude
pub(crate) fn build_systemd_units(inv: &TickInvocation) -> SystemdUnits {
    let (service_name, timer_name) = unit_names(&inv.repo);
    let exec = std::iter::once(systemd_quote(&inv.exe, true))
        .chain(inv.args.iter().map(|a| (*a).to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let header = format!(
        "# {marker}\n\
         # Managed by `aida schedule install-systemd`; remove it with `aida schedule uninstall-systemd`.\n",
        marker = inv.marker
    );
    let service = format!(
        "{header}\
         # KillMode=process keeps a drain wave the tick launched running after the tick\n\
         # exits. The journal then logs a \"left-over process\" line for it; that is expected.\n\
         [Unit]\n\
         Description=AIDA scheduler tick for {repo}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         WorkingDirectory={repo}\n\
         Environment={path} {invoker}\n\
         ExecStart={exec}\n\
         KillMode=process\n\
         TimeoutStartSec=15min\n\
         StandardOutput=journal\n\
         StandardError=journal\n",
        repo = inv.repo,
        path = systemd_quote(&format!("PATH={}", inv.path_env), false),
        invoker = systemd_quote("AIDA_SCHEDULE_INVOKER=systemd", false),
    );
    let timer = format!(
        "{header}\
         [Unit]\n\
         Description=Run the AIDA scheduler tick for {repo} every {every}\n\
         \n\
         [Timer]\n\
         OnActiveSec=1min\n\
         OnBootSec=2min\n\
         OnUnitInactiveSec={every}\n\
         Unit={service_name}\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        repo = inv.repo,
        every = SYSTEMD_TICK_EVERY,
    );
    SystemdUnits {
        service_name,
        timer_name,
        service,
        timer,
        marker: inv.marker.clone(),
    }
}

/// Whether a unit file carries exactly our `# <marker>` line. Whole-line
/// equality, so a repo whose path is a prefix of this one never matches.
pub(crate) fn unit_has_marker(content: &str, marker: &str) -> bool {
    let want = format!("# {marker}");
    content.lines().any(|l| l.trim_end() == want)
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// The temporary file `write_atomic` renames over `path`: one per target
/// (`with_extension` gave the `.service` and `.timer` the same one).
// trace:BUG-1619 | ai:claude
fn atomic_tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".aida-tmp");
    path.with_file_name(name)
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp = atomic_tmp_path(path);
    std::fs::write(&tmp, content).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Leave no `.aida-tmp` behind when the rename fails.
        // trace:BUG-1619 | ai:claude
        let _ = std::fs::remove_file(&tmp);
        anyhow::Error::new(e).context(format!("failed to write {}", path.display()))
    })
}

fn systemctl_ok(host: &mut dyn DriverHost, args: &[&str]) -> Result<CommandOutput> {
    let out = host.systemctl_user(args)?;
    if !out.success {
        anyhow::bail!(
            "`systemctl --user {}` failed: {}",
            args.join(" "),
            out.stderr.trim()
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Install / remove (A13)
// ---------------------------------------------------------------------------

/// Why a driver install reported [`DriverInstallOutcome::Repaired`]. More
/// than one can hold (rewritten files on a timer that was also disabled).
// trace:BUG-1619 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RepairCauses {
    /// What was rewritten because it ran an older invocation (unit file
    /// names, or "the crontab entry").
    pub rewrote: Vec<String>,
    /// The timer was disabled and has been enabled again.
    pub re_enabled: bool,
    /// The timer was enabled but not running and has been started.
    pub restarted: bool,
}

impl RepairCauses {
    /// PURE: the clause after "Repaired this repo's scheduler <what>: ".
    // trace:BUG-1619 | ai:claude
    pub(crate) fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.rewrote.is_empty() {
            parts.push(format!(
                "rewrote {} (it ran an older invocation)",
                self.rewrote.join(" and ")
            ));
        }
        if self.re_enabled {
            parts.push("re-enabled and started the timer (it was disabled)".to_string());
        } else if self.restarted {
            parts.push("started the timer (it was enabled but not running)".to_string());
        }
        if parts.is_empty() {
            "enabled and started the timer (its earlier state could not be read)".to_string()
        } else {
            parts.join("; ")
        }
    }
}

/// What [`install_systemd_units`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemdInstall {
    pub outcome: DriverInstallOutcome,
    /// Set when `outcome` is `Repaired`.
    pub repair: RepairCauses,
}

/// What a failed [`install_systemd_units`] left in the unit directory.
// trace:BUG-1619 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnitFilesAfterFailure {
    /// No unit file was written (the existing files already matched, or
    /// the first write failed).
    Unchanged,
    /// A fresh install was undone: the timer disabled (when it had been
    /// enabled) and every file carrying our marker removed.
    CleanedUp {
        /// `enable` had been attempted, so the cleanup ran `disable --now`.
        /// False when a write failed first: nothing was enabled.
        // trace:BUG-1619 | ai:claude
        timer_enabled: bool,
        removed: Vec<String>,
        /// Files this install wrote that could not be removed.
        left: Vec<String>,
        /// `disable --now` failed during the cleanup.
        disable_error: Option<String>,
    },
    /// An existing install's files were rewritten before the failure and
    /// left in place (a failed repair never tears down an existing install).
    Rewritten(Vec<String>),
}

/// The error context of a failed systemd install: what happened to the
/// unit files. `aida shift install` downcasts to it to word its summary.
// trace:BUG-1619 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemdInstallFailure {
    pub timer_name: String,
    pub files: UnitFilesAfterFailure,
}

impl std::fmt::Display for SystemdInstallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let timer = &self.timer_name;
        match &self.files {
            UnitFilesAfterFailure::Unchanged => {
                write!(f, "could not install {timer}; no unit file was changed")?
            }
            UnitFilesAfterFailure::CleanedUp {
                removed,
                left,
                disable_error,
                ..
            } => {
                write!(f, "could not install {timer}; ")?;
                if let Some(e) = disable_error {
                    write!(f, "disabling it failed ({e}); ")?;
                }
                if removed.is_empty() {
                    write!(f, "none of the unit files this install wrote were removed")?;
                } else {
                    write!(
                        f,
                        "removed the unit files this install wrote ({})",
                        removed.join(", ")
                    )?;
                }
                if !left.is_empty() {
                    write!(
                        f,
                        "; could not remove {} (delete it with `aida schedule uninstall-systemd`)",
                        left.join(", ")
                    )?;
                }
            }
            UnitFilesAfterFailure::Rewritten(names) => write!(
                f,
                "could not finish repairing {timer}; the rewritten unit files ({}) were left in \
                 place",
                names.join(", ")
            )?,
        }
        write!(f, ". Any other driver for this repo was left in place.")
    }
}

/// Write (or repair) both unit files, enable and start the timer, then
/// verify with `systemctl --user is-enabled` and `is-active`. Refuses, before
/// writing anything, when a file with our name exists without our marker, and
/// on a fresh install when systemd already knows the timer from another
/// directory or cannot be asked (BUG-1619).
/// A fresh install that fails at any later step (writing the second file,
/// `daemon-reload`, `enable`, `start`, or the verify) disables the timer
/// and removes the files carrying our marker, so nothing is orphaned. A
/// failed repair of an existing install leaves its files in place. Every
/// failure carries a [`SystemdInstallFailure`] context.
// trace:TASK-1491 | ai:claude
// trace:BUG-1619 | ai:claude
pub(crate) fn install_systemd_units(
    host: &mut dyn DriverHost,
    units: &SystemdUnits,
) -> Result<SystemdInstall> {
    let dir = host.unit_dir()?;
    let files = [
        (&units.service_name, &units.service),
        (&units.timer_name, &units.timer),
    ];
    let mut current = Vec::with_capacity(files.len());
    for (name, _) in files {
        let path = dir.join(name);
        let cur = read_optional(&path)?;
        if let Some(c) = &cur {
            if !unit_has_marker(c, &units.marker) {
                anyhow::bail!(
                    "{} exists but was not written by aida for this repo (no `# {}` line). \
                     Refusing to overwrite it; move it aside and run the install again.",
                    path.display(),
                    units.marker
                );
            }
        }
        current.push(cur);
    }
    let none_existed = current.iter().all(Option::is_none);
    // A fresh install (neither file in our unit directory) goes ahead only
    // when systemd does not know the timer at all. If it is known, a unit
    // with our name was loaded from another directory (an `XDG_CONFIG_HOME`
    // mismatch, `~/.local/share/systemd/user`, ...), and enabling or, on a
    // failure, disabling that name would act on that other install. So
    // refuse before writing or disabling anything.
    // trace:BUG-1619 | ai:claude
    if none_existed {
        let probe = match host.systemctl_user(&["is-enabled", &units.timer_name]) {
            Ok(out) => classify_fresh_probe(&out, &units.timer_name),
            Err(e) => FreshProbe::Unknown(format!("{e:#}")),
        };
        let refusal = match probe {
            FreshProbe::NotFound => None,
            FreshProbe::Known(state) => Some(format!(
                // trace:BUG-1619 | ai:claude
                "systemd already knows {timer} (`systemctl --user is-enabled` reports \
                 {state:?}), but its unit file is not in {dir}. Another install of this repo's \
                 timer is loaded from a different unit directory (for example a different \
                 XDG_CONFIG_HOME, or ~/.local/share/systemd/user). Refusing to install over it; \
                 check `systemctl --user status {timer}` and remove the other copy first",
                timer = units.timer_name,
                dir = dir.display(),
            )),
            FreshProbe::Unknown(why) => Some(format!(
                // trace:BUG-1619 | ai:claude
                "could not tell whether systemd already knows {timer} ({why}). Refusing to \
                 install without that check; make sure the systemd user manager is reachable \
                 (`systemctl --user status`) and run the install again",
                timer = units.timer_name,
            )),
        };
        if let Some(msg) = refusal {
            return Err(anyhow::anyhow!(msg).context(SystemdInstallFailure {
                timer_name: units.timer_name.clone(),
                files: UnitFilesAfterFailure::Unchanged,
            }));
        }
    }
    // The timer's state before this run decides AlreadyUpToDate versus
    // Repaired and, for a repair, which cause to report.
    let prior = if none_existed {
        SystemdDriverStatus::Missing
    } else {
        timer_state(host, &units.timer_name)
    };
    let mut rewrote: Vec<String> = Vec::new();
    let mut enable_attempted = false;
    let result = (|| -> Result<()> {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        for ((name, content), cur) in files.iter().zip(&current) {
            if cur.as_deref() != Some(content.as_str()) {
                write_atomic(&dir.join(name), content)?;
                rewrote.push((*name).clone());
            }
        }
        if !rewrote.is_empty() {
            systemctl_ok(host, &["daemon-reload"])?;
        }
        enable_attempted = true;
        systemctl_ok(host, &["enable", &units.timer_name])?;
        // A rewritten timer only picks up its new settings on a restart.
        let verb = if rewrote.is_empty() {
            "start"
        } else {
            "restart"
        };
        systemctl_ok(host, &[verb, &units.timer_name])?;
        let state = timer_state(host, &units.timer_name);
        if state != SystemdDriverStatus::Installed {
            anyhow::bail!(
                "`systemctl --user is-enabled` / `is-active` report {} {}, not enabled and \
                 running",
                units.timer_name,
                state.describe()
            );
        }
        Ok(())
    })();
    if let Err(e) = result {
        let files_after = if rewrote.is_empty() {
            UnitFilesAfterFailure::Unchanged
        } else if none_existed {
            // Fresh install: undo it. Disable first so no enabled timer
            // points at a deleted unit, then remove only our marked files.
            let disable_error = if enable_attempted {
                match systemctl_ok(host, &["disable", "--now", &units.timer_name]) {
                    Ok(_) => None,
                    Err(d) => Some(format!("{d:#}")),
                }
            } else {
                None
            };
            let removed = remove_marked_files(&dir, &files.map(|(n, _)| n.as_str()), &units.marker);
            if !removed.is_empty() {
                let _ = host.systemctl_user(&["daemon-reload"]);
            }
            let left = rewrote
                .iter()
                .filter(|n| !removed.contains(n))
                .cloned()
                .collect();
            UnitFilesAfterFailure::CleanedUp {
                timer_enabled: enable_attempted,
                removed,
                left,
                disable_error,
            }
        } else {
            UnitFilesAfterFailure::Rewritten(rewrote)
        };
        return Err(e.context(SystemdInstallFailure {
            timer_name: units.timer_name.clone(),
            files: files_after,
        }));
    }
    let outcome = if none_existed {
        DriverInstallOutcome::Installed
    } else if rewrote.is_empty() && prior == SystemdDriverStatus::Installed {
        DriverInstallOutcome::AlreadyUpToDate
    } else {
        DriverInstallOutcome::Repaired
    };
    let repair = if outcome == DriverInstallOutcome::Repaired {
        RepairCauses {
            rewrote,
            re_enabled: prior == SystemdDriverStatus::Disabled,
            restarted: prior == SystemdDriverStatus::Stopped,
        }
    } else {
        RepairCauses::default()
    };
    Ok(SystemdInstall { outcome, repair })
}

/// Delete each named file in `dir` that carries our marker; returns the
/// names deleted. Best effort: used only to undo a failed fresh install.
fn remove_marked_files(dir: &Path, names: &[&str], marker: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for name in names {
        let path = dir.join(name);
        if let Ok(Some(c)) = read_optional(&path) {
            if unit_has_marker(&c, marker) && std::fs::remove_file(&path).is_ok() {
                removed.push((*name).to_string());
            }
        }
    }
    removed
}

/// What [`remove_systemd_units`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemdRemoval {
    /// `disable --now` was run on the timer.
    pub disabled_timer: bool,
    /// Unit files deleted (they carried our marker).
    pub deleted: Vec<String>,
    /// Files with our name but without our marker: left untouched.
    pub refused: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

impl SystemdRemoval {
    pub(crate) fn removed_anything(&self) -> bool {
        self.disabled_timer || !self.deleted.is_empty()
    }
}

/// Remove this repo's systemd driver. Only the `.timer` is disabled
/// (`disable --now`); the `.service` is never stopped, so a tick in flight
/// finishes and a wave it launched is not killed. A unit file is deleted only
/// if it carries our marker; a foreign file with our name is reported and
/// left alone. Idempotent.
// trace:TASK-1491 | ai:claude
pub(crate) fn remove_systemd_units(
    host: &mut dyn DriverHost,
    repo: &str,
    marker: &str,
) -> Result<SystemdRemoval> {
    let mut r = SystemdRemoval::default();
    if !host.systemd_supported() {
        return Ok(r);
    }
    let (service_name, timer_name) = unit_names(repo);
    let dir = host.unit_dir()?;
    let timer_path = dir.join(&timer_name);
    match read_optional(&timer_path)? {
        Some(c) if unit_has_marker(&c, marker) => {
            systemctl_ok(host, &["disable", "--now", &timer_name])?;
            r.disabled_timer = true;
            std::fs::remove_file(&timer_path)
                .with_context(|| format!("failed to delete {}", timer_path.display()))?;
            r.deleted.push(timer_name);
        }
        Some(_) => r.refused.push(timer_path),
        None => {}
    }
    let service_path = dir.join(&service_name);
    match read_optional(&service_path)? {
        Some(c) if unit_has_marker(&c, marker) => {
            std::fs::remove_file(&service_path)
                .with_context(|| format!("failed to delete {}", service_path.display()))?;
            r.deleted.push(service_name);
        }
        Some(_) => r.refused.push(service_path),
        None => {}
    }
    if !r.deleted.is_empty() {
        if let Err(e) = systemctl_ok(host, &["daemon-reload"]) {
            r.warnings.push(format!("{e:#}"));
        }
    }
    Ok(r)
}

/// Which driver an install switches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Driver {
    Cron,
    Systemd,
}

impl Driver {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Driver::Cron => "crontab entry",
            Driver::Systemd => "systemd user timer",
        }
    }
}

/// What a driver install did, including what it removed of the other one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SwitchReport {
    pub driver: Driver,
    pub outcome: DriverInstallOutcome,
    /// Why, when `outcome` is `Repaired`.
    // trace:BUG-1619 | ai:claude
    pub repair: RepairCauses,
    /// Crontab lines removed after the systemd timer was verified.
    pub removed_cron_lines: Vec<String>,
    /// What was removed of systemd after the crontab entry was verified.
    pub systemd_removal: Option<SystemdRemoval>,
    /// Linger is off: the timer stops when the user logs out.
    pub linger_off: bool,
    pub warnings: Vec<String>,
}

/// Install `driver` for this invocation, verify it, and only then remove
/// the other driver (A13a). An error after the verify step names what is
/// installed, so the operator knows both are present.
// trace:TASK-1491 | ai:claude
pub(crate) fn switch_driver(
    host: &mut dyn DriverHost,
    inv: &TickInvocation,
    driver: Driver,
) -> Result<SwitchReport> {
    match driver {
        Driver::Systemd => switch_to_systemd(host, inv),
        Driver::Cron => switch_to_cron(host, inv),
    }
}

fn switch_to_systemd(host: &mut dyn DriverHost, inv: &TickInvocation) -> Result<SwitchReport> {
    if !host.systemd_supported() {
        anyhow::bail!(
            "systemd user timers are only available on Linux; use `aida schedule install-cron`"
        );
    }
    let units = build_systemd_units(inv);
    let installed = install_systemd_units(host, &units)?;
    let mut report = SwitchReport {
        driver: Driver::Systemd,
        outcome: installed.outcome,
        repair: installed.repair,
        removed_cron_lines: Vec::new(),
        systemd_removal: None,
        linger_off: false,
        warnings: Vec::new(),
    };
    match host.read_crontab() {
        Ok(Some(body)) => {
            if let Some((new_body, removed)) = crontab_after_driver_switch(&body, &inv.marker) {
                host.write_crontab(&new_body).context(
                    "the systemd timer is installed and enabled, but removing this repo's \
                     crontab entry failed, so both drivers now run (harmless: the tick lock \
                     keeps them from overlapping; `aida doctor` reports it)",
                )?;
                report.removed_cron_lines = removed;
            }
        }
        Ok(None) => {}
        Err(e) => report.warnings.push(format!(
            "could not read your crontab to remove an older entry for this repo ({e:#}); \
             if one exists both drivers run, and `aida doctor` reports it"
        )),
    }
    report.linger_off = host.linger_enabled() == Some(false);
    Ok(report)
}

fn switch_to_cron(host: &mut dyn DriverHost, inv: &TickInvocation) -> Result<SwitchReport> {
    let line = render_tick_cron_line(inv);
    let existing = host.read_crontab()?.unwrap_or_default();
    let had_entry = crontab_has_repair_target(&existing, &inv.marker);
    let outcome = match crontab_after_install(&existing, &inv.marker, &line) {
        None => DriverInstallOutcome::AlreadyUpToDate,
        Some(body) => {
            host.write_crontab(&body)?;
            if had_entry {
                DriverInstallOutcome::Repaired
            } else {
                DriverInstallOutcome::Installed
            }
        }
    };
    let back = host.read_crontab()?.unwrap_or_default();
    if !back.lines().any(|l| l == line) {
        anyhow::bail!(
            "wrote the crontab entry, but reading the crontab back does not show it. \
             This repo's systemd timer, if any, was left in place."
        );
    }
    let removal = remove_systemd_units(host, &inv.repo, &inv.marker).context(
        "the crontab entry is installed, but removing this repo's systemd timer failed, so \
         both drivers now run (harmless: the tick lock keeps them from overlapping; \
         `aida doctor` reports it)",
    )?;
    // trace:BUG-1619 | ai:claude
    let repair = if outcome == DriverInstallOutcome::Repaired {
        RepairCauses {
            rewrote: vec!["the crontab entry".to_string()],
            ..RepairCauses::default()
        }
    } else {
        RepairCauses::default()
    };
    Ok(SwitchReport {
        driver: Driver::Cron,
        outcome,
        repair,
        removed_cron_lines: Vec::new(),
        systemd_removal: Some(removal),
        linger_off: false,
        warnings: Vec::new(),
    })
}

/// Non-binding advisor note: a binary under a git checkout's `target/`
/// directory is swapped by the next build there, under the running driver.
pub(crate) fn exe_is_build_output(exe: &Path) -> bool {
    exe.ancestors().any(|a| {
        a.file_name().is_some_and(|n| n == "target")
            && a.parent().is_some_and(|p| p.join(".git").exists())
    })
}

// ---------------------------------------------------------------------------
// Status (doctor, shift status)
// ---------------------------------------------------------------------------

/// Whether this repo's systemd timer drives the tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SystemdDriverStatus {
    /// Our marked timer exists, is enabled, and is active.
    Installed,
    /// Our marked timer exists but is not enabled (or systemd does not know it).
    Disabled,
    /// Our marked timer is enabled but not active (stopped or failed).
    Stopped,
    /// No timer of ours.
    Missing,
    /// Not a systemd platform.
    Unsupported,
    /// Could not tell (no user bus, unrecognised `systemctl` output, an
    /// unreadable unit directory). PRIN-5: never collapse this into a verdict.
    Unknown(String),
}

impl SystemdDriverStatus {
    /// A short phrase for error messages.
    pub(crate) fn describe(&self) -> String {
        match self {
            SystemdDriverStatus::Installed => "enabled and running".to_string(),
            SystemdDriverStatus::Disabled => "not enabled".to_string(),
            SystemdDriverStatus::Stopped => "enabled but not running".to_string(),
            SystemdDriverStatus::Missing => "missing".to_string(),
            SystemdDriverStatus::Unsupported => "unsupported on this platform".to_string(),
            SystemdDriverStatus::Unknown(r) => format!("unknown ({r})"),
        }
    }
}

/// Both drivers' state for one repo (`CronDriverStatus` before slice 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverStatus {
    pub cron: CronDriverStatus,
    pub systemd: SystemdDriverStatus,
}

impl DriverStatus {
    pub(crate) fn cron_installed(&self) -> bool {
        self.cron == CronDriverStatus::Installed
    }

    pub(crate) fn systemd_installed(&self) -> bool {
        self.systemd == SystemdDriverStatus::Installed
    }

    pub(crate) fn both_installed(&self) -> bool {
        self.cron_installed() && self.systemd_installed()
    }

    pub(crate) fn any_installed(&self) -> bool {
        self.cron_installed() || self.systemd_installed()
    }

    /// Why a driver's state could not be read, one entry per unknown
    /// driver. Non-empty with nothing installed means "unknown", not "none".
    // trace:TASK-1491 | ai:claude
    pub(crate) fn unknown_reasons(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let CronDriverStatus::Unknown(r) = &self.cron {
            out.push(format!("crontab: {r}"));
        }
        if let SystemdDriverStatus::Unknown(r) = &self.systemd {
            out.push(format!("systemd: {r}"));
        }
        out
    }

    /// One phrase for `aida shift status`.
    pub(crate) fn label(&self) -> String {
        match (self.cron_installed(), self.systemd_installed()) {
            (true, true) => {
                "cron and systemd (both installed; remove one with `aida schedule uninstall-cron` or `uninstall-systemd`)".to_string()
            }
            (true, false) => "cron (installed)".to_string(),
            (false, true) => "systemd timer (installed)".to_string(),
            (false, false) => {
                let unknown = self.unknown_reasons();
                if !unknown.is_empty() {
                    format!("unknown ({})", unknown.join("; "))
                } else if self.systemd == SystemdDriverStatus::Disabled {
                    "systemd timer installed but disabled (`aida shift install --systemd-user`)"
                        .to_string()
                } else if self.systemd == SystemdDriverStatus::Stopped {
                    "systemd timer enabled but not running (`aida shift install --systemd-user`)"
                        .to_string()
                } else {
                    "none installed (`aida shift install --systemd-user` or `--cron`)".to_string()
                }
            }
        }
    }
}

/// PURE: why `systemctl` output carries no recognised state.
fn unreadable(verb: &str, out: &CommandOutput) -> String {
    let stderr = out.stderr.trim();
    let stdout = out.stdout.trim();
    if !stderr.is_empty() {
        format!("`systemctl --user {verb}` failed: {stderr}")
    } else if !stdout.is_empty() {
        format!("`systemctl --user {verb}` printed an unrecognised state {stdout:?}")
    } else {
        format!("`systemctl --user {verb}` printed nothing")
    }
}

/// PURE: `is-enabled` output as enabled (`Ok(true)`), a known not-enabled
/// state (`Ok(false)`), or unreadable (`Err`). No user bus prints nothing on
/// stdout and exits 1, which is unreadable, never "disabled".
// trace:TASK-1491 | ai:claude
pub(crate) fn classify_is_enabled(out: &CommandOutput) -> Result<bool, String> {
    match out.stdout.trim() {
        "enabled" | "enabled-runtime" => Ok(true),
        "disabled" | "masked" | "masked-runtime" | "static" | "linked" | "linked-runtime"
        | "indirect" | "not-found" => Ok(false),
        _ => Err(unreadable("is-enabled", out)),
    }
}

/// What `is-enabled` says about a timer whose unit file is not in our
/// unit directory.
// trace:BUG-1619 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FreshProbe {
    /// No unit file with this name anywhere systemd looks.
    NotFound,
    /// systemd knows the unit (the reported state).
    Known(String),
    /// Could not tell (no user bus, unrecognised output).
    Unknown(String),
}

/// PURE: classify `is-enabled <timer_name>` for the fresh-install check.
/// Newer systemd prints `not-found`; older releases print nothing on stdout
/// and `Failed to get unit file state for <timer_name>: No such file or
/// directory` on stderr. Any other known state means the unit exists
/// somewhere. Everything else is unknown, so the check fails closed: in
/// particular a bus/connection failure, which can also end in "No such file
/// or directory" (`Failed to connect to bus: No such file or directory`).
// trace:BUG-1619 | ai:claude
pub(crate) fn classify_fresh_probe(out: &CommandOutput, timer_name: &str) -> FreshProbe {
    let stdout = out.stdout.trim();
    let stderr = out.stderr.trim();
    // A bus or connection failure is never "not found", whatever errno text
    // follows it. trace:BUG-1619 | ai:claude
    const BUS_FAILURES: [&str; 5] = [
        "Failed to connect to bus",
        "Failed to connect to user scope bus",
        "Failed to get D-Bus connection",
        "Transport endpoint is not connected",
        "Connection refused",
    ];
    if BUS_FAILURES.iter().any(|m| stderr.contains(m)) {
        return FreshProbe::Unknown(format!(
            "`systemctl --user is-enabled {timer_name}` could not reach the user manager: {stderr}"
        ));
    }
    if stdout == "not-found" {
        return FreshProbe::NotFound;
    }
    // Old systemd: only the is-enabled message for this exact unit counts.
    // trace:BUG-1619 | ai:claude
    let legacy_prefix = format!("Failed to get unit file state for {timer_name}:");
    if stdout.is_empty()
        && !out.success
        && stderr
            .lines()
            .any(|l| l.starts_with(&legacy_prefix) && l.contains("No such file or directory"))
    {
        return FreshProbe::NotFound;
    }
    match classify_is_enabled(out) {
        Ok(_) => FreshProbe::Known(stdout.to_string()),
        Err(r) => FreshProbe::Unknown(r),
    }
}

/// PURE: `is-active` output as active (`Ok(true)`), a known inactive state
/// (`Ok(false)`), or unreadable (`Err`).
// trace:TASK-1491 | ai:claude
pub(crate) fn classify_is_active(out: &CommandOutput) -> Result<bool, String> {
    match out.stdout.trim() {
        "active" | "activating" | "reloading" => Ok(true),
        "inactive" | "failed" | "deactivating" => Ok(false),
        _ => Err(unreadable("is-active", out)),
    }
}

/// The run state of a timer whose unit file is ours: `is-enabled`, then
/// `is-active`. Never returns `Missing` or `Unsupported`.
// trace:TASK-1491 | ai:claude
pub(crate) fn timer_state(host: &mut dyn DriverHost, timer_name: &str) -> SystemdDriverStatus {
    let enabled = match host.systemctl_user(&["is-enabled", timer_name]) {
        Ok(out) => classify_is_enabled(&out),
        Err(e) => Err(format!("{e:#}")),
    };
    match enabled {
        Err(r) => return SystemdDriverStatus::Unknown(r),
        Ok(false) => return SystemdDriverStatus::Disabled,
        Ok(true) => {}
    }
    let active = match host.systemctl_user(&["is-active", timer_name]) {
        Ok(out) => classify_is_active(&out),
        Err(e) => Err(format!("{e:#}")),
    };
    match active {
        Ok(true) => SystemdDriverStatus::Installed,
        Ok(false) => SystemdDriverStatus::Stopped,
        Err(r) => SystemdDriverStatus::Unknown(r),
    }
}

/// Whether this repo's marked timer file is present. A filesystem read
/// only (no `systemctl`), cheap enough for every `aida doctor` run.
// trace:TASK-1491 | ai:claude
pub(crate) fn systemd_timer_file_present(host: &dyn DriverHost, repo: &str, marker: &str) -> bool {
    if !host.systemd_supported() {
        return false;
    }
    let Ok(dir) = host.unit_dir() else {
        return false;
    };
    matches!(
        read_optional(&dir.join(unit_names(repo).1)),
        Ok(Some(c)) if unit_has_marker(&c, marker)
    )
}

/// Read this repo's systemd driver state through `host`.
// trace:TASK-1491 | ai:claude
pub(crate) fn systemd_driver_status_with(
    host: &mut dyn DriverHost,
    repo: &str,
    marker: &str,
) -> SystemdDriverStatus {
    if !host.systemd_supported() {
        return SystemdDriverStatus::Unsupported;
    }
    let dir = match host.unit_dir() {
        Ok(d) => d,
        Err(e) => return SystemdDriverStatus::Unknown(format!("{e:#}")),
    };
    let (_, timer_name) = unit_names(repo);
    match read_optional(&dir.join(&timer_name)) {
        Ok(Some(c)) if unit_has_marker(&c, marker) => {}
        Ok(_) => return SystemdDriverStatus::Missing,
        Err(e) => return SystemdDriverStatus::Unknown(format!("{e:#}")),
    }
    timer_state(host, &timer_name)
}

/// Both drivers' state through `host`.
pub(crate) fn driver_status_with(
    host: &mut dyn DriverHost,
    repo: &str,
    marker: &str,
) -> DriverStatus {
    let cron = classify_cron_driver(host.read_crontab().map_err(|e| format!("{e:#}")), marker);
    let systemd = systemd_driver_status_with(host, repo, marker);
    DriverStatus { cron, systemd }
}

fn canonical_repo(project_root: &Path) -> PathBuf {
    project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
}

/// Both drivers' state for `project_root` on the real host.
pub(crate) fn driver_status(project_root: &Path) -> DriverStatus {
    let repo = canonical_repo(project_root).display().to_string();
    driver_status_with(&mut RealDriverHost, &repo, &tick_cron_marker(project_root))
}

/// Whether `project_root`'s marked timer file is present on the real host
/// (a file read; never `systemctl`).
pub(crate) fn systemd_timer_file_present_for(project_root: &Path) -> bool {
    let repo = canonical_repo(project_root).display().to_string();
    systemd_timer_file_present(&RealDriverHost, &repo, &tick_cron_marker(project_root))
}

/// The shared invocation for `project_root` and the running `aida` binary.
pub(crate) fn real_tick_invocation(project_root: &Path) -> Result<TickInvocation> {
    let exe = crate::aida_exe_path();
    let exe = exe.canonicalize().unwrap_or(exe);
    tick_invocation(&canonical_repo(project_root), &exe)
}

// ---------------------------------------------------------------------------
// Wave units (TASK-1510): one transient unit per night-shift drain wave
// ---------------------------------------------------------------------------

/// The longest the `systemd-run` call of a wave launch may take. With
/// [`WAVE_UNIT_PROBE_TIMEOUT`] the two calls fit in the tick's 30s launch
/// reserve, well inside the scheduler's 120s kill (A5).
// trace:TASK-1510 | ai:claude
pub(crate) const WAVE_UNIT_RUN_TIMEOUT: Duration = Duration::from_secs(20);
/// The longest the read-only `systemctl --user show` probe may take.
pub(crate) const WAVE_UNIT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// `RuntimeMaxSec` = the wave's `--max-runtime` plus this grace (Q2): the
/// drain stops between specs, so one spec can overrun; this is the hard stop.
pub(crate) const WAVE_UNIT_RUNTIME_GRACE_SECS: u64 = 30 * 60;

/// PURE: `aida-wave-<hex8>-<YYYYMMDD-HHMMSS>-<tick pid>.service`. Only
/// `[a-z0-9-]`, so it needs no escaping and never holds a `%` specifier.
// trace:TASK-1510 | ai:claude
pub(crate) fn wave_unit_name(
    repo: &str,
    now: chrono::DateTime<chrono::Utc>,
    tick_pid: u32,
) -> String {
    format!(
        "aida-wave-{}-{}-{tick_pid}.service",
        repo_hash8(repo),
        now.format("%Y%m%d-%H%M%S")
    )
}

/// PURE: a value systemd would expand (`%` specifiers, `$` variables) in a
/// unit property or command line. Such a value is never escaped in place:
/// the wave falls back to the detached launch (A6).
// trace:TASK-1510 | ai:claude
pub(crate) fn expands_in_unit(value: &str) -> bool {
    value.contains('%') || value.contains('$')
}

/// PURE: whether `/proc/self/cgroup` puts this process inside `unit` (the
/// last path segment of a cgroup line is the unit's own cgroup).
// trace:TASK-1510 | ai:claude
pub(crate) fn cgroup_in_unit(cgroup: &str, unit: &str) -> bool {
    cgroup.lines().any(|line| {
        line.splitn(3, ':')
            .nth(2)
            .and_then(|path| path.trim_end().rsplit('/').next())
            .is_some_and(|last| last == unit)
    })
}

/// Everything one wave unit is made of.
// trace:TASK-1510 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct WaveUnitSpec<'a> {
    pub unit: &'a str,
    pub repo: &'a str,
    pub exe: &'a str,
    pub log: &'a str,
    pub description: &'a str,
    pub runtime_max_secs: u64,
    /// The per-wave resource limits (TASK-1517).
    // trace:TASK-1517 | ai:claude
    pub limits: &'a WaveLimits,
    /// `-E KEY=VALUE` pairs.
    pub set_env: &'a [(String, String)],
    /// Keys removed from the environment the unit inherits from the user
    /// manager (`-p UnsetEnvironment=`): `-E` can only set, never unset.
    pub unset_env: &'a [&'a str],
    /// The wave argv, exactly as the detached launch passes it.
    pub argv: &'a [String],
}

/// PURE: the full `systemd-run` argument list for one wave unit (A7): a
/// transient `Type=exec` service, garbage-collected when it ends, with the
/// wave log appended, `RuntimeMaxSec`, `OOMPolicy=stop` and the env delta.
/// No timer flag and no `Restart=`: systemd never starts or restarts a wave
/// on its own.
// trace:TASK-1510 | ai:claude
pub(crate) fn build_wave_unit_argv(s: &WaveUnitSpec<'_>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--user".to_string(),
        format!("--unit={}", s.unit),
        "--collect".to_string(),
        "--no-ask-password".to_string(),
        "--quiet".to_string(),
        "--service-type=exec".to_string(),
        format!("--working-directory={}", s.repo),
    ];
    for (k, v) in s.set_env {
        a.push("-E".to_string());
        a.push(format!("{k}={v}"));
    }
    for k in s.unset_env {
        a.push("-p".to_string());
        a.push(format!("UnsetEnvironment={k}"));
    }
    for p in [
        format!("StandardOutput=append:{}", s.log),
        format!("StandardError=append:{}", s.log),
        format!("RuntimeMaxSec={}", s.runtime_max_secs),
        "OOMPolicy=stop".to_string(),
        // trace:TASK-1517 | ai:claude — the wave absorbs its own runaway:
        // reclaim throttle, then a cgroup-local OOM kill, and a CPU/IO
        // weight that leaves the operator's session responsive.
        format!("MemoryHigh={}", s.limits.memory_high),
        format!("MemoryMax={}", s.limits.memory_max),
        format!("CPUWeight={}", s.limits.cpu_weight),
        format!("IOWeight={}", s.limits.io_weight),
        format!("TasksMax={}", s.limits.tasks_max),
    ] {
        a.push("-p".to_string());
        a.push(p);
    }
    a.push(format!("--description={}", s.description));
    a.push("--".to_string());
    a.push(s.exe.to_string());
    a.extend(s.argv.iter().cloned());
    a
}

/// Why a `systemd-run` call did not report a clean start.
// trace:TASK-1510 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunFailure {
    /// Provably nothing started: the detached fallback is safe.
    NotStarted(String),
    /// The unit may or may not have started: probe it, never relaunch blind.
    Ambiguous(String),
    /// A unit with this name already exists (it could be a live wave).
    Exists(String),
}

/// `systemd-run` messages that mean the user manager was never reached, so
/// nothing was started. Only counted together with a non-zero exit.
const WAVE_RUN_NO_BUS: [&str; 5] = [
    "Failed to connect to bus",
    "Failed to connect to user scope bus",
    "Failed to get D-Bus connection",
    "No medium found",
    "$DBUS_SESSION_BUS_ADDRESS",
];

/// PURE: `Ok(())` for a clean start, else why not. A spawn failure proves
/// nothing ran; a non-zero exit is "not started" only with a recognised
/// no-bus message; a timeout or any other failure is ambiguous (A5).
// trace:TASK-1510 | ai:claude
pub(crate) fn classify_systemd_run(run: &BoundedRun) -> Result<(), RunFailure> {
    match run {
        BoundedRun::SpawnFailed(e) => Err(RunFailure::NotStarted(format!(
            "systemd-run could not be started ({e})"
        ))),
        BoundedRun::TimedOut => Err(RunFailure::Ambiguous(format!(
            "systemd-run did not finish within {}s",
            WAVE_UNIT_RUN_TIMEOUT.as_secs()
        ))),
        BoundedRun::Exited(out) if out.success => Ok(()),
        BoundedRun::Exited(out) => {
            let stderr = out.stderr.trim();
            if stderr.contains("already exists")
                || stderr.contains("already loaded")
                || stderr.contains("has a fragment file")
            {
                Err(RunFailure::Exists(format!("systemd-run: {stderr}")))
            } else if WAVE_RUN_NO_BUS.iter().any(|m| stderr.contains(m)) {
                Err(RunFailure::NotStarted(format!(
                    "systemd-run could not reach the user manager: {stderr}"
                )))
            } else if stderr.is_empty() {
                Err(RunFailure::Ambiguous(
                    "systemd-run failed without a message".to_string(),
                ))
            } else {
                Err(RunFailure::Ambiguous(format!(
                    "systemd-run failed: {stderr}"
                )))
            }
        }
    }
}

/// What `systemctl --user show -p LoadState -p ActiveState -p MainPID`
/// says about a wave unit.
// trace:TASK-1510 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnitProbe {
    /// Active, activating or reloading. `main_pid` is `None` for `MainPID=0`.
    Active { main_pid: Option<u32> },
    /// systemd does not know the unit (never started, or already collected).
    NotFound,
    /// Loaded but not running (the state).
    Inactive(String),
    /// Could not tell.
    Unknown(String),
}

/// The `show` arguments for [`classify_unit_show`].
pub(crate) fn unit_show_args(unit: &str) -> [&str; 8] {
    [
        "show",
        "-p",
        "LoadState",
        "-p",
        "ActiveState",
        "-p",
        "MainPID",
        unit,
    ]
}

/// PURE: classify the `show` probe of a wave unit.
// trace:TASK-1510 | ai:claude
pub(crate) fn classify_unit_show(run: &BoundedRun) -> UnitProbe {
    let out = match run {
        BoundedRun::Exited(out) if out.success => out,
        BoundedRun::Exited(out) => return UnitProbe::Unknown(unreadable("show", out)),
        BoundedRun::TimedOut => {
            return UnitProbe::Unknown(format!(
                "`systemctl --user show` did not finish within {}s",
                WAVE_UNIT_PROBE_TIMEOUT.as_secs()
            ))
        }
        BoundedRun::SpawnFailed(e) => return UnitProbe::Unknown(e.clone()),
    };
    let prop = |key: &str| {
        out.stdout.lines().find_map(|l| {
            l.trim()
                .strip_prefix(key)
                .and_then(|r| r.strip_prefix('='))
                .map(str::to_string)
        })
    };
    let (Some(load), Some(active)) = (prop("LoadState"), prop("ActiveState")) else {
        return UnitProbe::Unknown(unreadable("show", out));
    };
    if load == "not-found" {
        return UnitProbe::NotFound;
    }
    match active.as_str() {
        "active" | "activating" | "reloading" => UnitProbe::Active {
            main_pid: prop("MainPID")
                .and_then(|p| p.parse::<u32>().ok())
                .filter(|p| *p > 0),
        },
        "inactive" | "failed" | "deactivating" if load == "loaded" => UnitProbe::Inactive(active),
        _ => UnitProbe::Unknown(unreadable("show", out)),
    }
}

// ---------------------------------------------------------------------------
// Per-wave resource limits (TASK-1517)
// ---------------------------------------------------------------------------

// The 2026-09-25 class: one runaway test process reached 30 GB RSS in 27
// seconds on a 70 GB machine, the whole terminal scope went over the
// system's memory-pressure threshold, and systemd-oomd killed the scope —
// the orchestrator and every background agent with it. A wave unit that
// carries its own limits absorbs that alone: the reclaim throttle bites
// first, then the cgroup's own OOM killer stops the wave's processes, and
// the memory the rest of the machine sees never disappears.
//
// These limits only CONSTRAIN a wave. Nothing here enables, schedules or
// starts an unattended run, and nothing here changes who may dispatch one.
// trace:TASK-1517 | ai:claude

/// Default `MemoryHigh` (advisor: 40% of total memory): the share above
/// which the kernel throttles the wave's own cgroup by reclaim instead of
/// letting it eat the machine.
// trace:TASK-1517 | ai:claude
pub(crate) const DEFAULT_WAVE_MEMORY_HIGH: &str = "40%";
/// Default `MemoryMax` (advisor: 50% of total memory): the hard wall. Above
/// it the cgroup's OOM killer stops the wave and only the wave.
pub(crate) const DEFAULT_WAVE_MEMORY_MAX: &str = "50%";
/// Default `CPUWeight`: half of systemd's 100, so an interactive shell and
/// the orchestrator win CPU contention against a wave. A weight only binds
/// while there IS contention, so an idle machine still gives a wave
/// everything it asks for.
pub(crate) const DEFAULT_WAVE_CPU_WEIGHT: u64 = 50;
/// Default `IOWeight`, for the same reason as [`DEFAULT_WAVE_CPU_WEIGHT`].
pub(crate) const DEFAULT_WAVE_IO_WEIGHT: u64 = 50;
/// Default `TasksMax`: tasks (processes + threads) one wave may hold at
/// once. Well above a full workspace build with its test binaries, far
/// below anything that would exhaust the machine's pids.
pub(crate) const DEFAULT_WAVE_TASKS_MAX: u64 = 2048;
/// systemd's accepted range for `CPUWeight` / `IOWeight`.
const WAVE_WEIGHT_RANGE: (u64, u64) = (1, 10_000);
/// A sane range for `TasksMax`: at least one task, never above the kernel's
/// largest `pid_max`.
const WAVE_TASKS_RANGE: (u64, u64) = (1, 4_194_304);

/// The limits one wave unit carries, already validated and rendered the way
/// systemd takes them.
// trace:TASK-1517 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WaveLimits {
    pub memory_high: String,
    pub memory_max: String,
    pub cpu_weight: u64,
    pub io_weight: u64,
    pub tasks_max: u64,
}

impl Default for WaveLimits {
    fn default() -> Self {
        Self {
            memory_high: DEFAULT_WAVE_MEMORY_HIGH.to_string(),
            memory_max: DEFAULT_WAVE_MEMORY_MAX.to_string(),
            cpu_weight: DEFAULT_WAVE_CPU_WEIGHT,
            io_weight: DEFAULT_WAVE_IO_WEIGHT,
            tasks_max: DEFAULT_WAVE_TASKS_MAX,
        }
    }
}

impl WaveLimits {
    /// A one-line description for the guard verdict and the tick report.
    pub(crate) fn describe(&self) -> String {
        format!(
            "MemoryHigh={} MemoryMax={} CPUWeight={} IOWeight={} TasksMax={}",
            self.memory_high, self.memory_max, self.cpu_weight, self.io_weight, self.tasks_max
        )
    }
}

/// A memory limit as systemd takes it: a percentage of total memory, or an
/// absolute size. Deliberately no `infinity`: a limit that is not a limit is
/// a configuration mistake, not a setting (turn the unit off with
/// `wave_unit = "off"` instead).
// trace:TASK-1517 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaveMemory {
    Percent(u64),
    Bytes(u64),
}

impl WaveMemory {
    fn render(self) -> String {
        match self {
            WaveMemory::Percent(p) => format!("{p}%"),
            WaveMemory::Bytes(b) => b.to_string(),
        }
    }
}

/// PURE: parse one `MemoryHigh` / `MemoryMax` value (`40%`, `24G`, `512M`,
/// a bare byte count).
// trace:TASK-1517 | ai:claude
fn parse_wave_memory(raw: &str) -> Result<WaveMemory, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("is empty".to_string());
    }
    if let Some(pct) = s.strip_suffix('%') {
        let pct: u64 = pct
            .trim()
            .parse()
            .map_err(|_| format!("{s:?} is not a percentage such as \"40%\""))?;
        if !(1..=100).contains(&pct) {
            return Err(format!("{s:?} is not between 1% and 100%"));
        }
        return Ok(WaveMemory::Percent(pct));
    }
    let (digits, scale) = match s.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&s[..s.len() - 1], 1024u64),
        Some('M') => (&s[..s.len() - 1], 1024u64.pow(2)),
        Some('G') => (&s[..s.len() - 1], 1024u64.pow(3)),
        Some('T') => (&s[..s.len() - 1], 1024u64.pow(4)),
        Some('B') => (&s[..s.len() - 1], 1u64),
        _ => (s, 1u64),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("{s:?} is not a size such as \"40%\", \"24G\" or \"512M\""))?;
    let bytes = n
        .checked_mul(scale)
        .ok_or_else(|| format!("{s:?} does not fit in a byte count"))?;
    if bytes == 0 {
        return Err(format!("{s:?} is zero"));
    }
    Ok(WaveMemory::Bytes(bytes))
}

/// The raw `[shift] wave_*` limit values, before validation.
// trace:TASK-1517 | ai:claude
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WaveLimitInput<'a> {
    pub memory_high: Option<&'a toml::Value>,
    pub memory_max: Option<&'a toml::Value>,
    pub cpu_weight: Option<&'a toml::Value>,
    pub io_weight: Option<&'a toml::Value>,
    pub tasks_max: Option<&'a toml::Value>,
}

fn wave_memory_value(
    key: &str,
    v: Option<&toml::Value>,
    default: &str,
) -> Result<WaveMemory, String> {
    let raw = match v {
        None => default.to_string(),
        Some(toml::Value::String(s)) => s.clone(),
        Some(toml::Value::Integer(n)) => n.to_string(),
        Some(other) => {
            return Err(format!(
                "{key} {other} is not a size such as \"40%\", \"24G\" or \"512M\""
            ))
        }
    };
    parse_wave_memory(&raw).map_err(|e| format!("{key} {e}"))
}

fn wave_memory_spelling(v: Option<&toml::Value>, default: &str) -> String {
    match v {
        None => default.to_string(),
        Some(toml::Value::String(s)) => s.clone(),
        Some(toml::Value::Integer(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

fn wave_int_value(
    key: &str,
    v: Option<&toml::Value>,
    default: u64,
    range: (u64, u64),
) -> Result<u64, String> {
    let (lo, hi) = range;
    match v {
        None => Ok(default),
        Some(toml::Value::Integer(n)) if (lo as i64..=hi as i64).contains(n) => Ok(*n as u64),
        Some(other) => Err(format!(
            "{key} {other} is not a whole number between {lo} and {hi}"
        )),
    }
}

/// PURE: the effective per-wave limits, plus the first configuration error
/// if the operator's values cannot be used.
///
/// FAIL CLOSED on configuration: a value that cannot be understood keeps the
/// defaults for display and refuses every launch through the wave-limits
/// guard, because the alternative — quietly running with no limit — is the
/// incident this exists to prevent. Missing cgroup delegation is the
/// opposite case and fails OPEN: see [`classify_wave_limit_delegation`].
// trace:TASK-1517 | ai:claude
pub(crate) fn build_wave_limits(input: &WaveLimitInput<'_>) -> (WaveLimits, Option<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut take_memory = |key: &str, v: Option<&toml::Value>, default: &str| {
        match wave_memory_value(key, v, default) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e);
                parse_wave_memory(default).expect("the built-in default parses")
            }
        }
    };
    let high = take_memory(
        "wave_memory_high",
        input.memory_high,
        DEFAULT_WAVE_MEMORY_HIGH,
    );
    let max = take_memory("wave_memory_max", input.memory_max, DEFAULT_WAVE_MEMORY_MAX);
    let high_spelling = wave_memory_spelling(input.memory_high, DEFAULT_WAVE_MEMORY_HIGH);
    let max_spelling = wave_memory_spelling(input.memory_max, DEFAULT_WAVE_MEMORY_MAX);
    // Keep validation pure: without an injected memory basis, mixed units
    // cannot be ordered safely. Reject them rather than let systemd launch a
    // wave whose throttle may sit above its hard limit.
    let over = match (high, max) {
        (WaveMemory::Percent(h), WaveMemory::Percent(m)) => h > m,
        (WaveMemory::Bytes(h), WaveMemory::Bytes(m)) => h > m,
        _ => {
            errors.push(format!(
                "wave_memory_high ({}) and wave_memory_max ({}) use mixed units; express both limits in the same unit",
                high_spelling,
                max_spelling
            ));
            false
        }
    };
    if over {
        errors.push(format!(
            "wave_memory_high ({}) is above wave_memory_max ({}), so the throttle would never bite before the hard limit",
            high_spelling,
            max_spelling
        ));
    }
    let mut take_int =
        |key: &str, v: Option<&toml::Value>, default: u64, range: (u64, u64)| match wave_int_value(
            key, v, default, range,
        ) {
            Ok(n) => n,
            Err(e) => {
                errors.push(e);
                default
            }
        };
    let cpu_weight = take_int(
        "wave_cpu_weight",
        input.cpu_weight,
        DEFAULT_WAVE_CPU_WEIGHT,
        WAVE_WEIGHT_RANGE,
    );
    let io_weight = take_int(
        "wave_io_weight",
        input.io_weight,
        DEFAULT_WAVE_IO_WEIGHT,
        WAVE_WEIGHT_RANGE,
    );
    let tasks_max = take_int(
        "wave_tasks_max",
        input.tasks_max,
        DEFAULT_WAVE_TASKS_MAX,
        WAVE_TASKS_RANGE,
    );
    let limits = WaveLimits {
        memory_high: high.render(),
        memory_max: max.render(),
        cpu_weight,
        io_weight,
        tasks_max,
    };
    match errors.into_iter().next() {
        // Unusable configuration shows the built-in defaults, exactly as an
        // unparseable `max_runtime` does, and the guard refuses the launch.
        Some(first) => (WaveLimits::default(), Some(first)),
        None => (limits, None),
    }
}

/// The cgroup controllers the wave limits need from the user manager.
/// `memory` is the one the 2026-09-25 class turns on.
// trace:TASK-1517 | ai:claude
pub(crate) const WAVE_LIMIT_CONTROLLERS: [&str; 4] = ["memory", "pids", "cpu", "io"];

/// PURE: the cgroup path of the user manager (`user@<uid>.service`) taken
/// from a `/proc/self/cgroup` body. Its `cgroup.controllers` is what a
/// transient user unit can actually be limited by.
// trace:TASK-1517 | ai:claude
pub(crate) fn user_manager_cgroup_path(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .filter_map(|l| l.splitn(3, ':').nth(2))
        .find_map(|path| {
            let mut out = String::new();
            for seg in path.trim_end().split('/').filter(|s| !s.is_empty()) {
                out.push('/');
                out.push_str(seg);
                if seg.starts_with("user@") && seg.ends_with(".service") {
                    return Some(out);
                }
            }
            None
        })
}

/// What the user manager can actually enforce on a wave unit.
// trace:TASK-1517 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaveLimitDelegation {
    /// Every controller a wave limit needs is delegated.
    Delegated,
    /// These are not: the limits that need them are not enforced. The wave
    /// still runs.
    Missing(Vec<String>),
    /// Could not tell. PRIN-5: not "ok".
    Unknown(String),
}

/// PURE: classify the user manager's `cgroup.controllers`.
///
/// FAIL OPEN: this only ever produces a doctor warning. A wave is never
/// refused and never pushed onto the detached fallback because a controller
/// is missing — an unlimited wave in its own unit is still better than no
/// wave, and the operator is told where to fix the delegation.
// trace:TASK-1517 | ai:claude
pub(crate) fn classify_wave_limit_delegation(controllers: Option<&str>) -> WaveLimitDelegation {
    let Some(body) = controllers else {
        return WaveLimitDelegation::Unknown(
            "the user manager's cgroup.controllers could not be read".to_string(),
        );
    };
    let have: Vec<&str> = body.split_whitespace().collect();
    if have.is_empty() {
        return WaveLimitDelegation::Unknown(
            "the user manager's cgroup.controllers is empty".to_string(),
        );
    }
    let missing: Vec<String> = WAVE_LIMIT_CONTROLLERS
        .iter()
        .filter(|c| !have.contains(*c))
        .map(|c| (*c).to_string())
        .collect();
    if missing.is_empty() {
        WaveLimitDelegation::Delegated
    } else {
        WaveLimitDelegation::Missing(missing)
    }
}

/// The real user manager's delegated controllers. The only impure step of the
/// wave-limit doctor check: two read-only file reads.
// trace:TASK-1517 | ai:claude
pub(crate) fn real_wave_limit_delegation() -> WaveLimitDelegation {
    classify_wave_limit_delegation(RealDriverHost.cgroup_controllers().as_deref())
}

/// PURE: the report-only `aida doctor` findings for wave limits. A finding
/// is raised only when the MEMORY controller is missing (the limit that
/// stops the 2026-09-25 class) or when delegation cannot be read at all; a
/// machine that merely lacks, say, `io` enforces the limits that matter and
/// stays quiet.
// trace:TASK-1517 | ai:claude
pub(crate) fn build_wave_limit_findings(
    delegation: &WaveLimitDelegation,
) -> Vec<crate::DoctorFinding> {
    let finding = |id: &str, summary: String| {
        crate::DoctorFinding {
        category: "scheduler-driver".to_string(),
        id: id.to_string(),
        summary,
        action: "ask an administrator to delegate the memory controller to the systemd user manager (a Delegate= drop-in for the user manager unit)".to_string(),
        safe_heal: false,
    }
    };
    match delegation {
        WaveLimitDelegation::Delegated => Vec::new(),
        WaveLimitDelegation::Missing(missing) => {
            if !missing.iter().any(|c| c == "memory") {
                return Vec::new();
            }
            vec![finding(
                "shift-wave-limits-undelegated",
                format!(
                    "the systemd user manager does not have the memory controller delegated ({} missing), so an unattended drain wave runs without its memory ceiling: one runaway process can still push the whole session into an out-of-memory kill. Waves still run.",
                    missing.join(", ")
                ),
            )]
        }
        WaveLimitDelegation::Unknown(why) => vec![finding(
            "shift-wave-limits-delegation-unknown",
            format!(
                "cannot confirm that the systemd user manager can limit an unattended drain wave's memory ({why}) — unknown, not ok. Waves still run."
            ),
        )],
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// PURE: why a driver install must refuse for this caller, if it must.
/// Installing a driver starts unattended scheduled runs, so it carries the
/// same floor as `aida shift enable`: a human at an interactive terminal,
/// outside agent output mode, answering an explicit yes.
// trace:TASK-1491 | ai:claude
pub(crate) fn driver_gate_refusal(
    command: &str,
    stdin_tty: bool,
    agent_mode: bool,
) -> Option<String> {
    let why = if agent_mode {
        "agent output mode is on"
    } else if !stdin_tty {
        "stdin is not an interactive terminal"
    } else {
        return None;
    };
    Some(format!(
        "`{command}` installs a scheduler that runs this repo's registered jobs unattended, so \
         it needs a human at an interactive terminal ({why}). Run it yourself in a terminal."
    ))
}

/// The confirmation question for installing `driver`.
pub(crate) fn install_question(driver: Driver) -> String {
    match driver {
        Driver::Systemd => format!(
            "Install a systemd user timer that runs this repo's scheduled jobs unattended every \
             {SYSTEMD_TICK_EVERY} (this repo's crontab entry, if any, is removed once the timer \
             is verified)? [y/N] "
        ),
        Driver::Cron => "Install a crontab entry that runs this repo's scheduled jobs unattended \
             every 15 minutes (this repo's systemd timer, if any, is removed once the entry is \
             verified)? [y/N] "
            .to_string(),
    }
}

/// Print what a switch did.
pub(crate) fn print_switch_report(report: &SwitchReport, exe: &Path) {
    let what = report.driver.label();
    match report.outcome {
        DriverInstallOutcome::Installed => {
            println!("Installed the scheduler {what} for this repo.")
        }
        DriverInstallOutcome::AlreadyUpToDate => {
            println!("Already installed: this repo's scheduler {what} is up to date.")
        }
        // trace:BUG-1619 | ai:claude
        DriverInstallOutcome::Repaired => println!(
            "Repaired this repo's scheduler {what}: {}.",
            report.repair.describe()
        ),
    }
    if !report.removed_cron_lines.is_empty() {
        println!("Removed this repo's crontab entries (the timer replaces them):");
        for l in &report.removed_cron_lines {
            println!("  - {l}");
        }
    }
    if let Some(r) = &report.systemd_removal {
        if r.disabled_timer {
            println!("Disabled this repo's systemd timer (the crontab entry replaces it).");
        }
        for name in &r.deleted {
            println!("  deleted {name}");
        }
        for path in &r.refused {
            println!(
                "  left {} alone: it was not written by aida for this repo",
                path.display()
            );
        }
        for w in &r.warnings {
            println!("  warning: {w}");
        }
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    if report.linger_off {
        println!(
            "Linger is off for your user, so the timer stops when you log out. To keep it \
             running, run `loginctl enable-linger` yourself."
        );
    }
    if exe_is_build_output(exe) {
        println!(
            "warning: {} is inside a git checkout's build directory; the next build there \
             replaces the binary this driver runs. Install from a released binary to avoid that.",
            exe.display()
        );
    }
}

/// `aida schedule install-systemd` / `install-cron`, gated.
pub(crate) fn install_driver_command(
    project_root: &Path,
    command: &str,
    driver: Driver,
    op: &mut crate::shift::Operator<'_>,
    host: &mut dyn DriverHost,
) -> Result<Option<SwitchReport>> {
    if let Some(msg) = driver_gate_refusal(command, op.stdin_tty, op.agent_mode) {
        anyhow::bail!("{msg}");
    }
    if !(op.confirm)(&install_question(driver))? {
        println!("Not installed (declined).");
        return Ok(None);
    }
    let inv = real_tick_invocation(project_root)?;
    let report = switch_driver(host, &inv, driver)?;
    print_switch_report(&report, Path::new(&inv.exe));
    Ok(Some(report))
}

/// `aida schedule uninstall-systemd`. Never gated: removing is always safe.
pub(crate) fn uninstall_systemd_command(project_root: &Path) -> Result<()> {
    let mut host = RealDriverHost;
    if !host.systemd_supported() {
        println!("systemd user timers are only available on Linux; nothing to remove.");
        return Ok(());
    }
    let repo = canonical_repo(project_root).display().to_string();
    let r = remove_systemd_units(&mut host, &repo, &tick_cron_marker(project_root))?;
    if r.removed_anything() {
        println!("Removed this repo's scheduler systemd timer.");
        for name in &r.deleted {
            println!("  deleted {name}");
        }
    } else {
        println!("No systemd timer found for this repo.");
    }
    for path in &r.refused {
        println!(
            "  left {} alone: it was not written by aida for this repo",
            path.display()
        );
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    Ok(())
}

/// Run `f` with the real operator seams (TTY, agent mode, y/N prompt).
pub(crate) fn with_real_operator<T>(
    f: impl FnOnce(&mut crate::shift::Operator<'_>) -> Result<T>,
) -> Result<T> {
    use std::io::IsTerminal;
    let mut confirm = |q: &str| crate::prompt_yes_no(q, false);
    let mut op = crate::shift::Operator {
        stdin_tty: std::io::stdin().is_terminal(),
        agent_mode: crate::agent_output_mode(),
        confirm: &mut confirm,
    };
    f(&mut op)
}

#[cfg(test)]
#[path = "tests/task_1491_schedule_driver_tests.rs"]
mod task_1491_schedule_driver_tests;
