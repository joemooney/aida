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

    fn parse(s: &str) -> Result<Self> {
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
            batch,
            dry_run,
            note,
            user,
        } => {
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
        let entry = QueueEntry {
            user_id: user_id.clone(),
            requirement_id: req.id,
            position: i64::MAX,
            added_by: user_id.clone(),
            note: note.map(str::to_string),
            added_at: Utc::now(),
            for_role: std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage
            .queue_add(entry)
            .with_context(|| format!("queueing {}", display_id(req)))?;
        updated += 1;

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
                        storage.save(&store_now).with_context(|| {
                            format!("tagging {} batch:{}", display_id(req), name)
                        })?;
                    }
                }
            }
        }

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
            implementation_info: None,
            ai_evaluation: None,
            attention_reason: None,
            failure_reason: None,
            human_only: false,
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
}
