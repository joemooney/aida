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
    for _ in 0..3 {
        let error = select_train_members(root, &[1], false).err().unwrap();
        train::record_pass_failure(root, error).unwrap();
    }
    let body = failure_state(root).unwrap().alert.unwrap().message.body;
    let (_, age) = body
        .split_once("review-watch updating PR-4502, age ")
        .unwrap();
    let age: i64 = age.split_once('s').unwrap().0.parse().unwrap();
    assert!((120..=Utc::now().timestamp() - at).contains(&age));
}
