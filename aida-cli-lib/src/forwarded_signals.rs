//! Forward the terminal signals this process receives to a child that runs
//! in its own process group (STORY-1917).
//!
//! A child put in its own group no longer gets a terminal's Ctrl+C directly,
//! so while [`Forwarding`] is alive SIGINT, SIGTERM and SIGHUP are caught
//! here and handed to the child's tree by the caller's poll loop. A signal
//! the process was already ignoring (`nohup` → SIGHUP) is left alone, so the
//! child keeps inheriting that ignore. The previous dispositions come back
//! when the last [`Forwarding`] drops; [`reraise`] then lets a forwarded
//! signal end this process the way it would have without forwarding.
//!
//! trace:STORY-1917 | ai:claude

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Times each of [`SIGNALS`] arrived. Counters rather than a pending flag, so
/// every live [`Forwarding`] sees every signal.
static COUNTS: [AtomicUsize; 3] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// Live [`Forwarding`] count and, per signal, the disposition replaced while
/// it is non-zero (`None`: the signal was ignored and is left alone).
static STATE: Mutex<(usize, [Option<libc::sigaction>; 3])> = Mutex::new((0, [None; 3]));

extern "C" fn on_signal(sig: libc::c_int) {
    // Async-signal-safe: atomic increments only.
    for (i, s) in SIGNALS.iter().enumerate() {
        if *s == sig {
            COUNTS[i].fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Catches SIGINT/SIGTERM/SIGHUP while alive; [`Forwarding::take`] reports
/// each one that arrived since the last call.
pub(crate) struct Forwarding {
    seen: [usize; 3],
    // Tests raise real signals at this process; see `tests::exclusive`.
    #[cfg(test)]
    _gate: Option<std::sync::RwLockReadGuard<'static, ()>>,
}

impl Forwarding {
    pub(crate) fn install() -> std::io::Result<Self> {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.0 == 0 {
            for (i, sig) in SIGNALS.iter().enumerate() {
                // SAFETY: zeroed `sigaction` is a valid "default" value; the
                // calls only read/replace this process's dispositions.
                unsafe {
                    let mut old: libc::sigaction = std::mem::zeroed();
                    if libc::sigaction(*sig, std::ptr::null(), &mut old) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if old.sa_sigaction == libc::SIG_IGN {
                        state.1[i] = None;
                        continue;
                    }
                    let mut new: libc::sigaction = std::mem::zeroed();
                    new.sa_sigaction = on_signal as libc::sighandler_t;
                    new.sa_flags = libc::SA_RESTART;
                    libc::sigemptyset(&mut new.sa_mask);
                    if libc::sigaction(*sig, &new, std::ptr::null_mut()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    state.1[i] = Some(old);
                }
            }
        }
        state.0 += 1;
        #[cfg(test)]
        let _gate = (!tests::EXCLUSIVE.with(|c| c.get()))
            .then(|| tests::GATE.read().unwrap_or_else(|e| e.into_inner()));
        Ok(Self {
            seen: std::array::from_fn(|i| COUNTS[i].load(Ordering::SeqCst)),
            #[cfg(test)]
            _gate,
        })
    }

    /// The next signal that arrived since the previous call, if any.
    pub(crate) fn take(&mut self) -> Option<libc::c_int> {
        for (i, sig) in SIGNALS.iter().enumerate() {
            let now = COUNTS[i].load(Ordering::SeqCst);
            if now != self.seen[i] {
                self.seen[i] = now;
                return Some(*sig);
            }
        }
        None
    }
}

impl Drop for Forwarding {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.0 -= 1;
        if state.0 == 0 {
            for (i, sig) in SIGNALS.iter().enumerate() {
                if let Some(old) = state.1[i].take() {
                    // SAFETY: restores the disposition saved by `install`.
                    unsafe {
                        libc::sigaction(*sig, &old, std::ptr::null_mut());
                    }
                }
            }
        }
    }
}

/// Deliver `sig` to this process again, after every [`Forwarding`] has
/// dropped, so it acts as it would have without forwarding (by default:
/// terminates the process).
pub(crate) fn reraise(sig: libc::c_int) {
    // A test cannot survive its own SIGINT; it asserts the forwarding instead.
    if cfg!(test) {
        return;
    }
    // SAFETY: plain signal delivery to this process.
    unsafe {
        libc::raise(sig);
    }
}

/// Send `sig` to the process group of `root` (a child spawned with
/// `process_group(0)`) and to every other group its descendants in `tree`
/// moved into (a headless vendor launched in its own group, a `setsid`
/// helper), and to each descendant directly. This process's own group is
/// never signalled.
pub(crate) fn signal_child_tree(root: u32, tree: &[u32], sig: libc::c_int) {
    // SAFETY: `getpgrp`/`getpgid` only read process-group ids.
    let own = unsafe { libc::getpgrp() };
    let mut groups = vec![root as libc::pid_t];
    for &pid in tree {
        let g = unsafe { libc::getpgid(pid as libc::pid_t) };
        if g > 1 && g != own && !groups.contains(&g) {
            groups.push(g);
        }
    }
    // SAFETY: signal delivery to this process's own descendants and their
    // groups, all collected above; ESRCH (already gone) is ignored.
    unsafe {
        for g in groups {
            libc::killpg(g, sig);
        }
        for &pid in tree {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;
    use std::sync::RwLock;
    #[cfg(target_os = "linux")]
    use std::time::{Duration, Instant};

    /// Held shared by every [`super::Forwarding`] and exclusively by a test
    /// that signals this process, so a signal meant for one test's child is
    /// never forwarded to another's under a threaded `cargo test`.
    pub(crate) static GATE: RwLock<()> = RwLock::new(());
    thread_local! {
        pub(crate) static EXCLUSIVE: Cell<bool> = const { Cell::new(false) };
    }

    #[cfg(target_os = "linux")]
    fn wait_for(path: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if text.ends_with('\n') {
                    return text;
                }
            }
            assert!(
                Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A SIGINT this process receives reaches a grandchild the child started
    /// in a session (and process group) of its own, and the runner reports it.
    // trace:STORY-1917 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn sigint_to_the_parent_reaches_a_grandchild_in_its_own_group() {
        let _exclusive = GATE.write().unwrap_or_else(|e| e.into_inner());
        EXCLUSIVE.with(|c| c.set(true));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                EXCLUSIVE.with(|c| c.set(false));
            }
        }
        let _reset = Reset;

        let tmp = tempfile::tempdir().unwrap();
        let gpid = tmp.path().join("gpid");
        let got = tmp.path().join("got");
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                "setsid sh -c 'trap \"echo INT > {got}; exit 0\" INT; echo $$ > {gpid}; \
                 while :; do sleep 0.05; done'",
                got = got.display(),
                gpid = gpid.display()
            ))
            .stdin(std::process::Stdio::null());
        // The runner stays on this thread, which holds the exclusive gate;
        // a helper signals the process once the grandchild is up.
        let signaller = std::thread::spawn(move || {
            let grandchild: i32 = wait_for(&gpid).trim().parse().unwrap();
            // SAFETY: `getpgid`/`getpgrp`/`kill` only read ids or signal.
            unsafe {
                assert_eq!(libc::getpgid(grandchild), grandchild, "own group");
                assert_ne!(libc::getpgid(grandchild), libc::getpgrp());
                libc::kill(libc::getpid(), libc::SIGINT);
            }
        });
        let (_, interrupted) =
            crate::command_output_with_timeout_forwarding(cmd, Duration::from_secs(30), 1024)
                .unwrap();
        signaller.join().unwrap();
        assert_eq!(interrupted, Some(libc::SIGINT));
        assert_eq!(wait_for(&got), "INT\n");
    }
}
