// Shared fixed-service engine. Each inclusion has independent controller TLS.
// trace:STORY-2691 | ai:codex
use super::*;
use crate::advisor_cmd_root::AdvisorLoopsCommand;
use crate::gate::seat_authority::{self, SeatGrant};
use chrono::{DateTime, Utc};
use std::sync::atomic::Ordering;

const SERVICE: &str = "service.json";
const NAME: &str = if TRAIN { "auto-train" } else { "review-watch" };
// The admission_owner purpose this service's controller records.
// trace:BUG-2621 | ai:claude
const HOLDER_PURPOSE: &str = if TRAIN {
    "legacy auto-train"
} else {
    "legacy review-watch"
};
// Bound retained ancestry even if startup repeatedly fails behind a review
// whose descendant evidence needs operator recovery.
// trace:STORY-2571 | ai:codex
const MAX_RETIRED_GRANTS: usize = 4;

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    grant: String,
    subject: String,
    principal: String,
    issued_at: DateTime<Utc>,
    delegations: Vec<String>,
}

impl Binding {
    fn from_grant(grant: &SeatGrant) -> Self {
        Self {
            grant: grant.id.clone(),
            subject: grant.subject.clone(),
            principal: grant.principal.clone(),
            issued_at: grant.issued_at,
            delegations: grant.delegable_seats.clone(),
        }
    }
    fn validate(&self, root: &Path) -> Result<SeatGrant> {
        let grant = seat_authority::validated_grant(root, &self.grant, &self.subject, Utc::now())
            .context(
            "advisor loop is blocked-authority; rebind from the current advisor session",
        )?;
        anyhow::ensure!(
            grant.seat == "advisor"
                && grant.principal == self.principal
                && grant.issued_at == self.issued_at
                && grant.delegable_seats == self.delegations,
            "advisor loop binding no longer matches its grant"
        );
        anyhow::ensure!(
            grant.delegable_seats.iter().any(|s| s == "advisor")
                && grant.delegable_seats.iter().any(|s| s == "reviewer"),
            "advisor loop needs advisor and reviewer child delegation"
        );
        Ok(grant)
    }
    fn succeeds(&self, old: &Self) -> Result<()> {
        if self.grant == old.grant {
            return Ok(());
        }
        anyhow::ensure!(
            self.principal == old.principal && self.issued_at > old.issued_at,
            "stale advisor loop successor refused"
        );
        anyhow::ensure!(
            self.delegations.iter().all(|s| old.delegations.contains(s)),
            "advisor loop successor widens delegation; see the operator recovery procedure in docs/cli/06-roles-sessions.md#review-watch-service-recovery"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Service {
    version: u32,
    deputy: String,
    binding: Binding,
    /// None with child_grant present is an ambiguous spawn, never restartable.
    worker: Option<Identity>,
    child_grant: Option<String>,
    /// Confirmed exited controllers whose review descendants may still run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_grants: Vec<String>,
    host_active: bool,
    host: Option<Identity>,
    checked_at: Option<DateTime<Utc>>,
    retry_after: DateTime<Utc>,
    last_exit: Option<i32>,
    /// The live worker runs a superseded image; its exit is an orderly
    /// promotion handoff, replaced without failure backoff.
    // trace:BUG-2645 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    image_handoff: bool,
}

fn directory(root: &Path, create: bool) -> Result<Option<Directory>> {
    if create {
        let runtime = Directory::shared(&root.join(".aida"))?;
        return Directory::open(&runtime.path().join(NAME)).map(Some);
    }
    let Some(runtime) = Directory::existing(&root.join(".aida"))? else {
        return Ok(None);
    };
    let dir = Directory::existing(&runtime.path().join(NAME))?;
    if let Some(dir) = &dir {
        anyhow::ensure!(
            dir.handle.metadata()?.mode() & 0o022 == 0,
            "unsafe advisor loop service directory"
        );
    }
    Ok(dir)
}
fn read(dir: &Directory) -> Result<Option<Service>> {
    let Some(bytes) = dir.read(SERVICE)? else {
        anyhow::ensure!(dir.read("service-initialized")?.is_none(), "advisor loop service receipt is missing after initialization; follow docs/cli/06-roles-sessions.md#review-watch-service-recovery");
        return Ok(None);
    };
    let state: Service =
        serde_json::from_slice(&bytes).context("invalid advisor loop service receipt")?;
    anyhow::ensure!(
        state.version == 1 && !state.deputy.is_empty(),
        "unsupported advisor loop service receipt"
    );
    anyhow::ensure!(
        state.worker.is_none() || state.child_grant.is_some(),
        "incomplete advisor loop controller receipt"
    );
    Ok(Some(state))
}
fn write(dir: &Directory, state: &Service) -> Result<()> {
    // Validate the prior opened file before replacing it.
    read(dir)?;
    dir.write(SERVICE, &serde_json::to_vec_pretty(state)?)?;
    if dir.read("service-initialized")?.is_none() {
        dir.file("service-initialized", true)?.sync_all()?;
    }
    dir.handle.sync_all()?;
    Ok(())
}

pub(crate) fn rebind(root: &Path, deputy: &str, grant: &SeatGrant) -> Result<()> {
    let binding = Binding::from_grant(grant);
    binding.validate(root)?;
    let dir = directory(root, true)?.context("missing advisor loop directory")?;
    let _lock = dir.admission_lock(&format!("{NAME} rebinding controller"))?;
    // Revalidate at the locked acceptance boundary, including the full parent chain.
    binding.validate(root)?;
    let state = match read(&dir)? {
        Some(mut state) => {
            anyhow::ensure!(
                state.deputy == deputy,
                "advisor loop belongs to another deputy"
            );
            binding.succeeds(&state.binding)?;
            // Operator roots may hand off to their first advisor child. Once
            // bound to a session, its descendants are sub-advisors, not rotations.
            // trace:STORY-2571 | ai:codex
            if seat_authority::read_grant(&state.binding.grant)?
                .parent_grant_id
                .is_some()
            {
                let mut ancestor = grant.parent_grant_id.clone();
                while let Some(id) = ancestor {
                    anyhow::ensure!(
                        id != state.binding.grant,
                        "advisor loop sub-advisor cannot replace its ancestor binding"
                    );
                    ancestor = seat_authority::read_grant(&id)?.parent_grant_id;
                }
            }
            state.binding = binding;
            state
        }
        None => Service {
            version: 1,
            deputy: deputy.into(),
            binding,
            worker: None,
            child_grant: None,
            retired_grants: Vec::new(),
            host_active: true,
            host: None,
            checked_at: None,
            retry_after: Utc::now(),
            last_exit: None,
            image_handoff: false,
        },
    };
    write(&dir, &state)
}

pub(crate) fn command(action: &AdvisorLoopsCommand) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    match action {
        AdvisorLoopsCommand::Status { json } | AdvisorLoopsCommand::TrainStatus { json } => {
            let Some(dir) = directory(&root, false)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let _lock = dir.admission_lock(&format!("{NAME} reading status"))?;
            let Some(state) = read(&dir)? else {
                print_status("not-configured", None, &[], *json)?;
                return Ok(());
            };
            let started = state.worker.as_ref().and_then(|w| w.recorded_at);
            let (report, degraded) = pass_health(&dir, started, Utc::now());
            let status = if let Some(status) = host_refusal(&state) {
                status
            } else if state.binding.validate(&root).is_err() {
                "blocked-authority"
            } else if state.child_grant.is_some() && state.worker.is_none() {
                "blocked-ambiguous-spawn"
            } else if state.worker.as_ref().is_some_and(Identity::alive) {
                "running"
            } else {
                "waiting-for-supervisor"
            };
            let status = pass_status(status, &degraded);
            let mut blockers = review_blockers(&dir)?;
            blockers.extend(retry_blockers(&root, &dir)?);
            let degraded = if status == "degraded" {
                degraded
            } else {
                Vec::new()
            };
            print_status_with_pass(
                status,
                Some(&state),
                &blockers,
                report.as_ref(),
                &degraded,
                *json,
            )?;
            Ok(())
        }
        AdvisorLoopsCommand::Rebind { deputy } => {
            let grant = seat_authority::grant_check(&root)
                .into_valid()
                .context("rebind requires a live advisor seat grant")?;
            let queue = crate::deputy_shell::current_queue(deputy)?;
            let reply = crate::deputy_shell::socket::request(
                &queue,
                &crate::deputy_shell::socket::Request::ReviewWatchRebind {
                    project: root,
                    grant: grant.id,
                },
            )?;
            anyhow::ensure!(
                reply.ok,
                "review-watch rebind refused: {}",
                reply.why.unwrap_or_default()
            );
            println!("advisor loops: bindings accepted; supervisors will reconcile prior controllers before restart");
            Ok(())
        }
        // trace:BUG-2703 | ai:claude
        AdvisorLoopsCommand::Drain { reason, duration } => {
            println!(
                "{}",
                crate::admission_drain::drain(&root, reason, duration.as_deref())?
            );
            Ok(())
        }
        AdvisorLoopsCommand::Resume => {
            println!("{}", crate::admission_drain::resume(&root)?);
            Ok(())
        }
        // trace:TASK-3924 | ai:claude
        AdvisorLoopsCommand::ReleaseWaiter { id } => {
            crate::admission_owner::release_waiter(&root, id)
        }
        AdvisorLoopsCommand::Worker | AdvisorLoopsCommand::TrainWorker => worker(&root),
    }
}

/// This service's admission claim, the active drain and any waiting land,
/// ship or train, for status. Unreadable evidence is reported, not hidden.
// trace:BUG-2703 | ai:claude
fn admission_status(root: &Path, state: Option<&Service>, now: DateTime<Utc>) -> serde_json::Value {
    let worker = state
        .and_then(|s| s.worker.as_ref())
        .filter(|w| w.alive())
        .map(|w| w.pid);
    // The other loop's controller, from its own service receipt (read-only).
    // trace:BUG-2703 | ai:claude
    let other = if TRAIN {
        crate::review_watch::service::live_worker_pid(root)
    } else {
        crate::review_watch::train_service::live_worker_pid(root)
    };
    let (claim, claims, waiting) = match crate::admission_owner::holders(root) {
        Ok(rows) => {
            let own = claim_view(&rows, HOLDER_PURPOSE, worker, now);
            let other_purpose = if TRAIN {
                "legacy review-watch"
            } else {
                "legacy auto-train"
            };
            let other = match &other {
                Ok(pid) => claim_view(&rows, other_purpose, *pid, now),
                Err(error) => serde_json::json!({ "error": format!("{error:#}") }),
            };
            let (review_watch, auto_train) = if TRAIN {
                (other, own.clone())
            } else {
                (own.clone(), other)
            };
            let waiting: Vec<_> = rows
                .iter()
                .filter(|r| r.state == "waiting")
                .map(|r| format!("PID {} ({}) since {}", r.pid, r.purpose, r.since.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default()))
                .collect();
            (
                own,
                serde_json::json!({ "review-watch": review_watch, "auto-train": auto_train }),
                serde_json::json!(waiting),
            )
        }
        Err(error) => {
            let error = serde_json::json!({ "error": format!("{error:#}") });
            (
                error.clone(),
                serde_json::json!({ "review-watch": error.clone(), "auto-train": error }),
                serde_json::Value::Null,
            )
        }
    };
    let drain = match crate::admission_drain::active(root) {
        Ok(Some(drained)) => serde_json::json!(drained),
        Ok(None) => serde_json::Value::Null,
        Err(error) => serde_json::json!({ "error": format!("{error:#}") }),
    };
    serde_json::json!({
        "admission_claim": claim,
        "admission_claims": claims,
        "drain": drain,
        "admission_waiting": waiting,
    })
}

/// One loop's live claim for status: the claim its recorded controller
/// holds, else any live claim for its purpose (a controller the service
/// receipt no longer names still holds admission). A receipt written before
/// claims carried a time (legacy) shows an unknown age, never zero.
// trace:BUG-2703 | ai:claude
fn claim_view(
    rows: &[crate::admission_owner::HolderView],
    purpose: &str,
    worker: Option<u32>,
    now: DateTime<Utc>,
) -> serde_json::Value {
    let mine = |r: &&crate::admission_owner::HolderView| r.state == "claim" && r.purpose == purpose;
    rows.iter()
        .filter(mine)
        .find(|r| Some(r.pid) == worker)
        .or_else(|| rows.iter().find(mine))
        .map(|r| {
            serde_json::json!({
                "pid": r.pid,
                "controller": worker,
                "acquired_at": r.since,
                "age_seconds": r.since.map(|at| (now - at).num_seconds().max(0)),
            })
        })
        .unwrap_or(serde_json::Value::Null)
}

/// This service's live controller PID, from its receipt; read-only and
/// lock-free (the receipt is replaced atomically), so another service's
/// status can show it without taking this service's lock.
// trace:BUG-2703 | ai:claude
pub(crate) fn live_worker_pid(root: &Path) -> Result<Option<u32>> {
    let Some(dir) = directory(root, false)? else {
        return Ok(None);
    };
    Ok(read(&dir)?
        .and_then(|s| s.worker)
        .filter(Identity::alive)
        .map(|w| w.pid))
}

// trace:BUG-2703 | ai:claude
fn claim_phrase(claim: &serde_json::Value) -> String {
    if claim.is_null() {
        "not held".to_string()
    } else if let Some(error) = claim["error"].as_str() {
        format!("unreadable ({error})")
    } else {
        match claim["age_seconds"].as_i64() {
            Some(age) => format!("held by pid {} for {}m{:02}s", claim["pid"], age / 60, age % 60),
            None => format!("held by pid {} (age unknown)", claim["pid"]),
        }
    }
}

// trace:BUG-2703 | ai:claude
fn admission_line(value: &serde_json::Value) -> String {
    // Both workers' claims, whichever service's status this is.
    let claims = &value["admission_claims"];
    let claim = format!(
        "review-watch {}; auto-train {}",
        claim_phrase(&claims["review-watch"]),
        claim_phrase(&claims["auto-train"])
    );
    let drain = &value["drain"];
    let drain = if drain.is_null() {
        "none".to_string()
    } else if let Some(error) = drain["error"].as_str() {
        format!("unreadable ({error}); loops treat it as drained")
    } else {
        format!(
            "by {} until {}: {}",
            drain["actor"].as_str().unwrap_or_default(),
            drain["expires_at"].as_str().unwrap_or_default(),
            drain["reason"].as_str().unwrap_or_default()
        )
    };
    let waiting = value["admission_waiting"]
        .as_array()
        .filter(|w| !w.is_empty())
        .map(|w| {
            w.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_else(|| "none".into());
    format!("admission: claim {claim}; drain {drain}; waiting {waiting}")
}

// trace:STORY-2571 | ai:codex
fn print_status(
    status: &str,
    state: Option<&Service>,
    blockers: &[String],
    json: bool,
) -> Result<()> {
    print_status_with_pass(status, state, blockers, None, &[], json)
}

// trace:STORY-2571 | ai:codex
// trace:BUG-2674 | ai:claude
fn print_status_with_pass(
    status: &str,
    state: Option<&Service>,
    blockers: &[String],
    pass: Option<&PassReport>,
    degraded: &[String],
    json: bool,
) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    let value = status_value(&root, status, state, blockers, pass, degraded)?;
    if json || crate::output_format_is_json() {
        println!("{}", crate::cache_output::json_pretty(&value)?);
    } else if matches!(
        crate::output_format_override(),
        Some(crate::OutputFormat::Toon)
    ) {
        for (key, value) in value.as_object().unwrap() {
            println!(
                "{}",
                crate::toon::scalar(
                    key,
                    &value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                )
            );
        }
    } else {
        for line in status_lines(status, &value, pass, degraded) {
            println!("{line}");
        }
    }
    Ok(())
}

/// The status projection both `loops status` and `train-status` print, as
/// JSON. Separate from printing so the command's projection is testable.
// trace:BUG-2703 | ai:claude
fn status_value(
    root: &Path,
    status: &str,
    state: Option<&Service>,
    blockers: &[String],
    pass: Option<&PassReport>,
    degraded: &[String],
) -> Result<serde_json::Value> {
    let failure = failure_state(root)?;
    let live = crate::dev_build::default_live_dir()?.join("aida");
    let binary = binary_status(state, &live, crate::dev_build::binary_sha_label);
    let mut value = serde_json::json!({
        "service": NAME, "status": status,
        "worker_binary_sha": binary["worker_binary_sha"],
        "live_binary_sha": binary["live_binary_sha"],
        "binary_stale": binary["binary_stale"],
        "consecutive_failures": failure.count, "alert": failure.alert,
        "deputy": state.map(|s| &s.deputy),
        "binding_issued_at": state.map(|s| s.binding.issued_at),
        "controller": state.and_then(|s| s.worker.as_ref().map(|w| w.pid)),
        "last_exit": state.and_then(|s| s.last_exit),
        "retired_controllers": state.map(|s| s.retired_grants.len()).unwrap_or(0),
        "review_blockers": blockers,
        "last_pass": pass,
        "degraded_reasons": degraded,
        "recovery": "docs/cli/06-roles-sessions.md#review-watch-service-recovery",
        "deputy_binary": host_deputy_binary(state, &live), // trace:BUG-2774 | ai:claude
    });
    // The train service says when its passes do nothing: review-gated mode,
    // or a landing policy it cannot read (STORY-2905, advisor call (c)).
    // The review-watch service's display is unchanged. trace:STORY-2905 | ai:claude
    if TRAIN {
        if let Some(line) = crate::gate::landing_merge::train_stand_down_line(root) {
            value["standing_down"] = serde_json::json!(line);
        }
        // A long auto-train hold, live, with its phase. trace:BUG-2769 | ai:claude
        value["admission_hold"] = serde_json::json!(super::train::live_hold_line(root));
        // A manual train the auto-train yields its next train to. trace:BUG-2771 | ai:claude
        // Every queued waiter, first in line first. trace:TASK-3924 | ai:claude
        value["manual_waiters"] = serde_json::json!(crate::admission_owner::waiter_lines(root));
        // A recovery parked on its train's CI. trace:TASK-3920 | ai:claude
        value["parked_recovery"] = serde_json::json!(crate::pr_train::parked_line(root));
    }
    // trace:BUG-2703 | ai:claude
    let admission = admission_status(root, state, Utc::now());
    for key in ["admission_claim", "admission_claims", "drain", "admission_waiting"] {
        value[key] = admission[key].clone();
    }
    Ok(value)
}

/// The human status lines for a [`status_value`] projection.
// trace:BUG-2703 | ai:claude
fn status_lines(
    status: &str,
    value: &serde_json::Value,
    pass: Option<&PassReport>,
    degraded: &[String],
) -> Vec<String> {
    let mut lines = vec![binary_line(value)];
    lines.push(format!("{NAME}: {status}; deputy {}; binding issued {}; controller {}; last exit {}; retired controllers {}; review blockers {}; recovery {}; consecutive failures {}; alert {}",
        value["deputy"], value["binding_issued_at"], value["controller"], value["last_exit"], value["retired_controllers"], value["review_blockers"], value["recovery"], value["consecutive_failures"], value["alert"]));
    if value["deputy_binary"].is_object() {
        lines.push(format!("{NAME}: {}", deputy_binary_line(&value["deputy_binary"]))); // trace:BUG-2774 | ai:claude
    }
    if let Some(line) = value["standing_down"].as_str() {
        lines.push(format!("{NAME}: {line}"));
    }
    lines.push(format!("{NAME}: {}", admission_line(value))); // trace:BUG-2703 | ai:claude
    if let Some(line) = value["admission_hold"].as_str() {
        lines.push(line.to_string()); // trace:BUG-2769 | ai:claude
    }
    if let Some(line) = value["parked_recovery"].as_str() {
        lines.push(format!("{NAME}: {line}")); // trace:TASK-3920 | ai:claude
    }
    for line in value["manual_waiters"].as_array().into_iter().flatten() {
        lines.push(format!("{NAME}: {}", line.as_str().unwrap_or_default())); // trace:BUG-2771 TASK-3924 | ai:claude
    }
    // trace:BUG-2674 | ai:claude
    if let Some(pass) = pass {
        lines.push(format!(
            "last pass: {}; {} open change(s); {} dispatched; {} not admitted; {} busy; {} refused{}",
            pass.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            pass.changes,
            pass.dispatched,
            pass.unadmitted,
            pass.busy,
            pass.refused,
            skipped_summary(pass)
        ));
    }
    for reason in degraded {
        lines.push(format!("degraded: {reason}"));
    }
    lines
}

// Read the kernel-held worker image, not the path that may have been
// replaced. A missing live image or worker reports null, never "current".
// trace:BUG-2645 | ai:codex
fn binary_status(
    state: Option<&Service>,
    live: &Path,
    sha: impl Fn(&Path) -> Option<String>,
) -> serde_json::Value {
    let worker_image = state
        .and_then(|s| s.worker.as_ref())
        .filter(|w| w.alive())
        .map(|w| std::path::PathBuf::from(format!("/proc/{}/exe", w.pid)));
    let stale = worker_image
        .as_deref()
        .and_then(|image| crate::launch::control::service_image_is_live(image, live).ok())
        .map(|same| !same);
    serde_json::json!({
        "worker_binary_sha": worker_image.as_deref().and_then(&sha),
        "live_binary_sha": sha(live),
        "binary_stale": stale,
    })
}

// trace:BUG-2645 | ai:claude
fn binary_line(value: &serde_json::Value) -> String {
    format!(
        "binary: worker {}; live {}; stale {}",
        value["worker_binary_sha"], value["live_binary_sha"], value["binary_stale"]
    )
}

/// The doctor category for a service's deputy on an old binary.
// trace:BUG-2774 | ai:claude
pub(crate) const DEPUTY_BINARY_CATEGORY: &str = "deputy-binary";

/// The hosting deputy's runner image, `exe` (its `/proc/<pid>/exe`), when it
/// is not the live build: removed from disk (the kernel's ` (deleted)`
/// marker) or replaced by a promotion (another inode with another sha). A
/// deputy on an old binary cannot run current jobs, so the service blocks
/// behind it. An unreadable image or live build reports nothing.
// trace:BUG-2774 | ai:claude
pub(crate) fn deputy_binary(
    deputy: &str,
    pid: u32,
    exe: &Path,
    live: &Path,
    sha: impl Fn(&Path) -> Option<String>,
    now: std::time::SystemTime,
) -> Option<serde_json::Value> {
    let path = std::fs::read_link(exe).ok()?.to_string_lossy().into_owned();
    let deleted = path.ends_with(" (deleted)");
    let replaced = || {
        crate::launch::control::service_image_is_live(exe, live).is_ok_and(|same| !same)
            && sha(exe).is_some_and(|image| sha(live).is_some_and(|live| live != image))
    };
    if !deleted && !replaced() {
        return None;
    }
    let age = std::fs::metadata(exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|built| now.duration_since(built).ok())
        .map(|age| age.as_secs());
    Some(serde_json::json!({
        "deputy": deputy, "pid": pid, "path": path, "deleted": deleted,
        "binary_age_seconds": age,
    }))
}

/// The probe for this service's recorded live deputy host.
// trace:BUG-2774 | ai:claude
fn host_deputy_binary(state: Option<&Service>, live: &Path) -> Option<serde_json::Value> {
    host_deputy_binary_at(state, live, proc_exe)
}

fn proc_exe(pid: u32) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/exe"))
}

/// [`host_deputy_binary`] with the host's image link supplied by `exe`.
// trace:TASK-3936 | ai:claude
fn host_deputy_binary_at(
    state: Option<&Service>,
    live: &Path,
    exe: impl Fn(u32) -> PathBuf,
) -> Option<serde_json::Value> {
    let state = state?;
    let host = state.host.as_ref().filter(|h| h.alive())?;
    deputy_binary(
        &state.deputy,
        host.pid,
        &exe(host.pid),
        live,
        crate::dev_build::binary_sha_label,
        std::time::SystemTime::now(),
    )
}

/// The report line for a [`deputy_binary`] finding, with the fix.
// trace:BUG-2774 | ai:claude
fn deputy_binary_line(value: &serde_json::Value) -> String {
    let age = value["binary_age_seconds"]
        .as_u64()
        .map_or_else(
            || "age unknown".to_string(),
            |s| format!("{}h{}m old", s / 3600, s % 3600 / 60),
        );
    format!(
        "deputy {} (pid {}) runs a {} binary {} ({age}); it cannot run current jobs, so {NAME} blocks behind it. Fix: {}",
        value["deputy"].as_str().unwrap_or("?"),
        value["pid"],
        if value["deleted"] == true { "deleted" } else { "stale" },
        value["path"].as_str().unwrap_or("?"),
        deputy_binary_fix(value["deputy"].as_str().unwrap_or("NAME")),
    )
}

fn deputy_binary_fix(deputy: &str) -> String {
    format!("restart the deputy on the live build with the binding reset (docs/cli/06-roles-sessions.md#review-watch-service-recovery), then `aida advisor loops rebind --deputy {deputy}`")
}

/// `aida doctor`'s finding for this service's deputy on an old binary.
// trace:BUG-2774 | ai:claude
pub(crate) fn deputy_binary_finding(root: &Path) -> Option<crate::DoctorFinding> {
    deputy_binary_finding_at(root, proc_exe)
}

/// [`deputy_binary_finding`] with the host's image link supplied by `exe`.
// trace:TASK-3936 | ai:claude
pub(crate) fn deputy_binary_finding_at(
    root: &Path,
    exe: impl Fn(u32) -> PathBuf,
) -> Option<crate::DoctorFinding> {
    let state = read(&directory(root, false).ok()??).ok()??;
    let live = crate::dev_build::default_live_dir().ok()?.join("aida");
    let value = host_deputy_binary_at(Some(&state), &live, exe)?;
    Some(crate::DoctorFinding {
        category: DEPUTY_BINARY_CATEGORY.into(),
        id: format!("{}:{}", state.deputy, value["pid"]),
        summary: deputy_binary_line(&value),
        action: deputy_binary_fix(&state.deputy),
        safe_heal: false,
    })
}

/// Test fixture: a service receipt whose live deputy host is `pid`.
// trace:TASK-3936 | ai:claude
#[cfg(test)]
pub(crate) fn write_test_host_receipt(root: &Path, deputy: &str, pid: u32) {
    let binding = Binding {
        grant: "fixture-grant".into(),
        subject: "fixture-subject".into(),
        principal: "fixture-principal".into(),
        issued_at: Utc::now(),
        delegations: Vec::new(),
    };
    let state = Service {
        version: 1,
        deputy: deputy.into(),
        binding,
        worker: None,
        child_grant: None,
        retired_grants: Vec::new(),
        host_active: true,
        host: Some(Identity::of(pid).unwrap()),
        checked_at: None,
        retry_after: Utc::now(),
        last_exit: None,
        image_handoff: false,
    };
    write(&directory(root, true).unwrap().unwrap(), &state).unwrap();
}

/// Test fixture: a stand-in for `/proc/<pid>/exe` of a deputy whose image a
/// promotion removed — a link whose target carries the kernel's ` (deleted)`
/// marker. Returns the link and the removed image path. No process or /proc
/// layout is involved, so it is host independent.
// trace:TASK-3936 | ai:claude
#[cfg(test)]
pub(crate) fn deleted_image_link(dir: &Path) -> (PathBuf, PathBuf) {
    let image = dir.join("deputy-image").join("aida");
    let link = dir.join("deputy-exe");
    std::os::unix::fs::symlink(format!("{} (deleted)", image.display()), &link).unwrap();
    (link, image)
}

// A promotion is an orderly retirement, not a failed dispatch episode.
// trace:BUG-2645 | ai:codex
#[derive(Debug)]
pub(super) struct ImageSuperseded;
impl std::fmt::Display for ImageSuperseded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("advisor loop binary superseded by live promotion")
    }
}
impl std::error::Error for ImageSuperseded {}

thread_local! { static LIVE_IMAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

thread_local! { static CONTROLLER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

/// Ordinary review-watch invocations keep their existing behavior. The fixed
/// service worker checks binding and authority at every pass and PR admission.
pub(super) fn check_controller(root: &Path) -> Result<()> {
    // A regression running the real worker loops in threads ends them here,
    // where a withdrawn controller would end. trace:BUG-2703 | ai:claude
    #[cfg(test)]
    if crate::review_watch::test_loop_stopped(root) {
        anyhow::bail!("test fixture stopped the loop");
    }
    if !CONTROLLER.with(|id| id.borrow().is_some()) {
        return Ok(());
    }
    let dir = directory(root, false)?.context("advisor loop service state disappeared")?;
    let _lock = dir.admission_lock(&format!("{NAME} checking controller"))?;
    check_controller_locked(root, &dir)
}

// Service controllers validate delegation before per-PR work. Ordinary
// watchers may still finish superseding obsolete heads after authority narrows.
// trace:STORY-2571 | ai:codex
pub(super) fn validate_controller_delegation(root: &Path) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        seat_authority::validate_child_delegation(root, "reviewer")?;
    }
    Ok(())
}

// Called while ordinary review admission already holds admission.lock. Rebind
// and reservation creation thus compare the generation at the same boundary.
// trace:STORY-2571 | ai:codex
pub(super) fn check_controller_locked(root: &Path, dir: &Directory) -> Result<()> {
    CONTROLLER
        .with(|id| {
            let id = id.borrow();
            let Some(id) = id.as_ref() else {
                return Ok(());
            };
            // trace:BUG-2645 | ai:codex
            let mut state = read(dir)?.context("advisor loop service state disappeared")?;
            // Only a confirmed image mismatch retires the worker. An installation
            // without a live image can run the current executable.
            if LIVE_IMAGE.with(|enabled| enabled.get())
                && crate::dev_build::default_live_dir().is_ok_and(|live| {
                    crate::launch::control::service_image_is_live(
                        Path::new("/proc/self/exe"),
                        &live.join("aida"),
                    )
                    .is_ok_and(|same| !same)
                })
            {
                // Only the recorded worker may classify its exit for the reaper.
                anyhow::ensure!(
                    state.child_grant.as_ref() == Some(id)
                        && state.worker.as_ref() == Some(&Identity::current()?),
                    "advisor loop controller receipt superseded"
                );
                if !state.image_handoff {
                    state.image_handoff = true;
                    write(dir, &state)?;
                }
                return Err(ImageSuperseded.into());
            }
            anyhow::ensure!(
                host_fresh(&state),
                "advisor loop deputy authority ended or its heartbeat is stale"
            );
            let child = seat_authority::grant_check(root)
                .into_valid()
                .context("advisor loop controller authority ended")?;
            anyhow::ensure!(child.id == *id, "advisor loop controller grant changed");
            check_binding(root, &state, &child)?;
            Ok(())
        })
        .context(GlobalState)
}

// trace:STORY-2571 | ai:codex
fn check_binding(root: &Path, state: &Service, child: &SeatGrant) -> Result<()> {
    anyhow::ensure!(
        state.child_grant.as_ref() == Some(&child.id)
            && child.parent_grant_id.as_ref() == Some(&state.binding.grant),
        "advisor loop controller binding superseded"
    );
    state.binding.validate(root)?;
    Ok(())
}

fn worker(root: &Path) -> Result<()> {
    let child = seat_authority::grant_check(root)
        .into_valid()
        .context("advisor loop worker needs its child grant")?;
    let own = Identity::current()?;
    // Parent writes the verified child receipt before any discovery/admission.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let dir = directory(root, false)?.context("advisor loop service state missing")?;
        let ready = {
            let _lock = dir.admission_lock(&format!("{NAME} confirming worker"))?;
            let state = read(&dir)?.context("advisor loop service state missing")?;
            anyhow::ensure!(
                state.child_grant.as_ref() == Some(&child.id),
                "unexpected advisor loop worker grant"
            );
            state.worker.as_ref() == Some(&own)
        };
        if ready {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "advisor loop controller receipt not confirmed"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    LIVE_IMAGE.with(|enabled| enabled.set(true));
    CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id));
    let result = if TRAIN {
        super::train::run(root)
    } else {
        run_with_program(&worker_review_watch_args(), root, None).map(|_| ())
    };
    LIVE_IMAGE.with(|enabled| enabled.set(false));
    CONTROLLER.with(|id| *id.borrow_mut() = None);
    match result {
        Err(error) if error.is::<ImageSuperseded>() => Ok(()),
        result => result,
    }
}

// Let the watcher resolve project configuration, including its default of two.
// trace:BUG-2644 | ai:codex
fn worker_review_watch_args() -> ReviewWatchArgs {
    ReviewWatchArgs {
        control: None,
        once: false,
        interval: 10,
        max_concurrent: None,
        vendor: None,
        release: None,
        launch_reservation: None,
    }
}

/// The operator-started deputy owns this thread. Controllers always execute
/// the promoted live AIDA image's fixed worker verb.
// trace:STORY-2571 | ai:codex
pub(crate) struct Supervisor {
    host: HostAuthority,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Supervisor {
    pub(crate) fn start(queue: crate::deputy_shell::Queue, host: HostAuthority) -> Self {
        let guard = host.clone();
        let deputy = queue.name.clone();
        let thread = std::thread::spawn(move || {
            let mut child: Option<Child> = None;
            let mut reported = None;
            let mut skipped = None;
            let mut recovery_at = std::time::Instant::now();
            while guard.active.load(Ordering::SeqCst) {
                // Closed-change recovery must run even while the retirement cap
                // prevents spawning the watcher that normally prunes entries.
                // trace:BUG-2640 | ai:codex
                let recovery = if !TRAIN && std::time::Instant::now() >= recovery_at {
                    recovery_at = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    recover_closed_reviews(&guard.project)
                } else {
                    Ok(())
                };
                // A refused forge observation must not suppress heartbeat or
                // child reaping; neither depends on discovery succeeding.
                let outcome = tick(
                    &guard.project,
                    &deputy,
                    guard.validate().is_ok() && !queue.paused(),
                    &mut child,
                );
                settle(
                    &guard.project,
                    outcome,
                    recovery,
                    &mut skipped,
                    &mut reported,
                );
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let _ = tick(&guard.project, &deputy, false, &mut child);
            // Dropping a child handle never proves exit or clears its receipt.
        });
        Self {
            host,
            thread: Some(thread),
        }
    }
}
// Account one supervisor pass. A verified same-service holder is a quiet
// skip: one line per episode, no failure count, no alert. Every refusal,
// including unverified contention, is counted and alerts as before.
// trace:BUG-2621 | ai:claude
fn settle(
    root: &Path,
    outcome: Result<Option<u32>>,
    recovery: Result<()>,
    skipped: &mut Option<u32>,
    reported: &mut Option<String>,
) {
    let outcome = outcome.map(|holder| {
        if let Some(pid) = holder.filter(|_| *skipped != holder) {
            eprintln!("skipped: {NAME} tick already running (pid {pid})");
        }
        *skipped = holder;
    });
    if let Err(error) = outcome.and(recovery) {
        *skipped = None;
        let reason = format!("{error:#}");
        if let Err(alert_error) = record_failure(root, Some(&reason)) {
            eprintln!("{NAME}: could not persist/deliver service alert: {alert_error:#}");
        }
        if reported.as_ref() != Some(&reason) {
            eprintln!("{NAME} supervisor refused: {reason}");
            *reported = Some(reason);
        }
    } else {
        *reported = None;
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.host.active.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// A dead, unknown, paused or stalled deputy cannot lend remembered authority
// to a surviving controller. The heartbeat carries no independent authority.
// trace:STORY-2571 | ai:codex
fn host_refusal(state: &Service) -> Option<&'static str> {
    if !state.host_active {
        return Some("blocked-deputy");
    }
    match state.host.as_ref().map(Identity::verdict) {
        Some(aida_core::proc_identity::Verdict::Alive) => {
            if state.checked_at.is_some_and(|at| {
                at <= Utc::now() && Utc::now() - at < chrono::Duration::seconds(5)
            }) {
                None
            } else {
                Some("blocked-deputy")
            }
        }
        Some(aida_core::proc_identity::Verdict::Dead(_)) => Some("blocked-deputy"),
        _ => Some("unknown"),
    }
}

fn host_fresh(state: &Service) -> bool {
    host_refusal(state).is_none()
}

/// `Ok(Some(pid))` means a verified live tick of this same service holds
/// admission.lock and this tick skipped without touching service state.
fn tick(
    root: &Path,
    deputy: &str,
    host_valid: bool,
    child: &mut Option<Child>,
) -> Result<Option<u32>> {
    // Obsolete-image detection and handoff run under the admission lock in
    // tick_with_launcher, so production and fixture launchers share them.
    // trace:BUG-2645 | ai:codex
    tick_with_launcher(root, deputy, host_valid, child, |root, parent, reuse| {
        if let Some(grant) = reuse {
            return Ok((
                grant.clone(),
                crate::launch::review_watch_controller_command(grant)?,
            ));
        }
        crate::launch::review_watch_controller(root, &parent.id, &parent.subject)
    })
}

// A missing or unreadable live image never retires a controller.
// trace:BUG-2645 | ai:claude
fn worker_image_obsolete(worker: &Identity) -> bool {
    let Some(live) = live_image() else {
        return false;
    };
    crate::launch::control::service_image_is_live(
        Path::new(&format!("/proc/{}/exe", worker.pid)),
        &live,
    )
    .is_ok_and(|same| !same)
}

#[cfg(not(test))]
fn live_image() -> Option<PathBuf> {
    crate::dev_build::default_live_dir()
        .ok()
        .map(|dir| dir.join("aida"))
}

// Tests opt in per thread, so a developer's or runner's installed live build
// can never retire fixture controllers.
// trace:BUG-2645 | ai:claude
#[cfg(test)]
thread_local! { static TEST_LIVE_IMAGE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
fn live_image() -> Option<PathBuf> {
    TEST_LIVE_IMAGE.with(|image| image.borrow().clone())
}

/// Idle between passes in one-second slices, so a supervised controller sees
/// a withdrawn heartbeat (promotion, rotation, stop) within one supervision
/// cycle. Ordinary watchers only sleep: check_controller is a no-op for them.
// trace:BUG-2645 | ai:claude
pub(super) fn idle(root: &Path, seconds: u64) -> Result<()> {
    for _ in 0..seconds {
        std::thread::sleep(std::time::Duration::from_secs(1));
        // A briefly busy admission lock is not a withdrawal; the next slice
        // or the pass's own check enforces it.
        match check_controller(root) {
            Err(error) if !is_busy(&error) => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

// Keep the supervision/spawn boundary testable without running a CLI worker
// inside the libtest image. Production always uses the fixed launcher above.
// trace:STORY-2571 | ai:codex
fn tick_with_launcher(
    root: &Path,
    deputy: &str,
    host_valid: bool,
    child: &mut Option<Child>,
    launch: impl FnOnce(&Path, &SeatGrant, Option<&SeatGrant>) -> Result<(SeatGrant, Command)>,
) -> Result<Option<u32>> {
    let Some(dir) = directory(root, false)? else {
        return Ok(None);
    };
    let _lock = match dir.admission_lock(&format!("{NAME} supervising worker")) {
        Ok(lock) => lock,
        Err(error) => return contended(root, error),
    };
    let Some(mut state) = read(&dir)? else {
        return Ok(None);
    };
    if state.deputy != deputy {
        return Ok(None);
    }
    let before = serde_json::to_vec(&state)?;
    let exited = if let Some(process) = child.as_mut() {
        // Capture the exit before dropping the reaped handle. Retain the
        // persisted identity and grant while reviews may still descend from it.
        if let Some(status) = process.try_wait()? {
            state.last_exit = status.code();
            *child = None;
            true
        } else {
            false
        }
    } else {
        state.worker.as_ref().is_some_and(|worker| {
            matches!(worker.verdict(), aida_core::proc_identity::Verdict::Dead(_))
        })
    };
    // A live worker on a superseded image loses the heartbeat, so it yields
    // at its next idle check or admission boundary (an active train or review
    // dispatch first finishes). Never signal it or its review descendants.
    // Its exit is then replaced in the reaping tick, without failure backoff.
    // trace:BUG-2645 | ai:claude
    let obsolete = !exited
        && state
            .worker
            .as_ref()
            .filter(|worker| worker.alive())
            .is_some_and(worker_image_obsolete);
    if obsolete {
        state.image_handoff = true;
    }
    let host_valid = host_valid && !obsolete;
    state.host_active = host_valid;
    if host_valid
        && state
            .checked_at
            .is_none_or(|at| Utc::now() - at >= chrono::Duration::seconds(2))
    {
        state.host = Some(Identity::current()?);
        state.checked_at = Some(Utc::now());
    }
    // Restart and grant revocation are independent: supersede progress may
    // require a live controller. Unclaimed reservations can also hide
    // a dispatcher spawned before its identity was persisted. Neither may
    // individually block replacement, nor justify revoking possible reviewer
    // descendants. Repeated failures still obey the bounded retirement cap.
    // trace:STORY-2571 | ai:codex
    if exited {
        let rotated = state
            .child_grant
            .as_ref()
            .and_then(|id| seat_authority::read_grant(id).ok())
            .is_some_and(|child| child.parent_grant_id.as_ref() != Some(&state.binding.grant));
        // trace:BUG-2645 | ai:claude
        let handoff = std::mem::take(&mut state.image_handoff);
        if !rotated && !handoff && state.last_exit != Some(0) {
            record_failure(root, Some("controller exited before successful dispatch"))?;
        }
        if let Some(grant) = state.child_grant.take() {
            state.retired_grants.push(grant);
        }
        state.worker = None;
        state.retry_after = if handoff {
            Utc::now()
        } else {
            Utc::now() + chrono::Duration::seconds(10)
        };
    }
    // Persist retirement before cleanup or spawn, including across failures.
    if before != serde_json::to_vec(&state)? {
        write(&dir, &state)?;
    }
    let before = serde_json::to_vec(&state)?;
    if !state.retired_grants.is_empty() && reviews_finished(root, &dir)? {
        for id in &state.retired_grants {
            seat_authority::revoke_by_id(id)?;
        }
        state.retired_grants.clear();
    }
    if before != serde_json::to_vec(&state)? {
        write(&dir, &state)?;
    }
    if !host_valid || state.child_grant.is_some() || Utc::now() < state.retry_after {
        return Ok(None);
    }
    // An active drain holds the respawn; an unreadable drain journal refuses
    // (counted and alerted) rather than restarting a loop it may be holding.
    // trace:BUG-2703 | ai:claude
    if crate::admission_drain::active(root)
        .context("advisor loops drain state unreadable; not restarting the worker")?
        .is_some()
    {
        return Ok(None);
    }
    let parent = state.binding.validate(root)?;
    // Prepare fallible I/O before minting authority. Every failed attempt
    // backs off; a minted grant is revoked if reservation persistence fails.
    // trace:STORY-2571 | ai:codex
    // Reuse only after the recorded controller is confirmed exited. Validate
    // both the current binding and the retained grant before resuming it.
    // trace:BUG-2640 | ai:codex
    let reuse = if state.retired_grants.len() >= MAX_RETIRED_GRANTS {
        let id = state.retired_grants.last().unwrap();
        let grant = seat_authority::read_grant(id)?;
        let grant = seat_authority::validated_grant(root, id, &grant.subject, Utc::now())
            .context("retained controller authority ended")?;
        anyhow::ensure!(
            grant.parent_grant_id.as_ref() == Some(&parent.id),
            "retained controller binding superseded"
        );
        Some(grant)
    } else {
        None
    };
    let mut minted = None;
    let prepared = (|| {
        reconcile_reviews(root, &dir)?;
        let log = dir.file("service.log", true)?;
        let stdout = log.try_clone()?;
        let (grant, mut command) = launch(root, &parent, reuse.as_ref())?;
        if reuse.is_none() {
            minted = Some(grant.id.clone());
        }
        state.retired_grants.retain(|id| id != &grant.id);
        state.child_grant = Some(grant.id.clone());
        write(&dir, &state)?; // reserve before spawn; a crash retains ambiguity
        command
            .current_dir(root)
            .args([
                "advisor",
                "loops",
                if TRAIN { "train-worker" } else { "worker" },
            ])
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(log);
        Ok::<_, anyhow::Error>((grant, command))
    })();
    let (grant, mut command) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            let revoked = minted.as_ref().map(|id| seat_authority::revoke_by_id(id));
            if revoked.as_ref().is_none_or(|result| result.is_ok()) {
                state.child_grant = None;
            }
            if let Some(grant) = &reuse {
                if !state.retired_grants.contains(&grant.id) {
                    state.retired_grants.push(grant.id.clone());
                }
            }
            let saved = write(&dir, &state);
            if let Some(result) = revoked {
                result?;
            }
            saved?;
            return Err(error);
        }
    };
    match command.spawn() {
        Ok(process) => {
            // Keep the handle even if recording fails: never reap first and
            // turn a fast-exiting worker into an ambiguous capture.
            *child = Some(process);
            state.worker = Some(Identity::child(child.as_ref().unwrap())?);
            state.image_handoff = false; // trace:BUG-2645 | ai:claude
            write(&dir, &state).map(|_| None)
        }
        Err(error) => {
            let revoked = if reuse.is_some() {
                state.retired_grants.push(grant.id.clone());
                Ok(())
            } else {
                seat_authority::revoke_by_id(&grant.id).map(|_| ())
            };
            if revoked.is_ok() {
                state.child_grant = None;
            }
            state.retry_after = Utc::now() + chrono::Duration::seconds(10);
            let saved = write(&dir, &state);
            revoked?;
            saved?;
            Err(error.into())
        }
    }
}

// Only WouldBlock is contention; unsafe files and I/O errors stay refusals.
// After the lock attempt fails, re-read the holder receipts and liveness now:
// skip quietly only for a verified live tick of this same service. Unknown,
// foreign, dead or recycled holders remain a counted, alerting refusal that
// carries the holder evidence. Flock stays authoritative; nothing is stolen.
// trace:BUG-2621 | ai:claude
fn contended(root: &Path, error: anyhow::Error) -> Result<Option<u32>> {
    let busy = error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock);
    if !busy {
        return Err(error);
    }
    match crate::admission_owner::service_holder(root, HOLDER_PURPOSE) {
        crate::admission_owner::ServiceHolder::Verified(pid) => Ok(Some(pid)),
        crate::admission_owner::ServiceHolder::Unverified(evidence) => {
            let message = format!("{NAME} admission.lock busy; holder not verified: {error}; {evidence}");
            Err(error.context(message))
        }
    }
}

// Read forge evidence without holding the service/admission lock. Pruning
// re-reads current attempts under the lock, preserving newly recorded children.
// trace:BUG-2640 | ai:codex
fn recover_closed_reviews(root: &Path) -> Result<()> {
    let Some(dir) = directory(root, false)? else {
        return Ok(());
    };
    if !read(&dir)?.is_some_and(|s| s.retired_grants.len() >= MAX_RETIRED_GRANTS) {
        return Ok(());
    }
    let repo = forge::Repository::load(root)?;
    let open = repo
        .discover_for_watch(root)?
        .iter()
        .map(forge::Change::key)
        .collect();
    Store::open(root, &repo.identity)?.prune(&open)?;
    Ok(())
}

// Retired grants protect reviews; the cap bounds controller replacement.
// Reservations do not record grant ancestry, so conservatively protect every
// unresolved attempt, including unrecorded and unknown descendants.
// trace:STORY-2571 | ai:codex
fn reviews_finished(root: &Path, dir: &Directory) -> Result<bool> {
    reconcile_reviews(root, dir)?;
    Ok(review_blockers(dir)?.is_empty())
}

// Expose the exact reservations retaining retired grants without disclosing
// grant selectors. Unknown descendants remain blockers even after termination.
// trace:STORY-2571 | ai:codex
fn review_blockers(dir: &Directory) -> Result<Vec<String>> {
    if TRAIN {
        return Ok(Vec::new());
    }
    let Some(bytes) = dir.read(STATE)? else {
        return Ok(Vec::new());
    };
    let reviews: State = serde_json::from_slice(&bytes)?;
    Ok(reviews
        .entries
        .iter()
        .flat_map(|(key, entry)| {
            entry
                .attempts
                .iter()
                .filter(|a| {
                    active(a)
                        || a.unrecorded_descendant()
                        || a.recorded().iter().any(|p| p.alive())
                        || (a.pgid.is_some() && confirm(a).is_err())
                })
                .map(move |a| format!("{key}:{} ({:?})", a.token, a.phase))
        })
        .collect())
}

// Exhaustion is informational and must not retain retired grants.
// trace:BUG-2518 | ai:codex
pub(super) fn retry_blockers(root: &Path, dir: &Directory) -> Result<Vec<String>> {
    if TRAIN {
        return Ok(Vec::new());
    }
    let Some(bytes) = dir.read(STATE)? else {
        return Ok(Vec::new());
    };
    let reviews: State = serde_json::from_slice(&bytes)?;
    let store = Store::open(root, &reviews.repository)?;
    let mut blockers = Vec::new();
    for (key, entry) in &reviews.entries {
        for attempt in &entry.attempts {
            if let Some(reason) = no_verdict_blocker(attempt) {
                if !store.history(key)?.conclusive(&attempt.head) {
                    blockers.push(format!("{key}: {reason}"));
                }
            }
        }
    }
    Ok(blockers)
}

// Reuse the ordinary reservation recovery while holding admission.lock. A
// controller restart never clears an unrecorded or live review descendant.
// trace:STORY-2571 | ai:codex
fn reconcile_reviews(root: &Path, dir: &Directory) -> Result<()> {
    if TRAIN {
        return Ok(());
    }
    let Some(bytes) = dir.read(STATE)? else {
        anyhow::ensure!(
            dir.read("initialized")?.is_none(),
            "advisor loop state disappeared after initialization"
        );
        return Ok(());
    };
    let state: State = serde_json::from_slice(&bytes)
        .context("invalid advisor loop state before service restart")?;
    let store = Store {
        runtime: Directory::existing(&dir.path().join(".."))?
            .context("advisor loop runtime disappeared")?,
        dir: Directory {
            handle: dir.handle.try_clone()?,
        },
        repository: state.repository.clone(),
        max_rounds: resolve_max_rounds(root)?,
        root: root.into(),
    };
    let mut state = store.load()?;
    let before = serde_json::to_vec(&state)?;
    store.reconcile_lost_dispatchers(&mut state);
    // A crash after persisting supersede intent must not leave the service
    // dependent on an old watcher pass. Finish only with the ordinary complete
    // process/group proof; a replacement handles signals and ambiguous cases.
    // trace:STORY-2571 | ai:codex
    for attempt in state.entries.values_mut().flat_map(|e| &mut e.attempts) {
        if attempt.phase == Phase::Superseding && confirm(attempt).is_ok() {
            attempt.phase = Phase::Superseded;
        }
    }
    validate_state(&state, &store.repository)?;
    if before != serde_json::to_vec(&state)? {
        dir.write(STATE, &serde_json::to_vec_pretty(&state)?)?;
    }
    Ok(())
}

/// The trusted launcher calls this only after issuing the new advisor grant.
/// Socket attribution still checks the launcher's process binding; a remembered
/// advisor registration never authorizes this operation.
// trace:STORY-2571 | ai:codex
pub(crate) fn rebind_after_launch(start: &Path, grant: &SeatGrant) -> Result<()> {
    let root = forge::main_root(start)?;
    let default_running = matches!(
        crate::deputy_shell::current_queue("default")?.runner_status(),
        crate::deputy_shell::RunnerStatus::Live(_) | crate::deputy_shell::RunnerStatus::Legacy(_)
    );
    rebind_after_launch_with(
        &root,
        grant,
        default_running,
        |deputy, root, grant, configured| {
            let queue = crate::deputy_shell::current_queue(deputy)?;
            let reply = match crate::deputy_shell::socket::request(
                &queue,
                &crate::deputy_shell::socket::Request::ReviewWatchRebind {
                    project: root.into(),
                    grant: grant.id.clone(),
                },
            ) {
                Ok(reply) => reply,
                // A deputy can disappear between the liveness probe and connect.
                // An unused service is silent unless a deputy actually refuses it.
                Err(_) if !configured => return Ok(()),
                Err(error) => return Err(error),
            };
            anyhow::ensure!(
                reply.ok,
                "review-watch rotation rebind refused: {}",
                reply.why.unwrap_or_default()
            );
            Ok(())
        },
    )
}

// Trusted launchers may bootstrap the default deputy or rotate an operator's
// binding to the advisor handle they just minted. The socket still attributes
// the launcher to that grant and rebind applies all successor checks.
// trace:STORY-2571 | ai:codex
fn rebind_after_launch_with(
    root: &Path,
    grant: &SeatGrant,
    default_running: bool,
    request: impl FnOnce(&str, &Path, &SeatGrant, bool) -> Result<()>,
) -> Result<()> {
    let state = if let Some(dir) = directory(root, false)? {
        let _lock = dir.admission_lock(&format!("{NAME} checking deputy binding"))?;
        read(&dir)?
    } else {
        None
    };
    if state.is_none() && !default_running {
        return Ok(());
    }
    let deputy = state
        .as_ref()
        .map(|s| s.deputy.as_str())
        .unwrap_or("default");
    request(deputy, root, grant, state.is_some())
}

// Durable failure episodes: stable notice IDs make retries idempotent across
// restarts. Success starts a new episode; backoff ticks never clear failures.
// trace:STORY-2691 | ai:codex
#[derive(Default, Serialize, Deserialize)]
struct Failures {
    #[serde(default)]
    dispatch_pending: bool,
    count: u32,
    // Bound quiet-hours queueing as well as delivered push notifications.
    // trace:BUG-2640 | ai:codex
    #[serde(default)]
    push_after: Option<DateTime<Utc>>,
    episode: String,
    /// The current episode's threshold notice, sent or not.
    alert: Option<Notice>,
    /// Undelivered notices from earlier episodes; each is an independent
    /// obligation retried until delivered (never replaced by a later one).
    // trace:STORY-2691 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    carried: Vec<Notice>,
}
fn failure_state(root: &Path) -> Result<Failures> {
    let Some(dir) = directory(root, false)? else {
        return Ok(Failures::default());
    };
    dir.read("failures.json")?
        .map(|b| serde_json::from_slice(&b).map_err(Into::into))
        .transpose()
        .map(|v| v.unwrap_or_default())
}
pub(super) fn record_failure(root: &Path, reason: Option<&str>) -> Result<()> {
    // Supervisor observations prove no dispatch success, even on healthy ticks.
    // Retain pending failures until the controller confirms a successful dispatch.
    record_outcome(root, reason, None)
}
fn record_outcome(root: &Path, reason: Option<&str>, dispatched: Option<bool>) -> Result<()> {
    record_outcome_with_push(root, reason, dispatched, |count, reason| {
        crate::notify::send_direct_bounded(
            root,
            &format!("advisor-loop-{NAME}-blocked"),
            &format!("AIDA: {NAME} is blocked"),
            &format!("{NAME}: {count} consecutive failures. {}. Inspect aida advisor loops status and .aida/{NAME}/service.log.", bounded_diagnostic(reason)),
            Some(std::time::Duration::from_secs(10)),
        )?;
        Ok(())
    })
}

// trace:BUG-2640 | ai:codex
fn record_outcome_with_push(
    root: &Path,
    reason: Option<&str>,
    dispatched: Option<bool>,
    mut push: impl FnMut(u32, &str) -> Result<()>,
) -> Result<()> {
    let dir = directory(root, true)?.context("service directory missing")?;
    let _lock = dir.lock("failures.lock")?;
    let mut state = failure_state(root)?;
    match dispatched {
        Some(false) => state.dispatch_pending = true,
        Some(true) => state.dispatch_pending = false,
        None => {}
    }
    if let Some(reason) = reason {
        if state.episode.is_empty() {
            state.episode = uuid::Uuid::new_v4().to_string();
        }
        state.count = state.count.saturating_add(1);
        let id = format!("advisor-loop-{NAME}-{}", state.episode);
        // `alert` belongs only to the current episode. A notice from an earlier
        // episode (e.g. a pre-upgrade file) is retired into `carried` if unsent,
        // so it can never suppress this episode's own threshold alert.
        // trace:STORY-2691 | ai:claude
        if state.alert.as_ref().is_some_and(|a| a.message.id != id) {
            if let Some(old) = state.alert.take().filter(|a| !a.sent) {
                state.carried.push(old);
            }
        }
        if state.count >= 3 && state.alert.is_none() {
            state.alert = Some(Notice {
                message: notice::message(id, "advisor",
                    format!("{NAME}: repeated service failure"),
                    format!("{NAME} failed {} consecutive times. Last failure: {}. Inspect aida advisor loops status and train-status, and .aida/{NAME}/service.log; renew/rebind the advisor grant if authority ended.", state.count, bounded_diagnostic(reason))),
                sent: false,
            });
        }
    } else if !state.dispatch_pending {
        // Success resets bookkeeping, but cannot discard an undelivered notice:
        // every episode's unsent alert stays an obligation of its own.
        let mut carried = std::mem::take(&mut state.carried);
        carried.extend(state.alert.take().filter(|alert| !alert.sent));
        state = Failures {
            carried,
            ..Failures::default()
        };
    }
    for alert in state.carried.iter_mut().chain(state.alert.as_mut()) {
        alert.message.from = NAME.into();
    }
    dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    // Use the configured operator push transport as well as advisor mail.
    // Its durable rate limiter/quiet-hours queue bounds repeated alerts while
    // a growing streak keeps retrying, including across supervisor restarts.
    // trace:BUG-2640 | ai:codex
    let mut push_result = Ok(());
    if let Some(reason) = reason.filter(|_| state.count > 0) {
        if state.count > failure_escalation_threshold(root)?
            && state.push_after.is_none_or(|at| Utc::now() >= at)
        {
            state.push_after = Some(Utc::now() + chrono::Duration::minutes(10));
            dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
            push_result = push(state.count, reason);
        }
    }
    // Oldest obligations first; each delivery is persisted on its own so a
    // later failure never loses an earlier success (stable IDs dedupe retries).
    while let Some(old) = state.carried.first() {
        deliver(root, old)?;
        state.carried.remove(0);
        dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    }
    if let Some(alert) = state.alert.as_mut().filter(|a| !a.sent) {
        deliver(root, alert)?;
        alert.sent = true;
        dir.write("failures.json", &serde_json::to_vec_pretty(&state)?)?;
    }
    push_result
}

// trace:BUG-2640 | ai:codex
fn failure_escalation_threshold(root: &Path) -> Result<u32> {
    let path = root.join(".aida/config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(100),
        Err(error) => return Err(error.into()),
    };
    let config: toml::Value = toml::from_str(&text)?;
    config
        .get("review_watch")
        .and_then(|s| s.get("failure_escalation_threshold"))
        .map(|value| {
            value
                .as_integer()
                .filter(|n| (1..=u32::MAX as i64).contains(n))
                .map(|n| n as u32)
                .context("review_watch.failure_escalation_threshold must be a positive integer")
        })
        .unwrap_or(Ok(100))
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // trace:BUG-2720 | ai:codex
    /// A change held behind a kept reviewer checkout ages across passes and
    /// degrades the worker after 30 minutes, naming the fix; a pass that no
    /// longer finds it held clears it. trace:BUG-2776 | ai:claude
    #[test]
    fn bug2776_retained_checkout_block_surfaces_after_thirty_minutes() {
        let held = Unadmitted { key: "PR-4638".into(), head: "d69af92a".into(), cause: "dirty".into() };
        let counts = PassCounts { retained: vec![held], ..Default::default() };
        let start = Utc::now() - chrono::Duration::minutes(RETAINED_STALE_MINUTES + 1);
        let first = next_pass_report(None, &counts, &[], start);
        assert!(retained_blocks(&first, start + chrono::Duration::minutes(29)).is_empty());
        let now = Utc::now();
        let later = next_pass_report(Some(&first), &counts, &[], now);
        let blocks = retained_blocks(&later, now);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let (key, line) = &blocks[0];
        assert_eq!(key, "PR-4638");
        assert!(line.starts_with("review blocked by a retained reviewer worktree: PR-4638 held"), "{line}");
        assert!(line.contains("(dirty)") && line.contains("aida session end --branch pr-4638 --yes"), "{line}");
        assert!(line.contains("aida review watch --release PR-4638"), "{line}");
        assert!(pass_degradation(&Ok(Some(later.clone())), None, now).iter().any(|r| r == line));
        let findings = crate::landing_drought::retained_findings(blocks);
        assert_eq!(findings[0].id, "PR-4638");
        assert!(findings[0].action.starts_with("inspect the `pr-4638` checkout"));
        // A capped retry surfaces at once, with the next step. trace:TASK-3921 | ai:claude
        let cause = crate::review_watch::PICKUP_RETRY_CAP.to_string();
        let capped = Unadmitted { key: "PR-4639".into(), head: "e0".into(), cause };
        let capped = PassCounts { retained: vec![capped], ..Default::default() };
        let blocks = retained_blocks(&next_pass_report(None, &capped, &[], now), now);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(blocks[0].1.contains("held 0 min (pickup retry cap reached)"), "{}", blocks[0].1);
        assert!(blocks[0].1.contains("aida review watch --release PR-4639"), "{}", blocks[0].1);
        let cleared = next_pass_report(Some(&later), &PassCounts::default(), &[], now);
        assert!(cleared.retained.is_empty());
    }

    #[test]
    fn bug2720_pass_summary_names_skipped_heads() {
        let mut counts = PassCounts::default();
        counts.skipped.insert("held by exited attempts".into(), vec!["PR-7".into(), "MR-9".into()]);
        counts.skipped.insert("capped".into(), vec!["PR-10".into()]);
        let report = next_pass_report(None, &counts, &[], Utc::now());
        let line = pass_log_line(&report);
        assert!(line.contains("2 held by exited attempts (PR-7, MR-9)"));
        assert!(line.contains("1 capped (PR-10)"));
    }

    // trace:BUG-2621 | ai:claude
    fn bug2621_sleeper() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap()
    }

    // trace:BUG-2621 | ai:claude
    fn bug2621_busy_tick(root: &Path) -> Result<Option<u32>> {
        tick_with_launcher(root, "fixture", true, &mut None, |_, _, _| {
            panic!("busy supervisor must not launch")
        })
    }

    // A verified live tick of this same service (admission_owner receipt names
    // this service's purpose; PID alive with matching start identity) is a
    // quiet skip: no service read, no launch, no failure count, no alert.
    // trace:BUG-2621 | ai:claude
    #[test]
    fn bug2621_busy_supervisor_tick_skips_quietly_for_verified_same_service_holder() {
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), true).unwrap().unwrap();
        let lock = dir.lock("admission.lock").unwrap();
        dir.write(SERVICE, b"invalid receipt").unwrap();
        let mut holder = bug2621_sleeper();
        let pid = holder.id();
        let identity = aida_core::proc_identity::ProcIdentity::capture(pid).unwrap();
        let receipt =
            crate::admission_owner::write_test_receipt(temp.path(), identity, HOLDER_PURPOSE);
        let outcome = bug2621_busy_tick(temp.path());
        assert_eq!(outcome.as_ref().unwrap(), &Some(pid));
        assert_eq!(dir.read(SERVICE).unwrap().unwrap(), b"invalid receipt");
        let (mut skipped, mut reported) = (None, None);
        for _ in 0..3 {
            settle(
                temp.path(),
                bug2621_busy_tick(temp.path()),
                Ok(()),
                &mut skipped,
                &mut reported,
            );
        }
        assert_eq!(skipped, Some(pid));
        assert!(reported.is_none());
        let failures = failure_state(temp.path()).unwrap();
        assert_eq!(failures.count, 0);
        assert!(failures.alert.is_none());
        assert!(receipt.exists(), "decision must not mutate receipts");
        // Revalidated at decision time, never cached: the same receipt for a
        // holder that has since exited no longer verifies.
        holder.kill().unwrap();
        holder.wait().unwrap();
        let error = bug2621_busy_tick(temp.path()).unwrap_err();
        assert!(format!("{error:#}").contains("dead or PID reused"));
        assert!(receipt.exists(), "receipts are never unlinked by the tick");
        drop(lock);
        // A stale, unlocked lock file cannot hide a real receipt refusal.
        let error = tick(temp.path(), "fixture", true, &mut None).unwrap_err();
        assert!(format!("{error:#}").contains("invalid advisor loop service receipt"));
    }

    // trace:BUG-2746 | ai:codex
    #[test]
    fn bug2746_busy_alert_carries_activity_and_age() {
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), true).unwrap().unwrap();
        let _lock = dir.admission_lock("checking deputy binding").unwrap();
        let mut skipped = None;
        let mut reported = None;
        for _ in 0..3 {
            settle(
                temp.path(),
                bug2621_busy_tick(temp.path()),
                Ok(()),
                &mut skipped,
                &mut reported,
            );
        }
        let alert = failure_state(temp.path()).unwrap().alert.unwrap();
        let (_, age) = alert
            .message
            .body
            .split_once("last recorded activity checking deputy binding, age ")
            .unwrap();
        assert!(age.split_once('s').unwrap().0.parse::<u64>().is_ok());
    }

    // Every other contention stays a counted, alerting refusal carrying the
    // holder evidence: unknown holder, another service, a dead PID, a
    // recycled PID (start identity mismatch), a malformed receipt, and a
    // receipt naming this very process.
    // trace:BUG-2621 | ai:claude
    #[test]
    fn bug2621_busy_supervisor_tick_refuses_unverified_holders() {
        let other = if TRAIN {
            "legacy review-watch"
        } else {
            "legacy auto-train"
        };
        let mut live = bug2621_sleeper();
        let live_identity = aida_core::proc_identity::ProcIdentity::capture(live.id()).unwrap();
        let mut dead = bug2621_sleeper();
        let dead_identity = aida_core::proc_identity::ProcIdentity::capture(dead.id()).unwrap();
        dead.kill().unwrap();
        dead.wait().unwrap();
        let mut recycled = live_identity.clone();
        recycled.starttime = recycled.starttime.wrapping_add(1);
        let own = aida_core::proc_identity::ProcIdentity::capture(std::process::id()).unwrap();
        type Setup = Box<dyn Fn(&Path)>;
        let cases: Vec<(&str, Setup, &str)> = vec![
            (
                "unknown holder",
                Box::new(|_| {}),
                "holder PID/start identity/purpose unknown",
            ),
            (
                "another service",
                {
                    let id = live_identity.clone();
                    Box::new(move |root| {
                        crate::admission_owner::write_test_receipt(root, id.clone(), other);
                    })
                },
                "alive",
            ),
            (
                "dead PID",
                Box::new(move |root| {
                    crate::admission_owner::write_test_receipt(
                        root,
                        dead_identity.clone(),
                        HOLDER_PURPOSE,
                    );
                }),
                "dead or PID reused",
            ),
            (
                "recycled PID",
                Box::new(move |root| {
                    crate::admission_owner::write_test_receipt(
                        root,
                        recycled.clone(),
                        HOLDER_PURPOSE,
                    );
                }),
                "dead or PID reused",
            ),
            (
                "malformed receipt",
                Box::new(|root| {
                    let dir = root.join(".aida/admission-owner");
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join("broken.json"), b"{not json").unwrap();
                }),
                "malformed receipt broken.json",
            ),
            (
                "this process",
                Box::new(move |root| {
                    crate::admission_owner::write_test_receipt(root, own.clone(), HOLDER_PURPOSE);
                }),
                "alive",
            ),
        ];
        for (name, setup, evidence) in cases {
            let temp = tempfile::tempdir().unwrap();
            let dir = directory(temp.path(), true).unwrap().unwrap();
            let _lock = dir.lock("admission.lock").unwrap();
            dir.write(SERVICE, b"invalid receipt").unwrap();
            setup(temp.path());
            let receipts = std::fs::read_dir(temp.path().join(".aida/admission-owner"))
                .map(|d| {
                    d.filter(|e| {
                        e.as_ref()
                            .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "json"))
                    })
                    .count()
                })
                .unwrap_or(0);
            let (mut skipped, mut reported) = (None, None);
            for count in 1..=3 {
                let outcome = bug2621_busy_tick(temp.path());
                let error = outcome.as_ref().unwrap_err();
                let text = format!("{error:#}");
                assert!(
                    text.contains("admission.lock busy; holder not verified"),
                    "{name}: {text}"
                );
                assert!(
                    text.contains("review-watch lock busy: admission.lock"),
                    "{name}"
                );
                assert!(text.contains(evidence), "{name}: {text}");
                assert_eq!(
                    error.downcast_ref::<std::io::Error>().unwrap().kind(),
                    std::io::ErrorKind::WouldBlock,
                    "{name}"
                );
                settle(temp.path(), outcome, Ok(()), &mut skipped, &mut reported);
                assert_eq!(failure_state(temp.path()).unwrap().count, count, "{name}");
            }
            assert!(skipped.is_none(), "{name}");
            let alert = failure_state(temp.path()).unwrap().alert.unwrap();
            assert!(alert.sent, "{name}");
            assert!(alert.message.body.contains("admission.lock busy"), "{name}");
            assert!(alert.message.body.contains(evidence), "{name}");
            assert_eq!(
                dir.read(SERVICE).unwrap().unwrap(),
                b"invalid receipt",
                "{name}"
            );
            let after = std::fs::read_dir(temp.path().join(".aida/admission-owner"))
                .map(|d| {
                    d.filter(|e| {
                        e.as_ref()
                            .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "json"))
                    })
                    .count()
                })
                .unwrap_or(0);
            assert_eq!(after, receipts, "{name}: receipts must not be unlinked");
        }
        live.kill().unwrap();
        live.wait().unwrap();
    }

    // trace:BUG-2621 | ai:codex
    #[test]
    fn bug2621_unsafe_admission_lock_remains_a_refusal() {
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), true).unwrap().unwrap();
        dir.file("admission.lock", true).unwrap();
        std::fs::set_permissions(
            dir.path().join("admission.lock"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let error = tick(temp.path(), "fixture", true, &mut None).unwrap_err();
        assert!(format!("{error:#}").contains("unsafe review-watch file admission.lock"));
    }

    // trace:BUG-2640 | ai:codex
    #[test]
    fn bug2640_growing_failure_streak_retries_operator_push_and_resets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review_watch]\nfailure_escalation_threshold = 3\n",
        )
        .unwrap();
        let mut calls = Vec::new();
        for _ in 0..5 {
            record_outcome_with_push(root, Some("blocked"), None, |count, _| {
                calls.push(count);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(calls, vec![4]);
        // A restarted supervisor reloads the deadline, then alerts again
        // once the backoff expires while the same episode keeps growing.
        let mut state = failure_state(root).unwrap();
        state.push_after = Some(Utc::now() - chrono::Duration::seconds(1));
        directory(root, false)
            .unwrap()
            .unwrap()
            .write("failures.json", &serde_json::to_vec_pretty(&state).unwrap())
            .unwrap();
        record_outcome_with_push(root, Some("still blocked"), None, |count, _| {
            calls.push(count);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, vec![4, 6]);
        record_outcome_with_push(root, None, Some(true), |_, _| {
            panic!("recovery must not alert")
        })
        .unwrap();
        record_outcome_with_push(root, Some("new episode"), None, |_, _| {
            panic!("below threshold")
        })
        .unwrap();
        assert_eq!(failure_state(root).unwrap().count, 1);
    }

    // trace:BUG-2640 | ai:codex
    #[test]
    fn bug2640_failed_push_persists_backoff_and_delivers_mailbox_notice() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review_watch]\nfailure_escalation_threshold = 2\n",
        )
        .unwrap();
        for _ in 0..2 {
            record_outcome_with_push(root, Some("blocked"), None, |_, _| Ok(())).unwrap();
        }
        let before = Utc::now();
        assert!(
            record_outcome_with_push(root, Some("blocked"), None, |_, _| anyhow::bail!(
                "push unavailable"
            ))
            .is_err()
        );
        let state = failure_state(root).unwrap();
        assert!(state.push_after.unwrap() >= before + chrono::Duration::minutes(10));
        assert!(state.alert.unwrap().sent);
        record_outcome_with_push(root, Some("still blocked"), None, |_, _| {
            panic!("persisted backoff must survive failed transport")
        })
        .unwrap();
    }

    // trace:BUG-2645 | ai:codex
    #[test]
    fn bug2645_promotion_refuses_controller_admission() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        env.set_key("AIDA_DEV_LIVE_DIR", root);
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        write(&dir, &state).unwrap();
        std::fs::write(root.join("aida"), "new image").unwrap();
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id.clone()));
        LIVE_IMAGE.with(|enabled| enabled.set(true));
        let stale = check_controller(root).unwrap_err();
        assert!(
            format!("{stale:#}").contains("controller receipt superseded"),
            "{stale:#}"
        );
        assert!(!read(&dir).unwrap().unwrap().image_handoff);
        state.worker = Some(Identity::current().unwrap());
        write(&dir, &state).unwrap();
        let result = check_controller(root);
        LIVE_IMAGE.with(|enabled| enabled.set(false));
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        let error = result.unwrap_err();
        assert!(format!("{error:#}").contains("binary superseded"));
        assert!(stops_watcher(&error));
        assert!(error.is::<ImageSuperseded>());
        assert!(seat_authority::read_grant(&child.id).is_ok());
        let state = read(&dir).unwrap().unwrap();
        assert!(state.image_handoff);
        assert!(state.retired_grants.is_empty());
    }

    // trace:BUG-2645 | ai:codex
    #[test]
    fn bug2645_supervisor_withdraws_obsolete_worker_heartbeat() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let _live = LiveImage::set(&root.join("aida"));
        std::fs::write(root.join("aida"), "promoted image").unwrap();
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let mut process = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let worker = Identity::child(&process).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(worker.clone());
        state.child_grant = Some(grant.id.clone());
        write(&dir, &state).unwrap();
        let result = tick(root, "default", true, &mut None);
        let after = read(&dir).unwrap().unwrap();
        // Reaping belongs to the owner; staleness must not kill a train.
        let alive = process.try_wait().unwrap().is_none();
        let _ = process.kill();
        let _ = process.wait();
        result.unwrap();
        assert!(alive);
        assert!(!after.host_active);
        assert!(after.image_handoff);
        assert_eq!(after.worker, Some(worker));
        assert_eq!(after.child_grant, Some(grant.id.clone()));
        assert!(seat_authority::read_grant(&grant.id).is_ok());
    }

    /// Point supervision at a fixture live image for this test thread only.
    // trace:BUG-2645 | ai:claude
    pub(super) struct LiveImage;
    impl LiveImage {
        pub(super) fn set(image: &Path) -> Self {
            TEST_LIVE_IMAGE.with(|live| *live.borrow_mut() = Some(image.to_path_buf()));
            Self
        }
    }
    impl Drop for LiveImage {
        fn drop(&mut self) {
            TEST_LIVE_IMAGE.with(|live| *live.borrow_mut() = None);
        }
    }

    /// Promote `source` to `dest` the way `aida dev build` does: a fresh inode
    /// renamed over the live path.
    // trace:BUG-2645 | ai:claude
    #[cfg(target_os = "linux")]
    pub(super) fn promote_image(source: &str, dest: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let incoming = dest.with_extension("incoming");
        std::fs::copy(source, &incoming).unwrap();
        std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&incoming, dest).unwrap();
    }

    /// A controller stand-in executed from `image` (a copied shell), so its
    /// /proc/<pid>/exe is that image. Retries ETXTBSY from concurrent forks.
    // trace:BUG-2645 | ai:claude
    #[cfg(target_os = "linux")]
    pub(super) fn image_command(image: &Path) -> Command {
        let mut command = Command::new(image);
        command.args(["-c", "while :; do sleep 0.1; done"]);
        command
    }
    #[cfg(target_os = "linux")]
    pub(super) fn spawn_retrying(command: &mut Command) -> Child {
        loop {
            match command.spawn() {
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                result => return result.unwrap(),
            }
        }
    }
    #[cfg(target_os = "linux")]
    pub(super) fn image_launcher(
        image: PathBuf,
    ) -> impl Fn(&Path, &SeatGrant, Option<&SeatGrant>) -> Result<(SeatGrant, Command)> {
        move |root, parent, reuse| {
            let grant = match reuse {
                Some(grant) => grant.clone(),
                None => {
                    seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject)?
                }
            };
            Ok((grant, image_command(&image)))
        }
    }
    #[cfg(target_os = "linux")]
    fn stop(controller: &mut Option<Child>) {
        if let Some(process) = controller.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    // This module is included by both fixed worker services.
    // trace:BUG-2645 | ai:codex
    #[test]
    fn bug2645_worker_checks_controller_without_live_dir() {
        use std::io::{Read, Write};

        const FIXTURE_GRANT: &str = "AIDA_TEST_BUG2645_FALLBACK_GRANT";
        let environment = aida_core::exec_context::Environment::capture();
        if let Ok(id) = environment.var(FIXTURE_GRANT) {
            std::io::stdin().read_exact(&mut [0]).unwrap();
            let root = std::env::current_dir().unwrap();
            assert!(!crate::dev_build::default_live_dir().unwrap().exists());
            CONTROLLER.with(|controller| *controller.borrow_mut() = Some(id.to_owned()));
            LIVE_IMAGE.with(|enabled| enabled.set(true));
            check_controller(&root).unwrap();
            let dir = directory(&root, false).unwrap().unwrap();
            let mut state = read(&dir).unwrap().unwrap();
            assert!(!state.image_handoff);
            state.host_active = false;
            write(&dir, &state).unwrap();
            let error = check_controller(&root).unwrap_err();
            assert!(
                format!("{error:#}").contains("heartbeat is stale"),
                "{error:#}"
            );
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let live = root.join("missing-live");
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        env.set_key("AIDA_DEV_LIVE_DIR", &live);
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut command = crate::launch::review_watch_controller_command(&grant).unwrap();
        assert_eq!(
            command.get_program(),
            crate::aida_bin::current_executable_path()
                .unwrap()
                .as_os_str()
        );
        let thread = std::thread::current();
        let mut slot = Some(
            command
                .args(["--exact", thread.name().unwrap()])
                .current_dir(root)
                .env(FIXTURE_GRANT, &grant.id)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let reap = Reap(&mut slot);
        let process = reap.0.as_mut().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::child(process).unwrap());
        state.child_grant = Some(grant.id);
        state.host = Some(Identity::current().unwrap());
        state.host_active = true;
        state.checked_at = Some(Utc::now());
        write(&dir, &state).unwrap();
        process.stdin.take().unwrap().write_all(b"go").unwrap();
        assert!(process.wait().unwrap().success());
    }

    // Run the actual admission check in the recorded worker process, with
    // the supervisor paused from promotion through orderly worker exit.
    // trace:BUG-2645 | ai:codex
    #[test]
    #[cfg(target_os = "linux")]
    fn bug2645_worker_first_handoff_replaces_in_reaping_tick() {
        use std::io::{Read, Write};

        const FIXTURE_GRANT: &str = "AIDA_TEST_BUG2645_WORKER_GRANT";
        let environment = aida_core::exec_context::Environment::capture();
        if let Ok(id) = environment.var(FIXTURE_GRANT) {
            std::io::stdin().read_exact(&mut [0]).unwrap();
            let root = std::env::current_dir().unwrap();
            CONTROLLER.with(|controller| *controller.borrow_mut() = Some(id.to_owned()));
            LIVE_IMAGE.with(|enabled| enabled.set(true));
            let result = if TRAIN {
                check_controller(&root)
            } else {
                let store = Store::open(&root, "github:github.com/owner/repo").unwrap();
                let change = forge::Change {
                    id: 1,
                    head: format!("{:040x}", 1),
                    base: "main".into(),
                    author: "writer".into(),
                    gitlab: false,
                    overlap: Some(false),
                    conflict: None,
                    behind: Some(0),
                    head_branch: None,
                };
                super::super::watch_changes(&store, vec![change], |change| {
                    let result =
                        super::super::select_with_coverage(&store, &change, None, 2, || None)
                            .map(|_| ());
                    super::super::observe_dispatch_result(&root, result)
                })
                .map(|_| ())
            };
            assert!(result.unwrap_err().is::<ImageSuperseded>());
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let live = root.join("aida");
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let grant =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        if !TRAIN {
            for _ in 0..2 {
                record_failure(root, Some("prior failure")).unwrap();
            }
        }
        let failures = dir.read("failures.json").unwrap();

        let thread = std::thread::current();
        let mut slot = Some(
            Command::new("/proc/self/exe")
                .args(["--exact", thread.name().unwrap()])
                .current_dir(root)
                .env(FIXTURE_GRANT, &grant.id)
                .env("AIDA_DEV_LIVE_DIR", root)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let reap = Reap(&mut slot);
        let controller = &mut *reap.0;
        let process = controller.as_mut().unwrap();
        let old_worker = Identity::child(process).unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(old_worker.clone());
        state.child_grant = Some(grant.id);
        write(&dir, &state).unwrap();
        let old_image = PathBuf::from(format!("/proc/{}/exe", old_worker.pid));
        std::os::unix::fs::symlink(&old_image, &live).unwrap();
        assert!(crate::launch::control::service_image_is_live(&old_image, &live).unwrap());
        promote_image("/bin/sh", &live);
        process.stdin.take().unwrap().write_all(b"go").unwrap();
        assert!(process.wait().unwrap().success());
        assert!(read(&dir).unwrap().unwrap().image_handoff);
        assert_eq!(dir.read("failures.json").unwrap(), failures);

        tick_with_launcher(root, "default", true, controller, image_launcher(live)).unwrap();
        let state = read(&dir).unwrap().unwrap();
        assert_eq!(state.last_exit, Some(0));
        assert_ne!(
            state.worker.expect("replacement launched").pid,
            old_worker.pid
        );
        assert!(!state.image_handoff);
        assert!(state.retry_after <= Utc::now());
        assert_eq!(dir.read("failures.json").unwrap(), failures);
    }

    /// Kill a fixture controller even when an assertion fails first.
    #[cfg(target_os = "linux")]
    struct Reap<'a>(&'a mut Option<Child>);
    #[cfg(target_os = "linux")]
    impl Drop for Reap<'_> {
        fn drop(&mut self) {
            stop(self.0);
        }
    }

    // Follow a promotion end to end for each fixed service: detection keeps
    // the controller and its descendants' authority intact, the controller's
    // exit is an orderly handoff, and the reaping tick launches the
    // replacement from the promoted image with no failure backoff.
    // trace:BUG-2645 | ai:claude
    #[test]
    #[cfg(target_os = "linux")]
    fn bug2645_promotion_handoff_replaces_controller_in_reaping_tick() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let _live = LiveImage::set(&root.join("aida"));
        let live = root.join("aida");
        promote_image("/bin/sh", &live);
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let launch = image_launcher(live.clone());
        let mut slot = None;
        let reap = Reap(&mut slot);
        let controller = &mut *reap.0;
        tick_with_launcher(root, "default", true, controller, &launch).unwrap();
        let first = read(&dir).unwrap().unwrap();
        let old_worker = first.worker.clone().expect("controller launched");
        let old_grant = first.child_grant.clone().unwrap();
        let exe = |pid: u32| PathBuf::from(format!("/proc/{pid}/exe"));
        assert!(
            crate::launch::control::service_image_is_live(&exe(old_worker.pid), &live).unwrap()
        );
        let reviewer = seat_authority::issue_child_from(
            seat_authority::read_grant(&old_grant).unwrap(),
            "reviewer",
            "surviving-reviewer",
            false,
        )
        .unwrap();

        promote_image("/bin/sh", &live);
        tick_with_launcher(root, "default", true, controller, |_, _, _| {
            panic!("must not spawn over the live obsolete controller")
        })
        .unwrap();
        let handoff = read(&dir).unwrap().unwrap();
        assert!(handoff.image_handoff);
        assert!(!handoff.host_active, "heartbeat withdrawn");
        assert_eq!(handoff.worker.as_ref(), Some(&old_worker));
        assert_eq!(handoff.child_grant.as_ref(), Some(&old_grant));
        assert!(controller.as_mut().unwrap().try_wait().unwrap().is_none());
        assert!(seat_authority::read_grant(&old_grant)
            .unwrap()
            .revoked_at
            .is_none());
        assert!(seat_authority::read_grant(&reviewer.id)
            .unwrap()
            .revoked_at
            .is_none());

        // The controller yields at its boundary (exit status is irrelevant to
        // an orderly handoff). One supervision tick reaps and replaces it.
        stop(controller);
        tick_with_launcher(root, "default", true, controller, &launch).unwrap();
        let after = read(&dir).unwrap().unwrap();
        {
            let new_worker = after.worker.clone().expect("replacement launched");
            assert_ne!(new_worker.pid, old_worker.pid);
            assert!(after.retry_after <= Utc::now());
            assert!(!after.image_handoff);
            assert!(after.host_active);
            assert_eq!(failure_state(root).unwrap().count, 0);
            assert!(
                crate::launch::control::service_image_is_live(&exe(new_worker.pid), &live).unwrap()
            );
            let new_grant =
                seat_authority::read_grant(after.child_grant.as_ref().unwrap()).unwrap();
            assert_ne!(new_grant.id, old_grant);
            assert_eq!(new_grant.parent_grant_id.as_ref(), Some(&parent.id));
            // With no unresolved review descendants the retired controller
            // authority is released; retention with blockers is covered by
            // the review service's handoff regression.
            assert!(after.retired_grants.is_empty());
        }
        assert!(controller.is_some());
    }

    // An idle controller must notice a withdrawn heartbeat within one
    // supervision cycle rather than after a full discovery interval.
    // trace:BUG-2645 | ai:claude
    #[test]
    fn bug2645_idle_controller_yields_within_one_supervision_cycle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        state.worker = Some(Identity::current().unwrap());
        state.host = Some(Identity::current().unwrap());
        state.checked_at = Some(Utc::now());
        state.host_active = false;
        write(&dir, &state).unwrap();
        env.set_key(seat_authority::GRANT_ENV, &child.id);
        env.set_key("AIDA_USER", &child.subject);
        env.set_key("AIDA_AGENT_NAME", &child.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id.clone()));
        let started = std::time::Instant::now();
        let result = idle(root, 30);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        assert!(result.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        // Ordinary watchers keep plain interval sleeps.
        assert!(idle(root, 0).is_ok());
    }

    // Status reports the kernel-held worker image against the live image for
    // both services: matching, promoted, deleted, and unavailable.
    // trace:BUG-2645 | ai:claude
    #[test]
    #[cfg(target_os = "linux")]
    fn bug2645_status_reports_worker_and_live_binary() {
        use sha2::Digest;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let sha = |path: &Path| {
            std::fs::read(path)
                .ok()
                .map(|bytes| format!("{:x}", sha2::Sha256::digest(bytes))[..12].to_string())
        };
        let digest = |path: &str| sha(Path::new(path)).unwrap();
        let live = root.join("aida");
        let image = root.join("worker-image");

        // No worker: worker fields are null even with a live image.
        promote_image("/bin/sh", &live);
        let state = read(&dir).unwrap().unwrap();
        let value = binary_status(Some(&state), &live, sha);
        assert_eq!(value["worker_binary_sha"], serde_json::Value::Null);
        assert_eq!(value["live_binary_sha"], digest("/bin/sh"));
        assert_eq!(value["binary_stale"], serde_json::Value::Null);
        assert_eq!(
            binary_line(&value),
            format!(
                "binary: worker null; live \"{}\"; stale null",
                digest("/bin/sh")
            )
        );
        assert_eq!(
            binary_status(None, &live, sha)["worker_binary_sha"],
            serde_json::Value::Null
        );

        // Matching: the worker runs the live inode.
        std::fs::hard_link(&live, &image).unwrap();
        let mut slot = Some(spawn_retrying(&mut image_command(&image)));
        let reap = Reap(&mut slot);
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::child(reap.0.as_ref().unwrap()).unwrap());
        write(&dir, &state).unwrap();
        {
            let value = binary_status(Some(&state), &live, sha);
            assert_eq!(value["worker_binary_sha"], digest("/bin/sh"));
            assert_eq!(value["live_binary_sha"], digest("/bin/sh"));
            assert_eq!(value["binary_stale"], false);
            assert!(binary_line(&value).ends_with("stale false"));

            // Promoted: a different live image; the worker keeps its own.
            promote_image("/bin/true", &live);
            let value = binary_status(Some(&state), &live, sha);
            assert_eq!(value["worker_binary_sha"], digest("/bin/sh"));
            assert_eq!(value["live_binary_sha"], digest("/bin/true"));
            assert_eq!(value["binary_stale"], true);
            assert_eq!(
                binary_line(&value),
                format!(
                    "binary: worker \"{}\"; live \"{}\"; stale true",
                    digest("/bin/sh"),
                    digest("/bin/true")
                )
            );

            // Deleted worker image: still read through the kernel-held link.
            std::fs::remove_file(&image).unwrap();
            let value = binary_status(Some(&state), &live, sha);
            assert_eq!(value["worker_binary_sha"], digest("/bin/sh"));
            assert_eq!(value["binary_stale"], true);

            // Live image unavailable: never reported as current.
            std::fs::remove_file(&live).unwrap();
            let value = binary_status(Some(&state), &live, sha);
            assert_eq!(value["live_binary_sha"], serde_json::Value::Null);
            assert_eq!(value["binary_stale"], serde_json::Value::Null);
            assert_eq!(value["worker_binary_sha"], digest("/bin/sh"));
        }
        drop(reap);
        // Exited worker: no image to report.
        let value = binary_status(Some(&state), &live, sha);
        assert_eq!(value["worker_binary_sha"], serde_json::Value::Null);
        assert_eq!(value["binary_stale"], serde_json::Value::Null);
    }

    // trace:BUG-2644 | ai:codex
    #[test]
    fn bug2644_supervised_watcher_honors_project_concurrency() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let args = worker_review_watch_args();
        assert_eq!(
            resolve_max_concurrent(root, args.max_concurrent).unwrap(),
            2
        );
        std::fs::create_dir(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[review_watch]\nmax_concurrent = 5\n",
        )
        .unwrap();
        assert_eq!(
            resolve_max_concurrent(root, args.max_concurrent).unwrap(),
            5
        );
        std::fs::write(root.join(".aida/config.toml"), "[review_watch]\n").unwrap();
        assert_eq!(
            resolve_max_concurrent(root, args.max_concurrent).unwrap(),
            2
        );
    }

    // A change refused mid-pass (here one that closed after discovery) is
    // recorded and skipped; the open change after it is admitted in the same
    // pass, and status then reports the running worker as degraded, naming it.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_refused_change_never_blocks_the_next_and_degrades_status() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        let change = |id: u64| forge::Change {
            id,
            head: format!("{id:040x}"),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            conflict: None,
            behind: Some(0),
            head_branch: None,
        };
        let mut admitted = Vec::new();
        let changes = vec![change(1), change(3), change(2)];
        let watched = super::super::watch_changes(&store, changes, |c| match c.id {
            1 => anyhow::bail!("change is closed or draft; no review started"),
            3 => Err(std::io::Error::from(std::io::ErrorKind::WouldBlock).into()),
            _ => {
                let action = super::super::select(&store, &c, None, 2)?;
                assert!(matches!(action, super::super::Action::Dispatch(_)));
                admitted.push(c.key());
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(admitted, ["PR-2"]);
        assert_eq!(watched.busy, 1);
        assert_eq!(watched.refused.len(), 1);
        assert!(watched.refused[0].starts_with("PR-1: refused: change is closed"));
        let counts = PassCounts {
            changes: 3,
            dispatched: 1,
            unadmitted: Vec::new(),
            busy: watched.busy,
            ..Default::default()
        };

        // Without a controller (a manual watcher) nothing is recorded.
        assert!(controller_pass_report(root, &counts, &watched.refused)
            .unwrap()
            .is_none());
        let dir = directory(root, true).unwrap().unwrap();
        assert!(pass_report(&dir).unwrap().is_none());

        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        controller_pass_report(root, &counts, &watched.refused).unwrap();
        let report = pass_report(&dir);
        let pass = report.clone().unwrap().unwrap();
        assert_eq!(
            (pass.changes, pass.dispatched, pass.busy, pass.refused),
            (3, 1, 1, 1)
        );
        // It dispatched, so it is not part of a stuck run.
        assert_eq!(pass.stuck_since, None);
        let degraded = pass_degradation(&report, Some(pass.at), Utc::now());
        assert_eq!(degraded.len(), 1);
        assert!(degraded[0].contains("PR-1"), "{degraded:?}");
        assert_eq!(pass_status("running", &degraded), "degraded");
        // Only a live worker is downgraded; other states keep their meaning.
        assert_eq!(
            pass_status("blocked-authority", &degraded),
            "blocked-authority"
        );

        // A clean pass restores health.
        let clean = PassCounts {
            changes: 1,
            ..Default::default()
        };
        controller_pass_report(root, &clean, &[]).unwrap();
        let report = pass_report(&dir);
        assert!(pass_degradation(&report, None, Utc::now()).is_empty());
        assert_eq!(pass_status("running", &[]), "running");
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Every pass, quiet or not, writes its own timestamped service.log line
    // with the dispatch count. trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_every_quiet_pass_logs_its_dispatch_count() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let quiet = PassCounts {
            changes: 10,
            ..Default::default()
        };
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        let lines: Vec<String> = (0..3)
            .map(|_| controller_pass_report(root, &quiet, &[]).unwrap().unwrap())
            .collect();
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        for line in &lines {
            let (at, rest) = line.split_once(' ').unwrap();
            assert!(DateTime::parse_from_rfc3339(at).is_ok(), "{line}");
            assert!(
                rest.contains("pass: 10 open change(s), 0 dispatched"),
                "{line}"
            );
        }
    }

    // More refusals than the diagnostic sample keeps: the total survives and
    // the omission is named. trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_refusal_total_survives_the_capped_sample() {
        let errors: Vec<String> = (1..=20).map(|n| format!("PR-{n}: refused: x")).collect();
        let report = next_pass_report(
            None,
            &PassCounts {
                changes: 20,
                ..Default::default()
            },
            &errors,
            Utc::now(),
        );
        assert_eq!(report.refused, 20);
        assert_eq!(report.errors.len(), PASS_REPORT_ERRORS);
        assert!(pass_log_line(&report).ends_with("20 refused"));
        let degraded = pass_degradation(&Ok(Some(report)), None, Utc::now());
        assert!(
            degraded
                .iter()
                .any(|r| r.contains("4 more refusal(s) not shown (20 in total)")),
            "{degraded:?}"
        );
    }

    // Fresh passes that keep finding eligible work and dispatch nothing are a
    // stall even though each pass completes; capacity waits and settled heads
    // are not eligible work, and one dispatch ends the run.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_repeated_zero_dispatch_passes_with_eligible_work_degrade() {
        let start = Utc::now() - chrono::Duration::minutes(PASS_STALE_MINUTES + 5);
        let blocked = PassCounts {
            changes: 10,
            busy: 2,
            ..Default::default()
        };
        // One pass every ten seconds, each fresh and error-free.
        let mut report: Option<PassReport> = None;
        let mut at = start;
        while at < Utc::now() {
            report = Some(next_pass_report(report.as_ref(), &blocked, &[], at));
            at += chrono::Duration::seconds(10);
        }
        let report = report.unwrap();
        assert_eq!(report.stuck_since, Some(start));
        let now = report.at;
        let degraded = pass_degradation(&Ok(Some(report.clone())), Some(start), now);
        assert_eq!(degraded.len(), 1, "{degraded:?}");
        assert!(
            degraded[0].starts_with("idle ") && degraded[0].contains("min with eligible work"),
            "{degraded:?}"
        );
        assert_eq!(pass_status("running", &degraded), "degraded");
        // Within the threshold the same run is not yet degraded.
        let young = next_pass_report(None, &blocked, &[], now);
        assert!(pass_degradation(&Ok(Some(young)), Some(start), now).is_empty());

        // A pass that waits on capacity or finds every head settled has no
        // eligible work, and ends the run; so does a pass that dispatches.
        let waiting = PassCounts {
            changes: 10,
            ..Default::default()
        };
        let next = next_pass_report(Some(&report), &waiting, &[], now);
        assert_eq!(next.stuck_since, None);
        assert!(pass_degradation(&Ok(Some(next)), Some(start), now).is_empty());
        let progressed = PassCounts {
            dispatched: 1,
            ..blocked.clone()
        };
        let next = next_pass_report(Some(&report), &progressed, &[], now);
        assert_eq!(next.stuck_since, None);
    }

    // A live worker with no valid report of its own is degraded once its
    // start deadline passes; a fresh one gets a grace period. A predecessor's
    // report does not vouch for it, and an unreadable report is surfaced.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_worker_without_a_pass_report_degrades_after_its_grace() {
        let now = Utc::now();
        let old = now - chrono::Duration::minutes(PASS_STALE_MINUTES + 5);
        let fresh = now - chrono::Duration::minutes(1);
        let degraded = pass_degradation(&Ok(None), Some(old), now);
        assert_eq!(degraded.len(), 1, "{degraded:?}");
        assert!(
            degraded[0].starts_with("no discovery pass since the worker started"),
            "{degraded:?}"
        );
        assert_eq!(pass_status("running", &degraded), "degraded");
        assert!(pass_degradation(&Ok(None), Some(fresh), now).is_empty());
        assert_eq!(pass_degradation(&Ok(None), None, now).len(), 1);

        // A clean report from before this worker started does not count.
        let predecessor = next_pass_report(None, &PassCounts::default(), &[], old);
        let report = Ok(Some(predecessor));
        assert!(pass_degradation(&report, Some(fresh), now).is_empty());
        let started = old + chrono::Duration::seconds(1);
        let degraded = pass_degradation(&report, Some(started), now);
        assert!(
            degraded[0].starts_with("no discovery pass since the worker started"),
            "{degraded:?}"
        );

        // A stalled worker whose own last pass is old is degraded too.
        let degraded = pass_degradation(&report, Some(old), now);
        assert!(
            degraded
                .iter()
                .any(|r| r.starts_with("no discovery pass for")),
            "{degraded:?}"
        );

        // Unreadable or malformed evidence is a reason, not silence.
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), true).unwrap().unwrap();
        dir.write(PASS_REPORT, b"{not json").unwrap();
        let report = pass_report(&dir);
        assert!(report.is_err());
        let degraded = pass_degradation(&report, Some(fresh), now);
        assert!(degraded[0].contains("unreadable"), "{degraded:?}");
    }

    // The admission hub refuses a head once; reserve_dispatch then persists
    // RemoteHeld, and every later pass reaches the head through
    // probe_unneeded. Each of those passes must still report the head as
    // unadmitted, keep its first-seen time, and, once the hold outlasts a
    // review, degrade status naming the PR and cause. A recorded review or a
    // carried approval settles it.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_remote_hold_stays_unadmitted_across_passes_and_degrades() {
        use super::super::{probe_unneeded, probe_unneeded_with_coverage, skipped_admission};
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
        let change = forge::Change {
            id: 3793,
            head: format!("{:040x}", 7),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            conflict: None,
            behind: Some(0),
            head_branch: None,
        };
        let super::super::Action::Dispatch(attempt) =
            super::super::select(&store, &change, None, 2).unwrap()
        else {
            panic!("an unreviewed head is dispatched");
        };
        // What reserve_dispatch records when the hub refuses the claim.
        store
            .transaction(&change.key(), |state| {
                super::super::attempt_mut(state, &change.key(), &attempt.token)
                    .unwrap()
                    .phase = super::super::Phase::RemoteHeld;
                Ok(())
            })
            .unwrap();

        let start = Utc::now() - chrono::Duration::minutes(HELD_STALE_MINUTES + 5);
        let mut report: Option<PassReport> = None;
        let mut at = start;
        while at < Utc::now() {
            // The production selection each pass runs for this head.
            let reason = probe_unneeded(&store, &change).unwrap().unwrap();
            let super::super::Admission::Unadmitted(cause) = skipped_admission(&store, &reason) else {
                panic!("an unresolved remote hold is not settled: {reason}");
            };
            let counts = PassCounts {
                changes: 1,
                unadmitted: vec![Unadmitted {
                    key: change.key(),
                    head: change.head.clone(),
                    cause,
                }],
                ..Default::default()
            };
            report = Some(next_pass_report(report.as_ref(), &counts, &[], at));
            at += chrono::Duration::minutes(1);
        }
        let report = report.unwrap();
        assert_eq!(report.unadmitted, 1);
        assert_eq!(report.held.len(), 1);
        assert_eq!(report.held[0].since, start);
        let now = report.at;
        let degraded = pass_degradation(&Ok(Some(report.clone())), Some(start), now);
        assert_eq!(degraded.len(), 1, "{degraded:?}");
        assert!(
            degraded[0].starts_with("PR-3793 head 000000000000 unadmitted for")
                && degraded[0].contains("held at the admission hub"),
            "{degraded:?}"
        );
        assert_eq!(pass_status("running", &degraded), "degraded");

        // Within the grace a hold may be a remote review in progress.
        let young = next_pass_report(
            None,
            &PassCounts {
                unadmitted: report
                    .held
                    .iter()
                    .map(|h| Unadmitted {
                        key: h.key.clone(),
                        head: h.head.clone(),
                        cause: h.cause.clone(),
                    })
                    .collect(),
                ..Default::default()
            },
            &[],
            now,
        );
        assert!(pass_degradation(&Ok(Some(young)), Some(start), now).is_empty());

        // A new head for the change starts its own hold.
        let pushed = PassCounts {
            unadmitted: vec![Unadmitted {
                key: change.key(),
                head: format!("{:040x}", 8),
                cause: "held".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            next_pass_report(Some(&report), &pushed, &[], now).held[0].since,
            now
        );

        // Coverage settles the same RemoteHeld head; it then drops out.
        let covered = probe_unneeded_with_coverage(&store, &change, || {
            Some("carried approval covers this head".into())
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            skipped_admission(&store, &covered),
            super::super::Admission::Settled
        );
        let settled = next_pass_report(Some(&report), &PassCounts::default(), &[], now);
        assert!(settled.held.is_empty());
        assert!(pass_degradation(&Ok(Some(settled)), Some(start), now).is_empty());
    }

    // The auto-train worker records no discovery passes; a long-lived healthy
    // one must not be degraded for lacking them. The review watcher is.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_long_lived_worker_pass_health_is_review_watch_only() {
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), true).unwrap().unwrap();
        let now = Utc::now();
        let started = now - chrono::Duration::hours(6);
        let (report, degraded) = pass_health(&dir, Some(started), now);
        assert!(report.is_none());
        if TRAIN {
            assert!(degraded.is_empty(), "{degraded:?}");
            assert_eq!(pass_status("running", &degraded), "running");
        } else {
            assert!(
                degraded[0].starts_with("no discovery pass since the worker started"),
                "{degraded:?}"
            );
        }
    }

    // A whole-watcher stop names itself, not a refusal of the change it hit.
    // trace:BUG-2674 | ai:claude
    #[test]
    fn bug2674_watcher_stop_is_not_reported_as_one_change_refusal() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
        let change = forge::Change {
            id: 3793,
            head: format!("{:040x}", 1),
            base: "main".into(),
            author: "writer".into(),
            gitlab: false,
            overlap: Some(false),
            conflict: None,
            behind: Some(0),
            head_branch: None,
        };
        let error = super::super::watch_changes(&store, vec![change], |_| {
            Err(anyhow::anyhow!("advisor loop controller binding superseded").context(GlobalState))
        })
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("review-watch pass stopped while processing PR-3793"),
            "{text}"
        );
        assert!(!text.contains("review watch refused"), "{text}");
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_synchronous_dispatch_failures_survive_idle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        for count in 1..=3 {
            controller_dispatch(root, Some("spawn refused"), false).unwrap();
            controller_pass(root, None).unwrap();
            assert_eq!(failure_state(root).unwrap().count, count);
        }
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        controller_dispatch(root, None, true).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_supervisor_restart_failure_preserves_pending_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("old-controller".into()));
        controller_dispatch(root, Some("spawn refused"), false).unwrap();
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        record_failure(root, Some("controller exited before successful dispatch")).unwrap();
        assert!(failure_state(root).unwrap().dispatch_pending);
        // A healthy supervisor tick is not evidence of a successful dispatch.
        record_failure(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 2);
        assert!(failure_state(root).unwrap().dispatch_pending);
        CONTROLLER.with(|id| *id.borrow_mut() = Some("replacement-controller".into()));
        controller_pass(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 2);
        controller_dispatch(root, None, true).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_three_synchronous_failures_in_one_pass_alert() {
        if TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        let changes = (1..=3)
            .map(|id| forge::Change {
                id,
                head: format!("{id:040x}"),
                base: "main".into(),
                author: "writer".into(),
                gitlab: false,
                overlap: Some(false),
                conflict: None,
                behind: Some(0),
                head_branch: None,
            })
            .collect();
        let mut observed = 0;
        super::super::watch_changes(&store, changes, |_| {
            observed += 1;
            let result = super::super::observe_dispatch_result(
                root,
                Err(anyhow::anyhow!("spawn {observed} refused")),
            );
            assert_eq!(failure_state(root).unwrap().count, observed);
            result
        })
        .unwrap();
        controller_pass(root, None).unwrap();
        let state = failure_state(root).unwrap();
        assert_eq!(state.count, 3);
        assert!(state.dispatch_pending);
        assert!(state.alert.unwrap().sent);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Reap actual asynchronous children, rather than simulating record_failure calls.
    // trace:STORY-2691 | ai:codex
    #[cfg(target_os = "linux")]
    #[test]
    fn story2691_failed_dispatch_children_raise_a_durable_alert() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Store::open(root, "github:github.com/owner/repo").unwrap();
        for batch in [2, 1] {
            let mut children = Vec::new();
            for _ in 0..batch {
                let mut child = std::process::Command::new("/bin/sh")
                    .args(["-c", "exit 23"])
                    .spawn()
                    .unwrap();
                child.wait().unwrap();
                children.push(super::super::Dispatcher {
                    child,
                    key: "PR-7".into(),
                    token: "missing".into(),
                });
            }
            let outcomes = super::super::reap_dispatchers(&store, &mut children);
            assert_eq!(outcomes.len(), batch);
            assert!(children.is_empty());
            for failure in outcomes {
                record_outcome(root, Some(&failure.unwrap()), Some(false)).unwrap();
            }
            record_outcome(root, None, None).unwrap();
        }
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        // Reloading the durable episode and an idle pass cannot erase it.
        record_outcome(root, None, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 3);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let mut children = vec![super::super::Dispatcher {
            child,
            key: "PR-7".into(),
            token: "missing".into(),
        }];
        let outcomes = super::super::reap_dispatchers(&store, &mut children);
        assert_eq!(outcomes, vec![None]);
        record_outcome(root, None, Some(true)).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_discovery_and_sync_failures_alert_then_recover() {
        if TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        for label in ["discovery", "synchronization", "discovery"] {
            assert!(super::super::observe_pass_input::<()>(
                root,
                label,
                Err(anyhow::anyhow!("authentication unavailable"))
            )
            .is_err());
        }
        assert_eq!(failure_state(root).unwrap().count, 3);
        assert!(failure_state(root).unwrap().alert.unwrap().sent);
        super::super::observe_pass_input(root, "discovery", Ok(())).unwrap();
        controller_pass(root, None).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Both fixed services exercise the same durable alert state machine.
    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_alert_once_per_failure_episode_survives_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for n in 1..=5 {
            record_failure(root, Some("dispatch authority refused")).unwrap();
            assert_eq!(failure_state(root).unwrap().count, n);
        }
        let first = failure_state(root).unwrap().alert.unwrap();
        assert!(first.sent);
        record_failure(root, Some("another failure")).unwrap();
        assert_eq!(
            failure_state(root).unwrap().alert.unwrap().message.id,
            first.message.id
        );
        let count = || {
            std::fs::read_dir(root.join(".aida/mailbox"))
                .unwrap()
                .count()
        };
        assert_eq!(count(), 1);
        record_outcome(root, None, Some(true)).unwrap();
        assert_eq!(failure_state(root).unwrap().count, 0);
        for _ in 0..3 {
            record_failure(root, Some("new episode")).unwrap();
        }
        assert_ne!(
            failure_state(root).unwrap().alert.unwrap().message.id,
            first.message.id
        );
        assert_eq!(count(), 2);
    }

    // A carried-over notice delivered after success must not suppress the next
    // episode's alert: each episode alerts exactly once with its own ID.
    // trace:STORY-2691 | ai:claude
    #[test]
    fn story2691_carried_alert_does_not_suppress_next_episode() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        let mailbox = root.join(".aida/mailbox");
        controller_dispatch(root, Some("failed"), false).unwrap();
        controller_dispatch(root, Some("failed"), false).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(&mailbox, "temporarily unavailable").unwrap();
        assert!(controller_dispatch(root, Some("failed"), false).is_err());
        let first = failure_state(root).unwrap().alert.unwrap();
        // Success resets the count but keeps the unsent notice pending.
        assert!(controller_dispatch(root, None, true).is_err());
        std::fs::remove_file(&mailbox).unwrap();
        // The next episode's first failure delivers the carried notice...
        controller_dispatch(root, Some("failed again"), false).unwrap();
        // ...and its third failure still raises a fresh alert of its own.
        controller_dispatch(root, Some("failed again"), false).unwrap();
        controller_dispatch(root, Some("failed again"), false).unwrap();
        let second = failure_state(root).unwrap().alert.unwrap();
        assert!(second.sent);
        assert_ne!(second.message.id, first.message.id);
        let mut ids: Vec<String> = std::fs::read_dir(&mailbox)
            .unwrap()
            .map(|entry| {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
                value["id"].as_str().unwrap().to_string()
            })
            .collect();
        ids.sort();
        let mut expected = vec![first.message.id.clone(), second.message.id.clone()];
        expected.sort();
        assert_eq!(ids, expected);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // An outage spanning two episodes' thresholds keeps both notices as
    // independent obligations: recovery delivers two distinct stable IDs
    // without a fourth failure in the second episode.
    // trace:STORY-2691 | ai:claude
    #[test]
    fn story2691_outage_across_two_thresholds_delivers_both_notices() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
        let mailbox = root.join(".aida/mailbox");
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(&mailbox, "temporarily unavailable").unwrap();
        // Episode A reaches its threshold while the mailbox is unavailable.
        controller_dispatch(root, Some("A"), false).unwrap();
        controller_dispatch(root, Some("A"), false).unwrap();
        assert!(controller_dispatch(root, Some("A"), false).is_err());
        let a = failure_state(root).unwrap().alert.unwrap();
        assert!(!a.sent);
        // Success carries unsent A.
        assert!(controller_dispatch(root, None, true).is_err());
        // Episode B reaches its threshold, still during the outage.
        for _ in 0..3 {
            assert!(controller_dispatch(root, Some("B"), false).is_err());
        }
        let saved = failure_state(root).unwrap();
        let b = saved.alert.clone().expect("B has its own durable notice");
        assert_eq!(saved.count, 3);
        assert!(!b.sent);
        assert_ne!(b.message.id, a.message.id);
        assert_eq!(saved.carried.len(), 1);
        assert_eq!(saved.carried[0].message.id, a.message.id);
        // A replacement controller retries the persisted obligations during
        // idle passes. Neither failed delivery may change IDs or require an
        // additional dispatch failure to recreate B's threshold notice.
        // trace:STORY-2691 | ai:codex
        CONTROLLER.with(|id| *id.borrow_mut() = Some("replacement".into()));
        for _ in 0..2 {
            assert!(controller_pass(root, None).is_err());
            let saved = failure_state(root).unwrap();
            assert_eq!(saved.count, 3);
            assert!(saved.dispatch_pending);
            assert_eq!(saved.carried.len(), 1);
            assert_eq!(saved.carried[0].message.id, a.message.id);
            let pending_b = saved.alert.unwrap();
            assert_eq!(pending_b.message.id, b.message.id);
            assert!(!pending_b.sent);
        }
        // Mailbox recovers; the next observation is a success.
        std::fs::remove_file(&mailbox).unwrap();
        controller_dispatch(root, None, true).unwrap();
        let saved = failure_state(root).unwrap();
        assert!(saved.carried.is_empty());
        assert!(saved.alert.is_none());
        assert_eq!(saved.count, 0);
        // Repeated success and idle observations must not publish duplicates.
        controller_dispatch(root, None, true).unwrap();
        controller_pass(root, None).unwrap();
        let mut ids: Vec<String> = std::fs::read_dir(&mailbox)
            .unwrap()
            .map(|entry| {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
                value["id"].as_str().unwrap().to_string()
            })
            .collect();
        ids.sort();
        let mut expected = vec![a.message.id, b.message.id];
        expected.sort();
        assert_eq!(ids, expected);
        CONTROLLER.with(|id| *id.borrow_mut() = None);
    }

    // Obligations accumulate rather than collapse: three threshold episodes in
    // one outage each keep a durable notice, whether recovery is observed as a
    // success or as the first failure of a fresh episode, and a repeat
    // observation after recovery never re-publishes or drops one.
    // trace:STORY-2691 | ai:claude
    #[test]
    fn story2691_outage_across_many_thresholds_keeps_every_notice() {
        for recover_with_failure in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            CONTROLLER.with(|id| *id.borrow_mut() = Some("fixture".into()));
            let mailbox = root.join(".aida/mailbox");
            std::fs::create_dir_all(root.join(".aida")).unwrap();
            std::fs::write(&mailbox, "temporarily unavailable").unwrap();
            let mut ids = Vec::new();
            for episode in ["A", "B", "C"] {
                for _ in 0..3 {
                    let _ = controller_dispatch(root, Some(episode), false);
                }
                let saved = failure_state(root).unwrap();
                let alert = saved.alert.expect("each episode persists its notice");
                assert!(!alert.sent);
                assert_eq!(saved.carried.len(), ids.len());
                ids.push(alert.message.id);
                assert!(controller_dispatch(root, None, true).is_err());
                assert_eq!(failure_state(root).unwrap().carried.len(), ids.len());
            }
            std::fs::remove_file(&mailbox).unwrap();
            if recover_with_failure {
                controller_dispatch(root, Some("D"), false).unwrap();
                let saved = failure_state(root).unwrap();
                assert_eq!(saved.count, 1);
                assert!(saved.alert.is_none());
            } else {
                controller_dispatch(root, None, true).unwrap();
            }
            assert!(failure_state(root).unwrap().carried.is_empty());
            controller_dispatch(root, None, true).unwrap();
            let mut delivered: Vec<String> = std::fs::read_dir(&mailbox)
                .unwrap()
                .map(|entry| {
                    let value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap())
                            .unwrap();
                    value["id"].as_str().unwrap().to_string()
                })
                .collect();
            delivered.sort();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), 3);
            assert_eq!(delivered, ids);
            CONTROLLER.with(|id| *id.borrow_mut() = None);
        }
    }

    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_unsent_alert_retries_after_restart_before_idle_or_success() {
        for success in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            CONTROLLER.with(|id| *id.borrow_mut() = Some("old".into()));
            controller_dispatch(root, Some("failed"), false).unwrap();
            controller_dispatch(root, Some("failed"), false).unwrap();
            let mailbox = root.join(".aida/mailbox");
            std::fs::write(&mailbox, "temporarily unavailable").unwrap();
            assert!(controller_dispatch(root, Some("failed"), false).is_err());
            let original = failure_state(root).unwrap().alert.unwrap();
            assert!(!original.sent);
            CONTROLLER.with(|id| *id.borrow_mut() = Some("replacement".into()));
            assert!(controller_pass(root, None).is_err());
            if success {
                assert!(controller_dispatch(root, None, true).is_err());
                let saved = failure_state(root).unwrap();
                assert_eq!(saved.count, 0);
                assert!(!saved.dispatch_pending);
                assert!(saved.alert.is_none());
                assert_eq!(saved.carried.len(), 1);
                assert_eq!(saved.carried[0].message.id, original.message.id);
            }
            std::fs::remove_file(&mailbox).unwrap();
            if success {
                controller_dispatch(root, None, true).unwrap();
            } else {
                controller_pass(root, None).unwrap();
                assert!(failure_state(root).unwrap().alert.unwrap().sent);
                controller_dispatch(root, None, true).unwrap();
            }
            controller_pass(root, None).unwrap();
            let entries: Vec<_> = std::fs::read_dir(&mailbox).unwrap().collect();
            assert_eq!(entries.len(), 1);
            let message: serde_json::Value = serde_json::from_slice(
                &std::fs::read(entries[0].as_ref().unwrap().path()).unwrap(),
            )
            .unwrap();
            assert_eq!(message["id"], original.message.id);
            assert_eq!(failure_state(root).unwrap().count, 0);
            CONTROLLER.with(|id| *id.borrow_mut() = None);
        }
    }

    // Rotation changes only the binding, never a live worker or its grant.
    // It observes supersession at the next pass, after any current train.
    // trace:STORY-2691 | ai:codex
    #[test]
    fn story2691_rotation_preserves_live_worker_until_pass_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let child =
            seat_authority::issue_review_watch_child(root, &parent.id, &parent.subject).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.child_grant = Some(child.id.clone());
        state.worker = Some(Identity::current().unwrap());
        state.host = state.worker.clone();
        state.checked_at = Some(Utc::now());
        write(&dir, &state).unwrap();
        let id = seat_authority::test_support::mint_grant_for(
            root,
            &parent.subject,
            "advisor",
            &["advisor", "reviewer"],
        );
        let next = seat_authority::read_grant(&id).unwrap();
        rebind(root, "default", &next).unwrap();
        let after = read(&dir).unwrap().unwrap();
        assert_eq!(after.child_grant, state.child_grant);
        assert_eq!(after.worker, state.worker);
        assert_eq!(after.binding.grant, next.id);
        env.set_key(seat_authority::GRANT_ENV, &child.id);
        env.set_key("AIDA_USER", &child.subject);
        env.set_key("AIDA_AGENT_NAME", &child.subject);
        CONTROLLER.with(|id| *id.borrow_mut() = Some(child.id.clone()));
        let refused = check_controller(root).unwrap_err();
        CONTROLLER.with(|id| *id.borrow_mut() = None);
        assert!(format!("{refused:#}").contains("superseded"));
        tick_with_launcher(root, "default", true, &mut None, |_, _, _| {
            panic!("must not spawn over live worker")
        })
        .unwrap();
        assert!(seat_authority::read_grant(&child.id).is_ok());
    }

    /// AC4: an active drain holds the supervisor's respawn (no launch, no
    /// failure), and once it ends the supervisor launches again.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_drain_holds_the_supervisor_respawn() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        crate::admission_drain::fixture_drain(root, "joe (uid 1)", "land #4324", 600, Utc::now())
            .unwrap();
        let held = tick_with_launcher(root, "default", true, &mut None, |_, _, _| {
            panic!("a drained loop must not be restarted")
        });
        assert_eq!(held.unwrap(), None);
        assert_eq!(failure_state(root).unwrap().count, 0);
        let dir = directory(root, false).unwrap().unwrap();
        assert!(read(&dir).unwrap().unwrap().child_grant.is_none());
        // An unreadable drain journal refuses rather than restarting.
        let journal = root.join(".aida/admission-owner/loops-drain.journal");
        let saved = std::fs::read(&journal).unwrap();
        std::fs::write(&journal, b"{broken").unwrap();
        let refused = tick_with_launcher(root, "default", true, &mut None, |_, _, _| {
            panic!("an unreadable drain must not restart the loop")
        })
        .unwrap_err();
        assert!(format!("{refused:#}").contains("drain"), "{refused:#}");
        std::fs::write(&journal, saved).unwrap();
        crate::admission_drain::resume(root).unwrap();
        let launched = std::cell::Cell::new(false);
        let error = tick_with_launcher(root, "default", true, &mut None, |_, _, _| {
            launched.set(true);
            anyhow::bail!("fixture launcher")
        })
        .unwrap_err();
        assert!(launched.get(), "after the drain the supervisor launches: {error:#}");
    }

    /// AC4: status shows the controller's claim and its age, the drain and
    /// any waiting land, ship or train.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_status_shows_claim_age_drain_and_waiters() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        state.worker = Some(Identity::current().unwrap());
        let value = admission_status(root, Some(&state), Utc::now());
        assert_eq!(value["admission_claim"], serde_json::Value::Null);
        assert!(
            admission_line(&value).contains(
                "claim review-watch not held; auto-train not held; drain none; waiting none"
            ),
            "{}",
            admission_line(&value)
        );
        let claim = crate::admission_owner::acquire(root, HOLDER_PURPOSE, true).unwrap();
        let later = Utc::now() + chrono::Duration::seconds(125);
        let value = admission_status(root, Some(&state), later);
        assert_eq!(value["admission_claim"]["pid"], std::process::id());
        assert!(value["admission_claim"]["age_seconds"].as_i64().unwrap() >= 125);
        assert!(
            admission_line(&value).contains(&format!("held by pid {} for 2m0", std::process::id())),
            "{}",
            admission_line(&value)
        );
        crate::admission_drain::fixture_drain(root, "joe (uid 1)", "land #4324", 600, Utc::now())
            .unwrap();
        let line = admission_line(&admission_status(root, Some(&state), Utc::now()));
        assert!(line.contains("drain by joe (uid 1) until"), "{line}");
        assert!(line.contains("land #4324"), "{line}");
        drop(claim);
    }

    /// Review finding 4 (round 2): `advisor loops status` (the review-watch
    /// service's command projection, human and JSON) shows BOTH workers'
    /// claims and ages: its own controller's and the auto-train's, whose
    /// legacy receipt (no acquisition time) shows an unknown age, not zero.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_loops_status_projects_both_workers_claims() {
        if TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(
            root,
            "advisor",
            &["advisor", "reviewer"],
        );
        let parent = seat_authority::grant_check(root).into_valid().unwrap();
        rebind(root, "default", &parent).unwrap();
        let dir = directory(root, false).unwrap().unwrap();
        let mut state = read(&dir).unwrap().unwrap();
        let own = Identity::current().unwrap();
        state.worker = Some(own.clone());
        let pid = own.pid;
        // Both workers hold admission: review-watch with a timed claim, the
        // auto-train with a receipt written before claims carried a time.
        let review = crate::admission_owner::acquire(root, HOLDER_PURPOSE, true).unwrap();
        let legacy = crate::admission_owner::write_test_receipt(
            root,
            aida_core::proc_identity::ProcIdentity::capture(pid).unwrap(),
            "legacy auto-train",
        );
        let value = status_value(root, "running", Some(&state), &[], None, &[]).unwrap();
        let claims = &value["admission_claims"];
        assert_eq!(claims["review-watch"]["pid"], pid);
        assert_eq!(claims["review-watch"]["controller"], pid);
        assert!(claims["review-watch"]["age_seconds"].as_i64().unwrap() >= 0);
        assert_eq!(claims["auto-train"]["pid"], pid);
        assert!(claims["auto-train"]["age_seconds"].is_null(), "{claims}");
        assert!(claims["auto-train"]["acquired_at"].is_null(), "{claims}");
        assert_eq!(value["admission_claim"]["pid"], pid);
        let lines = status_lines("running", &value, None, &[]);
        let line = lines
            .iter()
            .find(|l| l.starts_with("review-watch: admission:"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(
            line.contains(&format!("claim review-watch held by pid {pid} for 0m0")),
            "{line}"
        );
        assert!(
            line.contains(&format!("auto-train held by pid {pid} (age unknown)")),
            "{line}"
        );
        // Neither worker holding: both show as not held.
        drop(review);
        std::fs::remove_file(legacy).unwrap();
        let value = status_value(root, "running", Some(&state), &[], None, &[]).unwrap();
        assert!(value["admission_claims"]["review-watch"].is_null());
        assert!(value["admission_claims"]["auto-train"].is_null());
        assert!(status_lines("running", &value, None, &[])
            .iter()
            .any(|l| l.contains("claim review-watch not held; auto-train not held")));
    }

    /// AC2/AC3: a deputy host on a deleted or replaced binary is reported,
    /// with its name, pid, path, age and the fix; one on the live build (or
    /// on an identical copy of it) is not.
    // trace:BUG-2774 | ai:claude
    #[test]
    fn bug2774_a_deputy_on_a_stale_or_deleted_binary_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let file = |name: &str, body: &[u8]| {
            let path = temp.path().join(name);
            std::fs::write(&path, body).unwrap();
            path
        };
        let link = |name: &str, target: &Path| {
            let path = temp.path().join(name);
            std::os::unix::fs::symlink(target, &path).unwrap();
            path
        };
        let (live, copy, old) = (
            file("aida", b"live"),
            file("copy", b"live"),
            file("old", b"old"),
        );
        let sha = |p: &Path| std::fs::read(p).ok().map(|b| format!("{b:?}"));
        let now = std::time::SystemTime::now() + std::time::Duration::from_secs(7200);
        let probe = |exe: &Path| deputy_binary("default", 7, exe, &live, sha, now);
        assert!(probe(&link("live-exe", &live)).is_none());
        assert!(probe(&link("copy-exe", &copy)).is_none(), "same sha");

        let stale = probe(&link("old-exe", &old)).expect("a replaced image");
        assert_eq!(stale["deleted"], false);
        assert_eq!(stale["pid"], 7);
        assert!(stale["path"].as_str().unwrap().ends_with("/old"), "{stale}");
        assert!(stale["binary_age_seconds"].as_u64().unwrap() >= 7200);
        assert!(deputy_binary_line(&stale).contains("runs a stale binary"));

        let gone = Path::new(&format!("{} (deleted)", old.display())).to_path_buf();
        let deleted = probe(&link("gone-exe", &gone)).expect("the deleted marker");
        assert_eq!(deleted["deleted"], true);
        let value = serde_json::json!({ "deputy_binary": deleted });
        let lines = status_lines("running", &value, None, &[]);
        let line = lines
            .iter()
            .find(|l| l.contains("runs a deleted binary"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        for part in [
            &format!("{NAME}: deputy default (pid 7)"),
            "(age unknown)",
            "with the binding reset",
            "#review-watch-service-recovery",
            "`aida advisor loops rebind --deputy default`",
        ] {
            assert!(line.contains(part), "{part}: {line}");
        }
        let quiet = status_lines("running", &serde_json::json!({}), None, &[]);
        assert!(!quiet.iter().any(|l| l.contains("runs a")), "{quiet:?}");
    }

    // A receipt naming a live deputy on a deleted image yields a doctor
    // finding that names the deputy, pid, binary and the fix; once the deputy
    // is gone the finding clears. trace:TASK-3936 | ai:claude
    #[test]
    fn task3936_deputy_on_a_deleted_image_is_reported_with_pid_binary_and_fix() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut env = crate::test_env::AmbientGuard::hermetic(root, None);
        env.set_key("AIDA_DEV_LIVE_DIR", root);
        let (exe, image) = deleted_image_link(root);
        let at = |exe: PathBuf| move |_: u32| exe.clone();
        assert!(
            deputy_binary_finding_at(root, at(exe.clone())).is_none(),
            "no receipt"
        );
        // The test process hosts the receipt, so the host is alive anywhere.
        let pid = std::process::id();
        write_test_host_receipt(root, "night", pid);

        let finding = deputy_binary_finding_at(root, at(exe)).expect("a deputy on a deleted image");
        assert_eq!(finding.category, DEPUTY_BINARY_CATEGORY);
        assert_eq!(finding.id, format!("night:{pid}"));
        assert!(!finding.safe_heal);
        for part in [
            format!("deputy night (pid {pid}) runs a deleted binary"),
            format!("{} (deleted)", image.display()),
            format!("{NAME} blocks behind it"),
            "restart the deputy on the live build with the binding reset".into(),
            "`aida advisor loops rebind --deputy night`".into(),
        ] {
            assert!(finding.summary.contains(&part), "{part}: {}", finding.summary);
        }
        assert!(finding.action.contains("restart the deputy"), "{}", finding.action);
        assert!(finding.action.contains("rebind --deputy night"), "{}", finding.action);

        let live = root.join("aida");
        std::fs::write(&live, "live").unwrap();
        let on_live = root.join("live-exe");
        std::os::unix::fs::symlink(&live, &on_live).unwrap();
        assert!(
            deputy_binary_finding_at(root, at(on_live)).is_none(),
            "a deputy on the live build"
        );
    }
}

/// What one review-watch pass did with the open changes it considered.
// trace:BUG-2674 | ai:claude
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PassCounts {
    pub(super) skipped: std::collections::BTreeMap<String, Vec<String>>,
    pub(super) changes: usize,
    pub(super) dispatched: usize,
    /// Heads local state found eligible that the admission hub holds for
    /// another node, with no recorded verdict yet.
    pub(super) unadmitted: Vec<Unadmitted>,
    /// Changes an exited attempt holds behind a kept leftover review
    /// checkout, with why it was kept. trace:BUG-2776 | ai:claude
    pub(super) retained: Vec<Unadmitted>,
    /// Changes that found watcher admission busy.
    pub(super) busy: usize,
    pub(super) stopped: Option<String>,
}

/// One eligible head a pass could not admit, and why.
// trace:BUG-2674 | ai:claude
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Unadmitted {
    pub(super) key: String,
    pub(super) head: String,
    pub(super) cause: String,
}

/// An unadmitted head as the pass report keeps it: `since` is the first pass
/// that found this exact head held, carried while every later pass still
/// does, so a hold that never resolves ages instead of resetting.
// trace:BUG-2674 | ai:claude
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Held {
    key: String,
    head: String,
    cause: String,
    since: DateTime<Utc>,
}

/// The controller's latest completed (or stopped) discovery pass. Status uses
/// it to report a running worker as degraded rather than healthy.
// trace:BUG-2674 | ai:claude
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct PassReport {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    skipped: std::collections::BTreeMap<String, Vec<String>>,
    at: DateTime<Utc>,
    changes: usize,
    dispatched: usize,
    #[serde(default)]
    unadmitted: usize,
    #[serde(default)]
    busy: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopped: Option<String>,
    /// Every refusal in the pass; `errors` keeps a bounded sample.
    #[serde(default)]
    refused: usize,
    /// Start of the current run of passes that left eligible work refused,
    /// busy or stopped and dispatched nothing. A pass that dispatches, or
    /// finds nothing eligible, ends the run; a capacity wait or a settled
    /// head is not eligible work. A remote hold ages separately in `held`,
    /// since another node may be reviewing that head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stuck_since: Option<DateTime<Utc>>,
    /// Every head this pass found held at the admission hub, with when it
    /// was first found held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    held: Vec<Held>,
    /// Every change held behind a retained review checkout, with when a pass
    /// first found it so. trace:BUG-2776 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retained: Vec<Held>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}
const PASS_REPORT: &str = "last-pass.json";
const PASS_REPORT_ERRORS: usize = 16;
// The worker polls every ten seconds; this long without a pass, or with
// eligible work left unadmitted and nothing dispatched, is a stall.
const PASS_STALE_MINUTES: i64 = 15;
// A head held for another node with no verdict this long is lost work, not a
// remote review in progress. Longer than a review normally takes.
// trace:BUG-2674 | ai:claude
const HELD_STALE_MINUTES: i64 = 60;
// A review blocked this long by a kept reviewer checkout needs an operator.
// trace:BUG-2776 | ai:claude
const RETAINED_STALE_MINUTES: i64 = 30;

// trace:BUG-2674 | ai:claude
fn next_pass_report(
    previous: Option<&PassReport>,
    counts: &PassCounts,
    errors: &[String],
    now: DateTime<Utc>,
) -> PassReport {
    let stuck = counts.busy > 0 || !errors.is_empty() || counts.stopped.is_some();
    let stuck_since = if counts.dispatched > 0 || !stuck {
        None
    } else {
        Some(previous.and_then(|p| p.stuck_since).unwrap_or(now))
    };
    // A held head keeps its first-seen time only while the same head stays
    // held; a new head, or a pass that no longer finds it held, restarts it.
    let held = counts
        .unadmitted
        .iter()
        .map(|u| Held {
            key: u.key.clone(),
            head: u.head.clone(),
            cause: bounded_diagnostic(&u.cause),
            since: previous
                .and_then(|p| p.held.iter().find(|h| h.key == u.key && h.head == u.head))
                .map_or(now, |h| h.since),
        })
        .collect();
    // Aged by change: the block persists while the change waits on it.
    // trace:BUG-2776 | ai:claude
    let retained = counts
        .retained
        .iter()
        .map(|u| Held {
            key: u.key.clone(),
            head: u.head.clone(),
            cause: bounded_diagnostic(&u.cause),
            since: previous
                .and_then(|p| p.retained.iter().find(|h| h.key == u.key))
                .map_or(now, |h| h.since),
        })
        .collect();
    PassReport {
        at: now,
        retained,
        skipped: counts.skipped.clone(),
        changes: counts.changes,
        dispatched: counts.dispatched,
        unadmitted: counts.unadmitted.len(),
        busy: counts.busy,
        refused: errors.len(),
        stopped: counts.stopped.as_deref().map(bounded_diagnostic),
        stuck_since,
        held,
        errors: errors
            .iter()
            .take(PASS_REPORT_ERRORS)
            .map(|e| bounded_diagnostic(e))
            .collect(),
    }
}

// trace:BUG-2720 | ai:codex
fn skipped_summary(report: &PassReport) -> String {
    report
        .skipped
        .iter()
        .map(|(category, keys)| format!("; {} {category} ({})", keys.len(), keys.join(", ")))
        .collect()
}

/// The one bounded `service.log` line every pass writes.
// trace:BUG-2674 | ai:claude
fn pass_log_line(report: &PassReport) -> String {
    format!(
        "{} {NAME} pass: {} open change(s), {} dispatched, {} not admitted, {} busy, {} refused{}",
        report.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        report.changes,
        report.dispatched,
        report.unadmitted,
        report.busy,
        report.refused,
        skipped_summary(report)
    )
}

/// Records a controller pass: one timestamped `service.log` line (stderr is
/// the service log) and `last-pass.json`. Returns the logged line; a manual
/// watcher (no controller) records nothing.
// trace:BUG-2674 | ai:claude
pub(super) fn controller_pass_report(
    root: &Path,
    counts: &PassCounts,
    errors: &[String],
) -> Result<Option<String>> {
    if CONTROLLER.with(|id| id.borrow().is_none()) {
        return Ok(None);
    }
    let dir = directory(root, true)?.context("service directory missing")?;
    // An unreadable previous report only loses the stuck-run start.
    let previous = pass_report(&dir).ok().flatten();
    let report = next_pass_report(previous.as_ref(), counts, errors, Utc::now());
    let line = pass_log_line(&report);
    eprintln!("{line}");
    dir.write(PASS_REPORT, &serde_json::to_vec_pretty(&report)?)?;
    Ok(Some(line))
}

/// A report is diagnostics: failing to write it must not stop dispatch.
// trace:BUG-2674 | ai:claude
pub(super) fn record_pass(root: &Path, counts: &PassCounts, errors: &[String]) {
    if let Err(error) = controller_pass_report(root, counts, errors) {
        eprintln!("{NAME}: could not record the pass report: {error:#}");
    }
}

/// The last pass report; `Err` names why an existing one cannot be read.
// trace:BUG-2674 | ai:claude
fn pass_report(dir: &Directory) -> std::result::Result<Option<PassReport>, String> {
    let Some(bytes) = dir
        .read(PASS_REPORT)
        .map_err(|e| bounded_diagnostic(&format!("{e:#}")))?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| bounded_diagnostic(&e.to_string()))
}

/// Why a running worker is degraded: its pass report is unreadable; its last
/// pass refused changes or stopped; it has left eligible work unadmitted with
/// nothing dispatched for a sustained run; or it has not completed a pass
/// within the deadline counted from its last pass or, before any pass of its
/// own, from its start (a grace period for a fresh worker).
// trace:BUG-2674 | ai:claude
fn pass_degradation(
    report: &std::result::Result<Option<PassReport>, String>,
    worker_started: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let report = match report {
        Ok(report) => report.as_ref(),
        Err(error) => {
            return vec![format!(
                "pass report .aida/{NAME}/{PASS_REPORT} unreadable: {error}"
            )]
        }
    };
    let minutes = |since: DateTime<Utc>| now.signed_duration_since(since).num_minutes();
    // A report older than this worker belongs to its predecessor.
    let current = report.filter(|r| worker_started.is_none_or(|start| r.at >= start));
    let Some(report) = current else {
        return match worker_started {
            Some(start) if minutes(start) >= PASS_STALE_MINUTES => vec![format!(
                "no discovery pass since the worker started {} min ago",
                minutes(start)
            )],
            Some(_) => Vec::new(),
            None => vec!["no discovery pass recorded and the worker start time is unknown".into()],
        };
    };
    let mut reasons: Vec<String> = report
        .errors
        .iter()
        .map(|e| format!("last pass: {e}"))
        .collect();
    if let Some(stop) = &report.stopped {
        reasons.push(format!("last pass: {stop}"));
    }
    let omitted = report.refused.saturating_sub(report.errors.len());
    if omitted > 0 {
        reasons.push(format!(
            "last pass: {omitted} more refusal(s) not shown ({} in total)",
            report.refused
        ));
    }
    if let Some(since) = report.stuck_since {
        if minutes(since) >= PASS_STALE_MINUTES {
            reasons.push(format!(
                "idle {} min with eligible work: no review dispatched while {} change(s) found admission busy and {} were refused",
                minutes(since),
                report.busy,
                report.refused
            ));
        }
    }
    // trace:BUG-2674 | ai:claude
    for held in &report.held {
        if minutes(held.since) >= HELD_STALE_MINUTES {
            reasons.push(format!(
                "{} head {} unadmitted for {} min: {}",
                held.key,
                held.head.get(..12).unwrap_or(&held.head),
                minutes(held.since),
                held.cause
            ));
        }
    }
    reasons.extend(retained_blocks(report, now).into_iter().map(|(_, line)| line));
    if minutes(report.at) >= PASS_STALE_MINUTES {
        reasons.push(format!("no discovery pass for {} min", minutes(report.at)));
    }
    reasons
}

/// Each change held behind a retained reviewer checkout for at least
/// [`RETAINED_STALE_MINUTES`], with the fix. trace:BUG-2776 | ai:claude
fn retained_blocks(report: &PassReport, now: DateTime<Utc>) -> Vec<(String, String)> {
    report
        .retained
        .iter()
        // A capped retry will not try again: surface it now. trace:TASK-3921 | ai:claude
        .filter(|h| {
            now.signed_duration_since(h.since).num_minutes() >= RETAINED_STALE_MINUTES
                || h.cause.starts_with(crate::review_watch::PICKUP_RETRY_CAP)
        })
        .map(|h| {
            let branch = h.key.to_ascii_lowercase();
            let line = format!(
                "review blocked by a retained reviewer worktree: {} held {} min ({}); fix: inspect the `{branch}` checkout (`git worktree list`), and once it is safe to drop run `aida session end --branch {branch} --yes`, then `aida review watch --release {}`",
                h.key,
                now.signed_duration_since(h.since).num_minutes(),
                h.cause,
                h.key
            );
            (h.key.clone(), line)
        })
        .collect()
}

/// The pass evidence status reports, and why it degrades a live worker. Only
/// the review watcher records discovery passes; the auto-train worker runs
/// train passes with no such report, so its status is not held to one.
// trace:BUG-2674 | ai:claude
fn pass_health(
    dir: &Directory,
    worker_started: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> (Option<PassReport>, Vec<String>) {
    if TRAIN {
        return (None, Vec::new());
    }
    let report = pass_report(dir);
    let degraded = pass_degradation(&report, worker_started, now);
    (report.ok().flatten(), degraded)
}

/// A live worker is healthy only while its passes complete without refusals.
// trace:BUG-2674 | ai:claude
fn pass_status(status: &'static str, degraded: &[String]) -> &'static str {
    if status == "running" && !degraded.is_empty() {
        "degraded"
    } else {
        status
    }
}

// trace:STORY-2691 | ai:codex
pub(super) fn controller_pass(root: &Path, failure: Option<&str>) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        record_outcome(root, failure, None)?;
    }
    Ok(())
}

// A failed asynchronous dispatch remains an episode across controller restarts.
// Only a confirmed successful dispatch clears that part of the episode.
// trace:STORY-2691 | ai:codex
pub(super) fn controller_dispatch(root: &Path, failure: Option<&str>, success: bool) -> Result<()> {
    if CONTROLLER.with(|id| id.borrow().is_some()) {
        if failure.is_some() {
            record_outcome(root, failure, Some(false))?;
        } else if success {
            record_outcome(root, None, Some(true))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod task3924_waiter_status_tests {
    use super::*;

    /// TASK-3924 AC3: `train-status` shows one line per queued manual train,
    /// with its id, record file, expiry and release command; released, the
    /// line goes. trace:TASK-3924 trace:TASK-3924.AC1 trace:TASK-3924.AC3 | ai:claude
    #[test]
    fn task3924_train_status_lists_each_waiter_with_its_release_command() {
        if !TRAIN {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat(root, "advisor", &["advisor"]);
        let first = crate::admission_owner::record_test_waiter(root, &[4464]);
        crate::admission_owner::record_test_waiter(root, &[]);
        let value = status_value(root, "running", None, &[], None, &[]).unwrap();
        let lines = status_lines("running", &value, None, &[]);
        let waiting: Vec<_> = lines.iter().filter(|l| l.contains("waiting since")).collect();
        assert_eq!(waiting.len(), 2, "{lines:?}");
        assert!(waiting[0].starts_with(&format!("{NAME}: operator train for #4464 waiting since ")));
        assert!(waiting[1].contains("operator train for all ready PRs"), "{lines:?}");
        assert!(waiting[0].contains("manual-train.waiters, expires "), "{lines:?}");
        assert!(
            waiting[0].ends_with(&format!("release: aida advisor loops release-waiter {first})")),
            "{lines:?}"
        );
        crate::admission_owner::release_waiter(root, &first).unwrap();
        let value = status_value(root, "running", None, &[], None, &[]).unwrap();
        let lines = status_lines("running", &value, None, &[]);
        assert_eq!(lines.iter().filter(|l| l.contains("waiting since")).count(), 1);
        assert!(crate::admission_owner::release_waiter(root, &first).is_err());
    }
}
