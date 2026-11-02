//! `aida goal advisor set | show | check | history`: the operator's standing
//! goal for the advisor seat, stored in the requirements store
//! (`aida_core::advisor_goal`) so it survives a new or rotated advisor
//! session and reads the same from every vendor.
//!
//! Only the operator sets it: `set` refuses from any agent session, any
//! session carrying a seat grant, any process under an agent harness, and
//! any non-interactive terminal. `show`, `check` and `history` work from any
//! seat.
// trace:STORY-2243 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::advisor_goal::{self, AdvisorGoal, ClauseKind, GoalClauseRecord};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use super::AdvisorGoalAction;

/// What `set` refuses with.
pub(crate) const OPERATOR_ONLY: &str = "only the operator sets the advisor's goal: run \
    `aida goal advisor set` yourself at an interactive terminal, outside any agent session";

/// Environment variables whose presence marks an agent session or a seat
/// grant, whichever vendor launched it.
const AGENT_SESSION_MARKERS: &[&str] = &[
    "AIDA_AGENT_NAME",
    "AIDA_AGENT_TYPE",
    // The seat-grant handles, named by their owner (TASK-1752).
    crate::gate::seat_authority::GRANT_ENV,
    crate::gate::seat_authority::GRANT_FILE_ENV,
    "AIDA_HEADLESS",
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "ANTIGRAVITY_SESSION_ID",
];

/// What the operator-only decision looks at.
#[derive(Debug, Default, Clone)]
pub(crate) struct SetterFacts {
    pub stdin_tty: bool,
    pub stderr_tty: bool,
    /// The agent-session markers set in the environment.
    pub markers: Vec<String>,
    /// A recognised agent harness (Claude Code, Codex, Gemini, Antigravity)
    /// is an ancestor of this process.
    pub under_harness: bool,
}

impl SetterFacts {
    fn observed() -> Self {
        Self {
            stdin_tty: crate::gate::kernel_deps::authority_stdin_is_terminal(),
            stderr_tty: crate::gate::kernel_deps::authority_stderr_is_terminal(),
            markers: AGENT_SESSION_MARKERS
                .iter()
                .filter(|k| std::env::var(k).ok().is_some_and(|v| !v.trim().is_empty()))
                .map(|k| k.to_string())
                .collect(),
            under_harness: crate::gate::review_authority::IdentitySource::live()
                .agent_root()
                .is_some(),
        }
    }
}

/// Why `set` is refused, or `None` for a direct operator. A PTY is not
/// enough: an agent session, a grant or a harness ancestor refuses too.
pub(crate) fn set_refusal(facts: &SetterFacts) -> Option<String> {
    if let Some(marker) = facts.markers.first() {
        return Some(format!(
            "{OPERATOR_ONLY} (this session is an agent session: {marker} is set)"
        ));
    }
    if facts.under_harness {
        return Some(format!(
            "{OPERATOR_ONLY} (this process runs under an agent harness)"
        ));
    }
    if !(facts.stdin_tty && facts.stderr_tty) {
        return Some(format!("{OPERATOR_ONLY} (no interactive terminal)"));
    }
    None
}

fn main_root() -> Result<PathBuf> {
    crate::find_main_worktree_root()
        .or_else(|_| crate::find_project_root())
        .context("not inside an AIDA project")
}

fn store_root(main_root: &Path) -> PathBuf {
    main_root.join(".aida-store")
}

/// How a read action renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fmt {
    Human,
    Toon,
    Json,
}

impl Fmt {
    /// `--json` (or `--format json`) is JSON; an agent seat gets toon.
    fn observed(json: bool) -> Self {
        if json || crate::output_format_is_json() {
            Fmt::Json
        } else if crate::agent_output_mode() {
            Fmt::Toon
        } else {
            Fmt::Human
        }
    }
}

pub(crate) fn handle(action: &AdvisorGoalAction) -> Result<()> {
    match action {
        AdvisorGoalAction::Set {
            text,
            batch,
            epic,
            spec,
            pr,
            queue_empty,
        } => {
            if let Some(why) = set_refusal(&SetterFacts::observed()) {
                anyhow::bail!(why);
            }
            let text = text.trim();
            if text.is_empty() {
                anyhow::bail!("the goal text is empty");
            }
            let root = main_root()?;
            let any_clause = batch.is_some()
                || epic.is_some()
                || spec.is_some()
                || pr.is_some()
                || queue_empty.is_some();
            let clauses = if any_clause {
                super::build_goal_clauses_for_forge(
                    batch.as_deref(),
                    epic.as_deref(),
                    spec.as_deref(),
                    *pr,
                    queue_empty.as_deref(),
                    crate::forge::resolve_forge_kind(&root),
                )?
                .iter()
                .map(super::GoalClause::to_record)
                .collect()
            } else {
                Vec::new()
            };
            let goal = AdvisorGoal {
                text: text.to_string(),
                clauses,
                set_by: crate::current_user_id(None),
                set_at: Utc::now(),
                scope: aida_core::git_ops::default_branch_name(&root),
            };
            advisor_goal::set_cas(&store_root(&root), goal.clone())?;
            println!("Advisor goal set.");
            print!("{}", render_goal(Some(&goal), Fmt::Human));
            Ok(())
        }
        AdvisorGoalAction::Show { json } => {
            let log = advisor_goal::load(&store_root(&main_root()?))?;
            print!("{}", render_goal(log.active(), Fmt::observed(*json)));
            Ok(())
        }
        AdvisorGoalAction::History { json } => {
            let log = advisor_goal::load(&store_root(&main_root()?))?;
            let goals: Vec<&AdvisorGoal> = log.history().collect();
            print!("{}", render_history(&goals, Fmt::observed(*json)));
            Ok(())
        }
        AdvisorGoalAction::Check { json } => {
            let root = main_root()?;
            let log = advisor_goal::load(&store_root(&root))?;
            let mut evaluator = StoreEvaluator::new(&root);
            let outcome = run_check(
                log.active(),
                |c| evaluator.evaluate(c),
                Fmt::observed(*json),
            );
            print!("{}", outcome.output);
            if outcome.exit_code != 0 {
                std::process::exit(outcome.exit_code);
            }
            Ok(())
        }
    }
}

// ────────────────────────────── check ──────────────────────────────

/// One clause's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ClauseResult {
    pub description: String,
    pub verify: String,
    pub met: bool,
    /// Why it is unmet (or could not be checked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CheckReport {
    pub text: String,
    pub clauses: Vec<ClauseResult>,
}

impl CheckReport {
    /// Met only with at least one clause and every clause met.
    pub(crate) fn met(&self) -> bool {
        !self.clauses.is_empty() && self.clauses.iter().all(|c| c.met)
    }
}

/// Evaluate every clause of `goal` with `eval` (`Ok(())` met, `Err(why)`
/// unmet), in order.
pub(crate) fn check_goal(
    goal: &AdvisorGoal,
    mut eval: impl FnMut(&GoalClauseRecord) -> std::result::Result<(), String>,
) -> CheckReport {
    CheckReport {
        text: goal.text.clone(),
        clauses: goal
            .clauses
            .iter()
            .map(|c| {
                let outcome = eval(c);
                ClauseResult {
                    description: c.description.clone(),
                    verify: c.verify.clone(),
                    met: outcome.is_ok(),
                    detail: outcome.err(),
                }
            })
            .collect(),
    }
}

/// What `aida goal advisor check` prints and the status it exits with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckOutcome {
    pub output: String,
    /// 0 only when the goal has clauses and every one is met; 1 when a
    /// clause is unmet, the goal is free text only, or no goal is set.
    pub exit_code: i32,
}

/// The whole `check` action for `goal` (`None`: no goal set), with `eval`
/// deciding each clause. `handle` prints `output` and exits `exit_code`.
pub(crate) fn run_check(
    goal: Option<&AdvisorGoal>,
    eval: impl FnMut(&GoalClauseRecord) -> std::result::Result<(), String>,
    fmt: Fmt,
) -> CheckOutcome {
    let Some(goal) = goal else {
        return CheckOutcome {
            output: render_check_no_goal(fmt),
            exit_code: 1,
        };
    };
    let report = check_goal(goal, eval);
    CheckOutcome {
        output: render_check(&report, fmt),
        exit_code: if report.met() { 0 } else { 1 },
    }
}

/// Evaluates clauses against the project's store, queue files and forge —
/// the same conditions each clause's verify command shows.
struct StoreEvaluator<'a> {
    root: &'a Path,
    store: Option<std::result::Result<aida_core::RequirementsStore, String>>,
}

impl<'a> StoreEvaluator<'a> {
    fn new(root: &'a Path) -> Self {
        Self { root, store: None }
    }

    fn store(&mut self) -> std::result::Result<&aida_core::RequirementsStore, String> {
        let root = self.root;
        self.store
            .get_or_insert_with(|| {
                aida_core::Storage::new(store_root(root))
                    .load()
                    .map_err(|e| format!("cannot read the store: {e:#}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn evaluate(&mut self, clause: &GoalClauseRecord) -> std::result::Result<(), String> {
        let arg = clause.arg.as_str();
        match clause.kind {
            ClauseKind::Batch => {
                let tag = format!("batch:{arg}");
                let store = self.store()?;
                let open: Vec<&aida_core::Requirement> = store
                    .requirements
                    .iter()
                    .filter(|r| r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
                    .filter(|r| !aida_core::conflict::is_terminal_status(&r.status))
                    .collect();
                unresolved(&open)
            }
            ClauseKind::Epic => {
                let store = self.store()?;
                let id = crate::parse_requirement_id(arg, store)
                    .map_err(|_| format!("{arg} is not in the store"))?;
                let children: std::collections::HashSet<uuid::Uuid> =
                    aida_core::graph_walk::oriented_hierarchy_edges(store)
                        .into_iter()
                        .filter(|(parent, _)| *parent == id)
                        .map(|(_, child)| child)
                        .collect();
                let open: Vec<&aida_core::Requirement> = store
                    .requirements
                    .iter()
                    .filter(|r| children.contains(&r.id))
                    .filter(|r| !aida_core::conflict::is_terminal_status(&r.status))
                    .collect();
                unresolved(&open)
            }
            ClauseKind::Spec => {
                let store = self.store()?;
                let id = crate::parse_requirement_id(arg, store)
                    .map_err(|_| format!("{arg} is not in the store"))?;
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == id)
                    .ok_or_else(|| format!("{arg} is not in the store"))?;
                if req.status == aida_core::RequirementStatus::Completed {
                    Ok(())
                } else {
                    Err(format!("{arg} is {}", req.status))
                }
            }
            ClauseKind::Pr => {
                let n: u64 = arg
                    .parse()
                    .map_err(|_| format!("`{arg}` is not a change number"))?;
                let meta = crate::forge::forge_for(self.root)
                    .change_metadata(n, &mut crate::network_retry::NoopSink)
                    .map_err(|e| format!("cannot read #{n} from the forge: {e:#}"))?;
                if meta.state == crate::forge::ChangeState::Merged {
                    Ok(())
                } else {
                    Err(format!("#{n} is {:?}", meta.state).to_lowercase())
                }
            }
            ClauseKind::QueueEmpty => {
                let role = crate::canonical_role_name(arg);
                let storage = aida_core::Storage::new(store_root(self.root));
                let entries = crate::queue_role_fallback::queue_entries_routed_to_role(
                    &storage, &role, false,
                )
                .map_err(|e| format!("cannot read the queue: {e:#}"))?;
                let store = self.store()?;
                let queued: Vec<&aida_core::Requirement> = entries
                    .iter()
                    .filter_map(|e| store.requirements.iter().find(|r| r.id == e.requirement_id))
                    .filter(|r| !aida_core::conflict::is_terminal_status(&r.status))
                    .collect();
                if queued.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "{} queued for {role}: {}",
                        queued.len(),
                        display_ids(&queued)
                    ))
                }
            }
        }
    }
}

fn display_ids(reqs: &[&aida_core::Requirement]) -> String {
    const SHOWN: usize = 5;
    let mut ids: Vec<String> = reqs
        .iter()
        .take(SHOWN)
        .map(|r| {
            r.agreed_id
                .clone()
                .or_else(|| r.spec_id.clone())
                .unwrap_or_else(|| r.id.to_string())
        })
        .collect();
    if reqs.len() > SHOWN {
        ids.push(format!("+{} more", reqs.len() - SHOWN));
    }
    ids.join(", ")
}

fn unresolved(open: &[&aida_core::Requirement]) -> std::result::Result<(), String> {
    if open.is_empty() {
        Ok(())
    } else {
        Err(format!("{} unresolved: {}", open.len(), display_ids(open)))
    }
}

// ────────────────────────────── render ─────────────────────────────

const NO_GOAL: &str = "No advisor goal set. The operator sets one with \
    `aida goal advisor set \"<text>\"`.";

fn stamp(goal: &AdvisorGoal) -> String {
    goal.set_at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// The `show` output for `goal` (`None`: no goal set).
pub(crate) fn render_goal(goal: Option<&AdvisorGoal>, fmt: Fmt) -> String {
    match fmt {
        Fmt::Json => {
            let value = serde_json::json!({ "goal": goal });
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            )
        }
        Fmt::Toon => match goal {
            None => "goal: none\n".to_string(),
            Some(g) => {
                let mut out = format!(
                    "goal: {}\nset_by: {}\nset_at: {}\nscope: {}\nclauses[{}]:\n",
                    g.text,
                    g.set_by,
                    g.set_at.to_rfc3339(),
                    g.scope,
                    g.clauses.len()
                );
                for c in &g.clauses {
                    out.push_str(&format!("  - {} | verify: {}\n", c.description, c.verify));
                }
                out
            }
        },
        Fmt::Human => match goal {
            None => format!("{NO_GOAL}\n"),
            Some(g) => {
                let mut out = format!(
                    "Advisor goal (set by {} at {}, scope {}):\n  {}\n",
                    g.set_by,
                    stamp(g),
                    g.scope,
                    g.text
                );
                if g.clauses.is_empty() {
                    out.push_str("Clauses: none (free text only; `check` cannot mark it met)\n");
                } else {
                    out.push_str("Clauses:\n");
                    for (i, c) in g.clauses.iter().enumerate() {
                        out.push_str(&format!(
                            "  {}. {}\n     verify: {}\n",
                            i + 1,
                            c.description,
                            c.verify
                        ));
                    }
                }
                out
            }
        },
    }
}

/// The `history` output, `goals` newest first.
pub(crate) fn render_history(goals: &[&AdvisorGoal], fmt: Fmt) -> String {
    match fmt {
        Fmt::Json => {
            let value = serde_json::json!({ "goals": goals });
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            )
        }
        _ if goals.is_empty() => match fmt {
            Fmt::Toon => "goals[0]:\n".to_string(),
            _ => format!("{NO_GOAL}\n"),
        },
        Fmt::Toon => {
            let mut out = format!("goals[{}]:\n", goals.len());
            for (i, g) in goals.iter().enumerate() {
                let state = if i == 0 { "active" } else { "replaced" };
                out.push_str(&format!(
                    "  - {} | {} | {} | clauses: {} | {}\n",
                    g.set_at.to_rfc3339(),
                    g.set_by,
                    state,
                    g.clauses.len(),
                    g.text
                ));
            }
            out
        }
        Fmt::Human => {
            let mut out = String::new();
            for (i, g) in goals.iter().enumerate() {
                let state = if i == 0 { "active" } else { "replaced" };
                out.push_str(&format!(
                    "{}  {}  {:<8}  {}\n",
                    stamp(g),
                    g.set_by,
                    state,
                    g.text
                ));
                for c in &g.clauses {
                    out.push_str(&format!("    · {}\n", c.description));
                }
            }
            out
        }
    }
}

/// The `check` output.
pub(crate) fn render_check(report: &CheckReport, fmt: Fmt) -> String {
    let unmet = report.clauses.iter().filter(|c| !c.met).count();
    match fmt {
        Fmt::Json => {
            let value = serde_json::json!({
                "goal": report.text,
                "met": report.met(),
                "checkable": !report.clauses.is_empty(),
                "clauses": report.clauses,
            });
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            )
        }
        Fmt::Toon => {
            let mut out = format!(
                "goal: {}\nmet: {}\ncheckable: {}\nclauses[{}]:\n",
                report.text,
                report.met(),
                !report.clauses.is_empty(),
                report.clauses.len()
            );
            for c in &report.clauses {
                let state = if c.met { "met" } else { "unmet" };
                out.push_str(&format!("  - {state} | {}", c.description));
                if let Some(d) = &c.detail {
                    out.push_str(&format!(" | detail: {d}"));
                }
                if !c.met {
                    out.push_str(&format!(" | verify: {}", c.verify));
                }
                out.push('\n');
            }
            out
        }
        Fmt::Human if report.clauses.is_empty() => format!(
            "Goal: {}\nno checkable clauses: this goal is free text only and is never \
             reported met\n",
            report.text
        ),
        Fmt::Human => {
            let mut out = format!("Goal: {}\n", report.text);
            for c in &report.clauses {
                let mark = if c.met { "met  " } else { "UNMET" };
                out.push_str(&format!("  {mark} {}\n", c.description));
                if let Some(d) = &c.detail {
                    out.push_str(&format!("        {d}\n"));
                }
                if !c.met {
                    out.push_str(&format!("        verify: {}\n", c.verify));
                }
            }
            if unmet == 0 {
                out.push_str("All clauses met.\n");
            } else {
                out.push_str(&format!(
                    "{unmet} of {} clause{} unmet.\n",
                    report.clauses.len(),
                    if report.clauses.len() == 1 { "" } else { "s" }
                ));
            }
            out
        }
    }
}

/// The `check` output when no goal is set.
fn render_check_no_goal(fmt: Fmt) -> String {
    match fmt {
        Fmt::Json => {
            let value = serde_json::json!({
                "goal": null,
                "met": false,
                "checkable": false,
                "clauses": [],
            });
            format!(
                "{}\n",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            )
        }
        Fmt::Toon => "goal: none\nmet: false\ncheckable: false\nclauses[0]:\n".to_string(),
        Fmt::Human => format!("{NO_GOAL}\n"),
    }
}

// ─────────────────────── surfaces (launch, handoff) ─────────────────

/// The launch-context `## Goal` section for an advisor seat: the active
/// goal, read from the store at launch. `None` for every other seat and when
/// no goal is set.
pub(crate) fn render_launch_goal_section(store_root: &Path, role: &str) -> Option<String> {
    if crate::canonical_role_name(role) != "advisor" {
        return None;
    }
    let goal = advisor_goal::active(store_root)?;
    Some(format!(
        "## Goal\n\nThe operator's standing goal for this seat:\n\n{}(Point-in-time; run \
         `aida goal advisor show` for the current goal and `aida goal advisor check` to \
         test it.)\n\n",
        render_goal(Some(&goal), Fmt::Human)
    ))
}

/// The `ADVISOR GOAL` block `aida session handoff --seat advisor --show`
/// prints, or `None` for another seat or with no goal set.
pub(crate) fn handoff_goal_block(store_root: &Path, seat: &str) -> Option<String> {
    if crate::canonical_role_name(seat) != "advisor" {
        return None;
    }
    let goal = advisor_goal::active(store_root)?;
    Some(format!(
        "ADVISOR GOAL\n{}",
        render_goal(Some(&goal), Fmt::Human)
    ))
}

/// The whoami `goal:` text for an advisor seat: the active goal's text.
pub(crate) fn whoami_goal_text(store_root: &Path, role: &str) -> Option<String> {
    if crate::canonical_role_name(role) != "advisor" {
        return None;
    }
    advisor_goal::active(store_root).map(|g| g.text)
}

#[cfg(test)]
#[path = "tests/story_2243_advisor_goal_tests.rs"]
mod tests;
