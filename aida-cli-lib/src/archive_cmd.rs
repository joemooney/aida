//! `aida archive` / `aida unarchive` command cluster (STORY-441 / BUG-492).
//!
//! The view-level archive flag orthogonal to lifecycle status: single-id and
//! bulk-sweep archive, the active-work guard that refuses to silently sweep
//! non-terminal / queued specs, and the inverse unarchive. Extracted verbatim
//! from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

use aida_core::DatabaseBackend;
use aida_core::RequirementStatus;
use aida_core::Storage;

use crate::not_found;
use crate::{
    current_user_id, is_terminal_status_str, parse_since_arg, prompt_yes_no, record_role_activity,
    shorten_text,
};

// STORY-441: `aida archive <ID>` and `aida archive --older-than <DUR>`.
// Single-id form mutates one spec; bulk form sweeps every spec whose
// status is in `status` (default `completed,rejected`) and whose
// `modified_at` is older than the cutoff. `--dry-run` prints the plan
// and exits. trace:STORY-441 | ai:claude
// BUG-492: the single-spec archive guard decision, factored out as a pure
// function so the "non-terminal / queued / forced" matrix is unit-testable
// without touching the store or a TTY. `Allow` archives silently;
// `Confirm` carries the warning reason and demands an interactive y/N (or
// `--force`). trace:BUG-492 | ai:claude
#[derive(Debug, PartialEq, Eq)]
enum ArchiveGuard {
    Allow,
    Confirm { reason: String },
}

// trace:BUG-492 | ai:claude
// TASK-741: the legality of an archive — the `archived ⇒ terminal ∧ ¬queued`
// cross-axis invariant — is single-sourced in the lifecycle model
// (`lifecycle::archive_invariant_block_for_type`, declared as the `BUG-492`
// row of `lifecycle::INVARIANTS`). This function maps the model's verdict onto
// the CLI's `--force` override and user-facing warning wording (both CLI
// concerns that stay at the call site). trace:TASK-741 | ai:claude
// BUG-761: the gate is class-aware — for the decision class (ADRs), Approved
// IS the terminal "accepted" state (BUG-751 convention: draft = proposed,
// approved = accepted), so an accepted ADR archives bare instead of sitting
// in the open lens forever. Work-spec refusal is unchanged.
// trace:BUG-761 | ai:claude
fn archive_guard_decision(
    req_type: &aida_core::RequirementType,
    status: &RequirementStatus,
    queued: bool,
    force: bool,
) -> ArchiveGuard {
    use aida_core::lifecycle::{archive_invariant_block_for_type, ArchiveBlock, State};
    if force {
        return ArchiveGuard::Allow;
    }
    match archive_invariant_block_for_type(req_type, State::from_status(status), queued) {
        None => ArchiveGuard::Allow,
        Some(ArchiveBlock::Queued) => ArchiveGuard::Confirm {
            reason: format!(
                "is {status} AND in the queue — archiving it leaves the queue pointing at a \
                 hidden spec (`aida list` won't show it)."
            ),
        },
        Some(ArchiveBlock::NonTerminal) => ArchiveGuard::Confirm {
            reason: format!(
                "is {status} (not Completed/Rejected). Archive is for the closed long-tail; \
                 archiving live work hides it from `aida list`."
            ),
        },
    }
}

pub(crate) fn handle_archive_command(
    id: Option<&str>,
    older_than: Option<&str>,
    status_csv: Option<&str>,
    dry_run: bool,
    force: bool,
    verbose: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    match (id, older_than) {
        (Some(id), None) => archive_single(id, force, backend, store_path),
        (None, Some(dur)) => archive_sweep(dur, status_csv, dry_run, force, verbose, backend),
        (Some(_), Some(_)) => {
            // Clap's `conflicts_with` should catch this — but defend in depth.
            anyhow::bail!("--older-than cannot be used with a positional SPEC-ID");
        }
        (None, None) => anyhow::bail!(
            "either pass a SPEC-ID (`aida archive FR-1`) or `--older-than <DURATION>` \
             for a bulk sweep (e.g. `--older-than 30d`)"
        ),
    }
}

// BUG-492: archive is for the closed long-tail (Completed/Rejected). A
// non-terminal spec — and especially a QUEUED one — being archived is the
// active-work footgun that silently swept 128 Approved specs (incl. 4
// queued items) in the Session-63 reset. The single-id path now refuses a
// non-terminal archive without `--force` (or an interactive confirm), warns
// louder + dequeues when the spec is in the queue, and `queue list` flags
// any archived member. trace:BUG-492 | ai:claude
fn archive_single(
    id: &str,
    force: bool,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if req.archived {
        println!(
            "{} {display_id} is already archived (since {})",
            "Note:".dimmed(),
            req.archived_at
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown".into())
        );
        return Ok(());
    }

    // BUG-492: is this spec sitting in the queue? An archived + queued spec
    // is contradictory state (`list` hides it, `queue list` keeps showing
    // it), so we name the queue loudly and dequeue on the way out.
    // trace:BUG-492 | ai:claude
    let queue_storage = Storage::new(store_path.to_path_buf());
    let queue_user_id = current_user_id(None);
    let queued = queue_storage
        .queue_list(&queue_user_id, /* include_completed */ true)
        .map(|entries| entries.iter().any(|e| e.requirement_id == req.id))
        .unwrap_or(false);

    // BUG-492: guard the active-work case. A non-terminal or queued spec
    // needs --force (or an interactive y/N confirm). Archive's job is the
    // closed long-tail; sweeping live work is almost always a mistake.
    // BUG-761: class-aware — an accepted (Approved) decision spec is closed
    // and archives bare. trace:BUG-492 trace:BUG-761 | ai:claude
    match archive_guard_decision(&req.req_type, &req.status, queued, force) {
        ArchiveGuard::Allow => {}
        ArchiveGuard::Confirm { reason } => {
            eprintln!("{} {display_id} {reason}", "Warning:".yellow().bold());
            let confirmed = if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                prompt_yes_no(&format!("Archive {display_id} anyway?"), false)?
            } else {
                false
            };
            if !confirmed {
                anyhow::bail!(
                    "refused to archive {display_id} ({}{}) — pass --force to override",
                    req.status,
                    if queued { ", queued" } else { "" }
                );
            }
        }
    }

    // BUG-492: resolve the contradiction — dequeue before archiving so we
    // never leave an archived spec in the queue. trace:BUG-492 | ai:claude
    if queued {
        if let Err(e) = queue_storage.queue_remove(&queue_user_id, &req.id) {
            eprintln!(
                "{} could not dequeue {display_id} ({e}); archiving anyway, but the \
                 queue still references it.",
                "Warning:".yellow(),
            );
        } else {
            println!("{} {display_id} removed from the queue", "Dequeued:".cyan());
        }
    }

    let now = chrono::Utc::now();
    req.archived = true;
    req.archived_at = Some(now);
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "archive");
    println!("{} {display_id}", "Archived:".cyan().bold());
    Ok(())
}

fn archive_sweep(
    duration: &str,
    status_csv: Option<&str>,
    dry_run: bool,
    force: bool,
    verbose: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    let cutoff = parse_since_arg(duration)
        .map_err(|e| anyhow::anyhow!("invalid --older-than `{duration}`: {e}"))?;
    let statuses: Vec<String> = status_csv
        .unwrap_or("completed,rejected")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if statuses.is_empty() {
        anyhow::bail!("--status must list at least one status (default: completed,rejected)");
    }

    // BUG-492: a bulk sweep that includes non-terminal statuses
    // (Draft/Approved/Planned/InProgress) is the scripted form of the same
    // active-work footgun. The default csv (completed,rejected) is
    // terminal-only; an explicit `--status approved` must also carry
    // `--force` so a wide `--older-than … --status approved` can't silently
    // archive live backlog. trace:BUG-492 | ai:claude
    if !force {
        let non_terminal: Vec<&String> = statuses
            .iter()
            .filter(|s| !is_terminal_status_str(s))
            .collect();
        if !non_terminal.is_empty() {
            let names = non_terminal
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",");
            anyhow::bail!(
                "--older-than refuses non-terminal status(es) [{names}] by default — \
                 archive is for closed work (Completed/Rejected). Pass --force to \
                 include live backlog in the sweep."
            );
        }
    }

    // Pull all non-archived rows matching any of the requested statuses,
    // then post-filter by modified_at < cutoff. The cache's status filter
    // is single-value so we do one query per status and merge.
    let mut candidates: Vec<aida_core::RequirementSummary> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for s in &statuses {
        let filter = aida_core::ListFilter {
            status: Some(s.clone()),
            archive: aida_core::ArchiveFilter::NonArchivedOnly,
            ..Default::default()
        };
        for row in backend.list_summaries(&filter)? {
            if seen.insert(row.id) {
                candidates.push(row);
            }
        }
    }

    // Filter by age. `modified_at` is RFC3339; parse and compare.
    let eligible: Vec<aida_core::RequirementSummary> = candidates
        .into_iter()
        .filter(|s| {
            chrono::DateTime::parse_from_rfc3339(&s.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc) < cutoff)
                .unwrap_or(false)
        })
        .collect();

    if eligible.is_empty() {
        println!(
            "{} no specs match --older-than {duration} --status {} (nothing to archive)",
            "Note:".dimmed(),
            statuses.join(",")
        );
        return Ok(());
    }

    if dry_run {
        println!(
            "{} {} spec(s) older than {duration} with status in {} (--dry-run, no writes):",
            "Would archive:".cyan().bold(),
            eligible.len(),
            statuses.join(",")
        );
        for s in &eligible {
            let display_id = s
                .agreed_id
                .as_deref()
                .or(s.spec_id.as_deref())
                .unwrap_or("?");
            println!(
                "  {:<14} {:<10} {}",
                display_id,
                s.status,
                shorten_text(&s.title, 60)
            );
        }
        return Ok(());
    }

    let now = chrono::Utc::now();
    let total = eligible.len();
    // BUG-425: collect the mutations, then commit them all in ONE store commit
    // via `bulk_update`, instead of one git commit per spec. A 679-spec sweep
    // used to make 679 commits (minutes of git, burying the store history);
    // now it's a single fast commit. trace:BUG-425 | ai:claude
    //
    // BUG-497: BUG-425's premise ("single fast commit → no per-spec progress
    // needed") breaks at scale — the collection loop reads N YAMLs and the
    // bulk_update writes + commits N files, which is multi-second silence on a
    // 200+ spec sweep that looks hung. Restore a THROTTLED tick (every ~250ms
    // or every K specs) for large sweeps; small sweeps stay quiet. All
    // progress goes to stderr so stdout stays clean for pipe consumers.
    // trace:BUG-497 | ai:claude
    const PROGRESS_THRESHOLD: usize = 50;
    const PROGRESS_EVERY: usize = 25;
    let progress_interval = std::time::Duration::from_millis(250);
    let show_progress = total > PROGRESS_THRESHOLD;
    eprintln!(
        "{} {total} spec(s) older than {duration} (status in {})…",
        "Archiving:".cyan().bold(),
        statuses.join(",")
    );
    let mut to_archive = Vec::with_capacity(total);
    let mut last_tick = std::time::Instant::now();
    for (i, s) in eligible.iter().enumerate() {
        let display_id = s
            .agreed_id
            .as_deref()
            .or(s.spec_id.as_deref())
            .unwrap_or_default()
            .to_string();
        let Some(mut req) = backend.get_requirement(&s.id)? else {
            continue;
        };
        if req.archived {
            continue;
        }
        req.archived = true;
        req.archived_at = Some(now);
        req.modified_at = now;
        if verbose {
            eprintln!("  [{}/{}] {display_id}", i + 1, total);
        } else if show_progress
            && (i + 1 == total
                || (i + 1) % PROGRESS_EVERY == 0
                || last_tick.elapsed() >= progress_interval)
        {
            eprintln!("  {} [{}/{}]", "…".dimmed(), i + 1, total);
            last_tick = std::time::Instant::now();
        }
        to_archive.push(req);
        record_role_activity(&display_id, "archive");
    }
    let archived_count = backend.bulk_update(&to_archive, "chore(archive)")?;
    println!(
        "{} {archived_count} spec(s) in 1 commit (older than {duration}, status in {})",
        "Archived:".cyan().bold(),
        statuses.join(",")
    );
    Ok(())
}

// trace:STORY-441 | ai:claude
pub(crate) fn handle_unarchive_command(
    id: &str,
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
) -> Result<()> {
    let mut req = backend
        .get_requirement_by_spec_id(id)?
        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
    let display_id = req.spec_id.clone().unwrap_or_else(|| id.to_string());
    if !req.archived {
        anyhow::bail!(
            "{display_id} is not archived — nothing to unarchive. \
             Use `aida archive {display_id}` if you meant to archive it."
        );
    }
    let now = chrono::Utc::now();
    req.archived = false;
    req.archived_at = None;
    req.modified_at = now;
    backend.update_requirement(&req)?;
    record_role_activity(&display_id, "unarchive");
    println!("{} {display_id}", "Unarchived:".cyan().bold());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{archive_guard_decision, ArchiveGuard};
    use crate::is_terminal_status;
    use aida_core::RequirementStatus;

    // TASK-741: exhaustive parity — `archive_guard_decision` (now delegating
    // to `lifecycle::archive_invariant_block_for_type`) yields the same
    // verdict as the pre-migration predicate over every
    // `(status, queued, force)` triple for a WORK-class spec (the decision
    // class deliberately diverges — BUG-761). The `--force` override and the
    // two reason wordings (queued-louder vs non-terminal) must be
    // byte-identical. trace:TASK-741 trace:BUG-761
    #[test]
    fn archive_guard_decision_parity_with_oracle() {
        use aida_core::models::RequirementStatus as S;
        use aida_core::RequirementType;
        // The pre-migration body, verbatim.
        fn oracle(status: &S, queued: bool, force: bool) -> ArchiveGuard {
            let non_terminal = !is_terminal_status(status);
            if force || !(non_terminal || queued) {
                return ArchiveGuard::Allow;
            }
            let reason = if queued {
                format!(
                    "is {status} AND in the queue — archiving it leaves the queue pointing at a \
                     hidden spec (`aida list` won't show it)."
                )
            } else {
                format!(
                    "is {status} (not Completed/Rejected). Archive is for the closed long-tail; \
                     archiving live work hides it from `aida list`."
                )
            };
            ArchiveGuard::Confirm { reason }
        }
        let all = [
            S::Draft,
            S::Approved,
            S::Planned,
            S::InProgress,
            S::Done,
            S::Completed,
            S::Rejected,
            S::NeedsAttention,
        ];
        for s in &all {
            for queued in [false, true] {
                for force in [false, true] {
                    assert_eq!(
                        archive_guard_decision(&RequirementType::Task, s, queued, force),
                        oracle(s, queued, force),
                        "parity mismatch at status={s} queued={queued} force={force}"
                    );
                }
            }
        }
    }

    // BUG-761: the gate is class-aware — for the decision class (ADRs),
    // `approved` IS the terminal "accepted" state (BUG-751: draft = proposed,
    // approved = accepted), so `aida archive ADR-N` on an accepted decision
    // succeeds bare. Work-spec refusal is unchanged (covered by the parity
    // test above); the queue axis still blocks first even for a decision.
    // trace:BUG-761 | ai:claude
    #[test]
    fn archive_guard_allows_accepted_decision_bare() {
        use aida_core::RequirementType as T;

        // Accepted (Approved) decision, unqueued, no --force → archives
        // silently. This is the bug: it used to demand --force forever.
        assert_eq!(
            archive_guard_decision(&T::Decision, &RequirementStatus::Approved, false, false),
            ArchiveGuard::Allow,
            "an accepted ADR is closed — it must archive bare"
        );

        // A proposed (Draft) decision is still open — refusal unchanged.
        assert!(
            matches!(
                archive_guard_decision(&T::Decision, &RequirementStatus::Draft, false, false),
                ArchiveGuard::Confirm { .. }
            ),
            "a proposed (Draft) decision is still open and must confirm"
        );

        // Queued precedence unchanged: even an accepted decision in the
        // queue is contradictory state and must confirm, naming the queue.
        match archive_guard_decision(&T::Decision, &RequirementStatus::Approved, true, false) {
            ArchiveGuard::Confirm { reason } => assert!(
                reason.contains("queue"),
                "queued accepted decision must name the queue: {reason}"
            ),
            ArchiveGuard::Allow => panic!("a queued decision must still require confirmation"),
        }

        // Work-spec refusal unchanged: an Approved non-decision still blocks.
        assert!(
            matches!(
                archive_guard_decision(&T::Task, &RequirementStatus::Approved, false, false),
                ArchiveGuard::Confirm { .. }
            ),
            "Approved work specs must still require confirmation"
        );
    }

    // BUG-492: `aida archive <ID>` must not silently sweep live work. The
    // guard fires (demands confirm / --force) for any non-terminal status,
    // and louder when the spec is also queued; a terminal spec archives
    // freely; `--force` always passes through. This is the regression for
    // the Session-63 over-sweep that archived 128 Approved specs (incl. 4
    // queued items). trace:BUG-492 | ai:claude
    #[test]
    fn archive_guard_blocks_non_terminal_and_queued() {
        // Terminal, not queued → archive silently.
        assert_eq!(
            archive_guard_decision(
                &aida_core::RequirementType::Task,
                &RequirementStatus::Completed,
                false,
                false
            ),
            ArchiveGuard::Allow,
            "closed long-tail archives without a prompt"
        );
        assert_eq!(
            archive_guard_decision(
                &aida_core::RequirementType::Task,
                &RequirementStatus::Rejected,
                false,
                false
            ),
            ArchiveGuard::Allow
        );

        // Non-terminal, not queued → must confirm (the bug: this used to
        // archive silently).
        let g = archive_guard_decision(
            &aida_core::RequirementType::Task,
            &RequirementStatus::Approved,
            false,
            false,
        );
        match &g {
            ArchiveGuard::Confirm { reason } => {
                assert!(
                    reason.contains("Approved") && reason.contains("aida list"),
                    "non-terminal warning must name the status + the hidden-from-list risk: {reason}"
                );
                assert!(
                    !reason.contains("queue"),
                    "un-queued spec should not mention the queue: {reason}"
                );
            }
            ArchiveGuard::Allow => {
                panic!("Approved must require confirmation, not archive silently")
            }
        }

        // Non-terminal AND queued → confirm with the louder, queue-naming
        // reason (the unambiguously-wrong case from the bug).
        let g = archive_guard_decision(
            &aida_core::RequirementType::Task,
            &RequirementStatus::Approved,
            true,
            false,
        );
        match &g {
            ArchiveGuard::Confirm { reason } => assert!(
                reason.contains("queue"),
                "queued+non-terminal warning must name the queue: {reason}"
            ),
            ArchiveGuard::Allow => panic!("queued spec must require confirmation"),
        }

        // Even a *terminal* spec that is still queued should be flagged so
        // the contradiction gets reconciled.
        assert!(
            matches!(
                archive_guard_decision(
                    &aida_core::RequirementType::Task,
                    &RequirementStatus::Completed,
                    true,
                    false
                ),
                ArchiveGuard::Confirm { .. }
            ),
            "a queued terminal spec is still contradictory state and must confirm"
        );

        // --force always passes through, regardless of status / queue.
        assert_eq!(
            archive_guard_decision(
                &aida_core::RequirementType::Task,
                &RequirementStatus::Approved,
                true,
                true
            ),
            ArchiveGuard::Allow,
            "--force opts past the guard"
        );
        assert_eq!(
            archive_guard_decision(
                &aida_core::RequirementType::Task,
                &RequirementStatus::InProgress,
                false,
                true
            ),
            ArchiveGuard::Allow
        );
    }
}
