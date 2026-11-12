// trace:TASK-3426 | ai:codex
use super::*;

/// TASK-218: smart status-transition table for `aida queue rework`.
/// Returns `Some(target)` when the spec's current status maps to a flip
/// per the table below, or `None` when the status is already in a
/// reasonable rework state (just queue-add it, don't flip).
///
///   Draft      → None         (unusual; preserve)
///   Approved   → None         (ready to queue as-is)
///   Planned    → InProgress
///   InProgress → None         (already there; caller warns unless --force)
///   NeedsAttention → InProgress (findings-led rework resumes paused work)
///   Done       → InProgress   (typical PR-review-found-issues case)
///   Completed  → InProgress   (requires --force at caller)
///   Rejected   → Approved     (requires --force at caller)
///
/// Pure function — separated from `handle_queue_rework` so the table
/// can be unit-tested without spinning up a storage backend.
/// trace:TASK-218 | ai:claude
pub(crate) fn rework_smart_target(current: &RequirementStatus) -> Option<RequirementStatus> {
    match current {
        RequirementStatus::Draft => None,
        RequirementStatus::Approved => None,
        RequirementStatus::Planned => Some(RequirementStatus::InProgress),
        RequirementStatus::InProgress => None,
        RequirementStatus::Done => Some(RequirementStatus::InProgress),
        RequirementStatus::Completed => Some(RequirementStatus::InProgress),
        RequirementStatus::Rejected => Some(RequirementStatus::Approved),
        // TASK-1176: a superseded spec was handed off to a successor — the
        // rework target is that successor, not this record. Refuse to guess a
        // status for it; the caller's --force + explicit --status still works.
        // trace:TASK-1176 | ai:claude
        RequirementStatus::Superseded => None,
        // STORY-332: reworking a punted spec resumes the paused work.
        // A findings-led rework is the triage decision, so leave the
        // NeedsAttention parking state before unified pickability checks run.
        // trace:BUG-1056 | ai:codex
        RequirementStatus::NeedsAttention => Some(RequirementStatus::InProgress),
    }
}

// trace:BUG-1470 | ai:codex
/// Metadata-only rework must leave the queued spec claimable.
/// `InProgress` is only valid when this command immediately chains into
/// `queue work`, which establishes the lease that backs that status.
pub fn rework_target_for_mode(
    current: &RequirementStatus,
    launches_work: bool,
) -> Option<RequirementStatus> {
    let target = rework_smart_target(current);
    if !launches_work {
        match current {
            RequirementStatus::Draft
            | RequirementStatus::Planned
            | RequirementStatus::InProgress
            | RequirementStatus::NeedsAttention
            | RequirementStatus::Done
            | RequirementStatus::Completed => Some(RequirementStatus::Approved),
            RequirementStatus::Approved
            | RequirementStatus::Rejected
            | RequirementStatus::Superseded => target,
        }
    } else {
        target
    }
}

/// The project root whose `.aida/` holds the leases, drain lock and event
/// log for `storage`: the store's parent directory, falling back to the
/// discovered project root.
// trace:STORY-1429 | ai:claude
pub fn requeue_project_root(storage: &Storage) -> std::path::PathBuf {
    storage
        .path()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .or_else(|| find_project_root().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// The queue route (`for_role`) the spec currently holds on any queue, if
/// any. Rework keeps this route, so the requeue preview names it.
// trace:STORY-1429 | ai:claude
pub(crate) fn current_queue_route(storage: &Storage, req_id: uuid::Uuid) -> Option<String> {
    let users = storage.queue_users().ok()?;
    users.into_iter().find_map(|u| {
        storage
            .queue_list(&u, true)
            .ok()?
            .into_iter()
            .find(|e| e.requirement_id == req_id)
            .and_then(|e| e.for_role)
            .filter(|r| !r.trim().is_empty())
            .map(|r| canonical_role_name(&r))
    })
}

/// Whether a drain is running, as far as the drain lock can say.
// trace:STORY-1429 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainRunning {
    Yes,
    No,
    CannotTell,
}

/// Read the drain lock for a definite answer. A missing lock, or one whose
/// process is dead, is a definite "no". A lock file that exists but cannot be
/// read or parsed is "cannot tell".
// trace:STORY-1429 | ai:claude
pub(crate) fn drain_running(project_root: &Path) -> DrainRunning {
    let path = crate::drain_lock::drain_lock_path(project_root);
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DrainRunning::No,
        Err(_) => return DrainRunning::CannotTell,
        Ok(_) => {}
    }
    match crate::drain_lock::probe_lock(project_root) {
        crate::drain_lock::LockStatus::Running(_) => DrainRunning::Yes,
        crate::drain_lock::LockStatus::Stale(_) => DrainRunning::No,
        // The file exists but did not parse.
        crate::drain_lock::LockStatus::None => DrainRunning::CannotTell,
    }
}

/// The one line a requeue prints about the drain that will pick the spec up.
// trace:STORY-1429 | ai:claude
pub(crate) fn drain_running_notice(project_root: &Path) -> String {
    match drain_running(project_root) {
        DrainRunning::Yes => {
            "a drain is running; it picks this spec up on its next head".to_string()
        }
        DrainRunning::No => "no drain is running; start one with `aida drain start`".to_string(),
        DrainRunning::CannotTell => {
            "cannot tell whether a drain is running (the drain lock is unreadable); check \
             with `aida drain status`"
                .to_string()
        }
    }
}

/// The flags `aida rework` / `aida queue rework` accept. Only `--tail` and
/// `--user` make sense without a spec ID; the rest act on one spec.
// trace:STORY-1429 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct ReworkFlags<'a> {
    pub work: bool,
    pub for_role: Option<&'a str>,
    pub tail: bool,
    pub status: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub ruling: Option<&'a str>,
    pub resume: bool,
    pub force: bool,
    pub steal: bool,
    pub permission_mode: Option<&'a str>,
    pub no_pull: bool,
    pub user: Option<&'a str>,
}

impl ReworkFlags<'_> {
    /// The spec-scoped flags that were passed, as their CLI spellings.
    pub(crate) fn spec_scoped(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.work {
            v.push("--work");
        }
        if self.for_role.is_some() {
            v.push("--for");
        }
        if self.status.is_some() {
            v.push("--status");
        }
        if self.reason.is_some() {
            v.push("--reason");
        }
        if self.ruling.is_some() {
            v.push("--ruling");
        }
        if self.resume {
            v.push("--resume");
        }
        if self.force {
            v.push("--force");
        }
        // Compatibility spelling only: handle_rework_entry refuses this before
        // spec_scoped or the queue-work pass-through can see steal=true.
        // trace:STORY-2325 | ai:codex
        if self.steal {
            v.push("--steal");
        }
        if self.permission_mode.is_some() {
            v.push("--permission-mode");
        }
        if self.no_pull {
            v.push("--no-pull");
        }
        v
    }
}

/// `aida rework [ID]` / `aida queue rework [ID]`. With an ID, the one-spec
/// rework. Without one, the triage loop over the NeedsAttention parks: at a
/// terminal it asks one keystroke per spec; without a terminal it prints the
/// requeue hints and exits 0, so agents keep the command form. A spec-scoped
/// flag without an ID is a usage error.
// trace:STORY-1429 | ai:claude
pub(crate) fn handle_rework_entry(
    storage: &Storage,
    id: Option<&str>,
    flags: &ReworkFlags<'_>,
) -> Result<()> {
    refuse_retired_steal(flags.steal)?;
    if let Some(id) = id {
        // The ruling is recorded first so a chained `--work` launch reads it
        // into its pickup prompt. trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
        if let Some(ruling) = flags.ruling {
            record_rework_ruling(storage, id, ruling)?;
        }
        return handle_queue_rework(
            storage,
            id,
            flags.work,
            flags.for_role,
            flags.tail,
            flags.status,
            flags.reason,
            flags.resume,
            flags.force,
            flags.steal,
            flags.permission_mode,
            flags.no_pull,
            flags.user,
        );
    }
    let scoped = flags.spec_scoped();
    if !scoped.is_empty() {
        anyhow::bail!(
            "{} {} a spec: pass its ID (`aida rework <ID> {}`), or run bare `aida rework` to \
             triage the parked specs one keystroke each",
            scoped.join(", "),
            if scoped.len() == 1 {
                "acts on"
            } else {
                "act on"
            },
            scoped.join(" ")
        );
    }
    use std::io::IsTerminal;
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    rework_triage_loop(
        storage,
        tty,
        flags.tail,
        flags.user,
        crate::requeue::caller_may_clear_escalation(),
        &mut input,
        &mut |spec| run_aida_interactive(&["decide", spec]),
        &mut |spec| run_aida_interactive(&["show", spec]),
    )
}

/// `aida rework --ruling TEXT`: record the ruling as a spec comment tagged so
/// [`latest_ruling_comment`] finds it, and every later pickup prompt leads
/// with it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
pub(crate) fn record_rework_ruling(storage: &Storage, id: &str, ruling: &str) -> Result<()> {
    let ruling = ruling.trim();
    anyhow::ensure!(!ruling.is_empty(), "--ruling needs the ruling text");
    let store = storage.load()?;
    let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_unambiguous(id)?
    }
    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;
    let comment = aida_core::Comment::new(get_default_author(), ruling_comment_text(ruling));
    storage.update_spec_atomically(req, |r| {
        r.add_comment(comment);
    })?;
    println!(
        "  {} ruling recorded; the next pickup prompt leads with it",
        "·".dimmed()
    );
    Ok(())
}

/// Run `aida <args>` with the terminal attached (the `[d]` and `[o]` keys).
pub(crate) fn run_aida_interactive(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(crate::aida_exe_path())
        .args(args)
        .status_retrying_etxtbsy()?;
    anyhow::ensure!(status.success(), "`aida {}` failed", args.join(" "));
    Ok(())
}

/// Every spec parked in NeedsAttention, most recently parked first (the
/// `aida findings list` order).
// trace:STORY-1429 | ai:claude
pub(crate) fn needs_attention_parks(store: &aida_core::RequirementsStore) -> Vec<Requirement> {
    let mut parks: Vec<Requirement> = store
        .requirements
        .iter()
        .filter(|r| r.status == RequirementStatus::NeedsAttention && !r.archived)
        .cloned()
        .collect();
    parks.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    parks
}

/// Only a reviewer shelf can complete the repeated-review loop. A punt or CI
/// pause may retain the same imported review while an advisor resolves a new
/// obstacle; replaying that receipt must not undo the triage decision.
/// Legacy loop-guard parks remain review shelves until a deliberate status edit.
/// Current recurrence refusals retain the original reviewer failure marker.
// trace:TASK-3777 | ai:codex
// trace:BUG-2502 | ai:codex
pub(crate) fn rework_is_review_shelf(req: &Requirement) -> bool {
    req.status == RequirementStatus::NeedsAttention
        && req.attention_reason.is_none()
        && (req
            .history
            .iter()
            .rev()
            .find(|entry| {
                entry
                    .changes
                    .iter()
                    .any(|change| change.field_name == "status")
            })
            .is_some_and(|entry| entry.author == aida_core::conflict::LOOP_GUARD_AUTHOR)
            || req.failure_reason.as_ref().is_some_and(|failure| {
                // Review recording and drain shelving use different rejection slugs.
                failure.phase == "reviewer"
                    && matches!(
                        failure.kind.as_str(),
                        "verdict:request-changes"
                            | "verdict:rejected"
                            | "verdict:reject"
                            | "verdict:stranded"
                    )
            }))
}

/// The BUG-1213 loop guard's write: return the spec to NeedsAttention after
/// identical review findings recurred.
///
/// BUG-1632: the guard's decision is automated even when a person ran `queue
/// rework`, so the transition is recorded under
/// [`aida_core::conflict::LOOP_GUARD_AUTHOR`], which the BUG-1625 merge guard
/// recognizes.
// trace:BUG-1213 trace:BUG-1632 | ai:claude
///
/// BUG-1637: like the shelve and the MCP punt, it refuses to leave a terminal
/// status, and the check runs on the copy read inside the atomic write, so a
/// spec completed between the status read and the park stays Completed.
/// Returns whether it parked the spec.
// trace:BUG-1637 | ai:claude
pub(crate) fn loop_guard_park(r: &mut Requirement) -> bool {
    if aida_core::conflict::is_terminal_status(&r.status) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        r,
        RequirementStatus::NeedsAttention,
        aida_core::conflict::LOOP_GUARD_AUTHOR,
    );
    r.modified_at = chrono::Utc::now();
    true
}

/// One line saying why a spec is parked: the shelve's failure, the punt, or
/// the escalation.
pub(crate) fn park_reason_line(r: &Requirement) -> String {
    if let Some(fr) = &r.failure_reason {
        let detail = fr.detail.lines().next().unwrap_or("").trim();
        return format!("shelved {}/{}: {detail}", fr.phase, fr.kind);
    }
    if let Some(a) = &r.attention_reason {
        let detail = a.detail.lines().next().unwrap_or("").trim();
        return format!("punted ({}): {detail}", a.category);
    }
    if r.tags.iter().any(|t| t.eq_ignore_ascii_case("needs-human")) {
        return "escalated to a human".to_string();
    }
    "parked with no recorded reason".to_string()
}

/// The triage loop body, with its input and the `[d]`/`[o]` hand-offs
/// injected so it can be driven without a terminal.
///
/// Keys: `[r]` requeue (shown only when the preview says it is offerable),
/// `[s]` skip, `[o]` show the spec, `[d]` answer its open decision (only when
/// one is pending; returns to the same spec afterwards), `[q]` quit. `[r]`
/// runs the same one-spec rework as `aida rework <ID>` with no status flag,
/// so the resulting status is the preview's by construction.
// trace:STORY-1429 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn rework_triage_loop(
    storage: &Storage,
    tty: bool,
    tail: bool,
    user: Option<&str>,
    clear_escalation: bool,
    input: &mut dyn std::io::BufRead,
    decide: &mut dyn FnMut(&str) -> Result<()>,
    show: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    use crate::requeue::{requeue_hint, requeue_preview, NotOfferable};
    use std::io::Write;
    let store = storage.load()?;
    let parks = needs_attention_parks(&store);
    if parks.is_empty() {
        println!("{}", "Nothing is parked in Needs Attention.".dimmed());
        return Ok(());
    }
    if !tty {
        // No terminal: never prompt. Print the command form for each park.
        println!(
            "{}",
            format!("Parked in Needs Attention ({})", parks.len()).bold()
        );
        for r in &parks {
            // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
            let did = r.friendly_id();
            let route = current_queue_route(storage, r.id);
            let preview = requeue_preview(r, Some(&store), route.as_deref(), clear_escalation);
            println!("  {:<14} {}", did, park_reason_line(r));
            println!("  {:<14} {}", "", requeue_hint(&did, &preview).dimmed());
        }
        println!(
            "{}",
            "Run `aida rework` at a terminal to triage them one keystroke each.".dimmed()
        );
        return Ok(());
    }

    let total = parks.len();
    let (mut requeued, mut skipped) = (0usize, 0usize);
    'specs: for (i, park) in parks.iter().enumerate() {
        let did = park.display_id();
        let mut current = park.clone();
        loop {
            let fresh_store = storage.load()?;
            if let Some(r) = fresh_store.requirements.iter().find(|r| r.id == park.id) {
                current = r.clone();
            }
            let route = current_queue_route(storage, current.id);
            let preview = requeue_preview(
                &current,
                Some(&fresh_store),
                route.as_deref(),
                clear_escalation,
            );
            if matches!(preview.not_offerable, Some(NotOfferable::NotParked(_))) {
                println!(
                    "  {} {did} is {} now; nothing to triage",
                    "·".dimmed(),
                    current.status
                );
                continue 'specs;
            }
            println!();
            println!(
                "{} {} {}",
                format!("[{}/{}]", i + 1, total).dimmed(),
                did.bold(),
                current.title
            );
            println!("  {}", park_reason_line(&current));
            let pending = matches!(preview.not_offerable, Some(NotOfferable::PendingDecision));
            let mut keys = Vec::new();
            if preview.offerable() {
                keys.push(format!("[r] requeue {}", preview.summary()));
            } else {
                println!("  {}", requeue_hint(&did, &preview).dimmed());
            }
            if pending {
                keys.push("[d] decide".to_string());
            }
            keys.push("[s] skip".to_string());
            keys.push("[o] show".to_string());
            keys.push("[q] quit".to_string());
            print!("  {} > ", keys.join("   "));
            std::io::stdout().flush()?;
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                println!();
                break 'specs;
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "r" if preview.offerable() => {
                    match handle_queue_rework(
                        storage, &did, false, None, tail, None, None, false, false, false, None,
                        false, user,
                    ) {
                        Ok(()) => requeued += 1,
                        Err(e) => println!("  {} {e}", "error:".red()),
                    }
                    continue 'specs;
                }
                "d" if pending => {
                    if let Err(e) = decide(&did) {
                        println!("  {} {e}", "error:".red());
                    }
                    // Back to the same spec with its preview recomputed.
                }
                "o" => {
                    if let Err(e) = show(&did) {
                        println!("  {} {e}", "error:".red());
                    }
                }
                "s" | "" => {
                    skipped += 1;
                    continue 'specs;
                }
                "q" => break 'specs,
                other => println!("  {} unknown key `{other}`", "·".dimmed()),
            }
        }
    }
    println!();
    println!(
        "{} {requeued} requeued, {skipped} skipped.",
        "Done.".green().bold()
    );
    Ok(())
}

/// TASK-218: shared implementation backing both `aida queue rework SPEC`
/// and the top-level `aida rework SPEC` alias. Encapsulates the three-
/// command rework sequence (status flip + queue add + optional session
/// launch) so the implementer → reviewer → fixup loop is one verb.
/// trace:TASK-218 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_rework(
    storage: &Storage,
    id: &str,
    work: bool,
    for_role: Option<&str>,
    tail: bool,
    status_override: Option<&str>,
    reason: Option<&str>,
    resume: bool,
    force: bool,
    steal: bool,
    permission_mode: Option<&str>,
    no_pull: bool,
    user: Option<&str>,
) -> Result<()> {
    refuse_retired_steal(steal)?;
    // `--resume` chains through to `aida queue work --resume` (TASK-112,
    // shipped). It implies `--work` — there is nothing to resume without
    // launching a session. trace:BUG-236 | ai:claude
    let work = work || resume;

    // The invoking shell's own identity. Used verbatim when `--user` is
    // explicit; otherwise it's only the fallback for an unrouted (`any`)
    // rework — a routed one lands on the role's shared queue instead (see
    // the `user_id` resolution below). trace:BUG-1277 | ai:claude
    let invoking_user_id = current_user_id(user);
    let store = storage.load()?;

    // Resolve the requirement (UUID first, then SPEC-ID).
    let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_unambiguous(id)? // trace:TASK-1468 | ai:claude
    }
    .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

    let req_id = req.id;
    let spec_id = req.spec_id.clone().unwrap_or_else(|| "???".to_string());
    let display_id = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| "???".to_string());
    // BUG-1277: look for a prior entry for this spec across EVERY queue
    // file, not just the invoking user's own — a previous `rework`/`add`
    // may already have routed it to a shared role queue, and both the
    // route-inheritance fallback below and the move-not-duplicate step
    // after it need to find it wherever it actually lives.
    // trace:BUG-1277 | ai:claude
    // BUG-1427: retain every matching identity. Rework promises a move, so a
    // stale duplicate in a third queue must not survive merely because it was
    // not the first match returned by queue_users(). trace:BUG-1427 | ai:codex
    let existing_entries_elsewhere: Vec<(String, aida_core::QueueEntry)> = storage
        .queue_users()?
        .into_iter()
        .map(|candidate_user| {
            let entry = storage
                .queue_list(&candidate_user, true)?
                .into_iter()
                .find(|entry| entry.requirement_id == req_id);
            Ok(entry.map(|entry| (candidate_user, entry)))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();

    // Route resolution: --for wins; otherwise rework preserves the existing
    // queue route so reviewer-requested fixups stay visible to the intended
    // role. If there is no usable prior route, default to implementer.
    // trace:BUG-851 | ai:codex
    let for_role_resolved: Option<String> = match for_role {
        Some("any") => None,
        Some(role) => Some(canonical_role_name(role)),
        None => existing_entries_elsewhere
            .first()
            .and_then(|(_, entry)| entry.for_role.as_deref())
            .filter(|role| !role.trim().is_empty())
            .map(canonical_role_name)
            .or_else(|| Some("implementer".to_string())),
    };

    // trace:BUG-2500 | ai:codex — refuse before status, comments or queue writes.
    if for_role_resolved.as_deref() != Some("reviewer") {
        if let Ok(root) = find_project_root() {
            let open = spec_open_change_key(&root, &[display_id.as_str(), spec_id.as_str()]);
            refuse_waiting_review(&root, &display_id, &spec_id, open.as_deref())
                .map_err(|why| anyhow::anyhow!(why))?;
        }
    }
    let title = req.title.clone();
    let current_status = req.status.clone();

    // Smart target-status resolution. `--status` always wins; otherwise
    // pick per the table in TASK-218's spec. See `rework_smart_target`.
    let smart_target = rework_target_for_mode(&current_status, work);
    let target_status: Option<RequirementStatus> = match status_override {
        Some(s) => Some(parse_status(s)?),
        None => smart_target,
    };
    // NOTE: the advisor-authority check for this same (current_status,
    // target_status) pair is NOT done here. It used to be (STORY-1353's
    // original hard `anyhow::bail!`), but that duplicated the BUG-1470 F1
    // gate below verbatim -- same predicate, same pair -- and being first,
    // silently upgraded BUG-1470's designed no-op-with-hint refusal into a
    // hard error, breaking `metadata_rework_of_a_draft_is_refused_without_advisor_authority`'s
    // pinned Ok(()) contract. One gate, at the flip, below. trace:STORY-1353 | ai:claude

    // Guards. Terminal status (Completed/Rejected) + already-InProgress
    // both require --force. We surface the spec id in the error so the
    // user can copy the exact `--force` invocation.
    if matches!(
        current_status,
        RequirementStatus::Completed | RequirementStatus::Rejected
    ) && !force
    {
        // BUG-671: override flag on the FIRST line so agent mode (first-line
        // error summary only) surfaces it. trace:BUG-671 | ai:claude
        anyhow::bail!(
            "{} is {} — re-opening closed work is usually a mistake; pass --force to \
             override.\n  Otherwise, file a new requirement that supersedes {}.",
            display_id,
            current_status,
            display_id
        );
    }
    // STORY-1429: the lease gate. Moving a NeedsAttention or InProgress spec
    // while another session holds a LIVE lease on it would reset work that
    // session owns, so it refuses under every flag, `--force` included
    // (`--force` keeps its meaning: the Completed/Rejected guard above). An
    // unknown lease state refuses too. Runs before any write.
    // trace:STORY-1429 | ai:claude
    let flips = target_status.as_ref().is_some_and(|t| t != &current_status);
    if flips
        && matches!(
            current_status,
            RequirementStatus::NeedsAttention | RequirementStatus::InProgress
        )
    {
        let root = requeue_project_root(storage);
        let check = crate::requeue_lease_gate(&root, &[spec_id.as_str(), display_id.as_str()]);
        if let Some(msg) = check.refusal(&display_id) {
            anyhow::bail!(msg);
        }
    }
    if matches!(current_status, RequirementStatus::InProgress) && !force {
        if work {
            eprintln!(
                "  {} {} is already In Progress — launching the rework session. \
                 Pass `--force` to silence this warning.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                display_id
            );
        } else {
            eprintln!(
                "  {} {} is In Progress without this command establishing a lease — \
                 metadata-only rework resets it to Approved so the queued worker can claim it. \
                 Pass `--force` to silence this warning.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                display_id
            );
        }
    }

    // BUG-1470 F1: the SAME advisor-authority gate the three sibling sites in
    // this file apply (`queue advance --approve`, and the two `queue add`
    // paths), plus `aida edit`. Lifting an un-triaged Draft or a punted
    // NeedsAttention spec into a protected target is an advisor act wherever
    // it happens, and `queue rework` reached that act through a different door.
    //
    // Before this PR the bypass was unreachable: `rework_smart_target(Draft)`
    // returned None, so there was no flip to gate. Mapping Draft -> Approved
    // for metadata-only rework opened the door, which is why the gate has to
    // arrive in the same change.
    //
    // Placed at the FLIP rather than at target selection, so it also covers an
    // explicit `--status` override — that path reaches the identical write and
    // would otherwise remain a side door once this one closed.
    //
    // Refuses BEFORE any side effect (no status write, no comment, no queue
    // entry): a partially-applied rework is harder to reason about than one
    // that did not run.
    // trace:BUG-1470 | ai:claude
    if let Some(ref new_status) = target_status {
        if new_status != &current_status
            && status_advance_requires_advisor_authority(&current_status, new_status)
            && !crate::gate::role_authority::has_advisor_authority()
        {
            println!(
                "  {} reworking {} from {} to {} needs advisor authority. Enter the advisor role at an interactive TTY with `aida role enter advisor`, or ask an advisor session to do it.{}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                display_id.bold(),
                current_status,
                new_status,
                roleless_recovery_sentence()
            );
            return Ok(());
        }
    }

    // Refuse predictable routing holds before any status or comment write.
    // The same owner projects the shelf cleanup; the live check below catches
    // changes during routing. trace:STORY-2690 | ai:codex
    if for_role_resolved.as_deref() != Some("reviewer") {
        let mut projected = req.clone();
        if let Some(target) = &target_status {
            if current_status == RequirementStatus::NeedsAttention && target != &current_status {
                crate::requeue::return_to_flight(
                    &mut projected,
                    &current_status,
                    target,
                    &crate::requeue::ReturnCtx {
                        via: "rework routing preflight".into(),
                        via_slug: "queue-rework",
                        author: get_default_author(),
                        clear_escalation: crate::requeue::caller_may_clear_escalation(),
                        reason: reason.map(str::to_owned),
                        automated: false,
                    },
                );
            } else {
                projected.status = target.clone();
            }
        }
        require_rework_pickable(&projected, &store)?;
    }

    // Decide recurrence before return-to-flight clears review evidence and
    // emits its audit/event: a refused blind retry never leaves the shelf.
    // trace:BUG-2502 | ai:codex
    // A reviewer RequestChanges/Rejection is the actual acceptance delta for a
    // rework pickup. Persist it onto the spec before queueing so every later
    // pickup surface sees the same blocking context. trace:BUG-814 | ai:codex
    let mut pending_findings = None;
    if let Ok(root) = find_project_root() {
        // BUG-2430: a review keyed by the spec's open PR counts too.
        // trace:BUG-2430 | ai:claude
        let open_change = spec_open_change_key(&root, &[display_id.as_str(), spec_id.as_str()]);
        let findings = match resolve_rework_findings(
            &root,
            &display_id,
            &spec_id,
            open_change.as_deref(),
        ) {
            Ok(block) => block,
            Err(why) => {
                // trace:BUG-2507 | ai:codex
                eprintln!("warning: no review findings imported for {display_id}: {why}. Read the review comment before starting rework.");
                None
            }
        };
        if let Some(block) = findings {
            // Decide from existing evidence without recording a round before
            // the status transition succeeds. trace:TASK-3777 | ai:codex
            let recurred = findings_recur_consecutively(&req.comments, &block);
            if !recurred {
                pending_findings = Some(block.clone());
            } else if rework_is_review_shelf(req) {
                // BUG-1213: the same blocking findings produced two consecutive
                // request-changes shelves. A third blind requeue is a loop, so
                // retain the NeedsAttention shelf and route an explicit
                // advisor finding + brief instead.
                // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
                // BUG-1632: the loop guard's decision is automated even when a
                // person ran `queue rework`, so it records the transition under
                // an automated author the BUG-1625 merge guard recognizes.
                // trace:BUG-1632 | ai:claude
                // BUG-1637: a spec that reached a terminal status meanwhile is
                // left alone, and there is no loop left to escalate.
                // trace:BUG-1637 | ai:claude
                let mut parked = false;
                storage.update_spec_atomically(req, |r| parked = loop_guard_park(r))?;
                if !parked {
                    println!(
                        "  {} {display_id} reached a final status meanwhile; left it as it is",
                        "·".dimmed()
                    );
                    return Ok(());
                }
                let note = format!(
                    "Identical reviewer findings recurred twice for {display_id}; do not \
                     requeue a third implementer round until an advisor chooses a different \
                     implementer/seat or resolves the dispute.\n\n{block}"
                );
                // BUG-1199: every in-process `aida` subprocess resolves through the
                // once-per-process resolver, never a raw executable lookup.
                let exe = crate::aida_exe_path();
                let finding_title = format!("Repeated unchanged review findings: {display_id}");
                let finding = std::process::Command::new(&exe)
                    .current_dir(&root)
                    .args([
                        "findings",
                        "add",
                        "--title",
                        &finding_title,
                        "--note",
                        &note,
                        "--severity",
                        "major",
                        "--linked-specs",
                        &display_id,
                        "--tags",
                        "loop-guard,review-recurrence",
                    ])
                    .status_retrying_etxtbsy()?;
                anyhow::ensure!(finding.success(), "could not file repeated-review finding");
                let brief = std::process::Command::new(&exe)
                    .current_dir(&root)
                    .args(["brief", "advisor", &display_id, "--note", &note, "--notify"])
                    .status_retrying_etxtbsy()?;
                anyhow::ensure!(
                    brief.success(),
                    "could not write repeated-review advisor brief"
                );
                println!(
                    "  {} identical review findings recurred — escalated to advisor; not re-queued",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                return Ok(());
            }
        }
    }

    // Status flip (if any). STORY-1429: the target was computed from a status
    // read before this point, so the write re-checks it (compare-and-swap)
    // and changes nothing when the spec moved in between. Leaving
    // NeedsAttention goes through the one
    // owner, `requeue::return_to_flight`, which also clears the shelve
    // markers and writes the single audit note that carries `--reason`.
    // trace:STORY-1429 trace:TASK-1311 | ai:claude
    let mut reason_recorded = false;
    let mut left_attention = false;
    if let Some(ref new_status) = target_status {
        if new_status != &current_status {
            if current_status == RequirementStatus::NeedsAttention {
                let ctx = crate::requeue::ReturnCtx {
                    via: "`aida queue rework`".to_string(),
                    via_slug: "queue-rework",
                    author: get_default_author(),
                    // Only a human at a terminal may undo the advisor's
                    // escalation to a human. trace:TASK-1311 | ai:claude
                    clear_escalation: crate::requeue::caller_may_clear_escalation(),
                    reason: reason.map(str::to_string),
                    automated: false, // trace:BUG-1632 | ai:claude
                };
                let (outcome, _) = crate::requeue::return_to_flight_in_storage(
                    storage,
                    req_id,
                    &current_status,
                    new_status,
                    &ctx,
                )?;
                match &outcome {
                    crate::requeue::ReturnOutcome::Returned { cleared, .. } => {
                        reason_recorded = true;
                        left_attention = true;
                        crate::requeue::emit_requeued(
                            &requeue_project_root(storage),
                            &display_id,
                            &ctx,
                            &outcome,
                        );
                        if !cleared.removed_tags.is_empty() {
                            println!(
                                "  {} cleared stale parking tag(s) {} so the drain can pick it up again",
                                "·".dimmed(),
                                cleared.removed_tags.join(", ")
                            );
                        }
                        if let Some(w) =
                            crate::requeue::kept_escalation_warning(&display_id, cleared)
                        {
                            println!(
                                "  {} {w}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                            );
                        }
                    }
                    crate::requeue::ReturnOutcome::AlreadyInFlight { .. } => {
                        if let Some(msg) = crate::requeue::unchanged_message(&display_id, &outcome)
                        {
                            println!("  {} {msg}", "·".dimmed());
                        }
                        return Ok(());
                    }
                    _ => {
                        let msg = crate::requeue::unchanged_message(&display_id, &outcome)
                            .unwrap_or_default();
                        anyhow::bail!(msg);
                    }
                }
            } else {
                let new_status = new_status.clone();
                let now = chrono::Utc::now();
                let mut moved_to: Option<RequirementStatus> = None;
                // Per-spec compare-and-swap under the store write lock; the
                // status recheck below runs on the copy read under it. trace:BUG-1612 | ai:claude
                storage.update_spec_atomically(req, |r| {
                    if r.status != current_status {
                        moved_to = Some(r.status.clone());
                        return;
                    }
                    r.set_status_from_str(&format!("{:?}", new_status));
                    // BUG-1637: the rework that does not start from
                    // NeedsAttention is a caller-authored status write.
                    // trace:BUG-1637 | ai:claude
                    crate::record_caller_status_transition(r, &current_status);
                    r.modified_at = now;
                    // TASK-1477: `queue rework` can reopen a Completed spec
                    // (Completed -> InProgress is `rework_smart_target`'s
                    // default) — clear the stale completed_at so the next
                    // completion stamps a fresh date. trace:TASK-1477 | ai:claude
                    crate::completion::record_reopen(
                        r,
                        &current_status,
                        Some(&requeue_project_root(storage)),
                    );
                })?;
                if let Some(actual) = moved_to {
                    anyhow::bail!(
                        "{display_id} moved from {current_status} to {actual} while this command \
                         ran; nothing was changed. Re-check it with `aida show {display_id}`"
                    );
                }
            }
            record_role_activity(&spec_id, "rework");
            update_manifest_for_status(&spec_id, &format!("{:?}", new_status));
        }
    }

    // A successful status write is not proof that a worker can pick it up.
    // Validate the live record before moving queue rows or reporting success.
    // trace:STORY-2690 | ai:codex
    if for_role_resolved.as_deref() != Some("reviewer") {
        let live = storage.load()?;
        let current = live
            .requirements
            .iter()
            .find(|r| r.id == req_id)
            .context("rework spec disappeared before routing")?;
        require_rework_pickable(current, &live)?;
    }

    // Persist the acceptance delta only after the flip succeeded, so a failed
    // flip followed by a retry cannot impersonate an implementer round.
    // trace:TASK-3777 | ai:codex
    if let Some(block) = pending_findings {
        let comment = aida_core::Comment::new(get_default_author(), block.clone());
        storage.update_spec_atomically(req, |r| {
            if !findings_recur_consecutively(&r.comments, &block) {
                r.add_comment(comment);
            }
        })?;
        println!("  {} review findings captured as comment", "·".dimmed());
    }

    // Optional audit comment. Mirrors `aida comment add` path so the
    // entry shows up in `aida show <spec>` history. STORY-1429: when the
    // spec left NeedsAttention the reason is already in the audit note, so
    // it is not written a second time.
    if let Some(reason_text) = reason.filter(|_| !reason_recorded) {
        let author = get_default_author();
        let comment = aida_core::Comment::new(author, reason_text.to_string());
        // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
        storage.update_spec_atomically(req, |r| {
            r.add_comment(comment);
        })?;
        println!(
            "  {} reason captured as comment ({} chars)",
            "·".dimmed(),
            reason_text.chars().count()
        );
    } else if let Some(reason_text) = reason {
        println!(
            "  {} reason captured in the audit note ({} chars)",
            "·".dimmed(),
            reason_text.chars().count()
        );
    }

    // Report success after the recurrence guard and status transition succeed.
    // trace:BUG-2502 | ai:codex
    if let Some(ref new_status) = target_status {
        if new_status != &current_status {
            println!(
                "{} {} status: {} → {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                display_id.bold(),
                current_status.to_string().dimmed(),
                new_status.to_string().cyan(),
            );
        }
    }

    // BUG-1277: a rework routed to a role (the default — see above) must
    // land where a batch drain actually reads it: the role's shared queue
    // (`role:<for_role>`), not the invoking user's personal queue. Before
    // this fix, the entry was always filed under the invoking user's own
    // identity, and the command's own printed next-step line advised
    // `--user <invoking-user>` — a queue a role-scoped drain never reads —
    // so the requeued fixup silently never got picked up. An explicit
    // `--user` still overrides verbatim; an unrouted (`--for any`) rework
    // has no natural shared queue and keeps landing on the invoking
    // identity. trace:BUG-1277 | ai:claude
    let user_id: String = match user {
        Some(_) => invoking_user_id.clone(),
        None => match &for_role_resolved {
            Some(role) => format!("role:{role}"),
            None => invoking_user_id.clone(),
        },
    };

    // BUG-1277: if a prior entry for this spec lives under a DIFFERENT
    // identity than where it's landing now, drop it there so rework MOVES
    // the entry instead of leaving a stale duplicate behind — the queue_add
    // below only upserts within the destination user's own file.
    // trace:BUG-1277 | ai:claude
    let mut moved_from = Vec::new();
    for (old_user, _) in &existing_entries_elsewhere {
        if aida_core::node::canonical_user_id(old_user)
            != aida_core::node::canonical_user_id(&user_id)
        {
            // A failed removal leaves the duplicate that this operation exists
            // to prevent, so it is a command failure rather than best-effort
            // cleanup. trace:BUG-1427 | ai:codex
            storage.queue_remove(old_user, &req_id)?;
            moved_from.push(old_user.clone());
        }
    }

    let existing_queue_entries = storage.queue_list(&user_id, true)?;
    let position = if tail {
        i64::MAX // backend resolves to existing_max + 1000
    } else {
        existing_queue_entries
            .first()
            .map(|entry| entry.position - 1000)
            .unwrap_or(1000)
    };

    // Queue add. queue_add upserts by requirement_id (replaces same-spec
    // entries), so re-running rework is idempotent on the queue side.
    let entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: req_id,
        position,
        added_by: user_id.clone(),
        note: reason.map(|r| r.to_string()),
        added_at: chrono::Utc::now(),
        for_role: for_role_resolved.clone(),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry)?;
    for old_user in moved_from {
        println!(
            "  Moved {} queue entry: {} -> {}",
            display_id, old_user, user_id
        );
    }
    record_role_activity(&spec_id, "queue-add");
    let routing = match &for_role_resolved {
        Some(r) => format!(" [for:{}]", r).cyan().to_string(),
        None => String::new(),
    };
    let summary = format!(
        "{} Queued {} ({}){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        display_id.bold(),
        title,
        routing
    );
    let details = queue_destination_details(
        &display_id,
        &user_id,
        "local",
        for_role_resolved.as_deref(),
        true,
    );
    print_queue_mutation_destination("rework", &display_id, Some(&title), &summary, &details)?;

    // STORY-1429: a requeued park re-enters a running drain on its next head
    // pick. Say whether one is running only when the drain lock answers
    // definitely; never launch one. trace:STORY-1429 | ai:claude
    if left_attention && !work {
        println!(
            "  {} {}",
            "·".dimmed(),
            drain_running_notice(&requeue_project_root(storage))
        );
    }

    // Optional --work chain. We don't run this in a sub-process — call
    // `handle_queue_work` directly with the same storage handle so any
    // failure surfaces here. trace:TASK-218 | ai:claude
    if work {
        handle_queue_work(
            storage,
            None,
            &user_id,
            Some(&spec_id),
            permission_mode,
            /* sandbox */ false,
            /* no_launch */ false,
            rework_role_input(for_role, for_role_resolved.as_deref()),
            no_pull,
            /* type_filter */ None,
            /* branch_override */ None,
            /* path_override */ None,
            /* stack */ false,
            /* base */ None,
            /* force_base */ false,
            /* steal is unreachable: handle_queue_rework refuses it at entry */ false,
            /* force */ false,
            /* force_claim */ false,
            // Bare `--resume` → resume the scope's most recent recorded
            // claude session (`resolve_queue_work_launch` fails clean when
            // there is none). trace:BUG-236 | ai:claude
            if resume { Some("") } else { None },
            /* fresh */ false,
            /* list_sessions */ false,
            /* session_id */ None,
            // STORY-761: the convenience chain honors the uniform
            // `[agents] vendor` knob instead of hard-coding claude.
            &find_project_root()
                .ok()
                .and_then(|root| aida_core::agents_config::resolve_default_vendor(&root))
                .unwrap_or_else(|| "claude".to_string()),
            /* no_human */ false,
            // TASK-1060: the `queue add --work` convenience chain has no `--zen`.
            /* autonomy */
            AutonomyMode::Default,
            /* batch_name */ None,
            /* quiet */ false,
            /* allow_stale_base */ false,
            /* allow_intermediate_only */ false,
            /* complexity */ None,
            /* assist_est */ None,
            /* effort */ None,
            /* strict */ false,
            /* plan_only */ false,
            /* guided */ false,
            /* dry_run */ false,
            /* model_override */ None,
        )?;
    } else {
        println!(
            "  ({})",
            format!(
                "run `{}` to start a session for this spec",
                details.pickup_command
            )
            .dimmed()
        );
    }

    Ok(())
}

/// Top-level alias for `aida queue rework SPEC` — single verb for the
/// recurring implementer → reviewer → fixup recovery sequence
/// (status flip + queue add, with optional `--work` session launch).
/// With no ID, triage the specs parked in Needs Attention one keystroke
/// each (at a terminal; otherwise it prints the requeue commands).
/// Mirrors `aida relations` → `aida rel list` discoverability pattern.
/// See `aida queue rework --help` for the full flag set.
// trace:TASK-218 trace:STORY-1429 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "rework")]
pub(crate) struct ReworkArgs {
    /// Requirement ID (UUID or SPEC-ID). Omit it to triage the parked
    /// specs one keystroke each.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
    #[clap(value_parser = crate::spec_arg::spec_id)]
    pub id: Option<String>,
    /// Also launch a session for the spec.
    #[clap(long)]
    pub work: bool,
    /// Override the routing role. Default: existing queue route,
    /// falling back to implementer.
    #[clap(long)]
    pub r#for: Option<String>,
    /// Re-queue at the tail instead of the default head position.
    // trace:BUG-851 | ai:codex
    #[clap(long)]
    pub tail: bool,
    /// Override the smart target status.
    #[clap(long, value_name = "STATE")]
    pub status: Option<String>,
    /// Capture a comment on the spec at rework time.
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub reason: Option<String>,
    /// Record an advisor ruling the next pickup prompt leads with.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
    #[clap(long, allow_hyphen_values = true, value_name = "TEXT")]
    pub ruling: Option<String>,
    /// Chain `aida queue work --resume`.
    #[clap(long)]
    pub resume: bool,
    /// Bypass terminal-status / already-in-progress guards.
    #[clap(long)]
    pub force: bool,
    /// Retired. Refuses explicitly; ownership handles proven-dead reclaim automatically.
    #[clap(long)]
    pub steal: bool,
    /// Permission mode (only used with `--work`).
    #[clap(long, value_name = "MODE")]
    pub permission_mode: Option<String>,
    /// Skip pre-pickup pull (only used with `--work`).
    #[clap(long)]
    pub no_pull: bool,
    /// User ID (defaults to AIDA_USER or system user).
    #[clap(long)]
    pub user: Option<String>,
}

pub(crate) fn run_rework(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    let args = <ReworkArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                // TASK-218: top-level `aida rework SPEC` → forwards to the same
                // handler as `aida queue rework SPEC`. trace:TASK-218 | ai:claude
                ReworkArgs {
                    id,
                    work,
                    r#for,
                    tail,
                    status,
                    reason,
                    ruling,
                    resume,
                    force,
                    steal,
                    permission_mode,
                    no_pull,
                    user,
                } => {
                    let storage = Storage::new(store_path);
                    // STORY-1429: an omitted ID opens the triage loop. trace:STORY-1429 | ai:claude
                    self::handle_rework_entry(
                        &storage,
                        id.as_deref(),
                        &self::ReworkFlags {
                            work: *work,
                            for_role: r#for.as_deref(),
                            tail: *tail,
                            status: status.as_deref(),
                            reason: reason.as_deref(),
                            ruling: ruling.as_deref(),
                            resume: *resume,
                            force: *force,
                            steal: *steal,
                            permission_mode: permission_mode.as_deref(),
                            no_pull: *no_pull,
                            user: user.as_deref(),
                        },
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                // TASK-218: top-level alias in the legacy SQLite dispatch path —
                // forwards to the same handler as `aida queue rework SPEC`.
                // trace:TASK-218 | ai:claude
                ReworkArgs {
                    id,
                    work,
                    r#for,
                    tail,
                    status,
                    reason,
                    ruling,
                    resume,
                    force,
                    steal,
                    permission_mode,
                    no_pull,
                    user,
                } => {
                    // STORY-1429: an omitted ID opens the triage loop. trace:STORY-1429 | ai:claude
                    self::handle_rework_entry(
                        &storage,
                        id.as_deref(),
                        &self::ReworkFlags {
                            work: *work,
                            for_role: r#for.as_deref(),
                            tail: *tail,
                            status: status.as_deref(),
                            reason: reason.as_deref(),
                            ruling: ruling.as_deref(),
                            resume: *resume,
                            force: *force,
                            steal: *steal,
                            permission_mode: permission_mode.as_deref(),
                            no_pull: *no_pull,
                            user: user.as_deref(),
                        },
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static REWORK: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "rework",
        help_after: "autopilot",
        command: <ReworkArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: true,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_rework),
    };

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        // TASK-218: encapsulate the implementer → reviewer → fixup
        // three-command recovery sequence (`aida edit --status` +
        // `aida queue add` + optional `aida queue work`) behind a single
        // verb. Smart status table flips per the current status; `--work`
        // chains the session launch. trace:TASK-218 | ai:claude
        QueueCommand::Rework {
            id,
            work,
            r#for,
            tail,
            status,
            reason,
            ruling,
            resume,
            force,
            steal,
            permission_mode,
            no_pull,
            user,
        } => {
            // Requeueing is dispatch; any status transition it entails is
            // checked separately by the handler. trace:STORY-1353 | ai:codex
            if !crate::gate::role_authority::has_dispatch_authority() {
                anyhow::bail!(
                    "reworking queued work needs dispatch authority (product, advisor, or \
                     integrator role, or a live orchestrator)"
                );
            }
            // STORY-1429: an omitted ID opens the triage loop. trace:STORY-1429 | ai:claude
            crate::queue_cmd::handle_rework_entry(
                storage,
                id.as_deref(),
                &crate::queue_cmd::ReworkFlags {
                    work: *work,
                    for_role: r#for.as_deref(),
                    tail: *tail,
                    status: status.as_deref(),
                    reason: reason.as_deref(),
                    ruling: ruling.as_deref(),
                    resume: *resume,
                    force: *force,
                    steal: *steal,
                    permission_mode: permission_mode.as_deref(),
                    no_pull: *no_pull,
                    user: user.as_deref(),
                },
            )?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}

// Preserve human escalation and all shared pickup gates even for open PRs.
// trace:STORY-2690 | ai:codex
pub(crate) fn require_rework_pickable(
    req: &Requirement,
    store: &aida_core::RequirementsStore,
) -> Result<()> {
    let tags: Vec<_> = req.tags.iter().cloned().collect();
    if let Some(tag) = crate::burndown::parking_tag(&tags) {
        let code = if tag.eq_ignore_ascii_case("needs-human") {
            "needs-human"
        } else {
            "parking-tag"
        };
        anyhow::bail!(
            "rework-routing-refused reason={code}: a human must clear the parking tag {tag}"
        );
    }
    if req
        .decision_request
        .as_ref()
        .is_some_and(|request| request.is_pending())
    {
        anyhow::bail!(
            "rework-routing-refused reason=pending-decision: a human must answer the decision request"
        );
    }
    let policy = super::work::queue_drain_pickup_policy(req, store, false, None);
    let code = match &policy {
        super::work::QueueFreshPickup::Pickable => return Ok(()),
        super::work::QueueFreshPickup::Deferred => "deferred",
        super::work::QueueFreshPickup::Archived => "archived",
        super::work::QueueFreshPickup::NeedsGuidedOrOperatorSession(_) => "supervised-mode",
        super::work::QueueFreshPickup::NeedsReleaseOperatorSession => "release-operator",
        super::work::QueueFreshPickup::AwaitingMerge => "awaiting-merge",
        super::work::QueueFreshPickup::AwaitingRework => "awaiting-rework",
        super::work::QueueFreshPickup::Terminal(_) => "terminal",
        super::work::QueueFreshPickup::Blocked(reason) => match reason {
            aida_core::pickability::BlockedReason::NeedsTriage => "needs-triage",
            aida_core::pickability::BlockedReason::HumanOnly => "human-only",
            aida_core::pickability::BlockedReason::KnowledgeType { .. } => "knowledge-type",
            _ => "blocked-by",
        },
    };
    anyhow::bail!(
        "rework-routing-refused reason={code}: {}",
        super::work::queue_fresh_pickup_reason_label(&policy).unwrap_or_default()
    );
}

#[cfg(test)]
mod story_2690_tests {
    use super::*;

    #[test]
    fn story_2690_routing_preserves_pickup_and_human_gates() {
        let mut store = aida_core::RequirementsStore::default();
        let mut req = Requirement::new("rework".into(), String::new());
        for status in [
            RequirementStatus::Approved,
            RequirementStatus::InProgress,
            RequirementStatus::Planned,
        ] {
            req.status = status;
            assert!(require_rework_pickable(&req, &store).is_ok());
        }
        req.status = RequirementStatus::NeedsAttention;
        assert!(require_rework_pickable(&req, &store)
            .unwrap_err()
            .to_string()
            .contains("reason=needs-triage"));
        req.status = RequirementStatus::Approved;
        req.tags.insert("needs-human".into());
        assert!(require_rework_pickable(&req, &store)
            .unwrap_err()
            .to_string()
            .contains("reason=needs-human"));
        req.tags.clear();
        req.deferred = true;
        assert!(require_rework_pickable(&req, &store)
            .unwrap_err()
            .to_string()
            .contains("reason=deferred"));
        req.deferred = false;
        req.human_only = true;
        assert!(require_rework_pickable(&req, &store)
            .unwrap_err()
            .to_string()
            .contains("reason=human-only"));
        req.human_only = false;
        let blocker = Requirement::new("blocker".into(), String::new());
        req.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::BlockedBy,
            target_id: blocker.id,
            created_at: None,
            created_by: None,
        });
        store.requirements.push(blocker);
        assert!(require_rework_pickable(&req, &store)
            .unwrap_err()
            .to_string()
            .contains("reason=blocked-by"));
    }
}
