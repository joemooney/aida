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

fn live_available() -> bool {
    if !Path::new("/usr/bin/bwrap").exists() {
        eprintln!("SKIP: /usr/bin/bwrap unavailable");
        return false;
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
        eprintln!("SKIP: userns unavailable: {err}");
        return false;
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
try:
    socket.socket(socket.AF_INET,socket.SOCK_DGRAM);raise AssertionError('UDP allowed')
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
    std::fs::write(plan._state.path().join("lease"), "synthetic lease").unwrap();
    std::fs::write(plan._state.path().join("store"), "synthetic store").unwrap();
    let mut out = tempfile::tempfile().unwrap();
    plan.command.stderr(out.try_clone().unwrap());
    let mut run = plan.spawn(out.try_clone().unwrap().into()).unwrap();
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
    assert!(run.child.wait().unwrap().success());
    // Scan every persistent fixture artifact, including synthetic lease/store
    // and log sentinels; API credentials must never be persisted by the helper.
    for entry in std::fs::read_dir(run._state.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let b = std::fs::read(entry.path()).unwrap();
            assert!(!b.windows(23).any(|w| w == b"synthetic-only-key-2707"));
        }
    }
    use std::io::{Seek, SeekFrom};
    out.seek(SeekFrom::Start(0)).unwrap();
    let mut log = String::new();
    out.read_to_string(&mut log).unwrap();
    assert!(log.contains("exact env outer and inner"));
    assert!(!log.contains("synthetic-only-key-2707"));
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
    if !live_available() {
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
    let program = resolve_program(&Command::new("codex"), &env)
        .expect("installed Codex required when userns/bwrap are available");
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
