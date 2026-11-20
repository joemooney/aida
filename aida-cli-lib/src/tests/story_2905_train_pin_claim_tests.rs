//! STORY-2905: a PR a train has boarded cannot be claimed by another hand,
//! read through the real train journal reader the merge callers pass to
//! the gate's claim (mounted here, outside the gate, since the journal is
//! the command layer's).
// trace:STORY-2905 | ai:claude

use crate::gate::merge_claim::{claim, Hand};

/// A PR a train has boarded (its journal pin) cannot be claimed by any
/// other hand; the train itself may.
#[test]
fn a_train_pin_refuses_other_hands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let journal = root.join(".aida/train");
    std::fs::create_dir_all(&journal).unwrap();
    let entry = serde_json::json!({
        "version": 1, "branch": "train/example", "change_id": 8,
        "step": "Ci", "completed_members": [], "rejected": [], "call": "",
        "train": { "base": "base", "tip": "tip", "cars": [{
            "member": { "pr": 7, "head": "h", "subject": "s", "body": "" },
            "commit": "c", "patch_id": "p"
        }], "dropped": [] }
    });
    std::fs::write(journal.join("example.json"), entry.to_string()).unwrap();
    let why = claim(
        root,
        7,
        Some("h"),
        Hand::Ship,
        Some(&crate::pr_train::pr_head_pin_journals),
    )
    .unwrap_err();
    assert!(why.contains("boarded on train/example"), "{why}");
    // Round 2 finding 5: the refusal names the recovery, exactly.
    assert!(why.contains("`aida pr train --status`"), "{why}");
    assert!(
        why.contains("`aida pr train --abandon example.json`"),
        "{why}"
    );
    claim(root, 7, Some("h"), Hand::Train, None).expect("the train holds its own car");
}
