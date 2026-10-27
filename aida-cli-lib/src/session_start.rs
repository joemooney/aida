//! `aida session start` and its preflight, pool and dirty-tree gates (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;
use aida_core::git_ops::RemoteCredentialsExt as _;

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

/// Compare canonical requirement identity, so aliases work but an unrelated
/// existing requirement cannot authorize coexistence on this change.
// trace:TASK-1801 | ai:codex
pub(crate) fn review_anchor_matches_change(
    store: &aida_core::RequirementsStore,
    anchor: &str,
    title: &str,
    branch: &str,
) -> bool {
    let Some(anchor) = store.get_requirement_unambiguous(anchor).ok().flatten() else {
        return false;
    };
    crate::pr_ship::derive_squash_subject_spec_ids(title, branch, "")
        .iter()
        .any(|id| {
            store
                .get_requirement_unambiguous(id)
                .ok()
                .flatten()
                .is_some_and(|backing| backing.id == anchor.id)
        })
}

/// How a reviewer pickup meets the lease holding the PR's source branch. The
/// change title (a forge round trip) is fetched only once every other
/// coexistence condition holds; the branch is the source branch the caller
/// already resolved. Unavailable or uncrediting metadata withholds the
/// coexistence exception, so ordinary conflict handling applies.
// trace:TASK-1801 | ai:claude
pub(crate) fn review_pickup_conflict_decision(
    headless: bool,
    lease_live: bool,
    holder_role: Option<&str>,
    review_coexistence: Option<(&str, &str)>,
    store: Option<&aida_core::RequirementsStore>,
    source_branch: &str,
    change_title: impl FnOnce() -> anyhow::Result<String>,
) -> LeaseConflictDecision {
    let explicit_role = review_coexistence.map(|(role, _)| role);
    let decide = |backing| {
        review_source_branch_conflict_decision(
            headless,
            lease_live,
            holder_role,
            explicit_role,
            backing,
        )
    };
    let backing = review_coexistence
        .map(|(_, id)| id)
        .filter(|id| decide(Some(id)) == LeaseConflictDecision::Coexist)
        .and_then(|id| {
            let store = store?;
            let title = change_title().ok()?;
            review_anchor_matches_change(store, id, &title, source_branch).then_some(id)
        });
    decide(backing)
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
            // trace:BUG-2181 | ai:claude
            .remote_credentials(project_root, "origin")
            .output()?;
        if !fetch.status.success() {
            // TASK-1784: a branch that was never pushed has nothing on origin
            // to align with — the local branch IS the work (pure-git reports
            // it as the change). Only a confirmed absence (`ls-remote` exit 2)
            // proceeds; a network or auth failure still refuses.
            // trace:TASK-1784 | ai:claude
            if remote_branch_absent(project_root, branch) {
                eprintln!(
                    "  {} branch `{branch}` was never pushed — reusing the local branch as is",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt)
                );
                return Ok(());
            }
            anyhow::bail!(
                "could not refresh PR branch `{branch}` from origin before launch: {}",
                String::from_utf8_lossy(&fetch.stderr).trim()
            );
        }
    }
    align_reused_pr_branch_with_status(project_root, branch, has_open_pr)
}

/// TASK-1784: origin answered and has no `refs/heads/<branch>`.
/// `git ls-remote --exit-code` exits 2 exactly when no ref matched; any other
/// failure (unreachable remote, auth) is not proof of absence.
// trace:TASK-1784 | ai:claude
fn remote_branch_absent(project_root: &std::path::Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-remote", "--exit-code", "--heads", "origin"])
        .arg(format!("refs/heads/{branch}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status()
        .is_ok_and(|s| s.code() == Some(2))
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

/// BUG-2223: the orphaned lease an explicit `--path` + `--force-claim` start
/// adopts in place: the one lease that records exactly this tree, for this
/// scope and branch, whose owner is verifiably gone. A seat clone can be
/// re-entered ONLY this way, so for a clone every mismatch refuses: no lease
/// records it, the lease is not a clone lease or is for another scope or
/// branch, the clone has another branch checked out, or the lease is not
/// provably orphaned. For a linked worktree a lease that does not match is
/// left to the existing re-entry paths (`Ok(None)`).
// trace:BUG-2223 | ai:claude
pub(crate) fn orphan_lease_to_adopt(
    project_root: &std::path::Path,
    path: &std::path::Path,
    owns: &str,
    branch: &str,
    is_clone: bool,
) -> Result<Option<SessionLease>> {
    let canonical = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let tree = canonical(path);
    let recorded = list_leases(project_root)
        .into_iter()
        .find(|l| !l.worktree_path.as_os_str().is_empty() && canonical(&l.worktree_path) == tree);
    let refuse = |why: String| -> Result<Option<SessionLease>> {
        if is_clone {
            anyhow::bail!(
                "refusing to re-enter the contained seat's private clone {}: {why}. Nothing was \
                 changed in it.",
                path.display()
            );
        }
        Ok(None)
    };
    let Some(lease) = recorded else {
        return refuse(
            "no session lease records it, and only an orphaned session's clone can be \
             re-entered"
                .into(),
        );
    };
    let id = &lease.id[..lease.id.len().min(8)];
    if is_clone && lease.tree_kind != crate::seat_clone::TreeKind::Clone {
        return refuse(format!(
            "lease {id} records it as a worktree, not a seat clone"
        ));
    }
    if !lease.scope.eq_ignore_ascii_case(owns) {
        return refuse(format!(
            "it belongs to lease {id} for `{}`, not `{owns}`",
            lease.scope
        ));
    }
    if lease.branch != branch {
        return refuse(format!(
            "lease {id} records branch `{}`, not `{branch}`",
            lease.branch
        ));
    }
    if is_clone {
        let head = crate::seat_clone::checked_out_branch(path);
        if head.as_deref() != Some(branch) {
            return refuse(format!(
                "it has `{}` checked out, but lease {id} records `{branch}`",
                head.as_deref().unwrap_or("<detached or unreadable HEAD>")
            ));
        }
    }
    if let Some(why) = orphan_adoption_refusal(&stale_lease_recovery_for_lease(&lease)) {
        return refuse(format!("lease {id} is not orphaned: {why}"));
    }
    Ok(Some(lease))
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
        if lease_wt == target {
            let path = lease_path(project_root, &lease.id);
            // Revoke while the grant handle is still readable in the lease.
            // trace:TASK-1789 | ai:codex
            crate::session::revoke_lease_child_grant(&path);
            if std::fs::remove_file(path).is_ok() {
                removed += 1;
            }
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
    rollback: &mut Option<SessionStartCleanup>,
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
    // Record ownership before checkout: a failing post-checkout hook may
    // already have created the branch. trace:BUG-2188 | ai:codex
    if let Some(cleanup) = rollback.as_mut() {
        cleanup.own_tree(&acquired, None);
        if rollback_branch_absent(project_root, branch_name)? {
            cleanup.branch = Some(branch_name.into());
            cleanup.branch_sha = rollback_ref_sha(&acquired, "HEAD");
        }
    }

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
        if rollback.is_none() {
            let _ = aida_core::worktree_pool::return_to_pool(project_root, &acquired);
        }
        anyhow::bail!(
            "failed to create branch `{branch_name}` on pooled worktree {}: {}",
            acquired.display(),
            String::from_utf8_lossy(&co.stderr)
        );
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

/// An invocation-owned receipt: re-entry has no allocation to undo. The
/// launcher retains a fresh allocation until its child has actually started.
// trace:BUG-2188 | ai:codex
pub(crate) struct SessionStartReceipt {
    pub(crate) lease_id: Option<String>,
    cleanup: Option<SessionStartCleanup>,
}
impl SessionStartReceipt {
    fn existing(lease_id: Option<String>) -> Self {
        Self {
            lease_id,
            cleanup: None,
        }
    }
    pub(crate) fn owns_setup(&self) -> bool {
        self.cleanup.is_some()
    }
    pub(crate) fn disarm(&mut self) {
        if let Some(cleanup) = self.cleanup.as_mut() {
            cleanup.armed = false;
        }
        self.cleanup = None;
    }
}
// trace:BUG-2188 | ai:codex
pub(crate) struct SessionStartCleanup {
    root: std::path::PathBuf,
    scope: String,
    claim: Option<coordination::Claim>,
    tree: Option<std::path::PathBuf>,
    branch: Option<String>,
    branch_sha: Option<String>,
    gitdir: Option<std::path::PathBuf>,
    clone: Option<crate::seat_clone::CreatedClone>,
    clone_git_identity: Option<(u64, u64)>,
    lease_id: Option<String>,
    // The exact status/owner write this invocation owns, including owner-only changes.
    // trace:BUG-2188 TASK-2326 | ai:codex
    claim_write: Option<crate::claim::ClaimStatusChange>,
    armed: bool,
}
impl SessionStartCleanup {
    fn new(root: &std::path::Path, scope: &str) -> Self {
        Self {
            root: root.into(),
            scope: scope.into(),
            claim: None,
            tree: None,
            branch: None,
            branch_sha: None,
            gitdir: None,
            clone: None,
            clone_git_identity: None,
            lease_id: None,
            claim_write: None,
            armed: true,
        }
    }
    fn own_tree(&mut self, path: &std::path::Path, branch: Option<String>) {
        self.tree = Some(path.into());
        self.gitdir = worktree_pointer(path);
        self.branch_sha = branch
            .as_deref()
            .and_then(|b| rollback_ref_sha(&self.root, b));
        self.branch = branch;
    }
    /// Undo only this invocation's tree allocation (and the branch created on
    /// it), leaving claim and lease ownership in place, so the warm-pool path
    /// can fall back to a fresh worktree under the same receipt. False means
    /// the tree was refused (identity changed or in use) and was kept.
    // trace:BUG-2188 | ai:claude
    fn release_allocation(&mut self) -> bool {
        if !self.release_tree() {
            return false;
        }
        let _ = self.release_branch();
        self.tree = None;
        self.gitdir = None;
        self.branch = None;
        self.branch_sha = None;
        true
    }
    fn release_tree(&self) -> bool {
        let tree = self
            .tree
            .as_deref()
            .or_else(|| self.clone.as_ref().map(|c| c.path.as_path()));
        if let Some(path) = tree.filter(|p| std::fs::symlink_metadata(p).is_ok()) {
            if self.clone.is_some() && clone_git_identity(path) != self.clone_git_identity {
                eprintln!("launch cleanup refused changed private clone identity at {}; inspect `aida session leases --all`", path.display());
                return false;
            }
            if self.clone.is_none()
                && path.try_exists().unwrap_or(true)
                && (self.gitdir.is_none() || worktree_pointer(path) != self.gitdir)
            {
                eprintln!("launch cleanup refused changed worktree identity at {}; inspect `git -C '{}' worktree list --porcelain` and `aida session leases --all`", path.display(), self.root.display());
                return false;
            }
            if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(path) {
                eprintln!(
                    "launch cleanup kept {}: {}; inspect `aida session leases --all`",
                    path.display(),
                    in_use.summary()
                );
                return false;
            }
            let removed = if let Some(clone) = &self.clone {
                std::fs::remove_dir_all(&clone.path)
                    .map_err(anyhow::Error::from)
                    .and_then(|_| {
                        let out = std::process::Command::new("git")
                            .current_dir(&self.root)
                            .args(["update-ref", "-d", &clone.pin, &clone.base_sha])
                            .output()?;
                        anyhow::ensure!(
                            out.status.success(),
                            "private clone pin identity changed; inspect `git show-ref {}`",
                            clone.pin
                        );
                        Ok(())
                    })
            } else if aida_core::worktree_pool::is_pool_worktree(&self.root, path) {
                aida_core::worktree_pool::return_to_pool(&self.root, path)
            } else {
                std::process::Command::new("git")
                    .current_dir(&self.root)
                    .args(["worktree", "remove", "--force", "--"])
                    .arg(path)
                    .output()
                    .map_err(anyhow::Error::from)
                    .and_then(|out| {
                        if out.status.success() {
                            Ok(())
                        } else {
                            Err(anyhow::anyhow!("{}", String::from_utf8_lossy(&out.stderr)))
                        }
                    })
            };
            if let Err(error) = removed {
                eprintln!("launch cleanup could not remove {}: {error:#}; inspect `aida session leases --all`", path.display());
                return false;
            }
        }
        true
    }
    /// Undo the status AND owner write only while its exact values remain.
    // trace:BUG-2215 TASK-2326 | ai:codex
    fn revert_claim_write(&self) {
        let Some(change) = &self.claim_write else {
            return;
        };
        match crate::claim::rollback_claim_status(&self.root, change) {
            Ok(true) => {}
            Ok(false) => eprintln!(
                "launch cleanup kept {}: claim state changed after this launch",
                self.scope
            ),
            Err(error) => eprintln!(
                "launch cleanup could not restore {} claim state: {error:#}",
                self.scope
            ),
        }
    }
    /// Compare-and-delete the branch this invocation created. True when the
    /// branch is gone: deleted here, or never created (a setup that failed
    /// before Git made it is already clean, not "kept").
    // trace:BUG-2188 | ai:claude
    fn release_branch(&self) -> bool {
        let (Some(branch), Some(expected)) = (&self.branch, &self.branch_sha) else {
            return true;
        };
        if matches!(rollback_branch_absent(&self.root, branch), Ok(true)) {
            return true;
        }
        let reference = format!("refs/heads/{branch}");
        let out = std::process::Command::new("git")
            .current_dir(&self.root)
            .args(["update-ref", "-d", &reference, expected])
            .output();
        if matches!(out, Ok(ref out) if out.status.success()) {
            return true;
        }
        eprintln!(
            "launch cleanup kept branch {branch}; inspect `git branch --list` before retrying"
        );
        false
    }
}
impl Drop for SessionStartCleanup {
    fn drop(&mut self) {
        if !self.armed || !self.release_tree() {
            return;
        }
        if let Some(id) = &self.lease_id {
            if !remove_session_lease_record_only(&self.root, id) {
                eprintln!("launch cleanup could not remove lease {id}; inspect `aida session leases --all`");
                return;
            }
        }
        self.revert_claim_write();
        let _ = self.release_branch();
        if let Some(claim) = &self.claim {
            match coordination::release_claim_receipt(&self.root.join(".aida-store"), claim) {
                Ok(true) => {},
                Ok(false) => eprintln!("launch cleanup kept changed shared claim for {}; inspect `aida session leases --all`", self.scope),
                Err(error) => eprintln!("launch cleanup could not release shared claim for {}: {error:#}; inspect `aida session leases --all`", self.scope),
            }
        }
    }
}

#[cfg(test)]
mod bug2188_release_branch_tests {
    use super::*;

    // An absent branch is clean; a branch whose tip moved is kept.
    // trace:BUG-2188 | ai:claude
    #[test]
    fn release_branch_treats_an_absent_ref_as_clean_and_keeps_a_moved_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=f",
                "-c",
                "user.email=f@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "a",
            ],
        ] {
            assert!(crate::test_git::git_command()
                .current_dir(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let head = rollback_ref_sha(root, "HEAD").unwrap();
        let mut cleanup = SessionStartCleanup::new(root, "TASK-1");
        cleanup.armed = false;
        cleanup.branch = Some("never-created".into());
        cleanup.branch_sha = Some(head.clone());
        assert!(cleanup.release_branch(), "absent branch is already clean");

        assert!(crate::test_git::git_command()
            .current_dir(root)
            .args(["branch", "moved", "HEAD"])
            .status()
            .unwrap()
            .success());
        cleanup.branch = Some("moved".into());
        cleanup.branch_sha = Some("1".repeat(40));
        assert!(!cleanup.release_branch(), "changed identity is kept");
        assert!(!rollback_branch_absent(root, "moved").unwrap());

        cleanup.branch_sha = Some(head);
        assert!(cleanup.release_branch());
        assert!(rollback_branch_absent(root, "moved").unwrap());
    }
}

// trace:BUG-2188 | ai:codex
fn clone_git_identity(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path.join(".git")).ok()?;
    metadata.is_dir().then(|| (metadata.dev(), metadata.ino()))
}

// trace:BUG-2188 | ai:codex
fn rollback_branch_absent(root: &std::path::Path, branch: &str) -> Result<bool> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()?;
    match out.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => anyhow::bail!(
            "cannot establish branch pre-state for {branch}: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

// trace:BUG-2188 | ai:codex
fn rollback_ref_sha(root: &std::path::Path, reference: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// trace:BUG-2188 | ai:codex
fn worktree_pointer(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let marker = path.join(".git");
    if !std::fs::symlink_metadata(&marker)
        .ok()?
        .file_type()
        .is_file()
    {
        return None;
    }
    let text = std::fs::read_to_string(marker).ok()?;
    let gitdir = std::path::Path::new(text.trim().strip_prefix("gitdir: ")?);
    let resolved = if gitdir.is_absolute() {
        gitdir.to_path_buf()
    } else {
        path.join(gitdir)
    };
    resolved.canonicalize().ok()
}

/// Git can materialize a worktree even when post-checkout exits nonzero.
/// Allocate at an invocation-private path, recording absent pre-state and the
/// exact admin directory before Git runs. Publish to the requested absent path
/// only after success; never infer ownership from a failed add at a shared path.
// trace:BUG-2188 | ai:codex
fn transactional_worktree_add(
    root: &std::path::Path,
    args: &[&str],
    requested: &std::path::Path,
    branch: &str,
    rollback: &mut Option<SessionStartCleanup>,
) -> Result<std::process::Output> {
    let Some(cleanup) = rollback.as_mut() else {
        // trace:BUG-2211 | ai:claude
        return Ok(aida_core::git_ops::output_healing_config_lock(
            root,
            || {
                let mut cmd = std::process::Command::new("git");
                cmd.current_dir(root);
                cmd
            },
            args,
        )?);
    };
    if std::fs::symlink_metadata(requested).is_ok() {
        anyhow::bail!("worktree path {} already exists; kept it unchanged; inspect `git worktree list --porcelain`", requested.display());
    }
    let name = format!("aida-launch-{}", uuid::Uuid::new_v4());
    let staging = requested
        .parent()
        .context("worktree path has no parent")?
        .join(&name);
    let common = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()?;
    anyhow::ensure!(
        common.status.success(),
        "cannot resolve worktree administration directory"
    );
    let gitdir = std::path::PathBuf::from(String::from_utf8_lossy(&common.stdout).trim())
        .join("worktrees")
        .join(&name);
    anyhow::ensure!(
        std::fs::symlink_metadata(&staging).is_err() && std::fs::symlink_metadata(&gitdir).is_err(),
        "private worktree allocation already exists"
    );
    let path_index = args
        .iter()
        .position(|arg| std::path::Path::new(arg) == requested)
        .context("missing worktree path in Git arguments")?;
    let base = args.get(path_index + 1).copied().unwrap_or("HEAD");
    if rollback_branch_absent(root, branch)? {
        cleanup.branch = Some(branch.into());
        cleanup.branch_sha = rollback_ref_sha(root, base)
            .or_else(|| rollback_ref_sha(root, &format!("origin/{base}")));
        anyhow::ensure!(
            cleanup.branch_sha.is_some(),
            "cannot pin initial commit for branch {branch}"
        );
    }
    cleanup.tree = Some(staging.clone());
    cleanup.gitdir = Some(gitdir);
    let mut staged_args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    staged_args[path_index] = staging.to_string_lossy().into_owned();
    // trace:BUG-2211 | ai:claude
    let out = aida_core::git_ops::output_healing_config_lock(
        root,
        || {
            let mut cmd = std::process::Command::new("git");
            cmd.current_dir(root);
            cmd
        },
        &staged_args,
    )?;
    if !out.status.success() {
        return Ok(out);
    }
    anyhow::ensure!(worktree_pointer(&staging) == cleanup.gitdir, "worktree admin identity changed during allocation; inspect `git worktree list --porcelain`");
    let published = aida_core::git_ops::publish_worktree_noreplace(root, &staging, requested);
    // Atomic rename may succeed even if the subsequent admin repair fails.
    if worktree_pointer(requested) == cleanup.gitdir {
        cleanup.tree = Some(requested.into());
    }
    published?;
    anyhow::ensure!(
        worktree_pointer(requested) == cleanup.gitdir,
        "published worktree pointer differs from the allocation receipt"
    );
    Ok(out)
}

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
    // BUG-2010: true when caller is working the lease itself (self-claim).
    // false when launching/spawning another agent (agent new, session start --launch).
    self_claim: bool,
    // Only a caller with an explicit role flag and resolved backing spec may
    // request reviewer coexistence; a resolved/inferred role is insufficient.
    // trace:TASK-1786 | ai:codex
    review_coexistence: Option<(&str, &str)>,
    // BUG-2169: the seat this session is for runs contained, so under
    // `[contained] seat_tree = "clone"` it gets a private clone, not a worktree.
    contained_seat: bool,
) -> Result<()> {
    session_start_with_receipt(
        owns,
        branch,
        base,
        reuse_branch,
        explicit_path,
        forge_override,
        branch_style,
        launch_claude,
        launch_title,
        launch_set_title,
        launch_name,
        launch_permission_mode,
        launch_contained,
        launch_role,
        force_claim,
        use_pool,
        self_claim,
        review_coexistence,
        contained_seat,
        false,
        /* adopt_orphan */ false,
    )
    .map(|mut receipt| receipt.disarm())
}

// trace:BUG-2188 | ai:codex
// why: mirrors session_start so the launcher can retain this invocation's allocation receipt.
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_start_with_receipt(
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
    // BUG-2010: true when caller is working the lease itself (self-claim).
    // false when launching/spawning another agent (agent new, session start --launch).
    self_claim: bool,
    // Only a caller with an explicit role flag and resolved backing spec may
    // request reviewer coexistence; a resolved/inferred role is insufficient.
    // trace:TASK-1786 | ai:codex
    review_coexistence: Option<(&str, &str)>,
    // BUG-2169: the seat this session is for runs contained, so under
    // `[contained] seat_tree = "clone"` it gets a private clone, not a worktree.
    contained_seat: bool,
    rollback_failed_setup: bool,
    // BUG-2223: re-enter the orphaned lease `explicit_path` records, in place
    // (`aida queue work --resume`). Unlike `force_claim` it bypasses no live
    // claim: it only permits the path re-entry, and refuses when that path is
    // not an adoptable orphan's.
    adopt_orphan: bool,
) -> Result<SessionStartReceipt> {
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
        )
        .map(|_| SessionStartReceipt::existing(None));
    }
    // BUG-2169: decide the tree kind before any claim or lease is written, so
    // a malformed knob refuses with nothing to undo.
    // trace:BUG-2169 | ai:claude
    let tree_kind = crate::seat_clone::tree_kind_for_launch(&project_root, contained_seat)?;
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
    // BUG-2223: an orphan adoption may re-enter its recorded --path too.
    let path_reentry = force_claim || adopt_orphan;
    let reuse_existing_worktree_path = explicit_path.is_some()
        && path_reentry
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
    // BUG-2223: an explicit --path + --force-claim naming the tree of an
    // orphaned lease adopts it in place (`aida queue work --resume`). A seat
    // clone is re-entered only this way: `orphan_lease_to_adopt` refuses a
    // clone no orphaned lease of this scope and branch records.
    // trace:BUG-2223 | ai:claude
    let clone_reentry = explicit_path.is_some()
        && path_reentry
        && worktree_path.exists()
        && crate::seat_clone::seat_clone_root(&worktree_path)
            .is_some_and(|root| worktree_path.canonicalize().ok() == Some(root));
    let adopted_orphan = if clone_reentry || reuse_existing_worktree_path {
        orphan_lease_to_adopt(
            &project_root,
            &worktree_path,
            owns,
            &branch_name,
            clone_reentry,
        )?
    } else {
        None
    };
    if adopt_orphan && adopted_orphan.is_none() {
        anyhow::bail!(
            "cannot re-enter {} in place: it is no longer the tree of an orphaned `{owns}` \
             session on branch `{branch_name}`. Nothing was changed in it.",
            worktree_path.display()
        );
    }
    if clone_reentry && !contained_seat {
        anyhow::bail!(
            "{} is a contained seat's private clone, so only a contained launch may re-enter it: \
             re-run contained (--sandbox, AIDA_CONTAINED=1, or [agents] contained)",
            worktree_path.display()
        );
    }
    // TASK-2053 S4c: the default flipped to `clone`, but a clone cannot yet
    // host a review session, re-enter an existing --path, live at an
    // explicit --path that is not an `aida-seat-*` directory in the worktree
    // parent, or be made without an `origin` remote to clone from. With no
    // explicit knob those launches keep a worktree and say why; an explicit
    // `seat_tree = "clone"` refuses them (below, or here for a missing origin).
    // The launch facts (a git call, a config read, a canonicalize) are read
    // only for a clone; a worktree launch has nothing to fall back from.
    // trace:TASK-2053 | ai:claude
    let mut tree_kind = if clone_reentry {
        // The tree already is a clone, whatever the knob says now.
        crate::seat_clone::TreeKind::Clone
    } else if tree_kind != crate::seat_clone::TreeKind::Clone {
        tree_kind
    } else {
        match crate::seat_clone::default_clone_fallback(
            tree_kind,
            crate::seat_clone::seat_tree_is_explicit(&project_root),
            &crate::seat_clone::CloneLaunch {
                review: review_target.is_some(),
                reentry: reuse_existing_worktree_path,
                path_not_seat_location: explicit_path.is_some()
                    && !crate::seat_clone::is_seat_clone_location(&project_root, &worktree_path),
                has_origin: aida_core::git_ops::has_remote(&project_root, "origin"),
            },
        )? {
            (kind, Some(note)) => {
                eprintln!("note: {note}");
                kind
            }
            (kind, None) => kind,
        }
    };
    if worktree_path.exists() && !reuse_existing_worktree_path && !clone_reentry {
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
    crate::gate::operator_hold::enforce_reasoned_operator_hold(&project_root, owns)?;

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
    // BUG-2143: only a lease holding a live linked worktree is re-entered.
    // BUG-2223: the orphan being adopted is replaced below, not re-entered.
    let existing_leases: Vec<SessionLease> = crate::worktree_gc::reenterable_leases(&project_root)
        .into_iter()
        .filter(|e| !adopted_orphan.as_ref().is_some_and(|o| o.id == e.id))
        .collect();
    let existing = existing_lease_for_scope(&existing_leases, owns);
    // ...and another session's tree on the scope refuses the adoption rather
    // than being re-entered in its place.
    if let (Some(existing), Some(orphan)) = (existing, &adopted_orphan) {
        anyhow::bail!(
            "scope `{owns}` is also held by session {} ({}), so the orphaned lease {}'s tree \
             is not re-entered. Nothing was changed in it.",
            &existing.id[..existing.id.len().min(8)],
            existing.worktree_path.display(),
            &orphan.id[..orphan.id.len().min(8)],
        );
    }
    // ...and so does any other lease on the scope whose owner is still alive,
    // with or without a live linked worktree: adoption bypasses the BUG-637
    // live-claim gate's claude-process probe, so it checks owners itself.
    // An owner whose liveness cannot be determined (no recorded pid) is
    // treated as alive, the same rule queue work's conflict sweep applies.
    // trace:BUG-2223, TASK-b6b1278890774ef993d2ebc327862470-004 | ai:claude
    if let Some(orphan) = &adopted_orphan {
        let held = list_leases(&project_root).into_iter().find_map(|l| {
            if l.id == orphan.id || !l.scope.eq_ignore_ascii_case(owns) {
                return None;
            }
            match stale_lease_recovery_for_lease(&l).base_verdict {
                StaleLeaseRecovery::Live => Some((l, "live session")),
                StaleLeaseRecovery::UnknownLiveness => {
                    Some((l, "session whose owner cannot be proven gone"))
                }
                _ => None,
            }
        });
        if let Some((live, what)) = held {
            anyhow::bail!(
                "scope `{owns}` is also held by {what} {} ({}), so the orphaned lease \
                 {}'s tree is not re-entered. Nothing was changed in it.",
                &live.id[..live.id.len().min(8)],
                live.worktree_path.display(),
                &orphan.id[..orphan.id.len().min(8)],
            );
        }
    }
    if let Some(existing) = existing {
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
            return Ok(SessionStartReceipt::existing(Some(existing.id.clone())));
        }
    }

    let mut rollback = rollback_failed_setup.then(|| SessionStartCleanup::new(&project_root, owns));

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
        match coordination::acquire_claim_receipted(
            &store_root,
            owns,
            &project_root,
            &claim_role,
            /* review_verb */ is_review_session,
            force_claim || orchestrated,
        ) {
            Ok((coordination::AcquireOutcome::Acquired, claim)) => {
                if let Some(cleanup) = rollback.as_mut() {
                    cleanup.claim = claim;
                }
            }
            Ok((coordination::AcquireOutcome::Reclaimed(reason), claim)) => {
                if let Some(cleanup) = rollback.as_mut() {
                    cleanup.claim = claim;
                }
                eprintln!(
                    "  {} reclaiming a stale cross-clone lease on `{owns}` ({reason})",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt)
                );
            }
            Ok((coordination::AcquireOutcome::Unavailable(reason), claim)) => {
                if let Some(cleanup) = rollback.as_mut() {
                    cleanup.claim = claim;
                }
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
    let preflight_status: Option<RequirementStatus> = preflight_store.as_ref().and_then(|store| {
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
    // BUG-2223: the orphan being adopted is what holds the spec In Progress,
    // so adopting it lets exactly that status through; NeedsAttention and
    // every other refusal still stand, and it bypasses nothing after this.
    // trace:BUG-2223 | ai:claude
    let adoption_lifts_status =
        adopted_orphan.is_some() && matches!(preflight_status, Some(RequirementStatus::InProgress));
    match preflight_spec_status_review_aware(
        owns,
        preflight_status.as_ref(),
        force_claim_effective || adoption_lifts_status,
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
        // BUG-2143: the caller's OWN `aida claim` never blocks its own pickup —
        // `aida claim <spec>` then `aida worktree add <spec>` from one shell
        // is the documented fixer flow.
        // trace:BUG-2143 | ai:claude
        let my_pid = creator_shell_pid();
        let leases = crate::claim::without_own_claims(list_leases(&project_root), owns, my_pid);
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

    // BUG-2169: owns a just-made seat clone until the lease is written; any
    // error in between removes the clone and releases its pin.
    // trace:BUG-2169 | ai:claude
    let mut clone_cleanup: Option<crate::seat_clone::CloneCleanupGuard> = None;
    // Keep the original worktree placement if the default clone cannot reach
    // origin. No lease exists yet; create cleans its clone and pin first.
    // trace:TASK-2300 | ai:codex
    let fallback_worktree_path = worktree_path.clone();
    let created_clone = if tree_kind == crate::seat_clone::TreeKind::Clone && !clone_reentry {
        if reuse_existing_worktree_path || review_target.is_some() {
            anyhow::bail!(
                "a contained seat with `[contained] seat_tree = \"clone\"` cannot {} yet; \
                 launch it from an uncontained shell, or set seat_tree = \"worktree\"",
                if review_target.is_some() {
                    "start a review session"
                } else {
                    "re-enter an existing --path"
                }
            );
        }
        let base_ref = if reuse_existing {
            if session_git_ref_exists(&project_root, &format!("refs/heads/{branch_name}")) {
                branch_name.clone()
            } else {
                format!("origin/{branch_name}")
            }
        } else {
            match base {
                Some(b) => b.to_string(),
                None => detect_default_branch_ref(&project_root).ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot create a private clone for this contained seat: the project's \
                         default branch could not be resolved (pass --base)"
                    )
                })?,
            }
        };
        if explicit_path.is_none() {
            worktree_path = crate::seat_clone::clone_path(&project_root, &slug)?;
        } else if !crate::seat_clone::is_seat_clone_location(&project_root, &worktree_path) {
            // Only where `seat_clone::destroy` can remove it again: a clone
            // elsewhere could never be torn down by session end, reap or
            // dismiss. trace:TASK-2053 | ai:claude
            anyhow::bail!(
                "a contained seat's private clone must be an `{}*` directory directly in the \
                 worktree parent; {} is not one. Drop --path to let aida place it.",
                crate::seat_clone::CLONE_DIR_PREFIX,
                worktree_path.display()
            );
        }
        match crate::seat_clone::create(&project_root, &worktree_path, &branch_name, &base_ref) {
            Ok(created) => Some(created),
            Err(error) => {
                let Some(note) = crate::seat_clone::origin_failure_fallback(
                    crate::seat_clone::seat_tree_is_explicit(&project_root),
                    &error,
                ) else {
                    return Err(error);
                };
                // An explicit clone location has a reserved aida-seat-* name.
                // Host git refuses even a linked worktree under that name, so
                // do not create one or silently ignore the requested --path.
                // trace:TASK-2300 | ai:codex
                if explicit_path.is_some() {
                    anyhow::bail!(
                        "cloning from origin failed or timed out, but --path {} is reserved for \
                         private seat clones and cannot host a worktree fallback; drop --path \
                         to use an automatic worktree, or choose an ordinary worktree path",
                        worktree_path.display()
                    );
                }
                eprintln!("note: {note}");
                tree_kind = crate::seat_clone::TreeKind::Worktree;
                worktree_path = fallback_worktree_path;
                None
            }
        }
    } else {
        None
    };
    if let Some(created) = created_clone {
        eprintln!(
            "{} private clone for this contained seat: {} [{}] at {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            created.path.display(),
            branch_name,
            &created.base_sha[..created.base_sha.len().min(10)],
        );
        clone_cleanup = Some(crate::seat_clone::CloneCleanupGuard::new(
            &project_root,
            &created,
        ));
        if let Some(cleanup) = rollback.as_mut() {
            cleanup.clone_git_identity = clone_git_identity(&created.path);
            cleanup.clone = Some(created);
            if let Some(guard) = clone_cleanup.as_mut() {
                guard.disarm();
            }
        }
    } else if clone_reentry {
        // BUG-2223: adopt in place. The clone, its pin, its commits and its
        // uncommitted work are kept exactly as the orphaned session left them;
        // `orphan_lease_to_adopt` already checked its branch from plain files.
        // trace:BUG-2223 | ai:claude
        eprintln!(
            "{} re-entering the orphaned seat's private clone {} [{}] in place",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            worktree_path.display(),
            branch_name,
        );
    } else if reuse_existing_worktree_path {
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
                // BUG-2150: an explicit reviewer pickup coexists with the
                // implementer's lease; another reviewer's lease still conflicts.
                // trace:BUG-2150 | ai:claude
                // An existing spec alone is insufficient: the PR/MR must
                // actually credit that spec at this boundary. Unknown metadata
                // cannot grant the coexistence exception.
                // trace:TASK-1801 | ai:codex
                let decision = review_pickup_conflict_decision(
                    headless,
                    is_live,
                    l.role.as_deref(),
                    review_coexistence,
                    preflight_store.as_ref(),
                    &source_branch,
                    || {
                        crate::forge::forge_for_kind(&project_root, forge.forge_kind())
                            .change_metadata(n, &mut crate::network_retry::NoopSink)
                            .map(|meta| meta.title)
                    },
                );
                match decision {
                    LeaseConflictDecision::Coexist => {
                        eprintln!(
                            "{} PR-{}'s source branch `{}` is held by {} lease {} — the review checks out its own `{}` branch and runs alongside it",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            n,
                            source_branch,
                            l.role.as_deref().unwrap_or("an implementer"),
                            l.id,
                            branch_name
                        );
                    }
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
                if decision == LeaseConflictDecision::Prompt {
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
            // trace:BUG-2181 | ai:claude
            .remote_credentials(&project_root, "origin")
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
        let res = transactional_worktree_add(
            &project_root,
            &[
                "worktree",
                "add",
                "--",
                worktree_path.to_str().unwrap(),
                branch_name.as_str(),
            ],
            &worktree_path,
            &branch_name,
            &mut rollback,
        )?;
        let _ = std::io::stderr().write_all(&res.stdout);
        let _ = std::io::stderr().write_all(&res.stderr);
        if !res.status.success() {
            anyhow::bail!(
                "`git worktree add` failed for {}: {}",
                worktree_path.display(),
                String::from_utf8_lossy(&res.stderr).trim()
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
        let res = transactional_worktree_add(
            &project_root,
            &[
                "worktree",
                "add",
                "--",
                worktree_path.to_str().unwrap(),
                branch_name.as_str(),
            ],
            &worktree_path,
            &branch_name,
            &mut rollback,
        )?;
        use std::io::Write;
        let _ = std::io::stderr().write_all(&res.stdout);
        let _ = std::io::stderr().write_all(&res.stderr);
        if !res.status.success() {
            anyhow::bail!(
                "`git worktree add` (reuse existing branch `{}`) failed — \
                 the branch may already be checked out in another worktree (path {}): {}",
                branch_name,
                worktree_path.display(),
                String::from_utf8_lossy(&res.stderr).trim()
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
            match acquire_session_pool_worktree(&project_root, &branch_name, &mut rollback) {
                Ok(acquired) => Some(acquired),
                Err(e) => {
                    // A full pool must not fail an `agent new --spec` launch:
                    // undo whatever pool tree/branch this receipt took, then
                    // fall back to a fresh worktree under the same receipt.
                    // trace:BUG-2188 | ai:claude
                    if let Some(cleanup) = rollback.as_mut() {
                        if !cleanup.release_allocation() {
                            return Err(e.context(
                                "warm-pool setup failed and its pooled tree was kept; inspect `aida session leases --all`",
                            ));
                        }
                    }
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
            let res = transactional_worktree_add(
                &project_root,
                &args,
                &worktree_path,
                &branch_name,
                &mut rollback,
            )?;
            use std::io::Write;
            let _ = std::io::stderr().write_all(&res.stdout);
            let _ = std::io::stderr().write_all(&res.stderr);
            if !res.status.success() {
                anyhow::bail!(
                    "`git worktree add` failed for {}: {}",
                    worktree_path.display(),
                    String::from_utf8_lossy(&res.stderr).trim()
                );
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
        // BUG-2169 S2: a contained clone seat's sandbox binds this directory
        // read-write, and a sandbox skips a path missing at launch, so the
        // host makes it now.
        // trace:BUG-2169 | ai:claude
        if tree_kind == crate::seat_clone::TreeKind::Clone {
            std::fs::create_dir_all(&isolated_target).with_context(|| {
                format!(
                    "create the seat clone's cargo target directory {}",
                    isolated_target.display()
                )
            })?;
        }
        write_session_env_file(&worktree_path, &isolated_target).with_context(|| {
            format!("writing session env shim under {}", worktree_path.display())
        })?;
    }

    // Compose lease.
    let id_long = uuid::Uuid::now_v7().to_string();
    let id = id_long.replace('-', "")[..12].to_string();
    // BUG-2010: inside an `aida agent new` session (AIDA_AGENT_NAME set),
    // attribute the lease to that agent's live process rather than a short-lived
    // transient CLI / shell PID, but ONLY on self-claim paths. Leases minted
    // for another agent (agent new, session start --launch) skip attribution
    // so the launcher lane is not falsely pinned to the child's lease.
    // Drain phase children get their own lease PID and also skip agent resolution.
    // trace:BUG-2010 | ai:antigravity
    let is_drain_child = orchestrator::detect(&project_root).is_orchestrated();
    let session_agent = if is_drain_child || !self_claim {
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
        // trace:BUG-2169 | ai:claude
        tree_kind,
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
        child_grant_id: None,
    };
    let lease_file = lease_path(&project_root, &id);
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    // trace:BUG-2169 | ai:claude
    crate::seat_clone::failpoint::check_before_lease()?;
    aida_core::write_atomic(&lease_file, toml::to_string_pretty(&lease)?)?;
    if let Some(cleanup) = rollback.as_mut() {
        cleanup.lease_id = Some(id.clone());
    }
    if let Some(guard) = clone_cleanup.as_mut() {
        guard.disarm();
    }
    // BUG-2223: the adopted orphan's lease record goes only once its successor
    // is written, so the tree is never unleased; the tree itself stays.
    // trace:BUG-2223 | ai:claude
    if let Some(orphan) = &adopted_orphan {
        aggregate_session_activity_into_roles(&project_root, &orphan.id);
        if !remove_session_lease_record_only(&project_root, &orphan.id) {
            // Not `aida session end`: that would tear down the adopted tree.
            eprintln!(
                "{} could not remove the adopted orphaned lease {}; delete {} by hand (its \
                 tree is now leased by this session)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                &orphan.id[..orphan.id.len().min(8)],
                lease_path(&project_root, &orphan.id).display(),
            );
        }
    }
    // BUG-1918: durable record of who claimed this spec, surviving release.
    // trace:BUG-1918 | ai:claude
    crate::gate::review_authority::record_authoring_lease(&project_root, &lease);

    // BUG-2010: update the resolved agent's own registry entry with its new
    // current_spec and worktree path.
    // trace:BUG-2010 | ai:antigravity
    if let Some(ref agent) = session_agent {
        if let Err(e) = agent_registry::update_agent_current_spec(
            &project_root,
            &agent.id,
            owns,
            &worktree_path,
        ) {
            eprintln!("aida: warning: failed to update agent registry current_spec: {e}");
        }
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
        // trace:BUG-2215 | ai:codex
        let claimed = crate::claim::resolve_spec_for_claim(owns).and_then(|(_, req)| {
            crate::claim::persist_claim_status_with_receipt(&project_root, &req, &lease.owner)
        });
        match claimed {
            Ok(change) => {
                let bumped = change.bumped;
                if let Some(cleanup) = rollback.as_mut().filter(|_| change.changed()) {
                    cleanup.claim_write = Some(change);
                }
                bumped
            }
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
        // BUG-1918: the exec'd worker carries its lease id.
        // trace:BUG-1918 | ai:claude
        crate::gate::review_authority::export_worker_session_env(&id);
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
        )
        .map(|_| SessionStartReceipt::existing(None));
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
    Ok(SessionStartReceipt {
        lease_id: Some(id),
        cleanup: rollback,
    })
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

#[cfg(test)]
#[path = "tests/bug_2215_session_claim_rollback_tests.rs"]
mod claim_rollback_tests;
