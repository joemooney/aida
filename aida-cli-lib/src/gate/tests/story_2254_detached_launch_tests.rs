//! STORY-2254: the gate-owned one-use detached launch capability. Binding a
//! detached worker's own child grant happens only here, only for this
//! process's own live child with its exact start identity, only once, and
//! only while the parent grant still authorizes the seat.
// trace:STORY-2254 | ai:claude

use super::*;
use crate::test_env::EnvVarsGuard;

const SUBJECT: &str = "story-2254-fixture-user";

fn pinned_env() -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID", "AIDA_HOME"]);
    env.set_key("AIDA_USER", SUBJECT);
    env.unset_key("AIDA_SESSION_ID");
    env
}

/// A sleeping child this test owns and always reaps.
struct OwnChild(std::process::Child);
impl OwnChild {
    fn spawn() -> Self {
        Self(
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        )
    }
    fn start(&self) -> String {
        aida_core::liveness::process_start_identity(self.0.id()).unwrap()
    }
}
impl Drop for OwnChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The capability as a supervisor holds it after `receive` (the parent-link
/// proof itself needs a real dispatcher → supervisor process pair).
fn received(cap: DetachedLaunchCapability) -> DetachedLaunchCapability {
    DetachedLaunchCapability {
        inner: cap.inner,
        supervisor: own_process_token(),
    }
}

/// A byte-for-byte copy of a capability, as a replayed journal would hold.
fn replay(cap: &DetachedLaunchCapability) -> DetachedLaunchCapability {
    DetachedLaunchCapability {
        inner: serde_json::from_str(&serde_json::to_string(&cap.inner).unwrap()).unwrap(),
        supervisor: cap.supervisor.clone(),
    }
}

fn issue(root: &Path) -> (SeatGrant, DetachedLaunchCapability) {
    issue_detached_launch_child(
        root,
        "implementer",
        "codex-bg-child",
        false,
        &Uuid::new_v4().to_string(),
        "implementer:TASK-1",
    )
    .unwrap()
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_bg_own_grant_binds_own_child_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &parent);

    let (grant, cap) = issue(root);
    assert_eq!(grant.parent_grant_id.as_deref(), Some(parent.as_str()));
    assert!(grant.delegable_seats.is_empty());
    assert!(
        grant.launch_capability.is_some(),
        "digest recorded at issuance"
    );
    let stored =
        std::fs::read_to_string(grant_dir().unwrap().join(format!("{}.json", grant.id))).unwrap();
    assert!(
        !stored.contains(&cap.inner.nonce),
        "the capability itself is never written"
    );

    let worker = OwnChild::spawn();
    let cap = received(cap);
    let copy = replay(&cap);
    accept_detached_launch_child(cap, root, worker.0.id(), &worker.start()).unwrap();
    let after = read_grant(&grant.id).unwrap();
    assert!(after.launch_capability.is_none());
    assert!(after
        .binding
        .contains(&pid_binding_token(worker.0.id()).unwrap()));

    let second = OwnChild::spawn();
    let err = accept_detached_launch_child(copy, root, second.0.id(), &second.start()).unwrap_err();
    assert!(format!("{err:#}").contains("already accepted"), "{err:#}");
    assert!(!read_grant(&grant.id)
        .unwrap()
        .binding
        .contains(&pid_binding_token(second.0.id()).unwrap()));
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_bg_bind_refuses_unreceived_foreign_pid_and_reused_start() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &parent);
    let worker = OwnChild::spawn();

    // Not received by a supervisor: never binds.
    let (_, cap) = issue(root);
    let err = accept_detached_launch_child(cap, root, worker.0.id(), &worker.start()).unwrap_err();
    assert!(format!("{err:#}").contains("not received"), "{err:#}");

    // A process that is not this supervisor's own child (pid 1).
    let (_, cap) = issue(root);
    let init_start = aida_core::liveness::process_start_identity(1).unwrap_or_default();
    let err = accept_detached_launch_child(received(cap), root, 1, &init_start).unwrap_err();
    assert!(format!("{err:#}").contains("own child"), "{err:#}");

    // Our own child, but a stale start identity (a reused pid).
    let (grant, cap) = issue(root);
    let err = accept_detached_launch_child(received(cap), root, worker.0.id(), "linux-starttime:1")
        .unwrap_err();
    assert!(format!("{err:#}").contains("start identity"), "{err:#}");
    assert!(
        read_grant(&grant.id).unwrap().launch_capability.is_some(),
        "a refusal consumes nothing"
    );

    // Another project's capability.
    let (_, cap) = issue(root);
    let other = tempfile::tempdir().unwrap();
    let err =
        accept_detached_launch_child(received(cap), other.path(), worker.0.id(), &worker.start())
            .unwrap_err();
    assert!(format!("{err:#}").contains("another project"), "{err:#}");
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_bg_parent_revocation_and_child_revocation_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &parent);
    let worker = OwnChild::spawn();

    let (child, cap) = issue(root);
    revoke_by_id(&child.id).unwrap();
    let err = accept_detached_launch_child(received(cap), root, worker.0.id(), &worker.start())
        .unwrap_err();
    assert!(format!("{err:#}").contains("revoked or expired"), "{err:#}");

    let (_, cap) = issue(root);
    revoke_by_id(&parent).unwrap();
    let err = accept_detached_launch_child(received(cap), root, worker.0.id(), &worker.start())
        .unwrap_err();
    assert!(format!("{err:#}").contains("parent grant"), "{err:#}");
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_bg_issuance_requires_delegation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let parent = test_support::mint_grant_for(root, SUBJECT, "advisor", &["implementer"]);
    env.set_key(GRANT_ENV, &parent);
    let err = issue_detached_launch_child(
        root,
        "reviewer",
        "x",
        false,
        &Uuid::new_v4().to_string(),
        "s",
    )
    .unwrap_err();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
    env.unset_key(GRANT_ENV);
    let err = issue_detached_launch_child(
        root,
        "implementer",
        "x",
        false,
        &Uuid::new_v4().to_string(),
        "s",
    )
    .unwrap_err();
    assert!(ChildGrantRefused::is_in(&err), "{err:#}");
}
