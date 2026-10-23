//! Attested-approval carry-over across a clean merge from the base branch
//! (TASK-1960, STORY-1690 slice 1; closes BUG-2177).
//!
//! An approval is recorded at one commit (`reviewed_sha`). When the PR's head
//! later moves ONLY because the base branch was merged into it, the content
//! the PR lands is unchanged, so the approval may carry over to the new head.
//! Two independent proofs must both pass; either failing refuses, exactly as
//! a moved head refuses today:
//!
//! 1. **Structure.** The new head descends from the approved commit, and
//!    every commit it adds that is not on the base branch is a two-parent
//!    merge: one parent on the PR side (the approved commit or another such
//!    merge), the other already on the base branch. Each merge's tree must be
//!    identical to git's own automatic merge of its parents
//!    (`git merge-tree --write-tree`), so a conflict resolution, a manual
//!    edit (an "evil merge"), or a commit hidden behind a merge refuses.
//! 2. **Patch identity.** The PR's own diff — `merge-base(base, head)..head`
//!    — has the same `git patch-id --verbatim` as the approved diff,
//!    `merge-base(base, approved)..approved`. `--verbatim` keeps whitespace,
//!    so an indentation change still counts as a content change.
//!
//! The base is the PROTECTED base: the remote's default branch as the
//! remote reports it (`git ls-remote --symref origin HEAD`), never the PR's
//! own base, which its author can retarget. A PR whose base is not that
//! branch does not carry over, and the merge re-reads the PR's base right
//! before merging and refuses if it no longer matches the proven base.
//!
//! A rebase (the head no longer descends from the approved commit) is
//! REFUSED with its own reason, even when its patch is identical: rewritten
//! history cannot be shown to contain nothing but base merges, and patch-id
//! alone ignores where in a file a hunk lands. Re-review the rebased head.
//!
//! The proof is recorded under `.aida/review-carryover/PR-<N>/<head>.json`.
//! The record is evidence, never authority: every gate re-computes the proof,
//! and a stored record that disagrees with the fresh proof is replaced.
// trace:TASK-1960 | ai:claude

use std::path::{Path, PathBuf};

/// The machine proof that an approval carries over to a new head.
// trace:TASK-1960 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CarryOver {
    pub pr: u64,
    /// The commit the approval was recorded at (full sha).
    pub approved_sha: String,
    /// The PR head the approval carries over to (full sha).
    pub new_head: String,
    /// The base branch ref the proof was computed against (`origin/main`).
    pub base_ref: String,
    /// That ref's tip when the proof ran.
    pub base_tip: String,
    /// `merge-base(base, approved)..approved`: the approved diff's range.
    pub approved_range: String,
    /// `merge-base(base, new_head)..new_head`: the new head's diff range.
    pub head_range: String,
    pub approved_patch_id: String,
    pub head_patch_id: String,
    /// The base merges between the approved commit and the new head.
    pub merge_commits: Vec<String>,
    pub recorded_at: String,
}

impl CarryOver {
    /// The parts of the proof that must not change between two runs for the
    /// same approved commit and head. The base tip and timestamp may.
    // trace:TASK-1960 | ai:claude
    fn same_proof(&self, other: &CarryOver) -> bool {
        self.pr == other.pr
            && self.approved_sha == other.approved_sha
            && self.new_head == other.new_head
            && self.base_ref == other.base_ref
            && self.approved_range == other.approved_range
            && self.head_range == other.head_range
            && self.approved_patch_id == other.approved_patch_id
            && self.head_patch_id == other.head_patch_id
            && self.merge_commits == other.merge_commits
    }
}

/// Why an approval does NOT carry over. Each variant names the reason the
/// refusal prints.
// trace:TASK-1960 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CarryOverRefusal {
    /// A commit or ref could not be resolved locally.
    Unresolvable { what: String },
    /// The head does not descend from the approved commit (a rebase or other
    /// rewritten history).
    NotDescendant,
    /// A commit after the approved one is not a merge.
    NotAMerge { commit: String },
    /// A merge's parents are not one PR-side commit plus one commit already
    /// on the base branch.
    NotABaseMerge { commit: String },
    /// A merge's tree differs from git's automatic merge of its parents.
    MergeEdited { commit: String },
    /// A merge's parents do not merge cleanly (a conflict was resolved).
    MergeConflicted { commit: String },
    /// A diff endpoint has more than one merge base with the base branch.
    AmbiguousBase { sha: String },
    /// The PR's own diff is empty, so there is nothing to compare.
    EmptyDiff,
    /// The PR's own diff changed.
    PatchChanged {
        approved_patch_id: String,
        head_patch_id: String,
    },
    /// The PR's base is not the remote's default branch, so the carry-over
    /// has no protected base to be proven against.
    BaseNotProtected { base: String, protected: String },
    /// git itself failed.
    Git { detail: String },
}

impl CarryOverRefusal {
    /// The reason, as a clause for the ship refusal.
    // trace:TASK-1960 | ai:claude
    pub(crate) fn reason(&self) -> String {
        let short = |s: &str| crate::review_verdict::short_sha(s).to_string();
        match self {
            Self::Unresolvable { what } => format!("{what} could not be resolved locally"),
            Self::NotDescendant => "the head does not descend from the approved commit (a rebase \
                 or rewritten history), so it cannot be shown to add only base merges"
                .to_string(),
            Self::NotAMerge { commit } => format!(
                "commit {} after the approved commit is not a merge from the base branch",
                short(commit)
            ),
            Self::NotABaseMerge { commit } => format!(
                "merge {} does not merge the base branch into the PR (one parent must be the PR, \
                 the other already on the base branch)",
                short(commit)
            ),
            Self::MergeEdited { commit } => format!(
                "merge {} differs from git's automatic merge of its parents (it carries changes \
                 of its own)",
                short(commit)
            ),
            Self::MergeConflicted { commit } => format!(
                "merge {} resolved a conflict, so the PR's content may have changed",
                short(commit)
            ),
            Self::AmbiguousBase { sha } => format!(
                "{} has more than one merge base with the base branch",
                short(sha)
            ),
            Self::EmptyDiff => "the PR's own diff is empty".to_string(),
            Self::PatchChanged {
                approved_patch_id,
                head_patch_id,
            } => format!(
                "the PR's own diff changed (patch-id {} at the approved commit, {} at the head)",
                short(approved_patch_id),
                short(head_patch_id)
            ),
            Self::BaseNotProtected { base, protected } => format!(
                "the PR targets `{base}`, not the repository's default branch `{protected}`, so \
                 there is no protected base to prove the merges against"
            ),
            Self::Git { detail } => format!("git failed: {detail}"),
        }
    }
}

/// Whether [`record_carry_over`] wrote a new record or found the same proof
/// already on disk.
// trace:TASK-1960 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordOutcome {
    Written,
    /// The same proof was already recorded, at this time.
    AlreadyRecorded {
        recorded_at: String,
    },
    /// A record existed but did not match the fresh proof; it was replaced.
    Replaced,
}

/// Where the carry-over record for `pr` at `new_head` lives, beside
/// `.aida/review-verdicts/` (a sibling, so nothing that walks the verdict
/// directory mistakes it for a verdict).
// trace:TASK-1960 | ai:claude
pub(crate) fn record_path(root: &Path, pr: u64, new_head: &str) -> PathBuf {
    root.join(".aida")
        .join("review-carryover")
        .join(format!("PR-{pr}"))
        .join(format!("{}.json", new_head.trim().to_ascii_lowercase()))
}

/// The stored record for `pr` at `new_head`, if one is present and readable.
// trace:TASK-1960 | ai:claude
pub(crate) fn read_record(root: &Path, pr: u64, new_head: &str) -> Option<CarryOver> {
    let body = std::fs::read_to_string(record_path(root, pr, new_head)).ok()?;
    serde_json::from_str(&body).ok()
}

/// Record `proof` under `root`. A stored record with the same proof is kept
/// (its original timestamp stands); a stored record that disagrees, or does
/// not parse, is replaced.
// trace:TASK-1960 | ai:claude
pub(crate) fn record_carry_over(root: &Path, proof: &CarryOver) -> std::io::Result<RecordOutcome> {
    let path = record_path(root, proof.pr, &proof.new_head);
    let existed = path.exists();
    if let Some(stored) = read_record(root, proof.pr, &proof.new_head) {
        if stored.same_proof(proof) {
            return Ok(RecordOutcome::AlreadyRecorded {
                recorded_at: stored.recorded_at,
            });
        }
    }
    let body = serde_json::to_string_pretty(proof).map_err(std::io::Error::other)?;
    crate::review_verdict::write_verdict_atomic(&path, &format!("{body}\n"))?;
    Ok(if existed {
        RecordOutcome::Replaced
    } else {
        RecordOutcome::Written
    })
}

/// The one line a gate prints when an approval carries over.
// trace:TASK-1960 | ai:claude
pub(crate) fn carry_over_line(proof: &CarryOver, outcome: &RecordOutcome) -> String {
    let short = |s: &str| crate::review_verdict::short_sha(s).to_string();
    let noted = match outcome {
        RecordOutcome::Written => String::new(),
        RecordOutcome::AlreadyRecorded { recorded_at } => format!("; recorded {recorded_at}"),
        RecordOutcome::Replaced => "; a stale record was replaced".to_string(),
    };
    format!(
        "approval carried over: PR-{} approved at {} covers head {} — {} clean merge(s) of {} and \
         the PR's own diff is unchanged (patch-id {}){noted}",
        proof.pr,
        short(&proof.approved_sha),
        short(&proof.new_head),
        proof.merge_commits.len(),
        proof.base_ref,
        short(&proof.head_patch_id),
    )
}

/// Where a merge gate may try a carry-over: the clone to compute the proof
/// in (and record it under), and the PR's base branch as the forge reports
/// it (`None` = read it from the forge). The proof itself is always computed
/// against the remote's default branch, never against the PR's base: a base
/// the author can retarget must not choose what "a merge of the base" means.
// trace:TASK-1960 | ai:claude
#[derive(Debug)]
pub(crate) struct CarryOverSite<'a> {
    pub repo: &'a Path,
    pub base_branch: Option<&'a str>,
    /// Prove only: write no record and let the caller print nothing. For a
    /// preflight pass ahead of the gate that pins the merge, so one ship
    /// records, prints and logs one carry-over.
    pub quiet: bool,
    /// The proof the last successful carry-over through this site applied,
    /// so the caller can re-check the PR's base right before it merges.
    pub proven: std::cell::RefCell<Option<CarryOver>>,
}

impl<'a> CarryOverSite<'a> {
    // trace:TASK-1960 | ai:claude
    pub(crate) fn new(repo: &'a Path, base_branch: Option<&'a str>) -> Self {
        Self {
            repo,
            base_branch,
            quiet: false,
            proven: std::cell::RefCell::new(None),
        }
    }

    // trace:TASK-1960 | ai:claude
    pub(crate) fn quiet(repo: &'a Path, base_branch: Option<&'a str>) -> Self {
        Self {
            quiet: true,
            ..Self::new(repo, base_branch)
        }
    }

    /// The base branch the applied proof is bound to, when one was applied.
    // trace:TASK-1960 | ai:claude
    pub(crate) fn proven_base(&self) -> Option<String> {
        self.proven.borrow().as_ref().map(CarryOver::base_branch)
    }
}

impl CarryOver {
    /// The branch name of [`CarryOver::base_ref`] (`origin/main` → `main`).
    // trace:TASK-1960 | ai:claude
    pub(crate) fn base_branch(&self) -> String {
        self.base_ref
            .strip_prefix("origin/")
            .unwrap_or(&self.base_ref)
            .to_string()
    }
}

/// The remote's default branch, as the remote itself reports it
/// (`git ls-remote --symref origin HEAD`) — not a local `origin/HEAD`,
/// which any session sharing the clone could repoint. Unreadable refuses.
// trace:TASK-1960 | ai:claude
pub(crate) fn protected_base(repo: &Path) -> Result<String, CarryOverRefusal> {
    let unresolvable = || CarryOverRefusal::Unresolvable {
        what: "the remote's default branch".to_string(),
    };
    let out = origin_git(repo, &["ls-remote", "--symref", "origin", "HEAD"])
        .map_err(|_| unresolvable())?;
    out.lines()
        .find_map(|l| {
            let rest = l.strip_prefix("ref:")?;
            let (target, name) = rest.trim().split_once(char::is_whitespace)?;
            (name.trim() == "HEAD")
                .then(|| target.strip_prefix("refs/heads/"))
                .flatten()
                .map(str::to_string)
        })
        .filter(|b| !b.is_empty())
        .ok_or_else(unresolvable)
}

/// Prove the carry-over `refusal` may get, with no side effects: `None` when
/// the refusal is not a moved head (no other refusal can carry over). The
/// PR's base must be the remote's default branch, and the proof runs
/// against that branch.
// trace:TASK-1960 | ai:claude
pub(crate) fn prove_for_refusal(
    site: &CarryOverSite<'_>,
    pr: u64,
    refusal: &crate::pr_ship::ApprovalHeadRefusal,
) -> Option<Result<CarryOver, CarryOverRefusal>> {
    let crate::pr_ship::ApprovalHeadRefusal::Moved {
        reviewed_sha,
        head_sha,
    } = refusal
    else {
        return None;
    };
    let pr_base = site
        .base_branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::pr_cmd::pr_ship_target_branch(pr));
    Some(protected_base(site.repo).and_then(|protected| {
        if pr_base != protected {
            return Err(CarryOverRefusal::BaseNotProtected {
                base: pr_base,
                protected,
            });
        }
        prove_carry_over(site.repo, pr, &protected, reviewed_sha, head_sha)
    }))
}

/// [`prove_for_refusal`], then — unless the site is quiet — record the
/// proof. `RecordOutcome` is `None` for a quiet site. A proof that cannot be
/// recorded refuses: a carry-over is never applied without its evidence on
/// disk. A success is remembered on the site ([`CarryOverSite::proven`]).
// trace:TASK-1960 | ai:claude
pub(crate) fn carry_over_for_refusal(
    site: &CarryOverSite<'_>,
    pr: u64,
    refusal: &crate::pr_ship::ApprovalHeadRefusal,
) -> Option<Result<(CarryOver, Option<RecordOutcome>), CarryOverRefusal>> {
    let proved = prove_for_refusal(site, pr, refusal)?;
    Some(proved.and_then(|proof| {
        let outcome = if site.quiet {
            None
        } else {
            Some(
                record_carry_over(site.repo, &proof).map_err(|e| CarryOverRefusal::Git {
                    detail: format!("the carry-over could not be recorded: {e}"),
                })?,
            )
        };
        *site.proven.borrow_mut() = Some(proof.clone());
        Ok((proof, outcome))
    }))
}

/// Why a merge bound to a carried-over approval must not proceed: the PR's
/// base as the forge reports it at merge time (`None` = unreadable) is not
/// the base the proof was computed against. `None` = proceed.
// trace:TASK-1960 | ai:claude
pub(crate) fn proven_base_refusal(
    pr: u64,
    proven_base: &str,
    base_at_merge: Option<&str>,
) -> Option<String> {
    match base_at_merge.map(str::trim).filter(|b| !b.is_empty()) {
        Some(base) if base == proven_base => None,
        Some(base) => Some(format!(
            "PR-{pr} was not merged: its approval carried over against `{proven_base}`, but the \
             PR now targets `{base}`. Re-run `aida pr ship` against the current base."
        )),
        None => Some(format!(
            "PR-{pr} was not merged: its approval carried over against `{proven_base}`, but the \
             PR's base could not be re-read before the merge, so the carry-over cannot be shown \
             to still apply."
        )),
    }
}

/// The `.aida/advisor-activity.jsonl` line recording an applied carry-over.
// trace:TASK-1960 | ai:claude
pub(crate) fn carry_over_event(now_iso: &str, command: &str, proof: &CarryOver) -> String {
    serde_json::json!({
        "ts": now_iso,
        "command": command,
        "step": "pr-merge-approval-carried-over",
        "status": "carried-over",
        "pr": proof.pr,
        "reviewed_sha": proof.approved_sha,
        "head_sha": proof.new_head,
        "approved_patch_id": proof.approved_patch_id,
        "head_patch_id": proof.head_patch_id,
        "approved_range": proof.approved_range,
        "head_range": proof.head_range,
        "base_ref": proof.base_ref,
    })
    .to_string()
}

/// Prove (or refuse) that the approval at `approved` carries over to `head`
/// in `repo`, against the remote-tracking ref of `base_branch`. Fetches the
/// base branch and any missing full sha from `origin` first, best effort: a
/// stale or missing base fails the proof closed rather than passing it.
// trace:TASK-1960 | ai:claude
pub(crate) fn prove_carry_over(
    repo: &Path,
    pr: u64,
    base_branch: &str,
    approved: &str,
    head: &str,
) -> Result<CarryOver, CarryOverRefusal> {
    let base_branch = base_branch.trim();
    if base_branch.is_empty()
        || base_branch.starts_with('-')
        || !git_ok(repo, &["check-ref-format", "--branch", base_branch])
    {
        return Err(CarryOverRefusal::Unresolvable {
            what: format!("the base branch `{base_branch}`"),
        });
    }
    let base_ref = format!("origin/{base_branch}");
    let _ = origin_git(
        repo,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "origin",
            &format!("+refs/heads/{base_branch}:refs/remotes/origin/{base_branch}"),
        ],
    );
    let base_tip =
        resolve_commit(repo, &format!("refs/remotes/origin/{base_branch}")).ok_or_else(|| {
            CarryOverRefusal::Unresolvable {
                what: format!("the base ref {base_ref}"),
            }
        })?;
    let approved =
        resolve_sha_fetching(repo, approved).ok_or_else(|| CarryOverRefusal::Unresolvable {
            what: format!("the approved commit {}", approved.trim()),
        })?;
    let head = resolve_sha_fetching(repo, head).ok_or_else(|| CarryOverRefusal::Unresolvable {
        what: format!("the PR head {}", head.trim()),
    })?;

    // Proof 1: structure — only clean, unedited merges of the base branch.
    if !git_ok(repo, &["merge-base", "--is-ancestor", &approved, &head]) {
        return Err(CarryOverRefusal::NotDescendant);
    }
    let merge_commits = base_merges(repo, &approved, &head, &base_tip)?;

    // Proof 2: patch identity of the PR's own diff.
    let approved_base = single_merge_base(repo, &base_tip, &approved)?;
    let head_base = single_merge_base(repo, &base_tip, &head)?;
    let approved_patch_id = patch_id(repo, &approved_base, &approved)?;
    let head_patch_id = patch_id(repo, &head_base, &head)?;
    if approved_patch_id != head_patch_id {
        return Err(CarryOverRefusal::PatchChanged {
            approved_patch_id,
            head_patch_id,
        });
    }
    Ok(CarryOver {
        pr,
        approved_range: format!("{approved_base}..{approved}"),
        head_range: format!("{head_base}..{head}"),
        approved_sha: approved,
        new_head: head,
        base_ref,
        base_tip,
        approved_patch_id,
        head_patch_id,
        merge_commits,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Every commit `approved..head` adds that is not on the base branch, each
/// checked to be a clean, unedited merge of the base branch into the PR.
// trace:TASK-1960 | ai:claude
fn base_merges(
    repo: &Path,
    approved: &str,
    head: &str,
    base_tip: &str,
) -> Result<Vec<String>, CarryOverRefusal> {
    let listing = git(
        repo,
        &[
            "rev-list",
            "--parents",
            head,
            &format!("^{approved}"),
            &format!("^{base_tip}"),
        ],
    )?;
    let rows: Vec<Vec<String>> = listing
        .lines()
        .map(|l| l.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|r| !r.is_empty())
        .collect();
    // The PR side: the approved commit and every commit it gained since.
    let pr_side: std::collections::BTreeSet<&str> = rows
        .iter()
        .map(|r| r[0].as_str())
        .chain(std::iter::once(approved))
        .collect();
    let mut merges = Vec::new();
    for row in &rows {
        let commit = &row[0];
        let parents = &row[1..];
        if parents.len() < 2 {
            return Err(CarryOverRefusal::NotAMerge {
                commit: commit.clone(),
            });
        }
        let (ours, theirs): (Vec<&String>, Vec<&String>) =
            parents.iter().partition(|p| pr_side.contains(p.as_str()));
        if parents.len() != 2
            || ours.len() != 1
            || theirs.len() != 1
            || !git_ok(repo, &["merge-base", "--is-ancestor", theirs[0], base_tip])
        {
            return Err(CarryOverRefusal::NotABaseMerge {
                commit: commit.clone(),
            });
        }
        let auto = auto_merge_tree(repo, &parents[0], &parents[1], commit)?;
        let actual = git(
            repo,
            &["rev-parse", "--verify", &format!("{commit}^{{tree}}")],
        )?;
        if auto != actual.trim() {
            return Err(CarryOverRefusal::MergeEdited {
                commit: commit.clone(),
            });
        }
        merges.push(commit.clone());
    }
    Ok(merges)
}

/// The tree git's own merge of `a` and `b` produces; a conflict refuses as a
/// resolved conflict in `commit`.
// trace:TASK-1960 | ai:claude
fn auto_merge_tree(
    repo: &Path,
    a: &str,
    b: &str,
    commit: &str,
) -> Result<String, CarryOverRefusal> {
    let out = git_raw(repo, &["merge-tree", "--write-tree", "--no-messages", a, b])?;
    match out.status.code() {
        Some(0) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .ok_or_else(|| CarryOverRefusal::Git {
                detail: "`git merge-tree` printed no tree".to_string(),
            }),
        Some(1) => Err(CarryOverRefusal::MergeConflicted {
            commit: commit.to_string(),
        }),
        _ => Err(CarryOverRefusal::Git {
            detail: format!(
                "`git merge-tree --write-tree` failed (needs git 2.38 or later): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        }),
    }
}

/// The single merge base of `base_tip` and `sha`; criss-cross history (more
/// than one) refuses rather than picking one.
// trace:TASK-1960 | ai:claude
fn single_merge_base(repo: &Path, base_tip: &str, sha: &str) -> Result<String, CarryOverRefusal> {
    let out = git(repo, &["merge-base", "--all", base_tip, sha])?;
    match out.split_whitespace().collect::<Vec<_>>().as_slice() {
        [one] => Ok(one.to_string()),
        _ => Err(CarryOverRefusal::AmbiguousBase {
            sha: sha.to_string(),
        }),
    }
}

/// `git patch-id --verbatim` of `from..to`, diffed without renames, external
/// drivers or text conversion, binary content included.
// trace:TASK-1960 | ai:claude
pub(crate) fn patch_id(repo: &Path, from: &str, to: &str) -> Result<String, CarryOverRefusal> {
    let diff = git_raw(
        repo,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-relative",
            "--full-index",
            "--binary",
            "--diff-algorithm=myers",
            from,
            to,
            "--",
        ],
    )?;
    if !diff.status.success() {
        return Err(CarryOverRefusal::Git {
            detail: format!(
                "`git diff {from} {to}` failed: {}",
                String::from_utf8_lossy(&diff.stderr).trim()
            ),
        });
    }
    if diff.stdout.is_empty() {
        return Err(CarryOverRefusal::EmptyDiff);
    }
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["patch-id", "--verbatim"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CarryOverRefusal::Git {
            detail: format!("could not run `git patch-id`: {e}"),
        })?;
    let wrote = child
        .stdin
        .take()
        .map(|mut stdin| stdin.write_all(&diff.stdout));
    let out = child
        .wait_with_output()
        .map_err(|e| CarryOverRefusal::Git {
            detail: format!("`git patch-id` failed: {e}"),
        })?;
    if !matches!(wrote, Some(Ok(()))) || !out.status.success() {
        return Err(CarryOverRefusal::Git {
            detail: format!(
                "`git patch-id --verbatim` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .filter(|id| id.len() >= 40 && id.bytes().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or(CarryOverRefusal::EmptyDiff)
}

/// `sha` resolved to a full commit id; a full sha missing locally is fetched
/// from `origin` once. An abbreviated sha is never fetched or guessed.
// trace:TASK-1960 | ai:claude
fn resolve_sha_fetching(repo: &Path, sha: &str) -> Option<String> {
    let sha = sha.trim().to_ascii_lowercase();
    if sha.len() < 7 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    resolve_commit(repo, &sha)
        .or_else(|| {
            if sha.len() != 40 && sha.len() != 64 {
                return None;
            }
            origin_git(repo, &["fetch", "--quiet", "--no-tags", "origin", &sha]).ok()?;
            resolve_commit(repo, &sha)
        })
        .filter(|full| full.starts_with(&sha))
}

fn resolve_commit(repo: &Path, rev: &str) -> Option<String> {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn git_raw(repo: &Path, args: &[&str]) -> Result<std::process::Output, CarryOverRefusal> {
    git_raw_via(repo, None, args)
}

/// `git_raw`, plus `remote`'s stored credentials kept out of argv when the
/// call contacts it.
// trace:BUG-2181 | ai:claude
fn git_raw_via(
    repo: &Path,
    remote: Option<&str>,
    args: &[&str],
) -> Result<std::process::Output, CarryOverRefusal> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    if let Some(remote) = remote {
        aida_core::git_ops::configure_remote_credentials(&mut cmd, repo, remote);
    }
    cmd.stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CarryOverRefusal::Git {
            detail: format!("could not run git: {e}"),
        })
}

fn git(repo: &Path, args: &[&str]) -> Result<String, CarryOverRefusal> {
    git_via(repo, None, args)
}

/// [`git`] for a call that contacts origin.

// trace:BUG-2181 | ai:claude
fn origin_git(repo: &Path, args: &[&str]) -> Result<String, CarryOverRefusal> {
    git_via(repo, Some("origin"), args)
}

fn git_via(repo: &Path, remote: Option<&str>, args: &[&str]) -> Result<String, CarryOverRefusal> {
    let out = git_raw_via(repo, remote, args)?;
    if !out.status.success() {
        return Err(CarryOverRefusal::Git {
            detail: format!(
                "`git {}` failed: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    git_raw(repo, args).is_ok_and(|o| o.status.success())
}

#[cfg(test)]
#[path = "tests/task_1960_carryover_tests.rs"]
pub(crate) mod tests;
