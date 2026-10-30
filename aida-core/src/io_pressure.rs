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

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Overall bound on a fail-fast lock wait (`aida rel add`, `aida pr
/// auto-queue-review`): once spent, a wait whose holder is in another PID
/// namespace or blocked on a saturated disk gives up instead of running its
/// full timeout.
pub const FAIL_FAST_LOCK_BUDGET: Duration = Duration::from_secs(30);

/// Carries an armed fail-fast deadline (unix milliseconds) to child `aida`
/// processes, so a command's subcommands share one budget instead of each
/// restarting it.
pub const FAIL_FAST_DEADLINE_ENV: &str = "AIDA_LOCK_FAIL_FAST_DEADLINE_MS";

static FAIL_FAST_DEADLINE: std::sync::Mutex<Option<SystemTime>> = std::sync::Mutex::new(None);

/// Arm this process's fail-fast lock deadline: `budget` from now, or an
/// earlier deadline inherited from a parent through [`FAIL_FAST_DEADLINE_ENV`].
pub fn arm_fail_fast_lock_deadline(budget: Duration) -> SystemTime {
    let inherited = std::env::var(FAIL_FAST_DEADLINE_ENV).ok();
    let deadline = fail_fast_deadline_from(SystemTime::now(), budget, inherited.as_deref());
    *FAIL_FAST_DEADLINE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(deadline);
    deadline
}

/// [`arm_fail_fast_lock_deadline`]'s arithmetic with the clock injected.
pub fn fail_fast_deadline_from(
    now: SystemTime,
    budget: Duration,
    inherited_ms: Option<&str>,
) -> SystemTime {
    let own = now + budget;
    inherited_ms
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .map(|ms| UNIX_EPOCH + Duration::from_millis(ms))
        .map_or(own, |inherited| inherited.min(own))
}

/// The armed fail-fast deadline, if this command armed one.
pub fn fail_fast_lock_deadline() -> Option<SystemTime> {
    *FAIL_FAST_DEADLINE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The `(name, value)` env pair that hands the armed deadline to a child.
pub fn fail_fast_deadline_env() -> Option<(&'static str, String)> {
    let deadline = fail_fast_lock_deadline()?;
    let ms = deadline.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some((FAIL_FAST_DEADLINE_ENV, ms.to_string()))
}

/// Why a lock holder cannot be expected to let go soon.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockedHolder {
    /// The disk is saturated; the holder is waiting on the device.
    Disk(DiskSaturation),
    /// The holder runs in another PID namespace, so nothing here can probe
    /// it; only its heartbeat (silent this long) speaks for it.
    ForeignNamespace {
        pid: u32,
        command: String,
        silent: Option<Duration>,
    },
}

impl BlockedHolder {
    pub fn describe(&self) -> String {
        match self {
            BlockedHolder::Disk(disk) => disk.describe(),
            BlockedHolder::ForeignNamespace {
                pid,
                command,
                silent,
            } => {
                let heartbeat = silent
                    .map(|s| format!("; its last heartbeat was {}s ago", s.as_secs()))
                    .unwrap_or_default();
                format!(
                    "the holder (pid {pid}, {command}) runs in another PID namespace, so it \
                     cannot be probed from here{heartbeat}. On a busy disk such a holder can \
                     sit for minutes"
                )
            }
        }
    }
}

/// The fail-fast decision for one lock-wait iteration: once `now` reaches
/// `deadline`, give up when `evidence` names a blocked holder. Before the
/// deadline, or with no evidence, keep waiting (the wait's own timeout still
/// applies). `evidence` is only called past the deadline.
pub fn give_up_lock_wait(
    deadline: Option<SystemTime>,
    now: SystemTime,
    evidence: impl FnOnce() -> Option<BlockedHolder>,
) -> Option<BlockedHolder> {
    if now < deadline? {
        return None;
    }
    evidence()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_fast_deadline_gives_up_only_past_the_deadline_with_evidence() {
        let t0 = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let deadline = fail_fast_deadline_from(t0, FAIL_FAST_LOCK_BUDGET, None);
        assert_eq!(deadline, t0 + Duration::from_secs(30));
        let foreign = || {
            Some(BlockedHolder::ForeignNamespace {
                pid: 19,
                command: "aida cache refresh --worker".into(),
                silent: Some(Duration::from_secs(36_000)),
            })
        };
        // Unarmed: never gives up early.
        assert_eq!(
            give_up_lock_wait(None, t0 + Duration::from_secs(999), foreign),
            None
        );
        // Armed, before the deadline: keeps waiting, and never probes.
        assert_eq!(
            give_up_lock_wait(Some(deadline), t0 + Duration::from_secs(29), || {
                panic!("evidence probed before the deadline")
            }),
            None
        );
        // Past the deadline with a foreign-namespace holder: gives up.
        let blocked = give_up_lock_wait(Some(deadline), t0 + Duration::from_secs(30), foreign)
            .expect("a foreign holder past the deadline gives up");
        let text = blocked.describe();
        assert!(
            text.contains("pid 19") && text.contains("another PID namespace"),
            "{text}"
        );
        // Past the deadline with a saturated disk: gives up with the IO-pressure text.
        let disk = disk_saturation_with(Some(90.0), None, |_| None).map(BlockedHolder::Disk);
        let blocked = give_up_lock_wait(Some(deadline), t0 + Duration::from_secs(31), || disk)
            .expect("a saturated disk past the deadline gives up");
        assert!(blocked.describe().contains("IO pressure 90%"));
        // Past the deadline but nothing blocked: keep waiting.
        assert_eq!(
            give_up_lock_wait(Some(deadline), t0 + Duration::from_secs(60), || None),
            None
        );
    }

    #[test]
    fn fail_fast_deadline_inherits_an_earlier_parent_deadline() {
        let t0 = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let earlier = (t0 + Duration::from_secs(5))
            .duration_since(UNIX_EPOCH)
            .unwrap();
        assert_eq!(
            fail_fast_deadline_from(
                t0,
                FAIL_FAST_LOCK_BUDGET,
                Some(&earlier.as_millis().to_string())
            ),
            t0 + Duration::from_secs(5)
        );
        // A later or garbled inherited deadline never extends the budget.
        assert_eq!(
            fail_fast_deadline_from(t0, FAIL_FAST_LOCK_BUDGET, Some("99999999999999")),
            t0 + Duration::from_secs(30)
        );
        assert_eq!(
            fail_fast_deadline_from(t0, FAIL_FAST_LOCK_BUDGET, Some("soon")),
            t0 + Duration::from_secs(30)
        );
    }

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
