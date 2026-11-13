//! STORY-2737 slice (b): the `landed-in-integration` tag plan.
// trace:STORY-2737 | ai:claude

use super::*;

use aida_core::{Requirement, RequirementStatus as S, RequirementsStore};
use std::collections::HashSet;

fn stored(spec_id: &str, agreed: Option<&str>, status: S, tags: &[&str]) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed.map(String::from);
    r.status = status;
    r.tags = tags.iter().map(|t| t.to_string()).collect();
    r
}

fn store_of(reqs: Vec<Requirement>) -> RequirementsStore {
    let mut store = RequirementsStore::default();
    store.requirements = reqs;
    store
}

fn ids(specs: &[&ListedSpec]) -> Vec<String> {
    specs.iter().map(|s| s.display_id.clone()).collect()
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
    let store = store_of(vec![
        stored("BUG-1", None, S::Approved, &[]), // open, landed: add
        stored("BUG-2", None, S::InProgress, &[LANDED_TAG]), // tagged: keep
        stored("BUG-3", None, S::Approved, &[LANDED_TAG, "perf"]), // not landed: remove
        stored("BUG-4", None, S::Completed, &[LANDED_TAG]), // closed: remove
        stored("BUG-5", None, S::Completed, &[]), // closed: never add
        stored("BUG-6", None, S::Draft, &[]),    // untouched
        // Review round 2: Done is implemented work awaiting promotion.
        stored("BUG-7", None, S::Done, &[]), // done, landed: add
        stored("BUG-8", None, S::Done, &[LANDED_TAG]), // done, landed: keep
        stored("BUG-10", None, S::Done, &[LANDED_TAG]), // done, not landed: remove
    ]);
    let landed: HashSet<uuid::Uuid> = ["BUG-1", "BUG-2", "BUG-5", "BUG-7", "BUG-8"]
        .iter()
        .filter_map(|t| resolve_trailer(&store, t))
        .collect();
    assert_eq!(landed.len(), 5);
    let specs: Vec<ListedSpec> = store.requirements.iter().map(ListedSpec::of).collect();
    let (adds, removes) = plan_tag_changes(&specs, &landed);
    assert_eq!(ids(&adds), ["BUG-1", "BUG-7"]);
    assert_eq!(ids(&removes), ["BUG-10", "BUG-3", "BUG-4"]);
    // Edits address the stored spec by its stable UUID.
    assert_eq!(adds[0].uuid, store.requirements[0].id);
}

// Review round 1: a trailer naming a spec by any identity the store knows
// (origin id after agreed-id assignment, padding, a node alias) resolves to
// that one stored spec; padding-equal ids naming two specs resolve to none.
// trace:STORY-2737 | ai:claude
#[test]
fn trailers_resolve_to_stored_spec_identity() {
    let store = store_of(vec![
        stored("TASK-HOME-42", Some("TASK-77"), S::Approved, &[]),
        stored("TASK-ZZ-178", None, S::Approved, &[]),
        stored("BUG-9", None, S::Approved, &[]),
        stored("BUG-09", None, S::Approved, &[]),
    ]);
    let id = |n: usize| Some(store.requirements[n].id);
    // Agreed display id and origin (native) id are the same spec.
    assert_eq!(resolve_trailer(&store, "TASK-77"), id(0));
    assert_eq!(resolve_trailer(&store, "TASK-HOME-42"), id(0));
    assert_eq!(resolve_trailer(&store, "task-0077"), id(0));
    // A node alias resolves through the store's literal-first order.
    let mut retired = aida_core::node_alias::RetiredIds::default();
    assert!(retired.add("TASK-1-418", "TASK-ZZ-178", "u"));
    let _guard = aida_core::node_alias::install_for_thread(aida_core::node_alias::NodeAliasMap {
        retired_ids: retired.index(),
        ..Default::default()
    });
    assert_eq!(resolve_trailer(&store, "TASK-1-418"), id(1));
    // Literal ids win; an unknown token names nothing.
    assert_eq!(resolve_trailer(&store, "BUG-9"), id(2));
    assert_eq!(resolve_trailer(&store, "BUG-404"), None);
    // A padding-only match that names two stored specs tags neither.
    assert_eq!(resolve_trailer(&store, "BUG-009"), None);
}

// Review round 2: padding-equal literal trailers that name two stored specs
// each resolve to their own spec, from collector to resolver, whichever
// order the store holds them in; a padded token matching both names neither.
// trace:STORY-2737 | ai:claude
#[test]
fn distinct_padding_equal_literals_each_land_their_own_spec() {
    for flip in [false, true] {
        let mut reqs = vec![
            stored("BUG-9", None, S::Approved, &[]),
            stored("BUG-09", None, S::Approved, &[]),
        ];
        if flip {
            reqs.reverse();
        }
        let store = store_of(reqs);
        let uuid_of = |id: &str| {
            store
                .requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some(id))
                .map(|r| r.id)
        };
        let tokens = completing_ids(&["fix: a (BUG-9)", "fix: b (BUG-09)"]);
        assert_eq!(tokens, ["BUG-9", "BUG-09"]);
        let landed: HashSet<uuid::Uuid> = tokens
            .iter()
            .filter_map(|t| resolve_trailer(&store, t))
            .collect();
        let want: HashSet<uuid::Uuid> = [uuid_of("BUG-9"), uuid_of("BUG-09")]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(landed, want, "flip={flip}");
        assert_eq!(resolve_trailer(&store, "BUG-009"), None, "flip={flip}");
    }
}
