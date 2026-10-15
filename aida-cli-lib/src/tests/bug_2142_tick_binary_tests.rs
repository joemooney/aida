//! BUG-2142: a tick driver must run the stable live `aida` binary, and
//! doctor must flag one that does not.
//!
//! No test touches the real crontab, `systemctl` or `~/.config/systemd`:
//! the host is an in-memory fake and the live/link dirs are temp dirs.
//!
//! trace:BUG-2142 | ai:claude

use super::*;
use crate::maintenance_schedule::{render_tick_cron_line, tick_invocation};
use std::time::{Duration as StdDuration, SystemTime};

const REPO: &str = "/home/op/ai/aida";

fn marker() -> String {
    format!("aida-schedule-tick:{REPO}")
}

fn live_paths() -> LiveAidaPaths {
    LiveAidaPaths {
        link: Some(PathBuf::from("/home/op/.local/bin/aida")),
        live: Some(PathBuf::from("/home/op/.local/share/aida/live/aida")),
    }
}

struct FakeHost {
    unit_dir: PathBuf,
    crontab: Option<String>,
}

impl DriverHost for FakeHost {
    fn read_crontab(&mut self) -> Result<Option<String>> {
        Ok(self.crontab.clone())
    }
    fn write_crontab(&mut self, _body: &str) -> Result<()> {
        unreachable!("read-only")
    }
    fn systemctl_user(&mut self, _args: &[&str]) -> Result<CommandOutput> {
        unreachable!("doctor's binary check never calls systemctl")
    }
    fn unit_dir(&self) -> Result<PathBuf> {
        Ok(self.unit_dir.clone())
    }
    fn linger_enabled(&mut self) -> Option<bool> {
        None
    }
    fn systemd_supported(&self) -> bool {
        true
    }
}

#[test]
fn bug_2142_stable_exe_prefers_the_link_then_the_live_copy() {
    let running = Path::new("/home/op/ai/aida/target/release/aida");
    assert_eq!(
        stable_tick_exe(running, &live_paths()),
        PathBuf::from("/home/op/.local/bin/aida")
    );
    let live_only = LiveAidaPaths {
        link: None,
        ..live_paths()
    };
    assert_eq!(
        stable_tick_exe(running, &live_only),
        PathBuf::from("/home/op/.local/share/aida/live/aida")
    );
    assert_eq!(
        stable_tick_exe(running, &LiveAidaPaths::default()),
        running,
        "no live install: fall back to the running binary"
    );
}

#[test]
fn bug_2142_install_writes_the_live_link_and_its_path_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let link_dir = tmp.path().join("bin");
    let live_dir = tmp.path().join("live");
    std::fs::create_dir_all(&link_dir).unwrap();
    std::fs::create_dir_all(&live_dir).unwrap();
    let name = format!("aida{}", std::env::consts::EXE_SUFFIX);
    std::fs::write(live_dir.join(&name), b"live").unwrap();
    std::fs::write(link_dir.join(&name), b"link").unwrap();
    let (link_s, live_s) = (
        link_dir.display().to_string(),
        live_dir.display().to_string(),
    );
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_DEV_LINK_DIR", link_s.as_str()),
        ("AIDA_DEV_LIVE_DIR", live_s.as_str()),
    ]);

    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let inv = real_tick_invocation(&repo).unwrap();
    assert_eq!(inv.exe, link_dir.join(&name).display().to_string());
    assert!(
        inv.path_env
            .starts_with(&format!("{}:", link_dir.display())),
        "{}",
        inv.path_env
    );
    let units = build_systemd_units(&inv);
    assert_eq!(
        exec_start_binary(&units.service),
        Some(link_dir.join(&name))
    );

    // Without the link, the live copy.
    std::fs::remove_file(link_dir.join(&name)).unwrap();
    assert_eq!(real_stable_tick_exe(), live_dir.join(&name));
}

#[test]
fn bug_2142_driver_binaries_round_trip_through_unit_and_crontab() {
    let exe = "/home/op/odd dir/it's $HOME \"q\"/aida";
    let inv = tick_invocation(Path::new(REPO), Path::new(exe)).unwrap();
    let units = build_systemd_units(&inv);
    assert_eq!(exec_start_binary(&units.service), Some(PathBuf::from(exe)));
    let line = render_tick_cron_line(&inv);
    assert_eq!(cron_line_binary(&line), Some(PathBuf::from(exe)));
}

#[test]
fn bug_2142_installed_tick_exes_reads_unit_and_crontab() {
    let tmp = tempfile::tempdir().unwrap();
    let old = "/home/op/ai/aida/target/release/aida";
    let inv = tick_invocation(Path::new(REPO), Path::new(old)).unwrap();
    let units = build_systemd_units(&inv);
    std::fs::write(tmp.path().join(&units.service_name), &units.service).unwrap();
    let mut host = FakeHost {
        unit_dir: tmp.path().to_path_buf(),
        crontab: Some(format!("# mine\n{}\n", render_tick_cron_line(&inv))),
    };
    let found = installed_tick_exes(&mut host, REPO, &marker());
    assert_eq!(
        found,
        vec![
            InstalledTickExe {
                driver: Driver::Systemd,
                exe: PathBuf::from(old),
            },
            InstalledTickExe {
                driver: Driver::Cron,
                exe: PathBuf::from(old),
            },
        ]
    );
    // Another repo's marker finds nothing.
    assert!(installed_tick_exes(&mut host, REPO, "aida-schedule-tick:/elsewhere").is_empty());
}

#[test]
fn bug_2142_doctor_flags_a_driver_not_on_the_live_binary() {
    let now = SystemTime::now();
    let older = now - StdDuration::from_secs(9 * 86_400);
    let evidence = |driver, exe: &str, modified| TickExeEvidence {
        installed: InstalledTickExe {
            driver,
            exe: PathBuf::from(exe),
        },
        modified,
    };
    let stale = [evidence(
        Driver::Systemd,
        "/home/op/ai/aida/target/release/aida",
        Some(older),
    )];
    let f = build_stale_tick_binary_findings(&stale, &live_paths(), Some(now));
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].id, "scheduler-tick-stale-binary:systemd");
    assert!(
        f[0].summary.contains("target/release/aida"),
        "{}",
        f[0].summary
    );
    assert!(
        f[0].summary.contains("older than the live binary"),
        "{}",
        f[0].summary
    );
    assert!(
        f[0].action.starts_with("aida schedule install-systemd"),
        "{}",
        f[0].action
    );
    assert!(
        f[0].action.contains("/home/op/.local/bin/aida"),
        "{}",
        f[0].action
    );

    let gone = [evidence(Driver::Cron, "/home/op/old/aida", None)];
    let f = build_stale_tick_binary_findings(&gone, &live_paths(), Some(now));
    assert_eq!(f[0].id, "scheduler-tick-stale-binary:cron");
    assert!(f[0].summary.contains("no longer exists"));
    assert!(f[0].action.starts_with("aida schedule install-cron"));

    // Either stable path is fine; no live install means nothing to compare.
    let ok = [
        evidence(Driver::Systemd, "/home/op/.local/bin/aida", Some(now)),
        evidence(
            Driver::Cron,
            "/home/op/.local/share/aida/live/aida",
            Some(now),
        ),
    ];
    assert!(build_stale_tick_binary_findings(&ok, &live_paths(), Some(now)).is_empty());
    assert!(build_stale_tick_binary_findings(&stale, &LiveAidaPaths::default(), None).is_empty());
}
