//! Fuzzy command palette core (STORY-682, EPIC-52 slice 2).
//!
//! The redesigned `aida tui` (design:
//! `docs/plans/2026-06-25-tui-redesign-fuzzy-command-palette.md`) opens as a
//! single Claude-Code-like input line: you type a few characters and a
//! fuzzy-filtered list of `aida` commands rises beneath it. This module is the
//! **pure, fully-unit-testable core** of that front door — no terminal, no
//! rendering, no launcher wiring (that is the separate supervised UI slice).
//!
//! Two pieces live here:
//!
//! 1. [`enumerate`] — walks the real `aida` clap [`clap::Command`] tree and
//!    flattens every subcommand into a [`CommandEntry`] (`path` = the
//!    space-joined subcommand path, `about` = clap's about text). Sourcing the
//!    surface from clap (rather than a hand-maintained list) means it can never
//!    drift from the CLI: a new `aida` subcommand shows up in the palette for
//!    free.
//!
//! 2. [`rank`] — a tiny in-repo subsequence fuzzy matcher with Claude-like
//!    ranking (contiguous-run, word-boundary, and prefix boosts; gap
//!    penalties), plus a small curated **common-actions** boost so the verbs a
//!    user reaches for most float to the top on short/ambiguous queries. Zero
//!    new dependencies.
//!
//! trace:STORY-682 | ai:claude

use clap::Command;

/// One entry in the command surface: a runnable `aida` (sub)command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    /// Space-joined subcommand path, e.g. `"queue work"` or `"config menu"`.
    /// This is the canonical match target and the string the palette echoes.
    pub path: String,
    /// clap's `about` text for the leaf command (empty string if none).
    pub about: String,
}

/// A [`CommandEntry`] paired with its match [`score`](Scored::score) for a
/// given query. Returned by [`rank`], best-first.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// The matched command surface entry.
    pub entry: CommandEntry,
    /// Higher is a better match. Comparable only within one [`rank`] call —
    /// the absolute magnitude is not meaningful across queries.
    pub score: i32,
}

/// The curated "common actions" — the handful of verbs a user reaches for most
/// often. These are used **only to BOOST ranking** (a small bonus added when a
/// matched entry's path is one of these), never to replace or filter the
/// clap-derived surface. Empty queries also fall back to this list, in this
/// order, so the cold-open palette shows something sensible.
///
/// Keep this short and high-signal; it is a ranking nudge, not a menu.
pub const COMMON_ACTIONS: &[&str] = &[
    "queue work",
    "list",
    "status",
    "findings list",
    "config menu",
    "show",
];

/// Bonus added to an entry's score when its `path` is in [`COMMON_ACTIONS`].
/// Large enough to break ties and lift a common action over a comparably-scored
/// neighbor, small enough that a clearly-better textual match still wins.
const COMMON_ACTION_BOOST: i32 = 40;

// ---------------------------------------------------------------------------
// 1. Command-surface enumeration from clap
// ---------------------------------------------------------------------------

/// Walk a clap [`Command`] tree and flatten every (transitive) subcommand into
/// a [`CommandEntry`]. The caller passes the real CLI root —
/// `aida_cli::Cli::command()` — so the surface never drifts from the binary.
///
/// `path` is the space-joined chain of subcommand names from (but not
/// including) the root, e.g. the `work` subcommand of the `queue` subcommand of
/// `aida` yields `"queue work"`. The root command itself is not emitted (it is
/// the `aida` binary, not a runnable action); only its descendants are.
///
/// Hidden subcommands (`clap`'s `hide(true)`) and the auto-generated `help`
/// subcommand are skipped — they are not actions a user would pick from the
/// palette.
pub fn enumerate(root: &Command) -> Vec<CommandEntry> {
    let mut out = Vec::new();
    for sub in root.get_subcommands() {
        walk(sub, "", &mut out);
    }
    out
}

/// Recurse into `cmd`, appending it and all its descendants to `out`. `prefix`
/// is the already-joined path of ancestors (empty at the first level).
fn walk(cmd: &Command, prefix: &str, out: &mut Vec<CommandEntry>) {
    let name = cmd.get_name();
    // Skip clap's synthetic `help` subcommand and anything explicitly hidden.
    if name == "help" || cmd.is_hide_set() {
        return;
    }

    let path = if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix} {name}")
    };

    let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    out.push(CommandEntry {
        path: path.clone(),
        about,
    });

    for sub in cmd.get_subcommands() {
        walk(sub, &path, out);
    }
}

// ---------------------------------------------------------------------------
// 2. The tiny in-repo fuzzy matcher
// ---------------------------------------------------------------------------

/// Score a `query` against a `candidate` using a subsequence matcher with
/// Claude-like ranking. Returns `Some(score)` when every char of `query`
/// appears in `candidate` in order (case-insensitive), `None` otherwise.
///
/// Heuristics, all additive into the score:
/// - **base hit** — a flat reward per matched char, so longer matches that
///   still subsequence-match aren't unfairly out-scored by trivially short ones.
/// - **contiguous run** — consecutive query chars landing on consecutive
///   candidate chars compound (the run length feeds a growing bonus), so a
///   substring match dominates a scattered one.
/// - **word-boundary** — a char that lands at the start, or right after a space
///   / `-` / `_`, scores extra: typing `qw` should privilege the `q` of
///   `queue` and the `w` of `work`.
/// - **prefix** — if the whole query is a prefix of the candidate, a flat bonus
///   on top, so `que` ranks `queue …` above a command that merely contains
///   those letters out of position.
/// - **gap penalty** — skipped candidate chars between two matches cost a
///   little, so tighter matches win.
///
/// The candidate is matched as its full text (e.g. the whole `"queue work"`
/// path); callers typically match against [`CommandEntry::path`].
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query.trim().to_lowercase().chars().collect();
    if q.is_empty() {
        // An empty query "matches" everything with a neutral score; callers
        // (`rank`) special-case empty queries before reaching here, but keep
        // this well-defined for direct callers.
        return Some(0);
    }

    let cand_lower: Vec<char> = candidate.to_lowercase().chars().collect();
    let cand_orig: Vec<char> = candidate.chars().collect();

    let mut score: i32 = 0;
    let mut qi = 0usize; // index into the query
    let mut run: i32 = 0; // current contiguous-run length
    let mut prev_match: Option<usize> = None; // candidate index of the last match

    for (ci, &cc) in cand_lower.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if cc != q[qi] {
            continue;
        }

        // --- this candidate char matches the next query char ---
        score += BASE_HIT;

        // Contiguous run: did we match the immediately preceding char too?
        if prev_match == Some(ci.wrapping_sub(1)) {
            run += 1;
            score += CONTIGUOUS_STEP * run;
        } else {
            run = 0;
        }

        // Word-boundary bonus: start of string, or preceded by a separator.
        let at_boundary =
            ci == 0 || matches!(cand_orig.get(ci - 1), Some(' ') | Some('-') | Some('_'));
        if at_boundary {
            score += WORD_BOUNDARY_BONUS;
        }

        // Gap penalty: how many candidate chars did we skip since the last
        // match? Only penalize gaps *after* the first matched char.
        if let Some(prev) = prev_match {
            let gap = ci.saturating_sub(prev + 1) as i32;
            score -= gap * GAP_PENALTY;
        }

        prev_match = Some(ci);
        qi += 1;
    }

    if qi < q.len() {
        // Ran out of candidate before consuming the whole query: no match.
        return None;
    }

    // Whole-query-prefix bonus.
    if cand_lower.len() >= q.len() && cand_lower[..q.len()] == q[..] {
        score += PREFIX_BONUS;
    }

    Some(score)
}

const BASE_HIT: i32 = 4;
const CONTIGUOUS_STEP: i32 = 6;
const WORD_BOUNDARY_BONUS: i32 = 10;
const PREFIX_BONUS: i32 = 18;
const GAP_PENALTY: i32 = 1;

// ---------------------------------------------------------------------------
// 3. The public ranking API
// ---------------------------------------------------------------------------

/// Rank `entries` against `query`, returning only those that match, sorted
/// best-first. A common-actions boost ([`COMMON_ACTIONS`]) is applied so the
/// most-reached-for verbs float up on short / ambiguous queries.
///
/// **Empty query** (after trimming) returns the [`COMMON_ACTIONS`] entries that
/// exist in `entries`, in `COMMON_ACTIONS` order — the sensible cold-open list
/// — rather than the entire surface.
///
/// Ties (equal score) are broken by shorter `path` first (a more specific /
/// shallower command is the likelier intent), then alphabetically for
/// determinism.
pub fn rank(entries: &[CommandEntry], query: &str) -> Vec<Scored> {
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return common_action_entries(entries);
    }

    let mut scored: Vec<Scored> = entries
        .iter()
        .filter_map(|e| {
            fuzzy_score(trimmed, &e.path).map(|mut s| {
                if is_common_action(&e.path) {
                    s += COMMON_ACTION_BOOST;
                }
                Scored {
                    entry: e.clone(),
                    score: s,
                }
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.entry.path.len().cmp(&b.entry.path.len()))
            .then_with(|| a.entry.path.cmp(&b.entry.path))
    });

    scored
}

/// The [`COMMON_ACTIONS`] that are present in `entries`, in `COMMON_ACTIONS`
/// order, each scored with the boost. Used for the empty-query cold open.
fn common_action_entries(entries: &[CommandEntry]) -> Vec<Scored> {
    COMMON_ACTIONS
        .iter()
        .filter_map(|&action| {
            entries.iter().find(|e| e.path == action).map(|e| Scored {
                entry: e.clone(),
                score: COMMON_ACTION_BOOST,
            })
        })
        .collect()
}

/// Is `path` one of the curated common actions?
fn is_common_action(path: &str) -> bool {
    COMMON_ACTIONS.contains(&path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built surface that mirrors the relevant slice of the real `aida`
    /// command tree, so ranking tests don't depend on the full CLI.
    fn fixture() -> Vec<CommandEntry> {
        let mk = |p: &str, a: &str| CommandEntry {
            path: p.to_string(),
            about: a.to_string(),
        };
        vec![
            mk("queue work", "drain a spec end-to-end"),
            mk("queue list", "show the work queue"),
            mk("queue next", "claim the next item"),
            mk("list", "list requirements"),
            mk("status", "project snapshot"),
            mk("show", "show a requirement"),
            mk("findings list", "list findings"),
            mk("config menu", "interactive config editor"),
            mk("search", "search requirements and code"),
        ]
    }

    fn paths(scored: &[Scored]) -> Vec<&str> {
        scored.iter().map(|s| s.entry.path.as_str()).collect()
    }

    // --- subsequence matching correctness -------------------------------

    #[test]
    fn subsequence_matches_in_order() {
        // "qw" is a subsequence of "queue work" (q...w).
        assert!(fuzzy_score("qw", "queue work").is_some());
        // chars present but out of order do NOT match.
        assert!(fuzzy_score("wq", "queue work").is_none());
    }

    #[test]
    fn non_subsequence_returns_none() {
        // 'z' never appears.
        assert!(fuzzy_score("zzz", "queue work").is_none());
        // query longer than what's available in order.
        assert!(fuzzy_score("queuework!", "queue work").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(fuzzy_score("QW", "queue work").is_some());
        assert!(fuzzy_score("qUeUe", "QUEUE WORK").is_some());
    }

    #[test]
    fn exact_substring_matches() {
        assert!(fuzzy_score("queue", "queue work").is_some());
        assert!(fuzzy_score("work", "queue work").is_some());
    }

    // --- ranking order --------------------------------------------------

    #[test]
    fn qw_ranks_queue_work_above_queue_list() {
        // "qw": queue WORK has the w at a word boundary; queue list has no
        // 'w' so it shouldn't even match.
        let res = rank(&fixture(), "qw");
        let p = paths(&res);
        assert_eq!(p.first(), Some(&"queue work"));
        assert!(!p.contains(&"queue list"), "queue list has no 'w' to match");
    }

    #[test]
    fn full_phrase_ranks_queue_work_first() {
        let res = rank(&fixture(), "queue work");
        assert_eq!(paths(&res).first(), Some(&"queue work"));
    }

    #[test]
    fn prefix_beats_scattered_match() {
        // "lis" is a clean prefix of "list" but a scattered subsequence of
        // "findings list" (... l-i-s near the end) and "config menu" (no
        // match). The prefix match must rank first.
        let res = rank(&fixture(), "lis");
        let p = paths(&res);
        let list_pos = p.iter().position(|&x| x == "list").unwrap();
        let findings_pos = p.iter().position(|&x| x == "findings list");
        if let Some(fp) = findings_pos {
            assert!(
                list_pos < fp,
                "prefix match 'list' must outrank scattered 'findings list': {p:?}"
            );
        }
        assert_eq!(p.first(), Some(&"list"));
    }

    #[test]
    fn word_boundary_boost_lifts_boundary_match() {
        // "cm" matches "config menu" at two word boundaries (Config, Menu).
        // It should score well and rank first among any matches.
        let res = rank(&fixture(), "cm");
        assert_eq!(paths(&res).first(), Some(&"config menu"));
    }

    #[test]
    fn contiguous_run_beats_gappy_match() {
        // Direct score comparison: contiguous "stat" in "status" should beat
        // the same query forced to be gappy in a synthetic candidate.
        let contiguous = fuzzy_score("stat", "status").unwrap();
        let gappy = fuzzy_score("stat", "s-t-a-t-x").unwrap();
        assert!(
            contiguous > gappy,
            "contiguous {contiguous} should beat gappy {gappy}"
        );
    }

    // --- empty-query behavior -------------------------------------------

    #[test]
    fn empty_query_returns_common_actions_in_order() {
        let res = rank(&fixture(), "");
        let p = paths(&res);
        // Only the common actions present in the fixture, in COMMON_ACTIONS
        // order. The fixture omits none of them.
        assert_eq!(
            p,
            vec![
                "queue work",
                "list",
                "status",
                "findings list",
                "config menu",
                "show",
            ]
        );
    }

    #[test]
    fn whitespace_only_query_is_treated_as_empty() {
        let res = rank(&fixture(), "   ");
        assert_eq!(paths(&res).first(), Some(&"queue work"));
        // and it returns the common-actions set, not the full surface
        assert!(!paths(&res).contains(&"search"));
    }

    // --- common-actions boost -------------------------------------------

    #[test]
    fn common_action_boost_applied() {
        // Build two entries that score identically on the raw matcher: a
        // common action and a non-common one with the same path shape.
        let entries = vec![
            CommandEntry {
                path: "list".into(),
                about: String::new(),
            },
            CommandEntry {
                path: "lint".into(),
                about: String::new(),
            },
        ];
        // "li" is a 2-char prefix of both; only "list" is a common action.
        let res = rank(&entries, "li");
        let list = res.iter().find(|s| s.entry.path == "list").unwrap();
        let lint = res.iter().find(|s| s.entry.path == "lint").unwrap();
        assert!(
            list.score > lint.score,
            "common-action 'list' ({}) must outscore 'lint' ({})",
            list.score,
            lint.score
        );
        assert_eq!(paths(&res).first(), Some(&"list"));
    }

    #[test]
    fn boost_does_not_resurrect_non_matches() {
        // A common action that does NOT match the query must not appear.
        let res = rank(&fixture(), "zzz");
        assert!(res.is_empty(), "no entry contains 'zzz' as a subsequence");
    }

    // --- no-match returns empty -----------------------------------------

    #[test]
    fn no_match_returns_empty() {
        assert!(rank(&fixture(), "qqqqq").is_empty());
        assert!(rank(&fixture(), "%%%").is_empty());
    }

    #[test]
    fn results_are_sorted_descending_by_score() {
        let res = rank(&fixture(), "s");
        for w in res.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "scores must be non-increasing: {} then {}",
                w[0].score,
                w[1].score
            );
        }
    }

    // --- clap enumeration -----------------------------------------------

    #[test]
    fn enumerate_flattens_nested_subcommands() {
        let root = Command::new("aida")
            .subcommand(
                Command::new("queue")
                    .about("work queue")
                    .subcommand(Command::new("work").about("drain a spec"))
                    .subcommand(Command::new("list").about("show the queue")),
            )
            .subcommand(Command::new("status").about("project snapshot"));

        let entries = enumerate(&root);
        let by_path: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        // Both the parent and the nested children are present, space-joined.
        assert!(by_path.contains(&"queue"));
        assert!(by_path.contains(&"queue work"));
        assert!(by_path.contains(&"queue list"));
        assert!(by_path.contains(&"status"));
        // The root itself is NOT emitted.
        assert!(!by_path.contains(&"aida"));

        // About text is carried through.
        let work = entries.iter().find(|e| e.path == "queue work").unwrap();
        assert_eq!(work.about, "drain a spec");
    }

    #[test]
    fn enumerate_skips_help_and_hidden() {
        let root = Command::new("aida")
            .subcommand(Command::new("visible").about("shown"))
            .subcommand(Command::new("secret").about("hidden").hide(true));

        let entries = enumerate(&root);
        let by_path: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(by_path.contains(&"visible"));
        assert!(
            !by_path.contains(&"secret"),
            "hidden command must be skipped"
        );
        // clap auto-injects a `help` subcommand when there are subcommands;
        // our walker drops it.
        assert!(!by_path.contains(&"help"), "synthetic help must be skipped");
    }

    #[test]
    fn enumerated_surface_is_rankable() {
        // End-to-end: enumerate a small tree, then rank over it.
        let root = Command::new("aida")
            .subcommand(
                Command::new("queue")
                    .subcommand(Command::new("work").about("drain"))
                    .subcommand(Command::new("list").about("show")),
            )
            .subcommand(Command::new("list").about("list reqs"));

        let entries = enumerate(&root);
        let res = rank(&entries, "queue work");
        assert_eq!(paths(&res).first(), Some(&"queue work"));
    }
}
