//! `aida zen` command cluster — `handle_zen_command`, extracted from
//! `lib.rs` (SPIKE-78 / STORY-771; pure movement, no behavior change).
//! The zen drive engine stays in `lib.rs`/`zen_drive.rs`.
// trace:STORY-771 | ai:claude

use crate::*;

/// `aida zen status` — print the corroborated zen context of the current
/// process. The bare status word (`zen` / `interactive`) goes to stdout so a
/// skill can branch on it cleanly; any informational note goes to stderr.
/// trace:BUG-237 | ai:claude
pub(crate) fn handle_zen_command(cmd: &ZenCommand) -> Result<()> {
    match cmd {
        ZenCommand::Status { json } => {
            // Resolve the shared `.aida/` root from any worktree so a child in
            // a sibling worktree reads the orchestrator's marker dir and the
            // canonical lease set. On a resolution failure the lookups simply
            // find nothing — the verdict fails safe to interactive.
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            let ctx = zen::detect(&project_root, &cwd);
            if *json {
                println!(
                    "{{\"context\":\"{}\",\"corroborated\":{},\"reason\":\"{}\"}}",
                    ctx.status_word(),
                    ctx.is_zen(),
                    ctx.reason_slug()
                );
            } else {
                println!("{}", ctx.status_word());
            }
            // The note is informational. For the no-provenance case it names
            // a real stale value and suggests `unset AIDA_ZEN`.
            if let Some(note) = ctx.informational_note() {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    note.dimmed()
                );
            }
            Ok(())
        }
        // STORY-564: the clean-vs-human-needed finish decision for the
        // standalone `--zen` lane. The `/aida-pr` checkpoint branches off
        // the bare decision word: `auto-exit` → run `aida session end` and
        // exit; `pause` → render the grab-next/stop table. trace:STORY-564
        ZenCommand::Finish { json } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            let is_zen = zen::detect(&project_root, &cwd).is_zen();
            let lease = active_lease_for_cwd(&project_root, &cwd);
            // Marker: the session flagged itself human-needed (paused on a
            // design-fork / raised a punt) via `aida zen needs-human`.
            let needs_human_marked = lease
                .as_ref()
                .map(|l| zen::has_needs_human_marker(&project_root, &l.id))
                .unwrap_or(false);
            // Punt: a substrate record of a human-needed fork raised during
            // this session — scoped to the lease's spec + start time so a
            // sibling session's punt sharing the ledger doesn't leak in.
            let has_open_punt = lease
                .as_ref()
                .map(|l| {
                    let spec = l.scope.trim().to_ascii_uppercase();
                    punt::read_ledger(&project_root)
                        .iter()
                        .any(|r| r.timestamp >= l.started_at && r.spec.to_ascii_uppercase() == spec)
                })
                .unwrap_or(false);
            let pause_always = zen_pause_always_in_force(&project_root);
            let presence_bias = if presence::read_presence_file().is_some() {
                match presence::current_presence(chrono::Utc::now()) {
                    presence::Presence::Away => zen::FinishPresence::Away,
                    presence::Presence::Home => zen::FinishPresence::Home,
                }
            } else {
                zen::FinishPresence::NoOpinion
            };
            let verdict = zen::classify_finish(
                is_zen,
                needs_human_marked,
                has_open_punt,
                pause_always,
                presence_bias,
            );
            if *json {
                println!(
                    "{{\"decision\":\"{}\",\"reason\":\"{}\",\"corroborated\":{}}}",
                    verdict.decision_word(),
                    verdict.reason_slug(),
                    is_zen
                );
            } else {
                println!("{}", verdict.decision_word());
            }
            // STORY-569: on a CLEAN finish — auto-exit, or a pause only
            // because the operator elected `--pause-always` — hand the
            // just-opened PR to the advisor through the agent mailbox.
            // This lives in the gate (not skill text) so the finish
            // sequence is finish → PR → review brief → exit by
            // construction, and the build→review handoff never falls back
            // to an operator relay. A needs-human / punt pause skips: the
            // operator is actively in the loop and the PR may still
            // change. Fail-open: a notify failure must never block the
            // finish decision. trace:STORY-569 | ai:claude
            let clean_finish = matches!(
                verdict,
                zen::ZenFinish::AutoExit | zen::ZenFinish::Pause(zen::FinishPause::PauseAlways)
            );
            if clean_finish {
                if let Some(l) = lease.as_ref() {
                    match file_zen_review_brief(&project_root, l) {
                        Ok(Some((agent, path))) => eprintln!(
                            "  {} review brief filed to the {} mailbox: {}",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            agent.cyan(),
                            path.display()
                        ),
                        Ok(None) => {}
                        Err(e) => eprintln!(
                            "  {} could not file the review brief ({e}) — \
                             hand the PR to your reviewer manually",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        ),
                    }
                }
            }
            Ok(())
        }
        // STORY-564: record that this `--zen` session needed a human. Keyed
        // to the session lease so concurrent zen sessions don't collide.
        ZenCommand::NeedsHuman { reason } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            match active_lease_for_cwd(&project_root, &cwd) {
                Some(lease) => {
                    zen::mark_needs_human(&project_root, &lease.id, reason)?;
                    println!(
                        "{} marked this --zen session human-needed — the finish checkpoint will pause",
                        crate::glyph(crate::glyphs::Glyph::Check).green()
                    );
                    Ok(())
                }
                None => {
                    // No lease over this cwd: nothing to scope the marker to.
                    // Don't fail — just tell the operator the gate can't see it.
                    eprintln!(
                        "  {} no active session lease covers this directory — `aida zen finish` \
                         keys the needs-human marker off the lease, so there is nothing to mark",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                    Ok(())
                }
            }
        }
    }
}
