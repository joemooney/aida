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
}

/// One regenerated test as the probe reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RegeneratedTest {
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
pub(crate) fn build_match_prompt(
    spec: &str,
    pairs: &[(String, Vec<(String, String)>, Vec<RegeneratedTest>)],
    out_path: &Path,
) -> String {
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

fn aida_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aida"))
}

fn aida_stdout(project_root: &Path, args: &[&str]) -> String {
    std::process::Command::new(aida_exe())
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
    score_matched: usize,
    score_total: usize,
    score_label: &'static str,
    regenerated: usize,
    matches: Vec<TestMatch>,
    untested_criteria: Vec<String>,
    unanchored_tests: Vec<String>,
    candidates_file: Option<String>,
    scratch_dir: String,
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
    if opts.dry_run {
        println!(
            "dry run — not launching. {} real traced test(s), {} criteria, {} symbol(s). Probe brief:\n\n{prompt}",
            real_tests.len(),
            criteria_ids.len(),
            symbols.len()
        );
        return Ok(());
    }
    let logs = project_root.join(".aida").join("headless-logs");
    std::fs::create_dir_all(&logs)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    eprintln!(
        "  {} probing {} from the store alone (empty scratch dir, no source)…",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        display
    );
    run_headless(
        project_root,
        &scratch,
        &prompt,
        &format!("probe-{}", display.to_ascii_lowercase()),
        &logs.join(format!(
            "probe-{}-{stamp}.jsonl",
            display.to_ascii_lowercase()
        )),
    )?;
    let regenerated = parse_regenerated(
        &std::fs::read_to_string(&probe_out)
            .with_context(|| format!("the probe wrote no output at {}", probe_out.display()))?,
    )?;

    // Matching pass: per criterion, real (name, source) vs regenerated.
    let pairs: Vec<(String, Vec<(String, String)>, Vec<RegeneratedTest>)> = report
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
        .collect();
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
                &scratch,
                &build_match_prompt(&display, &pairs, &match_out),
                &format!("match-{}", display.to_ascii_lowercase()),
                &logs.join(format!(
                    "match-{}-{stamp}.jsonl",
                    display.to_ascii_lowercase()
                )),
            )?;
            parse_matches(&std::fs::read_to_string(&match_out).with_context(|| {
                format!("the matcher wrote no output at {}", match_out.display())
            })?)?
        };

    let (matched, total) = score(&matches, real_tests.len());
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
        score_matched: matched,
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
            println!("    {glyph} {} — {}", m.real_test, m.reason.trim());
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
    let pct = if total == 0 {
        0.0
    } else {
        matched as f64 / total as f64
    };
    println!(
        "\n  Score ({}): {}/{} real traced tests reproducible from the store ({:.0}%)",
        "heuristic".yellow(),
        matched,
        total,
        pct * 100.0
    );
    match &candidates_file {
        Some(f) => println!(
            "\n  {} {} divergence(s) written as harvest candidates → confirm with:\n    aida harvest {} --from {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            candidates.len(),
            display,
            f.display()
        ),
        None if total > 0 => println!(
            "\n  {} every real traced test was reproducible — nothing to harvest",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        None => println!(
            "\n  {} no real traced tests yet — run `aida criteria {}` and trace tests to criteria first",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            display
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
            std::fs::File::open(&run)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let expired = root.path().join("expired");
        std::fs::create_dir(&expired).unwrap();
        std::fs::File::open(&expired)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(now - SCRATCH_MAX_AGE - Duration::from_secs(1)),
            )
            .unwrap();
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
}
