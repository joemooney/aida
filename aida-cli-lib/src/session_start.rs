//! `aida session start` and its preflight, pool and dirty-tree gates (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// STORY-61: forge-specific PR/MR scope. When `aida session start
/// --owns PR-1` or `--owns MR-42` is invoked, we route through a
/// review-branch fetch + worktree-on-existing-branch flow instead of
/// the default new-branch flow.
/// trace:STORY-61 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewForge {
    GitHub,
    GitLab,
}

impl ReviewForge {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" | "gh" => Some(Self::GitHub),
            "gitlab" | "glab" => Some(Self::GitLab),
            _ => None,
        }
    }

    /// Standard refspec for the head of a PR/MR (works for fork PRs
    /// too because both forges expose the head ref on origin).
    pub(crate) fn pr_head_ref(&self, n: u64) -> String {
        match self {
            Self::GitHub => format!("pull/{}/head", n),
            Self::GitLab => format!("merge-requests/{}/head", n),
        }
    }

    pub(crate) fn local_branch_for(&self, n: u64) -> String {
        match self {
            Self::GitHub => format!("pr-{}", n),
            Self::GitLab => format!("mr-{}", n),
        }
    }

    /// STORY-71: forge CLI binary name used to enrich the lease with the
    /// PR's head/base SHAs (and to surface a clear stderr note when it's
    /// not on PATH).
    /// trace:STORY-71 | ai:claude
    pub(crate) fn cli_name(&self) -> &'static str {
        match self {
            Self::GitHub => "gh",
            Self::GitLab => "glab",
        }
    }

    pub(crate) fn cli_install_url(&self) -> &'static str {
        match self {
            Self::GitHub => "https://cli.github.com",
            Self::GitLab => "https://gitlab.com/gitlab-org/cli",
        }
    }

    // trace:STORY-1164 | ai:codex
    pub(crate) fn forge_kind(&self) -> crate::forge::ForgeKind {
        match self {
            Self::GitHub => crate::forge::ForgeKind::GitHub,
            Self::GitLab => crate::forge::ForgeKind::GitLab,
        }
    }
}

/// STORY-71: PR/MR head + base metadata captured at session-start time.
/// Recorded into the lease so `aida session show` can display it without
/// re-querying the forge, and so a reviewer can recompute the diff range
/// later even after the PR has moved on.
/// trace:STORY-71 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct PrMetadata {
    pub(crate) head_sha: Option<String>,
    pub(crate) base_sha: Option<String>,
    pub(crate) base_ref: Option<String>,
}

/// STORY-71: query the forge CLI for a PR/MR's head/base SHAs + base ref.
/// Returns `Err(reason)` when the CLI isn't installed or the query fails
/// (caller turns that into a stderr note + Default fallback) and
/// `Ok(None)` when the JSON parsed but key fields were missing.
/// trace:STORY-71 | ai:claude
pub(crate) fn query_pr_metadata(
    forge: ReviewForge,
    n: u64,
    project_root: &std::path::Path,
) -> std::result::Result<PrMetadata, String> {
    let cli = forge.cli_name();
    let n_str = n.to_string();
    let args: Vec<&str> = match forge {
        ReviewForge::GitHub => vec![
            "pr",
            "view",
            n_str.as_str(),
            "--json",
            "headRefOid,baseRefOid,baseRefName",
        ],
        ReviewForge::GitLab => vec!["mr", "view", n_str.as_str(), "--output", "json"],
    };
    // gh/glab inherit the working directory; both honor cwd to find the
    // matching repo for the PR/MR number, so we set current_dir rather
    // than relying on `-C` which neither CLI accepts.
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy()
        .map_err(|e| {
            // TASK-50: the install URL belongs to the `not on PATH`
            // case only — that's the one where "go install this" is
            // the relevant action. Other failures (spawn errors,
            // exited-with-failure, JSON parse) mean the CLI is
            // present but unhappy; appending the install URL there
            // would be confusing. The outer call-site keeps its
            // message terse so the URL stops appearing twice for
            // the missing-binary path. trace:TASK-50 | ai:claude
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "`{}` not on PATH — install from {}",
                    cli,
                    forge.cli_install_url()
                )
            } else {
                format!("`{}` failed to spawn: {}", cli, e)
            }
        })?;
    if !out.status.success() {
        return Err(format!(
            "`{} {}` exited {}",
            cli,
            args.join(" "),
            out.status
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("`{}` JSON parse failed: {}", cli, e))?;
    Ok(parse_pr_metadata_json(forge, &json))
}

/// STORY-71: extract the head SHA / base SHA / base ref out of the JSON
/// the forge CLI returned. Pulled out of `query_pr_metadata` so unit
/// tests can pin the parsing without invoking the CLI.
/// trace:STORY-71 | ai:claude
pub(crate) fn parse_pr_metadata_json(forge: ReviewForge, json: &serde_json::Value) -> PrMetadata {
    let s = |v: &serde_json::Value| -> Option<String> {
        v.as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
    };
    match forge {
        ReviewForge::GitHub => PrMetadata {
            head_sha: s(&json["headRefOid"]),
            base_sha: s(&json["baseRefOid"]),
            base_ref: s(&json["baseRefName"]),
        },
        ReviewForge::GitLab => {
            // glab's mr view --output json field names mirror GitLab's
            // REST API: `sha` (head), `diff_refs.base_sha`, `target_branch`.
            // Source-of-truth list: `glab api projects/:id/merge_requests/N`.
            PrMetadata {
                head_sha: s(&json["sha"]),
                base_sha: s(&json["diff_refs"]["base_sha"]),
                base_ref: s(&json["target_branch"]),
            }
        }
    }
}

/// TASK-76: query the PR/MR's source/head branch name via gh/glab. Used by
/// the session_start pre-flight to detect when the PR's source branch is
/// already held by another lease (which would make `gh pr checkout` fail
/// with `branch already used by worktree`). Returns None when the forge
/// CLI isn't installed or fails — pre-flight degrades to "no warning."
/// trace:TASK-76 | ai:claude
pub(crate) fn query_pr_source_branch(
    forge: ReviewForge,
    n: u64,
    project_root: &std::path::Path,
) -> Option<String> {
    let (cli, args): (&str, Vec<String>) = match forge {
        ReviewForge::GitHub => (
            "gh",
            vec![
                "pr".into(),
                "view".into(),
                n.to_string(),
                "--json".into(),
                "headRefName".into(),
                "-q".into(),
                ".headRefName".into(),
            ],
        ),
        ReviewForge::GitLab => (
            "glab",
            vec![
                "mr".into(),
                "view".into(),
                n.to_string(),
                "-F".into(),
                "json".into(),
            ],
        ),
    };
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    match forge {
        ReviewForge::GitHub => Some(stdout),
        ReviewForge::GitLab => {
            // glab's mr view --output json doesn't accept -q; parse the
            // `source_branch` field out of the JSON ourselves.
            let json: serde_json::Value = serde_json::from_str(&stdout).ok()?;
            json["source_branch"].as_str().map(|s| s.to_string())
        }
    }
}

/// STORY-61: parse `PR-N` / `MR-N` scope strings (case-insensitive).
/// Returns the implied forge + PR number when the scope matches; None
/// otherwise (lets normal scope handling proceed).
/// trace:STORY-61 | ai:claude
pub(crate) fn parse_review_scope(scope: &str) -> Option<(ReviewForge, u64)> {
    let trimmed = scope.trim();
    let (prefix, rest) = trimmed.split_once('-')?;
    let n: u64 = rest.parse().ok()?;
    match prefix.to_ascii_uppercase().as_str() {
        "PR" => Some((ReviewForge::GitHub, n)),
        "MR" => Some((ReviewForge::GitLab, n)),
        _ => None,
    }
}

/// TASK-85: short label for a review forge + number, e.g. "PR-14" / "MR-7".
/// Centralized so the error messages stay consistent with how STORY-66 /
/// STORY-90 produce the auto-queue titles. trace:TASK-85 | ai:claude
pub(crate) fn format_review_label(forge: ReviewForge, n: u64) -> String {
    let prefix = match forge {
        ReviewForge::GitHub => "PR",
        ReviewForge::GitLab => "MR",
    };
    format!("{}-{}", prefix, n)
}

/// TASK-85: predicate for `aida queue work PR-N` resolution. Returns
/// true when the requirement title is a review story for this PR/MR —
/// i.e. starts with `Review <LABEL>:` (case-insensitive on the prefix,
/// exact on the number). Kept pure so the resolver path can be unit
/// tested without a fake store. trace:TASK-85 | ai:claude
pub(crate) fn review_title_matches(title: &str, forge: ReviewForge, n: u64) -> bool {
    let label = format_review_label(forge, n);
    let lower = title.trim_start().to_ascii_lowercase();
    let want = format!("review {}:", label.to_ascii_lowercase());
    lower.starts_with(&want)
}

/// STORY-61: detect the project's forge by inspecting `origin`'s URL.
/// Returns None for hosts we don't recognize (Bitbucket, self-hosted
/// without telltale domain, etc.) — caller can require `--forge`.
/// trace:STORY-61 | ai:claude
pub(crate) fn detect_forge_from_origin(project_root: &std::path::Path) -> Option<ReviewForge> {
    // BUG-1228: ONE forge detector. This used to inline its own origin-URL
    // match that only knew `github.com`, `gitlab.com` and `/gitlab/`, so a
    // self-hosted GitLab (gitlab.<yourdomain>) fell through to `None` and the
    // reviewer/`--owns` paths defaulted to GitHub — fetching `refs/pull/N/head`
    // on a GitLab origin (TASK-1254 live finding). Route through
    // `forge::resolve_forge_kind`, which honours `[forge]` config and the
    // shared host heuristics. trace:BUG-1228 | ai:claude
    match crate::forge::resolve_forge_kind(project_root) {
        crate::forge::ForgeKind::GitHub => Some(ReviewForge::GitHub),
        crate::forge::ForgeKind::GitLab => Some(ReviewForge::GitLab),
        _ => None,
    }
}

/// BUG-1228: the remote-side head ref of PR/MR `n` for `forge`, in the full
/// `refs/…` form a fetch refspec wants (`refs/pull/N/head` on GitHub,
/// `refs/merge-requests/N/head` on GitLab). Pure; see
/// [`pr_head_remote_ref`] for the project-resolved form.
// trace:BUG-1228 | ai:claude
pub(crate) fn pr_head_remote_ref_for(forge: ReviewForge, n: u64) -> String {
    format!("refs/{}", forge.pr_head_ref(n))
}

/// BUG-1228: [`pr_head_remote_ref_for`] with the forge resolved from the
/// project (config, else origin URL); GitHub when nothing is detectable so the
/// pre-BUG-1228 behaviour is preserved for local-only repos.
// trace:BUG-1228 | ai:claude
pub(crate) fn pr_head_remote_ref(project_root: &std::path::Path, n: u64) -> String {
    pr_head_remote_ref_for(
        detect_forge_from_origin(project_root).unwrap_or(ReviewForge::GitHub),
        n,
    )
}

/// True if `branch` exists either locally or as `origin/<branch>`. Used by
/// STORY-65 auto-branch resolution. Treats any git error as "doesn't exist"
/// — better to try the name and let `git worktree add -b` fail loudly than
/// to refuse to start a session because origin is unreachable.
/// trace:STORY-65 | ai:claude
pub(crate) fn branch_exists_anywhere(project_root: &std::path::Path, branch: &str) -> bool {
    let local = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", branch),
        ])
        .output();
    if matches!(&local, Ok(o) if o.status.success()) {
        return true;
    }
    aida_core::git_ops::remote_branch_exists(project_root, "origin", branch)
}

#[cfg(test)]
#[path = "tests/bug_442_distributed_detection_tests.rs"]
mod bug_442_distributed_detection_tests;

/// TASK-245: decide whether `aida session start` should check out an
/// existing branch instead of forking a new one. True when
/// `--reuse-branch` was passed, or when an explicitly-named `--branch`
/// already exists (auto-reuse). An auto-derived branch name
/// (`branch_explicit == false`) never reuses — `resolve_session_branch`
/// already picked a name free locally and on origin.
/// trace:TASK-245 | ai:claude
pub(crate) fn should_reuse_branch(
    reuse_flag: bool,
    branch_explicit: bool,
    branch_preexists: bool,
) -> bool {
    reuse_flag || (branch_explicit && branch_preexists)
}

// trace:BUG-1270 | ai:codex
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PrBranchAlignment {
    Proceed,
    FastForward,
    RefuseDiverged,
}

/// Pure decision for an existing local branch that may back an open PR.
// trace:BUG-1270 | ai:codex
pub(crate) fn pr_branch_alignment(
    has_open_pr: bool,
    local_is_ancestor_of_remote: Option<bool>,
    remote_is_ancestor_of_local: Option<bool>,
) -> PrBranchAlignment {
    if !has_open_pr {
        return PrBranchAlignment::Proceed;
    }
    match (local_is_ancestor_of_remote, remote_is_ancestor_of_local) {
        (Some(true), Some(false)) => PrBranchAlignment::FastForward,
        (Some(false), Some(false)) => PrBranchAlignment::RefuseDiverged,
        _ => PrBranchAlignment::Proceed,
    }
}

pub(crate) fn git_is_ancestor(
    project_root: &std::path::Path,
    older: &str,
    newer: &str,
) -> Option<bool> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["merge-base", "--is-ancestor", older, newer])
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

pub(crate) fn session_git_ref_exists(project_root: &std::path::Path, ref_name: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "--quiet", ref_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Keep an explicitly reused PR branch aligned with its remote before a
/// worktree is created. Behind is safe to fast-forward; two-sided divergence
/// requires an explicit realign because choosing history to discard is an
/// operator decision.
// trace:BUG-1270 | ai:codex
pub(crate) fn align_reused_pr_branch(project_root: &std::path::Path, branch: &str) -> Result<()> {
    let has_open_pr = matches!(
        change_lookup_for_branch(project_root, branch),
        crate::forge::ChangeLookup::Found(_)
    );
    if has_open_pr {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        let fetch = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["fetch", "origin", &refspec])
            .output()?;
        if !fetch.status.success() {
            anyhow::bail!(
                "could not refresh PR branch `{branch}` from origin before launch: {}",
                String::from_utf8_lossy(&fetch.stderr).trim()
            );
        }
    }
    align_reused_pr_branch_with_status(project_root, branch, has_open_pr)
}

pub(crate) fn align_reused_pr_branch_with_status(
    project_root: &std::path::Path,
    branch: &str,
    has_open_pr: bool,
) -> Result<()> {
    let local_ref = format!("refs/heads/{branch}");
    let remote_ref = format!("refs/remotes/origin/{branch}");
    if !session_git_ref_exists(project_root, &local_ref)
        || !session_git_ref_exists(project_root, &remote_ref)
    {
        return Ok(());
    }

    match pr_branch_alignment(
        has_open_pr,
        git_is_ancestor(project_root, &local_ref, &remote_ref),
        git_is_ancestor(project_root, &remote_ref, &local_ref),
    ) {
        PrBranchAlignment::Proceed => Ok(()),
        PrBranchAlignment::FastForward => {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(project_root)
                // trace:BUG-1622 | ai:claude
                .args([
                    "branch",
                    "-f",
                    git_arg_guard::END_OF_OPTIONS,
                    branch,
                    &format!("origin/{branch}"),
                ])
                .status()?;
            if !status.success() {
                anyhow::bail!("could not fast-forward local branch `{branch}` to `origin/{branch}`");
            }
            eprintln!(
                "{} fast-forwarded PR branch `{}` to `origin/{}` before creating its worktree",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(), branch, branch
            );
            Ok(())
        }
        PrBranchAlignment::RefuseDiverged => anyhow::bail!(
            "refusing to launch on PR branch `{branch}` because local `{branch}` has diverged from `origin/{branch}`; creating a side branch would strand this round's commits off the PR. Realign it explicitly, then retry:\n  git fetch origin {branch} && git branch -f {branch} origin/{branch}"
        ),
    }
}

/// STORY-248: resolve the stacked-branch base for `aida queue work`.
///
/// Returns `Ok(None)` when neither `--stack` nor `--base` is set — the
/// caller then proceeds with the default origin/main fork.
///
/// With `--stack`: walks the active lease set, filters to implementer
/// roles with a branch that has NOT been merged, skips the lease whose
/// worktree covers cwd (so a session can't stack on itself), and picks
/// the freshest by `started_at`. Returns an error if nothing qualifies.
///
/// With `--base BRANCH`: validates the branch exists locally or on
/// origin via `branch_exists_anywhere`. Unless `force` is set, checks
/// `detect_merged_pr_for_branch` and refuses if the PR has merged
/// (which means the branch is about to be / has been deleted on origin
/// — branching from it now would create commits doomed to be stranded).
/// The error names `--force-base` as the override.
///
/// Pure-ish: the lease + branch / PR lookups are all best-effort
/// against the project's git state. trace:STORY-248 | ai:claude
pub(crate) fn resolve_stack_base(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stack: bool,
    base: Option<&str>,
    force: bool,
) -> anyhow::Result<Option<String>> {
    if !stack && base.is_none() {
        return Ok(None);
    }
    if let Some(b) = base {
        if !branch_exists_anywhere(project_root, b) {
            anyhow::bail!(
                "--base `{}` does not exist locally or on origin — \
                 verify the branch name (and that origin has it)",
                b
            );
        }
        if !force {
            let merged = detect_merged_pr_for_branch_via_forge(project_root, b);
            if let PrLookup::Found(pr) = &merged {
                anyhow::bail!(
                    "--base `{}` is already merged (PR-{}); branching from it \
                     would strand any new commits. Run `aida pull` and pass \
                     `--base main` (or a different un-merged branch), or pass \
                     `--force-base` to override.",
                    b,
                    pr.number
                );
            }
        }
        return Ok(Some(b.to_string()));
    }
    // `--stack` — auto-detect.
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut candidates: Vec<SessionLease> = list_leases(project_root)
        .into_iter()
        .filter(|l| {
            l.role
                .as_deref()
                .map(|r| r.eq_ignore_ascii_case("implementer"))
                .unwrap_or(false)
        })
        .filter(|l| !lease_covers_cwd(l, &canon_cwd))
        .filter(|l| {
            !matches!(
                detect_merged_pr_for_branch_via_forge(project_root, &l.branch),
                PrLookup::Found(_)
            )
        })
        .collect();
    candidates.sort_by_key(|l| std::cmp::Reverse(l.started_at));
    let Some(pick) = candidates.into_iter().next() else {
        anyhow::bail!(
            "--stack: no un-merged in-flight implementer branch found — \
             start one with `aida queue work <SPEC>` (default base origin/main), \
             then run `--stack` from a separate session"
        );
    };
    Ok(Some(pick.branch))
}

#[cfg(test)]
#[path = "tests/session_start_reuse_tests.rs"]
mod session_start_reuse_tests;

/// STORY-248: `resolve_stack_base` integration tests against a real
/// temp git repo. Covers the "no flags" pass-through and the
/// `--base BRANCH` validation legs. The `--stack` auto-detect leg also
/// runs against a temp repo with a hand-written lease file, exercising
/// the lease walker without spawning `gh` (we use a branch with no PR,
/// so `detect_merged_pr_for_branch` returns NoOpenPr / GhMissing — both
/// pass the "not merged" filter). trace:STORY-248 | ai:claude
#[cfg(test)]
#[path = "tests/resolve_stack_base_tests.rs"]
mod resolve_stack_base_tests;

#[cfg(test)]
#[path = "tests/session_start_args_tests.rs"]
mod session_start_args_tests;

/// TASK-1-108: when the current process is the orchestrator-spawned phase-1
/// child (corroborated via AIDA_AUTO_COMPLETE + AIDA_AUTO_COMPLETE_TOKEN
/// against the live drain-state file), the parent has already bumped
/// status to InProgress before spawning us — so the InProgress-without-
/// lease state we'll see at preflight isn't "another worktree owns it",
/// it's "the orchestrator's about to spawn the implementer". Promote
/// force_claim to true in that case so preflight doesn't bounce work the
/// parent already authorized. Returns `force_claim` unchanged when not
/// orchestrator-corroborated (interactive runs keep the existing safety
/// gate). trace:TASK-1-108 | ai:claude
pub(crate) fn effective_force_claim_for_session_start(
    explicit_force_claim: bool,
    orchestrator_corroborated: bool,
) -> bool {
    explicit_force_claim || orchestrator_corroborated
}

/// BUG-379: pre-flight spec-status gate tests. trace:BUG-379 | ai:claude
#[cfg(test)]
#[path = "tests/preflight_spec_status_tests.rs"]
mod preflight_spec_status_tests;

/// Walk a candidate list of branch names and return the first that's free
/// locally and on origin. STORY-65: auto for slug → slug-2..-10 →
/// slug-YYYY-MM-DD → slug-YYYY-MM-DD-2..-10; date for slug-YYYY-MM-DD
/// (with -N suffix on collision). Bails if 21 candidates all collide —
/// that's a strong signal the user wants an explicit --branch.
/// trace:STORY-65 | ai:claude
pub(crate) fn resolve_session_branch(
    project_root: &std::path::Path,
    slug: &str,
    branch_style: &str,
) -> Result<String> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let dated = format!("{}-{}", slug, today);
    let candidates: Vec<String> = match branch_style {
        "auto" => {
            let mut v = vec![slug.to_string()];
            for n in 2..=10 {
                v.push(format!("{}-{}", slug, n));
            }
            v.push(dated.clone());
            for n in 2..=10 {
                v.push(format!("{}-{}", dated, n));
            }
            v
        }
        "date" => {
            let mut v = vec![dated.clone()];
            for n in 2..=10 {
                v.push(format!("{}-{}", dated, n));
            }
            v
        }
        other => anyhow::bail!(
            "unknown --branch-style `{}` (expected `auto` or `date`)",
            other
        ),
    };
    for cand in &candidates {
        if !branch_exists_anywhere(project_root, cand) {
            return Ok(cand.clone());
        }
    }
    anyhow::bail!(
        "all {} candidate branch names are taken (slug `{}`); pass --branch explicitly",
        candidates.len(),
        slug
    )
}

/// BUG-1628: the default worktree path of an ordinary pickup — `<repo>-<slug>`,
/// placed by the same rule the warm pool uses. `session_start`, the `queue work
/// --dry-run` preview, and the auto-complete phase-1 fallback all derive the
/// path here, so they cannot drift apart again.
///
/// TASK-1561: the NAME is local; the PLACEMENT is not. This used to hardcode
/// `project_root.parent()`, so `[worktree_pool] worktree_parent` was honoured by
/// every pooled tree and by nothing else. That made the warm-pool fallback — the
/// branch taken whenever the pool cannot serve a tree — scatter worktrees back
/// into the project root's parent, silently, with only a warning about the pool
/// and nothing about the layout. Since BUG-1700 exists so one folder-trust grant
/// on one directory covers every worktree AIDA mints, a fallback that ignores
/// the key defeats the guarantee exactly when it matters: the operator is handed
/// a fresh untrusted path with its own modal, and the only alternative is to
/// trust the shared parent and every unrelated project under it.
///
/// Placement therefore goes through `worktree_pool::worktree_placement_path`,
/// the one rule, rather than a second copy of its sibling half. With the key
/// unset the result is byte-identical to the historical `<parent>/<repo>-<slug>`.
// trace:BUG-1628 | ai:claude
// trace:TASK-1561 | ai:claude
pub(crate) fn pickup_worktree_path(
    project_root: &std::path::Path,
    slug: &str,
) -> Result<std::path::PathBuf> {
    let repo_name = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let name = format!("{}-{}", repo_name, slug);
    let parent_dir = worktree_pool_config_worktree_parent(project_root);
    // The historical error when the root is a filesystem root is preserved for
    // the UNCONFIGURED case only: with a configured parent the root's own parent
    // is never consulted, so there is nothing to fail on.
    if parent_dir.is_none() && project_root.parent().is_none() {
        anyhow::bail!("project root has no parent");
    }
    Ok(aida_core::worktree_pool::worktree_placement_path(
        project_root,
        &name,
        parent_dir.as_deref(),
    ))
}

/// BUG-1628: the branch + worktree an ordinary pickup (`aida queue work
/// <spec>` with no `--branch`/`--path`) would create for `scope`. This is the
/// single resolver; the auto-complete phase-1 fallback calls it instead of the
/// epic-worktree defaults (`aida-<slug>` / `<id>-work`), which produced paths
/// outside the project naming convention for IDs such as `SPEC-016`.
// trace:BUG-1628 | ai:claude
pub(crate) fn resolve_pickup_workspace(
    project_root: &std::path::Path,
    scope: &str,
    branch_style: &str,
) -> Result<(std::path::PathBuf, String)> {
    let slug = slugify(scope);
    if slug.is_empty() {
        anyhow::bail!(
            "scope `{}` slugifies to empty — pick something with letters/digits",
            scope
        );
    }
    let branch = resolve_session_branch(project_root, &slug, branch_style)?;
    let path = pickup_worktree_path(project_root, &slug)?;
    Ok((path, branch))
}

/// Best-effort PID of the shell that invoked `aida session start`. The
/// `aida` shell wrapper is a function (not a forked process) so the
/// binary's parent IS the user's interactive shell. Goes through sysinfo
/// — already on for STORY-69 — so we don't add a libc dep just for
/// `getppid`. Returns None if sysinfo can't see this process at all (a
/// platform that didn't make it into the probe). trace:STORY-73 | ai:claude
pub(crate) fn creator_shell_pid() -> Option<u32> {
    let mut sys = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
    );
    sys.refresh_processes_specifics(sysinfo::ProcessRefreshKind::new());
    let me = sysinfo::Pid::from_u32(std::process::id());
    sys.process(me)?.parent().map(|p| p.as_u32())
}

/// BUG-379: decision returned by the `aida session start` pre-flight
/// spec-status gate. Pure-data so the gate is unit-testable without
/// spinning up a worktree / store fixture. trace:BUG-379 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreflightDecision {
    /// Status is Approved — proceed and bump to InProgress after lease save.
    AllowAndBump,
    /// Status is fine as-is (Planned, or scope is not a SPEC-ID) — proceed.
    Allow,
    /// Status is ambiguous but `--force-claim` was passed — proceed; show
    /// the message to the user as a warning.
    AllowWithWarning(String),
    /// Refuse with the given message.
    Refuse(String),
}

/// BUG-379: decide whether `aida session start --owns <scope>` is allowed
/// given the spec's current status. Pure function — takes the looked-up
/// status (None when the scope isn't a SPEC-ID) and the `--force-claim`
/// flag, returns the decision. trace:BUG-379 | ai:claude
pub(crate) fn preflight_spec_status(
    owns: &str,
    status: Option<&RequirementStatus>,
    force_claim: bool,
) -> PreflightDecision {
    let Some(status) = status else {
        // Scope isn't a SPEC-ID (e.g. EPIC-name, path glob, freeform tag) —
        // status gate doesn't apply.
        return PreflightDecision::Allow;
    };
    match status {
        // STORY-729 (FIX 8c): name the reopen command — the old text said
        // "reopen the spec first" but never told the user *how*, unlike its
        // Draft/InProgress siblings which name the command. trace:STORY-729
        RequirementStatus::Done | RequirementStatus::Completed => {
            PreflightDecision::Refuse(format!(
                "spec `{}` is {:?} — work already shipped, refusing to start a new session. \
                 Use a different scope, or reopen the spec first \
                 (`aida edit {} --status approved --force`).",
                owns, status, owns
            ))
        }
        // STORY-729 (FIX 8b): name the reopen command — the old text named no
        // reopen/override path. trace:STORY-729
        RequirementStatus::Rejected => PreflightDecision::Refuse(format!(
            "spec `{}` is Rejected — refusing to start a session against work that's been dropped. \
             Reopen it first (`aida edit {} --status approved --force`) if you mean to revive it.",
            owns, owns
        )),
        // trace:TASK-1176 | ai:claude — superseded work was adopted then
        // replaced; the session belongs on the successor, not this record.
        RequirementStatus::Superseded => PreflightDecision::Refuse(format!(
            "spec `{}` is Superseded — a later spec replaced it. Start the session against \
             the successor instead (`aida show {}` prints the superseded-by link).",
            owns, owns
        )),
        RequirementStatus::Draft => PreflightDecision::Refuse(format!(
            "spec `{}` is Draft — not ready for implementation. \
             Transition it to Approved first (`aida edit {} --status approved`).",
            owns, owns
        )),
        // trace:TASK-1-108 | ai:claude — message refined to acknowledge
        // the more common cause: stuck state from a prior orchestrator
        // run that didn't clean up the InProgress status. "Another
        // worktree" is possible but rare; the substrate has no way to
        // tell which case it is, so cover both honestly.
        RequirementStatus::InProgress if !force_claim => PreflightDecision::Refuse(format!(
            "spec `{}` is InProgress but no local lease holds it. Two common causes: \
             (a) a prior `aida queue work` or `--auto-complete` run left the status \
             stuck (the parent died after the Approved → InProgress bump without ever \
             saving a lease), or (b) another worktree / machine genuinely owns the work. \
             Recovery: `aida queue work {} --force-claim` to take over here, or \
             `aida edit {} --status approved` to reset the substrate first if you know \
             no other worktree owns it.",
            owns, owns, owns
        )),
        RequirementStatus::NeedsAttention if !force_claim => PreflightDecision::Refuse(format!(
            "spec `{}` is in NeedsAttention — punted by an autonomous agent for advisor \
             triage. Triage it first (`aida findings list`), or re-run with --force-claim \
             to claim anyway.",
            owns
        )),
        // Message stays mechanism-neutral: this branch fires for both
        // explicit --force-claim AND orchestrator-corroborated auto-claim
        // (via effective_force_claim_for_session_start). Saying
        // "--force-claim" specifically would be wrong when the user
        // didn't pass that flag (orchestrator-spawned child).
        // trace:TASK-1-108 | ai:claude
        RequirementStatus::InProgress | RequirementStatus::NeedsAttention => {
            PreflightDecision::AllowWithWarning(format!(
                "spec `{}` is {:?} — claiming it anyway",
                owns, status
            ))
        }
        RequirementStatus::Approved => PreflightDecision::AllowAndBump,
        RequirementStatus::Planned => PreflightDecision::Allow,
    }
}

/// BUG-436: review-aware wrapper around [`preflight_spec_status`]. A *review*
/// session (the orchestrator's phase-3 reviewer, or a human running
/// `aida queue work PR-N`) reviews a PR — it does not implement or own the
/// spec — so the BUG-379 "work already shipped, refusing to start a session"
/// guard must not block it. `Done` is the **normal** pre-review state (work on a
/// branch, PR open, awaiting review); `Completed` (merged) is harmless to open
/// read-only. For a non-review (implementer) session this is exactly
/// [`preflight_spec_status`], so the re-implement guard is fully preserved.
/// trace:BUG-436 | ai:claude
pub(crate) fn preflight_spec_status_review_aware(
    owns: &str,
    status: Option<&RequirementStatus>,
    force_claim: bool,
    is_review: bool,
) -> PreflightDecision {
    if is_review
        && matches!(
            status,
            Some(RequirementStatus::Done | RequirementStatus::Completed)
        )
    {
        return PreflightDecision::Allow;
    }
    preflight_spec_status(owns, status, force_claim)
}

// BUG-882: reviewer-scoped queue work can reach `session_start` with `owns`
// still set to the backing spec id rather than `PR-N`. Role intent must still
// classify the launch as review-shaped so Done-backed open PRs are reviewable.
// trace:BUG-882 | ai:codex
pub(crate) fn session_start_is_review_session(
    review_target: Option<(ReviewForge, u64)>,
    launch_role: Option<&str>,
) -> bool {
    review_target.is_some()
        || launch_role.is_some_and(|role| role.eq_ignore_ascii_case("reviewer"))
        || std::env::var("AIDA_REVIEW_VERDICT_FILE")
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
}

/// TASK-619: outcome of the pull-then-re-check guard that runs immediately
/// before `aida queue work` claims a spec. Session leases live under
/// `.aida/sessions/` and are machine-local (gitignored), so they cannot stop
/// a *second machine* from grabbing the same spec — the only shared "someone
/// is on this" signal is the git-canonical status flip, which is
/// eventually-consistent. `handle_queue_work` already pulls the orphan store
/// before claiming; this guard re-reads the spec's status from that freshly
/// pulled view and refuses if the spec was grabbed (or shipped) elsewhere in
/// the window between when we planned the pickup and now. trace:TASK-619 |
/// ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DupPickupDecision {
    /// Nothing claimed it under us — let the existing preflight take over.
    Proceed,
    /// The status changed under us to a "claimed/shipped elsewhere" state —
    /// refuse with the given operator-facing message.
    Refuse(String),
}

/// TASK-619: pure decision for the cross-machine duplicate-pickup guard.
///
/// Fires only when the spec was *pickable* at plan time (i.e. not already
/// owned by us / mid-flight) but the post-pull status says it is now claimed
/// or shipped somewhere else. That transition-under-us is the cross-machine
/// dup-pickup signal. Resuming a spec that was *already* InProgress at plan
/// time is a legitimate flow (the operator/orchestrator knows the state and
/// `--force-claim`/`--resume` cover it), so a stable InProgress → InProgress
/// is left to the existing `preflight_spec_status` gate, not refused here.
///
/// Bypassed entirely when `--force-claim` is set, when the orchestrator
/// corroborated this child (the parent already flipped status before spawning
/// us), or for review sessions (reviewing a Done/Completed PR is the normal
/// pre-review state). trace:TASK-619 | ai:claude
pub(crate) fn dup_pickup_recheck(
    spec_id: &str,
    status_at_plan: Option<&RequirementStatus>,
    status_after_pull: Option<&RequirementStatus>,
    force_claim: bool,
    orchestrator_corroborated: bool,
    is_review: bool,
) -> DupPickupDecision {
    if force_claim || orchestrator_corroborated || is_review {
        return DupPickupDecision::Proceed;
    }
    // Only act on real specs we can compare.
    let (Some(before), Some(after)) = (status_at_plan, status_after_pull) else {
        return DupPickupDecision::Proceed;
    };

    // "Pickable at plan time" = a state where starting fresh implementer work
    // is the normal intent. If the spec was already InProgress/Done/etc. when
    // we planned, the existing preflight (and --force-claim/--resume) owns that
    // case — don't double-refuse here.
    let was_pickable = matches!(
        before,
        RequirementStatus::Approved | RequirementStatus::Planned | RequirementStatus::Draft
    );
    if !was_pickable {
        return DupPickupDecision::Proceed;
    }

    // A "claimed or shipped elsewhere" post-pull state means another machine
    // flipped the git-canonical status in the window we were planning.
    let claimed_elsewhere = matches!(
        after,
        RequirementStatus::InProgress
            | RequirementStatus::Done
            | RequirementStatus::Completed
            | RequirementStatus::NeedsAttention
    );
    if !claimed_elsewhere {
        return DupPickupDecision::Proceed;
    }

    DupPickupDecision::Refuse(format!(
        "spec `{spec_id}` was {before} when this pickup was planned but is now {after} \
         after pulling the latest store — another machine or session most likely claimed \
         it in between (session leases are machine-local, so the git-canonical status flip \
         is the only cross-machine signal). Refusing to double-claim. \
         Re-run `aida queue work` to pick the next free item, or pass `--force-claim` to \
         take it over deliberately."
    ))
}

pub(crate) fn session_start_status_bump_preconditions_met(
    preflight_status: Option<&RequirementStatus>,
    worktree_path: &std::path::Path,
    lease_file: &std::path::Path,
) -> bool {
    matches!(preflight_status, Some(RequirementStatus::Approved))
        && worktree_path.exists()
        && lease_file.exists()
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
/// Resolve whether a session should pool its worktree: the explicit
/// `--pool`/`--no-pool` flag wins; otherwise `[worktree_pool] enabled` in the
/// repo config decides. Default is now **ON** (TASK-985): the build-delta
/// measurement (docs/research/2026-06-29-warm-pool-build-delta.md) showed ~30×
/// faster per-spec builds on fan-out, so pooling is the default and `--no-pool`
/// / `[worktree_pool] enabled = false` are the escape hatches.
// trace:STORY-714 trace:TASK-985 | ai:claude
pub(crate) fn resolve_worktree_pool_enabled(
    flag: Option<bool>,
    project_root: &std::path::Path,
) -> bool {
    if let Some(f) = flag {
        return f;
    }
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|b| toml::from_str::<toml::Value>(&b).ok())
        .and_then(|v| v.get("worktree_pool")?.get("enabled")?.as_bool())
        .unwrap_or(true)
}

/// BUG-669: a recycled pool worktree may still carry the PRIOR occupant's
/// session lease — `return_to_pool` resets the *tree* but the session lease
/// (`.aida/sessions/<id>.toml`) keyed to that worktree path is left behind. Drop
/// every lease pointing at `worktree_path` so the reused tree never inherits the
/// previous session's spec identity (an ADR-7 session reading as the prior
/// occupant's TASK-0439 in `aida ps`). The new session writes its own fresh
/// lease — new spec, started_at, role, id — right after acquisition. Returns the
/// count removed. An idle pooled tree never has a LIVE session, so removing any
/// lease pointing at it is safe. Path comparison is canonicalized to match the
/// canonicalized `worktree_path` `list_leases` reads back.
// trace:BUG-669 | ai:claude
pub(crate) fn clear_worktree_session_leases(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
) -> usize {
    let target = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.to_path_buf());
    let mut removed = 0usize;
    for lease in list_leases(project_root) {
        let lease_wt = lease
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| lease.worktree_path.clone());
        if lease_wt == target && std::fs::remove_file(lease_path(project_root, &lease.id)).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Acquire a warm-pool worktree for a new session and create `branch_name` on
/// it. The tree is handed out at a detached furthest-ahead default HEAD, so the
/// branch forks from the right base; a durable lease (keyed on the branch) keeps
/// it reserved while the session is live even though this start process exits.
/// On branch-creation failure the tree is returned, not leaked.
// trace:STORY-714 | ai:claude
// trace:BUG-1561 | ai:claude
pub(crate) fn acquire_session_pool_worktree(
    project_root: &std::path::Path,
    branch_name: &str,
) -> Result<std::path::PathBuf> {
    let opts = aida_core::worktree_pool::AcquireOptions {
        lease_holder: Some(branch_name.to_string()),
        max_trees: worktree_pool_config_max_trees(project_root),
        lease_ttl_secs: Some(worktree_pool_config_lease_ttl_secs(project_root)),
        post_create_hooks: worktree_pool_global_hooks("post_create"),
        init_submodules: worktree_config_init_submodules(project_root),
        parent_dir: worktree_pool_config_worktree_parent(project_root),
    };
    let acquired = aida_core::worktree_pool::acquire(project_root, &opts)?;

    // BUG-669: reset the lease on reuse — drop any stale session lease the prior
    // occupant left pointing at this recycled tree, so `aida ps` attributes the
    // reused worktree to the NEW session's spec, not the inherited one.
    // trace:BUG-669 | ai:claude
    let cleared = clear_worktree_session_leases(project_root, &acquired);
    if cleared > 0 {
        eprintln!(
            "{} reset {} stale lease{} from the recycled worktree",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            cleared,
            if cleared == 1 { "" } else { "s" }
        );
    }

    let co = std::process::Command::new("git")
        .arg("-C")
        .arg(&acquired)
        .args(["checkout", "-b", branch_name])
        .output()?;
    use std::io::Write;
    let _ = std::io::stderr().write_all(&co.stdout);
    let _ = std::io::stderr().write_all(&co.stderr);
    if !co.status.success() {
        // Hand the tree back so a failed branch-create doesn't pin it.
        let _ = aida_core::worktree_pool::return_to_pool(project_root, &acquired);
        anyhow::bail!("failed to create branch `{branch_name}` on the pooled worktree");
    }
    eprintln!(
        "{} using warm-pool worktree {} (recycled — build cache kept)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        acquired.display().to_string().dimmed()
    );
    Ok(acquired)
}

/// What `session end`'s dirty-worktree gate should do.
// trace:BUG-652 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DirtyGateOutcome {
    /// Clean, or `--force` (discard) — continue without ceremony.
    Proceed,
    /// Dirty + `--return`ing a pool tree — patch-salvage, then continue (the
    /// return resets the tree, so refusing would only break reuse).
    Salvage,
    /// Dirty, not forced, not a pool return — refuse and ask for `--force`.
    Refuse,
}

/// Decide the dirty-gate outcome from the three facts. Pure so the BUG-652
/// reuse-vs-refuse contract is unit-testable without the session machinery.
// trace:BUG-652 | ai:claude
pub(crate) fn dirty_gate_outcome(
    dirty: bool,
    force: bool,
    returning_pool_tree: bool,
) -> DirtyGateOutcome {
    if !dirty || force {
        return DirtyGateOutcome::Proceed;
    }
    if returning_pool_tree {
        DirtyGateOutcome::Salvage
    } else {
        DirtyGateOutcome::Refuse
    }
}

#[cfg(test)]
#[path = "tests/dirty_gate_tests.rs"]
mod dirty_gate_tests;

#[cfg(test)]
#[path = "tests/worktree_pool_resolve_tests.rs"]
mod worktree_pool_resolve_tests;

pub(crate) fn session_start(
    owns: &str,
    branch: Option<&str>,
    base: Option<&str>,
    reuse_branch: bool,
    explicit_path: Option<&str>,
    forge_override: Option<&str>,
    branch_style: &str,
    launch_claude: bool,
    launch_title: Option<String>,
    launch_set_title: bool,
    launch_name: Option<String>,
    // STORY-495: `None` → faithful native launch (no `--permission-mode`).
    launch_permission_mode: Option<&str>,
    launch_contained: bool,
    launch_role: Option<String>,
    // BUG-379: claim a spec stuck in InProgress (no local lease) or
    // NeedsAttention. Done/Completed/Rejected/Draft still refuse.
    // trace:BUG-379 | ai:claude
    force_claim: bool,
    // STORY-714: pool the session's worktree? Some(true)=--pool,
    // Some(false)=--no-pool, None=resolve from [worktree_pool] enabled config.
    use_pool: Option<bool>,
) -> Result<()> {
    // BUG-75: derive paths from the MAIN worktree root, not from cwd's
    // git ancestor — when the user invokes `aida session start` from
    // inside a linked worktree, find_project_root() returns the linked
    // worktree's path and new sessions stack as nested-sibling-of-sibling
    // (~/ai/aida-pr-9-epic-20 instead of ~/ai/aida-epic-20).
    // trace:BUG-75 | ai:claude
    let project_root = find_main_worktree_root()?;
    // A dash-led branch or base would reach `git worktree add` / `git
    // rev-parse` as an option. trace:BUG-1622 | ai:claude
    for (flag, value) in [("--branch", branch), ("--base", base)] {
        if let Some(v) = value {
            git_arg_guard::reject_option_like(flag, v)?;
        }
    }
    // Stakeholder personas are conversations, not build seats. Launch them in
    // the current checkout with the shared persona envelope and never create a
    // spec worktree or lease. trace:TASK-1261 | ai:codex
    if launch_claude
        && launch_role
            .as_deref()
            .is_some_and(queue_cmd::role_is_stakeholder_only)
    {
        return session::new_session(
            launch_title,
            launch_permission_mode,
            launch_role,
            launch_name,
            launch_contained,
            launch_set_title,
        );
    }
    let invoking_root = find_project_root().unwrap_or_else(|_| project_root.clone());
    let invoked_from_linked_worktree = invoking_root
        .canonicalize()
        .ok()
        .zip(project_root.canonicalize().ok())
        .map(|(a, b)| a != b)
        .unwrap_or(false);
    let slug = slugify(owns);
    if slug.is_empty() {
        anyhow::bail!(
            "scope `{}` slugifies to empty — pick something with letters/digits",
            owns
        );
    }

    // STORY-61: review-mode dispatch. When the scope is `PR-N` / `MR-N`
    // and the forge resolves (via override or origin-URL detection), we
    // hand off to the review flow which fetches the PR head ref into a
    // local branch and creates the worktree on that existing branch
    // (rather than `git worktree add -b <new-branch>`).
    let review_target: Option<(ReviewForge, u64)> = parse_review_scope(owns).map(|(f, n)| {
        let resolved_forge = match forge_override.and_then(ReviewForge::parse) {
            Some(f) => f,
            None => detect_forge_from_origin(&project_root).unwrap_or(f),
        };
        (resolved_forge, n)
    });
    let is_review_session = session_start_is_review_session(review_target, launch_role.as_deref());

    // STORY-65: auto-branch with collision-aware naming. If --branch isn't
    // given (and we're not in review mode, which has its own deterministic
    // naming), walk a candidate list — slug, slug-2..-10, slug-YYYY-MM-DD,
    // slug-YYYY-MM-DD-2..-10 — and pick the first that doesn't exist
    // locally OR on origin. `--branch-style date` skips straight to the
    // dated form for callers that want every session traceable to a date.
    // trace:STORY-65 | ai:claude
    let branch_name = match (&review_target, branch) {
        (Some((forge, n)), None) => forge.local_branch_for(*n),
        (_, Some(b)) => b.to_string(),
        (None, None) => resolve_session_branch(&project_root, &slug, branch_style)?,
    };
    // STORY-714: `mut` because the warm-pool flow reassigns this to an
    // acquired pool tree (../aida-pool-<slug>-N) in the default creation path.
    // trace:BUG-1628 | ai:claude — the shared pickup resolver.
    let mut worktree_path = match explicit_path {
        Some(p) => std::path::PathBuf::from(p),
        None => pickup_worktree_path(&project_root, &slug)?,
    };
    let reuse_existing_worktree_path = explicit_path.is_some()
        && force_claim
        && worktree_path.exists()
        && aida_core::git_ops::is_git_repo(&worktree_path)
        && std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                crate::worktree::is_registered(&String::from_utf8_lossy(&o.stdout), &worktree_path)
            })
            .unwrap_or(false);
    if worktree_path.exists() && !reuse_existing_worktree_path {
        anyhow::bail!(
            "{} already exists — pick a different --path or remove it first",
            worktree_path.display()
        );
    }

    // BUG-61: refuse to recreate a worktree at a path that still has a
    // leaked claude process pinning the old (now-dangling) inode. Even
    // though `worktree_path.exists()` is false (the dir was unlinked),
    // a live claude with `cwd=<path> (deleted)` will silently misroute
    // hooks and writes against the new worktree we're about to create.
    // The user must `kill <pid>` first. trace:BUG-61 | ai:claude
    let dangling = probe_dangling_claudes_at_path(&worktree_path);
    if !dangling.is_empty() {
        let mut msg = format!(
            "refusing to start session at {} — {} leaked claude process(es) still hold the previous (deleted) inode:",
            worktree_path.display(),
            dangling.len()
        );
        for p in &dangling {
            msg.push_str(&format!(
                "\n  pid {}    `kill {}` to clean up",
                p.pid, p.pid
            ));
        }
        msg.push_str("\n\nThen retry `aida session start`.");
        anyhow::bail!(msg)
    }

    // BUG-1793: an operator-held spec must refuse before creating a lease or
    // acquiring cross-clone coordination state. trace:BUG-1793 | ai:codex
    defer_cmd::enforce_reasoned_operator_hold(&project_root, owns)?;

    // Check the lease dir exists; create if not.
    let leases = leases_dir(&project_root);
    std::fs::create_dir_all(&leases)?;

    // Don't double-claim the same scope from this project root.
    //
    // BUG-574: re-running `aida session start <scope>` when THIS clone already
    // holds a lease for that scope is benign idempotent re-entry — the agent /
    // loop / wrapper just wants the session it already has, not a second one.
    // Treating it as a hard error (exit 1) poisoned `session start`'s usage
    // telemetry (35% non-zero) and papercut every retry-on-resume path. A
    // read-of-existing-state that COMPLETES exits 0: we report the existing
    // session (id + worktree + how to enter/end) and emit the eval line so a
    // wrapped caller still gets `AIDA_SESSION_ID` set to the live session.
    // Genuine create failures (worktree add, fetch, bad arg) stay non-zero.
    // Mirrors the BUG-573 contract (a completing read-only op exits 0).
    // trace:BUG-574 | ai:claude
    let existing_leases = list_leases(&project_root);
    if let Some(existing) = existing_lease_for_scope(&existing_leases, owns) {
        {
            eprintln!(
                "{} scope `{}` is already owned by session {} ({}) — re-entering it.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                owns,
                &existing.id[..existing.id.len().min(8)],
                existing.worktree_path.display(),
            );
            eprintln!();
            eprintln!("Next:");
            eprintln!(
                "  {}",
                format!("cd {}", existing.worktree_path.display()).cyan()
            );
            eprintln!(
                "  {}",
                format!(
                    "aida session end {}    # when done",
                    &existing.id[..existing.id.len().min(8)]
                )
                .dimmed()
            );
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                // Wrapped in eval — point the caller's shell at the existing
                // session rather than failing the re-entry.
                // trace:TASK-1171 | ai:claude
                let _eval = crate::shell_eval::EvalBlock::open();
                // The id is read back from a lease file; quote it for the
                // eval. trace:BUG-1624 | ai:claude
                println!("export AIDA_SESSION_ID='{}'", sh_single_quote(&existing.id));
            }
            return Ok(());
        }
    }

    // STORY-637: cross-clone lease claim on the shared store. The intra-clone
    // check above only sees THIS clone's `.aida/sessions/`; this consults the
    // `coordination/leases/` registry on the `aida-store` branch so a second
    // clone is REFUSED a lease another clone already holds (MU-504). Best-
    // effort: no remote / unreachable store WARNs and proceeds local-only —
    // session start never becomes brittle on the network. A corroborated
    // orchestrator subprocess (our own clone already driving this spec) and
    // `--force-claim` both force-acquire. trace:STORY-637 | ai:claude
    {
        let store_root = project_root.join(".aida-store");
        let orchestrated = orchestrator::detect(&project_root).is_orchestrated();
        let claim_role = launch_role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .unwrap_or_default();
        match coordination::acquire_claim(
            &store_root,
            owns,
            &project_root,
            &claim_role,
            /* review_verb */ is_review_session,
            force_claim || orchestrated,
        ) {
            Ok(coordination::AcquireOutcome::Acquired) => {}
            Ok(coordination::AcquireOutcome::Reclaimed(reason)) => {
                eprintln!(
                    "  {} reclaiming a stale cross-clone lease on `{owns}` ({reason})",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt)
                );
            }
            Ok(coordination::AcquireOutcome::Unavailable(reason)) => {
                eprintln!(
                    "  {} cross-clone coordination unavailable: {reason}, proceeding",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            Err(e) => anyhow::bail!("{e}"),
        }
    }

    // BUG-379: pre-flight spec-status gate. When `owns` is a SPEC-ID,
    // refuse states that mean "not work for me" (Done/Completed/Rejected/
    // Draft) and require --force-claim for ambiguous ones (already
    // InProgress with no local lease, or NeedsAttention awaiting advisor
    // triage). The Approved → InProgress bump itself happens after lease
    // save below, atomic-enough with the lease creation.
    // trace:BUG-379 | ai:claude
    let preflight_store = Storage::new(project_root.join(".aida-store")).load().ok();
    // TASK-1468: a session never claims (and later bumps) a guessed spec — an
    // `owns` id naming more than one requirement refuses the start.
    // trace:TASK-1468 | ai:claude
    if let Some(store) = &preflight_store {
        if let Err(e) = store.get_requirement_unambiguous(owns) {
            anyhow::bail!("{e}");
        }
    }
    let preflight_status: Option<RequirementStatus> = preflight_store.and_then(|store| {
        store
            .get_requirement_by_spec_id(owns)
            .map(|r| r.status.clone())
    });
    // TASK-1-108: when we're a corroborated orchestrator subprocess (the
    // parent --auto-complete process already bumped status to InProgress
    // via prepare_auto_complete_phase1_status before spawning us, per
    // BUG-369), the InProgress-without-lease state means "the orchestrator
    // owns this spec and is about to spawn the implementer subprocess",
    // not "another worktree has it". Treat that case as --force-claim so
    // the preflight refusal at line 21991 doesn't bounce work the parent
    // already authorized. Surfaced 2026-05-28 by operator running
    // `aida queue work TASK-570 --zen --auto-complete` repeatedly and
    // hitting "spec is already In Progress" on every retry.
    // trace:TASK-1-108 | ai:claude
    let orchestrator_corroborated = orchestrator::detect(&project_root).is_orchestrated();
    let force_claim_effective =
        effective_force_claim_for_session_start(force_claim, orchestrator_corroborated);
    match preflight_spec_status_review_aware(
        owns,
        preflight_status.as_ref(),
        force_claim_effective,
        is_review_session,
    ) {
        PreflightDecision::Refuse(msg) => anyhow::bail!("{}", msg),
        PreflightDecision::AllowWithWarning(msg) => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                msg
            );
        }
        PreflightDecision::AllowAndBump | PreflightDecision::Allow => {}
    }

    // BUG-637: pre-pickup spec-CLAIM gate. The status preflight above catches
    // "InProgress with no local lease" and cross-machine status drift, but it
    // does NOT catch the same-clone duplicate-fanout that motivated this fix:
    // a SECOND `aida queue work <spec>` while a FIRST live session already holds
    // a spec-scoped lease for it (the BUG-634 incident — two agents implemented
    // the same spec concurrently). Refuse when a DIFFERENT, LIVE session already
    // claims `owns` so we don't duplicate in-flight work.
    //
    // Bypasses (all narrow, to avoid wrongly blocking legitimate work):
    //   - `--force-claim` / orchestrator-corroborated → operator/parent takeover;
    //   - review sessions → reviewing a PR is not a fresh implementer claim;
    //   - the caller's OWN lease (resume) → never self-blocks;
    //   - DEAD/stale claims → ignored (a crashed agent can't lock its spec).
    // trace:BUG-637 | ai:claude
    if !force_claim_effective && !is_review_session {
        let leases = list_leases(&project_root);
        if !leases.is_empty() {
            let self_lease = std::env::current_dir()
                .ok()
                .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd));
            let now = chrono::Utc::now();
            let live = process_probe::probe_live_claude_sessions();
            if let Some(claim) = live_spec_claim_by_other(
                &leases,
                self_lease.as_ref(),
                &[owns],
                lease_is_live(&live, now),
            ) {
                let claim_id_short: String = claim.id.chars().take(8).collect();
                let started = claim
                    .started_at
                    .with_timezone(&chrono::Local)
                    .format("%H:%M");
                anyhow::bail!(
                    "`{owns}` is already claimed by session {claim_id_short} (live, started \
                     {started}) — skipping to avoid duplicate work.\n  \
                     worktree: {}\n  \
                     If that session is wrong/abandoned, end it (`aida session end \
                     {claim_id_short}`) or take over with `--force-claim`.",
                    claim.worktree_path.display(),
                );
            }
        }
    }

    // STORY-71: PR metadata captured from `gh`/`glab` for review sessions.
    // Populated below and stitched into the lease so `aida session show`
    // can display head/base SHAs without a round-trip to the forge.
    // trace:STORY-71 | ai:claude
    let mut pr_metadata: PrMetadata = PrMetadata::default();

    // TASK-245: decide whether to fork a new branch or check out an
    // existing one. `--reuse-branch` is the explicit opt-in (the
    // fixup-on-an-existing-PR-branch flow); an explicitly-named
    // `--branch` that already exists is auto-reused so the cryptic
    // `git worktree add -b` collision error never surfaces. An
    // auto-derived branch name is never reused — resolve_session_branch
    // already dodged collisions. trace:TASK-245 | ai:claude
    let branch_preexists = branch_exists_anywhere(&project_root, &branch_name);
    let reuse_existing = should_reuse_branch(reuse_branch, branch.is_some(), branch_preexists);

    if reuse_existing_worktree_path {
        let current = current_branch_at(&worktree_path);
        if current.as_deref() != Some(branch_name.as_str()) {
            anyhow::bail!(
                "{} exists but is on branch `{}`; retry expected `{}`",
                worktree_path.display(),
                current.unwrap_or_else(|| "<detached>".to_string()),
                branch_name,
            );
        }
        aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(|| {
            format!("exclude AIDA runtime files in {}", worktree_path.display())
        })?;
        aida_core::git_ops::init_submodules_or_warn(
            &worktree_path,
            worktree_config_init_submodules(&project_root),
        )
        .with_context(|| format!("prepare submodules in worktree {}", worktree_path.display()))?;
    } else if let Some((forge, n)) = review_target {
        // TASK-76: pre-flight check — if PR-N's source branch is held by
        // another active lease, any subsequent `gh pr checkout` (manual
        // or future auto) will fail with `branch already used by worktree
        // at ...`. Detect now and offer end / force-end / proceed-anyway.
        // Gracefully skipped when gh/glab isn't installed.
        // trace:TASK-76 | ai:claude
        if let Some(source_branch) = query_pr_source_branch(forge, n, &project_root) {
            let conflicting: Vec<_> = list_leases(&project_root)
                .into_iter()
                .filter(|l| l.branch == source_branch)
                .collect();
            if !conflicting.is_empty() {
                let l = &conflicting[0];
                // BUG-1205: there is nobody to answer the prompt under a
                // headless drain (or any non-TTY caller). Decide from the
                // substrate: a lease whose owning process is gone is ended
                // automatically (the option a human would pick); a LIVE lease
                // refuses with a typed message instead of a silent cancel.
                // trace:BUG-1205 | ai:claude
                let headless = std::env::var_os("AIDA_HEADLESS").is_some()
                    || !std::io::IsTerminal::is_terminal(&std::io::stdin());
                let live_sessions = process_probe::probe_live_claude_sessions();
                let is_live = lease_is_live(&live_sessions, chrono::Utc::now())(l);
                let decision = lease_conflict_decision(headless, is_live);
                match decision {
                    LeaseConflictDecision::AutoEnd => {
                        eprintln!(
                            "{} PR-{}'s source branch `{}` is held by lease {} whose session has exited — ending it so the review can check out the branch",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            n,
                            source_branch,
                            l.id
                        );
                        let ended = std::process::Command::new(crate::aida_exe_path())
                            .current_dir(&project_root)
                            .args(["session", "end", &l.id, "--yes", "--skip-ci"])
                            .stdin(std::process::Stdio::null())
                            .status_retrying_etxtbsy()
                            .map(|st| st.success())
                            .unwrap_or(false);
                        if !ended {
                            anyhow::bail!(
                                "could not end dead lease {} holding `{}` — run `aida session end {}` to see why; add `--force` to discard uncommitted changes or `--include-in-use` to override a live process, then retry", // trace:BUG-1986 | ai:claude
                                l.id,
                                source_branch,
                                l.id
                            );
                        }
                    }
                    LeaseConflictDecision::Refuse => {
                        anyhow::bail!(
                            "PR-{}'s source branch `{}` is held by LIVE lease {} ({}, worktree {}) and there is no terminal to choose an action — end that session or wait for it, then retry",
                            n,
                            source_branch,
                            l.id,
                            l.role.as_deref().unwrap_or("(unset)"),
                            l.worktree_path.display()
                        );
                    }
                    LeaseConflictDecision::Prompt => {}
                }
                if decision != LeaseConflictDecision::AutoEnd {
                    eprintln!();
                    eprintln!(
                        "{} PR-{}'s source branch `{}` is held by lease {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        n,
                        source_branch.yellow(),
                        l.id.yellow()
                    );
                    eprintln!("    role:     {}", l.role.as_deref().unwrap_or("(unset)"));
                    eprintln!("    worktree: {}", l.worktree_path.display());
                    eprintln!();
                    eprintln!(
                        "    Any `gh pr checkout {}` from the new session will fail with \
                     `branch already used by worktree`.",
                        n
                    );
                    eprintln!();
                    eprintln!("    Options:");
                    eprintln!("      1. End that session: `aida session end {}`", l.id);
                    eprintln!(
                    "      2. Force-end (kills any live claude there): `aida session end {} --force --include-in-use`",
                    l.id
                );
                    eprintln!("      3. Proceed anyway (auto-checkout will need manual fixup)");
                    eprintln!("      4. Cancel — let me handle it manually (no changes made)");
                    eprintln!();
                    use std::io::Write;
                    eprint!("    Choice [1/2/3/4, Ctrl+C or empty to cancel]: ");
                    let _ = std::io::stderr().flush();
                    let mut ans = String::new();
                    // TASK-34: treat read_line errors (Ctrl+D / closed stdin) as
                    // a cancel rather than bubbling up an unrelated I/O error.
                    // trace:TASK-34 | ai:claude
                    let read = std::io::stdin().read_line(&mut ans);
                    let cancel_clean = || -> ! {
                        eprintln!();
                        eprintln!(
                            "{} cancelled — no changes made; re-run when ready.",
                            crate::glyph(crate::glyphs::Glyph::Check).dimmed()
                        );
                        std::process::exit(1);
                    };
                    if read.is_err() {
                        cancel_clean();
                    }
                    match ans.trim() {
                        "1" => {
                            anyhow::bail!(
                            "stopping so you can `aida session end {}`; re-run this command after",
                            l.id
                        );
                        }
                        "2" => {
                            anyhow::bail!(
                            "stopping; run `aida session end {} --force --include-in-use` then re-run this command",
                            l.id
                        );
                        }
                        "3" => {
                            eprintln!(
                            "{} proceeding — `gh pr checkout {}` from the new session will need manual fixup",
                            "→".dimmed(),
                            n
                        );
                        }
                        // TASK-34: cancel path — accepts the explicit `4`, plus
                        // empty input / q / x as common "get me out of here"
                        // signals. Spec calls for exit 1 with no state changes.
                        // trace:TASK-34 | ai:claude
                        "4" | "" | "q" | "Q" | "x" | "X" | "cancel" | "Cancel" => {
                            cancel_clean();
                        }
                        other => {
                            anyhow::bail!("unrecognized choice {:?} — aborting", other);
                        }
                    }
                }
            }
        }

        // STORY-61 review flow:
        //   1. fetch the PR/MR head ref from origin into the local branch
        //      <forge>-<N>; safe to re-run (creates or fast-forwards).
        //   2. `git worktree add <path> <local-branch>` checks out the
        //      existing branch — does NOT create a new one. Reviewer can
        //      then commit feedback / try fixes locally without disturbing
        //      the contributor's branch on origin.
        let head_ref = forge.pr_head_ref(n);
        let refspec = format!("+{}:{}", head_ref, branch_name);
        let fetch = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["fetch", "origin", refspec.as_str()])
            .output()?;
        use std::io::Write;
        let _ = std::io::stderr().write_all(&fetch.stdout);
        let _ = std::io::stderr().write_all(&fetch.stderr);
        if !fetch.status.success() {
            anyhow::bail!(
                "`git fetch origin {}` failed — is {} #{} valid? \
                 (For self-hosted forges, override with --forge github|gitlab.)",
                refspec,
                match forge {
                    ReviewForge::GitHub => "PR",
                    ReviewForge::GitLab => "MR",
                },
                n
            );
        }
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            // `--` ends options: the path and branch that follow are
            // positional even when dash-led. trace:BUG-1622 | ai:claude
            .args([
                "worktree",
                "add",
                "--",
                worktree_path.to_str().unwrap(),
                branch_name.as_str(),
            ])
            .output()?;
        let _ = std::io::stderr().write_all(&res.stdout);
        let _ = std::io::stderr().write_all(&res.stderr);
        if !res.status.success() {
            anyhow::bail!("`git worktree add` failed");
        }
        aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(|| {
            format!("exclude AIDA runtime files in {}", worktree_path.display())
        })?;
        aida_core::git_ops::warn_worktree_container_gitdir(&project_root, &worktree_path);
        aida_core::git_ops::init_submodules_or_warn(
            &worktree_path,
            worktree_config_init_submodules(&project_root),
        )
        .with_context(|| format!("prepare submodules in worktree {}", worktree_path.display()))?;

        // STORY-71: enrich the lease with PR metadata via gh/glab. The
        // worktree is already on the PR's code (the fetch above did the
        // real work) — this pass is just for the head/base SHAs the
        // reviewer wants to see in `aida session show`. CLI-not-installed
        // is a soft failure: the session start succeeds and we print one
        // stderr line so the user knows what they're missing.
        // trace:STORY-71 | ai:claude
        match query_pr_metadata(forge, n, &project_root) {
            Ok(meta) => pr_metadata = meta,
            Err(reason) => {
                // TASK-50: outer template stays terse. The reason
                // string already contains the install URL when
                // relevant (`not on PATH` path); appending it again
                // here was producing duplicated content.
                // trace:TASK-50 | ai:claude
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    format!("skipped PR metadata capture: {}", reason).dimmed()
                );
            }
        }
    } else if reuse_existing {
        // TASK-245: check out an EXISTING branch instead of forking.
        // trace:TASK-245 | ai:claude
        if reuse_branch && !branch_preexists {
            anyhow::bail!(
                "--reuse-branch given but branch `{}` exists neither locally \
                 nor on origin — drop --reuse-branch to fork a fresh branch, \
                 or check the --branch name",
                branch_name
            );
        }
        if base.is_some() {
            eprintln!(
                "{} --base is ignored when checking out the existing branch `{}`",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                branch_name
            );
        }
        if !reuse_branch {
            // Auto-reuse path — make the non-forking behavior visible.
            eprintln!(
                "{} branch `{}` already exists — checking it out \
                 (pass a different --branch to fork a new one)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                branch_name
            );
        }
        // BUG-1270: keep the next round on the branch the PR actually tracks.
        align_reused_pr_branch(&project_root, &branch_name)?;
        // `git worktree add <path> <branch>` checks out an existing
        // local branch, or DWIM-creates a local tracking branch from a
        // unique `origin/<branch>` — no `-b`, no base.
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            // `--` ends options: the path and branch that follow are
            // positional even when dash-led. trace:BUG-1622 | ai:claude
            .args([
                "worktree",
                "add",
                "--",
                worktree_path.to_str().unwrap(),
                branch_name.as_str(),
            ])
            .output()?;
        use std::io::Write;
        let _ = std::io::stderr().write_all(&res.stdout);
        let _ = std::io::stderr().write_all(&res.stderr);
        if !res.status.success() {
            anyhow::bail!(
                "`git worktree add` (reuse existing branch `{}`) failed — \
                 the branch may already be checked out in another worktree",
                branch_name
            );
        }
        aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(|| {
            format!("exclude AIDA runtime files in {}", worktree_path.display())
        })?;
        aida_core::git_ops::warn_worktree_container_gitdir(&project_root, &worktree_path);
        aida_core::git_ops::init_submodules_or_warn(
            &worktree_path,
            worktree_config_init_submodules(&project_root),
        )
        .with_context(|| format!("prepare submodules in worktree {}", worktree_path.display()))?;
    } else {
        // Default flow: create worktree on a NEW branch (the original
        // EPIC-20 behavior — work-in-progress sessions, not reviews).
        //
        // BUG-76: when --base isn't given, fork from the project's
        // default branch (origin/main or fallback). The git default
        // (HEAD) is wrong when the caller is inside a linked worktree on
        // a feature branch — the new session would inherit unmerged
        // commits from whatever branch happened to be checked out.
        // trace:BUG-76 | ai:claude
        let resolved_base: Option<String> = match base {
            Some(b) => Some(b.to_string()),
            None => detect_default_branch_ref(&project_root),
        };

        if base.is_none() {
            let cwd_branch = current_branch_at(&invoking_root);
            let default_short = resolved_base
                .as_deref()
                .and_then(|s| s.strip_prefix("origin/").or(Some(s)))
                .unwrap_or("main");
            let warn = match cwd_branch.as_deref() {
                Some(b) if b != default_short && b != "HEAD" => Some(b.to_string()),
                _ if invoked_from_linked_worktree => {
                    Some(cwd_branch.unwrap_or_else(|| "<detached>".into()))
                }
                _ => None,
            };
            if let Some(b) = warn {
                if let Some(rb) = &resolved_base {
                    eprintln!(
                        "{} cwd is on `{}`; forking new branch from `{}` (pass --base if you wanted this branch's HEAD)",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                        b,
                        rb
                    );
                }
            }
        }

        // STORY-714: warm-pool path. When pooling is enabled (config or
        // --pool) AND no explicit --base is given (the pool resets to the
        // furthest-ahead default, which is what an unqualified session wants),
        // acquire a recycled detached tree and create the session branch ON
        // it, instead of a fresh `git worktree add`. The directory's warm
        // `target/` cache is reused; the durable lease keeps the tree reserved
        // for this session even after this short-lived start process exits
        // (so a concurrent acquire can't grab the live session's tree).
        // trace:STORY-714 | ai:claude
        let pool_enabled = resolve_worktree_pool_enabled(use_pool, &project_root);
        let pooled = if pool_enabled && base.is_none() {
            match acquire_session_pool_worktree(&project_root, &branch_name) {
                Ok(acquired) => Some(acquired),
                Err(e) => {
                    eprintln!(
                        "{} warm-pool acquire failed ({e}); falling back to a fresh worktree",
                        "Warning:".yellow().bold()
                    );
                    None
                }
            }
        } else {
            if pool_enabled && base.is_some() {
                eprintln!(
                    "{} --base given — not pooling this session (the pool resets to the default branch)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
            }
            None
        };

        if let Some(acquired) = pooled {
            worktree_path = acquired;
            // BUG-916: pooled/session-start handoff is a worktree materialization
            // path too; keep it as build-ready as the fresh `git worktree add`
            // paths before reporting the session ready.
            aida_core::git_ops::init_submodules_or_warn(
                &worktree_path,
                worktree_config_init_submodules(&project_root),
            )
            .with_context(|| {
                format!("prepare submodules in worktree {}", worktree_path.display())
            })?;
        } else {
            // `--` ends options: the path and base that follow are
            // positional even when dash-led. trace:BUG-1622 | ai:claude
            let mut args = vec![
                "worktree",
                "add",
                "-b",
                branch_name.as_str(),
                "--",
                worktree_path.to_str().unwrap(),
            ];
            if let Some(rb) = resolved_base.as_deref() {
                args.push(rb);
            }
            // STORY-73: capture stdout and re-emit to stderr. `git worktree add`
            // prints things like "HEAD is now at <sha>" to stdout, which would
            // otherwise contaminate session_start's stdout (where the wrapper
            // expects only `export AIDA_SESSION_ID=...` for eval).
            // trace:STORY-73 | ai:claude
            let res = std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root)
                .args(&args)
                .output()?;
            use std::io::Write;
            let _ = std::io::stderr().write_all(&res.stdout);
            let _ = std::io::stderr().write_all(&res.stderr);
            if !res.status.success() {
                anyhow::bail!("`git worktree add` failed");
            }
            aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(
                || format!("exclude AIDA runtime files in {}", worktree_path.display()),
            )?;
            aida_core::git_ops::warn_worktree_container_gitdir(&project_root, &worktree_path);
            aida_core::git_ops::init_submodules_or_warn(
                &worktree_path,
                worktree_config_init_submodules(&project_root),
            )
            .with_context(|| {
                format!("prepare submodules in worktree {}", worktree_path.display())
            })?;
        }
    }

    // Make AIDA state from the parent visible inside the new worktree.
    // .aida-store/ is gitignored so a whole-directory symlink works.
    // .aida/ is partially tracked (config.toml, setup.sh, docker-compose,
    // etc. live in main's tree), so a whole-dir symlink would skip when
    // git checks out those tracked files. Instead, ensure .aida/ exists
    // and symlink only the gitignored runtime subdirs into it.
    // trace:BUG-52 trace:BUG-1644 | ai:claude
    #[cfg(unix)]
    link_worktree_runtime_state(&project_root, &worktree_path)?;

    // STORY-248: when an explicit `--base` was passed (queue work
    // --stack / --base, or session start --base), capture the
    // fork-point SHA so the auto-rebase cascade later runs
    // `git rebase --onto origin/main <sha> <branch>` instead of a
    // plain rebase that would replay the parent's pre-squash commits.
    // The lookup uses the base name as a rev (works for local branches
    // and origin/-prefixed refs alike); a failure to resolve is non-
    // fatal — we just skip recording the SHA, which means the cascade
    // will refuse to auto-rebase this entry (safer than guessing).
    // trace:STORY-248 | ai:claude
    let stack_parent_branch: Option<String> = base
        .filter(|_| review_target.is_none() && !reuse_existing)
        .map(|s| s.strip_prefix("origin/").unwrap_or(s).to_string());
    let stack_parent_sha: Option<String> = base
        .filter(|_| review_target.is_none() && !reuse_existing)
        .and_then(|b| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root)
                .args([
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    git_arg_guard::END_OF_OPTIONS,
                    b,
                ]) // trace:BUG-1622 | ai:claude
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if sha.is_empty() {
                None
            } else {
                Some(sha)
            }
        });

    // STORY-52: share parent's cargo target/ with the new worktree so the
    // first `cargo build` inside the session reuses the existing build
    // cache instead of rebuilding from scratch (~2min for aida-cli). We
    // detect a parent target/, write it into the lease, and drop a
    // `.aida/session-env.sh` shim the user sources after `cd`.
    // trace:STORY-52 | ai:claude
    // BUG-1783: append worktree-specific subdir so concurrent builds
    // across main and worktrees don't silently overwrite each other's
    // artifacts.
    // trace:BUG-1783 | ai:codex
    let cargo_target_dir = detect_cargo_target_dir(&project_root);
    let mut recorded_target_dir = cargo_target_dir.clone();
    if let Some(target) = &cargo_target_dir {
        let isolated_target = isolate_cargo_target_dir(target, &worktree_path);
        recorded_target_dir = Some(isolated_target.clone());
        write_session_env_file(&worktree_path, &isolated_target).with_context(|| {
            format!("writing session env shim under {}", worktree_path.display())
        })?;
    }

    // Compose lease.
    let id_long = uuid::Uuid::now_v7().to_string();
    let id = id_long.replace('-', "")[..12].to_string();
    // BUG-2010: inside an `aida agent new` session (AIDA_AGENT_NAME set),
    // attribute the lease to that agent's live process rather than a short-lived
    // transient CLI / shell PID. Drain phase children get their own lease PID
    // and skip agent resolution.
    // trace:BUG-2010 | ai:antigravity
    let is_drain_child = orchestrator::detect(&project_root).is_orchestrated();
    let session_agent = if is_drain_child {
        None
    } else {
        agent_registry::resolve_session_agent_for_lease(&project_root)
    };
    let owner = if let Some(ref agent) = session_agent {
        agent.name.clone().unwrap_or_else(|| agent.id.clone())
    } else {
        aida_core::git_ops::git_config_get("user.email")
            .ok()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "unknown".to_string())
    };
    // STORY-65 + TASK-67: role resolution by INTENT, not blind env
    // inheritance. Order:
    //   1. --role <name>             explicit override always wins
    //   2. scope-derived default     PR-N/MR-N → reviewer; else → implementer
    //   3. $AIDA_SESSION_ROLE        env fallback when no scope default fires
    //                                (which currently never happens — every
    //                                scope shape has a default — but kept as
    //                                a safety net for future scope kinds)
    //   4. None                      no role recorded
    //
    // The earlier (pre-TASK-67) behavior was step 3 only: a reviewer shell
    // starting an EPIC session recorded reviewer, which then misrouted the
    // queue/activity surfaces while real implementer work happened inside.
    // When the env and scope-default disagree we warn (don't fail) — the
    // user might genuinely want the env's role (e.g. an architect shell
    // starting an EPIC; the implementer default would be wrong for them).
    // trace:TASK-67 | ai:claude
    let env_role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let scope_default_role: Option<&'static str> = if review_target.is_some() {
        Some("reviewer")
    } else {
        Some("implementer")
    };
    let (inherited_role, role_origin) = if let Some(r) = launch_role.as_ref() {
        (Some(r.clone()), "--role flag")
    } else if let Some(d) = scope_default_role {
        // Warn when env-role disagrees with the scope default — the user
        // probably wanted the scope default but might have meant otherwise.
        if let Some(env) = env_role.as_ref() {
            if !env.eq_ignore_ascii_case(d) {
                eprintln!(
                    "{} active role {} doesn't match this scope's default {}.\n  \
                     Recording: {} (scope-derived). Pass --role {} to override.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    format!("({})", env).cyan(),
                    format!("({})", d).cyan(),
                    d.cyan().bold(),
                    env.cyan(),
                );
            }
        }
        (Some(d.to_string()), "scope-derived")
    } else if let Some(env) = env_role.clone() {
        (Some(env), "inherited")
    } else {
        (None, "")
    };

    // STORY-73: capture the parent shell PID so `aida session end` (without
    // an arg) can resolve the right lease from the shell that ran `start`,
    // even when cwd doesn't help. Best effort — if PPID isn't set / process
    // self-introspection fails, we leave the field None and fall back to
    // the cwd / single-active heuristics. trace:STORY-73 | ai:claude
    let (creator_pid, creator_pid_start_time, active_pid, active_pid_start_time) =
        if let Some(ref agent) = session_agent {
            let pid = agent.pid;
            let start = agent
                .pid_start
                .clone()
                .or_else(|| process_probe::process_start_identity(pid));
            (Some(pid), start.clone(), Some(pid), start)
        } else {
            let creator_pid = creator_shell_pid();
            let creator_pid_start_time =
                creator_pid.and_then(process_probe::process_start_identity);
            (creator_pid, creator_pid_start_time, None, None)
        };

    let lease = SessionLease {
        id: id.clone(),
        scope: owns.to_string(),
        slug: slug.clone(),
        owner,
        worktree_path: worktree_path
            .canonicalize()
            .unwrap_or_else(|_| worktree_path.clone()),
        branch: branch_name.clone(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        role: inherited_role.clone(),
        creator_pid,
        creator_pid_start_time,
        active_pid,
        active_pid_start_time,
        cargo_target_dir: recorded_target_dir,
        // STORY-58: record the parent project root so `aida session list`
        // run from inside the new worktree can also walk the parent's
        // Claude Code session storage and present a merged view.
        // trace:STORY-58 | ai:claude
        parent_project_root: Some(
            project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.clone()),
        ),
        // STORY-71: PR/MR head/base metadata when this is a review session.
        // trace:STORY-71 | ai:claude
        pr_head_sha: pr_metadata.head_sha.clone(),
        pr_base_sha: pr_metadata.base_sha.clone(),
        pr_base_ref: pr_metadata.base_ref.clone(),
        // BUG-237: copy the zen-intent token the `--zen` dispatch minted into
        // `AIDA_ZEN_TOKEN`. Present iff this `aida queue work` was genuinely
        // `--zen` (the `Default` / `--no-human` arms scrub it), so a leaked
        // `AIDA_ZEN=1` never lands a token in a non-zen session's lease.
        // trace:BUG-237 | ai:claude
        zen_intent_token: std::env::var(zen::ZEN_TOKEN_ENV)
            .ok()
            .filter(|t| !t.is_empty()),
        // TASK-358: a fresh session is never an escalation; the orchestrator
        // stamps this later via `mark_lease_escalated_to_human` if and when
        // its `--escalate-blocks` path parks the spec for a human.
        // trace:TASK-358 | ai:claude
        escalated_to_human: None,
        // STORY-248: stacked-branch lineage captured above when `base`
        // was passed. Default `None` for the BUG-76 "fork from
        // origin/main" path. trace:STORY-248 | ai:claude
        parent_branch: stack_parent_branch.clone(),
        parent_branch_sha: stack_parent_sha.clone(),
        review_verb: is_review_session,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: aida_core::driver_registry::driver_from_env(), // trace:STORY-1653 | ai:claude
    };
    let lease_file = lease_path(&project_root, &id);
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(&lease_file, toml::to_string_pretty(&lease)?)?;

    // BUG-2010: update the resolved agent's own registry entry with its new
    // current_spec and worktree path.
    // trace:BUG-2010 | ai:antigravity
    if let Some(ref agent) = session_agent {
        let _ = agent_registry::update_agent_current_spec(
            &project_root,
            &agent.id,
            owns,
            &worktree_path,
        );
    }

    // BUG-379: atomic-enough with the lease just persisted, bump
    // Approved → InProgress. Idempotent — if the spec already moved
    // (manual edit, doctor heal, another path) we leave the status alone.
    // Failure is logged, not fatal — the doctor's spec-status-drift
    // category is the backstop. trace:BUG-379 | ai:claude
    let can_bump_status = session_start_status_bump_preconditions_met(
        preflight_status.as_ref(),
        &worktree_path,
        &lease_file,
    );
    let bumped_to_in_progress = if can_bump_status {
        let storage = Storage::new(project_root.join(".aida-store"));
        match storage.load().and_then(|mut store| {
            let mut bumped = false;
            // trace:TASK-1468 | ai:claude
            if let Some(req) = store.get_requirement_unambiguous_mut(owns)? {
                // BUG-1637: automated, recorded under its own author.
                // trace:BUG-1637 | ai:claude
                bumped =
                    approved_to_in_progress_bump(req, aida_core::conflict::SESSION_START_AUTHOR);
            }
            if bumped {
                storage.save(&store)?;
            }
            Ok::<bool, anyhow::Error>(bumped)
        }) {
            Ok(b) => b,
            Err(e) => {
                eprintln!(
                    "{} couldn't bump spec status for `{}` (stays at Approved): {} — `aida doctor` will heal",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    owns,
                    e
                );
                false
            }
        }
    } else {
        false
    };

    // STORY-1480: the Approved → InProgress bump IS the interactive claim —
    // emit the same PhaseEntered the drain's implementer phase emits, with an
    // empty run_uuid (no orchestrator run), so an interactively-worked spec's
    // timeline gets a work-span start instead of reading unknown. Keyed to
    // the bump, so a rework re-entry (NeedsAttention → InProgress goes
    // through other doors) and a re-claim never double-emit.
    // trace:STORY-1480 | ai:claude
    if bumped_to_in_progress {
        events::emit_interactive_lifecycle(
            &project_root,
            &[owns.to_string()],
            &events::EventKind::PhaseEntered {
                idx: 1,
                // trace:BUG-1786 | ai:antigravity
                slug: "implementer".to_string(),
                vendor: None,
                seat: None,
                model: None,
                effort: None,
                attempt: 1,
            },
        );
    }

    // STORY-73: human output to stderr, eval-friendly export to stdout when
    // stdout is captured (i.e., the shell wrapper's `eval "$(...)"` is
    // running). Direct invocation (TTY stdout) prints only the human output
    // — no raw `export` lines bleeding into the terminal. The wrapper
    // captures stdout into eval; stderr passes through to the user; stdin
    // is unaffected so any prompts still work. trace:STORY-73 | ai:claude
    eprintln!(
        "{} session {} started",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        id.yellow()
    );
    eprintln!("  {}: {}", "scope".bold(), owns.cyan());
    eprintln!("  {}: {}", "branch".bold(), branch_name.cyan());
    eprintln!(
        "  {}: {}",
        "worktree".bold(),
        worktree_path.display().to_string().cyan()
    );
    if let Some(r) = &inherited_role {
        eprintln!(
            "  {}: {} {}",
            "role".bold(),
            r.cyan(),
            format!("({})", role_origin).dimmed()
        );
    }
    if bumped_to_in_progress {
        eprintln!(
            "  {}: {} → {}",
            "spec".bold(),
            "Approved".dimmed(),
            "In Progress".magenta()
        );
    }

    // TASK-53: pre-flight conflict detection. Look for OTHER active
    // leases on the same scope and surface their recent file
    // touches so the user notices likely-overlap before they start
    // working. Informational only — never blocks the session start.
    // trace:TASK-53 | ai:claude
    {
        let other_leases: Vec<SessionLease> = list_leases(&project_root)
            .into_iter()
            .filter(|l| l.id != id && l.scope.eq_ignore_ascii_case(owns))
            .collect();
        if !other_leases.is_empty() {
            eprintln!();
            eprintln!(
                "{} Concurrent session{} on {} detected:",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                if other_leases.len() == 1 { "" } else { "s" },
                owns.cyan()
            );
            for other in &other_leases {
                let files = recent_files_for_branch(
                    &project_root,
                    &other.branch,
                    /* since */ "14 days ago",
                    /* max */ 8,
                );
                let started_ago = humanize_relative(other.started_at);
                let short_id = &other.id[..other.id.len().min(8)];
                if files.is_empty() {
                    eprintln!(
                        "    {} ({}) — started {}; no recent commits on branch",
                        short_id.dimmed(),
                        other.branch.cyan(),
                        started_ago.dimmed()
                    );
                } else {
                    let preview: Vec<&str> = files.iter().take(5).map(|s| s.as_str()).collect();
                    let extra = if files.len() > 5 {
                        format!(" (+{} more)", files.len() - 5)
                    } else {
                        String::new()
                    };
                    eprintln!(
                        "    {} ({}) — started {}; recent files: {}{}",
                        short_id.dimmed(),
                        other.branch.cyan(),
                        started_ago.dimmed(),
                        preview.join(", "),
                        extra.dimmed()
                    );
                }
            }
            eprintln!(
                "  {}",
                "Informational — coordinate with the other session if you'll touch the same files."
                    .dimmed()
            );
        }
    }
    // STORY-71: surface captured PR head/base in the summary so the user
    // sees what the reviewer flow recorded (matches `aida session show`).
    // trace:STORY-71 | ai:claude
    if let Some(head) = pr_metadata.head_sha.as_deref() {
        let head_short = &head[..head.len().min(12)];
        let base_disp = match (
            pr_metadata.base_ref.as_deref(),
            pr_metadata.base_sha.as_deref(),
        ) {
            (Some(r), Some(b)) => format!("{} ({})", r, &b[..b.len().min(12)]),
            (Some(r), None) => r.to_string(),
            (None, Some(b)) => b[..b.len().min(12)].to_string(),
            (None, None) => "-".to_string(),
        };
        eprintln!("  {}: {}", "pr-head".bold(), head_short.cyan());
        eprintln!("  {}: {}", "pr-base".bold(), base_disp.cyan());
    }
    eprintln!(
        "  {}: {}",
        "lease".bold(),
        lease_file.display().to_string().dimmed()
    );

    if launch_claude {
        // STORY-54: --launch collapses "start → cd → session new" into one
        // command. We chdir into the new worktree (so claude inherits it
        // and the launch-log records the worktree path, not the parent),
        // then delegate to session::new_session for the title prompt,
        // launch-log append, and `exec claude --permission-mode <mode>`.
        // exec replaces this process; control doesn't return here on
        // success.
        // trace:STORY-54 | ai:claude
        //
        // TASK-63: before the exec, source `.aida/session-env.sh` into
        // this process's env so the launched claude inherits
        // CARGO_TARGET_DIR (and any future session-scoped exports).
        // Without this, --launch defeats STORY-52's target-sharing —
        // the user has no chance to run `source` between chdir and
        // exec. The manual non-launch flow is unchanged: the user
        // sources the shim themselves after `cd`.
        // trace:TASK-63 | ai:claude
        let env_shim = worktree_path.join(".aida").join("session-env.sh");
        let applied_vars: Vec<String> = match std::fs::read_to_string(&env_shim) {
            Ok(body) => apply_session_env_to_process(&body),
            Err(_) => Vec::new(),
        };
        if !applied_vars.is_empty() {
            eprintln!();
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                format!(
                    "sourced .aida/session-env.sh ({}) into launched claude's env",
                    applied_vars.join(", ")
                )
                .dimmed()
            );
        }
        eprintln!();
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::FlowActive)
                .green()
                .bold(),
            format!(
                "launching claude in {} ({})",
                worktree_path.display(),
                if launch_contained {
                    "contained sandbox".to_string()
                } else {
                    launch_permission_mode
                        .map(|m| format!("permission-mode {}", m))
                        .unwrap_or_else(|| "native permission posture".to_string())
                }
            )
            .cyan()
        );
        std::env::set_current_dir(&worktree_path)
            .with_context(|| format!("failed to chdir into {}", worktree_path.display()))?;
        // TASK-31: pass a display name to claude so the /resume picker and
        // terminal title can distinguish concurrent worktrees. Explicit
        // --name wins; otherwise derive from scope+branch+role.
        // trace:TASK-31 | ai:claude
        let role_for_name = launch_role
            .clone()
            .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "implementer".to_string());
        let derived_name = launch_name
            .filter(|s| !s.trim().is_empty())
            .or_else(|| session::derive_session_name(owns, &branch_name, &role_for_name));
        return session::new_session(
            launch_title,
            launch_permission_mode,
            launch_role,
            derived_name,
            launch_contained,
            launch_set_title,
        );
    }

    if std::env::var_os("AIDA_SUPPRESS_SESSION_NEXT").is_none() {
        eprintln!();
        eprintln!("Next:");
        eprintln!(
            "  {}",
            format!("claude    # then /aida-implement {}", owns).cyan()
        );
        eprintln!("  {}", format!("cd {}", worktree_path.display()).cyan());
        // Point at the filtered path, not a raw `source` of a worktree file a
        // branch can commit. trace:BUG-1624 | ai:claude
        if cargo_target_dir.is_some() {
            eprintln!(
                "  {}    {}",
                format!("aida worktree enter {owns}").cyan(),
                "# cd in + warm build cache (filtered session env)".dimmed()
            );
        }
        eprintln!();
        eprintln!("When finished:");
        eprintln!("  {}", format!("aida session end {}", &id[..8]).dimmed());
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // Wrapped in eval — emit the env modification.
        // trace:TASK-1171 | ai:claude
        let _eval = crate::shell_eval::EvalBlock::open();
        println!("export AIDA_SESSION_ID='{}'", id);
    }
    Ok(())
}

/// BUG-61: enumerate live `claude` processes whose cwd is at-or-under
/// `worktree`. Used by both `session end` (refuse to remove a worktree
/// with live claudes inside) and `session start` (refuse to recreate a
/// worktree path that has a leaked claude pinning its dangling inode).
/// Returns an empty vec when probing isn't possible — better to let the
/// session op proceed than to block on a probe failure.
/// trace:BUG-61 | ai:claude
pub(crate) fn probe_live_claudes_in_worktree(
    worktree: &std::path::Path,
) -> Vec<process_probe::LiveSession> {
    // BUG-734: a worktree-less lease has an empty `worktree_path`. An empty
    // path must never reach the cwd scan below — `Path::starts_with` on an
    // empty base matches EVERY path, so every live claude on the machine
    // would count as "inside" the blank worktree and block `session end`.
    // Nothing on disk ⇒ nothing can leak. trace:BUG-734 | ai:claude
    if worktree.as_os_str().is_empty() {
        return Vec::new();
    }
    let canon = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    process_probe::probe_live_claude_sessions()
        .into_iter()
        .filter(|s| !s.stale_cwd && (s.cwd == canon || s.cwd.starts_with(&canon)))
        .collect()
}

/// BUG-61: enumerate dangling-cwd claude processes that USED to be inside
/// `worktree` (cwd matches the path with `(deleted)` suffix). Used by
/// `session start` to refuse recreating a worktree path that has an
/// orphan claude attached — the new worktree's writes would be ignored
/// by the leaked process and any hook it fires would resolve against a
/// stale inode. trace:BUG-61 | ai:claude
pub(crate) fn probe_dangling_claudes_at_path(
    worktree: &std::path::Path,
) -> Vec<process_probe::LiveSession> {
    // BUG-734: same empty-path hazard as probe_live_claudes_in_worktree —
    // `starts_with` on an empty base matches everything. trace:BUG-734 | ai:claude
    if worktree.as_os_str().is_empty() {
        return Vec::new();
    }
    process_probe::probe_live_claude_sessions()
        .into_iter()
        .filter(|s| s.stale_cwd && (s.cwd == worktree || s.cwd.starts_with(worktree)))
        .collect()
}

#[cfg(test)]
#[path = "tests/bug_734_empty_worktree_probe_tests.rs"]
mod bug_734_empty_worktree_probe_tests;
