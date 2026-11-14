//! STORY-2745 review F1: a session's named seat reaches its registry row
//! whichever of the launcher and the SessionStart hook writes first, and for
//! a lease-linked session too, so `seat:<name>` mail resolves to it.
// trace:STORY-2745 | ai:claude

use super::*;
use tempfile::TempDir;

fn binary() -> AgentBinaryIdentity {
    AgentBinaryIdentity::new("0.16.0".into(), "abc1234".into())
}

fn env(seat: Option<&str>, scope: Option<&str>) -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_TYPE", Some("claude")),
        (crate::agent_rotation::SEAT_ENV, seat),
        ("AIDA_SESSION_SCOPE", scope),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_HANDLE", None),
    ])
}

fn launcher_row(root: &Path, seat: Option<&str>) -> SpawnedRow<'static> {
    SpawnedRow {
        role: Some("implementer".into()),
        current_spec: None,
        worktree_path: root.to_path_buf(),
        binary: None,
        name: Some("integ-7".into()),
        native_session_id: None,
        resumed_from: None,
        description: None,
        delegate_seats: Vec::new(),
        seat: seat.map(str::to_string),
    }
}

fn own_seat(root: &Path) -> Option<String> {
    let path = registry_path(root, &agent_id("claude", std::process::id()));
    toml::from_str::<AgentRegistryEntry>(&std::fs::read_to_string(path).unwrap())
        .unwrap()
        .seat
}

/// `seat:integration` resolves to this process's row.
fn assert_resolves(root: &Path) {
    let d =
        crate::mailbox_resolver::RecipientDirectory::load(root, &root.join(".aida-store")).unwrap();
    let r = d.resolve_send("seat:integration", false).unwrap();
    assert_eq!(r.address, "seat:integration");
    assert!(!r.cohort.expect("cohort").queued, "a live holder reads it");
}

#[test]
fn story2745_hook_then_launcher_keeps_the_seat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let _env = env(Some("integration"), None);
    let hook = touch_session_start_agent_for(root, root, Some("sid-a"), &binary(), pid).unwrap();
    assert_eq!(hook.seat.as_deref(), Some("integration"));
    // A launcher write that names no seat keeps the hook's.
    register_spawned_agent_with_row(root, "claude", pid, launcher_row(root, None)).unwrap();
    assert_eq!(own_seat(root).as_deref(), Some("integration"));
    assert_resolves(root);
}

#[test]
fn story2745_launcher_then_hook_keeps_the_seat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    // The hook's env carries no seat (a background spare's, say).
    let _env = env(None, None);
    register_spawned_agent_with_row(root, "claude", pid, launcher_row(root, Some("integration")))
        .unwrap();
    assert_eq!(own_seat(root).as_deref(), Some("integration"));
    let hook = touch_session_start_agent_for(root, root, Some("sid-b"), &binary(), pid).unwrap();
    assert_eq!(hook.seat.as_deref(), Some("integration"));
    assert_resolves(root);
}

/// A lease and its manifest linking session `sid`, as a `--spec` or `--bg`
/// launch writes them; `seat` is the manifest's recorded seat.
fn linked_lease(root: &Path, scope: &str, sid: &str, seat: Option<&str>) {
    let dir = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lease2745.toml"),
        format!(
            "id = \"lease2745\"\nscope = \"{scope}\"\nslug = \"s\"\nowner = \"t\"\nworktree_path = '{}'\nbranch = \"b\"\nstarted_at = \"2026-11-13T00:00:00Z\"\nhostname = \"h\"\nrole = \"implementer\"\n",
            root.display()
        ),
    )
    .unwrap();
    let seat = seat
        .map(|s| format!("seat = \"{s}\"\n"))
        .unwrap_or_default();
    std::fs::write(
        dir.join("lease2745.manifest.toml"),
        format!(
            "session_id = \"lease2745\"\nplanned_at = \"2026-11-13T00:00:00Z\"\nplan_source = \"agent new\"\nclaude_session_id = \"{sid}\"\n{seat}items = []\n"
        ),
    )
    .unwrap();
}

/// No live session holds `seat:<seat>`.
fn assert_unheld(root: &Path, seat: &str) {
    let d =
        crate::mailbox_resolver::RecipientDirectory::load(root, &root.join(".aida-store")).unwrap();
    assert!(
        d.resolve_send(&format!("seat:{seat}"), false).is_err(),
        "seat:{seat} must have no holder"
    );
}

/// The link is the launch's record: a launch that named no seat holds none,
/// even when the env's scope is the lease's (a spare reused for the same
/// spec still carries the earlier launch's seat).
#[test]
fn story2745_linked_launch_without_a_seat_ignores_the_env_seat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    linked_lease(root, "TASK-2745", "c0ffee01", None);
    let _env = env(Some("old-seat"), Some("TASK-2745"));
    let entry = touch_session_start_agent_for(
        root,
        root,
        Some("c0ffee01-0000-4000-8000-000000000000"),
        &binary(),
        std::process::id(),
    )
    .unwrap();
    assert_eq!(entry.current_spec.as_deref(), Some("TASK-2745"));
    assert_eq!(entry.seat, None);
    assert_unheld(root, "old-seat");
}

/// Review round 2, finding 1: a recycled `--bg` spare whose hook fired
/// before the link wrote the earlier launch's seat. The seatless launch's
/// adopt step clears it, and a hook firing after the link keeps it clear.
#[test]
fn story2745_recycled_spare_drops_the_earlier_launchs_seat_on_a_seatless_link() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let sid = "c0ffee04-0000-4000-8000-000000000000";
    // The spare's env is an earlier launch's: its scope and its seat.
    let _env = env(Some("old-seat"), Some("TASK-1111"));
    let pre = touch_session_start_agent_for(root, root, Some(sid), &binary(), pid).unwrap();
    assert_eq!(pre.seat.as_deref(), Some("old-seat"), "the stale row");
    // The launcher links the session to a launch with no `--seat`.
    linked_lease(root, "TASK-2747", "c0ffee04", None);
    adopt_bg_session_identity(
        root,
        "c0ffee04",
        "impl-4",
        Some("TASK-2747"),
        Some("implementer"),
        None,
        root,
    );
    assert_eq!(own_seat(root), None);
    let post = touch_session_start_agent_for(root, root, Some(sid), &binary(), pid).unwrap();
    assert_eq!(post.seat, None);
    assert_eq!(own_seat(root), None);
    assert_unheld(root, "old-seat");
}

/// The same stale row, when the post-link hook runs before the adopt step:
/// the hook alone drops the seat.
#[test]
fn story2745_post_link_hook_alone_drops_a_stale_row_seat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let sid = "c0ffee05-0000-4000-8000-000000000000";
    let _env = env(Some("old-seat"), Some("TASK-1111"));
    touch_session_start_agent_for(root, root, Some(sid), &binary(), pid).unwrap();
    assert_eq!(own_seat(root).as_deref(), Some("old-seat"));
    linked_lease(root, "TASK-2748", "c0ffee05", None);
    let post = touch_session_start_agent_for(root, root, Some(sid), &binary(), pid).unwrap();
    assert_eq!(post.seat, None);
    assert_unheld(root, "old-seat");
}

#[test]
fn story2745_lease_linked_bg_session_takes_the_manifest_seat_not_a_stale_env() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    linked_lease(root, "TASK-2746", "c0ffee02", Some("integration"));
    // A recycled spare: its env names an earlier launch's scope and seat.
    let _env = env(Some("stale-seat"), Some("TASK-1111"));
    let entry = touch_session_start_agent_for(
        root,
        root,
        Some("c0ffee02-0000-4000-8000-000000000000"),
        &binary(),
        std::process::id(),
    )
    .unwrap();
    assert_eq!(entry.seat.as_deref(), Some("integration"));
    assert_resolves(root);
}

#[test]
fn story2745_bg_link_adopts_the_seat_onto_an_earlier_hook_row() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let _env = env(None, None);
    touch_session_start_agent_for(
        root,
        root,
        Some("c0ffee03-0000-4000-8000-000000000000"),
        &binary(),
        std::process::id(),
    )
    .unwrap();
    assert_eq!(own_seat(root), None);
    adopt_bg_session_identity(
        root,
        "c0ffee03",
        "integ-9",
        None,
        Some("implementer"),
        Some("integration"),
        root,
    );
    assert_eq!(own_seat(root).as_deref(), Some("integration"));
    assert_resolves(root);
}
