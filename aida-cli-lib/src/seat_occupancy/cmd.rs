//! `aida session seat claim|status|release|takeover|ack` (TASK-1607).
//!
//! Graceful takeover only: `--kill` is accepted by the parser so the surface
//! is stable, but always refuses until STORY-1485 slice B (TASK-1608).
//!
//! trace:TASK-1607 | ai:claude

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::Path;

use super::config::{validate_grace, SeatConfig};
use super::runtime::{current_caller, with_system_ctx};
use super::store::SeatStore;
use super::{
    holder_liveness, holder_view, in_flight, Handover, HolderView, SeatRecord, TakeoverRequest,
    TakeoverStep, WaitState, KILL_SCAFFOLDING,
};
use crate::cli::SeatCommand;

/// How often a waiting requester re-reads the seat.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) fn dispatch(cmd: &SeatCommand) -> Result<()> {
    // Refuse --kill before touching any state: it is scaffolding until
    // STORY-1485 slice B, whatever the caller's authority.
    if matches!(
        cmd,
        SeatCommand::Claim { kill: true, .. } | SeatCommand::Takeover { kill: true, .. }
    ) {
        bail!("{KILL_SCAFFOLDING}");
    }
    let project_root = crate::find_project_root()?;
    match cmd {
        SeatCommand::Claim {
            seat,
            force,
            grace_secs,
            no_wait,
            ..
        } => {
            let seat = single_seat(&project_root, seat)?;
            if *force {
                return run_takeover(&project_root, &seat, *grace_secs, None, *no_wait);
            }
            run_claim(&project_root, &seat)
        }
        SeatCommand::Status { seat, json } => {
            let seat = single_seat(&project_root, seat)?;
            run_status(&project_root, &seat, *json)
        }
        SeatCommand::Release {
            seat,
            generation,
            request,
        } => {
            let seat = single_seat(&project_root, seat)?;
            run_release(&project_root, &seat, *generation, request.as_deref())
        }
        SeatCommand::Takeover {
            seat,
            force,
            grace_secs,
            request,
            no_wait,
            ..
        } => {
            let seat = single_seat(&project_root, seat)?;
            if !*force {
                bail!("a takeover asks a live session to stand down; pass --force to request it");
            }
            run_takeover(
                &project_root,
                &seat,
                *grace_secs,
                request.as_deref(),
                *no_wait,
            )
        }
        SeatCommand::Ack {
            seat,
            request,
            generation,
            safe_to_stop,
        } => {
            let seat = single_seat(&project_root, seat)?;
            if !*safe_to_stop {
                bail!("acknowledge only after writing your handoff and stopping new dispatch; pass --safe-to-stop to confirm");
            }
            run_ack(&project_root, &seat, request, *generation)
        }
    }
}

fn single_seat(project_root: &Path, seat: &str) -> Result<String> {
    let seat = aida_core::team::canonical_role(seat);
    let cfg = SeatConfig::load(project_root)?;
    if !cfg.is_single(&seat) {
        bail!(
            "`{seat}` is not a single-occupancy seat (configured: {})",
            cfg.single_seats.join(", ")
        );
    }
    Ok(seat)
}

fn store(project_root: &Path) -> Result<SeatStore> {
    SeatStore::for_project(project_root)
}

fn run_claim(project_root: &Path, seat: &str) -> Result<()> {
    let caller = current_caller(project_root);
    let mut guard = store(project_root)?.lock(seat)?;
    let out = with_system_ctx(|ctx| super::claim(&mut guard.rec, ctx, &caller));
    guard.save()?;
    match out {
        Ok(Some(notice)) => println!("{notice}"),
        Ok(None) => println!(
            "You already hold the {seat} seat (generation {}).",
            guard.rec.generation
        ),
        Err(refusal) => return Err(refusal.into()),
    }
    Ok(())
}

// Local reads only, performed while the caller holds the seat lock. No grant
// snapshot crosses lock acquisition. Reuse the same resolver as role current.
// trace:TASK-1607 | ai:codex
fn with_transfer_ctx<T>(project_root: &Path, f: impl FnOnce(&super::Ctx) -> T) -> T {
    with_system_ctx(|ctx| {
        let validate = |req: &TakeoverRequest, seat: &str| {
            req.requester_subject
                .as_deref()
                .and_then(|subject| {
                    crate::seat_authority::validated_grant(
                        project_root,
                        &req.requester_grant_id,
                        subject,
                        chrono::Utc::now(),
                    )
                })
                .is_some_and(|g| {
                    g.session_id == req.requester_session
                        && g.seat == seat
                        && g.parent_grant_id.is_none()
                        && chrono::Utc::now() < req.deadline
                })
        };
        f(&super::Ctx {
            requester_valid: Some(&validate),
            ..*ctx
        })
    })
}

fn run_release(
    project_root: &Path,
    seat: &str,
    generation: u64,
    request: Option<&str>,
) -> Result<()> {
    let caller = current_caller(project_root);
    let mut guard = store(project_root)?.lock(seat)?;
    let out = with_transfer_ctx(project_root, |ctx| {
        super::release(&mut guard.rec, ctx, &caller, generation, request)
    });
    guard.save()?;
    match out? {
        Handover::Released { generation } => {
            println!("Released the {seat} seat (now generation {generation}, empty).")
        }
        Handover::Transferred {
            to_session,
            generation,
        } => println!(
            "Released the {seat} seat to the takeover requester (session {to_session}, generation {generation})."
        ),
    }
    Ok(())
}

fn run_ack(project_root: &Path, seat: &str, request: &str, generation: u64) -> Result<()> {
    let caller = current_caller(project_root);
    let mut guard = store(project_root)?.lock(seat)?;
    let out = with_transfer_ctx(project_root, |ctx| {
        super::ack(&mut guard.rec, ctx, &caller, request, generation)
    });
    guard.save()?;
    match out? {
        Handover::Transferred {
            to_session,
            generation,
        } => println!(
            "Acknowledged: the {seat} seat now belongs to session {to_session} (generation {generation}). This session must not dispatch further work."
        ),
        Handover::Released { generation } => {
            println!("The {seat} seat is now empty (generation {generation}).")
        }
    }
    Ok(())
}

fn run_takeover(
    project_root: &Path,
    seat: &str,
    grace_override: Option<i64>,
    resume: Option<&str>,
    no_wait: bool,
) -> Result<()> {
    let cfg = SeatConfig::load(project_root)?;
    let grace = match grace_override {
        Some(secs) => validate_grace(secs)?,
        None => cfg.takeover_grace_secs,
    };
    let caller = current_caller(project_root);
    let store = store(project_root)?;
    let step = {
        let mut guard = store.lock(seat)?;
        let step = with_system_ctx(|ctx| {
            super::takeover(
                &mut guard.rec,
                ctx,
                &caller,
                chrono::Duration::seconds(grace as i64),
                resume,
            )
        });
        guard.save()?;
        step?
        // The lock drops here: notification and waiting happen unlocked.
    };
    let (request, created) = match step {
        TakeoverStep::Claimed(notice) => {
            println!("{notice}");
            return Ok(());
        }
        TakeoverStep::AlreadyHolder => {
            println!("You already hold the {seat} seat.");
            return Ok(());
        }
        TakeoverStep::Pending { request, created } => (request, created),
    };
    let holder = store.peek(seat)?;
    if created {
        if let Err(e) = notify_holder(project_root, seat, &holder, &request) {
            // The request is durable; the holder also sees it on its next
            // protected op. A failed notification is reported, not fatal.
            eprintln!("warning: could not post the takeover notice to the mailbox: {e:#}");
        }
    }
    println!(
        "{} takeover request {} for the {seat} seat (holder session {}, generation {}).",
        if created { "Sent" } else { "Resuming" },
        request.id,
        request.expected_holder_session,
        request.expected_generation
    );
    println!(
        "Waiting until {} for the holder to acknowledge. No process will be stopped.",
        request.deadline.format("%Y-%m-%d %H:%M:%SZ")
    );
    println!(
        "If interrupted, resume with: aida session seat takeover --seat {seat} --force --request {}",
        request.id
    );
    if no_wait {
        return Ok(());
    }
    wait_for_transfer(&store, seat, &request)
}

fn wait_for_transfer(store: &SeatStore, seat: &str, request: &TakeoverRequest) -> Result<()> {
    loop {
        let state = {
            let mut guard = store.lock(seat)?;
            let state = with_system_ctx(|ctx| {
                super::poll_takeover(&mut guard.rec, ctx, &request.id, &request.requester_session)
            });
            guard.save()?;
            state
        };
        match state {
            WaitState::Granted { generation } => {
                println!("The {seat} seat is yours (generation {generation}).");
                return Ok(());
            }
            WaitState::Waiting => std::thread::sleep(POLL_INTERVAL),
            WaitState::TimedOut => bail!(
                "the holder did not acknowledge request {} before its deadline; the request was withdrawn, no process was stopped, and the holder keeps the {seat} seat",
                request.id
            ),
            WaitState::Ended(why) => bail!("{why}"),
        }
    }
}

/// Notification only (sketch section 3): mail never transfers the seat.
fn notify_holder(
    project_root: &Path,
    seat: &str,
    rec: &SeatRecord,
    request: &TakeoverRequest,
) -> Result<()> {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let in_flight = in_flight(rec);
    let body = format!(
        "Takeover requested for the {seat} seat.\n\
         request: {id}\n\
         holder session: {holder} (generation {generation})\n\
         requested by: session {requester} ({cmd} pid {pid})\n\
         deadline: {deadline}\n\
         in flight: {in_flight}\n\n\
         If you hold this seat: stop dispatching new work, write your handoff\n\
         (aida session handoff --seat {seat} --write FILE), then acknowledge:\n\
         aida session seat ack --seat {seat} --request {id} --generation {generation} --safe-to-stop\n\n\
         This message is a notification. Replying to it does not hand over the seat.",
        id = request.id,
        holder = request.expected_holder_session,
        generation = request.expected_generation,
        requester = request.requester_session,
        cmd = request.requester_command,
        pid = request.requester_anchor.pid,
        deadline = request.deadline.format("%Y-%m-%d %H:%M:%SZ"),
        in_flight = if in_flight.is_empty() {
            "none recorded".to_string()
        } else {
            in_flight.join(", ")
        },
    );
    let (from, from_source) = crate::resolve_mail_sender_identity(None);
    let id = uuid::Uuid::new_v4().to_string();
    let msg = Message {
        subject: Some(format!("Takeover requested: {seat} seat")),
        id: id.clone(),
        thread_id: id,
        from,
        to: Recipient::Agent(seat.to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: true,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source,
        from_role: crate::resolve_mail_sender_role(),
        relayed_from: None,
    };
    crate::mailbox_store::write_message(project_root, &msg).context("writing the takeover notice")
}

#[derive(Serialize)]
struct StatusJson<'a> {
    seat: &'a str,
    generation: u64,
    holder: Option<HolderView>,
    request: Option<&'a TakeoverRequest>,
    in_flight: Vec<String>,
    demoted: Vec<String>,
    record: String,
}

fn run_status(project_root: &Path, seat: &str, json: bool) -> Result<()> {
    let store = store(project_root)?;
    let rec = store.peek(seat)?;
    let holder = with_system_ctx(|ctx| holder_view(&rec, ctx));
    let demoted: Vec<String> = rec
        .tombstones
        .iter()
        .map(|t| {
            format!(
                "pid {} (session {}, demoted at generation {}: {})",
                t.process.pid, t.grant_session, t.generation, t.reason
            )
        })
        .collect();
    let record = store
        .dir()
        .join(format!("{seat}.json"))
        .display()
        .to_string();
    if json {
        let out = StatusJson {
            seat,
            generation: rec.generation,
            holder,
            request: rec.request.as_ref(),
            in_flight: in_flight(&rec),
            demoted,
            record,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("Seat:        {seat} (generation {})", rec.generation);
    match (&holder, &rec.holder) {
        (Some(v), Some(h)) => {
            let live = with_system_ctx(|ctx| holder_liveness(h, ctx));
            println!("Holder:      session {}", v.session);
            println!("Process:     {} pid {}", v.anchor_command, v.anchor_pid);
            for b in &h.bound {
                println!("Loop:        pid {}", b.pid);
            }
            println!("Since:       {}", v.since.format("%Y-%m-%d %H:%M:%SZ"));
            match live.reason() {
                Some(r) => println!("Liveness:    {} ({r})", live.label()),
                None => println!("Liveness:    {}", live.label()),
            }
            println!(
                "Release:     aida session seat release --seat {seat} --generation {}",
                rec.generation
            );
        }
        _ => println!("Holder:      none"),
    }
    if let Some(r) = &rec.request {
        println!(
            "Takeover:    request {} by session {} until {}",
            r.id,
            r.requester_session,
            r.deadline.format("%Y-%m-%d %H:%M:%SZ")
        );
    }
    for item in in_flight(&rec) {
        println!("In flight:   {item}");
    }
    for d in &demoted {
        println!("Demoted:     {d}");
    }
    Ok(())
}

#[cfg(test)]
mod transfer_authority_tests {
    use super::*;

    // Exercise the production callback inside the real seat guard: creating the
    // context must not cache authority across a later local revocation/change.
    // trace:TASK-1607 | ai:codex
    #[test]
    fn transfer_context_reads_current_authority_and_binds_subject_session_seat() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = crate::test_env::AmbientGuard::hermetic(tmp.path(), None);
        let id = crate::seat_authority::test_support::mint_grant_for(
            tmp.path(),
            "requester",
            "orchestrator",
            &[],
        );
        let grant_path = crate::aida_home_dir()
            .unwrap()
            .join(".aida/session-grants")
            .join(format!("{id}.json"));
        let original = std::fs::read(&grant_path).unwrap();
        let grant: crate::seat_authority::SeatGrant = serde_json::from_slice(&original).unwrap();
        let now = chrono::Utc::now();
        let request = TakeoverRequest {
            id: uuid::Uuid::new_v4().to_string(),
            requester_session: grant.session_id.clone(),
            requester_grant_id: id,
            requester_subject: Some("requester".into()),
            requester_anchor: aida_core::process_probe::ProcessIdentity {
                pid: 1,
                start: "fixture-only-not-probed".into(),
            },
            requester_command: "fixture".into(),
            expected_holder_session: "holder".into(),
            expected_generation: 1,
            created_at: now,
            deadline: now + chrono::Duration::seconds(120),
        };
        let store = SeatStore::at(tmp.path().join(".aida/seat-occupancy"));
        let _guard = store.lock("orchestrator").unwrap();
        with_transfer_ctx(tmp.path(), |ctx| {
            let validate = ctx.requester_valid.unwrap();
            assert!(validate(&request, "orchestrator"));
            assert!(!validate(&request, "advisor"));
            let mut changed = request.clone();
            changed.requester_session = "other".into();
            assert!(!validate(&changed, "orchestrator"));
            changed = request.clone();
            changed.requester_subject = Some("other".into());
            assert!(!validate(&changed, "orchestrator"));
            changed.requester_subject = None;
            assert!(
                !validate(&changed, "orchestrator"),
                "legacy request cannot borrow authority"
            );
            for mutation in ["revoked", "expired", "missing", "corrupt"] {
                let mut invalid = grant.clone();
                match mutation {
                    "revoked" => invalid.revoked_at = Some(now),
                    "expired" => invalid.expires_at = now,
                    _ => {}
                }
                std::fs::write(&grant_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
                if mutation == "missing" {
                    std::fs::remove_file(&grant_path).unwrap();
                }
                if mutation == "corrupt" {
                    std::fs::write(&grant_path, b"broken").unwrap();
                }
                assert!(!validate(&request, "orchestrator"), "{mutation}");
                std::fs::write(&grant_path, &original).unwrap();
                assert!(validate(&request, "orchestrator"));
            }
            let roster = crate::detect_distributed_store_from(tmp.path())
                .unwrap()
                .join("registry/team.toml");
            let roster_bytes = std::fs::read(&roster).unwrap();
            for content in ["[members]\n", "malformed ["] {
                std::fs::write(&roster, content).unwrap();
                assert!(!validate(&request, "orchestrator"));
            }
            std::fs::remove_file(&roster).unwrap();
            assert!(!validate(&request, "orchestrator"));
            std::fs::write(&roster, roster_bytes).unwrap();
            assert!(validate(&request, "orchestrator"));
        });
    }
}
