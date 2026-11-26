//! `aida report backlog`: classify open specs and rank areas by hotness.
//! Read-only; it prescribes nothing and changes nothing.
// trace:STORY-3143 | ai:claude

use crate::forge::Forge as _;
use aida_core::models::{Requirement, RequirementPriority, RequirementStatus, RequirementType};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct BacklogConfig {
    #[serde(default)]
    pub areas: BTreeMap<String, AreaRule>,
    #[serde(default)]
    pub weights: Weights,
}

fn default_areas() -> BTreeMap<String, AreaRule> {
    let area = |k: &[&str], p: &[&str]| AreaRule {
        keywords: k.iter().map(|s| s.to_string()).collect(),
        paths: p.iter().map(|s| s.to_string()).collect(),
    };
    BTreeMap::from([
        (
            "review-gate".into(),
            area(
                &[
                    "review", "gate", "verdict", "merge", "train", "landing", "hold",
                ],
                &[],
            ),
        ),
        (
            "lanes".into(),
            area(
                &[
                    "lane",
                    "seat",
                    "session",
                    "lease",
                    "worktree",
                    "contained",
                    "sandbox",
                ],
                &[],
            ),
        ),
        ("cli".into(), area(&["cli", "command", "flag", "help"], &[])),
        (
            "storage".into(),
            area(
                &["store", "sync", "cache", "db", "migration", "schema"],
                &[],
            ),
        ),
        ("mcp".into(), area(&["mcp"], &[])),
        (
            "docs".into(),
            area(&["doc", "docs", "readme", "guide"], &["docs/"]),
        ),
        (
            "ci".into(),
            area(
                &["ci", "build", "cargo", "test", "tests"],
                &[".github/", ".gitlab-ci.yml"],
            ),
        ),
    ])
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
            None => BacklogConfig::default(),
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

    fn area_of_path(&self, path: &str) -> Option<&str> {
        self.areas
            .iter()
            .find(|(_, r)| r.paths.iter().any(|p| path.starts_with(p.as_str())))
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

fn gather_signals(
    root: &Path,
    cfg: &BacklogConfig,
    area_by_id: &HashMap<String, String>,
) -> Signals {
    let mut s = Signals::default();
    let area_for = |id: &str| area_by_id.get(&id.to_ascii_uppercase()).cloned();
    let week_ago = chrono::Utc::now() - chrono::Duration::days(7);
    for event in crate::gate::ledger::project::all_verdict_events(root) {
        let recent =
            chrono::DateTime::parse_from_rfc3339(event.at.trim()).is_ok_and(|t| t >= week_ago);
        let word = event
            .verdict_record()
            .and_then(|r| r.get("verdict"))
            .and_then(|v| v.as_str());
        let cr = word
            .is_some_and(|w| crate::gate::review_verdict::VerdictKind::parse(w).requires_changes());
        if let (true, true, Some(area)) = (recent, cr, event.spec.as_deref().and_then(area_for)) {
            *s.change_requests_7d.entry(area).or_default() += 1;
        }
    }
    match crate::forge::forge_for(root).landing_open_changes() {
        Ok(changes) => {
            for c in changes {
                let text = format!("{} {}", c.title.unwrap_or_default(), c.branch);
                if let Some(area) = spec_ids_in(&text).iter().find_map(|id| area_for(id)) {
                    *s.open_prs.entry(area).or_default() += 1;
                }
            }
        }
        Err(e) => s.unavailable.push(format!("open PRs: {e}")),
    }
    let log = std::process::Command::new("git")
        .current_dir(root)
        .args(["log", "--since=14.days.ago", "--name-only", "--format="])
        .output();
    match log {
        Ok(out) if out.status.success() => {
            for path in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(area) = cfg.area_of_path(path.trim()) {
                    *s.churn_14d.entry(area.to_string()).or_default() += 1;
                }
            }
        }
        _ => s.unavailable.push("14-day churn: git log failed".into()),
    }
    s
}

pub(crate) fn handle_backlog(storage: &aida_core::Storage, json: bool) -> Result<()> {
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
    let signals = gather_signals(&root, &cfg, &area_by_id);
    let areas = hotness(&rows, &open, &signals, &cfg.weights);
    let noise = rows.iter().filter(|r| r.noise).count();
    if json {
        let body = serde_json::json!({
            "open": rows.len(), "noise": noise, "areas": areas, "specs": rows,
            "unavailable": signals.unavailable,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
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
    Ok(())
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
            unavailable: vec![],
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
}
