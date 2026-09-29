use super::*;

// trace:TASK-1542 | ai:codex
fn write_child_lease(
    root: &Path,
    id: &str,
    creator: u32,
    pid: u32,
    start: Option<&str>,
    interrupted: bool,
) {
    let mut body = format!("id = \"{id}\"\ncreator_pid = {creator}\nactive_pid = {pid}\n");
    if let Some(start) = start {
        body.push_str(&format!(
            "active_pid_start_time = {}\n",
            aida_core::toml_quote::toml_string(start)
        ));
    }
    if interrupted {
        body.push_str("interrupted_at = \"2026-09-28T00:00:00Z\"\n");
    }
    std::fs::write(root.join(".aida/sessions").join(format!("{id}.toml")), body).unwrap();
}

#[test]
// trace:TASK-1542 | ai:codex
fn task_1542_collects_only_matching_live_owned_children_including_interrupted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let pid = std::process::id();
    let mut live = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let child_pid = live.id();
    let start = aida_core::liveness::process_start_identity(child_pid).expect("child identity");
    write_child_lease(root, "a-good", pid, child_pid, Some(&start), false);
    write_child_lease(
        root,
        "b-other-owner",
        pid.wrapping_add(10),
        child_pid,
        Some(&start),
        false,
    );
    write_child_lease(root, "c-no-start", pid, child_pid, None, false);
    write_child_lease(
        root,
        "d-wrong-start",
        pid,
        child_pid,
        Some("wrong-identity"),
        false,
    );
    let found = in_flight_vendor_children(root, pid);
    assert_eq!(
        found
            .iter()
            .map(|c| c.lease_id.as_str())
            .collect::<Vec<_>>(),
        vec!["a-good"]
    );
    live.kill().unwrap();
    let _ = live.wait();
}

#[test]
// trace:TASK-1542 | ai:codex
fn task_1542_collects_lease_already_stamped_interrupted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let pid = std::process::id();
    let mut live = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let child_pid = live.id();
    let start = aida_core::liveness::process_start_identity(child_pid).unwrap();
    write_child_lease(
        root,
        "already-interrupted",
        pid,
        child_pid,
        Some(&start),
        true,
    );
    assert_eq!(
        in_flight_vendor_children(root, pid)
            .iter()
            .map(|c| c.lease_id.as_str())
            .collect::<Vec<_>>(),
        vec!["already-interrupted"]
    );
    live.kill().unwrap();
    let _ = live.wait();
}

#[test]
// trace:TASK-1542 | ai:codex
fn task_1542_excludes_drain_pid_zero_and_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
    let active_live_pid = std::process::id();
    let drain_pid = active_live_pid.saturating_add(100_000);
    let start = aida_core::liveness::process_start_identity(active_live_pid).unwrap();
    write_child_lease(
        root,
        "valid-control",
        drain_pid,
        active_live_pid,
        Some(&start),
        false,
    );
    for active in [drain_pid, 0, 1] {
        write_child_lease(
            root,
            &format!("lease-{active}"),
            drain_pid,
            active,
            Some("x"),
            false,
        );
    }
    assert_eq!(
        in_flight_vendor_children(root, drain_pid)
            .iter()
            .map(|c| c.lease_id.as_str())
            .collect::<Vec<_>>(),
        vec!["valid-control"]
    );
}

/// The send-side re-check must be IDENTITY-aware, not a bare pid-liveness
/// check. Between the collector reading a lease and the kill landing, the
/// child can exit and its pid be recycled onto an unrelated process of the
/// operator's — and `process_identity_is_alive(pid, None)` would happily call
/// that recycled pid "alive" and SIGTERM it. A `VendorChild` is built here
/// DIRECTLY, bypassing the collector, because the collector's own filter would
/// otherwise hide whether the sender re-checks at all.
// trace:TASK-1542 | ai:claude
#[cfg(unix)]
#[test]
fn task_1542_forwarding_refuses_a_child_whose_identity_no_longer_matches() {
    let mut survivor = std::process::Command::new("sleep")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = survivor.id();
    let real_start = aida_core::liveness::process_start_identity(pid).expect("child identity");

    // The pid IS live; only the recorded identity disagrees — exactly the
    // shape a recycled pid presents.
    let stale_identity = VendorChild {
        lease_id: "recycled".to_string(),
        pid,
        start_time: format!("{real_start}-not-this-process"),
    };
    assert!(
        forward_term_to_children(&[stale_identity]).is_empty(),
        "an identity mismatch must never be signalled"
    );
    assert!(
        survivor.try_wait().unwrap().is_none(),
        "the process must be untouched by the refused forward"
    );

    // Positive control in the same test, so a forwarder that signals NOTHING
    // cannot pass the assertion above by doing nothing at all.
    let matching = VendorChild {
        lease_id: "genuine".to_string(),
        pid,
        start_time: real_start,
    };
    assert_eq!(forward_term_to_children(&[matching]), vec![pid]);
    let status = survivor.wait().unwrap();
    use std::os::unix::process::ExitStatusExt as _;
    assert_eq!(
        status.signal(),
        Some(15),
        "the matching child must be terminated by the forwarded SIGTERM"
    );
}
