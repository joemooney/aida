//! TASK-1247 / ADR-42 — `aida harvest <SPEC>`: the advisory harvest gate
//! (STORY-1178 slice 2 of the reconstitution loop, EPIC-70).
//!
//! After a change lands on a branch, the store should still be sufficient to
//! reconstitute what was built. `harvest` closes that gap: it hands a headless
//! agent the diff plus the spec's contract (existing acceptance criteria,
//! linked decisions, prior `[aida:sem]` micro-decisions) and asks for the
//! *observable, non-obvious* facts the diff established that the store does
//! not yet say. Candidates pass a strict selectivity filter, the operator
//! confirms an opt-in checklist, and confirmed items land per kind:
//!
//! - **ac** → a new labeled line in the spec's `## Acceptance` section, so
//!   `aida criteria` sees it on the next run;
//! - **sem** → an `[aida:sem]` marked comment (the STORY-1173 ledger pattern,
//!   no schema change);
//! - **adr** → a Draft decision spec linked to the source spec.
//!
//! Every run appends an `[aida:harvest]` ledger comment recording what ran,
//! what was confirmed and what was SKIPPED (with the reason) — nothing is ever
//! silently dropped. v1 is ADVISORY: it never blocks a merge. The agent runs
//! through the existing headless launcher (`session::spawn_vendor_headless`);
//! there is no new agent transport.
// trace:TASK-1247 | ai:claude

use std::collections::BTreeSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};

pub(crate) const HARVEST_MARKER: &str = "[aida:harvest]";
pub(crate) const SEM_MARKER: &str = "[aida:sem]";

// TASK-1249 / ADR-45: whether the drain runs the advisory harvest step.
/// Controls whether drains run the advisory harvest step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HarvestGate {
    /// Run after the review gates, propose-only (default).
    Advisory,
    /// Skip the step in drains.
    Off,
}

/// `[harvest]` config — the calibration surface for the selectivity filter.
/// Strict by default (anti-slop); loosen on evidence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HarvestConfig {
    /// `[harvest] gate = "advisory" | "off"`.
    pub(crate) gate: HarvestGate,
    /// Candidates rated below this confidence are skipped.
    pub(crate) min_confidence: f64,
    /// Skip candidates the agent itself marks as conventional/obvious.
    pub(crate) skip_conventional: bool,
    /// Case-insensitive substrings; a candidate mentioning one is skipped as
    /// conventional unless it also matches `allow_keywords`.
    pub(crate) deny_keywords: Vec<String>,
    /// Case-insensitive substrings that override the deny list.
    pub(crate) allow_keywords: Vec<String>,
}

impl Default for HarvestConfig {
    fn default() -> Self {
        Self {
            gate: HarvestGate::Advisory,
            min_confidence: 0.7,
            skip_conventional: true,
            deny_keywords: [
                "formatting",
                "rustfmt",
                "clippy",
                "naming",
                "renamed",
                "logging",
                "log line",
                "typo",
                "whitespace",
                "comment only",
                "doc comment",
                "import",
                "unused",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            allow_keywords: Vec::new(),
        }
    }
}

/// Read `[harvest]` from `.aida/config.toml`; absent section/keys keep the
/// strict defaults. An explicit empty `deny_keywords = []` disables that list.
pub(crate) fn read_harvest_config(config_path: &Path) -> HarvestConfig {
    let mut cfg = HarvestConfig::default();
    let Ok(body) = std::fs::read_to_string(config_path) else {
        return cfg;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return cfg;
    };
    let Some(table) = value.get("harvest") else {
        return cfg;
    };
    if let Some(v) = table.get("gate").and_then(|v| v.as_str()) {
        cfg.gate = match v.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "no" | "disabled" => HarvestGate::Off,
            _ => HarvestGate::Advisory,
        };
    }
    if let Some(v) = table.get("min_confidence").and_then(|v| v.as_float()) {
        cfg.min_confidence = v.clamp(0.0, 1.0);
    }
    if let Some(v) = table.get("skip_conventional").and_then(|v| v.as_bool()) {
        cfg.skip_conventional = v;
    }
    let strings = |key: &str| -> Option<Vec<String>> {
        table.get(key).and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
    };
    if let Some(v) = strings("deny_keywords") {
        cfg.deny_keywords = v;
    }
    if let Some(v) = strings("allow_keywords") {
        cfg.allow_keywords = v;
    }
    cfg
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CandidateKind {
    /// A new acceptance criterion the diff established.
    Ac,
    /// A small design decision made in passing (a SEM micro-decision).
    Sem,
    /// A decision large enough to deserve its own ADR.
    Adr,
}

impl CandidateKind {
    fn label(self) -> &'static str {
        match self {
            Self::Ac => "AC",
            Self::Sem => "SEM",
            Self::Adr => "ADR",
        }
    }
}

/// One candidate as the agent reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Candidate {
    pub(crate) kind: CandidateKind,
    /// The criterion / decision text, one or two sentences.
    pub(crate) text: String,
    /// Why the agent thinks the store should carry it.
    #[serde(default)]
    pub(crate) rationale: String,
    /// 0.0–1.0: how sure the agent is that this is observable AND non-obvious.
    #[serde(default)]
    pub(crate) confidence: f64,
    /// The agent's own read: is this a conventional/obvious fact?
    #[serde(default)]
    pub(crate) conventional: bool,
}

/// Why a candidate was filtered before the operator saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SkipReason {
    LowConfidence,
    Conventional,
    DenyListed,
    Empty,
}

impl SkipReason {
    fn describe(&self) -> &'static str {
        match self {
            Self::LowConfidence => "below min_confidence",
            Self::Conventional => "agent marked it conventional",
            Self::DenyListed => "matches a deny keyword",
            Self::Empty => "empty text",
        }
    }
}

/// Apply the selectivity filter: `(kept, skipped-with-reason)`, both in the
/// agent's order. Pure so the calibration surface is unit-testable.
pub(crate) fn filter_candidates(
    candidates: Vec<Candidate>,
    cfg: &HarvestConfig,
) -> (Vec<Candidate>, Vec<(Candidate, SkipReason)>) {
    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for c in candidates {
        let hay = format!("{} {}", c.text, c.rationale).to_lowercase();
        let allowed = cfg.allow_keywords.iter().any(|k| hay.contains(k.as_str()));
        let reason = if c.text.trim().is_empty() {
            Some(SkipReason::Empty)
        } else if c.confidence < cfg.min_confidence {
            Some(SkipReason::LowConfidence)
        } else if cfg.skip_conventional && c.conventional && !allowed {
            Some(SkipReason::Conventional)
        } else if !allowed && cfg.deny_keywords.iter().any(|k| hay.contains(k.as_str())) {
            Some(SkipReason::DenyListed)
        } else {
            None
        };
        match reason {
            Some(r) => skipped.push((c, r)),
            None => kept.push(c),
        }
    }
    (kept, skipped)
}

/// Parse the agent's output file: either `{"candidates": [...]}` or a bare
/// array. Tolerates a fenced ```json block around it.
pub(crate) fn parse_candidates(raw: &str) -> Result<Vec<Candidate>> {
    let body = strip_code_fence(raw);
    #[derive(Deserialize)]
    struct Wrapped {
        candidates: Vec<Candidate>,
    }
    if let Ok(w) = serde_json::from_str::<Wrapped>(body) {
        return Ok(w.candidates);
    }
    serde_json::from_str::<Vec<Candidate>>(body).context("harvest output is not a candidate list")
}

fn strip_code_fence(raw: &str) -> &str {
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

/// The next explicit criterion label: `AC<n>` where n is one past the highest
/// existing `AC<n>`-style label (any letters + digits count), never colliding
/// with an existing label.
pub(crate) fn next_ac_label(existing_labels: &[String]) -> String {
    let mut max = 0u32;
    for l in existing_labels {
        let digits: String = l.chars().skip_while(|c| !c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u32>() {
            max = max.max(n);
        }
    }
    let mut n = max + 1;
    loop {
        let label = format!("AC{n}");
        if !existing_labels
            .iter()
            .any(|l| l.eq_ignore_ascii_case(&label))
        {
            return label;
        }
        n += 1;
    }
}

/// Append `- LABEL. text` to the spec's `## Acceptance` section (creating the
/// section when absent). Pure so the description surgery is unit-testable.
pub(crate) fn append_acceptance_line(description: &str, label: &str, text: &str) -> String {
    let line = format!("- {label}. {}", text.trim());
    let lines: Vec<&str> = description.lines().collect();
    let mut header_idx = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if let Some(title) = t.strip_prefix('#') {
            let title = title.trim_start_matches('#').trim().to_ascii_lowercase();
            if title == "acceptance" || title.starts_with("acceptance criteria") {
                header_idx = Some(i);
                break;
            }
        }
    }
    let Some(h) = header_idx else {
        let mut out = description.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("## Acceptance\n");
        out.push_str(&line);
        out.push('\n');
        return out;
    };
    // The section ends at the next heading (or EOF); insert after its last
    // non-blank line so trailing blank lines stay where they were.
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(h + 1) {
        if l.trim_start().starts_with('#') {
            end = i;
            break;
        }
    }
    let mut insert_at = end;
    while insert_at > h + 1 && lines[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }
    let mut out: Vec<String> = lines[..insert_at].iter().map(|s| s.to_string()).collect();
    out.push(line);
    out.extend(lines[insert_at..].iter().map(|s| s.to_string()));
    let mut s = out.join("\n");
    if description.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Parse the operator's checklist answer: `1 3 5-7`, `all`, or empty (none).
/// Numbers are 1-based; out-of-range is an error rather than a silent drop.
pub(crate) fn parse_selection(input: &str, n: usize) -> Result<BTreeSet<usize>> {
    let t = input.trim().to_ascii_lowercase();
    let mut out = BTreeSet::new();
    if t.is_empty() || t == "none" || t == "n" {
        return Ok(out);
    }
    if t == "all" || t == "a" || t == "*" {
        out.extend(1..=n);
        return Ok(out);
    }
    for tok in t.split(|c: char| c == ',' || c.is_whitespace()) {
        if tok.is_empty() {
            continue;
        }
        let (lo, hi) = match tok.split_once('-') {
            Some((a, b)) => (a.parse::<usize>()?, b.parse::<usize>()?),
            None => {
                let v = tok.parse::<usize>()?;
                (v, v)
            }
        };
        if lo == 0 || hi > n || lo > hi {
            anyhow::bail!("selection `{tok}` is outside 1..={n}");
        }
        out.extend(lo..=hi);
    }
    Ok(out)
}

/// The `[aida:sem]` marked comment body for a confirmed micro-decision.
pub(crate) fn sem_comment_body(c: &Candidate, source: &str) -> String {
    let mut s = format!("{SEM_MARKER}\ndecision: {}\n", c.text.trim());
    if !c.rationale.trim().is_empty() {
        s.push_str(&format!("rationale: {}\n", c.rationale.trim()));
    }
    s.push_str(&format!(
        "source: {source}\nconfidence: {:.2}\n",
        c.confidence
    ));
    s
}

/// The `[aida:harvest]` ledger comment: what ran, what landed, what was
/// skipped and why. Written on EVERY run (even zero candidates).
pub(crate) fn ledger_comment_body(
    source: &str,
    vendor: &str,
    landed: &[(Candidate, String)],
    declined: &[Candidate],
    skipped: &[(Candidate, SkipReason)],
) -> String {
    let mut s = format!(
        "{HARVEST_MARKER}\nsource: {source}\nagent: {vendor}\nran-at: {}\nconfirmed: {}\ndeclined: {}\nskipped: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M %Z"),
        landed.len(),
        declined.len(),
        skipped.len()
    );
    for (c, landing) in landed {
        s.push_str(&format!(
            "+ {} {} — {}\n",
            c.kind.label(),
            landing,
            c.text.trim()
        ));
    }
    for c in declined {
        s.push_str(&format!(
            "- {} declined by operator — {}\n",
            c.kind.label(),
            c.text.trim()
        ));
    }
    for (c, r) in skipped {
        s.push_str(&format!(
            "- {} skipped ({}) — {}\n",
            c.kind.label(),
            r.describe(),
            c.text.trim()
        ));
    }
    s
}

/// The headless agent's brief. The agent must write JSON to `out_path`, so no
/// stream-json parsing is needed (the same file handshake the reviewer uses).
pub(crate) fn build_harvest_prompt(
    spec: &str,
    title: &str,
    description: &str,
    existing_criteria: &[String],
    linked_decisions: &[String],
    prior_sems: &[String],
    diff: &str,
    cfg: &HarvestConfig,
    out_path: &Path,
) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "You are AIDA's harvest gate for {spec} — \"{title}\".\n\n\
         Goal: read the diff below and name the facts it ESTABLISHED that the requirement store does not yet say, so a future agent could rebuild this behavior from the store alone. Report ONLY facts that are both OBSERVABLE (a test or a user could check them) and NON-OBVIOUS (not implied by the existing criteria, not a coding convention). Conventions, formatting, naming, logging, obvious error handling: skip them.\n\n\
         Rules:\n\
         - Do not ask questions, do not use AskUserQuestion, do not edit any file except the output file.\n\
         - Do not run `aida edit`, `aida add`, `aida comment` or git commands that write.\n\
         - Write your answer as JSON to `{}` and nothing else. Shape:\n\
           {{\"candidates\": [{{\"kind\": \"ac\"|\"sem\"|\"adr\", \"text\": \"...\", \"rationale\": \"...\", \"confidence\": 0.0-1.0, \"conventional\": false}}]}}\n\
         - kind: ac = an acceptance criterion the diff satisfies that is missing from the list below; sem = a small design decision made in passing; adr = a decision big enough to deserve its own record.\n\
         - confidence below {:.2} will be filtered; be calibrated, not generous. An empty candidates list is a valid, good answer.\n\n",
        out_path.display(),
        cfg.min_confidence
    ));
    p.push_str("## Requirement\n\n");
    p.push_str(description.trim());
    p.push_str("\n\n## Existing acceptance criteria (do NOT repeat these)\n");
    if existing_criteria.is_empty() {
        p.push_str("(none recorded)\n");
    }
    for c in existing_criteria {
        p.push_str(&format!("- {c}\n"));
    }
    if !linked_decisions.is_empty() {
        p.push_str("\n## Linked decisions (already recorded)\n");
        for d in linked_decisions {
            p.push_str(&format!("- {d}\n"));
        }
    }
    if !prior_sems.is_empty() {
        p.push_str("\n## Prior micro-decisions (already recorded)\n");
        for s in prior_sems {
            p.push_str(&format!("- {s}\n"));
        }
    }
    p.push_str("\n## Diff\n\n```diff\n");
    p.push_str(diff);
    if !diff.ends_with('\n') {
        p.push('\n');
    }
    p.push_str("```\n");
    p
}

// TASK-1249: the `[aida:harvest]` ledger for an UNATTENDED (drain) run —
/// nothing lands; kept candidates are proposed via a file for a human /
/// advisor to confirm with `aida harvest <SPEC> --from <file>`.
// trace:TASK-1249 | ai:claude
pub(crate) fn ledger_comment_body_proposed(
    source: &str,
    vendor: &str,
    proposed: &[Candidate],
    skipped: &[(Candidate, SkipReason)],
    file: Option<&Path>,
) -> String {
    let mut s = format!(
        "{HARVEST_MARKER}\nsource: {source}\nagent: {vendor}\nran-at: {}\nmode: propose-only (drain)\nproposed: {}\nskipped: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M %Z"),
        proposed.len(),
        skipped.len()
    );
    if let Some(f) = file {
        s.push_str(&format!("candidates-file: {}\n", f.display()));
    }
    for c in proposed {
        s.push_str(&format!(
            "? {} ({:.2}) — {}\n",
            c.kind.label(),
            c.confidence,
            c.text.trim()
        ));
    }
    for (c, r) in skipped {
        s.push_str(&format!(
            "- {} skipped ({}) — {}\n",
            c.kind.label(),
            r.describe(),
            c.text.trim()
        ));
    }
    s
}

// TASK-1249: outcome of an unattended, propose-only harvest.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProposeSummary {
    pub(crate) candidates: usize,
    pub(crate) proposed: usize,
    pub(crate) skipped: usize,
    pub(crate) file: Option<PathBuf>,
}

// TASK-1249 / ADR-45: the drain's advisory harvest. Runs the agent + filter
/// on the PR diff, writes the ledger comment and — when anything survives
/// the filter — a candidates file plus a brief for the advisor (surfaced by
/// `aida awaiting`). NEVER edits the contract: confirmation is a later,
/// human/advisor `aida harvest <SPEC> --from <file>`.
// trace:TASK-1249 | ai:claude
pub(crate) fn propose_only(
    project_root: &Path,
    store: &aida_core::RequirementsStore,
    spec: &str,
    pr: u64,
) -> Result<ProposeSummary> {
    let req = store
        .get_requirement_by_spec_id(spec)
        .or_else(|| {
            store.requirements.iter().find(|r| {
                r.agreed_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(spec))
            })
        })
        .ok_or_else(|| anyhow::anyhow!("requirement not found: {spec}"))?;
    let display = req.display_id();
    let opts = HarvestOptions {
        pr: Some(pr),
        ..HarvestOptions::default()
    };
    let (diff, source) = collect_diff(project_root, &opts)?;
    if diff.trim().is_empty() {
        anyhow::bail!("PR-{pr} has an empty diff — nothing to harvest");
    }
    let cfg = read_harvest_config(&project_root.join(".aida").join("config.toml"));
    let ctx = spec_context(store, req, &display);
    let run_id = uuid::Uuid::now_v7().to_string();
    let harvest_dir = project_root.join(".aida").join("harvest");
    std::fs::create_dir_all(&harvest_dir)?;
    let out_path = harvest_dir.join(format!("{}-{run_id}.json", display.to_ascii_lowercase()));
    let prompt = build_harvest_prompt(
        &display,
        &req.title,
        &req.description,
        &ctx.existing,
        &ctx.linked_decisions,
        &ctx.prior_sems,
        &diff,
        &cfg,
        &out_path,
    );
    let vendor = crate::session::resolve_headless_vendor(project_root);
    let raw = run_harvest_agent(project_root, &display, &prompt, &run_id, &out_path)?;
    let candidates = parse_candidates(&raw)?;
    let total = candidates.len();
    let (kept, skipped) = filter_candidates(candidates, &cfg);
    let file = if kept.is_empty() {
        None
    } else {
        let f = harvest_dir.join(format!(
            "{}-drain-{run_id}.json",
            display.to_ascii_lowercase()
        ));
        std::fs::write(
            &f,
            serde_json::to_string_pretty(&serde_json::json!({ "candidates": kept }))?,
        )?;
        Some(f)
    };
    let ledger =
        ledger_comment_body_proposed(&source, vendor.as_str(), &kept, &skipped, file.as_deref());
    let ledger_path = persisted_ledger_path(file.as_deref().unwrap_or(&out_path));
    persist_ledger_and_publish(
        &ledger_path,
        &ledger,
        || {
            run_aida_with_retry(
                project_root,
                &["comment", "add", &display, &ledger, "--author", "harvest"],
            )
            .map(|_| ())
        },
        || {
            if let Some(f) = &file {
                let note = format!(
                    "drain harvest proposed {} candidate(s) from {source} — confirm with: aida harvest {display} --from {}",
                    kept.len(),
                    f.display()
                );
                let _ = run_aida(
                    project_root,
                    &["brief", "advisor", &display, "--note", &note],
                );
            }
        },
    )?;
    Ok(ProposeSummary {
        candidates: total,
        proposed: kept.len(),
        skipped: skipped.len(),
        file,
    })
}

/// The spec-side context the harvest brief carries besides the diff.
struct SpecContext {
    existing: Vec<String>,
    linked_decisions: Vec<String>,
    prior_sems: Vec<String>,
}

fn spec_context(
    store: &aida_core::RequirementsStore,
    req: &aida_core::Requirement,
    display: &str,
) -> SpecContext {
    let existing = crate::criteria::parse_acceptance_criteria(display, &req.description)
        .iter()
        .map(|c| format!("{}: {}", c.label, c.text))
        .collect();
    let linked_decisions: Vec<String> = req
        .relationships
        .iter()
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .chain(store.requirements.iter().filter(|r| {
            r.req_type == aida_core::RequirementType::Decision
                && r.relationships.iter().any(|rel| rel.target_id == req.id)
        }))
        .filter(|r| r.req_type == aida_core::RequirementType::Decision)
        .map(|r| format!("{}: {}", r.display_id(), r.title))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let prior_sems = req
        .comments
        .iter()
        .filter(|c| c.content.trim_start().starts_with(SEM_MARKER))
        .filter_map(|c| {
            c.content
                .lines()
                .find_map(|l| l.strip_prefix("decision:"))
                .map(|d| d.trim().to_string())
        })
        .collect();
    SpecContext {
        existing,
        linked_decisions,
        prior_sems,
    }
}

/// Launch the harvest agent through the existing headless launcher and read
/// back the JSON it wrote to `out_path`.
fn run_harvest_agent(
    project_root: &Path,
    display: &str,
    prompt: &str,
    run_id: &str,
    out_path: &Path,
) -> Result<String> {
    let vendor = crate::session::resolve_headless_vendor(project_root);
    let log_path = project_root
        .join(".aida")
        .join("headless-logs")
        .join(format!(
            "harvest-{}-{}.jsonl",
            display.to_ascii_lowercase(),
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
        .with_label(format!("harvest-{}", display.to_ascii_lowercase()));
    let previous_role = std::env::var_os("AIDA_SESSION_ROLE");
    std::env::set_var("AIDA_SESSION_ROLE", "advisor");
    let status =
        crate::session::spawn_vendor_headless(vendor, prompt, run_id, &log_path, &tee, false);
    match previous_role {
        Some(v) => std::env::set_var("AIDA_SESSION_ROLE", v),
        None => std::env::remove_var("AIDA_SESSION_ROLE"),
    }
    let status = status?;
    if !status.success() {
        anyhow::bail!(
            "the harvest agent exited with {} — see {}",
            status.code().unwrap_or(1),
            log_path.display()
        );
    }
    std::fs::read_to_string(out_path).with_context(|| {
        format!(
            "the harvest agent wrote no output file at {} — see {}",
            out_path.display(),
            log_path.display()
        )
    })
}

// BUG-1198: whitespace- and case-insensitive form used to decide whether a
/// harvested text is already on the spec.
// trace:BUG-1198 | ai:claude
pub(crate) fn normalize_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

// BUG-1198: the label of an existing criterion whose text equals `text`
/// (normalized), if any.
// trace:BUG-1198 | ai:claude
pub(crate) fn acceptance_label_for_text(
    existing: &[(String, String)],
    text: &str,
) -> Option<String> {
    let want = normalize_text(text);
    existing
        .iter()
        .find(|(_, t)| *t == want)
        .map(|(label, _)| label.clone())
}

// BUG-1198: the sidecar that marks a candidates file as consumed
/// (`<file>.confirmed`), if it exists.
// trace:BUG-1198 | ai:claude
pub(crate) fn consumed_marker_for(file: &Path) -> Option<PathBuf> {
    let marker = consumed_marker_path(file);
    marker.exists().then_some(marker)
}

fn consumed_marker_path(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".confirmed");
    file.with_file_name(name)
}

// BUG-1198: mark a candidates file consumed by writing its sidecar with the
/// confirmation time. The candidates file itself is left in place for audit.
// trace:BUG-1198 | ai:claude
pub(crate) fn mark_consumed(file: &Path) -> std::io::Result<()> {
    std::fs::write(
        consumed_marker_path(file),
        format!(
            "confirmed-at: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M %Z")
        ),
    )
}

/// Flags the handler resolves from the clap subcommand.
#[derive(Debug, Clone, Default)]
pub(crate) struct HarvestOptions {
    // TASK-1248: confirm candidates from a file (a `reconstitute` probe's
    /// divergence output) instead of running the harvest agent.
    pub(crate) from: Option<PathBuf>,
    pub(crate) pr: Option<u64>,
    pub(crate) base: Option<String>,
    pub(crate) yes_all: bool,
    pub(crate) dry_run: bool,
    pub(crate) json: bool,
}

#[derive(Debug, Serialize)]
struct RunSummary {
    spec: String,
    source: String,
    candidates: usize,
    kept: usize,
    confirmed: usize,
    declined: usize,
    skipped: usize,
    landed: Vec<serde_json::Value>,
}

fn run_aida(project_root: &Path, args: &[&str]) -> Result<String> {
    let command = concise_aida_command(args);
    let out = std::process::Command::new(crate::aida_exe_path())
        .current_dir(project_root)
        .args(args)
        .output()
        .map_err(|error| {
            anyhow::anyhow!("could not run `{command}`: {:?}: {error}", error.kind())
        })?;
    if !out.status.success() {
        anyhow::bail!(
            "`{command}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn concise_aida_command(args: &[&str]) -> String {
    const LIMIT: usize = 160;
    let flat = format!("aida {}", args.join(" ")).replace(['\n', '\r'], " ");
    if flat.chars().count() <= LIMIT {
        return flat;
    }
    format!("{}…", flat.chars().take(LIMIT).collect::<String>())
}

fn persisted_ledger_path(candidates_file: &Path) -> PathBuf {
    PathBuf::from(format!("{}.ledger.md", candidates_file.display()))
}

// Persist first and always surface the local brief, even when the canonical
// comment write cannot spawn or exhausts its retries. trace:BUG-1199 | ai:codex
fn persist_ledger_and_publish(
    ledger_path: &Path,
    ledger: &str,
    mut publish_comment: impl FnMut() -> Result<()>,
    mut publish_brief: impl FnMut(),
) -> Result<()> {
    std::fs::write(ledger_path, ledger)?;
    let comment_result = publish_comment();
    publish_brief();
    comment_result.with_context(|| {
        format!(
            "harvest ledger comment failed; persisted ledger: {}",
            ledger_path.display()
        )
    })
}

fn run_aida_with_retry(project_root: &Path, args: &[&str]) -> Result<String> {
    let mut last = None;
    for attempt in 0..3 {
        match run_aida(project_root, args) {
            Ok(output) => return Ok(output),
            Err(error) => last = Some(error),
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    Err(last.expect("retry loop always attempts at least once"))
}

fn collect_diff(project_root: &Path, opts: &HarvestOptions) -> Result<(String, String)> {
    if let Some(pr) = opts.pr {
        let out = std::process::Command::new("gh")
            .current_dir(project_root)
            .args(["pr", "diff", &pr.to_string()])
            .output()
            .context("could not run `gh pr diff`")?;
        if !out.status.success() {
            anyhow::bail!(
                "`gh pr diff {pr}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        return Ok((
            String::from_utf8_lossy(&out.stdout).to_string(),
            format!("PR-{pr}"),
        ));
    }
    let base = opts
        .base
        .clone()
        .unwrap_or_else(|| "origin/main".to_string());
    let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.to_path_buf());
    let out = std::process::Command::new("git")
        .current_dir(&cwd)
        .args(["diff", &format!("{base}...HEAD")])
        .output()
        .context("could not run `git diff`")?;
    if !out.status.success() {
        anyhow::bail!(
            "`git diff {base}...HEAD` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let head = std::process::Command::new("git")
        .current_dir(&cwd)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "HEAD".to_string());
    Ok((
        String::from_utf8_lossy(&out.stdout).to_string(),
        format!("{base}...{head}"),
    ))
}

pub(crate) fn handle_harvest_command(
    project_root: &Path,
    store: &aida_core::RequirementsStore,
    spec: &str,
    opts: HarvestOptions,
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

    // TASK-1248 / ADR-44: a probe's divergence file is a ready candidate set —
    // no diff and no agent run, only the filter + the opt-in checklist.
    // trace:TASK-1248 | ai:claude
    let from_file = opts.from.clone();
    let (diff, source) = match &from_file {
        Some(f) => (
            String::new(),
            format!(
                "file:{}",
                f.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("candidates")
            ),
        ),
        None => {
            let (diff, source) = collect_diff(project_root, &opts)?;
            if diff.trim().is_empty() {
                anyhow::bail!(
                    "nothing to harvest: the diff for {source} is empty (pass --pr <N> or --base <ref>)"
                );
            }
            (diff, source)
        }
    };

    let cfg = read_harvest_config(&project_root.join(".aida").join("config.toml"));
    let existing = crate::criteria::parse_acceptance_criteria(&display, &req.description);
    let existing_lines: Vec<String> = existing
        .iter()
        .map(|c| format!("{}: {}", c.label, c.text))
        .collect();
    let linked_decisions: Vec<String> = req
        .relationships
        .iter()
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .chain(store.requirements.iter().filter(|r| {
            r.req_type == aida_core::RequirementType::Decision
                && r.relationships.iter().any(|rel| rel.target_id == req.id)
        }))
        .filter(|r| r.req_type == aida_core::RequirementType::Decision)
        .map(|r| format!("{}: {}", r.display_id(), r.title))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let prior_sems: Vec<String> = req
        .comments
        .iter()
        .filter(|c| c.content.trim_start().starts_with(SEM_MARKER))
        .filter_map(|c| {
            c.content
                .lines()
                .find_map(|l| l.strip_prefix("decision:"))
                .map(|d| d.trim().to_string())
        })
        .collect();

    let harvest_dir = project_root.join(".aida").join("harvest");
    std::fs::create_dir_all(&harvest_dir)?;
    let run_id = uuid::Uuid::now_v7().to_string();
    let out_path = harvest_dir.join(format!("{}-{run_id}.json", display.to_ascii_lowercase()));
    let _ = std::fs::remove_file(&out_path);
    let prompt = build_harvest_prompt(
        &display,
        &req.title,
        &req.description,
        &existing_lines,
        &linked_decisions,
        &prior_sems,
        &diff,
        &cfg,
        &out_path,
    );

    let vendor = crate::session::resolve_headless_vendor(project_root);
    if let Some(f) = &from_file {
        if opts.dry_run {
            println!("dry run — would confirm candidates from {}", f.display());
            return Ok(());
        }
    } else if opts.dry_run {
        println!(
            "dry run — not launching {} (would harvest {} for {}).\n\n{prompt}",
            vendor.as_str(),
            source,
            display
        );
        return Ok(());
    }
    if !opts.yes_all && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "harvest needs a terminal to confirm candidates — pass --yes-all to accept every filtered candidate headlessly, or --dry-run"
        );
    }

    let raw = if let Some(f) = &from_file {
        // BUG-1198: a candidates file is consumed by ONE confirmation. A
        // second `--from` on the same file (another seat already confirmed
        // it) would re-land every candidate; refuse and point at the ledger.
        // trace:BUG-1198 | ai:claude
        if let Some(marker) = consumed_marker_for(f) {
            anyhow::bail!(
                "{} was already confirmed ({} exists) — see the [aida:harvest] ledger on {}; nothing to do",
                f.display(),
                marker.display(),
                display
            );
        }
        std::fs::read_to_string(f)
            .with_context(|| format!("could not read candidates file {}", f.display()))?
    } else {
        eprintln!(
            "  {} harvesting {} from {} with a headless {} agent…",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            display,
            source,
            vendor.as_str()
        );
        let log_path = project_root
            .join(".aida")
            .join("headless-logs")
            .join(format!(
                "harvest-{}-{}.jsonl",
                display.to_ascii_lowercase(),
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            ));
        let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false)
            .with_label(format!("harvest-{}", display.to_ascii_lowercase()));
        let previous_role = std::env::var_os("AIDA_SESSION_ROLE");
        std::env::set_var("AIDA_SESSION_ROLE", "advisor");
        let status =
            crate::session::spawn_vendor_headless(vendor, &prompt, &run_id, &log_path, &tee, false);
        match previous_role {
            Some(v) => std::env::set_var("AIDA_SESSION_ROLE", v),
            None => std::env::remove_var("AIDA_SESSION_ROLE"),
        }
        let status = status?;
        if !status.success() {
            anyhow::bail!(
                "the harvest agent exited with {} — see {}",
                status.code().unwrap_or(1),
                log_path.display()
            );
        }
        std::fs::read_to_string(&out_path).with_context(|| {
            format!(
                "the harvest agent wrote no output file at {} — see {}",
                out_path.display(),
                log_path.display()
            )
        })?
    };
    let candidates = parse_candidates(&raw)?;
    let total = candidates.len();
    let (kept, skipped) = filter_candidates(candidates, &cfg);

    // Confirm: opt-IN checklist (ADR-42) — nothing lands unless chosen.
    let chosen: BTreeSet<usize> = if opts.yes_all {
        (1..=kept.len()).collect()
    } else if kept.is_empty() {
        BTreeSet::new()
    } else {
        println!(
            "\n{} {} candidate(s) for {} ({} filtered out, recorded in the ledger):\n",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            kept.len(),
            display,
            skipped.len()
        );
        for (i, c) in kept.iter().enumerate() {
            println!(
                "  {:>2}. [{}] ({:.2}) {}",
                i + 1,
                c.kind.label(),
                c.confidence,
                c.text.trim()
            );
            if !c.rationale.trim().is_empty() {
                println!("      {}", c.rationale.trim());
            }
        }
        loop {
            let answer = crate::read_line_prompt(
                "\nAccept which? (numbers / ranges, `all`, or Enter for none): ",
            )?;
            match parse_selection(&answer, kept.len()) {
                Ok(sel) => break sel,
                Err(e) => println!("  {e} — try again"),
            }
        }
    };

    let mut landed: Vec<(Candidate, String)> = Vec::new();
    let mut declined: Vec<Candidate> = Vec::new();
    let mut description = req.description.clone();
    let mut labels: Vec<String> = existing.iter().map(|c| c.label.clone()).collect();
    // BUG-1198: landing is idempotent — an AC whose text is already in
    // `## Acceptance`, or a SEM whose decision is already recorded, is
    // reported as already-present and not written again.
    // trace:BUG-1198 | ai:claude
    let mut ac_texts: Vec<(String, String)> = existing
        .iter()
        .map(|c| (c.label.clone(), normalize_text(&c.text)))
        .collect();
    let mut sem_texts: Vec<String> = req
        .comments
        .iter()
        .filter(|c| c.content.trim_start().starts_with(SEM_MARKER))
        .filter_map(|c| {
            c.content
                .lines()
                .find_map(|l| l.strip_prefix("decision:"))
                .map(|d| normalize_text(d))
        })
        .collect();
    let mut description_changed = false;
    for (i, c) in kept.into_iter().enumerate() {
        if !chosen.contains(&(i + 1)) {
            declined.push(c);
            continue;
        }
        let landing = match c.kind {
            CandidateKind::Ac => {
                if let Some(label) = acceptance_label_for_text(&ac_texts, &c.text) {
                    format!("already present as {display}.{label}")
                } else {
                    let label = next_ac_label(&labels);
                    description = append_acceptance_line(&description, &label, &c.text);
                    labels.push(label.clone());
                    ac_texts.push((label.clone(), normalize_text(&c.text)));
                    description_changed = true;
                    format!("{display}.{label}")
                }
            }
            CandidateKind::Sem => {
                if sem_texts.contains(&normalize_text(&c.text)) {
                    "already recorded as an [aida:sem] comment".to_string()
                } else {
                    let body = sem_comment_body(&c, &source);
                    run_aida(
                        project_root,
                        &["comment", "add", &display, &body, "--author", "harvest"],
                    )?;
                    sem_texts.push(normalize_text(&c.text));
                    "[aida:sem] comment".to_string()
                }
            }
            CandidateKind::Adr => {
                let desc = format!(
                    "Harvested from {source} on {display} by `aida harvest` (advisory; confirmed by the operator).\n\n{}\n\nConfidence: {:.2}",
                    if c.rationale.trim().is_empty() { c.text.trim() } else { c.rationale.trim() },
                    c.confidence
                );
                let out = run_aida(
                    project_root,
                    &[
                        "add",
                        "--type",
                        "decision",
                        "--status",
                        "draft",
                        "--title",
                        c.text.trim(),
                        "--description",
                        &desc,
                        "--tags",
                        &format!("parent:{display},harvested"),
                    ],
                )?;
                match crate::parse_spec_id_from_add_output(&out) {
                    Some(id) => {
                        let _ = run_aida(
                            project_root,
                            &["rel", "add", &id, &display, "--type", "references"],
                        );
                        format!("draft {id}")
                    }
                    None => "draft ADR (id not parsed)".to_string(),
                }
            }
        };
        landed.push((c, landing));
    }
    if description_changed {
        run_aida(
            project_root,
            &["edit", &display, "--description", &description],
        )?;
    }
    let ledger = ledger_comment_body(&source, vendor.as_str(), &landed, &declined, &skipped);
    run_aida(
        project_root,
        &["comment", "add", &display, &ledger, "--author", "harvest"],
    )?;
    if let Some(f) = &from_file {
        // BUG-1198: consumed — a second confirmation of this file is refused.
        if let Err(e) = mark_consumed(f) {
            eprintln!(
                "  {} could not mark {} as consumed ({e}) — a second `--from` on it would re-land",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                f.display()
            );
        }
    }

    let summary = RunSummary {
        spec: display.clone(),
        source: source.clone(),
        candidates: total,
        kept: landed.len() + declined.len(),
        confirmed: landed.len(),
        declined: declined.len(),
        skipped: skipped.len(),
        landed: landed
            .iter()
            .map(|(c, l)| serde_json::json!({"kind": c.kind, "landing": l, "text": c.text}))
            .collect(),
    };
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "\n{} harvest of {} from {}: {} candidate(s), {} filtered, {} confirmed, {} declined — ledger comment written",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            display,
            source,
            total,
            skipped.len(),
            landed.len(),
            declined.len()
        );
        for (c, l) in &landed {
            println!("  + {} → {}", c.kind.label(), l);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(kind: CandidateKind, text: &str, conf: f64, conventional: bool) -> Candidate {
        Candidate {
            kind,
            text: text.into(),
            rationale: String::new(),
            confidence: conf,
            conventional,
        }
    }

    // Named in the STORY-1178 plan.
    #[test]
    fn filter_drops_conventional_keeps_observable_nonobvious() {
        let cfg = HarvestConfig::default();
        let cands = vec![
            cand(
                CandidateKind::Ac,
                "A merged (squash) PR no longer counts as unshipped",
                0.9,
                false,
            ),
            cand(CandidateKind::Sem, "Renamed helper for clarity", 0.9, true),
            cand(CandidateKind::Ac, "Adds a log line on merge", 0.95, false),
            cand(
                CandidateKind::Adr,
                "Lease is merge-scoped, not merge-to-pull",
                0.6,
                false,
            ),
            cand(CandidateKind::Ac, "", 0.9, false),
        ];
        let (kept, skipped) = filter_candidates(cands, &cfg);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].text.starts_with("A merged"));
        let reasons: Vec<&SkipReason> = skipped.iter().map(|(_, r)| r).collect();
        assert_eq!(
            reasons,
            vec![
                &SkipReason::Conventional,
                &SkipReason::DenyListed,
                &SkipReason::LowConfidence,
                &SkipReason::Empty
            ]
        );
        // allow_keywords override the deny list; skip_conventional=false keeps agent-flagged ones.
        let cfg = HarvestConfig {
            allow_keywords: vec!["log line".into()],
            skip_conventional: false,
            ..HarvestConfig::default()
        };
        let (kept, _) = filter_candidates(
            vec![
                cand(CandidateKind::Ac, "Adds a log line on merge", 0.95, false),
                cand(
                    CandidateKind::Sem,
                    "Agent-flagged as conventional",
                    0.9,
                    true,
                ),
            ],
            &cfg,
        );
        assert_eq!(
            kept.len(),
            2,
            "allow-list overrides deny; skip_conventional=false keeps agent-flagged"
        );
    }

    // TASK-1249: the drain gate and the propose-only ledger.
    #[test]
    fn drain_gate_config_and_propose_only_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        assert_eq!(
            read_harvest_config(&p).gate,
            HarvestGate::Advisory,
            "advisory by default"
        );
        std::fs::write(&p, "[harvest]\ngate = \"off\"\n").unwrap();
        assert_eq!(read_harvest_config(&p).gate, HarvestGate::Off);
        std::fs::write(&p, "[harvest]\ngate = \"advisory\"\n").unwrap();
        assert_eq!(read_harvest_config(&p).gate, HarvestGate::Advisory);
        let proposed = vec![cand(CandidateKind::Ac, "a real one", 0.9, false)];
        let skipped = vec![(
            cand(CandidateKind::Sem, "meh", 0.2, false),
            SkipReason::LowConfidence,
        )];
        let f = std::path::Path::new("harvest/t-1-drain-x.json");
        let body = ledger_comment_body_proposed("PR-9", "codex", &proposed, &skipped, Some(f));
        assert!(body.starts_with(HARVEST_MARKER));
        assert!(body.contains("mode: propose-only (drain)"));
        assert!(body.contains("proposed: 1") && body.contains("skipped: 1"));
        assert!(body.contains("candidates-file: harvest/t-1-drain-x.json"));
        assert!(body.contains("? AC (0.90) — a real one"));
        assert!(body.contains("- SEM skipped (below min_confidence) — meh"));
        assert!(
            !body.contains("confirmed:"),
            "nothing lands in propose-only"
        );
        assert_eq!(
            persisted_ledger_path(f),
            std::path::Path::new("harvest/t-1-drain-x.json.ledger.md")
        );
        let command = concise_aida_command(&["comment", "add", "TASK-1", &body]);
        assert!(!command.contains('\n'));
        assert!(command.chars().count() <= 161);
    }

    #[test]
    fn comment_failure_keeps_ledger_and_still_files_brief() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidates.json.ledger.md");
        let mut brief_filed = false;
        let error = persist_ledger_and_publish(
            &path,
            "audit body",
            || Err(anyhow::anyhow!("NotFound: No such file or directory")),
            || brief_filed = true,
        )
        .unwrap_err();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "audit body");
        assert!(brief_filed);
        let message = format!("{error:#}");
        assert!(message.contains("persisted ledger"));
        assert!(message.contains("NotFound"));
        assert_eq!(message.lines().count(), 1);
    }

    // BUG-1198: confirming the same file twice must not re-land anything.
    #[test]
    fn already_present_texts_are_detected_and_consumed_files_are_marked() {
        let existing = vec![
            (
                "A1".to_string(),
                normalize_text("Scratch-run pruning applies BOTH bounds."),
            ),
            ("AC2".to_string(), normalize_text("another one")),
        ];
        assert_eq!(
            acceptance_label_for_text(&existing, "  scratch-run   pruning applies both bounds"),
            Some("A1".to_string())
        );
        assert_eq!(acceptance_label_for_text(&existing, "a third thing"), None);
        assert_eq!(normalize_text("  A  b\tC. "), "a b c");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t-1-drain-x.json");
        std::fs::write(&f, "{\"candidates\":[]}").unwrap();
        assert!(consumed_marker_for(&f).is_none());
        mark_consumed(&f).unwrap();
        let marker = consumed_marker_for(&f).expect("marked");
        assert!(marker.ends_with("t-1-drain-x.json.confirmed"));
        assert!(std::fs::read_to_string(marker)
            .unwrap()
            .starts_with("confirmed-at:"));
        assert!(f.exists(), "the candidates file stays for audit");
    }

    #[test]
    fn config_reads_harvest_section_with_strict_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        assert_eq!(read_harvest_config(&p), HarvestConfig::default());
        std::fs::write(
            &p,
            "[harvest]\nmin_confidence = 0.5\nskip_conventional = false\ndeny_keywords = []\nallow_keywords = [\"Retry\"]\n",
        )
        .unwrap();
        let cfg = read_harvest_config(&p);
        assert_eq!(cfg.min_confidence, 0.5);
        assert!(!cfg.skip_conventional);
        assert!(
            cfg.deny_keywords.is_empty(),
            "explicit [] disables the list"
        );
        assert_eq!(cfg.allow_keywords, vec!["retry"]);
        std::fs::write(&p, "[harvest]\nmin_confidence = 7.0\n").unwrap();
        assert_eq!(read_harvest_config(&p).min_confidence, 1.0, "clamped");
    }

    #[test]
    fn parse_candidates_accepts_wrapped_bare_and_fenced() {
        let one =
            r#"{"kind":"ac","text":"x","rationale":"r","confidence":0.8,"conventional":false}"#;
        assert_eq!(
            parse_candidates(&format!("{{\"candidates\":[{one}]}}"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse_candidates(&format!("[{one}]")).unwrap().len(), 1);
        assert_eq!(
            parse_candidates(&format!("```json\n[{one}]\n```"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse_candidates(r#"{"candidates":[]}"#).unwrap().len(), 0);
        assert!(parse_candidates("not json").is_err());
        // Missing optional fields default.
        let c = parse_candidates(r#"[{"kind":"sem","text":"t"}]"#).unwrap();
        assert_eq!(c[0].confidence, 0.0);
        assert!(!c[0].conventional);
    }

    #[test]
    fn next_ac_label_is_one_past_the_highest_and_never_collides() {
        assert_eq!(next_ac_label(&[]), "AC1");
        assert_eq!(
            next_ac_label(&["A1".into(), "AC3".into(), "acf00d1".into()]),
            "AC4"
        );
        assert_eq!(next_ac_label(&["AC1".into(), "ac2".into()]), "AC3");
    }

    #[test]
    fn append_acceptance_line_targets_the_section_or_creates_it() {
        let d = "Intro.\n\n## Acceptance\n- A1. first\n- A2. second\n\n## Notes\nx\n";
        let out = append_acceptance_line(d, "AC3", "third thing");
        assert_eq!(
            out,
            "Intro.\n\n## Acceptance\n- A1. first\n- A2. second\n- AC3. third thing\n\n## Notes\nx\n"
        );
        // The new line parses as a labeled criterion.
        let parsed = crate::criteria::parse_acceptance_criteria("T-1", &out);
        assert!(parsed
            .iter()
            .any(|c| c.label == "AC3" && c.text == "third thing"));
        // Section at EOF.
        let out = append_acceptance_line("## Acceptance\n- A1. a", "AC2", "b");
        assert_eq!(out, "## Acceptance\n- A1. a\n- AC2. b");
        // No section → created.
        let out = append_acceptance_line("Just prose.", "AC1", "b");
        assert_eq!(out, "Just prose.\n\n## Acceptance\n- AC1. b\n");
    }

    #[test]
    fn parse_selection_is_one_based_opt_in() {
        assert!(
            parse_selection("", 5).unwrap().is_empty(),
            "Enter = none (opt-in)"
        );
        assert_eq!(
            parse_selection("all", 3)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            parse_selection("1, 3-4", 5)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        assert!(parse_selection("0", 5).is_err());
        assert!(parse_selection("6", 5).is_err());
        assert!(parse_selection("x", 5).is_err());
    }

    #[test]
    fn ledger_records_confirmed_declined_and_skipped_with_reasons() {
        let landed = vec![(
            cand(CandidateKind::Ac, "kept one", 0.9, false),
            "T-1.AC3".to_string(),
        )];
        let declined = vec![cand(CandidateKind::Sem, "no thanks", 0.8, false)];
        let skipped = vec![(
            cand(CandidateKind::Adr, "meh", 0.2, false),
            SkipReason::LowConfidence,
        )];
        let body = ledger_comment_body("PR-7", "claude", &landed, &declined, &skipped);
        assert!(body.starts_with(HARVEST_MARKER));
        assert!(
            body.contains("confirmed: 1")
                && body.contains("declined: 1")
                && body.contains("skipped: 1")
        );
        assert!(body.contains("+ AC T-1.AC3 — kept one"));
        assert!(body.contains("- SEM declined by operator — no thanks"));
        assert!(body.contains("- ADR skipped (below min_confidence) — meh"));
        let sem = sem_comment_body(&declined[0], "PR-7");
        assert!(sem.starts_with(SEM_MARKER) && sem.contains("decision: no thanks"));
    }

    #[test]
    fn prompt_names_output_file_threshold_and_excludes_nothing_secret() {
        let out = Path::new("harvest-out.json");
        let p = build_harvest_prompt(
            "T-1",
            "Title",
            "Body\n\n## Acceptance\n- A1. a",
            &["A1: a".into()],
            &["ADR-9: chosen".into()],
            &["prior sem".into()],
            "diff --git a b\n+x",
            &HarvestConfig::default(),
            out,
        );
        assert!(p.contains("harvest-out.json") && p.contains("0.70"));
        assert!(p.contains("- A1: a") && p.contains("ADR-9: chosen") && p.contains("prior sem"));
        assert!(p.contains("```diff\ndiff --git a b\n+x\n```"));
        assert!(p.contains("Do not ask questions"));
    }
}
