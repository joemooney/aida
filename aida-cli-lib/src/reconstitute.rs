//! TASK-1248 / ADR-44 — `aida reconstitute <SPEC>`: the round-trip verifier
//! (STORY-1178 slice 3, EPIC-70).
//!
//! Question answered: *could a future agent rebuild this spec's behavior from
//! the store alone?* The probe agent gets STORE-ONLY context — the spec, its
//! linked decisions, `[aida:sem]` micro-decisions, graph context and the names
//! of the symbols the spec's trace comments point at — and NO source: it runs
//! in an empty scratch directory outside the project and is told not to read
//! anything else. It regenerates the tests it believes the criteria imply; a
//! second pass judges, per criterion, whether each real traced test has a
//! regenerated counterpart (matched / partial / missing, with a reason).
//!
//! The **divergence report is the product**; the score (matched real traced
//! tests / total) is a heuristic trend line and is always labelled so. Real
//! tests the store could not reproduce become harvest candidates written to
//! `.aida/harvest/<spec>-probe-<id>.json` — confirmation stays opt-in through
//! `aida harvest <SPEC> --from <file>` (C4: the loop closes into the store,
//! never around it). The probe itself never writes to the spec.
// trace:TASK-1248 | ai:claude

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::criteria::{CriteriaReport, TracedTest};
use crate::harvest::{Candidate, CandidateKind};

const SCRATCH_RUN_LIMIT: usize = 20;
const SCRATCH_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Default)]
pub(crate) struct ReconstituteOptions {
    pub(crate) json: bool,
    pub(crate) dry_run: bool,
    // trace:STORY-1425 | ai:claude
    pub(crate) yes: bool,
}

/// One regenerated test as the probe reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RegeneratedTest {
    // Arm A (the baseline) never sees criterion labels, so it may omit this.
    // trace:STORY-1425 | ai:claude
    #[serde(default)]
    pub(crate) criterion: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MatchVerdict {
    Matched,
    Partial,
    Missing,
}

/// The matching pass's answer for one real traced test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TestMatch {
    pub(crate) criterion: String,
    pub(crate) real_test: String,
    pub(crate) verdict: MatchVerdict,
    #[serde(default)]
    pub(crate) reason: String,
}

/// Per criterion: its id, the real traced tests `(name, source)`, and the
/// regenerated tests the matcher compares them against.
pub(crate) type CriterionPair = (String, Vec<(String, String)>, Vec<RegeneratedTest>);

/// Parse the probe's output: `{"tests": [...]}` or a bare array, fenced or not.
pub(crate) fn parse_regenerated(raw: &str) -> Result<Vec<RegeneratedTest>> {
    let body = strip_fence(raw);
    #[derive(Deserialize)]
    struct W {
        tests: Vec<RegeneratedTest>,
    }
    if let Ok(w) = serde_json::from_str::<W>(body) {
        return Ok(w.tests);
    }
    serde_json::from_str(body).context("probe output is not a regenerated-test list")
}

/// Parse the matching pass's output: `{"matches": [...]}` or a bare array.
pub(crate) fn parse_matches(raw: &str) -> Result<Vec<TestMatch>> {
    let body = strip_fence(raw);
    #[derive(Deserialize)]
    struct W {
        matches: Vec<TestMatch>,
    }
    if let Ok(w) = serde_json::from_str::<W>(body) {
        return Ok(w.matches);
    }
    serde_json::from_str(body).context("match output is not a match list")
}

fn strip_fence(raw: &str) -> &str {
    let t = raw.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    rest.trim()
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(rest.trim())
}

/// Heuristic score: matched real traced tests over all real traced tests.
/// `(matched, total)`; a spec with no traced tests scores `(0, 0)`.
pub(crate) fn score(matches: &[TestMatch], real_total: usize) -> (usize, usize) {
    let matched = matches
        .iter()
        .filter(|m| m.verdict == MatchVerdict::Matched)
        .count()
        .min(real_total);
    (matched, real_total)
}

/// C4: every real traced test the store could NOT reproduce (partial or
/// missing) becomes a harvest candidate — an acceptance-criterion proposal
/// carrying the test's name and the matcher's reason, in `aida harvest`'s
/// candidate shape so confirmation stays opt-in.
pub(crate) fn divergence_to_candidates(
    matches: &[TestMatch],
    real: &[TracedTest],
) -> Vec<Candidate> {
    matches
        .iter()
        .filter(|m| m.verdict != MatchVerdict::Matched)
        .map(|m| {
            let path = real
                .iter()
                .find(|t| t.name == m.real_test)
                .map(|t| format!("{}:{}", t.path, t.line))
                .unwrap_or_default();
            Candidate {
                kind: CandidateKind::Ac,
                text: format!(
                    "The behavior verified by test `{}` ({}) is not reproducible from the store: {}",
                    m.real_test,
                    m.criterion,
                    if m.reason.trim().is_empty() {
                        "the regenerated tests do not cover it"
                    } else {
                        m.reason.trim()
                    }
                ),
                rationale: format!(
                    "reconstitution probe verdict = {:?} for {}{}",
                    m.verdict,
                    m.real_test,
                    if path.is_empty() {
                        String::new()
                    } else {
                        format!(" at {path}")
                    }
                ),
                confidence: if m.verdict == MatchVerdict::Missing { 0.8 } else { 0.7 },
                conventional: false,
            }
        })
        .collect()
}

/// The probe brief. Store-only by construction: it carries the spec text,
/// decisions, micro-decisions, graph context and symbol NAMES — never a line
/// of implementation or test source.
pub(crate) fn build_probe_prompt(
    spec: &str,
    store_context: &str,
    criteria_ids: &[String],
    symbols: &[String],
    out_path: &Path,
) -> String {
    let mut p = format!(
        "You are AIDA's reconstitution probe for {spec}.\n\n\
         Question: could a future engineer rebuild this behavior from the requirement store ALONE? \
         Using ONLY the context below — you are in an empty directory; do not read, list or search any file, \
         do not run git, do not fetch anything — write the Rust tests the acceptance criteria imply. \
         One or more tests per criterion. Prefer concrete, observable assertions over prose. \
         If a criterion is too vague to test, still emit a test whose body states the ambiguity in a comment.\n\n\
         Output: JSON to `{}` and nothing else, shape\n\
         {{\"tests\": [{{\"criterion\": \"{spec}.AC1\", \"name\": \"snake_case_test_name\", \"source\": \"fn ... {{ ... }}\"}}]}}\n\n",
        out_path.display()
    );
    p.push_str("## Criteria to cover\n");
    if criteria_ids.is_empty() {
        p.push_str("(the spec has no labeled acceptance criteria — derive the criteria from the description and label them AC1, AC2, …)\n");
    }
    for id in criteria_ids {
        p.push_str(&format!("- {id}\n"));
    }
    if !symbols.is_empty() {
        p.push_str("\n## Symbols the implementation traces to this spec (names only)\n");
        for s in symbols {
            p.push_str(&format!("- {s}\n"));
        }
    }
    p.push_str("\n## Store context\n\n");
    p.push_str(store_context.trim());
    p.push('\n');
    p
}

/// The matching brief: per criterion, the real traced tests and the
/// regenerated ones; the agent judges each real test.
pub(crate) fn build_match_prompt(spec: &str, pairs: &[CriterionPair], out_path: &Path) -> String {
    let mut p = format!(
        "You are AIDA's reconstitution matcher for {spec}.\n\n\
         For EACH real test below, decide whether the regenerated tests for the same criterion verify the same \
         observable behavior: `matched` (same behavior, same essential assertions), `partial` (overlaps but misses \
         an essential assertion or case), `missing` (nothing regenerated covers it). Judge intent, not wording. \
         Give a one-line reason naming what is missing when not matched.\n\n\
         Output: JSON to `{}` and nothing else, shape\n\
         {{\"matches\": [{{\"criterion\": \"...\", \"real_test\": \"<real test fn name>\", \"verdict\": \"matched|partial|missing\", \"reason\": \"...\"}}]}}\n\n",
        out_path.display()
    );
    for (criterion, real, regen) in pairs {
        p.push_str(&format!("## {criterion}\n\n### Real traced tests\n"));
        if real.is_empty() {
            p.push_str("(none)\n");
        }
        for (name, src) in real {
            p.push_str(&format!("#### {name}\n```rust\n{}\n```\n", src.trim_end()));
        }
        p.push_str("\n### Regenerated tests\n");
        if regen.is_empty() {
            p.push_str("(none)\n");
        }
        for t in regen {
            p.push_str(&format!(
                "#### {}\n```rust\n{}\n```\n",
                t.name,
                t.source.trim_end()
            ));
        }
        p.push('\n');
    }
    p
}

// ---------------------------------------------------------------------------
// STORY-1425: the counterfactual arm, the delta, and coverage.
//
// A single store-only score cannot be attributed to AIDA: a high number may
// mean the work was guessable without the store, a low one may mean the target
// is intrinsically hard. So the probe runs twice. Arm B is the store-only probe
// above. Arm A is the SAME probe given only what a project that never used
// AIDA would still have once the code is gone — git log subjects touching the
// relevant paths, the README, and file/symbol names. The per-spec delta
// (B − A) is AIDA's measured contribution. Recall is over the CAPTURED set, so
// coverage (how much was captured at all) is reported alongside it and never
// folded into it. Anything that cannot be computed is reported as unknown
// (PRIN-5), never as zero.
// trace:STORY-1425 | ai:claude

/// What the baseline arm is never given. Stated in every report so the
/// counterfactual is auditable.
pub(crate) const BASELINE_WITHHELD: &[&str] = &[
    "spec title and body",
    "acceptance criteria (text and labels)",
    "linked decisions (ADRs)",
    "[aida:sem] micro-decisions",
    "relationships / graph context",
    "trace comments and SPEC-IDs",
];

const BASELINE_SUBJECT_LIMIT: usize = 40;
const BASELINE_README_CHARS: usize = 12_000;

/// One arm's recall: `matched` is `None` when the arm could not be computed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ArmScore {
    pub(crate) arm: &'static str,
    pub(crate) context: &'static str,
    pub(crate) matched: Option<usize>,
    pub(crate) total: usize,
    pub(crate) pct: Option<f64>,
    pub(crate) regenerated: Option<usize>,
    pub(crate) unknown_reason: Option<String>,
}

impl ArmScore {
    pub(crate) fn known(
        arm: &'static str,
        context: &'static str,
        matches: &[TestMatch],
        total: usize,
        regenerated: usize,
    ) -> Self {
        let (matched, total) = score(matches, total);
        if total == 0 {
            return Self::unknown(arm, context, 0, "no real traced tests to recall");
        }
        Self {
            arm,
            context,
            matched: Some(matched),
            total,
            pct: Some(matched as f64 * 100.0 / total as f64),
            regenerated: Some(regenerated),
            unknown_reason: None,
        }
    }

    pub(crate) fn unknown(
        arm: &'static str,
        context: &'static str,
        total: usize,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            arm,
            context,
            matched: None,
            total,
            pct: None,
            regenerated: None,
            unknown_reason: Some(reason.into()),
        }
    }

    fn render(&self) -> String {
        match (self.matched, self.pct) {
            (Some(m), Some(p)) => format!("{m}/{} ({p:.0}%)", self.total),
            _ => format!(
                "unknown ({})",
                self.unknown_reason.as_deref().unwrap_or("not computed")
            ),
        }
    }
}

/// Marginal lift in percentage points (arm B − arm A). `None` — unknown — the
/// moment either arm is unknown; never a guess.
pub(crate) fn delta_points(arm_a: &ArmScore, arm_b: &ArmScore) -> Option<f64> {
    Some(arm_b.pct? - arm_a.pct?)
}

fn delta_unknown_reason(arm_a: &ArmScore, arm_b: &ArmScore) -> Option<String> {
    if delta_points(arm_a, arm_b).is_some() {
        return None;
    }
    let side = |a: &ArmScore| {
        a.unknown_reason
            .clone()
            .map(|r| format!("arm {} unknown: {r}", a.arm))
    };
    Some(
        [side(arm_a), side(arm_b)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// A coverage ratio; constructed only when the denominator is non-zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Ratio {
    pub(crate) num: usize,
    pub(crate) den: usize,
    pub(crate) pct: f64,
}

impl Ratio {
    pub(crate) fn new(num: usize, den: usize) -> Option<Self> {
        (den > 0).then(|| Self {
            num: num.min(den),
            den,
            pct: num.min(den) as f64 * 100.0 / den as f64,
        })
    }
}

fn render_ratio(r: &Option<Ratio>) -> String {
    match r {
        Some(r) => format!("{}/{} ({:.0}%)", r.num, r.den, r.pct),
        None => "unknown".to_string(),
    }
}

/// How much was captured at all. Each metric stands alone; they are never
/// combined with each other or with recall.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Coverage {
    /// Rust test fns carrying any `trace:` marker / all Rust test fns.
    pub(crate) tests_traced: Option<Ratio>,
    /// Open work specs whose description carries acceptance criteria.
    pub(crate) specs_with_criteria: Option<Ratio>,
    /// Criteria (across those specs) with at least one criterion-traced test.
    pub(crate) criteria_with_traced_test: Option<Ratio>,
    /// Non-merge commits whose subject carries a `(SPEC-ID)` trailer.
    pub(crate) commits_with_spec_trailer: Option<Ratio>,
    pub(crate) note: &'static str,
}

/// Which project the numbers describe, and whether they are a lower bound.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ValidationSubject {
    pub(crate) name: String,
    pub(crate) lower_bound: bool,
    pub(crate) label: String,
}

pub(crate) fn validation_subject(project_root: &Path) -> ValidationSubject {
    let name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| project_root.display().to_string());
    // AIDA's own repo predates its capture discipline, so a run here
    // understates what a project disciplined from its first commit achieves.
    let is_aida_itself = project_root.join("aida-core").join("Cargo.toml").is_file()
        && project_root.join("aida-cli-lib").is_dir();
    let label = if is_aida_itself {
        "lower bound — this is AIDA's own repository, which predates its capture discipline; validate on a project that used AIDA from its first commit".to_string()
    } else {
        "project under test".to_string()
    };
    ValidationSubject {
        name,
        lower_bound: is_aida_itself,
        label,
    }
}

/// Everything arm A was given, so a reader can audit the counterfactual.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct BaselineInputs {
    pub(crate) git_subjects: Vec<String>,
    pub(crate) readme: Option<String>,
    pub(crate) readme_chars: usize,
    pub(crate) files: Vec<String>,
    pub(crate) symbols: Vec<String>,
    pub(crate) withheld: Vec<&'static str>,
    #[serde(skip)]
    pub(crate) readme_text: String,
}

fn spec_id_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static IDS: OnceLock<regex::Regex> = OnceLock::new();
    IDS.get_or_init(|| {
        regex::Regex::new(
            r"\b(?:FR|NFR|SR|UR|CR|BUG|EPIC|STORY|TASK|SPIKE|SPRINT|FOLDER|META|PRIN|VIS|CON|ADR|TERM|DOC)-\d+(?:-\d+)?(?:\.[A-Za-z0-9]+)?\b",
        ).expect("valid regex")
    })
}

/// Strip AIDA-only residue from a README before arm A sees it: whole lines
/// that are trace comments, inline `trace:` markers (with their `| ai:` tail),
/// `[aida:…]` markers, SPEC-IDs and `.aida` paths. Without this the baseline
/// would receive exactly what `BASELINE_WITHHELD` says it is denied.
// trace:STORY-1425 | ai:claude
pub(crate) fn sanitize_readme(text: &str) -> String {
    use std::sync::OnceLock;
    static TRACE: OnceLock<regex::Regex> = OnceLock::new();
    static MARK: OnceLock<regex::Regex> = OnceLock::new();
    static DOT_AIDA: OnceLock<regex::Regex> = OnceLock::new();
    let trace = TRACE.get_or_init(|| {
        regex::Regex::new(r"(?i)trace:\s*[A-Za-z0-9._-]*(?:\s*\|\s*ai:[A-Za-z0-9:+_-]*)?")
            .expect("valid regex")
    });
    let mark = MARK.get_or_init(|| regex::Regex::new(r"(?i)\[aida:[^\]]*\]").expect("valid regex"));
    let dot_aida = DOT_AIDA.get_or_init(|| {
        regex::Regex::new(r"[^\s`'\x22()]*\.aida[^\s`'\x22()]*").expect("valid regex")
    });
    let mut out = String::new();
    for line in text.lines() {
        // A line that is nothing but a trace comment (`// trace:…`,
        // `# trace:…`, `<!-- trace:… -->`) is dropped whole.
        let body = line.trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '/' | '#' | '-' | '<' | '!' | '*' | '`')
        });
        if body.to_ascii_lowercase().starts_with("trace:") {
            continue;
        }
        let l = trace.replace_all(line, "");
        let l = mark.replace_all(&l, "");
        let l = spec_id_regex().replace_all(&l, "");
        let l = dot_aida.replace_all(&l, "");
        out.push_str(l.trim_end());
        out.push('\n');
    }
    out
}

/// Strip AIDA-only residue from a commit subject — the `[AI:tool]` prefix and
/// any SPEC-ID — so the baseline sees what a non-AIDA log would carry.
pub(crate) fn sanitize_subject(subject: &str) -> String {
    use std::sync::OnceLock;
    static AI: OnceLock<regex::Regex> = OnceLock::new();
    static EMPTY: OnceLock<regex::Regex> = OnceLock::new();
    let ai = AI.get_or_init(|| regex::Regex::new(r"\[AI:[^\]]*\]\s*").expect("valid regex"));
    let ids = spec_id_regex();
    let empty =
        EMPTY.get_or_init(|| regex::Regex::new(r"\(\s*[,\s]*\)|\s{2,}").expect("valid regex"));
    let s = ai.replace_all(subject, "");
    let s = ids.replace_all(&s, "");
    let s = empty.replace_all(&s, " ");
    s.trim()
        .trim_end_matches(|c: char| c == '-' || c == ',' || c.is_whitespace())
        .to_string()
}

/// Whether a commit subject carries a `(SPEC-ID)` trailer.
pub(crate) fn has_spec_trailer(subject: &str) -> bool {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\((?:[A-Z][A-Z0-9]*-\d+(?:-\d+)?[,\s]*)+\)\s*$").expect("valid regex")
    })
    .is_match(subject.trim())
}

/// The arm-A brief: identical task and output shape to the store probe, but
/// carrying only what a project without AIDA retains.
pub(crate) fn build_baseline_prompt(inputs: &BaselineInputs, out_path: &Path) -> String {
    let mut p = format!(
        "You are the BASELINE arm of a reconstitution probe.\n\n\
         Question: could a future engineer rebuild this behavior from what an ordinary project keeps — its git history, \
         README and file/symbol names — with no requirements store? \
         Using ONLY the context below — you are in an empty directory; do not read, list or search any file, \
         do not run git, do not fetch anything — write the Rust tests you believe the behavior implemented by these \
         files and symbols should satisfy. Prefer concrete, observable assertions over prose.\n\n\
         Output: JSON to `{}` and nothing else, shape\n\
         {{\"tests\": [{{\"criterion\": \"\", \"name\": \"snake_case_test_name\", \"source\": \"fn ... {{ ... }}\"}}]}}\n\n",
        out_path.display()
    );
    p.push_str("## Files involved (names only)\n");
    if inputs.files.is_empty() {
        p.push_str("(none known)\n");
    }
    for f in &inputs.files {
        p.push_str(&format!("- {f}\n"));
    }
    if !inputs.symbols.is_empty() {
        p.push_str("\n## Symbols (names only)\n");
        for s in &inputs.symbols {
            p.push_str(&format!("- {s}\n"));
        }
    }
    p.push_str("\n## Git log subjects touching those files\n");
    if inputs.git_subjects.is_empty() {
        p.push_str("(none)\n");
    }
    for s in &inputs.git_subjects {
        p.push_str(&format!("- {s}\n"));
    }
    if !inputs.readme_text.trim().is_empty() {
        p.push_str("\n## README\n\n");
        p.push_str(inputs.readme_text.trim());
        p.push('\n');
    }
    p
}

/// Arm A's regenerated tests carry no criterion labels, so the matcher sees
/// the whole baseline set under every criterion.
pub(crate) fn baseline_match_pairs(
    pairs: &[CriterionPair],
    baseline: &[RegeneratedTest],
) -> Vec<CriterionPair> {
    pairs
        .iter()
        .map(|(c, real, _)| (c.clone(), real.clone(), baseline.to_vec()))
        .collect()
}

/// Line-based count of Rust test fns in one file: `(total, traced, markers)`,
/// where `markers` are the trace ids (upper-cased, criterion suffix kept)
/// found on the comment lines directly above a test or inside its body.
pub(crate) fn rust_test_tracing(content: &str) -> (usize, usize, BTreeSet<String>) {
    fn ids(line: &str, out: &mut Vec<String>) {
        let mut rest = line;
        while let Some(pos) = rest.find("trace:") {
            let hit = &rest[pos..];
            let next = hit["trace:".len()..]
                .find("trace:")
                .map(|n| n + "trace:".len())
                .unwrap_or(hit.len());
            if let Some(id) = crate::parse_trace_id_token(&hit[..next]) {
                out.push(id);
            }
            rest = &hit["trace:".len()..];
        }
    }
    fn delta(line: &str) -> isize {
        line.chars().fold(0, |a, c| match c {
            '{' => a + 1,
            '}' => a - 1,
            _ => a,
        })
    }
    let (mut total, mut traced) = (0, 0);
    let mut markers = BTreeSet::new();
    let mut pending_test = false;
    let mut above: Vec<String> = Vec::new();
    let mut current: Option<(isize, Vec<String>, bool)> = None;
    for line in content.lines() {
        let t = line.trim_start();
        if let Some((depth, found, started)) = current.as_mut() {
            ids(line, found);
            *depth += delta(line);
            *started |= line.contains('{');
            if *started && *depth <= 0 {
                let (_, found, _) = current.take().expect("current test");
                total += 1;
                if !found.is_empty() {
                    traced += 1;
                    markers.extend(found);
                }
            }
            continue;
        }
        if t.starts_with("#[test]") || t.starts_with("#[tokio::test") {
            pending_test = true;
        }
        if pending_test
            && (t.starts_with("fn ")
                || t.starts_with("async fn ")
                || t.starts_with("pub fn ")
                || t.starts_with("pub async fn "))
        {
            let mut found = std::mem::take(&mut above);
            ids(line, &mut found);
            let d = delta(line);
            pending_test = false;
            if line.contains('{') && d <= 0 {
                total += 1;
                if !found.is_empty() {
                    traced += 1;
                    markers.extend(found);
                }
            } else {
                current = Some((d, found, line.contains('{')));
            }
            continue;
        }
        if t.starts_with("//") || t.starts_with("#[") {
            ids(line, &mut above);
        } else if !pending_test {
            above.clear();
        }
    }
    (total, traced, markers)
}

fn is_work_spec(r: &aida_core::Requirement) -> bool {
    use aida_core::RequirementType as T;
    !r.archived
        && matches!(
            r.req_type,
            T::Functional
                | T::NonFunctional
                | T::System
                | T::User
                | T::ChangeRequest
                | T::Bug
                | T::Story
                | T::Task
                | T::Spike
        )
        && !matches!(
            r.status,
            aida_core::RequirementStatus::Rejected | aida_core::RequirementStatus::Superseded
        )
}

/// Coverage over already-gathered inputs: `(spec_id, criterion ids)` per work
/// spec, the trace markers seen in tests, test counts and commit subjects
/// (`None` when git history could not be read).
pub(crate) fn coverage_from_parts(
    spec_criteria: &[(String, Vec<String>)],
    test_markers: &BTreeSet<String>,
    tests: (usize, usize),
    subjects: Option<&[String]>,
) -> Coverage {
    let with = spec_criteria.iter().filter(|(_, c)| !c.is_empty()).count();
    let all_criteria: Vec<&String> = spec_criteria.iter().flat_map(|(_, c)| c).collect();
    let traced_criteria = all_criteria
        .iter()
        .filter(|c| test_markers.contains(&c.to_ascii_uppercase()))
        .count();
    Coverage {
        tests_traced: Ratio::new(tests.1, tests.0),
        specs_with_criteria: Ratio::new(with, spec_criteria.len()),
        criteria_with_traced_test: Ratio::new(traced_criteria, all_criteria.len()),
        commits_with_spec_trailer: subjects.and_then(|s| {
            Ratio::new(s.iter().filter(|x| has_spec_trailer(x)).count(), s.len())
        }),
        note: "reported alongside recall; never combined with it or with each other into a single score",
    }
}

fn git_lines(project_root: &Path, args: &[&str]) -> Option<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .filter(|l| !l.trim().is_empty())
            .collect(),
    )
}

fn compute_coverage(project_root: &Path, store: &aida_core::RequirementsStore) -> Coverage {
    let spec_criteria: Vec<(String, Vec<String>)> = store
        .requirements
        .iter()
        .filter(|r| is_work_spec(r))
        .map(|r| {
            let id = r.display_id();
            let criteria = crate::criteria::parse_acceptance_criteria(&id, &r.description)
                .into_iter()
                .map(|c| c.id)
                .collect();
            (id, criteria)
        })
        .collect();
    let mut files = Vec::new();
    crate::criteria::collect_rust_files(project_root, &mut files);
    let (mut total, mut traced) = (0, 0);
    let mut markers = BTreeSet::new();
    for f in files {
        if let Ok(content) = std::fs::read_to_string(&f) {
            let (t, tr, m) = rust_test_tracing(&content);
            total += t;
            traced += tr;
            markers.extend(m);
        }
    }
    let subjects = git_lines(project_root, &["log", "--no-merges", "--format=%s"]);
    coverage_from_parts(
        &spec_criteria,
        &markers,
        (total, traced),
        subjects.as_deref(),
    )
}

fn gather_baseline_inputs(
    project_root: &Path,
    spec: &str,
    real: &[TracedTest],
    symbols: &[String],
) -> BaselineInputs {
    let needle = format!("trace:{}", spec.to_ascii_uppercase());
    let mut files: BTreeSet<String> = real.iter().map(|t| t.path.clone()).collect();
    let mut rs = Vec::new();
    crate::criteria::collect_rust_files(project_root, &mut rs);
    for f in rs {
        let hit = std::fs::read_to_string(&f)
            .is_ok_and(|c| c.lines().any(|l| l.to_ascii_uppercase().contains(&needle)));
        if hit {
            let rel = f
                .strip_prefix(project_root)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(rel);
        }
    }
    let files: Vec<String> = files.into_iter().collect();
    let git_subjects = if files.is_empty() {
        Vec::new()
    } else {
        let limit = format!("-n{BASELINE_SUBJECT_LIMIT}");
        let mut args = vec!["log", "--no-merges", "--format=%s", limit.as_str(), "--"];
        args.extend(files.iter().map(String::as_str));
        git_lines(project_root, &args)
            .unwrap_or_default()
            .iter()
            .map(|s| sanitize_subject(s))
            .filter(|s| !s.is_empty())
            .collect()
    };
    let (readme, readme_text) = ["README.md", "README", "README.rst", "README.txt"]
        .iter()
        .find_map(|n| {
            std::fs::read_to_string(project_root.join(n))
                .ok()
                .map(|t| (n.to_string(), t))
        })
        .map(|(n, t)| {
            let cut: String = sanitize_readme(&t)
                .chars()
                .take(BASELINE_README_CHARS)
                .collect();
            (Some(n), cut)
        })
        .unwrap_or((None, String::new()));
    BaselineInputs {
        git_subjects,
        readme,
        readme_chars: readme_text.chars().count(),
        files,
        symbols: symbols.to_vec(),
        withheld: BASELINE_WITHHELD.to_vec(),
        readme_text,
    }
}

fn aida_stdout(project_root: &Path, args: &[&str]) -> String {
    std::process::Command::new(crate::aida_exe_path())
        .current_dir(project_root)
        .args(args)
        .env("AIDA_OUTPUT_FORMAT", "human")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Assemble the store-only context for the probe.
fn store_only_context(
    project_root: &Path,
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
    display: &str,
) -> String {
    let mut ctx = String::new();
    let shown = aida_stdout(project_root, &["show", display, "--full", "--no-git"]);
    ctx.push_str("### Requirement (aida show --full)\n");
    ctx.push_str(if shown.trim().is_empty() {
        &req.description
    } else {
        &shown
    });
    ctx.push('\n');
    let decisions: Vec<&aida_core::Requirement> = req
        .relationships
        .iter()
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .chain(
            store
                .requirements
                .iter()
                .filter(|r| r.relationships.iter().any(|rel| rel.target_id == req.id)),
        )
        .filter(|r| r.req_type == aida_core::RequirementType::Decision)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for d in decisions {
        if !seen.insert(d.id) {
            continue;
        }
        ctx.push_str(&format!(
            "\n### Decision {} — {}\n{}\n",
            d.display_id(),
            d.title,
            d.description.trim()
        ));
    }
    let sems: Vec<&str> = req
        .comments
        .iter()
        .filter(|c| {
            c.content
                .trim_start()
                .starts_with(crate::harvest::SEM_MARKER)
        })
        .map(|c| c.content.as_str())
        .collect();
    if !sems.is_empty() {
        ctx.push_str("\n### Micro-decisions ([aida:sem])\n");
        for s in sems {
            ctx.push_str(s.trim());
            ctx.push_str("\n\n");
        }
    }
    let graph = aida_stdout(project_root, &["graph", "tree", display]);
    if !graph.trim().is_empty() {
        ctx.push_str("\n### Graph context (aida graph tree)\n");
        ctx.push_str(graph.trim());
        ctx.push('\n');
    }
    ctx
}

fn run_headless(
    project_root: &Path,
    cwd: &Path,
    prompt: &str,
    label: &str,
    log_path: &Path,
) -> Result<()> {
    let vendor = crate::session::resolve_headless_vendor(project_root);
    let tee =
        crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label(label.to_string());
    let session_id = uuid::Uuid::now_v7().to_string();
    let previous_dir = std::env::current_dir().ok();
    let previous_role = std::env::var_os("AIDA_SESSION_ROLE");
    // ADR-44: the probe runs INSIDE an empty directory outside the project so
    // the headless launcher's cwd (and any file the agent might reach for)
    // is that scratch dir, not the repo.
    std::env::set_current_dir(cwd)
        .with_context(|| format!("could not enter scratch dir {}", cwd.display()))?;
    std::env::set_var("AIDA_SESSION_ROLE", "advisor");
    let status =
        crate::session::spawn_vendor_headless(vendor, prompt, &session_id, log_path, &tee, false);
    match previous_role {
        Some(v) => std::env::set_var("AIDA_SESSION_ROLE", v),
        None => std::env::remove_var("AIDA_SESSION_ROLE"),
    }
    if let Some(d) = previous_dir {
        let _ = std::env::set_current_dir(d);
    }
    let status = status?;
    if !status.success() {
        anyhow::bail!(
            "the {label} agent exited with {} — see {}",
            status.code().unwrap_or(1),
            log_path.display()
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct Report {
    spec: String,
    // STORY-1425: the result is the arm pair plus the delta. `score_*` below
    // are kept for existing consumers and are arm B's numbers.
    // trace:STORY-1425 | ai:claude
    arm_a: ArmScore,
    arm_b: ArmScore,
    delta_pct_points: Option<f64>,
    delta_unknown_reason: Option<String>,
    baseline_inputs: BaselineInputs,
    baseline_brief: Option<String>,
    baseline_matches: Vec<TestMatch>,
    coverage: Coverage,
    validation_subject: ValidationSubject,
    score_matched: Option<usize>,
    score_total: usize,
    score_label: &'static str,
    regenerated: usize,
    matches: Vec<TestMatch>,
    untested_criteria: Vec<String>,
    unanchored_tests: Vec<String>,
    candidates_file: Option<String>,
    scratch_dir: String,
}

/// Upper bound on headless agent runs for one probe: with no real traced
/// tests only the store probe runs; otherwise probe + matcher for each arm.
// trace:STORY-1425 | ai:claude
pub(crate) fn planned_agent_runs(real_traced_tests: usize) -> usize {
    if real_traced_tests == 0 {
        1
    } else {
        4
    }
}

/// Unattended (non-TTY) runs must opt in with --yes before spending agents.
// trace:STORY-1425 | ai:claude
pub(crate) fn cost_guard(yes: bool, stdin_is_tty: bool, planned: usize) -> Result<()> {
    if yes || stdin_is_tty {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to launch {planned} headless agent run(s) without a terminal; pass --yes to confirm, or --dry-run to inspect the briefs"
    )
}

/// Per criterion: real traced tests (name, source) vs the regenerated tests
/// labelled with that criterion.
fn criterion_pairs(
    project_root: &Path,
    report: &CriteriaReport,
    regenerated: &[RegeneratedTest],
) -> Vec<CriterionPair> {
    report
        .criteria
        .iter()
        .map(|row| {
            let real = row
                .tests
                .iter()
                .map(|t| {
                    (
                        t.name.clone(),
                        crate::criteria::extract_fn_source(&project_root.join(&t.path), t.line),
                    )
                })
                .collect();
            let regen = regenerated
                .iter()
                .filter(|r| r.criterion.eq_ignore_ascii_case(&row.criterion.id))
                .cloned()
                .collect();
            (row.criterion.id.clone(), real, regen)
        })
        .collect()
}

/// Run arm B: the store-only probe, then the matcher. Returns
/// `(regenerated, pairs, matches)`.
#[allow(clippy::too_many_arguments)]
fn run_store_arm(
    project_root: &Path,
    scratch: &Path,
    prompt: &str,
    probe_out: &Path,
    display: &str,
    report: &CriteriaReport,
    real_tests: &[TracedTest],
    logs: &Path,
    stamp: &str,
) -> Result<(Vec<RegeneratedTest>, Vec<CriterionPair>, Vec<TestMatch>)> {
    let lower = display.to_ascii_lowercase();
    eprintln!(
        "  {} probing {} from the store alone (empty scratch dir, no source)…",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        display
    );
    run_headless(
        project_root,
        scratch,
        prompt,
        &format!("probe-{lower}"),
        &logs.join(format!("probe-{lower}-{stamp}.jsonl")),
    )?;
    let regenerated = parse_regenerated(
        &std::fs::read_to_string(probe_out)
            .with_context(|| format!("the probe wrote no output at {}", probe_out.display()))?,
    )?;
    let pairs = criterion_pairs(project_root, report, &regenerated);
    let match_out = scratch.join("matches.json");
    let matches =
        if real_tests.is_empty() {
            Vec::new()
        } else {
            eprintln!(
                "  {} judging {} real traced test(s) against {} regenerated…",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                real_tests.len(),
                regenerated.len()
            );
            run_headless(
                project_root,
                scratch,
                &build_match_prompt(display, &pairs, &match_out),
                &format!("match-{lower}"),
                &logs.join(format!("match-{lower}-{stamp}.jsonl")),
            )?;
            parse_matches(&std::fs::read_to_string(&match_out).with_context(|| {
                format!("the matcher wrote no output at {}", match_out.display())
            })?)?
        };
    Ok((regenerated, pairs, matches))
}

/// Run arm A: the baseline probe in its own empty dir, then the matcher (in
/// the run dir) against the same real traced tests arm B was judged on.
/// Returns `(matches, regenerated count)`.
// trace:STORY-1425 | ai:claude
#[allow(clippy::too_many_arguments)]
fn run_baseline_arm(
    project_root: &Path,
    scratch: &Path,
    baseline_scratch: &Path,
    prompt: &str,
    out: &Path,
    display: &str,
    pairs: &[CriterionPair],
    logs: &Path,
    stamp: &str,
) -> Result<(Vec<TestMatch>, usize)> {
    let lower = display.to_ascii_lowercase();
    eprintln!(
        "  {} probing {} from a no-store baseline (git subjects, README, names only)…",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        display
    );
    run_headless(
        project_root,
        baseline_scratch,
        prompt,
        &format!("baseline-{lower}"),
        &logs.join(format!("baseline-{lower}-{stamp}.jsonl")),
    )?;
    let regenerated =
        parse_regenerated(&std::fs::read_to_string(out).with_context(|| {
            format!("the baseline probe wrote no output at {}", out.display())
        })?)?;
    if regenerated.is_empty() {
        // Nothing regenerated: every real test is missing — a known zero.
        let ms = pairs
            .iter()
            .flat_map(|(c, real, _)| {
                real.iter().map(move |(name, _)| TestMatch {
                    criterion: c.clone(),
                    real_test: name.clone(),
                    verdict: MatchVerdict::Missing,
                    reason: "the baseline regenerated no tests".into(),
                })
            })
            .collect();
        return Ok((ms, 0));
    }
    let match_out = scratch.join("baseline-matches.json");
    run_headless(
        project_root,
        scratch,
        &build_match_prompt(
            display,
            &baseline_match_pairs(pairs, &regenerated),
            &match_out,
        ),
        &format!("baseline-match-{lower}"),
        &logs.join(format!("baseline-match-{lower}-{stamp}.jsonl")),
    )?;
    let ms = parse_matches(&std::fs::read_to_string(&match_out).with_context(|| {
        format!(
            "the baseline matcher wrote no output at {}",
            match_out.display()
        )
    })?)?;
    Ok((ms, regenerated.len()))
}

pub(crate) fn handle_reconstitute_command(
    project_root: &Path,
    store: &aida_core::RequirementsStore,
    spec: &str,
    opts: ReconstituteOptions,
) -> Result<()> {
    let req = store
        .get_requirement_by_spec_id(spec)
        .or_else(|| {
            store.requirements.iter().find(|r| {
                r.agreed_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(spec))
                    || r.id.to_string().eq_ignore_ascii_case(spec)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("requirement not found: {spec}"))?;
    let display = req.display_id();

    let report: CriteriaReport =
        crate::criteria::build_criteria_report(project_root, &display, &req.description)?;
    let criteria_ids: Vec<String> = report
        .criteria
        .iter()
        .map(|r| r.criterion.id.clone())
        .collect();
    let real_tests: Vec<TracedTest> = report
        .criteria
        .iter()
        .flat_map(|r| r.tests.iter().cloned())
        .collect();
    let symbols = crate::criteria::symbols_traced_to_spec(project_root, &display);
    let context = store_only_context(project_root, store, req, &display);

    // Scratch dir OUTSIDE the project (so find_project_root cannot climb back in).
    // trace:TASK-1255 | ai:codex
    let scratch_root = dirs_home().join(".aida").join("reconstitute");
    std::fs::create_dir_all(&scratch_root)?;
    // Leave room for this run so a successful probe never leaves more than the
    // configured machine-wide limit behind.
    prune_scratch_runs(
        &scratch_root,
        SCRATCH_RUN_LIMIT.saturating_sub(1),
        SCRATCH_MAX_AGE,
        SystemTime::now(),
    )?;
    let run_id = uuid::Uuid::now_v7().to_string();
    let scratch = scratch_root.join(format!("{}-{run_id}", display.to_ascii_lowercase()));
    std::fs::create_dir_all(&scratch)?;
    let probe_out = scratch.join("regenerated.json");
    let prompt = build_probe_prompt(&display, &context, &criteria_ids, &symbols, &probe_out);
    // STORY-1425: arm A — the counterfactual baseline — runs in its OWN empty
    // scratch dir so it cannot see arm B's output (and vice versa).
    // trace:STORY-1425 | ai:claude
    let baseline = gather_baseline_inputs(project_root, &display, &real_tests, &symbols);
    // No spec id in arm A's path: the brief names its output file, and the
    // id is part of what the baseline is denied.
    let baseline_scratch = scratch_root.join(format!("baseline-{run_id}"));
    std::fs::create_dir_all(&baseline_scratch)?;
    let baseline_out = baseline_scratch.join("regenerated.json");
    let baseline_prompt = build_baseline_prompt(&baseline, &baseline_out);
    // Kept for audit in arm A's own dir (it is arm A's own prompt, so it
    // leaks nothing), never in arm B's dir.
    let baseline_brief = baseline_scratch.join("baseline-brief.md");
    std::fs::write(&baseline_brief, &baseline_prompt)?;
    if opts.dry_run {
        println!(
            "dry run — not launching. {} real traced test(s), {} criteria, {} symbol(s).\n\nArm B (store) probe brief:\n\n{prompt}\n\nArm A (baseline, no store) probe brief:\n\n{baseline_prompt}",
            real_tests.len(),
            criteria_ids.len(),
            symbols.len()
        );
        return Ok(());
    }
    // STORY-1425: cost guard — say how many headless agents will run, and
    // refuse to spend them unattended without an explicit --yes.
    // trace:STORY-1425 | ai:claude
    let planned = planned_agent_runs(real_tests.len());
    eprintln!(
        "  {} this probe will launch up to {planned} headless agent run(s) (store probe + matcher, baseline probe + matcher)",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
    );
    {
        use std::io::IsTerminal;
        cost_guard(opts.yes, std::io::stdin().is_terminal(), planned)?;
    }
    let logs = project_root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&logs)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    // Arm B never aborts the report either: a failed store probe or matcher is
    // reported as unknown (PRIN-5), and so is the delta.
    let arm_b_run = run_store_arm(
        project_root,
        &scratch,
        &prompt,
        &probe_out,
        &display,
        &report,
        &real_tests,
        &logs,
        &stamp,
    );
    let (regenerated, pairs, matches, arm_b_error) = match arm_b_run {
        Ok((regen, pairs, ms)) => (regen, pairs, ms, None),
        Err(e) => (
            Vec::new(),
            criterion_pairs(project_root, &report, &[]),
            Vec::new(),
            Some(format!("{e:#}")),
        ),
    };

    let (_, total) = score(&matches, real_tests.len());
    let arm_b = match &arm_b_error {
        _ if real_tests.is_empty() => {
            ArmScore::unknown("B", "store", 0, "no real traced tests to recall")
        }
        Some(e) => ArmScore::unknown("B", "store", real_tests.len(), e.clone()),
        None => ArmScore::known("B", "store", &matches, total, regenerated.len()),
    };
    // Arm A never blocks the report: a failed baseline run is reported as
    // unknown (PRIN-5), and so is the delta. It is skipped when arm B failed,
    // because the delta is unknown either way and the runs cost money.
    let (arm_a, baseline_matches) = if real_tests.is_empty() {
        (
            ArmScore::unknown(
                "A",
                "baseline (no store)",
                0,
                "no real traced tests to recall",
            ),
            Vec::new(),
        )
    } else if arm_b_error.is_some() {
        (
            ArmScore::unknown(
                "A",
                "baseline (no store)",
                total,
                "not run: arm B failed, so the delta is unknown",
            ),
            Vec::new(),
        )
    } else {
        match run_baseline_arm(
            project_root,
            &scratch,
            &baseline_scratch,
            &baseline_prompt,
            &baseline_out,
            &display,
            &pairs,
            &logs,
            &stamp,
        ) {
            Ok((ms, regen)) => (
                ArmScore::known("A", "baseline (no store)", &ms, total, regen),
                ms,
            ),
            Err(e) => (
                ArmScore::unknown("A", "baseline (no store)", total, format!("{e:#}")),
                Vec::new(),
            ),
        }
    };
    let delta = delta_points(&arm_a, &arm_b);
    let delta_reason = delta_unknown_reason(&arm_a, &arm_b);
    let coverage = compute_coverage(project_root, store);
    let subject = validation_subject(project_root);
    let candidates = divergence_to_candidates(&matches, &real_tests);
    let candidates_file = if candidates.is_empty() {
        None
    } else {
        let dir = project_root.join(".aida").join("harvest");
        std::fs::create_dir_all(&dir)?;
        let f = dir.join(format!(
            "{}-probe-{run_id}.json",
            display.to_ascii_lowercase()
        ));
        std::fs::write(
            &f,
            serde_json::to_string_pretty(&serde_json::json!({ "candidates": candidates }))?,
        )?;
        Some(f)
    };

    let out = Report {
        spec: display.clone(),
        arm_a: arm_a.clone(),
        arm_b: arm_b.clone(),
        delta_pct_points: delta,
        delta_unknown_reason: delta_reason.clone(),
        baseline_inputs: baseline.clone(),
        baseline_brief: Some(baseline_brief.display().to_string()),
        baseline_matches: baseline_matches.clone(),
        coverage: coverage.clone(),
        validation_subject: subject.clone(),
        score_matched: arm_b.matched,
        score_total: total,
        score_label: "heuristic",
        regenerated: regenerated.len(),
        matches: matches.clone(),
        untested_criteria: report.untested.clone(),
        unanchored_tests: report.unanchored.iter().map(|t| t.name.clone()).collect(),
        candidates_file: candidates_file.as_ref().map(|f| f.display().to_string()),
        scratch_dir: scratch.display().to_string(),
    };
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("\nReconstitution report for {}\n", display.bold());
    for (criterion, real, regen) in &pairs {
        println!(
            "  {}  ({} real, {} regenerated)",
            criterion.cyan(),
            real.len(),
            regen.len()
        );
        for m in matches.iter().filter(|m| &m.criterion == criterion) {
            let glyph = match m.verdict {
                MatchVerdict::Matched => "✓".green().to_string(),
                MatchVerdict::Partial => "~".yellow().to_string(),
                MatchVerdict::Missing => "✗".red().to_string(),
            };
            let base = baseline_matches
                .iter()
                .find(|b| b.real_test == m.real_test)
                .map(|b| format!(" [baseline: {:?}]", b.verdict).to_lowercase())
                .unwrap_or_default();
            println!(
                "    {glyph} {} — {}{}",
                m.real_test,
                m.reason.trim(),
                base.dimmed()
            );
        }
        if real.is_empty() {
            println!(
                "    {} no traced tests — criterion is untested",
                "·".dimmed()
            );
        }
    }
    if !report.unanchored.is_empty() {
        println!("\n  Unanchored tests (trace a bare/unknown criterion):");
        for t in &report.unanchored {
            println!("    - {} ({}:{})", t.name, t.path, t.line);
        }
    }
    // STORY-1425: the result is the pair and the delta — never arm B alone.
    println!(
        "\n  Recall ({}) — real traced tests reproduced:",
        "heuristic".yellow()
    );
    println!("    Arm A  baseline, no store : {}", arm_a.render());
    println!("    Arm B  store              : {}", arm_b.render());
    match delta {
        Some(d) => println!(
            "    Delta  B − A (AIDA's lift): {}",
            format!("{d:+.0} pts").bold()
        ),
        None => println!(
            "    Delta  B − A (AIDA's lift): unknown ({})",
            delta_reason.as_deref().unwrap_or("not computed")
        ),
    }
    println!(
        "\n  Baseline given: {} git subject(s), README: {}, {} file name(s), {} symbol name(s)",
        baseline.git_subjects.len(),
        baseline
            .readme
            .as_deref()
            .map(|r| format!("{r} ({} chars)", baseline.readme_chars))
            .unwrap_or_else(|| "none".to_string()),
        baseline.files.len(),
        baseline.symbols.len()
    );
    println!("  Baseline withheld: {}", BASELINE_WITHHELD.join(", "));
    println!("  Baseline brief: {}", baseline_brief.display());
    println!("\n  Coverage (separate from recall; never combined):");
    println!(
        "    Rust tests carrying a trace      : {}",
        render_ratio(&coverage.tests_traced)
    );
    println!(
        "    Work specs with acceptance       : {}",
        render_ratio(&coverage.specs_with_criteria)
    );
    println!(
        "    Criteria with a traced test      : {}",
        render_ratio(&coverage.criteria_with_traced_test)
    );
    println!(
        "    Commits with a SPEC-ID trailer   : {}",
        render_ratio(&coverage.commits_with_spec_trailer)
    );
    println!(
        "\n  Validation subject: {} — {}",
        subject.name.bold(),
        if subject.lower_bound {
            subject.label.yellow().to_string()
        } else {
            subject.label.clone()
        }
    );
    match &candidates_file {
        Some(f) => println!(
            "\n  {} {} divergence(s) written as harvest candidates → confirm with:\n    aida harvest {} --from {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            candidates.len(),
            display,
            f.display()
        ),
        None if arm_b.matched.is_some() => println!(
            "\n  {} every real traced test was reproducible — nothing to harvest",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        None if real_tests.is_empty() => println!(
            "\n  {} no real traced tests yet — run `aida criteria {}` and trace tests to criteria first",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            display
        ),
        None => println!(
            "\n  {} the store arm could not be computed, so there is nothing to harvest",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!("  scratch: {}", scratch.display());
    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// Remove expired scratch runs, then retain at most `keep` of the newest
/// remaining directories. Files and symlinks in the root are left untouched.
fn prune_scratch_runs(root: &Path, keep: usize, max_age: Duration, now: SystemTime) -> Result<()> {
    let mut runs = Vec::new();
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("failed to inspect scratch root {}", root.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            runs.push((metadata.modified().unwrap_or(UNIX_EPOCH), entry.path()));
        }
    }
    runs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

    for (index, (modified, path)) in runs.into_iter().enumerate() {
        let expired = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if expired || index >= keep {
            std::fs::remove_dir_all(&path).with_context(|| {
                format!("failed to remove stale scratch run {}", path.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:BUG-1233 | ai:codex
    fn set_dir_modified(path: &Path, modified: SystemTime) {
        #[cfg(windows)]
        let directory = {
            use std::os::windows::fs::OpenOptionsExt;

            // Round 3 (reviewer): backup semantics lets a DIRECTORY be opened,
            // but a read-only handle lacks the access SetFileTime needs
            // (OS error 5). Ask for FILE_WRITE_ATTRIBUTES explicitly — the
            // one right set_times requires — instead of GENERIC_READ.
            // trace:BUG-1233 | ai:claude
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
            std::fs::OpenOptions::new()
                .access_mode(FILE_WRITE_ATTRIBUTES)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
                .unwrap()
        };
        #[cfg(not(windows))]
        let directory = std::fs::File::open(path).unwrap();

        directory
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    // trace:BUG-1233 | ai:codex
    fn windows_directory_handle_can_write_modified_time() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("timestamp-target");
        std::fs::create_dir(&directory).unwrap();
        let expected = SystemTime::now() - Duration::from_secs(60 * 60);

        set_dir_modified(&directory, expected);

        let actual = std::fs::metadata(&directory).unwrap().modified().unwrap();
        let drift = actual
            .duration_since(expected)
            .unwrap_or_else(|error| error.duration());
        assert!(
            drift < Duration::from_secs(2),
            "directory mtime differs from the requested value by {drift:?}"
        );
    }

    fn m(c: &str, t: &str, v: MatchVerdict, r: &str) -> TestMatch {
        TestMatch {
            criterion: c.into(),
            real_test: t.into(),
            verdict: v,
            reason: r.into(),
        }
    }

    // Named in the STORY-1178 plan: the isolation guard.
    #[test]
    fn probe_prompt_contains_no_test_source() {
        let real_test_source = "#[test]\nfn merge_lease_conflict_hint_names_merge_lock_and_pr_ship() {\n    assert!(hint.contains(\"aida merge-lock\"));\n}";
        let context =
            "### Requirement\nDrain merge-lease: shelve as lease-conflict on contention.\n";
        let p = build_probe_prompt(
            "TASK-1244",
            context,
            &["TASK-1244.AC1".into()],
            &["fn drain_merge_lease_failure".into()],
            Path::new("regenerated.json"),
        );
        assert!(!p.contains("#[test]"), "no test attributes in the brief");
        assert!(!p.contains("assert!("), "no assertion source in the brief");
        assert!(!p.contains(real_test_source));
        assert!(p.contains("TASK-1244.AC1") && p.contains("fn drain_merge_lease_failure"));
        assert!(p.contains("do not read, list or search any file"));
        assert!(p.contains("regenerated.json"));
    }

    // Named in the plan: deterministic on a fixture match table.
    #[test]
    fn score_is_matched_over_total_real_traced() {
        let ms = [
            m("S.AC1", "a", MatchVerdict::Matched, ""),
            m(
                "S.AC1",
                "b",
                MatchVerdict::Partial,
                "misses the timeout case",
            ),
            m("S.AC2", "c", MatchVerdict::Missing, "no regenerated test"),
            m("S.AC2", "d", MatchVerdict::Matched, ""),
        ];
        assert_eq!(score(&ms, 4), (2, 4));
        assert_eq!(score(&[], 0), (0, 0));
        // Matched can never exceed the real total even if the matcher over-reports.
        assert_eq!(score(&ms, 1), (1, 1));
    }

    // Named in the plan: C4 — divergences feed the harvest gate.
    #[test]
    fn divergence_feeds_harvest_candidates() {
        let real = vec![TracedTest {
            name: "b".into(),
            path: "aida-cli-lib/src/x.rs".into(),
            line: 12,
            traces: vec!["S.AC1".into()],
        }];
        let ms = [
            m("S.AC1", "a", MatchVerdict::Matched, ""),
            m(
                "S.AC1",
                "b",
                MatchVerdict::Partial,
                "misses the timeout case",
            ),
            m("S.AC2", "c", MatchVerdict::Missing, ""),
        ];
        let cands = divergence_to_candidates(&ms, &real);
        assert_eq!(cands.len(), 2, "only non-matched tests become candidates");
        assert!(cands.iter().all(|c| c.kind == CandidateKind::Ac));
        assert!(cands[0].text.contains("`b`") && cands[0].text.contains("misses the timeout case"));
        assert!(cands[0].rationale.contains("aida-cli-lib/src/x.rs:12"));
        assert!(cands[1].text.contains("do not cover it"));
        assert!(
            cands[1].confidence > cands[0].confidence,
            "missing outranks partial"
        );
        // Round-trips through the harvest candidate parser.
        let json = serde_json::to_string(&serde_json::json!({ "candidates": cands })).unwrap();
        assert_eq!(crate::harvest::parse_candidates(&json).unwrap().len(), 2);
    }

    #[test]
    fn parses_probe_and_match_outputs_fenced_or_bare() {
        let t = r#"{"criterion":"S.AC1","name":"t1","source":"fn t1() {}"}"#;
        assert_eq!(
            parse_regenerated(&format!("{{\"tests\":[{t}]}}"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse_regenerated(&format!("```json\n[{t}]\n```"))
                .unwrap()
                .len(),
            1
        );
        let mm = r#"{"criterion":"S.AC1","real_test":"a","verdict":"partial","reason":"x"}"#;
        let parsed = parse_matches(&format!("{{\"matches\":[{mm}]}}")).unwrap();
        assert_eq!(parsed[0].verdict, MatchVerdict::Partial);
        assert!(parse_matches("nope").is_err());
    }

    #[test]
    fn match_prompt_shows_real_and_regenerated_per_criterion() {
        let pairs = vec![(
            "S.AC1".to_string(),
            vec![(
                "real_a".to_string(),
                "fn real_a() { assert!(x); }".to_string(),
            )],
            vec![RegeneratedTest {
                criterion: "S.AC1".into(),
                name: "regen_a".into(),
                source: "fn regen_a() { assert!(y); }".into(),
            }],
        )];
        let p = build_match_prompt("S", &pairs, Path::new("matches.json"));
        assert!(p.contains("## S.AC1") && p.contains("#### real_a") && p.contains("#### regen_a"));
        assert!(p.contains("matched|partial|missing"));
    }

    #[test]
    fn scratch_gc_expires_old_runs_and_keeps_only_the_newest() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        for index in 0..5 {
            let run = root.path().join(format!("run-{index}"));
            std::fs::create_dir(&run).unwrap();
            let modified = now - Duration::from_secs((index as u64 + 1) * 60);
            set_dir_modified(&run, modified);
        }
        let expired = root.path().join("expired");
        std::fs::create_dir(&expired).unwrap();
        set_dir_modified(&expired, now - SCRATCH_MAX_AGE - Duration::from_secs(1));
        let marker = root.path().join("leave-me.txt");
        std::fs::write(&marker, "not a run directory").unwrap();

        prune_scratch_runs(root.path(), 3, SCRATCH_MAX_AGE, now).unwrap();

        let mut remaining = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        remaining.sort();
        assert_eq!(remaining, ["run-0", "run-1", "run-2"]);
        assert!(
            marker.exists(),
            "non-directory entries are not scratch runs"
        );
    }

    // --- STORY-1425: counterfactual arm, delta, coverage ---------------------

    // trace:STORY-1425 | ai:claude
    #[test]
    fn delta_is_arm_b_minus_arm_a_per_spec() {
        let b = [
            m("S.AC1", "a", MatchVerdict::Matched, ""),
            m("S.AC1", "b", MatchVerdict::Matched, ""),
            m("S.AC2", "c", MatchVerdict::Matched, ""),
            m("S.AC2", "d", MatchVerdict::Missing, ""),
        ];
        let a = [
            m("S.AC1", "a", MatchVerdict::Matched, ""),
            m("S.AC1", "b", MatchVerdict::Partial, ""),
            m("S.AC2", "c", MatchVerdict::Missing, ""),
            m("S.AC2", "d", MatchVerdict::Missing, ""),
        ];
        let arm_b = ArmScore::known("B", "store", &b, 4, 5);
        let arm_a = ArmScore::known("A", "baseline (no store)", &a, 4, 3);
        assert_eq!(arm_b.matched, Some(3));
        assert_eq!(arm_a.matched, Some(1));
        assert_eq!(delta_points(&arm_a, &arm_b), Some(50.0));
        assert!(delta_unknown_reason(&arm_a, &arm_b).is_none());
        // A baseline that out-recalls the store is a NEGATIVE lift, not clamped.
        assert_eq!(delta_points(&arm_b, &arm_a), Some(-50.0));
    }

    // PRIN-5: an uncomputable baseline is unknown, and so is the delta — never 0.
    // trace:STORY-1425 | ai:claude
    #[test]
    fn unknown_baseline_makes_delta_unknown_not_zero() {
        let arm_b = ArmScore::known(
            "B",
            "store",
            &[m("S.AC1", "a", MatchVerdict::Matched, "")],
            1,
            1,
        );
        let arm_a = ArmScore::unknown(
            "A",
            "baseline (no store)",
            1,
            "the baseline agent exited with 1",
        );
        assert_eq!(arm_a.matched, None);
        assert_eq!(arm_a.pct, None);
        assert_eq!(delta_points(&arm_a, &arm_b), None);
        let why = delta_unknown_reason(&arm_a, &arm_b).unwrap();
        assert!(
            why.contains("arm A unknown") && why.contains("exited with 1"),
            "{why}"
        );
        assert!(arm_a.render().starts_with("unknown"));
        // No traced tests at all: both arms unknown, no 0/0 = 0% score.
        let empty = ArmScore::known("B", "store", &[], 0, 0);
        assert_eq!(empty.pct, None);
        assert!(empty.unknown_reason.is_some());
    }

    // Arm A's context is auditable, and carries none of the store.
    // trace:STORY-1425 | ai:claude
    #[test]
    fn baseline_prompt_carries_only_non_aida_context() {
        let inputs = BaselineInputs {
            git_subjects: vec![sanitize_subject(
                "[AI:claude] feat(drain): shelve on lease contention (TASK-1244)",
            )],
            readme: Some("README.md".into()),
            readme_chars: 14,
            files: vec!["aida-cli-lib/src/drain.rs".into()],
            symbols: vec!["fn drain_merge_lease_failure".into()],
            withheld: BASELINE_WITHHELD.to_vec(),
            readme_text: "# Tool\nIt drains.".into(),
        };
        let p = build_baseline_prompt(&inputs, Path::new("regenerated.json"));
        assert!(p.contains("feat(drain): shelve on lease contention"));
        assert!(
            p.contains("aida-cli-lib/src/drain.rs") && p.contains("fn drain_merge_lease_failure")
        );
        assert!(p.contains("It drains."));
        assert!(p.contains("do not read, list or search any file"));
        for leaked in [
            "TASK-1244",
            "[AI:",
            "Acceptance",
            "Decision",
            "[aida:sem]",
            ".AC",
        ] {
            assert!(!p.contains(leaked), "baseline brief leaked {leaked}:\n{p}");
        }
        // The report states exactly what was withheld.
        let json = serde_json::to_value(&inputs).unwrap();
        assert_eq!(
            json["withheld"].as_array().unwrap().len(),
            BASELINE_WITHHELD.len()
        );
        assert!(
            json.get("readme_text").is_none(),
            "README body is not duplicated in JSON"
        );
    }

    // trace:STORY-1425 | ai:claude
    #[test]
    fn sanitize_subject_and_trailer_detection() {
        assert_eq!(
            sanitize_subject("[AI:claude:med] fix(api): handle null response (BUG-23)"),
            "fix(api): handle null response"
        );
        assert_eq!(
            sanitize_subject("chore(integrate): batch 39 - STORY-1416 STORY-1462 (#2150)"),
            "chore(integrate): batch 39 - (#2150)"
        );
        assert!(has_spec_trailer("[AI:claude] feat(x): y (STORY-1425)"));
        assert!(has_spec_trailer("fix: z (FR-1-042, BUG-9)"));
        assert!(!has_spec_trailer("docs: update README"));
        assert!(!has_spec_trailer("chore: merge (#2150)"));
    }

    // trace:STORY-1425 | ai:claude
    #[test]
    fn baseline_matcher_sees_every_baseline_test_under_every_criterion() {
        let pairs = vec![
            (
                "S.AC1".to_string(),
                vec![("a".to_string(), String::new())],
                vec![],
            ),
            (
                "S.AC2".to_string(),
                vec![("b".to_string(), String::new())],
                vec![],
            ),
        ];
        let base = vec![RegeneratedTest {
            criterion: String::new(),
            name: "guess".into(),
            source: String::new(),
        }];
        let out = baseline_match_pairs(&pairs, &base);
        assert!(out
            .iter()
            .all(|(_, _, r)| r.len() == 1 && r[0].name == "guess"));
        // Arm A's output may omit the criterion label entirely.
        assert_eq!(
            parse_regenerated(r#"{"tests":[{"name":"g","source":"fn g(){}"}]}"#).unwrap()[0]
                .criterion,
            ""
        );
    }

    // trace:STORY-1425 | ai:claude
    #[test]
    fn rust_test_tracing_counts_traced_and_untraced_tests() {
        let src = "// trace:T-1.AC1 | ai:claude\n#[test]\nfn above() {\n    assert!(true);\n}\n\n#[test]\nfn inside() {\n    // trace:T-2 | ai:claude\n    assert!(true);\n}\n\n#[test]\nfn bare() {\n    assert!(true);\n}\n\n// trace:T-3 | ai:claude\nfn helper() {}\n\n#[test]\nfn one_liner() { assert!(true); }\n";
        let (total, traced, markers) = rust_test_tracing(src);
        assert_eq!((total, traced), (4, 2));
        assert!(markers.contains("T-1.AC1") && markers.contains("T-2"));
        assert!(
            !markers.contains("T-3"),
            "a non-test fn's trace does not count"
        );
    }

    // Coverage is reported per metric, unknown when uncomputable, never summed.
    // trace:STORY-1425 | ai:claude
    #[test]
    fn coverage_metrics_stand_alone_and_report_unknown() {
        let specs = vec![
            (
                "S-1".to_string(),
                vec!["S-1.AC1".to_string(), "S-1.AC2".to_string()],
            ),
            ("S-2".to_string(), vec![]),
        ];
        let markers: BTreeSet<String> = ["S-1.AC1".to_string()].into_iter().collect();
        let subjects = vec!["feat: a (S-1)".to_string(), "docs: b".to_string()];
        let c = coverage_from_parts(&specs, &markers, (10, 3), Some(&subjects));
        assert_eq!(c.tests_traced, Ratio::new(3, 10));
        assert_eq!(c.specs_with_criteria, Ratio::new(1, 2));
        assert_eq!(c.criteria_with_traced_test, Ratio::new(1, 2));
        assert_eq!(c.commits_with_spec_trailer, Ratio::new(1, 2));
        assert!(c.note.contains("never combined"));
        let none = coverage_from_parts(&[], &BTreeSet::new(), (0, 0), None);
        assert!(none.tests_traced.is_none() && none.specs_with_criteria.is_none());
        assert!(
            none.criteria_with_traced_test.is_none() && none.commits_with_spec_trailer.is_none()
        );
        assert_eq!(render_ratio(&none.tests_traced), "unknown");
    }

    // trace:STORY-1425 | ai:claude
    #[test]
    fn validation_subject_labels_aida_itself_as_lower_bound() {
        let dir = tempfile::tempdir().unwrap();
        let other = validation_subject(dir.path());
        assert!(!other.lower_bound);
        std::fs::create_dir_all(dir.path().join("aida-core")).unwrap();
        std::fs::write(dir.path().join("aida-core/Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("aida-cli-lib")).unwrap();
        let aida = validation_subject(dir.path());
        assert!(aida.lower_bound && aida.label.contains("lower bound"));
        assert!(!aida.name.is_empty());
    }

    // The coordinator's BLOCK: a README carrying trace comments and SPEC-IDs
    // must not reach arm A.
    // trace:STORY-1425 | ai:claude
    #[test]
    fn baseline_readme_is_stripped_of_traces_spec_ids_and_aida_paths() {
        let readme = "# Tool\n\nIt drains the queue.\n\n```rust\n// trace:STORY-1 | ai:claude\nfn drain() {}\n```\n\n# trace:TASK-9\n<!-- trace:BUG-3 -->\nSee specs/STORY-118.md; this exists because of STORY-118 (trace:FR-1-042 | ai:codex).\nConfig lives in .aida/config.toml and the store in `.aida-store/`.\nMarker [aida:sem] kept out.\n";
        let clean = sanitize_readme(readme);
        for leaked in [
            "trace:",
            "STORY-",
            "TASK-",
            "BUG-",
            "FR-1",
            ".aida",
            "[aida:",
            "ai:claude",
            "ai:codex",
        ] {
            assert!(!clean.contains(leaked), "README leaked {leaked}:\n{clean}");
        }
        assert!(clean.contains("It drains the queue."));
        assert!(clean.contains("fn drain() {}"));
        assert!(clean.contains("Config lives in"));
        // End to end: the arm-A brief built from it carries none either.
        let inputs = BaselineInputs {
            readme: Some("README.md".into()),
            readme_text: clean,
            ..Default::default()
        };
        let p = build_baseline_prompt(&inputs, Path::new("regenerated.json"));
        assert!(!p.contains("trace:") && !p.contains("STORY-118") && !p.contains(".aida"));
    }

    // trace:STORY-1425 | ai:claude
    #[test]
    fn cost_guard_requires_yes_without_a_terminal() {
        assert_eq!(planned_agent_runs(0), 1);
        assert_eq!(planned_agent_runs(3), 4);
        assert!(cost_guard(false, true, 4).is_ok(), "a TTY proceeds");
        assert!(
            cost_guard(true, false, 4).is_ok(),
            "--yes proceeds headless"
        );
        let err = cost_guard(false, false, 4).unwrap_err().to_string();
        assert!(err.contains("--yes") && err.contains('4'), "{err}");
    }

    // A failed store arm is unknown with an unknown delta — never an abort, never zero.
    // trace:STORY-1425 | ai:claude
    #[test]
    fn failed_store_arm_is_unknown_and_delta_unknown() {
        let arm_b = ArmScore::unknown("B", "store", 3, "the probe agent exited with 1");
        let arm_a = ArmScore::known(
            "A",
            "baseline (no store)",
            &[m("S.AC1", "a", MatchVerdict::Matched, "")],
            3,
            1,
        );
        assert_eq!(arm_b.matched, None);
        assert_eq!(delta_points(&arm_a, &arm_b), None);
        assert!(delta_unknown_reason(&arm_a, &arm_b)
            .unwrap()
            .contains("arm B unknown"));
    }
}
