//! What a rework pickup prompt leads with (BUG-057).
//!
//! An advisor steers a relaunched lane with spec comments: rulings
//! (`Advisor ruling …`, BUG-021) and briefs (`ADVISOR BRIEF …` /
//! `REWORK BRIEF …`). A brief written after the last review verdict is the
//! current task. The prompt puts it first and marks the older verdict's
//! findings as superseded context. Before this, a lane re-ran the old
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

/// Whether a comment steers the next lane: an advisor ruling or a brief.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn is_advisor_direction_comment(content: &str) -> bool {
    is_brief_comment(content) || crate::queue_cmd::is_ruling_comment(content)
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

/// The ruling or brief recorded LAST on the spec, replies included.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn latest_advisor_direction(
    comments: &[aida_core::Comment],
) -> Option<&aida_core::Comment> {
    latest_matching(comments, is_advisor_direction_comment)
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

/// Whether `direction` is a brief written after the last verdict, so the
/// verdict's findings no longer set the task. A verdict with no known time is
/// treated as current: the findings stay authoritative.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn brief_supersedes_verdict(
    direction: Option<&aida_core::Comment>,
    verdict_at: Option<DateTime<Utc>>,
) -> bool {
    match (direction, verdict_at) {
        (Some(d), Some(at)) => is_brief_comment(&d.content) && d.created_at > at,
        _ => false,
    }
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
    let _ = aida_core::git_ops::host_git_command(project_root)
        .args(["fetch", "--quiet", "origin", "--"])
        .arg(&base_name)
        .arg(head_branch)
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

/// The newest advisor direction as a binding block.
fn direction_block(direction: &aida_core::Comment, supersedes: bool) -> String {
    let label = if is_brief_comment(&direction.content) {
        "ADVISOR BRIEF"
    } else {
        "ADVISOR RULING"
    };
    let scope = if supersedes {
        "the newest advisor direction, written after the last review verdict. This is your \
         task; it supersedes the older review findings below and the spec description where \
         they differ"
    } else {
        "binding; where it differs from the spec description, the ruling wins"
    };
    format!(
        "{label} ({} by {}) — {scope}:\n{}",
        direction.created_at.format("%Y-%m-%d %H:%M UTC"),
        direction.author,
        direction.content.trim()
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
/// main, then the newest advisor direction, then (when that direction is a
/// brief newer than the verdict) the old findings as superseded context.
/// The pickup prompt stays last, so the skill line is still the final line.
/// Agent gates have a stricter verdict contract and get the prompt as-is.
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
pub(crate) fn lead_rework_prompt(
    prompt: String,
    direction: Option<&aida_core::Comment>,
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
    if let Some(d) = direction {
        blocks.push(direction_block(d, superseded_findings.is_some()));
    }
    if let Some(f) = superseded_findings {
        blocks.push(superseded_block(f));
    }
    if blocks.is_empty() {
        return prompt;
    }
    blocks.push(prompt);
    blocks.join("\n\n")
}
