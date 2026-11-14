//! `aida now`: the one view of what is being worked on right now. It renders
//! the snapshot the landing reconciler writes on every pass and reads
//! nothing else: no git, no forge, no process table.
// trace:STORY-2768 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::cmdkit::{CmdPolicy, CommandReg, Handler};
use crate::land::{Action, Pass, State};
use crate::stakeholder_role::StakeholderAction;

/// A snapshot older than this means the reconciler is not running.
const STALE_SECS: i64 = 600;
/// An item with no live actor for longer than this needs attention.
const UNOWNED_SECS: i64 = 2 * 3600;
const DEFAULT_WIP_MAX: u64 = 6;
const NO_ACTOR: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Stage {
    Designing,
    Implementing,
    Reviewing,
    Reworking,
    WaitingCi,
    Ready,
    Shipping,
    Blocked,
}

impl Stage {
    const ALL: [Stage; 8] = [
        Stage::Designing,
        Stage::Implementing,
        Stage::Reviewing,
        Stage::Reworking,
        Stage::WaitingCi,
        Stage::Ready,
        Stage::Shipping,
        Stage::Blocked,
    ];

    fn label(self) -> &'static str {
        match self {
            Stage::Designing => "DESIGNING",
            Stage::Implementing => "IMPLEMENTING",
            Stage::Reviewing => "REVIEWING",
            Stage::Reworking => "REWORKING",
            Stage::WaitingCi => "WAITING-CI",
            Stage::Ready => "READY",
            Stage::Shipping => "SHIPPING",
            Stage::Blocked => "BLOCKED",
        }
    }
}

/// One in-flight piece of work, in exactly one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Item {
    pub spec: String,
    pub title: String,
    pub stage: Stage,
    /// Who is on it (`role@host`), or `none`.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// When it entered its stage, as closely as the facts say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<String>,
    pub next_action: String,
    /// Who carries the next action out.
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
}

/// Something that needs a person or a ruling. Never silent: it always says
/// why, what happens next and who does it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attention {
    pub spec: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    pub why: String,
    pub next_action: String,
    pub owner: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Queues {
    pub approved_not_started: usize,
    pub awaiting_review: usize,
    pub awaiting_rework: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MainBranch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_merge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_merge_at: Option<String>,
    pub merges_today: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub version: u32,
    pub generated_at: String,
    /// How long the pass that produced it took.
    pub pass_seconds: u64,
    pub open_prs: usize,
    pub wip_max: u64,
    pub main: MainBranch,
    pub items: Vec<Item>,
    pub queues: Queues,
    pub attention: Vec<Attention>,
    /// What the pass could not read, so what this snapshot may be missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn stage_of(state: &State, ci: Option<&str>) -> Stage {
    match state {
        // Approved, with the checks that gate its landing still running.
        State::Ready if ci == Some("pending") => Stage::WaitingCi,
        State::Landing => Stage::Shipping,
        State::Conflict | State::CiFailed | State::ChangesRequested => Stage::Reworking,
        State::Reviewing | State::NeedsReview => Stage::Reviewing,
        State::Ready => Stage::Ready,
        // Awaiting the mandate's cover is not landable yet: never READY.
        State::Blocked | State::Unknown | State::ReadyPendingCover => Stage::Blocked,
    }
}

fn wip_max(root: &Path) -> u64 {
    std::fs::read_to_string(root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|config| config.get("land")?.get("wip_max")?.as_integer())
        .and_then(|max| u64::try_from(max).ok())
        .unwrap_or(DEFAULT_WIP_MAX)
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(args)
        .stdin(std::process::Stdio::null());
    let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(20))?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|line| !line.is_empty())
}

/// The protected branch's last merge and today's count, from the base the
/// pass just fetched.
fn main_branch(root: &Path) -> MainBranch {
    let Ok(base) = crate::gate::review_carryover::protected_base(root) else {
        return MainBranch::default();
    };
    let tip = format!("refs/aida/land/base/{base}");
    let tip = if git_line(root, &["rev-parse", "--verify", "--quiet", &tip]).is_some() {
        tip
    } else {
        format!("origin/{base}")
    };
    let last = git_line(
        root,
        &["log", "-1", "--first-parent", "--format=%h %cI", &tip],
    );
    let (sha, at) = match last.as_deref().and_then(|line| line.split_once(' ')) {
        Some((sha, at)) => (Some(sha.to_string()), Some(at.to_string())),
        None => (None, None),
    };
    MainBranch {
        last_merge: sha,
        last_merge_at: at,
        merges_today: git_line(
            root,
            &[
                "rev-list",
                "--count",
                "--first-parent",
                "--since=midnight",
                &tip,
            ],
        )
        .and_then(|count| count.parse().ok())
        .unwrap_or(0),
    }
}

fn spec_label(id: &str) -> String {
    aida_core::node_alias::display_spec_id(id)
}

fn is_spec(requirement: &aida_core::models::Requirement, id: &str) -> bool {
    use aida_core::id_identity::same_id;
    requirement
        .agreed_id
        .as_deref()
        .is_some_and(|own| same_id(own, id))
        || requirement
            .spec_id
            .as_deref()
            .is_some_and(|own| same_id(own, id))
}

/// A lane's scope when it is a spec (not a review of a change, not a
/// free-form worktree scope).
fn lane_spec(scope: &str) -> Option<String> {
    let ids = crate::pr_ship::extract_spec_ids_from_text(scope);
    (ids.len() == 1 && !scope.contains(':')).then(|| ids[0].clone())
}

/// The snapshot of one pass: every open change and live lane as an item in
/// one stage, the queues, and the attention list.
// trace:STORY-2768 | ai:claude
pub(crate) fn build(pass: &Pass, took: std::time::Duration) -> Snapshot {
    let requirements = crate::store_lookup::load_store_for_lookup(&pass.root)
        .map(|store| store.requirements)
        .unwrap_or_default();
    assemble(
        pass,
        &requirements,
        main_branch(&pass.root),
        wip_max(&pass.root),
        Utc::now(),
        took,
    )
}

/// [`build`] over injected facts.
pub(crate) fn assemble(
    pass: &Pass,
    requirements: &[aida_core::models::Requirement],
    main: MainBranch,
    wip_max: u64,
    now: DateTime<Utc>,
    took: std::time::Duration,
) -> Snapshot {
    use aida_core::models::RequirementStatus as Status;
    let title_of = |id: &str| {
        requirements
            .iter()
            .find(|requirement| is_spec(requirement, id))
            .map(|requirement| requirement.title.clone())
    };
    let mut items: Vec<Item> = Vec::new();
    let mut queues = Queues::default();
    for row in &pass.rows {
        let Some(pr) = pass.changes.iter().find(|pr| pr.id == row.pr) else {
            continue;
        };
        let spec = crate::land::specs_of(pr).into_iter().next();
        let stage = stage_of(&row.state, row.ci.as_deref());
        match (&row.state, &row.action) {
            (State::NeedsReview, _) => queues.awaiting_review += 1,
            (_, Action::DispatchRework) => queues.awaiting_rework += 1,
            _ => {}
        }
        items.push(Item {
            title: spec
                .as_deref()
                .and_then(title_of)
                .unwrap_or_else(|| pr.title.clone()),
            spec: spec
                .as_deref()
                .map(spec_label)
                .unwrap_or_else(|| "-".into()),
            stage,
            actor: row.actor.clone().unwrap_or_else(|| NO_ACTOR.into()),
            pr: Some(pr.id),
            head: Some(pr.head.clone()),
            since: row.since.clone().or_else(|| pr.updated_at.clone()),
            last_progress_at: pr.updated_at.clone(),
            next_action: row.action.label().to_string(),
            owner: row.action.owner().to_string(),
            blocked_by: (stage == Stage::Blocked).then(|| row.reason.clone()),
        });
    }
    // A live lane whose spec has no open change yet is implementing.
    let covered = |scope: &str| {
        pass.changes
            .iter()
            .any(|pr| crate::land::claim_covers(scope, pr))
    };
    let mut lanes: Vec<(String, String, String, String)> = Vec::new();
    for lease in &pass.local_lanes {
        if lease.role.as_deref() == Some("reviewer")
            || pass
                .changes
                .iter()
                .any(|pr| pr.branch.as_deref() == Some(lease.branch.as_str()))
        {
            continue;
        }
        lanes.push((
            lease.scope.clone(),
            format!(
                "{}@{}",
                lease.role.as_deref().unwrap_or("lane"),
                lease.hostname
            ),
            lease.started_at.to_rfc3339(),
            lease.started_at.to_rfc3339(),
        ));
    }
    for claim in &pass.foreign_lanes {
        if claim.review_verb {
            continue;
        }
        lanes.push((
            claim.scope.clone(),
            crate::land::claim_actor(claim),
            claim.started_at.clone(),
            claim.heartbeat_at.clone(),
        ));
    }
    for (scope, actor, started, progress) in lanes {
        let Some(spec) = lane_spec(&scope) else {
            continue;
        };
        if covered(&scope) || items.iter().any(|item| item.spec == spec_label(&spec)) {
            continue;
        }
        items.push(Item {
            title: title_of(&spec).unwrap_or_default(),
            spec: spec_label(&spec),
            stage: Stage::Implementing,
            actor,
            pr: None,
            head: None,
            since: Some(started),
            last_progress_at: Some(progress),
            next_action: "open a change".into(),
            owner: "lane".into(),
            blocked_by: None,
        });
    }
    items.sort_by(|a, b| (a.stage, a.pr, &a.spec).cmp(&(b.stage, b.pr, &b.spec)));

    let has_item = |requirement: &aida_core::models::Requirement| {
        let label = spec_label(&requirement.display_id()); // canonical-id: key matched against item labels
        items.iter().any(|item| item.spec == label)
    };
    queues.approved_not_started = requirements
        .iter()
        .filter(|requirement| requirement.status == Status::Approved && !has_item(requirement))
        .count();

    let older_than = |at: Option<&str>, secs: i64| {
        at.and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| (now - at.with_timezone(&Utc)).num_seconds() > secs)
    };
    let mut attention: Vec<Attention> = Vec::new();
    for item in &items {
        let why = if let Some(blocker) = &item.blocked_by {
            Some(format!("blocked: {blocker}"))
        // Every stage counts, READY included: a change nobody is moving
        // for two hours is the thing this list exists to show.
        } else if item.actor == NO_ACTOR && older_than(item.since.as_deref(), UNOWNED_SECS) {
            Some(format!(
                "unowned: {} with no live actor for over 2h",
                item.stage.label()
            ))
        } else {
            None
        };
        if let Some(why) = why {
            attention.push(Attention {
                spec: match item.pr {
                    Some(pr) => format!("{} (#{pr})", item.spec),
                    None => item.spec.clone(),
                },
                title: item.title.clone(),
                since: item.since.clone(),
                why,
                next_action: item.next_action.clone(),
                owner: item.owner.clone(),
            });
        }
    }
    for requirement in requirements {
        let label = spec_label(&requirement.display_id()); // canonical-id: key matched against item labels
        let item = items.iter().find(|item| item.spec == label);
        let entry = match (&requirement.status, item) {
            (Status::NeedsAttention, Some(item)) if item.actor != NO_ACTOR => Some((
                format!(
                    "marked NeedsAttention while {} is on it ({})",
                    item.actor,
                    item.stage.label()
                ),
                item.next_action.clone(),
                item.actor.clone(),
            )),
            (Status::NeedsAttention, Some(item)) => Some((
                format!(
                    "parked NeedsAttention; its change is {} with no live actor",
                    item.stage.label()
                ),
                item.next_action.clone(),
                item.owner.clone(),
            )),
            (Status::NeedsAttention, None) => Some((
                "parked NeedsAttention, unowned: no live lane and no open change".to_string(),
                "advisor ruling: requeue, reassign or close".to_string(),
                "advisor".to_string(),
            )),
            (Status::InProgress, None)
                if older_than(Some(&requirement.modified_at.to_rfc3339()), UNOWNED_SECS) =>
            {
                Some((
                    "In Progress, unowned: no live lane and no open change".to_string(),
                    "reset to Approved".to_string(),
                    "reconciler".to_string(),
                ))
            }
            _ => None,
        };
        if let Some((why, next_action, owner)) = entry {
            attention.push(Attention {
                spec: label,
                title: requirement.title.clone(),
                since: Some(requirement.modified_at.to_rfc3339()),
                why,
                next_action,
                owner,
            });
        }
    }
    attention.sort_by(|a, b| (&a.since, &a.spec).cmp(&(&b.since, &b.spec)));

    Snapshot {
        version: 1,
        generated_at: now.to_rfc3339(),
        pass_seconds: took.as_secs(),
        open_prs: pass.changes.len(),
        wip_max,
        main,
        items,
        queues,
        attention,
        warnings: pass.warnings.clone(),
    }
}

fn snapshot_path(root: &Path) -> PathBuf {
    root.join(".aida").join("cache").join("now.json")
}

/// Write the snapshot beside the other caches, atomically.
pub(crate) fn write(root: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path(root);
    let dir = path.parent().context("snapshot path has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = dir.join(format!("now.json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(snapshot)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))
}

/// The primary checkout of the project at `start`, found from files alone: a
/// linked worktree's `.git` file names its primary's `.git/worktrees/<name>`.
fn primary_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            return dir.to_path_buf();
        }
        if let Ok(text) = std::fs::read_to_string(&git) {
            if let Some(primary) = text
                .trim()
                .strip_prefix("gitdir:")
                .map(str::trim)
                .and_then(|gitdir| gitdir.split_once("/.git/worktrees/"))
                .map(|(primary, _)| PathBuf::from(primary))
            {
                return primary;
            }
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

fn age(at: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(at) = at.and_then(|at| DateTime::parse_from_rfc3339(at).ok()) else {
        return "-".into();
    };
    let secs = (now - at.with_timezone(&Utc)).num_seconds().max(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d{}h", secs / 86_400, secs % 86_400 / 3600),
    }
}

fn clip(text: &str, width: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= width {
        return flat;
    }
    let mut cut: String = flat.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The screen: header, one block per stage, queues, attention.
pub(crate) fn render(snapshot: &Snapshot, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    let snapshot_age = DateTime::parse_from_rfc3339(&snapshot.generated_at)
        .map(|at| (now - at.with_timezone(&Utc)).num_seconds().max(0))
        .unwrap_or(i64::MAX);
    let main = match (&snapshot.main.last_merge, &snapshot.main.last_merge_at) {
        (Some(sha), at) => format!(
            "main last merge {sha} {} ago · {} today",
            age(at.as_deref(), now),
            snapshot.main.merges_today
        ),
        _ => "main last merge unknown".to_string(),
    };
    let wip = format!(
        "{} open PRs (WIP limit {})",
        snapshot.open_prs, snapshot.wip_max
    );
    out.push_str(&format!(
        "{} snapshot {} old · {main} · {}\n",
        "NOW".bold(),
        age(Some(&snapshot.generated_at), now),
        if snapshot.open_prs as u64 >= snapshot.wip_max {
            wip.yellow().to_string()
        } else {
            wip
        }
    ));
    if snapshot_age > STALE_SECS {
        out.push_str(&format!(
            "{}\n",
            "The snapshot is over 10 minutes old: the landing reconciler is not running. \
             Refresh it with `aida land status`."
                .red()
                .bold()
        ));
    }
    for warning in &snapshot.warnings {
        out.push_str(&format!("{}\n", format!("WARNING {warning}").yellow()));
    }
    for stage in Stage::ALL {
        let rows: Vec<&Item> = snapshot
            .items
            .iter()
            .filter(|item| item.stage == stage)
            .collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{} ({})\n", stage.label().bold(), rows.len()));
        for item in rows {
            out.push_str(&format!(
                "  {:<14} {:<44} {:<26} {:<6} {:>7}  {}\n",
                clip(&item.spec, 14),
                clip(&item.title, 44),
                clip(&item.actor, 26),
                item.pr.map(|pr| format!("#{pr}")).unwrap_or_default(),
                age(item.since.as_deref(), now),
                if item.owner == "-" {
                    item.next_action.clone()
                } else {
                    format!("{} ({})", item.next_action, item.owner)
                }
            ));
        }
    }
    out.push_str(&format!(
        "\n{} approved, not started {} · awaiting review {} · awaiting rework {}\n",
        "QUEUES".bold(),
        snapshot.queues.approved_not_started,
        snapshot.queues.awaiting_review,
        snapshot.queues.awaiting_rework
    ));
    out.push_str(&format!(
        "\n{} ({})\n",
        "ATTENTION".bold(),
        snapshot.attention.len()
    ));
    if snapshot.attention.is_empty() {
        out.push_str("  nothing needs you\n");
    }
    for entry in &snapshot.attention {
        out.push_str(&format!(
            "  {:<22} {:>7}  {}\n  {:<22} {:>7}  next: {} ({})\n",
            clip(&entry.spec, 22),
            age(entry.since.as_deref(), now),
            clip(&entry.why, 120),
            "",
            "",
            clip(&entry.next_action, 90),
            entry.owner
        ));
    }
    out
}

/// What is being worked on right now: every in-flight spec by stage with its
/// actor, change, age and next action, the queues, and everything that needs
/// attention with its reason and owner. Reads the snapshot the landing
/// reconciler writes; refresh it with `aida land status`.
// trace:STORY-2768 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "now")]
pub(crate) struct NowArgs {
    /// Re-render every 10 seconds until interrupted.
    #[clap(long)]
    pub watch: bool,
    /// Print the snapshot itself.
    #[clap(long)]
    pub json: bool,
}

fn read(root: &Path) -> Result<(String, Snapshot)> {
    let path = snapshot_path(root);
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no snapshot at {}: run `aida land status` once to write it",
            path.display()
        )
    })?;
    let snapshot = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a snapshot this build reads", path.display()))?;
    Ok((text, snapshot))
}

fn run(matches: &clap::ArgMatches, ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <NowArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let root = primary_root(&std::env::current_dir()?);
    let json =
        args.json || ctx.globals.and_then(|g| g.format) == Some(crate::cmdkit::OutputFormat::Json);
    loop {
        let (text, snapshot) = read(&root)?;
        if json {
            println!("{}", text.trim_end());
        } else {
            if args.watch {
                // Clear the screen and home the cursor.
                print!("\x1b[2J\x1b[H");
            }
            print!("{}", render(&snapshot, Utc::now()));
        }
        if !args.watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

// trace:STORY-2768 | ai:claude
pub(crate) static NOW: CommandReg = CommandReg {
    name: "now",
    help_after: "land",
    command: <NowArgs as clap::CommandFactory>::command,
    policy: |_| CmdPolicy {
        stakeholder: StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    },
    run: Handler::Early(run),
};

#[cfg(test)]
#[path = "tests/story_2768_now_tests.rs"]
mod tests;
