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

/// Why the deputy (or a TTY-policy edit) must not start here, if it must not.
/// Pure so the refusal table is unit-testable. `need_wrapper` is false for
/// `allow`/`deny`, which only need the human terminal.
pub(crate) fn human_terminal_refusal(
    ttys: [bool; 3],
    agent: bool,
    headless: bool,
    via_wrapper: bool,
    need_wrapper: bool,
) -> Option<String> {
    if agent {
        return Some("this runs in an agent session (AIDA_AGENT_TYPE is set); only a person at their own terminal can do this".into());
    }
    if headless {
        return Some("this is a headless run (AIDA_HEADLESS=1); only a person at their own terminal can do this".into());
    }
    if ttys.iter().any(|t| !t) {
        return Some(
            "stdin, stdout and stderr must all be your terminal (nothing piped or redirected)"
                .into(),
        );
    }
    if need_wrapper && !via_wrapper {
        return Some("start it from your shell with `aida deputy`, so it runs under the `aida()` shell function and inherits that shell's grant (install the function with `aida dev shell-init --install`; `command aida deputy` bypasses it and is refused)".into());
    }
    None
}

fn ambient_refusal(need_wrapper: bool) -> Option<String> {
    use crate::gate::kernel_deps as k;
    let ttys = [
        k::authority_stdin_is_terminal(),
        k::authority_stdout_is_terminal(),
        k::authority_stderr_is_terminal(),
    ];
    let agent = std::env::var_os("AIDA_AGENT_TYPE").is_some_and(|v| !v.is_empty());
    let headless = std::env::var("AIDA_HEADLESS").as_deref() == Ok("1");
    let via = std::env::var(deputy::VIA_WRAPPER_ENV).as_deref() == Ok("1");
    human_terminal_refusal(ttys, agent, headless, via, need_wrapper)
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
    match args.command {
        None => start(&args.start),
        Some(DeputyCommand::Submit(a)) => submit(&a),
        Some(DeputyCommand::Status(a)) => status(&a),
        Some(DeputyCommand::Log(a)) => log(&a),
        Some(DeputyCommand::Cancel(a)) => cancel(&a),
        Some(DeputyCommand::List(a)) => list(&a),
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
    let tty_idle = parse_secs(&a.tty_idle, "--tty-idle")?.max(5);
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
        aida_program: std::env::current_exe().context("locate the aida binary")?,
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

fn submit(a: &DeputySubmitArgs) -> Result<()> {
    let body = match (&a.file, &a.script, a.argv.is_empty()) {
        (Some(f), None, true) => JobBody::Script(
            std::fs::read_to_string(f).with_context(|| format!("read {}", f.display()))?,
        ),
        (None, Some(s), true) => JobBody::Script(s.clone()),
        (None, None, false) => JobBody::Argv(a.argv.clone()),
        _ => bail!("give the job as `-- <command…>`, `-f FILE` or `--script BODY` (exactly one)"),
    };
    let timeout_secs = parse_secs(&a.timeout, "--timeout")?.max(1);
    let spool = deputy::current_spool(&a.name)?;
    let rec = JobRecord {
        id: deputy::new_job_id(),
        created_at: Utc::now(),
        requester: deputy::current_requester(a.spec.clone()),
        serial: a.serial.clone(),
        timeout_secs,
        sha256: body.digest(),
        body,
    };
    spool.submit(&rec)?;
    let mut row = LedgerRow::new(&a.name, &rec.id, "submitted");
    row.requester = Some(rec.requester.clone());
    row.summary = Some(rec.body.summary());
    row.sha256 = Some(rec.sha256.clone());
    let _ = spool.ledger(&row);
    println!("{}", rec.id);
    if !a.wait {
        eprintln!(
            "queued for deputy {:?}. Result: `aida deputy status {}`; output: `aida deputy log {}`",
            a.name, rec.id, rec.id
        );
        return Ok(());
    }
    let limit = Duration::from_secs(timeout_secs + 600);
    let res = wait_for(&spool, &rec.id, limit)?;
    let mut out = String::new();
    if let Ok(mut f) = std::fs::File::open(&res.output) {
        let _ = f.read_to_string(&mut out);
    }
    print!("{out}");
    let _ = std::io::stdout().flush();
    eprintln!(
        "job {}: rc {} ({}) — output: {}",
        res.id,
        res.rc,
        res.reason,
        res.output.display()
    );
    if res.rc != 0 {
        std::process::exit(res.rc.clamp(1, 255));
    }
    Ok(())
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

fn status(a: &DeputyIdArgs) -> Result<()> {
    let spool = deputy::current_spool(&a.name)?;
    let Some((state, path)) = spool.locate(&a.id)? else {
        bail!("no job {} in deputy {:?}", a.id, a.name);
    };
    println!("job {}: {state}", a.id);
    if let Ok(rec) = std::fs::read(&path)
        .map_err(anyhow::Error::from)
        .and_then(|b| serde_json::from_slice::<JobRecord>(&b).map_err(Into::into))
    {
        println!("  command:   {}", rec.body.summary());
        println!(
            "  requester: session {} · role {} · seat {}{}",
            rec.requester.session_id.as_deref().unwrap_or("?"),
            rec.requester.role.as_deref().unwrap_or("?"),
            rec.requester.seat.as_deref().unwrap_or("?"),
            if rec.requester.grant_verified {
                " (grant verified)"
            } else {
                ""
            }
        );
        println!("  submitted: {}", rec.created_at.to_rfc3339());
    }
    if let Some(r) = spool.read_result(&a.id) {
        println!("  rc:        {} ({})", r.rc, r.reason);
        println!("  terminal:  {}", if r.tty { "yes" } else { "no" });
        println!("  output:    {}", r.output.display());
    } else if state == "running" {
        println!("  output:    {}", spool.out_path(&a.id).display());
    }
    Ok(())
}

fn log(a: &DeputyLogArgs) -> Result<()> {
    let spool = deputy::current_spool(&a.name)?;
    if spool.locate(&a.id)?.is_none() {
        bail!("no job {} in deputy {:?}", a.id, a.name);
    }
    let path = spool.out_path(&a.id);
    let mut pos = 0u64;
    let mut stdout = std::io::stdout();
    loop {
        if let Ok(mut f) = std::fs::File::open(&path) {
            f.seek(std::io::SeekFrom::Start(pos))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            pos += buf.len() as u64;
            stdout.write_all(&buf)?;
            stdout.flush()?;
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

fn cancel(a: &DeputyIdArgs) -> Result<()> {
    let spool = deputy::current_spool(&a.name)?;
    match spool.locate(&a.id)? {
        None => bail!("no job {} in deputy {:?}", a.id, a.name),
        Some(("incoming", path)) => {
            let target = spool.job_path("failed", &a.id);
            std::fs::rename(&path, &target).context("the deputy may have just claimed it")?;
            let res = deputy::JobResult {
                id: a.id.clone(),
                rc: deputy::RC_CANCELLED,
                started_at: None,
                finished_at: Utc::now(),
                reason: "cancelled before it ran".into(),
                tty: false,
                policy_entry: None,
                output: spool.out_path(&a.id),
            };
            spool.write_result("failed", &res)?;
            let mut row = LedgerRow::new(&a.name, &a.id, "cancelled");
            row.requester = Some(deputy::current_requester(None));
            row.rc = Some(res.rc);
            row.reason = Some(res.reason);
            spool.ledger(&row)?;
            println!("job {}: cancelled before it ran", a.id);
        }
        Some(("running", _)) => {
            let marker = spool.cancel_marker_path(&a.id);
            std::fs::write(&marker, b"cancel\n")?;
            println!(
                "job {}: cancel requested; the deputy stops it shortly",
                a.id
            );
        }
        Some((state, _)) => println!("job {}: already {state}", a.id),
    }
    Ok(())
}

fn list(a: &DeputyListArgs) -> Result<()> {
    let spool = deputy::current_spool(&a.name)?;
    if !spool.root.exists() {
        println!(
            "deputy {:?} has no spool yet (start it with `aida deputy`)",
            a.name
        );
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
        println!("{state} ({}):", ids.len());
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
            println!("  {id}  {summary}{rc}");
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
