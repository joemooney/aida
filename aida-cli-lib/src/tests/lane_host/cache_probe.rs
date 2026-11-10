//! Synthetic transport controls exercise the real doctor probe and lifecycle.
//! Synthetic bwrap checks the launch contract; a separate live test exercises the sandbox.
// trace:STORY-2642 | ai:codex
use super::*;

#[test]
// trace:STORY-b6b1278890774ef993d2ebc327862470-565.ace1a25e | ai:codex
fn doctor_cache_probe_transport_controls() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    probe_transport_controls(false);
}

#[test]
// trace:STORY-b6b1278890774ef993d2ebc327862470-565.ace1a25e | ai:codex
fn doctor_cache_probe_live_transport_controls() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let available = std::process::Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "/bin/true",
        ])
        .output();
    if !available
        .as_ref()
        .is_ok_and(|output| output.status.success())
    {
        assert!(
            aida_core::exec_context::Environment::capture()
                .get(crate::test_env::REQUIRE_BWRAP_LIVE_ENV)
                .is_none(),
            "live cache probe requires bubblewrap: {available:?}"
        );
        eprintln!("SKIP live cache probe: bubblewrap unavailable: {available:?}");
        return;
    }
    probe_transport_controls(true);
}

#[cfg(target_os = "linux")]
fn probe_transport_controls(live: bool) {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path();
    let bin = home.join("bin");
    // Keep every fixture input under the bound seat: real bwrap hides host /tmp.
    let seat = home.to_path_buf();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(seat.join(".git")).unwrap();
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    let write_executable = |name: &str, body: &str| {
        let path = bin.join(name);
        crate::test_exec::write_executable(&path, body);
    };
    if !live {
        write_executable(
            "bwrap",
            r#"#!/bin/sh
# Preflight uses true; the real probe uses /bin/sh.
case "$*" in *'/bin/sh'*) ;; *) exit 0 ;; esac
printf '%s\n' "$@" >"$HOME/argv"
/usr/bin/env >"$HOME/projected-env"
[ ! -e "$HOME/deny" ] || exit 42
# Model bwrap's private /tmp without touching the host's fixed socket.
export SCCACHE_SERVER_UDS="$HOME/probe.sock"
while [ "$1" != /bin/sh ]; do shift; done
exec "$@"
"#,
        );
    }
    write_executable(
        "sccache",
        r#"#!/usr/bin/python3
import os, socket, sys
path = os.environ['SCCACHE_SERVER_UDS']
if len(sys.argv) == 1:
    if os.path.exists(os.environ['HOME'] + '/startup-denied'):
        sys.exit(45)
    server = socket.socket(socket.AF_UNIX)
    server.bind(path)
    server.listen()
    try:
        while True:
            client, _ = server.accept()
            with client:
                if client.recv(32) == b'stop':
                    break
    finally:
        server.close()
        os.unlink(path)
elif sys.argv[1] in ('--show-stats', '--stop-server'):
    if sys.argv[1] == '--show-stats' and os.path.exists(os.environ['HOME'] + '/transport-denied'):
        sys.exit(43)
    with socket.socket(socket.AF_UNIX) as client:
        client.connect(path)
        client.sendall(b'stop' if sys.argv[1] == '--stop-server' else b'stats')
else:
    sys.exit(44)
"#,
    );
    std::fs::write(
        home.join(".cargo/config.toml"),
        format!(
            "[build]\nrustc-wrapper = {:?}\n",
            bin.join("sccache").to_str().unwrap()
        ),
    )
    .unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.to_str().unwrap()),
        ("CARGO_HOME", home.join(".cargo").to_str().unwrap()),
        ("PATH", &path),
        ("AIDA_OS_WRAP", "1"),
        ("AIDA_HANDLE", "must-not-reach-probe"),
    ]);
    probe_cache(&seat).unwrap();
    let cache = home.join(".cache/aida/lane-sccache");
    if !live {
        let argv = std::fs::read_to_string(home.join("argv")).unwrap();
        assert!(
            argv.contains(&format!("--bind-try\n{0}\n{0}\n", cache.display())),
            "{argv}"
        );
        assert!(argv.contains("--tmpfs\n/tmp\n"));
        let env = std::fs::read_to_string(home.join("projected-env")).unwrap();
        assert!(!env.contains("AIDA_HANDLE="));
        assert!(!env.contains("must-not-reach-probe"));
        assert!(env.contains("CARGO_NET_OFFLINE=true"));
        assert!(env.contains("SCCACHE_SERVER_UDS=/tmp/aida-lane-sccache.sock"));
    }
    assert!(!home.join("probe.sock").exists(), "server did not clean up");
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    let denials: &[&str] = if live {
        &["transport-denied"]
    } else {
        &["startup-denied", "transport-denied", "deny"]
    };
    for denial in denials {
        std::fs::write(home.join(denial), "").unwrap();
        let error = probe_cache(&seat).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("sandbox cache transport/write probe exited"),
            "{error:#}"
        );
        assert!(!home.join("probe.sock").exists());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        std::fs::remove_file(home.join(denial)).unwrap();
    }
}
