//! Disk-saturation evidence for lock waits.
//!
//! On a saturated disk every small fsync-heavy write (git, SQLite, mail) queues
//! behind the device, so a lock holder can sit in uninterruptible disk sleep
//! (`D` state) for minutes. To a waiter that looks exactly like a stuck or
//! stale lock. This module reads the kernel's IO pressure (`/proc/pressure/io`)
//! and the holder's process state (`/proc/<pid>/stat`) so a wait can say "the
//! disk is saturated" instead, and so the stale-lock healer never steals a
//! lock from a holder that is only waiting on the disk.
//!
//! Linux only; elsewhere every probe returns `None` and nothing is reported.
//!
//! trace:BUG-HOME-041 | ai:claude

use std::time::{Duration, Instant};

/// `some avg10` at or above this percentage means the disk is saturated.
pub const SATURATED_SOME_AVG10_PCT: f64 = 50.0;

/// How long a lock wait runs before it checks for disk saturation.
pub const LOCK_WAIT_NOTICE_AFTER: Duration = Duration::from_secs(5);

/// The `avg10` value of the `some` line of a `/proc/pressure/io` body:
///
/// ```text
/// some avg10=85.12 avg60=80.01 avg300=79.50 total=123456
/// full avg10=40.00 avg60=38.20 avg300=37.90 total=654321
/// ```
pub fn parse_io_pressure_some_avg10(body: &str) -> Option<f64> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix("some "))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse()
        .ok()
}

/// The single-letter state (field 3) of a `/proc/<pid>/stat` line. The command
/// name (field 2) is parenthesised and may contain spaces or `)`, so parsing
/// resumes after the LAST `)`.
pub fn parse_proc_stat_state(stat: &str) -> Option<char> {
    stat[stat.rfind(')')? + 1..]
        .split_whitespace()
        .next()?
        .chars()
        .next()
}

/// Current `some avg10` IO pressure, or `None` off Linux, on a kernel without
/// PSI, or in this crate's unit tests (which never read the real machine).
pub fn read_io_pressure_some_avg10() -> Option<f64> {
    if cfg!(test) || !cfg!(target_os = "linux") {
        return None;
    }
    parse_io_pressure_some_avg10(&std::fs::read_to_string("/proc/pressure/io").ok()?)
}

/// State letter of a process visible in this PID namespace (`D` = waiting on
/// disk), or `None` when it is not visible or not on Linux.
pub fn process_state(pid: u32) -> Option<char> {
    if pid == 0 || !cfg!(target_os = "linux") {
        return None;
    }
    parse_proc_stat_state(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// True when `pid` is visible here and in uninterruptible disk sleep.
pub fn waiting_on_disk(pid: u32) -> bool {
    process_state(pid) == Some('D')
}

/// Evidence that the disk, not a stuck holder, explains a lock wait.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskSaturation {
    /// `some avg10` from `/proc/pressure/io`, a percentage.
    pub some_avg10: f64,
    /// The holder's pid and state letter, when its pid is visible here.
    pub holder: Option<(u32, char)>,
}

impl DiskSaturation {
    /// True when the holder is visible and in `D` state.
    pub fn holder_on_disk(&self) -> bool {
        matches!(self.holder, Some((_, 'D')))
    }

    /// One sentence for a wait message or a terminal lock error.
    pub fn describe(&self) -> String {
        let holder = match self.holder {
            Some((pid, 'D')) => {
                format!("; holder pid {pid} is in state D (waiting on disk), so it is not stuck")
            }
            Some((pid, state)) => format!("; holder pid {pid} is in state {state}"),
            None => String::new(),
        };
        format!(
            "the disk is saturated (IO pressure {:.0}% over the last 10s){holder}. \
             The wait clears as the disk catches up; this is not a stale lock",
            self.some_avg10
        )
    }
}

/// [`observe_disk_saturation`] with the probes injected.
pub fn disk_saturation_with(
    some_avg10: Option<f64>,
    holder_pid: Option<u32>,
    state: impl Fn(u32) -> Option<char>,
) -> Option<DiskSaturation> {
    let some_avg10 = some_avg10.filter(|pct| *pct >= SATURATED_SOME_AVG10_PCT)?;
    Some(DiskSaturation {
        some_avg10,
        holder: holder_pid.and_then(|pid| state(pid).map(|s| (pid, s))),
    })
}

/// Disk-saturation evidence right now, or `None` when the disk is not
/// saturated (or pressure cannot be read). `holder_pid` must be a pid visible
/// in this PID namespace.
pub fn observe_disk_saturation(holder_pid: Option<u32>) -> Option<DiskSaturation> {
    disk_saturation_with(read_io_pressure_some_avg10(), holder_pid, process_state)
}

/// Emits one disk-saturation notice on stderr once a lock wait passes
/// [`LOCK_WAIT_NOTICE_AFTER`]. Silent when the disk is not saturated, so an
/// ordinary wait prints exactly what it printed before.
#[derive(Debug)]
pub struct LockWaitNotice {
    started: Instant,
    checked: bool,
}

impl Default for LockWaitNotice {
    fn default() -> Self {
        Self::new()
    }
}

impl LockWaitNotice {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            checked: false,
        }
    }

    /// Call on every wait iteration. `what` names the lock ("store write
    /// lock"); `holder_pid` is called at most once, past the threshold.
    pub fn tick(&mut self, what: &str, holder_pid: impl FnOnce() -> Option<u32>) {
        if self.checked || self.started.elapsed() < LOCK_WAIT_NOTICE_AFTER {
            return;
        }
        self.checked = true;
        if let Some(disk) = observe_disk_saturation(holder_pid()) {
            eprintln!("aida: still waiting for the {what}: {}.", disk.describe());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PSI: &str = "some avg10=85.12 avg60=80.01 avg300=79.50 total=123456\n\
                       full avg10=40.00 avg60=38.20 avg300=37.90 total=654321\n";

    #[test]
    fn parses_some_avg10_from_fixture() {
        assert_eq!(parse_io_pressure_some_avg10(PSI), Some(85.12));
        assert_eq!(
            parse_io_pressure_some_avg10("some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"),
            Some(0.0)
        );
        // `full` alone, garbage and empty input yield nothing.
        assert_eq!(
            parse_io_pressure_some_avg10("full avg10=40.00 avg60=1 avg300=1 total=1"),
            None
        );
        assert_eq!(parse_io_pressure_some_avg10("some avg10=x"), None);
        assert_eq!(parse_io_pressure_some_avg10(""), None);
    }

    #[test]
    fn parses_state_after_last_paren() {
        assert_eq!(
            parse_proc_stat_state("2427682 (aida) D 1 2 3 0 -1"),
            Some('D')
        );
        assert_eq!(
            parse_proc_stat_state("19 (cache (re)fresh) S 1 2 3"),
            Some('S')
        );
        assert_eq!(parse_proc_stat_state("garbage"), None);
    }

    #[test]
    fn saturation_threshold_and_holder_state() {
        let d = |_| Some('D');
        assert_eq!(disk_saturation_with(Some(49.9), Some(7), d), None);
        assert_eq!(disk_saturation_with(None, Some(7), d), None);
        let sat = disk_saturation_with(Some(85.0), Some(7), d).unwrap();
        assert!(sat.holder_on_disk());
        let text = sat.describe();
        assert!(
            text.contains("disk is saturated (IO pressure 85%"),
            "{text}"
        );
        assert!(
            text.contains("pid 7 is in state D (waiting on disk)"),
            "{text}"
        );
        assert!(text.contains("not a stale lock"), "{text}");
        assert!(text.contains("so it is not stuck"), "{text}");
        assert!(!text.contains("If it's stuck"), "{text}");

        // An invisible holder is simply not named.
        let hidden = disk_saturation_with(Some(60.0), Some(7), |_| None).unwrap();
        assert_eq!(hidden.holder, None);
        assert!(!hidden.holder_on_disk());
        assert!(!hidden.describe().contains("pid"));
    }
}
