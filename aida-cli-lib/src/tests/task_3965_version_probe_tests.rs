//! TASK-3965: every agent-CLI `--version` probe is bounded. A CLI that never
//! answers must cost the probe's bound, not the CI job's 45-minute limit.
// trace:TASK-3965 | ai:claude

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Generous slack over the 5s bound for a loaded CI runner; a hang is 45 min.
const CEILING: Duration = Duration::from_secs(20);

/// A fake agent CLI that never answers `--version` (it also never reads
/// stdin, so a probe that inherited a TTY would block the same way).
fn sleeping_cli(dir: &Path) -> PathBuf {
    let path = dir.join("sleepy-agent");
    crate::test_exec::write_executable(&path, "#!/bin/sh\nexec sleep 600\n");
    path
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

#[cfg(unix)]
#[test]
fn probe_kills_a_cli_that_never_answers_and_reports_a_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let cli = sleeping_cli(dir.path());
    let (probe, took) =
        timed(|| crate::bounded_process::probe_version_within(&cli, Duration::from_secs(1)));
    assert!(matches!(
        probe,
        crate::bounded_process::VersionProbe::TimedOut
    ));
    assert!(took < CEILING, "probe took {took:?}");
    let msg = crate::bounded_process::version_probe_timeout_message("x", Duration::from_secs(5));
    assert!(msg.contains("did not answer --version within 5s"), "{msg}");
}

#[test]
fn probe_reports_a_missing_cli_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let probe = crate::probe_version(dir.path().join("no-such-agent-cli"));
    assert!(matches!(
        probe,
        crate::bounded_process::VersionProbe::Missing
    ));
}

#[cfg(unix)]
#[test]
fn probe_reads_the_banner_with_stdin_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answers");
    // Exits non-zero if stdin is anything but empty/closed.
    crate::test_exec::write_executable(
        &path,
        "#!/bin/sh\nif read -r _; then exit 3; fi\necho 'codex-cli 0.150.2'\n",
    );
    assert_eq!(
        crate::scaffold_cmd::installed_codex_version_of(&path.to_string_lossy()),
        Some((0, 150, 2))
    );
}

#[cfg(unix)]
#[test]
fn init_agent_detection_returns_within_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let cli = sleeping_cli(dir.path());
    let (found, took) = timed(|| crate::init_cmd::agent_cli_detected(&cli.to_string_lossy()));
    assert!(!found);
    assert!(took < CEILING, "init detection took {took:?}");
}

#[cfg(unix)]
#[test]
fn codex_version_probe_returns_within_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let cli = sleeping_cli(dir.path());
    let (version, took) =
        timed(|| crate::scaffold_cmd::installed_codex_version_of(&cli.to_string_lossy()));
    assert_eq!(version, None);
    assert!(took < CEILING, "codex version probe took {took:?}");
    // init's Codex MCP auto-configure gate uses the same availability probe.
    let (found, took) = timed(|| crate::version_probe_available(&cli));
    assert!(!found);
    assert!(took < CEILING, "codex availability probe took {took:?}");
}

/// Reviewed `--version` spawns that do not go through `crate::probe_version`:
/// the helper itself, probes of AIDA's own binaries, git/jq/asciinema, and
/// probes already bounded in place. A new spawn, or a raised count, fails.
const REVIEWED_VERSION_SPAWNS: &[(&str, usize)] = &[
    ("aida-cli-lib/src/bounded_process.rs", 1),
    ("aida-cli-lib/src/cast_recording.rs", 1),
    ("aida-cli-lib/src/dev_build.rs", 4),
    ("aida-cli-lib/src/dev_cmd.rs", 1),
    ("aida-cli-lib/src/implementer_preflight.rs", 1),
    ("aida-cli-lib/src/kernel.rs", 1),
    ("aida-cli-lib/src/mail_wake.rs", 1),
    ("aida-cli-lib/src/show_negative.rs", 1),
    ("aida-cli-lib/src/upgrade_cmd.rs", 1),
    ("aida-cli-lib/src/upgrade_release.rs", 1),
    ("aida-core/src/store_hooks.rs", 1),
    ("aida-core/src/scaffolding/codex_hooks.rs", 1),
    ("aida-mcp/src/mcp.rs", 1),
];

fn version_spawns(src: &str) -> usize {
    let compact: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    compact.matches(".arg(\"--version\")").count() + compact.matches(".args([\"--version\"").count()
}

#[test]
fn task_3965_no_unreviewed_version_spawn_bypasses_the_bounded_probe() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut errors = Vec::new();
    let mut seen = std::collections::BTreeMap::new();
    for (path, src) in crate::seat_clone::clone_git_ratchet_tests::workspace_sources(workspace) {
        let n = version_spawns(&src);
        if n > 0 {
            seen.insert(path, n);
        }
    }
    for (path, n) in &seen {
        let allowed = REVIEWED_VERSION_SPAWNS
            .iter()
            .find(|(p, _)| p == path)
            .map_or(0, |(_, k)| *k);
        if *n > allowed {
            errors.push(format!(
                "{path}: {n} `--version` spawn(s), {allowed} reviewed. Probe an agent or \
                 forge CLI with crate::probe_version / crate::version_probe_available \
                 (bounded, stdin closed); add a non-agent probe here only once it is bounded."
            ));
        }
    }
    for (path, k) in REVIEWED_VERSION_SPAWNS {
        if seen.get(*path).copied().unwrap_or(0) < *k {
            errors.push(format!(
                "{path}: fewer spawns than reviewed ({k}); lower the ledger"
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
