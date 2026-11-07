// trace:STORY-2593 | ai:codex
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::os::unix::fs::FileExt;

fn token(value: serde_json::Value) -> String {
    format!(
        "e30.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    )
}
fn host_auth(home: &Path, exp: i64) -> serde_json::Value {
    let value = serde_json::json!({"auth_mode":"chatgpt", "OPENAI_API_KEY":"full-cache-sentinel",
        "tokens":{"access_token":token(serde_json::json!({"exp":exp,"synthetic_access_sentinel":"2593"})),
        "id_token":token(serde_json::json!({"exp":4102444800_i64,"email":"fixture@example.invalid","https://api.openai.com/auth":{"chatgpt_account_id":"account-sentinel","chatgpt_plan_type":"plus"}})),
        "account_id":"account-sentinel", "refresh_token":"host-refresh-sentinel"},
        "last_refresh":"2026-11-07T00:00:00Z"});
    let path = home.join("auth.json");
    std::fs::write(&path, value.to_string()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    value
}
fn prepare_auth(home: &Path, ttl: i64) -> Result<codex_auth::Credential> {
    codex_auth::prepare(
        home,
        unsafe { libc::getuid() },
        Some("codex-cli 0.166.0"),
        ttl,
    )
}
fn error(home: &Path) -> String {
    match prepare_auth(home, 3600) {
        Ok(_) => panic!("unexpected credential admission"),
        Err(e) => format!("{e:#}"),
    }
}
#[test]
fn story_2593_reduced_sealed_leaf_and_lifetime() {
    let home = tempfile::tempdir().unwrap();
    let original = host_auth(home.path(), chrono::Utc::now().timestamp() + 7200);
    let before = std::fs::read(home.path().join("auth.json")).unwrap();
    let credential = prepare_auth(home.path(), 3600).unwrap();
    assert!(credential.deadline <= chrono::Utc::now().timestamp() + 3600);
    let mut bytes = vec![0; 65536];
    let n = credential.file.read_at(&mut bytes, 0).unwrap();
    let reduced: serde_json::Value = serde_json::from_slice(&bytes[..n]).unwrap();
    assert_eq!(reduced.as_object().unwrap().len(), 3);
    assert_eq!(reduced["tokens"].as_object().unwrap().len(), 4);
    assert_eq!(reduced["tokens"]["refresh_token"], "");
    assert_eq!(reduced["tokens"]["account_id"], "account-sentinel");
    assert_ne!(reduced["last_refresh"], original["last_refresh"]);
    assert_eq!(
        reduced["tokens"]["access_token"],
        original["tokens"]["access_token"]
    );
    assert_eq!(
        reduced["tokens"]["id_token"],
        original["tokens"]["id_token"]
    );
    let text = String::from_utf8(bytes[..n].into()).unwrap();
    assert!(!text.contains("host-refresh-sentinel") && !text.contains("full-cache-sentinel"));
    assert!(credential.file.write_at(b"x", 0).is_err());
    assert_eq!(
        before,
        std::fs::read(home.path().join("auth.json")).unwrap()
    );
    assert!(prepare_auth(home.path(), 7200).is_err());
    assert!(prepare_auth(home.path(), i64::MAX).is_err());
    for exp in [0, chrono::Utc::now().timestamp() + 3601] {
        host_auth(home.path(), exp);
        assert!(error(home.path()).contains("relaunch"));
    }
    host_auth(home.path(), chrono::Utc::now().timestamp() + 7200);
    for version in [None, Some("codex-cli 0.166.1"), Some("0.166.0")] {
        assert!(
            codex_auth::prepare(home.path(), unsafe { libc::getuid() }, version, 3600).is_err()
        );
    }
}
#[test]
fn story_2593_credential_identity_schema_and_redaction() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("auth.json");
    assert!(error(home.path()).contains("keyring"));
    host_auth(home.path(), chrono::Utc::now().timestamp() + 7200);
    assert!(codex_auth::prepare(
        home.path(),
        unsafe { libc::getuid() }.wrapping_add(1),
        Some("codex-cli 0.166.0"),
        3600
    )
    .is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(error(home.path()).contains("permissions"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = home.path().join("alias");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(error(home.path()).contains("identity"));
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&alias, &path).unwrap();
    assert!(error(home.path()).contains("unavailable"));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(error(home.path()).contains("identity"));
    std::fs::remove_dir(&path).unwrap();
    let valid = host_auth(home.path(), chrono::Utc::now().timestamp() + 7200);
    for value in [
        serde_json::json!({}),
        serde_json::json!({"auth_mode":"keyring"}),
        serde_json::json!({"auth_mode":"chatgpt", "tokens":{"access_token":"secret-sentinel"}}),
    ] {
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(!error(home.path()).contains("secret-sentinel"));
    }
    for bad in ["not-json-secret-sentinel".to_string(), "x".repeat(65537)] {
        std::fs::write(&path, bad).unwrap();
        assert!(!error(home.path()).contains("secret-sentinel"));
    }
    for access in [
        "secret-sentinel".into(),
        token(serde_json::json!({})),
        token(serde_json::json!({"exp":"secret-sentinel"})),
    ] {
        let mut invalid = valid.clone();
        invalid["tokens"]["access_token"] = access.into();
        std::fs::write(&path, invalid.to_string()).unwrap();
        assert!(!error(home.path()).contains("secret-sentinel"));
    }
    let mut invalid = valid;
    invalid["tokens"]["account_id"] = "different-account".into();
    std::fs::write(&path, invalid.to_string()).unwrap();
    assert!(error(home.path()).contains("routing mismatch"));
}

/// Real common wrapper: terminal reads only the reduced leaf, cannot modify
/// it, cannot recover host credentials through the masked relocation, and
/// cannot connect to the OAuth token endpoint even when policy listed it.
#[test]
fn story_2593_live_credential_and_refresh_sentinels() {
    if !live_available() {
        return;
    }
    let host = tempfile::tempdir().unwrap();
    let original = host_auth(host.path(), chrono::Utc::now().timestamp() + 7200);
    let before = std::fs::read(host.path().join("auth.json")).unwrap();
    let script = format!(
        r#"
import os,json,socket
from pathlib import Path
p=Path('private-codex/auth.json')
a=json.loads(p.read_text())
assert a['tokens']['refresh_token']==''
assert set(a['tokens'])=={{'id_token','access_token','refresh_token','account_id'}}
assert a['tokens']['account_id']=='account-sentinel'
mount=next(line for line in Path('/proc/self/mountinfo').read_text().splitlines() if line.split()[4]==str(p.resolve()))
assert ' - tmpfs ' in mount and 'ro' in mount.split()[5].split(',')
assert not Path({host:?},'auth.json').exists()
try:
    p.write_text('changed');raise AssertionError('credential writable')
except OSError: pass
s=socket.socket();s.settimeout(3);s.connect(('127.0.0.1',int(os.environ['HTTP_PROXY'].rsplit(':',1)[1])))
s.sendall(b'CONNECT auth.openai.com:443 HTTP/1.1\r\nHost: auth.openai.com:443\r\n\r\n')
assert b'200' not in s.recv(4096)
print('credential and refresh sentinels passed')
"#,
        host = host.path().to_str().unwrap()
    );
    let mut plan = fixture(&script, "print('seat_grant_valid=true expected_seat=true')");
    let private = plan._state.path().join("private-codex");
    std::fs::create_dir(&private).unwrap();
    let credential = prepare_auth(host.path(), 3600).unwrap();
    codex_auth::bind(&mut plan.profile, &private, credential).unwrap();
    let empty = plan._state.path().join("empty-mask");
    std::fs::create_dir(&empty).unwrap();
    plan.profile.add(host.path(), false).unwrap(); // readable positive control before masking
    plan.profile.mounts.push(Mount {
        data: false,
        file: File::open(empty).unwrap(),
        path: host.path().into(),
        writable: false,
    });
    let mut allowed = vec!["auth.openai.com".into(), "chatgpt.com".into()];
    codex_auth::exclude_oauth_hosts(&mut allowed);
    let args: Vec<_> = plan.command.get_args().collect();
    let mut cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    cfg["allowed"] = serde_json::json!(allowed);
    cfg["bwrap"] = serde_json::json!(plan.profile.args(plan._state.path()));
    let env = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    let root = plan._state.path().to_path_buf();
    let mut plan = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut output = tempfile::tempfile().unwrap();
    plan.command.stderr(output.try_clone().unwrap());
    let mut run = match plan.spawn(output.try_clone().unwrap().into()) {
        Ok(run) => run,
        Err(error) => {
            use std::io::{Seek, SeekFrom};
            output.seek(SeekFrom::Start(0)).unwrap();
            let mut diagnostic = String::new();
            output.read_to_string(&mut diagnostic).unwrap();
            panic!("credential fixture failed: {error:#}: {diagnostic}");
        }
    };
    run.release().unwrap();
    assert!(run.child.wait().unwrap().success());
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    assert!(text.contains("credential and refresh sentinels passed"));
    assert!(!text.contains("host-refresh-sentinel") && !text.contains("full-cache-sentinel"));
    assert!(!text.contains(original["tokens"]["access_token"].as_str().unwrap()));
    assert_eq!(
        before,
        std::fs::read(host.path().join("auth.json")).unwrap()
    );
    let failed = Command::new("/bin/false").status().unwrap();
    run.chatgpt = true;
    assert_eq!(
        run.vendor_exit(failed).unwrap_err().to_string(),
        codex_auth::RELAUNCH
    );
}

#[test]
fn story_2593_oauth_policy_denies_before_resolution() {
    let mut allowed = vec![
        "auth.openai.com".into(),
        "auth0.openai.com".into(),
        "openai.auth0.com".into(),
        "auth.chatgpt.com".into(),
        "chatgpt.com".into(),
    ];
    codex_auth::exclude_oauth_hosts(&mut allowed);
    assert_eq!(allowed, vec!["chatgpt.com"]);
    let script = format!(
        r#"{COMMON}
{HOST}
a,b=socket.socketpair(socket.AF_UNIX,socket.SOCK_SEQPACKET)
request=dict(version=1,method='CONNECT',host='auth.openai.com',port=443,
             host_header='auth.openai.com:443',protocol='tcp')
resolve=lambda *_:[(socket.AF_INET,socket.SOCK_STREAM,socket.IPPROTO_TCP,'',('93.184.216.34',443))]
assert destination(request,{{('auth.openai.com',443)}},resolve)[-1]==('93.184.216.34',443)
try:
    destination(request,{{(h,443) for h in {allowed:?}}},lambda *_: (_ for _ in ()).throw(AssertionError('DNS reached')))
    raise AssertionError('OAuth destination admitted')
except RuntimeError as error:
    assert str(error)=='proxy destination refused'
request=json.dumps(request).encode()
proxy_request(request,[a.detach()],{{(h,443) for h in {allowed:?}}},None)
assert b.recv(100)==b'denied-v1'
"#
    );
    assert!(Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", &script])
        .status()
        .unwrap()
        .success());
}

#[test]
fn story_2593_live_host_deadline_stops_lane() {
    if !live_available() {
        return;
    }
    let plan = fixture(
        "import time;time.sleep(30)",
        "print('seat_grant_valid=true expected_seat=true')",
    );
    let args: Vec<_> = plan.command.get_args().collect();
    let mut cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    cfg["chatgpt_deadline"] = (chrono::Utc::now().timestamp() + 3).into();
    let env = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
        .collect();
    let root = plan._state.path().to_path_buf();
    let mut plan = PreparedLaunch::from_config(
        plan.profile,
        plan._state,
        "synthetic-sha".into(),
        cfg,
        env,
        &root,
    )
    .unwrap();
    let mut output = tempfile::tempfile().unwrap();
    plan.command.stderr(output.try_clone().unwrap());
    let mut run = plan.spawn(Stdio::null()).unwrap();
    run.release().unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = run.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < end,
            "host credential deadline did not stop lane"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    output.read_to_string(&mut text).unwrap();
    assert!(text.contains("relaunch"), "{text}");
}

#[test]
fn story_2593_bind_keeps_secrets_off_disk_and_out_of_command() {
    let host = tempfile::tempdir().unwrap();
    let original = host_auth(host.path(), chrono::Utc::now().timestamp() + 7200);
    let private = tempfile::tempdir().unwrap();
    let mut profile = WriteProfile::new();
    profile.add(private.path(), true).unwrap();
    let credential = prepare_auth(host.path(), 3600).unwrap();
    codex_auth::bind(&mut profile, private.path(), credential).unwrap();
    let leaf = profile
        .mounts
        .iter()
        .find(|m| m.path == private.path().join("auth.json"))
        .unwrap();
    assert!(!leaf.writable);
    assert!(leaf.data);
    assert_eq!(std::fs::read(&leaf.path).unwrap(), b"");
    let config = std::fs::read_to_string(private.path().join("config.toml")).unwrap();
    assert!(config.contains("cli_auth_credentials_store = \"file\""));
    let diagnostic = format!("{:?}{}", profile.args(private.path()), config);
    for secret in [
        original["tokens"]["access_token"].as_str().unwrap(),
        "host-refresh-sentinel",
        "full-cache-sentinel",
    ] {
        assert!(!diagnostic.contains(secret));
    }
}

#[test]
fn story_2593_production_prepare_uses_private_chatgpt_leaf() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    if !Path::new("/usr/bin/bwrap").exists() {
        skip_under(live_required(), "prepare requires bwrap");
        return;
    }
    if let Err(error) = host_prerequisites() {
        skip_under(live_required(), &format!("{error:#}"));
        return;
    }
    let home = crate::home_dir().unwrap();
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "codex = \"aida\"");
    for path in crate::contained_boundary::contained_store_write_paths(root.path()) {
        std::fs::create_dir_all(path).unwrap();
    }
    let relocated = home.join("relocated-codex");
    for dir in [home.join(".codex"), relocated.clone()] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let original = host_auth(&relocated, chrono::Utc::now().timestamp() + 7200);
    std::fs::write(home.join(".codex/host-sentinel"), "default-secret").unwrap();
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("codex");
    std::fs::write(&program, "#!/bin/sh\necho 'codex-cli 0.166.0'\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[("CODEX_HOME", relocated.to_str().unwrap())]);
    let mut child = Command::new(&program);
    child
        .args(["exec", "synthetic"])
        .env("AIDA_HEADLESS", "1")
        .env("AIDA_SESSION_GRANT", "")
        .env("AIDA_SESSION_ROLE", "companion")
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("OPENAI_API_KEY", "alternate-credential-sentinel");
    let plan = PreparedLaunch::prepare(&child, root.path(), None).unwrap();
    assert!(plan.chatgpt);
    let env: BTreeMap<_, _> = plan
        .command
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_str()?, v?.to_str()?)))
        .collect();
    assert!(!env.contains_key("OPENAI_API_KEY"));
    let private = Path::new(env["CODEX_HOME"]);
    assert!(private.starts_with(plan._state.path()));
    assert!(plan
        .profile
        .mounts
        .iter()
        .any(|m| m.path == private.join("auth.json") && !m.writable));
    for path in [home.join(".codex"), relocated] {
        let mount = plan.profile.mounts.iter().find(|m| m.path == path).unwrap();
        assert!(!mount.writable);
        assert!(
            std::fs::read_dir(format!("/proc/self/fd/{}", mount.file.as_raw_fd()))
                .unwrap()
                .next()
                .is_none()
        );
    }
    let args: Vec<_> = plan.command.get_args().collect();
    let cfg: serde_json::Value = serde_json::from_str(args[5].to_str().unwrap()).unwrap();
    assert!(cfg["chatgpt_deadline"].as_i64().unwrap() > chrono::Utc::now().timestamp());
    let diagnostic = format!("{:?}", plan.command);
    for secret in [
        original["tokens"]["access_token"].as_str().unwrap(),
        "host-refresh-sentinel",
        "alternate-credential-sentinel",
    ] {
        assert!(!diagnostic.contains(secret));
    }
    drop(plan);
    std::fs::write(&program, "#!/bin/sh\necho 'codex-cli 0.166.1'\n").unwrap();
    let result = PreparedLaunch::prepare(&child, root.path(), None);
    assert!(match result {
        Ok(_) => false,
        Err(e) => format!("{e:#}").contains("re-probe this codex version"),
    });
}

#[test]
fn story_2593_installed_codex_exact_leaf_probe() {
    if !installed_codex_required() {
        eprintln!("SKIP: exact installed-Codex credential probe requires AIDA_REQUIRE_CODEX_INSTALLED_LIVE=1");
        return;
    }
    let env = BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]);
    let program = resolve_program(&Command::new("codex"), &env).unwrap();
    let host = tempfile::tempdir().unwrap();
    host_auth(host.path(), 4102444800);
    let credential = prepare_auth(host.path(), 3600).unwrap();
    let fd = credential.file.as_raw_fd();
    let mut probe = Command::new("/usr/bin/python3");
    probe
        .env_clear()
        .args(["-I", "-S", "-c", include_str!("codex_auth_probe.py")])
        .arg(program)
        .arg(fd.to_string());
    unsafe {
        probe.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    assert!(probe.status().unwrap().success());
}

#[test]
#[ignore = "watchdog child only"]
fn story_2593_fifo_reader_child() {
    let home = tempfile::tempdir().unwrap();
    let name = std::ffi::CString::new(home.path().join("auth.json").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(error(home.path()).contains("identity"));
}

#[test]
fn story_2593_fifo_refusal_has_watchdog() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launch::contained::tests::codex_auth_tests::story_2593_fifo_reader_child",
            "--ignored",
            "--nocapture",
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("FIFO credential open blocked");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
