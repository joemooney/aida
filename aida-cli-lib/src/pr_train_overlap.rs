use std::collections::{BTreeMap, BTreeSet};

/// Prefer the PR that disturbs the fewest files in the remaining approved
/// PRs. Recompute after each choice; numeric PR order breaks ties. This is a
/// deterministic greedy heuristic, not a claim of globally optimal scheduling.
// trace:STORY-2693 | ai:codex
pub(super) fn order(files: &BTreeMap<u64, BTreeSet<Vec<u8>>>) -> Vec<u64> {
    let mut remaining = files.clone();
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let next = *remaining
            .keys()
            .min_by_key(|id| {
                let cost: usize = remaining
                    .iter()
                    .filter(|(other, _)| *other != *id)
                    .map(|(_, paths)| remaining[id].intersection(paths).count())
                    .sum();
                (cost, **id)
            })
            .unwrap();
        remaining.remove(&next);
        result.push(next);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-2693 | ai:codex
    #[test]
    fn story2693_orders_low_overlap_before_shared_files_and_breaks_ties() {
        let files = [(9, vec!["shared"]), (2, vec!["shared"]), (7, vec!["alone"])]
            .into_iter()
            .map(|(id, paths)| {
                (
                    id,
                    paths.into_iter().map(|p| p.as_bytes().to_vec()).collect(),
                )
            })
            .collect();
        assert_eq!(order(&files), vec![7, 2, 9]);
        assert!(order(&BTreeMap::new()).is_empty());
    }

    // trace:STORY-2693 | ai:codex
    #[test]
    fn story2693_recomputes_cost_after_each_choice() {
        let files = [
            (1, vec!["a", "b"]),
            (2, vec!["a"]),
            (3, vec!["b", "c"]),
            (4, vec!["c"]),
        ]
        .into_iter()
        .map(|(id, paths)| {
            (
                id,
                paths.into_iter().map(|p| p.as_bytes().to_vec()).collect(),
            )
        })
        .collect();
        // Initial costs are 2, 1, 2, 1. Removing 2 lowers 1's cost;
        // a once-only sort would incorrectly put 4 ahead of 1.
        assert_eq!(order(&files), vec![2, 1, 3, 4]);
    }
}
