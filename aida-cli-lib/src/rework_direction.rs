//! What a rework pickup prompt leads with (BUG-057).
//!
//! An advisor steers a relaunched lane with spec comments: rulings
//! (`Advisor ruling …`, BUG-021) and briefs (`ADVISOR BRIEF …` /
//! `REWORK BRIEF …`). The newest brief written after the last review verdict
//! is the current task, whatever rulings came before or after it. The prompt
//! puts it first and marks the older verdict's findings as superseded
//! context. A brief recorded before the last verdict is shown as context
//! that defers to the newer findings. Before this, a lane re-ran the old
//! verdict's work and skipped the merge-only brief. A rework PR that
//! conflicts with main gets a "resolve conflicts first" task above
//! everything else.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude

use chrono::{DateTime, Utc};

/// The first non-empty line of a comment, lowercased, with leading markdown
/// emphasis / heading / bracket characters stripped.
fn lead_line(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches(['#', '*', '_', '[', ' '])
        .to_ascii_lowercase()
}

/// Whether a comment is an advisor brief: its first line starts with
/// `Advisor brief` or `Rework brief` (any case).
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn is_brief_comment(content: &str) -> bool {
    let first = lead_line(content);
    first.starts_with("advisor brief") || first.starts_with("rework brief")
}

/// Every comment on the spec, replies included.
fn flatten(comments: &[aida_core::Comment]) -> Vec<&aida_core::Comment> {
    fn walk<'a>(comments: &'a [aida_core::Comment], out: &mut Vec<&'a aida_core::Comment>) {
        for c in comments {
            out.push(c);
            walk(&c.replies, out);
        }
    }
    let mut all = Vec::new();
    walk(comments, &mut all);
    all
}

/// The newest comment (replies included) whose content matches `pred`.
pub(crate) fn latest_matching(
    comments: &[aida_core::Comment],
    pred: fn(&str) -> bool,
) -> Option<&aida_core::Comment> {
    flatten(comments)
        .into_iter()
        .filter(|c| pred(&c.content))
        .max_by_key(|c| c.created_at)
}

/// The verdict time a findings block carries on its `Recorded:` line.
fn parse_recorded_line(findings: &str) -> Option<DateTime<Utc>> {
    let raw = findings
        .lines()
        .find_map(|l| l.trim().strip_prefix("Recorded:"))?
        .trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Some(t.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(raw.trim_end_matches(" UTC"), fmt) {
            return Some(t.and_utc());
        }
    }
    None
}

/// When the last review verdict landed: the latest of the live findings
/// block's `Recorded:` time and the newest findings block recorded as a
/// spec comment. `None` when neither gives a time.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn last_verdict_at(
    findings: Option<&str>,
    comments: &[aida_core::Comment],
) -> Option<DateTime<Utc>> {
    let from_block = findings.and_then(parse_recorded_line);
    let from_comments = flatten(comments)
        .into_iter()
        .filter(|c| crate::review_verdict_display::is_findings_block(&c.content))
        .map(|c| c.created_at)
        .max();
    from_block.max(from_comments)
}

/// How the newest brief stands against the review findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BriefStanding {
    /// Written after the last verdict: the brief is the task and the
    /// verdict's findings become superseded context.
    SupersedesVerdict,
    /// No review findings are open: the brief is binding direction.
    Binding,
    /// Recorded before the last verdict (or the verdict has no known time):
    /// context only; the newer findings stay the task.
    PredatesVerdict,
}

/// The advisor direction a rework pickup leads with: the newest brief and
/// the newest ruling, chosen independently, so a ruling recorded after a
/// post-verdict brief does not hide the brief.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AdvisorDirection<'a> {
    pub brief: Option<(&'a aida_core::Comment, BriefStanding)>,
    pub ruling: Option<&'a aida_core::Comment>,
}

impl AdvisorDirection<'_> {
    /// Whether the brief replaces the verdict's findings as the task.
    pub(crate) fn supersedes_verdict(&self) -> bool {
        matches!(self.brief, Some((_, BriefStanding::SupersedesVerdict)))
    }
}

/// Resolve the spec's advisor direction against the live review findings.
/// A verdict with no known time is treated as current: the findings stay
/// authoritative and any brief is context.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn resolve_direction<'a>(
    comments: &'a [aida_core::Comment],
    review_findings: Option<&str>,
) -> AdvisorDirection<'a> {
    let ruling = latest_matching(comments, crate::queue_cmd::is_ruling_comment);
    let brief = latest_matching(comments, is_brief_comment).map(|b| {
        let standing = if review_findings.is_none() {
            BriefStanding::Binding
        } else {
            match last_verdict_at(review_findings, comments) {
                Some(at) if b.created_at > at => BriefStanding::SupersedesVerdict,
                _ => BriefStanding::PredatesVerdict,
            }
        };
        (b, standing)
    });
    AdvisorDirection { brief, ruling }
}

/// A rework PR whose branch does not merge cleanly into main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MainConflict {
    pub pr: u64,
    pub base: String,
    pub files: Vec<String>,
}

/// Forecast whether the rework PR's branch merges cleanly into the default
/// branch. Fetches both first (best effort), then runs `git merge-tree`
/// without touching any worktree. A clean or unknown result returns `None`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn detect_main_conflict(
    project_root: &std::path::Path,
    pr: u64,
    head_branch: &str,
) -> Option<MainConflict> {
    let base = crate::worktree_gc::detect_default_branch_ref(project_root)?;
    let base_name = base.strip_prefix("origin/").unwrap_or(&base).to_string();
    // Never prompt for credentials: with no terminal a prompt would hang the
    // launch (or a test) instead of failing the best-effort fetch.
    let _ = aida_core::git_ops::host_git_command(project_root)
        .args(["fetch", "--quiet", "origin", "--"])
        .arg(&base_name)
        .arg(head_branch)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let head = format!("origin/{head_branch}");
    match crate::worktree_gc::forecast_rebase_onto(project_root, &base, &head) {
        crate::integrate::RebaseForecast::Conflict(files) => Some(MainConflict {
            pr,
            base: base_name,
            files,
        }),
        _ => None,
    }
}

/// The "resolve conflicts with main first" task.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn conflict_task(conflict: &MainConflict) -> String {
    let files = if conflict.files.is_empty() {
        String::new()
    } else {
        format!(" (conflicting files: {})", conflict.files.join(", "))
    };
    format!(
        "FIRST TASK — resolve conflicts with {base} first: PR #{pr} conflicts with \
         {base}{files}. Run `git fetch origin {base} && git merge origin/{base}`, resolve \
         every conflict, commit the merge, and push before any other work. The PR cannot \
         merge until this is done.",
        base = conflict.base,
        pr = conflict.pr,
    )
}

fn comment_block(label: &str, comment: &aida_core::Comment, scope: &str) -> String {
    format!(
        "{label} ({} by {}) — {scope}:\n{}",
        comment.created_at.format("%Y-%m-%d %H:%M UTC"),
        comment.author,
        comment.content.trim()
    )
}

/// The brief, worded for how it stands against the findings.
fn brief_block(brief: &aida_core::Comment, standing: BriefStanding) -> String {
    let scope = match standing {
        BriefStanding::SupersedesVerdict => {
            "the newest advisor brief, written after the last review verdict. This is your \
             task; it supersedes the older review findings below and the spec description \
             where they differ"
        }
        BriefStanding::Binding => {
            "binding; where it differs from the spec description, the brief wins"
        }
        BriefStanding::PredatesVerdict => {
            "OLDER CONTEXT, recorded before the latest review verdict. The review findings \
             above are the current task; where this brief differs from them, the findings win"
        }
    };
    comment_block("ADVISOR BRIEF", brief, scope)
}

fn ruling_block(ruling: &aida_core::Comment) -> String {
    comment_block(
        "ADVISOR RULING",
        ruling,
        "binding; where it differs from the spec description, the ruling wins",
    )
}

/// Older review findings, shown for context only.
fn superseded_block(findings: &str) -> String {
    format!(
        "SUPERSEDED CONTEXT — older review verdict, recorded before the advisor brief above. \
         Do not act on it where it differs from the brief:\n{}",
        findings.trim()
    )
}

/// Lead a pickup prompt with what the lane must do first: a conflict with
/// main; then a post-verdict (or binding) brief; then the newest ruling;
/// then, when a brief supersedes the verdict, the old findings as
/// superseded context; then the pickup prompt. A pre-verdict brief is not a
/// lead: it goes after the pickup prompt's authoritative findings, as older
/// context, just above the final skill line, which stays last.
/// Agent gates have a stricter verdict contract and get the prompt as-is.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn lead_rework_prompt(
    prompt: String,
    direction: &AdvisorDirection<'_>,
    superseded_findings: Option<&str>,
    conflict: Option<&MainConflict>,
) -> String {
    if std::env::var("AIDA_AGENT_GATE_NAME").is_ok() {
        return prompt;
    }
    let mut blocks = Vec::new();
    if let Some(c) = conflict {
        blocks.push(conflict_task(c));
    }
    let leading_brief = direction
        .brief
        .filter(|(_, s)| *s != BriefStanding::PredatesVerdict);
    if let Some((b, s)) = leading_brief {
        blocks.push(brief_block(b, s));
    }
    if let Some(r) = direction.ruling {
        blocks.push(ruling_block(r));
    }
    if let Some(f) = superseded_findings.filter(|_| direction.supersedes_verdict()) {
        blocks.push(superseded_block(f));
    }
    let prompt = match direction.brief {
        Some((b, s @ BriefStanding::PredatesVerdict)) => {
            append_before_skill_line(prompt, &brief_block(b, s))
        }
        _ => prompt,
    };
    if blocks.is_empty() {
        return prompt;
    }
    blocks.push(prompt);
    blocks.join("\n\n")
}

/// Insert `block` after the body of `prompt` but before its final
/// `/aida-…` skill line, so background context follows the findings and the
/// skill line stays last.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
fn append_before_skill_line(prompt: String, block: &str) -> String {
    let trimmed = prompt.trim_end();
    match trimmed.rsplit_once('\n') {
        Some((body, last)) if last.trim_start().starts_with("/aida-") => {
            format!("{}\n\n{block}\n\n{last}", body.trim_end())
        }
        _ if trimmed.trim_start().starts_with("/aida-") => format!("{block}\n\n{trimmed}"),
        _ => format!("{trimmed}\n\n{block}"),
    }
}

/// Everything the queue-work pickup prompt is built from, gathered by
/// `handle_queue_work`.
pub(crate) struct PickupInputs<'a> {
    pub plan: &'a crate::queue_cmd::QueueWorkPlan,
    pub role: &'a str,
    pub plan_only: bool,
    pub guided: bool,
    /// The live review findings block, if the spec is in rework.
    pub review_findings: Option<&'a str>,
    /// The spec's comments (replies included in the tree).
    pub spec_comments: &'a [aida_core::Comment],
    /// The rework PR's conflict with main, if any.
    pub conflict: Option<&'a MainConflict>,
}

/// Assemble the queue-work pickup prompt: the base prompt (with the
/// findings as the authoritative round unless a newer brief supersedes
/// them), led by the advisor direction and any conflict with main for an
/// implementer item pickup.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn assemble_pickup_prompt(inputs: &PickupInputs<'_>) -> String {
    let implementer_item = !inputs.role.eq_ignore_ascii_case("reviewer")
        && !inputs.plan_only
        && !inputs.guided
        && inputs.plan.mode == crate::queue_cmd::QueueWorkMode::Item;
    // Agent gates keep the findings in the base prompt: they get no lead.
    let gate = std::env::var("AIDA_AGENT_GATE_NAME").is_ok();
    let direction = if implementer_item && !gate {
        resolve_direction(inputs.spec_comments, inputs.review_findings)
    } else {
        AdvisorDirection::default()
    };
    let supersedes = direction.supersedes_verdict();
    let prompt = crate::queue_cmd::derive_rework_queue_work_prompt(
        inputs.plan,
        inputs.role,
        inputs.plan_only,
        inputs.guided,
        inputs.review_findings.filter(|_| !supersedes),
        if inputs.review_findings.is_some() {
            inputs.spec_comments
        } else {
            &[][..]
        },
    );
    if !implementer_item {
        return prompt;
    }
    lead_rework_prompt(
        prompt,
        &direction,
        inputs.review_findings.filter(|_| supersedes),
        inputs.conflict,
    )
}

#[cfg(test)]
#[path = "tests/rework_direction_tests.rs"]
mod tests;
