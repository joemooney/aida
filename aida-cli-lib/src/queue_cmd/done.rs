// trace:TASK-3426 | ai:codex
use super::*;
/// Collect commit and requirement-reference evidence introduced since the
/// current branch diverged from the default branch. `None` means the range
/// could not be read and intentionally makes the ownership gate fail closed.
// trace:BUG-1244 | ai:codex
pub(crate) fn queue_done_commit_evidence(
    project_root: &Path,
    branch: &str,
) -> Option<workflow_hints::QueueDoneCommitEvidence> {
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return None;
    };
    let range = format!("{default_ref}..{branch}");
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "--format=%B%x00", &range])
        .output();
    let Ok(output) = output else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    let mut ids = Vec::new();
    let output = String::from_utf8_lossy(&output.stdout);
    let messages: Vec<_> = output
        .split('\0')
        .filter(|message| !message.trim().is_empty())
        .collect();
    for message in &messages {
        let mut found = extract_spec_ids_from_commit(message);
        found.extend(extract_referenced_spec_ids_from_commit(message));
        for id in found {
            if !ids
                .iter()
                .any(|seen: &String| seen.eq_ignore_ascii_case(&id))
            {
                ids.push(id);
            }
        }
    }
    Some(workflow_hints::QueueDoneCommitEvidence {
        commits_seen: messages.len(),
        spec_ids: ids,
    })
}

/// The CHEAP half of the BUG-1515 outstanding-refusal check: read the
/// locally-recorded verdict (trying `project_root` then, only as a fallback,
/// the ALREADY-RESOLVED `primary_root`) and test whether its SHAPE is an
/// outstanding refusal (`RequestChanges`/`Rejected`, never closed by a later
/// merge — `review_verdict::is_outstanding_refusal`; a `Done` spec is never
/// `Completed`, so that half of the predicate is always `false` here). Pure
/// filesystem reads — no git spawn. Callers resolve `primary_root` ONCE
/// (`main_worktree_root_from`, a `git worktree list` spawn) and reuse it
/// across every spec being checked, rather than each check resolving its own
/// — the per-spec spawn this split exists to avoid.
// trace:TASK-1456 | ai:claude
/// BUG-2403: what a Done spec's refusal check found. `Unavailable` (its
/// current review cannot be selected: ambiguous or refused evidence) is
/// protective — never classified as merge-ready.
// trace:BUG-2403 | ai:claude
#[derive(Debug)]
pub(crate) enum RefusalCheck {
    Clear,
    Outstanding(crate::gate::review_verdict::RecordedVerdict),
    Unavailable(String),
}

pub(crate) fn cheap_outstanding_refusal_candidate(
    selector: &mut crate::gate::review_alias::Selector<'_>,
    primary: &mut Option<crate::gate::review_alias::Selector<'_>>,
    display_id: &str,
    spec_id: &str,
) -> RefusalCheck {
    use crate::gate::review_alias::AliasSelection;
    // BUG-2403: one selection across both labels. Only an ABSENT verdict
    // falls back to the primary checkout; ambiguous or refused evidence is
    // reported as unavailable, never read as "no outstanding refusal".
    // trace:BUG-2403 | ai:claude
    let mut selection = selector.select(&[display_id, spec_id]);
    if matches!(selection, AliasSelection::Absent) {
        if let Some(primary) = primary.as_mut() {
            selection = primary.select(&[display_id, spec_id]);
        }
    }
    let unavailable = selection.unavailable_reason().map(str::to_string);
    match selection.fail_closed() {
        Some(verdict)
            if crate::gate::review_verdict::is_outstanding_refusal(
                &verdict, /* spec_completed */ false,
            ) =>
        {
            RefusalCheck::Outstanding(verdict)
        }
        _ => match unavailable {
            Some(reason) => RefusalCheck::Unavailable(reason),
            None => RefusalCheck::Clear,
        },
    }
}

/// BUG-1515: whether a `Done` spec's most recently recorded review verdict is
/// an OUTSTANDING refusal that is STILL LIVE at the branch tip — the cheap
/// shape check ([`cheap_outstanding_refusal_candidate`]) AND whose reviewed
/// sha is still the tip (`verdict_tip_relation` == `AtReviewedSha`). A
/// refusal recorded against an OLD head (new commits pushed since, or the
/// branch rewritten) is history, not a live blocker — after a normal rework
/// round (refusal, new commits, `queue done` again) that old refusal must
/// not keep reading as "REWORK NEEDED" when what it actually needs is
/// RE-REVIEW; `queue_fresh_pickup_policy` falls back to `AwaitingMerge` in
/// that case. Reads the verdict the same way `evaluate_review_verdict_gate`
/// does (either id form, primary-worktree fallback for a reviewer verdict
/// recorded outside an implementer worktree) — no new verdict reader, per
/// BUG-1515's acceptance.
///
/// **Single-spec path** — resolves the primary worktree root itself (a `git
/// worktree list` spawn), so it is O(1) per call and fine for `aida queue
/// work <ID>` / `aida queue list`'s per-row Done classification. A caller
/// iterating MANY Done specs at once (e.g. `aida list`'s default open lens)
/// must NOT call this per spec — that resolves the primary root once PER
/// SPEC and, worse, pays the tip/ancestry git check for every spec with any
/// recorded verdict, not just a capped set. Use
/// [`select_done_rework_rows`]/[`resolve_done_rework_ids`] instead; they
/// resolve the root once and cap the expensive half. Returns the outstanding
/// verdict so a caller can build a richer message from it.
// trace:BUG-1515 | ai:claude
pub(crate) fn done_spec_outstanding_refusal(
    project_root: &std::path::Path,
    display_id: &str,
    spec_id: &str,
) -> RefusalCheck {
    let primary_root = main_worktree_root_from(project_root);
    let mut selector = crate::gate::review_alias::Selector::new(project_root);
    let mut primary = (primary_root != project_root)
        .then(|| crate::gate::review_alias::Selector::new(&primary_root));
    let verdict =
        match cheap_outstanding_refusal_candidate(&mut selector, &mut primary, display_id, spec_id)
        {
            RefusalCheck::Outstanding(v) => v,
            other => return other,
        };
    let relation = verdict_tip_relation(
        project_root,
        verdict.reviewed_branch.as_deref(),
        verdict.reviewed_sha.as_deref(),
    );
    if matches!(
        relation,
        crate::gate::review_verdict::TipRelation::AtReviewedSha
    ) {
        RefusalCheck::Outstanding(verdict)
    } else {
        RefusalCheck::Clear
    }
}

/// Cap on how many candidates [`resolve_done_rework_ids`] will run the git
/// tip/ancestry check against, after the cheap local-file pass has already
/// narrowed the set to specs with an outstanding-SHAPED verdict. A project
/// with more than this many Done specs simultaneously carrying an unresolved
/// refusal has bigger problems than one `aida list` run being slightly
/// incomplete; PRIN-5 still holds — a spec past the cap is simply not
/// confirmed this run (not silently claimed clean), and re-appears once the
/// backlog ahead of it clears.
// trace:TASK-1456 | ai:claude
pub(crate) const MAX_REWORK_TIP_CHECKS: usize = 20;

/// TASK-1456 (follow-up to BUG-1515): `aida list`'s default open lens
/// (`RequirementStatus::open_statuses()`) deliberately excludes `Done` — it
/// sits with Completed/Rejected on the "closed" side. That is correct for an
/// ordinary Done spec awaiting merge, but a Done spec whose PR carries a
/// still-live refusal at its tip is NOT finished; BUG-1515 already taught
/// `queue_fresh_pickup_policy` to read that state as rework, not
/// merge-ready. Left alone, the open-lens exclusion made these specs
/// disappear from the main work list entirely rather than just from the
/// (already correct) awaiting-merge bucket.
///
/// **Perf** (review follow-up to the first cut of this function, which called
/// `done_spec_outstanding_refusal` per row — an O(N) `git worktree list`
/// spawn plus, for every Done row that happened to carry ANY recorded
/// verdict, a further 2-3 git spawns for the tip/ancestry check, breaking the
/// sub-second `aida list` contract on a project with many Done specs). This
/// resolves the primary worktree root ONCE
/// ([`main_worktree_root_from`]), then runs the CHEAP local-file-only pass
/// ([`cheap_outstanding_refusal_candidate`], no git spawn) over every
/// candidate, and only pays for the git tip/ancestry check on the resulting
/// candidate set — capped at [`MAX_REWORK_TIP_CHECKS`].
///
/// Returns the rows to fold into the list (a strict subset of `done_rows`)
/// plus the SAME decision as a `Uuid` set, so a caller rendering those rows
/// at several sites (JSON, agent-mode notes, the human table's "Rework
/// needed" block) can reuse the set by plain membership check instead of
/// recomputing the verdict/git check per render site.
///
/// `project_root` is `None` in the rare caller that cannot resolve one
/// (mirrors `queue_fresh_pickup_policy`'s degrade-safe contract) — PRIN-5:
/// unknown verdict state is never treated as "must be rework", so it
/// degrades to "not included" rather than a guess.
// trace:TASK-1456 | ai:claude
pub(crate) fn select_done_rework_rows(
    done_rows: Vec<aida_core::RequirementSummary>,
    project_root: Option<&std::path::Path>,
) -> (
    Vec<aida_core::RequirementSummary>,
    std::collections::HashSet<uuid::Uuid>,
) {
    let Some(root) = project_root else {
        return (Vec::new(), std::collections::HashSet::new());
    };
    let ids = resolve_done_rework_ids(&done_rows, root);
    if ids.is_empty() {
        return (Vec::new(), ids);
    }
    let rows = done_rows
        .into_iter()
        .filter(|r| ids.contains(&r.id))
        .collect();
    (rows, ids)
}

/// The id-set half of [`select_done_rework_rows`] — see its doc comment for
/// the perf rationale. Exposed separately so a caller that already holds the
/// `RequirementSummary` rows (no need for `select_done_rework_rows` to hand
/// back a filtered copy) can ask "which of these are rework" directly.
// trace:TASK-1456 | ai:claude
pub(crate) fn resolve_done_rework_ids(
    done_rows: &[aida_core::RequirementSummary],
    project_root: &std::path::Path,
) -> std::collections::HashSet<uuid::Uuid> {
    // Resolved ONCE for the whole batch — was resolved once PER Done row
    // with no local verdict (the common case), an O(N) `git worktree list`
    // spawn. trace:TASK-1456 | ai:claude
    let primary_root = main_worktree_root_from(project_root);
    // BUG-2403: one ledger discovery for the whole batch; a row whose review
    // cannot be selected is kept (protective), without a tip check.
    // trace:BUG-2403 | ai:claude
    let mut selector = crate::gate::review_alias::Selector::new(project_root);
    let mut primary = (primary_root != project_root)
        .then(|| crate::gate::review_alias::Selector::new(&primary_root));
    let mut unavailable: std::collections::HashSet<uuid::Uuid> = Default::default();
    let mut candidates: Vec<(uuid::Uuid, crate::gate::review_verdict::RecordedVerdict)> =
        Vec::new();
    for r in done_rows {
        match cheap_outstanding_refusal_candidate(
            &mut selector,
            &mut primary,
            r.agreed_id.as_deref().unwrap_or_default(),
            r.spec_id.as_deref().unwrap_or_default(),
        ) {
            RefusalCheck::Outstanding(v) => candidates.push((r.id, v)),
            RefusalCheck::Unavailable(_) => {
                unavailable.insert(r.id);
            }
            RefusalCheck::Clear => {}
        }
    }
    let mut ids: std::collections::HashSet<uuid::Uuid> = candidates
        .into_iter()
        .take(MAX_REWORK_TIP_CHECKS)
        .filter_map(|(id, verdict)| {
            let relation = verdict_tip_relation(
                project_root,
                verdict.reviewed_branch.as_deref(),
                verdict.reviewed_sha.as_deref(),
            );
            matches!(
                relation,
                crate::gate::review_verdict::TipRelation::AtReviewedSha
            )
            .then_some(id)
        })
        .collect();
    ids.extend(unavailable);
    ids
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        QueueCommand::Done {
            id,
            user,
            yes,
            force,
            skip_pr_check,
            interface_cli,
            interface_mcp,
            interface_tui,
            interface_other,
            no_interface_change,
            test_plan,
            no_test_plan,
        } => {
            let user_id = get_user(user);
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            // BUG-81: display short id when one's been assigned; canonical
            // spec_id stays for activity-log / manifest matching (those
            // keys were written with spec_id form). trace:BUG-81 | ai:claude
            let display_id = req
                .agreed_id
                .as_deref()
                .or(req.spec_id.as_deref())
                .unwrap_or("???");

            // BUG-1611: the lifecycle guard runs first, before any other gate
            // or side effect. `queue done` from Draft / NeedsAttention needs
            // the approval authority the lifecycle guard names; a closed spec
            // (Rejected / Completed / Superseded) is refused outright because
            // `queue done` is not a reopen verb. `--force` does not bypass
            // this: it overrides evidence gates, not authority. The drain's
            // own `queue done` from Approved / In Progress is unaffected.
            // trace:BUG-1611 | ai:claude
            if let Some(refusal) = crate::queue_done_lifecycle_refusal(
                &req.status,
                crate::gate::role_authority::has_advisor_authority(),
            ) {
                anyhow::bail!(crate::queue_done_lifecycle_refusal_message(
                    display_id,
                    &req.status,
                    refusal
                ));
            }

            // A native seat cannot publish its pending edits. Resolve the
            // branch from the host lease, then defer completion before any
            // committed-only ownership/PR/test diagnostics or mutation.
            // trace:TASK-2879 | ai:codex
            if std::env::var("AIDA_CONTAINMENT_POSTURE").as_deref() == Ok("native") {
                if let Ok(root) = find_project_root() {
                    crate::queue_misc::refuse_native_queue_done(&root)?;
                }
            }

            // BUG-1244: fail closed before any lifecycle mutation when a
            // branch names another spec, commit evidence exclusively names
            // other specs, or the since-merge-base range cannot be read.
            // `--force` remains the explicit, ledgered recovery escape hatch.
            // trace:BUG-1244 | ai:codex
            if let Ok(root) = find_project_root() {
                // trace:TASK-2887 | ai:codex
                if let Some(branch) = workflow_hints::queue_done_ownership_branch(
                    &root,
                    *force,
                    crate::gate::kernel_deps::current_branch_at_checked,
                )? {
                    let commit_evidence = queue_done_commit_evidence(&root, &branch);
                    // BUG-1629: the same configured prefixes auto-complete
                    // phase 1 checks with. trace:BUG-1629 | ai:claude
                    let configured_prefixes = workflow_hints::project_spec_prefixes(&root);
                    match workflow_hints::queue_done_ownership(
                        &branch,
                        display_id,
                        commit_evidence.as_ref(),
                        *force,
                        &configured_prefixes,
                        // The stored origin ID remains authoritative after the
                        // merge gate assigns a short display ID.
                        // trace:TASK-1328 | ai:codex
                        &[spec_id],
                    ) {
                        workflow_hints::QueueDoneOwnership::Proceed => {}
                        workflow_hints::QueueDoneOwnership::Refuse(reason) => {
                            eprintln!(
                                "queue done refused: {reason}.\nRun from {display_id}'s worktree/branch, commit work referencing {display_id}, or use `--force` (the override is ledgered)."
                            );
                            std::process::exit(1);
                        }
                        workflow_hints::QueueDoneOwnership::Forced(reason) => {
                            eprintln!(
                                "warning: --force overriding queue-done ownership check: {reason}."
                            );
                        }
                    }
                }
            }

            // BUG-269: refuse `queue done` when the branch carries
            // committed-but-unshipped work with no open PR. Without this
            // gate, a `--zen` or interactive session that forgot to run
            // `/aida-pr` leaves the spec Done locally and unmergeable —
            // the orchestrator (or a watching human) has no way to advance
            // it. The BUG-232 hint nudges; this *enforces*.
            //
            // Skip when `--force` or `--skip-pr-check` (BUG-285: explicit
            // opt-in for the rare case where the spec shipped via a
            // different already-merged branch) and when the BUG-232 hint
            // detector itself can't decide (gh missing / unauthenticated /
            // network failure) — proceeding is the safe default when we
            // can't prove "no PR." Branch lookup and commits-ahead failures
            // refuse before mutation. `--yes` does NOT bypass — it's the
            // confirmation-skip for the prompt below, not a gate override.
            // trace:BUG-269 BUG-285 | ai:claude
            let bypass_pr_check =
                workflow_hints::queue_done_should_bypass_pr_check(*yes, *force, *skip_pr_check);
            if !bypass_pr_check {
                // BUG-360 / TASK-500: the gate's decision tree is now a pure
                // `queue_done_precheck_diagnose` (workflow_hints) with the
                // git/gh lookups injected as closures, so every skip path,
                // the refusal, and the proceed are unit-testable in isolation.
                // Each silent-skip path still emits a diagnostic warning so a
                // future bypass leaves a trace in the headless log / cast /
                // stderr — the operator can identify WHICH condition failed
                // without re-instrumenting. The closure mapping ChangeLookup →
                // PrState keeps the gate forge-routed (STORY-516).
                // trace:TASK-500 BUG-360 STORY-516 | ai:claude
                let diagnose = workflow_hints::queue_done_precheck_diagnose(
                    display_id,
                    find_project_root(),
                    // trace:TASK-2887 | ai:codex
                    crate::gate::kernel_deps::current_branch_at_checked,
                    // BUG-775: the tri-state form — the gate must be able to
                    // tell "on the default branch" from "comparison failed".
                    |root, branch| branch_commits_ahead_default(root, branch),
                    |root, branch| match change_lookup_for_branch(root, branch) {
                        crate::forge::ChangeLookup::Found(c) => workflow_hints::PrState::Open(c.id),
                        crate::forge::ChangeLookup::NoChange => workflow_hints::PrState::Absent,
                        crate::forge::ChangeLookup::CliMissing
                        | crate::forge::ChangeLookup::CliFailed(_)
                        | crate::forge::ChangeLookup::Unreachable(_) => {
                            workflow_hints::PrState::Unknown
                        }
                    },
                );
                match diagnose {
                    workflow_hints::QueueDoneGateDiagnose::Refuse(lines) => {
                        for line in &lines {
                            eprintln!("{}", line);
                        }
                        std::process::exit(1);
                    }
                    workflow_hints::QueueDoneGateDiagnose::SilentSkip { warning_line, .. } => {
                        eprintln!("{}", warning_line);
                    }
                    workflow_hints::QueueDoneGateDiagnose::Proceed => {}
                }
            } else {
                // BUG-285: an opt-in bypass should NEVER be silent — the
                // headless drain's tee + the user's terminal both need a
                // record that the gate was deliberately skipped, otherwise
                // a routine bypass looks identical to the happy path. Use
                // a `warning:` prefix (matches the rest of the CLI) so the
                // line stands out in scrollback and parses as a soft error
                // for any tooling watching stderr.
                // trace:BUG-285 | ai:claude
                let flag = if *skip_pr_check {
                    "--skip-pr-check"
                } else {
                    "--force"
                };
                eprintln!(
                    "{} bypassing PR check for {} via {} — proceeding without an open PR.",
                    "warning:".yellow().bold(),
                    display_id,
                    flag
                );
            }

            // BUG-775 Guard: refuse when the LAST review of this spec asked
            // for changes and the branch has not moved past the commit that
            // review examined. A reviewer's "changes requested" used to live
            // only in prose (a comment); the next session merged the exact
            // rejected commit and marked the spec Done with nothing objecting.
            // Now the verdict is state, and this reads it back.
            // `--force` overrides — loudly, naming the verdict it overrides.
            // trace:BUG-775 | ai:claude
            if let Ok(project_root) = find_project_root() {
                match evaluate_review_verdict_gate(&project_root, display_id, spec_id) {
                    (crate::gate::review_verdict::VerdictGate::Refuse(lines), _) if !*force => {
                        for line in &lines {
                            eprintln!("{}", line);
                        }
                        std::process::exit(1);
                    }
                    (crate::gate::review_verdict::VerdictGate::Warn(lines), _) => {
                        for line in &lines {
                            eprintln!("{}", line);
                        }
                    }
                    (crate::gate::review_verdict::VerdictGate::Refuse(_), Some(v)) => {
                        eprintln!(
                            "{} --force: overriding the recorded review verdict for {} ({}).",
                            "warning:".yellow().bold(),
                            display_id,
                            crate::review_verdict_display::verdict_notice_line(&v)
                        );
                    }
                    _ => {}
                }
            }

            // TASK-1290: reuse `aida criteria`'s existing tracer (STORY-1178) to
            // name an untraced acceptance criterion before review instead of
            // letting the reviewer discover it. Default WARN (proceed, loudly);
            // `[protocol] enforce = "refuse"` blocks, quoting the criterion
            // verbatim, and `--force` overrides with a ledgered comment. Quiet
            // when clean — a spec whose criteria are all traced produces no
            // output. This is the single choke point a future TASK-1277
            // protocol-item done-gate can fold in alongside its own checks; see
            // `criteria_gate.rs`'s module doc for the absorb points.
            // trace:TASK-1290 | ai:claude
            if let Ok(project_root) = find_project_root() {
                if let Ok(criteria_report) = crate::criteria::build_criteria_report(
                    &project_root,
                    display_id,
                    &req.description,
                ) {
                    let enforce = crate::criteria_gate::read_enforce_mode(&project_root);
                    match crate::criteria_gate::evaluate(&criteria_report, enforce) {
                        crate::criteria_gate::CriteriaGate::Proceed => {}
                        crate::criteria_gate::CriteriaGate::Warn(lines) => {
                            eprintln!(
                                "{} {} has untraced acceptance criteria (no test traces them):",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                display_id
                            );
                            for line in &lines {
                                eprintln!("  {line}");
                            }
                        }
                        crate::criteria_gate::CriteriaGate::Refuse(lines) => {
                            if *force {
                                eprintln!(
                                    "{} --force: overriding untraced acceptance criteria for {}:",
                                    "warning:".yellow().bold(),
                                    display_id
                                );
                                for line in &lines {
                                    eprintln!("  {line}");
                                }
                                let author = get_default_author();
                                let comment = aida_core::Comment::new(
                                    author,
                                    crate::criteria_gate::force_override_ledger_comment(
                                        display_id, &lines,
                                    ),
                                );
                                // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
                                storage.update_spec_atomically(req, |r| {
                                    r.add_comment(comment);
                                })?;
                            } else {
                                eprintln!(
                                    "queue done refused: {} has untraced acceptance criteria \
                                     ([protocol] enforce = \"refuse\"):",
                                    display_id
                                );
                                for line in &lines {
                                    eprintln!("  {line}");
                                }
                                eprintln!("Override with `--force` (the override is ledgered).");
                                std::process::exit(1);
                            }
                        }
                    }
                }
            }

            let mut protocol_ledger: Option<(String, String)> = None;
            // TASK-1277: typed protocols slice 2 — evaluate the machine-
            // checkable items of the spec type's protocol (spike deliverable,
            // ADR accepted + references edge, bug test-file change). Same
            // `[protocol] enforce` posture as the criteria gate above: warn
            // (print + ledger + proceed), refuse (block, item named), `--force`
            // (proceed + ledger naming who forced). Unevaluable items are
            // reported UNKNOWN, never passed (PRIN-5). Quiet for types with
            // no machine-checkable item and when every item is met.
            // trace:TASK-1277 | ai:claude
            if let Ok(project_root) = find_project_root() {
                let report = crate::protocol_gate::evaluate_for_repo(req, &project_root);
                let enforce = crate::criteria_gate::read_enforce_mode(&project_root);
                let outcome = crate::protocol_gate::decide(&report, enforce, *force);
                let (missing, unknown, refuse, forced) = match outcome {
                    crate::protocol_gate::GateOutcome::Proceed { unknown } => {
                        (Vec::new(), unknown, false, false)
                    }
                    crate::protocol_gate::GateOutcome::Warn {
                        missing,
                        unknown,
                        forced,
                    } => (missing, unknown, false, forced),
                    crate::protocol_gate::GateOutcome::Refuse { missing, unknown } => {
                        (missing, unknown, true, false)
                    }
                };
                for line in unknown.iter().chain(missing.iter()) {
                    eprintln!(
                        "{} {line}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                }
                if refuse {
                    eprintln!(
                        "queue done refused: {} is missing protocol items \
                         ([protocol] enforce = \"refuse\"). Override with `--force` \
                         (the override is ledgered).",
                        display_id
                    );
                    std::process::exit(1);
                }
                if !missing.is_empty() {
                    let author = get_default_author();
                    let body = crate::protocol_gate::ledger_comment(
                        "aida queue done",
                        display_id,
                        &missing,
                        forced.then_some(author.as_str()),
                    );
                    // Written only once every later guard passes (below).
                    protocol_ledger = Some((author, body));
                }
            }

            // STORY-469 Guard 1: validate trailer spec-IDs before flipping the
            // spec to Done. Catch a hallucinated / typo'd / since-rejected
            // `(SPEC-ID)` trailer on this branch's commits BEFORE the spec is
            // marked Done (and before the branch's commits — already authored —
            // ride a later merge into shared history). Reuses the STORY-498
            // gate's pure validator + store resolver, client-side. `--force`
            // bypasses (matching its existing "I know what I'm doing" role for
            // the PR check above). trace:STORY-469 | ai:claude
            if let Ok(project_root) = find_project_root() {
                run_client_trailer_guard(&project_root, "queue done", *force);
            }

            // BUG-671: `queue done` is non-destructive and reversible (marks
            // Done + dequeues). With no human to answer the prompt
            // (`non_interactive_confirm`), the old `read_line` gate hit EOF,
            // failed the 'y' check, and CANCELLED — marking nothing Done while
            // an agent capturing stdout (the "Cancelled" notice went to stderr)
            // believed it succeeded. AUTO-CONFIRM instead: the agent explicitly
            // invoked `done`. Only a real interactive non-`--yes` run still
            // prompts. trace:BUG-671 | ai:claude
            if !yes && !non_interactive_confirm() {
                eprintln!(
                    "Mark {} ({}) as done and remove from queue?",
                    display_id.bold(),
                    req.title
                );
                eprintln!("Type 'y' to confirm:");
                let mut answer = String::new();
                if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
                    eprintln!("Cancelled.");
                    return Ok(());
                }
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    eprintln!("Cancelled. Requirement and queue untouched.");
                    return Ok(());
                }
            }

            // TASK-1277: the protocol ledger lands only after the trailer
            // guard and the confirmation pass, and only once — a drain retry
            // or rework round re-running `queue done` with the identical gap
            // does not stack duplicate comments.
            // trace:TASK-1277 | ai:claude
            if let Some((author, body)) = protocol_ledger.take() {
                // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
                storage.update_spec_atomically(req, |r| {
                    if !crate::protocol_gate::ledger_already_recorded(r, &body) {
                        r.add_comment(aida_core::Comment::new(author, body));
                    }
                })?;
            }

            // BUG-1611: Draft can no longer reach here without approval
            // authority (the lifecycle guard above refuses it); the warning
            // below still flags an authorized Draft and the never-started
            // Approved / Planned cases. trace:BUG-1611 | ai:claude
            //
            // BUG-684: `queue done` promotes ANY status straight to Done — a
            // Draft/Approved/Planned spec (never started) flips to Done with a
            // green check, so a typo'd id silently corrupts state. This is
            // asymmetric with the strong re-open guard that gates Done → *. Warn
            // (non-blocking) when marking done a spec that was never In Progress
            // so the mistake is VISIBLE, then proceed — a legitimate "I finished,
            // don't nag" agent path exists, so we don't hard-block. Statuses
            // that HAVE been worked (InProgress, NeedsAttention) and the
            // already-terminal ones (Done/Completed) don't trip the warning.
            // trace:BUG-684 | ai:claude
            if matches!(
                req.status,
                RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned
            ) {
                eprintln!(
                    "{} {} was never In Progress (was {}) — marking done anyway.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display_id.bold(),
                    req.status
                );
            }

            // Update status to Done via update_atomically — works across
            // SQLite and git-canonical modes. set_status_from_str also
            // clears any stale custom_status so the canonical enum value
            // actually takes effect (BUG-1-025).
            // trace:BUG-1-025 | ai:claude
            //
            // STORY-86: `queue done` flips to **Done** (work finished on
            // a branch), not Completed. The auto-bump scan in
            // `aida pull` / `aida db sync --pull` advances Done →
            // Completed once a referencing commit lands on the default
            // branch. Existing call sites that say "completed" in their
            // UI strings have been retargeted to "done" here; the
            // `Completed` semantics ("shipped on main") are preserved.
            // trace:STORY-86 | ai:claude
            //
            // STORY-81 Part 2: also stamp `implementation_info` so the
            // req permanently records who completed it and when, with
            // the AI source-tool when known. This survives the queue
            // entry's deletion (which happens right below) so post-
            // merge `aida show` still surfaces the completion context.
            // `completed_at` / `completion_sha` are intentionally LEFT
            // UNSET here — those are stamped by the STORY-86 auto-bump.
            // trace:STORY-81 | ai:claude
            let req_id = req.id;
            let now = chrono::Utc::now();
            let completer = get_default_author();
            let source_tool = std::env::var("AIDA_AI_TOOL").ok().filter(|s| !s.is_empty());
            // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
            storage.update_spec_atomically(req, |r| {
                // BUG-1637: caller-authored, recorded under the caller.
                // trace:BUG-1637 | ai:claude
                let from = r.status.clone();
                r.set_status_from_str("Done");
                crate::record_caller_status_transition(r, &from);
                r.modified_at = now;
                // Don't clobber prior `summary` / `risk_notes` /
                // `test_coverage_notes` if the user / `/aida-pr`
                // skill already populated them. We only set the
                // fields the queue-done path can know.
                let info = r
                    .implementation_info
                    .get_or_insert_with(aida_core::ImplementationInfo::default);
                info.implemented = true;
                info.implemented_at.get_or_insert(now);
                if info.implemented_by.is_none() {
                    info.implemented_by = Some(completer.clone());
                }
                if let Some(ref tool) = source_tool {
                    info.source_tool.get_or_insert_with(|| tool.clone());
                }
            })?;
            // STORY-1480 / ADR-64: Done is the WORK machine's terminal
            // state (Completed often lands later via auto-bump on another
            // clone), so this is the publish that actually carries the
            // phase evidence. Best-effort; never blocks the flip.
            // trace:STORY-1480 | ai:claude
            if let Ok(project_root) = find_project_root() {
                let _ = crate::timing_record::publish(&project_root, &display_id, "queue-done");
            }
            let done_remove_role = std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| canonical_role_name(&s));
            queue_role_fallback::remove_visible_queue_entry(
                storage,
                &user_id,
                &req_id,
                done_remove_role.as_deref(),
            )?;
            // BUG-65: queue done bypasses Command::Edit (sets status via
            // update_atomically directly), so the role activity log used
            // to miss it entirely — leaving statusline @SPEC stuck on the
            // last show/comment after every shipped spec. Bump explicitly
            // here so the most recently shipped spec wins.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "done");

            // STORY-98: same reason as the BUG-65 hookup above — `queue
            // done` bypasses Command::Edit, so the manifest-flip path
            // there doesn't fire. Mirror it explicitly so `aida session
            // show --plan` flips the Done check at completion time. STORY-86:
            // pass "Done" since that's the canonical status now (manifest
            // treats Done + Completed equivalently — both check the item
            // off the planned cluster).
            // trace:STORY-98 STORY-86 | ai:claude
            update_manifest_for_status(spec_id, "Done");

            println!(
                "{} {} marked done and removed from queue.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold()
            );
            println!(
                "  ({})",
                "run `aida queue next` to see what's next".dimmed()
            );

            // BUG-673: next-step breadcrumb after `queue done`. The agent chain
            // is add -> approve -> queue list -> queue work -> queue done ->
            // `aida pull`, but the `next[]` block dropped out exactly here, so
            // an agent that just marked a spec Done was never told the next move
            // is `aida pull` (which fires the Done -> Completed auto-bump once
            // the PR merges) and stranded the spec at Done. Emit the lifecycle
            // block on both surfaces: the TOON `next[]` block in agent mode, the
            // `Next:` block on the human TTY (same idiom `aida show` uses).
            // trace:BUG-673 | ai:claude
            {
                let next = crate::help_next::queue_done_next(display_id);
                let rendered = if agent_output_mode() {
                    crate::help_next::render(&next)
                } else {
                    crate::help_next::render_human(&next)
                };
                if let Some(block) = rendered {
                    println!("{block}");
                }
            }

            // TASK-96: offer to file the plan's Followups bullets as child
            // TASKs. Interactive prompt unless `--yes` (then file all).
            // Best-effort — a failure here never blocks `queue done`.
            // trace:TASK-96 | ai:claude
            if let Ok(project_root) = find_project_root() {
                if let Err(e) =
                    extract_plan_followups(storage, &project_root, spec_id, display_id, !yes)
                {
                    eprintln!(
                        "{} followup extraction skipped: {}",
                        "Warning:".yellow().bold(),
                        e
                    );
                }
            }

            // STORY-542: capture the spec's user-facing interface changes (the
            // deterministic Layer-1 source for `aida digest --audience
            // operator`). Flags win and skip the prompt; otherwise at a TTY
            // (and unless --yes / --no-interface-change) ask per surface.
            // Best-effort — a failure here never blocks `queue done`.
            // trace:STORY-542 | ai:claude
            if let Err(e) = capture_interface_changes(
                storage,
                req,
                display_id,
                interface_cli,
                interface_mcp,
                interface_tui,
                interface_other,
                *no_interface_change,
                /* interactive = */ !yes,
            ) {
                eprintln!(
                    "{} interface-change capture skipped: {}",
                    "Warning:".yellow().bold(),
                    e
                );
            }

            // STORY-698: capture the verification steps the builder actually
            // ran — the implementation audit trail surfaced in the PR body.
            // Stored in implementation_info.test_coverage_notes (no new field).
            // `--test-plan` flags win and skip the prompt; otherwise at a TTY
            // (and unless --yes / --no-test-plan) ask. Best-effort — a failure
            // here never blocks `queue done`. trace:STORY-698 | ai:claude
            if let Err(e) = capture_test_plan(
                storage,
                req,
                display_id,
                test_plan,
                *no_test_plan,
                /* interactive = */ !yes,
            ) {
                eprintln!(
                    "{} verification-step capture skipped: {}",
                    "Warning:".yellow().bold(),
                    e
                );
            }

            // STORY-106: workflow hint when the queue is now empty for the
            // active role+scope. Best-effort: any state-detection failure
            // skips the hint silently rather than failing the command.
            // trace:STORY-106 | ai:claude
            maybe_hint_after_queue_drain(storage, &user_id);

            // STORY-700: passive first-run chain — advancing past the first
            // `queue done` to the review → merge → `aida pull` hint. Only fires
            // when the arc sits at exactly the spec-filed step; no-op otherwise.
            // trace:STORY-700 | ai:claude
            if let Ok(project_root) = find_project_root() {
                first_run::after_work_done(&project_root);
            }

            // BUG-378: substrate-as-bouncer for the scratchpad-drift ceiling.
            // An agent about to declare "all done" gets told here, before it
            // exits, if new work is queued in the brief surface for its type.
            // trace:BUG-378 | ai:claude
            if let Ok(project_root) = find_project_root() {
                warn_pending_briefs_for_running_agent(&project_root);
            }
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
