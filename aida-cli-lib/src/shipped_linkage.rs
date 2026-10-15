//! STORY-1649: the cheap "where did this spec ship?" read against the default
//! branch, and its two consumers — the one-line `linkage:` scalar on the TOON
//! `aida show` surface, and the warning `aida edit --status completed` prints
//! when a code spec is completed with no shipped commit.
//!
//! Deliberately NOT `collect_git_linkage`: that walks every ref (`git log
//! --all`, ~0.6s on a 70k-commit repo) and, for in-flight work, up to twenty
//! `git branch --contains` calls. The default TOON read path has a latency
//! contract (BUG-1480, BUG-1594), so this scan looks only at the default branch
//! refs — the remote-tracking `origin/<default>` first, then the local
//! `<default>` — which is one bounded `git log <ref> --grep` plus one
//! fixed-string `git grep` per ref (~65 ms + ~70 ms measured on this repo).
//!
//! The corroboration rule is the doctor's `completed-without-commit` rule
//! (TASK-673 / TASK-1089): a commit on the default branch whose message names
//! the spec (plan commits and `update SPEC` bookkeeping excluded, via the same
//! `referenced_spec_ids_from_messages` the scan uses), OR a tracked file at
//! that ref carrying a `trace:SPEC-ID` comment. Sharing the message filter is
//! what keeps the completion warning and `aida doctor` from disagreeing (AC5).
//!
//! Advisor amendment A1: `gh pr merge` lands on the remote first; the local
//! default branch is often stale when the agent runs `--status completed`.
//! Both refs are consulted, and when neither carries the spec the warning says
//! to `git fetch` rather than asserting nothing shipped.
// trace:STORY-1649 | ai:claude

use crate::*;

/// The default-branch refs a linkage scan consults, in scan order:
/// remote-tracking first (the merged-on-GitHub truth), local second.
// trace:STORY-1649 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DefaultBranchRefs {
    /// `origin/<default>`, when that remote-tracking ref exists.
    pub(crate) remote: Option<String>,
    /// Local `<default>`, when that branch exists.
    pub(crate) local: Option<String>,
}

impl DefaultBranchRefs {
    /// Scan order: remote-tracking ref, then local.
    pub(crate) fn in_scan_order(&self) -> Vec<String> {
        self.remote
            .iter()
            .chain(self.local.iter())
            .cloned()
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remote.is_none() && self.local.is_none()
    }
}

/// What the default branch says about one spec.
// trace:STORY-1649 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DefaultBranchLinkage {
    /// Every ref consulted, in scan order.
    pub(crate) scanned_refs: Vec<String>,
    /// The first ref that corroborated the spec (commit or trace comment).
    pub(crate) found_on: Option<String>,
    /// `(short_sha, subject)` of the referencing commits on `found_on`,
    /// newest first. Empty when nothing corroborated the spec.
    pub(crate) commits: Vec<(String, String)>,
    /// Distinct tracked files at `found_on` carrying a `trace:SPEC-ID` comment.
    pub(crate) trace_files: usize,
    /// PR number parsed from the newest squash-merge subject (`(#NN)`).
    pub(crate) shipped_pr: Option<u64>,
    /// BUG-1993: a git call ran past [`SHIPPED_SCAN_BUDGET`], so a missing
    /// corroboration means "unknown", not "not shipped".
    pub(crate) scan_incomplete: bool,
}

impl DefaultBranchLinkage {
    /// The doctor's corroboration rule: a referencing commit or a tracked
    /// trace comment on the default branch.
    pub(crate) fn corroborated(&self) -> bool {
        self.found_on.is_some()
    }
}

/// BUG-1993: the wall-clock budget for one whole default-branch scan (the
/// ancestor check plus a `git log` and a `git grep` per ref). Measured on this
/// repo (73k commits, 2026-10-14): ~60 ms per log, ~80 ms per grep, under
/// 10 ms for the ancestor check. The headroom is for a loaded host; past it
/// the scan reports itself incomplete instead of "not shipped".
// trace:BUG-1993 | ai:claude
pub(crate) const SHIPPED_SCAN_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// BUG-1993: the scan deadline passed before a git call finished.
#[derive(Debug)]
struct ScanTimedOut;

/// [`git_out`], bounded by the scan `deadline`. `Ok(None)` is a git error or
/// non-zero exit, as with `git_out`; `Err` means the deadline passed, either
/// before the spawn or while git ran.
// trace:BUG-1993 | ai:claude
fn git_out_until(
    project_root: &std::path::Path,
    args: &[&str],
    deadline: std::time::Instant,
) -> Result<Option<String>, ScanTimedOut> {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err(ScanTimedOut);
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(project_root).args(args);
    match command_output_with_timeout_detail(cmd, remaining) {
        BoundedCommandOutput::Completed(o) if o.status.success() => Ok(Some(
            String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        )),
        BoundedCommandOutput::TimedOut => Err(ScanTimedOut),
        _ => Ok(None),
    }
}

fn git_out(project_root: &std::path::Path, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
}

fn ref_exists(project_root: &std::path::Path, full_ref: &str) -> bool {
    git_out(
        project_root,
        &["rev-parse", "--verify", "--quiet", full_ref],
    )
    .is_some()
}

/// Resolve the default branch the way the doctor scan does
/// (`resolve_default_branch_ref`: `origin/HEAD`, then `init.defaultBranch`,
/// then `main`/`master`), then expand it to BOTH the remote-tracking and the
/// local form of that name, keeping only the refs that exist.
// trace:STORY-1649 | ai:claude
pub(crate) fn default_branch_refs(project_root: &std::path::Path) -> DefaultBranchRefs {
    let Some(resolved) = resolve_default_branch_ref(project_root) else {
        return DefaultBranchRefs::default();
    };
    let name = resolved
        .strip_prefix("origin/")
        .unwrap_or(resolved.as_str())
        .to_string();
    let remote_short = format!("origin/{name}");
    let remote =
        ref_exists(project_root, &format!("refs/remotes/{remote_short}")).then_some(remote_short);
    let local = ref_exists(project_root, &format!("refs/heads/{name}")).then_some(name);
    DefaultBranchRefs { remote, local }
}

/// Upper-cased spec ids named by trace tokens on one line, with any
/// criterion suffix (`.A1`) dropped — the same token shape the doctor's
/// `git grep -o` regex yields, so a criterion-suffixed trace comment
/// corroborates its spec here exactly as it does there.
// trace:STORY-1649 | ai:claude
fn trace_tokens(line: &str) -> Vec<String> {
    line.split("trace:")
        .skip(1)
        .filter_map(|rest| {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(rest.len());
            let token = &rest[..end];
            let first_alpha = token
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic());
            (first_alpha && token.contains('-')).then(|| token.to_ascii_uppercase())
        })
        .collect()
}

/// Referencing commits on `git_ref` — `(short_sha, subject)`, newest first —
/// by the doctor's message rule. The `-F --grep` is only a prefilter (it
/// also matches `TASK-10` for `TASK-1`); every candidate message is then run
/// through `referenced_spec_ids_from_messages`, which applies the exact-token
/// match and the plan-commit / bookkeeping exclusions the scan applies.
// trace:STORY-1649 | ai:claude
fn referencing_commits_on_ref(
    project_root: &std::path::Path,
    git_ref: &str,
    wanted: &HashSet<String>,
    deadline: std::time::Instant,
) -> Result<Vec<(String, String)>, ScanTimedOut> {
    let mut args: Vec<&str> = vec!["log", git_ref, "-F", "--pretty=format:%h%x1f%s%x1f%B%x1e"];
    for id in wanted {
        args.push("--grep");
        args.push(id.as_str());
    }
    // BUG-1958: `--` ends the revision list, so a tracked path named like
    // the branch (`main`, `origin/main`) cannot make git reject the ref as
    // ambiguous — a swallowed error here would read as "nothing shipped".
    // trace:BUG-1958 | ai:claude
    args.push("--");
    let Some(log) = git_out_until(project_root, &args, deadline)? else {
        return Ok(Vec::new());
    };
    Ok(log
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            if record.is_empty() {
                return None;
            }
            let mut parts = record.splitn(3, '\u{1f}');
            let short = parts.next()?.to_string();
            let subject = parts.next()?.to_string();
            let body = parts.next().unwrap_or("");
            let referenced = referenced_spec_ids_from_messages(std::iter::once(body));
            referenced
                .iter()
                .any(|id| wanted.contains(id))
                .then_some((short, subject))
        })
        .collect())
}

/// Distinct tracked files at `git_ref` whose content carries a trace token
/// for a wanted id (exact token: a longer id that merely shares the prefix,
/// like `TASK-10` for `TASK-1`, does not count).
// trace:STORY-1649 | ai:claude
fn trace_files_on_ref(
    project_root: &std::path::Path,
    git_ref: &str,
    wanted: &HashSet<String>,
    deadline: std::time::Instant,
) -> Result<usize, ScanTimedOut> {
    // BUG-1958: `-z` NUL-terminates the `<ref>:<path>` head so a path that
    // itself contains ':' parses intact, and `--` keeps a path named like the
    // branch from making the ref ambiguous. trace:BUG-1958 | ai:claude
    let mut args: Vec<String> = vec!["grep".into(), "-I".into(), "-F".into(), "-z".into()];
    for id in wanted {
        args.push("-e".into());
        args.push(format!("trace:{id}"));
    }
    args.push(git_ref.to_string());
    args.push("--".into());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(hits) = git_out_until(project_root, &arg_refs, deadline)? else {
        return Ok(0);
    };
    Ok(count_trace_hit_files(&hits, git_ref, wanted))
}

/// Distinct files in `git grep -z <ref>` output (`<ref>:<path>\0<line>` per
/// hit) whose matched line carries a trace token for a wanted id. Splitting
/// on the NUL, not the first ':', is what keeps a ':' in the path from being
/// read as the start of the content.
// trace:BUG-1958 | ai:claude
fn count_trace_hit_files(hits: &str, git_ref: &str, wanted: &HashSet<String>) -> usize {
    let prefix = format!("{git_ref}:");
    let mut files: HashSet<&str> = HashSet::new();
    for line in hits.lines() {
        let Some((head, content)) = line.split_once('\0') else {
            continue;
        };
        let Some(path) = head.strip_prefix(&prefix) else {
            continue;
        };
        if trace_tokens(content).iter().any(|t| wanted.contains(t)) {
            files.insert(path);
        }
    }
    files.len()
}

/// The refs worth scanning, in scan order. BUG-1958: when the local default
/// branch is the same commit as the remote-tracking ref, or an ancestor of
/// it, it is dropped, halving the cost of the common unshipped-spec read. A
/// local branch ahead of (or diverged from) the remote is still scanned.
///
/// For commit history the drop loses nothing: every commit on local is also
/// on the remote. The `git grep <ref>` trace scan is different: it reads the
/// tip tree only, so a `trace:` comment deleted upstream while local is
/// behind would have corroborated on local and now does not (shipped=false).
/// That is acceptable — the remote tip is the truthful current answer, and
/// local's stale tree only reflects an older state of the same history.
///
/// One `git merge-base --is-ancestor <local> <remote>` covers both cases (a
/// commit is its own ancestor). Fail-safe: any git error (missing ref, no
/// merge base, git unavailable, or the BUG-1993 `deadline` passing) reads as
/// "not covered", so both refs are scanned.
// trace:BUG-1958 | ai:claude
pub(crate) fn refs_worth_scanning(
    project_root: &std::path::Path,
    refs: &DefaultBranchRefs,
    deadline: std::time::Instant,
) -> Vec<String> {
    let (Some(remote), Some(local)) = (refs.remote.as_deref(), refs.local.as_deref()) else {
        return refs.in_scan_order();
    };
    // `git_out` is `Some` only on exit 0: exit 1 (not an ancestor) and any
    // error both come back `None`.
    let covered = matches!(
        git_out_until(
            project_root,
            &["merge-base", "--is-ancestor", local, remote],
            deadline,
        ),
        Ok(Some(_))
    );
    if covered {
        vec![remote.to_string()]
    } else {
        refs.in_scan_order()
    }
}

/// The cheap scan: consult the default-branch refs in order and stop at the
/// first one that corroborates any of `ids`. BUG-1993: the whole scan shares
/// one [`SHIPPED_SCAN_BUDGET`]; running out sets `scan_incomplete`.
// trace:STORY-1649 | ai:claude
pub(crate) fn scan_default_branch_linkage(
    project_root: &std::path::Path,
    ids: &[String],
) -> DefaultBranchLinkage {
    scan_default_branch_linkage_within(project_root, ids, SHIPPED_SCAN_BUDGET)
}

/// [`scan_default_branch_linkage`] with an explicit budget.
// trace:BUG-1993 | ai:claude
pub(crate) fn scan_default_branch_linkage_within(
    project_root: &std::path::Path,
    ids: &[String],
    budget: std::time::Duration,
) -> DefaultBranchLinkage {
    let deadline = std::time::Instant::now() + budget;
    // trace:BUG-1958 | ai:claude
    let refs = refs_worth_scanning(project_root, &default_branch_refs(project_root), deadline);
    let wanted: HashSet<String> = ids
        .iter()
        .map(|id| id.trim().to_ascii_uppercase())
        .filter(|id| !id.is_empty())
        .collect();
    let mut out = DefaultBranchLinkage {
        scanned_refs: refs.clone(),
        ..Default::default()
    };
    if wanted.is_empty() {
        return out;
    }
    for git_ref in refs {
        // BUG-1993: a timed-out log ends the scan unknown. A timed-out grep
        // after the log found commits still corroborates; only the file count
        // is then partial. trace:BUG-1993 | ai:claude
        let Ok(commits) = referencing_commits_on_ref(project_root, &git_ref, &wanted, deadline)
        else {
            out.scan_incomplete = true;
            break;
        };
        let trace_files = match trace_files_on_ref(project_root, &git_ref, &wanted, deadline) {
            Ok(n) => n,
            Err(ScanTimedOut) => {
                out.scan_incomplete = true;
                0
            }
        };
        if commits.is_empty() && trace_files == 0 {
            if out.scan_incomplete {
                break;
            }
            continue;
        }
        out.shipped_pr = commits
            .iter()
            .find_map(|(_, subject)| parse_squash_pr_number(subject));
        out.commits = commits;
        out.trace_files = trace_files;
        out.found_on = Some(git_ref);
        break;
    }
    out
}

/// The ids git may know a spec by: its display id plus the agreed / origin
/// ids, deduplicated — the same set `aida show --format json` scans.
// trace:STORY-1649 | ai:claude
pub(crate) fn spec_lookup_ids(req: &aida_core::Requirement) -> Vec<String> {
    let mut ids = vec![req.display_id()];
    for extra in [req.agreed_id.as_deref(), req.spec_id.as_deref()]
        .into_iter()
        .flatten()
    {
        if !ids.iter().any(|id| id == extra) {
            ids.push(extra.to_string());
        }
    }
    ids
}

/// Render the one-line TOON `linkage` value. Bare tokens only (no colon or
/// comma) so `toon::scalar` emits it unquoted and an agent can split on
/// whitespace.
// trace:STORY-1649 | ai:claude
pub(crate) fn render_toon_linkage(linkage: &DefaultBranchLinkage) -> String {
    if linkage.scanned_refs.is_empty() {
        return "unknown (no default branch found)".to_string();
    }
    // BUG-1993: an incomplete scan that found nothing is unknown, not false.
    // trace:BUG-1993 | ai:claude
    let shipped = if linkage.corroborated() {
        "true"
    } else if linkage.scan_incomplete {
        "unknown"
    } else {
        "false"
    };
    let mut parts: Vec<String> = vec![format!("shipped={shipped}")];
    if let Some(pr) = linkage.shipped_pr {
        parts.push(format!("shipped_pr={pr}"));
    }
    if let Some((short, _)) = linkage.commits.first() {
        parts.push(format!("commit={short}"));
    }
    parts.push(format!("commits={}", linkage.commits.len()));
    if linkage.scan_incomplete {
        parts.push("files=unknown".to_string());
        parts.push("reason=scan-timeout".to_string());
    } else {
        parts.push(format!("files={}", linkage.trace_files));
    }
    match linkage.found_on.as_deref() {
        Some(found) => parts.push(format!("on={found}")),
        None => parts.push(format!("checked={}", linkage.scanned_refs.join("+"))),
    }
    if !linkage.corroborated() {
        // BUG-1958: one whitespace-free token, so the line still splits on
        // whitespace. trace:BUG-1958 | ai:claude
        parts.push("detail=--format=json".to_string());
    }
    parts.join(" ")
}

/// The `linkage:` scalar for the TOON `aida show` surface (AC1). The caller
/// skips this entirely under `--no-git` (AC2).
// trace:STORY-1649 | ai:claude
pub(crate) fn toon_show_linkage_line(project_root: &std::path::Path, ids: &[String]) -> String {
    let linkage = scan_default_branch_linkage(project_root, ids);
    crate::toon::scalar("linkage", &render_toon_linkage(&linkage))
}

/// The tag that opts a Completed spec out of the completed-without-commit
/// rule — named in the warning so the agent knows how to silence it.
// trace:STORY-1649 | ai:claude
pub(crate) const COMPLETED_NO_CODE_OPT_OUT_TAG: &str = "doctor:no-code";

/// The warning text for a spec that just reached Completed while the default
/// branch carries no commit or trace comment for it — or `None` when the
/// rule does not apply (non-code type, `doctor:no-code` opt-out, no default
/// branch to check) or the spec is corroborated on either ref (AC3/AC4/A1).
///
/// Warn only, never refuse: work can legitimately complete outside this
/// repo. The text names the spec, the refs checked, the `git fetch` hint for
/// a merge that landed remotely, and the opt-out tag.
// trace:STORY-1649 | ai:claude
pub(crate) fn completion_linkage_warning(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) -> Option<String> {
    if !completed_without_commit_expects_code(&req.req_type) {
        return None;
    }
    if completed_without_commit_opted_out(&req.tags) {
        return None;
    }
    let ids = spec_lookup_ids(req);
    let linkage = scan_default_branch_linkage(project_root, &ids);
    if !linkage_warrants_warning(&linkage) {
        return None;
    }
    // trace:BUG-1958 | ai:claude
    Some(format_completion_linkage_warning(
        &req.display_id(),
        &linkage.scanned_refs,
        is_shallow_repository(project_root),
    ))
}

/// Whether a scan result supports "no shipped commit": there was a default
/// branch to check, nothing corroborated, and the scan finished. BUG-1993: a
/// timed-out scan cannot say nothing shipped, so it does not warn; `aida
/// doctor --category completed-without-commit` applies the rule unbounded.
// trace:BUG-1993 | ai:claude
fn linkage_warrants_warning(linkage: &DefaultBranchLinkage) -> bool {
    !linkage.scanned_refs.is_empty() && !linkage.corroborated() && !linkage.scan_incomplete
}

/// `git rev-parse --is-shallow-repository` — false when git says no or the
/// probe fails.
// trace:BUG-1958 | ai:claude
fn is_shallow_repository(project_root: &std::path::Path) -> bool {
    git_out(project_root, &["rev-parse", "--is-shallow-repository"]).as_deref() == Some("true")
}

/// The completion-warning text, pure so it is testable without a repo.
/// BUG-1958: in a shallow clone an old spec's shipping commit can sit below
/// the history cut, so the warning says "(shallow clone)" rather than
/// asserting nothing shipped.
// trace:BUG-1958 | ai:claude
pub(crate) fn format_completion_linkage_warning(
    spec_id: &str,
    scanned_refs: &[String],
    shallow: bool,
) -> String {
    let refs = scanned_refs.join(" or ");
    let mut text = format!(
        "{spec_id} is Completed, but no commit referencing it was found on {refs} \
         (and no tracked `trace:{spec_id}` comment). If its PR merged on the remote, \
         run `git fetch` and check `aida show {spec_id}`. If the work legitimately \
         landed outside this repo, opt out with `aida edit {spec_id} --add-tag \
         {COMPLETED_NO_CODE_OPT_OUT_TAG}`. `aida doctor --category \
         completed-without-commit` applies the same rule."
    );
    if shallow {
        text.push_str(
            " (shallow clone: older history is missing, so an earlier shipping commit \
             may not be visible; `git fetch --unshallow` to check)",
        );
    }
    text
}

#[cfg(test)]
thread_local! {
    static LAST_WARNING: std::cell::RefCell<Option<Option<String>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn take_last_warning() -> Option<Option<String>> {
    LAST_WARNING.with(|w| w.borrow_mut().take())
}

/// Print [`completion_linkage_warning`] to stderr, if any. Shared by the
/// canonical and legacy `aida edit` paths, `aida done` (BUG-1958), and
/// `aida queue advance` closing a parent (BUG-1996) so all say the same thing.
// trace:STORY-1649 | ai:claude
// trace:BUG-1996 | ai:antigravity
pub(crate) fn warn_if_completed_without_shipped_commit(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) -> Option<String> {
    let warning = completion_linkage_warning(project_root, req);
    #[cfg(test)]
    LAST_WARNING.with(|w| *w.borrow_mut() = Some(warning.clone()));
    if let Some(ref text) = warning {
        eprintln!("{} {text}", "Warning:".yellow().bold());
    }
    warning
}

#[cfg(test)]
#[path = "tests/story_1649_shipped_linkage_tests.rs"]
mod story_1649_shipped_linkage_tests;

#[cfg(test)]
#[path = "tests/bug_1958_shipped_linkage_tests.rs"]
mod bug_1958_shipped_linkage_tests;

#[cfg(test)]
#[path = "tests/bug_1993_shipped_scan_tests.rs"]
mod bug_1993_shipped_scan_tests;
