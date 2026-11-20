//! Host-wide admission for managed headless implementer starts: a cap on
//! concurrent cold starts and a memory-headroom floor. `--over-wip` never
//! reaches this; only the explicit host-limit override ([`OVERRIDE_ENV`],
//! with a reason) passes either check, and every pass is recorded.
// trace:BUG-2682 | ai:codex
// trace:BUG-2682 | ai:claude

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::ErrorKind;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::Duration;

#[cfg_attr(test, allow(dead_code))]
const CONFIG: &str = "/etc/aida/lanes.toml";
#[cfg_attr(test, allow(dead_code))]
const SLOT_PREFIX: &str = "aida-lane-cold-start-v1";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const KIB_PER_GIB: u64 = 1024 * 1024;

/// The explicit host-limit override: set in the launching command's
/// environment, its value is the reason recorded with the override. It is
/// independent of `--over-wip` and of any configuration file.
pub(crate) const OVERRIDE_ENV: &str = "AIDA_OVER_HOST_LIMIT";

/// BUG-2697: marks a launch error that happened before the agent spawned
/// (host admission, disk floor, gate reservation), so the pickup rolls back
/// its setup and releases its shared claim even after its launch mark.
// trace:BUG-2697 | ai:claude
#[derive(Debug)]
pub(crate) struct RefusedBeforeSpawn;

impl std::fmt::Display for RefusedBeforeSpawn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lane admission refused before the agent started")
    }
}

/// Mark `error` as a refusal before any agent spawned.
// trace:BUG-2697 | ai:claude
pub(crate) fn refused_before_spawn(error: anyhow::Error) -> anyhow::Error {
    error.context(RefusedBeforeSpawn)
}

/// Whether `error`, under any later context, carries [`RefusedBeforeSpawn`].
// trace:BUG-2697 | ai:claude
pub(crate) fn is_refused_before_spawn(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RefusedBeforeSpawn>().is_some()
}

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
    #[cfg_attr(test, allow(dead_code))]
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

#[cfg_attr(test, allow(dead_code))]
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

/// The explicit, reasoned host-limit override a launcher passes to
/// [`Startup::acquire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostOverride {
    reason: String,
}

impl HostOverride {
    /// The override [`OVERRIDE_ENV`] requests, if any. Set but blank is an
    /// error: an override must say why.
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let environment = aida_core::exec_context::Environment::capture();
        Self::parse(environment.get(OVERRIDE_ENV))
    }

    fn parse(value: Option<&std::ffi::OsStr>) -> Result<Option<Self>> {
        let Some(value) = value else {
            return Ok(None);
        };
        let reason = value.to_string_lossy().trim().to_string();
        anyhow::ensure!(
            !reason.is_empty(),
            "lane launch refused: {OVERRIDE_ENV} is set but empty; the host-limit override needs a reason (`{OVERRIDE_ENV}=\"<why>\"`)"
        );
        Ok(Some(Self { reason }))
    }
}

/// What admission granted: the slot (none when the cap was overridden) and
/// the host checks the override passed.
struct Admitted {
    slot: Option<UnixDatagram>,
    overridden: Vec<String>,
}

/// Why `memory` is below the floor, or `None` when it is not.
fn memory_shortfall(settings: &Settings, memory: &impl Fn() -> Result<u64>) -> Option<String> {
    match memory() {
        Err(error) => Some(format!("cannot measure host memory headroom: {error:#}")),
        Ok(available) if available < settings.min_memory_headroom_gib * KIB_PER_GIB => {
            Some(format!(
                "host memory headroom is {:.2} GiB (available RAM + free swap), below the {} GiB floor (lanes.min_memory_headroom_gib)",
                available as f64 / KIB_PER_GIB as f64,
                settings.min_memory_headroom_gib
            ))
        }
        Ok(_) => None,
    }
}

fn reserve(settings: &Settings, prefix: &str) -> Result<Option<UnixDatagram>> {
    for index in 0..settings.max_concurrent_cold_starts {
        // Abstract sockets are scoped to the host launchers' network
        // namespace and released by the kernel even after a crash.
        let address = SocketAddr::from_abstract_name(format!("{prefix}.{index}"))?;
        match UnixDatagram::bind_addr(&address) {
            Ok(slot) => return Ok(Some(slot)),
            Err(error) if error.kind() == ErrorKind::AddrInUse => {}
            Err(error) => {
                return Err(error).context("lane launch refused: cannot reserve cold-start slot")
            }
        }
    }
    Ok(None)
}

/// Take a cold-start slot, waiting (`on_wait`) while every slot is held, and
/// refuse below the memory floor. With `over`, a full cap or a memory
/// shortfall is passed and named instead.
fn admit(
    settings: &Settings,
    prefix: &str,
    memory: impl Fn() -> Result<u64>,
    over: bool,
    mut on_wait: impl FnMut(),
) -> Result<Admitted> {
    loop {
        let slot = reserve(settings, prefix)?;
        let mut overridden = Vec::new();
        if slot.is_none() {
            if !over {
                if let Some(why) = memory_shortfall(settings, &memory) {
                    anyhow::bail!("lane launch refused: {why}");
                }
                on_wait();
                continue;
            }
            overridden.push(format!(
                "cold-start cap: all {} slots (lanes.max_concurrent_cold_starts) are in use",
                settings.max_concurrent_cold_starts
            ));
        }
        if let Some(why) = memory_shortfall(settings, &memory) {
            if !over {
                anyhow::bail!("lane launch refused: {why}");
            }
            overridden.push(format!("memory floor: {why}"));
        }
        return Ok(Admitted { slot, overridden });
    }
}

#[cfg(test)]
fn wait_for_slot(
    settings: &Settings,
    prefix: &str,
    memory: impl Fn() -> Result<u64>,
    on_wait: impl FnMut(),
) -> Result<UnixDatagram> {
    Ok(admit(settings, prefix, memory, false, on_wait)?
        .slot
        .expect("admission without an override holds a slot"))
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
    /// The lane's first cargo build finished: its target is warm, so the
    /// next lane may start.
    pub(crate) fn first_build_finished(&self) {
        if let Some(slot) = self.0.upgrade() {
            slot.socket.lock().unwrap().take();
            slot.released.notify_one();
        }
    }
}

/// A launch-boundary stand-in for the host's settings, slot namespace,
/// memory and waiting, for tests that drive the production launchers.
#[cfg(test)]
pub(crate) struct TestHost {
    pub(crate) max_concurrent_cold_starts: u16,
    pub(crate) min_memory_headroom_gib: u64,
    pub(crate) prefix: String,
    pub(crate) memory_gib: std::rc::Rc<std::cell::Cell<u64>>,
    pub(crate) on_wait: Box<dyn FnMut()>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_HOST: std::cell::RefCell<Option<TestHost>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
impl TestHost {
    /// A free slot namespace for one test.
    pub(crate) fn prefix() -> String {
        format!("aida-test-{}", uuid::Uuid::new_v4())
    }

    /// Hold one of `prefix`'s slots, as an earlier cold lane would.
    pub(crate) fn hold(prefix: &str, index: u16) -> UnixDatagram {
        let address = SocketAddr::from_abstract_name(format!("{prefix}.{index}")).unwrap();
        UnixDatagram::bind_addr(&address).unwrap()
    }
}

impl Startup {
    /// Admit one implementer launch on this host. `root` and `spec` say where
    /// and for what an override is recorded.
    pub(crate) fn acquire(
        root: &Path,
        spec: Option<&str>,
        over: Option<&HostOverride>,
    ) -> Result<Self> {
        #[cfg(test)]
        if let Some(mut host) = TEST_HOST.with(|h| h.borrow_mut().take()) {
            let settings = Settings {
                max_concurrent_cold_starts: host.max_concurrent_cold_starts,
                min_memory_headroom_gib: host.min_memory_headroom_gib,
            };
            let memory = host.memory_gib.clone();
            let admitted = admit(
                &settings,
                &host.prefix,
                || Ok(memory.get() * KIB_PER_GIB),
                over.is_some(),
                &mut host.on_wait,
            );
            TEST_HOST.with(|h| *h.borrow_mut() = Some(host));
            return Self::admitted(root, spec, over, admitted?);
        }
        #[cfg(test)]
        {
            // Unhooked tests never contend with the host's real lanes.
            static PREFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let settings = Settings {
                max_concurrent_cold_starts: 256,
                min_memory_headroom_gib: 1,
            };
            let prefix = PREFIX.get_or_init(TestHost::prefix);
            let admitted = admit(
                &settings,
                prefix,
                || Ok(u64::MAX),
                over.is_some(),
                || thread::sleep(Duration::from_millis(10)),
            )?;
            return Self::admitted(root, spec, over, admitted);
        }
        #[cfg(not(test))]
        {
            let settings = Settings::load()?;
            let mut announced = false;
            let admitted = admit(
                &settings,
                SLOT_PREFIX,
                memory_headroom,
                over.is_some(),
                || {
                    if !announced {
                        eprintln!(
                            "lane launch queued: host cold-start limit reached (lanes.max_concurrent_cold_starts = {})",
                            settings.max_concurrent_cold_starts
                        );
                        announced = true;
                    }
                    thread::sleep(Duration::from_secs(1));
                },
            )?;
            Self::admitted(root, spec, over, admitted)
        }
    }

    fn admitted(
        root: &Path,
        spec: Option<&str>,
        over: Option<&HostOverride>,
        admitted: Admitted,
    ) -> Result<Self> {
        if let Some(over) = over {
            if admitted.overridden.is_empty() {
                eprintln!(
                    "lane launch: {OVERRIDE_ENV} is set but the host limits were not reached; nothing was overridden"
                );
            } else {
                // Who and when are the event's actor and timestamp.
                let line = format!(
                    "host limits overridden ({OVERRIDE_ENV}): {}; reason: {}",
                    admitted.overridden.join("; "),
                    over.reason
                );
                // The record is the override's audit trail, so it must land
                // before the launch proceeds: an unrecorded override refuses
                // (and drops any slot it took). trace:BUG-2682 | ai:claude
                let event = crate::events::gate_held_event(
                    crate::events::GATE_HOST_LIMIT_OVERRIDE,
                    spec.map(str::to_string),
                    None,
                    &line,
                    crate::events::gate_actor(),
                    crate::events::active_seat(),
                );
                if let Err(why) = crate::events::emit_recorded(root, &event) {
                    anyhow::bail!(
                        "lane launch refused: the host-limit override ({}) cannot be recorded, and an unrecorded override is not allowed: {why}",
                        admitted.overridden.join("; ")
                    );
                }
                eprintln!("lane launch: {line}");
            }
        }
        Self::with_timeout(admitted.slot, STARTUP_TIMEOUT)
    }

    fn with_timeout(slot: Option<UnixDatagram>, timeout: Duration) -> Result<Self> {
        let holds = slot.is_some();
        let slot = Arc::new(Slot {
            socket: Mutex::new(slot),
            released: Condvar::new(),
        });
        if holds {
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
        }
        Ok(Self(slot))
    }

    #[cfg(test)]
    pub(crate) fn test_reservation() -> Self {
        Self::with_timeout(Some(UnixDatagram::unbound().unwrap()), STARTUP_TIMEOUT).unwrap()
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
        self.release_handle().first_build_finished();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_2682_queues_until_first_build_releases_slot() {
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = TestHost::prefix();
        let memory = || Ok(8 * KIB_PER_GIB);
        let slot =
            wait_for_slot(&settings, &prefix, memory, || panic!("first launch queued")).unwrap();
        let startup = Startup::with_timeout(Some(slot), STARTUP_TIMEOUT).unwrap();
        let mut queued = 0;
        // A concurrent test's fork can hold the released socket until its
        // exec, so the wait may take more than one turn.
        let _next = wait_for_slot(&settings, &prefix, memory, || {
            queued += 1;
            startup.release_handle().first_build_finished();
            thread::sleep(Duration::from_millis(1));
        })
        .unwrap();
        assert!(queued >= 1, "the second launch queued behind the first");
        assert!(!startup.is_reserved());
    }

    #[test]
    fn bug_2682_timeout_releases_startup_slot() {
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = TestHost::prefix();
        let memory = || Ok(8 * KIB_PER_GIB);
        let first =
            wait_for_slot(&settings, &prefix, memory, || panic!("first launch queued")).unwrap();
        let _startup = Startup::with_timeout(Some(first), Duration::from_millis(10)).unwrap();
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
        let prefix = TestHost::prefix();
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

    /// Without the override a full cap waits and a low floor refuses; with
    /// it both are passed and named, and a free slot is still taken.
    #[test]
    fn bug_2682_host_override_passes_and_names_each_check() {
        let settings = Settings {
            max_concurrent_cold_starts: 1,
            ..Settings::default()
        };
        let prefix = TestHost::prefix();
        let low = || Ok(KIB_PER_GIB);
        assert!(admit(&settings, &prefix, low, false, || panic!("queued"))
            .err()
            .unwrap()
            .to_string()
            .contains("below the 4 GiB floor"));
        let free = admit(&settings, &prefix, low, true, || panic!("queued")).unwrap();
        assert!(free.slot.is_some(), "a free slot is still counted");
        assert_eq!(free.overridden.len(), 1, "{:?}", free.overridden);
        assert!(free.overridden[0].starts_with("memory floor:"));
        let full = admit(
            &settings,
            &prefix,
            || Ok(8 * KIB_PER_GIB),
            true,
            || panic!("an override does not wait"),
        )
        .unwrap();
        assert!(full.slot.is_none());
        assert_eq!(full.overridden.len(), 1, "{:?}", full.overridden);
        assert!(full.overridden[0].starts_with("cold-start cap:"));
    }

    /// An over-limit admission stands only on a written record: an
    /// unwritable `events.jsonl` or disabled events refuse it and return the
    /// slot it took; a writable log admits it and holds the record.
    #[test]
    fn bug_2682_host_override_refuses_when_its_record_cannot_be_written() {
        let prefix = TestHost::prefix();
        let install = || {
            TEST_HOST.with(|h| {
                *h.borrow_mut() = Some(TestHost {
                    max_concurrent_cold_starts: 1,
                    min_memory_headroom_gib: 4,
                    prefix: prefix.clone(),
                    memory_gib: std::rc::Rc::new(std::cell::Cell::new(1)),
                    on_wait: Box::new(|| panic!("an override does not queue")),
                })
            })
        };
        let over = HostOverride {
            reason: "dell1 recovery".into(),
        };
        let free_slot = || {
            let settings = Settings {
                max_concurrent_cold_starts: 1,
                ..Settings::default()
            };
            wait_for_slot(
                &settings,
                &prefix,
                || Ok(8 * KIB_PER_GIB),
                || panic!("a refused override kept its slot"),
            )
            .unwrap()
        };

        let tmp = tempfile::tempdir().unwrap();
        let unwritable = tmp.path().join("unwritable");
        fs::create_dir_all(crate::events::events_path(&unwritable)).unwrap();
        let error = {
            let _env =
                crate::test_env::EnvVarsGuard::apply(&[(crate::events::EVENTS_DISABLE_ENV, None)]);
            install();
            Startup::acquire(&unwritable, Some("STORY-9"), Some(&over))
                .err()
                .expect("an unrecorded override refuses")
        };
        let text = format!("{error:#}");
        assert!(text.contains("cannot be recorded"), "{text}");
        assert!(text.contains("memory floor"), "{text}");
        drop(free_slot());

        let disabled = tmp.path().join("disabled");
        fs::create_dir_all(disabled.join(".aida")).unwrap();
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(
                crate::events::EVENTS_DISABLE_ENV,
                Some("1"),
            )]);
            install();
            let error = Startup::acquire(&disabled, Some("STORY-9"), Some(&over))
                .err()
                .expect("disabled events refuse the override");
            assert!(
                format!("{error:#}").contains(crate::events::EVENTS_DISABLE_ENV),
                "{error:#}"
            );
        }
        assert!(!crate::events::events_path(&disabled).exists());
        drop(free_slot());

        let writable = tmp.path().join("writable");
        fs::create_dir_all(writable.join(".aida")).unwrap();
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[(crate::events::EVENTS_DISABLE_ENV, None)]);
        install();
        let startup = Startup::acquire(&writable, Some("STORY-9"), Some(&over))
            .expect("a recorded override admits");
        let events = fs::read_to_string(crate::events::events_path(&writable)).unwrap();
        let line = events
            .lines()
            .find(|line| line.contains(crate::events::GATE_HOST_LIMIT_OVERRIDE))
            .unwrap_or_else(|| panic!("no override record: {events}"));
        for part in ["memory floor", "dell1 recovery", "STORY-9"] {
            assert!(line.contains(part), "{part} missing: {line}");
        }
        drop(startup);
        TEST_HOST.with(|h| h.borrow_mut().take());
    }

    #[test]
    fn bug_2682_host_override_needs_a_reason() {
        use std::ffi::OsStr;
        assert_eq!(HostOverride::parse(None).unwrap(), None);
        assert!(HostOverride::parse(Some(OsStr::new("  ")))
            .unwrap_err()
            .to_string()
            .contains("needs a reason"));
        assert_eq!(
            HostOverride::parse(Some(OsStr::new(" dell1 recovery \n"))).unwrap(),
            Some(HostOverride {
                reason: "dell1 recovery".into()
            })
        );
    }
}
