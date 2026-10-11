//! Natural (numeric-aware) ordering for requirement IDs.
//!
//! `aida list --sort id` used to compare IDs as text, so `TASK-1000` sorted
//! before `TASK-2` and `FAQ-11` before `FAQ-2`. This module defines the one
//! ordering every list surface shares (human table, agent TOON, `--json`,
//! `--short`):
//!
//! 1. the type prefix (`ADR`, `BUG`, `FAQ`, `STORY`, …) compared as a string,
//!    so mixed types group by prefix;
//! 2. within a prefix, agreed IDs (`FAQ-2`, one numeric segment) before
//!    unpromoted node-aware IDs (`FAQ-1-232`, two or more numeric segments),
//!    so a node-aware ID never falls between `FAQ-1` and `FAQ-2`;
//! 3. each numeric segment compared as an integer (any width, leading zeros
//!    ignored);
//! 4. the raw text as a final, total tie-break.
//!
//! IDs that don't parse as `PREFIX-N[-N…]` (a bare UUID, a free-form key)
//! sort after every parseable ID of the same prefix, by text.
// trace:STORY-1644 | ai:claude

use std::cmp::Ordering;

/// Rank of an ID within its prefix group. Lower sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IdClass {
    /// `PREFIX-N` — an agreed (or legacy short) ID.
    Agreed,
    /// `PREFIX-N-M[-…]` — an unpromoted node-aware ID.
    NodeAware,
    /// Anything else.
    Other,
}

/// A parsed sort key borrowing from the ID text.
#[derive(Debug)]
struct IdKey<'a> {
    prefix: &'a str,
    class: IdClass,
    numbers: Vec<&'a str>,
    raw: &'a str,
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn parse(raw: &str) -> IdKey<'_> {
    let trimmed = raw.trim();
    // The prefix runs up to the first `-` that is followed by an all-digit
    // segment; everything after must be all-digit segments for a numeric key.
    let mut search_from = 0;
    while let Some(rel) = trimmed[search_from..].find('-') {
        let dash = search_from + rel;
        let prefix = &trimmed[..dash];
        let rest = &trimmed[dash + 1..];
        let segments: Vec<&str> = rest.split('-').collect();
        if !prefix.is_empty() && segments.iter().all(|s| is_digits(s)) {
            let class = if segments.len() == 1 {
                IdClass::Agreed
            } else {
                IdClass::NodeAware
            };
            return IdKey {
                prefix,
                class,
                numbers: segments,
                raw,
            };
        }
        search_from = dash + 1;
    }
    let prefix = trimmed.split('-').next().unwrap_or(trimmed);
    IdKey {
        prefix,
        class: IdClass::Other,
        numbers: Vec::new(),
        raw,
    }
}

/// Compare two all-digit strings by integer value without overflow:
/// strip leading zeros, then shorter is smaller, then lexicographic.
fn cmp_digits(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Natural numeric ordering of two requirement IDs (see module docs).
pub fn natural_id_cmp(a: &str, b: &str) -> Ordering {
    let ka = parse(a);
    let kb = parse(b);
    ka.prefix
        .to_ascii_uppercase()
        .cmp(&kb.prefix.to_ascii_uppercase())
        .then_with(|| ka.class.cmp(&kb.class))
        .then_with(|| {
            for (x, y) in ka.numbers.iter().zip(kb.numbers.iter()) {
                let o = cmp_digits(x, y);
                if o != Ordering::Equal {
                    return o;
                }
            }
            ka.numbers.len().cmp(&kb.numbers.len())
        })
        .then_with(|| ka.raw.cmp(kb.raw))
}

/// The ID a list row sorts by: the agreed ID when present, else the
/// (possibly node-aware) spec ID, else the UUID text.
pub fn list_sort_key<'a>(
    agreed_id: Option<&'a str>,
    spec_id: Option<&'a str>,
    uuid: &'a str,
) -> &'a str {
    agreed_id
        .filter(|s| !s.trim().is_empty())
        .or(spec_id.filter(|s| !s.trim().is_empty()))
        .unwrap_or(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(ids: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| natural_id_cmp(a, b));
        v
    }

    #[test]
    fn short_ids_sort_numerically() {
        assert_eq!(
            sorted(&["FAQ-11", "FAQ-2", "FAQ-10", "FAQ-1", "FAQ-9"]),
            ["FAQ-1", "FAQ-2", "FAQ-9", "FAQ-10", "FAQ-11"]
        );
        assert_eq!(
            sorted(&["TASK-1000", "TASK-2", "TASK-100"]),
            ["TASK-2", "TASK-100", "TASK-1000"]
        );
    }

    #[test]
    fn mixed_types_group_by_prefix() {
        assert_eq!(
            sorted(&["TASK-3", "BUG-20", "ADR-7", "STORY-1", "BUG-3", "EPIC-2", "FAQ-1"]),
            ["ADR-7", "BUG-3", "BUG-20", "EPIC-2", "FAQ-1", "STORY-1", "TASK-3"]
        );
    }

    #[test]
    fn node_aware_ids_follow_agreed_ids_of_their_type() {
        assert_eq!(
            sorted(&[
                "FAQ-1-232",
                "FAQ-2",
                "FAQ-1",
                "FAQ-12",
                "FAQ-1-5",
                "FAQ-2-1"
            ]),
            [
                "FAQ-1",
                "FAQ-2",
                "FAQ-12",
                "FAQ-1-5",
                "FAQ-1-232",
                "FAQ-2-1"
            ]
        );
        // ... and before the next type.
        assert_eq!(
            sorted(&["STORY-1", "FAQ-1-232", "FAQ-3"]),
            ["FAQ-3", "FAQ-1-232", "STORY-1"]
        );
    }

    #[test]
    fn leading_zeros_and_huge_numbers() {
        assert_eq!(
            sorted(&["FR-1-042", "FR-1-41", "FR-1-0043"]),
            ["FR-1-41", "FR-1-042", "FR-1-0043"]
        );
        assert_eq!(
            sorted(&["TASK-99999999999999999999999", "TASK-3"]),
            ["TASK-3", "TASK-99999999999999999999999"]
        );
        // Equal value, different text: deterministic by raw text.
        assert_eq!(natural_id_cmp("FR-07", "FR-7"), Ordering::Less);
    }

    #[test]
    fn unparseable_ids_sort_after_parseable_same_prefix() {
        assert_eq!(
            sorted(&["TASK-abc", "TASK-2", "TASK-1-1"]),
            ["TASK-2", "TASK-1-1", "TASK-abc"]
        );
        // A bare word groups with its own text as the prefix.
        assert_eq!(sorted(&["zeta", "ADR-1"]), ["ADR-1", "zeta"]);
    }

    #[test]
    fn hyphenated_prefix_is_supported() {
        assert_eq!(sorted(&["X-RAY-10", "X-RAY-9"]), ["X-RAY-9", "X-RAY-10"]);
    }

    #[test]
    fn prefix_compare_ignores_case() {
        assert_eq!(sorted(&["faq-10", "FAQ-9"]), ["FAQ-9", "faq-10"]);
    }

    #[test]
    fn sort_key_prefers_agreed_id() {
        assert_eq!(
            list_sort_key(Some("FAQ-2"), Some("FAQ-1-232"), "u"),
            "FAQ-2"
        );
        assert_eq!(list_sort_key(None, Some("FAQ-1-232"), "u"), "FAQ-1-232");
        assert_eq!(list_sort_key(Some(""), None, "u"), "u");
    }
}
