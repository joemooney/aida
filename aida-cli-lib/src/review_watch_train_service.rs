//! Train service using the same binding and supervision as review-watch.
// trace:STORY-2691 | ai:codex
use super::service::HostAuthority;
const TRAIN: bool = true;
include!("advisor_loop_service.rs");

// trace:BUG-2746 | ai:codex
#[test]
fn bug2746_train_member_selection_alert() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dir = Store::open(root, "github:github.com/owner/repo")
        .unwrap()
        .dir;
    let at = Utc::now().timestamp() - 120;
    let pid = std::process::id();
    let start = aida_core::proc_identity::ProcIdentity::capture(pid)
        .unwrap()
        .starttime;
    let _lock = dir
        .admission_lock_with(|file| {
            writeln!(file, "{pid} {start} {at} review-watch updating PR-4502")
        })
        .unwrap();
    // The busy reader names the holder only when its best-effort read of
    // /proc/locks shows exactly this PID on the inode; parallel tests taking
    // and dropping flocks can make one read miss it. Record the refusal that
    // carries the holder line, retrying a bounded number of times (each
    // attempt already waits out lock()'s retry window, so no sleep).
    // trace:TASK-3929 | ai:claude
    let holder = "review-watch updating PR-4502, age ";
    for _ in 0..3 {
        let error = (0..10)
            .map(|_| select_train_members(root, &[1], false).err().unwrap())
            .find(|error| format!("{error:#}").contains(holder))
            .expect("member selection never reported the holder's activity");
        train::record_pass_failure(root, error).unwrap();
    }
    let body = failure_state(root).unwrap().alert.unwrap().message.body;
    let (_, age) = body.split_once(holder).unwrap();
    let age: i64 = age.split_once('s').unwrap().0.parse().unwrap();
    assert!((120..=Utc::now().timestamp() - at).contains(&age));
}
