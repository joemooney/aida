//! Scheduler drivers for `aida schedule tick`: the systemd user timer, the
//! rules for switching between it and the crontab entry, and the two-driver
//! status `aida doctor` and `aida shift status` read (STORY-1218 slice 2).
//!
//! The binding advisor rules this module implements:
//!
//! - A12 (unit shape): `Type=oneshot`, `KillMode=process` (a detached drain
//!   wave stays in the tick's cgroup; without it systemd kills the wave when
//!   the oneshot exits), `OnActiveSec` + `OnBootSec` first-fire triggers
//!   (`OnUnitInactiveSec` alone never fires the first time), no
//!   `Persistent=` (it only applies to `OnCalendar`), `TimeoutStartSec=15min`
//!   as a backstop, and NO resource limits (they would silently constrain the
//!   wave left behind in the cgroup).
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
//! `systemctl` or resolve the real unit directory under `cfg(test)`.
//!
//! trace:TASK-1491 | ai:claude

use crate::maintenance_schedule::{
    classify_cron_driver, crontab_after_driver_switch, crontab_after_install,
    crontab_has_repair_target, render_tick_cron_line, tick_cron_marker, tick_invocation,
    CronDriverStatus, DriverInstallOutcome, TickInvocation,
};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
            dirs::home_dir(),
        )
        .context("could not resolve the home directory for the systemd user unit directory")
    }

    fn linger_enabled(&mut self) -> Option<bool> {
        real_linger_enabled()
    }

    fn systemd_supported(&self) -> bool {
        cfg!(target_os = "linux")
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
    let digest = Sha256::digest(repo.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("aida-tick-{hex}")
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
