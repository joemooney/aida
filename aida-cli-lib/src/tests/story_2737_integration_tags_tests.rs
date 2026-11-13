//! STORY-2737 slice (b): the `landed-in-integration` tag plan.
// trace:STORY-2737 | ai:claude

use super::*;

fn spec(id: &str, status: &str, tags: &[&str]) -> ListedSpec {
    ListedSpec {
        display_id: id.into(),
        status: status.into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

#[test]
fn completing_ids_use_the_completion_trailer_parser() {
    let ids = completing_ids(&[
        "[AI:claude] fix(queue): rework resolves bound keys (BUG-2540)",
        "[AI:claude] fix(launch): refusal names its reason (Refs BUG-2544)",
        "[AI:claude] chore(integration): merge PR #3902 (BUG-2539)",
        "[AI:codex] fix(lane): overlay fingerprints (bug-2539)",
        "Merge PR #3890 into integration (STORY-2737)",
        "docs: no trailer here",
    ]);
    assert_eq!(ids, ["BUG-2540", "BUG-2539", "STORY-2737"]);
}

#[test]
fn plan_tags_open_landed_specs_and_untags_closed_or_unlanded_ones() {
    let landed = vec![
        "BUG-1".to_string(),
        "BUG-2".to_string(),
        "BUG-5".to_string(),
    ];
    let specs = [
        spec("BUG-1", "Approved", &[]),                   // open, landed: add
        spec("BUG-2", "InProgress", &[LANDED_TAG]),       // already tagged: keep
        spec("BUG-3", "Approved", &[LANDED_TAG, "perf"]), // not landed: remove
        spec("BUG-4", "Completed", &[LANDED_TAG]),        // closed: remove
        spec("BUG-5", "Completed", &[]),                  // closed: never add
        spec("BUG-6", "Draft", &[]),                      // untouched
    ];
    let (adds, removes) = plan_tag_changes(&specs, &landed);
    assert_eq!(adds, ["BUG-1"]);
    assert_eq!(removes, ["BUG-3", "BUG-4"]);
}

#[test]
fn plan_matches_ids_by_numeric_identity() {
    let (adds, _) = plan_tag_changes(&[spec("TASK-42", "Approved", &[])], &["TASK-0042".into()]);
    assert_eq!(adds, ["TASK-42"]);
}
