//! `aida driver` — the operator/dev surface of the driver registry.
//!
//! A long-running loop that works specs on an operator's behalf (a relay, a
//! drain, a headless agent) registers itself so `aida ps` / `aida list` can
//! say WHO is driving an in-progress spec even when no session of the spec's
//! own is alive. Re-running `register` refreshes the heartbeat; `deregister`
//! removes the named record. Records are never reaped by readers: a dead pid
//! (or, with an explicit `--ttl`, a stale heartbeat) simply reads as not live.
//! Records live under the MAIN checkout, like session leases, so a driver
//! registered from a linked worktree is seen from every checkout.
// trace:STORY-1653 | ai:claude

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Subcommand;

use aida_core::driver_registry::{self, DriverKind, DriverRecord, Registration};

#[derive(Subcommand, Debug, Clone)]
pub enum DriverCommand {
    /// Register (or refresh the heartbeat of) a driver. The record stays live
    /// for as long as the registered pid is alive; it reads as not live once
    /// that process exits. Pass `--ttl` to also expire it when the heartbeat
    /// is not refreshed within that many seconds.
    Register {
        /// Driver kind: relay, drain or agent.
        #[clap(long, default_value = "relay")]
        kind: String,
        /// The driver's id (a relay run id, a drain pid, an agent session).
        #[clap(long)]
        run: String,
        /// Tag selector the driver works through, e.g. `batch:next`. In-progress
        /// specs carrying this tag resolve to this driver.
        #[clap(long)]
        tag: Option<String>,
        /// The driver's own long-lived process id. Pass `--pid "$$"` from the
        /// driver's loop: the parent of `aida` is a throwaway subshell when
        /// called from `$(...)` or a pipe. Required for relays; other kinds
        /// fall back to the parent process. The pid must be running unless
        /// `--ttl` is given.
        #[clap(long)]
        pid: Option<u32>,
        /// Seat the driver occupies, if any.
        #[clap(long)]
        seat: Option<String>,
        /// Path to the driver's handoff/notes file, if any.
        #[clap(long)]
        handoff: Option<String>,
        /// Heartbeat TTL in seconds. When set, the record also stops being
        /// live if `register` is not re-run within this window. Default: no
        /// heartbeat expiry (liveness follows the pid alone).
        #[clap(long)]
        ttl: Option<i64>,
        /// Machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
    /// Remove the driver record registered under this kind and id. There is
    /// no ownership check: any caller can remove any record by name.
    Deregister {
        /// Driver kind: relay, drain or agent.
        #[clap(long, default_value = "relay")]
        kind: String,
        /// The driver's id, as passed to `register --run`.
        #[clap(long)]
        run: String,
    },
    /// List registered drivers and whether each is live.
    List {
        /// Machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
}

fn parse_kind(kind: &str) -> Result<DriverKind> {
    DriverKind::parse(kind)
        .with_context(|| format!("unknown driver kind `{kind}`; valid kinds: relay, drain, agent"))
}

/// The MAIN checkout's root: driver records are shared across worktrees the
/// way session leases are, so register/list/deregister from a linked worktree
/// all address the same `.aida/drivers/`.
// trace:STORY-1653 | ai:claude
fn project_root() -> std::path::PathBuf {
    let start =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    crate::driver_view::driver_registry_root(&start)
}

fn record_json(r: &DriverRecord, live: bool) -> serde_json::Value {
    let mut v = serde_json::to_value(r).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("driver".into(), r.label().into());
        obj.insert("live".into(), live.into());
    }
    v
}

pub(crate) fn handle_driver_command(cmd: &DriverCommand) -> Result<()> {
    let root = project_root();
    let now = Utc::now();
    match cmd {
        DriverCommand::Register {
            kind,
            run,
            tag,
            pid,
            seat,
            handoff,
            ttl,
            json,
        } => {
            let kind = parse_kind(kind)?;
            // trace:STORY-1653 | ai:claude — a relay outlives every shell
            // that registers it, so the parent pid is never right for it.
            let pid = match (pid, kind) {
                (Some(p), _) => *p,
                (None, DriverKind::Relay) => anyhow::bail!(
                    "a relay must pass its own pid: `aida driver register --run {run} --pid \"$$\"`"
                ),
                (None, _) => parent_pid(),
            };
            if pid <= 1 || pid > i32::MAX as u32 {
                anyhow::bail!(
                    "pid {pid} is not a usable driver process id; pass the driver's own pid with `--pid \"$$\"`"
                );
            }
            let reg = Registration {
                kind,
                id: run.trim().to_string(),
                pid,
                selector: tag
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                seat: seat.clone(),
                handoff: handoff.clone(),
                ttl_secs: *ttl,
            };
            let start = aida_core::liveness::process_start_identity(pid);
            // trace:STORY-1653 | ai:claude — with no start identity and no
            // TTL the record has no guard against pid reuse, so it would
            // never read live; refuse instead of writing a dead record.
            if start.is_none() && ttl.filter(|t| *t > 0).is_none() {
                anyhow::bail!(
                    "pid {pid} is not running (or its start time cannot be read), so the record could never be trusted as live; pass the driver's own live pid with `--pid \"$$\"`, or add `--ttl <secs>` to register with heartbeat expiry instead"
                );
            }
            let rec = driver_registry::register(&root, reg, start, now)
                .with_context(|| format!("registering driver {}:{run}", kind.as_str()))?;
            let live = rec.is_live(now);
            if *json || crate::output_format_is_json() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&record_json(&rec, live))?
                );
            } else {
                println!(
                    "registered {} (pid {}{}){}",
                    rec.label(),
                    rec.pid,
                    rec.selector
                        .as_deref()
                        .map(|s| format!(", selector {s}"))
                        .unwrap_or_default(),
                    if live {
                        ""
                    } else {
                        " — warning: pid is not alive"
                    }
                );
            }
            Ok(())
        }
        DriverCommand::Deregister { kind, run } => {
            let kind = parse_kind(kind)?;
            let removed = driver_registry::deregister(&root, kind, run.trim())
                .with_context(|| format!("deregistering driver {}:{run}", kind.as_str()))?;
            if removed {
                println!("deregistered {}:{}", kind.as_str(), run.trim());
            } else {
                println!("no driver {}:{} was registered", kind.as_str(), run.trim());
            }
            Ok(())
        }
        DriverCommand::List { json } => {
            let all = driver_registry::read_all(&root);
            if *json || crate::output_format_is_json() {
                let rows: Vec<serde_json::Value> =
                    all.iter().map(|r| record_json(r, r.is_live(now))).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "drivers": rows }))?
                );
                return Ok(());
            }
            let headers = [
                "driver",
                "kind",
                "pid",
                "selector",
                "live",
                "heartbeat_age",
                "seat",
            ];
            let rows: Vec<Vec<String>> = all
                .iter()
                .map(|r| {
                    let age = now
                        .signed_duration_since(r.heartbeat_at)
                        .num_seconds()
                        .max(0);
                    vec![
                        r.label(),
                        r.kind.as_str().to_string(),
                        r.pid.to_string(),
                        r.selector.clone().unwrap_or_default(),
                        r.is_live(now).to_string(),
                        format!("{age}s"),
                        r.seat.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            if crate::agent_output_mode() {
                print!("{}", crate::toon::table_raw("drivers", &headers, &rows));
                return Ok(());
            }
            if rows.is_empty() {
                println!("No drivers registered.");
                return Ok(());
            }
            println!(
                "{:<28} {:>8}  {:<16} {:<5} {:>9}  seat",
                "DRIVER", "PID", "SELECTOR", "LIVE", "HEARTBEAT"
            );
            for r in &rows {
                println!(
                    "{:<28} {:>8}  {:<16} {:<5} {:>9}  {}",
                    r[0],
                    r[2],
                    r[3],
                    if r[4] == "true" { "yes" } else { "no" },
                    r[5],
                    r[6]
                );
            }
            Ok(())
        }
    }
}

/// The invoking shell's pid: `aida driver register` is normally run from the
/// driver's own loop, so its parent is the driver process.
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: getppid has no preconditions and cannot fail.
        unsafe { libc::getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        std::process::id()
    }
}
