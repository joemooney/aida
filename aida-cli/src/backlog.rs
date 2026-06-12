// Backlog grooming surface: turn Approved-but-not-queued work into a queue
// drain (optionally tagged `batch:NAME`) with advisory risk + pairwise
// file-overlap heuristics.
// trace:STORY-444 | ai:claude

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use colored::Colorize;
use serde::Serialize;

use aida_core::{
    QueueEntry, Relationship, RelationshipType, Requirement, RequirementPriority,
    RequirementStatus, RequirementType, Storage,
};

use crate::burndown::{self, BurndownCandidate, Pickability};
use crate::cli::BacklogCommand;
use crate::{
    apply_tag_deltas, batch_tag_of, current_user_id, find_plan_files_for_spec, find_project_root,
    format_tag_chip, normalize_batch_name, parse_plan_critical_files, parse_priority,
    parse_requirement_type, scan_trace_graph, tag_matches_exact, tag_matches_prefix,
};

const LOW_RISK_TAGS: &[&str] = &[
    "papercut",
    "cosmetic",
    "severity:cosmetic",
    "lifecycle:trivial",
    "docs-only",
    "fmt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RiskLevel {
    Low,
    Medium,
    High,
    Unknown,
}

impl RiskLevel {
    fn chip(self) -> String {
        match self {
            RiskLevel::Low => "low".green().to_string(),
            RiskLevel::Medium => "med".yellow().to_string(),
            RiskLevel::High => "high".red().to_string(),
            RiskLevel::Unknown => "?".dimmed().to_string(),
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(RiskLevel::Low),
            "medium" | "med" => Ok(RiskLevel::Medium),
            "high" => Ok(RiskLevel::High),
            "unknown" | "?" => Ok(RiskLevel::Unknown),
            other => Err(anyhow!(
                "unknown risk level `{other}` — expected one of: low, medium, high, unknown"
            )),
        }
    }

    /// Plain (uncolored) lowercase token — for env passing + machine output,
    /// where the ANSI-colored [`RiskLevel::chip`] would be noise. trace:STORY-560
    pub(crate) fn token(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Unknown => "unknown",
        }
    }

    /// Ordering for the `--risk <max>` ceiling. Higher = riskier / less safe to
    /// auto-select. `Unknown` ranks above `Medium` (we don't know its blast
    /// radius, so admit it only when the operator explicitly allows `high`).
    /// trace:STORY-554 | ai:claude
    fn rank(self) -> u8 {
        match self {
            RiskLevel::Low => 0,
            RiskLevel::Medium => 1,
            RiskLevel::Unknown => 2,
            RiskLevel::High => 3,
        }
    }

    /// True when a candidate at `self` risk is admitted under a `--risk max`
    /// ceiling. `--risk high` admits everything; `--risk low` only low.
    pub(crate) fn within_ceiling(self, max: RiskLevel) -> bool {
        self.rank() <= max.rank()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum PairVerdict {
    SafeParallel,
    Serialize { shared: Vec<String> },
    Unknown,
}

impl PairVerdict {
    fn label(&self) -> String {
        match self {
            PairVerdict::SafeParallel => "safe-parallel".green().to_string(),
            PairVerdict::Serialize { shared } => format!(
                "{}: shares {}",
                "serialize".red(),
                shared.join(", ").dimmed()
            ),
            PairVerdict::Unknown => "unknown (no signals)".dimmed().to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct BacklogListRow {
    pub spec_id: String,
    pub title: String,
    pub req_type: String,
    pub priority: String,
    pub risk: RiskLevel,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BacklogListReport {
    pub rows: Vec<BacklogListRow>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AnalyzePair {
    pub a: String,
    pub b: String,
    pub verdict: String,
    pub shared_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AnalyzeReport {
    pub specs: Vec<String>,
    pub pairs: Vec<AnalyzePair>,
}

/// Classify a backlog candidate's blast radius from its type, priority,
/// tags, and relationships. Heuristic, not gating — `groom` always defers
/// to the operator. trace:STORY-444 | ai:claude
pub(crate) fn classify_risk(req: &Requirement, has_plan: bool) -> RiskLevel {
    let high_type = matches!(
        req.req_type,
        RequirementType::Story | RequirementType::Epic | RequirementType::Spike
    );
    let high_priority = matches!(req.priority, RequirementPriority::High);
    let blocked_by_or_child = req.relationships.iter().any(|r: &Relationship| {
        matches!(
            r.rel_type,
            RelationshipType::BlockedBy | RelationshipType::Child
        )
    });
    if high_type || high_priority || blocked_by_or_child {
        return RiskLevel::High;
    }

    let low_type = matches!(req.req_type, RequirementType::Task | RequirementType::Doc);
    let low_priority = matches!(req.priority, RequirementPriority::Low);
    let lowered = req
        .tags
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let has_low_tag = LOW_RISK_TAGS.iter().any(|t| lowered.contains(*t));
    if low_type && low_priority && has_low_tag {
        return RiskLevel::Low;
    }

    let med_type = matches!(req.req_type, RequirementType::Task | RequirementType::Bug);
    let med_priority = matches!(req.priority, RequirementPriority::Medium);
    if (med_type && med_priority) || has_plan {
        return RiskLevel::Medium;
    }

    RiskLevel::Unknown
}

fn collect_backlog_candidates(
    store: &aida_core::RequirementsStore,
    queued_ids: &HashSet<uuid::Uuid>,
) -> Vec<Requirement> {
    let mut out: Vec<Requirement> = store
        .requirements
        .iter()
        // Approved is non-terminal by definition; no defense-in-depth
        // !is_terminal_status filter needed. trace:TASK-536 | ai:claude
        .filter(|r| matches!(r.status, RequirementStatus::Approved))
        .filter(|r| !r.archived)
        .filter(|r| !queued_ids.contains(&r.id))
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        let pa = priority_rank(&a.priority);
        let pb = priority_rank(&b.priority);
        pb.cmp(&pa).then(a.created_at.cmp(&b.created_at))
    });
    out
}

fn priority_rank(p: &RequirementPriority) -> u8 {
    match p {
        RequirementPriority::High => 3,
        RequirementPriority::Medium => 2,
        RequirementPriority::Low => 1,
    }
}

/// Union of the spec's trace-comment file references and the
/// `## Critical Files` paths from any plan that owns the spec. Empty when
/// no signals are found — the caller treats that as Unknown.
pub(crate) fn collect_spec_files(project_root: &Path, spec_id: &str) -> BTreeSet<String> {
    let mut wanted = HashSet::new();
    wanted.insert(spec_id.to_string());
    let trace_hits = scan_trace_graph(project_root, &wanted);
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Some(hits) = trace_hits.get(spec_id) {
        for h in hits {
            out.insert(h.file.clone());
        }
    }
    for plan in find_plan_files_for_spec(project_root, spec_id) {
        if let Ok(content) = std::fs::read_to_string(&plan) {
            for p in parse_plan_critical_files(&content) {
                out.insert(p);
            }
        }
    }
    out
}

pub(crate) fn classify_pair_overlap(
    a_files: &BTreeSet<String>,
    b_files: &BTreeSet<String>,
) -> PairVerdict {
    if a_files.is_empty() || b_files.is_empty() {
        return PairVerdict::Unknown;
    }
    let shared: Vec<String> = a_files.intersection(b_files).cloned().collect();
    if shared.is_empty() {
        PairVerdict::SafeParallel
    } else {
        PairVerdict::Serialize { shared }
    }
}

pub(crate) fn handle_backlog_command(cmd: &BacklogCommand, storage: &Storage) -> Result<()> {
    match cmd {
        BacklogCommand::List {
            risk,
            r#type,
            priority,
            tag,
            tag_prefix,
            limit,
            json,
            user,
        } => handle_list(
            storage,
            risk.as_deref(),
            r#type.as_deref(),
            priority.as_deref(),
            tag.as_deref(),
            tag_prefix.as_deref(),
            *limit,
            *json,
            user.as_deref(),
        ),
        BacklogCommand::Analyze { specs, pair, json } => {
            let merged = merge_spec_csv_and_pair(specs.as_deref(), pair.as_deref())?;
            handle_analyze(storage, &merged, *json)
        }
        BacklogCommand::Groom {
            specs,
            from_stdin,
            pickable,
            risk,
            apply,
            batch,
            dry_run,
            note,
            user,
        } => {
            if *pickable {
                let max_risk = RiskLevel::parse(risk)?;
                handle_groom_pickable(
                    storage,
                    max_risk,
                    *apply,
                    batch.as_deref().map(normalize_batch_name),
                    note.as_deref(),
                    user.as_deref(),
                )
            } else {
                let ids = resolve_groom_ids(specs.as_deref(), *from_stdin)?;
                handle_groom(
                    storage,
                    &ids,
                    batch.as_deref().map(normalize_batch_name),
                    *dry_run,
                    note.as_deref(),
                    user.as_deref(),
                )
            }
        }
        BacklogCommand::Load => {
            anyhow::bail!("`aida backlog load` is handled by the load-report dispatcher")
        }
    }
}

fn merge_spec_csv_and_pair(specs: Option<&str>, pair: Option<&[String]>) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim();
        if !t.is_empty() && !out.iter().any(|s| s == t) {
            out.push(t.to_string());
        }
    };
    if let Some(raw) = specs {
        for s in raw.split(',') {
            push(s);
        }
    }
    if let Some(p) = pair {
        for s in p {
            push(s);
        }
    }
    if out.len() < 2 {
        anyhow::bail!(
            "aida backlog analyze needs at least two spec ids — pass `--specs A,B,C` or `--pair A B`"
        );
    }
    Ok(out)
}

fn resolve_groom_ids(specs: Option<&str>, from_stdin: bool) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let t = raw.trim();
        if !t.is_empty() && !out.iter().any(|s| s == t) {
            out.push(t.to_string());
        }
    };
    if from_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading spec ids from stdin")?;
        for line in buf.lines() {
            for tok in line.split(|c: char| c == ',' || c.is_whitespace()) {
                push(tok);
            }
        }
    } else if let Some(raw) = specs {
        for s in raw.split(',') {
            push(s);
        }
    } else {
        anyhow::bail!(
            "aida backlog groom needs `--specs A,B,C` or `--from-stdin` (interactive selection is the skill's job)"
        );
    }
    if out.is_empty() {
        anyhow::bail!("aida backlog groom received no spec ids");
    }
    Ok(out)
}

fn lookup_spec<'a>(store: &'a aida_core::RequirementsStore, id: &str) -> Option<&'a Requirement> {
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(id)
    }
}

fn display_id(req: &Requirement) -> &str {
    req.agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or("???")
}

#[allow(clippy::too_many_arguments)]
fn handle_list(
    storage: &Storage,
    risk: Option<&str>,
    type_filter: Option<&str>,
    priority_filter: Option<&str>,
    tag: Option<&str>,
    tag_prefix: Option<&str>,
    limit: usize,
    json: bool,
    user: Option<&str>,
) -> Result<()> {
    let risk_filter = risk.map(RiskLevel::parse).transpose()?;
    let type_filter = type_filter.map(parse_requirement_type).transpose()?;
    let priority_filter = priority_filter.map(parse_priority).transpose()?;

    let store = storage.load()?;
    let user_id = current_user_id(user);
    let queued_ids = collect_queued_ids(storage, &user_id)?;
    let project_root = find_project_root().unwrap_or_else(|_| PathBuf::from("."));

    let candidates = collect_backlog_candidates(&store, &queued_ids);
    let mut rows: Vec<BacklogListRow> = Vec::new();
    for req in &candidates {
        if let Some(t) = &type_filter {
            if &req.req_type != t {
                continue;
            }
        }
        if let Some(p) = &priority_filter {
            if &req.priority != p {
                continue;
            }
        }
        if let Some(want) = tag {
            if !tag_matches_exact(&req.tags, want) {
                continue;
            }
        }
        if let Some(pfx) = tag_prefix {
            if !tag_matches_prefix(&req.tags, pfx) {
                continue;
            }
        }
        let spec_id = display_id(req).to_string();
        let has_plan = !find_plan_files_for_spec(&project_root, &spec_id).is_empty();
        let risk_level = classify_risk(req, has_plan);
        if let Some(want) = risk_filter {
            if risk_level != want {
                continue;
            }
        }
        let mut tags_sorted: Vec<String> = req.tags.iter().cloned().collect();
        tags_sorted.sort();
        rows.push(BacklogListRow {
            spec_id,
            title: req.title.clone(),
            req_type: req.req_type.to_string(),
            priority: req.priority.to_string(),
            risk: risk_level,
            tags: tags_sorted,
        });
        if rows.len() >= limit {
            break;
        }
    }

    if json {
        let report = BacklogListReport { rows };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    render_list_table(&rows);
    Ok(())
}

fn collect_queued_ids(storage: &Storage, user_id: &str) -> Result<HashSet<uuid::Uuid>> {
    let entries = storage
        .queue_list(user_id, true)
        .context("listing queue entries")?;
    Ok(entries.iter().map(|e| e.requirement_id).collect())
}

fn render_list_table(rows: &[BacklogListRow]) {
    if rows.is_empty() {
        println!("(no Approved items in the backlog match the filters)");
        return;
    }
    println!(
        "{:<14}  {:<6}  {:<5}  {:<7}  {}",
        "SPEC".bold(),
        "TYPE".bold(),
        "PRI".bold(),
        "RISK".bold(),
        "TITLE".bold(),
    );
    for r in rows {
        let chip = r.risk.chip();
        let tag_set: HashSet<String> = r.tags.iter().cloned().collect();
        let tag_chip = format_tag_chip(&tag_set)
            .map(|s| format!("  [{}]", s.dimmed()))
            .unwrap_or_default();
        println!(
            "{:<14}  {:<6}  {:<5}  {:<7}  {}{}",
            r.spec_id.bold(),
            short_type(&r.req_type),
            short_priority(&r.priority),
            chip,
            r.title,
            tag_chip,
        );
    }
    println!();
    println!(
        "{}",
        "Risk chips are advisory — the operator picks (see `aida backlog groom --help`)".dimmed()
    );
}

fn short_type(t: &str) -> &str {
    match t {
        "Story" => "story",
        "Task" => "task",
        "Bug" => "bug",
        "Epic" => "epic",
        "Spike" => "spike",
        "Doc" => "doc",
        _ => t,
    }
}

fn short_priority(p: &str) -> &str {
    match p {
        "High" => "high",
        "Medium" => "med",
        "Low" => "low",
        _ => p,
    }
}

fn handle_analyze(storage: &Storage, ids: &[String], json: bool) -> Result<()> {
    let store = storage.load()?;
    let project_root = find_project_root().unwrap_or_else(|_| PathBuf::from("."));

    let mut resolved: Vec<(String, BTreeSet<String>)> = Vec::new();
    for raw in ids {
        let req = lookup_spec(&store, raw).ok_or_else(|| {
            anyhow!("spec `{raw}` not found — pass a SPEC-ID (e.g. `STORY-N`), agreed id, or UUID")
        })?;
        let spec = display_id(req).to_string();
        if resolved.iter().any(|(s, _)| s == &spec) {
            continue;
        }
        let files = collect_spec_files(&project_root, &spec);
        resolved.push((spec, files));
    }

    let mut pairs: Vec<AnalyzePair> = Vec::new();
    for i in 0..resolved.len() {
        for j in (i + 1)..resolved.len() {
            let (a, af) = &resolved[i];
            let (b, bf) = &resolved[j];
            let verdict = classify_pair_overlap(af, bf);
            let (label, shared) = match &verdict {
                PairVerdict::SafeParallel => ("safe-parallel".to_string(), Vec::new()),
                PairVerdict::Serialize { shared } => ("serialize".to_string(), shared.clone()),
                PairVerdict::Unknown => ("unknown".to_string(), Vec::new()),
            };
            pairs.push(AnalyzePair {
                a: a.clone(),
                b: b.clone(),
                verdict: label,
                shared_files: shared,
            });
        }
    }

    if json {
        let report = AnalyzeReport {
            specs: resolved.iter().map(|(s, _)| s.clone()).collect(),
            pairs,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    render_analyze_text(&resolved, &pairs);
    Ok(())
}

fn render_analyze_text(resolved: &[(String, BTreeSet<String>)], pairs: &[AnalyzePair]) {
    println!("Analyzed {} specs:", resolved.len());
    for (spec, files) in resolved {
        if files.is_empty() {
            println!(
                "  {} {}",
                spec.bold(),
                "(no trace + no plan signals)".dimmed()
            );
        } else {
            println!(
                "  {} {} files",
                spec.bold(),
                format!("{}", files.len()).dimmed()
            );
        }
    }
    println!();
    if pairs.is_empty() {
        println!("(no pairs to compare)");
        return;
    }
    let unknown = pairs.iter().any(|p| p.verdict == "unknown");
    for p in pairs {
        let verdict = pair_verdict_from(p);
        println!("  {} ↔ {}  →  {}", p.a.bold(), p.b.bold(), verdict.label());
    }
    if unknown {
        println!();
        println!(
            "{}",
            "?  unknown = no trace comments and no plan file on this side — treat as serialize"
                .dimmed()
        );
    }
}

fn pair_verdict_from(p: &AnalyzePair) -> PairVerdict {
    match p.verdict.as_str() {
        "safe-parallel" => PairVerdict::SafeParallel,
        "serialize" => PairVerdict::Serialize {
            shared: p.shared_files.clone(),
        },
        _ => PairVerdict::Unknown,
    }
}

/// Place one already-resolved, eligibility-checked requirement onto the
/// caller's queue, mirroring the `aida backlog groom` enqueue (user resolution
/// via `current_user_id`, `AIDA_SESSION_ROLE` routing, optional note, optional
/// `batch:NAME` tag). Shared between `aida backlog groom` and `aida add
/// --queue` so the two surfaces stay byte-for-byte consistent.
/// trace:TASK-754 | ai:claude
pub(crate) fn enqueue_groomed(
    storage: &Storage,
    req: &Requirement,
    batch: Option<&str>,
    note: Option<&str>,
    user_id: &str,
) -> Result<()> {
    let entry = QueueEntry {
        user_id: user_id.to_string(),
        requirement_id: req.id,
        position: i64::MAX,
        added_by: user_id.to_string(),
        note: note.map(str::to_string),
        added_at: Utc::now(),
        for_role: std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry)?;

    if let Some(name) = batch {
        let tag = format!("batch:{}", name);
        // Re-load the full store, mutate the spec's tag set, and save
        // the whole store — same pattern `Command::Edit { add_tag }`
        // uses (`storage.save(&store)?`).
        let mut store_now = storage.load()?;
        if let Some(slot) = store_now.requirements.iter_mut().find(|r| r.id == req.id) {
            let already = batch_tag_of(&slot.tags)
                .map(|t| t.eq_ignore_ascii_case(&tag))
                .unwrap_or(false);
            if !already {
                let adds = vec![tag.clone()];
                let removes: Vec<String> = Vec::new();
                let changed = apply_tag_deltas(&mut slot.tags, &adds, &removes);
                if changed {
                    slot.modified_at = Utc::now();
                    storage
                        .save(&store_now)
                        .with_context(|| format!("tagging {} batch:{}", display_id(req), name))?;
                }
            }
        }
    }
    Ok(())
}

fn handle_groom(
    storage: &Storage,
    ids: &[String],
    batch: Option<&str>,
    dry_run: bool,
    note: Option<&str>,
    user: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;
    let user_id = current_user_id(user);
    let queued_ids = collect_queued_ids(storage, &user_id)?;

    // Validate every id upfront — refuse the whole groom on the first
    // invalid one to avoid a half-applied state the operator has to undo
    // by hand.
    let mut to_groom: Vec<Requirement> = Vec::new();
    for raw in ids {
        let req = lookup_spec(&store, raw).ok_or_else(|| {
            anyhow!("spec `{raw}` not found — pass a SPEC-ID (e.g. `STORY-N`), agreed id, or UUID")
        })?;
        if !matches!(req.status, RequirementStatus::Approved) {
            anyhow::bail!(
                "spec `{}` is `{}`, not Approved — `aida backlog groom` only moves Approved work onto the queue. \
                 Bump status with `aida edit {} --status approved` first if appropriate.",
                display_id(req),
                req.status,
                display_id(req)
            );
        }
        if queued_ids.contains(&req.id) {
            anyhow::bail!(
                "spec `{}` is already in the queue — remove it first with `aida queue remove {}` if you want to re-groom",
                display_id(req),
                display_id(req)
            );
        }
        if req.archived {
            anyhow::bail!(
                "spec `{}` is archived — un-archive before grooming",
                display_id(req)
            );
        }
        to_groom.push(req.clone());
    }

    if dry_run {
        println!(
            "{} (no writes — dry run)",
            "aida backlog groom --dry-run".bold()
        );
        for req in &to_groom {
            print_groom_line(req, batch);
        }
        let suffix = batch
            .map(|n| format!(", each tagged `batch:{}`", n))
            .unwrap_or_default();
        println!();
        println!(
            "Would queue {} item(s){}.",
            to_groom.len().to_string().bold(),
            suffix
        );
        return Ok(());
    }

    let mut updated = 0usize;
    for req in &to_groom {
        enqueue_groomed(storage, req, batch, note, &user_id)
            .with_context(|| format!("queueing {}", display_id(req)))?;
        updated += 1;
        print_groom_line(req, batch);
    }

    println!();
    if let Some(name) = batch {
        println!(
            "{} Groomed {} item(s) into the queue, tagged `batch:{}`.",
            "✓".green(),
            updated.to_string().bold(),
            name.bold()
        );
        println!(
            "  Drain with: {}",
            format!("aida queue work --batch {}", name).bold()
        );
    } else {
        println!(
            "{} Groomed {} item(s) into the queue.",
            "✓".green(),
            updated.to_string().bold()
        );
    }
    Ok(())
}

fn print_groom_line(req: &Requirement, batch: Option<&str>) {
    let did = display_id(req);
    let tag_suffix = batch
        .map(|n| format!("  [tagged {}]", format!("batch:{}", n).dimmed()))
        .unwrap_or_default();
    println!(
        "  {} {}  —  {}{}",
        "→".dimmed(),
        did.bold(),
        req.title,
        tag_suffix
    );
}

/// One backlog item paired with its advisory risk chip and the already-probed
/// burndown gate facts. The pure [`select_pickable`] consumes a slice of these
/// so the auto-selection logic is filesystem-free and unit-testable.
/// trace:STORY-554 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct PickableItem {
    pub id: String,
    pub risk: RiskLevel,
    pub candidate: BurndownCandidate,
}

/// Why an item was held back from auto-selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParkReason {
    /// Failed the burndown pickability gate (epic / blocked / pending decision
    /// / parking-tag). Carries the gate's own human-readable reason — single
    /// source of truth with the burndown.
    Gate(String),
    /// Passed the gate but is riskier than the `--risk <max>` ceiling.
    RiskCeiling(RiskLevel),
}

impl ParkReason {
    fn label(&self) -> String {
        match self {
            ParkReason::Gate(r) => r.clone(),
            ParkReason::RiskCeiling(level) => {
                format!("risk {} exceeds ceiling", level.chip())
            }
        }
    }
}

/// Pure auto-selection: apply the burndown pickability gate (reused verbatim via
/// [`burndown::classify`]) then the `--risk <max>` ceiling, partitioning the
/// backlog into the would-groom ids and the would-park items with reasons.
/// Order-preserving so output mirrors the input ranking. trace:STORY-554
pub(crate) fn select_pickable(
    items: &[PickableItem],
    max_risk: RiskLevel,
) -> (Vec<String>, Vec<(String, ParkReason)>) {
    let mut groom: Vec<String> = Vec::new();
    let mut park: Vec<(String, ParkReason)> = Vec::new();
    for item in items {
        // Gate first — a parked spec is parked regardless of its risk chip, and
        // the gate reason is the more fundamental signal.
        match burndown::classify(&item.candidate) {
            Pickability::Parked(reason) => {
                park.push((item.id.clone(), ParkReason::Gate(reason)));
                continue;
            }
            Pickability::Ready => {}
        }
        if item.risk.within_ceiling(max_risk) {
            groom.push(item.id.clone());
        } else {
            park.push((item.id.clone(), ParkReason::RiskCeiling(item.risk)));
        }
    }
    (groom, park)
}

#[allow(clippy::too_many_arguments)]
fn handle_groom_pickable(
    storage: &Storage,
    max_risk: RiskLevel,
    apply: bool,
    batch: Option<&str>,
    note: Option<&str>,
    user: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;
    let user_id = current_user_id(user);
    let queued_ids = collect_queued_ids(storage, &user_id)?;
    let project_root = find_project_root().unwrap_or_else(|_| PathBuf::from("."));

    // The backlog set = Approved-but-not-queued, ranked exactly as
    // `aida backlog list` ranks it (priority then age). Build a PickableItem per
    // member: its advisory risk chip (reused `classify_risk`) + the burndown
    // gate facts (reused `BurndownCandidate` construction). trace:STORY-554
    let candidates = collect_backlog_candidates(&store, &queued_ids);
    let mut items: Vec<PickableItem> = Vec::with_capacity(candidates.len());
    // id → req, so we can resolve the would-groom set back to requirements for
    // the enqueue write without re-scanning the store.
    let mut by_id: std::collections::HashMap<String, Requirement> =
        std::collections::HashMap::new();
    for req in &candidates {
        let spec_id = display_id(req).to_string();
        let has_plan = !find_plan_files_for_spec(&project_root, &spec_id).is_empty();
        let risk = classify_risk(req, has_plan);
        let has_unsatisfied_blocker = aida_core::pickability::blocked_by_incomplete(req, &store);
        let has_pending_decision = req
            .decision_request
            .as_ref()
            .map(|d| d.is_pending())
            .unwrap_or(false);
        let candidate = BurndownCandidate {
            id: spec_id.clone(),
            req_type: format!("{:?}", req.req_type).to_ascii_lowercase(),
            tags: req.tags.iter().cloned().collect(),
            has_unsatisfied_blocker,
            has_pending_decision,
        };
        items.push(PickableItem {
            id: spec_id.clone(),
            risk,
            candidate,
        });
        by_id.insert(spec_id, req.clone());
    }

    let (groom_ids, parked) = select_pickable(&items, max_risk);

    // ---- DRY-RUN BY DEFAULT ----
    if !apply {
        render_pickable_dry_run(&groom_ids, &parked, &by_id, max_risk, batch);
        return Ok(());
    }

    // ---- APPLY: write the survivors via the same enqueue path normal groom uses.
    if groom_ids.is_empty() {
        println!(
            "{} No pickable backlog items at risk ≤ {} — nothing to groom.",
            "•".dimmed(),
            max_risk.chip()
        );
        return Ok(());
    }
    let mut updated = 0usize;
    for id in &groom_ids {
        let req = by_id
            .get(id)
            .ok_or_else(|| anyhow!("internal: pickable id `{id}` lost between select and apply"))?;
        enqueue_groomed(storage, req, batch, note, &user_id)
            .with_context(|| format!("queueing {}", id))?;
        updated += 1;
        print_groom_line(req, batch);
    }
    println!();
    if let Some(name) = batch {
        println!(
            "{} Auto-groomed {} pickable item(s) into the queue, tagged `batch:{}`.",
            "✓".green(),
            updated.to_string().bold(),
            name.bold()
        );
        println!(
            "  Drain with: {}",
            format!("aida queue work --batch {}", name).bold()
        );
    } else {
        println!(
            "{} Auto-groomed {} pickable item(s) into the queue.",
            "✓".green(),
            updated.to_string().bold()
        );
    }
    if !parked.is_empty() {
        println!(
            "  {} {} item(s) parked (gate / risk ceiling) — see `aida backlog groom --pickable` (dry run) for reasons.",
            "•".dimmed(),
            parked.len().to_string().bold()
        );
    }
    Ok(())
}

fn render_pickable_dry_run(
    groom_ids: &[String],
    parked: &[(String, ParkReason)],
    by_id: &std::collections::HashMap<String, Requirement>,
    max_risk: RiskLevel,
    batch: Option<&str>,
) {
    println!(
        "{} (no writes — pass {} to queue these)",
        "aida backlog groom --pickable".bold(),
        "--apply".bold()
    );
    println!(
        "{}",
        format!("risk ceiling: ≤ {}", max_risk.chip()).dimmed()
    );
    println!();

    println!("{}", "Would groom (pickable + within risk):".bold());
    if groom_ids.is_empty() {
        println!("  {}", "(none)".dimmed());
    } else {
        for id in groom_ids {
            let title = by_id.get(id).map(|r| r.title.as_str()).unwrap_or("");
            println!("  {} {}  —  {}", "→".green(), id.bold(), title);
        }
    }

    println!();
    println!("{}", "Would park (held back):".bold());
    if parked.is_empty() {
        println!("  {}", "(none)".dimmed());
    } else {
        for (id, reason) in parked {
            let title = by_id.get(id).map(|r| r.title.as_str()).unwrap_or("");
            println!(
                "  {} {}  —  {}  {}",
                "⊘".dimmed(),
                id.bold(),
                title,
                format!("[{}]", reason.label()).dimmed()
            );
        }
    }

    println!();
    let suffix = batch
        .map(|n| format!(", each tagged `batch:{}`", n))
        .unwrap_or_default();
    println!(
        "Would queue {} item(s){}; {} parked.",
        groom_ids.len().to_string().bold(),
        suffix,
        parked.len().to_string().bold()
    );
    println!(
        "{}",
        "Tip: run `aida questions` first — the gate's \"decision-free\" check only catches \
         attached DecisionRequests, not every design-latitude spec. Keep this dry-run review."
            .dimmed()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn req_fixture(
        spec: &str,
        ty: RequirementType,
        prio: RequirementPriority,
        status: RequirementStatus,
        tags: &[&str],
    ) -> Requirement {
        let mut t: HashSet<String> = HashSet::new();
        for tag in tags {
            t.insert(tag.to_string());
        }
        Requirement {
            id: uuid::Uuid::new_v4(),
            spec_id: Some(spec.to_string()),
            agreed_id: None,
            prefix_override: None,
            title: format!("fixture {}", spec),
            description: String::new(),
            status,
            priority: prio,
            owner: "test".into(),
            feature: "fixture".into(),
            created_at: Utc::now(),
            created_by: None,
            modified_at: Utc::now(),
            req_type: ty,
            meta_subtype: None,
            dependencies: Vec::new(),
            tags: t,
            weight: None,
            relationships: Vec::new(),
            comments: Vec::new(),
            history: Vec::new(),
            archived: false,
            archived_at: None,
            custom_status: None,
            custom_priority: None,
            custom_fields: Default::default(),
            urls: Vec::new(),
            attachments: Vec::new(),
            trace_links: Vec::new(),
            gitlab_issues: Vec::new(),
            // trace:STORY-476 | ai:claude
            external_refs: Vec::new(),
            implementation_info: None,
            ai_evaluation: None,
            attention_reason: None,
            failure_reason: None,
            human_only: false,
            // trace:STORY-522 | ai:claude
            decision_request: None,
            // trace:STORY-542 | ai:claude
            interface_changes: None,
            version: 1,
        }
    }

    #[test]
    fn classify_risk_low_for_papercut_task() {
        let r = req_fixture(
            "TASK-1",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &["papercut"],
        );
        assert_eq!(classify_risk(&r, false), RiskLevel::Low);
    }

    #[test]
    fn classify_risk_low_for_lifecycle_trivial() {
        let r = req_fixture(
            "TASK-2",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &["lifecycle:trivial"],
        );
        assert_eq!(classify_risk(&r, false), RiskLevel::Low);
    }

    #[test]
    fn classify_risk_high_for_story_even_at_low_priority() {
        let r = req_fixture(
            "STORY-7",
            RequirementType::Story,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &["papercut"],
        );
        assert_eq!(classify_risk(&r, false), RiskLevel::High);
    }

    #[test]
    fn classify_risk_medium_when_plan_owns_spec() {
        let r = req_fixture(
            "TASK-5",
            RequirementType::Task,
            RequirementPriority::Medium,
            RequirementStatus::Approved,
            &[],
        );
        assert_eq!(classify_risk(&r, true), RiskLevel::Medium);
    }

    #[test]
    fn classify_risk_high_when_blocked_by_present() {
        let mut r = req_fixture(
            "TASK-6",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &["papercut"],
        );
        r.relationships.push(Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id: uuid::Uuid::new_v4(),
            created_at: None,
            created_by: None,
        });
        assert_eq!(classify_risk(&r, false), RiskLevel::High);
    }

    #[test]
    fn classify_risk_unknown_for_uncategorized() {
        let r = req_fixture(
            "TASK-9",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &["random-tag"],
        );
        assert_eq!(classify_risk(&r, false), RiskLevel::Unknown);
    }

    #[test]
    fn collect_backlog_candidates_excludes_queued_and_terminal() {
        let approved_queued = req_fixture(
            "T-1",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &[],
        );
        let approved_unqueued = req_fixture(
            "T-2",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Approved,
            &[],
        );
        let completed = req_fixture(
            "T-3",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Completed,
            &[],
        );
        let draft = req_fixture(
            "T-4",
            RequirementType::Task,
            RequirementPriority::Low,
            RequirementStatus::Draft,
            &[],
        );
        let store = aida_core::RequirementsStore {
            requirements: vec![
                approved_queued.clone(),
                approved_unqueued.clone(),
                completed,
                draft,
            ],
            ..Default::default()
        };
        let mut queued = HashSet::new();
        queued.insert(approved_queued.id);
        let got = collect_backlog_candidates(&store, &queued);
        assert_eq!(got.len(), 1, "only the Approved-unqueued spec is in scope");
        assert_eq!(got[0].id, approved_unqueued.id);
    }

    #[test]
    fn classify_pair_overlap_safe_when_no_shared_files() {
        let a: BTreeSet<String> = ["aida-cli/src/foo.rs".to_string()].into_iter().collect();
        let b: BTreeSet<String> = ["aida-cli/src/bar.rs".to_string()].into_iter().collect();
        assert!(matches!(
            classify_pair_overlap(&a, &b),
            PairVerdict::SafeParallel
        ));
    }

    #[test]
    fn classify_pair_overlap_serialize_when_shared_file() {
        let a: BTreeSet<String> = ["a.rs".to_string(), "shared.rs".to_string()]
            .into_iter()
            .collect();
        let b: BTreeSet<String> = ["shared.rs".to_string(), "b.rs".to_string()]
            .into_iter()
            .collect();
        match classify_pair_overlap(&a, &b) {
            PairVerdict::Serialize { shared } => {
                assert_eq!(shared, vec!["shared.rs".to_string()]);
            }
            other => panic!("expected Serialize, got {other:?}"),
        }
    }

    #[test]
    fn classify_pair_overlap_unknown_when_either_empty() {
        let a: BTreeSet<String> = BTreeSet::new();
        let b: BTreeSet<String> = ["b.rs".to_string()].into_iter().collect();
        assert!(matches!(
            classify_pair_overlap(&a, &b),
            PairVerdict::Unknown
        ));
    }

    #[test]
    fn analyze_report_json_shape_is_stable() {
        let report = AnalyzeReport {
            specs: vec!["TASK-1".into(), "TASK-2".into(), "TASK-3".into()],
            pairs: vec![AnalyzePair {
                a: "TASK-1".into(),
                b: "TASK-2".into(),
                verdict: "safe-parallel".into(),
                shared_files: Vec::new(),
            }],
        };
        let v = serde_json::to_value(&report).expect("serializes");
        let pair = &v["pairs"][0];
        assert!(pair.get("a").is_some());
        assert!(pair.get("b").is_some());
        assert!(pair.get("verdict").is_some());
        assert!(pair.get("shared_files").is_some());
    }

    #[test]
    fn backlog_list_row_json_shape_is_stable() {
        let row = BacklogListRow {
            spec_id: "TASK-1".into(),
            title: "t".into(),
            req_type: "Task".into(),
            priority: "Low".into(),
            risk: RiskLevel::Low,
            tags: vec!["papercut".into()],
        };
        let v = serde_json::to_value(&row).expect("serializes");
        for key in ["spec_id", "title", "req_type", "priority", "risk", "tags"] {
            assert!(v.get(key).is_some(), "row JSON missing field {key}");
        }
        assert_eq!(v["risk"], serde_json::Value::String("low".into()));
    }

    #[test]
    fn merge_spec_csv_and_pair_dedupes() {
        let merged =
            merge_spec_csv_and_pair(Some("A,B,C"), Some(&["B".into(), "D".into()])).unwrap();
        assert_eq!(merged, vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn merge_spec_csv_and_pair_requires_two() {
        let err = merge_spec_csv_and_pair(Some("A"), None).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("at least two"),
            "error should mention the minimum count, got: {msg}"
        );
    }

    fn pickable(id: &str, risk: RiskLevel, candidate: BurndownCandidate) -> PickableItem {
        PickableItem {
            id: id.to_string(),
            risk,
            candidate,
        }
    }

    fn cand(
        id: &str,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
    ) -> BurndownCandidate {
        BurndownCandidate {
            id: id.to_string(),
            req_type: req_type.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
        }
    }

    #[test]
    fn select_pickable_grooms_clean_low_risk_task() {
        let items = vec![pickable(
            "TASK-1",
            RiskLevel::Low,
            cand("TASK-1", "task", &["papercut"], false, false),
        )];
        let (groom, park) = select_pickable(&items, RiskLevel::Medium);
        assert_eq!(groom, vec!["TASK-1".to_string()]);
        assert!(park.is_empty());
    }

    #[test]
    fn select_pickable_parks_epic_via_gate_reusing_burndown() {
        let items = vec![pickable(
            "EPIC-1",
            RiskLevel::Low,
            cand("EPIC-1", "epic", &[], false, false),
        )];
        let (groom, park) = select_pickable(&items, RiskLevel::High);
        assert!(groom.is_empty());
        assert_eq!(park.len(), 1);
        match &park[0].1 {
            ParkReason::Gate(r) => assert!(r.contains("decompose"), "gate reason: {r}"),
            other => panic!("expected gate park, got {other:?}"),
        }
    }

    #[test]
    fn select_pickable_parks_pending_decision_and_blocked_via_gate() {
        let items = vec![
            pickable(
                "TASK-2",
                RiskLevel::Low,
                cand("TASK-2", "task", &[], false, true),
            ),
            pickable(
                "TASK-3",
                RiskLevel::Low,
                cand("TASK-3", "task", &[], true, false),
            ),
        ];
        let (groom, park) = select_pickable(&items, RiskLevel::High);
        assert!(groom.is_empty());
        assert_eq!(park.len(), 2);
        assert!(park.iter().all(|(_, r)| matches!(r, ParkReason::Gate(_))));
    }

    #[test]
    fn select_pickable_parks_parking_tagged_via_gate() {
        let items = vec![pickable(
            "TASK-4",
            RiskLevel::Low,
            cand("TASK-4", "task", &["needs-decision"], false, false),
        )];
        let (groom, park) = select_pickable(&items, RiskLevel::High);
        assert!(groom.is_empty());
        match &park[0].1 {
            ParkReason::Gate(r) => assert!(r.to_lowercase().contains("needs-decision")),
            other => panic!("expected gate park, got {other:?}"),
        }
    }

    #[test]
    fn select_pickable_risk_ceiling_excludes_higher_risk() {
        // Gate-clean (task, no blockers/decision) but high risk.
        let items = vec![pickable(
            "TASK-5",
            RiskLevel::High,
            cand("TASK-5", "task", &[], false, false),
        )];
        // Default-medium ceiling: parked on risk.
        let (groom, park) = select_pickable(&items, RiskLevel::Medium);
        assert!(groom.is_empty());
        assert_eq!(park[0].1, ParkReason::RiskCeiling(RiskLevel::High));
        // --risk high admits it.
        let (groom_hi, park_hi) = select_pickable(&items, RiskLevel::High);
        assert_eq!(groom_hi, vec!["TASK-5".to_string()]);
        assert!(park_hi.is_empty());
    }

    #[test]
    fn select_pickable_low_ceiling_admits_only_low() {
        let items = vec![
            pickable(
                "TASK-6",
                RiskLevel::Low,
                cand("TASK-6", "task", &[], false, false),
            ),
            pickable(
                "TASK-7",
                RiskLevel::Medium,
                cand("TASK-7", "task", &[], false, false),
            ),
        ];
        let (groom, park) = select_pickable(&items, RiskLevel::Low);
        assert_eq!(groom, vec!["TASK-6".to_string()]);
        assert_eq!(park.len(), 1);
        assert_eq!(park[0].0, "TASK-7");
        assert_eq!(park[0].1, ParkReason::RiskCeiling(RiskLevel::Medium));
    }

    #[test]
    fn select_pickable_unknown_risk_admitted_only_at_high_ceiling() {
        let items = vec![pickable(
            "TASK-8",
            RiskLevel::Unknown,
            cand("TASK-8", "task", &[], false, false),
        )];
        // Unknown ranks above Medium — excluded at medium ceiling.
        let (groom, _) = select_pickable(&items, RiskLevel::Medium);
        assert!(groom.is_empty());
        // Admitted at high.
        let (groom_hi, _) = select_pickable(&items, RiskLevel::High);
        assert_eq!(groom_hi, vec!["TASK-8".to_string()]);
    }

    #[test]
    fn select_pickable_gate_takes_precedence_over_risk() {
        // High-risk AND gate-parked (epic): the gate reason wins, not risk.
        let items = vec![pickable(
            "EPIC-2",
            RiskLevel::High,
            cand("EPIC-2", "epic", &[], false, false),
        )];
        let (_, park) = select_pickable(&items, RiskLevel::High);
        assert!(matches!(park[0].1, ParkReason::Gate(_)));
    }

    #[test]
    fn select_pickable_preserves_input_order() {
        let items = vec![
            pickable(
                "TASK-A",
                RiskLevel::Low,
                cand("TASK-A", "task", &[], false, false),
            ),
            pickable(
                "TASK-B",
                RiskLevel::Low,
                cand("TASK-B", "task", &[], false, false),
            ),
        ];
        let (groom, _) = select_pickable(&items, RiskLevel::Medium);
        assert_eq!(groom, vec!["TASK-A".to_string(), "TASK-B".to_string()]);
    }
}
