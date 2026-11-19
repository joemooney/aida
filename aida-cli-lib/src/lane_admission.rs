//! Host-wide admission for managed headless implementer starts.
// trace:BUG-2682 | ai:codex

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::ErrorKind;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::Duration;

const CONFIG: &str = "/etc/aida/lanes.toml";
const SLOT_PREFIX: &str = "aida-lane-cold-start-v1";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const KIB_PER_GIB: u64 = 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HostConfig {
    lanes: Settings,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    max_concurrent_cold_starts: u16,
    min_memory_headroom_gib: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_concurrent_cold_starts: 2,
            min_memory_headroom_gib: 4,
        }
    }
}

impl Settings {
    fn load() -> Result<Self> {
        let settings = match fs::read_to_string(CONFIG) {
            Ok(body) => {
                toml::from_str::<HostConfig>(&body)
                    .with_context(|| format!("invalid lane admission settings in {CONFIG}"))?
                    .lanes
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error).with_context(|| format!("cannot read {CONFIG}")),
        };
        anyhow::ensure!(
            (1..=256).contains(&settings.max_concurrent_cold_starts),
            "{CONFIG}: lanes.max_concurrent_cold_starts must be in 1..=256"
        );
        anyhow::ensure!(
            (1..=u64::MAX / KIB_PER_GIB).contains(&settings.min_memory_headroom_gib),
            "{CONFIG}: lanes.min_memory_headroom_gib is out of range"
        );
        Ok(settings)
    }
}

fn memory_headroom() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    headroom_from_meminfo(&meminfo)
}

fn headroom_from_meminfo(meminfo: &str) -> Result<u64> {
    let read_kib = |key: &str| -> Result<u64> {
        let mut fields = meminfo
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .with_context(|| format!("missing {key} in /proc/meminfo"))?
            .split_whitespace();
        let value = fields.next().context("missing memory value")?.parse()?;
        anyhow::ensure!(
            fields.next() == Some("kB") && fields.next().is_none(),
            "invalid units for {key} in /proc/meminfo"
        );
        Ok(value)
    };
    read_kib("MemAvailable:")?
        .checked_add(read_kib("SwapFree:")?)
        .context("host memory headroom overflow")
}

fn wait_for_slot(
    settings: &Settings,
    prefix: &str,
    memory: impl Fn() -> Result<u64>,
    mut on_wait: impl FnMut(),
) -> Result<UnixDatagram> {
    let check_memory = || {
        let available =
            memory().context("lane launch refused: cannot measure host memory headroom")?;
        anyhow::ensure!(
            available >= settings.min_memory_headroom_gib * KIB_PER_GIB,
            "lane launch refused: host memory headroom is {:.2} GiB (available RAM + free swap), below the {} GiB floor (lanes.min_memory_headroom_gib)",
            available as f64 / KIB_PER_GIB as f64,
            settings.min_memory_headroom_gib
        );
        Ok(())
    };
    loop {
        for index in 0..settings.max_concurrent_cold_starts {
            // Abstract sockets are scoped to the host launchers' network
            // namespace and released by the kernel even after a crash.
            let address = SocketAddr::from_abstract_name(format!("{prefix}.{index}"))?;
            match UnixDatagram::bind_addr(&address) {
                Ok(slot) => {
                    check_memory()?;
                    return Ok(slot);
                }
                Err(error) if error.kind() == ErrorKind::AddrInUse => {}
                Err(error) => {
                    return Err(error)
                        .context("lane launch refused: cannot reserve cold-start slot")
                }
            }
        }
        check_memory()?;
        on_wait();
    }
}

/// The launch owns the slot. The runner and timer can release it without
/// keeping it alive after an early return or failed spawn.
struct Slot {
    socket: Mutex<Option<UnixDatagram>>,
    released: Condvar,
}

pub(crate) struct Startup(Arc<Slot>);

pub(crate) struct Release(Weak<Slot>);

impl Release {
    pub(crate) fn first_job_finished(&self) {
        if let Some(slot) = self.0.upgrade() {
            slot.socket.lock().unwrap().take();
            slot.released.notify_one();
        }
    }
}

impl Startup {
    pub(crate) fn acquire() -> Result<Self> {
        let settings = Settings::load()?;
        let mut announced = false;
        let slot = wait_for_slot(&settings, SLOT_PREFIX, memory_headroom, || {
            if !announced {
                eprintln!(
                    "lane launch queued: host cold-start limit reached (lanes.max_concurrent_cold_starts = {})",
                    settings.max_concurrent_cold_starts
                );
                announced = true;
            }
            thread::sleep(Duration::from_secs(1));
        })?;
        Self::with_timeout(slot, STARTUP_TIMEOUT)
    }

    fn with_timeout(slot: UnixDatagram, timeout: Duration) -> Result<Self> {
        let slot = Arc::new(Slot {
            socket: Mutex::new(Some(slot)),
            released: Condvar::new(),
        });
        let timer_slot = Arc::clone(&slot);
        thread::Builder::new()
            .name("lane-cold-start-timeout".into())
            .spawn(move || {
                let socket = timer_slot.socket.lock().unwrap();
                let (mut socket, _) = timer_slot
                    .released
                    .wait_timeout_while(socket, timeout, |socket| socket.is_some())
                    .unwrap();
                socket.take();
            })
            .context("lane launch refused: cannot start cold-start timeout")?;
        Ok(Self(slot))
    }

    #[cfg(test)]
    pub(crate) fn test_reservation() -> Self {
        Self::with_timeout(UnixDatagram::unbound().unwrap(), STARTUP_TIMEOUT).unwrap()
    }

    #[cfg(test)]
    pub(crate) fn is_reserved(&self) -> bool {
        self.0.socket.lock().unwrap().is_some()
    }

    pub(crate) fn release_handle(&self) -> Release {
        Release(Arc::downgrade(&self.0))
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        self.release_handle().first_job_finished();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_2682_queues_until_first_job_releases_slot() {
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = format!("aida-test-{}", uuid::Uuid::new_v4());
        let memory = || Ok(8 * KIB_PER_GIB);
        let slot =
            wait_for_slot(&settings, &prefix, memory, || panic!("first launch queued")).unwrap();
        let startup = Startup::with_timeout(slot, STARTUP_TIMEOUT).unwrap();
        let mut queued = 0;
        let _next = wait_for_slot(&settings, &prefix, memory, || {
            queued += 1;
            startup.release_handle().first_job_finished();
        })
        .unwrap();
        assert_eq!(queued, 1);
    }

    #[test]
    fn bug_2682_over_wip_still_queues_and_refuses_low_memory() {
        use crate::wip_limit::{verdict, Verdict};

        assert!(matches!(
            verdict("BUG-2682", 4, 4, true),
            Verdict::Override(_)
        ));
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = format!("aida-test-{}", uuid::Uuid::new_v4());
        let first = std::cell::Cell::new(Some(
            wait_for_slot(
                &settings,
                &prefix,
                || Ok(8 * KIB_PER_GIB),
                || panic!("first launch queued"),
            )
            .unwrap(),
        ));
        let queued = std::cell::Cell::new(false);
        let refused = wait_for_slot(
            &settings,
            &prefix,
            || Ok(if queued.get() { 0 } else { 8 * KIB_PER_GIB }),
            || {
                assert!(first.take().is_some(), "queued twice");
                queued.set(true);
            },
        );
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("below the 4 GiB floor"));
    }

    #[test]
    fn bug_2682_timeout_releases_startup_slot() {
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = format!("aida-test-{}", uuid::Uuid::new_v4());
        let memory = || Ok(8 * KIB_PER_GIB);
        let first =
            wait_for_slot(&settings, &prefix, memory, || panic!("first launch queued")).unwrap();
        let _startup = Startup::with_timeout(first, Duration::from_millis(10)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let _next = wait_for_slot(&settings, &prefix, memory, || {
            assert!(
                std::time::Instant::now() < deadline,
                "startup timeout leaked a slot"
            );
            thread::sleep(Duration::from_millis(1));
        })
        .unwrap();
    }

    #[test]
    fn bug_2682_refuses_low_or_unreadable_memory_and_returns_slot() {
        let settings = Settings::default();
        let prefix = format!("aida-test-{}", uuid::Uuid::new_v4());
        let low = || headroom_from_meminfo("MemAvailable: 2097152 kB\nSwapFree: 1048576 kB\n");
        let error =
            wait_for_slot(&settings, &prefix, low, || panic!("unexpected queue")).unwrap_err();
        assert!(error.to_string().contains("below the 4 GiB floor"));
        let unknown = || headroom_from_meminfo("MemAvailable: 2097152 kB\n");
        let error =
            wait_for_slot(&settings, &prefix, unknown, || panic!("unexpected queue")).unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot measure host memory headroom"));
        let _normal = wait_for_slot(
            &settings,
            &prefix,
            || Ok(4 * KIB_PER_GIB),
            || panic!("slot leaked"),
        )
        .unwrap();
    }
}
