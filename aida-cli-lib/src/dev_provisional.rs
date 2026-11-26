//! `aida dev provisional <branch|PR>`: during an outage, run a fail-closed
//! fix as the live binary before it lands. The named head is built and
//! smoke-tested in a fresh detached worktree, then promoted through the
//! ordinary `aida dev build` swap (old pair kept in `live/prev/`) with a
//! PROVISIONAL marker in the promotion record. The marker travels with its
//! generation, so `aida dev rollback` takes it off the live pair.
// trace:STORY-3148 | ai:claude

use anyhow::{bail, ensure, Context, Result};
use std::path::Path;

/// Who ran a provisional promote, of what, and why. Stored in the live
/// pair's `.promoted` record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProvisionalMarker {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The full commit SHA that was built and promoted.
    pub head: String,
    pub operator: String,
    pub reason: String,
    /// RFC 3339 time of the promote.
    pub at: String,
}

/// What `<branch|PR>` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Pr(u64),
    Branch(String),
}

/// Private ref the named head is fetched into.
const PROVISIONAL_REF: &str = "refs/aida/provisional/head";

/// PURE: `None` when an operator may run the provisional promote.
pub(crate) fn operator_refusal(
    operator_present: bool,
    headless: bool,
    lane: bool,
    role: &str,
    reason: &str,
) -> Option<String> {
    if !crate::pr_emergency_land::operator_allowed(operator_present, headless, lane, role) {
        return Some(
            "`aida dev provisional` replaces the binary every session runs, so only an operator \
             may run it: from a terminal or an operator-attributed deputy job. Lanes, headless \
             runs and implementer/reviewer sessions are refused. Nothing was changed."
                .into(),
        );
    }
    reason.trim().is_empty().then(|| {
        "--reason must say why this head runs live before review. Nothing was changed.".into()
    })
}

/// PURE: a PR number (`4615` or `#4615`) or a branch name.
pub(crate) fn parse_target(raw: &str) -> Result<Target> {
    let raw = raw.trim();
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Target::Pr(
            digits.parse().context("PR number out of range")?,
        ));
    }
    let bad = raw.is_empty()
        || raw.starts_with('-')
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || ":^~?*[\\".contains(c));
    ensure!(!bad, "`{raw}` is neither a PR number nor a branch name");
    Ok(Target::Branch(raw.to_string()))
}

/// PURE: `None` when `--expect-head` (required for a PR) is exactly the
/// resolved head, so the head cannot move between review and swap.
pub(crate) fn expect_head_refusal(pr: bool, expect: Option<&str>, head: &str) -> Option<String> {
    match expect.map(str::trim) {
        None if !pr => None,
        None => Some("a PR target needs --expect-head <full SHA you reviewed>".into()),
        Some(e) if e.len() == 40 && e.eq_ignore_ascii_case(head.trim()) => None,
        Some(e) => Some(format!("--expect-head {e} is not the resolved head {head}")),
    }
}

/// Checks `--expect-head`, shows the resolved head, then builds and promotes.
pub(crate) fn promote_expected(
    layout: &crate::dev_build::DevLayout,
    marker: &ProvisionalMarker,
    expect: Option<&str>,
    steps: &mut dyn crate::dev_build::BuildSteps,
    out: &mut dyn std::io::Write,
) -> Result<crate::dev_build::BuildOutcome> {
    if let Some(why) = expect_head_refusal(marker.pr.is_some(), expect, &marker.head) {
        bail!("refusing the provisional promote: {why}. Nothing was changed.");
    }
    use std::io::Write as _;
    writeln!(
        out,
        "Resolved head: {} (promoting this commit)",
        marker.head
    )?;
    let opts = crate::dev_build::DevBuildOptions::default();
    crate::dev_build::run_dev_build_with(layout, &opts, Some(marker), steps, out)
}

/// PURE: the line `aida dev status` shows first while live is provisional.
pub(crate) fn status_line(m: &ProvisionalMarker) -> String {
    let what = match (m.pr, m.branch.as_deref()) {
        (Some(pr), Some(b)) => format!("PR #{pr} ({b})"),
        (Some(pr), None) => format!("PR #{pr}"),
        (None, Some(b)) => format!("branch {b}"),
        (None, None) => "an unreviewed head".to_string(),
    };
    format!(
        "PROVISIONAL:  live runs {what} at {}, not a landed main; promoted by {} at {}: {}. \
         `aida dev rollback` restores the previous build.",
        &m.head[..m.head.len().min(12)],
        m.operator,
        m.at,
        m.reason.trim()
    )
}

fn git_command(repo: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = aida_core::git_ops::host_git_command(repo);
    cmd.args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null());
    cmd
}

fn run(mut cmd: std::process::Command, what: &str) -> Result<String> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    run(git_command(repo, args), &format!("git {}", args.join(" ")))
}

fn operator_label(env: &aida_core::exec_context::Environment) -> String {
    let user = env.var("USER").unwrap_or("unknown").to_string();
    match env.var(crate::gate::kernel_deps::DEPUTY_JOB_ENV) {
        Ok(job) if !job.is_empty() => format!("{user} (deputy job {job})"),
        _ => user,
    }
}

pub(crate) fn handle(raw_target: &str, reason: &str, expect: Option<&str>) -> Result<()> {
    let env = aida_core::exec_context::Environment::capture();
    if let Some(why) = operator_refusal(
        crate::pr_service::ship_interactive_tty(),
        env.var("AIDA_HEADLESS").unwrap_or("") == "1",
        env.var("AIDA_CONTAINMENT_POSTURE").unwrap_or("") == "native",
        &crate::role::effective_role(),
        reason,
    ) {
        bail!("{why}");
    }
    let target = parse_target(raw_target)?;
    let primary = crate::find_main_worktree_root()?;
    ensure!(
        aida_core::seat_clone_detect::seat_clone_root(&primary).is_none(),
        "`aida dev provisional` runs from the operator checkout, never a lane clone"
    );
    let (src, pr, branch) = match &target {
        Target::Pr(n) => (
            crate::session_start::pr_head_remote_ref(&primary, *n),
            Some(*n),
            crate::pr_lookup::pr_head_branch(&primary, *n),
        ),
        Target::Branch(b) => {
            git(&primary, &["check-ref-format", "--branch", b])?;
            (format!("refs/heads/{b}"), None, Some(b.clone()))
        }
    };
    let refspec = format!("+{src}:{PROVISIONAL_REF}");
    let mut fetch = git_command(&primary, &["fetch", "--no-tags", "origin", &refspec]);
    {
        use aida_core::git_ops::RemoteCredentialsExt as _;
        fetch
            .env("GIT_TERMINAL_PROMPT", "0")
            .remote_credentials(&primary, "origin");
    }
    run(fetch, "git fetch origin")?;
    let head = git(
        &primary,
        &[
            "rev-parse",
            "--verify",
            &format!("{PROVISIONAL_REF}^{{commit}}"),
        ],
    )?;
    // Serialise with `aida dev build` and the scheduled promote.
    let _lock = crate::dev_build::try_checkout_build_lock(&primary)?
        .context("build in progress in the primary checkout")?;
    // A fresh checkout of exactly `head`: never a PR checkout's own state.
    let base = primary.join("target").join("provisional");
    let tree = base.join("src");
    let tree_arg = tree.display().to_string();
    let _ = git(&primary, &["worktree", "remove", "--force", &tree_arg]);
    if tree.exists() {
        std::fs::remove_dir_all(&tree).with_context(|| format!("clearing {tree_arg}"))?;
    }
    let _ = git(&primary, &["worktree", "prune"]);
    git(&primary, &["worktree", "add", "--detach", &tree_arg, &head])?;
    let layout = crate::dev_build::DevLayout {
        repo: tree.clone(),
        target_dir: base.join("target"),
        live_dir: crate::dev_build::default_live_dir()?,
        link_dir: crate::dev_build::default_link_dir()?,
    };
    layout.check_live_outside_target()?;
    let marker = ProvisionalMarker {
        pr,
        branch,
        head,
        operator: operator_label(&env),
        reason: reason.trim().to_string(),
        at: chrono::Utc::now().to_rfc3339(),
    };
    let steps = &mut crate::dev_build::RealSteps;
    let result = promote_expected(&layout, &marker, expect, steps, &mut std::io::stdout());
    let _ = git(&primary, &["worktree", "remove", "--force", &tree_arg]);
    result?;
    println!("{}", status_line(&marker));
    Ok(())
}
