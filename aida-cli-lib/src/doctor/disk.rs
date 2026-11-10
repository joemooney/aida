// trace:TASK-3781 | ai:codex

use super::*;

/// The whole of the light path's work, pure over `project_root`: config read
/// + disk probe → report. Split out so a fixture can pin that it never needs
/// a loadable store.
// trace:BUG-1675 | ai:claude
// trace:STORY-1916 | ai:claude
pub(crate) fn disk_headroom_light_report(project_root: &std::path::Path) -> DoctorReport {
    let cfg = crate::read_project_config_value(project_root);
    let emergency = crate::disk_emergency::emergency_config(cfg.as_ref());
    let scheduled = std::env::var_os("AIDA_SCHEDULE_CHILD").is_some_and(|v| v == "1");
    // statvfs first: it resolves a bind-mounted `<main>/target` to its own
    // filesystem, which the mount-table lookup by path prefix may not.
    let free = |p: &std::path::Path| {
        crate::disk_emergency::fs_stats(p)
            .map(|s| s.avail)
            .or_else(|| disk_free_bytes(p))
    };
    disk_headroom_light_report_finish(
        project_root,
        cfg.as_ref(),
        scheduled,
        || {
            crate::disk_emergency::run_emergency(project_root, &emergency);
        },
        || disk_headroom_light_report_with(project_root, free, cargo_slot_dir()),
        // trace:STORY-2351 | ai:claude
        || floor_reclaim(project_root),
        |dev, _role| {
            let reclaimable = crate::disk_emergency::reclaimable_on(project_root, dev);
            // trace:STORY-1916.ac641c6e | ai:claude
            let top = if crate::disk_emergency::target_on_device(project_root, dev) {
                crate::disk_emergency::top_consumers(project_root, 5)
            } else {
                Vec::new()
            };
            (reclaimable, top)
        },
    )
}

/// STORY-2351: the guard's own reclaim on a floor breach — exactly the
/// `aida worktree reclaim --apply` the route used to ask the advisor to run,
/// with every rail on (no live lease, no open PR, no cache touched in the
/// last hour, no agent scratch). Returns the bytes freed.
///
/// It runs inside the `disk-headroom-guard` child, which the scheduler kills
/// at [`crate::maintenance_schedule::SCHEDULED_COMMAND_TIMEOUT`]. A killed
/// guard routes a timeout instead of the "freed X" evidence, so the survey
/// and every deletion run against [`FLOOR_RECLAIM_BUDGET`]: once it passes,
/// the remaining caches stay for the next guard run and the report still
/// says what was freed.
// trace:STORY-2351 | ai:claude
pub(super) fn floor_reclaim(project_root: &std::path::Path) -> u64 {
    let deadline = std::time::Instant::now() + FLOOR_RECLAIM_BUDGET;
    let opts = crate::worktree_reclaim::ReclaimOptions {
        apply: true,
        min_age_mins: 60,
        target_pct: None,
        include_live: false,
        include_open_prs: false,
        json: false,
        scratch_age_days: crate::scratch_reclaim::DEFAULT_MIN_AGE_DAYS,
        no_scratch: true,
    };
    let Ok(survey) = crate::worktree_reclaim::survey_from_env(project_root, &opts) else {
        return 0;
    };
    let plan = crate::worktree_reclaim::plan(&survey);
    crate::worktree_reclaim::apply_until(&plan, deadline, |p| std::fs::remove_dir_all(p))
        .map(|done| done.freed_bytes)
        .unwrap_or(0)
}

/// A finding the guard's floor reclaim can act on (the project or build-cache
/// filesystem below the floor; the Cargo slot tmpfs is not reclaimable here).
// trace:STORY-2351 | ai:claude
pub(super) fn is_floor_finding(f: &DoctorFinding) -> bool {
    f.id == disk_headroom_finding_id("project") || f.id == disk_headroom_finding_id("build-cache")
}

/// The light report around its expensive, injected halves: the emergency
/// run, the floor reclaim and the stuck scan (a reclaim dry run: `gh`, the
/// offline merge scan, a walk of every cache). All run only in the substrate
/// run (TASK-2264): a manual light check reports free space, the floor breach
/// and the status block, and stays cheap.
///
/// STORY-2351: on a floor breach the substrate run reclaims first and
/// re-probes, so a breach the reclaim fixes never routes to the advisor, and
/// one it does not fix routes saying what was already freed.
// trace:STORY-1916 | ai:claude
// trace:STORY-2351 | ai:claude
pub(super) fn disk_headroom_light_report_finish(
    project_root: &std::path::Path,
    cfg: Option<&toml::Value>,
    scheduled: bool,
    emergency_run: impl FnOnce(),
    build_report: impl Fn() -> DoctorReport,
    floor_reclaim: impl FnOnce() -> u64,
    stuck_scan: impl FnMut(u64, &str) -> (u64, Vec<(std::path::PathBuf, u64)>),
) -> DoctorReport {
    if scheduled {
        emergency_run();
    }
    let mut report = build_report();
    if scheduled && report.findings.iter().any(is_floor_finding) {
        let freed = floor_reclaim();
        report = report_after_floor_reclaim(build_report(), freed);
    }
    let min_free_gib = disk_headroom_min_free_gib(cfg);
    if scheduled {
        escalate_stuck_floor(project_root, &mut report.findings, min_free_gib, stuck_scan);
    }
    report.disk_status = Some(crate::disk_emergency::status(
        project_root,
        min_free_gib,
        crate::disk_emergency::emergency_config(cfg),
    ));
    report
}

/// The re-probed report after the guard's floor reclaim: a floor finding
/// still standing says how much the guard already freed, so the routed item
/// does not ask the advisor to repeat the reclaim.
// trace:STORY-2351 | ai:claude
pub(super) fn report_after_floor_reclaim(mut report: DoctorReport, freed: u64) -> DoctorReport {
    for f in report.findings.iter_mut().filter(|f| is_floor_finding(f)) {
        f.summary = format!(
            "{} (the guard already ran `aida worktree reclaim --apply` and freed {})",
            f.summary,
            crate::worktree_reclaim::human_bytes(freed)
        );
    }
    report
}

/// STORY-1916 AC3: for each filesystem below the floor, compare what a
/// reclaim dry run could free there (`stuck_scan`, keyed by device and
/// role) with the deficit. When it cannot restore the floor, say so in the
/// finding (the routed item carries it), with the top cache consumers, and
/// notify the operator, deduped. Substrate run only.
// trace:STORY-1916 | ai:claude
pub(super) fn escalate_stuck_floor(
    project_root: &std::path::Path,
    findings: &mut [DoctorFinding],
    min_free_gib: u64,
    mut stuck_scan: impl FnMut(u64, &str) -> (u64, Vec<(std::path::PathBuf, u64)>),
) {
    use crate::disk_emergency as de;
    let floor = min_free_gib.saturating_mul(1024 * 1024 * 1024);
    for (role, path, stats) in de::probe_paths(project_root, de::fs_stats) {
        if stats.avail >= floor {
            continue;
        }
        let id = disk_headroom_finding_id(role);
        let Some(finding) = findings.iter_mut().find(|f| f.id == id) else {
            continue;
        };
        let deficit = floor - stats.avail;
        let (reclaimable, top) = stuck_scan(stats.dev, role);
        if reclaimable >= deficit {
            continue;
        }
        let text = de::stuck_summary(reclaimable, deficit, &top);
        finding.summary = format!("{}. {text}", finding.summary);
        let mut state = de::load_guard_state(project_root);
        let now = chrono::Utc::now();
        if !de::should_escalate(&state, now, deficit) {
            continue;
        }
        let title = format!(
            "Disk floor cannot be restored automatically ({})",
            path.display()
        );
        if crate::notify::send_direct(project_root, "disk-stuck", &title, &finding.summary).is_ok()
        {
            state.last_stuck_at = Some(now);
            state.last_stuck_deficit = Some(deficit);
            let _ = de::save_guard_state(project_root, &state);
        }
    }
}

/// The finding id for a probed filesystem's floor breach.
// trace:STORY-1916 | ai:claude
pub(super) fn disk_headroom_finding_id(role: &str) -> &'static str {
    if role == "build-cache" {
        "disk-headroom-cache"
    } else {
        "disk-headroom"
    }
}

/// [`disk_headroom_light_report`] with the free-space probe injected, so a
/// test can pin both verdicts without depending on the host's real disk.
// trace:BUG-1675 | ai:claude
pub(super) fn disk_headroom_light_report_with(
    project_root: &std::path::Path,
    free_bytes: impl Fn(&std::path::Path) -> Option<u64>,
    // BUG-1702: injected, not discovered. Reading the real `$XDG_RUNTIME_DIR` here would put
    // host state back into tests whose whole point is a pinned verdict.
    // trace:BUG-1702 | ai:claude
    cargo_slot_dir: Option<std::path::PathBuf>,
) -> DoctorReport {
    // STORY-1916: the build cache may be its own filesystem (a bind mount of
    // `/mnt/build` on the dev host); the project-root probe never saw it.
    let cache_fs =
        crate::disk_emergency::probe_paths(project_root, crate::disk_emergency::fs_stats)
            .into_iter()
            .find(|(role, _, _)| *role == "build-cache")
            .map(|(_, path, _)| path);
    disk_headroom_light_report_probing(project_root, free_bytes, cargo_slot_dir, cache_fs)
}

/// [`disk_headroom_light_report_with`] with the separate build-cache
/// filesystem injected too.
// trace:STORY-1916 | ai:claude
pub(super) fn disk_headroom_light_report_probing(
    project_root: &std::path::Path,
    free_bytes: impl Fn(&std::path::Path) -> Option<u64>,
    cargo_slot_dir: Option<std::path::PathBuf>,
    cache_fs: Option<std::path::PathBuf>,
) -> DoctorReport {
    let cfg = crate::read_project_config_value(project_root);
    let min_free_gib = disk_headroom_min_free_gib(cfg.as_ref());
    let mut findings = disk_headroom_findings(free_bytes(project_root), min_free_gib);
    // trace:STORY-1916 | ai:claude
    if let Some(cache_fs) = cache_fs {
        findings.extend(
            disk_headroom_findings(free_bytes(&cache_fs), min_free_gib)
                .into_iter()
                .map(|mut f| {
                    f.id = disk_headroom_finding_id("build-cache").to_string();
                    f.summary = format!(
                        "build-cache filesystem ({}): {}",
                        cache_fs.display(),
                        f.summary
                    );
                    f
                }),
        );
    }
    // BUG-1702: the runtime tmpfs holding the Cargo slot locks is a separate, much smaller
    // filesystem than the project's, so the project-root floor above never sees it fill.
    // trace:BUG-1702 | ai:claude
    if let Some(slot_dir) = cargo_slot_dir {
        findings.extend(cargo_slot_permission_findings(&slot_dir, unsafe {
            libc::geteuid()
        }));
        findings.extend(cargo_slot_findings(
            free_bytes(&slot_dir),
            &cargo_slot_target_artifacts(&slot_dir),
            CARGO_SLOT_MIN_FREE_GIB,
        ));
    }
    DoctorReport::from_findings(findings)
}

/// A Rust link step for this workspace needs comfortably more than a GiB of scratch; below this
/// the failure mode is "Disk full?" and a bus error, not a compile error.
// trace:BUG-1702 | ai:claude
pub(super) const CARGO_SLOT_MIN_FREE_GIB: u64 = 1;

/// STORY-1367 default: the free-space floor a project has not configured
/// explicitly. Matches the number measured against the incident this job
/// exists to catch (root hit 100% with 16K free; a check at this floor would
/// have surfaced it hours earlier).
// trace:STORY-1367 | ai:claude
pub(super) const DEFAULT_DISK_HEADROOM_MIN_FREE_GIB: u64 = 60;

/// The doctor category name the disk-headroom job is dispatched under.
// trace:BUG-1675 | ai:claude
pub(crate) const DISK_HEADROOM_CATEGORY: &str = "disk-headroom";

/// Read `[doctor.disk_headroom] min_free_gib`, falling back to the default
/// floor. A project on a smaller or larger disk than the measured incident
/// tunes this rather than the check code.
// trace:STORY-1367 | ai:claude
pub(crate) fn disk_headroom_min_free_gib(cfg: Option<&toml::Value>) -> u64 {
    cfg.and_then(|c| c.get("doctor"))
        .and_then(|d| d.get("disk_headroom"))
        .and_then(|d| d.get("min_free_gib"))
        .and_then(|v| v.as_integer())
        .filter(|v| *v > 0)
        .map(|v| v as u64)
        .unwrap_or(DEFAULT_DISK_HEADROOM_MIN_FREE_GIB)
}

/// Pure threshold judgment, separated from the disk probe so the floor logic
/// is testable without manufacturing a full filesystem (same shape as
/// `machine_readiness::decide_capacity`). `None` = healthy = no finding — a
/// job that reports every run trains people to ignore it.
// trace:STORY-1367 | ai:claude
pub(super) fn disk_headroom_finding(free_bytes: u64, min_free_gib: u64) -> Option<DoctorFinding> {
    let min_free_bytes = min_free_gib.saturating_mul(1024 * 1024 * 1024);
    if free_bytes >= min_free_bytes {
        return None;
    }
    let free_gib = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    Some(DoctorFinding {
        category: "disk-headroom".to_string(),
        id: "disk-headroom".to_string(),
        summary: format!(
            "free disk space is {free_gib:.1} GiB, below the {min_free_gib} GiB floor \
             (`[doctor.disk_headroom] min_free_gib`)"
        ),
        // The remediation must name a command that can actually reclaim THIS
        // space. `cargo clean` cannot: run inside a worktree it clears the
        // shared CARGO_TARGET_DIR, which points at the main checkout, so it
        // deletes the live fleet's cache and leaves every stale one in place.
        // trace:TASK-1562 | ai:claude
        action: "reclaim space — stale worktree build caches (`aida worktree reclaim`, dry run \
                 by default), finished worktrees (`aida session reap`), or raise \
                 `min_free_gib` deliberately if the floor no longer matches this host. \
                 `cargo clean` does not reclaim this space: inside a worktree it clears the \
                 shared CARGO_TARGET_DIR pointing at the main checkout, deleting the live \
                 cache rather than the stale ones"
            .to_string(),
        safe_heal: false,
    })
}

/// The Cargo slot coordination directory, `$XDG_RUNTIME_DIR/cargo-slots-<uid>`.
///
/// AIDA does not create this directory — it is a host convention, and seats are handed
/// `--add-dir <it>` so codex can `flock` the slot files. None when the runtime dir is unset or
/// the directory does not exist, so hosts without the convention stay silent.
// trace:BUG-1702 | ai:claude
pub(super) fn cargo_slot_dir() -> Option<std::path::PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let uid = unsafe { libc::geteuid() };
    let path = std::path::PathBuf::from(runtime).join(format!("cargo-slots-{uid}"));
    path.is_dir().then_some(path)
}

// Discovery for diagnostics deliberately differs from launch admission: an unsafe
// existing directory must remain visible and must never be widened by doctor.
// trace:STORY-2326 | ai:codex
pub(super) fn cargo_slot_permission_findings(
    path: &std::path::Path,
    uid: u32,
) -> Vec<DoctorFinding> {
    use std::os::unix::fs::MetadataExt;
    let problem = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && m.uid() == uid && m.mode() & 0o7777 == 0o700 => return vec![],
        Ok(m) => format!("uid {}, mode {:04o}", m.uid(), m.mode() & 0o7777),
        Err(err) => format!("unreadable: {err}"),
    };
    vec![DoctorFinding {
        category: "disk-headroom".into(),
        id: "cargo-slot-permissions".into(),
        summary: format!("Cargo slot directory {} is not a uid-{uid}-owned mode-0700 directory ({problem}); launch resource admission refuses it", path.display()),
        action: "Ask the host operator to repair the cargo slot directory ownership and permissions".into(),
        safe_heal: false,
    }]
}

/// Entries in the slot directory that mean a Cargo TARGET directory was pointed at it.
///
/// The slot directory is for advisory lock files. A target directory there fills the runtime
/// tmpfs, which is small. Returns `(name, marker)` pairs. Pure over `dir`.
// trace:BUG-1702 | ai:claude
pub(super) fn cargo_slot_target_artifacts(dir: &std::path::Path) -> Vec<String> {
    const PROFILE_DIRS: [&str; 4] = ["debug", "release", "wasm32-unknown-unknown", "tmp"];
    const TARGET_MARKERS: [&str; 3] = [".rustc_info.json", "CACHEDIR.TAG", ".cargo-lock"];
    let mut found = Vec::new();
    for marker in TARGET_MARKERS {
        if dir.join(marker).exists() {
            found.push(marker.to_string());
        }
    }
    for profile in PROFILE_DIRS {
        let candidate = dir.join(profile);
        // An EMPTY `tmp/` is normal scratch; only a populated one is a target artifact.
        let populated = std::fs::read_dir(&candidate)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if candidate.is_dir() && populated {
            found.push(format!("{profile}/"));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// BUG-1702: the runtime tmpfs holding the Cargo slot locks is small (7.1 GiB on the dev host).
/// On 2026-09-27 it held a 6.8 GiB `debug/` target directory and a 280 MiB per-review target
/// directory, at 0 bytes free; an implementer's build died at link time with "Disk full?" and a
/// bus error, and it correctly refused to commit anything. A full `/run/user` also threatens the
/// systemd user session and dbus sockets, so this is not only a build failure mode.
///
/// Two findings, because they need different actions: the artifacts are the cause, the free space
/// is the symptom. Pure over its inputs so both verdicts are testable without a real tmpfs.
// trace:BUG-1702 | ai:claude
pub(super) fn cargo_slot_findings(
    free_bytes: Option<u64>,
    artifacts: &[String],
    min_free_gib: u64,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();

    if !artifacts.is_empty() {
        findings.push(DoctorFinding {
            category: "disk-headroom".to_string(),
            id: "cargo-slot-target-dir".to_string(),
            summary: format!(
                "the Cargo slot directory holds build artifacts ({}) — a target directory was \
                 pointed at the runtime tmpfs, which is for lock files only",
                artifacts.join(", ")
            ),
            action: "never set CARGO_TARGET_DIR under $XDG_RUNTIME_DIR — `--add-dir <slot dir>` \
                     grants lock access, not a target location. Delete the artifacts and KEEP \
                     every `*.lock` file"
                .to_string(),
            safe_heal: false,
        });
    }

    if let Some(free) = free_bytes {
        let floor = min_free_gib.saturating_mul(1024 * 1024 * 1024);
        if free < floor {
            let free_mib = free as f64 / (1024.0 * 1024.0);
            findings.push(DoctorFinding {
                category: "disk-headroom".to_string(),
                id: "cargo-slot-free".to_string(),
                summary: format!(
                    "the Cargo slot filesystem has {free_mib:.0} MiB free, below the \
                     {min_free_gib} GiB a link step needs — builds will fail with \"Disk full?\" \
                     or a bus error rather than a compile error"
                ),
                action: "reclaim the runtime tmpfs (`du -sh $XDG_RUNTIME_DIR/*`), keeping the \
                         slot `*.lock` files; a seat that fails a build here is not failing on \
                         your code"
                    .to_string(),
                safe_heal: false,
            });
        }
    }

    findings
}

pub(super) fn scan_disk_headroom(
    project_root: &std::path::Path,
    min_free_gib: u64,
) -> Vec<DoctorFinding> {
    disk_headroom_findings(disk_free_bytes(project_root), min_free_gib)
}

/// Free bytes on the filesystem holding `project_root` (the longest mount
/// point that prefixes it), or `None` when no mount resolves.
// trace:STORY-1367 | ai:claude
pub(super) fn disk_free_bytes(project_root: &std::path::Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| project_root.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// An unresolved filesystem is silent rather than a false positive.
// trace:BUG-1675 | ai:claude
pub(super) fn disk_headroom_findings(
    free_bytes: Option<u64>,
    min_free_gib: u64,
) -> Vec<DoctorFinding> {
    free_bytes
        .and_then(|free| disk_headroom_finding(free, min_free_gib))
        .into_iter()
        .collect()
}
