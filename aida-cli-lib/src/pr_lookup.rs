//! open/merged PR detection, PR credit matching and PR-to-spec resolution (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Open-PR metadata captured by `gh pr list` for a session's branch. Just
/// the fields the auto-queue side-effect needs to brief a reviewer.
/// trace:STORY-66 | ai:claude
pub(crate) struct OpenPrInfo {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) head_branch: Option<String>,
}

/// Why `detect_open_pr_for_branch` returned no PR — so the caller (and the
/// user reading `aida session end` output) can tell "no PR yet" from "gh
/// isn't installed" from "gh blew up". The old None-everything return type
/// hid the difference and made BUG-72 hard to diagnose.
///
/// BUG-257 split `GhFailed` further: a *transient* `GhUnreachable` (the GH
/// API was unreachable) is distinct from a `GhFailed` (auth, parse, gh
/// itself errored). The orchestrator's phase-1 reports the first as
/// *Inconclusive* (drain pauses, retry later) and the second as a phase
/// failure — same as before. Conflating the two made every network blip
/// look like a "no PR" failure with a misleading recovery hint.
/// trace:BUG-72 BUG-257 | ai:claude
pub(crate) enum PrLookup {
    Found(OpenPrInfo),
    /// `gh` ran cleanly but reported no open PR for this branch.
    NoOpenPr,
    /// `gh` is not on $PATH (binary missing).
    GhMissing,
    /// `gh` was found but its invocation failed (auth, parse, ...). The
    /// String carries the trimmed stderr / parse error so the user sees
    /// the actual cause instead of a silent no-op.
    GhFailed(String),
    /// `gh` ran but could not reach the GitHub API — a *transient* network
    /// error (DNS, TCP, TLS, githubstatus pointer). The orchestrator treats
    /// this as Inconclusive (the API outage means we cannot tell whether a
    /// PR exists), not as a phase failure. trace:BUG-257 | ai:claude
    GhUnreachable(String),
}

/// True when `path` exists as a file and is executable by the current
/// process. On Windows we just check existence (the PATHEXT-aware Rust
/// spawn handles the rest); on Unix we check the executable bit on the
/// metadata. trace:BUG-74 | ai:claude
pub(crate) fn is_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

/// Format a `gh` spawn failure for a `PrLookup::GhFailed`. The OS reports
/// ENOENT against the *binary* when the spawn's working directory has been
/// removed — even though the binary is perfectly fine. `aida session end`
/// removes the session worktree, so this case is common; say so plainly
/// rather than letting the message read as "gh is missing".
/// trace:BUG-107 | ai:claude
pub(crate) fn gh_spawn_error(
    gh_bin: &std::path::Path,
    cwd: &std::path::Path,
    e: &std::io::Error,
) -> String {
    if !cwd.is_dir() {
        format!(
            "working directory `{}` no longer exists — the `{}` binary is fine \
             (os reports the missing cwd as ENOENT on the spawn): {}",
            cwd.display(),
            gh_bin.display(),
            e
        )
    } else {
        format!(
            "spawn `{}` (cwd `{}`): {}",
            gh_bin.display(),
            cwd.display(),
            e
        )
    }
}

/// Run `gh pr list <filter> --limit 1 --json number,title,url,headRefName` and parse the
/// single result line into a [`PrLookup`]. The shared core behind the
/// branch-keyed and spec-keyed PR lookups — each caller supplies only its
/// distinguishing `<filter>` args (`--head <branch>` / `--search <query>`
/// plus `--state`).
///
/// BUG-74: resolves `gh` via an explicit PATH walk + absolute-path fallback
/// rather than `Command::new("gh")`, because a child Rust process can see a
/// PATH that omits the user's gh install dir. BUG-107: a spawn `NotFound`
/// after the binary resolved means a removed working directory far more
/// often than a missing binary — `gh_spawn_error` names the real culprit.
/// AIDA_DEBUG_GH=1 surfaces the binary search.
/// trace:STORY-66 BUG-72 BUG-74 BUG-107 BUG-223 | ai:claude
pub(crate) fn gh_pr_list_first(project_root: &std::path::Path, filter: &[&str]) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let mut args: Vec<&str> = vec!["pr", "list"];
    args.extend_from_slice(filter);
    args.extend_from_slice(&[
        "--limit",
        "1",
        "--json",
        // trace:BUG-1812 | ai:antigravity
        "number,title,url,headRefName,isDraft",
        "-q",
        r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
    ]);
    // BUG-1594: honor a caller-scoped ceiling (`aida show`) so one slow
    // `gh` call cannot stall an interactive read. trace:BUG-1594 | ai:claude
    let mut gh_cmd = std::process::Command::new(&gh_bin);
    gh_cmd.current_dir(project_root).args(&args);
    let out = match forge_lookup_output(gh_cmd) {
        Ok(Some(o)) => o,
        Ok(None) => return PrLookup::GhUnreachable(FORGE_LOOKUP_TIMED_OUT.to_string()),
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }
    parse_gh_pr_line(&String::from_utf8_lossy(&out.stdout))
}

/// Classify a `gh` stderr line as a transient network error (the GH API
/// was unreachable) rather than an auth/parse/other failure. Used by
/// [`gh_pr_list_first`] to split `PrLookup::GhFailed` into the
/// `GhUnreachable` variant so the orchestrator can report *Inconclusive*
/// instead of conflating "no PR" with "can't reach the API".
///
/// Patterns are taken from real-world `gh` output (the Go HTTP client +
/// gh's own diagnostic suffixes). The match is conservative —
/// case-insensitive substring matching against a small allow-list — so an
/// auth/parse failure never gets re-classified as a transient network
/// blip. trace:BUG-257 | ai:claude
pub(crate) fn gh_stderr_is_network_error(stderr: &str) -> bool {
    // external-prose-classifier: gh_stderr_is_network_error
    aida_core::external_tool_output::contains_any_case_insensitive(
        stderr,
        aida_core::external_tool_output::GH_NETWORK_TRANSIENT,
    )
}

/// STORY-516: reverse of `change_lookup_from_pr_lookup` — convert a forge
/// `ChangeLookup` back to the orchestrator's `PrLookup` so the `*_via_forge`
/// helpers are a pure name-swap at their call sites (match arms stay PrLookup).
/// trace:STORY-516 | ai:claude
pub(crate) fn pr_lookup_from_change_lookup(c: crate::forge::ChangeLookup) -> PrLookup {
    match c {
        crate::forge::ChangeLookup::Found(r) => PrLookup::Found(OpenPrInfo {
            number: r.id,
            title: r.title.unwrap_or_default(),
            url: r.url,
            head_branch: (!r.branch.is_empty()).then_some(r.branch),
        }),
        crate::forge::ChangeLookup::NoChange => PrLookup::NoOpenPr,
        crate::forge::ChangeLookup::CliMissing => PrLookup::GhMissing,
        crate::forge::ChangeLookup::CliFailed(s) => PrLookup::GhFailed(s),
        crate::forge::ChangeLookup::Unreachable(s) => PrLookup::GhUnreachable(s),
    }
}

// STORY-1163: inverse adapter for legacy callers that still consume
// `ChangeLookup` while the central PR helper now dispatches by forge.
// trace:STORY-1163 | ai:codex
pub(crate) fn change_lookup_from_pr_lookup_for_branch(
    p: PrLookup,
    branch_hint: &str,
) -> crate::forge::ChangeLookup {
    match p {
        PrLookup::Found(pr) => crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: pr.number,
            url: pr.url,
            branch: pr
                .head_branch
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| branch_hint.to_string()),
            base: String::new(),
            title: Some(pr.title),
        }),
        PrLookup::NoOpenPr => crate::forge::ChangeLookup::NoChange,
        PrLookup::GhMissing => crate::forge::ChangeLookup::CliMissing,
        PrLookup::GhFailed(s) => crate::forge::ChangeLookup::CliFailed(s),
        PrLookup::GhUnreachable(s) => crate::forge::ChangeLookup::Unreachable(s),
    }
}

/// STORY-516: forge-routed spec-search open-PR lookup (BUG-223 fallback). GitHub
/// delegates to `detect_open_pr_for_spec`. trace:STORY-516 | ai:claude
pub(crate) fn detect_open_pr_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    detect_open_pr_for_spec(project_root, spec)
}

/// STORY-516: forge-routed merged-PR-for-branch lookup. GitHub delegates to
/// `detect_merged_pr_for_branch`. trace:STORY-516 | ai:claude
pub(crate) fn detect_merged_pr_for_branch_via_forge(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    detect_merged_pr_for_branch(project_root, branch)
}

/// STORY-516: forge-routed branch lookup — the view-op entry point the call
/// sites use instead of `detect_open_pr_for_branch` directly, so a GitLab /
/// pure-git repo goes through its own provider. GitHubForge delegates back to
/// `detect_open_pr_for_branch`, so behaviour on GitHub is unchanged. The
/// provider returns `Result<ChangeLookup>`; GitHub never errs (it adapts a
/// PrLookup), and a provider Err (GitLab stub) collapses to `CliFailed` so
/// callers keep a single 5-state match. trace:STORY-516 | ai:claude
pub(crate) fn change_lookup_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> crate::forge::ChangeLookup {
    change_lookup_from_pr_lookup_for_branch(detect_open_pr_for_branch(project_root, branch), branch)
}

// trace:BUG-876 | ai:codex
pub(crate) fn change_lookup_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> crate::forge::ChangeLookup {
    crate::forge::forge_for(project_root)
        .change_for_spec(spec)
        .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}")))
}

/// Look up a single open PR keyed on `branch`. The richer [`PrLookup`]
/// return shape lets callers print an honest skip reason (no PR yet / gh
/// missing / gh failed) instead of collapsing every case to `None`.
/// trace:STORY-66 BUG-72 BUG-74 | ai:claude
pub(crate) fn detect_open_pr_for_branch(project_root: &std::path::Path, branch: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub lookup used by GitHubForge after the public helper
// became forge-dispatched. The `gh pr list` argv/parsing stay unchanged.
// trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "open"])
}

/// BUG-257: the three states `probe_branch_on_origin` can settle on when the
/// orchestrator narrows a `PrLookup::GhUnreachable` with `git ls-remote`. The
/// existing `aida_core::git_ops::remote_branch_exists` collapses `Absent` and
/// `LsRemoteFailed` into one `false` — exactly the conflation BUG-257 must
/// avoid here, so we keep this enum and the probe local to the orchestrator
/// caller. trace:BUG-257 | ai:claude
pub(crate) enum BranchOriginProbe {
    /// `git ls-remote` reported the branch is on origin — a PR may exist.
    Present,
    /// `git ls-remote` ran cleanly and reported the branch is absent — no
    /// PR can exist regardless of GH-API state.
    Absent,
    /// `git ls-remote` itself failed (network down end-to-end, auth, ...).
    /// Cannot narrow the diagnosis at all.
    LsRemoteFailed,
}

/// BUG-257: narrow a `PrLookup::GhUnreachable` outcome with `git ls-remote`
/// against `origin`. The git protocol is HTTPS-API-independent, so when the
/// GH API is down `ls-remote` often still works — and a missing branch
/// proves no PR can exist (the implementer never pushed). When the branch
/// IS present we still cannot tell whether a PR was opened against it
/// without the API; that case becomes Inconclusive. When `ls-remote` also
/// fails, the orchestrator has no way to tell and stays Inconclusive.
/// Best-effort: short timeout so a hung connection never blocks phase 1.
/// trace:BUG-257 | ai:claude
pub(crate) fn probe_branch_on_origin(
    project_root: &std::path::Path,
    branch: &str,
) -> BranchOriginProbe {
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "-c",
            // Keep the probe bounded — a hung HTTPS dial against origin
            // would otherwise re-introduce the freeze BUG-257 is fixing.
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--exit-code",
            "--heads",
            "origin",
            &format!("refs/heads/{branch}"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => BranchOriginProbe::Present,
        // `git ls-remote --exit-code` exits 2 when the ref is missing on a
        // remote it could otherwise reach. Distinguish that from a true
        // ls-remote failure (network down, auth) by checking the exit code.
        Ok(o) => match o.status.code() {
            Some(2) => BranchOriginProbe::Absent,
            _ => BranchOriginProbe::LsRemoteFailed,
        },
        Err(_) => BranchOriginProbe::LsRemoteFailed,
    }
}

pub(crate) fn spec_branch_slug(spec: &str) -> String {
    spec.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

pub(crate) fn branch_name_references_spec(branch: &str, spec: &str) -> bool {
    let slug = spec_branch_slug(spec);
    let branch = branch
        .trim()
        .trim_start_matches("origin/")
        .to_ascii_lowercase();
    if slug.is_empty() {
        return false;
    }
    // BUG-1525 review fix: the slug must be a whole segment of the branch
    // name. A raw substring let BUG-15 match `bug-150-work` and BUG-152
    // match `claude/bug-1525`. A boundary is start/end or one of `/-._`,
    // and the character after the slug must not be a digit or letter.
    // trace:BUG-1525 | ai:claude
    let is_sep = |c: char| matches!(c, '/' | '-' | '.' | '_');
    branch.match_indices(&slug).any(|(at, m)| {
        let before_ok = branch[..at].chars().next_back().is_none_or(is_sep);
        let after_ok = branch[at + m.len()..].chars().next().is_none_or(is_sep);
        before_ok && after_ok
    })
}

#[cfg(test)]
mod bug_1525_branch_match_tests {
    use super::branch_name_references_spec as m;

    // trace:BUG-1525 | ai:claude
    #[test]
    fn branch_match_is_whole_segment_only() {
        assert!(m("bug-15-work", "BUG-15"));
        assert!(m("claude/bug-15", "BUG-15"));
        assert!(m("origin/bug-15", "BUG-15"));
        assert!(m("bug-15", "BUG-15"));
        assert!(!m("bug-150-work", "BUG-15"));
        assert!(!m("claude/bug-1525", "BUG-152"));
        assert!(!m("xbug-15", "BUG-15"));
    }
}

pub(crate) fn open_pr_commit_headlines_reference_spec(
    project_root: &std::path::Path,
    pr: u64,
    spec: &str,
) -> bool {
    let Some(gh_bin) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return false;
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "commits",
            "-q",
            ".commits[].messageHeadline",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let wanted = spec.to_ascii_uppercase();
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        extract_spec_ids_from_commit(line)
            .into_iter()
            .any(|id| id.eq_ignore_ascii_case(&wanted))
    })
}

pub(crate) fn detect_open_pr_for_spec_by_head_or_commit(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let spawned = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,url,headRefName,isDraft",
            "-q",
            r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let out = match spawned {
        Ok(o) => o,
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }

    let mut candidates: Vec<OpenPrInfo> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let PrLookup::Found(pr) = parse_gh_pr_line(line) {
            candidates.push(pr);
        }
    }

    for pr in &candidates {
        if pr
            .head_branch
            .as_deref()
            .is_some_and(|b| branch_name_references_spec(b, spec))
        {
            return PrLookup::Found(OpenPrInfo {
                number: pr.number,
                title: pr.title.clone(),
                url: pr.url.clone(),
                head_branch: pr.head_branch.clone(),
            });
        }
    }
    for pr in candidates {
        if open_pr_commit_headlines_reference_spec(project_root, pr.number, spec) {
            return PrLookup::Found(pr);
        }
    }
    PrLookup::NoOpenPr
}

/// Fallback PR lookup for BUG-223/BUG-876: find an open PR that references
/// `spec`, used when branch/lease linkage is unavailable or stale. Search title
/// / body first, then open PR head branches (`story-818`) and commit headlines
/// carrying `(SPEC-ID)` trailers so releasing a lease cannot hide a pushed PR.
/// trace:BUG-223 | ai:claude
// trace:BUG-876 | ai:codex
pub(crate) fn detect_open_pr_for_spec(project_root: &std::path::Path, spec: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_spec(spec)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub spec lookup used by GitHubForge after the public helper
// became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_spec_github(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    match gh_pr_list_first(project_root, &["--search", spec, "--state", "open"]) {
        PrLookup::NoOpenPr => detect_open_pr_for_spec_by_head_or_commit(project_root, spec),
        other => other,
    }
}

/// TASK-843: list ALL open PRs that reference `spec` (number + head branch),
/// not just the first. `detect_open_pr_for_spec_via_forge` returns at most one,
/// so a spec with >1 open PR (a reopened/duplicate) was resolved by a silent,
/// non-deterministic pick. This surfaces the full candidate set so the pure
/// [`integrate::select_canonical_pr`] policy can pick the newest-canonical PR
/// and report the rest. Best-effort: an empty vec when gh is missing/failing or
/// no PR references the spec — the caller falls back to the single-PR path.
/// trace:TASK-843 | ai:claude
pub(crate) fn all_open_prs_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> Vec<(u64, String)> {
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--search",
            spec,
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "number,headRefName",
            "-q",
            r#".[] | "\(.number)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (num, branch) = line.split_once('\t')?;
            let number = num.trim().parse::<u64>().ok()?;
            Some((number, branch.trim().to_string()))
        })
        .collect()
}

/// Look up the head branch of an open PR by number — used by the BUG-223
/// spec-id fallback to realign the orchestrator's branch after a swap the
/// worktree-HEAD reconciliation missed, so the CI / merge phases probe the
/// PR's actual head. Best-effort: `None` on any `gh` failure.
/// trace:BUG-223 | ai:claude
pub(crate) fn pr_head_branch(project_root: &std::path::Path, pr_number: u64) -> Option<String> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// TASK-1230: the UNIQUE registered worktree checked out on `branch`, excluding
/// the main checkout and the store worktree. Returns None on zero or ambiguous
/// (>1) matches — the caller must never guess which of several to remove.
// trace:TASK-1230 | ai:claude
pub(crate) fn unique_worktree_on_branch(
    project_root: &std::path::Path,
    branch: &str,
    main_root: &std::path::Path,
    store_wt: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if branch.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Porcelain blocks are blank-line-separated; each has a `worktree <path>`
    // line and (when on a branch) a `branch refs/heads/<name>` line.
    let mut matches: Vec<std::path::PathBuf> = Vec::new();
    let mut cur_path: Option<std::path::PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            cur_path = Some(std::path::PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
            if short == branch {
                if let Some(p) = &cur_path {
                    let is_protected = p == main_root
                        || p == store_wt
                        || p.canonicalize().ok() == main_root.canonicalize().ok()
                        || p.canonicalize().ok() == store_wt.canonicalize().ok();
                    if !is_protected {
                        matches.push(p.clone());
                    }
                }
            }
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// TASK-1230: count of commits on the worktree's HEAD that are NOT yet on the
/// pushed PR branch (`origin/<branch>`) — genuinely-at-risk unpushed work. Zero
/// means every local commit is safely on the remote PR branch. On any git
/// failure returns a conservative 1 (treat as at-risk → keep).
// trace:TASK-1230 | ai:claude
pub(crate) fn local_commits_not_on_branch(worktree: &std::path::Path, branch: &str) -> u32 {
    if branch.is_empty() {
        return 1;
    }
    let remote_ref = format!("origin/{branch}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-list", "--count", "HEAD", "--not", &remote_ref])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            .unwrap_or(1),
        // The remote ref not existing, or any other git error, is treated as
        // "cannot prove the work is pushed" → keep (conservative).
        _ => 1,
    }
}

/// Detect whether `branch` was the head of a now-MERGED PR. Used by
/// `aida push` (BUG-88) to warn that new commits will be stranded —
/// pushing to a merged-PR branch puts the commit on `origin/<branch>`
/// but it won't reach `main` without a new PR. Mirrors the shape of
/// [`detect_open_pr_for_branch`] but queries `--state merged` and returns
/// only the first hit (most recent). trace:BUG-88 | ai:claude
pub(crate) fn detect_merged_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .merged_change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub merged lookup used by GitHubForge after the public
// helper became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_merged_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "merged"])
}

/// Is PR #`pr` merged on GitHub? Ground truth for the BUG-241 reconcile step
/// — a reviewer that escalated the merge to a human who merged out-of-band
/// leaves the PR merged but the orchestrator's verdict file absent. `None` on
/// any `gh` failure (binary missing, auth, network): the caller treats
/// "cannot confirm" as "the failure stands", never as a silent success.
/// trace:BUG-241 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_is_merged` but with a
/// caller-supplied retry sink, so the orchestrator can surface a sub-second
/// transient blip to the drain-state file rather than treating it as
/// "cannot confirm." Non-orchestrator callers (none today) can pass
/// [`network_retry::NoopSink`] to keep the original silent best-effort
/// semantics. trace:BUG-286 | ai:claude
pub(crate) fn pr_is_merged_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<bool> {
    // STORY-621 Slice 2: routed through Forge::change_metadata so the
    // reconcile works on GitLab. GitHubForge keeps the resolve_gh_binary +
    // BUG-286 retry-with-sink behaviour this site had inline; any provider
    // error (CLI missing / failed / unreachable) maps to None — "cannot
    // confirm", never a silent success. trace:TASK-963 | ai:claude
    crate::forge::forge_for(project_root)
        .change_metadata(pr as u64, sink)
        .ok()
        .map(|m| m.state == crate::forge::ChangeState::Merged)
}

/// BUG-245: which SPEC-ID does the PR's commits actually credit?
///
/// Reads the PR's commit subjects via `gh pr view <N> --json commits` and
/// parses `(SPEC-ID)` trailers out of each. Returns:
///   - `Some(dispatched)` when the dispatched id appears among the credits —
///     the common path. No mismatch.
///   - `Some(other)` when the dispatched id is *absent* and a different id is
///     credited. The orchestrator reports this as a `shipped-mismatch`
///     anomaly and credits the truth.
///   - `None` when the PR cannot be inspected, no commit names any spec, or
///     `gh` is unreachable — "cannot determine" resolves to "trust the
///     dispatched id", so the reconcile only ratifies a real mismatch.
///
/// Picks the first credited id when multiple non-dispatched ids appear. The
/// observed BUG-245 case is one commit / one trailer; a PR carrying multiple
/// genuinely-different specs is an open-ended case the operator must triage.
/// trace:BUG-245 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_credited_spec_id` but with a
/// caller-supplied retry sink. The orchestrator passes a [`crate::network_retry::DualSink`]
/// (stderr + drain-state); a hypothetical silent caller can pass
/// [`network_retry::NoopSink`]. trace:BUG-286 | ai:claude
pub(crate) fn pr_credited_spec_id_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<String> {
    // STORY-621 Slice 2: routed through Forge::change_commit_headlines
    // (GitHubForge issues the same `gh pr view --json commits` argv this site
    // had inline, retry-wrapped). trace:TASK-963 | ai:claude
    let headlines = crate::forge::forge_for(project_root)
        .change_commit_headlines(pr as u64, sink)
        .ok()?;
    pick_credited_spec(&headlines.join("\n"), dispatched)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrCreditMatch {
    Dispatched,
    Other(String),
    Unknown,
}

/// BUG-357: classify whether PR metadata credits the dispatched spec before a
/// reconcile step treats a merged PR as out-of-band success. Commit trailers
/// are strongest; a different commit trailer is an explicit mismatch. PR title
/// trailers are accepted when commits carry no SPEC-ID, matching `aida pr ship`
/// and manual squash flows where the PR title is the only durable trailer.
pub(crate) fn classify_pr_credit(
    commit_subjects: &str,
    pr_title: Option<&str>,
    dispatched: &str,
) -> PrCreditMatch {
    match pick_credited_spec(commit_subjects, dispatched) {
        Some(id) if id.eq_ignore_ascii_case(dispatched) => return PrCreditMatch::Dispatched,
        Some(id) => return PrCreditMatch::Other(id),
        None => {}
    }

    if let Some(title) = pr_title {
        let mut first_other: Option<String> = None;
        for id in extract_spec_ids_from_commit(title) {
            if id.eq_ignore_ascii_case(dispatched) {
                return PrCreditMatch::Dispatched;
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
        if let Some(id) = first_other {
            return PrCreditMatch::Other(id);
        }
    }

    PrCreditMatch::Unknown
}

pub(crate) fn pr_credit_match_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    pr_title: Option<&str>,
    sink: &mut dyn network_retry::RetrySink,
) -> PrCreditMatch {
    let initial_title_match = pr_title
        .map(|title| classify_pr_credit("", Some(title), dispatched))
        .unwrap_or(PrCreditMatch::Unknown);
    if matches!(initial_title_match, PrCreditMatch::Dispatched) {
        return PrCreditMatch::Dispatched;
    }

    // STORY-621 Slice 2: routed through the forge metadata reads (BUG-357
    // reconcile works on GitLab). Commit headlines settle most cases; the PR
    // title (Forge::change_metadata) is fetched only when the commits leave
    // the credit Unknown — same precedence as the single gh call this
    // replaces, paying for the scalar read only when it is load-bearing.
    // trace:TASK-963 | ai:claude
    let forge = crate::forge::forge_for(project_root);
    let subjects = match forge.change_commit_headlines(pr as u64, sink) {
        Ok(headlines) => headlines.join("\n"),
        Err(_) => return PrCreditMatch::Unknown,
    };
    match classify_pr_credit(&subjects, None, dispatched) {
        PrCreditMatch::Dispatched => PrCreditMatch::Dispatched,
        PrCreditMatch::Other(id) => PrCreditMatch::Other(id),
        PrCreditMatch::Unknown => {
            let fetched_title = forge
                .change_metadata(pr as u64, sink)
                .ok()
                .map(|m| m.title)
                .filter(|t| !t.trim().is_empty());
            match classify_pr_credit("", fetched_title.as_deref(), dispatched) {
                PrCreditMatch::Unknown => initial_title_match,
                title_match => title_match,
            }
        }
    }
}

/// Pure selector for the credited spec id, given newline-separated commit
/// subjects (one per commit on the PR) and the dispatched id. Split out so
/// the precedence rules are unit-testable without spawning `gh`.
/// trace:BUG-245 | ai:claude
pub(crate) fn pick_credited_spec(commit_subjects: &str, dispatched: &str) -> Option<String> {
    let mut first_other: Option<String> = None;
    for subject in commit_subjects.lines() {
        for id in extract_spec_ids_from_commit(subject) {
            if id == dispatched {
                return Some(id);
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
    }
    first_other
}

/// Parse a single tab-separated `gh pr list -q ...` line into a
/// `PrLookup`. Extracted for unit-testing the empty / well-formed /
/// malformed shapes without spawning `gh`. trace:BUG-88 | ai:claude
pub(crate) fn parse_gh_pr_line(stdout: &str) -> PrLookup {
    let Some(line) = stdout.lines().next().map(str::trim) else {
        return PrLookup::NoOpenPr;
    };
    if line.is_empty() {
        return PrLookup::NoOpenPr;
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return PrLookup::GhFailed(format!("could not parse gh output: {:?}", line));
    }
    let Ok(number) = parts[0].parse::<u64>() else {
        return PrLookup::GhFailed(format!("non-numeric PR number from gh: {:?}", parts[0]));
    };
    PrLookup::Found(OpenPrInfo {
        number,
        title: parts[1].to_string(),
        url: parts[2].to_string(),
        head_branch: parts
            .get(3)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

pub(crate) fn open_pr_review_story_using(
    project_root: &std::path::Path,
    pr_number: u64,
    aida: &std::path::Path,
) -> Option<String> {
    let Ok(out) = std::process::Command::new(aida)
        .current_dir(project_root)
        .args(["list", "--type", "story", "--format", "json"])
        .output_retrying_etxtbsy()
    else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    open_review_story_id(&String::from_utf8_lossy(&out.stdout), pr_number)
}

/// BUG-1186: does `aida list --type story --format json` output carry a
/// `Review PR-<n>:` story whose round is still open (not Done / Completed /
/// Rejected / Superseded)? Pure so the round semantics are unit-testable.
// trace:BUG-1186 | ai:claude
#[cfg(test)]
pub(crate) fn review_story_round_is_open(list_json: &str, pr_number: u64) -> bool {
    open_review_story_id(list_json, pr_number).is_some()
}

pub(crate) fn open_review_story_id(list_json: &str, pr_number: u64) -> Option<String> {
    let needle = format!("review pr-{}:", pr_number);
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(list_json) else {
        return None;
    };
    rows.iter().find_map(|row| {
        let title = row
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_start()
            .to_ascii_lowercase();
        if !title.starts_with(&needle) {
            return None;
        }
        let status = row
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        (!matches!(
            status.as_str(),
            "done" | "completed" | "rejected" | "superseded" | "released"
        ))
        .then(|| row.get("spec_id")?.as_str().map(str::to_owned))
        .flatten()
    })
}

pub(crate) fn reviewer_queue_story_ids(project_root: &std::path::Path) -> Option<Vec<String>> {
    let out = std::process::Command::new(aida_exe_path())
        .current_dir(project_root)
        .args([
            "queue",
            "list",
            "--all",
            "--for",
            "reviewer",
            "--json",
            "--no-scope",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reviewer_queue_story_ids(&out.stdout)
}

// TASK-192: keep malformed output distinguishable from a valid empty queue so
// callers that gate operator action can fail closed.
// trace:TASK-192 | ai:codex
pub(crate) fn parse_reviewer_queue_story_ids(bytes: &[u8]) -> Option<Vec<String>> {
    let rows = serde_json::from_slice::<Vec<serde_json::Value>>(bytes).ok()?;
    Some(
        rows.iter()
            .filter(|row| row.get("for_role").and_then(|v| v.as_str()) == Some("reviewer"))
            .filter_map(|row| {
                row.get("spec_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .collect(),
    )
}

// trace:TASK-192 | ai:codex
pub(crate) fn pr_has_merge_hold(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> bool {
    merge_hold::read_hold(project_root, pr.number).is_some()
        || pr
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("aida:merge-hold"))
}

// trace:TASK-192 | ai:codex
pub(crate) fn reviewer_route_for_pr(
    routed_prs: Option<&std::collections::HashSet<u64>>,
    pr_number: u64,
) -> awaiting_you::ReviewerRoute {
    match routed_prs {
        Some(prs) if prs.contains(&pr_number) => awaiting_you::ReviewerRoute::Routed,
        Some(_) => awaiting_you::ReviewerRoute::Unrouted,
        None => awaiting_you::ReviewerRoute::Unknown,
    }
}

#[cfg(test)]
mod task_192_fail_closed_fact_tests {
    use super::*;

    fn pr(labels: &[&str]) -> status_cleanup::OpenPrItem {
        status_cleanup::OpenPrItem {
            number: 2035,
            is_draft: false,
            title: "broken".into(),
            head_branch: "broken-pr".into(),
            ci_rollup: Some("fail".into()),
            mergeable: Some("MERGEABLE".into()),
            review_decision: None,
            head_sha: Some("deadbeef".into()),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            created_at: None,
        }
    }

    #[test]
    fn forge_label_only_merge_hold_is_authoritative() {
        let root = tempfile::tempdir().unwrap();
        assert!(pr_has_merge_hold(root.path(), &pr(&["aida:merge-hold"])));
        assert!(pr_has_merge_hold(root.path(), &pr(&["AIDA:MERGE-HOLD"])));
        assert!(!pr_has_merge_hold(root.path(), &pr(&["bug"])));
    }

    #[test]
    fn reviewer_queue_parser_distinguishes_empty_from_malformed() {
        assert_eq!(parse_reviewer_queue_story_ids(b"[]"), Some(Vec::new()));
        assert_eq!(parse_reviewer_queue_story_ids(b"not json"), None);
        assert_eq!(parse_reviewer_queue_story_ids(b"{}"), None);
    }

    #[test]
    fn unavailable_queue_maps_to_unknown_not_unrouted() {
        assert_eq!(
            reviewer_route_for_pr(None, 2035),
            awaiting_you::ReviewerRoute::Unknown
        );
        let available_empty = std::collections::HashSet::new();
        assert_eq!(
            reviewer_route_for_pr(Some(&available_empty), 2035),
            awaiting_you::ReviewerRoute::Unrouted
        );
    }

    #[test]
    fn forge_snapshot_retains_merge_hold_label() {
        let snapshot = parse_open_pr_snapshot(
            r#"[{"number":2035,"title":"broken","headRefName":"broken-pr","labels":[{"name":"aida:merge-hold"}],"statusCheckRollup":[]}]"#,
            None,
        );
        assert_eq!(
            snapshot.by_branch["broken-pr"].labels,
            vec!["aida:merge-hold"]
        );
    }
}

/// Complete inventory for the review-story creation gate only. Bulk readers
/// intentionally tolerate bad objects; absence here must not use that contract.
// trace:BUG-1807 | ai:codex
pub(crate) fn load_review_story_inventory(
    project_root: &std::path::Path,
) -> Result<RequirementsStore> {
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        let objects = store_path.join("objects");
        let mut store = aida_core::GitBackend::read_metadata_only(&store_path)?;
        store.requirements = aida_core::object_store::list_objects_strict(&objects)?
            .into_iter()
            .map(|(_, path)| aida_core::object_store::read_object_from_path(&path))
            .collect::<Result<Vec<_>>>()?;
        return Ok(store);
    }

    // The normal resolver is best effort. Before allowing legacy resolution,
    // distinguish absent config from unreadable config or unavailable storage.
    for root in project_root.ancestors() {
        if aida_core::store_locate::is_system_temp_dir(root) {
            break;
        }
        let path = root.join(".aida/config.toml");
        match std::fs::read_to_string(&path) {
            Ok(config) => {
                toml::from_str::<toml::Table>(&config)
                    .with_context(|| format!("Cannot parse {}", path.display()))?;
                anyhow::ensure!(
                    !config_declares_distributed(&config)
                        && aida_core::store_locate::store_path_candidates(&config).is_empty(),
                    "Configured canonical review-story storage is unavailable at {}",
                    path.display()
                );
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    let path = aida_core::resolve_requirements_path_in(
        project_root,
        None,
        None,
        std::path::Path::new(""),
    )?;
    Storage::new(path).load()
}

pub(crate) fn canonical_review_story<'a>(
    store: &'a RequirementsStore,
    forge: ReviewForge,
    number: u64,
    covered_specs: Option<&[String]>,
    preferred_spec: Option<&str>,
) -> Option<&'a aida_core::Requirement> {
    let expected: std::collections::BTreeSet<String> = covered_specs
        .unwrap_or_default()
        .iter()
        .map(|id| id.to_ascii_uppercase())
        .collect();
    let covered = |req: &aida_core::Requirement| {
        req.relationships
            .iter()
            .filter(|rel| rel.rel_type.to_string().eq_ignore_ascii_case("implements"))
            .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
            .filter_map(|r| r.spec_id.as_deref().or(r.agreed_id.as_deref()))
            .map(|id| id.to_ascii_uppercase())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut candidates: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            !is_terminal_status(&req.status) && review_title_matches(&req.title, forge, number)
        })
        .collect();
    candidates.sort_by_key(|req| {
        let coverage_rank = if covered_specs.is_some() && covered(req) != expected {
            1
        } else {
            0
        };
        let display = req.display_id();
        let preferred_rank = if preferred_spec.is_some_and(|id| id.eq_ignore_ascii_case(&display)) {
            0
        } else {
            1
        };
        (coverage_rank, preferred_rank, display.to_ascii_uppercase())
    });
    candidates.into_iter().next()
}

/// Strip ANSI SGR sequences (`ESC[...m`) so we can match output text
/// regardless of whether the child invocation colored its output. Keep it
/// minimal — we only need the SGR shape `aida add` emits.
/// trace:STORY-66 | ai:claude
pub(crate) fn strip_ansi_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for cc in chars.by_ref() {
                if cc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Self-invoke `aida add` for a Story-typed review requirement and parse the
/// resulting spec id out of stdout. Returns None on any failure.
/// trace:STORY-66 | ai:claude
pub(crate) fn aida_subcmd_add_review_story(
    project_root: &std::path::Path,
    title: &str,
    description: &str,
) -> Option<String> {
    let aida = aida_exe_path();
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--priority",
            "medium",
            "--title",
            title,
            "--description",
            description,
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} auto-queue review story: `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_spec_id_from_add_output(&stdout)
}

/// Parse the spec id printed by `aida add`. The git-canonical path prints
/// `Added: STORY-N - <title>` (one line); the legacy YAML/SQLite path
/// prints a standalone `ID: STORY-N` line. Accept either so this hook keeps
/// working across backends. trace:STORY-66 | ai:claude
pub(crate) fn parse_spec_id_from_add_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let cleaned = strip_ansi_color(line);
        let candidate = cleaned
            .strip_prefix("Added: ")
            .map(|rest| rest.split(" - ").next().unwrap_or("").trim().to_string())
            .or_else(|| {
                cleaned
                    .strip_prefix("ID: ")
                    .map(|rest| rest.trim().to_string())
            });
        if let Some(c) = candidate {
            if !c.is_empty() {
                return Some(c);
            }
        }
    }
    None
}

/// Extract a trailing `(#N)` PR-number suffix from a commit subject (the
/// shape `gh` writes when squash-merging a PR). Returns None when the
/// subject doesn't end with `(#<digits>)`. trace:BUG-102 | ai:claude
pub(crate) fn extract_pr_number_from_commit_subject(message: &str) -> Option<u64> {
    let subject = message.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = subject.trim();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open_at = trimmed.rfind('(')?;
    let inner = &trimmed[open_at + 1..trimmed.len() - 1];
    let digits = inner.strip_prefix('#')?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// True when `subject` references any of `ids` as a real spec-completion
/// reference — i.e. the id appears in the commit's trailing `(REQ-ID)` group
/// or leading `SPEC-ID:` prefix, the same shapes the auto-bump scan honours
/// (`extract_spec_ids_from_commit`). Plan commits (`docs(plans): …`) are NOT a
/// completion signal, so they never count even when they name the id.
///
/// This is the testable core of TASK-579: when a finding is promoted, its
/// origin-ID may already have a merged fix referencing it (the fix shipped
/// against the id the finding carried *before* it became real work). Matching
/// is case-insensitive so `task-1-097` in a subject matches `TASK-1-097`.
/// trace:TASK-579 | ai:claude
pub(crate) fn commit_subject_references_id(subject: &str, ids: &[String]) -> bool {
    if is_plan_commit_subject(subject) {
        return false;
    }
    let referenced = extract_spec_ids_from_commit(subject);
    ids.iter().any(|wanted| {
        referenced
            .iter()
            .any(|got| got.eq_ignore_ascii_case(wanted))
    })
}

/// Scan the default branch's recent git log for a merged commit that references
/// any of `ids` (a finding's spec-id + agreed-id). Returns the first matching
/// `(short_sha, subject)` — first as in newest-first git-log order, so the most
/// recent landing wins. Returns `None` when no merged commit references the ids,
/// when there's no resolvable default branch, or when git is unavailable.
///
/// Used by `aida findings promote` so a finding whose underlying fix already
/// shipped against its origin-ID can warn / auto-complete instead of being
/// queued as fresh (no-op) work. trace:TASK-579 | ai:claude
pub(crate) fn find_merged_commit_referencing_ids(
    project_root: &std::path::Path,
    ids: &[String],
) -> Option<(String, String)> {
    use std::process::Command as ProcessCommand;

    if ids.is_empty() {
        return None;
    }

    // Resolve a default-branch ref to scan. Reuse the shared resolver so this
    // matches `aida db reconcile-status` / auto-bump branch semantics: prefer
    // origin/HEAD, fall back to origin/main|master then local main|master.
    let branch_ref = resolve_default_branch_ref(project_root)?;

    let log_out = ProcessCommand::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "--max-count=500",
            "--pretty=format:%h%x09%s",
            &branch_ref,
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let log_str = String::from_utf8_lossy(&log_out.stdout);

    for line in log_str.lines() {
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or("").trim();
        let subject = parts.next().unwrap_or("").trim();
        if sha.is_empty() || subject.is_empty() {
            continue;
        }
        if commit_subject_references_id(subject, ids) {
            return Some((sha.to_string(), subject.to_string()));
        }
    }
    None
}

/// Parse the PR number out of a review-story title. Titles are filed by
/// /aida-pr's auto-queue as `Review PR-<n>: <pr-title>` (see
/// `aida_subcmd_add_review_story` callsite); anything not matching that
/// shape returns None. trace:BUG-102 | ai:claude
pub(crate) fn parse_review_story_pr_number(title: &str) -> Option<u64> {
    let rest = title.strip_prefix("Review PR-")?;
    let (num, _) = rest.split_once(':')?;
    num.trim().parse::<u64>().ok()
}

/// Pull spec-id-shaped `(REQ-ID)` parens from BODY lines of a commit message
/// (i.e. everything after the subject). These are informational
/// "referenced" specs — the commit's code touches their areas but doesn't
/// deliver them per AIDA's "one (REQ-ID) trailer in the subject" convention.
/// IDs already present in the subject are excluded so the two lists are
/// disjoint. trace:BUG-85 | ai:claude
/// BUG-412: heuristic — does a commit-body line look like CODE rather than a
/// prose/trace reference? Used to skip pasted snippets so `(PREFIX-NNN)` literals
/// inside them aren't mined as bogus "referenced" specs. Conservative: only the
/// markers that strongly imply code and don't appear in genuine reference lines
/// (`trace:SPEC-ID`, `(SPEC-ID)` in prose, `- (SPEC-ID) …` bullets).
///
/// BUG-1590: a bare `t.contains(';')` was WAY too broad — AIDA's own
/// integration-batch commit convention writes each folded spec as
/// `- SPEC-ID: <clause>; <clause> (SPEC-ID)`, a prose sentence with a
/// mid-line semicolon joining two clauses, terminated by the completion
/// trailer. That line contains a `;` but is not code, and the old check
/// silently discarded it from the squash-body scan — most lines of a
/// multi-spec squash body were dropped, exactly the shape the integration
/// workflow produces (observed: only 1-2 of 6-7 trailers survived per
/// batch). Real pasted-code semicolons are STATEMENT TERMINATORS — the
/// line ends in `;` (e.g. `let x = compute(CODE-42);`) — so the signal is
/// narrowed to that shape: `ends_with(';')`, not `contains(';')`. A line
/// that merely mentions a semicolon mid-sentence before its trailing
/// `(SPEC-ID)` still counts as a trailer.
// trace:BUG-1590 | ai:claude
pub(crate) fn body_line_is_code_like(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // Structural code punctuation that prose references don't use. `;` only
    // counts when it TERMINATES the line (a real code statement), not when
    // it merely appears mid-sentence ahead of a trailing `(SPEC-ID)`.
    if t.contains('{')
        || t.contains('}')
        || t.ends_with(';')
        || t.contains("=>")
        || t.contains("::")
    {
        return true;
    }
    // An assignment `=` (but not `==`/`!=`/`<=`/`>=` comparisons or `(SPEC-ID)`
    // prose). A lone `=` surrounded by spaces, or `x =`, signals code.
    if t.contains(" = ") || t.contains("=(") || t.contains(")=") {
        return true;
    }
    // Leading code keywords.
    const KW: [&str; 9] = [
        "fn ", "pub ", "const ", "let ", "enum ", "struct ", "impl ", "use ", "static ",
    ];
    KW.iter().any(|k| t.starts_with(k))
}

/// BUG-546: harvest every spec-id named in a `trace:SPEC-ID` token anywhere in
/// a commit message — the authoritative provenance marker AIDA writes in code
/// comments and commit bodies. The `(SPEC-ID)` paren parser misses these; when
/// a PR's subject paren is non-standard (`(A / B slice 1a)`) the body's
/// `trace:` lines are the reliable spec↔PR link. Handles `trace:` and
/// `trace: ` (with/without a space), comma/whitespace-separated runs
/// (`trace:TASK-800 trace:STORY-610` or `trace:TASK-800,STORY-610`), and the
/// `relates:`/`blocks:` qualifier forms (`trace:relates:STORY-611`) — the
/// trailing `<ALPHA>-<DIGITS>` token is taken in each case. trace:BUG-546
pub(crate) fn extract_trace_line_spec_ids(message: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in message.split("trace:").skip(1) {
        // Consume consecutive spec-id-shaped tokens after this `trace:`,
        // tolerating `relates:`/`blocks:`/etc. qualifier prefixes whose final
        // colon-segment is the id.
        for tok in raw.split(|c: char| c.is_whitespace() || c == ',') {
            let tok = tok.trim().trim_end_matches([')', '.', ';', ':']);
            // A qualifier like `relates:STORY-611` → take the last segment.
            let candidate = tok.rsplit(':').next().unwrap_or(tok);
            if looks_like_spec_id(candidate) {
                if !out.iter().any(|x| x.eq_ignore_ascii_case(candidate)) {
                    out.push(candidate.to_string());
                }
            } else if tok.is_empty() {
                continue;
            } else {
                // First non-id token after a `trace:` ends this run — the rest
                // is prose (`trace:BUG-546 | ai:claude` stops at `|`).
                break;
            }
        }
    }
    out
}

/// Which commit-body SHAPES count as a completion trailer:
/// every non-blank, non-subject body line whose TRAILING paren group
/// starts with a spec-id-shaped token — `- SPEC-ID: <prose>; <more prose>
/// (SPEC-ID)`, `* [AI:tool] fix(scope): thing (SPEC-ID)`, or bare
/// `<prose> (SPEC-ID)` — counts, one trailer per line, regardless of
/// interior punctuation (a mid-sentence `;` does NOT disqualify a line;
/// only a line that structurally looks like pasted code does — see
/// `body_line_is_code_like`). A line whose trailing group does not START
/// with a spec-id (release-note prose like `(scope)`, `(1.2.3)`) or a
/// code-like line (`{`/`}`/line-terminating `;`/`=>`/`::`/assignment `=`)
/// contributes nothing. Ids already delivered by the commit's SUBJECT
/// trailer are excluded (BUG-85) so a lead spec named in both places is
/// not double-reported.
// trace:BUG-1590 | ai:claude
pub(crate) fn extract_referenced_spec_ids_from_commit(message: &str) -> Vec<String> {
    let delivered = extract_spec_ids_from_commit(message);
    let mut lines = message.lines();
    // Skip blank prefix + subject line.
    let _subject = lines.by_ref().find(|l| !l.trim().is_empty());
    let mut raw: Vec<String> = Vec::new();
    for line in lines {
        // BUG-412: skip code-like body lines so a `(PREFIX-NNN)` literal inside a
        // pasted code snippet (e.g. `enum Status { OK = (STATUS-200) }`) isn't
        // mined as a bogus "referenced" spec on the auto-filed review story.
        // Genuine reference lines (trace:, prose, bullet lists) don't carry these
        // code markers. trace:BUG-412 | ai:claude
        if body_line_is_code_like(line) {
            continue;
        }
        push_paren_spec_ids_from_line(line, &mut raw);
    }
    let mut out: Vec<String> = Vec::new();
    for id in raw {
        if delivered.iter().any(|d| d.eq_ignore_ascii_case(&id)) {
            continue;
        }
        if out.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
            continue;
        }
        out.push(id);
    }
    out
}

/// Walk one commit-message line for trailing `(REQ-ID[, REQ-ID...])`
/// groups and push the spec-id-shaped tokens into `out`. Skips `(#N)`
/// PR-number groups (squash commits). A group whose tokens aren't ALL
/// spec-id-shaped contributes nothing and stops the walk, so prose-y
/// parens like `(1.2.3)`, `(foo BUG-23)`, or a conventional-commit
/// `(scope)` don't false-match.
///
/// Collects EVERY consecutive trailing spec-id group, not just the last
/// one — a cluster-PR squash subject like `fix: x (BUG-503) (BUG-504)
/// (#794)` names one group per shipped spec, and both the `aida pull`
/// auto-bump scan and `aida db reconcile-status` must graduate each.
/// IDs are pushed in left-to-right subject order.
/// trace:BUG-78 BUG-85 | ai:claude
// trace:BUG-506 | ai:claude
pub(crate) fn push_paren_spec_ids_from_line(line: &str, out: &mut Vec<String>) {
    let mut tail: &str = line.trim();
    let mut collected: Vec<String> = Vec::new();
    while tail.ends_with(')') {
        let Some(open_at) = tail.rfind('(') else {
            break;
        };
        let inner = &tail[open_at + 1..tail.len() - 1];
        // `(#N)` PR-number group (squash commits): skip it and keep walking.
        if inner.starts_with('#')
            && inner.len() > 1
            && inner[1..].chars().all(|c| c.is_ascii_digit())
        {
            tail = tail[..open_at].trim_end();
            continue;
        }
        // Parse this group. BUG-546: a real trailer may carry MULTIPLE specs
        // plus prose — `(TASK-800 / STORY-610 slice 1a)` — and the old
        // all-tokens-must-be-ids rule extracted NEITHER spec, stranding the
        // auto-bump / reconcile / review surfaces that share this parser. The
        // discriminator that still rejects release-note prose like
        // `(foo BUG-23)` or `(scope)`: the group must START with a spec-id
        // token (the `(SPEC-ID …)` trailer convention). When it does, harvest
        // EVERY spec-id-shaped token and skip the separators / prose
        // (`/`, `slice`, `1a`, …); when it doesn't, the group contributes
        // nothing and the walk stops, exactly as before. trace:BUG-546
        let mut group: Vec<String> = Vec::new();
        let mut first_token_is_id: Option<bool> = None;
        for tok in inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()) {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            let is_id = looks_like_spec_id(tok);
            if first_token_is_id.is_none() {
                first_token_is_id = Some(is_id);
            }
            if is_id {
                group.push(tok.to_string());
            }
        }
        // Reject the group unless it led with a spec-id (so `(foo BUG-23)`,
        // `(scope)`, `(1.2.3)` still contribute nothing and stop the walk).
        if first_token_is_id != Some(true) || group.is_empty() {
            break;
        }
        // Prepend: we walk right-to-left, but `out` keeps subject order.
        collected.splice(0..0, group);
        tail = tail[..open_at].trim_end();
    }
    out.extend(collected);
}

pub(crate) fn looks_like_spec_id(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 || s.len() > 40 {
        return false;
    }
    let mut parts = s.split('-');
    let Some(prefix) = parts.next() else {
        return false;
    };
    if prefix.len() < 2 || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let tail: Vec<&str> = parts.collect();
    if tail.is_empty() {
        return false;
    }
    if !tail
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return false;
    }
    tail.last()
        .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
}

pub fn parse_pr_arg(arg: &str) -> Option<u32> {
    let s = arg.trim();
    if s.is_empty() {
        return None;
    }
    let s_lower = s.to_lowercase();
    let digits = if let Some(rest) = s_lower.strip_prefix("pr-") {
        rest
    } else if let Some(rest) = s_lower.strip_prefix("pr") {
        rest
    } else if let Some(rest) = s_lower.strip_prefix('#') {
        rest
    } else {
        s
    };
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        digits.parse::<u32>().ok()
    } else {
        None
    }
}

pub(crate) fn extract_all_spec_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let split_chars = |c: char| {
        c.is_whitespace()
            || c == '('
            || c == ')'
            || c == '['
            || c == ']'
            || c == '{'
            || c == '}'
            || c == ','
            || c == '.'
            || c == ':'
            || c == ';'
            || c == '/'
            || c == '\\'
            || c == '"'
            || c == '\''
    };
    for word in text.split(split_chars) {
        let trimmed = word.trim();
        if looks_like_spec_id(trimmed) {
            let canon = trimmed.to_uppercase();
            if !canon.starts_with("PR-") && !out.contains(&canon) {
                out.push(canon);
            }
        }
    }
    for id in extract_spec_ids_from_commit(text) {
        let canon = id.to_uppercase();
        if !canon.starts_with("PR-") && !out.contains(&canon) {
            out.push(canon);
        }
    }
    out
}

/// Is `ancestor` a transitive parent of `node`, per a `child → parent uuids`
/// map? trace:BUG-431 | ai:claude
pub(crate) fn is_transitive_ancestor(
    parents: &std::collections::HashMap<uuid::Uuid, Vec<uuid::Uuid>>,
    ancestor: uuid::Uuid,
    node: uuid::Uuid,
) -> bool {
    let mut stack: Vec<uuid::Uuid> = parents.get(&node).cloned().unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    while let Some(p) = stack.pop() {
        if p == ancestor {
            return true;
        }
        if seen.insert(p) {
            if let Some(gps) = parents.get(&p) {
                stack.extend(gps.iter().copied());
            }
        }
    }
    false
}

/// BUG-431 #2: when a PR backs several specs that form a parent chain (an epic
/// and its child story, say), the most-specific one IS the answer — the
/// reviewer must not bail "multiple backing specs." Drop any spec in the set
/// that is a proven transitive ANCESTOR of another spec in the set; keep the
/// rest. Specs absent from the store, or genuinely unrelated to the others,
/// are left intact, so the caller still bails on real ambiguity. Order-
/// preserving. trace:BUG-431 | ai:claude
pub(crate) fn reduce_to_most_specific_specs(
    store: &RequirementsStore,
    specs: &[String],
) -> Vec<String> {
    if specs.len() < 2 {
        return specs.to_vec();
    }
    let uuid_of = |s: &str| -> Option<uuid::Uuid> {
        store
            .requirements
            .iter()
            .find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(s))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(s))
            })
            .map(|r| r.id)
    };
    // child uuid → its parent uuids. RelationshipType::Parent means "this is
    // parent of target" (→ target's parent is this req); Child means "this is
    // child of target" (→ this req's parent is target). trace:BUG-431
    let mut parents: std::collections::HashMap<uuid::Uuid, Vec<uuid::Uuid>> =
        std::collections::HashMap::new();
    for req in &store.requirements {
        for rel in &req.relationships {
            match rel.rel_type {
                aida_core::RelationshipType::Parent => {
                    parents.entry(rel.target_id).or_default().push(req.id);
                }
                aida_core::RelationshipType::Child => {
                    parents.entry(req.id).or_default().push(rel.target_id);
                }
                _ => {}
            }
        }
    }
    let resolved: Vec<(String, Option<uuid::Uuid>)> =
        specs.iter().map(|s| (s.clone(), uuid_of(s))).collect();
    let mut out = Vec::new();
    for s in specs {
        let keep = match uuid_of(s) {
            None => true, // unresolved → can't prove ancestry → keep
            Some(a) => !resolved.iter().any(|(other, ou)| {
                other != s && ou.is_some_and(|ou| is_transitive_ancestor(&parents, a, ou))
            }),
        };
        if keep {
            out.push(s.clone());
        }
    }
    out
}

/// STORY-501 / BUG-440: is a non-terminal "Review PR-N" story currently queued
/// for this user? When one is, `aida queue work PR-N` must NOT be resolved to
/// the PR's backing spec at the dispatch (TASK-518) — it must reach
/// `resolve_queue_work_plan`'s review-story pickup (TASK-85) so it routes to the
/// reviewer (`/aida-review` on a PR-scoped lease) instead of an implementer
/// pickup that re-implements the spec. trace:STORY-501 | ai:claude
// BUG-1195: what looking up a pickable review story for one PR/MR found.
/// The variants are the diagnosis: a caller can tell "no story" (a genuine
/// gap) from "the lookup itself failed" (a transient store/queue read), and
/// the bail message names which check fell through.
// trace:BUG-1195 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewStoryLookup {
    /// A queued, pickable `Review PR-n:` story (its display id).
    Found(String),
    /// Nothing queued for this user or the reviewer role at all.
    NoEntries,
    /// Entries exist but none is a review story for this PR.
    NoTitleMatch { entries: usize },
    /// The review story exists but is not pickable (Done / terminal / blocked).
    NotPickable { story: String, reason: String },
    /// The queue or store could not be read — NOT evidence of a missing story.
    Failed(String),
}

impl ReviewStoryLookup {
    pub(crate) fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Found(id) => format!("review story {id} is queued and pickable"),
            Self::NoEntries => "no queue entries for this user or the reviewer role".to_string(),
            Self::NoTitleMatch { entries } => format!(
                "{entries} queue entr{} but none is a review story for this change",
                if *entries == 1 { "y" } else { "ies" }
            ),
            Self::NotPickable { story, reason } => {
                format!("review story {story} is queued but not pickable: {reason}")
            }
            Self::Failed(cause) => format!("the review-story lookup failed: {cause}"),
        }
    }
}

// BUG-1195: pure classification over the entries the caller could see.
// trace:BUG-1195 | ai:claude
pub(crate) fn classify_review_story_lookup(
    entries: &[aida_core::QueueEntry],
    store: &RequirementsStore,
    forge: ReviewForge,
    n: u64,
) -> ReviewStoryLookup {
    if entries.is_empty() {
        return ReviewStoryLookup::NoEntries;
    }
    let mut not_pickable: Option<(String, String)> = None;
    for e in entries {
        let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
            continue;
        };
        if !review_title_matches(&req.title, forge, n) {
            continue;
        }
        // trace:BUG-1515 | ai:claude
        // No filesystem access here — this classifier stays PURE (BUG-1195),
        // so the Done/AwaitingRework distinction (which reads a verdict file)
        // is unavailable; it degrades to the pre-BUG-1515 AwaitingMerge
        // reading rather than doing I/O from a pure function.
        let policy = queue_cmd::queue_fresh_pickup_policy(req, store, false, None);
        if matches!(policy, queue_cmd::QueueFreshPickup::Pickable) {
            return ReviewStoryLookup::Found(req.display_id());
        }
        if not_pickable.is_none() {
            not_pickable = Some((
                req.display_id(),
                queue_cmd::queue_fresh_pickup_reason_label(&policy)
                    .unwrap_or_else(|| format!("{policy:?}")),
            ));
        }
    }
    match not_pickable {
        Some((story, reason)) => ReviewStoryLookup::NotPickable { story, reason },
        None => ReviewStoryLookup::NoTitleMatch {
            entries: entries.len(),
        },
    }
}

/// Is a pickable `Review PR-n:` story queued for this user OR routed to the
// reviewer role by any user? BUG-1195: reads the queue through the same
/// role-fallback the plan-builder uses (`queue_list_with_role_fallback`), so
/// a story filed by a sibling worktree session under a different user id —
/// the BUG-1193 shape — is found here exactly as `queue work <STORY>` finds
/// it. Read failures surface as `Failed`, never as "no story".
// trace:BUG-1195 | ai:claude
pub(crate) fn queued_review_story_for_pr(
    storage: &Storage,
    user_id: &str,
    forge: ReviewForge,
    n: u64,
) -> ReviewStoryLookup {
    let entries = match queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        Some("reviewer"),
        /* include_completed */ false,
    ) {
        Ok(e) => e,
        Err(e) => return ReviewStoryLookup::Failed(format!("queue read: {e:#}")),
    };
    let store = match storage.load() {
        Ok(s) => s,
        Err(e) => return ReviewStoryLookup::Failed(format!("store load: {e:#}")),
    };
    classify_review_story_lookup(&entries, &store, forge, n)
}

/// BUG-440: outcome of choosing a single review target from a PR's spec sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrSpecChoice {
    One(String),
    None,
    Ambiguous(Vec<String>),
}

/// BUG-440: pick the single spec a PR should be reviewed against, given the
/// DELIVERED specs (subject `(SPEC-ID)` trailers + review-story covers
/// relationships) and the broader REFERENCED specs (anything spec-id-shaped in
/// titles/bodies/trace lines). Delivered wins outright when non-empty, so a PR
/// that delivers one spec while merely *tracing* others resolves cleanly
/// instead of bailing on false ambiguity (the STORY-501 / TASK-642 case). Only
/// a genuinely ambiguous *delivered* set — or, absent any delivery signal, an
/// ambiguous referenced set — is reported as ambiguous. Both inputs are assumed
/// already reduced to most-specific. Pure, so the precedence is unit-testable
/// without `gh`. trace:BUG-440 | ai:claude
pub(crate) fn pick_pr_spec(delivered: &[String], referenced: &[String]) -> PrSpecChoice {
    let pool = if !delivered.is_empty() {
        delivered
    } else {
        referenced
    };
    match pool.len() {
        0 => PrSpecChoice::None,
        1 => PrSpecChoice::One(pool[0].clone()),
        _ => PrSpecChoice::Ambiguous(pool.to_vec()),
    }
}

pub fn resolve_pr_to_spec(
    project_root: &std::path::Path,
    pr: u32,
    store: &RequirementsStore,
) -> Result<String> {
    // BUG-440: keep DELIVERED specs (the authoritative "this PR ships X" signal:
    // a review story's covers/implements relationships + the `(SPEC-ID)` subject
    // trailers) separate from REFERENCED specs (the broad net: any spec-id-shaped
    // token in titles/descriptions/trace lines). A PR can legitimately deliver
    // one spec while tracing others; preferring delivered avoids a false
    // "multiple backing specs" bail. trace:BUG-440 | ai:claude
    let mut delivered: Vec<String> = Vec::new();
    let mut referenced: Vec<String> = Vec::new();
    let push_unique = |v: &mut Vec<String>, id: String| {
        if !v.contains(&id) {
            v.push(id);
        }
    };

    for req in &store.requirements {
        if let Some(num) = parse_review_story_pr_number(&req.title) {
            if num == pr as u64 {
                // covers/implements relationships = authoritative delivery link.
                for rel in &req.relationships {
                    if let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id)
                    {
                        if let Some(spec_id) = &target.spec_id {
                            let canon = spec_id.to_uppercase();
                            if !canon.starts_with("PR-") {
                                push_unique(&mut delivered, canon);
                            }
                        }
                    }
                }
                // The title carries the PR's `(SPEC-ID)` delivery trailer; the
                // free-text description is referenced-only (stray trace refs).
                for id in extract_spec_ids_from_commit(&req.title) {
                    push_unique(&mut delivered, id);
                }
                for id in extract_all_spec_ids(&req.title) {
                    push_unique(&mut referenced, id);
                }
                for id in extract_all_spec_ids(&req.description) {
                    push_unique(&mut referenced, id);
                }
            }
        }
    }

    if let Some(gh) = resolve_gh_binary() {
        let pr_str = pr.to_string();
        let mut c = std::process::Command::new(&gh);
        c.current_dir(project_root).args([
            "pr",
            "view",
            &pr_str,
            "--json",
            "title,commits",
            "-q",
            "[.title, (.commits[].messageHeadline)] | @tsv",
        ]);
        if let Ok(out) = c.output_retrying_etxtbsy() {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let fields = stdout.trim_end().split('\t');
                for field in fields {
                    // Delivered = the trailing `(SPEC-ID)` / leading `SPEC-ID:`
                    // group only; everything spec-id-shaped goes to referenced.
                    for id in extract_spec_ids_from_commit(field) {
                        push_unique(&mut delivered, id);
                    }
                    for id in extract_all_spec_ids(field) {
                        push_unique(&mut referenced, id);
                    }
                }
            }
        }
    }

    // Referenced is the fallback pool only — drop anything already delivered.
    referenced.retain(|r| !delivered.contains(r));

    // BUG-431 #2: a PR can legitimately back an epic + its child story (the
    // session was epic-scoped). Reduce each pool to the most-specific before
    // deciding — drop proven ancestors so epic+child collapses to the child.
    // Genuinely unrelated specs survive and still bail below.
    // trace:BUG-431 | ai:claude
    let delivered = reduce_to_most_specific_specs(store, &delivered);
    let referenced = reduce_to_most_specific_specs(store, &referenced);

    match pick_pr_spec(&delivered, &referenced) {
        PrSpecChoice::One(s) => Ok(s),
        PrSpecChoice::None => anyhow::bail!(
            "PR-{} has no backing specs (could not find any associated spec IDs in review stories or PR metadata)",
            pr
        ),
        PrSpecChoice::Ambiguous(specs) => anyhow::bail!(
            "PR-{} has multiple backing specs: {}. Use a single SPEC-ID instead.",
            pr,
            specs.join(", ")
        ),
    }
}
