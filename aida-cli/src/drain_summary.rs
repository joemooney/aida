//! Permanent drain EXIT SUMMARY with token accounting + diff stats (TASK-967).
//!
//! When `aida queue work --auto-complete` finishes a drain — drained,
//! cap-hit, drained-with-shelved, or failed — the orchestrator emits a
//! one-block exit summary: how many specs shipped / shelved / skipped, how
//! many iterations the drain ran, the CUMULATIVE reported tokens +
//! tokens-per-spec (reusing TASK-966's [`crate::drain_caps`] log accounting
//! over `.aida/headless-logs/*.jsonl`), the branch diff stats (files /
//! insertions / deletions) across the whole drain, and the
//! findings-to-triage count. The same numbers are appended to
//! `~/.aida/usage.jsonl` as a structured `drain_summary` event (STORY-122
//! schema/append pattern, distinct `event` discriminator) so `aida usage`
//! can chart cost-per-drain — feeding the calibration + budget-dispatching
//! loop.
//!
//! Everything here is pure (no I/O): the impure inputs — cumulative tokens
//! (summed via [`crate::drain_caps::tokens_from_log`]) and the
//! `git diff --numstat` body — are gathered by the CLI handler and handed in,
//! so the render + the JSONL record are unit-testable in isolation.
// trace:TASK-967 | ai:claude

use serde_json::json;

/// Branch diff stats accumulated across a whole drain (start-of-drain HEAD →
/// end HEAD on the integration branch).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DrainDiffStats {
    pub(crate) files_changed: usize,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

impl DrainDiffStats {
    /// Parse a `git diff --numstat <base>..HEAD` body. Each row is
    /// `<added>\t<removed>\t<path>`; a binary file shows `-`/`-` and counts as
    /// a changed file with zero line deltas. Blank / malformed rows are
    /// skipped, so an empty body (nothing shipped) yields all-zero stats.
    pub(crate) fn from_numstat(numstat: &str) -> Self {
        let mut s = DrainDiffStats::default();
        for line in numstat.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            let mut cols = line.split('\t');
            let (Some(added), Some(removed)) = (cols.next(), cols.next()) else {
                continue;
            };
            // A row with no path column is malformed — skip it.
            if cols.next().is_none() {
                continue;
            }
            s.files_changed += 1;
            // Binary files report `-`/`-`; a non-numeric column contributes
            // zero line delta but still counts as a changed file.
            if let Ok(a) = added.parse::<usize>() {
                s.insertions += a;
            }
            if let Ok(r) = removed.parse::<usize>() {
                s.deletions += r;
            }
        }
        s
    }
}

/// Per-disposition tallies for the specs a drain touched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DrainTallies {
    pub(crate) shipped: usize,
    pub(crate) shelved: usize,
    pub(crate) skipped: usize,
    pub(crate) punted: usize,
    pub(crate) escalated: usize,
}

impl DrainTallies {
    /// Specs the drain *acted on* — a full phase attempt each (shipped,
    /// shelved, punted, or escalated). Skipped dependents are NOT acted on
    /// (their blocker was shelved, so they were never dispatched), matching
    /// the iteration accounting [`crate::drain_caps`] uses for the
    /// `--max-iterations` cap.
    pub(crate) fn iterations(&self) -> usize {
        self.shipped + self.shelved + self.punted + self.escalated
    }

    /// Specs that parked needing triage — the findings a human must work after
    /// the drain (`aida findings list`): everything that consumed a slot but
    /// did not ship.
    pub(crate) fn findings_to_triage(&self) -> usize {
        self.shelved + self.punted + self.escalated
    }
}

/// The fully-assembled, pure drain exit summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainSummary {
    /// Drain shape: `next-n`, `batch`, or `batch-chain`.
    pub(crate) kind: String,
    /// Human label for the drain target, e.g. `next 5` / `batch:foo`.
    pub(crate) label: String,
    /// Outcome token — the `BatchDrainOutcome` label (`drained`,
    /// `drained-with-shelved`, `max-reached`, `failed`, …) or, when a hard
    /// budget cap stopped the drain, the cap flag (`max-tokens`, …).
    pub(crate) outcome: String,
    pub(crate) tallies: DrainTallies,
    /// Cumulative reported tokens across every headless phase of THIS drain
    /// (input + output + cache), summed from each phase's `stream-json` log via
    /// [`crate::drain_caps::tokens_from_log`]. `0` for a fully-interactive
    /// drain (no headless logs).
    pub(crate) cumulative_tokens: u64,
    pub(crate) diff: DrainDiffStats,
    /// Wall time the drain ran, whole seconds.
    pub(crate) elapsed_secs: u64,
}

impl DrainSummary {
    /// Mean reported tokens per acted-on spec — `cumulative_tokens /
    /// iterations`, or `0` when the drain acted on nothing.
    pub(crate) fn tokens_per_spec(&self) -> u64 {
        let n = self.tallies.iterations() as u64;
        if n == 0 {
            0
        } else {
            self.cumulative_tokens / n
        }
    }

    /// The permanent human EXIT SUMMARY block. Deliberately free of registry
    /// glyphs (the pure module routes no glyphs through [`crate::glyphs`]) —
    /// plain text + a box-drawing rule + the `·` separator only.
    pub(crate) fn render(&self) -> String {
        let t = &self.tallies;
        let iters = t.iterations();
        let mut out = String::new();
        out.push_str("─ drain summary ─\n");
        out.push_str(&format!(
            "  {} ({}) · {} shipped · {} shelved · {} skipped · {} iteration{}\n",
            self.label,
            self.outcome,
            t.shipped,
            t.shelved,
            t.skipped,
            iters,
            if iters == 1 { "" } else { "s" }
        ));
        out.push_str(&format!(
            "  tokens: {} cumulative · ~{}/spec\n",
            group_thousands(self.cumulative_tokens),
            group_thousands(self.tokens_per_spec())
        ));
        out.push_str(&format!(
            "  diff: +{} -{} across {} file{}\n",
            group_thousands(self.diff.insertions as u64),
            group_thousands(self.diff.deletions as u64),
            self.diff.files_changed,
            if self.diff.files_changed == 1 {
                ""
            } else {
                "s"
            }
        ));
        let triage = t.findings_to_triage();
        if triage > 0 {
            out.push_str(&format!(
                "  findings to triage: {triage} — `aida findings list`\n"
            ));
        } else {
            out.push_str("  findings to triage: 0\n");
        }
        out
    }

    /// The structured `drain_summary` JSONL record appended to
    /// `~/.aida/usage.jsonl` (STORY-122 schema/append pattern; the distinct
    /// `event` discriminator means the per-invocation `UsageEvent` reader skips
    /// it, while a cost-per-drain reader can select it). `ts` is RFC3339;
    /// `binary_sha` / `role` mirror the per-invocation event's release/role
    /// tagging.
    pub(crate) fn to_usage_value(
        &self,
        ts: &str,
        binary_sha: Option<&str>,
        role: Option<&str>,
    ) -> serde_json::Value {
        json!({
            "event": "drain_summary",
            "ts": ts,
            "kind": self.kind,
            "outcome": self.outcome,
            "shipped": self.tallies.shipped,
            "shelved": self.tallies.shelved,
            "skipped": self.tallies.skipped,
            "punted": self.tallies.punted,
            "escalated": self.tallies.escalated,
            "iterations": self.tallies.iterations(),
            "cumulative_tokens": self.cumulative_tokens,
            "tokens_per_spec": self.tokens_per_spec(),
            "files_changed": self.diff.files_changed,
            "insertions": self.diff.insertions,
            "deletions": self.diff.deletions,
            "findings_to_triage": self.tallies.findings_to_triage(),
            "elapsed_secs": self.elapsed_secs,
            "binary_sha": binary_sha,
            "role": role,
        })
    }
}

/// Group a non-negative integer with thousands separators (`1234567` →
/// `1,234,567`). Pure, ASCII-only.
fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tallies(shipped: usize, shelved: usize, skipped: usize) -> DrainTallies {
        DrainTallies {
            shipped,
            shelved,
            skipped,
            punted: 0,
            escalated: 0,
        }
    }

    #[test]
    fn from_numstat_sums_added_removed_and_files() {
        let numstat = "10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\n0\t7\tsrc/c.rs\n";
        let s = DrainDiffStats::from_numstat(numstat);
        assert_eq!(s.files_changed, 3);
        assert_eq!(s.insertions, 15);
        assert_eq!(s.deletions, 9);
    }

    #[test]
    fn from_numstat_counts_binary_file_with_zero_lines() {
        // A binary change reports `-`/`-` — still a changed file, no line delta.
        let numstat = "10\t2\tsrc/a.rs\n-\t-\timg.png\n";
        let s = DrainDiffStats::from_numstat(numstat);
        assert_eq!(s.files_changed, 2);
        assert_eq!(s.insertions, 10);
        assert_eq!(s.deletions, 2);
    }

    #[test]
    fn from_numstat_empty_is_all_zero() {
        let s = DrainDiffStats::from_numstat("");
        assert_eq!(s, DrainDiffStats::default());
        // Blank / pathless rows are skipped rather than counted.
        let s2 = DrainDiffStats::from_numstat("\n  \n10\t2\n");
        assert_eq!(s2, DrainDiffStats::default());
    }

    #[test]
    fn iterations_excludes_skipped_dependents() {
        let t = DrainTallies {
            shipped: 3,
            shelved: 1,
            skipped: 2,
            punted: 1,
            escalated: 1,
        };
        // shipped + shelved + punted + escalated; skipped excluded.
        assert_eq!(t.iterations(), 6);
        // findings = everything acted-on that did not ship.
        assert_eq!(t.findings_to_triage(), 3);
    }

    #[test]
    fn tokens_per_spec_divides_by_iterations() {
        let s = DrainSummary {
            kind: "next-n".into(),
            label: "next 3".into(),
            outcome: "drained".into(),
            tallies: tallies(3, 0, 0),
            cumulative_tokens: 900_000,
            diff: DrainDiffStats::default(),
            elapsed_secs: 0,
        };
        assert_eq!(s.tokens_per_spec(), 300_000);
    }

    #[test]
    fn tokens_per_spec_zero_when_nothing_acted_on() {
        let s = DrainSummary {
            kind: "next-n".into(),
            label: "next 1".into(),
            outcome: "drained".into(),
            tallies: tallies(0, 0, 0),
            cumulative_tokens: 0,
            diff: DrainDiffStats::default(),
            elapsed_secs: 0,
        };
        assert_eq!(s.tokens_per_spec(), 0);
    }

    #[test]
    fn render_includes_counts_tokens_diff_and_findings() {
        let s = DrainSummary {
            kind: "batch".into(),
            label: "batch:foo".into(),
            outcome: "drained-with-shelved".into(),
            tallies: DrainTallies {
                shipped: 4,
                shelved: 1,
                skipped: 2,
                punted: 0,
                escalated: 0,
            },
            cumulative_tokens: 1_234_567,
            diff: DrainDiffStats {
                files_changed: 37,
                insertions: 4210,
                deletions: 820,
            },
            elapsed_secs: 90,
        };
        let out = s.render();
        assert!(out.contains("drain summary"));
        assert!(out.contains("batch:foo"));
        assert!(out.contains("drained-with-shelved"));
        assert!(out.contains("4 shipped"));
        assert!(out.contains("1 shelved"));
        assert!(out.contains("2 skipped"));
        assert!(out.contains("5 iterations"));
        // Thousands-grouped token figures.
        assert!(out.contains("1,234,567 cumulative"));
        assert!(out.contains("246,913/spec"), "got: {out}");
        // Diff stats.
        assert!(out.contains("+4,210 -820 across 37 files"));
        // Findings → triage hint.
        assert!(out.contains("findings to triage: 1"));
        assert!(out.contains("aida findings list"));
    }

    #[test]
    fn render_singularizes_one_iteration_and_one_file() {
        let s = DrainSummary {
            kind: "next-n".into(),
            label: "next 1".into(),
            outcome: "drained".into(),
            tallies: tallies(1, 0, 0),
            cumulative_tokens: 50,
            diff: DrainDiffStats {
                files_changed: 1,
                insertions: 3,
                deletions: 0,
            },
            elapsed_secs: 0,
        };
        let out = s.render();
        // Singular: "1 iteration" (no plural s), end-of-line.
        assert!(out.contains("1 iteration\n"), "got: {out}");
        assert!(!out.contains("1 iterations"), "got: {out}");
        assert!(out.contains("across 1 file\n"), "got: {out}");
        assert!(out.contains("findings to triage: 0"));
    }

    #[test]
    fn to_usage_value_is_a_drain_summary_event() {
        let s = DrainSummary {
            kind: "batch-chain".into(),
            label: "batch:a".into(),
            outcome: "max-tokens".into(),
            tallies: DrainTallies {
                shipped: 2,
                shelved: 1,
                skipped: 0,
                punted: 1,
                escalated: 0,
            },
            cumulative_tokens: 800_000,
            diff: DrainDiffStats {
                files_changed: 9,
                insertions: 100,
                deletions: 40,
            },
            elapsed_secs: 600,
        };
        let v = s.to_usage_value("2026-06-28T00:00:00Z", Some("abc1234"), Some("advisor"));
        assert_eq!(v["event"], "drain_summary");
        assert_eq!(v["kind"], "batch-chain");
        assert_eq!(v["outcome"], "max-tokens");
        assert_eq!(v["shipped"], 2);
        assert_eq!(v["shelved"], 1);
        assert_eq!(v["punted"], 1);
        assert_eq!(v["iterations"], 4);
        assert_eq!(v["cumulative_tokens"], 800_000);
        assert_eq!(v["tokens_per_spec"], 200_000);
        assert_eq!(v["files_changed"], 9);
        assert_eq!(v["insertions"], 100);
        assert_eq!(v["deletions"], 40);
        assert_eq!(v["findings_to_triage"], 2);
        assert_eq!(v["elapsed_secs"], 600);
        assert_eq!(v["binary_sha"], "abc1234");
        assert_eq!(v["role"], "advisor");
        assert_eq!(v["ts"], "2026-06-28T00:00:00Z");
    }

    #[test]
    fn to_usage_value_nulls_absent_sha_and_role() {
        let s = DrainSummary {
            kind: "next-n".into(),
            label: "next 2".into(),
            outcome: "drained".into(),
            tallies: tallies(2, 0, 0),
            cumulative_tokens: 0,
            diff: DrainDiffStats::default(),
            elapsed_secs: 1,
        };
        let v = s.to_usage_value("2026-06-28T00:00:00Z", None, None);
        assert!(v["binary_sha"].is_null());
        assert!(v["role"].is_null());
    }

    #[test]
    fn group_thousands_formats() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(42), "42");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }
}
