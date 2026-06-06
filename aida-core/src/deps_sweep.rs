//! Read-only likely-dependency inference for `aida deps sweep`.
//!
//! STORY-447 (operator-scoped READ-ONLY slice): a "did I miss a dependency
//! before an overnight drain?" check. Given, for every spec, the set of files
//! it touches (harvested from stored `trace_links` plus code-scanned
//! `// trace:SPEC-ID` comments) and its parent set, this module ranks the
//! *other* specs that are likely dependencies:
//!
//! - **File overlap** — two specs whose trace links touch the same file are a
//!   strong signal they are coupled. Overlap of ≥2 shared files is `High`;
//!   exactly one shared file is `Medium`.
//! - **Same parent** — siblings under the same EPIC/STORY are a `Weak` signal
//!   (often spurious — a feature cluster shares a parent without any real code
//!   coupling) so they are surfaced but never allowed to outrank a real
//!   file-overlap hit.
//!
//! The ranking is a pure function over already-collected inputs so it can be
//! unit-tested without a store or a filesystem. The CLI layer
//! (`aida deps sweep`) is responsible for collecting the inputs and rendering
//! the result. No write-back: `--apply` and the scheduled-advisor variant are
//! deliberately gated until suggestion quality is observed (operator decision,
//! 2026-06-06).
//!
//! trace:STORY-447 | ai:claude

use std::collections::{BTreeSet, HashMap};

/// Confidence tier for an inferred dependency, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Two or more shared trace-link files — strong coupling signal.
    High,
    /// Exactly one shared trace-link file — moderate coupling signal.
    Medium,
    /// Same parent (sibling) with no file overlap — often spurious; surfaced
    /// but marked weak so it never drowns out a real file-overlap hit.
    Weak,
}

impl Confidence {
    /// Lowercase label used in CLI output (`Likely depends on: TASK-7 (high)`).
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Weak => "weak",
        }
    }

    /// Sort key — lower is stronger, so `High` ranks first.
    fn rank(self) -> u8 {
        match self {
            Confidence::High => 0,
            Confidence::Medium => 1,
            Confidence::Weak => 2,
        }
    }
}

/// One inferred likely-dependency suggestion for a given source spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The display id of the candidate dependency (e.g. `TASK-7`).
    pub candidate: String,
    /// How confident the signal is.
    pub confidence: Confidence,
    /// The trace-link files shared with the source spec (empty for a
    /// same-parent-only `Weak` suggestion). Sorted for stable output.
    pub shared_files: Vec<String>,
    /// Whether the two specs share a parent (sibling relationship).
    pub same_parent: bool,
}

/// The per-spec input the ranker needs: its display id, the set of files it
/// touches (normalized however the caller likes — the ranker only compares for
/// equality), and the set of parent display ids it is a child of.
#[derive(Debug, Clone)]
pub struct SpecSignals {
    pub display_id: String,
    pub files: BTreeSet<String>,
    pub parents: BTreeSet<String>,
}

/// Rank likely dependencies for `target` against every other spec in `all`.
///
/// Pure: no store, no filesystem. The caller collects [`SpecSignals`] (from
/// stored `trace_links` + a code trace-comment scan) and renders the result.
///
/// Ranking, strongest first:
/// 1. File overlap, by shared-file count descending (≥2 → `High`, 1 →
///    `Medium`).
/// 2. Same-parent siblings with no file overlap → `Weak`.
///
/// Ties broken by candidate display id for deterministic output. A candidate
/// that both shares files *and* shares a parent surfaces once, under its
/// file-overlap (stronger) tier, with `same_parent` set so the renderer can
/// note both signals. trace:STORY-447
pub fn rank_dependencies(target: &SpecSignals, all: &[SpecSignals]) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();

    for cand in all {
        if cand.display_id == target.display_id {
            continue;
        }
        let shared: Vec<String> = target.files.intersection(&cand.files).cloned().collect();
        let same_parent = !target.parents.is_disjoint(&cand.parents);

        let confidence = if shared.len() >= 2 {
            Confidence::High
        } else if shared.len() == 1 {
            Confidence::Medium
        } else if same_parent {
            Confidence::Weak
        } else {
            // No file overlap and no shared parent — not a signal.
            continue;
        };

        out.push(Suggestion {
            candidate: cand.display_id.clone(),
            confidence,
            shared_files: shared,
            same_parent,
        });
    }

    // Strongest tier first; within a tier more shared files first; then a
    // stable id tiebreak.
    out.sort_by(|a, b| {
        a.confidence
            .rank()
            .cmp(&b.confidence.rank())
            .then_with(|| b.shared_files.len().cmp(&a.shared_files.len()))
            .then_with(|| a.candidate.cmp(&b.candidate))
    });
    out
}

/// Convenience: rank every spec in `all` against all the others, returning
/// `(source_display_id, suggestions)` pairs for sources that have at least one
/// suggestion, in input order. Used by `aida deps sweep` (no `--for-spec`).
pub fn sweep_all(all: &[SpecSignals]) -> Vec<(String, Vec<Suggestion>)> {
    let mut result = Vec::new();
    for target in all {
        let suggestions = rank_dependencies(target, all);
        if !suggestions.is_empty() {
            result.push((target.display_id.clone(), suggestions));
        }
    }
    result
}

/// Helper for callers that have already built a spec_id → files map and a
/// spec_id → parents map: assemble [`SpecSignals`] for the given display ids.
/// Display ids without an entry in either map get empty sets.
pub fn assemble_signals(
    display_ids: &[String],
    files_by_spec: &HashMap<String, BTreeSet<String>>,
    parents_by_spec: &HashMap<String, BTreeSet<String>>,
) -> Vec<SpecSignals> {
    display_ids
        .iter()
        .map(|id| SpecSignals {
            display_id: id.clone(),
            files: files_by_spec.get(id).cloned().unwrap_or_default(),
            parents: parents_by_spec.get(id).cloned().unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(id: &str, files: &[&str], parents: &[&str]) -> SpecSignals {
        SpecSignals {
            display_id: id.to_string(),
            files: files.iter().map(|s| s.to_string()).collect(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn single_shared_file_is_medium() {
        let a = sig("TASK-1", &["src/main.rs"], &[]);
        let b = sig("TASK-2", &["src/main.rs"], &[]);
        let out = rank_dependencies(&a, &[a.clone(), b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].candidate, "TASK-2");
        assert_eq!(out[0].confidence, Confidence::Medium);
        assert_eq!(out[0].shared_files, vec!["src/main.rs".to_string()]);
        assert!(!out[0].same_parent);
    }

    #[test]
    fn two_shared_files_is_high() {
        let a = sig("TASK-1", &["src/main.rs", "src/cli.rs"], &[]);
        let b = sig("TASK-2", &["src/main.rs", "src/cli.rs"], &[]);
        let out = rank_dependencies(&a, &[a.clone(), b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, Confidence::High);
        assert_eq!(out[0].shared_files.len(), 2);
    }

    #[test]
    fn same_parent_only_is_weak() {
        let a = sig("TASK-1", &["src/a.rs"], &["EPIC-9"]);
        let b = sig("TASK-2", &["src/b.rs"], &["EPIC-9"]);
        let out = rank_dependencies(&a, &[a.clone(), b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, Confidence::Weak);
        assert!(out[0].same_parent);
        assert!(out[0].shared_files.is_empty());
    }

    #[test]
    fn no_signal_is_dropped() {
        let a = sig("TASK-1", &["src/a.rs"], &["EPIC-1"]);
        let b = sig("TASK-2", &["src/b.rs"], &["EPIC-2"]);
        let out = rank_dependencies(&a, &[a.clone(), b]);
        assert!(out.is_empty());
    }

    #[test]
    fn file_overlap_outranks_same_parent() {
        // weak (same-parent) sibling and a medium (one-file-overlap) candidate;
        // the file-overlap one must come first even though both are present.
        let a = sig("TASK-1", &["src/shared.rs"], &["EPIC-9"]);
        let weak = sig("TASK-2", &["src/other.rs"], &["EPIC-9"]); // same parent only
        let medium = sig("TASK-3", &["src/shared.rs"], &["EPIC-7"]); // file overlap
        let out = rank_dependencies(&a, &[a.clone(), weak, medium]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].candidate, "TASK-3");
        assert_eq!(out[0].confidence, Confidence::Medium);
        assert_eq!(out[1].candidate, "TASK-2");
        assert_eq!(out[1].confidence, Confidence::Weak);
    }

    #[test]
    fn high_outranks_medium_then_id_tiebreak() {
        let a = sig("TASK-1", &["a.rs", "b.rs", "c.rs"], &[]);
        let high = sig("TASK-9", &["a.rs", "b.rs"], &[]); // 2 shared
        let med = sig("TASK-2", &["c.rs"], &[]); // 1 shared
        let med2 = sig("TASK-5", &["c.rs"], &[]); // 1 shared, later id
        let out = rank_dependencies(&a, &[a.clone(), med, high, med2]);
        assert_eq!(
            out.iter().map(|s| s.candidate.as_str()).collect::<Vec<_>>(),
            vec!["TASK-9", "TASK-2", "TASK-5"]
        );
    }

    #[test]
    fn shared_file_and_shared_parent_surface_once_under_file_tier() {
        let a = sig("TASK-1", &["src/x.rs"], &["EPIC-9"]);
        let b = sig("TASK-2", &["src/x.rs"], &["EPIC-9"]); // shares file AND parent
        let out = rank_dependencies(&a, &[a.clone(), b]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, Confidence::Medium); // file tier, not weak
        assert!(out[0].same_parent); // but both signals recorded
    }

    #[test]
    fn sweep_all_skips_sources_with_no_suggestions() {
        let a = sig("TASK-1", &["a.rs"], &[]);
        let b = sig("TASK-2", &["a.rs"], &[]);
        let lonely = sig("TASK-3", &["z.rs"], &[]);
        let out = sweep_all(&[a, b, lonely]);
        let sources: Vec<&str> = out.iter().map(|(s, _)| s.as_str()).collect();
        assert!(sources.contains(&"TASK-1"));
        assert!(sources.contains(&"TASK-2"));
        assert!(!sources.contains(&"TASK-3"));
    }

    #[test]
    fn assemble_signals_fills_missing_with_empty() {
        let mut files = HashMap::new();
        files.insert(
            "TASK-1".to_string(),
            BTreeSet::from(["src/a.rs".to_string()]),
        );
        let parents = HashMap::new();
        let out = assemble_signals(
            &["TASK-1".to_string(), "TASK-2".to_string()],
            &files,
            &parents,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].files.len(), 1);
        assert!(out[1].files.is_empty());
        assert!(out[1].parents.is_empty());
    }
}
