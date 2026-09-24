//! Typed review findings: a small controlled vocabulary of defect classes a
//! reviewer can attach to a finding, and the corpus query that counts them.
//!
//! # Why this exists
//!
//! A review finding is free text. Asking "how often does defect class X
//! recur?" of free text means a keyword sweep, and a keyword sweep cannot
//! tell "covers all three sites" (an approval) from "left two of three
//! sites" (the defect): the two share their vocabulary and differ only in
//! what they assert. The only reliable class is the one the reviewer names
//! when recording the finding.
//!
//! So the class is recorded ALONGSIDE the text, never inferred from it:
//!
//!   - `findings`        — the free text, unchanged
//!   - `finding_classes` — a parallel array, one entry per finding, each a
//!     class name or `null` (unclassified)
//!
//! Both fields are optional; every verdict file written before this existed
//! reads as "all findings unclassified". The historical corpus is NOT
//! back-classified — counting starts the day the field exists.
//!
//! A class is never allowed to block recording a verdict: an unknown class
//! is warned about and the finding is recorded unclassified.
//
// trace:STORY-1417 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The defect-class vocabulary: `(name, meaning)`. Additive — a new class is
/// one line here. Names are kebab-case and stable once recorded.
pub const FINDING_CLASSES: &[(&str, &str)] = &[
    (
        "incomplete-fix",
        "fixed the cited site but left named siblings or other call sites",
    ),
    (
        "fail-open",
        "an error, missing input or unknown state is treated as success",
    ),
    (
        "absent-evidence-reads-as-good",
        "the absence of a signal is read as a pass",
    ),
    (
        "untested-path",
        "the changed behavior has no test that would fail without it",
    ),
    (
        "contract-drift",
        "a schema, API, CLI or doc contract changed without its consumers",
    ),
    ("race", "concurrency, ordering or TOCTOU defect"),
    ("perf", "avoidable cost on a hot path"),
    (
        "portability",
        "assumes one OS, shell, filesystem or toolchain",
    ),
    ("stale-base", "reviewed or built against an outdated base"),
    ("scope-creep", "changes outside what the spec asked for"),
];

/// Accepted spellings that map onto a vocabulary member.
const CLASS_ALIASES: &[(&str, &str)] = &[
    ("missing-test", "untested-path"),
    ("untested", "untested-path"),
    ("contract-break", "contract-drift"),
    ("partial-fix", "incomplete-fix"),
    ("performance", "perf"),
    ("toctou", "race"),
];

/// One `--finding-class` argument, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassArg {
    /// A vocabulary member (canonical name).
    Class(&'static str),
    /// An explicit "no class for this finding" (`-` or `none`).
    Skip,
    /// Not in the vocabulary; carries the input as given.
    Unknown(String),
}

/// Resolve a class name: case-insensitive, `_`/space folded to `-`, aliases
/// applied. Never guesses from anything but the name itself.
pub fn parse_class(raw: &str) -> ClassArg {
    let norm = raw.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    if norm.is_empty() || norm == "-" || norm == "none" {
        return ClassArg::Skip;
    }
    if let Some((name, _)) = FINDING_CLASSES.iter().find(|(n, _)| *n == norm) {
        return ClassArg::Class(name);
    }
    if let Some((_, target)) = CLASS_ALIASES.iter().find(|(a, _)| *a == norm) {
        return ClassArg::Class(target);
    }
    ClassArg::Unknown(raw.trim().to_string())
}

/// Pair `--finding-class` values with `--finding` values by position: the
/// i-th class belongs to the i-th finding. Returns one entry per finding plus
/// warnings for anything dropped. Never fails — a bad class costs its class,
/// not the verdict.
pub fn resolve_finding_classes(
    findings: &[String],
    raw_classes: &[String],
) -> (Vec<Option<String>>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out = vec![None; findings.len()];
    for (i, raw) in raw_classes.iter().enumerate() {
        if i >= findings.len() {
            warnings.push(format!(
                "finding class `{}` has no finding to attach to (classes pair with findings by position) — ignored",
                raw.trim()
            ));
            continue;
        }
        match parse_class(raw) {
            ClassArg::Class(c) => out[i] = Some(c.to_string()),
            ClassArg::Skip => {}
            ClassArg::Unknown(u) => warnings.push(format!(
                "unknown finding class `{u}` — finding {} recorded unclassified (known: {})",
                i + 1,
                FINDING_CLASSES
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    (out, warnings)
}

/// Write `finding_classes` onto a verdict object built from `findings`.
/// Mirrors the writer's findings rule: when `findings` is empty the record's
/// existing findings (and their classes) are preserved; otherwise the classes
/// are replaced, aligned to the findings that survive the writer's trim, and
/// omitted entirely when none is classified.
pub fn apply_finding_classes(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    findings: &[String],
    classes: &[Option<String>],
) {
    let kept: Vec<serde_json::Value> = findings
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.trim().is_empty())
        .map(|(i, _)| match classes.get(i).cloned().flatten() {
            Some(c) => serde_json::Value::String(c),
            None => serde_json::Value::Null,
        })
        .collect();
    if kept.is_empty() {
        return;
    }
    obj.remove("finding_classes");
    if kept.iter().any(|v| !v.is_null()) {
        obj.insert(
            "finding_classes".to_string(),
            serde_json::Value::Array(kept),
        );
    }
}

/// The typed-finding view of one recorded round. Every field defaults, so a
/// verdict file from before this schema existed parses as unclassified.
#[derive(Debug, Default, Deserialize)]
struct RoundView {
    #[serde(default)]
    findings: Vec<serde_json::Value>,
    #[serde(default)]
    finding_classes: Vec<Option<String>>,
    #[serde(default)]
    reviewed_sha: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    recorded_by: Option<String>,
    #[serde(default)]
    recorded_at: Option<String>,
    #[serde(default)]
    rounds: Vec<serde_json::Value>,
}

/// Running per-class totals: findings, subjects, first seen, last seen.
type ClassAcc = (usize, BTreeSet<String>, Option<String>, Option<String>);

/// Per-class totals across the corpus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ClassCount {
    pub class: String,
    /// Distinct findings recorded with this class.
    pub findings: usize,
    /// Distinct verdict keys (spec or `PR-<N>`) the class appeared under.
    pub subjects: Vec<String>,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

/// The `aida review classes` report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ClassReport {
    pub classes: Vec<ClassCount>,
    /// Distinct findings carrying a class.
    pub classified: usize,
    /// Distinct findings with no class (including every pre-schema finding).
    pub unclassified: usize,
    /// Classes on disk that are not in the vocabulary (hand-written files).
    pub unknown: BTreeMap<String, usize>,
}

/// Count classified findings across `.aida/review-verdicts/` — every current
/// file, every per-sha archive under `<KEY>/`, and every retained `rounds`
/// entry — counting each act of review once. Rounds recorded before `since`
/// (or with no parsable timestamp when `since` is set) are skipped.
pub fn collect_class_report(
    verdict_dir: &Path,
    since: Option<chrono::DateTime<chrono::Utc>>,
) -> ClassReport {
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    let json_files = |dir: &Path| -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        v.sort();
        v
    };
    for p in json_files(verdict_dir) {
        let key = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        files.push((key, p));
    }
    let mut dirs: Vec<_> = std::fs::read_dir(verdict_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let key = d
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        for p in json_files(&d) {
            files.push((key.clone(), p));
        }
    }

    let mut seen: HashSet<(String, String, String, String)> = HashSet::new();
    let mut acc: BTreeMap<String, ClassAcc> = BTreeMap::new();
    let mut report = ClassReport::default();
    for (key, path) in files {
        let Some(round) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|b| serde_json::from_str::<RoundView>(&b).ok())
        else {
            continue;
        };
        let mut stack = vec![round];
        while let Some(mut r) = stack.pop() {
            for nested in std::mem::take(&mut r.rounds) {
                if let Ok(n) = serde_json::from_value::<RoundView>(nested) {
                    stack.push(n);
                }
            }
            let at = r.recorded_at.clone().unwrap_or_default();
            if let Some(since) = since {
                let in_window = chrono::DateTime::parse_from_rfc3339(at.trim())
                    .is_ok_and(|t| t.with_timezone(&chrono::Utc) >= since);
                if !in_window {
                    continue;
                }
            }
            let sha = r
                .reviewed_sha
                .clone()
                .or(r.head.clone())
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let by = r.recorded_by.clone().unwrap_or_default();
            for (i, f) in r.findings.iter().enumerate() {
                let Some(text) = f.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
                    continue;
                };
                if !seen.insert((sha.clone(), by.clone(), at.clone(), text.to_string())) {
                    continue;
                }
                let class = r
                    .finding_classes
                    .get(i)
                    .cloned()
                    .flatten()
                    .map(|c| parse_class(&c));
                match class {
                    Some(ClassArg::Class(c)) => {
                        report.classified += 1;
                        let e = acc.entry(c.to_string()).or_default();
                        e.0 += 1;
                        e.1.insert(key.clone());
                        let at = (!at.is_empty()).then(|| at.clone());
                        if at.is_some() && (e.2.is_none() || at < e.2) {
                            e.2 = at.clone();
                        }
                        if at.is_some() && at > e.3 {
                            e.3 = at;
                        }
                    }
                    Some(ClassArg::Unknown(u)) => {
                        report.classified += 1;
                        *report.unknown.entry(u).or_default() += 1;
                    }
                    Some(ClassArg::Skip) | None => report.unclassified += 1,
                }
            }
        }
    }
    report.classes = acc
        .into_iter()
        .map(|(class, (n, subjects, first, last))| ClassCount {
            class,
            findings: n,
            subjects: subjects.into_iter().collect(),
            first_seen: first,
            last_seen: last,
        })
        .collect();
    report
        .classes
        .sort_by(|a, b| b.findings.cmp(&a.findings).then(a.class.cmp(&b.class)));
    report
}

#[cfg(test)]
#[path = "tests/story_1417_finding_classes_tests.rs"]
mod story_1417_finding_classes_tests;
