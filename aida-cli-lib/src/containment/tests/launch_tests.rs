// trace:TASK-2707 | ai:codex
use super::*;

// trace:TASK-3225 | ai:codex — broker fixtures load the same fixed service definitions.
const MUTATION: &str = store_mutation::SOURCE;
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
        || std::env::var("AIDA_REQUIRE_BWRAP_LIVE").as_deref() == Ok("1")
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
pub(super) fn skip(reason: &str) -> bool {
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
    let mut plan = fixture(script, "print('seat_grant_valid=true expected_seat=true')");
    let mut output = tempfile::tempfile().unwrap();
    plan.command.stderr(output.try_clone().unwrap());
    let mut run = match plan.spawn(output.try_clone().unwrap().into()) {
        Ok(run) => run,
        Err(error) => {
            output.seek(SeekFrom::Start(0)).unwrap();
            let mut text = String::new();
            output.read_to_string(&mut text).unwrap();
            panic!("lane preparation failed: {error:#}\n{text}");
        }
    };
    assert!(run.receipt().is_accepted());
    run.release().unwrap();
    let status = run.child.wait().unwrap();
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    assert!(status.success(), "lane exited {status}:\n{text}");
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
    assert!(crate::launch::profile::finalize_codex(&mut cmd)
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
    // A structurally valid receipt from disk cannot authorize outer-only argv.
    // trace:TASK-3048 | ai:codex
    let error = crate::launch::agy::AGY
        .candidate_argv(Path::new("/private/agy-runtime"), "probe", Some(&receipt))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("applied AIDA ContainmentReceipt"));
    for vendor in ["claude", "codex", "opencode"] {
        assert!(require_outer_only_receipt(vendor, Some(&receipt))
            .unwrap_err()
            .to_string()
            .contains("not an approved outer-only vendor"));
    }
}

// The wrapper must reject site-owned options before adding receipt-gated argv.
// trace:TASK-3048 | ai:codex
#[test]
fn task_3048_runtime_argv_accepts_only_the_site_prompt() {
    let policy = crate::containment::runtime_policy("agy").unwrap();
    let runtime = Path::new("/private/agy-runtime");
    let prompt = "--settings /host/settings.json --dangerously-skip-permissions";
    let args = runtime_argv(&policy, runtime, &["-p".into(), prompt.into()]).unwrap();
    assert_eq!(&args[args.len() - 2..], &["-p", prompt]);
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "--settings")
            .count(),
        1
    );
    for original in [
        vec![],
        vec!["-p"],
        vec!["--prompt", "text"],
        vec!["--settings", "/host/settings.json", "-p", "text"],
        vec!["-p", "text", "-dangerously-skip-permissions"],
    ] {
        let original = original.into_iter().map(String::from).collect::<Vec<_>>();
        assert!(runtime_argv(&policy, runtime, &original)
            .unwrap_err()
            .to_string()
            .contains("requires exactly -p <prompt>"));
    }
}

// This tests argv construction using the common wrapper's real accepted receipt,
// not AGY execution or all-tool conformance. trace:TASK-3048 | ai:codex
#[test]
fn task_3048_outer_only_candidate_requires_live_accepted_receipt() {
    if !live_available() {
        return;
    }
    let plan = fixture(
        "print('done')",
        "print('seat_grant_valid=true expected_seat=true')",
    );
    let mut run = plan.spawn(Stdio::null()).unwrap();
    let runtime = run._state.path().join("agy-runtime");
    let prompt = "--mode auto is prompt text";
    let args = crate::launch::agy::AGY
        .candidate_argv(&runtime, prompt, Some(run.receipt()))
        .unwrap();
    assert_eq!(&args[args.len() - 2..], &["-p", prompt]);
    assert_eq!(
        args.iter()
            .filter(|a| a.as_str() == "--dangerously-skip-permissions")
            .count(),
        0
    );
    assert!(args
        .windows(2)
        .any(|a| a[0] == "--settings" && Path::new(&a[1]) == runtime.join("settings.json")));
    assert!(!args.iter().any(|a| a == "--sandbox" || a == "--mode"));
    assert!(crate::launch::agy::AGY
        .candidate_argv(Path::new("/"), prompt, Some(run.receipt()))
        .is_err());
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
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

    let masks = host_vendor_state_masks(&host, &relocated, Some(&sqlite)).unwrap();
    assert_eq!(
        masks,
        vec![
            canon(&host.join(".codex")),
            canon(&host.join(".claude")),
            canon(&relocated),
            canon(&sqlite),
        ]
    );

    let default = host_vendor_state_masks(&host, &host.join(".codex"), None).unwrap();
    assert_eq!(
        default,
        vec![canon(&host.join(".codex")), canon(&host.join(".claude"))]
    );

    let link = tmp.path().join("link-to-default");
    std::os::unix::fs::symlink(host.join(".codex"), &link).unwrap();
    assert_eq!(
        host_vendor_state_masks(&host, &link, None).unwrap(),
        default
    );

    let missing = tmp.path().join("never-created");
    assert_eq!(
        host_vendor_state_masks(&host, &missing, Some(&missing)).unwrap(),
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
    ('sendto_notifies', 44, (3, 0, 1, 0, 0, 0), NOTIFY),
    ('sendmsg_notifies', 46, (3, 0, 0), NOTIFY),
    ('sendmmsg_denied', 307, (3, 0, 0), DENY),
]
cases += [('netlink_%d_denied' % proto, S, (socket.AF_NETLINK, socket.SOCK_RAW, proto), DENY)
          for proto in range(1, 32)]
for name, nr, args, want in cases:
    got = run(ops, nr, args)
    assert got == want, '%s: %#x != %#x' % (name, got, want)
assert run(ops,307,(3,0,1,0)) == (0x50000 | errno.EACCES)
print('filter decisions ok: %d' % len(cases))
"#
    );
    let out = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &script])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("filter decisions ok: 52"),
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
/// Round 8 F2: reachable UDP still refuses sendmmsg and named sendmsg with
/// EACCES; these compatibility deltas require advisor acceptance.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_lane_socket_connected_from_nested_netns_is_still_brokered() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import ctypes as C,socket,errno,struct,fcntl,os
probe=os.fork()
if probe==0:
    lib=C.CDLL(None,use_errno=True)
    rc=lib.unshare(0x10000000|0x40000000)
    os._exit(C.get_errno() if rc<0 else 0)
_,status=os.waitpid(probe,0)
assert os.WIFEXITED(status) and os.WEXITSTATUS(status)==0,'NESTED_USERNS_UNAVAILABLE: under-filter probe errno/status=%d' % status
lib=C.CDLL(None,use_errno=True)
lane=socket.socket()
other=socket.socket();other.bind(('127.0.0.1',0));other.listen();other.setblocking(False)
target=other.getsockname()
rc=lib.unshare(0x10000000|0x40000000)
assert rc==0,'NESTED_USERNS_UNAVAILABLE: lane unshare(CLONE_NEWUSER|CLONE_NEWNET) errno=%d; host must enable nested user namespaces' % C.get_errno()
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
# Reachability controls distinguish deliberate API refusals from no route.
receiver=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
receiver.bind(('127.0.0.1',0));receiver.settimeout(0.2)
udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
udp.connect(receiver.getsockname())
assert udp.send(b'connected-control')==17
assert receiver.recv(64)==b'connected-control'
assert udp.sendmsg([b'unnamed-control'])==15
assert receiver.recv(64)==b'unnamed-control'
class Iov(C.Structure):
    _fields_=[('base',C.c_void_p),('size',C.c_size_t)]
class Header(C.Structure):
    _fields_=[('name',C.c_void_p),('namelen',C.c_uint),('iov',C.c_void_p),
              ('iovlen',C.c_size_t),('control',C.c_void_p),
              ('controllen',C.c_size_t),('flags',C.c_int)]
class Mmsg(C.Structure):
    _fields_=[('header',Header),('length',C.c_uint)]
payload=C.create_string_buffer(b'blocked-query')
iov=Iov(C.addressof(payload),len(payload.raw)-1)
batch=(Mmsg*2)(*[Mmsg(Header(None,0,C.addressof(iov),1,None,0,0),0) for _ in range(2)])
C.set_errno(0)
rc=lib.sendmmsg(udp.fileno(),C.byref(batch),2,0)
assert rc==-1 and C.get_errno()==errno.EACCES,(rc,C.get_errno())
assert [msg.length for msg in batch]==[0,0]
unconnected=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
for sender in (udp,unconnected):
    assert sender.sendto(b'named-control',receiver.getsockname())==13
    assert receiver.recv(64)==b'named-control'
    try:
        sender.sendmsg([b'blocked-query'],[],0,receiver.getsockname())
        raise AssertionError('named UDP sendmsg allowed')
    except OSError as e: assert e.errno==errno.EACCES,e
try:
    receiver.recv(64);raise AssertionError('refused UDP query delivered')
except socket.timeout: pass
print('UDP sendmmsg and named sendmsg refused with EACCES')
print('lane socket brokered; nested socket passed through')
"#,
    );
    assert!(
        output.contains("lane socket brokered; nested socket passed through"),
        "{output}"
    );
    assert!(
        output.contains("UDP sendmmsg and named sendmsg refused with EACCES"),
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
    assert!(claude.nested_userns);
    assert_eq!(claude.auth, VendorAuth::ClaudeCredentialsFile);
    assert_eq!(claude.private_home_env, ["CLAUDE_CONFIG_DIR"]);
    assert_eq!(claude.private_tmp_env, ["TMPDIR", "CLAUDE_CODE_TMPDIR"]);
    let codex = ContainedVendor::for_program(std::ffi::OsStr::new("codex")).unwrap();
    assert!(!codex.nested_userns);
    assert_eq!(codex.auth, VendorAuth::OpenAiApiKey);
    assert_eq!(codex.private_home_env, ["CODEX_HOME", "CODEX_SQLITE_HOME"]);
    let runtime = ContainedVendor::for_program(std::ffi::OsStr::new("agy")).unwrap();
    assert_eq!(runtime.auth, VendorAuth::RuntimeCredentialsFile);
    assert!(!runtime.nested_userns);
    for other in ["opencode", "claude-wrapper", ""] {
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

/// Insufficient token lifetime fails fast with an operator remedy, never an
/// unproven `auth status` refresh. Unreadable/invalid leaves have distinct errors.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_claude_credentials_must_outlive_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let path = credentials_expiring_in(dir.path(), 8 * 3600);
    let expiry = claude_credentials_expiry_ms(&open_credentials_leaf(&path).unwrap()).unwrap();
    ensure_claude_credentials_outlive(expiry, 4 * 3600).unwrap();
    for ttl in [0, 4 * 3600] {
        let error =
            ensure_claude_credentials_outlive(chrono::Utc::now().timestamp_millis() + 60000, ttl)
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("CLAUDE_EXPIRED") && error.contains("claude /login"),
            "{error}"
        );
    }
    std::fs::write(&path, "invalid").unwrap();
    let error = claude_credentials_expiry_ms(&open_credentials_leaf(&path).unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("CLAUDE_CREDENTIALS_INVALID"),
        "{error}"
    );
    let error = claude_credentials_expiry_ms(&File::open(dir.path()).unwrap()).unwrap_err();
    assert!(
        error.to_string().contains("CLAUDE_CREDENTIALS_UNREADABLE"),
        "{error}"
    );
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
    let child = |credentials: Option<i64>| ContainedChild {
        child: Command::new("/bin/true").spawn().unwrap(),
        receipt: None,
        acceptance: None,
        credentials,
        chatgpt_deadline: None,
        oauth_denial: tempfile::tempfile().unwrap(),
        mutation: None,
        _state: tempfile::tempdir().unwrap(),
    };
    let failed = Command::new("/bin/false").status().unwrap();
    let ok = Command::new("/bin/true").status().unwrap();
    let expired = credentials_expiring_in(dir.path(), -60);
    let captured = claude_credentials_expiry_ms(&open_credentials_leaf(&expired).unwrap()).unwrap();
    let run = child(Some(captured));
    assert_eq!(
        run.vendor_exit(failed).unwrap_err().to_string(),
        CLAUDE_EXPIRED_IN_LANE
    );
    assert!(run.vendor_exit(ok).unwrap().success());
    let replacement = tempfile::tempdir().unwrap();
    let renewed = credentials_expiring_in(replacement.path(), 3600);
    std::fs::rename(renewed, &expired).unwrap();
    assert_eq!(
        run.vendor_exit(failed).unwrap_err().to_string(),
        CLAUDE_EXPIRED_IN_LANE
    );
    let fresh = claude_credentials_expiry_ms(&open_credentials_leaf(&expired).unwrap()).unwrap();
    assert!(!child(Some(fresh)).vendor_exit(failed).unwrap().success());
    std::fs::remove_file(&expired).unwrap();
    assert_eq!(
        run.vendor_exit(failed).unwrap_err().to_string(),
        CLAUDE_EXPIRED_IN_LANE
    );
    assert!(!child(Some(fresh)).vendor_exit(failed).unwrap().success());
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
/// outside owned composition scopes refuses by name, and inside one
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

/// Anonymous datagram IPC remains usable; naming any host Unix path via
/// sendto/sendmsg fails before delivery, including a non-existent name.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_unix_datagram_named_sends_refused() {
    if !live_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("host-dgram");
    let listener = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let output = run_script(&format!(
        r#"
import socket,errno
x,y=socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM)
x.send(b'pair');assert y.recv(4)==b'pair'
x.sendmsg([b'ipc']);assert y.recv(3)==b'ipc'
for path in [{path:?},'/no-such-host-socket']:
    for send in [lambda: x.sendto(b'bypass',path),lambda: x.sendmsg([b'bypass'],[],0,path)]:
        try:
            send();raise AssertionError('named Unix send allowed')
        except OSError as e: assert e.errno==errno.EACCES,e
print('named Unix sends refused')
"#,
        path = path.to_str().unwrap()
    ));
    assert!(output.contains("named Unix sends refused"), "{output}");
    let mut buf = [0; 16];
    assert_eq!(
        listener.recv(&mut buf).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

/// A socket installed by the relay is lane-owned; re-connect to any other
/// endpoint remains refused. Host-owned sockets fail ancestry proof as well.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_relay_reconnect_refused() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,os,errno
port=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
s=socket.socket();s.connect(('127.0.0.1',port))
try:
    s.connect(('127.0.0.1',1));raise AssertionError('relay reconnect allowed')
except OSError as e: assert e.errno==errno.ECONNREFUSED,e
s.sendall(b'GET / HTTP/1.1\r\n\r\n');assert b'405' in s.recv(4096)
print('relay reconnect refused')
"#,
    );
    assert!(output.contains("relay reconnect refused"), "{output}");
}

/// Exercise the real prepare path with only synthetic host state. Live QA
/// runs the prepared mount vector, with no real vendor or credentials.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_claude_prepare_fake_home() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !Path::new("/usr/bin/bwrap").exists() {
        skip_under(live_required(), "Claude prepare requires bwrap");
        return;
    }
    if let Err(error) = host_prerequisites() {
        skip_under(live_required(), &format!("{error:#}"));
        return;
    }
    let home = crate::home_dir().unwrap();
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "claude = \"aida\"");
    for p in crate::contained_boundary::contained_store_write_paths(root.path()) {
        std::fs::create_dir_all(p).unwrap();
    }
    let config = home.join("relocated-claude");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/host-secret"), "secret").unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("host-secret"), "secret").unwrap();
    for name in [".claude.json", ".claude.json.backup"] {
        std::fs::write(home.join(name), "host-secret").unwrap();
    }
    let path = credentials_expiring_in(&config, 8 * 3600);
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("CLAUDE_CONFIG_DIR", config.to_str().unwrap()),
        ("CODEX_HOME", home.join(".codex").to_str().unwrap()),
    ]);
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("claude");
    std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(&program);
    child
        .args(["-p", "--settings", "{}", "synthetic"])
        .env("AIDA_HEADLESS", "1")
        .env("AIDA_SESSION_GRANT", "")
        .env("AIDA_SESSION_ROLE", "companion");
    // Synthetic foreign-owner seam: exercise the same prepare path with a
    // mismatching issuer uid, without requiring host chown authority.
    assert!(PreparedLaunch::prepare_for_owner(
        &mut child,
        root.path(),
        None,
        unsafe { libc::getuid() } + 1
    )
    .err()
    .unwrap()
    .to_string()
    .contains("credentials identity refused"));
    let plan = PreparedLaunch::prepare(&mut child, root.path(), None)
        .unwrap_or_else(|e| panic!("prepare: {e:#}"));
    let cfg: serde_json::Value =
        serde_json::from_str(&plan.command.get_args().nth(5).unwrap().to_string_lossy()).unwrap();
    let vendor = ContainedVendor::for_program(std::ffi::OsStr::new("claude")).unwrap();
    assert!(vendor.nested_userns);
    assert_eq!(cfg["nested_userns"], vendor.nested_userns);
    let env: BTreeMap<_, _> = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let private = PathBuf::from(&env["CLAUDE_CONFIG_DIR"]);
    let bound = plan
        .profile
        .mounts
        .iter()
        .find(|m| m.path == private.join(CLAUDE_CREDENTIALS))
        .unwrap();
    assert!(!bound.writable);
    assert_eq!(
        claude_credentials_expiry_ms(&bound.file).unwrap(),
        plan.credentials.unwrap()
    );
    for name in ["TMPDIR", "CLAUDE_CODE_TMPDIR"] {
        assert!(Path::new(&env[name]).starts_with(plan._state.path()));
    }
    assert_eq!(env["DISABLE_AUTOUPDATER"], "1");
    for name in [
        ".claude",
        "relocated-claude",
        ".claude.json",
        ".claude.json.backup",
    ] {
        let m = plan
            .profile
            .mounts
            .iter()
            .find(|m| m.path == home.join(name))
            .unwrap();
        assert!(!m.writable);
        if m.file.metadata().unwrap().is_file() {
            assert_eq!(m.file.metadata().unwrap().len(), 0);
        }
    }
    // Use the prepared bwrap vector directly: fake vendor/seat authority
    // cannot manufacture an accepted production receipt.
    if live_available() {
        let script = format!(
            r#"
import os,json,errno
from pathlib import Path
home=Path({home:?})
assert not (home/'.claude/host-secret').exists()
assert not (home/'relocated-claude/host-secret').exists()
for name in ['.claude.json','.claude.json.backup']: assert (home/name).read_text()==''
p=Path(os.environ['CLAUDE_CONFIG_DIR'])/'.credentials.json'
assert json.loads(p.read_text())['claudeAiOauth']['accessToken']=='synthetic-2708'
try:
    p.write_text('changed');raise AssertionError('credential writable')
except OSError as e: assert e.errno in [errno.EROFS,errno.EACCES],e
print('Claude prepare live mounts ok')
"#,
            home = home.to_str().unwrap()
        );
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .args(
                cfg["bwrap"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_str().unwrap()),
            )
            .args(["--", "/usr/bin/python3", "-I", "-S", "-c", &script])
            .env_clear()
            .envs(&env);
        let fds: Vec<_> = plan
            .profile
            .mounts
            .iter()
            .map(|m| m.file.as_raw_fd())
            .collect();
        unsafe {
            command.pre_exec(move || {
                for fd in &fds {
                    libc::fcntl(*fd, libc::F_SETFD, 0);
                }
                Ok(())
            });
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    drop(plan);
    let saved = config.join("saved");
    std::fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &path).unwrap();
    assert!(PreparedLaunch::prepare(&mut child, root.path(), None)
        .err()
        .unwrap()
        .to_string()
        .contains("credentials identity refused"));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    std::fs::hard_link(&path, &saved).unwrap();
    assert!(PreparedLaunch::prepare(&mut child, root.path(), None)
        .err()
        .unwrap()
        .to_string()
        .contains("credentials identity refused"));
    std::fs::remove_file(&saved).unwrap();
    std::fs::write(&path, "not json").unwrap();
    assert!(PreparedLaunch::prepare(&mut child, root.path(), None)
        .err()
        .unwrap()
        .to_string()
        .contains("CLAUDE_CREDENTIALS_INVALID"));
    credentials_expiring_in(&config, -60);
    assert!(PreparedLaunch::prepare(&mut child, root.path(), None)
        .err()
        .unwrap()
        .to_string()
        .contains("CLAUDE_EXPIRED"));
}

/// Kernel-evidence branches, host/foreign socket rejection, bounded admission,
/// and a connect that never returns. No bubblewrap or external network needed.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_socket_ancestry_and_connect_deadline() {
    let code = format!(
        "{COMMON}\n{LANE}\n{MUTATION}\n{HOST}\n{}",
        r#"
import tempfile
with tempfile.TemporaryDirectory() as root:
    handles=[]
    for name in ['lane-net','child-net','host-net','foreign-net','lane-user','child-user','host-user','foreign-user']:
        f=open(root+'/'+name,'w+');handles.append(f)
    ids=[ns_identity(f.fileno()) for f in handles]
    owner={ids[1]:5,ids[2]:6,ids[3]:7}
    parents={ids[5]:4}
    real=LIBC
    class Kernel:
        def ioctl(self,fd,op,*args):
            op=getattr(op,'value',op)
            if op==SIOCGSKNS: return os.dup(fd)
            mapping=owner if op==NS_GET_USERNS else parents
            key=ns_identity(fd)
            if key not in mapping:
                C.set_errno(errno.EPERM);return -1
            return os.dup(handles[mapping[key]].fileno())
    LIBC=Kernel()
    assert socket_scope(handles[0].fileno(),ids[0],ids[4])=='lane'
    assert socket_scope(handles[1].fileno(),ids[0],ids[4])=='nested'
    for index in [2,3]:
        try:
            scope=socket_scope(handles[index].fileno(),ids[0],ids[4])
            assert scope=='foreign',scope
        except RuntimeError as error:
            assert 'NOT_DESCENDANT' in str(error),error
    LIBC=real
    # An actual host-netns socket must never receive nested authority.
    with socket.socket() as host_socket:
        try:
            assert socket_scope(host_socket.fileno(),ids[0],ids[4])=='foreign'
        except RuntimeError as error:
            assert any(word in str(error) for word in ['UNREADABLE','NOT_DESCENDANT']),error
    for _ in range(16): assert CONNECT_SLOTS.acquire(blocking=False)
    assert not CONNECT_SLOTS.acquire(blocking=False)
    for _ in range(16): CONNECT_SLOTS.release()
    NESTED_CONNECT_PROGRAM='import time; time.sleep(30)'
    responses=[]
    def ioctl(fd,op,response): responses.append(response.error)
    CONNECT_TIMEOUT=0.1
    assert CONNECT_SLOTS.acquire(blocking=False)
    started=time.monotonic()
    connect_nested(-1,1,os.dup(handles[0].fileno()),b'xx')
    assert responses==[-errno.ETIMEDOUT],responses
    assert time.monotonic()-started<2
    for _ in range(16): assert CONNECT_SLOTS.acquire(blocking=False)
    assert not CONNECT_SLOTS.acquire(blocking=False)
    for _ in range(16): CONNECT_SLOTS.release()
class Denied:
    def unshare(self,*args): C.set_errno(errno.EPERM);return -1
LIBC=Denied()
try:
    probe_nested_userns();raise AssertionError('probe accepted denied userns')
except RuntimeError as error:
    assert 'NESTED_USERNS_UNAVAILABLE' in str(error) and '1' in str(error),error
print('ancestry and connect deadline ok')
"#
    );
    let out = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &code])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A full blocking socket waits for its reader; multi-megabyte sendall and
/// sendmsg with more than 64 iovecs make progress through bounded short writes.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_blocking_stream_sends_and_large_payloads() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,threading,time,errno,signal
signal.alarm(20)
x,y=socket.socketpair()
x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,4096)
filled=0
while True:
    try: filled+=x.send(b'f'*4096,socket.MSG_DONTWAIT)
    except BlockingIOError: break
received=bytearray()
def drain():
    time.sleep(3.5)
    while True:
        b=y.recv(65536)
        if not b: break
        received.extend(b)
t=threading.Thread(target=drain);t.start()
started=time.monotonic()
assert x.send(b'marker')==6
assert time.monotonic()-started>=3.3
payload=b'abcd'*(1024*1024)
x.sendall(payload)
iovs=[b'v'*16384]*100
while iovs:
    n=x.sendmsg(iovs)
    assert n>0
    while iovs and n>=len(iovs[0]): n-=len(iovs.pop(0))
    if iovs: iovs[0]=iovs[0][n:]
x.shutdown(socket.SHUT_WR);t.join(5);assert not t.is_alive()
assert received==b'f'*filled+b'marker'+payload+b'v'*(16384*100)
# SO_SNDTIMEO is the only blocking-send deadline, returning EAGAIN.
a,b=socket.socketpair();a.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,4096)
while True:
    try: a.send(b'f'*4096,socket.MSG_DONTWAIT)
    except BlockingIOError: break
import struct
a.setsockopt(socket.SOL_SOCKET,socket.SO_SNDTIMEO,struct.pack('ll',0,500000))
started=time.monotonic()
try:
    a.send(b'timeout');raise AssertionError('full socket sent')
except BlockingIOError as e: assert e.errno==errno.EAGAIN,e
assert 0.4<=time.monotonic()-started<1.5
a.close();b.close()
# More blocked callers than worker slots queue until readers free capacity.
pairs=[];done=[]
def queued_send(index,sock):
    assert sock.send(b'z')==1
    done.append(index)
for index in range(20):
    a,b=socket.socketpair();a.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,4096)
    filled=0
    while True:
        try: filled+=a.send(b'f'*4096,socket.MSG_DONTWAIT)
        except BlockingIOError: break
    t=threading.Thread(target=queued_send,args=(index,a));t.start()
    pairs.append((a,b,filled,t))
time.sleep(0.2)
assert not done,done
for a,b,filled,t in pairs:
    while filled:
        data=b.recv(min(filled,65536));assert data;filled-=len(data)
for a,b,filled,t in pairs:
    t.join(3);assert not t.is_alive()
    assert b.recv(1)==b'z'
    a.close();b.close()
assert sorted(done)==list(range(20)),done
print('blocking and large stream sends ok')
"#,
    );
    assert!(
        output.contains("blocking and large stream sends ok"),
        "{output}"
    );
}

/// Invoke the shipped argument-snapshot path on real sockets and process
/// memory, with only seccomp notification/pidfd transport supplied by the test.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_send_bounds_namespace_timeout_queue_and_cancellation() {
    let code = format!(
        "{COMMON}\n{LANE}\n{MUTATION}\n{HOST}\n{}",
        r#"
import tempfile
responses=[]
def ioctl(fd,op,response):
    if op==SEND: responses.append((response.val,response.error))
def target_socket(pid,fd): return os.dup(fd)
def notify(sock,buf,name=None,flags=socket.MSG_DONTWAIT):
    n=Notification();n.id=1;n.pid=os.getpid();n.data.nr=44
    n.data.args[:]=(sock.fileno(),C.addressof(buf),len(buf.raw)-1,flags,
                    C.addressof(name) if name else 0,len(name.raw)-1 if name else 0)
    return n
def wait_response():
    deadline=time.monotonic()+2
    while not responses:
        assert time.monotonic()<deadline,'send worker did not answer'
        time.sleep(0.001)
    return responses.pop(0)
# A real host UDP socket and destination must not gain lane authority.
with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as host, socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as receiver, tempfile.TemporaryFile() as lane_net, tempfile.TemporaryFile() as lane_user:
    receiver.bind(('127.0.0.1',0));receiver.setblocking(False)
    address=C.create_string_buffer(struct.pack('H',socket.AF_INET)+struct.pack('!H',receiver.getsockname()[1])+socket.inet_aton('127.0.0.1')+b'\0'*8)
    payload=C.create_string_buffer(b'no-host-egress')
    broker_send(-1,notify(host,payload,address),ns_identity(lane_net.fileno()),ns_identity(lane_user.fileno()))
    assert wait_response()==(0,-errno.EACCES)
    try:
        receiver.recv(1024);raise AssertionError('host named send delivered')
    except BlockingIOError: pass
    real_scope=socket_scope
    for scope in ['foreign','unreadable','lane','nested']:
        def socket_scope(*args):
            if scope=='unreadable': raise RuntimeError('SOCKET_NETNS_UNREADABLE')
            return scope
        broker_send(-1,notify(host,payload,address),None,None)
        if scope in ('lane','nested'):
            assert wait_response()==(len(payload.raw)-1,0)
            assert receiver.recv(1024)==b'no-host-egress'
        else: assert wait_response()==(0,-errno.EACCES)
        # Named sendmsg refuses even with proven lane/descendant ownership.
        # Pin that extra AC1 deviation separately from foreign-socket refusal.
        iov=SendIov(C.addressof(payload),len(payload.raw)-1)
        header=SendHeader(C.addressof(address),len(address.raw)-1,
                          C.addressof(iov),1,None,0,0)
        msg=Notification();msg.id=2;msg.pid=os.getpid();msg.data.nr=46
        msg.data.args[:]=(host.fileno(),C.addressof(header),socket.MSG_DONTWAIT,0,0,0)
        broker_send(-1,msg,None,None)
        assert wait_response()==(0,-errno.EACCES),scope
        try:
            receiver.recv(1024);raise AssertionError('named sendmsg delivered')
        except BlockingIOError: pass
    socket_scope=real_scope
# Oversized datagrams retain message boundaries and are never truncated.
x,y=socket.socketpair(socket.AF_UNIX,socket.SOCK_DGRAM)
big=C.create_string_buffer(b'x'*(SEND_BYTES+1))
broker_send(-1,notify(x,big),None,None)
assert wait_response()==(0,-errno.EMSGSIZE)
for count,size in [(65,1),(1,SEND_BYTES+1)]:
    iovs=(SendIov*count)(*[SendIov(C.addressof(big),size) for _ in range(count)])
    header=SendHeader(None,0,C.addressof(iovs),count,None,0,0)
    n=Notification();n.id=2;n.pid=os.getpid();n.data.nr=46
    n.data.args[:]=(x.fileno(),C.addressof(header),socket.MSG_DONTWAIT,0,0,0)
    broker_send(-1,n,None,None)
    assert wait_response()==(0,-errno.EMSGSIZE)
y.setblocking(False)
try:
    y.recv(16);raise AssertionError('truncated datagram delivered')
except BlockingIOError: pass
x.close();y.close()
# A perpetually full blocking stream times out, restores capacity, and leaves
# O_NONBLOCK unchanged. A nonblocking caller still observes EAGAIN immediately.
x,y=socket.socketpair();x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,4096)
while True:
    try: x.send(b'f'*4096,socket.MSG_DONTWAIT)
    except BlockingIOError: break
payload=C.create_string_buffer(b'z')
broker_send(-1,notify(x,payload),None,None)
assert wait_response()==(0,-errno.EAGAIN)
x.setblocking(False)
broker_send(-1,notify(x,payload,flags=0),None,None)
assert wait_response()==(0,-errno.EAGAIN)
x.setblocking(True)
x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDTIMEO,struct.pack('ll',0,100000))
broker_send(-1,notify(x,payload,flags=0),None,None)
assert wait_response()==(0,-errno.EAGAIN)
assert not fcntl.fcntl(x.fileno(),fcntl.F_GETFL)&os.O_NONBLOCK
# Wait for the worker's final cleanup before asserting capacity restoration.
deadline=time.monotonic()+2
while SEND_SLOTS._value!=16:
    assert time.monotonic()<deadline
    time.sleep(0.001)
for _ in range(16): assert SEND_SLOTS.acquire(blocking=False)
# Saturation queues a notification without copying payloads or returning ENOBUFS.
x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDTIMEO,struct.pack('ll',0,0))
broker_send(-1,notify(x,payload,flags=0),None,None)
assert len(SEND_PENDING)==1 and not responses
for _ in range(16): SEND_SLOTS.release()
y.recv(65536)
drain_pending_sends()
assert wait_response()==(1,0) and not SEND_PENDING
# An invalid/dead notification releases a worker within a short poll interval.
while SEND_SLOTS._value!=16: time.sleep(0.001)
while True:
    try: x.send(b'f'*4096,socket.MSG_DONTWAIT)
    except BlockingIOError: break
real_ioctl=ioctl
valid=True
def ioctl(fd,op,value):
    if op==VALID and not valid: raise RuntimeError('notification cancelled')
    real_ioctl(fd,op,value)
broker_send(-1,notify(x,payload,flags=0),None,None)
valid=False
deadline=time.monotonic()+1
while SEND_SLOTS._value!=16:
    assert time.monotonic()<deadline,'cancelled caller retained send worker'
    time.sleep(0.001)
responses.clear()
# A dead queued caller frees its pinned socket without attempting the send.
valid=True
for _ in range(16): assert SEND_SLOTS.acquire(blocking=False)
# A queued call's SO_SNDTIMEO includes time spent waiting for a worker.
x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDTIMEO,struct.pack('ll',0,100000))
broker_send(-1,notify(x,payload,flags=0),None,None)
time.sleep(0.12);drain_pending_sends()
assert wait_response()==(0,-errno.EAGAIN) and not SEND_PENDING
x.setsockopt(socket.SOL_SOCKET,socket.SO_SNDTIMEO,struct.pack('ll',0,0))
broker_send(-1,notify(x,payload,flags=0),None,None)
queued_fd=SEND_PENDING[0][4]
valid=False
drain_pending_sends()
assert not SEND_PENDING
try: os.fstat(queued_fd);raise AssertionError('cancelled queued fd leaked')
except OSError as e: assert e.errno==errno.EBADF
for _ in range(16): SEND_SLOTS.release()
ioctl=real_ioctl
x.close();y.close()
class OldKernel:
    def prctl(self,*args): return 0
    def syscall(self,nr,op,flags,*args):
        assert (nr,op,flags)==(317,1,8|32),(nr,op,flags)
        C.set_errno(errno.EINVAL);return -1
real_libc=LIBC
LIBC=OldKernel()
try:
    install_lane_filter();raise AssertionError('old kernel accepted filter')
except RuntimeError as e: assert 'SECCOMP_WAIT_KILLABLE_RECV_UNAVAILABLE' in str(e),e
LIBC=real_libc
print('send bounds namespace timeout queue cancellation and kernel gate ok')
"#
    );
    let out = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &code])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Model Linux 5.19-6.8 pidfd errno behavior with an actual nonleader TID.
/// Only a proven shared table permits substituting the leader's pidfd.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_target_socket_old_kernel_thread_file_tables() {
    let code = format!(
        "{COMMON}\n{LANE}\n{MUTATION}\n{HOST}\n{}",
        r#"
import tempfile
leader=os.getpid()
stop=threading.Event()
worker=threading.Thread(target=stop.wait)
worker.start()
tid=worker.native_id
assert tid!=leader
opened=[];calls=[]
def old_pidfd_open(pid,flags=0):
    opened.append((pid,flags))
    if flags: raise OSError(errno.EINVAL,'PIDFD_THREAD unavailable')
    if pid!=leader: raise OSError(errno.EINVAL,'plain pidfd_open requires a group leader')
    return os.open('/dev/null',os.O_RDONLY)
class OldPidfdKernel:
    shared=0
    def syscall(self,nr,*args):
        calls.append((nr,args))
        if nr==312:
            assert args==(tid,leader,2,0,0),args  # KCMP_FILES
            C.set_errno(errno.EPERM)
            return self.shared
        assert nr==438,nr
        return os.dup(args[1])
real_pidfd_open=os.pidfd_open;real_libc=LIBC
old_kernel=OldPidfdKernel()
os.pidfd_open=old_pidfd_open;LIBC=old_kernel
try:
    # The mock models the kernel's EINVAL for a plain nonleader open, even
    # though the fixed path must never attempt that open.
    try:
        old_pidfd_open(tid)
        raise AssertionError('nonleader plain open succeeded')
    except OSError as e: assert e.errno==errno.EINVAL,e
    opened.clear()
    with tempfile.TemporaryFile() as f:
        # Leaders need no kcmp proof. The duplicate remains the requested fd.
        fd=target_socket(leader,f.fileno())
        try:
            assert os.fstat(fd)==os.fstat(f.fileno())
            assert not os.get_inheritable(fd)
        finally: os.close(fd)
        assert opened==[(leader,128),(leader,0)],opened
        assert [nr for nr,args in calls]==[438],calls
        # Nonleaders bypass plain pidfd_open(tid) and prove the table first.
        opened.clear();calls.clear()
        fd=target_socket(tid,f.fileno());os.close(fd)
        assert opened==[(tid,128),(leader,0)],opened
        assert [nr for nr,args in calls]==[312,438],calls
        # A different table or an unreadable kcmp result must fail closed,
        # before opening a leader pidfd or attempting pidfd_getfd.
        for result in (1,-1):
            old_kernel.shared=result;opened.clear();calls.clear()
            try:
                target_socket(tid,f.fileno())
                raise AssertionError('unproven thread file table admitted')
            except RuntimeError as e: assert 'SOCKET_THREAD_FILE_TABLE_UNPROVEN' in str(e),e
            assert opened==[(tid,128)],opened
            assert [nr for nr,args in calls]==[312],calls
        # Permission errors on PIDFD_THREAD are not unsupported-kernel evidence.
        def refused_pidfd_open(pid,flags=0):
            raise OSError(errno.EPERM,'pidfd permission denied')
        os.pidfd_open=refused_pidfd_open;calls.clear()
        try:
            target_socket(tid,f.fileno())
            raise AssertionError('pidfd permission refusal bypassed')
        except OSError as e: assert e.errno==errno.EPERM,e
        assert not calls,calls
finally:
    os.pidfd_open=real_pidfd_open;LIBC=real_libc
    stop.set();worker.join()
print('old kernel leader/nonleader pidfd and shared file table proof ok')
"#
    );
    let out = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &code])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Handled signals cannot restart completed sends; EPIPE preserves per-thread SIGPIPE.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_send_signals_exactly_once_and_sigpipe() {
    if !live_available() {
        return;
    }
    let output = run_script(
        r#"
import socket,threading,time,signal,errno,subprocess,sys
signal.alarm(20)
a,b=socket.socketpair();a.setsockopt(socket.SOL_SOCKET,socket.SO_SNDBUF,4096)
received=bytearray();hits=[]
def read():
    time.sleep(0.15)
    while True:
        chunk=b.recv(1024)
        if not chunk: break
        received.extend(chunk)
        time.sleep(0.0001)
t=threading.Thread(target=read);t.start()
signal.signal(signal.SIGALRM,lambda *_:hits.append(1))
signal.setitimer(signal.ITIMER_REAL,0.001,0.001)
payload=bytes(range(256))*512
for start in range(0,len(payload),256):
    a.sendall(payload[start:start+128])
    view=memoryview(payload)[start+128:start+256]
    while view:
        n=a.sendmsg([view]);assert n>0;view=view[n:]
signal.setitimer(signal.ITIMER_REAL,0)
a.shutdown(socket.SHUT_WR);t.join(5)
assert not t.is_alive() and received==payload,(len(received),len(payload))
assert len(hits)>20,len(hits)
a.close();b.close()
# MSG_NOSIGNAL suppresses delivery; both send syscalls otherwise signal the caller.
pipes=[]
signal.signal(signal.SIGPIPE,lambda *_:pipes.append(1))
a,b=socket.socketpair();b.close()
for send in [lambda flags:a.send(b'x',flags),lambda flags:a.sendmsg([b'x'],[],flags)]:
    before=len(pipes)
    try: send(socket.MSG_NOSIGNAL)
    except BrokenPipeError: pass
    else: raise AssertionError('closed peer accepted send')
    assert len(pipes)==before
    try: send(0)
    except BrokenPipeError: pass
    else: raise AssertionError('closed peer accepted send')
    deadline=time.monotonic()+1
    while len(pipes)==before and time.monotonic()<deadline: time.sleep(0.001)
    assert len(pipes)==before+1,pipes
a.close()
# A nonleader caller receives a thread-directed blocked SIGPIPE, not the leader.
signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGPIPE})
thread_results=[]
def broken_thread():
    x,y=socket.socketpair();y.close()
    try: x.send(b'x')
    except BrokenPipeError: pass
    thread_results.append(signal.sigtimedwait({signal.SIGPIPE},1) is not None)
    x.close()
t=threading.Thread(target=broken_thread);t.start();t.join(3)
assert not t.is_alive() and thread_results==[True],thread_results
signal.pthread_sigmask(signal.SIG_UNBLOCK,{signal.SIGPIPE})
# Default disposition still terminates a lane child.
r=subprocess.run([sys.executable,'-I','-S','-c',
    'import signal,socket;signal.signal(signal.SIGPIPE,signal.SIG_DFL);a,b=socket.socketpair();b.close();a.send(b"x")'])
assert r.returncode==-signal.SIGPIPE,r.returncode
print('signal storm exactly once and SIGPIPE ok')
"#,
    );
    assert!(
        output.contains("signal storm exactly once and SIGPIPE ok"),
        "{output}"
    );
}

/// Even an actual host-network UDP descriptor passed through permitted private
/// IPC cannot make a named send from the lane.
// trace:TASK-2708 | ai:codex
#[test]
fn task_2708_live_host_udp_named_send_refused() {
    if !live_available() {
        return;
    }
    let script = r#"
import socket,os,array,errno,signal
signal.alarm(15)
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as ipc:
    ipc.connect(os.environ['HOME']+'/host-fd.sock')
    port,anc,_,_=ipc.recvmsg(64,socket.CMSG_SPACE(4))
    assert len(anc)==1
    level,kind,data=anc[0]
    assert (level,kind)==(socket.SOL_SOCKET,socket.SCM_RIGHTS)
    fds=array.array('i');fds.frombytes(data)
    with socket.socket(fileno=fds[0]) as host:
        try:
            host.sendto(b'bypass',('127.0.0.1',int(port)))
            raise AssertionError('host netns named send allowed')
        except OSError as e: assert e.errno==errno.EACCES,e
    ipc.sendall(b'done')
print('host UDP named send refused')
"#;
    let mut plan = fixture(script, "print('seat_grant_valid=true expected_seat=true')");
    let host = r#"
import socket,array,sys,signal
signal.alarm(20)
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as ipc, socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as sender, socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as receiver:
    ipc.bind(sys.argv[1]);ipc.listen(1)
    receiver.bind(('127.0.0.1',0));receiver.settimeout(0.3)
    port=receiver.getsockname()[1]
    sender.sendto(b'control',('127.0.0.1',port));assert receiver.recv(64)==b'control'
    print('ready',flush=True)
    conn,_=ipc.accept()
    with conn:
        conn.sendmsg([str(port).encode()],[(socket.SOL_SOCKET,socket.SCM_RIGHTS,array.array('i',[sender.fileno()]))])
        assert conn.recv(64)==b'done'
    try:
        receiver.recv(64);raise AssertionError('host received bypass datagram')
    except socket.timeout: pass
"#;
    let mut host = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", host])
        .arg(plan._state.path().join("host-fd.sock"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = String::new();
    std::io::BufReader::new(host.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    let mut output = tempfile::tempfile().unwrap();
    plan.command.stderr(output.try_clone().unwrap());
    let mut run = plan.spawn(output.try_clone().unwrap().into()).unwrap();
    run.release().unwrap();
    let status = run.child.wait().unwrap();
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    assert!(status.success(), "{status}: {text}");
    assert!(text.contains("host UDP named send refused"), "{text}");
    let result = host.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// trace:TASK-3194 | ai:codex
#[test]
fn task_3194_receipt_diagnostic_pins_harness_line() {
    let policy_sha = "0123456789abcdef0123456789abcdef01234567";
    let receipt = ContainmentReceipt {
        accepted: true,
        version: 1,
        posture: "aida".into(),
        worker_pid: 123,
        worker_start: "456".into(),
        wrapper_pid: 789,
        namespaces: BTreeMap::new(),
        uid_map_identity: true,
        seat_preflight: true,
        broker: "seccomp_notify_addfd".into(),
        relay_port: 8080,
        policy_sha: policy_sha.into(),
        keyring: "isolated_child_only".into(),
        mutation_epoch: None,
        mutation_ready: false,
    };
    assert_eq!(
        receipt_diagnostic(&receipt, policy_sha).unwrap(),
        concat!(
            "aida-contained-receipt sha256=704bc726e2b131fd6bb4583cc081f5d36cbe9300e0b1623862c4b28517473237",
            " policy_sha=0123456789abcdef0123456789abcdef01234567"
        )
    );
}

// No model calls or real credentials: exercise the shared filesystem inputs.
// trace:TASK-3048 | ai:codex
#[test]
fn task_3048_private_runtime_pins_only_the_credential_leaf() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let policy = crate::containment::runtime_policy("agy").unwrap();
    let host = home.path().join(&policy.relative);
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(host.join("settings.json"), "host settings must not inherit").unwrap();
    let token = host.join(&policy.credential);
    std::fs::write(&token, "synthetic-credential").unwrap();
    let mut profile = WriteProfile::new();
    let runtime = install_private_runtime(
        &mut profile,
        state.path(),
        home.path(),
        &policy,
        workspace.path(),
    )
    .unwrap();
    assert_eq!(runtime, host);
    let source = state.path().join("vendor-runtime");
    assert_eq!(std::fs::metadata(&source).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(source.join("settings.json")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"permissions": {"allow": [format!("write_file({}/**)", workspace.path().display())]}})
    );
    assert!(std::fs::read(source.join(&policy.credential))
        .unwrap()
        .is_empty());
    bind_runtime_credential(&mut profile, &runtime, &token, &policy.credential, unsafe {
        libc::getuid()
    })
    .unwrap();
    assert!(profile.mounts[0].writable);
    assert_eq!(profile.mounts[0].path, host);
    assert!(!profile.mounts[1].writable);
    assert_eq!(profile.mounts[1].path, token);
    // Mount pins the original inode, even if the host replaces the pathname.
    std::fs::rename(&token, host.join("old-token")).unwrap();
    std::fs::write(&token, "replacement").unwrap();
    let mut pinned = String::new();
    profile.mounts[1]
        .file
        .try_clone()
        .unwrap()
        .read_to_string(&mut pinned)
        .unwrap();
    assert_eq!(pinned, "synthetic-credential");
    std::fs::remove_file(&token).unwrap();
    symlink(host.join("old-token"), &token).unwrap();
    assert!(
        bind_runtime_credential(&mut profile, &runtime, &token, &policy.credential, unsafe {
            libc::getuid()
        })
        .is_err()
    );
    std::fs::remove_file(&token).unwrap();
    std::fs::hard_link(host.join("old-token"), &token).unwrap();
    assert!(
        bind_runtime_credential(&mut profile, &runtime, &token, &policy.credential, unsafe {
            libc::getuid()
        })
        .is_err()
    );
}

// Inventory entries are requirements on policy, never grants or wildcard rules.
// trace:TASK-3048 | ai:codex
#[test]
fn task_3048_runtime_policy_fails_closed_before_launch() {
    let mut policy = crate::containment::runtime_policy("agy").unwrap();
    assert!(policy
        .require_auth_policy(&policy.auth_hosts)
        .unwrap_err()
        .to_string()
        .contains("inventory is incomplete"));
    policy.auth_inventory_complete = true; // synthetic complete inventory only
    assert!(policy
        .require_auth_policy(&[])
        .unwrap_err()
        .to_string()
        .contains("requires trusted"));
    policy.require_auth_policy(&policy.auth_hosts).unwrap();
    for bad in [
        "*.googleapis.com",
        "127.0.0.1",
        "https://accounts.google.com",
        "UPPER.com",
        "",
    ] {
        policy.auth_hosts = vec![bad.into()];
        assert!(policy.validate().is_err(), "{bad}");
    }
    policy = crate::containment::runtime_policy("agy").unwrap();
    for bad in ["/private/escape", "../escape", "", ".gemini/../.aida"] {
        policy.relative = bad.into();
        assert!(policy.validate().is_err(), "{bad}");
    }
    policy = crate::containment::runtime_policy("agy").unwrap();
    policy.raw_store_writes = true;
    assert!(policy.validate().is_err());
}

// Uses the actual common process boundary with synthetic credentials, not AGY
// execution or vendor conformance. It covers the nested runtime remount after
// masking the host parent, which argv/mount-list assertions cannot validate.
// trace:TASK-3048 | ai:codex
#[test]
fn task_3048_live_private_runtime_preserves_read_only_credential() {
    use std::os::unix::net::{UnixListener, UnixStream};
    if !live_available() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let policy = crate::containment::runtime_policy("agy").unwrap();
    let runtime = home.path().join(&policy.relative);
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("settings.json"), "host-settings").unwrap();
    std::fs::write(runtime.join("host-plugin"), "host-plugin").unwrap();
    let token = runtime.join(&policy.credential);
    std::fs::write(&token, "synthetic-credential").unwrap();
    // Real reachable host endpoints and writable canaries establish that
    // descendant refusals come from the common boundary, not absent targets.
    let ipc = home.path().join("host-tool.sock");
    let listener = UnixListener::bind(&ipc).unwrap();
    let _control = UnixStream::connect(&ipc).unwrap();
    listener.accept().unwrap();
    let tcp = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = tcp.local_addr().unwrap();
    let _control = std::net::TcpStream::connect(address).unwrap();
    tcp.accept().unwrap();
    let protected: Vec<_> = ["outside", "git", "store"]
        .map(|name| {
            let path = home.path().join(name);
            std::fs::write(&path, "host-canary").unwrap();
            path
        })
        .into();
    let workspace = tempfile::tempdir().unwrap();
    let settings_json = policy.settings(workspace.path()).unwrap().to_string();
    let runtime_json = serde_json::to_string(&runtime).unwrap();
    let leaf_json = serde_json::to_string(&policy.credential).unwrap();
    let protected_json = serde_json::to_string(&protected).unwrap();
    let ipc_json = serde_json::to_string(&ipc).unwrap();
    let script = format!(
        r#"
import pathlib,errno,subprocess,sys,json,os
runtime=pathlib.Path({runtime_json})
assert json.loads((runtime/'settings.json').read_text())=={settings_json}
assert not (runtime/'host-plugin').exists()
token=runtime/{leaf_json}
assert token.read_text()=='synthetic-credential'
try:
    token.write_text('replace')
    raise AssertionError('credential writable')
except OSError as e:
    assert e.errno in (errno.EACCES,errno.EPERM,errno.EROFS),e
# An exec descendant uses the same runtime mount, with no native permission layer.
subprocess.run([sys.executable,'-c',"import pathlib,sys; pathlib.Path(sys.argv[1]).write_text('private')",str(runtime/'child-state')],check=True)
assert (runtime/'child-state').read_text()=='private'
# Exercise external tool processes without a native permission monitor. Each
# tool uses the same interpreter but a different invocation/transport: a file
# helper, a URL helper and a long-lived stdio request server. This is boundary
# inheritance evidence, not evidence that AGY invokes or disables these tools.
probe = '''
import pathlib,socket,errno,os
def probe(request):
    runtime=pathlib.Path(request['runtime'])
    (runtime/request['name']).write_text('private-tool-state')
    for path in request['protected']:
        assert pathlib.Path(path).read_text()=='host-canary'
        try:
            pathlib.Path(path).write_text('escape')
            raise AssertionError('tool escaped filesystem boundary')
        except OSError as error:
            assert error.errno in (errno.EACCES,errno.EPERM,errno.EROFS),error
    with socket.socket(socket.AF_UNIX) as client:
        try:
            client.connect(request['ipc'])
            raise AssertionError('tool reached host IPC')
        except OSError as error:
            assert error.errno==errno.EACCES,error
    try:
        socket.create_connection(('127.0.0.1',request['port']),timeout=2)
        raise AssertionError('tool reached host TCP')
    except OSError as error:
        assert error.errno==errno.ECONNREFUSED,error
    # The inherited relay remains usable; denial is not a broken socket API.
    port=int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])
    with socket.create_connection(('127.0.0.1',port),timeout=2) as client:
        client.sendall(b'GET / HTTP/1.1\\r\\n\\r\\n')
        assert b'405 Method Not Allowed' in client.recv(4096)
'''
request=dict(runtime=str(runtime),protected={protected_json},ipc={ipc_json},port={port})
for name in ('file-helper','url-helper','stdio-server'):
    request['name']=name
    if name=='stdio-server':
        program=probe+'\nimport sys,json\nfor line in sys.stdin:\n probe(json.loads(line));print("ok",flush=True)'
        tool=subprocess.Popen([sys.executable,'-I','-S','-c',program],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
        try:
            stdout,_=tool.communicate(json.dumps(request)+'\n',timeout=10)
            assert tool.returncode==0 and stdout.strip()=='ok'
        finally:
            if tool.poll() is None:
                tool.kill();tool.wait(timeout=5)
    else:
        program=probe+'\nimport json,sys\nprobe(json.loads(sys.argv[1]))'
        subprocess.run([sys.executable,'-I','-S','-c',program,json.dumps(request)],check=True,timeout=10)
    assert (runtime/name).read_text()=='private-tool-state'
"#,
        port = address.port()
    );
    let mut plan = fixture(&script, "print('seat_grant_valid=true expected_seat=true')");
    plan.profile.add(home.path(), false).unwrap();
    let empty = plan._state.path().join("empty-runtime-parent");
    std::fs::create_dir_all(empty.join("antigravity-cli")).unwrap();
    plan.profile.add(&empty, false).unwrap();
    plan.profile.mounts.last_mut().unwrap().path = home.path().join(".gemini");
    install_private_runtime(
        &mut plan.profile,
        plan._state.path(),
        home.path(),
        &policy,
        workspace.path(),
    )
    .unwrap();
    bind_runtime_credential(
        &mut plan.profile,
        &runtime,
        &token,
        &policy.credential,
        unsafe { libc::getuid() },
    )
    .unwrap();
    let args: Vec<_> = plan.command.get_args().map(|a| a.to_os_string()).collect();
    let mut cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    cfg["bwrap"] = serde_json::json!(plan.profile.args(plan._state.path()));
    let env = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    let root = plan._state.path().to_path_buf();
    let plan = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut run = plan.spawn(Stdio::inherit()).unwrap();
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
    assert!(!runtime.join("child-state").exists());
    for name in ["file-helper", "url-helper", "stdio-server"] {
        assert!(!runtime.join(name).exists());
    }
    for path in protected {
        assert_eq!(std::fs::read_to_string(path).unwrap(), "host-canary");
    }
    assert_eq!(
        std::fs::read_to_string(&token).unwrap(),
        "synthetic-credential"
    );
}

// Masking is derived from descriptor data even for a different active vendor.
// trace:TASK-3048 | ai:codex
#[test]
fn task_3048_runtime_descriptor_masks_host_state_for_every_vendor() {
    let host = tempfile::tempdir().unwrap();
    let policy = crate::containment::runtime_policy("agy").unwrap();
    std::fs::create_dir_all(host.path().join(policy.relative)).unwrap();
    let masks = host_vendor_state_masks(host.path(), &host.path().join("absent"), None).unwrap();
    assert_eq!(
        masks,
        crate::containment::runtime_host_parents(host.path()).unwrap()
    );
}

// trace:TASK-3225 | ai:codex
#[test]
fn task_3225_service_required_refuses_absent_and_empty_binding_before_setup() {
    let root = tempfile::tempdir().unwrap();
    for grant in [None, Some("")] {
        let mut command = Command::new("codex");
        command.env("AIDA_STORE_MUTATION_REQUIRED", "1");
        if let Some(grant) = grant {
            command.env(crate::gate::seat_authority::GRANT_ENV, grant);
        }
        let error = match PreparedLaunch::prepare(&mut command, root.path(), None) {
            Ok(_) => panic!("service admitted missing binding"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("nonempty explicit grant binding"));
    }
}

// trace:TASK-3225 | ai:codex
#[test]
fn task_3225_service_receipt_and_empty_worker_binding_refuse() {
    let mut receipt: ContainmentReceipt = serde_json::from_value(serde_json::json!({
        "version":1,"posture":"aida","worker_pid":123,"worker_start":"456",
        "wrapper_pid":789,"namespaces":{},"uid_map_identity":true,"seat_preflight":true,
        "broker":"seccomp_notify_addfd","relay_port":8080,"policy_sha":"test",
        "keyring":"isolated_child_only"
    }))
    .unwrap();
    assert!(validate_service_receipt(&receipt, None).is_ok());
    let epoch = "11111111-1111-4111-8111-111111111111";
    assert!(validate_service_receipt(&receipt, Some(epoch)).is_err());
    receipt.mutation_epoch = Some(epoch.into());
    assert!(validate_service_receipt(&receipt, Some(epoch)).is_err());
    receipt.mutation_ready = true;
    assert!(validate_service_receipt(&receipt, Some(epoch)).is_ok());
    assert!(validate_service_receipt(&receipt, None).is_err());
    assert!(validate_service_receipt(&receipt, Some("other")).is_err());
    receipt.accepted = true;
    let error =
        crate::gate::seat_authority::bind_contained_launch_worker("", &receipt).unwrap_err();
    assert!(error.to_string().contains("nonempty grant binding"));
}

// trace:TASK-3225 | ai:codex
#[test]
fn task_3225_required_service_refuses_unwrapped_launch() {
    let mut child = Command::new("codex");
    assert!(require_store_service_surface(&child, false).is_ok());
    child.env("AIDA_STORE_MUTATION_REQUIRED", "1");
    assert!(require_store_service_surface(&child, false).is_err());
    assert!(require_store_service_surface(&child, true).is_ok());
}

/// S1 exercises the coordinator builder and its actual spawn adapter. All
/// authority/credentials are synthetic; the real wrapper still attests and
/// holds the fake vendor behind the receipt/binding barrier.
// trace:TASK-3219 | ai:codex
#[test]
fn task_3219_live_claude_coordinator_wrapper() {
    use std::io::{Seek, SeekFrom};
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !live_available() {
        return;
    }
    use crate::session::{self, HeadlessVendor};
    use aida_core::agents_config::AgentSeat;
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "claude = \"aida\"");
    let _cwd = session::contained_cwd_seam::pin(root.path());
    for p in crate::contained_boundary::contained_store_write_paths(root.path()) {
        std::fs::create_dir_all(p).unwrap();
    }
    let home = crate::home_dir().unwrap();
    // Keep fake executables in the mounted project: the isolated test HOME
    // can be under /dev/shm, which the wrapper deliberately replaces.
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let config = home.join(".claude");
    std::fs::create_dir_all(&config).unwrap();
    credentials_expiring_in(&config, 8 * 3600);
    let program = bin.join("claude");
    std::fs::write(
        &program,
        r#"#!/usr/bin/python3
import os,sys,json,errno,socket
from pathlib import Path
if '--version' in sys.argv:
    print('synthetic-3219');sys.exit(0)
assert os.environ['AIDA_SESSION_ROLE']=='advisor'
assert 'AIDA_CLAUDE_COORDINATOR_WRAPPER' not in os.environ
assert os.environ['AIDA_SESSION_GRANT']==''
assert os.environ['AIDA_CONTAINMENT_POSTURE']=='aida'
assert 'AIDA_HANDLE_LAUNCH_TOKEN' not in os.environ
assert 'AIDA_AGENT_REGISTRY_TOKEN' not in os.environ
assert Path('preflight-ran').read_text()=='yes'
settings=json.loads(sys.argv[sys.argv.index('--settings')+1])
assert settings['sandbox']['enabled']
assert settings['sandbox']['failIfUnavailable']
assert not settings['sandbox']['allowUnsandboxedCommands']
# Retain the shared headless CLI/MCP argv, including its native MCP selection.
assert '--mcp-config' not in sys.argv and '--strict-mcp-config' not in sys.argv
assert sys.argv[sys.argv.index('--setting-sources')+1]=='project'
assert settings['env']['CLAUDE_CODE_DISABLE_BACKGROUND_TASKS']=='1'
assert 'Monitor' in settings['permissions']['deny']
port=settings['sandbox']['network']['httpProxyPort']
s=socket.socket();s.connect(('127.0.0.1',port))
s.sendall(b'GET / HTTP/1.1\r\n\r\n');assert b'405' in s.recv(4096)
try:
    Path('.git/config').write_text('escape');raise AssertionError('git writable')
except OSError as e: assert e.errno in (errno.EROFS,errno.EACCES,errno.EPERM),e
Path('model-ran').write_text('yes')
print('coordinator stream' if '--output-format' in sys.argv else 'coordinator quiet')
sys.exit(7)
"#,
    )
    .unwrap();
    let aida = bin.join("aida");
    std::fs::write(&aida, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'aida synthetic'; exit 0; fi\nprintf yes > preflight-ran\necho 'seat_grant_valid=true expected_seat=true'\n").unwrap();
    for file in [&program, &aida] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _aida = crate::kernel::test_aida_exe::set(aida.clone());
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("PATH", Some(&path)),
            ("AIDA_CONTAINED", None),
            ("AIDA_AGENT_CMD", None),
            ("AIDA_OS_WRAP", Some("1")),
            ("CLAUDE_CONFIG_DIR", Some(config.to_str().unwrap())),
        ],
    );
    for stream in [false, true] {
        let guard = session::child_reservation_guard();
        let mut cmd = session::build_coordinator_headless_command(
            None,
            HeadlessVendor::Claude,
            "synthetic",
            "dontAsk",
            AgentSeat::Advisor,
            "",
            true,
            stream,
        )
        .unwrap();
        assert_eq!(Path::new(cmd.get_program()).file_name().unwrap(), "claude");
        let mut output = tempfile::tempfile().unwrap();
        let status = session::coordinator_status_with_handle_guard(
            HeadlessVendor::Claude,
            &mut cmd,
            root.path(),
            Some(output.try_clone().unwrap()),
            guard,
        )
        .unwrap();
        assert_eq!(status.code(), Some(7));
        output.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        output.read_to_string(&mut text).unwrap();
        assert!(
            text.contains(if stream {
                "coordinator stream"
            } else {
                "coordinator quiet"
            }),
            "{text}"
        );
    }
    std::fs::remove_file(root.path().join("model-ran")).unwrap();
    // A failed seat preflight must not release the vendor, even though its
    // argv and credentials are otherwise valid.
    std::fs::write(&aida, "#!/bin/sh\nexit 1\n").unwrap();
    let guard = session::child_reservation_guard();
    let mut cmd = session::build_coordinator_headless_command(
        None,
        HeadlessVendor::Claude,
        "synthetic",
        "dontAsk",
        AgentSeat::Advisor,
        "",
        true,
        false,
    )
    .unwrap();
    assert!(session::coordinator_status_with_handle_guard(
        HeadlessVendor::Claude,
        &mut cmd,
        root.path(),
        None,
        guard,
    )
    .is_err());
    assert!(!root.path().join("model-ran").exists());
}

/// The opt-in affects only coordinator-owned composition; unwired entry
/// points still refuse, and the unset policy keeps the old command exactly.
// trace:TASK-3219 | ai:codex
#[test]
fn task_3219_coordinator_composition_scope_and_native_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::session::{self, HeadlessVendor};
    use aida_core::agents_config::AgentSeat;
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "");
    let _cwd = session::contained_cwd_seam::pin(root.path());
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let program = bin.join("claude");
    std::fs::write(&program, "#!/bin/sh\necho synthetic-3219\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_AGENT_CMD", None),
            ("PATH", Some(&path)),
        ],
    );
    for stream in [false, true] {
        let preview = session::coordinator_headless_preview(
            None,
            HeadlessVendor::Claude,
            "synthetic",
            "dontAsk",
            AgentSeat::Advisor,
            true,
            stream,
        )
        .unwrap();
        let mut expected = session::compose_headless_command_for(
            None,
            HeadlessVendor::Claude,
            "synthetic",
            "preview",
            true,
            None,
            None,
            false,
        )
        .unwrap();
        // Quiet coordinators omit the JSON flags; streaming adds partial events.
        if stream {
            let i = expected.1.iter().position(|a| a == "--verbose").unwrap();
            expected.1.insert(i, "--include-partial-messages".into());
        } else {
            let i = expected
                .1
                .iter()
                .position(|a| a == "--output-format")
                .unwrap();
            expected.1.drain(i..i + 2);
            expected.1.retain(|a| a != "--verbose");
        }
        assert_eq!(preview, expected);
    }
    trusted_policy(root.path(), "claude = \"aida\"");
    for stream in [false, true] {
        session::coordinator_headless_preview(
            None,
            HeadlessVendor::Claude,
            "synthetic",
            "dontAsk",
            AgentSeat::Advisor,
            true,
            stream,
        )
        .unwrap();
        let guard = session::child_reservation_guard();
        let mut cmd = session::build_coordinator_headless_command(
            None,
            HeadlessVendor::Claude,
            "synthetic",
            "dontAsk",
            AgentSeat::Advisor,
            "",
            true,
            stream,
        )
        .unwrap();
        assert!(cmd.get_args().any(|a| a == "--settings"));
        // Removing the opt-in after composition cannot downgrade an already
        // wrapper-owned command to the direct native spawn.
        trusted_policy(root.path(), "");
        cmd.env_remove(crate::gate::seat_authority::GRANT_ENV);
        let error = session::coordinator_status_with_handle_guard(
            HeadlessVendor::Claude,
            &mut cmd,
            root.path(),
            None,
            guard,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("explicit child grant decision"),
            "{error:#}"
        );
        trusted_policy(root.path(), "claude = \"aida\"");
        assert_eq!(
            session::claude_contained_flags_for(None, root.path())
                .unwrap_err()
                .to_string(),
            session::CLAUDE_WRAPPER_UNWIRED
        );
    }
}

#[path = "codex_auth_tests.rs"]
mod codex_auth_tests;

/// S2: exercise the actual foreground generation with synthetic Claude and
/// host terminal, including input, window changes, signal and refusal paths.
// trace:TASK-3219 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_3219_live_claude_interactive_private_pty() {
    claude_interactive_private_pty(false);
}

// The real agent-new issuer, receipt-bound worker and pre-release rollback.
// trace:TASK-3219 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_3219_live_claude_interactive_grant() {
    claude_interactive_private_pty(true);
}

#[cfg(target_os = "linux")]
fn claude_interactive_private_pty(granted: bool) {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !live_available() {
        return;
    }
    use crate::agent_launch::{
        run_agent_generation, AgentLaunchConfig, AgentLaunchPlan, AgentPromptStyle,
    };
    use crate::launch::contained::pty::{test_stdio, PrivatePty};
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "claude = \"aida\"");
    let _cwd = crate::session::contained_cwd_seam::pin(root.path());
    for p in crate::contained_boundary::contained_store_write_paths(root.path()) {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(
        root.path().join("host-net"),
        std::fs::read_link("/proc/self/ns/net")
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let home = crate::home_dir().unwrap();
    let config_home = home.join(".claude");
    std::fs::create_dir_all(&config_home).unwrap();
    credentials_expiring_in(&config_home, 8 * 3600);
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let program = bin.join("claude");
    std::fs::write(
        &program,
        r#"#!/usr/bin/python3
import os,sys,json,errno,fcntl,termios,signal,time
from pathlib import Path
if '--version' in sys.argv:
    if os.readlink('/proc/self/ns/net')==Path('host-net').read_text():
        Path(__file__).with_name('unconfined-version-ran').write_text('refused')
    assert os.environ['AIDA_CONTAINMENT_POSTURE']=='aida'
    assert 'AIDA_SESSION_GRANT' not in os.environ and 'AIDA_SESSION_ROLE' not in os.environ
    assert os.readlink('/proc/self/ns/net')!=Path('host-net').read_text()
    assert Path('preflight-ran').read_text()=='yes'
    Path(__file__).with_name('version-ran').write_text('confined query')
    print('synthetic-3219-pty');sys.exit(0)
signal.alarm(10)
assert all(os.isatty(fd) for fd in (0,1,2))
assert os.tcgetpgrp(0)==os.getpgrp()
assert os.environ['AIDA_HEADLESS']=='0'
assert os.environ['AIDA_HARNESS_VERSION']=='synthetic-3219-pty'
assert os.environ['AIDA_SESSION_ROLE']=='implementer'
assert bool(os.environ['AIDA_SESSION_GRANT'])==Path('expect-grant').exists()
assert os.environ['AIDA_USER']=='pty-3219'
assert os.environ['AIDA_CONTAINMENT_POSTURE']=='aida'
assert 'AIDA_AGENT_REGISTRY_TOKEN' not in os.environ
assert Path('preflight-ran').read_text()=='yes'
settings=json.loads(sys.argv[sys.argv.index('--settings')+1])
assert settings['sandbox']['enabled'] and settings['sandbox']['failIfUnavailable']
assert not settings['sandbox']['allowUnsandboxedCommands']
assert '--mcp-config' not in sys.argv and '--strict-mcp-config' not in sys.argv
try: Path('.git/config').write_text('escape');raise AssertionError('git writable')
except OSError as e: assert e.errno in (errno.EROFS,errno.EACCES,errno.EPERM)
Path('model-ran').write_text('yes')
print('PTY-READY',flush=True)
signal.signal(signal.SIGINT,lambda *_: (os.write(1,b'PTY-INT\n'),os._exit(7)))
signal.signal(signal.SIGTERM,lambda *_: (os.write(1,b'PTY-TERM\n'),os._exit(8)))
assert input()=='hello private terminal'
print('PTY-INPUT',flush=True)
end=time.monotonic()+5
while os.get_terminal_size().columns!=113 and time.monotonic()<end: time.sleep(.01)
assert os.get_terminal_size().columns==113
print('PTY-RESIZED',flush=True)
while True: signal.pause()
"#,
    )
    .unwrap();
    let aida = bin.join("aida");
    std::fs::write(
        &aida,
        "#!/bin/sh\nprintf yes > preflight-ran\necho 'seat_grant_valid=true expected_seat=true'\n",
    )
    .unwrap();
    for p in [&program, &aida] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _aida = crate::kernel::test_aida_exe::set(aida.clone());
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let mut env = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("PATH", Some(&path)),
            ("AIDA_CONTAINED", None),
            ("AIDA_AGENT_CMD", None),
            ("AIDA_OS_WRAP", Some("1")),
            ("CLAUDE_CONFIG_DIR", Some(config_home.to_str().unwrap())),
        ],
    );
    let flags = {
        let _scope = crate::session::ClaudeWrapperComposition::enter();
        crate::session::claude_contained_flags_for(None, root.path()).unwrap()
    };
    let mut config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: flags,
        prompt_style: AgentPromptStyle::Positional,
        contained: true,
        delegate_seats: vec!["reviewer".into()],
    };
    let mut plan = AgentLaunchPlan {
        project_root: root.path().into(),
        launch_cwd: root.path().into(),
        role: None,
        role_instance: crate::role::RoleInstanceKind::Driver,
        current_spec: None,
        name: "pty-3219".into(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    use crate::gate::seat_authority as authority;
    let parent = if granted {
        std::fs::write(root.path().join("expect-grant"), "yes").unwrap();
        env.set_key("AIDA_USER", "pty-parent");
        let id = authority::test_support::mint_grant_for_with_ttl(
            root.path(),
            "pty-parent",
            "advisor",
            &["implementer"],
            Some(chrono::Duration::hours(1)),
        );
        env.set_key(authority::GRANT_ENV, &id);
        plan.role = Some("implementer".into());
        config.delegate_seats.clear();
        Some(id)
    } else {
        None
    };
    let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _binding = if let Some(parent) = parent.clone() {
        let observed = observed.clone();
        let root = root.path().to_path_buf();
        Some(crate::launch::contained::pty::test_before_release(
            Box::new(move |id, pid| {
                assert!(
                    !root.join("model-ran").exists(),
                    "vendor ran before release"
                );
                let child = authority::read_grant(id).unwrap();
                assert_eq!(child.parent_grant_id.as_deref(), Some(parent.as_str()));
                assert_eq!(child.subject, "pty-3219");
                assert_eq!(child.seat, "implementer");
                assert!(child.revoked_at.is_none());
                assert!(
                    child
                        .binding
                        .iter()
                        .filter_map(|b| authority::binding_process(b))
                        .any(|(bound, _, _)| bound == pid),
                    "grant not bound to attested worker"
                );
                observed.borrow_mut().push(id.to_string());
                Ok(())
            }),
        ))
    } else {
        None
    };
    for external_term in [false, true] {
        let _ = std::fs::remove_file(root.path().join("model-ran"));
        let host = PrivatePty::open().unwrap();
        let (mut master, slave) = host.test_endpoints();
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut original) },
            0
        );
        let _stdio = test_stdio(slave.try_clone().unwrap(), slave.try_clone().unwrap());
        // Always collect a bounded terminal tail before checking the exit. A
        // worker traceback must survive early failure as well as the happy path.
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let driver_finished = finished.clone();
        let driver = std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(40);
            let mut tail = Vec::new();
            let (mut sent, mut resized, mut interrupted) = (false, false, false);
            while Instant::now() < end {
                let mut poll = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut poll, 1, 100) } <= 0 {
                    if driver_finished.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    continue;
                }
                let mut buf = [0; 4096];
                match master.read(&mut buf) {
                    Ok(n) if n > 0 => tail.extend_from_slice(&buf[..n]),
                    _ => break,
                }
                if tail.len() > 65536 {
                    tail.drain(..tail.len() - 65536);
                }
                let text = String::from_utf8_lossy(&tail);
                if !sent && text.contains("PTY-READY") {
                    master.write_all(b"hello private terminal\n").unwrap();
                    sent = true;
                }
                if !resized && text.contains("PTY-INPUT") {
                    let size = libc::winsize {
                        ws_row: 37,
                        ws_col: 113,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    assert_eq!(
                        unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
                        0
                    );
                    resized = true;
                }
                if !interrupted && text.contains("PTY-RESIZED") {
                    if external_term {
                        assert_eq!(
                            unsafe { libc::kill(std::process::id() as i32, libc::SIGTERM) },
                            0
                        );
                    } else {
                        master.write_all(&[3]).unwrap();
                    }
                    interrupted = true;
                }
                if text.contains(if external_term { "PTY-TERM" } else { "PTY-INT" }) {
                    return text.into_owned();
                }
            }
            String::from_utf8_lossy(&tail).into_owned()
        });
        let result = run_agent_generation(
            &program,
            &config,
            &plan,
            None,
            &[],
            None,
            false,
            false,
            None,
            None,
            || {
                // Regress the main-merge resolution: registry binding still
                // precedes supervision and retains the launcher's narrowing.
                let rows = crate::agent_registry::load_entries(root.path());
                let (_, row) = rows
                    .iter()
                    .find(|(_, row)| {
                        row.name.as_deref() == Some("pty-3219") && row.ended_at.is_none()
                    })
                    .expect("registered before spawn callback");
                assert_eq!(row.delegate_seats, config.delegate_seats);
                assert_ne!(row.pid, std::process::id());
            },
        );
        finished.store(true, std::sync::atomic::Ordering::Release);
        let terminal = driver.join().expect("PTY driver panicked");
        let result =
            result.unwrap_or_else(|error| panic!("generation refused: {error:#}\n{terminal}"));
        assert_eq!(
            result.status.code(),
            Some(if external_term { 8 } else { 7 }),
            "unexpected worker exit; PTY tail:\n{terminal}"
        );
        assert!(
            terminal.contains(if external_term { "PTY-TERM" } else { "PTY-INT" }),
            "incomplete terminal exchange; PTY tail:\n{terminal}"
        );
        let mut restored = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut restored) },
            0
        );
        assert_eq!(
            (
                restored.c_iflag,
                restored.c_oflag,
                restored.c_lflag,
                restored.c_cc
            ),
            (
                original.c_iflag,
                original.c_oflag,
                original.c_lflag,
                original.c_cc
            )
        );
    }
    drop(_binding);
    if granted {
        assert_eq!(observed.borrow().len(), 2, "both live workers were bound");
        for id in observed.borrow().iter() {
            authority::revoke_by_id(id).unwrap();
        }
        std::fs::remove_file(root.path().join("model-ran")).unwrap();
        // Refuse after binding but before release, then inspect the actual
        // child grant after IssuedLaunchGrant has unwound.
        let refused_observed = observed.clone();
        let _refuse =
            crate::launch::contained::pty::test_before_release(Box::new(move |id, pid| {
                let child = authority::read_grant(id).unwrap();
                assert!(child
                    .binding
                    .iter()
                    .filter_map(|b| authority::binding_process(b))
                    .any(|(bound, _, _)| bound == pid));
                refused_observed.borrow_mut().push(id.to_string());
                anyhow::bail!("injected pre-release refusal")
            }));
        let host = PrivatePty::open().unwrap();
        let (_, slave) = host.test_endpoints();
        let _stdio = test_stdio(slave.try_clone().unwrap(), slave.try_clone().unwrap());
        let error = run_agent_generation(
            &program,
            &config,
            &plan,
            None,
            &[],
            None,
            false,
            false,
            None,
            None,
            || panic!("released refused worker"),
        )
        .err()
        .expect("pre-release refusal must fail generation");
        assert!(format!("{error:#}").contains("injected pre-release refusal"));
        assert_eq!(observed.borrow().len(), 3);
        let id = observed.borrow().last().unwrap().clone();
        assert!(authority::read_grant(&id).unwrap().revoked_at.is_some());
        assert!(authority::read_grant(parent.as_ref().unwrap())
            .unwrap()
            .revoked_at
            .is_none());
        assert!(!root.path().join("model-ran").exists());
        return;
    }
    let host = PrivatePty::open().unwrap();
    let (_, slave) = host.test_endpoints();
    let _stdio = test_stdio(slave.try_clone().unwrap(), slave.try_clone().unwrap());
    assert_eq!(
        std::fs::read_to_string(bin.join("version-ran")).unwrap(),
        "confined query"
    );
    assert!(!bin.join("unconfined-version-ran").exists());
    std::fs::remove_file(root.path().join("model-ran")).unwrap();
    std::fs::write(&aida, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(run_agent_generation(
        &program,
        &config,
        &plan,
        None,
        &[],
        None,
        false,
        false,
        None,
        None,
        || panic!("released refused worker")
    )
    .is_err());
    assert!(!root.path().join("model-ran").exists());
}

// trace:TASK-3219 | ai:codex
#[test]
fn task_3219_interactive_native_generation_and_early_refusals() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::agent_launch::{
        run_agent_generation, AgentLaunchConfig, AgentLaunchPlan, AgentPromptStyle,
    };
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "");
    let _cwd = crate::session::contained_cwd_seam::pin(root.path());
    let program = root.path().join("claude");
    std::fs::write(&program,"#!/bin/sh\nif [ \"$1\" = --version ]; then echo native-3219; exit 0; fi\n[ \"$1\" = native-argument ] || exit 1\n[ \"$AIDA_CONTAINMENT_POSTURE\" = aida ] || exit 2\n[ -z \"${AIDA_SESSION_GRANT:-}\" ] || exit 3\nprintf native > native-ran\nexit 9\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_AGENT_CMD", None),
        ],
    );
    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec!["native-argument".into()],
        prompt_style: AgentPromptStyle::Positional,
        contained: true,
        delegate_seats: vec!["reviewer".into()],
    };
    let plan = AgentLaunchPlan {
        project_root: root.path().into(),
        launch_cwd: root.path().into(),
        role: None,
        role_instance: crate::role::RoleInstanceKind::Driver,
        current_spec: None,
        name: "native-3219".into(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    let mut spawned = false;
    let result = run_agent_generation(
        &program,
        &config,
        &plan,
        None,
        &[],
        None,
        false,
        false,
        None,
        None,
        || {
            spawned = true;
        },
    )
    .unwrap();
    assert!(spawned);
    let rows = crate::agent_registry::load_entries(root.path());
    let (_, row) = rows
        .iter()
        .find(|(_, row)| row.name.as_deref() == Some("native-3219"))
        .expect("native generation registered");
    assert_eq!(row.delegate_seats, config.delegate_seats);
    assert!(row.ended_at.is_some());
    assert_eq!(result.status.code(), Some(9));
    assert_eq!(
        std::fs::read_to_string(root.path().join("native-ran")).unwrap(),
        "native"
    );
    // These refuse before policy/credential/state work, even on a nonexistent root.
    for flag in [
        "--bg",
        "--resume",
        "--continue",
        "--fork-session",
        "--dangerously-skip-permissions",
    ] {
        let mut cmd = Command::new("claude");
        cmd.arg(flag);
        let error =
            PreparedLaunch::prepare_interactive(&mut cmd, Path::new("/nonexistent/3219"), None)
                .err()
                .expect("unsupported surface");
        assert!(
            error.to_string().contains(if flag == "--bg" {
                "supervisor"
            } else if flag == "--dangerously-skip-permissions" {
                "permission"
            } else {
                "resume"
            }),
            "{error:#}"
        );
    }
}

// trace:TASK-3219 | ai:codex
#[test]
fn task_3219_interactive_version_query_bounds_and_cleanup() {
    let check = r#"
import tempfile,pathlib,time
with tempfile.TemporaryDirectory() as directory:
    program=pathlib.Path(directory)/'claude'
    def query(body):
        program.write_text('#!/bin/sh\n'+body)
        program.chmod(0o700)
        return interactive_harness_version(str(program))
    assert query('printf "synthetic-version\\n"')=='synthetic-version'
    assert query('printf "two\\nlines\\n"') is None
    assert query('printf version; exit 1') is None
    assert query('printf "%0258d" 0') is None
    start=time.monotonic()
    assert query('sleep 30') is None
    assert time.monotonic()-start<5
    start=time.monotonic()
    assert query('(sleep 30) &\nprintf version') is None
    assert time.monotonic()-start<5
print('bounded version cases: 6 passed')
"#;
    let output = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c"])
        .arg(format!("{COMMON}\n{LANE}\n{check}"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("6 passed"));
}

// trace:BUG-2487 | ai:codex
fn bug_2487_codex_env() -> BTreeMap<String, String> {
    TOOL_NET_ENV
        .iter()
        .chain(TOOL_BUILD_ENV)
        .map(|name| ((*name).into(), String::new()))
        .chain([
            ("AIDA_SESSION_GRANT".into(), String::new()),
            ("AIDA_ROLE_INSTANCE".into(), "companion".into()),
        ])
        .collect()
}

#[test]
// trace:BUG-2487 | ai:codex
fn bug_2487_pinned_help_checks_production_argv_and_scopes() {
    let recorded: serde_json::Value =
        serde_json::from_str(include_str!("../../launch/codex_0166_help.json")).unwrap();
    assert_eq!(
        recorded["sha256"],
        codex_auth::PROGRAM_SHA256,
        "pin changed: re-record exec/sandbox help from the newly probed binary"
    );
    assert_eq!(recorded["version"], "codex-cli 0.166.0");
    let original = aida_core::vendors::profile(aida_core::vendors::LaunchVendor::Codex)
        .headless_args(
            "prompt containing -P and --unknown-flag",
            "",
            Some("fixture-model"),
            Some("high"),
            // trace:BUG-2495 | ai:codex — pair the real site posture with
            // the pinned exec help, including its top-level approval flag.
            Some(
                crate::codex_native_sandbox::CODEX_NATIVE_POSTURE
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
        )
        .unwrap();
    let (args, preflight) = codex_argv(
        Path::new("/fixture/codex"),
        &bug_2487_codex_env(),
        &original,
    )
    .unwrap();
    assert!(args
        .windows(2)
        .any(|v| v == ["-c", "default_permissions=\"aida\""]));
    assert!(!args
        .iter()
        .any(|v| v == "-P" || v == "--permission-profile"));
    assert!(preflight.windows(2).any(|v| v == ["-P", "aida"]));
    assert!(preflight.iter().any(|v| v == "--include-managed-config"));
    validate_codex_flags(&args, "exec").unwrap();
    validate_codex_flags(&preflight[1..preflight.len() - 1], "sandbox").unwrap();
    for unsupported in ["-P", "--permission-profile", "--future-unknown"] {
        let injected = vec!["exec".into(), unsupported.into(), "aida".into()];
        let error = validate_codex_flags(&injected, "exec")
            .unwrap_err()
            .to_string();
        assert!(error.contains(unsupported), "{error}");
    }
    let error = codex_argv(
        Path::new("/fixture/codex"),
        &bug_2487_codex_env(),
        &["exec".into(), "--future-unknown".into(), "prompt".into()],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("--future-unknown"), "{error}");
    assert!(validate_codex_flags(&["sandbox".into(), "--json".into()], "sandbox").is_err());
}

#[test]
// trace:BUG-2495 | ai:codex
fn bug_2495_codex_approval_flags_use_exec_config() {
    for approval in [
        vec!["--ask-for-approval", "never"],
        vec!["--ask-for-approval=never"],
        vec!["-a", "never"],
    ] {
        let original: Vec<String> = ["exec", "--sandbox", "workspace-write"]
            .into_iter()
            .chain(approval)
            .chain(["prompt containing --ask-for-approval on-request"])
            .map(String::from)
            .collect();
        let (args, preflight) = codex_argv(
            Path::new("/fixture/codex"),
            &bug_2487_codex_env(),
            &original,
        )
        .unwrap();
        assert!(args
            .windows(2)
            .any(|v| v == ["-c", "approval_policy=\"never\""]));
        assert!(!args.iter().any(|v| v == "-a"
            || v == "--ask-for-approval"
            || v.starts_with("--ask-for-approval=")));
        assert_eq!(args.last(), original.last());
        validate_codex_flags(&args, "exec").unwrap();
        validate_codex_flags(&preflight[1..preflight.len() - 1], "sandbox").unwrap();
    }
}

#[test]
// trace:BUG-2495 | ai:codex
fn bug_2495_codex_approval_flags_refuse_other_or_missing_values() {
    for value in ["on-request", "untrusted", "on-failure", "", "unknown"] {
        for approval in [
            vec!["--ask-for-approval".into(), value.into()],
            vec![format!("--ask-for-approval={value}")],
            vec!["-a".into(), value.into()],
        ] {
            let original = ["exec".into()]
                .into_iter()
                .chain(approval)
                .chain(["prompt".into()])
                .collect::<Vec<String>>();
            let error = codex_argv(
                Path::new("/fixture/codex"),
                &bug_2487_codex_env(),
                &original,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("approval policy must be never"), "{error}");
        }
    }
    for flag in ["--ask-for-approval", "-a"] {
        let error = codex_argv(
            Path::new("/fixture/codex"),
            &bug_2487_codex_env(),
            &["exec".into(), flag.into()],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("approval flag requires a value"), "{error}");
    }
}

#[test]
// trace:BUG-2487 | ai:codex
fn bug_2487_installed_exec_honours_builder_profile() {
    if !installed_codex_required() {
        eprintln!("SKIP: set AIDA_REQUIRE_CODEX_INSTALLED_LIVE=1 for pinned exec write probe");
        return;
    }
    let program = resolve_program(
        &Command::new("codex"),
        &BTreeMap::from([(
            "PATH".into(),
            aida_core::exec_context::Environment::capture()
                .var("PATH")
                .unwrap()
                .into(),
        )]),
    )
    .unwrap();
    let program = codex_auth::resolve_native_program(&program).unwrap();
    codex_auth::require_program(&program, codex_auth::PROGRAM_SHA256).unwrap();
    let (args, preflight) = codex_argv(
        &program,
        &bug_2487_codex_env(),
        &[
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--json".into(),
            "--sandbox".into(),
            "workspace-write".into(),
            "Run the fixture tool call.".into(),
        ],
    )
    .unwrap();
    let output = Command::new("/usr/bin/python3")
        .args(["-I", "-S"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/containment/tests/codex_exec_profile_probe.py"),
        )
        .arg(program)
        .arg(serde_json::to_string(&args).unwrap())
        .arg(serde_json::to_string(&preflight).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_wrapper_environment_keeps_ambient_values_implicit() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::launch::context::{LaunchContext, LaunchRequest};
    use aida_core::launch_policy::{LaunchKind, LaunchVendor};
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some("/parent/bin:/usr/bin:/bin")),
        ("AIDA_SESSION_ID", Some("parent-session")),
        ("AIDA_ROTATION_HANDOFF", Some("parent-handoff")),
        ("AIDA_AGENT_CMD", None),
    ]);
    let req = LaunchRequest {
        vendor: LaunchVendor::Claude,
        kind: LaunchKind::Fresh,
        program: "claude".into(),
        args: vec![],
        tail: vec![],
        posture: crate::session::ContainmentPosture::Aida,
        cwd: std::env::temp_dir(),
        set_cwd: false,
        host_main: None,
    };
    let ctx = LaunchContext::resolve_under(req, false)
        .unwrap()
        .with_wrapper_environment(true);
    let mut cmd = crate::launch::spawn_agent(&ctx).unwrap();
    let env = selected_environment(&cmd);
    for key in ["PATH", "AIDA_SESSION_ID", "AIDA_ROTATION_HANDOFF"] {
        assert!(
            !env.contains_key(key),
            "ambient {key} crossed wrapper selection"
        );
        assert!(explicit(&cmd, key).is_none());
    }
    cmd.env("PATH", "/site/bin")
        .env("AIDA_SESSION_ID", "child-session")
        .env("AIDA_ROTATION_HANDOFF", "child-handoff")
        .env("AIDA_REVIEW_VERDICT_FILE", "/site/verdict.json");
    assert_eq!(selected_environment(&cmd)["PATH"], "/site/bin");
    assert_eq!(
        selected_environment(&cmd)["AIDA_SESSION_ID"],
        "child-session"
    );
    assert_eq!(
        explicit(&cmd, "AIDA_ROTATION_HANDOFF").as_deref(),
        Some("child-handoff")
    );
    assert_eq!(
        selected_environment(&cmd)["AIDA_REVIEW_VERDICT_FILE"],
        "/site/verdict.json"
    );
}

/// Exercise both callers of the shared command builder under real trusted
/// wrapper policy, rather than setting LaunchContext's flag in the test.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_wrapper_headless_builders_keep_parent_context_out_of_selection() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::session::{self, HeadlessVendor};
    use aida_core::agents_config::AgentSeat;
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "claude = \"aida\"");
    let _cwd = session::contained_cwd_seam::pin(root.path());
    let _env = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_CONTAINED", None),
            ("AIDA_AGENT_CMD", None),
            ("AIDA_SESSION_ID", Some("parent-session")),
            ("AIDA_HARNESS_VERSION", Some("parent-harness")),
            ("AIDA_ROTATION_HANDOFF", Some("parent-handoff")),
            ("AIDA_AGENT_CONTEXT_FILE", Some("/parent/context")),
        ],
    );
    let coordinator = session::build_coordinator_headless_command(
        None,
        HeadlessVendor::Claude,
        "prompt",
        "dontAsk",
        AgentSeat::Advisor,
        "",
        true,
        false,
    )
    .unwrap();
    let exec = {
        let _composition = session::ClaudeWrapperComposition::enter();
        session::build_exec_vendor_headless_command(
            None,
            HeadlessVendor::Claude,
            "prompt",
            "child-session",
            true,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap()
    };
    for mut cmd in [coordinator, exec] {
        let env = selected_environment(&cmd);
        for key in [
            "AIDA_SESSION_ID",
            "AIDA_ROTATION_HANDOFF",
            "AIDA_AGENT_CONTEXT_FILE",
        ] {
            assert!(
                explicit(&cmd, key).is_none(),
                "ambient {key} became explicit"
            );
            assert!(
                !env.contains_key(key),
                "ambient {key} crossed wrapper selection"
            );
        }
        assert_ne!(
            env.get("AIDA_HARNESS_VERSION").map(String::as_str),
            Some("parent-harness")
        );
        // A child lease assigned by the owner remains transportable.
        cmd.env("AIDA_SESSION_ID", "child-lease");
        assert_eq!(selected_environment(&cmd)["AIDA_SESSION_ID"], "child-lease");
    }
}

// Both wrapper entrypoints reject an accidental env_clear/projection before
// touching the root, credentials, store or broker. This protects future sites
// that omit LaunchContext's wrapper_environment decision.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_wrapper_refuses_projected_commands_before_io() {
    for interactive in [false, true] {
        let mut cmd = Command::new("claude");
        cmd.env_clear().env("AIDA_SESSION_ID", "projected-parent");
        let result = if interactive {
            PreparedLaunch::prepare_interactive(&mut cmd, Path::new("/nonexistent/2326"), None)
        } else {
            PreparedLaunch::prepare(&mut cmd, Path::new("/nonexistent/2326"), None)
        };
        let error = result.err().expect("projected command must refuse");
        assert!(
            error
                .to_string()
                .contains("unprojected command environment"),
            "{error:#}"
        );
    }
}
