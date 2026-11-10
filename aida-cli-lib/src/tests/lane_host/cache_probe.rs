//! Synthetic transport controls exercise the real doctor probe and lifecycle.
//! The bwrap fixture checks the launch contract; it is not live confinement evidence.
// trace:STORY-2642 | ai:codex
use super::*;

#[test]
// trace:STORY-b6b1278890774ef993d2ebc327862470-565.ace1a25e | ai:codex
fn doctor_cache_probe_transport_controls() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path();
    let bin = home.join("bin");
    let seat = home.join("seat");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(seat.join(".git")).unwrap();
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    let write_executable = |name: &str, body: &str| {
        let path = bin.join(name);
        crate::test_exec::write_executable(&path, body);
    };
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
    let argv = std::fs::read_to_string(home.join("argv")).unwrap();
    let cache = home.join(".cache/aida/lane-sccache");
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
    assert!(!home.join("probe.sock").exists(), "server did not clean up");
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    for denial in ["startup-denied", "transport-denied", "deny"] {
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
