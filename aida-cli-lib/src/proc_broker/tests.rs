// trace:TASK-3368 | ai:codex
use super::transport::*;
use super::*;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

fn until() -> Instant {
    Instant::now() + DEADLINE
}
pub(super) fn kernel() -> bool {
    match probe() {
        Ok(()) => true,
        Err(e) => {
            // Only lack of kernel features may skip. Broken procfs or sender
            // authentication on a supporting kernel is a test failure.
            let unsupported = e.chain().any(|cause| {
                cause.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(
                        e.raw_os_error(),
                        Some(libc::ENOPROTOOPT | libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
                    )
                })
            });
            if unsupported {
                eprintln!("SKIP real-kernel SCM_PIDFD test: {e:#}");
                false
            } else {
                panic!("kernel probe/authentication failed: {e:#}");
            }
        }
    }
}
fn own() -> ProcIdentity {
    ProcIdentity::capture(std::process::id()).expect("proven host procfs coordinates")
}
fn request() -> Request {
    let id = own();
    Request {
        version: 1,
        nonce: Uuid::new_v4(),
        project: "project-a".into(),
        store: "store-a".into(),
        operation: Operation::ObserveSelf,
        subject: Identity {
            boot_id: id.boot_id.parse().unwrap(),
            pidns_inode: id.pidns_inode,
            pid: id.pid,
            starttime: id.starttime,
        },
        holder_generation: Uuid::new_v4(),
        grant: None,
    }
}
fn dispatch(session: &mut Session, request: &Request) -> Response {
    session.dispatch(
        &serde_json::to_vec(request).unwrap(),
        &own(),
        "project-a",
        "store-a",
    )
}
fn positive(session: &mut Session) {
    assert_eq!(dispatch(session, &request()).refusal, Refusal::Disabled);
}
fn runtime() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[test]
fn service_stays_disabled() {
    assert!(run()
        .unwrap_err()
        .to_string()
        .contains("manual rollout gate pending"));
}
#[test]
fn closed_schema_and_scope_with_positive_controls() {
    let mut session = Session::new();
    positive(&mut session);
    let cases = [
        ("version", serde_json::json!(2), Refusal::Schema),
        ("operation", serde_json::json!("signal"), Refusal::Schema),
        ("project", serde_json::json!("project-b"), Refusal::Scope),
        ("store", serde_json::json!("store-b"), Refusal::Scope),
        ("uid", serde_json::json!(0), Refusal::Schema),
        ("nonce", serde_json::json!(Uuid::nil()), Refusal::Schema),
        (
            "holder_generation",
            serde_json::json!(Uuid::nil()),
            Refusal::Schema,
        ),
    ];
    for (field, value, expected) in cases {
        let mut wire = serde_json::to_value(request()).unwrap();
        wire[field] = value;
        assert_eq!(
            session
                .dispatch(
                    &serde_json::to_vec(&wire).unwrap(),
                    &own(),
                    "project-a",
                    "store-a"
                )
                .refusal,
            expected
        );
        positive(&mut session);
    }
    for field in ["pid", "pidns_inode", "starttime"] {
        let mut wire = serde_json::to_value(request()).unwrap();
        wire["subject"][field] = 0.into();
        assert_eq!(
            session
                .dispatch(
                    &serde_json::to_vec(&wire).unwrap(),
                    &own(),
                    "project-a",
                    "store-a"
                )
                .refusal,
            Refusal::Schema
        );
        positive(&mut session);
    }
    let mut wire = serde_json::to_value(request()).unwrap();
    wire["subject"]["untrusted_namespace"] = 123.into();
    assert_eq!(
        session
            .dispatch(
                &serde_json::to_vec(&wire).unwrap(),
                &own(),
                "project-a",
                "store-a"
            )
            .refusal,
        Refusal::Schema
    );
    let wire = serde_json::to_string(&request()).unwrap();
    let duplicate = format!("{{\"version\":1,{}", &wire[1..]);
    assert_eq!(
        session
            .dispatch(duplicate.as_bytes(), &own(), "project-a", "store-a")
            .refusal,
        Refusal::Schema
    );
    positive(&mut session);
    assert_eq!(
        session
            .dispatch(b"{", &own(), "project-a", "store-a")
            .refusal,
        Refusal::Schema
    );
    positive(&mut session);
}
#[test]
fn foreign_coordinates_and_forged_subject_never_observe() {
    let mut session = Session::new();
    let mut req = request();
    req.subject.pidns_inode += 1;
    assert_eq!(
        dispatch(&mut session, &req).refusal,
        Refusal::NamespaceMismatch
    );
    positive(&mut session);
    for field in [0, 1, 2] {
        let mut req = request();
        match field {
            0 => req.subject.pid += 1,
            1 => req.subject.starttime += 1,
            _ => req.subject.boot_id = Uuid::new_v4(),
        }
        assert_eq!(
            dispatch(&mut session, &req).refusal,
            Refusal::IdentityMismatch
        );
        positive(&mut session);
    }
    let mut req = request();
    req.grant = Some(Uuid::new_v4());
    assert_eq!(dispatch(&mut session, &req).refusal, Refusal::Disabled);
}
#[test]
fn bounded_replay_state_closes_without_eviction() {
    let mut session = Session::new();
    let req = request();
    assert_eq!(dispatch(&mut session, &req).refusal, Refusal::Disabled);
    assert_eq!(dispatch(&mut session, &req).refusal, Refusal::Replay);
    while session.nonces.len() < MAX_NONCES {
        positive(&mut session);
    }
    assert_eq!(
        dispatch(&mut session, &request()).refusal,
        Refusal::SessionFull
    );
    assert!(session.closed);
    assert_eq!(dispatch(&mut session, &req).refusal, Refusal::SessionFull);
    positive(&mut Session::new());
}
#[test]
fn real_kernel_attaches_credentials_to_every_packet() {
    if !kernel() {
        return;
    }
    let (receiver, sender) = pair().unwrap();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        send(sender.as_raw_fd(), bytes, until()).unwrap();
        let message = receive(receiver.as_raw_fd(), until()).unwrap();
        assert_eq!(message.bytes, bytes);
        assert_eq!(message.authenticate().unwrap(), own());
    }
}
#[test]
fn missing_credentials_oversize_and_timeout_refuse_then_recover() {
    if !kernel() {
        return;
    }
    for disabled in [
        &[76][..],
        &[libc::SO_PASSCRED][..],
        &[76, libc::SO_PASSCRED][..],
    ] {
        let (receiver, sender) = pair().unwrap();
        let zero: i32 = 0;
        for option in disabled {
            assert_eq!(
                unsafe {
                    libc::setsockopt(
                        receiver.as_raw_fd(),
                        libc::SOL_SOCKET,
                        *option,
                        (&zero as *const i32).cast(),
                        size_of::<i32>() as _,
                    )
                },
                0
            );
        }
        send(sender.as_raw_fd(), b"unattested", until()).unwrap();
        assert!(receive(receiver.as_raw_fd(), until()).is_err());
    }
    let (receiver, sender) = pair().unwrap();
    // Bypass the sender's outbound limit to exercise the receiver's MSG_TRUNC.
    let oversized = vec![b'x'; MAX_FRAME + 1];
    assert_eq!(
        unsafe {
            libc::send(
                sender.as_raw_fd(),
                oversized.as_ptr().cast(),
                oversized.len(),
                libc::MSG_NOSIGNAL,
            )
        },
        oversized.len() as isize
    );
    assert!(receive(receiver.as_raw_fd(), until()).is_err());
    let start = Instant::now();
    assert!(receive(receiver.as_raw_fd(), start + Duration::from_millis(20)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    send(sender.as_raw_fd(), b"valid", until()).unwrap();
    assert_eq!(
        receive(receiver.as_raw_fd(), until())
            .unwrap()
            .authenticate()
            .unwrap(),
        own()
    );
}

// Exact-test child uses a deliberately inherited fd, without ambient env input.
#[test]
#[ignore = "subprocess fixture, invoked by inherited_connection_authenticates_actual_sender"]
fn inherited_sender_child() {
    send(198, b"child", until()).unwrap();
    let reply = receive(198, until()).unwrap();
    assert_eq!(reply.bytes, b"ack");
}
#[test]
fn inherited_connection_authenticates_actual_sender_and_exit() {
    use std::os::unix::process::CommandExt;
    if !kernel() {
        return;
    }
    let (receiver, sender) = pair().unwrap();
    send(sender.as_raw_fd(), b"parent", until()).unwrap();
    assert_eq!(
        receive(receiver.as_raw_fd(), until())
            .unwrap()
            .authenticate()
            .unwrap(),
        own()
    );
    let source = sender.as_raw_fd();
    let mut command = std::process::Command::new(
        crate::aida_bin::current_executable_path().expect("resolve current test executable"),
    );
    command.args([
        "--ignored",
        "--exact",
        "proc_broker::tests::inherited_sender_child",
        "--nocapture",
    ]);
    // SAFETY: only async-signal-safe fd syscalls in the child hook.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(source, 198) < 0 || libc::fcntl(198, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let message = receive(receiver.as_raw_fd(), until()).unwrap();
    assert_eq!(message.bytes, b"child");
    let observed = message.authenticate().unwrap();
    assert_eq!(observed.pid, child.id());
    assert_ne!(observed.pid, std::process::id());
    send(receiver.as_raw_fd(), b"ack", until()).unwrap();
    let end = until();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child watchdog expired");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        message.authenticate().is_err(),
        "exited sender must never authenticate"
    );
    send(sender.as_raw_fd(), b"parent again", until()).unwrap();
    assert_eq!(
        receive(receiver.as_raw_fd(), until())
            .unwrap()
            .authenticate()
            .unwrap(),
        own()
    );
}
#[test]
fn exclusive_runtime_lock_and_disabled_end_to_end() {
    if !kernel() {
        return;
    }
    let dir = runtime();
    let service = Service::bind(dir.path(), "project-a".into(), "store-a".into()).unwrap();
    assert!(Service::bind(dir.path(), "project-a".into(), "store-a".into()).is_err());
    let client = connect(&dir.path().join("broker.sock")).unwrap();
    let req = request();
    send(
        client.as_raw_fd(),
        &serde_json::to_vec(&req).unwrap(),
        until(),
    )
    .unwrap();
    service.serve_one().unwrap();
    let message = receive(client.as_raw_fd(), until()).unwrap();
    assert_eq!(message.authenticate().unwrap(), own());
    let response: Response = serde_json::from_slice(&message.bytes).unwrap();
    assert_eq!(response.refusal, Refusal::Disabled);
    assert_eq!(response.nonce, Some(req.nonce));
    drop(service);
    assert!(!dir.path().join("broker.sock").exists());
    // Restart reacquires the retained lock file; no old session/handle survives.
    drop(Service::bind(dir.path(), "project-a".into(), "store-a".into()).unwrap());
}
#[test]
fn unsafe_runtime_and_replaced_socket_refuse() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    if !kernel() {
        return;
    }
    let dir = runtime();
    let link = dir.path().join("alias");
    symlink(dir.path(), &link).unwrap();
    assert!(Service::bind(&link, "p".into(), "s".into()).is_err());
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Service::bind(dir.path(), "p".into(), "s".into()).is_err());
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let service = Service::bind(dir.path(), "p".into(), "s".into()).unwrap();
    std::fs::remove_file(dir.path().join("broker.sock")).unwrap();
    let _replacement = UnixListener::bind(dir.path().join("broker.sock")).unwrap();
    assert!(service.serve_one().is_err());
    drop(service);
    assert!(
        dir.path().join("broker.sock").exists(),
        "drop must retain replaced endpoint"
    );
}

#[test]
fn replaced_runtime_lock_and_unsafe_lock_refuse() {
    use std::os::unix::fs::symlink;
    if !kernel() {
        return;
    }
    let dir = runtime();
    let victim = dir.path().join("victim");
    std::fs::write(&victim, b"retain").unwrap();
    symlink(&victim, dir.path().join("broker.lock")).unwrap();
    assert!(Service::bind(dir.path(), "p".into(), "s".into()).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"retain");
    std::fs::remove_file(dir.path().join("broker.lock")).unwrap();
    let service = Service::bind(dir.path(), "p".into(), "s".into()).unwrap();
    std::fs::remove_file(dir.path().join("broker.lock")).unwrap();
    std::fs::write(dir.path().join("broker.lock"), b"replacement").unwrap();
    assert!(service.serve_one().is_err());
    drop(service);
    std::fs::remove_file(dir.path().join("broker.lock")).unwrap();
    drop(Service::bind(dir.path(), "p".into(), "s".into()).unwrap());

    let parent = runtime();
    let path = parent.path().join("runtime");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let service = Service::bind(&path, "p".into(), "s".into()).unwrap();
    std::fs::rename(&path, parent.path().join("moved")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(service.serve_one().is_err());
    drop(service);
    drop(Service::bind(&path, "p".into(), "s".into()).unwrap());
}
