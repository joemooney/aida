// ─── STORY-707: fast cache-backed `aida status` default ────────────────────
//
// The bare `aida status` (no flags) is an ORIENTATION command — it must be
// instant. The historic heavy path (`handle_status_command_distributed`) loads
// the FULL store, makes `gh` PR/CI NETWORK calls (~16s), and probes live Claude
// sessions before any of the --short/--queue branches, so every form paid that
// cost. The fast path below answers "where am I right now" from cache-cheap
// inputs ONLY: role (env/default), current git branch, queue depth (the queue
// dir scan), and key requirement counts sourced from `.aida/cache.db` — the SAME
// cache that powers sub-ms `aida list`. No `backend.load()`, no `gh`, no
// live-session probe. The rich view is preserved opt-in via `aida status --full`
// / `--ci` (which route through the heavy path); the moved diagnostics (PR/CI,
// liveness, worktrees, roster, coordination, hygiene) now live in `aida doctor`.
// trace:STORY-707 | ai:claude

// ----------------------------------------------------------------------------
// `aida upgrade` — fetch latest release and replace the running binary.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ---- shared helpers -------------------------------------------------------

// TASK-1082: did-you-mean for a mistyped spec id. When `aida show`/`edit`/
// `queue work` (and every other spec-resolution surface) hits "Requirement not
// found: <id>", compare the requested id against the existing id set and — if a
// close match exists — append `did you mean <ID>?`, mirroring the affordance
// clap gives for mistyped subcommands and `nearest_standard_rel_type`
// (TASK-887) gives for rel-type typos. Scoped to the not-found branch; the
// found path is untouched and the exit code stays non-zero.

// ============================================================================
// Trace Command Handlers
// ============================================================================

// ---------------------------------------------------------------------------
// `aida review` — review-workflow helpers (STORY-67)
// ---------------------------------------------------------------------------

// ============================================================================
// STORY-498: CI spec-id validity gate (MVP slice of EPIC-34)
//
// Walk a commit range, resolve every `(SPEC-ID)` subject trailer against the
// live requirement graph, and FAIL when a trailer references a SPEC-ID that
// does not exist or is rejected — a dead/dangling provenance link. Promotes
// AIDA's headline guarantee (code traces to a *live* requirement) from a
// client-side message-grammar check into a server-side validity gate that is
// non-bypassable from a dev machine (`git commit --no-verify` cannot dodge a
// CI step). Reuses the existing `(SPEC-ID)` trailer parser and honours the
// TASK-488 mechanical-commit exemption (a commit with no trailer carries
// nothing to validate, matching the "no REQ-ID needed" rule for mechanical /
// release / docs commits).
// ============================================================================

// ============================================================================
// TASK-868: trace-ROT detector (`aida trace check`) — move #1 of EPIC-50.
//
// `gate` validates commit `(SPEC-ID)` trailers; `coverage` checks the changed
// CODE carries provenance. `check` closes the third corner: it walks the inline
// `// trace:SPEC-ID` markers *already in the source* and resolves each against
// the live graph, flagging the ones that have ROTTED — the target was deleted,
// renumbered, or rejected. This neutralizes the sharpest self-undercut of the
// separate-model approach ("trace comments rot invisibly"): a stale trace now
// goes red like a failing type. Report-only by default; `--block` for CI.
//
// Reuses `walk_source_for_traces` (the same inline scanner `aida doctor
// validate-trace-comments` uses) and `resolve_spec_in_store` / `load_store_for_lookup`
// (the same resolution `aida show` and `trace gate` use). trace:TASK-868
// ============================================================================

// ============================================================================
// STORY-469 Guard 1: client-side trailer spec-ID validation at ship/done.
//
// STORY-498 shipped a SERVER-side (CI) version of this check. Guard 1 is its
// client-side twin: it runs in `aida pr ship` and `aida queue done` so a
// hallucinated/typo'd/dead spec-ID is caught BEFORE the commit reaches the PR
// or the spec is flipped to Done — never lands in shared git history. It reuses
// the same pure validator (`validate_trailer_references`) + resolver
// (`resolve_spec_in_store`) as the gate; only the call site differs.
// ============================================================================

// ============================================================================
// TASK-1442: PR-open spec-attribution guard (containment for BUG-1510).
//
// STORY-469's `run_client_trailer_guard` above checks that every `(SPEC-ID)`
// trailer on the branch resolves to a LIVE spec — it never checks that a
// trailer names the SPEC THE BRANCH IS FOR. BUG-1510's incident: STORY-1391's
// drain opened PR #2043 whose commits were all trailered BUG-1420 — every
// trailer was live, so Guard 1 passed, but the PR was misattributed. This
// guard closes that gap: before a NEW PR is opened, at least one commit the
// branch adds over the default branch must carry a trailer naming the spec
// the branch is leased for.
// ============================================================================

// ============================================================================
// TASK-1444 (containment for BUG-1510 AC4): reviewer-verdict shelve
// attribution.
//
// The incident: STORY-1391's drain got a RequestChanges verdict whose
// findings were about BUG-1420 (the PR's commits were all trailered
// BUG-1420, per the TASK-1442 guard above), and the orchestrator shelved it
// onto STORY-1391 — the lease's spec — silently. A shelve must record
// against the spec the VERDICT is about, or say the attribution is
// uncertain; it must never read as a confirmed attribution to the lease
// when that was never checked.
// ============================================================================

// ============================================================================
// TASK-1445 (containment for BUG-1510 AC5): surface a PR attribution split
// between `aida drain status` (attributes a PR by the lease it ran under)
// and the trailer/title-based detectors (`awaiting_you.rs` unshipped-work
// code) — instead of letting the two sources silently disagree, as happened
// for 52 seconds in the BUG-1510 incident before a reviewer verdict landed
// on the wrong spec.
// ============================================================================

// ============================================================================
// STORY-499: diff-level trace-COVERAGE check (gap #1 of EPIC-34).
//
// STORY-498 shut gaps #2 + #3: `aida trace gate` validates that every commit
// `(SPEC-ID)` *trailer* resolves to a live spec. STORY-499 closes gap #1 —
// COVERAGE: does the changed CODE in a diff actually carry the required
// provenance (`// trace:` anchor OR a live commit trailer), per the SPIKE-47
// definition? It is report-only by default (SPIKE-47 §5/§6: a coverage gate's
// failure mode is adoption death; ship it report-first, flip to block after a
// calibration period).
//
// The hard part is precision without false positives. SPIKE-47 settled the
// definition deterministically:
//   - UNIT: the changed source hunk (NOT line, NOT function — §2).
//   - EXEMPTIONS: a two-layer, explicit, audited list (§3) — commit-level
//     (reusing STORY-498's plan/no-trailer machinery) + file/path-level
//     (tests, generated, docs, config, vendored) + hunk-level (pure deletion,
//     whitespace/fmt-only, comment-only, trivial one-liner).
//   - ATTRIBUTION (§4.2): a coverable hunk is COVERED if ANY of — an in-hunk
//     `// trace:<live-id>`; a `// trace:` within N lines ABOVE the hunk in the
//     post-change file; a module-level `//! trace:`; OR the commit carries a
//     live `(SPEC-ID)` trailer (the floor — STORY-498 already enforces this for
//     validity, so the default world is fully covered and the gate stays
//     silent).
//
// The CORE (`compute_diff_coverage`) is a PURE, fully-isolated-tested fn: it
// takes parsed hunks + per-file post-change content + a commit-trailer-covered
// flag + an anchor resolver, and returns the per-hunk classification + ratio.
// No git, no store, no I/O. The CLI handler gathers those inputs from git and
// the live graph and calls it. trace:STORY-499 | ai:claude
// ============================================================================

// ============================================================================
// TASK-939: diff-driven doc nudge at PR-open — `aida doc suggest`.
//
// `aida doc coverage` is the release-time backstop (every Completed spec should
// have a Doc). This is the front-of-the-pipeline nudge: at PR-open, when the
// branch ADDS new public surface (a CLI flag, a named CLI subcommand, or an MCP
// tool) and the spec that surface traces to has no Doc entry about it, remind
// the author to capture one while the "why" is fresh. Warn-only — `/aida-pr`
// calls it and it never blocks the PR (exit 0 always).
//
// Reuses the STORY-499 diff machinery (`parse_unified_diff_hunks`,
// `read_diff_for_range`, `resolve_gate_range`, `coverage_anchor_re`) and the
// same `Doc References` documented-set logic as `find_uncovered_completed_specs`
// (TASK-680). The detection core is pure and unit-tested. trace:TASK-939
// ============================================================================

#[cfg(test)]
#[path = "tests/task_518_pr_to_spec_tests.rs"]
mod task_518_pr_to_spec_tests;

/// trace:STORY-67 | ai:claude
/// The review surface a held spec is sitting on — what `aida review <SPEC>`
/// resolves the spec to before reviewing. NEVER asserts a closed/absent PR
/// (the TASK-715 / BUG-493 failure mode); each variant is a verified state.
/// trace:STORY-553 | ai:claude
#[derive(Debug)]
pub(crate) enum ReviewSurface {
    /// An OPEN change (PR/MR) exists for the spec's branch.
    OpenChange {
        branch: String,
        number: u64,
        url: String,
    },
    /// Work is on a feature branch with commits, but no open change.
    /// (Either never pushed a PR, or the held draft PR was closed.)
    BranchNoChange { branch: String, commits: usize },
    /// Commits reference the spec but already landed on the default branch.
    Shipped { number: Option<u64> },
    /// No commits and no trace comments reference the spec — built locally
    /// and never pushed, or not started.
    Local,
}

pub(crate) fn review_branch_no_change_context_card(
    spec_id: &str,
    branch: &str,
    commits: usize,
    change_noun: &str,
    open_change_cmd: &str,
) -> context_prompt::ContextCard {
    context_prompt::ContextCard {
        decision: format!(
            "whether to open a {change_noun} from `{branch}` before running `aida review {spec_id}`"
        ),
        provenance: vec![
            format!(
                "branch `{branch}` has {commits} commit{} linked to {spec_id}",
                if commits == 1 { "" } else { "s" }
            ),
            format!("no open {change_noun} was found for that branch"),
        ],
        answers: vec![
            format!(
                "y: print `{open_change_cmd}`; reversible by closing the {change_noun}"
            ),
            format!(
                "n: leave the work held without opening a {change_noun}; reversible by rerunning `aida review {spec_id}`"
            ),
        ],
        recommended_default: format!(
            "n - avoid creating a {change_noun} until you decide this branch is the review surface"
        ),
    }
}

// trace:BUG-816 | ai:codex
// trace:BUG-1610 | ai:claude — `base` is now always explicit (see
// `ForgeKind::create_cmd_for_branch`), so this hint never lets the forge
// infer (and possibly mis-infer) the target branch.
pub(crate) fn review_open_change_hint(
    forge: crate::forge::ForgeKind,
    branch: &str,
    base: &str,
) -> String {
    let push = format!("git push -u origin {}", shell_quote(branch));
    match forge.create_cmd_for_branch(branch, base) {
        Some(create) => format!("{push} && {create}"),
        None => push,
    }
}

pub(crate) fn review_rebase_context_card(
    change_noun: &str,
    number: u64,
    behind: u32,
    overlap: &[String],
) -> context_prompt::ContextCard {
    let mut provenance = vec![format!(
        "{change_noun}-{number} is {behind} commit{} behind its base branch",
        if behind == 1 { "" } else { "s" }
    )];
    if overlap.is_empty() {
        provenance.push("no changed file overlap was detected".to_string());
    } else {
        provenance.push(format!(
            "changed-file overlap detected: {}",
            overlap.join(", ")
        ));
    }
    let default = !overlap.is_empty();
    context_prompt::ContextCard {
        decision: format!("whether to rebase {change_noun}-{number} before review"),
        provenance,
        answers: vec![
            format!(
                "y: run `aida pr rebase {number}` now; reversible through normal git/forge recovery if the rebase fails"
            ),
            format!(
                "n: continue reviewing the existing {change_noun} diff against its stale base; reversible by rebasing later"
            ),
        ],
        recommended_default: if default {
            "y - overlapping files make the stale-base review less reliable".to_string()
        } else {
            "n - no file overlap was detected, so the review can proceed".to_string()
        },
    }
}

pub(crate) fn review_diffstat(project_root: &std::path::Path, branch: &str) -> String {
    let base =
        resolve_default_branch_ref(project_root).unwrap_or_else(|| "origin/main".to_string());
    let range = format!("{base}...{branch}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--stat", &range])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let stat = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stat.is_empty() {
                format!("No diffstat for `{range}`.")
            } else {
                stat
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            format!("Diffstat unavailable for `{range}`: {err}")
        }
        Err(e) => format!("Diffstat unavailable for `{range}`: {e}"),
    }
}

pub(crate) fn ask_ai_review_prompt(
    req: &aida_core::Requirement,
    spec_id: &str,
    card: &context_prompt::ContextCard,
    diffstat: &str,
) -> String {
    format!(
        "You are AIDA's one-shot Ask-AI reviewer at an interactive CLI fork.\n\n\
         Return a concise recommendation for the operator. Start with exactly one line:\n\
         RECOMMEND: y|n - <short reason>\n\n\
         Then add at most three bullets naming the decisive facts. Do not run tools. \
         Judge only from the context below.\n\n\
         ## Prompt context\n\
         {}\n\
         ## Spec\n\
         ID: {spec_id}\n\
         Title: {}\n\
         Status: {}\n\n\
         Description and acceptance:\n{}\n\n\
         ## Diffstat\n{}\n",
        card.render(),
        req.title,
        req.status,
        req.description.trim(),
        diffstat.trim(),
    )
}

pub(crate) fn ask_ai_review_once<W: std::io::Write + ?Sized>(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    spec_id: &str,
    card: &context_prompt::ContextCard,
    diffstat: &str,
    output: &mut W,
) -> Result<()> {
    let vendor = session::resolve_headless_vendor(project_root);
    let adapter = match compete::vendor_adapter(vendor.as_str()) {
        Some(adapter @ compete::VendorAdapter::Headless { .. }) => adapter,
        Some(compete::VendorAdapter::HumanBriefed) | None => {
            writeln!(
                output,
                "\nAsk-AI unavailable: `{}` is not a one-shot headless review vendor here.",
                vendor.as_str()
            )?;
            return Ok(());
        }
    };
    let prompt = ask_ai_review_prompt(req, spec_id, card, diffstat);
    let args = compete::ask_ai_argv(&adapter, &prompt).unwrap_or_default();
    let command = match &adapter {
        compete::VendorAdapter::Headless { command, .. } => *command,
        compete::VendorAdapter::HumanBriefed => unreachable!("human-briefed returned above"),
    };
    writeln!(
        output,
        "\nAsk-AI via {}…",
        format!(
            "{} {}",
            command,
            args.first().map(String::as_str).unwrap_or("")
        )
        .trim()
    )?;
    let result = std::process::Command::new(command)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy();
    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stdout.is_empty() {
                writeln!(output, "{stdout}")?;
            }
            if !out.status.success() {
                writeln!(output, "Ask-AI exited with {status}.", status = out.status)?;
                if !stderr.is_empty() {
                    writeln!(output, "{stderr}")?;
                }
            }
        }
        Err(e) => {
            writeln!(
                output,
                "Ask-AI unavailable: could not spawn `{}` ({e}).",
                command
            )?;
        }
    }
    Ok(())
}

/// Classify a spec's review surface from its git linkage + (optional) forge
/// change-lookup. Pure: the side-effecting `collect_git_linkage` /
/// `change_lookup_for_branch` run at the call site; this is the decision so
/// it's unit-testable. Order matters — shipped wins over branch, and only a
/// `ChangeLookup::Found` becomes `OpenChange` (a closed/absent/unreachable
/// PR must NOT be asserted as open — the TASK-715 / BUG-493 failure mode).
/// trace:STORY-553 trace:BUG-493 | ai:claude
pub(crate) fn classify_review_surface(
    linkage: &GitLinkage,
    change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    classify_review_surface_forge_first(linkage, None, change)
}

/// BUG-876: resolve review surfaces forge-first. A lease/local branch is only
/// an accelerator; an open PR whose head branch or commit trailers reference
/// the spec remains the review surface after the lease is released.
// trace:BUG-876 | ai:codex
pub(crate) fn classify_review_surface_forge_first(
    linkage: &GitLinkage,
    spec_change: Option<crate::forge::ChangeLookup>,
    branch_change: Option<crate::forge::ChangeLookup>,
) -> ReviewSurface {
    if linkage.shipped {
        return ReviewSurface::Shipped {
            number: linkage.shipped_pr,
        };
    }

    if let Some(crate::forge::ChangeLookup::Found(c)) = spec_change {
        let branch = if c.branch.is_empty() {
            linkage
                .branch
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            c.branch
        };
        return ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        };
    }

    match (linkage.branch.clone(), branch_change) {
        (Some(branch), Some(crate::forge::ChangeLookup::Found(c))) => ReviewSurface::OpenChange {
            branch,
            number: c.id,
            url: c.url,
        },
        (Some(branch), _) => ReviewSurface::BranchNoChange {
            branch,
            commits: linkage.commits.len(),
        },
        (None, _) => ReviewSurface::Local,
    }
}

/// BUG-881: shared forge-first review-surface resolver for human review and
/// PR-only drains. A lease/local branch is only an accelerator; a standalone
/// `queue work --from-pr` must see the same open PR that `aida review` sees
/// when the PR is discoverable by spec trailer or spec-named head branch.
// trace:BUG-881 | ai:codex
pub(crate) fn resolve_review_surface_forge_first(
    project_root: &std::path::Path,
    spec_id: &str,
    linkage: &GitLinkage,
) -> ReviewSurface {
    let spec_change = change_lookup_for_spec(project_root, spec_id);
    let branch_change = linkage
        .branch
        .as_deref()
        .map(|b| change_lookup_for_branch(project_root, b));
    classify_review_surface_forge_first(linkage, Some(spec_change), branch_change)
}

/// BUG-582: the robust invariant guarding the `aida human` reviews-awaiting
/// bucket — a finished spec can NEVER be resurrected onto the operator's seat
/// by a lingering local branch / stale review surface.
///
/// Two stale-data leaks made BUG-581 (already `Completed`, PR-1048 already
/// MERGED) surface as "needs review": (1) a spec whose status is terminal must
/// never be a review candidate regardless of any branch the Agent-tool
/// worktrees left lying around (`worktree-agent-*` branches accumulate), and
/// (2) only a genuinely-open surface (`OpenChange` / `BranchNoChange`) is a
/// review; a merged (`Shipped`) or never-pushed (`Local`) surface is not.
///
/// Centralizing both checks in one pure predicate keeps the guarantee from
/// drifting: the candidate pre-filter in `reviews_awaiting_human` is a cost
/// bound (it reads the cache, which can be stale), while THIS is the
/// load-bearing invariant — even a stale cache row that slipped through the
/// pre-filter is dropped here. Pure ⇒ unit-testable without a repo/forge.
/// trace:BUG-582 | ai:claude
pub(crate) fn spec_eligible_for_review_awaiting(
    status: aida_core::RequirementStatus,
    surface: &ReviewSurface,
) -> bool {
    // A finished (merged) or abandoned spec is never review work — a lingering
    // local branch must not put it back on the operator's seat.
    if matches!(
        status,
        aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
    ) {
        return false;
    }
    // Only a genuinely-open review surface counts. `Shipped` = the PR already
    // merged (the merged-PR signal); `Local` = never pushed. Neither is a
    // review awaiting a human. The LEGITIMATE case — Done-on-a-branch with an
    // OPEN PR not yet merged — is `OpenChange` and stays included.
    matches!(
        surface,
        ReviewSurface::OpenChange { .. } | ReviewSurface::BranchNoChange { .. }
    )
}

/// BUG-722: the requirements-store orphan branch is never a code-review target.
/// Commits that reference a spec but landed on `aida-store` during a store
/// reconcile must not register as "a branch awaiting review" — the STORY-760
/// false positive (`[branch aida-store]`). The store branch name is the
/// project-wide constant the rest of the CLI hard-codes (see the
/// `branch_exists_anywhere(.., "aida-store")` probe). Any configured store/
/// mirror ref would also belong here, but a mirror is a REMOTE, not a branch —
/// the branch name is uniformly `aida-store`.
// trace:BUG-722 | ai:claude
pub(crate) fn is_store_branch(branch: &str) -> bool {
    branch == "aida-store"
}

/// BUG-722: the branch a review surface sits on, when it has one. `Shipped`
/// (merged) and `Local` (never pushed) carry no live branch. Lets the
/// classifier apply the store-branch exclusion without re-plumbing git.
// trace:BUG-722 | ai:claude
pub(crate) fn review_surface_branch(surface: &ReviewSurface) -> Option<&str> {
    match surface {
        ReviewSurface::OpenChange { branch, .. } | ReviewSurface::BranchNoChange { branch, .. } => {
            Some(branch.as_str())
        }
        ReviewSurface::Shipped { .. } | ReviewSurface::Local => None,
    }
}

/// BUG-722: which `aida human` bucket a candidate spec lands in once its
/// status, view-state, and review surface are known. See
/// [`classify_human_review_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanReviewBucket {
    /// A genuine code-review gate — an open PR, or a pushed branch on active/
    /// Done work — the same surface `aida review <SPEC>` locates.
    ReviewsAwaiting,
    /// Loose work-in-progress: a `Draft` spec on a pushed branch with commits
    /// but no open PR. Visible under `wip-branches`, but NOT a review gate.
    WipBranch,
    /// Not the human's review work at all (finished/abandoned/deferred/archived
    /// spec, the store branch, or a merged/never-pushed surface).
    Excluded,
}

/// BUG-722: classify what `aida human` bucket a candidate spec belongs to.
/// Pure ⇒ every false-positive class is unit-testable without a repo/forge.
/// Layers three gates on top of the BUG-582 status/surface invariant, in
/// precedence order:
///  1. **view-state** — a deferred or archived spec is hidden from the human's
///     review seat exactly as `aida list` hides it from the default view
///     (STORY-441 archived, STORY-584 deferred);
///  2. **status/surface** — the BUG-582 invariant: a Completed/Rejected spec or
///     a merged (`Shipped`) / never-pushed (`Local`) surface is never review
///     work, even with a lingering local branch;
///  3. **branch-type** — the `aida-store` requirements-store orphan branch is
///     never a code-review target (the STORY-760 false positive);
/// then splits the survivors: a `Draft` spec whose only surface is a pushed
/// branch with no PR is loose WIP (`wip-branches`), not a review gate — nothing
/// awaits a human until there's a PR or a Done claim (operator decision,
/// 2026-07-12). Everything else is a genuine review.
// trace:BUG-722 | ai:claude
pub(crate) fn classify_human_review_bucket(
    status: aida_core::RequirementStatus,
    archived: bool,
    deferred: bool,
    surface: &ReviewSurface,
) -> HumanReviewBucket {
    // Gate 1 — view-state: a filed-away or primed-conditional spec is not the
    // human's review work, even with a live branch (mirrors `aida list`).
    if archived || deferred {
        return HumanReviewBucket::Excluded;
    }
    // Gate 2 — the BUG-582 status/surface invariant.
    if !spec_eligible_for_review_awaiting(status.clone(), surface) {
        return HumanReviewBucket::Excluded;
    }
    // Gate 3 — branch-type: the requirements-store orphan branch is never a
    // code-review target.
    if review_surface_branch(surface).is_some_and(is_store_branch) {
        return HumanReviewBucket::Excluded;
    }
    // Draft + pushed branch + no open PR ⇒ loose WIP, not a review gate.
    if matches!(status, aida_core::RequirementStatus::Draft)
        && matches!(surface, ReviewSurface::BranchNoChange { .. })
    {
        return HumanReviewBucket::WipBranch;
    }
    HumanReviewBucket::ReviewsAwaiting
}

/// BUG-722: mirror the deferred view-state predicate `aida list` applies — the
/// `deferred` flag OR any legacy `deferred:*` parking tag (STORY-584's
/// honor-both rule). Keeps the `aida human` review seat hidden for exactly the
/// specs the default list hides.
// trace:BUG-722 | ai:claude
pub(crate) fn requirement_is_deferred(req: &aida_core::Requirement) -> bool {
    req.deferred || req.tags.iter().any(|t| t.starts_with("deferred:"))
}

/// BUG-511: RAII release for the review-verb lease — removing the lease
/// file on drop covers every exit path of [`handle_review_spec`] (verdict
/// presented, surface bailed early, reviewer launch failed, `?` errors).
/// A SIGKILL'd review leaks the file; the dead-PID reaping in
/// [`acquire_review_lease_with_mode`] / [`auto_release_decision_for_lease`] cleans
/// that up on the next coordination touch. trace:BUG-511 | ai:claude
#[derive(Debug)]
pub(crate) struct ReviewLeaseGuard {
    pub(crate) path: std::path::PathBuf,
}

impl Drop for ReviewLeaseGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewLeaseConflictMode {
    AutoReleaseStale,
    PromptBeforeRelease,
    RefuseStaleNonInteractive,
}

pub(crate) fn review_stale_lease_refusal(spec_id: &str, conflict: &SessionLease) -> String {
    format!(
        "`{spec_id}` is already in flight behind stale lease {} (owner {}, since {}). \
         Headless review cannot ask whether to release it. Re-run from a TTY to release-and-proceed, \
         or end it explicitly with `aida session end {} --yes` and then re-run `aida review {spec_id}`.",
        &conflict.id[..conflict.id.len().min(8)],
        conflict.owner,
        conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
        &conflict.id[..conflict.id.len().min(8)],
    )
}

pub(crate) fn confirm_release_stale_review_lease(
    spec_id: &str,
    conflict: &SessionLease,
) -> Result<bool> {
    use std::io::Write;
    eprintln!(
        "  {} stale lease {} is blocking review of `{}` (owner {}, since {}).",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        (&conflict.id[..conflict.id.len().min(8)]).yellow(),
        spec_id,
        conflict.owner,
        conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
    );
    eprintln!(
        "  {} release it via session cleanup and continue? [y/N] ",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    std::io::stderr().flush()?;
    let mut ans = String::new();
    std::io::stdin().read_line(&mut ans)?;
    Ok(matches!(
        ans.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// BUG-511: take a session lease scoped to the spec for the duration of an
/// `aida review <spec>` run — the same coordination substrate `aida queue
/// work` uses (`.aida/sessions/`, [`find_scope_lease_conflict`], the
/// BUG-307 auto-release sweep), so the open-spec classifier sees the spec
/// in flight ("being reviewed") and a second `aida review` / `queue work`
/// on it refuses instead of double-starting. The lease is an advisory lock
/// (empty `worktree_path`, the TASK-474 convention); its liveness signal
/// is this process's PID. trace:BUG-511 | ai:claude
pub(crate) fn acquire_review_lease_with_mode(
    project_root: &std::path::Path,
    spec_id: &str,
    branch: &str,
    conflict_mode: ReviewLeaseConflictMode,
) -> Result<ReviewLeaseGuard> {
    let cfg = orchestrator::OrchestratorConfig::load(project_root);
    let mut remaining = 16usize; // defense-in-depth bound, mirrors queue work's sweep
    while let Some(conflict) = find_scope_lease_conflict(&list_leases(project_root), spec_id) {
        let recovery = stale_lease_recovery_for_lease(&conflict);
        if matches!(
            recovery.verdict,
            StaleLeaseRecovery::ReclaimableClean { .. }
        ) && conflict_mode != ReviewLeaseConflictMode::AutoReleaseStale
        {
            match conflict_mode {
                ReviewLeaseConflictMode::PromptBeforeRelease => {
                    // BUG-890: human review is interactive, so a stale
                    // clean/advisory lease should be an explicit
                    // release-and-proceed choice rather than a hard stop.
                    // trace:BUG-890 | ai:codex
                    if !confirm_release_stale_review_lease(spec_id, &conflict)? {
                        anyhow::bail!(
                            "review aborted — stale lease {} on `{spec_id}` is still held.",
                            &conflict.id[..conflict.id.len().min(8)]
                        );
                    }
                }
                ReviewLeaseConflictMode::RefuseStaleNonInteractive => {
                    anyhow::bail!(review_stale_lease_refusal(spec_id, &conflict));
                }
                ReviewLeaseConflictMode::AutoReleaseStale => unreachable!(),
            }
            eprintln!(
                "  {} released stale lease {} on {} (process dead)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                spec_id,
            );
            let _ = force_cleanup_lease(project_root, &conflict);
            remaining -= 1;
            if remaining == 0 {
                anyhow::bail!(
                    "stale-lease release gave up after 16 iterations on `{spec_id}` — \
                     the lease store may be corrupt; inspect `.aida/sessions/`"
                );
            }
            continue;
        }
        match auto_release_decision_for_lease(project_root, &conflict, &cfg) {
            orchestrator::AutoReleaseDecision::SafelyDormant { process_dead, .. } => {
                match conflict_mode {
                    ReviewLeaseConflictMode::AutoReleaseStale => {}
                    ReviewLeaseConflictMode::PromptBeforeRelease => {
                        // BUG-890: human review is interactive, so a stale
                        // clean/advisory lease should be an explicit
                        // release-and-proceed choice rather than a hard stop.
                        // trace:BUG-890 | ai:codex
                        if !confirm_release_stale_review_lease(spec_id, &conflict)? {
                            anyhow::bail!(
                                "review aborted — stale lease {} on `{spec_id}` is still held.",
                                &conflict.id[..conflict.id.len().min(8)]
                            );
                        }
                    }
                    ReviewLeaseConflictMode::RefuseStaleNonInteractive => {
                        anyhow::bail!(review_stale_lease_refusal(spec_id, &conflict));
                    }
                }
                eprintln!(
                    "  {} released stale lease {} on {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    spec_id,
                    if process_dead {
                        "process dead"
                    } else {
                        "dormant"
                    }
                );
                let _ = force_cleanup_lease(project_root, &conflict);
                remaining -= 1;
                if remaining == 0 {
                    anyhow::bail!(
                        "auto-release sweep gave up after 16 iterations on `{spec_id}` — \
                         the lease store may be corrupt; inspect `.aida/sessions/`"
                    );
                }
            }
            orchestrator::AutoReleaseDecision::DormantDirty { dirty_entries } => {
                anyhow::bail!(
                    "lease {} on `{spec_id}` looks orphaned but its worktree at {} has \
                     {dirty_entries} uncommitted change(s) — resolve that session first \
                     (`aida queue work {spec_id} --resume` keeps the work; \
                     `aida session end {} --force` discards it).",
                    &conflict.id[..conflict.id.len().min(8)],
                    conflict.worktree_path.display(),
                    &conflict.id[..conflict.id.len().min(8)],
                );
            }
            orchestrator::AutoReleaseDecision::Live => {
                let holder = if conflict.review_verb {
                    "another review of it is already running".to_string()
                } else {
                    format!(
                        "a live {} session holds it",
                        conflict.role.as_deref().unwrap_or("work")
                    )
                };
                anyhow::bail!(
                    "`{spec_id}` is already in flight — {holder} (lease {}, owner {}, \
                     since {}). Reviewing it now would double-start; wait for that \
                     session to finish, or release it with `aida session end {}`.",
                    &conflict.id[..conflict.id.len().min(8)],
                    conflict.owner,
                    conflict.started_at.format("%Y-%m-%d %H:%M UTC"),
                    &conflict.id[..conflict.id.len().min(8)],
                );
            }
        }
    }

    let id_long = uuid::Uuid::now_v7().to_string();
    let id = id_long.replace('-', "")[..12].to_string();
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let lease = SessionLease {
        id: id.clone(),
        scope: spec_id.to_string(),
        slug: slugify(spec_id),
        owner,
        worktree_path: std::path::PathBuf::new(),
        branch: branch.to_string(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        role: Some("reviewer".to_string()),
        creator_pid: Some(std::process::id()),
        creator_pid_start_time: process_probe::process_start_identity(std::process::id()),
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: true,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
    };
    std::fs::create_dir_all(leases_dir(project_root))?;
    let path = lease_path(project_root, &id);
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(&path, toml::to_string_pretty(&lease)?)?;
    Ok(ReviewLeaseGuard { path })
}

/// BUG-539: a spec already in a terminal/merged state has no live review
/// surface — running a full review + the Approve / Request-changes menu over
/// it is at best confusing and at worst lets Request-changes → `aida queue
/// rework` disrupt already-merged work. `Completed` (merged to the default
/// branch) is the authoritative signal; `Rejected` is likewise terminal and
/// has nothing to review. Keep this a plain `//`-doc on a non-clap fn so the
/// trace marker can't leak into `--help`.
// trace:BUG-539 | ai:claude
pub(crate) fn review_is_terminal_noop(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Completed | RequirementStatus::Rejected
    )
}

/// BUG-1607: resolve (and preflight) the vendor `handle_review_spec` will
/// launch the interactive reviewer with — the SAME shared contract
/// `aida queue work` resolves for the implementer/reviewer launch (flag >
/// `[agents]` project/user config > default, filtered to an enabled AND
/// installed profile). This verb has no `--vendor` flag of its own, so a
/// Codex-only project (no Claude installed) must be picked up from config
/// alone.
///
/// `None` only for `no_agent` (no launch, so no vendor is needed) — every
/// other refusal (disabled profile, unreachable binary, unsupported
/// interactive vendor) propagates as `Err` from here, BEFORE
/// `handle_review_spec` acquires the review lease below it. Previously
/// there was no resolution here at all: the launch always hardcoded
/// `claude`, so the command reached "running reviewer" holding a live
/// lease and then failed with a raw ENOENT.
///
/// Split out from `handle_review_spec` (which gates on an interactive TTY
/// and therefore cannot run inside `cargo test`) so an automated test can
/// exercise this exact resolution-and-preflight code directly.
// trace:BUG-1607 | ai:claude
pub(crate) fn review_spec_resolve_vendor(
    project_root: &std::path::Path,
    no_agent: bool,
) -> Result<Option<session::HeadlessVendor>> {
    if no_agent {
        return Ok(None);
    }
    let vendor = session::resolve_enabled_headless_vendor(project_root)?;
    session::preflight_launch_vendor(vendor, true)?;
    Ok(Some(vendor))
}

/// BUG-1607: build + spawn the interactive reviewer [`session::ReviewerLaunchPlan`]
/// for `vendor` — the exact launch `handle_review_spec` runs once a human is
/// confirmed at an interactive terminal. Split out for the same testability
/// reason as [`review_spec_resolve_vendor`]: `handle_review_spec` itself
/// gates on a TTY and cannot run under `cargo test`, but this is the real
/// launch code, not a reimplemented stand-in for it.
// trace:BUG-1607 | ai:claude
pub(crate) fn review_spec_launch_reviewer(
    vendor: session::HeadlessVendor,
    session_name: &str,
    prompt: &str,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    let plan = session::interactive_reviewer_launch_plan(
        vendor,
        None,
        Some(session_name),
        prompt,
        session_id,
        false,
    )
    .map_err(|e| anyhow::anyhow!("aida review: {e}"))?;
    // ADR-66: the reviewer child grant validates against the PROJECT root,
    // resolved the same way every other authority check resolves it — not
    // whatever directory the process happens to sit in.
    // trace:STORY-1473 | ai:claude
    session::spawn_reviewer_launch_plan(&plan, &find_project_root()?)
        .context("failed to launch the reviewer")
}

/// trace:STORY-553 | ai:claude — `aida review <SPEC>`: the human-review
/// counterpart to `aida queue work`. Resolves the spec's review surface,
/// runs the existing headless reviewer tier (`/aida-review`) over the diff
/// against the spec's `## Acceptance` criteria, then presents the verdict
/// and lets the human decide (approve / request changes / open the diff /
/// defer). NEVER auto-merges — that would defeat the review:draft-only gate
/// (cf. STORY-529 self-merge bug-class).
pub(crate) fn handle_review_spec(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    spec: &str,
    no_agent: bool,
    allow_stale_base: bool,
    target_branch: Option<&str>,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;

    // ---- Resolve the spec ----
    let req = backend
        .get_requirement_by_spec_id(spec)
        .with_context(|| format!("failed to load {spec}"))?
        .ok_or_else(|| not_found::requirement_not_found(spec, Some(store_path)))?;
    let spec_id = req.spec_id.clone().unwrap_or_else(|| spec.to_string());
    record_role_activity(&spec_id, "review");

    println!(
        "{} {} — {}",
        "Reviewing".green().bold(),
        spec_id.cyan(),
        req.title
    );

    // ---- Locate the review surface (gh-free linkage, then forge lookup) ----
    let ids = vec![spec_id.clone()];
    let linkage = collect_git_linkage(project_root, &ids);
    let forge = crate::forge::resolve_forge_kind(project_root);

    // BUG-539: short-circuit a spec already in a terminal/merged state. When
    // the spec is Completed (work merged to the default branch) — or Rejected
    // — a full review + the Approve / Request-changes menu is at best
    // confusing and at worst lets the Request-changes → `aida queue rework`
    // path disrupt already-merged work. Detect it up front and report instead
    // of reviewing. We key off status as the authoritative signal — git-linkage
    // squash-PR parsing can miss the merge — and enrich the message with the
    // merge commit / PR number from linkage when available. At most we offer a
    // read-only `git show` of the merged diff. trace:BUG-539 | ai:claude
    if review_is_terminal_noop(&req.status) {
        let change_noun = forge.change_noun();
        let merged = matches!(req.status, RequirementStatus::Completed);
        let mut details: Vec<String> = Vec::new();
        if let Some((_, short, _)) = linkage.commits.first() {
            details.push(format!("commit {short}"));
        }
        if let Some(pr) = linkage.shipped_pr {
            details.push(format!("{change_noun}-{pr}"));
        }
        let detail_suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };
        let headline = if merged {
            "already merged/completed — nothing to review"
        } else {
            "rejected — nothing to review"
        };
        println!(
            "  {} {}{}",
            "Surface".bold(),
            headline.yellow(),
            detail_suffix.dimmed()
        );
        if merged {
            if let Some((full, _, _)) = linkage.commits.first() {
                println!(
                    "  {} to see the merged diff: {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                    format!("git show {full}").cyan()
                );
            }
            println!(
                "  {} if it genuinely needs another look, {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("aida queue rework {spec_id}").cyan()
            );
        }
        return Ok(());
    }

    // BUG-876 / BUG-881: use the shared forge-first review-surface resolver so
    // `aida review` and `queue work --from-pr` agree after the implementer
    // lease is gone. trace:BUG-881 | ai:codex
    let surface = resolve_review_surface_forge_first(project_root, &spec_id, &linkage);

    let change_noun = forge.change_noun();

    // Report the surface honestly, then either review or guide.
    match &surface {
        ReviewSurface::OpenChange {
            branch,
            number,
            url,
        } => {
            println!(
                "  {} open {}-{} on branch {} {}",
                "Surface".bold(),
                change_noun,
                number,
                branch.cyan(),
                url.dimmed()
            );
        }
        ReviewSurface::BranchNoChange {
            branch, commits, ..
        } => {
            println!(
                "  {} branch {} ({} commit{}) — {}",
                "Surface".bold(),
                branch.cyan(),
                commits,
                if *commits == 1 { "" } else { "s" },
                format!("no open {change_noun}").yellow(),
            );
            println!(
                "  {} the held draft {change_noun} is closed or was never opened.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
            // BUG-1610: resolve the intended base BEFORE offering to open a
            // change, and verify it — a bare `--source-branch` create with
            // no explicit base can silently target the source branch itself
            // when it became the project default (remote main never
            // pushed). An explicit `--target-branch` always wins over
            // whatever a previous attempt saved; the saved state itself
            // never has source == target (`write_mr_recovery_state` refuses
            // that pair). trace:BUG-1610 | ai:claude
            let recovery_path = crate::pr_cmd::mr_recovery_path(project_root, &spec_id);
            let saved_recovery = crate::pr_cmd::read_mr_recovery_state(&recovery_path);
            let default_base = crate::forge::default_branch_of(project_root);
            let base = crate::pr_cmd::resolve_mr_target_branch(
                target_branch,
                saved_recovery.as_ref(),
                &default_base,
            );
            let base_check = crate::pr_cmd::preflight_mr_base(project_root, branch, &base);
            if base_check != crate::pr_cmd::MrBasePreflight::Ok {
                println!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    crate::pr_cmd::mr_base_diagnosis_message(forge, base_check, branch, &base)
                        .yellow()
                );
                return Ok(());
            }
            // AC-5: offer to (re)open a PR before review.
            let open_change_cmd = review_open_change_hint(forge, branch, &base);
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                let card = review_branch_no_change_context_card(
                    &spec_id,
                    branch,
                    *commits,
                    change_noun,
                    &open_change_cmd,
                );
                let diffstat = review_diffstat(project_root, branch);
                let reopen = context_prompt::confirm_with_context_and_ai(
                    &format!("Open a {change_noun} from `{branch}` first?"),
                    false,
                    &card,
                    |out| ask_ai_review_once(project_root, &req, &spec_id, &card, &diffstat, out),
                )
                .unwrap_or(false);
                if reopen {
                    println!("  {} run: {}", "→".green(), open_change_cmd.cyan());
                    println!(
                        "  {} then re-run {} once the {change_noun} is open.",
                        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                        format!("aida review {spec_id}").cyan()
                    );
                    let _ = crate::pr_cmd::write_mr_recovery_state(
                        &recovery_path,
                        &crate::pr_cmd::MrRecoveryState {
                            spec: spec_id.clone(),
                            source_branch: branch.clone(),
                            target_branch: base.clone(),
                        },
                    );
                    return Ok(());
                }
            } else {
                println!(
                    "  {} to open one: {}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                    open_change_cmd.cyan()
                );
            }
        }
        ReviewSurface::Shipped { number } => {
            match number {
                Some(n) => println!(
                    "  {} already merged to the default branch (via {}-{}).",
                    "Surface".bold(),
                    change_noun,
                    n
                ),
                None => println!(
                    "  {} already merged to the default branch.",
                    "Surface".bold()
                ),
            }
            println!(
                "  {} nothing to review — the work has landed. \
                 If it needs another look, {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("aida queue rework {spec_id}").cyan()
            );
            return Ok(());
        }
        ReviewSurface::Local => {
            println!(
                "  {} {}",
                "Surface".bold(),
                "built locally, never pushed (no commits reference this spec)".yellow()
            );
            println!(
                "  {} commit your work with a {} trailer, push, and open a {change_noun}; \
                 then re-run {}.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                format!("({spec_id})").cyan(),
                format!("aida review {spec_id}").cyan()
            );
            return Ok(());
        }
    }

    // ---- Run the reviewer over the diff (AC-2 / AC-4: reuse /aida-review) ----
    let interactive = review_may_launch_reviewer(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    );
    let (pr_number, surface_branch) = match &surface {
        ReviewSurface::OpenChange { branch, number, .. } => (Some(*number), branch.clone()),
        ReviewSurface::BranchNoChange { branch, .. } => (None, branch.clone()),
        _ => unreachable!("shipped/local surfaces returned above"),
    };

    // BUG-721: `aida review <spec>` is the human-driven review verb. When stdin
    // isn't a terminal (piped/redirected/CI/agent), do NOT silently launch a
    // blind headless reviewer subprocess the caller can neither see nor
    // interrupt — the surface report above is already printed, so a
    // non-interactive caller gets the read-only surface only. Refuse loudly and
    // exit non-zero. `--no-agent` (the explicit read-only opt-out) still degrades
    // honestly below. This gate is scoped to THIS verb; the orchestrator's drain
    // review phase (`run_reviewer`) never reaches here — it launches its headless
    // reviewer via `aida queue work PR-N` on purpose — so drains are untouched.
    if !no_agent && !interactive {
        anyhow::bail!(
            "aida review needs an interactive terminal to run the reviewer.\n  \
             In a drain the orchestrator runs the reviewer for you; to see the \
             read-only surface without launching one, add --no-agent."
        );
    }

    // BUG-1607: resolve (and preflight) the launch vendor — see
    // `review_spec_resolve_vendor`'s doc for why this is BEFORE the lease
    // below is acquired, and why it is split out as its own function.
    let review_vendor = review_spec_resolve_vendor(project_root, no_agent)?;

    // BUG-511: hold a session lease scoped to the spec while the review
    // runs — same substrate as `aida queue work`, so the footer / `aida
    // why` / burndown-explain see the spec in flight and a concurrent
    // review or pickup refuses instead of double-starting. Released on
    // every exit path via the guard's Drop. trace:BUG-511 | ai:claude
    let review_lease_mode = if interactive {
        ReviewLeaseConflictMode::PromptBeforeRelease
    } else {
        ReviewLeaseConflictMode::RefuseStaleNonInteractive
    };
    let _review_lease =
        acquire_review_lease_with_mode(project_root, &spec_id, &surface_branch, review_lease_mode)?;
    // STORY-1405: while the reviewer runs, PR-N is visibly under review to
    // every merge surface. Released with the lease when this verb exits.
    // trace:STORY-1405 | ai:claude
    let _review_marker = match pr_number.filter(|_| !no_agent) {
        Some(n) => review_marker::hold(
            &main_worktree_root_from(project_root),
            review_marker::Marker::for_this_process(
                n,
                review_marker_head_best_effort(project_root, n, &surface_branch).as_deref(),
                Some(&spec_id),
                &format!("`aida review {spec_id}`"),
            ),
        )
        .map_err(|e| {
            eprintln!(
                "  {} could not mark PR-{n} as under review ({e}) — continuing",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            )
        })
        .ok(),
        None => None,
    };

    // BUG-510: stale-base pre-flight — same predicate as the reviewer-role
    // path (`aida queue work <PR-N> --for reviewer` / orchestrator phase 3:
    // preflight_stale_base_check → classify_stale_base), different posture.
    // A human is driving this verb and the verdict on the code is valid
    // either way, so even the overlap case warns-and-proceeds instead of
    // refusing. `--allow-stale-base` mirrors the reviewer-role opt-out so a
    // deliberate stale review isn't nagged. Fails open on infra errors
    // (gh missing, fetch failure) like both existing call sites.
    // trace:BUG-510 | ai:claude
    if let Some(n) = pr_number {
        if !allow_stale_base && matches!(forge, crate::forge::ForgeKind::GitHub) {
            let stale = match preflight_stale_base_check(project_root, n) {
                Ok(pr_rebase::StaleBaseOutcome::Current) => None,
                Ok(pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind }) => {
                    Some((behind, Vec::new()))
                }
                Ok(pr_rebase::StaleBaseOutcome::StaleOverlap {
                    behind,
                    overlap_files,
                    ..
                }) => Some((behind, overlap_files)),
                Err(e) => {
                    eprintln!(
                        "  {} stale-base check for {change_noun}-{n} failed ({e}); \
                         proceeding with review",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                    None
                }
            };
            if let Some((behind, overlap)) = stale {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    pr_rebase::stale_base_review_warn_message(n, behind, &overlap).yellow()
                );
                // Offer the rebase inline while a human is at the prompt —
                // the review then runs against current code. Defaults to yes
                // when a file the PR touches has also moved on the base.
                if interactive {
                    let card = review_rebase_context_card(change_noun, n, behind, &overlap);
                    let diffstat = review_diffstat(project_root, &surface_branch);
                    let rebase_now = context_prompt::confirm_with_context_and_ai(
                        &format!("Rebase {change_noun}-{n} now?"),
                        !overlap.is_empty(),
                        &card,
                        |out| {
                            ask_ai_review_once(project_root, &req, &spec_id, &card, &diffstat, out)
                        },
                    )
                    .unwrap_or(false);
                    if rebase_now {
                        if let Err(e) = pr_rebase_handler(n, false, false, false, None, None) {
                            eprintln!(
                                "  {} rebase did not complete ({e}); the review \
                                 will run against the stale base",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                            );
                        }
                    }
                }
            }
        }
    }

    if no_agent {
        // AC: degrade-honest path — surface + recommended command, no agent.
        match pr_number {
            Some(n) => {
                let diff_cmd = forge
                    .change_cmd_hint("diff", &n.to_string())
                    .unwrap_or_else(|| format!("inspect change {n}"));
                println!(
                    "\n  {} review the diff yourself: {}",
                    "→".green(),
                    diff_cmd.cyan()
                );
            }
            None => println!(
                "\n  {} no open {change_noun} to diff.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
        return Ok(());
    }

    // The verdict file the `/aida-review` skill writes (it keys off this env
    // var — same handshake the standalone reviewer uses). trace:BUG-226
    // TASK-1460: the SAME path `review_verdict::record_verdict` writes, so
    // the skill's direct write and the record below are one file, one round.
    // trace:TASK-1460 | ai:claude
    let verdict_path = review_verdict::verdict_path(project_root, &spec_id);
    if let Some(dir) = verdict_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&verdict_path);
    std::env::set_var("AIDA_REVIEW_VERDICT_FILE", &verdict_path);

    // The reviewer reads the actual DIFF against `## Acceptance` (AC-2).
    let prompt = match pr_number {
        Some(n) => format!("/aida-review --pr {n}"),
        None => "/aida-review".to_string(),
    };
    let session_id = uuid::Uuid::new_v4().to_string();

    println!(
        "\n  {} running reviewer over the diff against {}'s acceptance criteria…",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        spec_id.cyan()
    );

    // BUG-721: a non-interactive `aida review` was refused above, so a human is
    // at the terminal here — spawn the INTERACTIVE reviewer only, and let them
    // watch it under the native permission posture. This verb never launches a
    // blind headless reviewer; the orchestrator owns that (see `run_reviewer`).
    debug_assert!(
        interactive,
        "BUG-721: non-interactive review must be refused before the reviewer launch"
    );
    let name = format!("review-{}", spec_id.to_ascii_lowercase());
    // BUG-1607: launch through the SAME shared plan resolver
    // `run_standalone_reviewer` uses, instead of hardcoding `claude` — see
    // `review_spec_launch_reviewer`'s doc. `review_vendor` is always `Some`
    // here — the only `None` arm (`no_agent`) already returned above.
    // trace:BUG-1607 | ai:claude
    let vendor = review_vendor
        .expect("review_vendor is Some whenever no_agent is false (already returned above)");
    let status: std::process::ExitStatus =
        review_spec_launch_reviewer(vendor, &name, &prompt, &session_id)?;
    if !status.success() {
        eprintln!(
            "  {} the reviewer exited non-zero ({})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            status
        );
    }

    // ---- Read + present the verdict (AGENT ANALYZES + RECOMMENDS) ----
    // TASK-1460: the skill wrote this file directly; archive its round
    // (when it names a commit) before the record below stamps provenance.
    // trace:TASK-1460 | ai:claude
    let _ = review_verdict::adopt_direct_write(&verdict_path);
    let verdict = std::fs::read_to_string(&verdict_path)
        .ok()
        .and_then(|body| reviewer_summary::parse_verdict_file(&body));

    // BUG-775: stamp WHICH COMMIT this review looked at (and when) onto the
    // verdict record. The reviewer skill writes the verdict word; only AIDA
    // knows the branch tip it was pointed at. Without that sha "has the branch
    // moved since the review?" is unanswerable, and the `queue done` gate has
    // to fail closed. Best-effort — never blocks presenting the verdict.
    // trace:BUG-775 | ai:claude
    if let Some(v) = verdict.as_ref() {
        let reviewed_branch = linkage.branch.clone();
        let reviewed_sha =
            resolve_commit_sha(project_root, reviewed_branch.as_deref().unwrap_or("HEAD"))
                .or_else(|| resolve_commit_sha(project_root, "HEAD"));
        let _ = review_verdict::record_verdict(
            project_root,
            &spec_id,
            Some(&v.verdict),
            reviewed_sha.as_deref(),
            reviewed_branch.as_deref(),
            v.summary.as_deref(),
            &[],
            "aida review",
        );
    }

    println!();
    let recommended = match &verdict {
        Some(v) => {
            // trace:BUG-1505 | ai:claude — the one canonical parser.
            let kind = review_verdict::VerdictKind::parse(&v.verdict);
            let label = match kind {
                review_verdict::VerdictKind::Approved => "APPROVE".green().bold().to_string(),
                review_verdict::VerdictKind::RequestChanges => {
                    "REQUEST CHANGES".yellow().bold().to_string()
                }
                review_verdict::VerdictKind::Rejected => "REJECT".red().bold().to_string(),
                review_verdict::VerdictKind::Unknown => v.verdict.clone(),
            };
            println!("  {} {}", "Reviewer verdict:".bold(), label);
            if let Some(s) = v.summary.as_deref().filter(|s| !s.trim().is_empty()) {
                println!("  {} {}", "Summary:".bold(), s);
            }
            if let Some(url) = v.comment_url.as_deref().filter(|s| !s.is_empty()) {
                println!("  {} {}", "Review comment:".bold(), url.dimmed());
            }
            review_verdict::canonical_verdict_word(&v.verdict)
        }
        None => {
            println!(
                "  {} the reviewer produced no verdict file — review against {} yourself.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                spec_id.cyan()
            );
            String::new()
        }
    };

    // ---- HUMAN DECIDES + ACTS (AC-3). NEVER auto-merge (AC-3 / STORY-529) ----
    if !interactive {
        // No human to prompt — surface the recommended next command honestly.
        let recommend = match recommended.as_str() {
            // No --delete-branch: the implementer's worktree may still hold
            // the PR branch, so the delete's local-cleanup step would fail;
            // `;` keeps the auto-bump pull from being dropped by a broken
            // chain. Worktree cleanup owns branch deletion.
            // trace:BUG-758 | ai:claude
            "approved" => match pr_number {
                Some(n) => format!("gh pr merge {n} --squash; aida pull"),
                None => format!("open a {change_noun}, then merge"),
            },
            _ => format!("aida queue rework {spec_id}"),
        };
        println!("\n  {} recommended next: {}", "→".green(), recommend.cyan());
        return Ok(());
    }

    let merge_label = "Approve → print the merge command";
    let rework_label = "Request changes → aida queue rework";
    let diff_label = "Open the diff myself";
    let defer_label = "Defer (leave it held)";
    let options = vec![merge_label, rework_label, diff_label, defer_label];
    let choice = match inquire::Select::new("What now?", options).prompt() {
        Ok(c) => c,
        Err(_) => {
            println!(
                "  {} deferred — the spec stays held.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
            );
            return Ok(());
        }
    };

    if choice == merge_label {
        // NEVER auto-merge — print the paste-ready command for the human to
        // run. The review:draft-only gate exists precisely so a person, not
        // this verb, performs the merge. trace:STORY-553 | ai:claude
        match pr_number {
            Some(n) => {
                // No --delete-branch: the implementer's worktree may still
                // hold the PR branch (local-cleanup refusal); `;` so the
                // auto-bump pull always runs. trace:BUG-758 | ai:claude
                println!(
                    "\n  {} run to merge: {}",
                    "→".green(),
                    format!("gh pr merge {n} --squash; aida pull").cyan()
                );
                println!(
                    "  {} `aida pull` auto-bumps the spec Done → Completed once the merge lands; branch cleanup is deferred to worktree cleanup.",
                    crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
                );
            }
            None => println!(
                "\n  {} open a {change_noun} first, then merge it.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
    } else if choice == rework_label {
        // Route to rework in-process (reuse the existing handler).
        let storage = Storage::new(store_path);
        handle_queue_rework(
            &storage, &spec_id, false, None, false, None, None, false, false, false, None, false,
            None,
        )?;
    } else if choice == diff_label {
        match pr_number {
            Some(n) => {
                let diff_cmd = forge
                    .change_cmd_hint("diff", &n.to_string())
                    .unwrap_or_else(|| format!("inspect change {n}"));
                println!("  {} {}", "→".green(), diff_cmd.cyan());
                // Route the interactive diff opener through the Forge trait so
                // GitLab review surfaces run `glab mr diff`, not `gh pr diff`.
                // trace:STORY-1164 | ai:codex
                if let Err(e) = crate::forge::forge_for_kind(project_root, forge).diff_change(n) {
                    eprintln!(
                        "  {} could not open {change_noun}-{n} diff ({e:#})",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            None => println!(
                "  {} no open {change_noun} to diff.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            ),
        }
    } else {
        println!(
            "  {} deferred — the spec stays held.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
        );
    }

    Ok(())
}

pub(crate) fn handle_review_command(cmd: &ReviewCommand, storage: &Storage) -> Result<()> {
    match cmd {
        ReviewCommand::Prompt {
            specs,
            pr,
            forge,
            write,
        } => generate_review_prompt(
            storage,
            specs.as_deref(),
            *pr,
            forge.as_deref(),
            write.as_deref(),
        ),
        ReviewCommand::Assemble { output } => {
            let project_root = find_project_root()?;
            let out_path = rules_sync::assemble_review_md(&project_root, output.as_deref())?;
            println!(
                "{} Assembled root REVIEW.md at {}",
                crate::glyph(crate::glyphs::Glyph::Check),
                out_path.display()
            );
            Ok(())
        }
        // trace:BUG-775 | ai:claude
        ReviewCommand::Record {
            spec,
            verdict,
            sha,
            branch,
            summary,
            finding,
            finding_class,
            pr,
        } => handle_review_record(
            spec,
            verdict,
            sha.as_deref(),
            branch.as_deref(),
            summary.as_deref(),
            finding,
            finding_class,
            *pr,
        ),
        // trace:STORY-1417 | ai:claude
        ReviewCommand::Classes { since, json } => handle_review_classes(since.as_deref(), *json),
        // trace:STORY-1405 | ai:claude
        ReviewCommand::Claim {
            pr,
            sha,
            spec,
            ttl_mins,
            release,
        } => handle_review_claim(*pr, sha.as_deref(), spec.as_deref(), *ttl_mins, *release),
        // trace:BUG-775 | ai:claude
        ReviewCommand::Verdict { spec, json } => handle_review_verdict_show(spec, *json),
        ReviewCommand::List { json } => handle_review_list(*json),
        // trace:BUG-1516 | ai:claude
        ReviewCommand::NormalizeShas { dry_run } => handle_review_normalize_shas(*dry_run),
        // trace:TASK-1307 | ai:claude
        // trace:TASK-1423 | ai:claude
        ReviewCommand::Stranded { json, fix, age } => handle_review_stranded(*json, *fix, *age),
        // trace:STORY-1415 | ai:claude
        ReviewCommand::Mode { mode } => match mode {
            cli::ReviewModeCommand::MassChange { action, days, json } => {
                mass_change::handle_mass_change_command(action.as_str(), *days, *json)
            }
        },
    }
}

/// `aida review normalize-shas` — BUG-1516 criterion 3's repair verb: expand
/// every abbreviated `reviewed_sha` on disk to its full commit sha where this
/// repo can still resolve it, and mark the rest unresolvable rather than
/// guessing. Deliberately never runs on its own — `.aida/review-verdicts/` is
/// live coordination state other seats may be reading right now, so touching
/// it is always an explicit, operator-invoked action.
// trace:BUG-1516 | ai:claude
pub(crate) fn handle_review_normalize_shas(dry_run: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let report = review_verdict::backfill_abbreviated_shas(&project_root, dry_run)
        .with_context(|| "could not sweep .aida/review-verdicts for abbreviated shas")?;
    let verb = if dry_run { "would resolve" } else { "resolved" };
    println!(
        "{} {} {} abbreviated sha(s), {} unresolvable, {} already full, {} with no sha",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        verb,
        report.resolved.len(),
        report.unresolvable.len(),
        report.already_full,
        report.skipped_no_sha
    );
    for name in &report.resolved {
        println!("  {} {name}", "→".green());
    }
    for name in &report.unresolvable {
        println!(
            "  {} {name} (kept verbatim, marked reviewed_sha_unresolvable)",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    Ok(())
}

/// TASK-1307: read-only sweep for specs stranded by a refusal recorded
/// before BUG-1452 started protecting new ones — refused at the PR's
/// CURRENT head, spec still Done, no merge hold, no queue entry to re-drive
/// it. Walks the bounded `.aida/review-verdicts/` directory (never a full
/// store scan), asks the forge for each candidate's LIVE PR head sha so a
/// refusal against a superseded head is never mistaken for one against the
/// current head, and reports without mutating anything.
// trace:TASK-1307 | ai:claude
pub(crate) fn run_stranded_sweep(
    project_root: &std::path::Path,
) -> Result<Vec<stranded_sweep::StrandedRow>> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let mut spec_ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stranded_sweep::is_spec_keyed_verdict_filename(stem) {
                continue;
            }
            spec_ids.push(stem.to_string());
        }
    }
    spec_ids.sort();
    spec_ids.dedup();
    if spec_ids.is_empty() {
        return Ok(Vec::new());
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Vec::new());
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    // Condition 3 ("no queue entry exists to re-drive it") reads every
    // user's queue. Queue files are small and few — this is not the
    // full-store scan the storage-model convention warns against.
    let mut queued_ids: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    if let Ok(users) = backend.queue_users() {
        for user in users {
            if let Ok(entries) = backend.queue_list(&user, true) {
                for e in entries {
                    queued_ids.insert(e.requirement_id);
                }
            }
        }
    }

    let mut rows = Vec::new();
    for spec_id in spec_ids {
        let Some(verdict) = review_verdict::read_recorded_verdict(project_root, &spec_id) else {
            continue;
        };
        if !verdict.kind.blocks_done() {
            continue;
        }
        let Ok(Some(req)) = backend.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        let status_is_done = matches!(req.status, aida_core::RequirementStatus::Done);

        // BUG-1454's search-by-spec-id open-PR lookup. `None` means the
        // forge lookup itself failed; `Some(..)` without this spec means no
        // open PR was found. Either way there is nothing to strand without
        // a live open PR, so skip rather than guess one.
        let Some(open_prs) = specs_with_open_prs(project_root, [spec_id.clone()]) else {
            continue;
        };
        let Some(&pr) = open_prs.get(&spec_id) else {
            continue;
        };

        let mut sink = crate::network_retry::NoopSink;
        let Ok(meta) = forge::forge_for(project_root).change_metadata(pr, &mut sink) else {
            continue;
        };
        if meta.state != forge::ChangeState::Open {
            continue;
        }

        // No local ancestry probe: comparing the verdict's `reviewed_sha`
        // directly against the forge-reported live head is sufficient to
        // tell "refused at the current head" (exact match) from "refused at
        // a superseded head" (any mismatch reads as `Unknown` here, which
        // `classify_stranded` treats identically to a confirmed rewrite —
        // never flagged) — acceptance criterion 2, with no need to fetch
        // the branch locally.
        let relation = review_verdict::classify_tip_relation(
            verdict.reviewed_sha.as_deref(),
            Some(meta.head_sha.as_str()),
            None,
        );
        let hold_present = merge_hold::read_hold(project_root, pr).is_some();
        let queue_entry_present = queued_ids.contains(&req.id);

        let conditions = stranded_sweep::classify_stranded(
            status_is_done,
            hold_present,
            queue_entry_present,
            Some(&verdict),
            relation,
        );
        if conditions.is_stranded() {
            rows.push(stranded_sweep::StrandedRow {
                spec_id: req.display_id(),
                pr,
                conditions,
                verdict_summary: verdict.summary.clone(),
                reviewed_sha: verdict.reviewed_sha.clone(),
                current_head: Some(meta.head_sha.clone()),
            });
        }
    }
    Ok(rows)
}

/// TASK-1423: resolve a branch's CURRENT tip commit from LOCAL git objects
/// only — no `git fetch`, no forge call. Tries the local branch first (a
/// still-live worktree may have it), then the `origin` remote-tracking ref
/// (what a fetched-but-locally-deleted branch leaves behind — the normal
/// shape after a session that pushed and exited without merging). `None`
/// when neither resolves: offline and honest about what it doesn't know,
/// never a guess.
// trace:TASK-1423 | ai:claude
pub(crate) fn local_branch_tip(project_root: &std::path::Path, branch: &str) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    for refname in [
        format!("refs/heads/{branch}"),
        format!("refs/remotes/origin/{branch}"),
    ] {
        let Ok(out) = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--verify", "--quiet", &refname])
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    None
}

/// TASK-1423: the offline counterpart to [`run_stranded_sweep`] — every spec
/// whose last recorded verdict still blocks done, with no forge call: the
/// "did anything land since the review" check comes from `local_branch_tip`
/// (LOCAL git only) instead of asking the forge for the PR's live head, so
/// this stays fast and answerable with no network and no open-PR
/// requirement. Same file-discovery pass as `run_stranded_sweep` (every
/// spec-keyed file under `.aida/review-verdicts/`), so the two reports can
/// never disagree about which files exist.
// trace:TASK-1423 | ai:claude
pub(crate) fn run_stalled_sweep(
    project_root: &std::path::Path,
) -> Result<Vec<stranded_sweep::StalledRow>> {
    let dir = project_root.join(".aida").join("review-verdicts");
    let mut spec_ids: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stranded_sweep::is_spec_keyed_verdict_filename(stem) {
                continue;
            }
            spec_ids.push(stem.to_string());
        }
    }
    spec_ids.sort();
    spec_ids.dedup();
    if spec_ids.is_empty() {
        return Ok(Vec::new());
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Vec::new());
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let now = chrono::Utc::now();
    let mut rows = Vec::new();
    for spec_id in spec_ids {
        let Some(verdict) = review_verdict::read_recorded_verdict(project_root, &spec_id) else {
            continue;
        };
        if !verdict.kind.blocks_done() {
            continue;
        }
        let Ok(Some(req)) = backend.get_requirement_by_spec_id(&spec_id) else {
            continue;
        };
        let spec_completed = matches!(req.status, aida_core::RequirementStatus::Completed);
        if !review_verdict::is_outstanding_refusal(&verdict, spec_completed) {
            continue;
        }

        // No forge, no gh: the branch tip comes from whatever local git
        // already has on disk. `verdict.reviewed_branch` is what
        // `aida review record` stamps (defaulting to the checked-out branch
        // at record time), so it names the branch to look up.
        let tip = verdict
            .reviewed_branch
            .as_deref()
            .and_then(|b| local_branch_tip(project_root, b));
        let relation = review_verdict::classify_tip_relation(
            verdict.reviewed_sha.as_deref(),
            tip.as_deref(),
            None,
        );
        if !stranded_sweep::is_stalled(&verdict, spec_completed, relation) {
            continue;
        }

        let age = awaiting_you::blocked_age(verdict.recorded_at.as_deref(), now);
        rows.push(stranded_sweep::StalledRow {
            spec_id: req.display_id(),
            verdict_kind: verdict.kind,
            reviewed_sha: verdict.reviewed_sha.clone(),
            reviewed_branch: verdict.reviewed_branch.clone(),
            recorded_at: verdict.recorded_at.clone(),
            summary: verdict.summary.clone(),
            stall_secs: age.map(|a| a.secs),
            overdue: age.is_some_and(|a| a.overdue),
        });
    }
    stranded_sweep::sort_by_staleness(&mut rows);
    Ok(rows)
}

/// `aida review stranded --age` — TASK-1423. Answers "which specs have a
/// review verdict with no subsequent run, and for how long" entirely
/// offline: the per-spec verdict files plus local git, no forge call, no
/// open-PR requirement. Split out of BUG-1470 acceptance criterion 3 — the
/// session that filed it diagnosed a throughput stall as ten already-refused
/// PRs with zero head movement and no owner, invisible because nothing
/// reported "verdict, no subsequent run".
// trace:TASK-1423 | ai:claude
pub(crate) fn handle_review_stalled(project_root: &std::path::Path, json: bool) -> Result<()> {
    let rows = run_stalled_sweep(project_root)?;

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec": r.spec_id,
                    "verdict": r.verdict_kind.label(),
                    "reviewed_sha": r.reviewed_sha,
                    "reviewed_branch": r.reviewed_branch,
                    "recorded_at": r.recorded_at,
                    "summary": r.summary,
                    "stall_seconds": r.stall_secs,
                    "stall": r
                        .stall_secs
                        .map(crate::last_drain::format_age)
                        .unwrap_or_else(|| "unknown".to_string()),
                    "overdue": r.overdue,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": rows.len(),
                "stalled": arr,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "{} no stalled review verdicts found",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }
    println!(
        "{} {} spec(s) with a review verdict and no subsequent run — oldest first:",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        rows.len()
    );
    for row in &rows {
        let stall = row
            .stall_secs
            .map(crate::last_drain::format_age)
            .unwrap_or_else(|| "unknown".to_string());
        let overdue = if row.overdue { " — overdue" } else { "" };
        println!(
            "  {} {} at {} — {stall}{overdue}",
            row.spec_id.cyan(),
            row.verdict_kind.label(),
            row.reviewed_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("?"),
        );
        if let Some(s) = &row.summary {
            println!("      {}", s.dimmed());
        }
    }
    Ok(())
}

/// `aida review stranded` — TASK-1307. Default is a read-only report;
/// `--fix` is the separate explicit remediation pass (acceptance 3),
/// applying the same protection BUG-1452 now gives a fresh refusal: a merge
/// hold plus parking the spec in Needs Attention. Idempotent (acceptance
/// 4) because it re-derives the stranded set from live state each call — a
/// spec the previous `--fix` already held/parked no longer classifies as
/// stranded, so a second run changes nothing.
///
/// `--age` (TASK-1423) is a different report over the same directory of
/// verdict files: offline (local git only, no forge lookup, so it never
/// requires an open PR or `gh`), answering "which specs have a verdict with
/// no subsequent run, and for how long" — see `handle_review_stalled`.
// trace:TASK-1307 | ai:claude
pub(crate) fn handle_review_stranded(json: bool, fix: bool, age: bool) -> Result<()> {
    let project_root = find_project_root()?;
    if age {
        return handle_review_stalled(&project_root, json);
    }
    let rows = run_stranded_sweep(&project_root)?;

    if fix {
        let mut fixed_specs = Vec::new();
        for row in &rows {
            let reason = format!(
                "stranded refusal recovered for {} at {}",
                row.spec_id,
                row.reviewed_sha
                    .as_deref()
                    .map(review_verdict::short_sha)
                    .unwrap_or("unknown"),
            );
            // trace:STORY-1397 | ai:claude — a stranded refusal is rework.
            // BUG-1532 criterion 10: reference the verdict record, not a quote.
            // trace:BUG-1532 | ai:claude
            let mut hold = merge_hold::typed_hold(
                row.pr,
                merge_hold::HoldReasonKind::Rework,
                &reason,
                row.reviewed_sha.clone(),
            );
            let current = review_verdict::read_recorded_verdict(&project_root, &row.spec_id);
            hold.verdict_ref = Some(merge_hold::VerdictRef::new(
                &row.spec_id,
                Some(row.pr),
                row.reviewed_sha.clone(),
                current.and_then(|v| v.recorded_by),
            ));
            hold.spec = Some(row.spec_id.to_ascii_uppercase());
            merge_hold::write_typed_hold(&project_root, &hold)
                .with_context(|| format!("could not protect PR-{} with a merge hold", row.pr))?;
            let detail = row
                .verdict_summary
                .clone()
                .unwrap_or_else(|| "stranded refusal recovered by sweep".to_string());
            let recovery = format!(
                "address the review findings, move {} back to In Progress, and clear the PR hold only after approval",
                row.spec_id
            );
            if shelve_spec_on_failure(
                &project_root,
                &row.spec_id,
                "reviewer",
                3,
                "verdict:stranded",
                &detail,
                &recovery,
            )?
            .is_some()
            {
                fixed_specs.push(row.spec_id.clone());
            }
        }
        let remaining = run_stranded_sweep(&project_root)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "fixed": fixed_specs,
                    "remaining": remaining.iter().map(|r| &r.spec_id).collect::<Vec<_>>(),
                }))?
            );
        } else {
            println!(
                "{} recovered {} stranded spec(s); {} still require attention",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                fixed_specs.len(),
                remaining.len(),
            );
            for spec in &fixed_specs {
                println!("  {} {}", "→".green(), spec.cyan());
            }
            for row in &remaining {
                println!(
                    "  {} {} (PR-{}) — could not be parked; check manually",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    row.spec_id.cyan(),
                    row.pr
                );
            }
        }
        return Ok(());
    }

    // Acceptance 5: the count, once measured, is recorded durably rather
    // than only printed to a scrollback that will be gone by the time
    // anyone asks how many there were. Best-effort, append-only, and never
    // touches a spec — recording a measurement is not remediation.
    record_stranded_sweep_measurement(&project_root, rows.len());

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "spec": r.spec_id,
                    "pr": r.pr,
                    "status_not_moved": r.conditions.status_not_moved,
                    "hold_absent": r.conditions.hold_absent,
                    "queue_entry_absent": r.conditions.queue_entry_absent,
                    "verdict_refusing_at_head": r.conditions.verdict_refusing_at_head,
                    "reviewed_sha": r.reviewed_sha,
                    "current_head": r.current_head,
                    "summary": r.verdict_summary,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": rows.len(),
                "stranded": arr,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "{} no stranded specs found",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }
    println!(
        "{} {} stranded spec(s) — refused at the current head, still Done, unheld, unqueued:",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        rows.len()
    );
    for row in &rows {
        println!(
            "  {} PR-{} — refused at {}",
            row.spec_id.cyan(),
            row.pr,
            row.reviewed_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("?"),
        );
        if let Some(s) = &row.verdict_summary {
            println!("      {}", s.dimmed());
        }
    }
    println!(
        "  {} remediate with `aida review stranded --fix`",
        "→".dimmed()
    );
    Ok(())
}

/// Best-effort durable log of each real measurement — append-only, never
/// touches a spec. `.aida/stranded-sweep-history.jsonl` is local runtime
/// state (unshared, like `.aida/review-verdicts/`), but it is the record
/// TASK-1307's acceptance 5 asks for: the pre-sweep count is unrecoverable
/// once the underlying PRs are merged or closed, so the first real count
/// this sweep ever produces must not evaporate with scrollback.
// trace:TASK-1307 | ai:claude
pub(crate) fn record_stranded_sweep_measurement(project_root: &std::path::Path, count: usize) {
    let path = project_root
        .join(".aida")
        .join("stranded-sweep-history.jsonl");
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let line = serde_json::json!({
        "measured_at": chrono::Utc::now().to_rfc3339(),
        "count": count,
    });
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub(crate) fn guided_review_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         aida show {spec}\n\
         aida review {spec} --no-agent\n\n\
         You are the guided-review advisor for {spec}. Keep this interactive and bounded: \
         inspect the linked review surface, compare the diff to the spec acceptance criteria, \
         surface consequential review forks to the operator with concrete options and a \
         recommendation, and finish by telling the operator whether to approve, request changes, \
         inspect the diff manually, or defer. Do not auto-merge."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuidedReviewLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

pub(crate) fn guided_review_launch_request(spec: &str) -> GuidedReviewLaunchRequest {
    GuidedReviewLaunchRequest {
        role: Some("advisor".to_string()),
        // A guided review shell is an advisor reviewing an already-held surface,
        // not a new implementation claim. Keep the spec in the prompt/name only
        // so `agent new` does not run `session_start` for a Done spec.
        // trace:STORY-818 | ai:codex
        spec: None,
        prompt: guided_review_prompt(spec),
        name: Some(format!("guided-review-{}", slugify(spec))),
        description: Some(format!("guided review for {spec}")),
    }
}

pub(crate) fn derisk_prompt(spec: &str) -> String {
    format!(
        "Read your AIDA launch context first:\n\
         cat \"$AIDA_AGENT_CONTEXT_FILE\"\n\
         /aida-derisk {spec}\n\n\
         You are the de-risk advisor for {spec}. Follow the /aida-derisk skill: \
         read the spec and graph, classify decision/blast-radius risk, surface \
         load-bearing forks to the operator, record decisions as ADRs, fold \
         decisions and testable safety gates into acceptance, lint the result, \
         and only write a new execution_mode after explicit operator confirmation. \
         Do not implement the spec."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeriskLaunchRequest {
    pub(crate) role: Option<String>,
    pub(crate) spec: Option<String>,
    pub(crate) prompt: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
}

// trace:TASK-1235 | ai:codex
pub(crate) fn derisk_launch_request(spec: &str) -> DeriskLaunchRequest {
    DeriskLaunchRequest {
        role: Some("advisor".to_string()),
        // De-risking is an advisor disposition session, not an implementation
        // claim. Keep the target in the prompt/name so the launcher does not
        // acquire an implementer-style spec worktree or mutate status.
        spec: None,
        prompt: derisk_prompt(spec),
        name: Some(format!("derisk-{}", slugify(spec))),
        description: Some(format!("de-risk {spec}")),
    }
}

/// `aida derisk <SPEC>` — vendor-aware launcher for the `/aida-derisk` skill.
/// The Rust command is deliberately thin: it starts an advisor session seeded
/// with the skill prompt and leaves all mode decisions inside the skill's
/// explicit-confirmation workflow.
// trace:TASK-1235 | ai:codex
pub(crate) fn handle_derisk_command(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("aida derisk needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = derisk_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "aida derisk does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// STORY-818: `aida human review <SPEC> --guided` — spawn an interactive
/// advisor shell for review, using the existing `aida agent new` launch-context
/// machinery instead of inventing a second session substrate. The launched
/// agent deliberately has no `--spec` scope: review normally runs while the spec
/// is Done, and a spec-scoped `agent new` means implementation lease/worktree
/// semantics.
// trace:STORY-818 | ai:codex
pub(crate) fn handle_guided_human_review(spec: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("guided review needs an interactive terminal");
    }
    let vendor = session::resolve_session_vendor();
    let launch = guided_review_launch_request(spec);
    match vendor {
        session::HeadlessVendor::Claude => agent_new_claude(
            launch.role,
            launch.spec,
            false,
            None,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
            false,
        ),
        session::HeadlessVendor::Codex => agent_new_codex(
            launch.role,
            launch.spec,
            false,
            None,
            false,
            AgentContextOptions::new(true, false),
            true,
            false,
            false,
            false,
            AgentPromptOptions::new(Some(launch.prompt), false),
            AgentResumeOptions::new(false, None, true, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            launch.name,
            launch.description,
        ),
        session::HeadlessVendor::Agy => anyhow::bail!(
            "guided review does not run on agy; set the session vendor to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// `aida review record` — write the spec's review verdict as state a gate can
/// read. The recorded commit defaults to the branch tip, which is what makes
/// "has the branch moved since the review?" answerable later.
// trace:BUG-775 | ai:claude
/// BUG-802: the drive root a phase session should write orchestrator
/// handshake artifacts into — the env anchor the spawn/exec paths export,
/// falling back to `find_project_root`. The anchor exists because reviewers
/// routinely check the PR out elsewhere; from inside such a checkout,
/// `find_project_root` resolves to the CHECKOUT (it has its own `.aida/`),
/// and a verdict written there is invisible to the orchestrator. Proven live:
/// three consecutive phase-3 shelves on 2026-08-29, the last with a
/// byte-perfect verdict file in the wrong tree.
// trace:BUG-802 | ai:claude
pub(crate) fn drive_root_or_project_root() -> Result<std::path::PathBuf> {
    if let Some(root) = std::env::var_os("AIDA_DRIVE_ROOT") {
        let p = std::path::PathBuf::from(root);
        if p.is_dir() {
            return Ok(p);
        }
    }
    find_project_root()
}

// BUG-912: phase-3 reviewers may run inside the review worktree while the
// orchestrator polls an explicit verdict-file anchor in the parent project.
// The PR handshake must honor that exact file when present; otherwise prefer
// roots that phase 4 can observe before falling back to the worktree.
// trace:BUG-912 | ai:codex
pub(crate) fn review_pr_handshake_path(
    project_root: &std::path::Path,
    pr_number: u64,
) -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("AIDA_REVIEW_VERDICT_FILE")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return path;
    }
    if let Some(root) = std::env::var_os("AIDA_PROJECT_ROOT")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
    {
        return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(root) = parent_project_root_for_session(&cwd).filter(|p| p.is_dir()) {
            return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
        }
    }
    if let Some(root) = std::env::var_os("AIDA_DRIVE_ROOT")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
    {
        return review_verdict::verdict_path(&root, &format!("PR-{pr_number}"));
    }
    review_verdict::verdict_path(project_root, &format!("PR-{pr_number}"))
}

/// `aida review record` — emit the seat-tagged `ReviewVerdictRecorded` event
/// naming the spec (as `Event.spec`), PR, verdict and reviewed sha. Split out
/// for direct unit testing, same rationale as BUG-1423's `emit_ship_pr_merged`
/// in `pr_cmd.rs`. Best-effort: `events::emit` never fails the command.
// trace:TASK-1450 | ai:claude
pub(crate) fn emit_review_verdict_recorded(
    project_root: &std::path::Path,
    spec_display: &str,
    pr: Option<u32>,
    verdict: String,
    reviewed_sha: Option<String>,
) {
    let mut ev = events::Event::new(
        Some(spec_display.to_string()),
        "",
        events::EventKind::ReviewVerdictRecorded {
            pr,
            verdict,
            reviewed_sha,
        },
    );
    ev.seat = events::active_seat();
    events::emit(project_root, &ev);
}

/// STORY-1436: refusal recording seams that live in this module.
// trace:STORY-1436 | ai:claude
#[cfg(test)]
mod story_1436_gate_held_tests {
    use super::*;

    #[test]
    fn ambiguous_id_is_found_through_context_layers() {
        let amb = aida_core::id_collisions::AmbiguousIdError {
            id: "task-7".into(),
            candidates: vec![],
        };
        let err = anyhow::Error::new(amb)
            .context("loading spec")
            .context("aida edit");
        assert_eq!(
            ambiguous_id_in_chain(&err).map(|a| a.id.as_str()),
            Some("task-7")
        );
        assert!(ambiguous_id_in_chain(&anyhow::anyhow!("other")).is_none());
    }

    #[test]
    fn blocked_by_pickup_is_named_distinctly() {
        use aida_core::pickability::BlockedReason;
        use queue_cmd::{pickup_refusal_gate, QueueFreshPickup};
        let blocked = QueueFreshPickup::Blocked(BlockedReason::PermanentlyBlocked {
            target_spec: "TASK-1".into(),
        });
        assert_eq!(
            pickup_refusal_gate(&blocked),
            events::GATE_BLOCKED_BY_PICKUP
        );
        let human = QueueFreshPickup::Blocked(BlockedReason::HumanOnly);
        assert_eq!(pickup_refusal_gate(&human), events::GATE_QUEUE_PICKUP);
        assert_eq!(
            pickup_refusal_gate(&QueueFreshPickup::AwaitingMerge),
            events::GATE_QUEUE_PICKUP
        );
    }

    #[test]
    fn dry_run_pickup_refusal_is_not_recorded() {
        use aida_core::pickability::BlockedReason;
        let blocked = queue_cmd::QueueFreshPickup::Blocked(BlockedReason::PermanentlyBlocked {
            target_spec: "TASK-1".into(),
        });
        let _on = crate::test_env::EnvVarGuard::unset(events::EVENTS_DISABLE_ENV);
        let preview = tempfile::tempdir().unwrap();
        queue_cmd::record_pickup_refusal(preview.path(), &blocked, "S-1".into(), "r", true);
        assert!(events::read_all(preview.path()).is_empty());
        let real = tempfile::tempdir().unwrap();
        queue_cmd::record_pickup_refusal(real.path(), &blocked, "S-1".into(), "r", false);
        let evs = events::read_all(real.path());
        assert_eq!(evs.len(), 1);
        assert!(matches!(
            &evs[0].kind,
            events::EventKind::GateHeld { gate, .. } if gate == events::GATE_BLOCKED_BY_PICKUP
        ));
    }

    #[test]
    fn merge_hold_clear_floor_refusal_is_recorded_before_the_bail() {
        let src = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs"),
            include_str!("merge_hold_cmd.rs")
        );
        let clear = src
            .find(concat!("MergeHoldAction::", "Clear { pr, stale }"))
            .expect("clear arm");
        let tail = &src[clear..];
        let rec = tail
            .find(concat!("GATE_MERGE_HOLD_", "CLEAR_FLOOR"))
            .expect("floor refusal recorded");
        let bail = tail.find("anyhow::bail!(refusal)").expect("bail");
        assert!(rec < bail);
    }

    #[test]
    fn closure_hold_reason_names_blockers_and_criteria() {
        let hold = ClosureHold {
            flip: AutoBumpFlip::new(
                "STORY-9".to_string(),
                "abcdef1234".to_string(),
                RequirementStatus::Done,
            ),
            blockers: vec![],
            cycle_members: vec![],
            criteria: vec!["docs updated".to_string()],
        };
        let reason = closure_hold_reason(&hold);
        assert!(reason.contains("abcdef1"), "{reason}");
        assert!(reason.contains("docs updated"), "{reason}");
    }
}

#[cfg(test)]
mod task_1450_review_verdict_event_tests {
    use super::*;

    /// A recorded review verdict must emit a seat-tagged event naming the
    /// spec, PR, verdict and reviewed sha — the reviewer-seat coordination
    /// decision the BUG-1423 feed still missed.
    // trace:TASK-1450 | ai:claude
    #[test]
    fn review_record_emits_seat_tagged_spec_pr_verdict_and_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "reviewer", &[]);

        emit_review_verdict_recorded(
            tmp.path(),
            "BUG-1450",
            Some(2119),
            "approved".to_string(),
            Some("abc123def".to_string()),
        );

        let events = events::read_all(tmp.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].spec.as_deref(), Some("BUG-1450"));
        assert_eq!(events[0].seat.as_deref(), Some("reviewer"));
        assert!(matches!(
            &events[0].kind,
            events::EventKind::ReviewVerdictRecorded { pr, verdict, reviewed_sha }
                if *pr == Some(2119)
                    && verdict == "approved"
                    && reviewed_sha.as_deref() == Some("abc123def")
        ));
    }
}

/// BUG-1536 acceptance criteria 2 + 3 + 5: a test-only, construction-time
/// refusal to let [`handle_review_record_at`] write outside a tempdir. Every
/// filesystem write the recording path makes *from the resolved root* (the
/// review verdict and the merge-hold marker) and the one forge call it can
/// make (`merge_hold::sync_label`, which shells out with the SAME root as its
/// cwd — see `run_forge_cli`) are scoped to this one `project_root`. So
/// refusing here, in test builds only, to operate on a root that is not
/// under the process's temp directory is the belt-and-braces net for the
/// whole call: it holds regardless of which ambient env var a test did or
/// didn't set, because it does not consult any env var at all — it only
/// looks at the resolved root the call was actually given. Compiled out
/// entirely in a non-test build (`cfg(test)`), so it costs nothing and
/// changes no production behavior.
///
/// BUG-1743 amends the claim above. It said "every filesystem write ...
/// including the phase-3 handshake ... are scoped to this one
/// `project_root`", and for the handshake that was never true: it resolves
/// its OWN anchor through [`review_pr_handshake_path`], which by design
/// (BUG-912) prefers ambient `AIDA_REVIEW_VERDICT_FILE` /
/// `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT` over the root passed in. This
/// guard therefore cannot see the handshake write, and a root-under-`/tmp`
/// check could not have caught the leak anyway, because the root it leaked
/// to was another test's tempdir — also under `/tmp`. That gap is what
/// [`assert_handshake_anchor_is_pinned`] closes.
// trace:BUG-1536 | ai:claude
// trace:BUG-1743 | ai:claude
#[cfg(test)]
pub(crate) fn assert_review_write_root_is_isolated(project_root: &std::path::Path) {
    let tmp = std::env::temp_dir();
    let canon_tmp = tmp.canonicalize().unwrap_or(tmp);
    let canon_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    assert!(
        canon_root.starts_with(&canon_tmp),
        "BUG-1536: handle_review_record_at was about to write a review \
         verdict / merge-hold / forge label outside a tempdir (root = {}, \
         temp dir = {}). This looks like a test resolved its write root \
         ambiently instead of pinning an explicit tempdir — pass a tempdir \
         path in, not an env var.",
        canon_root.display(),
        canon_tmp.display(),
    );
}

/// BUG-1743: a test-only refusal to let the phase-3 handshake be written into
/// a temp tree this test does not own.
///
/// [`review_pr_handshake_path`] deliberately prefers ambient
/// `AIDA_REVIEW_VERDICT_FILE` / `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT` over
/// the root the call was given — that is BUG-912's orchestrator anchor and it
/// is correct in production, where those vars name a real repository. Under
/// `cargo test` the same preference is a hazard: `std::env` is
/// process-global, so a sibling test on another thread that pins
/// `AIDA_PROJECT_ROOT` at its own `tempfile` root redirects this call's
/// handshake into that root, and when the sibling's `TempDir` drops, the file
/// this call just wrote and verified ceases to exist. That is precisely the
/// two-faced flake BUG-1743 was filed for: the write failing with `ENOENT`
/// when the sibling had already torn down, and the `is_file()` check finding
/// nothing when it tore down a few syscalls later.
///
/// The discriminator is NOT the path — a foreign tempdir and an owned one are
/// both `/tmp/.tmpXXXXXX`. It is [`test_env::holds_env_lock`]: a test that
/// pointed the anchor somewhere on purpose did so under an `EnvVarsGuard`,
/// which holds `ENV_LOCK` for its whole lifetime and so cannot be racing
/// anyone. A test that resolved a foreign anchor while holding nothing
/// inherited it from a sibling, by accident, and is the bug.
///
/// So: an anchor outside `project_root` is allowed only while this thread
/// holds the env lock. Otherwise fail here, loudly, naming both paths —
/// deterministically at the moment of the mistake, instead of as a 2-in-287
/// CI flake whose message points at a file that no longer exists.
// trace:BUG-1743 | ai:claude
#[cfg(test)]
pub(crate) fn assert_handshake_anchor_is_pinned(
    project_root: &std::path::Path,
    handshake: &std::path::Path,
) {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // The handshake's own parents may not exist yet (the writer creates
    // them), so canonicalize the root and compare against the un-canonical
    // handshake as well as its existing ancestor.
    let canon_root = canon(project_root);
    let inside = handshake.starts_with(project_root) || handshake.starts_with(&canon_root);
    if inside || test_env::holds_env_lock() {
        return;
    }
    panic!(
        "BUG-1743: the phase-3 handshake was about to be written to {}, which \
         is OUTSIDE this call's project root {}, and this thread does not hold \
         ENV_LOCK. That means an ambient AIDA_REVIEW_VERDICT_FILE / \
         AIDA_PROJECT_ROOT / AIDA_DRIVE_ROOT was inherited from a SIBLING test \
         running in parallel, not set by this one. The handshake would land in \
         that sibling's tempdir and vanish when its TempDir drops — the flake \
         BUG-1743 is about. Fix the TEST, not this guard: hold a \
         `test_env::EnvVarsGuard` pinning those three keys (set the roots to \
         your own tempdir, unset AIDA_REVIEW_VERDICT_FILE) for the whole \
         window, which also serialises you against every sibling setter.",
        handshake.display(),
        canon_root.display(),
    );
}

/// STORY-1405: remove the review-in-progress marker(s) a recorded verdict
/// settles — PR-keyed when `pr` is known, otherwise every marker naming
/// `spec`. Checks the given root and the main clone (where merge surfaces
/// look). Returns the PRs cleared.
// trace:STORY-1405 | ai:claude
pub(crate) fn clear_review_markers_for_verdict(
    project_root: &std::path::Path,
    spec: &str,
    pr: Option<u64>,
) -> Vec<u64> {
    let mut roots = vec![project_root.to_path_buf()];
    let main = main_worktree_root_from(project_root);
    if main != project_root {
        roots.push(main);
    }
    let mut cleared = Vec::new();
    for root in &roots {
        let targets: Vec<u64> = match pr {
            Some(n) => vec![n],
            None => review_marker::list(root)
                .into_iter()
                .filter(|m| {
                    m.spec
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(spec))
                })
                .map(|m| m.pr)
                .collect(),
        };
        for n in targets {
            if review_marker::clear(root, n) {
                cleared.push(n);
            }
        }
    }
    cleared
}

/// `aida review claim` — STORY-1405's deliberate entry point for a review
/// that runs through no other aida verb (criterion 5d): a reviewer seat reading
/// a diff by hand. No process stays behind to watch, so the claim is TTL-only.
// trace:STORY-1405 | ai:claude
pub(crate) fn handle_review_claim(
    pr: u64,
    sha: Option<&str>,
    spec: Option<&str>,
    ttl_mins: u64,
    release: bool,
) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    let root = main_worktree_root_from(&project_root);
    if release {
        if review_marker::clear(&root, pr) {
            println!(
                "{} released the review claim on PR-{pr}",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!("no review claim on PR-{pr} — nothing to release");
        }
        return Ok(());
    }
    anyhow::ensure!(ttl_mins > 0, "`--ttl-mins` must be at least 1");
    // TASK-1459: an explicit claim has no process to watch, so cap it — an
    // unbounded/mistyped TTL could otherwise hold a PR "under review" for
    // days. Clamp with a note rather than refuse, so a generous-but-honest
    // value (or a scripted default) never hard-fails the claim.
    let (ttl_mins, clamped) = review_marker::clamp_claim_ttl_mins(ttl_mins);
    if clamped {
        eprintln!(
            "  {} `--ttl-mins` is capped at {}m (24h) for an explicit claim — clamped",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            review_marker::MAX_CLAIM_TTL_MINS,
        );
    }
    let head = sha
        .map(str::to_string)
        .or_else(|| std::env::var("AIDA_FROM_PR_HEAD_SHA").ok())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            pr_cmd::fetch_change_info_via_resolved_forge(
                &project_root,
                pr,
                crate::forge::resolve_open_change_forge_kind(&project_root),
            )
            .ok()
            .map(|info| info.head_oid)
        });
    if let Some(prev) = review_marker::read(&root, pr) {
        if review_marker::is_live(&prev) {
            eprintln!(
                "  {} replacing a live review claim on PR-{pr}: {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                prev.describe()
            );
        }
    }
    let mut marker = review_marker::Marker::for_this_process(
        pr,
        head.as_deref(),
        spec.map(|s| s.to_ascii_uppercase()).as_deref(),
        &review_recorded_by(&project_root).replace("aida review record", "aida review claim"),
    );
    marker.pid = 0;
    marker.ttl_secs = ttl_mins.saturating_mul(60);
    review_marker::write(&root, &marker)
        .with_context(|| format!("could not mark PR-{pr} as under review"))?;
    println!(
        "{} PR-{pr} is marked under review{} for {ttl_mins}m — merges wait for your verdict",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        head.as_deref()
            .map(|h| format!(" at {}", review_verdict::short_sha(h)))
            .unwrap_or_else(|| " (head unknown — every head is covered)".to_string()),
    );
    println!(
        "  {} `aida review record <SPEC> --verdict … --pr {pr}` clears it; \
         `aida review claim --pr {pr} --release` abandons it.",
        crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
    );
    Ok(())
}

/// `aida review record`'s CLI entry point. Resolves the write root exactly
/// ONCE (drive root, falling back to the found project root) and hands it to
/// [`handle_review_record_at`] — the ambient env is read here and nowhere
/// else in the recording path. BUG-1536: the previous shape resolved this
/// root here AND separately, deeper in the call, at the merge-hold write
/// site (preferring `AIDA_PROJECT_ROOT`) — two independent ambient reads that
/// could and did disagree (a seat shell has `AIDA_PROJECT_ROOT` set to the
/// real repo; pointing only `AIDA_DRIVE_ROOT` at a tempdir left the merge
/// hold + forge label call resolving against the real repo). A single
/// resolved root threaded explicitly through the whole call cannot diverge.
// trace:BUG-1536 | ai:claude
pub(crate) fn handle_review_record(
    spec: &str,
    verdict: &str,
    sha: Option<&str>,
    branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    finding_classes: &[String],
    pr: Option<u64>,
) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    handle_review_record_at(
        project_root,
        spec,
        verdict,
        sha,
        branch,
        summary,
        findings,
        finding_classes,
        pr,
    )
}

/// `aida review classes` — count recorded findings per defect class across
/// the whole verdict corpus (current files, per-sha archives, retained
/// rounds). Read-only.
// trace:STORY-1417 | ai:claude
pub(crate) fn handle_review_classes(since: Option<&str>, json: bool) -> Result<()> {
    let project_root = drive_root_or_project_root()?;
    let since = since.map(queue_cmd::parse_since_arg).transpose()?;
    let dir = project_root.join(".aida").join("review-verdicts");
    let report = review_classes::collect_class_report(&dir, since);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.classes.is_empty() && report.unknown.is_empty() {
        println!(
            "No classified findings{}.",
            if since.is_some() {
                " in that window"
            } else {
                ""
            }
        );
    } else {
        println!(
            "{:<32} {:>8} {:>9}  {}",
            "CLASS".bold(),
            "FINDINGS".bold(),
            "SUBJECTS".bold(),
            "LAST SEEN".bold()
        );
        for c in &report.classes {
            println!(
                "{:<32} {:>8} {:>9}  {}",
                c.class,
                c.findings,
                c.subjects.len(),
                c.last_seen
                    .as_deref()
                    .map(|s| s.get(..10).unwrap_or(s))
                    .unwrap_or("-")
            );
        }
        for (class, n) in &report.unknown {
            println!("{:<32} {:>8}  (not in the vocabulary)", class, n);
        }
    }
    println!(
        "{} classified, {} unclassified findings.",
        report.classified, report.unclassified
    );
    if report.classified == 0 {
        println!(
            "  {} record a class with `aida review record <SPEC> --finding <TEXT> --finding-class <CLASS>`; older findings stay unclassified.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed()
        );
    }
    Ok(())
}

/// The core of `aida review record`, taking its write root as an explicit
/// parameter rather than resolving it ambiently. Used directly by tests so a
/// test can pin the root to a tempdir and know — by construction, not by
/// convention — that nothing the call does can escape it. `handle_review_record`
/// is the only caller that resolves `project_root` from the environment.
// trace:BUG-1536 | ai:claude
pub(crate) fn handle_review_record_at(
    project_root: std::path::PathBuf,
    spec: &str,
    verdict: &str,
    sha: Option<&str>,
    branch: Option<&str>,
    summary: Option<&str>,
    findings: &[String],
    finding_classes: &[String],
    pr: Option<u64>,
) -> Result<()> {
    #[cfg(test)]
    assert_review_write_root_is_isolated(&project_root);
    // STORY-1417: resolve classes up front; a bad class is a warning, never a
    // reason to refuse the verdict. trace:STORY-1417 | ai:claude
    let (classes, class_warnings) =
        review_classes::resolve_finding_classes(findings, finding_classes);
    for w in &class_warnings {
        eprintln!(
            "  {} {w}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    let kind = review_verdict::VerdictKind::parse(verdict);
    if kind == review_verdict::VerdictKind::Unknown {
        anyhow::bail!(
            "unrecognised verdict `{verdict}` — use one of: approved, request-changes, rejected"
        );
    }
    let branch = branch
        .map(str::to_string)
        .or_else(|| current_branch_at(&project_root));
    // A from-PR reviewer receives the forge-resolved head in its launch
    // envelope. Prefer it to a checkout's HEAD, which may be stale or may be
    // the drive root rather than the review worktree.
    // trace:BUG-1467 | ai:codex
    let envelope_pr_sha = pr
        .and_then(|_| std::env::var("AIDA_FROM_PR_HEAD_SHA").ok())
        .filter(|s| !s.trim().is_empty());
    let forge_pr_sha = if sha.is_none() && envelope_pr_sha.is_none() {
        match pr {
            // TASK-1455: the verdict's head is read from the PINNED repo and
            // refused if the forge answers for any other repo, so a PR number
            // can never stamp this verdict with another repo's head.
            // trace:TASK-1455 | ai:claude
            Some(n) => Some(
                merge_hold::fetch_pinned_change(
                    &project_root,
                    crate::forge::resolve_open_change_forge_kind(&project_root),
                    n,
                )
                .and_then(|change| {
                    change
                        .ok_or_else(|| "this project has no forge (pure-git)".to_string())?
                        .head_sha
                        .ok_or_else(|| format!("the forge did not return a head SHA for #{n}"))
                })
                .map_err(|e| anyhow::anyhow!(e))
                .with_context(|| {
                    format!(
                        "could not resolve PR {n}'s current head for the verdict; pass `--sha <commit>` explicitly"
                    )
                })?,
            ),
            None => None,
        }
    } else {
        None
    };
    let resolved_sha = sha
        .map(str::to_string)
        .or(envelope_pr_sha)
        .or(forge_pr_sha)
        .or_else(|| resolve_commit_sha(&project_root, branch.as_deref().unwrap_or("HEAD")))
        .or_else(|| resolve_commit_sha(&project_root, "HEAD"));
    if resolved_sha.is_none() {
        anyhow::bail!(
            "no commit could be resolved for this review — pass `--sha <commit>`; an unstamped verdict is unverifiable and will not be recorded"
        );
    }

    // BUG-1983: a session that exports a grant which fails validation claims
    // a seat it does not hold; refuse rather than attribute the verdict to
    // it, with the same sentence the advisor-gated edit gives. No exported
    // grant stays permissive (agent identity, or the operator fallback below).
    // trace:BUG-1983 | ai:claude
    if let Some(rejection) = crate::seat_authority::grant_check(&project_root).rejection_sentence()
    {
        anyhow::bail!("refused: review record. {rejection}.");
    }

    // BUG-1802: bind recording identity and enforce policy.
    // trace:BUG-1802 | ai:antigravity
    let recorded_by = review_recorded_by(&project_root);
    #[cfg(not(test))]
    if recorded_by == "aida review record (operator)" {
        // BUG-1978: the refusal decision is unchanged; only its wording now
        // distinguishes a granted agent seat that never identified itself.
        // trace:BUG-1978 | ai:claude
        if let Err(err) = crate::seat_authority::require_direct_human() {
            let grant_seat =
                crate::seat_authority::current_grant(&project_root).map(|grant| grant.seat);
            return Err(review_record_attribution_refusal(
                err,
                grant_seat.as_deref(),
            ));
        }
    }
    if !crate::rework_hold_release::record_seat_allowed(
        crate::seat_authority::current_seat(&project_root).as_deref(),
    ) {
        anyhow::bail!("refused: implementer seat cannot record a review verdict (self-review)");
    }

    // STORY-1416 criterion 1b: recording a verdict for a PR surfaces the
    // standing marker (its text and who placed it) and any prior verdict
    // BEFORE either is overwritten below. Informational only.
    // trace:STORY-1416 | ai:claude
    if let Some(n) = pr {
        let standing = pr_claim_surface::read_pr_record(&project_root, n, &[spec]);
        pr_claim_surface::print(
            n,
            &pr_claim_surface::lines_for_verdict_write(
                &standing,
                &pr_claim_surface::IncomingVerdict {
                    key: &spec.trim().to_ascii_uppercase(),
                    kind,
                    sha: resolved_sha.as_deref(),
                    recorded_by: &recorded_by,
                },
            ),
        );
    }

    // A refusal is a protection event, not merely metadata. Arm the local
    // merge chokepoint before publishing the verdict; mirroring the label also
    // protects raw forge/UI merges when branch policy consumes it. The hold is
    // later approval can release only an eligible Rework-only hold through
    // the recorded release seam below; all other kinds keep human clear.
    // trace:BUG-1452 trace:STORY-1687 | ai:codex
    if kind.blocks_done() {
        let n = pr.ok_or_else(|| {
            anyhow::anyhow!(
                "a refusing verdict must name `--pr <N>` so AIDA can protect the pull request"
            )
        })?;
        let reason = format!(
            "{} for {} at {}",
            kind.label(),
            spec.to_ascii_uppercase(),
            resolved_sha
                .as_deref()
                .map(review_verdict::short_sha)
                .unwrap_or("unknown")
        );
        // BUG-1536: this used to re-resolve a SEPARATE "protection root" here
        // (preferring `AIDA_PROJECT_ROOT`, ambiently) rather than reusing the
        // one root this whole call was given. That second ambient read is
        // exactly what let a test's `AIDA_DRIVE_ROOT`-only tempdir pin leave
        // the hold, verdict and forge label call resolving against a seat
        // shell's real `AIDA_PROJECT_ROOT`. There is now exactly one root for
        // the whole call — `project_root`, the parameter — so a hold and its
        // label can never target a different tree than the verdict itself.
        // STORY-1397: the hold is typed Rework so it is not mistaken for a
        // recusal or supervision hold. trace:STORY-1397 | ai:codex
        // BUG-1532 criterion 10 / BUG-1562: the hold REFERENCES this verdict
        // record (key, PR, sha, recording seat) and states its release
        // condition, so a later round of the same verdict is followed rather
        // than a frozen quote of this one. trace:BUG-1532 trace:BUG-1562 | ai:claude
        let spec_key = spec.trim().to_ascii_uppercase();
        let mut hold = merge_hold::typed_hold(
            n,
            merge_hold::HoldReasonKind::Rework,
            &reason,
            resolved_sha.clone(),
        );
        hold.verdict_ref = Some(merge_hold::VerdictRef::new(
            &spec_key,
            Some(n),
            resolved_sha.clone(),
            Some(recorded_by.clone()),
        ));
        hold.spec = Some(spec_key.clone());
        hold.release_condition = Some(format!(
            "a fresh APPROVED verdict for {spec_key} recorded at PR #{n}'s current head; then a human ships it"
        ));
        merge_hold::write_typed_hold(&project_root, &hold)
            .with_context(|| format!("could not protect PR-{n} with a merge hold"))?;
        if let Err(err) = merge_hold::sync_label(&project_root, n, true) {
            eprintln!(
                "  {} merge-hold label not applied on PR-{n}: {err} — the local merge chokepoint remains armed; run `aida merge-hold list --fix`",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            );
        }
    }
    // STORY-1417: build + classify + one verified write — the same boundary
    // `record_verdict` uses, with the typed classes layered on before the
    // single durable write (BUG-1571). trace:STORY-1417 | ai:claude
    let path = review_verdict::verdict_path(&project_root, spec);
    let mut verdict_obj = review_verdict::build_verdict_object(
        &project_root,
        &path,
        Some(verdict),
        resolved_sha.as_deref(),
        branch.as_deref(),
        summary,
        findings,
        &recorded_by,
    )
    .with_context(|| "could not write the review verdict")?;
    // trace:STORY-1687 | ai:codex
    crate::rework_hold_release::stamp_record_grant(&project_root, &mut verdict_obj);
    review_classes::apply_finding_classes(&mut verdict_obj, findings, &classes);
    review_verdict::write_verdict_object(&path, &verdict_obj)
        .with_context(|| "could not write the review verdict")?;
    // PRIN-5 / BUG-1571: `record_verdict` already writes atomically and
    // verifies the bytes landed, but never print a path this process has
    // not itself just confirmed exists on disk.
    // trace:BUG-1571 | ai:claude
    if !path.is_file() {
        anyhow::bail!(
            "the review verdict at {} disappeared immediately after being written — not reporting it as recorded",
            path.display()
        );
    }
    // STORY-1405: the verdict is on disk, so the review is no longer "in
    // progress" — clear the PR's marker (by `--pr`, else any marker naming
    // this spec) so merge surfaces now read the verdict instead.
    // trace:STORY-1405 | ai:claude
    clear_review_markers_for_verdict(&project_root, spec, pr);
    // A verified record is required; direct artifact writers do not grant release.
    // trace:STORY-1687 | ai:codex
    if kind == review_verdict::VerdictKind::Approved {
        if let Some(pr) = pr {
            if let Err(error) = crate::rework_hold_release::auto_release(&project_root, pr) {
                eprintln!("  merge-hold remains armed: {error:#}");
            }
        }
    }
    // TASK-1450: a recorded review verdict is a reviewer-seat coordination
    // decision — the other gap BUG-1423's event feed left (that bug closed
    // the merge-path gap; this is the review-path gap). Emitted only after
    // the write above lands, so a failed record never produces a phantom
    // event. Best-effort: `events::emit` never fails the command.
    // trace:TASK-1450 | ai:claude
    emit_review_verdict_recorded(
        &project_root,
        &spec.to_ascii_uppercase(),
        pr.map(|n| n as u32),
        kind.label().to_string(),
        resolved_sha.clone(),
    );
    println!(
        "{} recorded {} for {}{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        kind.label().bold(),
        spec.to_ascii_uppercase().cyan(),
        resolved_sha
            .as_deref()
            .map(|s| format!(" against {}", review_verdict::short_sha(s)))
            .unwrap_or_default()
    );
    if kind.blocks_done() {
        let detail = summary
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| kind.label());
        let recovery = format!(
            "address the review findings, move {} back to In Progress, and clear the PR hold only after approval",
            spec.to_ascii_uppercase()
        );
        match shelve_spec_on_failure(
            &project_root,
            spec,
            "reviewer",
            3,
            match kind {
                review_verdict::VerdictKind::RequestChanges => "verdict:request-changes",
                review_verdict::VerdictKind::Rejected => "verdict:rejected",
                _ => unreachable!("blocks_done only covers refusing verdicts"),
            },
            detail,
            &recovery,
        )? {
            Some(_) => println!(
                "  {} {} parked in Needs Attention and surfaced by `aida awaiting`.",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                spec.to_ascii_uppercase()
            ),
            None => anyhow::bail!(
                "recorded the refusal and merge hold, but could not park {} in Needs Attention",
                spec.to_ascii_uppercase()
            ),
        }
        println!(
            "  {} `aida queue done {}` will refuse until the branch moves past that commit.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            spec.to_ascii_uppercase()
        );
    }
    println!(
        "  {} {}",
        "record:".dimmed(),
        path.display().to_string().dimmed()
    );

    // BUG-802: with --pr, also write the orchestrator's phase-3 handshake at
    // the orchestrator-visible anchor. The verdict string is the tolerant-parse
    // canonical label so `auto_complete::Verdict::parse` accepts it byte-for-byte.
    if let Some(n) = pr {
        let handshake = review_pr_handshake_path(&project_root, n);
        // trace:BUG-1743 | ai:claude
        #[cfg(test)]
        assert_handshake_anchor_is_pinned(&project_root, &handshake);
        // Read back the canonical record rather than rebuilding provenance.
        // Besides keeping the timestamp byte-identical, this carries the
        // full SHA produced by record_verdict's write-boundary normalization
        // when the caller supplied an abbreviation.
        // trace:BUG-1466 | ai:codex
        // trace:BUG-1516 | ai:codex
        let recorded = review_verdict::read_recorded_verdict(&project_root, spec)
            .ok_or_else(|| anyhow::anyhow!("the verdict was written but could not be read back"))?;
        // BUG-1571: build the full handshake object (base fields + the
        // orchestrator's `mode`/`recorded_at` overlay) in memory and commit
        // it with exactly ONE durable, verified write. The previous code
        // wrote the base record, read it back, patched two fields, and wrote
        // AGAIN — two separate `fs::write`s to the same path, each a window
        // where the artefact could fail to land while the command still
        // walked forward as if it had. `write_verdict_object` verifies the
        // bytes are actually readable back before this function is allowed
        // to claim the handshake exists.
        // trace:BUG-1581 | ai:codex
        // trace:BUG-1571 | ai:claude
        let mut handshake_obj = review_verdict::build_verdict_object(
            &project_root,
            &handshake,
            Some(kind.label()),
            recorded.reviewed_sha.as_deref(),
            recorded.reviewed_branch.as_deref(),
            summary,
            findings,
            recorded.recorded_by.as_deref().unwrap_or(&recorded_by),
        )
        .with_context(|| format!("could not prepare {}", handshake.display()))?;
        // trace:STORY-1417 | ai:claude
        review_classes::apply_finding_classes(&mut handshake_obj, findings, &classes);
        // The two artifacts describe the same act of review, so retain the
        // spec record's timestamp byte-for-byte while preserving any displaced
        // PR-keyed round through the shared writer above.
        handshake_obj.insert(
            "mode".to_string(),
            serde_json::Value::String("orchestrator-phase-3".to_string()),
        );
        if let Some(recorded_at) = recorded.recorded_at.as_deref() {
            handshake_obj.insert(
                "recorded_at".to_string(),
                serde_json::Value::String(recorded_at.to_string()),
            );
        }
        review_verdict::write_verdict_object(&handshake, &handshake_obj).with_context(|| {
            format!(
                "the phase-3 handshake was NOT written to {} — the orchestrator will not see this verdict; re-run `aida review record`",
                handshake.display()
            )
        })?;
        // Belt-and-suspenders: only ever print a path this process has just
        // confirmed exists. `write_verdict_object` already verified the
        // content by reading it back, but a missing/unreadable file at this
        // point must still block the success line rather than merely being
        // ignored. PRIN-5: never print a path that wasn't written.
        // trace:BUG-1571 | ai:claude
        if !handshake.is_file() {
            anyhow::bail!(
                "the phase-3 handshake at {} disappeared immediately after being written — not reporting it as recorded",
                handshake.display()
            );
        }
        println!(
            "  {} {} (phase-3 handshake — the orchestrator reads this to proceed)",
            "handshake:".dimmed(),
            handshake.display().to_string().dimmed()
        );
    }
    Ok(())
}

#[cfg(test)]
mod story_1405_review_marker_tests {
    use super::*;

    fn tempdir_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        root
    }

    // A reviewer's claim blocks a merge; recording the verdict (with --pr)
    // clears it, so the same merge gate then lets the merge through.
    //
    // BUG-1743: `--pr` also writes the phase-3 handshake, whose anchor is
    // resolved ambiently (`review_pr_handshake_path`), NOT from the root
    // passed in. Unpinned, this test wrote
    // `<real repo>/.aida/review-verdicts/PR-14051.json` into the operator's
    // checkout on every run — a fabricated APPROVED verdict keyed to a PR
    // number, invisible because `.aida/` is gitignored. Pin all three keys to
    // this test's own tempdir; the guard holds `ENV_LOCK` so no sibling can
    // move them mid-test. See `assert_handshake_anchor_is_pinned`.
    // trace:BUG-1743 | ai:claude
    #[test]
    fn recording_the_verdict_clears_the_marker_and_unblocks_merge() {
        let root = tempdir_root();
        let root_str = root.path().to_str().expect("tempdir path is utf-8");
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
        ]);
        let mut m = review_marker::Marker::for_this_process(
            14051,
            Some("abc1234"),
            Some("BUG-14051"),
            "reviewer seat",
        );
        m.pid = 0;
        m.ttl_secs = review_marker::CLAIM_TTL_SECS;
        review_marker::write(root.path(), &m).unwrap();
        assert!(matches!(
            review_marker::merge_gate(root.path(), 14051, || Some("abc1234".into())),
            review_marker::MergeGate::UnderReview(_)
        ));

        handle_review_record_at(
            root.path().to_path_buf(),
            "BUG-14051",
            "approved",
            Some("abc1234"),
            Some("bug-14051-work"),
            Some("looks right"),
            &[],
            &[],
            Some(14051),
        )
        .expect("recording an approval must succeed");

        assert!(review_marker::read(root.path(), 14051).is_none());
        assert_eq!(
            review_marker::merge_gate(root.path(), 14051, || panic!("no marker, no lookup")),
            review_marker::MergeGate::Clear
        );
    }

    #[test]
    fn verdict_without_pr_clears_markers_naming_the_spec_only() {
        let root = tempdir_root();
        for (pr, spec) in [(1, "TASK-1"), (2, "TASK-2")] {
            let m = review_marker::Marker::for_this_process(pr, Some("abc"), Some(spec), "r");
            review_marker::write(root.path(), &m).unwrap();
        }
        let cleared = clear_review_markers_for_verdict(root.path(), "task-1", None);
        assert_eq!(cleared, vec![1]);
        assert!(review_marker::read(root.path(), 2).is_some());
    }

    // TASK-1459: `aida review <SPEC>` prefers the forge's live PR head over
    // the local `origin/<branch>` tracking ref, falling back to that ref
    // when the forge can't be reached. A repo with no recognized forge
    // remote resolves to `ForgeKind::None` (`PureGitForge`, which errors
    // immediately with no subprocess — see `forge.rs`), so this exercises
    // the fallback deterministically with no `gh`/network dependency.
    #[test]
    fn marker_head_best_effort_falls_back_to_local_ref_without_a_forge() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .expect("git")
                .success());
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.test"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        let head = resolve_commit_sha(repo, "HEAD").expect("HEAD resolves");
        // A local remote-tracking ref, as if fetched, but no `origin` remote
        // URL is configured — `resolve_open_change_forge_kind` degrades to
        // `ForgeKind::None`.
        git(&["update-ref", "refs/remotes/origin/some-branch", "HEAD"]);

        let got = review_marker_head_best_effort(repo, 999, "some-branch");
        assert_eq!(got.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn marker_head_best_effort_is_none_with_no_forge_and_no_local_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "-q"])
            .status()
            .expect("git init")
            .success());
        assert_eq!(
            review_marker_head_best_effort(repo, 999, "no-such-branch"),
            None
        );
    }

    #[test]
    fn awaiting_annotation_is_head_scoped_and_read_only() {
        let root = tempdir_root();
        let m = review_marker::Marker::for_this_process(5, Some("aaa111"), None, "reviewer seat");
        review_marker::write(root.path(), &m).unwrap();
        assert!(review_marker::live_description(root.path(), 5, Some("aaa111")).is_some());
        assert!(review_marker::live_description(root.path(), 5, Some("bbb222")).is_none());
        assert!(review_marker::live_description(root.path(), 6, Some("aaa111")).is_none());
        assert!(review_marker::read(root.path(), 5).is_some());
    }
}

/// Audit identity for a supported seat-written verdict. The launched seat's
/// name is stable and distinguishable from the drain's own metadata writer.
// trace:BUG-1467 | ai:codex
pub(crate) fn review_recorded_by(project_root: &std::path::Path) -> String {
    review_recorded_by_from(
        std::env::var("AIDA_AGENT_NAME").ok().as_deref(),
        std::env::var("AIDA_AGENT_TYPE").ok().as_deref(),
        crate::seat_authority::current_seat(project_root).as_deref(),
    )
}

pub(crate) fn review_recorded_by_from(
    name: Option<&str>,
    kind: Option<&str>,
    role: Option<&str>,
) -> String {
    let role_disp = role.unwrap_or("reviewer");
    match (
        name.map(str::trim).filter(|s| !s.is_empty()),
        kind.map(str::trim).filter(|s| !s.is_empty()),
    ) {
        (Some(name), Some(kind)) => format!("{name} ({kind} {role_disp} seat)"),
        (Some(name), None) => format!("{name} ({role_disp} seat)"),
        (None, Some(kind)) => format!("{kind} {role_disp} seat"),
        (None, None) => "aida review record (operator)".to_string(),
    }
}

/// Error for `aida review record` when no agent identity is set and the
/// direct-human check refused. With a valid seat grant, the caller is an
/// agent seat that has not identified itself, so point at the identity
/// variables rather than `aida role enter` (which it cannot use anyway).
/// Without a grant, keep the existing role-enter guidance.
// trace:BUG-1978 | ai:claude
pub(crate) fn review_record_attribution_refusal(
    err: anyhow::Error,
    grant_seat: Option<&str>,
) -> anyhow::Error {
    match grant_seat {
        Some(seat) => anyhow::anyhow!(
            "aida review record attribution fallback: a valid {seat} seat grant was found, \
             but no agent identity is set; export AIDA_AGENT_TYPE (for example claude) and/or \
             AIDA_AGENT_NAME in this session, then re-run the review record command"
        ),
        None => err.context("aida review record attribution fallback"),
    }
}

#[cfg(test)]
mod bug_1978_attribution_message_tests {
    use super::*;

    fn direct_human_refusal() -> anyhow::Error {
        anyhow::anyhow!(
            "role grants require a human at an interactive TTY; run `aida role enter` from your terminal"
        )
    }

    #[test]
    fn grant_without_agent_identity_names_the_identity_variables() {
        let msg = format!(
            "{:#}",
            review_record_attribution_refusal(direct_human_refusal(), Some("advisor"))
        );
        assert!(msg.contains("seat grant was found"), "{msg}");
        assert!(msg.contains("no agent identity is set"), "{msg}");
        assert!(msg.contains("AIDA_AGENT_TYPE"), "{msg}");
        assert!(msg.contains("AIDA_AGENT_NAME"), "{msg}");
        assert!(!msg.contains("aida role enter"), "{msg}");
        // Agent output mode turns the first backticked `aida ...` command in
        // an error into a `help:` retry line; re-running the same command
        // without an identity would fail the same way, so none may be offered.
        assert_eq!(crate::extract_aida_suggestion(&msg), None, "{msg}");
        assert!(
            !msg.contains("BUG-"),
            "internal IDs must stay out of user text: {msg}"
        );
    }

    #[test]
    fn no_grant_keeps_the_existing_role_enter_guidance() {
        let msg = format!(
            "{:#}",
            review_record_attribution_refusal(direct_human_refusal(), None)
        );
        assert_eq!(
            msg,
            "aida review record attribution fallback: role grants require a human at an \
             interactive TTY; run `aida role enter` from your terminal"
        );
    }
}

#[cfg(test)]
mod bug_1467_reviewer_record_tests {
    use super::*;

    #[test]
    fn launched_reviewer_identity_is_distinct_from_the_drain_writer() {
        let identity = review_recorded_by_from(Some("review-pr-1986"), Some("claude"), None);
        assert_eq!(identity, "review-pr-1986 (claude reviewer seat)");
        assert_ne!(identity, "aida drain reviewer");
    }

    #[test]
    fn cold_reviewer_context_names_the_supported_verdict_writer() {
        let guidance = default_role_guidance("reviewer");
        assert!(guidance.contains("aida review record <SPEC> --pr <N>"));
        assert!(guidance.contains("never hand-write verdict JSON"));
    }
}

#[cfg(test)]
mod bug_1452_refusal_aftermath_tests {
    use super::*;
    use aida_core::db::DatabaseBackend;

    /// Criterion 5 asks for the aftermath OF A VERDICT, so the test drives
    /// `handle_review_record` and asserts what the RECORDING PATH did. The
    /// earlier version called `write_hold` and `shelve_spec_on_failure` itself
    /// and then asserted they had run — which proved only that two primitives
    /// work, and would have kept passing if the recording path stopped calling
    /// either one.
    ///
    /// `sha` and `branch` are passed explicitly so the call does not reach the
    /// forge for a head or the checkout for a branch: the forge lookup is
    /// skipped when `sha` is present, and `current_branch_at` only runs when
    /// `branch` is None.
    /// BUG-1743: the `--pr` argument below makes this call write a SECOND
    /// artifact, the phase-3 handshake, and that one does NOT go to the root
    /// passed in — `review_pr_handshake_path` prefers ambient
    /// `AIDA_REVIEW_VERDICT_FILE` / `AIDA_PROJECT_ROOT` / `AIDA_DRIVE_ROOT`,
    /// then the cwd's enclosing project, and only falls back to the explicit
    /// root. That is BUG-912's orchestrator anchor and is correct in
    /// production. This test read it ambiently, which made it write outside
    /// its own tempdir on every single run — measured 40/40 before the pin,
    /// in two shapes:
    ///
    /// - into a SIBLING test's `tempfile` root, whenever a parallel test held
    ///   `AIDA_PROJECT_ROOT` pinned at its own tempdir. The handshake landed
    ///   there, the sibling's `TempDir` dropped, and the file this call had
    ///   just written and read back ceased to exist — the two-faced CI flake
    ///   BUG-1743 was filed for (`ENOENT` at write time when the sibling had
    ///   already torn down, `!is_file()` a few syscalls later when it tore
    ///   down just after).
    /// - into the REAL repository, via the cwd branch, whenever no sibling
    ///   held a pin. `/home/joe/ai/aida/.aida/review-verdicts/PR-1452.json`
    ///   had accumulated three `rounds` of this fixture's payload
    ///   (`BUG-14520` / `abc123` / "the regression is still open") dated
    ///   09-25, 09-27 and 09-30. It went unnoticed because `.aida/` is
    ///   gitignored, so the pollution never showed in `git status`.
    ///
    /// Pinning all three keys under one `EnvVarsGuard` fixes both: the anchor
    /// now resolves to this tempdir, and — because the guard holds `ENV_LOCK`
    /// for its whole lifetime — no sibling can move it mid-test. The guard is
    /// what `assert_handshake_anchor_is_pinned` checks for, and the final
    /// assertion below is the positive half: the handshake is INSIDE `root`.
    // trace:BUG-1452 | ai:claude
    // trace:BUG-1743 | ai:claude
    #[test]
    fn refusal_aftermath_is_parked_held_and_awaiting_visible() {
        let root = tempfile::tempdir().unwrap();
        let root_str = root.path().to_str().expect("tempdir path is utf-8");
        let _pinned = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(root_str)),
            ("AIDA_DRIVE_ROOT", Some(root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
        ]);
        let store = root.path().join(".aida-store");
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();

        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let mut req = aida_core::Requirement::new("Refused work".into(), "desc".into());
        req.spec_id = Some("BUG-14520".into());
        req.status = aida_core::RequirementStatus::Done;
        backend.add_requirement(req).unwrap();
        drop(backend);

        // BUG-1536 removed the env pinning this test used to need, on the
        // grounds that `handle_review_record_at` takes its write root as an
        // explicit parameter and "uses it everywhere". That is true of the
        // hold, the verdict and the forge label call — but NOT of the phase-3
        // handshake `--pr` triggers, which keeps its own ambient resolution
        // order by design (BUG-912). So the pin above is back, and it is not
        // the pre-BUG-1536 pin returning: this one exists to stop the
        // handshake escaping and to hold `ENV_LOCK` against sibling setters,
        // not to steer the hold. See BUG-1743 and the doc comment above.
        // trace:BUG-1743 | ai:claude
        handle_review_record_at(
            root.path().to_path_buf(),
            "BUG-14520",
            "request-changes",
            Some("abc123"),
            Some("bug-14520-work"),
            Some("the regression is still open"),
            &["fix the regression".to_string()],
            &[],
            Some(1452),
        )
        .expect("recording a refusing verdict must succeed");

        // 1. the PR is HELD — written by the recording path, not by this test
        assert!(
            merge_hold::read_hold(root.path(), 1452).is_some(),
            "a refusing verdict must leave a merge-hold on the PR"
        );
        // BUG-1532 criterion 10: the hold REFERENCES the verdict record
        // (key, PR, sha, recording seat) and states its release condition.
        // trace:BUG-1532 | ai:claude
        let hold = merge_hold::read_hold_record(root.path(), 1452).unwrap();
        assert_eq!(hold.reason_kind, merge_hold::HoldReasonKind::Rework);
        let vref = hold
            .verdict_ref
            .as_ref()
            .expect("marker references the verdict");
        assert_eq!(vref.key, "BUG-14520");
        assert_eq!(vref.pr, Some(1452));
        assert_eq!(vref.reviewed_sha.as_deref(), Some("abc123"));
        assert!(vref.recorded_by.is_some());
        assert_eq!(hold.spec.as_deref(), Some("BUG-14520"));
        assert!(hold
            .release_condition
            .as_deref()
            .is_some_and(|c| c.contains("fresh APPROVED verdict for BUG-14520")));

        // 2. the verdict is recorded and blocks done
        let recorded = review_verdict::read_recorded_verdict(root.path(), "PR-1452")
            .or_else(|| review_verdict::read_recorded_verdict(root.path(), "BUG-14520"))
            .expect("the verdict must be readable after recording");
        assert!(
            recorded.kind.blocks_done(),
            "a request-changes verdict must block done"
        );

        // 3. the spec is PARKED, not Done, and visible to `aida awaiting`
        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let parked = backend
            .get_requirement_by_spec_id("BUG-14520")
            .unwrap()
            .unwrap();
        assert_ne!(
            parked.status,
            aida_core::RequirementStatus::Done,
            "a refused spec must not remain Done — that is the defect this spec is about"
        );
        assert_eq!(parked.status, aida_core::RequirementStatus::NeedsAttention);
        assert!(
            parked.failure_reason.is_some(),
            "failure_reason-backed NeedsAttention is what `aida awaiting` counts as shelved work"
        );

        // 4. BUG-1743: the phase-3 handshake landed INSIDE this test's own
        // tempdir. Asserting the file merely exists would not have caught the
        // defect — it existed, in someone else's temp tree or in the real
        // repository, and `handle_review_record_at` had already read it back
        // and confirmed its bytes. The claim that has to hold is about WHERE.
        // trace:BUG-1743 | ai:claude
        let handshake = review_pr_handshake_path(root.path(), 1452);
        assert!(
            handshake.starts_with(root.path()),
            "the phase-3 handshake anchor must resolve inside this test's own \
             tempdir {}, not {} — an escape here is a file this test cannot \
             keep alive and did not mean to write",
            root.path().display(),
            handshake.display()
        );
        assert!(
            handshake.is_file(),
            "the phase-3 handshake must be on disk at {}",
            handshake.display()
        );
    }

    /// BUG-1536 acceptance criterion 2: driving the recording path with a
    /// pinned tempdir root CANNOT write the merge-hold protection (the exact
    /// artifact the incident leaked) outside that root — even with a bogus PR
    /// number and even with the ambient env set exactly the way the incident
    /// had it: a stand-in "real repo" named by BOTH `AIDA_DRIVE_ROOT` and
    /// `AIDA_PROJECT_ROOT`. Before BUG-1536, `protection_root` preferred
    /// `AIDA_PROJECT_ROOT` over the resolved root passed in here, so this
    /// exact setup sent the hold and the (would-be) forge label call into
    /// `real_root`. Now there is one root for the whole call, so the hold and
    /// the spec-keyed verdict land only in `explicit_root`.
    ///
    /// Deliberately NOT asserted here: the PR-keyed *handshake* file
    /// (`review_pr_handshake_path`) legitimately follows `AIDA_PROJECT_ROOT`
    /// when set — that is a separate, pre-existing, intentional orchestrator
    /// anchor (BUG-912, covered by its own test
    /// `review_record_pr_handshake_honors_explicit_verdict_file`), not part
    /// of the BUG-1536 defect. Asserting `real_root` gets no `.aida/` at all
    /// would be wrong: the handshake write puts one there by design.
    // trace:BUG-1536 | ai:claude
    #[test]
    fn recording_never_escapes_the_explicit_root_even_when_ambient_env_points_elsewhere() {
        let explicit_root = tempfile::tempdir().unwrap();
        let store = explicit_root.path().join(".aida-store");
        std::fs::create_dir_all(explicit_root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            explicit_root.path().join(".aida/config.toml"),
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        let backend = aida_core::CachedGitBackend::open(
            &store,
            &aida_core::CachedGitBackend::default_cache_path(&store),
        )
        .unwrap();
        let mut req = aida_core::Requirement::new("Explicit-root work".into(), "desc".into());
        req.spec_id = Some("BUG-99999".into());
        req.status = aida_core::RequirementStatus::Done;
        backend.add_requirement(req).unwrap();
        drop(backend);

        // A stand-in "real repo": set BOTH ambient env vars the way the
        // incident's seat shell had them, to prove the explicit parameter —
        // not fallback ordering between the two — is what decides the
        // protection root.
        let real_root = tempfile::tempdir().unwrap();
        let real_str = real_root.path().to_str().expect("tempdir path is utf-8");
        let _ambient = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_DRIVE_ROOT", real_str),
            ("AIDA_PROJECT_ROOT", real_str),
        ]);

        // A bogus, unlikely-to-collide-with-anything-real PR number — the
        // exact shape of the incident, where the number happened to match a
        // real merged PR purely by chance.
        handle_review_record_at(
            explicit_root.path().to_path_buf(),
            "BUG-99999",
            "request-changes",
            Some("deadbeef"),
            Some("bug-99999-work"),
            Some("isolation check"),
            &["nothing real".to_string()],
            &[],
            Some(9_999_999),
        )
        .expect("recording against the explicit root must succeed regardless of ambient env");

        assert!(
            merge_hold::read_hold(explicit_root.path(), 9_999_999).is_some(),
            "the hold must land in the explicit root"
        );
        assert!(
            review_verdict::read_recorded_verdict(explicit_root.path(), "BUG-99999").is_some(),
            "the spec-keyed verdict must land in the explicit root"
        );

        assert!(
            merge_hold::read_hold(real_root.path(), 9_999_999).is_none(),
            "no hold may be written to the ambient-pointed root — this is the exact \
             artifact BUG-1536's `protection_root` used to leak there"
        );
        assert!(
            !real_root.path().join(".aida/merge-holds").exists(),
            "the ambient-pointed root must not even gain a `.aida/merge-holds/` directory"
        );
    }
}

/// `aida review verdict <SPEC>` — read the recorded verdict back.
// trace:BUG-775 | ai:claude
// BUG-1508: this used to print the raw verdict with no answer to "does it
// still cover the current head" -- forcing a human to compare shas by hand
// (the exact defect measured against the reviewer queue: 4 of 5 routed
// entries were already-refused-at-the-current-head, and nothing said so).
// The branch checked out here is the ONLY head this process can resolve
// without a forge call, so the actionability line is scoped to it; a
// verdict recorded for a different worktree/branch still prints, just
// without the head comparison. trace:BUG-1508 | ai:claude
pub(crate) fn handle_review_verdict_show(spec: &str, json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let path = review_verdict::verdict_path(&project_root, spec);
    match review_verdict::read_recorded_verdict(&project_root, spec) {
        Some(v) => {
            let branch = current_branch_at(&project_root);
            let relation =
                verdict_tip_relation(&project_root, branch.as_deref(), v.reviewed_sha.as_deref());
            let actionability = review_verdict::review_actionability(Some(&v), relation);
            if json {
                let body = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
                let mut value: serde_json::Value =
                    serde_json::from_str(body.trim()).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "actionability".to_string(),
                        serde_json::Value::String(actionability.as_str().to_string()),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!(
                    "{} {}",
                    format!("{}:", spec.to_ascii_uppercase()).bold(),
                    review_verdict::verdict_notice_line(&v)
                );
                println!("  {} {}", "actionability:".dimmed(), actionability.as_str());
                println!(
                    "  {} {}",
                    "record:".dimmed(),
                    path.display().to_string().dimmed()
                );
            }
            Ok(())
        }
        None => {
            let actionability = review_verdict::ReviewActionability::NeedsReview;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "verdict": null,
                        "actionability": actionability.as_str(),
                    })
                );
            } else {
                println!(
                    "{} no review verdict recorded for {} — actionability: {}.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec.to_ascii_uppercase(),
                    actionability.as_str()
                );
            }
            Ok(())
        }
    }
}

// trace:TASK-1590 | ai:antigravity
pub(crate) fn handle_review_list(json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let verdicts = review_verdict::list_active_verdicts(&project_root);

    if verdicts.is_empty() {
        if json {
            println!("[]");
        } else {
            println!(
                "{} No active review verdicts found.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
        }
        return Ok(());
    }

    let branch = current_branch_at(&project_root);
    let mut entries: Vec<(String, String, String, String, String, colored::Color)> = Vec::new();
    let mut json_out = Vec::new();

    let mut max_spec = 7;
    let mut max_verdict = 7;
    let mut max_action = 13;
    let mut max_sha = 3;

    for (spec, v) in verdicts {
        let relation =
            verdict_tip_relation(&project_root, branch.as_deref(), v.reviewed_sha.as_deref());
        let actionability = review_verdict::review_actionability(Some(&v), relation);
        let spec_upper = spec.to_ascii_uppercase();
        let verdict_str = format!("{:?}", v.kind);
        let action_str = actionability.as_str().to_string();
        let sha_str =
            review_verdict::short_sha(v.reviewed_sha.as_deref().unwrap_or("")).to_string();

        max_spec = max_spec.max(spec_upper.len());
        max_verdict = max_verdict.max(verdict_str.len());
        max_action = max_action.max(action_str.len());
        max_sha = max_sha.max(sha_str.len());

        let color = match actionability {
            review_verdict::ReviewActionability::Resolved => colored::Color::Green,
            review_verdict::ReviewActionability::AwaitingRework => colored::Color::Red,
            review_verdict::ReviewActionability::NeedsReview => colored::Color::Yellow,
        };

        if json {
            json_out.push(serde_json::json!({
                "spec_id": spec_upper,
                "verdict": verdict_str,
                "actionability": action_str,
                "sha": sha_str,
            }));
        } else {
            entries.push((
                spec_upper,
                verdict_str,
                action_str,
                sha_str,
                String::new(),
                color,
            ));
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&json_out)?);
        return Ok(());
    }

    println!(
        "{:<spec_w$}  {:<verdict_w$}  {:<action_w$}  {:<sha_w$}",
        "Spec ID".bold(),
        "Verdict".bold(),
        "Actionability".bold(),
        "SHA".bold(),
        spec_w = max_spec,
        verdict_w = max_verdict,
        action_w = max_action,
        sha_w = max_sha,
    );

    for (spec, verdict, action, sha, _ignored, color) in entries {
        println!(
            "{:<spec_w$}  {:<verdict_w$}  {:<action_w$}  {:<sha_w$}",
            spec,
            verdict,
            action.color(color),
            sha,
            spec_w = max_spec,
            verdict_w = max_verdict,
            action_w = max_action,
            sha_w = max_sha,
        );
    }

    Ok(())
}

/// trace:STORY-67 | ai:claude
pub(crate) fn generate_review_prompt(
    storage: &Storage,
    specs_csv: Option<&str>,
    pr: Option<u64>,
    forge_override: Option<&str>,
    write_path: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;

    // Resolve the spec list. Preference order: --specs explicit,
    // --pr range parse, error if neither.
    // trace:BUG-1434 | ai:claude
    let (spec_ids, header_subtitle, staleness_note): (Vec<String>, String, Option<String>) =
        if let Some(csv) = specs_csv {
            let ids: Vec<String> = csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if ids.is_empty() {
                anyhow::bail!("--specs was empty after splitting on commas");
            }
            (ids.clone(), format!("Specs: {}", ids.join(", ")), None)
        } else if let Some(pr_n) = pr {
            let project_root = find_project_root()?;
            let forge = forge_override
                .and_then(ReviewForge::parse)
                .or_else(|| detect_forge_from_origin(&project_root))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "couldn't detect forge from origin URL — pass --forge github|gitlab"
                    )
                })?;
            let (base, head) = pr_base_head(&project_root, forge, pr_n)?;
            let messages = git_log_messages(&project_root, &base, &head)?;
            let mut ids: Vec<String> = Vec::new();
            for msg in &messages {
                for id in extract_spec_ids_from_commit(msg) {
                    if !ids
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&id))
                    {
                        ids.push(id);
                    }
                }
            }
            if ids.is_empty() {
                anyhow::bail!(
                    "no `(REQ-ID)` trailers found in {}..{} ({} commits inspected)",
                    base,
                    head,
                    messages.len()
                );
            }
            let label = match forge {
                ReviewForge::GitHub => format!("PR #{} — branch `{}`", pr_n, head),
                ReviewForge::GitLab => format!("MR !{} — branch `{}`", pr_n, head),
            };
            let note = stale_base_note(&project_root, &head, &base);
            (ids, label, note)
        } else {
            anyhow::bail!("pass --specs <CSV> or --pr <N>");
        };

    // Compose the markdown.
    let mut out = String::new();
    out.push_str("# Review Prompt\n\n");
    out.push_str(&header_subtitle);
    out.push_str("\n\n");
    if let Some(note) = &staleness_note {
        out.push_str("## Staleness\n\n");
        out.push_str(note);
        out.push('\n');
    }
    out.push_str("## What to verify\n\n");

    let mut missing: Vec<String> = Vec::new();
    for id in &spec_ids {
        // TASK-72 polish: PR/MR pseudo-IDs (e.g., `PR-9`, `MR-42`) aren't
        // real specs in the store — they're forge references that can
        // sneak into the trailer-extracted list. Skip them silently so
        // the review prompt doesn't lead with a misleading
        // "PR-9 — (not found in store)" row before the real spec list.
        // trace:TASK-72 | ai:claude
        if parse_review_scope(id).is_some() {
            continue;
        }
        let req = store
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            .or_else(|| {
                uuid::Uuid::parse_str(id)
                    .ok()
                    .and_then(|u| store.requirements.iter().find(|r| r.id == u))
            });
        let Some(req) = req else {
            out.push_str(&format!("### {}\n\n_(not found in store)_\n\n", id));
            missing.push(id.clone());
            continue;
        };
        out.push_str(&format!(
            "### {} — {}\n\n",
            req.spec_id.as_deref().unwrap_or(id.as_str()),
            req.title
        ));
        match extract_acceptance_section(&req.description) {
            Some(body) => {
                out.push_str(&body);
                out.push_str("\n\n");
            }
            None => {
                out.push_str("_(no `## Acceptance` / `## Verify` section in description — review against the requirement title and description.)_\n\n");
            }
        }
        append_implementer_approach(&mut out, &req.comments);
    }

    out.push_str(review_prompt_test_commands_section());

    out.push_str("## Decide\n\n");
    out.push_str(
        "If every item above passes: approve and merge (`gh pr merge --squash` / \
         `glab mr merge --squash`), then mark each linked req `completed`.\n\n",
    );
    out.push_str(
        "If any item fails: request changes with specifics tied to the spec_id, \
         so the contributor can address them by id (not by paraphrase).\n",
    );

    if let Some(path) = write_path {
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write review prompt to {}", path))?;
        eprintln!(
            "{} review prompt written to {} ({} spec{}{})",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            path,
            spec_ids.len(),
            if spec_ids.len() == 1 { "" } else { "s" },
            if missing.is_empty() {
                String::new()
            } else {
                format!(", {} missing in store", missing.len())
            },
        );
    } else {
        print!("{}", out);
    }
    Ok(())
}

/// The heading the pickup skill writes and the review prompt reads.
///
/// ONE constant for BOTH SIDES OF A HANDSHAKE THAT SPANS A TEMPLATE AND A
/// PARSER. The writer is `aida-core/templates/skills/aida-pickup.md`, the
/// reader is `extract_implementer_approach` below, and nothing in the type
/// system connects them — edit the heading in the template alone and the
/// reader silently stops matching, leaving reviews quietly missing the
/// approach with no error anywhere. The coupling test in
/// `story_1350_review_approach_tests` reads the template and asserts this
/// exact string appears in it, so the two cannot drift apart in one edit.
// trace:STORY-1350 | ai:claude
pub(crate) const IMPLEMENTER_APPROACH_MARKER: &str = "## Implementer approach";

/// Return the newest durable implementation-intent comment. The exact first
/// heading is the stable marker written by the pickup skill; ordinary comments
/// that merely mention an approach must not leak into the review checklist.
// trace:STORY-1350 | ai:codex
pub(crate) fn extract_implementer_approach(
    comments: &[aida_core::models::Comment],
) -> Option<&str> {
    comments.iter().rev().find_map(|comment| {
        let first_line = comment.content.trim_start().lines().next()?;
        (first_line.trim_end() == IMPLEMENTER_APPROACH_MARKER).then_some(comment.content.trim())
    })
}

// trace:STORY-1350 | ai:codex
/// Emit the recorded approach, or SAY THAT THERE IS NONE.
///
/// Silence here is indistinguishable from "the implementer recorded nothing"
/// and from "the section was dropped", and a reviewer reading a prompt with no
/// approach section cannot tell which. Absent evidence has to look different
/// from good evidence, so the absence is stated rather than left as a gap.
// trace:STORY-1350 | ai:claude
pub(crate) fn append_implementer_approach(
    out: &mut String,
    comments: &[aida_core::models::Comment],
) {
    match extract_implementer_approach(comments) {
        Some(approach) => {
            out.push_str("#### Recorded implementer approach\n\n");
            out.push_str(approach);
            out.push_str("\n\n");
        }
        None => {
            out.push_str("#### Recorded implementer approach\n\n");
            // The marker comes from the constant here too. Spelling it out
            // again would make this the THIRD copy — the prose that TELLS a
            // human what to write, drifting from the parser that reads it, so
            // a rename would leave the system parsing X while instructing
            // people to write Y. The coupling test covers this string.
            out.push_str(&format!(
                "_None recorded._ The implementer did not leave a comment beginning \
                 `{IMPLEMENTER_APPROACH_MARKER}`, so this review has no stated intent \
                 to check the diff against.\n\n"
            ));
        }
    }
}

// trace:BUG-800 | ai:codex
pub(crate) fn review_prompt_test_commands_section() -> &'static str {
    "## Test Commands\n\n\
     When you name focused tests, name the runnable command, not a bare test \
     identifier. For Rust, use `cargo test -p <crate> <test_name>` (or the \
     exact workspace/package command you ran) so the next agent does not \
     pass the test name to the wrong binary.\n\n"
}

/// trace:STORY-67 | ai:claude
pub(crate) fn pr_base_head(
    project_root: &std::path::Path,
    forge: ReviewForge,
    n: u64,
) -> Result<(String, String)> {
    // STORY-1164: route the review-prompt metadata read through the Forge trait
    // instead of hand-rolling `gh pr view` / `glab mr view` here. The provider
    // owns CLI resolution, retry behavior, and GitLab's REST-shaped metadata.
    if let Ok(m) = crate::forge::forge_for_kind(project_root, forge.forge_kind())
        .change_metadata(n, &mut network_retry::NoopSink)
    {
        if !m.base_ref.is_empty() && !m.head_ref.is_empty() {
            return Ok((m.base_ref, m.head_ref));
        }
    }

    // Fallback: pure-git. We don't know the contributor's base branch,
    // so we point at `main` (the most common case in this codebase) and
    // set `head` to the local review branch (`pr-N` / `mr-N`) the user
    // is presumed to have fetched via `aida session start --owns PR-N`
    // (STORY-61). Tell the user.
    eprintln!(
        "{} couldn't resolve PR base/head via {} — falling back to base=main, head={}-{}; \
         pass an explicit base via `git log <base>..<head>` if this is wrong.",
        "Note:".yellow().bold(),
        forge.cli_name(),
        if matches!(forge, ReviewForge::GitHub) {
            "pr"
        } else {
            "mr"
        },
        n
    );
    let head = match forge {
        ReviewForge::GitHub => format!("pr-{}", n),
        ReviewForge::GitLab => format!("mr-{}", n),
    };
    Ok(("main".to_string(), head))
}

/// How many commits `origin/<base>` (the default branch's last-fetched
/// state) is ahead of `head` — i.e. how far the PR's tree trails the
/// branch its checks and findings are compared against. Purely local: it
/// reads whatever `origin/<base>` already points at and never fetches, so
/// it's cheap enough to run on every prompt generation.
///
/// Returns `None` when the count can't be resolved (no such
/// remote-tracking ref locally, `head` not resolvable, git not on PATH,
/// …). Callers must render that as "unknown", never as `0` — a `0` reads
/// as "you're current" while `None` means "we don't know", and conflating
/// them turns absent evidence into false reassurance.
// trace:BUG-1434 | ai:claude
pub(crate) fn commits_behind_default(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<u64> {
    let remote_base = format!("origin/{}", base);
    let range = format!("{}..{}", head, remote_base);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

/// `git diff --name-only <a>...<b>` (triple-dot: files changed on `b`
/// since its merge-base with `a`). Best-effort — any git failure yields an
/// empty list rather than an error, since overlap is a nicety layered on
/// top of the commits-behind count, not load-bearing on its own.
// trace:BUG-1434 | ai:claude
pub(crate) fn git_diff_name_only(project_root: &std::path::Path, a: &str, b: &str) -> Vec<String> {
    let range = format!("{}...{}", a, b);
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Files the intervening commits (`head..origin/<base>`) touch that the
/// PR's own commits (`base..head`) also touch — the overlap where a stale
/// base is most likely to distort a check or finding (acceptance #2 of
/// this bug). Best-effort like `git_diff_name_only`: any git failure
/// yields an empty overlap rather than an error.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_file_overlap(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Vec<String> {
    let remote_base = format!("origin/{}", base);
    let intervening = git_diff_name_only(project_root, head, &remote_base);
    let pr_own = git_diff_name_only(project_root, base, head);
    let pr_own_set: std::collections::HashSet<&str> = pr_own.iter().map(|s| s.as_str()).collect();
    let mut overlap: Vec<String> = intervening
        .into_iter()
        .filter(|f| pr_own_set.contains(f.as_str()))
        .collect();
    overlap.sort();
    overlap.dedup();
    overlap
}

/// Render the staleness section for `generate_review_prompt`'s `--pr` path.
/// `None` means "say nothing" — either the head is level with
/// `origin/<base>` (acceptance #5: no extra output when current) or the
/// figure legitimately can't be computed and the caller has already fallen
/// back to a different note. `commits_behind_default` returning `None`
/// (PRIN-5: absent evidence, not a false `0`) still produces `Some` text
/// here, stating "unknown" explicitly rather than omitting the section.
// trace:BUG-1434 | ai:claude
pub(crate) fn stale_base_note(
    project_root: &std::path::Path,
    head: &str,
    base: &str,
) -> Option<String> {
    match commits_behind_default(project_root, head, base) {
        None => Some(format!(
            "PR head is an unknown number of commits behind `{base}` — the local \
             `origin/{base}` ref couldn't be resolved, so staleness could not be computed. \
             Treat the checks and findings below as unverified against the current default \
             branch.\n"
        )),
        Some(0) => None,
        Some(n) => {
            let plural = if n == 1 { "" } else { "s" };
            let mut note = format!(
                "PR head is {n} commit{plural} behind `{base}` — the checks and findings below \
                 were computed against a tree that far behind the default branch.\n"
            );
            let overlap = stale_base_file_overlap(project_root, head, base);
            if !overlap.is_empty() {
                let files = overlap
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                note.push_str(&format!(
                    "This PR also touches file{} those {n} intervening commit{plural} touched, \
                     the case where findings are most likely distorted: {files}\n",
                    if overlap.len() == 1 { "" } else { "s" }
                ));
            }
            Some(note)
        }
    }
}

/// Run `git log <base>..<head> --pretty=format:%B%n--END--`. Returns
/// each commit message as a separate string. trace:STORY-67 | ai:claude
pub(crate) fn git_log_messages(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>> {
    let range = format!("{}..{}", base, head);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--pretty=format:%B%n--END--",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            "--",
        ])
        .output()
        .with_context(|| format!("running git log {}", range))?;
    if !out.status.success() {
        anyhow::bail!(
            "git log {} failed: {}",
            range,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let messages: Vec<String> = stdout
        .split("--END--")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(messages)
}

// trace:FR-0259 | ai:claude:high
/// BUG-298: find `aida-*` entries under `.claude/{skills,commands,hooks}` that
/// no longer correspond to a template the current binary ships — left behind
/// when a template was renamed/consolidated/retired. Compared by base name, so
/// a flat `aida-x.md` and a folder-form `aida-x/` both resolve to `aida-x`.
/// Symlinks are skipped — the in-repo dogfood `.claude/` is per-file symlinks
/// into the master templates and must never be pruned. trace:BUG-298 | ai:claude
pub(crate) fn detect_obe_aida_scaffold_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    // trace:TASK-1519 | ai:codex
    use std::collections::{HashMap, HashSet};
    const DIRS: [&str; 3] = ["skills", "commands", "hooks"];

    // Expected base names per scaffold dir, from the embedded template set.
    let mut expected: HashMap<&str, HashSet<String>> =
        DIRS.iter().map(|d| (*d, HashSet::new())).collect();
    for key in aida_core::templates::EMBEDDED_TEMPLATES.keys() {
        for dir in DIRS {
            if let Some(rest) = key.strip_prefix(&format!("{dir}/")) {
                let first = rest.split('/').next().unwrap_or(rest);
                let base = std::path::Path::new(first)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(first)
                    .to_string();
                expected.get_mut(dir).unwrap().insert(base);
            }
        }
    }

    let mut obe = Vec::new();
    for dir in DIRS {
        for parent in if dir == "skills" {
            vec![".claude", ".agents"]
        } else {
            vec![".claude"]
        } {
            let d = root.join(parent).join(dir);
            if parent == ".agents"
                && (!root
                    .join(".agents")
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_dir())
                    || !d.symlink_metadata().is_ok_and(|m| m.file_type().is_dir()))
            {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            let exp = &expected[dir];
            for entry in entries.flatten() {
                let path = entry.path();
                // Never touch symlinks (the dogfood per-file symlink layout).
                if path
                    .symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("aida-") {
                    continue;
                }
                let base = std::path::Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string();
                if !exp.contains(&base) {
                    // Shared portable skills belong to the operator unless we can
                    // prove AIDA generated this exact, unedited regular file.
                    if parent == ".agents" {
                        if !path.is_dir() {
                            continue;
                        }
                        let skill = path.join("SKILL.md");
                        if skill.symlink_metadata().is_err()
                            || skill
                                .symlink_metadata()
                                .is_ok_and(|m| !m.file_type().is_file())
                        {
                            continue;
                        }
                        let Ok(content) = std::fs::read_to_string(&skill) else {
                            continue;
                        };
                        if aida_core::scaffolding::refresh::refresh_disposition(&content)
                            != aida_core::scaffolding::refresh::RefreshDisposition::Pristine
                        {
                            continue;
                        }
                    }
                    obe.push(path);
                }
            }
        }
    }
    obe.sort();
    obe
}

/// Search requirements for a pattern
#[allow(clippy::too_many_arguments)]
pub(crate) fn grep_requirements(
    storage: &Storage,
    pattern: &str,
    ignore_case: bool,
    extended_regex: bool,
    after_context: usize,
    before_context: usize,
    context: Option<usize>,
    field_filter: Option<&str>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    feature_filter: Option<&str>,
    files_with_matches: bool,
    count_only: bool,
    invert_match: bool,
) -> Result<()> {
    use regex::RegexBuilder;

    let store = storage.load_for_read()?;

    // Build the regex pattern
    let regex = if extended_regex {
        RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid regex pattern")?
    } else {
        // Escape special regex characters for literal search
        let escaped = regex::escape(pattern);
        RegexBuilder::new(&escaped)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid pattern")?
    };

    // Parse field filter
    let fields: HashSet<&str> = if let Some(f) = field_filter {
        f.split(',').map(|s| s.trim()).collect()
    } else {
        [
            "title",
            "description",
            "comments",
            "tags",
            "owner",
            "feature",
            "spec_id",
        ]
        .iter()
        .copied()
        .collect()
    };

    // Context lines (C overrides A and B)
    let ctx_before = context.unwrap_or(before_context);
    let ctx_after = context.unwrap_or(after_context);

    let mut total_matches = 0;
    let mut matching_reqs = 0;

    for req in &store.requirements {
        // Apply filters
        if let Some(status_str) = status_filter {
            let req_status = req.status.to_string().to_lowercase();
            if !req_status.contains(&status_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(type_str) = type_filter {
            let req_type = req.req_type.to_string().to_lowercase();
            if !req_type.contains(&type_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(feature_str) = feature_filter {
            if !req
                .feature
                .to_lowercase()
                .contains(&feature_str.to_lowercase())
            {
                continue;
            }
        }

        // Collect matches from all fields
        let mut matches: Vec<GrepMatch> = Vec::new();

        // Search title
        if fields.contains("title") {
            if let Some(m) = search_field(&regex, "title", &req.title, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search description
        if fields.contains("description") {
            for m in search_multiline_field(
                &regex,
                "description",
                &req.description,
                ctx_before,
                ctx_after,
            ) {
                matches.push(m);
            }
        }

        // Search spec_id
        if fields.contains("spec_id") {
            if let Some(spec_id) = &req.spec_id {
                if let Some(m) = search_field(&regex, "spec_id", spec_id, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search owner
        if fields.contains("owner") {
            if let Some(m) = search_field(&regex, "owner", &req.owner, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search feature
        if fields.contains("feature") {
            if let Some(m) = search_field(&regex, "feature", &req.feature, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search tags
        if fields.contains("tags") {
            for tag in &req.tags {
                if let Some(m) = search_field(&regex, "tags", tag, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search comments
        if fields.contains("comments") {
            for comment in &req.comments {
                for m in search_multiline_field(
                    &regex,
                    &format!("comment:{}", comment.author),
                    &comment.content,
                    ctx_before,
                    ctx_after,
                ) {
                    matches.push(m);
                }
            }
        }

        let has_matches = !matches.is_empty();
        let should_show = if invert_match {
            !has_matches
        } else {
            has_matches
        };

        if should_show {
            matching_reqs += 1;
            let match_count = matches.len();
            total_matches += match_count;

            let id_string = req.id.to_string();
            let spec_id = req.spec_id.as_deref().unwrap_or(&id_string);

            if files_with_matches {
                // Just print the SPEC-ID
                println!("{}", spec_id.cyan());
            } else if count_only {
                // Print count for this requirement
                println!("{}: {}", spec_id.cyan(), match_count);
            } else if invert_match {
                // For invert match, just show SPEC-ID and title
                println!("{}: {}", spec_id.cyan(), req.title);
            } else {
                // Full output with matches
                println!("{}: {}", spec_id.cyan().bold(), req.title);
                for m in matches {
                    print_grep_match(&m);
                }
                println!();
            }
        }
    }

    // Summary
    if !files_with_matches && !count_only {
        if matching_reqs == 0 {
            if invert_match {
                println!("{}", "All requirements matched the pattern.".yellow());
            } else {
                println!("{}", "No matches found.".yellow());
            }
        } else {
            println!(
                "{} match(es) in {} requirement(s)",
                total_matches.to_string().green(),
                matching_reqs.to_string().green()
            );
        }
    }

    Ok(())
}

/// `aida worker gc`: prune `drain <SPEC-ID>` directives whose target spec is
/// archived or terminal (Completed / Rejected) from `.aida/worker.cmd`.
// trace:BUG-723 trace:BUG-1670 | ai:claude
pub(crate) fn run_worker_gc(project_root: &std::path::Path, dry_run: bool) -> Result<()> {
    let path = worker::worker_cmd_path(project_root);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if worker::parse_directives_from_str(&body).is_empty() {
        println!("No pending directives.");
        return Ok(());
    }
    let store_path = detect_distributed_store_from(project_root).ok_or_else(|| {
        anyhow::anyhow!("no requirement store found — cannot resolve directive target specs")
    })?;
    let backend = advance_backend(&store_path)?;
    // Both view axes wide open: an archived (or deferred) target must
    // still resolve so its directive is classified correctly. Strict: the
    // tolerant read serves an old snapshot while another process writes the
    // cache, and this pass deletes directives. trace:BUG-1670 | ai:claude
    let summaries = backend.list_summaries_strict(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    })?;
    // Dead = the spec still exists AND is archived or terminal
    // (Completed / Rejected) — same predicate as the queue's GC. A
    // directive targeting an unknown spec is LEFT alone (fail-safe:
    // no store row means no evidence the work shipped).
    let mut dead_ids = std::collections::HashSet::new();
    for s in &summaries {
        if s.archived || is_terminal_status_str(&s.status) {
            for id in [s.spec_id.as_deref(), s.agreed_id.as_deref()]
                .into_iter()
                .flatten()
            {
                dead_ids.insert(id.to_ascii_uppercase());
            }
        }
    }
    // Every prune candidate is re-read from its stored object before its
    // directive is dropped; a read error or a missing object keeps it.
    // trace:BUG-1670 | ai:claude
    let is_dead = |spec: &str| {
        dead_ids.contains(&spec.to_ascii_uppercase()) && worker_gc_target_still_dead(&backend, spec)
    };
    let outcome = worker::gc_directives_body(&body, &is_dead);
    if outcome.pruned.is_empty() {
        println!("No stale directives to prune.");
        return Ok(());
    }
    println!(
        "{} stale directive{} (target spec archived / Completed / Rejected):",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" }
    );
    for d in &outcome.pruned {
        println!("  - {}", d.raw);
    }
    if dry_run {
        println!("Dry run — file unchanged.");
        return Ok(());
    }
    std::fs::write(&path, &outcome.kept_body)?;
    let remaining = worker::parse_directives_from_str(&outcome.kept_body).len();
    println!(
        "Pruned {} directive{}; {} remain{}.",
        outcome.pruned.len(),
        if outcome.pruned.len() == 1 { "" } else { "s" },
        remaining,
        if remaining == 1 { "s" } else { "" }
    );
    Ok(())
}

/// Whether the stored object for a worker directive's target is still
/// archived or terminal. `get_requirement_by_spec_id` reads the spec's YAML,
/// not the cache row, so a spec reopened since any cache snapshot reads as
/// live. An unreadable or missing object is not proof the work shipped.
// trace:BUG-1670 | ai:claude
pub(crate) fn worker_gc_target_still_dead(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
) -> bool {
    use aida_core::db::DatabaseBackend;
    match backend.get_requirement_by_spec_id(spec) {
        Ok(Some(req)) => req.archived || is_terminal_status_str(&format!("{:?}", req.status)),
        _ => false,
    }
}
