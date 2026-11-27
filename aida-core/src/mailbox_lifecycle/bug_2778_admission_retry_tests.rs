//! BUG-2778: a pre-start writer-admission refusal names the unverified
//! writer and retries only while every such writer runs a replaced binary.
//! trace:BUG-2778 | ai:claude

use super::activation::{admission_gap, BuildIdentity, ProcessView};
use super::guard::{retry_transient_admission_with, RetentionRefusal};
use super::retire::RefusalReason;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

const ME: (u64, u64) = (1, 10);

fn me() -> BuildIdentity {
    BuildIdentity {
        exe_id: ME,
        digest: Some("mine".into()),
        permit_marker: true,
    }
}

fn writer(pid: u32, exe: &str, id: (u64, u64)) -> ProcessView {
    ProcessView {
        pid,
        exe: Some(exe.into()),
        exe_id: Some(id),
        exe_digest: Some("0123456789abcdef".into()),
        permit_marker: Some(false),
        cmdline: vec![exe.into(), "mcp".into()],
    }
}

fn refused(transient: bool) -> Result<(), RetentionRefusal> {
    Err(RetentionRefusal {
        checkout: "/seat".into(),
        reason: "writer admission".into(),
        needs_activation: true,
        transient_admission: transient,
        retained_dir: None,
    })
}

fn transient(out: &Result<(), RetentionRefusal>) -> bool {
    matches!(out, Err(r) if r.transient_admission)
}

#[test]
fn a_deleted_binary_writer_is_named_and_transient() {
    let procs = [writer(4242, "/live/aida (deleted)", (1, 99))];
    let gap = admission_gap(&procs, Some(&me()), 1, None).expect("unverified");
    assert!(gap.transient, "{gap:?}");
    let named = gap.writers.join("; ");
    assert!(
        named.contains(
            "pid 4242 (aida, /live/aida (deleted), sha 0123456789ab, on a deleted binary)"
        ),
        "{named}"
    );
    let reason = RefusalReason::WriterAdmissionUnknown {
        writers: gap.writers,
        transient: gap.transient,
    }
    .to_string();
    assert!(
        reason.contains(
            "fresh writer-admission scan could not verify every mail writer (unverified: pid 4242"
        ),
        "{reason}"
    );
    assert!(reason.contains("should exit on their own"), "{reason}");
}

#[test]
fn a_live_unknown_writer_is_named_and_never_transient() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("aida");
    std::fs::write(&exe, b"other build").unwrap();
    let meta = std::fs::metadata(&exe).unwrap();
    let live = writer(77, exe.to_str().unwrap(), (meta.dev(), meta.ino()));
    let replaced = writer(78, "/gone/aida (deleted)", (1, 98));
    let gap = admission_gap(&[live, replaced], Some(&me()), 1, None).expect("unverified");
    assert!(!gap.transient, "{gap:?}");
    assert!(gap.writers[0].starts_with("pid 77 (aida, "), "{gap:?}");
    assert!(
        gap.writers[0].contains("live: it runs a different build"),
        "{gap:?}"
    );
    let reason = RefusalReason::WriterAdmissionUnknown {
        writers: gap.writers,
        transient: gap.transient,
    }
    .to_string();
    assert!(
        reason.contains("stop or restart the live writer"),
        "{reason}"
    );
    assert!(reason.contains("activate --dry-run"), "{reason}");
}

#[test]
fn a_stale_binary_writer_is_transient_and_an_incomplete_scan_is_not() {
    // The writer's path now holds another file (or none): stale.
    let stale = writer(5, "/nonexistent/bug2778/aida", (1, 97));
    let gap = admission_gap(std::slice::from_ref(&stale), Some(&me()), 1, None).unwrap();
    assert!(
        gap.transient && gap.writers[0].contains("stale binary"),
        "{gap:?}"
    );
    let gap = admission_gap(&[stale], Some(&me()), 1, Some("hidepid".into())).unwrap();
    assert!(!gap.transient, "{gap:?}");
    assert!(gap.writers.iter().any(|w| w.contains("hidepid")), "{gap:?}");
}

#[test]
fn transient_then_verified_on_retry_lets_the_launch_proceed() {
    let mut outcomes = vec![refused(true), Ok(())].into_iter();
    let mut slept = Vec::new();
    let out =
        retry_transient_admission_with(|| outcomes.next().unwrap(), transient, |d| slept.push(d));
    assert!(out.is_ok());
    assert_eq!(slept.len(), 1);
    assert!(
        slept[0] >= Duration::from_secs(80) && slept[0] <= Duration::from_secs(120),
        "{slept:?}"
    );
}

#[test]
fn persistently_transient_refuses_after_three_bounded_tries() {
    let mut tries = 0;
    let mut slept = Duration::ZERO;
    let out = retry_transient_admission_with(
        || {
            tries += 1;
            refused(true)
        },
        transient,
        |d| slept += d,
    );
    assert!(out.is_err());
    assert_eq!(tries, 3);
    assert!(
        slept >= Duration::from_secs(200) && slept <= Duration::from_secs(300),
        "{slept:?}"
    );
}

#[test]
fn a_live_unknown_writer_fails_closed_without_retry() {
    let mut tries = 0;
    let out = retry_transient_admission_with(
        || {
            tries += 1;
            refused(false)
        },
        transient,
        |_| panic!("never waits past a live unknown writer"),
    );
    assert!(out.is_err());
    assert_eq!(tries, 1);
}
