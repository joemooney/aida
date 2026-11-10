//! What a rework pickup prompt leads with (BUG-057).
//!
//! An advisor steers a relaunched lane with spec comments: rulings
//! (`Advisor ruling …`, BUG-021) and briefs (`ADVISOR BRIEF …`,
//! `ADVISOR REWORK BRIEF round N …` /
//! `REWORK BRIEF …`). The newest brief written after the last review verdict
//! supersedes the older verdict's findings. The newest advisor brief or
//! ruling leads; earlier advisor direction is labelled superseded background.
//! A brief recorded before the last verdict is shown as context that defers
//! to the newer findings. Before this, a lane re-ran the old
//! verdict's work and skipped the merge-only brief. A rework PR that
//! conflicts with main gets a "resolve conflicts first" task above
//! everything else.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude

use chrono::{DateTime, Utc};

/// The first non-empty line of a comment, lowercased, with leading markdown
/// emphasis / heading / bracket characters stripped.
pub(crate) fn lead_line(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_start_matches(['#', '*', '_', '[', ' '])
        .to_ascii_lowercase()
}

/// Whether a comment is an advisor brief: its first line starts with
/// `Advisor brief`, `Advisor rework brief` or `Rework brief` (any case,
/// after markdown heading / emphasis). `ADVISOR REWORK BRIEF round N` is
/// the header the advisor actually writes.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn is_brief_comment(content: &str) -> bool {
    let first = lead_line(content);
    ["advisor brief", "advisor rework brief", "rework brief"]
        .iter()
        .any(|p| first.starts_with(p))
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

/// When the last review verdict landed: the live findings block's
/// `Recorded:` time. Only when the block has no usable `Recorded:` line does
/// the newest findings block recorded as a spec comment stand in. That
/// comment uses its own `Recorded:` time when available, otherwise its creation
/// time. It is written by `aida queue rework`, usually after the advisor's
/// brief, so it must never push the cut-off past a real `Recorded:` time.
/// `None` when neither gives a time.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
// trace:BUG-2509 | ai:codex
pub(crate) fn last_verdict_at(
    findings: Option<&str>,
    comments: &[aida_core::Comment],
) -> Option<DateTime<Utc>> {
    findings.and_then(parse_recorded_line).or_else(|| {
        flatten(comments)
            .into_iter()
            .filter(|c| crate::review_verdict_display::is_findings_block(&c.content))
            .map(|c| parse_recorded_line(&c.content).unwrap_or(c.created_at))
            .max()
    })
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
/// the newest ruling. Their timestamps determine which leads; the older
/// comment remains visible only as superseded background. Brief standing
/// against review findings is resolved independently of advisor ordering.
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
        // trace:BUG-2509 | ai:codex
        let verdict_at = last_verdict_at(review_findings, comments);
        let standing = if review_findings.is_none() && verdict_at.is_none() {
            BriefStanding::Binding
        } else {
            match verdict_at {
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

/// How long the best-effort conflict-forecast fetch may run.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
const CONFLICT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Forecast whether the rework PR's branch merges cleanly into the default
/// branch. Fetches both first, then runs `git merge-tree` without touching
/// any worktree. A clean or unknown result, or a fetch that failed or timed
/// out, returns `None`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn detect_main_conflict(
    project_root: &std::path::Path,
    pr: u64,
    head_branch: &str,
) -> Option<MainConflict> {
    let base = crate::worktree_gc::detect_default_branch_ref(project_root)?;
    let base_name = base.strip_prefix("origin/").unwrap_or(&base).to_string();
    // Never prompt for credentials: with no terminal a prompt would hang the
    // launch (or a test) instead of failing the best-effort fetch. Stored
    // remote credentials are routed off argv like every other remote call.
    // The forecast is best-effort, so a stalled connection must not block
    // the launch either: the fetch is bounded. A failed or timed-out fetch
    // skips the forecast: merge-tree on stale refs could report a conflict
    // already fixed on the remote, or miss a new one.
    let mut fetch = aida_core::git_ops::host_remote_git_command(project_root, "origin");
    fetch
        .args(["fetch", "--quiet", "origin", "--"])
        .arg(&base_name)
        .arg(head_branch)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(fetch, CONFLICT_FETCH_TIMEOUT) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => {}
        _ => return None,
    }
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

/// Earlier advisor direction retains its history without granting it task authority.
// trace:BUG-2489 | ai:codex
fn superseded_direction_block(label: &str, comment: &aida_core::Comment) -> String {
    comment_block(
        label,
        comment,
        "EARLIER DIRECTION, superseded by the newer advisor direction above. Background \
         only; do not act on it where it differs from that direction",
    )
}

/// Older review findings, shown for context only.
fn superseded_block(findings: &str) -> String {
    format!(
        "SUPERSEDED CONTEXT — older review verdict, superseded by a later advisor brief. \
         Follow the current advisor direction above where it differs from these findings:\n{}",
        findings.trim()
    )
}

/// Lead with a conflict with main, then the newest advisor direction.
/// Older advisor direction is superseded background below the pickup body.
/// A pre-verdict brief still defers to the findings and follows the body;
/// a post-verdict brief still supersedes the findings even if a later ruling
/// supersedes that brief. The final skill line stays last.
/// Agent gates have a stricter verdict contract and get the prompt as-is.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
// trace:BUG-2489 | ai:codex
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
    let mut background = Vec::new();
    // Equal timestamps prefer the ruling deterministically; never emit two
    // binding advisor blocks for one pickup.
    let ruling_leads = match (direction.brief, direction.ruling) {
        (Some((brief, _)), Some(ruling)) => ruling.created_at >= brief.created_at,
        (None, Some(_)) => true,
        _ => false,
    };
    if ruling_leads {
        blocks.push(ruling_block(direction.ruling.unwrap()));
    }
    if let Some((brief, standing)) = direction.brief {
        if ruling_leads {
            background.push(superseded_direction_block("ADVISOR BRIEF", brief));
        } else if standing == BriefStanding::PredatesVerdict {
            background.push(brief_block(brief, standing));
        } else {
            blocks.push(brief_block(brief, standing));
        }
    }
    if !ruling_leads {
        if let Some(ruling) = direction.ruling {
            background.push(superseded_direction_block("ADVISOR RULING", ruling));
        }
    }
    if let Some(f) = superseded_findings.filter(|_| direction.supersedes_verdict()) {
        blocks.push(superseded_block(f));
    }
    let mut prompt = prompt;
    for block in background {
        prompt = append_before_skill_line(prompt, &block);
    }
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

/// Distinguish absent evidence from a review that authoritatively cleared findings.
// trace:BUG-2509 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewResult<T> {
    Available(T),
    ClearedByApproval,
    Unavailable,
}

impl ReviewResult<String> {
    pub(crate) fn as_deref(&self) -> ReviewResult<&str> {
        match self {
            Self::Available(value) => ReviewResult::Available(value.as_str()),
            Self::ClearedByApproval => ReviewResult::ClearedByApproval,
            Self::Unavailable => ReviewResult::Unavailable,
        }
    }

    pub(crate) fn findings(&self) -> Option<&str> {
        match self {
            Self::Available(value) => Some(value),
            _ => None,
        }
    }
}

/// Everything the queue-work pickup prompt is built from, gathered by
/// `handle_queue_work`.
pub(crate) struct PickupInputs<'a> {
    pub plan: &'a crate::queue_cmd::QueueWorkPlan,
    pub role: &'a str,
    pub plan_only: bool,
    pub guided: bool,
    /// Local review evidence; only Unavailable permits durable-comment fallback.
    pub review_findings: ReviewResult<&'a str>,
    /// The spec's comments (replies included in the tree).
    pub spec_comments: &'a [aida_core::Comment],
    /// The rework PR's conflict with main, if any.
    pub conflict: Option<&'a MainConflict>,
    /// The PR head's failing CI tests (BUG-2380), when the spec is in rework.
    /// It sits on top of the base prompt, below the advisor leads.
    pub ci_failures: Option<&'a str>,
}

/// Assemble the queue-work pickup prompt: the base prompt (with the
/// findings as the authoritative round unless a newer brief supersedes
/// them), led by the advisor direction and any conflict with main for an
/// implementer item pickup.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn assemble_pickup_prompt(inputs: &PickupInputs<'_>) -> String {
    // Remote lanes may have no host gate ledger. Keep the durable hand-off
    // visible as well as using it to order advisor direction.
    // trace:BUG-2509 | ai:codex
    let review_findings = match inputs.review_findings {
        ReviewResult::Available(findings) => Some(findings),
        ReviewResult::ClearedByApproval => None,
        ReviewResult::Unavailable => latest_matching(
            inputs.spec_comments,
            crate::review_verdict_display::is_findings_block,
        )
        .map(|comment| comment.content.as_str()),
    };
    let implementer_item = !inputs.role.eq_ignore_ascii_case("reviewer")
        && !inputs.plan_only
        && !inputs.guided
        && inputs.plan.mode == crate::queue_cmd::QueueWorkMode::Item;
    // Agent gates keep the findings in the base prompt: they get no lead.
    let gate = std::env::var("AIDA_AGENT_GATE_NAME").is_ok();
    let direction = if implementer_item && !gate {
        let mut direction = resolve_direction(inputs.spec_comments, review_findings);
        // A local clearance also retires the historical verdict timestamp.
        if matches!(inputs.review_findings, ReviewResult::ClearedByApproval) {
            if let Some((_, standing)) = &mut direction.brief {
                *standing = BriefStanding::Binding;
            }
        }
        direction
    } else {
        AdvisorDirection::default()
    };
    let supersedes = direction.supersedes_verdict();
    let prompt = crate::queue_cmd::derive_rework_queue_work_prompt(
        inputs.plan,
        inputs.role,
        inputs.plan_only,
        inputs.guided,
        review_findings.filter(|_| !supersedes),
        if review_findings.is_some() {
            inputs.spec_comments
        } else {
            &[][..]
        },
    );
    // trace:BUG-2380 | ai:claude
    let prompt = match inputs.ci_failures {
        Some(section) => format!("{}\n\n{prompt}", section.trim_end()),
        None => prompt,
    };
    let prompt = if implementer_item {
        lead_rework_prompt(
            prompt,
            &direction,
            review_findings.filter(|_| supersedes),
            inputs.conflict,
        )
    } else {
        prompt
    };
    // Preserve leading slash invocations for native expansion and skill inlining.
    // Append after assembly so existing rework context ordering is unchanged.
    // trace:STORY-2630 | ai:codex
    format!(
        "{prompt}\n\n{}",
        include_str!("../../aida-core/templates/dispatch/code-navigation.md").trim()
    )
}

#[cfg(test)]
#[path = "tests/rework_direction_tests.rs"]
mod tests;
