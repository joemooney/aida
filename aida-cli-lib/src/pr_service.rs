//! Shared PR operations used by command handlers and other CLI modules.
// trace:TASK-3786 | ai:codex

use crate::{
    active_lease_for_cwd, extract_spec_ids_from_commit, find_main_worktree_root,
    load_store_for_lookup, main_worktree_root_from, network_retry, orchestrator, pr_rebase,
    pr_ship, punt, read_commits_in_range, render_auto_queue_outcome, resolve_aida_exe,
    resolve_gate_range, try_auto_queue_pr_review, AutoQueueOrigin, AutoQueueStatus, Storage,
};
use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};

/// The [`punt::HoldSignal`] `aida pr hold` records for `spec` on `branch`.
/// The held spec resolves once, here, against the committed store of the
/// project at `project_root` (STORY-2324 E3).
// trace:STORY-2324 | ai:claude
pub(crate) fn hold_signal(
    project_root: &std::path::Path,
    spec: &str,
    branch: &str,
    reason: Option<&str>,
) -> punt::HoldSignal {
    punt::HoldSignal {
        spec: spec.to_string(),
        branch: branch.to_string(),
        reason: reason.map(str::to_string),
        spec_key: crate::gate::canon::GateSpec::resolve(project_root, spec).key,
    }
}

/// Parse a local branch name as a review-snapshot ref: exactly `pr-<digits>`
/// (GitHub) or `mr-<digits>` (GitLab) — the shape `ReviewForge::local_branch_for`
/// creates. Anything else (an authored spec branch that happens to contain
/// "pr", `pr-161-fixup`, …) is out of scope by construction.
// trace:TASK-1312 | ai:claude
pub(crate) fn parse_review_snapshot_branch(branch: &str) -> Option<(crate::forge::ForgeKind, u64)> {
    let (prefix_len, kind) = if branch.starts_with("pr-") {
        (3, crate::forge::ForgeKind::GitHub)
    } else if branch.starts_with("mr-") {
        (3, crate::forge::ForgeKind::GitLab)
    } else {
        return None;
    };
    let rest = &branch[prefix_len..];
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u64>().ok().map(|n| (kind, n))
}

/// STORY-621 Slice 2: fetch a change's scalar metadata through the forge and
/// adapt it to the `pr_rebase::PrInfo` shape the rebase + reviewer pre-flight
/// paths consume. Enforces the same required fields `pr_rebase::parse_pr_info`
/// does (base/head ref + head SHA are hard errors — the recipes can't proceed
/// without them) and carries a forge-aware "is the CLI installed?" context so
/// a GitLab user is never told to install `gh`.
// trace:TASK-963 | ai:claude
pub(crate) fn fetch_change_info_via_forge(
    project_root: &std::path::Path,
    n: u64,
) -> Result<pr_rebase::PrInfo> {
    fetch_change_info_via_resolved_forge(
        project_root,
        n,
        crate::forge::resolve_forge_kind(project_root),
    )
}

// trace:BUG-1037 | ai:codex
pub(crate) fn fetch_change_info_via_resolved_forge(
    project_root: &std::path::Path,
    n: u64,
    forge_kind: crate::forge::ForgeKind,
) -> Result<pr_rebase::PrInfo> {
    let noun = forge_kind.change_noun();
    let cli = forge_kind.cli_name();
    let m = crate::forge::forge_for_kind(project_root, forge_kind)
        .change_metadata(n, &mut network_retry::NoopSink)
        .with_context(|| {
            if cli.is_empty() {
                format!("could not resolve {noun} metadata — this project has no forge (pure-git)")
            } else {
                format!(
                    "could not resolve {noun} metadata — is `{cli}` installed and authenticated?"
                )
            }
        })?;
    anyhow::ensure!(
        !m.base_ref.is_empty(),
        "the forge did not return a base branch for {noun} {n}"
    );
    anyhow::ensure!(
        !m.head_ref.is_empty(),
        "the forge did not return a head branch for {noun} {n}"
    );
    anyhow::ensure!(
        !m.head_sha.is_empty(),
        "the forge did not return a head SHA for {noun} {n}"
    );
    Ok(pr_rebase::PrInfo {
        n,
        base_ref: m.base_ref,
        head_ref: m.head_ref,
        head_oid: m.head_sha,
        is_cross_repository: m.is_cross_repository,
        head_repo_owner: m.head_repo,
        is_draft: m.is_draft,
    })
}

/// Run `gh pr view <N> --json …` and return the parsed JSON. Pulled
/// out of `pr_rebase_handler` so the side-effecting call is isolated
/// (parse rules are pinned by `pr_rebase::parse_pr_info` tests).
// trace:TASK-308 | ai:claude
pub(crate) fn fetch_pr_info_via_gh_bin(
    project_root: &std::path::Path,
    n: u64,
    gh_bin: &std::ffi::OsStr,
) -> Result<serde_json::Value> {
    let n_str = n.to_string();
    let mut command = std::process::Command::new(gh_bin);
    command.current_dir(project_root).args([
        "pr",
        "view",
        n_str.as_str(),
        "--json",
        "baseRefName,headRefName,headRefOid,isCrossRepository,headRepository,isDraft",
    ]);
    // trace:BUG-1689 | ai:codex
    let out = crate::process_retry::command_output_retrying_etxtbsy(&mut command).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("`gh` not on PATH — install from https://cli.github.com/")
        } else {
            anyhow::anyhow!("`gh pr view {}` failed to spawn: {}", n, e)
        }
    })?;
    if !out.status.success() {
        anyhow::bail!(
            "`gh pr view {}` exited {} — {}",
            n,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("`gh pr view {}` returned non-JSON output", n))?;
    Ok(json)
}

/// `aida pr ship [<N>]` — collapse the "create-if-needed + watch CI +
/// squash-merge + pull + worktree-aware cleanup" recipe into one call.
/// The direct-publish counterpart to `aida queue work PR-N
/// --auto-complete` — used for human-pre-approved work where no
/// orchestrator review phase is needed.
///
/// All gh subprocess calls go through the BUG-286 retry wrapper so a
/// sub-second network blip during `gh pr merge` doesn't abort the
/// flow. The post-merge `aida pull` and `aida session end` are spawned
/// as `aida` subcommands (resolved via the hardened executable resolver) so the wrapper
/// inherits the existing pull / session-end implementations (the BUG-108
/// cwd warning, the auto-bump scan, the live-claude refusal) without
/// double-implementing them.
///
// trace:TASK-458 | ai:claude
/// STORY-529/BUG-727: the ship-time spec-gate lookup — the spec-IDs
/// referenced by the commits about to ship, joined against the store as
/// `(id, tags, execution_mode)`. Reuses the trailer-gate machinery (range →
/// commits → store) so the ship gates see the same specs the trailer guard
/// validates. Empty when no store/commits resolve or nothing matches.
/// STORY-2884: the mode is the effective one — a set mode as before, or the
/// project's `[gate.unset_mode]` default for an unset one, with its source.
// trace:STORY-529 trace:BUG-727 trace:STORY-2884 | ai:claude
pub(crate) type ShipGateSpecRecord = (String, Vec<String>, crate::gate::unset_mode::EffectiveMode);
pub(crate) fn ship_gate_spec_records(project_root: &std::path::Path) -> Vec<ShipGateSpecRecord> {
    let range = resolve_gate_range(project_root, None);
    let Ok(commits) = read_commits_in_range(project_root, &range) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = Vec::new();
    for (_sha, subject) in &commits {
        ids.extend(pr_ship::extract_trailing_spec_ids_from_subject(subject));
    }
    ship_gate_spec_records_for_ids(project_root, ids)
}

/// [`ship_gate_spec_records`] for an explicit set of spec IDs: what
/// `aida pr train` reads for a member PR, whose commits are not the local
/// range.
// trace:STORY-1690 | ai:claude
pub(crate) fn ship_gate_spec_records_for_ids(
    project_root: &std::path::Path,
    mut ids: Vec<String>,
) -> Vec<ShipGateSpecRecord> {
    let Some(store) = load_store_for_lookup(project_root) else {
        return Vec::new();
    };
    ids.sort();
    ids.dedup();
    // STORY-2884: read from the default branch's config, once per lookup.
    let unset_mode = crate::gate::unset_mode::UnsetModePolicy::load(project_root);
    let mut out = Vec::new();
    for id in ids {
        let want = id.to_ascii_uppercase();
        let found = store.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
                || r.agreed_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
        });
        if let Some(req) = found {
            let tags: Vec<String> = req.tags.iter().cloned().collect();
            let gating = crate::gate::ship::gating_execution_mode(&store.requirements, req);
            out.push((id, tags, unset_mode.resolve(gating, req)));
        }
    }
    out
}

/// The spec-IDs referenced by the commits about to ship whose spec carries
/// the `review:draft-only` tag — the ship-time draft gate's input.
// trace:STORY-529 | ai:claude
pub(crate) fn draft_only_specs_for_ship(project_root: &std::path::Path) -> Vec<String> {
    draft_only_specs_in(&ship_gate_spec_records(project_root))
}

/// The `review:draft-only` filter over already-read spec records.
// trace:STORY-529 trace:STORY-1690 | ai:claude
pub(crate) fn draft_only_specs_in(records: &[ShipGateSpecRecord]) -> Vec<String> {
    records
        .iter()
        .filter(|(_, tags, _)| crate::gate::ship::is_draft_only_tagged(tags))
        .map(|(id, _, _)| id.clone())
        .collect()
}

/// The specs behind the commits about to ship whose `execution_mode` holds
/// the auto-merge, as `(spec-id, mode-label)` pairs — the supervised-merge
/// gate's input. Empty when the merge may proceed (all specs are `drain`, or
/// the caller is an interactive human at a TTY). STORY-2884: `change_paths`
/// (the files the PR changes) is read only when a spec takes the project's
/// unset-mode default, which a protected path withdraws.
// trace:BUG-727 trace:STORY-2884 | ai:claude
pub(crate) fn supervised_specs_for_ship(
    project_root: &std::path::Path,
    interactive_tty: bool,
    change_paths: impl FnOnce() -> Option<Vec<String>>,
) -> Vec<(String, String)> {
    let mut records = ship_gate_spec_records(project_root);
    if !interactive_tty {
        withdraw_unset_defaults(&mut records, change_paths);
    }
    supervised_specs_in(records, interactive_tty)
}

/// STORY-2884: withdraw the project's unset-mode drain default from `records`
/// when the change touches a protected gate path (as read with the setting)
/// or its files are unknown.
// trace:STORY-2884 | ai:claude
pub(crate) fn withdraw_unset_defaults(
    records: &mut [ShipGateSpecRecord],
    change_paths: impl FnOnce() -> Option<Vec<String>>,
) {
    crate::gate::unset_mode::withdraw_defaults_for_change(
        records.iter_mut().map(|(_, _, mode)| mode),
        change_paths,
    );
}

/// The BUG-727 supervised-merge filter over already-read spec records.
// trace:BUG-727 trace:STORY-1690 | ai:claude
pub(crate) fn supervised_specs_in(
    records: Vec<ShipGateSpecRecord>,
    interactive_tty: bool,
) -> Vec<(String, String)> {
    // STORY-1698: every spec read during a ship feeds the mandate's
    // append-only exclusion ledger. trace:STORY-1698 | ai:claude
    if let Ok(dir) = crate::gate::mandate::mandate_dir() {
        for (id, tags, mode) in &records {
            let mut req = aida_core::Requirement::new(String::new(), String::new());
            req.tags = tags.iter().cloned().collect();
            req.execution_mode = mode.mode;
            crate::gate::mandate::note_exclusions(&dir, id, [&req]);
        }
    }
    crate::gate::ship::effective_supervised_merge_holds(&records, interactive_tty)
}
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
/// Resolve the PR's base (target) branch for the merge-lease key, falling back
/// to `main`. A per-target-branch lease serializes only mergers heading to the
/// same branch.
// trace:STORY-1171 | ai:claude
pub(crate) fn pr_ship_target_branch(pr: u64) -> String {
    finish_ceremony_pr_base(pr).unwrap_or_else(|| "main".to_string())
}

/// Read PR `pr`'s real base branch (`baseRefName`) from the forge. Returns
/// `None` — never a guessed default — when `gh` is missing, the call fails,
/// or the field comes back empty; callers decide the fallback and whether
/// to say so.
// trace:TASK-1416 | ai:claude
pub(crate) fn finish_ceremony_pr_base(pr: u64) -> Option<String> {
    let mut command = std::process::Command::new("gh");
    command.args([
        "pr",
        "view",
        &pr.to_string(),
        "--json",
        "baseRefName",
        "-q",
        ".baseRefName",
    ]);
    crate::process_retry::command_output_retrying_etxtbsy(&mut command)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

// Emit the same `PrMerged` event the drain's merge phase emits
// (`DriveContext::merge_pr` in `lib.rs`), from `aida pr ship` — so a feed
// consumer cannot tell whether a merge came from the orchestrator or an
// explicit ship. One event per spec the PR is credited to; a PR crediting no
// resolvable spec still emits once with `spec: None` so the merge is never
// silently dropped. `emit` itself is best-effort (never errors the ship);
// isolated here so the target-derivation logic is unit-testable without
// stubbing `gh`. trace:BUG-1423 | ai:claude
pub(crate) fn emit_ship_pr_merged(main_worktree: &std::path::Path, pr: u32, spec_ids: &[String]) {
    let seat = crate::events::active_seat();
    let targets: Vec<Option<String>> = if spec_ids.is_empty() {
        vec![None]
    } else {
        spec_ids.iter().cloned().map(Some).collect()
    };
    for spec in targets {
        let mut ev = crate::events::Event::new(spec, "", crate::events::EventKind::PrMerged { pr });
        ev.seat = seat.clone();
        crate::events::emit(main_worktree, &ev);
    }
}

/// `aida pr ship` released the marker but could not drop the forge hold
/// label. The marker is gone, so the label is now a label-only hold: name
/// the human clear that drops it and the ship re-run, not a bare
/// `merge-hold clear` retry.
// trace:TASK-1951 | ai:claude
pub(crate) fn ship_unlabel_failure(pr: u64, err: &crate::merge_hold::LabelSyncError) -> String {
    format!(
        "could not remove the merge-hold label ({err}) — branch protection will keep the merge closed. Drop it with `aida merge-hold clear {pr}` when the forge is reachable, then re-run `aida pr ship {pr}`."
    )
}

/// BUG-1532 criterion 4: does a fresh verdict at `head` release this refusal
/// hold? Verdicts are read under every root (the main clone and the calling
/// worktree can both hold `.aida/review-verdicts/`); the first root with an
/// answer wins. Never clears anything.
///
/// With `record_proof` off a carry-over proven here is not written to the
/// gate ledger, so the answer leaves no state behind (a status report).
// trace:BUG-1532 | ai:claude
// trace:STORY-2768 | ai:claude
pub(crate) fn ship_refusal_release(
    roots: &[&std::path::Path],
    record: &crate::gate::merge_hold_core::MergeHoldRecord,
    head: Option<&str>,
    record_proof: bool,
) -> crate::gate::merge_hold_core::RefusalRelease {
    let exact = crate::gate::merge_hold_core::refusal_release(
        record,
        head,
        |key| {
            roots
                .iter()
                .find_map(|root| crate::gate::review_verdict::read_recorded_verdict(root, key))
        },
        |key, sha| {
            roots
                .iter()
                .find_map(|root| crate::gate::review_verdict::read_verdict_for_sha(root, key, sha))
        },
    );
    // trace:STORY-2694 | ai:codex
    crate::gate::merge_hold_core::refusal_release_with_coverage(record, exact, || {
        let root = roots.first().ok_or("no verdict root is available")?;
        let head = head.ok_or("the PR head is unavailable")?;
        let keys = crate::gate::merge_hold_core::refusal_verdict_keys(record);
        crate::gate::merge_hold_core::live_carried_approval(
            roots,
            root,
            record.pr,
            head,
            None,
            &keys,
            record_proof,
        )
    })
}

/// BUG-1918: what PR `pr` ships, for the approval gate — `known_ids` (from
/// the local range, branch, title) plus what the FORGE says: the spec ids in
/// the PR's head ref and in the trailers of every commit `base..head` adds.
/// The head branch returned is the forge's head ref when known. Resolved
/// from the forge so shipping from `main` (where the local range is empty)
/// or under a renamed local branch still finds what the PR ships. The third
/// value says whether the forge answered: when it did not, the ids are only
/// what the local title/branch claim, so the caller must judge authorship
/// against EVERY author (fail closed), using the ids only to find verdicts.
// trace:BUG-1918 | ai:claude
// trace:BUG-2702 | ai:claude
pub(crate) fn pr_gate_subject(
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
) -> (Vec<String>, Option<String>, bool, Vec<String>) {
    let (ids, branch, forge_resolved, mut refs_ids) =
        pr_gate_inputs(main_worktree, pr, known_ids, head_branch);
    // BUG-HOME-048: the commit-subject trailers pass the PR's coverage, so a
    // verdict is never looked up under a foreign bare id.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    let ids = crate::gate::trailer_coverage::covered_gate_ids(
        main_worktree,
        branch.as_deref().unwrap_or(""),
        Some(pr),
        ids,
    );
    refs_ids.extend(ids.iter().cloned());
    (
        normalized_spec_ids(&ids),
        branch,
        forge_resolved,
        normalized_spec_ids(&refs_ids),
    )
}

// trace:BUG-2727 | ai:codex
pub(crate) fn pr_gate_inputs(
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
) -> (Vec<String>, Option<String>, bool, Vec<String>) {
    let mut ids = known_ids.to_vec();
    // `(Refs X)` subjects name the work's spec for authorship only, never
    // for verdict lookup or completion. trace:BUG-2199 | ai:claude
    let mut refs_ids: Vec<String> = Vec::new();
    let mut forge_resolved = false;
    let mut branch = head_branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    // BUG-2702: a branch names a spec only by AIDA branch convention, so
    // `promote/integration-23` contributes nothing. trace:BUG-2702 | ai:claude
    if let Some(b) = &branch {
        ids.extend(pr_ship::extract_branch_spec_ids(b));
    }
    match crate::review_pr_subject_from_forge(main_worktree, pr) {
        Ok(subject) => {
            forge_resolved = true;
            let head = subject.head_ref.trim();
            if !head.is_empty() {
                ids.extend(pr_ship::extract_branch_spec_ids(head));
                branch = Some(head.to_string());
            }
            for s in &subject.commit_subjects {
                ids.extend(pr_ship::extract_trailing_spec_ids_from_subject(s));
                refs_ids.extend(pr_ship::extract_authorship_spec_ids_from_subject(s));
            }
        }
        Err(e) => eprintln!(
            "  warning: could not read PR-{pr}'s head and commits from the forge ({e:#}); \
             every recorded author is treated as one of its authors"
        ),
    }
    (normalized_spec_ids(&ids), branch, forge_resolved, refs_ids)
}

/// The spec IDs the ship approval gate looks verdicts up under: the gate
/// records for the local range, then every ID in the branch and the title.
/// `aida pr ship-batch` waits on the same set, so its "approved at head"
/// cannot drift from what ship accepts.
///
/// BUG-HOME-048: the whole list passes the PR's trailer coverage, so a
/// foreign bare id (`TASK-139`) is neither demanded nor accepted. The mandate
/// cover and the pre-merge approval gate use this same set.
// trace:STORY-1887 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn ship_gate_known_ids(
    project_root: &std::path::Path,
    branch: &str,
    title: Option<&str>,
    pr: Option<u64>,
) -> Vec<String> {
    let ids = ship_gate_input_ids(project_root, branch, title);
    crate::gate::trailer_coverage::covered_gate_ids(project_root, branch, pr, ids)
}

// trace:BUG-2727 | ai:codex
pub(crate) fn ship_gate_input_ids(
    project_root: &std::path::Path,
    branch: &str,
    title: Option<&str>,
) -> Vec<String> {
    let mut ids: Vec<String> = ship_gate_spec_records(project_root)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    // BUG-2702: the branch by convention only; the title as written.
    // trace:BUG-2702 | ai:claude
    ids.extend(pr_ship::extract_pr_spec_ids(branch, title.unwrap_or("")));
    ids
}

/// BUG-2184: what is left of `--wait SECS` for the CI wait: the time since the
/// wait started, less the mandate scan's own time.
// trace:BUG-2184 | ai:claude
pub(crate) fn ship_ci_budget(
    secs: u64,
    since_wait_started: std::time::Duration,
    scan_spent: std::time::Duration,
) -> std::time::Duration {
    std::time::Duration::from_secs(secs)
        .saturating_sub(since_wait_started.saturating_sub(scan_spent))
}

/// TASK-2348: the ship's CI handoff. Hands `watch` only what is left of the
/// one `--wait` deadline (never a renewed allowance); no `--wait` stays
/// unbounded. The seam lets tests observe the budget actually forwarded.
// trace:TASK-2348 | ai:claude
pub(crate) fn ship_ci_handoff<T>(
    wait: Option<u64>,
    since_wait_started: std::time::Duration,
    scan_spent: std::time::Duration,
    watch: impl FnOnce(Option<std::time::Duration>) -> T,
) -> T {
    watch(wait.map(|secs| ship_ci_budget(secs, since_wait_started, scan_spent)))
}

/// BUG-1918: the committer date of `sha` in `repo`, when the commit is
/// present locally.
// trace:BUG-1918 | ai:claude
pub(crate) fn commit_committed_at(
    repo: &std::path::Path,
    sha: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    // trace:BUG-2398 | ai:codex
    crate::gate::kernel_deps::commit_committed_at(repo, sha)
}

/// Trimmed, upper-cased, deduplicated, non-empty spec ids.
// trace:BUG-1918 trace:BUG-2398 | ai:codex
pub(crate) fn normalized_spec_ids(spec_ids: &[String]) -> Vec<String> {
    crate::gate::kernel_deps::normalized_spec_ids(spec_ids)
}

/// STORY-1690: fetch `base_branch` and the PR head `ship_branch` from origin,
/// so the behind-base gate measures against the base as the forge has it
/// now, not a local ref another machine, a web merge or another seat has
/// since moved past.
// trace:STORY-1690 | ai:claude
pub(crate) fn fetch_ship_refs(
    project_root: &std::path::Path,
    base_branch: &str,
    ship_branch: &str,
) -> std::result::Result<(), String> {
    let mut refspecs = vec![format!(
        "+refs/heads/{base_branch}:refs/remotes/origin/{base_branch}"
    )];
    if ship_branch != base_branch {
        refspecs.push(format!(
            "+refs/heads/{ship_branch}:refs/remotes/origin/{ship_branch}"
        ));
    }
    // TASK-2143: through the host-git chokepoint, which refuses a seat clone.
    let out = aida_core::git_ops::host_git_command(project_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .args(&refspecs)
        // trace:TASK-1952 | ai:claude
        .remote_credentials(project_root, "origin")
        .output()
        .map_err(|e| format!("git fetch: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git fetch origin {base_branch} {ship_branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// BUG-727: a human at an interactive terminal is the explicit merge the
/// supervised-merge gate waits for.
///
/// BUG-2427: decided by the same function as `merge-hold clear`
/// (`has_integrity_floor_authority`: stdin is a terminal and, inside a deputy
/// job, the deputy's TTY policy authorized it). Stdout plays no part: the
/// script deputy tees a job's stdout into its audited transcript, so a stdout
/// check refused a human-authorized `pr ship`/`pr train` that `merge-hold
/// clear` accepted. Defined before `ship_preflight_gates`: the preflight takes
/// the hold authority the handler decided and must not compute its own.
// trace:BUG-727 trace:BUG-2427 | ai:claude
pub(crate) fn ship_interactive_tty() -> bool {
    crate::gate::role_authority::has_integrity_floor_authority()
}

/// TASK-1602: whether this caller is a drive implementer/reviewer seat.
// trace:TASK-1602 | ai:codex
pub(crate) fn ship_caller_is_drive_seat() -> bool {
    std::env::var("AIDA_HEADLESS").is_ok_and(|v| v == "1")
        || std::env::var(orchestrator::AUTO_COMPLETE_ENV).is_ok_and(|v| !v.trim().is_empty())
}

/// TASK-1602: the in-drive ownership gate `aida pr ship` applies before it
/// merges PR `pr`: `Some` when a live drive owns the PR or the caller is a
/// drive seat. `aida pr train` applies it to every member.
// trace:TASK-1602 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn probe_ship_drive_block(
    main_worktree: &std::path::Path,
    pr: u64,
) -> Option<crate::gate::ship::ShipDriveBlock> {
    let allow_in_drive = std::env::var("AIDA_PR_SHIP_ALLOW_IN_DRIVE").is_ok_and(|v| v == "1");
    let scope = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(main_worktree, &cwd))
        .map(|lease| lease.scope);
    crate::gate::ship::probe_ship_drive_guard(
        main_worktree,
        pr,
        scope.as_deref(),
        ship_caller_is_drive_seat(),
        allow_in_drive,
    )
}

/// Merge-hold markers are canonical in the main clone, even when `aida pr ship`
/// is launched from a sibling implementation worktree.
// trace:BUG-1188 | ai:codex
pub(crate) fn pr_ship_hold_root(project_root: &std::path::Path) -> std::path::PathBuf {
    main_worktree_root_from(project_root)
}

/// Loud "IMPLEMENTER COMPLETE — EXIT NOW" banner printed at the end of
/// a successful `aida pr ship`. Substrate-as-bouncer signal for BUG-376:
/// an interactive implementer session that has just shipped a PR has
/// nothing left to do and must exit. Lingering to "watch CI" forces a
/// manual Ctrl+D from the operator and prevents the orchestrator from
/// advancing phases.
///
/// Worded so it stays correct when `--no-pull` / `--no-cleanup` / the
/// cwd-inside-worktree fallback caused step 4 or 5 to skip — those
/// alternate paths print their own next-step lines above; this banner
/// just reinforces that *the implementer session itself* is done.
///
/// Takes `&mut impl Write` so the rendering is unit-testable without
/// spawning a subprocess — mirrors the `status_cleanup::render` pattern.
///
/// `merged_this_run` (BUG-1537) distinguishes a merge THIS invocation
/// performed from one it found already done (BUG-574's `already_merged`
/// no-op path, e.g. a sibling seat merged first). Rendering the two cases
/// identically is exactly the PRIN-5 defect BUG-1537 records: the loud
/// first-person "IMPLEMENTER COMPLETE" banner claims an act ("PR-N is
/// merged... ran in steps 2-5 above") that this run did not perform when
/// `false`. In that case we print a short, honest "observed merged" line
/// instead — still telling the session there is nothing left to do, without
/// claiming credit for the merge.
// trace:BUG-376 trace:BUG-1537 | ai:claude
pub(crate) fn write_implementer_complete_banner(
    w: &mut impl std::io::Write,
    pr_number: u64,
    merged_this_run: bool,
) -> std::io::Result<()> {
    if !merged_this_run {
        writeln!(w)?;
        writeln!(
            w,
            "  {} PR-{} was already merged by another run before this `aida pr \
             ship` started — this run only observed it and ran the idempotent \
             post-merge steps (pull / auto-bump, lease release). Nothing was \
             merged by this session; nothing left to watch or re-merge.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr_number,
        )?;
        // BUG-376's exit directive is load-bearing for the implementer
        // session and must survive the honest wording (BUG-1537 review).
        // trace:BUG-1537 | ai:claude
        writeln!(
            w,
            "  {} — this implementer session has nothing left to do.",
            "EXIT NOW".bold()
        )?;
        writeln!(w)?;
        return Ok(());
    }
    let bar = "═".repeat(64);
    writeln!(w)?;
    writeln!(w, "{}", bar.bold())?;
    writeln!(
        w,
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
        "IMPLEMENTER COMPLETE — EXIT NOW".bold()
    )?;
    writeln!(w, "{}", bar.bold())?;
    writeln!(w)?;
    writeln!(
        w,
        "  PR-{} is merged. The post-ship phases (CI watch, merge, pull /",
        pr_number
    )?;
    writeln!(
        w,
        "  auto-bump, lease release) either ran in steps 2-5 above or are"
    )?;
    writeln!(
        w,
        "  listed there as their own next-step lines. Nothing left for this"
    )?;
    writeln!(w, "  implementer session to do.")?;
    writeln!(w)?;
    writeln!(
        w,
        "  Do NOT watch CI further — step 2 already gated on green before the"
    )?;
    writeln!(
        w,
        "  merge. Do NOT wait for the merge — step 3 already landed it. Do"
    )?;
    writeln!(
        w,
        "  NOT re-run `aida pull` or `aida status` to verify the auto-bump /"
    )?;
    writeln!(
        w,
        "  cleanup — any skipped step is recoverable from the lines above and"
    )?;
    writeln!(w, "  this session adds nothing by polling for it.")?;
    writeln!(w)?;
    writeln!(
        w,
        "  {} Press Ctrl+D to exit the chat session. Anything that remains is",
        "→".cyan().bold()
    )?;
    writeln!(
        w,
        "    owned by the orchestrator (--auto-complete) or the next-phase"
    )?;
    writeln!(w, "    agent — not this implementer.")?;
    writeln!(w)?;
    Ok(())
}

pub(crate) fn pr_ship_post_merge_aida_exe() -> std::path::PathBuf {
    resolve_aida_exe()
}

/// Open a change for a just-pushed `queue recover` branch through the forge,
/// deriving title/body from the branch head commit — the forge-routed
/// equivalent of the old `gh pr create --fill` (which has no trait method, but
/// `open_change` + commit-derived title/body is what `pr_ship_create_pr`
/// already does). Prints a forge-aware action hint and returns whether the
/// change opened.
// trace:TASK-961 trace:STORY-621 | ai:claude
pub(crate) fn recover_open_change(probe_repo: &std::path::Path, branch: &str) -> bool {
    use pr_ship::{derive_pr_body_from_commit, derive_pr_title_from_commit};
    let hint = crate::forge::resolve_open_change_forge_kind(probe_repo)
        .create_cmd()
        .unwrap_or_else(|| "open change".to_string());
    println!(
        "  {} {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
        hint
    );
    let commit_msg = match std::process::Command::new("git")
        .current_dir(probe_repo)
        .args(["log", "-1", "--format=%B"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return false,
    };
    let title = derive_pr_title_from_commit(&commit_msg);
    if title.is_empty() {
        return false;
    }
    let req = crate::forge::OpenChange {
        branch: branch.to_string(),
        base: crate::forge::default_branch_of(probe_repo),
        title,
        body: derive_pr_body_from_commit(&commit_msg),
        draft: false,
    };
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    match crate::gate::ledger::open_change_bound(
        &*crate::forge::forge_for_open_change(probe_repo),
        probe_repo,
        req,
        &extract_spec_ids_from_commit(&commit_msg),
    ) {
        Ok(change) => {
            // STORY-1480: adopting unshipped work IS the integrate-side
            // implementer hand-off — before this, such a spec's history
            // read UnshippedWorkDetected → SpecCompleted with everything
            // between unknown. Record the adoption pair: the phase entry
            // (work existed; the adopter is picking it up) and the PR-open
            // boundary, both with empty run_uuid like every interactive
            // emit. trace:STORY-1480 | ai:claude
            let root = main_worktree_root_from(probe_repo);
            let specs = extract_spec_ids_from_commit(&commit_msg);
            crate::events::emit_interactive_lifecycle(
                &root,
                &specs,
                &crate::events::EventKind::PhaseEntered {
                    idx: 1,
                    slug: "implementer".to_string(),
                    vendor: None,
                    seat: None,
                    model: None,
                    effort: None,
                    attempt: 1,
                },
            );
            if change.id != 0 {
                crate::events::emit_interactive_lifecycle(
                    &root,
                    &specs,
                    &crate::events::EventKind::PhaseDonePr {
                        pr: change.id as u32,
                    },
                );
            }
            true
        }
        Err(e) => {
            eprintln!(
                "{} auto-open failed: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
            false
        }
    }
}

/// TASK-140: the full HEAD commit message of a BRANCH (not the local cwd HEAD).
/// Tries the local ref first, then `origin/<branch>` (shipping a PR whose branch
/// isn't checked out locally). `None` when neither resolves. Caller takes the
/// first line as the squash subject.
// trace:TASK-140 | ai:claude
pub(crate) fn branch_head_commit_message(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    // A forge-reported PR head a fork author named. trace:BUG-1622 | ai:claude
    if crate::git_arg_guard::is_option_like(branch) {
        return None;
    }
    for r in [branch.to_string(), format!("origin/{branch}")] {
        let out = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "log",
                "-1",
                "--format=%B",
                crate::git_arg_guard::END_OF_OPTIONS,
                &r,
                "--",
            ])
            .output()
            .ok()?;
        if out.status.success() {
            let msg = String::from_utf8_lossy(&out.stdout).to_string();
            if !msg.trim().is_empty() {
                return Some(msg);
            }
        }
    }
    None
}

/// Return an explicit `gh pr merge --subject` value when GitHub's default
/// branch-head squash subject would drop spec IDs or a better PR title.
// trace:SPEC-410 TASK-142 | ai:codex
///
/// BUG-1919: also returns the completing trailer with its provenance, so the
/// caller can print where it came from (title / branch / withheld) before the
/// merge. `no_complete` is the `--no-complete` flag; the PR body's
/// non-completing marker and a closure-held spec (`closure:pending` or open
/// slice children) withhold the branch-name fallback inside the derivation.
// trace:BUG-1919 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    // TASK-140: compare against the PR BRANCH's head commit, NOT the local cwd
    // HEAD. `aida pr ship` frequently runs from the main worktree, where HEAD is
    // main's tip. TASK-142: prefer the PR title as the explicit squash subject
    // base because the branch head may itself be a merge commit.
    // trace:TASK-140 TASK-142 | ai:codex
    let pr = fetch_pr_ship_metadata_via_forge(project_root, pr_number)?;
    derive_pr_ship_squash_subject_from(project_root, &pr, branch, no_complete)
}

/// The derivation behind [`derive_pr_ship_squash_subject`], given the PR's
/// title and body already fetched, so tests drive it without a fake `gh` on
/// the process-wide PATH.
// trace:BUG-1997 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject_from(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    let commit_msg = branch_head_commit_message(project_root, branch).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the head commit of branch `{branch}` (tried local and origin/) \
             to derive the squash subject"
        )
    })?;
    derive_pr_ship_squash_subject_from_message(project_root, pr, branch, &commit_msg, no_complete)
}

/// [`derive_pr_ship_squash_subject_from`] given the head commit's message,
/// so a caller that holds the exact gated head (the merge train) reads the
/// subject at that commit, not at whatever a same-named local branch names.
// trace:TASK-2272 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject_from_message(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    commit_msg: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    let current_subject = pr_ship::derive_pr_title_from_commit(&commit_msg);
    // A store read is required before granting an ID completion authority.
    // trace:TASK-1600 | ai:codex
    // This deliberately does NOT use `load_store_for_lookup` (the best-effort
    // loader the calibration and rework credit paths use, which yields `None`
    // on an unreadable store): here the store decides which specs the merge
    // completes, so an unreadable store must fail closed and stop the merge
    // rather than silently skipping id resolution and the closure hold.
    // trace:BUG-1957 | ai:claude
    let store_root = main_worktree_root_from(project_root);
    let store = Storage::new(store_root.join(".aida-store")).load()?;
    // BUG-HOME-048: a title trailer completes only a spec this PR covers.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    let coverage =
        crate::gate::trailer_coverage::pr_trailer_coverage(project_root, &store, branch, pr.number);
    let refusals = std::cell::RefCell::new(Vec::new());
    let (normalized, mut trailer) = pr_ship::derive_squash_subject_with_trailer(
        &pr.title,
        branch,
        &pr.body,
        commit_msg,
        no_complete,
        |id| crate::gate::trailer_coverage::admits(&store, coverage.as_ref(), id, &refusals),
        |id| pr_ship::spec_completion_hold_reason(&store, id),
    )
    .unwrap_or_else(|| {
        (
            String::new(),
            pr_ship::SquashTrailer {
                ids: Vec::new(),
                source: pr_ship::TrailerSource::None,
                notes: Vec::new(),
            },
        )
    });
    trailer.notes.extend(refusals.into_inner());
    // BUG-1997: never return None when spec IDs are withheld (via --no-complete,
    // a PR-body marker, or closure hold, or when the PR title carried an ID that
    // was stripped). Otherwise, gh pr merge defaults to the PR title on multi-commit
    // PRs and can carry the withheld ID into the squash subject on main.
    // trace:BUG-1997 | ai:antigravity
    let raw_title_ids = pr_ship::extract_trailing_spec_ids_from_subject(&pr.title);
    let title_ids_withheld = raw_title_ids.iter().any(|id| !trailer.ids.contains(id));
    let ids_withheld = no_complete || !trailer.notes.is_empty() || title_ids_withheld;
    if normalized.is_empty() || (normalized == current_subject && !ids_withheld) {
        return Ok((None, trailer));
    }
    Ok((Some(normalized), trailer))
}

/// STORY-1663: method-aware subject + completion-trailer derivation.
/// Squash merges derive both the squash subject and completion trailer;
/// rebase merges produce `None` for the squash subject and suppress spec
/// completion unless `final_slice` (`--final-slice` / `--complete-spec`) is set.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn derive_pr_ship_subject_for_method(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
    no_complete: bool,
    method: crate::forge::MergeMethod,
    final_slice: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    if method != crate::forge::MergeMethod::Rebase {
        return derive_pr_ship_squash_subject(project_root, pr_number, branch, no_complete);
    }
    let pr = fetch_pr_ship_metadata_via_forge(project_root, pr_number)?;
    derive_pr_ship_subject_for_method_from(
        project_root,
        &pr,
        branch,
        no_complete,
        method,
        final_slice,
    )
}

/// Method-aware derivation given already-fetched `PrShipMetadata`.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn derive_pr_ship_subject_for_method_from(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    no_complete: bool,
    method: crate::forge::MergeMethod,
    final_slice: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    if method != crate::forge::MergeMethod::Rebase {
        return derive_pr_ship_squash_subject_from(project_root, pr, branch, no_complete);
    }
    let store_root = main_worktree_root_from(project_root);
    let store = Storage::new(store_root.join(".aida-store")).load()?;
    let coverage =
        crate::gate::trailer_coverage::pr_trailer_coverage(project_root, &store, branch, pr.number);
    let trailer = ship_completion_trailer_for_method(
        &store,
        coverage.as_ref(),
        method,
        final_slice,
        &pr.title,
        branch,
        &pr.body,
        no_complete,
    );
    Ok((None, trailer))
}

/// BUG-1919: the one store-qualified completion-trailer derivation the ship
/// path uses for the post-merge credit (calibration slots, the `PrMerged`
/// event), so it cannot disagree with the squash subject about which specs
/// the PR completed. Resolution and closure-hold both read `store`.
// trace:BUG-1919 | ai:claude
pub(crate) fn ship_completion_trailer(
    store: &aida_core::RequirementsStore,
    coverage: Option<&crate::gate::trailer_coverage::TrailerCoverage>,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
) -> pr_ship::SquashTrailer {
    ship_completion_trailer_for_method(
        store,
        coverage,
        crate::forge::MergeMethod::Squash,
        false,
        pr_title,
        branch,
        pr_body,
        no_complete,
    )
}

/// STORY-1663: method-aware store-qualified completion trailer derivation.
/// Rebase merges suppress spec completion unless `final_slice` (`--final-slice`
/// / `--complete-spec`) is set.
// trace:STORY-1663 | ai:antigravity
///
/// BUG-HOME-048: with `coverage` known, a title trailer that resolves to a
/// spec the PR does not cover is withheld, and the refusal names both ids.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn ship_completion_trailer_for_method(
    store: &aida_core::RequirementsStore,
    coverage: Option<&crate::gate::trailer_coverage::TrailerCoverage>,
    method: crate::forge::MergeMethod,
    final_slice: bool,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
) -> pr_ship::SquashTrailer {
    let refusals = std::cell::RefCell::new(Vec::new());
    let mut trailer = pr_ship::derive_ship_trailer_for_method(
        method,
        final_slice,
        pr_title,
        branch,
        pr_body,
        no_complete,
        |id| crate::gate::trailer_coverage::admits(store, coverage, id, &refusals),
        |id| pr_ship::spec_completion_hold_reason(store, id),
    );
    trailer.notes.extend(refusals.into_inner());
    trailer
}

/// TASK-1765: `body` with an `aida: no-complete` declaration line appended,
/// so the squash commit on main carries the non-completing marker the pull
/// reads (`commit_declares_non_completing`).
// trace:TASK-1951 trace:TASK-1765 | ai:claude
pub(crate) fn non_completing_squash_body(body: &str) -> String {
    let body = body.trim_end();
    let marker = pr_ship::NO_COMPLETE_BODY_MARKERS[0];
    if body.is_empty() {
        marker.to_string()
    } else {
        format!("{body}\n\n{marker}")
    }
}

pub(crate) struct PrShipMetadata {
    pub(crate) title: String,
    pub(crate) body: String,
    /// The PR number, when known: selects the review story whose `## Covers`
    /// count toward the trailer coverage check.
    /// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
    pub(crate) number: Option<u64>,
}

pub(crate) fn fetch_pr_ship_metadata_via_forge(
    project_root: &std::path::Path,
    n: u64,
) -> Result<PrShipMetadata> {
    // trace:BUG-2515 | ai:codex
    let kind = crate::forge::resolve_forge_kind(project_root);
    let (title, body) = crate::forge::forge_for_kind(project_root, kind)
        .change_title_and_body(n)
        .with_context(|| {
            format!(
                "could not resolve {} metadata via `{}`",
                kind.change_noun(),
                kind.cli_name()
            )
        })?;
    Ok(PrShipMetadata {
        title,
        body,
        number: Some(n),
    })
}

/// Append a JSONL activity-log entry to `<main_worktree>/.aida/advisor-activity.jsonl`.
/// Best-effort: silently swallows IO errors — losing a log line must
/// never abort the ship flow. Composes with STORY-405's planned
/// `aida status` activity surface.
pub(crate) fn log_ship_activity(
    main_worktree: &std::path::Path,
    pr_number: Option<u64>,
    step: &pr_ship::ShipStep,
    outcome: &pr_ship::StepOutcome,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let line = pr_ship::format_activity_event(&now, pr_number, step, outcome);
    append_ship_activity_line(main_worktree, &line);
}

/// Append one pre-formatted JSON line to `.aida/advisor-activity.jsonl`
/// (best-effort, like every activity-log write).
// trace:TASK-1458 | ai:claude
pub(crate) fn append_ship_activity_line(main_worktree: &std::path::Path, line: &str) {
    let aida_dir = main_worktree.join(".aida");
    if std::fs::create_dir_all(&aida_dir).is_err() {
        return;
    }
    let log_path = aida_dir.join("advisor-activity.jsonl");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = writeln!(f, "{}", line);
    }
}

/// [`append_ship_activity_line`] for a caller whose action depends on the
/// line being on record.
// trace:STORY-2809 | ai:claude
pub(crate) fn try_append_ship_activity_line(
    main_worktree: &std::path::Path,
    line: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    let aida_dir = main_worktree.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(aida_dir.join("advisor-activity.jsonl"))?;
    writeln!(f, "{line}")
}

/// Outcome of checking the intended base before offering to create a
/// change.
// trace:BUG-1610 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MrBasePreflight {
    /// The base exists on the remote and differs from the source — safe to
    /// offer.
    Ok,
    /// `origin` has no `refs/heads/<base>` — the intended base was never
    /// pushed.
    MissingRemoteBase,
    /// The base and the source branch are the same ref.
    SourceEqualsBase,
    /// `origin` could not be reached (offline, auth, hung connection), so
    /// the base could not be verified either way. Fails closed: no offer.
    OriginUnreachable,
}

/// Check whether `base` is a safe MR/PR target for `source_branch`: it must
/// exist on `origin` and must not be the same ref as the source. Only talks
/// to the remote (`git ls-remote`) — no local fetch required, so it is
/// correct even against a bare fixture `origin` with nothing fetched yet.
// trace:BUG-1610 | ai:claude
pub(crate) fn preflight_mr_base(
    project_root: &std::path::Path,
    source_branch: &str,
    base: &str,
) -> MrBasePreflight {
    if base == source_branch {
        return MrBasePreflight::SourceEqualsBase;
    }
    // Bounded the same way as `probe_branch_on_origin` (BUG-257): a hung
    // HTTPS dial must not freeze `aida review`, and no credential prompt may
    // block it. `--exit-code` exits 2 only when origin answered and has no
    // such ref; any other failure means origin was not reached.
    // trace:BUG-1610 | ai:claude
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--exit-code",
            "--heads",
            crate::git_arg_guard::END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            "origin",
            base,
        ])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .output();
    match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => MrBasePreflight::Ok,
        Ok(o) if o.status.code() == Some(2) => MrBasePreflight::MissingRemoteBase,
        _ => MrBasePreflight::OriginUnreachable,
    }
}

/// Refusal text for a non-`Ok` [`MrBasePreflight`] — names both branches and
/// gives the exact, provider-correct recovery sequence instead of letting
/// the caller offer an impossible change-create.
// trace:BUG-1610 | ai:claude
pub(crate) fn mr_base_diagnosis_message(
    forge_kind: crate::forge::ForgeKind,
    outcome: MrBasePreflight,
    source_branch: &str,
    base: &str,
) -> String {
    let change_noun = forge_kind.change_noun();
    let condition = match outcome {
        MrBasePreflight::Ok => return String::new(),
        MrBasePreflight::OriginUnreachable => {
            return format!(
                "not offering a {change_noun} from `{source_branch}`: could not reach \
                 `origin` to confirm that `{base}` exists there. Check the network or \
                 credentials and run the command again."
            );
        }
        MrBasePreflight::MissingRemoteBase => format!(
            "the intended base `{base}` does not exist on `origin`. This usually \
             happens when the repository was initialized before `{base}` was pushed, \
             so the forge made `{source_branch}` the project's default branch instead"
        ),
        MrBasePreflight::SourceEqualsBase => format!(
            "it is also the intended base `{base}` (the project's current default \
             branch) — a change cannot merge into itself"
        ),
    };
    let mut msg = format!(
        "refusing to offer a {change_noun} from `{source_branch}` — {condition}.\n\n\
         Recovery:\n  1. git push -u origin {base}\n"
    );
    match forge_kind {
        crate::forge::ForgeKind::GitLab => {
            msg.push_str(&format!(
                "  2. glab repo update --defaultBranch {base}\n  \
                 3. glab mr create --source-branch {source_branch} --target-branch {base}"
            ));
        }
        crate::forge::ForgeKind::GitHub => {
            msg.push_str(&format!(
                "  2. gh repo edit --default-branch {base}\n  \
                 3. gh pr create --head {source_branch} --base {base}"
            ));
        }
        crate::forge::ForgeKind::None => {
            msg.push_str("  2. open the change against that base through your forge's UI");
        }
    }
    msg
}

/// Persisted state of a change offered/attempted via `aida review`'s AC-5
/// (re)open-a-change prompt — enough to remember which base a retry should
/// target without the operator re-typing it every time.
// trace:BUG-1610 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MrRecoveryState {
    pub spec: String,
    pub source_branch: String,
    pub target_branch: String,
}

/// `.aida/mr-recovery/<spec>.json` under the project root — mirrors
/// [`crate::punt::hold_signal_path`]'s convention for small, spec-scoped JSON
/// markers.
// trace:BUG-1610 | ai:claude
pub(crate) fn mr_recovery_path(project_root: &std::path::Path, spec: &str) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("mr-recovery")
        .join(format!("{spec}.json"))
}

/// Write a recovery-state marker, creating `.aida/mr-recovery/` if needed.
///
/// Refuses (returns `Err`, writes nothing) when `source_branch ==
/// target_branch` — persisting that pair is exactly the self-referential
/// state BUG-1610 observed a hand-run `glab mr create --source-branch
/// <branch>` (no `--target-branch`) leave behind when the branch was also
/// the project's default. A later, corrected `--target-branch` retry must
/// never read that broken pair back (see [`resolve_mr_target_branch`]), so
/// it must never be written in the first place — enforced here rather than
/// trusted to every call site.
// trace:BUG-1610 | ai:claude
pub(crate) fn write_mr_recovery_state(
    path: &std::path::Path,
    state: &MrRecoveryState,
) -> Result<()> {
    if state.source_branch == state.target_branch {
        anyhow::bail!(
            "refusing to save MR recovery state for {} with source == target (`{}`)",
            state.spec,
            state.source_branch
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

/// Read a recovery-state marker. `None` when absent or unparseable — either
/// way the caller falls back to the resolved default base.
// trace:BUG-1610 | ai:claude
pub(crate) fn read_mr_recovery_state(path: &std::path::Path) -> Option<MrRecoveryState> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Resolve the target branch for a (re)tried change-create. An explicit
/// `--target-branch` ALWAYS wins over whatever a previous attempt saved —
/// a corrected retry must never be silently overridden by stale recovery
/// state (the exact BUG-1610 failure: `--recover --target-branch main`
/// still failing because a saved `story-52-work -> story-52-work` pair won).
// trace:BUG-1610 | ai:claude
pub(crate) fn resolve_mr_target_branch(
    explicit_target_branch: Option<&str>,
    saved: Option<&MrRecoveryState>,
    default_base: &str,
) -> String {
    if let Some(t) = explicit_target_branch {
        return t.to_string();
    }
    if let Some(s) = saved {
        return s.target_branch.clone();
    }
    default_base.to_string()
}

/// The one shared file-the-reviewer-story step for a PR that is open on
/// `branch`: file (or skip, if already filed) the review story, render the
/// outcome, clear a satisfied PR-hold marker, bump the covered specs to In
/// Progress, and on skips other than train refusals print the exact re-run
/// command. `aida pr auto-queue-review` and `aida ship --no-merge` (BUG-2217)
/// both run it, so a PR opened by either path gets its reviewer story whatever
/// the spec status — the auto-queue has no status gate.
// trace:BUG-2217 | ai:claude
pub(crate) fn queue_review_for_open_pr(
    project_root: &std::path::Path,
    branch: &str,
) -> crate::AutoQueueOutcome {
    // Try to associate this run with the active session lease so the
    // review-story description records who opened the PR. Falls back to
    // a synthetic "(no-session)" id if no lease covers the cwd.
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    let lease_scope = lease.as_ref().map(|l| l.scope.clone());
    let session_id = lease
        .map(|l| l.id)
        .unwrap_or_else(|| "(no-sess)".to_string());

    let outcome =
        try_auto_queue_pr_review(project_root, branch, &session_id, AutoQueueOrigin::PrSkill);
    render_auto_queue_outcome(&outcome);

    // Train branches always refuse review-story creation; retrying the same
    // command cannot help. Keep the refusal without suggesting that retry.
    // trace:TASK-2963 | ai:codex
    if branch.starts_with(crate::pr_train::BRANCH_PREFIX) {
        return outcome;
    }

    // TASK-630: opening the PR is the event that ends a deliberate PR-hold
    // (BUG-250). `aida pr auto-queue-review` runs right after `gh pr create`
    // (the /aida-pr step), so a Filed/AlreadyExists outcome means the held PR is
    // now open — clear the persisted `.aida/pr-holds/<spec>.json` marker so a
    // later `aida queue work <spec> --resume` no longer treats the spec as held.
    // Best-effort: a missing marker (the common case — most PRs were never
    // held) or an unlink error never perturbs the queue-review flow.
    // trace:TASK-630 | ai:claude
    if matches!(
        outcome.status,
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
    ) {
        if let Some(spec) = &lease_scope {
            let main_root =
                find_main_worktree_root().unwrap_or_else(|_| project_root.to_path_buf());
            let marker = punt::hold_signal_path(&main_root, spec);
            if punt::read_hold_signal(&marker).is_some() {
                let _ = std::fs::remove_file(&marker);
            }
        }
        // BUG-1944: `try_auto_queue_pr_review` already moved the branch's and
        // covered specs to In Progress; the session scope is known only here.
        // trace:BUG-1944 | ai:claude
        if let Some(spec) = lease_scope
            .as_deref()
            .and_then(crate::worktree_lease::spec_id_from_branch)
        {
            let main_root =
                find_main_worktree_root().unwrap_or_else(|_| project_root.to_path_buf());
            crate::live_work::bump_spec_in_progress(
                &main_root,
                &spec,
                aida_core::conflict::PR_OPEN_AUTHOR,
            );
        }
    }

    // BUG-86: on non-train non-success outcomes (needs-attention OR by-design
    // skip), print the exact command to re-run manually. The skill side
    // (/aida-pr step 10) consumes this so the agent can quote it back at
    // the user instead of having them spelunk for the right invocation.
    // Filed and AlreadyExists are the only "no action needed" cases.
    // trace:BUG-86 | ai:claude
    match outcome.status {
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists => {}
        AutoQueueStatus::SkippedByDesign => {
            eprintln!(
                "  {} `aida pr auto-queue-review --branch {}`",
                "Re-run manually:".dimmed(),
                branch
            );
        }
        AutoQueueStatus::SkippedNeedsAttention => {
            eprintln!(
                "  {} `aida pr auto-queue-review --branch {}`",
                "Re-run manually:".yellow().bold(),
                branch
            );
        }
    }

    outcome
}

/// BUG-1944: the specs an opened PR starts work on — the session's spec scope,
/// the spec named by the branch, and the specs the PR covers — deduplicated,
/// upper-cased, non-spec scopes dropped.
// trace:BUG-1944 | ai:claude
pub(crate) fn pr_open_bump_targets(
    lease_scope: Option<&str>,
    branch: &str,
    covered_specs: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let candidates = lease_scope
        .and_then(crate::worktree_lease::spec_id_from_branch)
        .into_iter()
        .chain(crate::worktree_lease::spec_id_from_branch(branch))
        .chain(
            covered_specs
                .iter()
                .filter_map(|s| crate::worktree_lease::spec_id_from_branch(s)),
        );
    for id in candidates {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}
