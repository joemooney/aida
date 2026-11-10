//! Protocol negatives exercise real SEQPACKET exchanges without ambient inputs.
// trace:TASK-3223 | ai:codex
use super::*;
use std::os::unix::net::UnixDatagram;

fn inputs(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_pairs(pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())))
}

#[test]
fn mediated_selection_never_falls_back_when_endpoint_is_missing_or_invalid() {
    assert!(Client::from_environment(&inputs(&[])).unwrap().is_none());
    assert!(
        Client::from_environment(&inputs(&[("AIDA_STORE_MUTATION_ENDPOINT", "/bad")]))
            .unwrap()
            .is_none()
    );
    for endpoint in [
        None,
        Some("/invalid-service-endpoint"),
        Some("/run/aida-mutation/../service"),
        Some("/run/aida-mutation/00000000000000000000000000000000/service"),
    ] {
        let mut pairs = vec![("AIDA_STORE_MUTATION_PROFILE", "mediated")];
        if let Some(endpoint) = endpoint {
            pairs.push(("AIDA_STORE_MUTATION_ENDPOINT", endpoint));
        }
        assert!(Client::from_environment(&inputs(&pairs)).is_err());
    }
    let endpoint = format!("/run/aida-mutation/{}/service", Uuid::new_v4());
    assert!(Client::from_environment(&inputs(&[
        ("AIDA_STORE_MUTATION_PROFILE", "mediated"),
        ("AIDA_STORE_MUTATION_ENDPOINT", &endpoint)
    ]))
    .unwrap()
    .is_some());
}

fn exchange_with(reply: impl FnOnce(Value) -> Vec<u8> + Send + 'static) -> Result<Value> {
    let mut sockets = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        },
        0
    );
    let client = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
    let server = unsafe { UnixDatagram::from_raw_fd(sockets[1]) };
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let epoch = Uuid::new_v4();
    let id = Uuid::new_v4();
    let raw = serde_json::to_vec(&json!({"launch_epoch":epoch,"request_id":id})).unwrap();
    let peer = std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        let n = server.recv(&mut bytes).unwrap();
        server
            .send(&reply(serde_json::from_slice(&bytes[..n]).unwrap()))
            .unwrap();
    });
    let result = exchange(&client, &raw, epoch, id, Duration::from_secs(2));
    peer.join().unwrap();
    result
}

fn success(request: Value) -> Value {
    json!({"version":1,"launch_epoch":request["launch_epoch"],"request_id":request["request_id"],"outcome":"success","result":{"state":"completed"}})
}

#[test]
fn seqpacket_success_and_known_refusals_are_correlated() {
    assert_eq!(
        exchange_with(|r| serde_json::to_vec(&success(r)).unwrap()).unwrap(),
        json!({"state":"completed"})
    );
    let error = exchange_with(|r| serde_json::to_vec(&json!({"version":1,"launch_epoch":r["launch_epoch"],"request_id":r["request_id"],"outcome":"error","error":"Revoked"})).unwrap()).unwrap_err();
    assert_eq!(error.to_string(), "Revoked");
}

#[test]
fn preparse_refusals_are_nullable_but_mutation_outcomes_require_correlation() {
    for error in [
        "InvalidRequest",
        "UnsupportedVersion",
        "UnsupportedOperation",
        "Conflict",
        "Busy",
        "Revoked",
    ] {
        let expected = if matches!(
            error,
            "InvalidRequest" | "UnsupportedVersion" | "UnsupportedOperation"
        ) {
            error
        } else {
            "OutcomeUnknown"
        };
        let result = exchange_with(move |r| {
            serde_json::to_vec(&json!({
                "version":1,"launch_epoch":r["launch_epoch"],"request_id":null,
                "outcome":"error","error":error
            }))
            .unwrap()
        })
        .unwrap_err();
        assert_eq!(result.to_string(), expected);
    }
    for field in ["request_id", "launch_epoch"] {
        let result = exchange_with(move |r| {
            let mut reply = json!({"version":1,"launch_epoch":r["launch_epoch"],"request_id":null,
                "outcome":"error","error":"InvalidRequest"});
            reply.as_object_mut().unwrap().remove(field);
            serde_json::to_vec(&reply).unwrap()
        })
        .unwrap_err();
        assert_eq!(result.to_string(), "OutcomeUnknown");
    }
}

#[test]
fn direct_origin_preimage_ignores_unrelated_malformed_objects_but_not_unsafe_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let mut row = aida_core::Requirement::new("origin".into(), "description".into());
    let origin = "TASK-b6b1278890774ef993d2ebc327862470-880";
    row.spec_id = Some(origin.into());
    aida_core::object_store::write_object(&tmp.path().join("objects"), &row).unwrap();
    let bad = aida_core::object_store::object_path(&tmp.path().join("objects"), "TASK-2").unwrap();
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(bad, "invalid: [yaml").unwrap();
    let (sid, bytes) = canonical_preimage(tmp.path(), &origin.to_ascii_uppercase()).unwrap();
    assert_eq!(sid, origin);
    assert_eq!(
        serde_yaml::from_slice::<aida_core::Requirement>(&bytes)
            .unwrap()
            .id,
        row.id
    );
    let target = aida_core::object_store::object_path(&tmp.path().join("objects"), origin).unwrap();
    let moved = tmp.path().join("moved");
    std::fs::rename(&target, &moved).unwrap();
    std::os::unix::fs::symlink(moved, target).unwrap();
    assert!(canonical_preimage(tmp.path(), origin).is_err());
}

#[test]
fn malformed_mismatched_and_oversized_replies_have_unknown_outcomes() {
    for field in [
        "version",
        "launch_epoch",
        "request_id",
        "extra",
        "outcome",
        "result",
    ] {
        let error = exchange_with(move |r| {
            let mut response = success(r);
            response[field] = match field {
                "version" => json!(2),
                "launch_epoch" | "request_id" => json!(Uuid::new_v4()),
                "outcome" => json!("accepted"),
                "result" => {
                    response.as_object_mut().unwrap().remove("result");
                    return serde_json::to_vec(&response).unwrap();
                }
                _ => json!(true),
            };
            serde_json::to_vec(&response).unwrap()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "OutcomeUnknown", "{field}");
    }
    for raw in [b"{}".to_vec(), vec![b' '; LIMIT + 1], b"not-json".to_vec()] {
        assert_eq!(
            exchange_with(move |_| raw).unwrap_err().to_string(),
            "OutcomeUnknown"
        );
    }
}

#[test]
fn stalled_peer_is_bounded_and_is_not_retried() {
    let mut sockets = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        },
        0
    );
    let client = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
    let server = unsafe { UnixDatagram::from_raw_fd(sockets[1]) };
    let start = Instant::now();
    assert!(exchange(
        &client,
        b"request",
        Uuid::new_v4(),
        Uuid::new_v4(),
        Duration::from_millis(30)
    )
    .is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    let mut bytes = [0; 64];
    assert_eq!(server.recv(&mut bytes).unwrap(), 7);
    server.set_nonblocking(true).unwrap();
    assert_eq!(
        server.recv(&mut bytes).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn unsupported_mcp_fields_refuse_without_store_or_service_access() {
    let client = Client::new(&format!("/run/aida-mutation/{}/service", Uuid::new_v4())).unwrap();
    let missing = std::path::Path::new("/nonexistent-canonical-store");
    for (name, args) in [
        (
            "add_comment",
            json!({"id":"TASK-1","text":"hello","relayed_from":"advisor"}),
        ),
        (
            "update_requirement",
            json!({"id":"TASK-1","title":"safe","status":"Completed"}),
        ),
        ("claim_task", json!({"id":"TASK-1"})),
        ("queue_done", json!({"id":"TASK-1"})),
    ] {
        assert_eq!(
            client.mcp(missing, name, &args).unwrap_err().to_string(),
            "UnsupportedOperation"
        );
    }
}

#[test]
fn success_requires_the_typed_result_for_the_requested_target() {
    let data = json!({"spec_id":"TASK-1"});
    let result = json!({"spec_id":"TASK-1","revision":"a".repeat(64),"comment_id":Uuid::new_v4()});
    validate_result("comment_add", &data, &result).unwrap();
    for (field, value) in [
        ("spec_id", json!("TASK-2")),
        ("revision", json!("bad")),
        ("comment_id", Value::Null),
        ("extra", json!(true)),
    ] {
        let mut invalid = result.clone();
        invalid[field] = value;
        assert!(validate_result("comment_add", &data, &invalid).is_err());
    }
    let mut edit = result;
    edit["comment_id"] = Value::Null;
    validate_result("spec_edit", &data, &edit).unwrap();
}

#[test]
fn ancillary_rights_are_closed_and_never_accepted_as_a_reply() {
    let mut sockets = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        },
        0
    );
    let client = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
    let server = unsafe { OwnedFd::from_raw_fd(sockets[1]) };
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let reader = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let epoch = Uuid::new_v4();
    let id = Uuid::new_v4();
    let peer = std::thread::spawn(move || {
        let mut request = [0u8; 64];
        assert!(
            unsafe {
                libc::read(
                    server.as_raw_fd(),
                    request.as_mut_ptr().cast(),
                    request.len(),
                )
            } > 0
        );
        let raw =
            serde_json::to_vec(&success(json!({"launch_epoch":epoch,"request_id":id}))).unwrap();
        let mut iov = libc::iovec {
            iov_base: raw.as_ptr() as *mut _,
            iov_len: raw.len(),
        };
        let mut control = [0usize; 8];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) } as usize;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&message);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as usize;
            std::ptr::write_unaligned(
                libc::CMSG_DATA(cmsg).cast::<libc::c_int>(),
                writer.as_raw_fd(),
            );
            assert_eq!(
                libc::sendmsg(server.as_raw_fd(), &message, 0),
                raw.len() as isize
            );
        }
    });
    let error = exchange(&client, b"request", epoch, id, Duration::from_secs(2)).unwrap_err();
    assert_eq!(error.to_string(), "OutcomeUnknown");
    peer.join().unwrap();
    // The sender's writer and all received copies must be closed: EOF, rather
    // than EAGAIN, proves that no leaked SCM_RIGHTS writer remains alive.
    let mut byte = [0u8];
    assert_eq!(
        unsafe { libc::read(reader.as_raw_fd(), byte.as_mut_ptr().cast(), 1) },
        0
    );
}

/// The only synthetic part is the host reply; each route reads a real canonical
/// preimage and sends its complete production request over a SEQPACKET listener.
#[test]
fn canonical_preimages_route_agreed_and_origin_ids_over_seqpacket() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let mut row = aida_core::Requirement::new("before".into(), "description".into());
    let origin = "TASK-b6b1278890774ef993d2ebc327862470-880";
    row.spec_id = Some(origin.into());
    row.agreed_id = Some("TASK-3223".into());
    aida_core::object_store::write_object(&store.join("objects"), &row).unwrap();
    let bytes = aida_core::db::redo::object_bytes(&store, origin).unwrap();
    let revision = aida_core::db::redo::hash(&bytes);
    let path = tmp.path().join("service");
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    assert!(fd >= 0);
    let listener = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (out, byte) in addr.sun_path.iter_mut().zip(path.as_os_str().as_bytes()) {
        *out = *byte as libc::c_char;
    }
    assert_eq!(
        unsafe {
            libc::bind(
                listener.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                std::mem::size_of_val(&addr) as libc::socklen_t,
            )
        },
        0
    );
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 8) }, 0);
    let epoch = Uuid::new_v4();
    let expected_revision = revision.clone();
    let peer = std::thread::spawn(move || {
        let mut ids = std::collections::HashSet::new();
        for index in 0..8 {
            ready(
                &listener,
                libc::POLLIN,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            let fd = unsafe {
                libc::accept4(
                    listener.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC,
                )
            };
            assert!(fd >= 0);
            let channel = unsafe { UnixDatagram::from_raw_fd(fd) };
            channel
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut raw = [0; LIMIT];
            let n = channel.recv(&mut raw).unwrap();
            let request: Value = serde_json::from_slice(&raw[..n]).unwrap();
            let id = Uuid::parse_str(request["request_id"].as_str().unwrap()).unwrap();
            assert!(ids.insert(id), "each invocation has its own request ID");
            let (op, data) = if index % 4 == 0 || index % 4 == 2 {
                (
                    "comment_add",
                    json!({"spec_id":origin,"expected_revision":expected_revision,"content":"literal `text` $(text)\nsecond"}),
                )
            } else {
                (
                    "spec_edit",
                    json!({"spec_id":origin,"expected_revision":expected_revision,"fields":{"title":"new title","description":"","priority":"low"}}),
                )
            };
            assert_eq!(
                request,
                json!({"version":1,"launch_epoch":epoch,"request_id":id,"op":op,"data":data})
            );
            let result = json!({"spec_id":origin,"revision":"b".repeat(64),"comment_id": if op == "comment_add" { json!(Uuid::new_v4()) } else { Value::Null }});
            channel.send(&serde_json::to_vec(&json!({"version":1,"launch_epoch":epoch,"request_id":id,"outcome":"success","result":result})).unwrap()).unwrap();
        }
    });
    let client = Client {
        endpoint: path.to_str().unwrap().into(),
        epoch,
    };
    for id in ["task-3223", origin] {
        assert_eq!(
            client
                .comment(&store, id, "literal `text` $(text)\nsecond")
                .unwrap()["spec_id"],
            origin
        );
        let fields = || aida_core::db::canonical_mutation::EditFields {
            weight: None,
            priority: Some("low".into()),
            title: Some("new title".into()),
            description: Some("".into()),
        };
        client.edit(&store, id, fields()).unwrap();
        client
            .mcp(
                &store,
                "add_comment",
                &json!({"id":id,"text":"literal `text` $(text)\nsecond"}),
            )
            .unwrap();
        client
            .mcp(
                &store,
                "update_requirement",
                &json!({"id":id,"title":"new title","description":"","priority":"low"}),
            )
            .unwrap();
    }
    peer.join().unwrap();
    assert_eq!(
        aida_core::db::redo::object_bytes(&store, origin).unwrap(),
        bytes,
        "client never writes its preimage"
    );
    assert!(!store.join("metadata.yaml").exists(), "no backend setup");
    assert!(!store.join(".git").exists(), "no Git setup");
}

#[test]
fn canonical_resolution_refuses_ambiguous_corrupt_and_linked_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let mut row = aida_core::Requirement::new("before".into(), "description".into());
    row.spec_id = Some("TASK-1-1".into());
    row.agreed_id = Some("TASK-3223".into());
    aida_core::object_store::write_object(&tmp.path().join("objects"), &row).unwrap();
    assert_eq!(
        canonical_preimage(tmp.path(), "TASK-3223").unwrap().0,
        "TASK-1-1"
    );
    row.spec_id = Some("TASK-2-1".into());
    aida_core::object_store::write_object(&tmp.path().join("objects"), &row).unwrap();
    assert!(canonical_preimage(tmp.path(), "TASK-3223")
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    let path =
        aida_core::object_store::object_path(&tmp.path().join("objects"), "TASK-2-1").unwrap();
    std::fs::write(&path, "malformed: [").unwrap();
    assert!(canonical_preimage(tmp.path(), "TASK-3223").is_err());
    std::fs::remove_file(&path).unwrap();
    let target =
        aida_core::object_store::object_path(&tmp.path().join("objects"), "TASK-1-1").unwrap();
    std::os::unix::fs::symlink(target, &path).unwrap();
    assert!(canonical_preimage(tmp.path(), "TASK-3223").is_err());
}

#[test]
fn invalid_session_role_is_not_treated_as_an_unrestricted_missing_role() {
    use std::os::unix::ffi::OsStringExt;
    assert_eq!(session_role(&Environment::default()).unwrap(), None);
    assert_eq!(
        session_role(&inputs(&[("AIDA_SESSION_ROLE", " guest ")])).unwrap(),
        Some("guest".into())
    );
    let env = Environment::from_pairs([(
        "AIDA_SESSION_ROLE".into(),
        std::ffi::OsString::from_vec(vec![0xff]),
    )]);
    assert!(session_role(&env).is_err());
}

#[test]
fn connect_refusal_keeps_request_identity_without_uncertain_outcome_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    let client = Client::fixture(
        tmp.path().join("absent").to_str().unwrap().into(),
        Uuid::new_v4(),
    );
    let id = Uuid::new_v4();
    let error = client.request(id, "comment_add", json!({})).unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains(&id.to_string()));
    assert!(diagnostic.ends_with("Unavailable"));
    assert!(!diagnostic.contains("do not repeat"));
}
