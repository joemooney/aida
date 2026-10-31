//! Mail-backlog guard (`aida doctor check mail-backlog`).
//!
//! Finds unread `[request]` / `[handoff]` mail older than a threshold
//! (default 30 minutes) addressed to a live agent's handle or to a role a
//! live agent holds. Each such message is one finding naming the recipient,
//! sender, kind, age and subject. Silent when there is no backlog.
//!
//! Under the scheduled tick (`AIDA_SCHEDULE_CHILD=1`) each finding also
//! nudges the recipient through its own wake path, at most once per
//! re-nudge interval (default 1 hour) per message:
//!
//! - an armed Codex session gets a short `[fyi]` reminder mail, which its
//!   mail-wake watcher delivers to the session's thread;
//! - a Claude seat gets a `mail-backlog-guard-route-<seat>` due job, which
//!   its per-turn notice and `aida schedule due` surface;
//! - a recipient with neither is escalated to the operator as a
//!   `mail-backlog-guard-route-operator` due job in `aida awaiting`.
//!
//! Reading the message clears it: it stops being a finding and its re-nudge
//! record is dropped.
// trace:STORY-HOME-128 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use aida_core::mailbox::{Intent, Message, Recipient};
use chrono::{DateTime, Duration, Utc};

use crate::agent_registry::{AgentRegistryEntry, MailWake};
use crate::events::{self, Event, EventKind};
use crate::schedule_ledger;
use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "mail-backlog";
/// Prefix of the seat jobs a finding routes to (`<prefix><seat>`).
pub(crate) const ROUTE_JOB_PREFIX: &str = "mail-backlog-guard-route-";
/// The pseudo-seat for recipients with no wake path.
pub(crate) const OPERATOR_SEAT: &str = "operator";
/// Sender identity of the reminder mail.
const REMINDER_SENDER: &str = "aida-mail-backlog-guard";
const DEFAULT_MAX_AGE_MINS: i64 = 30;
const DEFAULT_RENUDGE_MINS: i64 = 60;
const SUBJECT_MAX: usize = 80;

/// `[doctor.mail_backlog]` thresholds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy {
    pub max_age: Duration,
    pub renudge: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_age: Duration::minutes(DEFAULT_MAX_AGE_MINS),
            renudge: Duration::minutes(DEFAULT_RENUDGE_MINS),
        }
    }
}

impl Policy {
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let table = cfg
            .and_then(|c| c.get("doctor"))
            .and_then(|d| d.get("mail_backlog"));
        let mins = |key: &str, default: i64| {
            table
                .and_then(|t| t.get(key))
                .and_then(toml::Value::as_integer)
                .filter(|m| *m > 0)
                .unwrap_or(default)
        };
        Self {
            max_age: Duration::minutes(mins("max_age_mins", DEFAULT_MAX_AGE_MINS)),
            renudge: Duration::minutes(mins("renudge_mins", DEFAULT_RENUDGE_MINS)),
        }
    }
}

/// How a recipient is woken.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Wake {
    /// Reminder mail to an armed Codex session's handle.
    Mail(String),
    /// A due job for a Claude seat.
    Seat(String),
    /// No wake path: escalate to the operator.
    Operator,
}

/// One unread, overdue request or handoff.
#[derive(Debug, Clone)]
pub(crate) struct Backlog {
    /// `<recipient>:<message id>`, stable across ticks.
    pub id: String,
    pub recipient: String,
    pub from: String,
    pub kind: &'static str,
    pub age: Duration,
    pub subject: String,
    pub wake: Wake,
}

impl Backlog {
    fn summary(&self) -> String {
        format!(
            "{}: unread [{}] from {}, {} old: {}",
            self.recipient,
            self.kind,
            self.from,
            format_age(self.age),
            self.subject
        )
    }

    fn into_doctor_finding(self) -> DoctorFinding {
        let action = match &self.wake {
            Wake::Mail(handle) => format!(
                "Reminder goes to {handle} through its mail wake; it reads the message with `aida mailbox inbox`."
            ),
            Wake::Seat(seat) => format!(
                "Routed to the {seat} seat as `{ROUTE_JOB_PREFIX}{seat}` (`aida schedule due --seat {seat}`)."
            ),
            Wake::Operator => format!(
                "{} has no wake path; escalated to the operator in `aida awaiting`. Wake or relaunch it, or resend the mail to a live seat.",
                self.recipient
            ),
        };
        DoctorFinding {
            category: CATEGORY.into(),
            id: format!("{CATEGORY}:{}", self.id),
            summary: self.summary(),
            action,
            safe_heal: false,
        }
    }
}

fn format_age(age: Duration) -> String {
    let mins = age.num_minutes();
    if mins < 120 {
        format!("{mins}m")
    } else {
        format!("{}h{:02}m", mins / 60, mins % 60)
    }
}

fn key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn is_codex_armed(e: &AgentRegistryEntry) -> bool {
    e.agent_type == "codex" && e.mail_wake == Some(MailWake::Armed)
}

fn handle(e: &AgentRegistryEntry) -> String {
    e.name.clone().unwrap_or_else(|| e.id.clone())
}

fn entry_role(e: &AgentRegistryEntry) -> Option<String> {
    e.role
        .as_deref()
        .map(crate::canonical_role_name)
        .filter(|r| !r.is_empty())
}

/// The wake path for mail to one live agent.
fn agent_wake(e: &AgentRegistryEntry) -> Wake {
    if is_codex_armed(e) {
        return Wake::Mail(handle(e));
    }
    match (e.agent_type.as_str(), entry_role(e)) {
        ("claude", Some(role)) => Wake::Seat(role),
        _ => Wake::Operator,
    }
}

/// The wake path for mail to a role held by `holders`: a Claude seat holder
/// first (one due job reaches the seat), else an armed Codex holder, else the
/// operator.
fn role_wake(role: &str, holders: &[&AgentRegistryEntry]) -> Wake {
    if holders.iter().any(|e| e.agent_type == "claude") {
        return Wake::Seat(role.to_string());
    }
    holders
        .iter()
        .find(|e| is_codex_armed(e))
        .map(|e| Wake::Mail(handle(e)))
        .unwrap_or(Wake::Operator)
}

/// PURE: the overdue unread requests/handoffs addressed to a live agent's
/// handle or a role a live agent holds. `is_read(recipient, msg)` is the
/// read evidence; mail with no recoverable time is never aged.
pub(crate) fn evaluate(
    messages: &[Message],
    is_read: &dyn Fn(&str, &Message) -> bool,
    live: &[AgentRegistryEntry],
    policy: &Policy,
    now: DateTime<Utc>,
) -> Vec<Backlog> {
    let mut by_handle: BTreeMap<String, &AgentRegistryEntry> = BTreeMap::new();
    let mut by_role: BTreeMap<String, Vec<&AgentRegistryEntry>> = BTreeMap::new();
    for e in live {
        by_handle.insert(key(&e.id), e);
        if let Some(name) = &e.name {
            by_handle.insert(key(name), e);
        }
        if let Some(role) = entry_role(e) {
            by_role.entry(role).or_default().push(e);
        }
    }
    let now_ms = now.timestamp_millis();
    let mut out = Vec::new();
    for m in messages {
        if m.deleted || m.retracted || m.archived || m.timestamp <= 0 {
            continue;
        }
        let kind = match m.intent {
            Intent::Request => "request",
            Intent::Handoff => "handoff",
            Intent::Fyi => continue,
        };
        let Recipient::Agent(to) = &m.to else {
            continue;
        };
        let wake = if let Some(e) = by_handle.get(&key(to)) {
            agent_wake(e)
        } else {
            let role = crate::canonical_role_name(to);
            match by_role.get(&role) {
                Some(holders) => role_wake(&role, holders),
                None => continue,
            }
        };
        let age = Duration::milliseconds(now_ms - m.timestamp);
        if age < policy.max_age || is_read(to, m) {
            continue;
        }
        out.push(Backlog {
            id: format!("{}:{}", key(to), m.id),
            recipient: to.clone(),
            from: m.from.clone(),
            kind,
            age,
            subject: aida_core::mailbox::subject_line(m, SUBJECT_MAX),
            wake,
        });
    }
    out.sort_by(|a, b| b.age.cmp(&a.age).then(a.id.cmp(&b.id)));
    out
}

/// PURE: the findings to nudge this tick, grouped by wake path. A finding is
/// due when it was never nudged or its last nudge is at least the re-nudge
/// interval old.
pub(crate) fn due_nudges<'a>(
    backlog: &'a [Backlog],
    nudged: &BTreeMap<String, DateTime<Utc>>,
    policy: &Policy,
    now: DateTime<Utc>,
) -> BTreeMap<Wake, Vec<&'a Backlog>> {
    let mut out: BTreeMap<Wake, Vec<&Backlog>> = BTreeMap::new();
    for b in backlog {
        let due = nudged
            .get(&b.id)
            .is_none_or(|last| now - *last >= policy.renudge);
        if due {
            out.entry(b.wake.clone()).or_default().push(b);
        }
    }
    out
}

/// Read-only scan (the `aida doctor check mail-backlog` path).
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    scan_and_route(project_root, false)
}

/// Scan, and when `route` is true (the scheduled tick) nudge each finding
/// through its wake path.
pub(crate) fn scan_and_route(project_root: &Path, route: bool) -> Vec<DoctorFinding> {
    let cfg = crate::read_project_config_value(project_root);
    let policy = Policy::from_config(cfg.as_ref());
    let store_root = crate::detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    let messages = match read_merged(project_root, &store_root) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("warning: mail-backlog could not read the mailbox: {err:#}");
            return Vec::new();
        }
    };
    let evidence = crate::mailbox_store::ReadEvidence::load_fail_closed(project_root);
    let live = crate::agent_registry::load_live_entries(project_root);
    let now = Utc::now();
    let backlog = evaluate(
        &messages,
        &|to, m| evidence.read_by(to, m),
        &live,
        &policy,
        now,
    );
    if route {
        route_backlog(project_root, &store_root, &backlog, &policy, now);
    }
    backlog
        .into_iter()
        .map(Backlog::into_doctor_finding)
        .collect()
}

fn read_merged(project_root: &Path, store_root: &Path) -> anyhow::Result<Vec<Message>> {
    let local = crate::mailbox_store::read_local_messages(project_root)?;
    let canonical = crate::mailbox_store::read_canonical_messages(store_root)?;
    Ok(aida_core::mailbox::merge_dedup(&local, &canonical))
}

fn nudged_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".aida")
        .join("mail-backlog")
        .join("nudged.json")
}

fn load_nudged(project_root: &Path) -> BTreeMap<String, DateTime<Utc>> {
    std::fs::read_to_string(nudged_path(project_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Nudge every due finding. Only a nudge that landed is recorded, so a
/// failed one retries next tick; records for mail no longer in the backlog
/// (read, deleted, or its agent gone) are dropped.
fn route_backlog(
    project_root: &Path,
    store_root: &Path,
    backlog: &[Backlog],
    policy: &Policy,
    now: DateTime<Utc>,
) {
    let previous = load_nudged(project_root);
    let current: BTreeSet<&str> = backlog.iter().map(|b| b.id.as_str()).collect();
    let mut nudged: BTreeMap<String, DateTime<Utc>> = previous
        .iter()
        .filter(|(id, _)| current.contains(id.as_str()))
        .map(|(id, at)| (id.clone(), *at))
        .collect();

    for (wake, items) in due_nudges(backlog, &previous, policy, now) {
        let landed = match &wake {
            Wake::Mail(handle) => send_reminder(project_root, handle, &items, now),
            Wake::Seat(seat) => route_seat(project_root, store_root, seat, &items, now),
            Wake::Operator => route_seat(project_root, store_root, OPERATOR_SEAT, &items, now),
        };
        if landed {
            for b in items {
                nudged.insert(b.id.clone(), now);
            }
        }
    }

    if nudged != previous {
        let path = nudged_path(project_root);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string_pretty(&nudged) {
            let _ = aida_core::write_atomic(&path, body.as_bytes());
        }
    }
}

fn evidence_lines(items: &[&Backlog]) -> String {
    items
        .iter()
        .map(|b| format!("- {}", b.summary()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `[fyi]` reminder to an armed Codex session; its mail wake delivers it.
fn send_reminder(
    project_root: &Path,
    handle: &str,
    items: &[&Backlog],
    now: DateTime<Utc>,
) -> bool {
    let id = uuid::Uuid::new_v4().to_string();
    let body = format!(
        "You have {} unread request/handoff message(s) older than the mail-backlog threshold:\n\n{}\n\nRead them with `aida mailbox inbox` and act on each.",
        items.len(),
        evidence_lines(items)
    );
    let msg = Message {
        subject: Some(format!("Unread mail waiting: {} message(s)", items.len())),
        id: id.clone(),
        thread_id: id,
        from: REMINDER_SENDER.to_string(),
        to: Recipient::Agent(handle.to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    match crate::mailbox_store::write_message(project_root, &msg) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("warning: mail-backlog could not mail a reminder to {handle}: {err:#}");
            false
        }
    }
}

/// Mark `<prefix><seat>` due with the evidence and wake the seat.
fn route_seat(
    project_root: &Path,
    store_root: &Path,
    seat: &str,
    items: &[&Backlog],
    now: DateTime<Utc>,
) -> bool {
    let job = route_job_name(seat);
    let evidence = items
        .iter()
        .map(|b| b.summary())
        .collect::<Vec<_>>()
        .join("; ");
    let trip_id = format!(
        "{job}@{}",
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if let Err(err) = schedule_ledger::write_cas(store_root, &job, |ledger| {
        if ledger.due_since.is_none() {
            ledger.due_since = Some(now);
        }
        ledger.due_reason = Some(format!("on CronJobFired:{job}"));
        ledger.note = Some(evidence.clone());
        ledger.due_failure = Some(schedule_ledger::RoutedFailure {
            trip_id: trip_id.clone(),
            performance: Vec::new(),
            job: Some(CATEGORY.to_string()),
            error: Some(evidence.clone()),
        });
    }) {
        eprintln!("warning: could not write schedule ledger for '{job}': {err}");
        return false;
    }
    events::emit(
        project_root,
        &Event::new(
            None,
            "",
            EventKind::CronJobFired {
                job,
                seat: seat.to_string(),
            },
        ),
    );
    true
}

/// Schedule job name of the route for `seat`.
pub(crate) fn route_job_name(seat: &str) -> String {
    format!("{ROUTE_JOB_PREFIX}{seat}")
}

/// The seat a `mail-backlog-guard-route-<seat>` job routes to, if it is one.
pub(crate) fn seat_for_job(job: &str) -> Option<&str> {
    job.strip_prefix(ROUTE_JOB_PREFIX).filter(|s| !s.is_empty())
}

/// The seat of a route job the guard actually raised: the operator
/// escalation, or a seat whose route ledger is in the store (`route_seat`
/// writes it before the job can surface anywhere). Any other
/// `<prefix><x>` name — a typo — is not a job.
// trace:TASK-B6B1278890774EF993D2EBC327862470-134 | ai:claude
pub(crate) fn known_route_seat<'a>(store_root: &Path, job: &'a str) -> Option<&'a str> {
    let seat = seat_for_job(job)?;
    (seat == OPERATOR_SEAT || schedule_ledger::ledger_path(store_root, job).is_file())
        .then_some(seat)
}

/// Whether a session may see (and clear) operator escalations: only sessions
/// a human reads — the operator terminal (no seat, or seat `operator`) and
/// interactive advisor sessions. Headless lanes and doer seats
/// (implementer/reviewer/…) never do, so an agent cannot act on or clear the
/// human's escalation before the human sees it.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
pub(crate) fn operator_escalation_visible(seat: Option<&str>, headless: bool) -> bool {
    !headless && seat.is_none_or(|s| s == OPERATOR_SEAT || s == "advisor")
}

/// A job whose only seat is `operator` — an escalation for the human.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
pub(crate) fn is_operator_job(seats: &[String]) -> bool {
    !seats.is_empty() && seats.iter().all(|s| s == OPERATOR_SEAT)
}

/// The prompt the routed seat job carries.
pub(crate) fn route_prompt(seat: &str) -> String {
    let job = route_job_name(seat);
    if seat == OPERATOR_SEAT {
        format!(
            "Agents with no wake path have unread requests or handoffs past the mail-backlog threshold. Read the evidence in `aida schedule due --seat {seat}` (or run `aida doctor check mail-backlog`), then wake or relaunch each recipient, or resend its mail to a live seat. Then run `aida schedule done {job}`."
        )
    } else {
        format!(
            "Requests or handoffs addressed to you or the {seat} seat have sat unread past the mail-backlog threshold. Read the evidence in `aida schedule due --seat {seat}` (or run `aida doctor check mail-backlog`), read your mail with `aida mailbox inbox`, and act on each message. Then run `aida schedule done {job}`."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn agent(id: &str, name: &str, agent_type: &str, role: Option<&str>) -> AgentRegistryEntry {
        let mut e: AgentRegistryEntry = toml::from_str(&format!(
            "id = \"{id}\"\nagent_type = \"{agent_type}\"\npid = 1\nstarted_at = \"2026-10-30T08:00:00Z\"\nlast_active_at = \"2026-10-30T08:00:00Z\"\nworktree_path = \"wt\"\nsource = \"test\"\n"
        ))
        .unwrap();
        e.name = Some(name.to_string());
        e.role = role.map(str::to_string);
        e
    }

    fn mail(id: &str, to: &str, intent: Intent, age_mins: i64) -> Message {
        Message {
            subject: Some(format!("subject {id}")),
            id: id.to_string(),
            thread_id: id.to_string(),
            from: "advisor".to_string(),
            to: Recipient::Agent(to.to_string()),
            timestamp: (now() - Duration::minutes(age_mins)).timestamp_millis(),
            in_reply_to: None,
            body: "body".to_string(),
            urgent: false,
            intent,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        }
    }

    fn unread(_: &str, _: &Message) -> bool {
        false
    }

    fn codex() -> AgentRegistryEntry {
        let mut e = agent("codex-1", "codex-agent-04", "codex", Some("implementer"));
        e.mail_wake = Some(MailWake::Armed);
        e
    }

    #[test]
    fn stale_request_to_live_agent_is_one_finding_and_one_nudge() {
        let live = [codex()];
        let msgs = [mail("m1", "codex-agent-04", Intent::Request, 45)];
        let policy = Policy::default();
        let backlog = evaluate(&msgs, &unread, &live, &policy, now());
        assert_eq!(backlog.len(), 1);
        let b = &backlog[0];
        assert_eq!(b.wake, Wake::Mail("codex-agent-04".into()));
        let summary = b.summary();
        for part in [
            "codex-agent-04",
            "[request]",
            "advisor",
            "45m",
            "subject m1",
        ] {
            assert!(summary.contains(part), "{part} missing from {summary}");
        }

        let nudges = due_nudges(&backlog, &BTreeMap::new(), &policy, now());
        assert_eq!(nudges.len(), 1);
        assert_eq!(nudges[&Wake::Mail("codex-agent-04".into())].len(), 1);

        // Nudged ten minutes ago: not again until the re-nudge interval.
        let mut nudged = BTreeMap::new();
        nudged.insert(b.id.clone(), now() - Duration::minutes(10));
        assert!(due_nudges(&backlog, &nudged, &policy, now()).is_empty());
        nudged.insert(b.id.clone(), now() - Duration::minutes(60));
        assert_eq!(due_nudges(&backlog, &nudged, &policy, now()).len(), 1);
    }

    #[test]
    fn fresh_request_is_not_a_finding() {
        let live = [codex()];
        let msgs = [mail("m1", "codex-agent-04", Intent::Handoff, 10)];
        assert!(evaluate(&msgs, &unread, &live, &Policy::default(), now()).is_empty());
    }

    #[test]
    fn read_request_clears_the_finding() {
        let live = [codex()];
        let msgs = [mail("m1", "codex-agent-04", Intent::Request, 90)];
        let read = |to: &str, m: &Message| to == "codex-agent-04" && m.id == "m1";
        assert!(evaluate(&msgs, &read, &live, &Policy::default(), now()).is_empty());
    }

    #[test]
    fn fyi_and_mail_to_unknown_recipients_are_ignored() {
        let live = [codex()];
        let msgs = [
            mail("m1", "codex-agent-04", Intent::Fyi, 90),
            mail("m2", "codex-agent-99", Intent::Request, 90),
            mail("m3", "product", Intent::Request, 90),
        ];
        assert!(evaluate(&msgs, &unread, &live, &Policy::default(), now()).is_empty());
    }

    #[test]
    fn wake_paths_by_recipient_kind() {
        let claude = agent("claude-7", "claude-advisor-1", "claude", Some("advisor"));
        let agy = agent("agy-1", "antigravity-1", "antigravity", Some("reviewer"));
        let live = [codex(), claude, agy];
        let msgs = [
            mail("a", "advisor", Intent::Request, 40),
            mail("b", "claude-advisor-1", Intent::Handoff, 40),
            mail("c", "implementer", Intent::Request, 40),
            mail("d", "antigravity-1", Intent::Request, 40),
        ];
        let backlog = evaluate(&msgs, &unread, &live, &Policy::default(), now());
        let wake = |id: &str| {
            backlog
                .iter()
                .find(|b| b.id.ends_with(&format!(":{id}")))
                .unwrap()
                .wake
                .clone()
        };
        assert_eq!(wake("a"), Wake::Seat("advisor".into()));
        assert_eq!(wake("b"), Wake::Seat("advisor".into()));
        assert_eq!(wake("c"), Wake::Mail("codex-agent-04".into()));
        assert_eq!(wake("d"), Wake::Operator);
    }

    #[test]
    fn policy_reads_config_and_route_names_round_trip() {
        let cfg: toml::Value =
            toml::from_str("[doctor.mail_backlog]\nmax_age_mins = 15\nrenudge_mins = 120\n")
                .unwrap();
        let p = Policy::from_config(Some(&cfg));
        assert_eq!(p.max_age, Duration::minutes(15));
        assert_eq!(p.renudge, Duration::minutes(120));
        assert_eq!(seat_for_job(&route_job_name("advisor")), Some("advisor"));
        assert_eq!(seat_for_job("mail-backlog-guard"), None);
        assert!(
            route_prompt("advisor").contains("aida schedule done mail-backlog-guard-route-advisor")
        );
    }

    fn backlog_item(id: &str, wake: Wake) -> Backlog {
        Backlog {
            id: id.to_string(),
            recipient: "someone".to_string(),
            from: "advisor".to_string(),
            kind: "request",
            age: Duration::minutes(45),
            subject: format!("subject {id}"),
            wake,
        }
    }

    // The route_backlog I/O layer: a landed nudge is recorded in
    // nudged.json once and not repeated inside the re-nudge interval; a
    // failed nudge is not recorded, so the next tick retries it; records for
    // mail that left the backlog are dropped.
    // trace:TASK-B6B1278890774EF993D2EBC327862470-133 | ai:claude
    #[test]
    fn route_backlog_records_landed_nudges_retries_failures_and_drops_cleared_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let store = root.join(".aida-store");
        let policy = Policy::default();
        let t0 = now();
        let seat = backlog_item("advisor:m1", Wake::Seat("advisor".into()));
        let mail = backlog_item("codex-agent-04:m2", Wake::Mail("codex-agent-04".into()));

        // Tick 1: both land, both are recorded, the seat job is due.
        route_backlog(root, &store, &[seat.clone(), mail.clone()], &policy, t0);
        let nudged = load_nudged(root);
        assert_eq!(nudged.get("advisor:m1"), Some(&t0));
        assert_eq!(nudged.get("codex-agent-04:m2"), Some(&t0));
        let ledger = schedule_ledger::load(&store, &route_job_name("advisor")).unwrap();
        assert_eq!(ledger.due_since, Some(t0));
        assert!(ledger.note.unwrap().contains("subject advisor:m1"));
        let reminders: Vec<Message> = crate::mailbox_store::read_local_messages(root)
            .unwrap()
            .into_iter()
            .filter(|m| m.from == REMINDER_SENDER)
            .collect();
        assert_eq!(reminders.len(), 1, "one nudge");
        assert_eq!(reminders[0].to, Recipient::Agent("codex-agent-04".into()));

        // Tick 2, inside the re-nudge interval: no second reminder, records kept.
        route_backlog(
            root,
            &store,
            &[seat.clone(), mail.clone()],
            &policy,
            t0 + Duration::minutes(10),
        );
        assert_eq!(load_nudged(root), nudged);
        let reminders = crate::mailbox_store::read_local_messages(root)
            .unwrap()
            .into_iter()
            .filter(|m| m.from == REMINDER_SENDER)
            .count();
        assert_eq!(reminders, 1, "still one nudge");

        // Tick 3: the mail was read (gone from the backlog) and a new operator
        // escalation fails to land (the store is unwritable).
        let broken_store = root.join("not-a-store");
        std::fs::write(&broken_store, "file, not a directory").unwrap();
        let operator = backlog_item("ghost:m3", Wake::Operator);
        let t3 = t0 + Duration::minutes(20);
        route_backlog(
            root,
            &broken_store,
            &[seat.clone(), operator.clone()],
            &policy,
            t3,
        );
        let nudged = load_nudged(root);
        assert!(
            !nudged.contains_key("codex-agent-04:m2"),
            "read mail dropped"
        );
        assert!(
            !nudged.contains_key("ghost:m3"),
            "failed nudge not recorded"
        );
        assert_eq!(nudged.get("advisor:m1"), Some(&t0));

        // Tick 4: the store is writable again; the failed nudge retries and lands.
        let t4 = t0 + Duration::minutes(25);
        route_backlog(root, &store, &[seat, operator], &policy, t4);
        let nudged = load_nudged(root);
        assert_eq!(nudged.get("ghost:m3"), Some(&t4));
        assert_eq!(nudged.get("advisor:m1"), Some(&t0), "not re-nudged early");
        assert!(
            schedule_ledger::load(&store, &route_job_name(OPERATOR_SEAT))
                .and_then(|l| l.due_since)
                .is_some()
        );

        // Everything cleared: every record is dropped.
        route_backlog(root, &store, &[], &policy, t4);
        assert!(load_nudged(root).is_empty());
    }

    // trace:TASK-B6B1278890774EF993D2EBC327862470-134 | ai:claude
    #[test]
    fn known_route_seat_requires_a_raised_route_or_the_operator() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path();
        assert_eq!(
            known_route_seat(store, "mail-backlog-guard-route-operator"),
            Some(OPERATOR_SEAT)
        );
        assert_eq!(
            known_route_seat(store, "mail-backlog-guard-route-typo"),
            None
        );
        assert_eq!(known_route_seat(store, "lane-liveness-advisor"), None);
        schedule_ledger::write_cas(store, "mail-backlog-guard-route-typo", |l| {
            l.due_since = Some(now())
        })
        .unwrap();
        assert_eq!(
            known_route_seat(store, "mail-backlog-guard-route-typo"),
            Some("typo")
        );
    }
}
