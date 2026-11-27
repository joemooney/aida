//! TASK-3922: salvage is idempotent per clone state (AC1), and both
//! pre-start retention call sites route only a transient writer-admission
//! refusal into the retry (AC3).
// trace:TASK-3922 | ai:claude

use super::identical_salvage;
use aida_core::mailbox_lifecycle::guard::RetentionRefusal;

fn refusal(transient: bool) -> RetentionRefusal {
    RetentionRefusal {
        checkout: "/seat".into(),
        reason: "writer admission".into(),
        needs_activation: true,
        transient_admission: transient,
        retained_dir: None,
    }
}

const BUNDLE: &[u8] = b"# v2 git bundle\n1111 refs/heads/feat\n\nPACK-a";

#[test]
fn the_newest_identical_salvage_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(identical_salvage(d, b"body", None), None, "no salvage yet");
    std::fs::write(d.join("2026-01-01T000000.000Z.patch"), b"body").unwrap();
    let newest = d.join("2026-01-02T000000.000Z.patch");
    std::fs::write(&newest, b"body").unwrap();
    assert_eq!(identical_salvage(d, b"body", None), Some(newest.clone()));
    // Another head or change: a new salvage.
    assert_eq!(identical_salvage(d, b"# head: 2222\nbody", None), None);
    // Unpushed commits now, a bundle beside the newest patch is absent.
    assert_eq!(identical_salvage(d, b"body", Some(BUNDLE)), None);
    std::fs::write(newest.with_extension("bundle"), BUNDLE).unwrap();
    assert_eq!(
        identical_salvage(d, b"body", Some(BUNDLE)),
        Some(newest.clone())
    );
    // A checkpoint ref moved, or the old bundle is damaged: a new salvage.
    // So does a bundle that is no longer needed.
    let moved = b"# v2 git bundle\n1111 refs/heads/feat\n3333 refs/tags/slice-1\n\nPACK-a";
    assert_eq!(identical_salvage(d, b"body", Some(moved)), None);
    assert_eq!(identical_salvage(d, b"body", Some(&BUNDLE[..20])), None);
    assert_eq!(identical_salvage(d, b"body", None), None);
}

#[test]
fn lane_reuse_retries_only_a_transient_refusal() {
    use crate::dispatch::reuse_refusal_is_transient as transient;
    assert!(transient(&Err(refusal(true))));
    assert!(!transient(&Err(refusal(false))));
    assert!(!transient(&Ok(None)));
}

#[test]
fn rework_reclaim_retries_only_a_clone_kept_for_a_transient_refusal() {
    use crate::queue_cmd::kept_for_transient_admission as transient;
    use crate::seat_clone::Destroyed;
    let kept = |retention| {
        Ok(Destroyed::Kept {
            reason: "kept".into(),
            salvage: None,
            retention,
        })
    };
    assert!(transient(&kept(Some(refusal(true)))));
    assert!(!transient(&kept(Some(refusal(false)))));
    assert!(!transient(&kept(None)));
    assert!(!transient(&Ok(Destroyed::Removed { salvage: None })));
    assert!(!transient(&Err(anyhow::anyhow!("teardown failed"))));
}
