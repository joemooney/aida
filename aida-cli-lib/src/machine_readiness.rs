//! Host-capacity preflight for unattended drains.
// trace:TASK-1298 | ai:codex

use crate::*;
use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use sysinfo::{Disks, System};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Level {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    pub remedy: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report {
    pub hours: u64,
    pub lanes: usize,
    pub specs: usize,
    pub disk_required_bytes: u64,
    pub inode_required: u64,
    pub memory_warn_bytes: u64,
    pub checks: Vec<Check>,
}

impl Report {
    pub(crate) fn ready(&self) -> bool {
        !self.checks.iter().any(|c| c.level == Level::Fail)
    }
}

/// Pure threshold policy: longer windows, wider fan-out, and more specs all
/// reserve additional build/log/worktree growth. This is deliberately easy to
/// tune and test without manufacturing a full filesystem.
pub(crate) fn thresholds(hours: u64, lanes: usize, specs: usize) -> (u64, u64) {
    let hours = hours.max(1);
    let lanes = lanes.max(1) as u64;
    let specs = specs.max(1) as u64;
    let disk_gib = 6 + lanes * 5 + specs * 2 + (hours.saturating_sub(1) / 4) * lanes;
    let memory_gib = (2 + lanes * 2).max(4);
    (disk_gib.saturating_mul(GIB), memory_gib.saturating_mul(GIB))
}

/// Inodes are consumed by checkouts, dependency trees, compiler outputs, and
/// logs even when those files are small. Keep this independent of byte space.
pub(crate) fn inode_threshold(hours: u64, lanes: usize, specs: usize) -> u64 {
    let hours = hours.max(1);
    let lanes = lanes.max(1) as u64;
    let specs = specs.max(1) as u64;
    10_000u64
        .saturating_add(lanes.saturating_mul(5_000))
        .saturating_add(specs.saturating_mul(2_000))
        .saturating_add(hours.saturating_mul(lanes).saturating_mul(250))
}

pub(crate) fn decide_capacity(available: u64, required: u64, warn_only: bool) -> Level {
    if available >= required {
        Level::Pass
    } else if warn_only {
        Level::Warn
    } else {
        Level::Fail
    }
}

fn containing_disk<'a>(disks: &'a Disks, path: &Path) -> Option<&'a sysinfo::Disk> {
    disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
}

fn human_bytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

fn earlyoom_policy() -> String {
    ["/etc/default/earlyoom", "/etc/sysconfig/earlyoom"]
        .into_iter()
        .find_map(|path| {
            let body = std::fs::read_to_string(path).ok()?;
            let args = body
                .lines()
                .find(|line| line.trim_start().starts_with("EARLYOOM_ARGS="))?
                .trim();
            Some(format!("; host policy {path}: {args}"))
        })
        .unwrap_or_else(|| "; earlyoom policy not found in /etc/default or /etc/sysconfig".into())
}

fn nearest_existing(mut path: PathBuf) -> PathBuf {
    while !path.exists() {
        if !path.pop() {
            return PathBuf::from("/");
        }
    }
    path
}

#[cfg(unix)]
fn available_inodes(path: &Path) -> std::io::Result<u64> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "filesystem path contains a NUL byte",
        )
    })?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and stats is writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success and initialized the structure.
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_favail as u64)
}

#[cfg(not(unix))]
fn available_inodes(_path: &Path) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "inode accounting is not available on this platform",
    ))
}

fn disk_checks(root: &Path, required: u64, required_inodes: u64, hours: u64) -> Vec<Check> {
    let disks = Disks::new_with_refreshed_list();
    let paths = [
        ("repo", root.to_path_buf()),
        ("build", target_dir(root)),
        ("temp", std::env::temp_dir()),
    ];
    let mut out = Vec::new();
    for (label, requested) in paths {
        let probe = nearest_existing(requested.clone());
        let Some(disk) = containing_disk(&disks, &probe) else {
            out.push(Check {
                name: format!("disk:{label}"),
                level: Level::Fail,
                detail: format!("could not resolve filesystem for {}", requested.display()),
                remedy: Some("verify the path is mounted and readable, then retry".into()),
            });
            continue;
        };
        let key = disk.mount_point().to_path_buf();
        let free = disk.available_space();
        let inode_result = available_inodes(&probe);
        let inode_level = inode_result.as_ref().map_or(Level::Fail, |available| {
            decide_capacity(*available, required_inodes, false)
        });
        let level = if decide_capacity(free, required, false) == Level::Fail
            || inode_level == Level::Fail
        {
            Level::Fail
        } else {
            Level::Pass
        };
        let inode_detail = inode_result.as_ref().map_or_else(
            |error| format!("inode availability unavailable ({error})"),
            |available| format!("{available} inodes free; {required_inodes} required"),
        );
        out.push(Check {
            name: format!("disk:{}", key.display()), level,
            detail: format!("{} free; {} required; {inode_detail} for this {}h / {}-path workload", human_bytes(free), human_bytes(required), hours, label),
            remedy: (level == Level::Fail).then(|| format!("reclaim at least {} and {} inodes on {}; the build directory is {} — inspect bytes with du -sh {} and inode-heavy paths with du --inodes -d 2 {}, then reclaim Rust artifacts with cargo clean --target-dir {}", human_bytes(required.saturating_sub(free)), inode_result.map_or(required_inodes, |available| required_inodes.saturating_sub(available)), key.display(), target_dir(root).display(), target_dir(root).display(), key.display(), target_dir(root).display())),
        });
    }
    out
}

pub(crate) fn probe(root: &Path, hours: u64, lanes: usize, specs: usize) -> Report {
    let (disk_required_bytes, memory_warn_bytes) = thresholds(hours, lanes, specs);
    let inode_required = inode_threshold(hours, lanes, specs);
    let mut checks = disk_checks(root, disk_required_bytes, inode_required, hours);
    let mut system = System::new();
    system.refresh_memory();
    let memory = system.available_memory();
    let memory_level = decide_capacity(memory, memory_warn_bytes, true);
    checks.push(Check { name: "memory".into(), level: memory_level,
        detail: format!("{} available; {} recommended for {} lane(s){}", human_bytes(memory), human_bytes(memory_warn_bytes), lanes, earlyoom_policy()),
        remedy: (memory_level != Level::Pass).then(|| "reduce --concurrency, close memory-heavy applications, or add swap; inspect earlyoom policy rather than disabling it".into()) });

    // Use the canonical executable resolver rather than repeating a raw
    // current_exe/PATH lookup; this is the same binary subsequent AIDA
    // self-invocations will actually use.
    let exe = aida_exe_path();
    let metadata = std::fs::metadata(&exe).ok();
    let mtime = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let exists = metadata.as_ref().is_some_and(|m| m.is_file());
    checks.push(Check { name: "binary".into(), level: if exists { Level::Pass } else { Level::Fail },
        detail: format!("{}; sha {}; mtime {}", exe.display(), build_git_sha(), mtime.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into())),
        remedy: (!exists).then(|| "rebuild the selected binary and re-run `aida dev activate`; do not rely on PATH fallback".into()) });
    Report {
        hours,
        lanes,
        specs,
        disk_required_bytes,
        inode_required,
        memory_warn_bytes,
        checks,
    }
}

pub(crate) fn print_human(report: &Report) {
    println!(
        "\n{} machine readiness ({}h · {} lanes · {} specs)",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan().bold(),
        report.hours,
        report.lanes,
        report.specs
    );
    for c in &report.checks {
        let badge = match c.level {
            Level::Pass => "PASS".green(),
            Level::Warn => "WARN".yellow(),
            Level::Fail => "FAIL".red(),
        };
        println!("  {badge} {} — {}", c.name, c.detail);
        if let Some(remedy) = &c.remedy {
            println!("       remedy: {remedy}");
        }
    }
}

pub(crate) fn run_command(hours: u64, lanes: usize, specs: usize, json: bool) -> Result<()> {
    let root = find_project_root().unwrap_or(std::env::current_dir()?);
    let report = probe(&root, hours, lanes, specs);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    if !report.ready() {
        anyhow::bail!(
            "machine readiness failed; apply the reported remedies before an unattended run"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thresholds_scale_with_the_work_ahead() {
        let short = thresholds(1, 1, 1);
        let overnight = thresholds(20, 4, 20);
        assert!(overnight.0 > short.0);
        assert!(overnight.1 > short.1);
        assert!(inode_threshold(20, 4, 20) > inode_threshold(1, 1, 1));
        assert_eq!(decide_capacity(short.0, short.0, false), Level::Pass);
        assert_eq!(decide_capacity(short.0 - 1, short.0, false), Level::Fail);
        let inodes = inode_threshold(1, 1, 1);
        assert_eq!(decide_capacity(inodes, inodes, false), Level::Pass);
        assert_eq!(decide_capacity(inodes - 1, inodes, false), Level::Fail);
        assert_eq!(decide_capacity(short.1 - 1, short.1, true), Level::Warn);
    }
}
