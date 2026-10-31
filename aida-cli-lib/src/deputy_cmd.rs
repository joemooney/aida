//! `aida deputy` (hidden alias `aida runloop`): the operator's stand-in that
//! runs submitted jobs under the grant held by the operator's interactive
//! shell. The start path keeps aida's human-terminal check: it refuses without
//! a human TTY, under an agent or headless drain, outside the `aida()` shell
//! function, and without a seat grant. `submit` is the non-TTY side and never
//! gets the human-terminal attestation itself.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::io::{Read, Seek, Write};
use std::time::Duration;

#[path = "deputy_cmd_args.rs"]
pub(crate) mod deputy_cmd_args;
pub(crate) use deputy_cmd_args::*;

use crate::command_registry::{CmdPolicy, CommandReg, Handler};
use crate::deputy::{self, policy, JobBody, JobRecord, LedgerRow, Spool};
use crate::stakeholder_role::StakeholderAction;

/// What the start path and the TTY-policy edits check about the caller.
/// Gathered by [`HumanTerminal::ambient`]; plain data so the refusal table is
/// unit-testable.
#[derive(Debug, Clone)]
pub(crate) struct HumanTerminal {
    /// stdin, stdout, stderr.
    pub ttys: [bool; 3],
    pub agent: bool,
    pub headless: bool,
    /// Running as (or under) a deputy job: `AIDA_DEPUTY_JOB` is set.
    pub in_deputy_job: bool,
    /// The `aida()` shell function routed this call.
    pub via_wrapper: bool,
    /// `AIDA_SHELL_WRAPPER`, the installed function's capability list.
    pub wrapper_marker: Option<String>,
}

impl HumanTerminal {
    fn ambient() -> Self {
        use crate::gate::kernel_deps as k;
        let set = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
        Self {
            ttys: [
                k::authority_stdin_is_terminal(),
                k::authority_stdout_is_terminal(),
                k::authority_stderr_is_terminal(),
            ],
            agent: set("AIDA_AGENT_TYPE"),
            headless: std::env::var("AIDA_HEADLESS").as_deref() == Ok("1"),
            in_deputy_job: set(deputy::JOB_ENV),
            via_wrapper: std::env::var(deputy::VIA_WRAPPER_ENV).as_deref() == Ok("1"),
            wrapper_marker: std::env::var("AIDA_SHELL_WRAPPER").ok(),
        }
    }

    /// Why the deputy (or a TTY-policy edit) must not run here, if it must
    /// not. `need_wrapper` is false for `allow`/`deny`, which need the human
    /// terminal but not the shell function.
    pub(crate) fn refusal(&self, need_wrapper: bool) -> Option<String> {
        if self.in_deputy_job {
            return Some("this runs inside a deputy job; a job cannot start a deputy or change what may run on the operator's terminal".into());
        }
        if self.agent {
            return Some("this runs in an agent session (AIDA_AGENT_TYPE is set); only a person at their own terminal can do this".into());
        }
        if self.headless {
            return Some("this is a headless run (AIDA_HEADLESS=1); only a person at their own terminal can do this".into());
        }
        if self.ttys.iter().any(|t| !t) {
            return Some(
                "stdin, stdout and stderr must all be your terminal (nothing piped or redirected)"
                    .into(),
            );
        }
        if need_wrapper && !self.via_wrapper {
            let marker = self.wrapper_marker.as_deref();
            if marker.is_some() && !crate::shell_eval::marker_has_cap(marker, deputy::WRAPPER_CAP) {
                return Some("your `aida()` shell function predates `aida deputy`. Update it with `aida dev shell-init --install`, open a new shell (or source your shell profile again), then run `aida deputy`".into());
            }
            return Some("start it from your shell with `aida deputy`, so it runs under the `aida()` shell function and inherits that shell's grant (install the function with `aida dev shell-init --install`; `command aida deputy` bypasses it and is refused)".into());
        }
        None
    }
}

fn ambient_refusal(need_wrapper: bool) -> Option<String> {
    HumanTerminal::ambient().refusal(need_wrapper)
}

fn parse_secs(raw: &str, what: &str) -> Result<u64> {
    crate::presence::parse_duration_secs(raw)
        .with_context(|| format!("{what}: not a duration: {raw:?} (try 30m, 8h, 90s)"))
}

fn run_deputy(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> Result<()> {
    let args = <DeputyArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let mut out = std::io::stdout();
    match args.command {
        None => start(&args.start),
        Some(DeputyCommand::Submit(a)) => {
            let spool = deputy::current_spool(&a.name)?;
            let requester = deputy::current_requester(a.spec.clone());
            let rc = submit(&spool, &a, requester, &mut out)?;
            out.flush()?;
            if rc != 0 {
                std::process::exit(rc.clamp(1, 255));
            }
            Ok(())
        }
        Some(DeputyCommand::Status(a)) => status(&deputy::current_spool(&a.name)?, &a, &mut out),
        Some(DeputyCommand::Log(a)) => log(&deputy::current_spool(&a.name)?, &a, &mut out),
        Some(DeputyCommand::Cancel(a)) => cancel(&deputy::current_spool(&a.name)?, &a, &mut out),
        Some(DeputyCommand::List(a)) => list(&deputy::current_spool(&a.name)?, &a, &mut out),
        Some(DeputyCommand::Allow(a)) => allow(&a),
        Some(DeputyCommand::Deny(a)) => deny(&a),
        Some(DeputyCommand::Policy(a)) => show_policy(&deputy::current_spool(&a.name)?),
    }
}

#[cfg(not(unix))]
fn start(_a: &DeputyStartArgs) -> Result<()> {
    bail!("aida deputy needs a Unix terminal");
}

#[cfg(unix)]
fn start(a: &DeputyStartArgs) -> Result<()> {
    use crate::deputy::worker::{GrantState, WorkerConfig};
    use crate::gate::seat_authority::{grant_check, GrantCheck};
    use std::sync::{Arc, Mutex};

    if let Some(why) = ambient_refusal(true) {
        bail!("aida deputy refuses to start: {why}");
    }
    if a.workers == 0 || a.workers > 16 {
        bail!("--workers must be between 1 and 16");
    }
    let ttl = parse_secs(&a.ttl, "--ttl")?;
    let tty_idle = parse_secs(&a.tty_idle, "--tty-idle")?.max(1);
    let main = crate::worktree_gc::find_main_worktree_root()
        .context("aida deputy needs a git checkout")?;
    let grant = match grant_check(&main) {
        GrantCheck::Valid(g) => g,
        other => bail!(
            "aida deputy refuses to start: {}",
            other.rejection_sentence().unwrap_or_else(|| {
                "this shell holds no seat grant. Run `aida role enter <seat>` here first".into()
            })
        ),
    };
    let mut allow_exec = Vec::new();
    for p in &a.allow_exec {
        allow_exec.push(
            std::fs::canonicalize(p)
                .with_context(|| format!("--allow-exec {}: not found", p.display()))?,
        );
    }
    let spool = deputy::spool_for(&main, &a.name)?;
    spool.ensure()?;
    let lock_check = deputy::worker::acquire_lock(&spool)?;
    drop(lock_check); // run() takes it for good; this only fails fast before the banner

    let now = Utc::now();
    let mut pol = policy::load(&spool)?;
    pol.prune(now);
    for prefix in &a.allow_tty {
        let e = pol.allow(policy::parse_prefix(prefix), ttl, false, now)?;
        log_policy(&spool, &a.name, "allow", &e.label());
    }
    policy::save(&spool, &pol)?;

    let left = crate::gate::seat_authority::format_remaining(grant.expires_at - now);
    eprintln!("aida deputy {:?} — {} worker(s)", a.name, a.workers);
    eprintln!(
        "  grant: seat {} for {} (expires in {left})",
        grant.seat, grant.principal
    );
    eprintln!("  spool: {}", spool.root.display());
    eprintln!(
        "  jobs:  `aida` commands{}{}",
        if allow_exec.is_empty() {
            String::new()
        } else {
            format!(
                ", plus {}",
                allow_exec
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        if a.allow_scripts {
            "; SHELL SCRIPTS ALLOWED (--allow-scripts)"
        } else {
            ""
        }
    );
    print_policy(&pol, now, Some(grant.expires_at));
    eprintln!(
        "  idle:  a terminal job silent for {tty_idle}s shows as waiting on a prompt (it keeps running until its --timeout)"
    );
    eprintln!(
        "  Sessions submit with `aida deputy submit -- <command…>`. Ctrl-C stops the deputy."
    );

    let main_for_probe = main.clone();
    let cache: Arc<Mutex<Option<(std::time::Instant, GrantState)>>> = Arc::new(Mutex::new(None));
    let probe = Arc::new(move || {
        let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, state)) = c.as_ref() {
            if at.elapsed() < Duration::from_secs(10) {
                return state.clone();
            }
        }
        let state = match grant_check(&main_for_probe) {
            GrantCheck::Valid(g) => GrantState::Valid {
                expires_at: Some(g.expires_at),
            },
            other => GrantState::Invalid(
                other
                    .rejection_sentence()
                    .unwrap_or_else(|| "the seat grant is gone or expired".into()),
            ),
        };
        *c = Some((std::time::Instant::now(), state.clone()));
        state
    });
    let cfg = WorkerConfig {
        name: a.name.clone(),
        workers: a.workers,
        allow_exec,
        allow_scripts: a.allow_scripts,
        tty_idle: Duration::from_secs(tty_idle),
        poll: Duration::from_millis(500),
        grant: probe,
        max_jobs: None,
        aida_program: crate::resolve_aida_exe(),
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    deputy::worker::run(&spool, &cfg, &stop)?;
    Ok(())
}

fn log_policy(spool: &Spool, name: &str, event: &str, label: &str) {
    let mut row = LedgerRow::new(name, "-", "policy");
    row.summary = Some(format!("{event} {label}"));
    let _ = spool.ledger(&row);
}

fn print_policy(
    pol: &policy::Policy,
    now: chrono::DateTime<Utc>,
    grant_expiry: Option<chrono::DateTime<Utc>>,
) {
    let live = pol.live_entries(now);
    if live.is_empty() {
        eprintln!("  terminal: no TTY entries — every job runs without a terminal");
        return;
    }
    eprintln!("  terminal: jobs matching these prefixes run on a terminal:");
    for e in live {
        let until = match grant_expiry {
            Some(g) if g < e.expires_at => g,
            _ => e.expires_at,
        };
        let left = crate::gate::seat_authority::format_remaining(until - now);
        eprintln!(
            "    [{}] {}  ({left} left{})",
            e.id,
            e.label(),
            if e.single_use { ", single use" } else { "" }
        );
    }
}

/// Queue a job and print its id; with `--wait`, print its output and the
/// result line and return the job's rc (the caller exits with it). Writes
/// only under the spool's `incoming/`, the one place a contained session may
/// write, so the ledger's first row for the job is the deputy's `claimed`,
/// which names the requester.
pub(crate) fn submit(
    spool: &Spool,
    a: &DeputySubmitArgs,
    requester: deputy::Requester,
    out: &mut dyn Write,
) -> Result<i32> {
    let body = match (&a.file, &a.script, a.argv.is_empty()) {
        (Some(f), None, true) => JobBody::Script(
            std::fs::read_to_string(f).with_context(|| format!("read {}", f.display()))?,
        ),
        (None, Some(s), true) => JobBody::Script(s.clone()),
        (None, None, false) => JobBody::Argv(a.argv.clone()),
        _ => bail!("give the job as `-- <command…>`, `-f FILE` or `--script BODY` (exactly one)"),
    };
    let timeout_secs = parse_secs(&a.timeout, "--timeout")?.max(1);
    let rec = JobRecord {
        id: deputy::new_job_id(),
        created_at: Utc::now(),
        requester,
        serial: a.serial.clone(),
        timeout_secs,
        sha256: body.digest(),
        body,
    };
    spool.submit(&rec)?;
    writeln!(out, "{}", rec.id)?;
    if !a.wait {
        eprintln!(
            "queued for deputy {:?}. Result: `aida deputy status {}`; output: `aida deputy log {}`",
            a.name, rec.id, rec.id
        );
        return Ok(0);
    }
    let limit = Duration::from_secs(timeout_secs + 600);
    let res = wait_for(spool, &rec.id, limit)?;
    if let Ok(mut f) = std::fs::File::open(&res.output) {
        let mut buf = Vec::new();
        let _ = f.read_to_end(&mut buf);
        out.write_all(&buf)?;
    }
    writeln!(
        out,
        "job {}: rc {} ({}) — output: {}",
        res.id,
        res.rc,
        res.reason,
        res.output.display()
    )?;
    Ok(res.rc)
}

fn wait_for(spool: &Spool, id: &str, limit: Duration) -> Result<deputy::JobResult> {
    let begun = std::time::Instant::now();
    let mut last_note = std::time::Instant::now();
    loop {
        if let Some(r) = spool.read_result(id) {
            return Ok(r);
        }
        if begun.elapsed() > limit {
            bail!(
                "job {id} has no result after {}s; check `aida deputy status {id}` (is the deputy running?)",
                limit.as_secs()
            );
        }
        if last_note.elapsed() > Duration::from_secs(30) {
            let state = spool.locate(id)?.map(|(s, _)| s).unwrap_or("missing");
            eprintln!(
                "job {id}: still {state} after {}s",
                begun.elapsed().as_secs()
            );
            last_note = std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// "waiting on a prompt" when a running terminal job has gone quiet.
fn waiting_note(spool: &Spool, id: &str) -> Option<String> {
    spool.waiting(id).map(|w| {
        format!(
            "waiting on a prompt (no terminal output for {}s+, since {}); it keeps running until it ends or times out",
            w.idle_secs,
            w.since.format("%H:%M:%S UTC")
        )
    })
}

pub(crate) fn status(spool: &Spool, a: &DeputyIdArgs, out: &mut dyn Write) -> Result<()> {
    let Some((state, path)) = spool.locate(&a.id)? else {
        bail!("no job {} in deputy {:?}", a.id, a.name);
    };
    writeln!(out, "job {}: {state}", a.id)?;
    if state == "running" {
        if let Some(note) = waiting_note(spool, &a.id) {
            writeln!(out, "  state:     {note}")?;
        }
    }
    if state != "done" && state != "failed" && spool.cancel_requested(&a.id) {
        writeln!(out, "  cancel:    requested")?;
    }
    if let Ok(rec) = std::fs::read(&path)
        .map_err(anyhow::Error::from)
        .and_then(|b| serde_json::from_slice::<JobRecord>(&b).map_err(Into::into))
    {
        writeln!(out, "  command:   {}", rec.body.summary())?;
        writeln!(
            out,
            "  requester: session {} · role {} · seat {}{}",
            rec.requester.session_id.as_deref().unwrap_or("?"),
            rec.requester.role.as_deref().unwrap_or("?"),
            rec.requester.seat.as_deref().unwrap_or("?"),
            if rec.requester.grant_verified {
                " (grant verified)"
            } else {
                ""
            }
        )?;
        writeln!(out, "  submitted: {}", rec.created_at.to_rfc3339())?;
    }
    if let Some(r) = spool.read_result(&a.id) {
        writeln!(out, "  rc:        {} ({})", r.rc, r.reason)?;
        writeln!(out, "  terminal:  {}", if r.tty { "yes" } else { "no" })?;
        writeln!(out, "  output:    {}", r.output.display())?;
    } else if state == "running" {
        writeln!(out, "  output:    {}", spool.out_path(&a.id).display())?;
    }
    Ok(())
}

pub(crate) fn log(spool: &Spool, a: &DeputyLogArgs, out: &mut dyn Write) -> Result<()> {
    if spool.locate(&a.id)?.is_none() {
        bail!("no job {} in deputy {:?}", a.id, a.name);
    }
    let path = spool.out_path(&a.id);
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
        if !a.follow || spool.read_result(&a.id).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if a.follow {
        if let Some(r) = spool.read_result(&a.id) {
            eprintln!("job {}: rc {} ({})", r.id, r.rc, r.reason);
        }
    }
    Ok(())
}

/// Ask the deputy to cancel a queued or running job. The request is a file
/// under `incoming/` (the only part of the spool a contained session may
/// write); the deputy acts on it at its next scan.
pub(crate) fn cancel(spool: &Spool, a: &DeputyIdArgs, out: &mut dyn Write) -> Result<()> {
    match spool.locate(&a.id)? {
        None => bail!("no job {} in deputy {:?}", a.id, a.name),
        Some((state @ ("incoming" | "running"), _)) => {
            spool.request_cancel(&a.id)?;
            let when = if state == "incoming" {
                "before it runs"
            } else {
                "shortly"
            };
            writeln!(
                out,
                "job {}: cancel requested; the deputy stops it {when}",
                a.id
            )?;
        }
        Some((state, _)) => writeln!(out, "job {}: already {state}", a.id)?,
    }
    Ok(())
}

pub(crate) fn list(spool: &Spool, a: &DeputyListArgs, out: &mut dyn Write) -> Result<()> {
    if !spool.root.exists() {
        writeln!(
            out,
            "deputy {:?} has no spool yet (start it with `aida deputy`)",
            a.name
        )?;
        return Ok(());
    }
    let states: Vec<&str> = match &a.state {
        Some(s) if deputy::STATES.contains(&s.as_str()) => vec![s.as_str()],
        Some(s) => bail!("--state must be one of incoming, running, done, failed (got {s:?})"),
        None => deputy::STATES.to_vec(),
    };
    for state in states {
        let mut ids = spool.list_ids(state);
        ids.reverse();
        writeln!(out, "{state} ({}):", ids.len())?;
        for id in ids.into_iter().take(a.limit) {
            let summary = std::fs::read(spool.job_path(state, &id))
                .ok()
                .and_then(|b| serde_json::from_slice::<JobRecord>(&b).ok())
                .map(|r| r.body.summary())
                .unwrap_or_else(|| "(unreadable)".into());
            let rc = spool
                .read_result(&id)
                .map(|r| format!("  rc {} ({})", r.rc, r.reason))
                .unwrap_or_default();
            let waiting = if state == "running" && spool.waiting(&id).is_some() {
                "  (waiting on a prompt)"
            } else {
                ""
            };
            writeln!(out, "  {id}  {summary}{rc}{waiting}")?;
        }
    }
    Ok(())
}

fn allow(a: &DeputyAllowArgs) -> Result<()> {
    if let Some(why) = ambient_refusal(false) {
        bail!("aida deputy allow refuses: {why}");
    }
    let ttl = parse_secs(&a.ttl, "--ttl")?;
    let spool = deputy::current_spool(&a.name)?;
    spool.ensure()?;
    let now = Utc::now();
    let mut pol = policy::load(&spool)?;
    pol.prune(now);
    let e = pol.allow(policy::parse_prefix(&a.prefix), ttl, a.single_use, now)?;
    policy::save(&spool, &pol)?;
    log_policy(&spool, &a.name, "allow", &e.label());
    println!(
        "[{}] {} may run on a terminal until {}",
        e.id,
        e.label(),
        e.expires_at.to_rfc3339()
    );
    Ok(())
}

fn deny(a: &DeputyDenyArgs) -> Result<()> {
    if let Some(why) = ambient_refusal(false) {
        bail!("aida deputy deny refuses: {why}");
    }
    let spool = deputy::current_spool(&a.name)?;
    let mut pol = policy::load(&spool)?;
    let n = pol.deny(&a.entry);
    policy::save(&spool, &pol)?;
    log_policy(&spool, &a.name, "deny", &a.entry);
    println!("removed {n} TTY entr{}", if n == 1 { "y" } else { "ies" });
    Ok(())
}

fn show_policy(spool: &Spool) -> Result<()> {
    let pol = policy::load(spool)?;
    let now = Utc::now();
    let live = pol.live_entries(now);
    if live.is_empty() {
        println!("no live TTY entries: every job runs without a terminal");
    }
    for e in live {
        println!(
            "[{}] {}  until {}{}",
            e.id,
            e.label(),
            e.expires_at.to_rfc3339(),
            if e.single_use { " (single use)" } else { "" }
        );
    }
    Ok(())
}

// The deputy touches only its own spool, never the requirement store.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) static DEPUTY: CommandReg = CommandReg {
    name: "deputy",
    help_after: "gate",
    command: <DeputyArgs as clap::CommandFactory>::command,
    policy: |_| CmdPolicy {
        stakeholder: StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: true,
        team_identity_write: false,
    },
    run: Handler::Early(run_deputy),
};
