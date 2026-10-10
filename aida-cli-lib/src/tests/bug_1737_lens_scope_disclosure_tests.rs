//! BUG-1737: the default `aida list` lens must admit that it also withholds
//! the archived and deferred tiers.
//!
//! The lens already discloses every other class it drops — closed rows
//! (STORY-723), accepted ADRs (BUG-781), machine-filed drafts (BUG-1498) —
//! each as a `note:` line carrying a count and a flag. Archived and deferred
//! were dropped silently, which let the three printed notes read as a complete
//! account of what was held back. An operator read `count: 30 of 53 open` plus
//! those three notes as the project's open backlog and concluded throughput had
//! stalled; on this repository 1548 archived and 119 deferred rows sat outside
//! the 53, unmentioned.
//!
//! What these tests pin:
//!
//! - the disclosure appears under the open-WORK lens, and names BOTH tiers,
//! - it is absent from every view that is not hiding them (`--all`,
//!   `--archived`, `--deferred`, an explicit `--status`),
//! - `aida list open` discloses identically to bare `aida list`, because
//!   BUG-788 established the two are one view and filed their disagreement as a
//!   defect,
//! - and it stays FREE: a `bool` in, a fixed string out, so it can never grow
//!   into the counted STORY-441 / STORY-584 nudge that BUG-783 deliberately
//!   switched off and that costs an extra cache query per tier.
//!
//! trace:BUG-1737 | ai:claude

use super::*;

/// The lens-decision helper both render paths feed from, spelled the way the
/// command builds it: bare `aida list` takes the default lens; the explicit
/// `open` alias sets a status filter (clearing the default lens) but is the
/// same view.
fn disclosure_for(
    default_open_lens: bool,
    explicit_open_alias: bool,
    all: bool,
    archived: bool,
    deferred: bool,
) -> Option<&'static str> {
    list_lens_scope_disclosure(list_applies_open_work_lens(
        default_open_lens,
        explicit_open_alias,
        all,
        archived,
        deferred,
    ))
}

/// Bare `aida list` — the surface that caused the misread.
// trace:BUG-1737 | ai:claude
#[test]
fn bare_list_discloses_the_two_hidden_tiers() {
    let line = disclosure_for(true, false, false, false, false)
        .expect("the default open lens hides archived and deferred rows; it must say so");
    assert!(
        line.contains("archived"),
        "the archived tier must be named: {line}"
    );
    assert!(
        line.contains("deferred"),
        "the deferred tier must be named: {line}"
    );
}

/// Naming only one tier leaves the other silently dropped — the exact defect,
/// half-fixed. Both words are load-bearing, so pin them separately from the
/// presence check above.
// trace:BUG-1737 | ai:claude
#[test]
fn disclosure_names_both_tiers_not_just_one() {
    let line = list_lens_scope_disclosure(true).unwrap();
    let named = ["archived", "deferred"]
        .iter()
        .filter(|tier| line.contains(**tier))
        .count();
    assert_eq!(named, 2, "both tiers must be named, found {named}: {line}");
}

/// `aida list open` is a second spelling of the same view (BUG-788). It sets a
/// status filter, so `default_open_lens` is FALSE — wiring the disclosure to
/// that bool instead of the open-work lens would leave this spelling silent and
/// re-open exactly the disagreement BUG-788 closed.
// trace:BUG-1737 trace:BUG-788 | ai:claude
#[test]
fn explicit_open_alias_discloses_identically_to_bare_list() {
    assert_eq!(
        disclosure_for(false, true, false, false, false),
        disclosure_for(true, false, false, false, false),
        "`aida list open` and bare `aida list` are one view; they must disclose alike"
    );
}

/// The widening flags. Each one is a request FOR the tiers, so there is nothing
/// held back to admit — and a line claiming otherwise would be false.
// trace:BUG-1737 | ai:claude
#[test]
fn widened_views_do_not_disclose() {
    for (label, all, archived, deferred) in [
        ("--all", true, false, false),
        ("--archived", false, true, false),
        ("--deferred", false, false, true),
    ] {
        assert_eq!(
            disclosure_for(false, false, all, archived, deferred),
            None,
            "{label} is not hiding these tiers; it must not claim to"
        );
        assert_eq!(
            disclosure_for(false, true, all, archived, deferred),
            None,
            "`aida list open {label}` widens the view too"
        );
    }
}

/// An ordinary explicit `--status` ask (e.g. `--status approved`) is a narrower,
/// deliberate query, not the front door. It keeps the BUG-783 quiet default.
// trace:BUG-1737 | ai:claude
#[test]
fn explicit_status_filter_does_not_disclose() {
    assert_eq!(disclosure_for(false, false, false, false, false), None);
}

// --- AC2: the disclosure must stay free. ---

/// A `bool` in and a `&'static str` out is what keeps this off the query
/// budget. BUG-783 turned the *counted* archived/deferred nudges off partly
/// because each costs an extra `list_summaries` round trip that the hot path
/// skips; the temptation to "just add the number here" would quietly put both
/// of them back. Guard the helper's body against ever reaching a backend.
// trace:BUG-1737 trace:BUG-783 | ai:claude
#[test]
fn disclosure_helper_never_queries_the_backend() {
    let src = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("../lib.rs"),
            include_str!("../lib_part1.rs"),
            include_str!("../lib_part2.rs"),
            include_str!("../lib_part3.rs"),
            include_str!("../lib_part4.rs"),
            include_str!("../lib_part5.rs"),
            include_str!("../lib_part6.rs")
        );
    let start = src
        .find("fn list_lens_scope_disclosure(")
        .expect("helper must exist");
    let body = &src[start..start + 400];
    for forbidden in ["list_summaries", "backend", "ListFilter"] {
        assert!(
            !body.contains(forbidden),
            "list_lens_scope_disclosure must stay a pure bool->str map; found `{forbidden}`. \
             Adding a count here re-adds the per-tier cache query BUG-783 removed."
        );
    }
}

// --- The wiring. The helper being correct is worthless if a render path
// --- forgets to call it, or calls it with the bare-list bool. ---

/// Both `aida list` render paths — the agent-mode TOON notes and the human
/// footer — must print the disclosure, and both must gate it on the open-WORK
/// lens. `default_open_lens` is the plausible wrong bool: it reads correctly
/// for bare `aida list` and silently drops `aida list open`.
// trace:BUG-1737 | ai:claude
#[test]
fn both_render_paths_are_wired_to_the_open_work_lens() {
    let src = include_str!("../git_backend_cmd.rs");
    let calls: Vec<&str> = src
        .match_indices("list_lens_scope_disclosure(")
        .map(|(i, _)| {
            let rest = &src[i..];
            let end = rest.find(')').expect("call must close");
            &rest[..=end]
        })
        .collect();
    assert_eq!(
        calls.len(),
        2,
        "expected the agent-mode note and the human footer to call it; found {calls:?}"
    );
    for call in &calls {
        assert!(
            call.contains("open_work_lens"),
            "render path must gate on the open-work lens, not `{call}` — \
             `default_open_lens` would silence `aida list open` (BUG-788)"
        );
    }
    assert!(
        src.contains("note: {scope} — `aida list --all` for every spec"),
        "agent mode must follow the existing `note:` convention"
    );
    assert!(
        src.contains("  ({scope} — pass --all to see them)"),
        "the human path must follow the existing dimmed footer idiom"
    );
}
