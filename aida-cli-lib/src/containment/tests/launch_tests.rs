// trace:TASK-2707 | ai:codex
use super::*;
#[test]
fn task_2707_chatgpt_auth_named_refusal() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("auth.json"), "{}").unwrap();
    assert!(refuse_chatgpt(home.path())
        .unwrap_err()
        .to_string()
        .contains("contained ChatGPT auth waits for P3 (PR2)"));
}
#[test]
fn task_2707_codex_applied_requires_receipt_ratchet() {
    use crate::session::{containment_applied, HeadlessVendor};
    for requested in [false, true] {
        assert!(!containment_applied(HeadlessVendor::Codex, requested, None));
    }
    assert!(containment_applied(HeadlessVendor::Claude, true, None));
}
#[test]
fn task_2707_directory_leaf_refuses_session_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let grants = tmp.path().join("session-grants");
    std::fs::create_dir(&grants).unwrap();
    assert!(WriteProfile::new().add(&grants, true).is_err());
    let f = tmp.path().join("file");
    std::fs::write(&f, "").unwrap();
    assert!(WriteProfile::new().add(&f, true).is_err());
    let leaf = tmp
        .path()
        .join("grant-notices/11111111-1111-4111-8111-111111111111");
    std::fs::create_dir_all(&leaf).unwrap();
    let mut p = WriteProfile::new();
    p.add(&leaf, true).unwrap();
    assert!(p.native().contains(&leaf.to_string_lossy().to_string()));
    assert!(!p.native().contains("session-grants"));
}

/// An operator/CI lane that must exercise the live boundary sets this; a
/// skip is then a failure instead of a silent pass.
// trace:TASK-2708 | ai:claude
fn live_required() -> bool {
    std::env::var("AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE").as_deref() == Ok("1")
}
/// The installed-Codex native-composition test has its own opt-in: the
/// required-live CI gate installs bwrap but not Codex, so that gate must
/// neither run it as required nor count it, and a host that merely happens
/// to have Codex installed does not run it either. Only an operator lane that
/// sets this runs it, and then any missing prerequisite is a failure.
// trace:TASK-2708 | ai:claude
fn installed_codex_required() -> bool {
    std::env::var("AIDA_REQUIRE_CODEX_INSTALLED_LIVE").as_deref() == Ok("1")
}
fn skip_under(required: bool, reason: &str) -> bool {
    assert!(!required, "required-live contained launch: {reason}");
    eprintln!("SKIP: {reason}");
    false
}
fn skip(reason: &str) -> bool {
    skip_under(live_required(), reason)
}
fn live_available() -> bool {
    live_available_under(live_required())
}
fn live_available_under(required: bool) -> bool {
    if !Path::new("/usr/bin/bwrap").exists() {
        return skip_under(required, "/usr/bin/bwrap unavailable");
    }
    // trace:TASK-2708 | ai:claude — the same kernel gate the helper applies
    // (x86_64, Landlock ABI 1..=8, seccomp notification): CI enables
    // bwrap+userns in every shard, so a newer kernel must skip, not fail.
    if let Err(error) = host_prerequisites() {
        return skip_under(required, &format!("{error:#}"));
    }
    let result = Command::new("/usr/bin/bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "/bin/true"])
        .output()
        .unwrap();
    if !result.status.success() {
        let err = String::from_utf8_lossy(&result.stderr);
        assert!(
            err.contains("namespace") || err.contains("Operation not permitted"),
            "unexpected bwrap failure: {err}"
        );
        return skip_under(required, &format!("userns unavailable: {err}"));
    }
    true
}
fn fixture(script: &str, preflight: &str) -> PreparedLaunch {
    let state = tempfile::tempdir().unwrap();
    std::fs::set_permissions(state.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut profile = WriteProfile::new();
    profile.add(state.path(), true).unwrap();
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let env = BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), state.path().to_string_lossy().into_owned()),
        ("LANG".into(), "C.UTF-8".into()),
        ("OPENAI_API_KEY".into(), "synthetic-only-key-2707".into()),
        ("AIDA_SESSION_GRANT".into(), "".into()),
        ("AIDA_ROLE_INSTANCE".into(), "companion".into()),
        ("HTTP_PROXY".into(), format!("http://127.0.0.1:{port}")),
    ]);
    let cfg = serde_json::json!({"grant":"","ttl":0,"mode":"normal","port":port,"allowed":[],
        "state":state.path(),"policy_sha":"synthetic-sha","bwrap":profile.args(state.path()),
        "preflight":["/usr/bin/python3","-I","-S","-c",preflight],
        "argv":["/usr/bin/python3","-I","-S","-c",script]});
    let root = state.path().to_path_buf();
    PreparedLaunch::from_config(profile, state, "synthetic-sha".into(), cfg, env, &root).unwrap()
}
fn run_script(script: &str) -> String {
    use std::io::{Seek, SeekFrom};
    let plan = fixture(script, "print('seat_grant_valid=true expected_seat=true')");
    let mut output = tempfile::tempfile().unwrap();
    let mut run = plan.spawn(output.try_clone().unwrap().into()).unwrap();
    assert!(run.receipt().is_accepted());
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    text
}
#[test]
fn task_2707_relay_reachable_and_other_loopback_refused_with_positive_control() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,os,errno
port=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
s=socket.socket();s.connect(('127.0.0.1',port))
s.sendall(b'GET / HTTP/1.1\r\n\r\n')
assert b'405 Method Not Allowed' in s.recv(4096);s.close()
other=socket.socket();other.bind(('127.0.0.1',0));other.listen()
try:
    socket.create_connection(other.getsockname(),timeout=2)
    raise AssertionError('other loopback port reachable')
except OSError as e: assert e.errno==errno.ECONNREFUSED,e
print('relay reached; other loopback refused')
"#,
    );
    // Same listener/connect succeeds in the same netns without the filter.
    let control=Command::new("/usr/bin/bwrap").args(["--unshare-all","--ro-bind","/","/","--","/usr/bin/python3","-I","-S","-c",
        "import socket; s=socket.socket();s.bind(('127.0.0.1',0));s.listen();c=socket.create_connection(s.getsockname());s.accept()[0].close()"])
        .output().unwrap();
    assert!(control.status.success(), "{:?}", control);
    assert!(output.contains("relay reached"));
}
#[test]
fn task_2707_host_dbus_and_codex_daemon_socket_unreachable_with_positive_control() {
    use std::os::unix::net::{UnixListener, UnixStream};
    if !live_available() {
        return;
    }
    let host = tempfile::tempdir().unwrap();
    let sockets = [
        host.path().join("dbus"),
        host.path().join("app-server-control.sock"),
    ];
    let listeners: Vec<_> = sockets
        .iter()
        .map(|p| UnixListener::bind(p).unwrap())
        .collect();
    for (path, listener) in sockets.iter().zip(&listeners) {
        let _ = UnixStream::connect(path).unwrap();
        listener.accept().unwrap();
    }
    let paths = serde_json::to_string(&sockets).unwrap();
    let script = format!(
        r#"
import socket,errno
for path in {paths}:
    s=socket.socket(socket.AF_UNIX)
    try:
        s.connect(path);raise AssertionError('host service reachable')
    except OSError as e: assert e.errno==errno.EACCES,e
# trace:TASK-2708 | ai:claude — inet DGRAM and any unix socketpair are now
# allowed (they reach no host path); RAW inet and unix DGRAM stay denied.
socket.socket(socket.AF_INET,socket.SOCK_DGRAM).close()
for pair in socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM): pair.close()
for args in ((socket.AF_INET,socket.SOCK_RAW,socket.IPPROTO_ICMP),(socket.AF_UNIX,socket.SOCK_DGRAM)):
    try:
        socket.socket(*args);raise AssertionError('%r allowed' % (args,))
    except OSError as e: assert e.errno==errno.EACCES,e
print('host services denied')
"#
    );
    // /tmp is private: expose the fixture at the same path, RO, so denial is
    // demonstrated by the broker, not by masking away the positive control.
    let mut plan = fixture(&script, "print('seat_grant_valid=true expected_seat=true')");
    plan.profile.add(host.path(), false).unwrap();
    // from_config has already encoded mounts; rebuild just this fixture cfg.
    let args: Vec<_> = plan.command.get_args().map(|a| a.to_os_string()).collect();
    let mut cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    cfg["bwrap"] = serde_json::json!(plan.profile.args(plan._state.path()));
    let env = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    let root = plan._state.path().to_path_buf();
    let rebuilt = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut run = rebuilt.spawn(Stdio::null()).unwrap();
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
}
#[test]
fn task_2707_racing_sockaddr_rewrite_still_reaches_only_relay() {
    if !live_available() {
        return;
    }
    run_script(
        r#"
import ctypes as C,socket,threading,os,errno,struct
lib=C.CDLL(None,use_errno=True)
relay=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
other=socket.socket();other.bind(('127.0.0.1',0));other.listen();other.setblocking(False)
buf=C.create_string_buffer(struct.pack('H',socket.AF_INET)+struct.pack('!H',relay)+socket.inet_aton('127.0.0.1')+b'\0'*8)
stop=threading.Event()
def race():
    while not stop.is_set():
        C.memmove(C.addressof(buf)+2,struct.pack('!H',other.getsockname()[1]),2)
        C.memmove(C.addressof(buf)+2,struct.pack('!H',relay),2)
t=threading.Thread(target=race);t.start()
try:
    for i in range(100):
        s=socket.socket();rc=lib.connect(s.fileno(),C.byref(buf),16)
        if rc==0:
            s.settimeout(2);s.sendall(b'GET / HTTP/1.1\r\n\r\n')
            assert b'405' in s.recv(4096)
        else: assert C.get_errno()==errno.ECONNREFUSED
        s.close()
finally: stop.set();t.join()
try:
    other.accept();raise AssertionError('racing sockaddr escaped to other listener')
except BlockingIOError: pass
print('race confined')
"#,
    );
}
// trace:TASK-2708 | ai:claude — /proc and namespaces: Linux-only evidence.
#[cfg(target_os = "linux")]
#[test]
fn task_2707_registry_and_launch_tokens_absent_in_outer_and_inner() {
    if !live_available() {
        return;
    }
    // Set only synthetic canaries on the parent, with a real inherited-env
    // positive control. Production uses the same env_clear helper builder.
    let mut guard = crate::test_env::EnvVarsGuard::snapshot(&[
        "AIDA_HANDLE_LAUNCH_TOKEN",
        "AIDA_AGENT_REGISTRY_TOKEN",
        "TASK2707_AMBIENT",
    ]);
    guard.set_key("AIDA_HANDLE_LAUNCH_TOKEN", "handle-canary-2707");
    guard.set_key("AIDA_AGENT_REGISTRY_TOKEN", "registry-canary-2707");
    guard.set_key("TASK2707_AMBIENT", "ambient-canary-2707");
    let control = Command::new("/usr/bin/python3")
        .args([
            "-I",
            "-S",
            "-c",
            "import os;assert os.environ['TASK2707_AMBIENT']=='ambient-canary-2707'",
        ])
        .status()
        .unwrap();
    assert!(control.success());
    let mut plan = fixture(
        r#"
import os,subprocess,json
expected={'PWD','PATH','HOME','LANG','OPENAI_API_KEY','AIDA_SESSION_GRANT','AIDA_ROLE_INSTANCE','HTTP_PROXY'}
assert set(os.environ)==expected, (set(os.environ),expected)
assert os.environ['AIDA_SESSION_GRANT']=='' and os.environ['AIDA_ROLE_INSTANCE']=='companion'
subprocess.run(['/usr/bin/python3','-I','-S','-c','import os;assert set(os.environ)=='+repr(expected)],check=True)
print('exact env outer and inner')
"#,
        "print('seat_grant_valid=true expected_seat=true')",
    );
    let expected: std::collections::BTreeSet<_> = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| v.map(|_| k.to_str().unwrap().to_owned()))
        .collect();
    assert_eq!(
        expected,
        std::collections::BTreeSet::from(
            [
                "PWD",
                "PATH",
                "HOME",
                "LANG",
                "OPENAI_API_KEY",
                "AIDA_SESSION_GRANT",
                "AIDA_ROLE_INSTANCE",
                "HTTP_PROXY"
            ]
            .map(str::to_owned)
        )
    );
    // trace:TASK-2708 | ai:claude — ruling 3: the PRODUCTION lease, lane
    // registry and headless-log writers run on this contained path (as in
    // `session::run_vendor_headless_child`), against a real lease in the
    // production layout; nothing below is written by the test itself except
    // the pre-existing lease the launcher would have claimed.
    let root = plan._state.path().to_path_buf();
    let lease_id = "task2707gi01";
    std::fs::create_dir_all(crate::leases::leases_dir(&root)).unwrap();
    std::fs::write(
        crate::leases::lease_path(&root, lease_id),
        "id = \"task2707gi01\"\nscope = \"TASK-2707\"\nslug = \"task-2707\"\n\
         owner = \"codex\"\nworktree_path = \"/nonexistent/task2707\"\n\
         branch = \"codex/task-2707\"\nstarted_at = \"2026-11-03T00:00:00Z\"\nhostname = \"h\"\n",
    )
    .unwrap();
    let logs = root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("codex-task2707gi01.log");
    let out = File::create(&log_path).unwrap();
    plan.command.stderr(out.try_clone().unwrap());
    let mut run = plan.spawn(out.try_clone().unwrap().into()).unwrap();
    let worker = run.receipt().worker_pid();
    crate::session::stamp_lease_active_pid_at(&root, lease_id, worker).unwrap();
    crate::lane_liveness::register_headless_lane_for_child(
        &log_path,
        Some(lease_id),
        "codex",
        worker,
    );
    let receipt_bytes = serde_json::to_vec(run.receipt()).unwrap();
    for secret in [
        "synthetic-only-key-2707",
        "handle-canary-2707",
        "registry-canary-2707",
    ] {
        assert!(!receipt_bytes
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
    }
    std::fs::write(run._state.path().join("receipt.json"), receipt_bytes).unwrap();
    for pid in [run.child.id(), run.receipt().wrapper_pid] {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
        for secret in [
            "synthetic-only-key-2707",
            "handle-canary-2707",
            "registry-canary-2707",
        ] {
            assert!(!cmdline
                .windows(secret.len())
                .any(|w| w == secret.as_bytes()));
        }
    }
    run.release().unwrap();
    let status = run.child.wait().unwrap();
    assert!(status.success());
    crate::lane_liveness::finish_headless_lane_for_child(&log_path, Some(lease_id), Some(status));
    // Positive controls: each production writer really wrote its artifact.
    let lease = std::fs::read_to_string(crate::leases::lease_path(&root, lease_id)).unwrap();
    assert!(lease.contains(&format!("active_pid = {worker}")), "{lease}");
    let lanes = root.join(".aida").join("lanes");
    assert!(
        lanes.join(format!("headless-{lease_id}.json")).exists(),
        "lane registry record missing"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains("exact env outer and inner"), "{log}");
    // Scan EVERY persisted file under the lane state (lease, lane registry,
    // headless log, receipt, generated config) for the API key and tokens.
    let mut pending = vec![root.clone()];
    let mut scanned = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                scanned += 1;
                let b = std::fs::read(entry.path()).unwrap();
                for secret in [
                    "synthetic-only-key-2707",
                    "handle-canary-2707",
                    "registry-canary-2707",
                ] {
                    assert!(
                        !b.windows(secret.len()).any(|w| w == secret.as_bytes()),
                        "{secret} persisted in {}",
                        entry.path().display()
                    );
                }
            }
        }
    }
    assert!(scanned >= 4, "lease, lane record, log and receipt scanned");
}
#[test]
fn task_2707_receipt_after_preflight_before_model_execution() {
    if !live_available() {
        return;
    }
    let plan = fixture(
        "raise AssertionError('model must not run')",
        "raise SystemExit(1)",
    );
    assert!(plan.spawn(Stdio::null()).is_err());
    run_script("print('executed after accepted receipt')");
}

#[test]
fn task_2707_exact_environment_selection_excludes_explicit_tokens() {
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_AGENT_REGISTRY_TOKEN", "registry-canary")
        .env("AIDA_HANDLE_LAUNCH_TOKEN", "launch-canary")
        .env("AMBIENT_UNLISTED", "ambient-canary")
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("AIDA_SESSION_GRANT", "");
    let env = selected_environment(&cmd);
    assert_eq!(
        env.keys().cloned().collect::<Vec<_>>(),
        vec!["AIDA_ROLE_INSTANCE", "AIDA_SESSION_GRANT"]
    );
    assert_eq!(env["AIDA_ROLE_INSTANCE"], "companion");
    assert_eq!(env["AIDA_SESSION_GRANT"], "");
    assert!(!TOOL_NET_ENV.contains(&"OPENAI_API_KEY"));
}
#[test]
fn task_2707_native_composition_companion_empty_grant_and_generated_profile() {
    // trace:TASK-2708 | ai:claude — its own opt-in, never the required-live gate.
    if !installed_codex_required() {
        eprintln!(
            "SKIP: installed-Codex native composition is opt-in \
             (AIDA_REQUIRE_CODEX_INSTALLED_LIVE=1)"
        );
        return;
    }
    if !live_available_under(true) {
        return;
    }
    let mut plan = fixture("print('placeholder')", "print('placeholder')");
    let result = crate::test_git::git_command()
        .args(["init", "-q"])
        .arg(plan._state.path())
        .status()
        .unwrap();
    assert!(result.success());
    std::fs::write(
        plan._state.path().join("tracked"),
        "model-free commit control",
    )
    .unwrap();
    let git = plan._state.path().join(".git");
    plan.profile.add(&git, true).unwrap();
    for name in ["config", "hooks", "info"] {
        plan.profile.add(&git.join(name), false).unwrap();
    }
    let home = plan._state.path().join("codex");
    ensure_dir(&home).unwrap();
    let config = home.join("config.toml");
    std::fs::write(&config, plan.profile.native()).unwrap();
    plan.profile.add(&config, false).unwrap();
    let mut env: BTreeMap<String, String> = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    env.insert("PATH".into(), std::env::var("PATH").unwrap());
    env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
    // trace:TASK-2708 | ai:claude — CI installs bwrap but not Codex.
    let Ok(program) = resolve_program(&Command::new("codex"), &env) else {
        skip_under(true, "installed Codex unavailable for native composition");
        return;
    };
    let check = r#"
import os,socket,errno,subprocess
subprocess.run(['/usr/bin/git','add','tracked'],check=True)
subprocess.run(['/usr/bin/git','-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-q','-m','contained native control'],check=True)
try:
    open('.git/config','a').write('unsafe');raise AssertionError('protected git config writable')
except OSError as e: assert e.errno in (errno.EROFS,errno.EACCES,errno.EPERM),e
assert os.environ['AIDA_SESSION_GRANT']==''
assert os.environ['AIDA_ROLE_INSTANCE']=='companion'
assert 'AIDA_HANDLE_LAUNCH_TOKEN' not in os.environ
assert 'AIDA_AGENT_REGISTRY_TOKEN' not in os.environ
port=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
s=socket.socket();s.connect(('127.0.0.1',port));s.sendall(b'GET / HTTP/1.1\r\n\r\n');assert b'405' in s.recv(4096)
print('seat_grant_valid=true expected_seat=true')
"#;
    let mut preflight = vec![
        program.to_string_lossy().into_owned(),
        "-c".into(),
        "shell_environment_policy.set.AIDA_SESSION_GRANT=\"\"".into(),
        "-c".into(),
        "shell_environment_policy.set.AIDA_ROLE_INSTANCE=\"companion\"".into(),
        "sandbox".into(),
        "-P".into(),
        "aida".into(),
        "-C".into(),
        plan._state.path().to_string_lossy().into_owned(),
        "--".into(),
        "/usr/bin/python3".into(),
        "-I".into(),
        "-S".into(),
        "-c".into(),
        check.into(),
    ];
    let cfg = serde_json::json!({"grant":"","ttl":0,"mode":"normal","port":env["HTTP_PROXY"].rsplit(':').next().unwrap().parse::<u16>().unwrap(),
        "allowed":[],"state":plan._state.path(),"policy_sha":"synthetic-sha","bwrap":plan.profile.args(plan._state.path()),
        "preflight":preflight,"argv":["/usr/bin/python3","-I","-S","-c","print('native preflight accepted')"]});
    let root = plan._state.path().to_path_buf();
    let rebuilt = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut run = rebuilt.spawn(Stdio::null()).unwrap();
    assert!(run.receipt().is_accepted());
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
    preflight.clear();
}

#[test]
fn task_2707_posture_without_receipt_refuses_unhandled_launch() {
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_CONTAINMENT_POSTURE", "aida");
    assert!(crate::codex_tool_env::prepare_launch(&mut cmd)
        .unwrap_err()
        .to_string()
        .contains("receipt-owning headless launch boundary"));
}
#[test]
fn task_2707_no_deserialized_receipt_grants_applied_credit() {
    let receipt:ContainmentReceipt=serde_json::from_value(serde_json::json!({"version":1,"posture":"aida","worker_pid":1,
        "worker_start":"fake","wrapper_pid":2,"namespaces":{},"uid_map_identity":true,"seat_preflight":true,
        "broker":"seccomp_notify_addfd","relay_port":3128,"policy_sha":"fake","keyring":"isolated_child_only"})).unwrap();
    assert!(!crate::session::containment_applied(
        crate::session::HeadlessVendor::Codex,
        true,
        Some(&receipt)
    ));
}
/// M5: a relocated CODEX_HOME and CODEX_SQLITE_HOME are masked as well as the
/// default `~/.codex`/`~/.claude`; the default layout masks exactly those two;
/// a symlinked relocation onto the default is masked once; absent paths are
/// skipped (bwrap needs a mount target).
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_relocated_codex_home_and_sqlite_home_are_masked() {
    let tmp = tempfile::tempdir().unwrap();
    let host = tmp.path().join("home");
    for name in [".codex", ".claude"] {
        std::fs::create_dir_all(host.join(name)).unwrap();
    }
    let relocated = tmp.path().join("elsewhere/codex");
    let sqlite = tmp.path().join("elsewhere/sqlite");
    std::fs::create_dir_all(&relocated).unwrap();
    std::fs::create_dir_all(&sqlite).unwrap();
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap();

    let masks = host_vendor_state_masks(&host, &relocated, Some(&sqlite));
    assert_eq!(
        masks,
        vec![
            canon(&host.join(".codex")),
            canon(&host.join(".claude")),
            canon(&relocated),
            canon(&sqlite),
        ]
    );

    let default = host_vendor_state_masks(&host, &host.join(".codex"), None);
    assert_eq!(
        default,
        vec![canon(&host.join(".codex")), canon(&host.join(".claude"))]
    );

    let link = tmp.path().join("link-to-default");
    std::os::unix::fs::symlink(host.join(".codex"), &link).unwrap();
    assert_eq!(host_vendor_state_masks(&host, &link, None), default);

    let missing = tmp.path().join("never-created");
    assert_eq!(
        host_vendor_state_masks(&host, &missing, Some(&missing)),
        default
    );
}
/// M3: grant binding derives the pid from the receipt and refuses one that
/// was never accepted — a deserialized receipt names a pid (1) it never
/// proved, and binding to it would let a caller aim a grant at any process.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_bind_contained_launch_worker_refuses_unaccepted_receipt() {
    let receipt: ContainmentReceipt = serde_json::from_value(serde_json::json!({"version":1,"posture":"aida","worker_pid":1,
        "worker_start":"fake","wrapper_pid":2,"namespaces":{},"uid_map_identity":true,"seat_preflight":true,
        "broker":"seccomp_notify_addfd","relay_port":3128,"policy_sha":"fake","keyring":"isolated_child_only"})).unwrap();
    assert!(!receipt.is_accepted());
    // Refused before the grant id is considered: a grant-less launch holding
    // an unaccepted receipt is still a refused launch, not a silent no-op.
    for grant_id in ["", "00000000-0000-4000-8000-000000000000"] {
        let error = crate::gate::seat_authority::bind_contained_launch_worker(grant_id, &receipt)
            .unwrap_err()
            .to_string();
        assert!(error.contains("receipt was not accepted"), "{error}");
    }
}
#[cfg(target_os = "linux")]
#[test]
fn task_2707_daemon_dies_with_supervisor_and_private_state_not_reused() {
    if !live_available() {
        return;
    }
    let plan = fixture(
        "import os,time; p=os.fork(); time.sleep(300)",
        "print('seat_grant_valid=true expected_seat=true')",
    );
    let first_state = plan._state.path().to_path_buf();
    let mut run = plan.spawn(Stdio::null()).unwrap();
    let worker = run.receipt().worker_pid();
    let expected = run.receipt().namespaces["pid"].clone();
    run.release().unwrap();
    drop(run);
    let deadline = Instant::now() + Duration::from_secs(3);
    while std::fs::read_link(format!("/proc/{worker}/ns/pid"))
        .ok()
        .as_ref()
        == Some(&expected)
    {
        assert!(
            Instant::now() < deadline,
            "namespace survived owner teardown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!first_state.exists());
    let next = fixture("pass", "print('seat_grant_valid=true expected_seat=true')");
    assert_ne!(next._state.path(), first_state);
}

#[test]
fn task_2707_hardlinked_writable_alias_refused() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::hard_link(outside.path(), dir.path().join("alias")).unwrap();
    assert!(WriteProfile::new()
        .add(dir.path(), true)
        .unwrap_err()
        .to_string()
        .contains("hard-linked writable alias"));
}
#[cfg(target_os = "linux")]
#[test]
fn task_2707_parent_leaked_socket_fd_absent_in_child() {
    use std::os::unix::net::UnixStream;
    if !live_available() {
        return;
    }
    let (a, mut b) = UnixStream::pair().unwrap();
    let mut a = a;
    a.write_all(b"positive-control").unwrap();
    let mut buf = [0; 16];
    b.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"positive-control");
    let high = unsafe { libc::fcntl(a.as_raw_fd(), libc::F_DUPFD, 600) };
    assert!(high >= 600);
    let inherited = unsafe { File::from_raw_fd(high) };
    run_script(&format!(
        "import os; assert not os.path.exists('/proc/self/fd/{high}')"
    ));
    drop(inherited);
}

#[test]
fn task_2707_tool_net_environment_is_only_credential_free_loopback() {
    let env = relay_environment(23456);
    assert_eq!(env.len(), TOOL_NET_ENV.len());
    for name in TOOL_NET_ENV {
        if *name == "NO_PROXY" {
            assert_eq!(env[*name], "");
        } else {
            assert_eq!(env[*name], "http://127.0.0.1:23456");
            assert!(!env[*name].contains('@'));
        }
    }
}

#[test]
fn task_2707_notice_leaf_allowed_but_overlapping_canonical_grants_read_only() {
    if !live_available() {
        return;
    }
    let script = r#"
from pathlib import Path
import errno
for path in ['.aida/session-grants/existing.json','.aida/session-grants/new.json','.aida/session-grant-locks/new.lock']:
    try:
        Path(path).write_text('unsafe');raise AssertionError('canonical grant authority writable')
    except OSError as e: assert e.errno in (errno.EROFS,errno.EACCES,errno.EPERM),e
Path('.aida/grant-notices/11111111-1111-4111-8111-111111111111/notice.json').write_text('informational')
"#;
    let mut plan = fixture(script, "print('seat_grant_valid=true expected_seat=true')");
    let authority = plan._state.path().join(".aida");
    let grants = authority.join("session-grants");
    let locks = authority.join("session-grant-locks");
    let notice = authority.join("grant-notices/11111111-1111-4111-8111-111111111111");
    for dir in [&grants, &locks, &notice] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // Host positive control: the same canonical path is writable beforehand.
    std::fs::write(grants.join("existing.json"), "canonical-control").unwrap();
    plan.profile.add(&authority, false).unwrap();
    plan.profile.add(&notice, true).unwrap();
    let args: Vec<_> = plan.command.get_args().map(|a| a.to_os_string()).collect();
    let mut cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    cfg["bwrap"] = serde_json::json!(plan.profile.args(plan._state.path()));
    let env = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    let root = plan._state.path().to_path_buf();
    let rebuilt = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut run = rebuilt.spawn(Stdio::null()).unwrap();
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
    assert_eq!(
        std::fs::read_to_string(grants.join("existing.json")).unwrap(),
        "canonical-control"
    );
    assert!(!grants.join("new.json").exists());
    assert!(!locks.join("new.lock").exists());
    assert_eq!(
        std::fs::read_to_string(notice.join("notice.json")).unwrap(),
        "informational"
    );
}

// ---- TASK-2708 AC1: F-a filter/broker amendments for Claude M1 ------------
// (advisor ruling 2026-11-05 11:25). trace:TASK-2708 | ai:claude

/// The shipped lane filter, evaluated by a classic-BPF interpreter over
/// synthetic seccomp_data. Runs everywhere (no namespace needed), so the
/// filter's socket decisions are pinned even where the live tests skip.
#[test]
fn task_2708_lane_filter_socket_decisions() {
    let script = format!(
        "{COMMON}\n{LANE}\n{}",
        r#"
def run(ops, nr, args):
    data = struct.pack('<iIQ6Q', nr, 0xc000003e, 0, *(list(args) + [0] * (6 - len(args))))
    pc, a = 0, 0
    while True:
        code, jt, jf, k = ops[pc]
        if code == 0x20:
            a = struct.unpack_from('<I', data, k)[0]; pc += 1
        elif code == 0x54:
            a &= k; pc += 1
        elif code == 0x15:
            pc += 1 + (jt if a == k else jf)
        elif code == 0x45:
            pc += 1 + (jt if a & k else jf)
        elif code == 0x06:
            return k
        else:
            raise AssertionError('unknown op %#x' % code)
ops = lane_filter()
S, P = 41, 53
CLOEXEC, NONBLOCK = 0o2000000, 0o4000
cases = [
    ('socketpair_unix_dgram', P, (socket.AF_UNIX, socket.SOCK_DGRAM), ALLOW),
    ('socketpair_unix_dgram_cloexec', P, (socket.AF_UNIX, socket.SOCK_DGRAM | CLOEXEC), ALLOW),
    ('socketpair_unix_stream', P, (socket.AF_UNIX, socket.SOCK_STREAM), ALLOW),
    ('socketpair_unix_seqpacket', P, (socket.AF_UNIX, socket.SOCK_SEQPACKET), ALLOW),
    ('socketpair_inet_denied', P, (socket.AF_INET, socket.SOCK_STREAM), DENY),
    ('unix_stream', S, (socket.AF_UNIX, socket.SOCK_STREAM), ALLOW),
    ('unix_seqpacket', S, (socket.AF_UNIX, socket.SOCK_SEQPACKET), ALLOW),
    ('unix_dgram_denied', S, (socket.AF_UNIX, socket.SOCK_DGRAM), DENY),
    ('unix_dgram_cloexec_denied', S, (socket.AF_UNIX, socket.SOCK_DGRAM | CLOEXEC), DENY),
    ('inet_stream', S, (socket.AF_INET, socket.SOCK_STREAM), ALLOW),
    ('inet_dgram', S, (socket.AF_INET, socket.SOCK_DGRAM | NONBLOCK), ALLOW),
    ('inet6_dgram', S, (socket.AF_INET6, socket.SOCK_DGRAM), ALLOW),
    ('inet_raw_denied', S, (socket.AF_INET, socket.SOCK_RAW, 1), DENY),
    ('inet6_raw_denied', S, (socket.AF_INET6, socket.SOCK_RAW, 58), DENY),
    ('packet_denied', S, (17, socket.SOCK_RAW), DENY),
    ('netlink_route_raw', S, (socket.AF_NETLINK, socket.SOCK_RAW, 0), ALLOW),
    ('netlink_route_dgram', S, (socket.AF_NETLINK, socket.SOCK_DGRAM | CLOEXEC, 0), ALLOW),
    ('connect_notifies', 42, (3, 0, 16), NOTIFY),
]
cases += [('netlink_%d_denied' % proto, S, (socket.AF_NETLINK, socket.SOCK_RAW, proto), DENY)
          for proto in range(1, 32)]
for name, nr, args, want in cases:
    got = run(ops, nr, args)
    assert got == want, '%s: %#x != %#x' % (name, got, want)
print('filter decisions ok: %d' % len(cases))
"#
    );
    let out = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &script])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("filter decisions ok: 49"),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Ruling item 2: only NETLINK_ROUTE; every other netlink protocol refuses.
#[test]
fn task_2708_live_netlink_non_route_protocols_denied() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,errno
socket.socket(socket.AF_NETLINK,socket.SOCK_RAW,0).close()
for proto in range(1,32):
    try:
        socket.socket(socket.AF_NETLINK,socket.SOCK_RAW,proto).close()
        raise AssertionError('netlink protocol %d allowed' % proto)
    except OSError as e: assert e.errno==errno.EACCES,(proto,e)
print('netlink route only')
"#,
    );
    assert!(output.contains("netlink route only"), "{output}");
}

/// Ruling item 3: direct v4/v6 TCP is refused by the broker, and the newly
/// allowed UDP reaches nothing: sendto has no route (ENETUNREACH), a UDP
/// connect is refused, and name resolution fails.
#[test]
fn task_2708_live_direct_v4_v6_dns_unreachable() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,errno
for fam,addr in ((socket.AF_INET,('8.8.8.8',53)),(socket.AF_INET6,('2001:4860:4860::8888',53))):
    s=socket.socket(fam,socket.SOCK_STREAM)
    try:
        s.connect(addr);raise AssertionError('direct tcp %r' % (addr,))
    except OSError as e: assert e.errno==errno.ECONNREFUSED,e
    u=socket.socket(fam,socket.SOCK_DGRAM)
    try:
        u.sendto(b'\0'*12,addr);raise AssertionError('direct udp %r' % (addr,))
    except OSError as e: assert e.errno==errno.ENETUNREACH,e
    try:
        u.connect(addr);raise AssertionError('udp connect %r' % (addr,))
    except OSError as e: assert e.errno==errno.ECONNREFUSED,e
try:
    socket.getaddrinfo('example.com',443);raise AssertionError('dns resolved')
except socket.gaierror: pass
print('direct v4 v6 dns unreachable')
"#,
    );
    assert!(output.contains("direct v4 v6 dns unreachable"), "{output}");
}

/// Ruling item 3: a lane UDP datagram to the relay port gets nothing, a UDP
/// connect to it is refused (never swapped for a relay stream), and the
/// relay still serves TCP afterwards (positive control).
#[test]
fn task_2708_live_lane_udp_to_relay_port_gets_nothing() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,errno,os
port=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
u=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);u.settimeout(1)
u.sendto(b'CONNECT github.com:443 HTTP/1.1\r\n\r\n',('127.0.0.1',port))
try:
    data=u.recv(4096);raise AssertionError('udp reply %r' % data)
except socket.timeout: pass
c=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
try:
    c.connect(('127.0.0.1',port));raise AssertionError('udp connect to relay')
except OSError as e: assert e.errno==errno.ECONNREFUSED,e
s=socket.socket();s.connect(('127.0.0.1',port))
s.sendall(b'GET / HTTP/1.1\r\n\r\n');assert b'405' in s.recv(4096)
print('udp to relay gets nothing')
"#,
    );
    assert!(output.contains("udp to relay gets nothing"), "{output}");
}

/// Ruling item 4: the broker judges the SOCKET's netns, not the caller's. A
/// socket made in the lane netns and connected after its process unshared
/// into a nested user+net namespace is still brokered (refused for a
/// non-relay loopback port). Positive control: a socket made in the nested
/// netns reaches a listener there, so the pass-through is real.
#[test]
fn task_2708_live_lane_socket_connected_from_nested_netns_is_still_brokered() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import ctypes as C,socket,errno,struct,fcntl
lib=C.CDLL(None,use_errno=True)
lane=socket.socket()
other=socket.socket();other.bind(('127.0.0.1',0));other.listen();other.setblocking(False)
target=other.getsockname()
assert lib.unshare(0x10000000|0x40000000)==0,C.get_errno()
try:
    lane.connect(target);raise AssertionError('lane socket connected unbrokered')
except OSError as e: assert e.errno==errno.ECONNREFUSED,e
try:
    other.accept();raise AssertionError('lane listener reached')
except BlockingIOError: pass
d=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
flags=struct.unpack_from('H',fcntl.ioctl(d.fileno(),0x8913,struct.pack('16sH22x',b'lo',0)),16)[0]
fcntl.ioctl(d.fileno(),0x8914,struct.pack('16sH22x',b'lo',flags|1))
n=socket.socket();n.bind(('127.0.0.1',0));n.listen()
c=socket.socket();c.connect(n.getsockname());n.accept()[0].close()
print('lane socket brokered; nested socket passed through')
"#,
    );
    assert!(
        output.contains("lane socket brokered; nested socket passed through"),
        "{output}"
    );
}

// ---- TASK-2708 AC1: Claude on the shared contained wrapper ----------------
// trace:TASK-2708 | ai:claude

/// Vendor differences are rows; an unknown program has no profile.
#[test]
fn task_2708_vendor_rows_are_data() {
    let claude = ContainedVendor::for_program(std::ffi::OsStr::new("/opt/bin/claude")).unwrap();
    assert_eq!(claude.key, "claude");
    assert_eq!(claude.auth, VendorAuth::ClaudeCredentialsFile);
    assert_eq!(claude.private_home_env, ["CLAUDE_CONFIG_DIR"]);
    assert_eq!(claude.private_tmp_env, ["TMPDIR", "CLAUDE_CODE_TMPDIR"]);
    let codex = ContainedVendor::for_program(std::ffi::OsStr::new("codex")).unwrap();
    assert_eq!(codex.auth, VendorAuth::OpenAiApiKey);
    assert_eq!(codex.private_home_env, ["CODEX_HOME", "CODEX_SQLITE_HOME"]);
    for other in ["agy", "opencode", "claude-wrapper", ""] {
        assert!(
            ContainedVendor::for_program(std::ffi::OsStr::new(other)).is_err(),
            "{other}"
        );
    }
}

/// Ruling item 5: Claude's own sandbox on, required, no unsandboxed escape,
/// chained to the relay by httpProxyPort with no SOCKS port; the rest of the
/// generated settings (permissions, env) is preserved.
#[test]
fn task_2708_claude_lane_settings_chain_the_relay() {
    let generated = serde_json::json!({
        "permissions": {"allow": ["Bash(aida *)"], "deny": ["Monitor"]},
        "env": {"CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1"},
        "sandbox": {"enabled": false, "allowUnsandboxedCommands": true,
                    "network": {"socksProxyPort": 1080, "allowedDomains": ["evil.example"]}}
    })
    .to_string();
    let allowed = vec!["api.anthropic.com".to_string(), "github.com".to_string()];
    let out: serde_json::Value =
        serde_json::from_str(&claude_lane_settings(&generated, 41234, &allowed).unwrap()).unwrap();
    let sandbox = &out["sandbox"];
    assert_eq!(sandbox["enabled"], true);
    assert_eq!(sandbox["failIfUnavailable"], true);
    assert_eq!(sandbox["allowUnsandboxedCommands"], false);
    assert_eq!(sandbox["network"]["httpProxyPort"], 41234);
    assert!(sandbox["network"].get("socksProxyPort").is_none(), "{out}");
    assert_eq!(
        sandbox["network"]["allowedDomains"],
        serde_json::json!(allowed)
    );
    assert_eq!(out["permissions"]["deny"], serde_json::json!(["Monitor"]));
    assert_eq!(out["env"]["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"], "1");
    assert!(claude_lane_settings("not json", 1, &[]).is_err());
    assert!(claude_lane_settings("[]", 1, &[]).is_err());
}

/// The Claude argv keeps the site's flags, finishes its one --settings, and
/// refuses permission skips, resume, and a missing or duplicate --settings.
#[test]
fn task_2708_claude_argv_finishes_settings_and_refuses_escapes() {
    let settings = r#"{"sandbox":{"enabled":true}}"#.to_string();
    let base = |extra: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--setting-sources", "project", "--settings"]
            .map(String::from)
            .into();
        args.push(settings.clone());
        args.extend(extra.iter().map(|s| s.to_string()));
        args.push("prompt".into());
        args
    };
    let out = claude_argv(&base(&["--session-id", "s1"]), 5555, &[]).unwrap();
    assert_eq!(out.len(), base(&["--session-id", "s1"]).len());
    let at = out.iter().position(|a| a == "--settings").unwrap();
    let finished: serde_json::Value = serde_json::from_str(&out[at + 1]).unwrap();
    assert_eq!(finished["sandbox"]["network"]["httpProxyPort"], 5555);
    assert_eq!(out.last().unwrap(), "prompt");
    for (extra, needle) in [
        (
            &["--dangerously-skip-permissions"][..],
            "skip its permission",
        ),
        (
            &["--allow-dangerously-skip-permissions"][..],
            "skip its permission",
        ),
        (
            &["--permission-mode", "bypassPermissions"][..],
            "skip its permission",
        ),
        (
            &["--permission-mode=bypassPermissions"][..],
            "skip its permission",
        ),
        (&["--resume", "abc"][..], "resume"),
        (&["--resume=abc"][..], "resume"),
        (&["-c"][..], "resume"),
        (&["--settings", "{}"][..], "exactly one"),
        (&["--settings={}"][..], "one generated --settings"),
    ] {
        let error = claude_argv(&base(extra), 1, &[]).unwrap_err().to_string();
        assert!(error.contains(needle), "{extra:?}: {error}");
    }
    let error = claude_argv(&["-p".into(), "prompt".into()], 1, &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("exactly one"), "{error}");
}

fn credentials_expiring_in(dir: &Path, secs: i64) -> PathBuf {
    let path = dir.join(CLAUDE_CREDENTIALS);
    let at = chrono::Utc::now().timestamp_millis() + secs * 1000;
    std::fs::write(
        &path,
        serde_json::json!({"claudeAiOauth": {"accessToken": "synthetic-2708", "expiresAt": at}})
            .to_string(),
    )
    .unwrap();
    path
}

/// 05:47 ruling: a token that outlives the grant launches without a refresh;
/// one that would not gets ONE host refresh; if that does not help, or the
/// credentials are missing or not OAuth, the launch refuses by name.
#[test]
fn task_2708_claude_credentials_must_outlive_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = credentials_expiring_in(dir.path(), 8 * 3600);
    ensure_claude_credentials_outlive(&fresh, 4 * 3600, || panic!("no refresh needed")).unwrap();

    let stale = credentials_expiring_in(dir.path(), 600);
    let refreshes = std::cell::Cell::new(0);
    let error = ensure_claude_credentials_outlive(&stale, 4 * 3600, || {
        refreshes.set(refreshes.get() + 1);
        Ok(())
    })
    .unwrap_err()
    .to_string();
    assert_eq!(refreshes.get(), 1);
    assert!(error.contains("claude credentials expire in"), "{error}");
    assert!(error.contains("relaunch"), "{error}");

    let renewed = ensure_claude_credentials_outlive(&stale, 4 * 3600, || {
        credentials_expiring_in(dir.path(), 8 * 3600);
        Ok(())
    });
    assert!(renewed.is_ok(), "{renewed:?}");

    // A baseline (grant-less, ttl 0) lane still needs an hour of token.
    credentials_expiring_in(dir.path(), 60);
    let failed = ensure_claude_credentials_outlive(&stale, 0, || anyhow::bail!("offline"));
    assert!(
        format!("{:#}", failed.unwrap_err()).contains("host-side Claude credential refresh failed")
    );

    let missing = dir.path().join("absent").join(CLAUDE_CREDENTIALS);
    let error = ensure_claude_credentials_outlive(&missing, 60, || Ok(())).unwrap_err();
    assert!(
        error.to_string().contains("subscription credentials"),
        "{error}"
    );
    std::fs::write(&stale, r#"{"apiKey":"x"}"#).unwrap();
    let error = ensure_claude_credentials_outlive(&stale, 60, || Ok(())).unwrap_err();
    assert!(error.to_string().contains("claudeAiOauth"), "{error}");
}

/// The credentials leaf is pinned by descriptor: never a symlink or a
/// hard-linked alias.
#[test]
fn task_2708_claude_credentials_leaf_identity() {
    let dir = tempfile::tempdir().unwrap();
    let real = credentials_expiring_in(dir.path(), 3600);
    assert!(open_credentials_leaf(&real).is_ok());
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(open_credentials_leaf(&link).is_err());
    let alias = dir.path().join("alias.json");
    std::fs::hard_link(&real, &alias).unwrap();
    assert!(open_credentials_leaf(&real).is_err());
}

/// A lane whose RO credentials expired exits with the named refusal; any
/// other exit is reported as the vendor's own.
#[test]
fn task_2708_claude_expired_in_lane_is_a_named_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let child = |credentials: Option<PathBuf>| ContainedChild {
        child: Command::new("/bin/true").spawn().unwrap(),
        receipt: None,
        acceptance: None,
        credentials,
        _state: tempfile::tempdir().unwrap(),
    };
    let failed = Command::new("/bin/false").status().unwrap();
    let ok = Command::new("/bin/true").status().unwrap();
    let expired = credentials_expiring_in(dir.path(), -60);
    let run = child(Some(expired));
    assert_eq!(
        run.vendor_exit(failed).unwrap_err().to_string(),
        CLAUDE_EXPIRED_IN_LANE
    );
    assert!(run.vendor_exit(ok).unwrap().success());
    credentials_expiring_in(dir.path(), 3600);
    assert!(!run.vendor_exit(failed).unwrap().success());
    assert!(!child(None).vendor_exit(failed).unwrap().success());
}

fn trusted_policy(root: &Path, vendors: &str) {
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    if !root.join(".git").exists() {
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "test@example.com"]);
    }
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        format!("[contained]\nenable = true\n[contained.vendors]\n{vendors}\n"),
    )
    .unwrap();
    git(&["add", ".aida/config.toml"]);
    git(&["commit", "-q", "--allow-empty", "-m", "trusted policy"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
}

/// Only an explicit trusted `claude = "aida"` selects the wrapper; then
/// every Claude AIDA composition outside a wrapper-owned headless launch
/// (interactive, --bg, coordinator, resume) refuses by name, and inside one
/// the legacy os_wrap is not applied. Applied credit for Claude with a
/// receipt is the receipt's acceptance, never the Claude-native credit.
#[test]
fn task_2708_claude_wrapper_selection_and_unwired_surfaces_refuse() {
    use crate::session::{self, HeadlessVendor};
    let root = tempfile::tempdir().unwrap();
    let _cwd = session::contained_cwd_seam::pin(root.path());
    trusted_policy(root.path(), "");
    assert!(!session::claude_uses_contained_wrapper(root.path()).unwrap());
    assert!(!session::contained_wrapper_launch(HeadlessVendor::Claude, true).unwrap());
    if let Err(error) = session::claude_contained_flags_for(None, root.path()) {
        assert!(
            !error.to_string().contains("shared contained wrapper"),
            "{error}"
        );
    }

    trusted_policy(root.path(), "claude = \"aida\"");
    assert!(session::claude_uses_contained_wrapper(root.path()).unwrap());
    assert!(session::contained_wrapper_launch(HeadlessVendor::Claude, true).unwrap());
    assert!(!session::contained_wrapper_launch(HeadlessVendor::Claude, false).unwrap());
    let error = session::claude_contained_flags_for(None, root.path()).unwrap_err();
    assert_eq!(error.to_string(), session::CLAUDE_WRAPPER_UNWIRED);
    {
        let _scope = session::ClaudeWrapperComposition::enter();
        if let Err(error) = session::claude_contained_flags_for(None, root.path()) {
            assert!(
                !error.to_string().contains("shared contained wrapper"),
                "{error}"
            );
        }
        let args = vec!["-p".to_string(), "x".to_string()];
        assert_eq!(
            session::os_wrapped_program_and_args(None, root.path(), "claude", args.clone())
                .unwrap(),
            ("claude".to_string(), args)
        );
    }
    // The scope ends with its guard.
    assert_eq!(
        session::claude_contained_flags_for(None, root.path())
            .unwrap_err()
            .to_string(),
        session::CLAUDE_WRAPPER_UNWIRED
    );

    trusted_policy(root.path(), "claude = \"bogus\"");
    assert!(session::claude_uses_contained_wrapper(root.path()).is_err());

    let unaccepted: ContainmentReceipt = serde_json::from_value(serde_json::json!({
        "version": 1, "posture": "aida", "worker_pid": 1, "worker_start": "0",
        "wrapper_pid": 1, "namespaces": {}, "uid_map_identity": true,
        "seat_preflight": true, "broker": "seccomp_notify_addfd", "relay_port": 1,
        "policy_sha": "x", "keyring": "isolated_child_only"
    }))
    .unwrap();
    assert!(!session::containment_applied(
        HeadlessVendor::Claude,
        true,
        Some(&unaccepted)
    ));
    assert!(session::containment_applied(
        HeadlessVendor::Claude,
        true,
        None
    ));
}
