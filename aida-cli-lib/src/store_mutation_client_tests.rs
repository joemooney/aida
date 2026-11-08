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
        Some("/tmp/socket"),
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
    for state in ["completed", "unknown"] {
        validate_result("request_status", &Value::Null, &json!({"state":state})).unwrap();
    }
    assert!(validate_result("request_status", &Value::Null, &json!({"state":"accepted"})).is_err());
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
