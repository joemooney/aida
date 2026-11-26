//! `aida report backlog`: classify open specs and rank areas by hotness.
//! Read-only; it prescribes nothing and changes nothing.
// trace:STORY-3143 | ai:claude

use crate::forge::Forge as _;
use aida_core::models::{Requirement, RequirementPriority, RequirementStatus, RequirementType};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// One area: a spec joins it by keyword (title/tags) and a changed file by path prefix.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AreaRule {
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Hotness weights (`[report.backlog.weights]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Weights {
    pub open: f64,
    pub high: f64,
    pub change_requests_7d: f64,
    pub open_prs: f64,
    pub churn_14d: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            open: 1.0,
            high: 2.0,
            change_requests_7d: 3.0,
            open_prs: 2.0,
            churn_14d: 0.1,
        }
    }
}

/// `[report.backlog]` in `.aida/config.toml`; defaults when absent.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BacklogConfig {
    #[serde(default)]
    pub areas: BTreeMap<String, AreaRule>,
    #[serde(default)]
    pub weights: Weights,
    /// An InProgress spec untouched this many days, with no lease or open PR, is a stale claim.
    #[serde(default = "default_stale_days")]
    pub stale_days: i64,
    /// Days of landings the flow section covers.
    #[serde(default = "default_flow_days")]
    pub flow_days: i64,
    /// How far back to look up when a landed PR was opened; a PR opened
    /// earlier has no open->landed time and counts as unmatched.
    #[serde(default = "default_opened_lookback_days")]
    pub opened_lookback_days: i64,
}

fn default_stale_days() -> i64 {
    3
}

fn default_flow_days() -> i64 {
    14
}

fn default_opened_lookback_days() -> i64 {
    60
}

/// Defaults: `(area, keywords, path prefixes)`; `@` stands for `aida-cli-lib/src/`.
const DEFAULT_AREAS: &[(&str, &str, &str)] = &[
    (
        "review-gate",
        "review gate verdict merge train landing hold",
        "@gate/ @review @land @train @integrat",
    ),
    (
        "lanes",
        "lane seat session lease worktree contained sandbox",
        "@lane @seat @session @lease @worktree @contain @sandbox",
    ),
    ("cli", "cli command flag help", "aida-cli/ aida-cmdkit/ @cli @command_registry"),
    (
        "storage",
        "store sync cache db migration schema",
        "aida-core/src/storage aida-core/src/db/ aida-core/src/store aida-core/src/object_store @store",
    ),
    ("mcp", "mcp", "aida-mcp/ @mcp"),
    ("docs", "doc docs readme guide", "docs/"),
    ("ci", "ci build cargo test tests", ".github/ .gitlab-ci.yml"),
];

fn default_areas() -> BTreeMap<String, AreaRule> {
    let words = |s: &str| {
        s.split_whitespace()
            .map(|w| w.replace('@', "aida-cli-lib/src/"))
            .collect()
    };
    DEFAULT_AREAS
        .iter()
        .map(|(name, keywords, paths)| {
            let rule = AreaRule {
                keywords: words(keywords),
                paths: words(paths),
            };
            (name.to_string(), rule)
        })
        .collect()
}

impl BacklogConfig {
    pub(crate) fn from_project_config(value: Option<&toml::Value>) -> Result<Self> {
        let section = value
            .and_then(|v| v.get("report"))
            .and_then(|v| v.get("backlog"));
        let mut cfg: BacklogConfig = match section {
            Some(s) => s
                .clone()
                .try_into()
                .map_err(|e| anyhow::anyhow!("[report.backlog]: {e}"))?,
            None => toml::Value::Table(Default::default())
                .try_into()
                .expect("an empty [report.backlog] deserializes"),
        };
        if cfg.areas.is_empty() {
            cfg.areas = default_areas();
        }
        Ok(cfg)
    }

    /// The area with the most keyword hits in `text`; ties go to the first
    /// name in sort order; no hit is `other`.
    pub(crate) fn area_of(&self, text: &str) -> String {
        let words: Vec<String> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        let mut best = ("other".to_string(), 0usize);
        for (name, rule) in &self.areas {
            let hits = rule
                .keywords
                .iter()
                .filter(|k| words.iter().any(|w| *w == k.to_lowercase()))
                .count();
            if hits > best.1 {
                best = (name.clone(), hits);
            }
        }
        best.0
    }

    /// The area whose path prefix is the longest match for `path`.
    fn area_of_path(&self, path: &str) -> Option<&str> {
        self.areas
            .iter()
            .flat_map(|(n, r)| r.paths.iter().map(move |p| (n, p)))
            .filter(|(_, p)| path.starts_with(p.as_str()))
            .max_by_key(|(_, p)| p.len())
            .map(|(n, _)| n.as_str())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct SpecRow {
    pub id: String,
    pub title: String,
    pub area: String,
    pub difficulty: &'static str,
    pub priority: String,
    pub status: String,
    pub noise: bool,
}

/// Review-finding specs: the tags the review fold leaves behind.
pub(crate) fn is_noise(req: &Requirement) -> bool {
    req.tags.iter().any(|t| is_noise_tag(t))
}

fn is_noise_tag(tag: &str) -> bool {
    let t = tag.to_lowercase();
    t.starts_with("from-review:") || t.starts_with("carried-from:") || t.starts_with("kind:")
}

/// S/M/L/XL from type, tags, description size and acceptance-criteria count.
pub(crate) fn difficulty(req: &Requirement) -> &'static str {
    let mut score = match req.req_type {
        RequirementType::Epic => 4,
        RequirementType::Story | RequirementType::Functional | RequirementType::System => 1,
        _ => 0,
    };
    let len = req.description.len();
    score += usize::from(len > 1500) + usize::from(len > 4000);
    let acs = req
        .description
        .lines()
        .map(str::trim_start)
        .filter(|l| {
            let l = l.trim_start_matches(['-', '*', ' ']);
            l.starts_with("[ ]")
                || (l.starts_with("AC") && l[2..].starts_with(|c: char| c.is_ascii_digit()))
        })
        .count();
    score += usize::from(acs > 3) + usize::from(acs > 6);
    let heavy = ["architecture", "keystone", "security", "supervised"];
    score += usize::from(
        req.tags
            .iter()
            .any(|t| heavy.iter().any(|h| t.to_lowercase().contains(h))),
    );
    ["S", "S", "M", "L"].get(score).copied().unwrap_or("XL")
}

fn spec_label(req: &Requirement) -> String {
    match req.friendly_id().as_str() {
        "?" => req.id.to_string(),
        id => id.to_string(),
    }
}

pub(crate) fn classify(req: &Requirement, cfg: &BacklogConfig) -> SpecRow {
    // Noise markers say where a spec came from, not what it is about: a
    // `from-review:` tag must not vote for the review area.
    let mut tags: Vec<&str> = req
        .tags
        .iter()
        .map(String::as_str)
        .filter(|t| !is_noise_tag(t))
        .collect();
    tags.sort_unstable();
    SpecRow {
        id: spec_label(req),
        title: req.title.clone(),
        area: cfg.area_of(&format!("{} {} {}", req.title, req.feature, tags.join(" "))),
        difficulty: difficulty(req),
        priority: req.priority.to_string(),
        status: req.status.to_string(),
        noise: is_noise(req),
    }
}

/// Per-area signals gathered outside the store (gate ledger, forge, git).
#[derive(Debug, Clone, Default)]
pub(crate) struct Signals {
    pub change_requests_7d: HashMap<String, u64>,
    pub open_prs: HashMap<String, u64>,
    pub churn_14d: HashMap<String, u64>,
    pub unavailable: Vec<String>,
    pub verdicts: Vec<Verdict>,
    pub landings: Vec<Landing>,
    /// PR number -> when it was opened.
    pub opened: HashMap<u64, DateTime<Utc>>,
    /// Upper-cased spec IDs held by a lease or named by an open PR; `None` when unknown.
    pub held: Option<HashSet<String>>,
    /// Upper-cased spec ID -> its driver's last lane-log write.
    pub activity: HashMap<String, DateTime<Utc>>,
}

/// One gate-ledger verdict, reduced to what the flow metrics need.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Verdict {
    /// `PR-<N>` / `MR-<N>`.
    pub change: String,
    pub spec: Option<String>,
    pub at: DateTime<Utc>,
    pub change_request: bool,
}

/// A change that landed on main or integration, found by `(#N)` in its subject.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Landing {
    pub pr: u64,
    pub at: DateTime<Utc>,
}

/// `%cI<TAB>%s` git-log lines -> landings. Train-landed PRs show CLOSED on
/// GitHub, so the `(#N)` subject suffix is the landing evidence; a PR seen on
/// both branches counts once, at its earliest commit.
pub(crate) fn parse_landings(log: &str) -> Vec<Landing> {
    let mut by_pr: BTreeMap<u64, DateTime<Utc>> = BTreeMap::new();
    for line in log.lines() {
        let Some((at, subject)) = line.split_once('\t') else {
            continue;
        };
        let pr = subject
            .trim_end()
            .strip_suffix(')')
            .and_then(|s| s.rsplit_once("(#"))
            .and_then(|(_, n)| n.parse::<u64>().ok());
        if let (Some(pr), Ok(at)) = (pr, DateTime::parse_from_rfc3339(at.trim())) {
            let at = at.with_timezone(&Utc);
            by_pr
                .entry(pr)
                .and_modify(|t| *t = (*t).min(at))
                .or_insert(at);
        }
    }
    by_pr
        .into_iter()
        .map(|(pr, at)| Landing { pr, at })
        .collect()
}

/// Change requests per area since `since`.
pub(crate) fn cr_by_area(
    verdicts: &[Verdict],
    area_for: impl Fn(&str) -> Option<String>,
    since: DateTime<Utc>,
) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for v in verdicts
        .iter()
        .filter(|v| v.change_request && v.at >= since)
    {
        if let Some(area) = v.spec.as_deref().and_then(&area_for) {
            *out.entry(area).or_default() += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Sink {
    pub spec: String,
    pub change_requests_7d: u64,
}

/// Change requests a landed spec took across all of its PRs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct SpecRounds {
    pub spec: String,
    pub changes: usize,
    pub change_requests: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub(crate) struct Flow {
    pub window_days: i64,
    pub landed: usize,
    pub landings_per_day: f64,
    pub by_day: BTreeMap<String, u64>,
    pub open_to_landed_median_hours: Option<f64>,
    pub open_to_landed_p90_hours: Option<f64>,
    /// Landings with no opened time inside the lookback window.
    pub open_to_landed_unmatched: usize,
    pub cr_rounds_before_landing_mean: Option<f64>,
    pub cr_rounds_before_landing_max: u64,
    /// Per landed spec, most change requests first (top 10).
    pub cr_rounds_by_spec: Vec<SpecRounds>,
    pub rework_sinks: Vec<Sink>,
    /// `None` when leases or open PRs could not be read.
    pub stale_claims: Option<Vec<String>>,
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.max(1) - 1).copied()
}

/// AC3 landing flow over the last `days`; stale claims are filled separately.
pub(crate) fn flow(s: &Signals, now: DateTime<Utc>, days: i64) -> Flow {
    let since = now - chrono::Duration::days(days);
    let landed: Vec<&Landing> = s.landings.iter().filter(|l| l.at >= since).collect();
    let mut by_day = BTreeMap::new();
    for l in &landed {
        *by_day
            .entry(l.at.format("%Y-%m-%d").to_string())
            .or_default() += 1;
    }
    let mut hours: Vec<f64> = landed
        .iter()
        .filter_map(|l| {
            s.opened
                .get(&l.pr)
                .map(|o| (l.at - *o).num_minutes() as f64 / 60.0)
        })
        .filter(|h| *h >= 0.0)
        .collect();
    let by_spec = rounds_by_spec(&s.verdicts, &landed);
    let unmatched = landed.len() - hours.len();
    hours.sort_by(f64::total_cmp);
    let rounds: Vec<u64> = landed
        .iter()
        .map(|l| {
            let keys = [format!("PR-{}", l.pr), format!("MR-{}", l.pr)];
            s.verdicts
                .iter()
                .filter(|v| v.change_request && v.at <= l.at && keys.contains(&v.change))
                .count() as u64
        })
        .collect();
    let mut sinks: BTreeMap<String, u64> = BTreeMap::new();
    let week_ago = now - chrono::Duration::days(7);
    for v in s
        .verdicts
        .iter()
        .filter(|v| v.change_request && v.at >= week_ago)
    {
        if let Some(spec) = &v.spec {
            *sinks.entry(spec.to_ascii_uppercase()).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, u64)> = sinks.into_iter().filter(|(_, n)| *n >= 2).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let rework_sinks: Vec<Sink> = ranked
        .into_iter()
        .take(10)
        .map(|(spec, change_requests_7d)| Sink {
            spec,
            change_requests_7d,
        })
        .collect();
    Flow {
        window_days: days,
        landed: landed.len(),
        landings_per_day: landed.len() as f64 / days.max(1) as f64,
        by_day,
        open_to_landed_median_hours: percentile(&hours, 0.5),
        open_to_landed_p90_hours: percentile(&hours, 0.9),
        open_to_landed_unmatched: unmatched,
        cr_rounds_before_landing_mean: (!rounds.is_empty())
            .then(|| rounds.iter().sum::<u64>() as f64 / rounds.len() as f64),
        cr_rounds_before_landing_max: rounds.into_iter().max().unwrap_or(0),
        cr_rounds_by_spec: by_spec,
        rework_sinks,
        stale_claims: None,
    }
}

/// Per spec with a PR landed in the window: every change request it took on
/// any of its PRs up to its last landing. A PR's spec comes from its verdicts.
fn rounds_by_spec(verdicts: &[Verdict], landed: &[&Landing]) -> Vec<SpecRounds> {
    let mut last: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    for l in landed {
        let keys = [format!("PR-{}", l.pr), format!("MR-{}", l.pr)];
        let specs: HashSet<String> = verdicts
            .iter()
            .filter(|v| keys.contains(&v.change))
            .filter_map(|v| v.spec.as_deref().map(str::to_ascii_uppercase))
            .collect();
        for spec in specs {
            let t = last.entry(spec).or_insert(l.at);
            *t = (*t).max(l.at);
        }
    }
    let mut out: Vec<SpecRounds> = last
        .into_iter()
        .map(|(spec, until)| {
            let mine: Vec<&Verdict> = verdicts
                .iter()
                .filter(|v| v.at <= until)
                .filter(|v| {
                    v.spec
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(&spec))
                })
                .collect();
            let changes: HashSet<&str> = mine.iter().map(|v| v.change.as_str()).collect();
            SpecRounds {
                changes: changes.len(),
                change_requests: mine.iter().filter(|v| v.change_request).count() as u64,
                spec,
            }
        })
        .collect();
    out.sort_by(|a, b| b.change_requests.cmp(&a.change_requests));
    out.into_iter().take(10).collect()
}

/// Every ID a requirement may be spelled by, upper-cased.
fn id_keys(r: &Requirement) -> Vec<String> {
    let uuid = r.id.to_string();
    [
        r.agreed_id.as_deref(),
        r.spec_id.as_deref(),
        Some(uuid.as_str()),
    ]
    .into_iter()
    .flatten()
    .map(str::to_ascii_uppercase)
    .collect()
}

/// InProgress specs no lease or open PR holds whose last driver activity
/// (status change or lane-log write, whichever is later) is `days` old.
pub(crate) fn stale_claims(
    open: &[&Requirement],
    held: &HashSet<String>,
    activity: &HashMap<String, DateTime<Utc>>,
    now: DateTime<Utc>,
    days: i64,
) -> Vec<String> {
    let cutoff = now - chrono::Duration::days(days);
    open.iter()
        .filter(|r| r.status == RequirementStatus::InProgress)
        .filter(|r| {
            let ids = id_keys(r);
            let last = ids
                .iter()
                .filter_map(|id| activity.get(id))
                .fold(r.modified_at, |a, b| a.max(*b));
            last <= cutoff && !ids.iter().any(|id| held.contains(id))
        })
        .map(|r| spec_label(r))
        .collect()
}

/// Lane logs are `<spec-or-branch>-<session>.jsonl` under
/// `.aida/headless-logs`; each spec ID a name spells gets the newest mtime.
pub(crate) fn driver_activity(dir: &Path) -> HashMap<String, DateTime<Utc>> {
    let mut out: HashMap<String, DateTime<Utc>> = HashMap::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        let Some(at) = entry.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        let at = DateTime::<Utc>::from(at);
        for id in spec_ids_in(stem) {
            let t = out.entry(id).or_insert(at);
            *t = (*t).max(at);
        }
    }
    out
}

/// AC4: prescribed commands only; the report never runs them.
pub(crate) fn recommendations(open: &[&Requirement], flow: &Flow) -> Vec<String> {
    let mut out = Vec::new();
    for r in open.iter().filter(|r| is_noise(r)) {
        let id = spec_label(r);
        let parent = r.tags.iter().find_map(|t| {
            t.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("carried-from"))
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !spec_ids_in(v).is_empty())
        });
        out.push(match parent {
            Some(p) => format!("aida edit {id} --superseded-by {p}   # fold review finding"),
            None => format!(
                "aida defer {id} --reason \"review-finding noise; fold into its parent\" --until manual:advisor"
            ),
        });
    }
    for id in flow.stale_claims.iter().flatten() {
        out.push(format!(
            "aida edit {id} --status approved   # release stale claim"
        ));
    }
    // Only an open spec can be parked; a sink that already landed is history.
    for sink in &flow.rework_sinks {
        let named = |r: &&&Requirement| {
            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .any(|id| id.eq_ignore_ascii_case(&sink.spec))
        };
        if let Some(r) = open.iter().find(named) {
            out.push(format!(
                "aida defer {} --reason \"rework sink: {} change requests in 7 days\" --until manual:advisor",
                spec_label(r),
                sink.change_requests_7d
            ));
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct AreaStat {
    pub area: String,
    pub open: u64,
    pub high: u64,
    pub change_requests_7d: u64,
    pub open_prs: u64,
    pub churn_14d: u64,
    pub hotness: f64,
}

pub(crate) fn hotness(
    rows: &[SpecRow],
    open_reqs: &[&Requirement],
    s: &Signals,
    w: &Weights,
) -> Vec<AreaStat> {
    let mut stats: BTreeMap<String, AreaStat> = BTreeMap::new();
    let names = rows.iter().map(|r| r.area.clone()).chain(
        [&s.change_requests_7d, &s.open_prs, &s.churn_14d]
            .into_iter()
            .flat_map(|m| m.keys().cloned()),
    );
    for area in names {
        stats.entry(area.clone()).or_insert(AreaStat {
            area: area.clone(),
            open: 0,
            high: 0,
            change_requests_7d: s.change_requests_7d.get(&area).copied().unwrap_or(0),
            open_prs: s.open_prs.get(&area).copied().unwrap_or(0),
            churn_14d: s.churn_14d.get(&area).copied().unwrap_or(0),
            hotness: 0.0,
        });
    }
    for (row, req) in rows.iter().zip(open_reqs) {
        let st = stats.get_mut(&row.area).expect("area seeded above");
        st.open += 1;
        st.high += u64::from(req.priority == RequirementPriority::High);
    }
    let mut out: Vec<AreaStat> = stats
        .into_values()
        .map(|mut st| {
            st.hotness = w.open * st.open as f64
                + w.high * st.high as f64
                + w.change_requests_7d * st.change_requests_7d as f64
                + w.open_prs * st.open_prs as f64
                + w.churn_14d * st.churn_14d as f64;
            st
        })
        .collect();
    out.sort_by(|a, b| b.hotness.total_cmp(&a.hotness).then(a.area.cmp(&b.area)));
    out
}

/// Candidate spec IDs a PR title or branch names, upper-cased, each token
/// followed by its shorter `-` truncations (`STORY-12-FIX` → `STORY-12`), so a
/// branch suffix still finds the spec.
pub(crate) fn spec_ids_in(text: &str) -> Vec<String> {
    let looks_like_id = |w: &str| {
        w.split_once('-').is_some_and(|(p, n)| {
            p.len() >= 2
                && p.chars().all(|c| c.is_ascii_uppercase())
                && n.chars().any(|c| c.is_ascii_digit())
        })
    };
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let mut w = word.trim_matches('-').to_ascii_uppercase();
        while looks_like_id(&w) {
            out.push(w.clone());
            match w.rfind('-') {
                Some(i) => w = w[..i].to_string(),
                None => break,
            }
        }
    }
    out
}

/// What [`read_signals`] read from the ledger, forge, leases, git and lane
/// logs; an `Err` says why a source was unavailable.
pub(crate) struct RawSignals {
    pub events: Vec<crate::gate::ledger::event::GateEvent>,
    pub open_changes: Result<Vec<crate::forge::ChangeRef>, String>,
    pub lease_scopes: Result<Vec<String>, String>,
    /// `git log --name-only` over the churn window.
    pub churn_log: Result<String, String>,
    /// `%cI<TAB>%s` lines from main + integration, and the branches missing.
    pub landed_log: (String, Vec<String>),
    pub opened: Result<Vec<(u64, DateTime<Utc>)>, String>,
    pub activity: HashMap<String, DateTime<Utc>>,
}

fn read_signals(root: &Path, cfg: &BacklogConfig, now: DateTime<Utc>) -> RawSignals {
    let forge = crate::forge::forge_for(root);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let since = format!("--since={}.days.ago", cfg.flow_days);
    let mut landed_log = (String::new(), Vec::new());
    for branch in ["origin/main", "origin/integration"] {
        match git(&["log", branch, &since, "--format=%cI%x09%s"]) {
            Some(log) => landed_log.0.push_str(&log),
            None => landed_log.1.push(branch.to_string()),
        }
    }
    let lookback = cfg.opened_lookback_days.max(cfg.flow_days);
    RawSignals {
        events: crate::gate::ledger::project::all_verdict_events(root),
        open_changes: forge.landing_open_changes().map_err(|e| e.to_string()),
        lease_scopes: crate::leases::list_leases_strict_all(root)
            .map(|ls| ls.into_iter().map(|l| l.scope).collect()),
        churn_log: git(&["log", "--since=14.days.ago", "--name-only", "--format="])
            .ok_or_else(|| "git log failed".to_string()),
        landed_log,
        opened: forge
            .landing_opened_since(now - chrono::Duration::days(lookback))
            .map_err(|e| e.to_string()),
        activity: driver_activity(&root.join(".aida").join("headless-logs")),
    }
}

/// Signals from raw reads: the 7-day CR count per area (ledger spec -> area),
/// open PRs per area (spec IDs in title/branch), churn per area path, landings,
/// and the holder set, which is `None` unless both PRs and leases were read.
pub(crate) fn assemble_signals(
    raw: RawSignals,
    cfg: &BacklogConfig,
    area_by_id: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> Signals {
    let mut s = Signals {
        activity: raw.activity,
        ..Default::default()
    };
    let area_for = |id: &str| area_by_id.get(&id.to_ascii_uppercase()).cloned();
    for event in &raw.events {
        let word = event
            .verdict_record()
            .and_then(|r| r.get("verdict"))
            .and_then(|v| v.as_str());
        if let Ok(at) = DateTime::parse_from_rfc3339(event.at.trim()) {
            s.verdicts.push(Verdict {
                change: event.change.to_ascii_uppercase(),
                spec: event.spec.clone(),
                at: at.with_timezone(&Utc),
                change_request: word.is_some_and(|w| {
                    crate::gate::review_verdict::VerdictKind::parse(w).requires_changes()
                }),
            });
        }
    }
    s.change_requests_7d = cr_by_area(&s.verdicts, area_for, now - chrono::Duration::days(7));
    let mut held = HashSet::new();
    let prs_known = match raw.open_changes {
        Ok(changes) => {
            for c in changes {
                let ids = spec_ids_in(&format!("{} {}", c.title.unwrap_or_default(), c.branch));
                // A multi-spec PR counts once toward every area it names.
                let areas: HashSet<String> = ids.iter().filter_map(|id| area_for(id)).collect();
                for area in areas {
                    *s.open_prs.entry(area).or_default() += 1;
                }
                held.extend(ids);
            }
            true
        }
        Err(e) => {
            s.unavailable.push(format!("open PRs: {e}"));
            false
        }
    };
    let leases_known = match raw.lease_scopes {
        Ok(scopes) => {
            held.extend(scopes.iter().map(|l| l.trim().to_ascii_uppercase()));
            true
        }
        Err(e) => {
            s.unavailable.push(format!("leases: {e}"));
            false
        }
    };
    // Unknown holders must not read as "unheld": no stale-claim verdicts then.
    s.held = (prs_known && leases_known).then_some(held);
    match raw.churn_log {
        Ok(log) => {
            for path in log.lines() {
                if let Some(area) = cfg.area_of_path(path.trim()) {
                    *s.churn_14d.entry(area.to_string()).or_default() += 1;
                }
            }
        }
        Err(e) => s.unavailable.push(format!("14-day churn: {e}")),
    }
    let (log, missing) = raw.landed_log;
    s.unavailable
        .extend(missing.iter().map(|b| format!("landings: no {b}")));
    s.landings = parse_landings(&log);
    match raw.opened {
        Ok(rows) => s.opened.extend(rows),
        Err(e) => s.unavailable.push(format!("open->landed: {e}")),
    }
    s
}

/// Everything one run reports; the JSON and HTML outputs render this.
#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct Report {
    pub open: usize,
    pub noise: usize,
    pub stale_days: i64,
    pub areas: Vec<AreaStat>,
    pub specs: Vec<SpecRow>,
    pub flow: Flow,
    pub recommendations: Vec<String>,
    pub unavailable: Vec<String>,
}

pub(crate) fn handle_backlog(
    storage: &aida_core::Storage,
    json: bool,
    report_format: &str,
) -> Result<()> {
    let format = match (json, report_format) {
        (true, "html") => anyhow::bail!("--json conflicts with --report-format html"),
        (true, _) => "json",
        (false, f) => f,
    };
    let root = crate::find_project_root()?;
    let cfg = BacklogConfig::from_project_config(
        crate::kernel::read_project_config_value(&root).as_ref(),
    )?;
    let store = storage.load_for_read()?;
    let open_statuses = RequirementStatus::open_statuses();
    let open: Vec<&Requirement> = store
        .requirements
        .iter()
        .filter(|r| !r.archived && open_statuses.contains(&r.status))
        .collect();
    let rows: Vec<SpecRow> = open.iter().map(|r| classify(r, &cfg)).collect();
    // PR titles and ledger events may spell either the agreed or the origin ID.
    let area_by_id: HashMap<String, String> = store
        .requirements
        .iter()
        .flat_map(|r| {
            let area = classify(r, &cfg).area;
            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                .into_iter()
                .flatten()
                .map(move |id| (id.to_ascii_uppercase(), area.clone()))
        })
        .collect();
    let now = Utc::now();
    let signals = assemble_signals(read_signals(&root, &cfg, now), &cfg, &area_by_id, now);
    let areas = hotness(&rows, &open, &signals, &cfg.weights);
    let noise = rows.iter().filter(|r| r.noise).count();
    let mut flow = flow(&signals, now, cfg.flow_days);
    flow.stale_claims = signals
        .held
        .as_ref()
        .map(|h| stale_claims(&open, h, &signals.activity, now, cfg.stale_days));
    let recs = recommendations(&open, &flow);
    if format != "table" {
        let report = Report {
            open: rows.len(),
            noise,
            stale_days: cfg.stale_days,
            areas,
            specs: rows,
            flow,
            recommendations: recs,
            unavailable: signals.unavailable,
        };
        match format {
            "html" => print!("{}", render_html(&report)),
            _ => println!("{}", serde_json::to_string_pretty(&report)?),
        }
        return Ok(());
    }
    println!(
        "Open specs: {} ({} review-finding noise)\n",
        rows.len(),
        noise
    );
    println!(
        "{:<14} {:>5} {:>5} {:>6} {:>5} {:>6} {:>8}",
        "AREA", "OPEN", "HIGH", "CR-7d", "PRS", "CHURN", "HOTNESS"
    );
    for a in &areas {
        println!(
            "{:<14} {:>5} {:>5} {:>6} {:>5} {:>6} {:>8.1}",
            a.area, a.open, a.high, a.change_requests_7d, a.open_prs, a.churn_14d, a.hotness
        );
    }
    for note in &signals.unavailable {
        println!("  (unavailable: {note})");
    }
    println!(
        "\n{:<16} {:<12} {:<4} {:<7} {:<16} {:<5} TITLE",
        "ID", "AREA", "SIZE", "PRIO", "STATUS", "NOISE"
    );
    let mut sorted: Vec<&SpecRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.area.cmp(&b.area).then(a.id.cmp(&b.id)));
    for r in sorted {
        let title: String = r.title.chars().take(60).collect();
        let noise = if r.noise { "yes" } else { "" };
        println!(
            "{:<16} {:<12} {:<4} {:<7} {:<16} {:<5} {title}",
            r.id, r.area, r.difficulty, r.priority, r.status, noise
        );
    }
    print_flow(&flow, cfg.stale_days);
    println!("\nRecommendations (prescribed only; nothing was run):");
    if recs.is_empty() {
        println!("  none");
    }
    for cmd in &recs {
        println!("  {cmd}");
    }
    Ok(())
}

fn print_flow(f: &Flow, stale_days: i64) {
    let hours = |h: Option<f64>| h.map_or("n/a".to_string(), |h| format!("{h:.1}h"));
    println!("\nFlow (last {} days)", f.window_days);
    println!("  landed: {} ({:.1}/day)", f.landed, f.landings_per_day);
    let days: Vec<String> = f
        .by_day
        .iter()
        .map(|(d, n)| format!("{}:{n}", &d[5..]))
        .collect();
    println!("  per day: {}", days.join(" "));
    println!(
        "  open->landed: median {}, p90 {} ({} with no opened time)",
        hours(f.open_to_landed_median_hours),
        hours(f.open_to_landed_p90_hours),
        f.open_to_landed_unmatched
    );
    let mean = f
        .cr_rounds_before_landing_mean
        .map_or("n/a".into(), |m| format!("{m:.2}"));
    println!(
        "  change-request rounds before landing: mean {mean}, max {}",
        f.cr_rounds_before_landing_max
    );
    let by_spec: Vec<String> = f
        .cr_rounds_by_spec
        .iter()
        .map(|r| format!("{} ({} over {} PRs)", r.spec, r.change_requests, r.changes))
        .collect();
    println!("  change requests by landed spec: {}", by_spec.join(", "));
    let sinks: Vec<String> = f
        .rework_sinks
        .iter()
        .map(|s| format!("{} ({})", s.spec, s.change_requests_7d))
        .collect();
    println!(
        "  rework sinks (7d): {}",
        if sinks.is_empty() {
            "none".into()
        } else {
            sinks.join(", ")
        }
    );
    match &f.stale_claims {
        Some(ids) if ids.is_empty() => println!("  stale claims (>= {stale_days}d): none"),
        Some(ids) => println!("  stale claims (>= {stale_days}d): {}", ids.join(", ")),
        None => println!("  stale claims: unknown (leases or open PRs unavailable)"),
    }
}

/// HTML-escape one cell.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn html_table(out: &mut String, title: &str, head: &[&str], rows: Vec<Vec<String>>) {
    let cells = |tag: &str, row: Vec<String>| {
        row.iter()
            .map(|c| format!("<{tag}>{}</{tag}>", esc(c)))
            .collect::<String>()
    };
    out.push_str(&format!("<h2>{}</h2>\n<table>", esc(title)));
    out.push_str(&format!(
        "<tr>{}</tr>",
        cells("th", head.iter().map(|h| h.to_string()).collect())
    ));
    if rows.is_empty() {
        out.push_str(&format!(
            "<tr><td colspan=\"{}\">none</td></tr>",
            head.len()
        ));
    }
    for row in rows {
        out.push_str(&format!("<tr>{}</tr>", cells("td", row)));
    }
    out.push_str("</table>\n");
}

/// AC1 `--report-format html`: one self-contained page (inline style, no
/// scripts, links or external assets); every cell is escaped.
pub(crate) fn render_html(r: &Report) -> String {
    let opt = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{v:.1}"));
    let f = &r.flow;
    let mut out = String::from(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>AIDA backlog report</title>\n\
         <style>body{font-family:sans-serif;margin:2em}table{border-collapse:collapse;margin-bottom:1em}\
         td,th{border:1px solid #999;padding:2px 6px;text-align:left}th{background:#eee}</style>\n\
         </head><body>\n<h1>Backlog report</h1>\n",
    );
    out.push_str(&format!(
        "<p>Open specs: {} ({} review-finding noise)</p>\n",
        r.open, r.noise
    ));
    let areas = r.areas.iter().map(|a| {
        let n = [
            a.open,
            a.high,
            a.change_requests_7d,
            a.open_prs,
            a.churn_14d,
        ];
        let mut row = vec![a.area.clone()];
        row.extend(n.iter().map(u64::to_string));
        row.push(format!("{:.1}", a.hotness));
        row
    });
    let head = [
        "Area",
        "Open",
        "High",
        "CR 7d",
        "PRs",
        "Churn 14d",
        "Hotness",
    ];
    html_table(&mut out, "Areas", &head, areas.collect());
    let stale = match &f.stale_claims {
        Some(ids) => format!("{} (>= {}d)", ids.join(", "), r.stale_days),
        None => "unknown (leases or open PRs unavailable)".into(),
    };
    let metrics = vec![
        ("Window (days)", f.window_days.to_string()),
        (
            "Landed",
            format!("{} ({:.1}/day)", f.landed, f.landings_per_day),
        ),
        ("Open->landed median h", opt(f.open_to_landed_median_hours)),
        ("Open->landed p90 h", opt(f.open_to_landed_p90_hours)),
        (
            "Landings with no opened time",
            f.open_to_landed_unmatched.to_string(),
        ),
        (
            "CR rounds before landing (mean)",
            opt(f.cr_rounds_before_landing_mean),
        ),
        (
            "CR rounds before landing (max)",
            f.cr_rounds_before_landing_max.to_string(),
        ),
        ("Stale claims", stale),
    ];
    let metrics = metrics.into_iter().map(|(k, v)| vec![k.to_string(), v]);
    html_table(&mut out, "Flow", &["Metric", "Value"], metrics.collect());
    let by_day = f.by_day.iter().map(|(d, n)| vec![d.clone(), n.to_string()]);
    html_table(
        &mut out,
        "Landings per day",
        &["Day", "Landed"],
        by_day.collect(),
    );
    let rounds = f.cr_rounds_by_spec.iter().map(|x| {
        vec![
            x.spec.clone(),
            x.changes.to_string(),
            x.change_requests.to_string(),
        ]
    });
    let head = ["Spec", "PRs", "Change requests"];
    html_table(
        &mut out,
        "Change requests by landed spec",
        &head,
        rounds.collect(),
    );
    let sinks = f.rework_sinks.iter();
    let sinks = sinks.map(|x| vec![x.spec.clone(), x.change_requests_7d.to_string()]);
    let head = ["Spec", "Change requests 7d"];
    html_table(&mut out, "Rework sinks", &head, sinks.collect());
    let recs = r.recommendations.iter().map(|c| vec![c.clone()]);
    let head = ["Command (prescribed only; nothing was run)"];
    html_table(&mut out, "Recommendations", &head, recs.collect());
    let specs = r.specs.iter().map(|x| {
        let noise = if x.noise { "yes" } else { "" };
        let row = [
            &x.id,
            &x.area,
            x.difficulty,
            &x.priority,
            &x.status,
            noise,
            &x.title,
        ];
        row.iter().map(|c| c.to_string()).collect()
    });
    let head = ["ID", "Area", "Size", "Priority", "Status", "Noise", "Title"];
    html_table(&mut out, "Open specs", &head, specs.collect());
    let notes = r.unavailable.iter().map(|n| vec![n.clone()]);
    html_table(
        &mut out,
        "Unavailable signals",
        &["Source"],
        notes.collect(),
    );
    out.push_str("</body></html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, title: &str, desc: &str, tags: &[&str]) -> Requirement {
        let mut r = Requirement::new(title.into(), desc.into());
        r.spec_id = Some(id.into());
        r.tags = tags.iter().map(|t| t.to_string()).collect();
        r
    }

    // AC1: area from the configured keyword map, size, noise.
    #[test]
    fn story_3143_classifies_area_difficulty_and_noise() {
        let cfg = BacklogConfig::from_project_config(None).unwrap();
        let mut epic = spec("EPIC-1", "Review gate verdict rework", "", &[]);
        epic.req_type = RequirementType::Epic;
        let row = classify(&epic, &cfg);
        assert_eq!(
            (row.area.as_str(), row.difficulty, row.noise),
            ("review-gate", "XL", false)
        );
        let acs = (1..=7)
            .map(|n| format!("- AC{n}: x"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut story = spec("STORY-2", "Lane seat lease cleanup", &acs, &["security"]);
        story.req_type = RequirementType::Story;
        assert_eq!(classify(&story, &cfg).difficulty, "XL");
        let mut bug = spec("BUG-3", "Unrelated thing", "short", &["from-review:PR-9"]);
        bug.req_type = RequirementType::Bug;
        let row = classify(&bug, &cfg);
        assert_eq!(
            (row.area.as_str(), row.difficulty, row.noise),
            ("other", "S", true)
        );
        assert!(is_noise(&spec("T-4", "t", "", &["Carried-From:PR-1"])));
        assert!(is_noise(&spec("T-5", "t", "", &["kind:nit"])));
    }

    // AC1: a configured area map replaces the defaults; keywords match whole words.
    #[test]
    fn story_3143_area_map_and_weights_come_from_config() {
        let toml: toml::Value = toml::from_str(
            "[report.backlog.areas.billing]\nkeywords = [\"invoice\"]\npaths = [\"src/billing/\"]\n\
             [report.backlog.weights]\nopen = 5.0\n",
        )
        .unwrap();
        let cfg = BacklogConfig::from_project_config(Some(&toml)).unwrap();
        assert_eq!(cfg.areas.len(), 1);
        assert_eq!(cfg.area_of("Fix INVOICE totals"), "billing");
        assert_eq!(cfg.area_of("invoices"), "other");
        assert_eq!(cfg.area_of_path("src/billing/a.rs"), Some("billing"));
        assert_eq!((cfg.weights.open, cfg.weights.high), (5.0, 2.0));
        let bad: toml::Value = toml::from_str("[report.backlog]\nweights = 3\n").unwrap();
        assert!(BacklogConfig::from_project_config(Some(&bad)).is_err());
    }

    // AC2: hotness is the weighted blend of the five signals, hottest first.
    #[test]
    fn story_3143_hotness_blends_weighted_signals() {
        let cfg = BacklogConfig::from_project_config(None).unwrap();
        let mut hot = spec("BUG-1", "merge train hold", "", &[]);
        hot.priority = RequirementPriority::High;
        let cold = spec("TASK-2", "docs guide", "", &[]);
        let open = vec![&hot, &cold];
        let rows: Vec<SpecRow> = open.iter().map(|r| classify(r, &cfg)).collect();
        let s = Signals {
            change_requests_7d: HashMap::from([("review-gate".into(), 2)]),
            open_prs: HashMap::from([("review-gate".into(), 1), ("ci".into(), 1)]),
            churn_14d: HashMap::from([("docs".into(), 10)]),
            ..Default::default()
        };
        let areas = hotness(&rows, &open, &s, &Weights::default());
        let names: Vec<&str> = areas.iter().map(|a| a.area.as_str()).collect();
        assert_eq!(names, ["review-gate", "ci", "docs"]);
        // 1 open + 2*1 high + 3*2 CR + 2*1 PR = 11
        assert_eq!(
            (areas[0].open, areas[0].high, areas[0].hotness),
            (1, 1, 11.0)
        );
        assert_eq!(areas[2].hotness, 1.0 + 0.1 * 10.0);
    }

    #[test]
    fn story_3143_spec_ids_in_pr_titles_and_branches() {
        assert_eq!(
            spec_ids_in("[AI:claude] feat(x): y (STORY-3143) aida/bug-12-fix"),
            ["STORY-3143", "BUG-12-FIX", "BUG-12"]
        );
        assert!(spec_ids_in("report-backlog AI:claude").is_empty());
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn cr(change: &str, spec: &str, at: &str) -> Verdict {
        Verdict {
            change: change.into(),
            spec: Some(spec.into()),
            at: t(at),
            change_request: true,
        }
    }

    // S1 follow-up: code areas have default path prefixes; the longest prefix wins.
    #[test]
    fn story_3143_default_areas_map_code_paths() {
        let cfg = BacklogConfig::from_project_config(None).unwrap();
        let path = |p| cfg.area_of_path(p);
        assert_eq!(
            path("aida-cli-lib/src/gate/ledger/event.rs"),
            Some("review-gate")
        );
        assert_eq!(path("aida-cli-lib/src/lane_job.rs"), Some("lanes"));
        assert_eq!(path("aida-mcp/src/lib.rs"), Some("mcp"));
        assert_eq!(path("aida-core/src/storage.rs"), Some("storage"));
        assert_eq!(path("aida-cli/src/main.rs"), Some("cli"));
        assert_eq!(path("README.md"), None);
        assert_eq!(cfg.stale_days, 3);
        let toml: toml::Value = toml::from_str(
            "[report.backlog.areas.a]\npaths = [\"src/\"]\n[report.backlog.areas.b]\npaths = [\"src/b/\"]\n",
        )
        .unwrap();
        let cfg = BacklogConfig::from_project_config(Some(&toml)).unwrap();
        assert_eq!(cfg.area_of_path("src/b/x.rs"), Some("b"));
        assert_eq!(cfg.area_of_path("src/c.rs"), Some("a"));
    }

    // AC3: landings come from `(#N)` subjects on main + integration, once per PR.
    #[test]
    fn story_3143_landings_parse_pr_numbers_from_subjects() {
        let log = "2026-11-26T00:09:28-07:00\tfeat(x): a (Refs STORY-1) (#4626)\n\
                   2026-11-25T10:00:00Z\tfeat(x): a (Refs STORY-1) (#4626)\n\
                   2026-11-25T11:00:00Z\tMerge remote-tracking branch 'origin/main'\n\
                   garbage line\n\
                   2026-11-25T12:00:00Z\tfix(y): b (BUG-2) (#4615)\n";
        assert_eq!(
            parse_landings(log),
            [
                Landing {
                    pr: 4615,
                    at: t("2026-11-25T12:00:00Z")
                },
                Landing {
                    pr: 4626,
                    at: t("2026-11-25T10:00:00Z")
                },
            ]
        );
    }

    // AC3: per-day landings, open->landed percentiles, CR rounds, rework sinks;
    // AC2: the 7-day CR count per area comes from the same ledger fixture.
    #[test]
    fn story_3143_flow_metrics_from_fixture_ledger() {
        let now = t("2026-11-26T00:00:00Z");
        let s = Signals {
            landings: vec![
                Landing {
                    pr: 1,
                    at: t("2026-11-24T10:00:00Z"),
                },
                Landing {
                    pr: 2,
                    at: t("2026-11-25T10:00:00Z"),
                },
                Landing {
                    pr: 3,
                    at: t("2026-11-25T12:00:00Z"),
                },
                Landing {
                    pr: 4,
                    at: t("2026-10-01T00:00:00Z"),
                }, // outside the window
            ],
            opened: HashMap::from([
                (1, t("2026-11-24T08:00:00Z")),
                (2, t("2026-11-25T00:00:00Z")),
                (3, t("2026-11-25T08:00:00Z")),
            ]),
            verdicts: vec![
                cr("PR-2", "STORY-9", "2026-11-25T05:00:00Z"),
                cr("PR-2", "STORY-9", "2026-11-25T07:00:00Z"),
                cr("PR-2", "STORY-9", "2026-11-25T11:00:00Z"), // after landing
                cr("PR-5", "BUG-7", "2026-11-25T09:00:00Z"),
                cr("PR-6", "BUG-7", "2026-11-01T09:00:00Z"), // older than 7 days
                Verdict {
                    change_request: false,
                    ..cr("PR-1", "STORY-9", "2026-11-24T09:00:00Z")
                },
            ],
            ..Default::default()
        };
        let f = flow(&s, now, 14);
        assert_eq!(f.landed, 3);
        assert_eq!(f.landings_per_day, 3.0 / 14.0);
        let by_day: Vec<(&str, u64)> = f.by_day.iter().map(|(d, n)| (d.as_str(), *n)).collect();
        assert_eq!(by_day, [("2026-11-24", 1), ("2026-11-25", 2)]);
        // open->landed hours: 2, 4, 10
        assert_eq!(f.open_to_landed_median_hours, Some(4.0));
        assert_eq!(f.open_to_landed_p90_hours, Some(10.0));
        assert_eq!(f.cr_rounds_before_landing_mean, Some(2.0 / 3.0));
        assert_eq!(f.cr_rounds_before_landing_max, 2);
        assert_eq!(
            f.rework_sinks,
            [Sink {
                spec: "STORY-9".into(),
                change_requests_7d: 3
            }]
        );
        let area = |id: &str| (id == "STORY-9").then(|| "lanes".to_string());
        let by_area = cr_by_area(&s.verdicts, area, now - chrono::Duration::days(7));
        assert_eq!(by_area, HashMap::from([("lanes".to_string(), 3)]));
        assert_eq!(
            flow(&Signals::default(), now, 14).open_to_landed_median_hours,
            None
        );
    }

    // AC3 stale claims + AC4 prescribed commands.
    #[test]
    fn story_3143_stale_claims_and_recommendations() {
        let now = t("2026-11-26T00:00:00Z");
        let claim = |id: &str, age_days: i64| {
            let mut r = spec(id, "work", "", &[]);
            r.status = RequirementStatus::InProgress;
            r.modified_at = now - chrono::Duration::days(age_days);
            r
        };
        let stale = claim("TASK-1", 5);
        let leased = claim("TASK-2", 5);
        let fresh = claim("TASK-3", 1);
        let mut noise = spec("BUG-4", "nit", "", &["carried-from:STORY-8"]);
        noise.status = RequirementStatus::Approved;
        let orphan = spec("BUG-5", "nit", "", &["from-review:PR-3"]);
        let sink = spec("STORY-9", "busy", "", &[]);
        let held = HashSet::from(["TASK-2".to_string()]);
        let mut f = Flow {
            rework_sinks: vec![
                Sink {
                    spec: "story-9".into(),
                    change_requests_7d: 4,
                },
                Sink {
                    spec: "STORY-77".into(), // landed: not open, so not parked
                    change_requests_7d: 2,
                },
            ],
            ..Default::default()
        };
        // S3: a lane log written yesterday keeps an old status change fresh.
        let stale_by_status = claim("TASK-6", 5);
        let open = vec![
            &stale,
            &leased,
            &fresh,
            &noise,
            &orphan,
            &sink,
            &stale_by_status,
        ];
        let activity = HashMap::from([("TASK-6".to_string(), now - chrono::Duration::days(1))]);
        f.stale_claims = Some(stale_claims(&open, &held, &activity, now, 3));
        assert_eq!(f.stale_claims, Some(vec!["TASK-1".to_string()]));
        let recs = recommendations(&open, &f);
        assert_eq!(recs.len(), 4);
        assert!(recs[0].starts_with("aida edit BUG-4 --superseded-by STORY-8"));
        assert!(recs[1].starts_with("aida defer BUG-5 --reason"));
        assert!(recs[2].starts_with("aida edit TASK-1 --status approved"));
        assert!(recs[3].starts_with("aida defer STORY-9 --reason \"rework sink: 4 change"));
        // Unknown holders yield no release prescriptions.
        f.stale_claims = None;
        assert!(recommendations(&[&stale], &f)
            .iter()
            .all(|r| !r.contains("approved")));
    }

    // S3: lane-log names map to spec IDs, newest write wins.
    #[test]
    fn story_3143_driver_activity_from_lane_logs() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, at: &str| {
            let f = std::fs::File::create(dir.path().join(name)).unwrap();
            f.set_modified(t(at).into()).unwrap();
        };
        write(
            "task-7-01a1bc7c-6f19-7363-9342-a3aa00e7fc4b.jsonl",
            "2026-11-20T00:00:00Z",
        );
        write(
            "task-7-01a1bc8b-6ca2-7ca0-8325-827d47904608.jsonl",
            "2026-11-25T00:00:00Z",
        );
        write("bug-8-notes.txt", "2026-11-25T00:00:00Z");
        let a = driver_activity(dir.path());
        assert_eq!(a.get("TASK-7"), Some(&t("2026-11-25T00:00:00Z")));
        assert!(!a.contains_key("BUG-8"));
        assert!(driver_activity(&dir.path().join("missing")).is_empty());
    }

    fn event(
        change: &str,
        spec: &str,
        at: &str,
        verdict: &str,
    ) -> crate::gate::ledger::event::GateEvent {
        serde_json::from_value(serde_json::json!({
            "v": 1, "seq": 1, "at": at, "change": change, "spec": spec,
            "kind": "verdict", "record": {"verdict": verdict},
        }))
        .unwrap()
    }

    fn change(id: u64, title: &str, branch: &str) -> crate::forge::ChangeRef {
        crate::forge::ChangeRef {
            id,
            url: String::new(),
            branch: branch.into(),
            base: "main".into(),
            title: Some(title.into()),
        }
    }

    // S3 test gap: gather_signals' joins, from fixture ledger, PR, git and lease reads.
    #[test]
    fn story_3143_assemble_signals_from_fixture_reads() {
        let now = t("2026-11-26T00:00:00Z");
        let cfg = BacklogConfig::from_project_config(None).unwrap();
        let area_by_id = HashMap::from([
            ("STORY-9".to_string(), "lanes".to_string()),
            ("BUG-2".to_string(), "mcp".to_string()),
        ]);
        let raw = || RawSignals {
            events: vec![
                event("pr-1", "story-9", "2026-11-25T00:00:00Z", "request-changes"),
                event("PR-1", "STORY-9", "2026-11-24T00:00:00Z", "request-changes"),
                event("PR-1", "STORY-9", "2026-11-10T00:00:00Z", "request-changes"), // > 7d
                event("PR-3", "BUG-2", "2026-11-25T00:00:00Z", "approve"),
                event("PR-4", "BUG-2", "not a time", "request-changes"),
            ],
            open_changes: Ok(vec![
                change(5, "feat: x (STORY-9, BUG-2)", "aida/story-9-x"),
                change(6, "chore: untracked", "misc"),
            ]),
            lease_scopes: Ok(vec![" task-1 ".into()]),
            churn_log: Ok(
                "aida-mcp/src/lib.rs\n\naida-cli-lib/src/lane_job.rs\nREADME.md\n".into(),
            ),
            landed_log: (
                "2026-11-25T10:00:00Z\tfix: y (BUG-2) (#3)\n".into(),
                vec!["origin/integration".into()],
            ),
            opened: Ok(vec![(3, t("2026-11-25T00:00:00Z"))]),
            activity: HashMap::new(),
        };
        let s = assemble_signals(raw(), &cfg, &area_by_id, now);
        assert_eq!(s.verdicts.len(), 4);
        assert_eq!(s.verdicts[0].change, "PR-1");
        assert_eq!(
            s.change_requests_7d,
            HashMap::from([("lanes".to_string(), 2)])
        );
        assert_eq!(
            s.open_prs,
            HashMap::from([("lanes".to_string(), 1), ("mcp".to_string(), 1)])
        );
        assert_eq!(
            s.churn_14d,
            HashMap::from([("mcp".to_string(), 1), ("lanes".to_string(), 1)])
        );
        assert_eq!(
            s.landings,
            [Landing {
                pr: 3,
                at: t("2026-11-25T10:00:00Z")
            }]
        );
        assert_eq!(s.opened.get(&3), Some(&t("2026-11-25T00:00:00Z")));
        assert_eq!(s.unavailable, ["landings: no origin/integration"]);
        let held = s.held.unwrap();
        assert!(held.contains("TASK-1") && held.contains("STORY-9") && held.contains("BUG-2"));
        // A failed PR or lease read leaves holders unknown, never empty.
        for broken in [
            RawSignals {
                open_changes: Err("gh down".into()),
                ..raw()
            },
            RawSignals {
                lease_scopes: Err("torn lease".into()),
                ..raw()
            },
        ] {
            let s = assemble_signals(broken, &cfg, &area_by_id, now);
            assert!(s.held.is_none());
            assert_eq!(s.unavailable.len(), 2);
        }
    }

    // S3: change requests summed per landed spec across all its PRs.
    #[test]
    fn story_3143_cr_rounds_grouped_by_spec() {
        let verdicts = vec![
            cr("PR-1", "STORY-9", "2026-11-20T00:00:00Z"), // first PR, closed unlanded
            cr("PR-2", "story-9", "2026-11-24T00:00:00Z"),
            Verdict {
                change_request: false,
                ..cr("PR-2", "STORY-9", "2026-11-24T12:00:00Z")
            },
            cr("PR-2", "STORY-9", "2026-11-25T12:00:00Z"), // after landing
            cr("PR-7", "BUG-1", "2026-11-24T00:00:00Z"),   // never landed
            Verdict {
                change_request: false,
                ..cr("PR-3", "TASK-4", "2026-11-24T00:00:00Z")
            },
        ];
        let l2 = Landing {
            pr: 2,
            at: t("2026-11-25T00:00:00Z"),
        };
        let l3 = Landing {
            pr: 3,
            at: t("2026-11-25T00:00:00Z"),
        };
        let l8 = Landing {
            pr: 8,
            at: t("2026-11-25T00:00:00Z"),
        }; // no verdicts
        let rounds = rounds_by_spec(&verdicts, &[&l2, &l3, &l8]);
        assert_eq!(
            rounds,
            [
                SpecRounds {
                    spec: "STORY-9".into(),
                    changes: 2,
                    change_requests: 2
                },
                SpecRounds {
                    spec: "TASK-4".into(),
                    changes: 1,
                    change_requests: 0
                },
            ]
        );
    }

    // AC1 html: self-contained, every cell escaped.
    #[test]
    fn story_3143_html_is_self_contained_and_escaped() {
        let report = Report {
            open: 1,
            specs: vec![SpecRow {
                id: "BUG-1".into(),
                title: "<script>alert('x')</script> & \"q\"".into(),
                area: "<b>gate</b>".into(),
                difficulty: "S",
                priority: "high".into(),
                status: "approved".into(),
                noise: true,
            }],
            areas: vec![AreaStat {
                area: "lanes".into(),
                open: 3,
                high: 1,
                change_requests_7d: 2,
                open_prs: 1,
                churn_14d: 4,
                hotness: 12.5,
            }],
            flow: Flow {
                stale_claims: Some(vec!["TASK-1".into()]),
                cr_rounds_by_spec: vec![SpecRounds {
                    spec: "STORY-9".into(),
                    changes: 2,
                    change_requests: 3,
                }],
                ..Default::default()
            },
            recommendations: vec!["aida defer BUG-1 --reason \"x\"".into()],
            unavailable: vec!["open PRs: <gh>".into()],
            ..Default::default()
        };
        let html = render_html(&report);
        assert!(html.starts_with("<!DOCTYPE html>"));
        for external in [
            "<script", "<link", "src=", "href=", "http", "@import", "url(",
        ] {
            assert!(!html.contains(external), "found {external}");
        }
        assert!(
            html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;q&quot;")
        );
        assert!(html.contains("<td>&lt;b&gt;gate&lt;/b&gt;</td>"));
        assert!(html.contains("<td>open PRs: &lt;gh&gt;</td>"));
        assert!(html.contains("<td>aida defer BUG-1 --reason &quot;x&quot;</td>"));
        for section in [
            "Areas",
            "Flow",
            "Change requests by landed spec",
            "Rework sinks",
            "Recommendations",
        ] {
            assert!(html.contains(&format!("<h2>{section}</h2>")));
        }
        assert!(html.contains("<td>12.5</td>") && html.contains("<td>TASK-1 (&gt;= 0d)</td>"));
    }
}
