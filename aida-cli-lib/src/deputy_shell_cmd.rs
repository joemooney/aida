//! `aida deputy shell`: the script deputy's verbs. `run` is the operator's
//! side (a human terminal, the `aida()` shell function, an advisor grant);
//! every other verb is the submitter's or a reader's.
// trace:STORY-2338 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::Duration;

#[path = "deputy_shell_cmd_args.rs"]
pub(crate) mod deputy_shell_cmd_args;
pub(crate) use deputy_shell_cmd_args::*;

use crate::deputy_cmd::HumanTerminal;
use crate::deputy_shell::{
    self as ds, JobState, JobView, Queue, ShellJob, OPERATOR_TTY, RC_WAIT_TIMEOUT,
};

/// How long `wait` treats a job dir with no state as a claim in progress.
// trace:TASK-2832 | ai:codex
const CLAIM_GRACE: Duration = Duration::from_secs(10);
/// The seats a refreshed grant may delegate unless `--delegate-seat` says.
const DEFAULT_DELEGATE_SEATS: [&str; 4] = ["advisor", "implementer", "reviewer", "integrator"];

pub(crate) fn run(args: DeputyShellArgs) -> Result<()> {
    let mut out = std::io::stdout();
    match args.command {
        DeputyShellCommand::Run(a) => start(&a),
        DeputyShellCommand::Submit(a) => {
            let q = ds::current_queue(&a.name)?;
            let main = main_root()?;
            let submitter = ds::attribution::capture(&main)?;
            // The live runner's seats (the operator's `--allow-seat`), else
            // the fixed default. Never the tracked config file.
            let seats = q
                .live_runner()
                .map(|r| r.submit_seats)
                .unwrap_or_else(ds::default_submit_seats);
            let rc = submit(&q, &a, submitter, &seats, &mut out)?;
            exit_with(rc, &mut out)
        }
        DeputyShellCommand::List(a) => list(&ds::current_queue(&a.name)?, &a, &mut out),
        DeputyShellCommand::Status(a) => status(&ds::current_queue(&a.name)?, &a.id, &mut out),
        DeputyShellCommand::Log(a) => log(&ds::current_queue(&a.name)?, &a, &mut out),
        DeputyShellCommand::Wait(a) => {
            let q = ds::current_queue(&a.name)?;
            let limit = match &a.timeout {
                Some(t) => Some(Duration::from_secs(parse_secs(t, "--timeout")?)),
                None => None,
            };
            let rc = wait(&q, &a.id, limit, &mut out)?;
            exit_with(rc, &mut out)
        }
        DeputyShellCommand::Cancel(a) => {
            let q = ds::current_queue(&a.name)?;
            let at_tty = HumanTerminal::ambient().refusal(false).is_none();
            cancel(
                &q,
                &a.id,
                crate::kernel::resolve_current_session_id().as_deref(),
                at_tty,
                &mut out,
            )
        }
        DeputyShellCommand::Pause(a) => pause(&ds::current_queue(&a.name)?, &mut out),
        DeputyShellCommand::Resume(a) => {
            if let Some(why) = HumanTerminal::ambient().refusal(false) {
                bail!("aida deputy shell resume refuses: {why}");
            }
            resume(&ds::current_queue(&a.name)?, &mut out)
        }
        DeputyShellCommand::Ack(a) => {
            let q = ds::current_queue(&a.name)?;
            let main = main_root()?;
            let session = std::env::var("AIDA_SESSION_ID")
                .ok()
                .filter(|s| !s.trim().is_empty());
            let grant = crate::gate::seat_authority::grant_check(&main).into_valid();
            let actor = ack_actor(grant.as_ref(), session.as_deref());
            ack(&q, &main, &a, session.as_deref(), &actor, &mut out)
        }
    }
}

fn main_root() -> Result<std::path::PathBuf> {
    crate::worktree_gc::find_main_worktree_root().context("aida deputy shell needs a git checkout")
}

fn exit_with(rc: i32, out: &mut dyn Write) -> Result<()> {
    out.flush()?;
    if rc != 0 {
        std::process::exit(rc.clamp(1, 255));
    }
    Ok(())
}

fn parse_secs(raw: &str, what: &str) -> Result<u64> {
    crate::presence::parse_duration_secs(raw)
        .with_context(|| format!("{what}: not a duration: {raw:?} (try 30m, 2h, 90s)"))
}

#[cfg(not(unix))]
fn start(_a: &ShellRunArgs) -> Result<()> {
    bail!("aida deputy shell needs a Unix terminal");
}

/// `aida deputy shell run`: the operator's side. Refuses exactly where the
/// command deputy's start refuses, plus without an advisor grant.
#[cfg(unix)]
fn start(a: &ShellRunArgs) -> Result<()> {
    use crate::deputy::worker::GrantState;
    use crate::gate::seat_authority::{grant_check, grant_check_id, GrantCheck};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    if let Some(why) = HumanTerminal::ambient().refusal(true) {
        bail!("aida deputy shell refuses to start: {why}");
    }
    let refresh = parse_secs(&a.refresh, "--refresh")?.max(60);
    let main = main_root()?;
    let grant = runner_grant(grant_check(&main))?;
    let q = ds::current_queue(&a.name)?;
    q.ensure()?;
    drop(ds::runner::acquire_lock(&q)?); // fail fast before the banner
    let config = ds::load_config(&main);
    let keep_days = a.keep_days.unwrap_or(config.keep_days).max(1);
    let submit_seats = submit_seats(&ds::default_submit_seats(), &a.allow_seat);
    let delegate: Vec<String> = if a.delegate_seat.is_empty() {
        DEFAULT_DELEGATE_SEATS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        a.delegate_seat.clone()
    };

    let now = Utc::now();
    let left = crate::gate::seat_authority::format_remaining(grant.expires_at - now);
    eprintln!("aida deputy shell {:?} — the script deputy", a.name);
    eprintln!(
        "  EVERY JOB RUNS AS YOU, AT THIS TERMINAL: each script counts as you at the keyboard."
    );
    eprintln!(
        "  grant:   seat {} for {} (expires in {left}); refreshed every {}",
        grant.seat,
        grant.principal,
        crate::gate::seat_authority::format_remaining(chrono::Duration::seconds(refresh as i64))
    );
    eprintln!("  queue:   {}", q.root.display());
    eprintln!("  submit:  seats {}", submit_seats.join(", "));
    eprintln!("  keep:    finished jobs for {keep_days} days (audit.jsonl is never truncated)");
    let mandate = crate::deputy_cmd::mandate_probe_with(
        a.keep_after_mandate,
        now,
        Arc::new(crate::gate::mandate::current_state),
    );
    if q.paused() {
        eprintln!("  PAUSED: claiming no jobs until `aida deputy shell resume`");
    }
    eprintln!(
        "  Advisor sessions submit with `aida deputy shell submit --name {} --file FILE --wait`. Ctrl-C (between jobs) stops it.",
        a.name
    );

    let main_for_probe = main.clone();
    type Cached = Option<(std::time::Instant, String, GrantState)>;
    let cache: Arc<Mutex<Cached>> = Arc::new(Mutex::new(None));
    let probe: ds::runner::GrantProbe = Arc::new(move |id: &str| {
        let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, cached_id, state)) = c.as_ref() {
            if cached_id == id && at.elapsed() < Duration::from_secs(10) {
                return state.clone();
            }
        }
        let state = match grant_check_id(&main_for_probe, id) {
            GrantCheck::Valid(g) => GrantState::Valid {
                expires_at: Some(g.expires_at),
            },
            other => GrantState::Invalid(
                other
                    .rejection_sentence()
                    .unwrap_or_else(|| "the seat grant is gone or expired".into()),
            ),
        };
        *c = Some((std::time::Instant::now(), id.to_string(), state.clone()));
        state
    });
    let main_for_mint = main.clone();
    let mint: ds::runner::Minter = Arc::new(move || {
        crate::gate::seat_authority::issue_runner_refresh(
            &main_for_mint,
            ds::RUNNER_SEAT,
            delegate.clone(),
        )
        .map(|g| g.id)
    });
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register(sig, Arc::clone(&stop))
            .context("install the runner's signal handler")?;
    }
    let cfg = ds::runner::RunnerConfig {
        project_root: main.clone(),
        grant_id: grant.id.clone(),
        grant: probe,
        mint,
        refresh: chrono::Duration::seconds(refresh as i64),
        submit_seats,
        keep_days,
        mandate,
        probes: ds::attribution::Probes::live(),
        clock: ds::system_clock(),
        poll: Duration::from_millis(500),
        kill_grace: Duration::from_secs(10),
        mail: true,
        terminal: Some(ds::runner::Terminal {
            input: 0,
            output: 1,
        }),
        job_home: crate::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")),
        bash: "bash".into(),
        max_jobs: None,
        max_rounds: None,
    };
    ds::runner::run(&q, &cfg, &stop)?;
    Ok(())
}

/// PURE: the grant `run` starts under, from this shell's grant check. It
/// refuses without a valid grant, and with a valid grant for any seat but the
/// advisor's.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
pub(crate) fn runner_grant(
    check: crate::gate::seat_authority::GrantCheck,
) -> Result<crate::gate::seat_authority::SeatGrant> {
    use crate::gate::seat_authority::GrantCheck;
    let grant = match check {
        GrantCheck::Valid(g) => g,
        other => bail!(
            "aida deputy shell refuses to start: {}",
            other.rejection_sentence().unwrap_or_else(|| {
                format!(
                    "this shell holds no seat grant. Run `aida role enter {}` here first",
                    ds::RUNNER_SEAT
                )
            })
        ),
    };
    let seat = crate::gate::dispatch_deps::canonical_role_name(&grant.seat);
    if seat != ds::RUNNER_SEAT {
        bail!(
            "aida deputy shell refuses to start: it runs under an {} grant and this shell holds a {seat} grant. Run `aida role enter {}` here first",
            ds::RUNNER_SEAT,
            ds::RUNNER_SEAT
        );
    }
    Ok(grant)
}

/// Who may submit: the default seats (the advisor) plus each `--allow-seat`
/// typed at the operator's terminal, canonical and deduplicated.
pub(crate) fn submit_seats(configured: &[String], extra: &[String]) -> Vec<String> {
    let mut seats: Vec<String> = configured
        .iter()
        .chain(extra)
        .map(|s| crate::gate::dispatch_deps::canonical_role_name(s))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    seats.retain(|s| seen.insert(s.clone()));
    seats
}

/// Queue a script and print its id; with `--wait`, print its transcript and
/// the result line and return the job's rc (the caller exits with it).
pub(crate) fn submit(
    q: &Queue,
    a: &ShellSubmitArgs,
    submitter: ds::Submitter,
    allowed_seats: &[String],
    out: &mut dyn Write,
) -> Result<i32> {
    if !allowed_seats.iter().any(|s| s == &submitter.seat) {
        bail!(
            "seat {} may not submit to script deputy {:?} (allowed: {}). The operator widens it with `aida deputy shell run --allow-seat {}`",
            submitter.seat,
            q.name,
            allowed_seats.join(", "),
            submitter.seat
        );
    }
    let script =
        std::fs::read_to_string(&a.file).with_context(|| format!("read {}", a.file.display()))?;
    let timeout_secs = parse_secs(&a.timeout, "--timeout")?.max(1);
    let job = ShellJob {
        schema: ds::SCHEMA,
        id: crate::deputy::new_job_id(),
        deputy: q.name.clone(),
        created_at: Utc::now(),
        sha256: ds::script_digest(&script),
        script,
        source: Some(
            std::fs::canonicalize(&a.file)
                .unwrap_or_else(|_| a.file.clone())
                .display()
                .to_string(),
        ),
        timeout_secs,
        spec: a.spec.clone(),
        submitter,
    };
    q.submit(&job)?;
    writeln!(out, "{}", job.id)?;
    if q.live_runner().is_none() {
        eprintln!(
            "no script deputy {:?} is running yet; the job waits until the operator starts `aida deputy shell run --name {}`",
            q.name, q.name
        );
    }
    if !a.wait {
        eprintln!(
            "queued for script deputy {:?}. Result: `aida deputy shell status {id}`; wait: `aida deputy shell wait {id}`",
            q.name,
            id = job.id
        );
        return Ok(0);
    }
    // The queue is serial, so allow for the jobs ahead of this one.
    let limit = Duration::from_secs(timeout_secs + 3600);
    wait(q, &job.id, Some(limit), out)
}

fn finished(state: &ds::ShellState) -> bool {
    matches!(state.state, JobState::Done | JobState::Cancelled)
}

/// Wait for job `id`; print its transcript and a result line, and return
/// its rc. When only the wait times out, say so and return 75.
pub(crate) fn wait(
    q: &Queue,
    id: &str,
    limit: Option<Duration>,
    out: &mut dyn Write,
) -> Result<i32> {
    wait_with_grace(q, id, limit, CLAIM_GRACE, out)
}

// Inject the claim grace so expiry regressions need not wait ten seconds.
// trace:TASK-2832 | ai:codex
pub(crate) fn wait_with_grace(
    q: &Queue,
    id: &str,
    limit: Option<Duration>,
    claim_grace: Duration,
    out: &mut dyn Write,
) -> Result<i32> {
    let begun = std::time::Instant::now();
    let mut last_note = std::time::Instant::now();
    let mut claiming_since: Option<std::time::Instant> = None;
    loop {
        let Some(view) = q.locate(id)? else {
            // Mid-claim the runner has moved the job out of `incoming/` into
            // its `jobs/<id>/` dir but not yet written `state.json`, so for a
            // moment it is in neither place. Keep polling through that window
            // (bounded, so an interrupted claim still errors) instead of
            // reporting a live job as missing.
            // trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
            // trace:TASK-2832 | ai:codex
            let since = *claiming_since.get_or_insert_with(std::time::Instant::now);
            if q.job_dir(id).is_dir() && since.elapsed() < claim_grace {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            bail!("no script job {id} in deputy {:?}", q.name);
        };
        claiming_since = None;
        if finished(&view.state) {
            if let Ok(bytes) = std::fs::read(q.transcript_path(id)) {
                out.write_all(&bytes)?;
            }
            let rc = view.state.rc.unwrap_or_default();
            writeln!(
                out,
                "script job {id}: {} rc {} ({}) — transcript: {}",
                view.state.state.as_str(),
                rc,
                view.state.reason.as_deref().unwrap_or("-"),
                q.transcript_path(id).display()
            )?;
            return Ok(rc);
        }
        if limit.is_some_and(|l| begun.elapsed() > l) {
            writeln!(
                out,
                "script job {id}: still {} after {}s; this is not a job failure. Check `aida deputy shell status {id}`",
                view.state.state.as_str(),
                begun.elapsed().as_secs()
            )?;
            return Ok(RC_WAIT_TIMEOUT);
        }
        if last_note.elapsed() > Duration::from_secs(60) {
            eprintln!(
                "script job {id}: still {} after {}s",
                view.state.state.as_str(),
                begun.elapsed().as_secs()
            );
            last_note = std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) fn status(q: &Queue, id: &str, out: &mut dyn Write) -> Result<()> {
    let Some(view) = q.locate(id)? else {
        bail!("no script job {id} in deputy {:?}", q.name);
    };
    let JobView {
        job,
        state,
        cancel_requested,
        ..
    } = &view;
    writeln!(out, "script job {id}: {}", state.state.as_str())?;
    if *cancel_requested && !finished(state) {
        writeln!(out, "  cancel:     requested")?;
    }
    if let Some(job) = job {
        let first = job.script.lines().next().unwrap_or("").trim();
        writeln!(out, "  script:     {first}")?;
        writeln!(out, "  sha256:     {}", job.sha256)?;
        writeln!(
            out,
            "  submitter:  {} · session {}",
            job.submitter.actor(),
            job.submitter.session_id.as_deref().unwrap_or("?")
        )?;
        if let Some(spec) = &job.spec {
            writeln!(out, "  spec:       {spec}")?;
        }
    }
    let ts = |t: Option<chrono::DateTime<Utc>>| t.map_or_else(|| "-".into(), |t| t.to_rfc3339());
    writeln!(out, "  queued:     {}", state.queued_at.to_rfc3339())?;
    writeln!(out, "  started:    {}", ts(state.started_at))?;
    writeln!(out, "  finished:   {}", ts(state.finished_at))?;
    if let Some(rc) = state.rc {
        writeln!(
            out,
            "  rc:         {rc} ({})",
            state.reason.as_deref().unwrap_or("-")
        )?;
    }
    if let Some(att) = &state.attribution {
        writeln!(
            out,
            "  attributed: {} [{}]",
            att.verdict,
            att.checks.join(", ")
        )?;
    }
    if state.failed() {
        writeln!(
            out,
            "  ack:        {}",
            if q.acknowledged(id) {
                "acknowledged".to_string()
            } else {
                format!("listed in `aida awaiting` until `aida deputy shell ack {id}`")
            }
        )?;
    }
    if state.state != JobState::Queued {
        writeln!(out, "  transcript: {}", q.transcript_path(id).display())?;
    }
    Ok(())
}

pub(crate) fn log(q: &Queue, a: &ShellLogArgs, out: &mut dyn Write) -> Result<()> {
    if q.locate(&a.id)?.is_none() {
        bail!("no script job {} in deputy {:?}", a.id, q.name);
    }
    let path = q.transcript_path(&a.id);
    let mut pos = 0u64;
    loop {
        if let Ok(mut f) = std::fs::File::open(&path) {
            f.seek(std::io::SeekFrom::Start(pos))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            pos += buf.len() as u64;
            out.write_all(&buf)?;
            out.flush()?;
        }
        let done = q.locate(&a.id)?.is_some_and(|v| finished(&v.state));
        if !a.follow || done {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// Ask the runner to cancel job `id`. Allowed for the session that
/// submitted it, or at the operator's own terminal.
pub(crate) fn cancel(
    q: &Queue,
    id: &str,
    session: Option<&str>,
    at_operator_tty: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let Some(view) = q.locate(id)? else {
        bail!("no script job {id} in deputy {:?}", q.name);
    };
    if finished(&view.state) {
        writeln!(
            out,
            "script job {id}: already {}",
            view.state.state.as_str()
        )?;
        return Ok(());
    }
    let owner = view
        .job
        .as_ref()
        .and_then(|j| j.submitter.session_id.clone());
    let own = session.is_some() && owner.as_deref() == session;
    if !own && !at_operator_tty {
        bail!(
            "script job {id} was submitted by {}, not this session; cancel it from that session or at the operator's terminal",
            owner.as_deref().unwrap_or("an unknown session")
        );
    }
    q.request_cancel(id)?;
    writeln!(
        out,
        "script job {id}: cancel requested; the runner stops it {}",
        if view.state.state == JobState::Queued {
            "before it runs"
        } else {
            "shortly"
        }
    )?;
    Ok(())
}

pub(crate) fn pause(q: &Queue, out: &mut dyn Write) -> Result<()> {
    if !q.root.is_dir() {
        bail!("no script deputy is set up at {}", q.root.display());
    }
    if q.pause()? {
        writeln!(out, "script deputy paused: it claims no new jobs; a running job finishes. Resume at your terminal with `aida deputy shell resume`.")?;
    } else {
        writeln!(out, "script deputy already paused")?;
    }
    Ok(())
}

pub(crate) fn resume(q: &Queue, out: &mut dyn Write) -> Result<()> {
    if q.resume()? {
        writeln!(
            out,
            "script deputy resumed: it claims jobs again at its next scan"
        )?;
    } else {
        writeln!(out, "script deputy was not paused")?;
    }
    Ok(())
}

/// PURE: the audit actor of an `ack`, in the `AuditRow::actor` form: the
/// acking shell's grant prefix and seat, else `operator-tty` outside any
/// session, else `- unattributed` (a session holding no valid grant).
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
pub(crate) fn ack_actor(
    grant: Option<&crate::gate::seat_authority::SeatGrant>,
    session: Option<&str>,
) -> String {
    match (grant, session) {
        (Some(g), _) => format!(
            "{} {}",
            g.id.chars().take(8).collect::<String>(),
            crate::gate::dispatch_deps::canonical_role_name(&g.seat)
        ),
        (None, None) => OPERATOR_TTY.to_string(),
        (None, Some(_)) => "- unattributed".to_string(),
    }
}

/// Acknowledge failed script jobs. `--all` takes every failed job `session`
/// submitted from this project (every one outside a session). Inside a
/// session, another session's job is refused, as for the command deputy.
pub(crate) fn ack(
    q: &Queue,
    main: &Path,
    a: &ShellAckArgs,
    session: Option<&str>,
    actor: &str,
    out: &mut dyn Write,
) -> Result<()> {
    let ids: Vec<String> = if a.all {
        ds::unacked_failed_jobs_in(&q.base, main, session)
            .into_iter()
            .filter(|j| j.deputy == q.name)
            .map(|j| j.id)
            .collect()
    } else {
        a.ids.clone()
    };
    let mut refused = 0usize;
    for id in &ids {
        let Some(view) = q.locate(id)? else {
            bail!("no script job {id} in deputy {:?}", q.name);
        };
        if !view.state.failed() {
            writeln!(
                out,
                "script job {id}: {}, not failed; nothing to acknowledge",
                view.state.state.as_str()
            )?;
            continue;
        }
        let owner = view
            .job
            .as_ref()
            .and_then(|j| j.submitter.session_id.clone());
        if let Some(me) = session {
            if owner.as_deref() != Some(me) {
                writeln!(
                    out,
                    "script job {id}: submitted by {}, not this session; not acknowledged (the operator's shell can)",
                    owner.as_deref().unwrap_or("an unknown session")
                )?;
                refused += 1;
                continue;
            }
        }
        q.acknowledge(id)?;
        q.audit_event(Utc::now(), "acked", Some(id), actor, |r| {
            r.detail = session.map(|s| serde_json::json!({ "session": s }));
        });
        writeln!(out, "script job {id}: acknowledged")?;
    }
    if ids.is_empty() {
        writeln!(out, "no failed script jobs to acknowledge")?;
    }
    if refused > 0 {
        bail!("{refused} job(s) belong to another session and were not acknowledged");
    }
    Ok(())
}

pub(crate) fn list(q: &Queue, a: &ShellListArgs, out: &mut dyn Write) -> Result<()> {
    if !q.root.exists() {
        writeln!(
            out,
            "script deputy {:?} has no queue yet (start it with `aida deputy shell run --name {}`)",
            q.name, q.name
        )?;
        return Ok(());
    }
    match q.live_runner() {
        Some(r) => writeln!(
            out,
            "runner: live, pid {} (grant {}…), submitters {}",
            r.pid,
            r.grant_prefix,
            r.submit_seats.join(", ")
        )?,
        None => writeln!(out, "runner: NOT running")?,
    }
    if q.paused() {
        writeln!(
            out,
            "PAUSED: claiming no new jobs (`aida deputy shell resume` at the operator's terminal)"
        )?;
    }
    let want = match a.state.as_deref() {
        None => None,
        Some(s @ ("queued" | "running" | "done" | "cancelled")) => Some(s.to_string()),
        Some(other) => {
            bail!("--state must be one of queued, running, done, cancelled (got {other:?})")
        }
    };
    let mut ids = q.queued_ids();
    ids.extend(q.job_ids());
    ids.sort();
    ids.dedup();
    ids.reverse();
    let mut shown = 0;
    for id in ids {
        if shown >= a.limit {
            break;
        }
        let Some(view) = q.locate(&id)? else {
            continue;
        };
        if want
            .as_deref()
            .is_some_and(|w| w != view.state.state.as_str())
        {
            continue;
        }
        let first = view
            .job
            .as_ref()
            .map(|j| j.script.lines().next().unwrap_or("").trim().to_string())
            .unwrap_or_else(|| "(unreadable)".into());
        let rc = view
            .state
            .rc
            .map(|rc| {
                format!(
                    "  rc {rc} ({})",
                    view.state.reason.as_deref().unwrap_or("-")
                )
            })
            .unwrap_or_default();
        let who = view
            .job
            .as_ref()
            .map(|j| j.submitter.actor())
            .unwrap_or_else(|| "?".into());
        writeln!(
            out,
            "  {id}  {:<9} {who}  {first}{rc}",
            view.state.state.as_str()
        )?;
        shown += 1;
    }
    if shown == 0 {
        writeln!(out, "  no jobs")?;
    }
    Ok(())
}
