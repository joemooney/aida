//! Who submitted a script job, and whether the runner believes it.
//!
//! Submit side ([`capture`]): the submitter must hold a VALID seat grant (the
//! existing presenter check: process ancestry, session leader, or the
//! grant's session key), and the record is filled from that grant record and
//! this process, never from free text.
//!
//! Claim side ([`verdict`]): the runner re-checks every job before its script
//! is written to `jobs/`, over injected probes so the table is testable. Any
//! failure is a named refusal and the script never runs.
//!
//! Submit is kernel-attested ([`super::socket`]): the runner accepts a job
//! only from a socket peer whose process ancestry reaches one of the named
//! grant's process bindings, and records that peer on the job. The claim side
//! refuses a job that carries no attested peer, so a record dropped into the
//! queue by any other route never runs.
// trace:STORY-2338 | ai:claude

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::Path;
use std::sync::Arc;

use super::{Host, ShellJob, Submitter};
use crate::gate::seat_authority::{GrantCheck, SeatGrant};

/// Q5: how fresh a key-only submitter's session heartbeat must be.
pub(crate) const HEARTBEAT_FRESH: Duration = Duration::minutes(10);

/// Fill the submitter block for a job submitted from this process.
/// `main` is the project's main checkout (roster and mail live there).
pub(crate) fn capture(main: &Path) -> Result<Submitter> {
    if std::env::var_os(super::JOB_ENV).is_some_and(|v| !v.is_empty()) {
        bail!("this runs inside a script-deputy job; a job cannot queue another script job");
    }
    let grant = match crate::gate::seat_authority::grant_check(main) {
        GrantCheck::Valid(g) => g,
        other => bail!(
            "submit needs a live session with a seat grant; run it from a session started with `aida role enter`{}",
            other
                .rejection_sentence()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        ),
    };
    let presented = crate::gate::seat_authority::presenter_tokens_for(&grant);
    let matched = crate::gate::seat_authority::matched_binding(&grant, &presented);
    let pid = std::process::id();
    Ok(submitter_from(
        &grant,
        &matched,
        crate::kernel::resolve_current_session_id(),
        pid,
        aida_core::liveness::process_start_identity(pid),
        main,
        &std::env::current_dir().unwrap_or_else(|_| main.to_path_buf()),
        Host::current(),
        Some(crate::resolve_mail_sender_identity(None).0),
    ))
}

/// PURE: the submitter block from a validated grant and the binding tokens
/// this process matched. A process token is preferred over a `key:` token,
/// since the runner can check a process for liveness.
#[allow(clippy::too_many_arguments)]
pub(crate) fn submitter_from(
    grant: &SeatGrant,
    matched: &[String],
    session_id: Option<String>,
    pid: u32,
    pid_start: Option<String>,
    project_root: &Path,
    cwd: &Path,
    host: Host,
    mail_to: Option<String>,
) -> Submitter {
    let presenter_token = matched
        .iter()
        .find(|t| !t.starts_with("key:"))
        .or_else(|| matched.first())
        .cloned();
    Submitter {
        session_id,
        seat: crate::gate::dispatch_deps::canonical_role_name(&grant.seat),
        subject: grant.subject.clone(),
        principal: grant.principal.clone(),
        grant_id: Some(grant.id.clone()),
        presenter_token,
        pid,
        pid_start,
        project_root: project_root.to_path_buf(),
        cwd: cwd.to_path_buf(),
        host,
        mail_to,
        peer: None,
    }
}

/// What the claim-side verdict reads the world with.
#[derive(Clone)]
pub(crate) struct Probes {
    /// The grant verdict for `(project_root, grant_id)`, as STORY-1687
    /// revalidates a recorded reviewer (missing, expired, revoked, roster
    /// ceiling, unbound).
    pub grant: Arc<dyn Fn(&Path, &str) -> GrantCheck + Send + Sync>,
    /// Whether process `pid` with start identity `start` is alive, in the
    /// runner's (host) pid namespace.
    pub alive: Arc<dyn Fn(u32, &str) -> bool + Send + Sync>,
    /// The last heartbeat of agent session `session_id` in `project_root`,
    /// counted only from a registry row bound to `grant` (see
    /// [`heartbeat_row_vouches`]), so another session's heartbeat never
    /// vouches for this grant.
    pub heartbeat: Arc<dyn Fn(&Path, &str, &SeatGrant) -> Option<DateTime<Utc>> + Send + Sync>,
    /// This machine.
    pub host: Host,
}

impl Probes {
    /// The real world, for the runner.
    pub(crate) fn live() -> Self {
        Self {
            // trace:BUG-2520 | ai:codex
            grant: Arc::new(|root, id| {
                crate::gate::seat_authority::grant_check_for_principal(
                    root,
                    id,
                    &crate::gate::kernel_deps::current_user_id(None),
                )
            }),
            alive: Arc::new(|pid, start| {
                aida_core::liveness::process_identity_matches_strict(pid, start)
            }),
            heartbeat: Arc::new(session_heartbeat),
            host: Host::current(),
        }
    }
}

/// The newest heartbeat of a live agent-registry row for `session_id` in
/// `project_root` (the per-prompt heartbeat) that is bound to `grant`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
pub(crate) fn session_heartbeat(
    project_root: &Path,
    session_id: &str,
    grant: &SeatGrant,
) -> Option<DateTime<Utc>> {
    std::fs::read_dir(crate::agent_registry::agents_dir(project_root))
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|b| toml::from_str::<crate::agent_registry::AgentRegistryEntry>(&b).ok())
        .filter(|row| heartbeat_row_vouches(row, session_id, grant))
        .map(|row| row.last_active_at)
        .max()
}

/// PURE: whether registry `row` may vouch for `session_id` under `grant`: a
/// live row of that session AND bound to that grant, by either
/// - the session key: the row's hooks proved they hold the key whose `key:`
///   token is one of the grant's bindings. This is how a directly issued
///   grant (`aida role enter`, subject = the OS user) is tied to a session;
///   or
/// - the agent name: a launched child's grant names the agent it was minted
///   for, and the registry row carries that name.
///
/// A row for another session or agent, or one whose recorded key the grant
/// does not carry, never vouches.
// trace:STORY-2338 | ai:claude
pub(crate) fn heartbeat_row_vouches(
    row: &crate::agent_registry::AgentRegistryEntry,
    session_id: &str,
    grant: &SeatGrant,
) -> bool {
    let same_session = row.claude_session_id.as_deref() == Some(session_id)
        || row.native_session_id.as_deref() == Some(session_id);
    let holds_key = row
        .grant_key
        .as_deref()
        .is_some_and(|k| k.starts_with("key:") && grant.binding.iter().any(|b| b == k));
    let subject = grant.subject.as_str();
    let same_agent =
        !subject.is_empty() && (row.id == subject || row.name.as_deref() == Some(subject));
    row.ended_at.is_none() && same_session && (holds_key || same_agent)
}

/// A refusal: the `refused:<code>` code and why, in one sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: &'static str,
    pub why: String,
}

impl Refusal {
    fn new(code: &'static str, why: impl Into<String>) -> Self {
        Self {
            code,
            why: why.into(),
        }
    }
    /// `refused:<code>`, the state's `reason`.
    pub(crate) fn reason(&self) -> String {
        format!("refused:{}", self.code)
    }
}

/// The claim-side verdict on `job`: the checks it passed, or the refusal.
/// Order: attribution present, same host, grant valid (and the record agrees
/// with it), seat allowed, submitter live.
pub(crate) fn verdict(
    job: &ShellJob,
    submit_seats: &[String],
    probes: &Probes,
    now: DateTime<Utc>,
) -> Result<Vec<String>, Refusal> {
    let s = &job.submitter;
    let mut checks = Vec::new();
    let (Some(grant_id), Some(token)) = (s.grant_id.as_deref(), s.presenter_token.as_deref())
    else {
        return Err(Refusal::new(
            "unattributed",
            "the job names no grant or no presenter token, so no submitter can be checked",
        ));
    };
    checks.push("attributed".to_string());
    // trace:STORY-2343 | ai:claude
    let Some(peer) = s.peer.as_ref().filter(|p| p.attested) else {
        return Err(Refusal::new(
            "unattested",
            "the job did not come through the runner's socket, so no kernel-attested submitter vouches for it",
        ));
    };
    if s.host.boot_id.is_empty()
        || s.host.boot_id != probes.host.boot_id
        || s.host.hostname != probes.host.hostname
    {
        return Err(Refusal::new(
            "foreign-host",
            format!(
                "it was submitted on {} (boot {}), not on this machine as of this boot",
                s.host.hostname, s.host.boot_id
            ),
        ));
    }
    checks.push("same-host".to_string());
    let grant = match (probes.grant)(&s.project_root, grant_id) {
        GrantCheck::Valid(g) => g,
        other => {
            return Err(Refusal::new(
                "grant",
                other
                    .rejection_sentence()
                    .unwrap_or_else(|| "the submitter's grant does not validate".into()),
            ))
        }
    };
    let seat = crate::gate::dispatch_deps::canonical_role_name(&grant.seat);
    if seat != s.seat || grant.subject != s.subject || grant.principal != s.principal {
        return Err(Refusal::new(
            "grant",
            "the job's seat, subject or principal does not match the grant it names",
        ));
    }
    if !grant
        .binding
        .iter()
        .any(|b| crate::gate::review_authority::binding_tokens_match(b, &peer.matched))
    {
        return Err(Refusal::new(
            "unattested",
            "the binding the submitting process was attested under is not one of the grant's bindings",
        ));
    }
    checks.push("grant-valid".to_string());
    checks.push("peer-attested".to_string());
    if !submit_seats
        .iter()
        .any(|allowed| crate::gate::dispatch_deps::canonical_role_name(allowed) == seat)
    {
        return Err(Refusal::new(
            "seat",
            format!(
                "seat {seat} may not submit to this deputy (allowed: {})",
                submit_seats.join(", ")
            ),
        ));
    }
    checks.push("seat-allowed".to_string());
    if !grant
        .binding
        .iter()
        .any(|b| crate::gate::review_authority::binding_tokens_match(b, token))
    {
        return Err(Refusal::new(
            "not-live",
            "the presenter token on the job is not one of the grant's bindings",
        ));
    }
    let process_live = grant
        .binding
        .iter()
        .filter_map(|t| crate::gate::seat_authority::binding_process(t))
        // BUG-2419: a pid bound in another pid namespace names some other
        // process here, so only a token taken in the runner's own namespace
        // (unqualified, or qualified with it) is probed. A foreign one is not
        // a live holder and falls through to the Q5 heartbeat path.
        // trace:STORY-2338 | ai:claude
        .filter(|(_, ns, _)| {
            ns.is_none_or(|ns| {
                crate::gate::review_authority::origin_pid_ns().as_deref() == Some(ns)
            })
        })
        .any(|(pid, _, start)| (probes.alive)(pid, start));
    if process_live {
        checks.push("holder-live".to_string());
        return Ok(checks);
    }
    // Q5: a key-only (sandboxed) submitter has no host process to probe; a
    // fresh heartbeat stands in for it. The job's self-declared session is
    // not enough on its own: the heartbeat must come from a registry row
    // bound to this grant (its session key, or the agent it was minted for).
    // TODO(STORY-2325): move this onto the liveness judge when it lands.
    // trace:STORY-2325 | ai:claude
    let fresh = s
        .session_id
        .as_deref()
        .and_then(|sid| (probes.heartbeat)(&s.project_root, sid, &grant))
        .is_some_and(|beat| now - beat <= HEARTBEAT_FRESH && beat <= now + Duration::minutes(1));
    if fresh {
        checks.push("session-heartbeat".to_string());
        return Ok(checks);
    }
    Err(Refusal::new(
        "not-live",
        "no process the grant is bound to is alive, and the grant's own agent session has no heartbeat in the last 10 minutes",
    ))
}
