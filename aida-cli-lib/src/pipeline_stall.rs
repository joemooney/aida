//! Independent, observation-based pipeline alarms. No completion/merge authority.
// trace:STORY-2476 | ai:codex
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[path = "pipeline_stall_signals.rs"]
mod signals;

pub(super) const ALARM_SENDER: &str = "aida-pipeline-detector";

// An absent observer cannot prove that a predicate held throughout a long gap.
const MAX_GAP_MS: i64 = 15 * 60 * 1000;

// trace:TASK-3074 | ai:codex
const ESCALATE_MS: i64 = 30 * 60 * 1000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct RequestIdentity {
    pub message_id: String,
    pub fingerprint: String,
}

/// Persisted mailbox identity is distinct from operator transport acceptance.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum SendResult {
    Persisted { request: RequestIdentity },
    Sent,
    Queued,
    Refused,
    Uncertain,
}

// Preserve ambiguous post-transport errors; only proven pre-publication errors retry.
// trace:TASK-3074 | ai:codex
pub(super) fn operator_delivery(
    result: Result<crate::notify::DirectNotifyOutcome>,
) -> Result<SendResult> {
    match result {
        Ok(outcome) => Ok(match outcome.delivery() {
            crate::notify::DirectDelivery::Sent => SendResult::Sent,
            crate::notify::DirectDelivery::Deferred => SendResult::Queued,
            _ => SendResult::Refused,
        }),
        Err(error) if crate::notify::direct_publication_refused(&error) => {
            eprintln!("pipeline operator delivery refused: {error:#}");
            Ok(SendResult::Refused)
        }
        Err(error) => Err(error),
    }
}

#[derive(Default, Deserialize, Serialize)]
struct State {
    version: u32,
    episodes: BTreeMap<String, Episode>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Episode {
    first_ms: i64,
    observed_ms: i64,
    accepted: bool,
    #[serde(default)]
    delivery_pending: bool,
    #[serde(default)]
    identity: String,
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    request: Option<RequestIdentity>,
    #[serde(default)]
    delivery: Option<SendResult>,
    #[serde(default)]
    delivered_ms: Option<i64>,
    #[serde(default)]
    continuity_ms: Option<i64>,
    #[serde(default)]
    escalation_pending: bool,
    #[serde(default)]
    escalation: Option<SendResult>,
    // Existing v2 escalation fields retain unresolved notices across upgrades.
    #[serde(default)]
    persistent_escalation_pending: bool,
    #[serde(default)]
    persistent_escalation: Option<SendResult>,
    // trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
    #[serde(default)]
    deferral_escalation_pending: bool,
    #[serde(default)]
    deferral_escalation: Option<SendResult>,
}
impl Episode {
    fn new(now: i64, identity: &str) -> Self {
        Self {
            first_ms: now,
            observed_ms: now,
            accepted: false,
            delivery_pending: false,
            identity: identity.into(),
            uuid: uuid::Uuid::new_v4().to_string(),
            request: None,
            delivery: None,
            delivered_ms: None,
            continuity_ms: Some(now),
            escalation_pending: false,
            escalation: None,
            persistent_escalation_pending: false,
            persistent_escalation: None,
            deferral_escalation_pending: false,
            deferral_escalation: None,
        }
    }
    // Keep each once-per-episode notice independent, including durable reservations.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
    fn escalation_slot(&mut self, kind: EscalationKind) -> (&mut bool, &mut Option<SendResult>) {
        match kind {
            EscalationKind::Unresolved => (&mut self.escalation_pending, &mut self.escalation),
            EscalationKind::Persistent => (
                &mut self.persistent_escalation_pending,
                &mut self.persistent_escalation,
            ),
            EscalationKind::Deferral => (
                &mut self.deferral_escalation_pending,
                &mut self.deferral_escalation,
            ),
        }
    }
}
// trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
#[derive(Clone, Copy)]
enum EscalationKind {
    Unresolved,
    Persistent,
    Deferral,
}
impl EscalationKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::Persistent => "persistent",
            Self::Deferral => "deferral",
        }
    }
}
#[derive(Default, Debug, Serialize)]
pub(super) struct Report {
    execute: bool,
    active: Vec<String>,
    due: Vec<String>,
    accepted: Vec<String>,
    unknown: Vec<String>,
    busy: bool,
    evidence: Vec<String>,
    deliveries: BTreeMap<String, SendResult>,
    escalations: BTreeMap<String, SendResult>,
}
impl Report {
    pub(super) fn summary(&self) -> String {
        format!(
            "pipeline alarms{}: active {:?}, due {:?}, accepted {:?}, unknown {:?}, delivery {:?}, escalation {:?}{}",
            if self.execute { "" } else { " (dry-run)" },
            self.active,
            self.due,
            self.accepted,
            self.unknown,
            self.deliveries,
            self.escalations,
            if self.busy {
                "; another tick holds lock"
            } else {
                ""
            }
        )
    }
}
#[derive(Default)]
struct Snapshot {
    lanes_running: Option<bool>,
    train_running: Option<bool>,
    ready: Option<bool>,
    work_waiting: Option<bool>,
    details: Vec<String>,
    failed_jobs: Vec<(u64, serde_json::Value)>,
    // trace:TASK-3075 | ai:codex
    extra: Vec<signals::Alarm>,
    empty_rework_members: Option<std::collections::BTreeSet<String>>,
}
impl Snapshot {
    fn predicates(&self) -> Vec<(&'static str, Option<bool>)> {
        let mut predicates = vec![
            (
                "pipeline-idle",
                and(&[
                    self.lanes_running.map(|v| !v),
                    self.ready.map(|v| !v),
                    self.work_waiting,
                ]),
            ),
            (
                "ready-without-train",
                and(&[self.ready, self.train_running.map(|v| !v)]),
            ),
        ];
        predicates.extend(self.extra.iter().map(|a| (a.kind, a.active)));
        predicates
    }
}
fn and(values: &[Option<bool>]) -> Option<bool> {
    if values.contains(&Some(false)) {
        Some(false)
    } else if values.contains(&None) {
        None
    } else {
        Some(true)
    }
}
fn any(values: &[Option<bool>]) -> Option<bool> {
    if values.contains(&Some(true)) {
        Some(true)
    } else if values.contains(&None) {
        None
    } else {
        Some(false)
    }
}

pub(super) fn run(
    root: &Path,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
) -> Result<Report> {
    run_with_home(
        root,
        crate::home_dir().as_deref(),
        execute,
        threshold_mins,
        send,
    )
}

// Alarm persistence failures must not terminate the oversight watch or hide its report.
pub(super) fn run_for_watch(
    root: &Path,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
) -> Report {
    run(root, execute, threshold_mins, send).unwrap_or_else(|e| Report {
        execute,
        unknown: vec![format!("pipeline alarm observation failed: {e:#}")],
        ..Default::default()
    })
}

fn run_with_home(
    root: &Path,
    home: Option<&Path>,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
) -> Result<Report> {
    anyhow::ensure!(
        threshold_mins >= 15,
        "alarm threshold must be at least 15 minutes"
    );
    // Read-only preview never creates a directory, lock, timer, eligibility cache or mail.
    let lock = if execute {
        let dir = root.join(".aida");
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("pipeline-stalls.lock"))?;
        if file.try_lock_exclusive().is_err() {
            return Ok(Report {
                execute,
                busy: true,
                ..Default::default()
            });
        }
        Some(file)
    } else {
        None
    };
    let snapshot = observe(root, home, threshold_mins);
    let report = tick(
        root,
        &snapshot,
        chrono::Utc::now().timestamp_millis(),
        execute,
        threshold_mins,
        send,
    );
    drop(lock); // permanent sidecar is never unlinked
    report
}
fn tick(
    root: &Path,
    snapshot: &Snapshot,
    now: i64,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
) -> Result<Report> {
    tick_with_writer(
        root,
        snapshot,
        now,
        execute,
        threshold_mins,
        send,
        &mut |path, bytes| Ok(aida_core::write_atomic(path, bytes)?),
    )
}
fn tick_with_writer(
    root: &Path,
    snapshot: &Snapshot,
    now: i64,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
    persist: &mut dyn FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Report> {
    tick_with_effects(
        root,
        snapshot,
        now,
        execute,
        threshold_mins,
        send,
        &mut |request| {
            let context = aida_core::mailbox_logical::MailboxContext::resolve(root, None)
                .map_err(|e| anyhow::anyhow!("action context unavailable: {e:?}"))?;
            let common = context
                .repository()
                .ok_or_else(|| anyhow::anyhow!("missing action repository"))?;
            aida_core::mailbox_lifecycle::action::latest_for_identity(
                common.path(),
                &request.message_id,
                &request.fingerprint,
            )
        },
        &mut |rule, body| {
            operator_delivery(crate::notify::send_direct_bounded(
                root,
                rule,
                "aida: pipeline stall escalation",
                body,
                Some(std::time::Duration::from_secs(10)),
            ))
        },
        persist,
    )
}

// All time and external effects are injected for deadline and crash-boundary tests.
// trace:TASK-3074 | ai:codex
#[allow(clippy::too_many_arguments)]
fn tick_with_effects(
    root: &Path,
    snapshot: &Snapshot,
    now: i64,
    execute: bool,
    threshold_mins: u32,
    send: &mut dyn FnMut(&str) -> Result<SendResult>,
    inspect: &mut dyn FnMut(
        &RequestIdentity,
    )
        -> Result<Option<aida_core::mailbox_lifecycle::action::ActionReceipt>>,
    notify: &mut dyn FnMut(&str, &str) -> Result<SendResult>,
    persist: &mut dyn FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Report> {
    let path = root.join(".aida/pipeline-stalls.json");
    let mut state = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<State>(&bytes).with_context(|| format!(
            "invalid pipeline alarm state {}; inspect, then remove the file to reset alarm tracking", path.display()
        ))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(
        state.version <= 2,
        "unsupported pipeline alarm state version {} in {}; inspect, then remove the file to reset alarm tracking",
        state.version, path.display()
    );
    let legacy = state.version == 1;
    state.version = 2;
    // Upgrade the former aggregate receipt episode without losing accepted or
    // unresolved deliveries. Receipt keys/heads contain no comma separators.
    // trace:TASK-3075 | ai:codex
    if let Some(episode) = state.episodes.remove("empty-rework-exit") {
        for member in episode.identity.split(',').filter(|id| !id.is_empty()) {
            let mut member_episode = episode.clone();
            member_episode.identity = member.into();
            state
                .episodes
                .entry(format!("empty-rework-exit:{member}"))
                .or_insert(member_episode);
        }
    }
    let mut report = Report {
        execute,
        evidence: snapshot
            .details
            .iter()
            .take(100)
            .map(|s| s.chars().take(1024).collect())
            .collect(),
        ..Default::default()
    };
    let mut closed = std::collections::BTreeSet::new();
    if legacy {
        state.episodes.retain(|key, episode| {
            if episode.request.is_none() {
                report
                    .unknown
                    .push(format!("{key}: action unknown; legacy episode closed"));
                closed.insert(key.clone());
                false
            } else {
                true
            }
        });
    }
    // Retire settled receipt/head episodes only on known evidence. Unresolved
    // delivery reservations survive even receipt deletion or PR closure.
    // trace:TASK-3075 | ai:codex
    if let Some(members) = &snapshot.empty_rework_members {
        state.episodes.retain(|key, episode| {
            !key.starts_with("empty-rework-exit:")
                || episode.delivery_pending
                || episode.escalation_pending
                || episode.persistent_escalation_pending
                || episode.deferral_escalation_pending
                || members.contains(&episode.identity)
        });
    }
    let mut escalation_due = Vec::new();
    let mut due_episodes = Vec::new();
    // Each empty-rework receipt/head owns its delivery state. Adding or removing
    // another blocked PR must never re-arm an accepted (or unresolved) head.
    // trace:TASK-3075 | ai:codex
    let observations = snapshot
        .predicates()
        .into_iter()
        .flat_map(|(kind, active)| {
            let identity = snapshot
                .extra
                .iter()
                .find(|a| a.kind == kind)
                .map_or("", |a| a.identity.as_str());
            if kind == "empty-rework-exit" && active == Some(true) {
                identity
                    .split(',')
                    .filter(|id| !id.is_empty())
                    .map(|member| (kind, active, member, format!("{kind}:{member}")))
                    .collect::<Vec<_>>()
            } else {
                vec![(kind, active, identity, kind.to_owned())]
            }
        });
    for (kind, active, identity, episode_key) in observations {
        if closed.contains(&episode_key) {
            continue;
        }
        let alarm = snapshot.extra.iter().find(|a| a.kind == kind);
        // Age/event delivery belongs to the evidence identity, not a continuous
        // run of true observations. Preserve accepted and unresolved deliveries
        // across unknown/false ticks (including CI reruns on an unchanged head).
        // trace:TASK-3075 | ai:codex
        let identity_keyed = alarm.is_some_and(|a| a.immediate);
        if active != Some(true) {
            if active == Some(false) && !identity_keyed {
                state.episodes.remove(kind);
            } else {
                for (key, episode) in &mut state.episodes {
                    if key == &episode_key || key.starts_with(&format!("{kind}:")) {
                        episode.continuity_ms = None;
                    }
                }
            }
            if active.is_none() {
                report.unknown.push(kind.into());
            }
            continue;
        }
        if !report.active.iter().any(|k| k == kind) {
            report.active.push(kind.into());
        }
        // Age/event predicates already include their own threshold. Identity changes
        // re-arm a new merge/mail/head episode without waiting for a clear tick.
        // trace:TASK-3075 | ai:codex
        let delay = if alarm.is_some_and(|a| a.immediate) {
            0
        } else {
            i64::from(threshold_mins) * 60_000
        };
        let episode = state
            .episodes
            .entry(episode_key.clone())
            .or_insert_with(|| Episode::new(now, identity));
        let gap = now < episode.observed_ms || now - episode.observed_ms > MAX_GAP_MS;
        if episode.identity != identity {
            *episode = Episode::new(now, identity);
        } else if gap {
            if !episode.accepted && !episode.delivery_pending {
                episode.first_ms = now;
            }
            episode.continuity_ms = Some(now);
        }
        if episode.continuity_ms.is_none() && !episode.accepted && !episode.delivery_pending {
            episode.first_ms = now;
        }
        let continuity = *episode.continuity_ms.get_or_insert(now);
        episode.observed_ms = now;
        if episode.escalation_pending
            || episode.persistent_escalation_pending
            || episode.deferral_escalation_pending
        {
            report
                .unknown
                .push(format!("{kind}: escalation outcome unresolved"));
        }
        if episode.delivery_pending {
            report.unknown.push(format!(
                "{kind}: prior delivery outcome unresolved; action unknown"
            ));
        }
        if episode.accepted || episode.delivery_pending {
            let receipt = episode.request.as_ref().map(&mut *inspect).transpose();
            let (escalation_kind, deadline, description) = match receipt {
                Ok(Some(Some(receipt))) => {
                    use aida_core::mailbox_lifecycle::action::Outcome;
                    let deadline = match receipt.outcome {
                        Outcome::Deferred => {
                            // A malformed receipt must not move its fallback deadline
                            // forward on every observation.
                            // trace:TASK-b6b1278890774ef993d2ebc327862470-868 | ai:codex
                            if receipt.until.is_none() {
                                report
                                    .unknown
                                    .push(format!("{kind}: deferred action missing deadline"));
                            }
                            receipt
                                .until
                                .unwrap_or(receipt.recorded_at.saturating_add(ESCALATE_MS))
                        }
                        Outcome::Acted | Outcome::Declined => {
                            receipt.recorded_at.saturating_add(ESCALATE_MS)
                        }
                    };
                    (
                        if receipt.outcome == Outcome::Deferred {
                            EscalationKind::Deferral
                        } else {
                            EscalationKind::Persistent
                        },
                        deadline,
                        format!(
                            "{}; {}",
                            if receipt.outcome == Outcome::Deferred {
                                if receipt.until.is_some() {
                                    "unresolved request; deferral deadline reached"
                                } else {
                                    "unresolved request; deferral deadline missing"
                                }
                            } else {
                                "persistent stall"
                            },
                            crate::mailbox_action::receipt_summary(&receipt)
                        ),
                    )
                }
                Ok(_) => (
                    EscalationKind::Unresolved,
                    episode
                        .delivered_ms
                        .unwrap_or(episode.first_ms)
                        .saturating_add(ESCALATE_MS),
                    if matches!(
                        episode.delivery,
                        Some(SendResult::Sent | SendResult::Queued)
                    ) {
                        "stall persists; no advisor request (operator-direct delivery)"
                    } else {
                        "unresolved request; action unknown"
                    }
                    .into(),
                ),
                Err(e) => {
                    report
                        .unknown
                        .push(format!("{kind}: action unavailable: {e:#}"));
                    (
                        EscalationKind::Unresolved,
                        episode
                            .delivered_ms
                            .unwrap_or(episode.first_ms)
                            .saturating_add(ESCALATE_MS),
                        "unresolved request; action unavailable".into(),
                    )
                }
            };
            let (pending, sent) = episode.escalation_slot(escalation_kind);
            if !*pending
                && sent.is_none()
                && now >= deadline
                && now.saturating_sub(continuity) >= ESCALATE_MS
            {
                escalation_due.push((episode_key.clone(), escalation_kind, description));
            }
        }
        if now - episode.first_ms >= delay && !episode.accepted && !episode.delivery_pending {
            due_episodes.push(episode_key);
            if !report.due.iter().any(|k| k == kind) {
                report.due.push(kind.into());
            }
        }
    }
    // Missing detector observations interrupt continuity too, even within a short gap.
    for episode in state.episodes.values_mut() {
        if episode.observed_ms != now {
            episode.continuity_ms = None;
        }
    }
    if execute && !report.due.is_empty() {
        let mut details = snapshot.details.clone();
        details.extend(failure_details(&snapshot.failed_jobs, &mut |job| {
            if cfg!(test) {
                return None;
            }
            let gh = crate::resolve_gh_binary()?;
            let mut cmd = std::process::Command::new(gh);
            cmd.current_dir(root).args([
                "api",
                &format!("repos/{{owner}}/{{repo}}/actions/jobs/{job}/logs"),
            ]);
            let crate::BoundedCommandOutput::Completed(out) =
                crate::command_output_with_timeout_capped(
                    cmd,
                    std::time::Duration::from_secs(5),
                    1024 * 1024,
                )
            else {
                return None;
            };
            (out.status.success() && out.stdout.len() <= 1024 * 1024)
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }));
        details.extend(
            due_episodes
                .iter()
                .map(|key| format!("Episode {}: {key}", state.episodes[key].uuid)),
        );
        let body = format!("Pipeline needs attention: {} (continuous alarms: >= {threshold_mins} minutes; age/event alarms: see evidence).\n{}\nInspect: aida ps; aida queue list --for implementer; aida integrate\nNo lanes, leases or PRs were changed by this alarm.",
            report.due.join(", "), bounded_digest(&details));
        // Reserve durably before external delivery. A failed post-send write leaves
        // an unresolved reservation, preventing duplicate urgent mail on later ticks.
        for key in &due_episodes {
            let episode = state.episodes.get_mut(key).unwrap();
            episode.delivery_pending = true;
            episode.delivered_ms = Some(now);
        }
        persist(&path, &serde_json::to_vec_pretty(&state)?)?;
        let delivered = match send(&body) {
            Ok(result) => result,
            Err(error) => {
                report
                    .unknown
                    .push(format!("delivery outcome uncertain: {error:#}"));
                SendResult::Uncertain
            }
        };
        for key in &due_episodes {
            let episode = state.episodes.get_mut(key).unwrap();
            episode.delivery_pending = delivered == SendResult::Uncertain;
            episode.accepted = matches!(
                delivered,
                SendResult::Persisted { .. } | SendResult::Sent | SendResult::Queued
            );
            if let SendResult::Persisted { request } = &delivered {
                episode.request = Some(request.clone());
            }
            episode.delivery = Some(delivered.clone());
            report.deliveries.insert(key.clone(), delivered.clone());
        }
        if matches!(delivered, SendResult::Persisted { .. } | SendResult::Sent) {
            report.accepted = report.due.clone();
        } else {
            report.unknown.push(format!("delivery: {delivered:?}"));
        }
    }
    for (key, escalation_kind, description) in escalation_due {
        let episode = state.episodes.get_mut(&key).unwrap();
        let rule = format!(
            "pipeline_escalation_{}_{}",
            episode.uuid,
            escalation_kind.suffix()
        );
        let body = format!(
            "Pipeline episode {} ({key}): {description}. Request: {}.\n{}",
            episode.uuid,
            episode.request.as_ref().map_or(
                if matches!(
                    episode.delivery,
                    Some(SendResult::Sent | SendResult::Queued)
                ) {
                    "none (operator-direct delivery)"
                } else {
                    "unknown"
                },
                |r| r.message_id.as_str()
            ),
            bounded_digest(&snapshot.details)
        );
        if execute {
            *episode.escalation_slot(escalation_kind).0 = true;
            persist(&path, &serde_json::to_vec_pretty(&state)?)?;
            let outcome = match notify(&rule, &body) {
                Ok(result) => result,
                Err(error) => {
                    report
                        .unknown
                        .push(format!("escalation outcome uncertain: {error:#}"));
                    SendResult::Uncertain
                }
            };
            let episode = state.episodes.get_mut(&key).unwrap();
            let (pending, sent) = episode.escalation_slot(escalation_kind);
            *pending = outcome == SendResult::Uncertain;
            if matches!(outcome, SendResult::Sent | SendResult::Queued) {
                *sent = Some(outcome.clone());
            }
            report.escalations.insert(key, outcome);
        } else {
            report.evidence.push(format!("escalation due: {body}"));
        }
    }
    if execute {
        persist(&path, &serde_json::to_vec_pretty(&state)?)?;
    }
    Ok(report)
}

fn bounded_digest(lines: &[String]) -> String {
    let mut body = String::new();
    for line in lines {
        let line: String = line.chars().take(1024).collect();
        if body.len() + line.len() > 24_000 {
            body.push_str("\nAdditional evidence omitted; inspect aida ps / aida integrate.\n");
            break;
        }
        body.push_str(&line);
        body.push('\n');
    }
    body
}

// Parse every entry: best-effort registry lists cannot establish a negative.
fn registry(root: &Path) -> Result<Vec<crate::agent_registry::AgentRegistryEntry>> {
    let mut out = Vec::new();
    for path in files(&crate::agent_registry::agents_dir(root))? {
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            out.push(toml::from_str(&std::fs::read_to_string(path)?)?);
        }
    }
    Ok(out)
}
fn files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| Ok(e?.path())).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
fn lane_role(role: Option<&str>) -> bool {
    // Only implementation/review lanes suppress idle; interactive observers do not.
    matches!(role, Some("implementer" | "reviewer"))
}
fn observe(root: &Path, home: Option<&Path>, threshold_mins: u32) -> Snapshot {
    let mut s = Snapshot::default();
    let observer = aida_core::pid_observer::PidObserver::current();
    let leases = crate::leases::list_leases_strict_all(root);
    let entries = registry(root);
    let implementers =
        signals::implementer_count(root, leases.as_deref().ok(), entries.as_deref().ok());
    if let (Ok(leases), Ok(entries)) = (&leases, &entries) {
        let mut alive = Vec::new();
        for l in leases.iter().filter(|l| lane_role(l.role.as_deref())) {
            let live = if l.hostname != crate::coordination::hostname()
                || !observer.can_observe(l.pid_namespace)
            {
                None
            } else {
                // Prefer the child identity; legacy leases retain their creator identity.
                l.active_pid.or(l.creator_pid).map(|pid| {
                    crate::process_probe::process_identity_is_alive(
                        pid,
                        if l.active_pid.is_some() {
                            l.active_pid_start_time.as_deref()
                        } else {
                            l.creator_pid_start_time.as_deref()
                        },
                    )
                })
            };
            alive.push(live);
            if live == Some(false) {
                s.details.push(format!("Lease {} ({}, branch {}): recorded worker exited; inspect aida session show {}; cleanup: aida session end {}", l.id, l.scope, l.branch, l.id, l.id));
            }
        }
        alive.extend(
            entries
                .iter()
                .filter(|e| lane_role(e.role.as_deref()))
                .map(|e| crate::agent_registry::observed_liveness_from(root, e, observer)),
        );
        s.lanes_running = any(&alive);
    }
    if s.lanes_running.is_none() {
        s.details.push("Worker liveness unknown: unreadable lease/registry data, foreign host/namespace, or missing process identity; run the tick on the host and inspect aida ps".into());
    }
    s.train_running = crate::merge_lock::observed_train_running(root);
    let forge = forge_snapshot(root);
    if let Some((prs, raw)) = &forge {
        observe_prs(root, home, prs, raw, &mut s);
    }
    let now = chrono::Utc::now().timestamp_millis();
    (s.extra, s.empty_rework_members) = signals::observe(
        root,
        forge.as_ref().map(|(prs, _)| prs),
        implementers,
        queue_waiting_for(root, home, true),
        threshold_mins,
        now,
        &mut s.details,
    );
    // A merge train is itself an active pipeline lane even without an agent row.
    s.lanes_running = any(&[s.lanes_running, s.train_running]);
    if s.ready.is_none() {
        s.details.push("PR readiness unknown: unsupported forge, incomplete snapshot, unreadable review or missing CI/mergeability evidence".into());
    }
    if s.train_running.is_none() {
        s.details
            .push("Train liveness unknown: unreadable or foreign merge-lease owner".into());
    }
    s
}

// Shared snapshot processing keeps digest, readiness and deferred log selection aligned.
fn observe_prs(
    root: &Path,
    home: Option<&Path>,
    prs: &crate::status_prs::OpenPrSnapshot,
    raw: &[serde_json::Value],
    s: &mut Snapshot,
) {
    let mut readiness = Vec::new();
    let mut blocked = false;
    let mut train = vec![s.train_running];
    for pr in prs.by_branch.values() {
        let row = raw
            .iter()
            .find(|r| r["number"].as_u64() == Some(pr.number))
            .unwrap();
        if pr.head_branch.starts_with("train/") {
            // An open train with unknown owner is never evidence of no train.
            train.push(train_pr_running(root, pr.number, &mut s.details));
            continue;
        }
        let ready = if hold_gate_blocking(row) {
            Some(false)
        } else {
            pr_ready(root, pr)
        };
        readiness.push(ready);
        let rejected = pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
            || current_rejection(root, pr);
        blocked |= pr.ci_rollup.as_deref() == Some("fail") || rejected;
        if pr.ci_rollup.as_deref() == Some("fail") {
            s.failed_jobs.push((pr.number, row.clone()));
        }
        s.details.extend(pr_digest(root, pr, row, ready, rejected));
    }
    s.ready = any(&readiness);
    s.train_running = any(&train);
    s.work_waiting = any(&[Some(blocked), queue_waiting(root, home)]);
}

fn queue_waiting(root: &Path, home: Option<&Path>) -> Option<bool> {
    queue_waiting_for(root, home, false)
}

// trace:TASK-3075 | ai:codex
fn queue_waiting_for(root: &Path, home: Option<&Path>, implementer_only: bool) -> Option<bool> {
    // Do not shell out to queue list: it opportunistically garbage-collects queues.
    let mut present = Vec::new();
    for path in files(&root.join(".aida-store/registry/queues")).ok()? {
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let bytes = std::fs::read(path).ok()?;
        let rows: Vec<aida_core::QueueEntry> = serde_yaml::from_slice(&bytes).ok()?;
        present.push(Some(rows.iter().any(|r| {
            !implementer_only
                || r.for_role
                    .as_deref()
                    .is_none_or(|role| role == "implementer")
        })));
    }
    if let Some(home) = home {
        let path = home.join(".aida/queue/implementer.yaml");
        let rows = crate::global_queue::read_queue(&path, "implementer").ok()?;
        present
            .push(Some(rows.iter().any(|r| {
                crate::main_worktree_root_from(&r.project_root) == root
            })));
    } else {
        present.push(None);
    }
    any(&present)
}
fn forge_snapshot(
    root: &Path,
) -> Option<(crate::status_prs::OpenPrSnapshot, Vec<serde_json::Value>)> {
    // The general forge resolver can repair config; observations must not.
    let kind = crate::forge::read_forge_config(root).or_else(|| {
        crate::forge::origin_url(root).map(|url| crate::forge::detect_forge_kind(&url))
    });
    if kind != Some(crate::forge::ForgeKind::GitHub) {
        return None;
    }
    let gh = crate::resolve_gh_binary()?;
    let mut cmd = std::process::Command::new(gh);
    cmd.current_dir(root).args(["pr", "list", "--state", "open", "--limit", "1000", "--json",
        "number,title,headRefName,headRefOid,statusCheckRollup,mergeable,reviewDecision,labels,isDraft,baseRefName,body"]);
    let out = crate::command_output_with_timeout(cmd, crate::FORGE_CLI_CALL_TIMEOUT)?;
    if !out.status.success() {
        return None;
    }
    let raw: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    // Saturated/truncated or malformed snapshots cannot prove no ready PR.
    if raw.len() >= 1000
        || raw.iter().any(|r| {
            r["number"].as_u64().is_none()
                || ["title", "headRefName", "headRefOid", "body"]
                    .iter()
                    .any(|k| r[k].as_str().is_none())
                || r["isDraft"].as_bool().is_none()
                || r["labels"].as_array().is_none()
                || r["reviewDecision"].as_str().is_none()
                || r["mergeable"].as_str().is_none()
                || r["statusCheckRollup"].as_array().is_none()
        })
    {
        return None;
    }
    let required = crate::status_prs::github_required_status_checks_uncached(root);
    let prs = parse_stall_prs(&raw, required.as_deref());
    if prs.by_branch.len() != raw.len() {
        return None;
    }
    Some((prs, raw))
}
// Latest attempts are the detector's evidence for both classification and logs.
// A hold gate is a lifecycle signal (pr_has_merge_hold), not a failing test.
// trace:STORY-2476 | ai:codex
fn check_name(check: &serde_json::Value) -> Option<&str> {
    check["name"].as_str().or(check["context"].as_str())
}
// trace:TASK-3112 | ai:codex
fn latest_checks(row: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut latest = BTreeMap::new();
    let mut unnamed = Vec::new();
    for check in row["statusCheckRollup"].as_array().into_iter().flatten() {
        let Some(name) = check_name(check) else {
            unnamed.push(check);
            continue;
        };
        // Compare parsed instants, including offsets. Missing timestamps retain
        // input order as a tie-breaker; a newer queued attempt supersedes old red.
        let time = |c: &serde_json::Value| {
            ["completedAt", "startedAt", "createdAt"]
                .iter()
                .filter_map(|key| c[key].as_str())
                .filter_map(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .max()
        };
        let previous = latest
            .entry((check["workflowName"].as_str(), name))
            .or_insert(check);
        if time(check) >= time(previous) {
            *previous = check;
        }
    }
    unnamed.extend(latest.into_values());
    unnamed
}
fn ci_checks(row: &serde_json::Value) -> Vec<&serde_json::Value> {
    latest_checks(row)
        .into_iter()
        .filter(|c| check_name(c) != Some("merge-hold-gate"))
        .collect()
}
fn hold_gate_blocking(row: &serde_json::Value) -> bool {
    latest_checks(row)
        .into_iter()
        // trace:BUG-2492 | ai:codex — commit statuses replaced failed hold jobs.
        .any(|c| {
            check_name(c).is_some_and(|name| {
                crate::ci_gate::check_name_eq(name, crate::ci_gate::HOLD_GATE_CHECK)
            }) && (check_failed(c)
                || c["state"] == "PENDING"
                || matches!(
                    c["status"].as_str(),
                    Some("PENDING" | "QUEUED" | "IN_PROGRESS")
                ))
        })
}
fn check_failed(check: &serde_json::Value) -> bool {
    matches!(
        check["conclusion"].as_str().or(check["state"].as_str()),
        Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED")
    )
}

// Keep shared status displays unchanged and required-check unknown guards intact.
fn parse_stall_prs(
    raw: &[serde_json::Value],
    required: Option<&[String]>,
) -> crate::status_prs::OpenPrSnapshot {
    let mut normalized = raw.to_vec();
    for row in &mut normalized {
        let checks: Vec<_> = ci_checks(row).into_iter().cloned().collect();
        row["statusCheckRollup"] = checks.into();
        if let Some(checks) = row["statusCheckRollup"].as_array_mut() {
            for check in checks {
                // StatusContext uses state/context rather than conclusion/name.
                if let Some(state) = check["state"].as_str() {
                    match state {
                        "SUCCESS" => check["conclusion"] = "SUCCESS".into(),
                        "FAILURE" | "ERROR" => check["conclusion"] = "FAILURE".into(),
                        "PENDING" => check["status"] = "PENDING".into(),
                        _ => {}
                    }
                }
                if check["status"] == "COMPLETED"
                    && matches!(check["conclusion"].as_str(), Some("SKIPPED" | "NEUTRAL"))
                {
                    check["conclusion"] = "SUCCESS".into();
                }
            }
        }
    }
    let ci_required = required.map(|names| {
        names
            .iter()
            .filter(|n| n.as_str() != "merge-hold-gate")
            .cloned()
            .collect::<Vec<_>>()
    });
    crate::status_prs::parse_open_pr_snapshot(
        &serde_json::to_string(&normalized).unwrap(),
        ci_required.as_deref(),
    )
}

fn train_pr_running(root: &Path, number: u64, details: &mut Vec<String>) -> Option<bool> {
    let key = crate::ownership::WorkKey::Train {
        train: crate::ownership::dual::change(number),
    };
    if crate::ownership::reader::ended(root, &key)
        .ok()
        .flatten()
        .is_some()
    {
        Some(false)
    } else {
        details.push(format!("Train PR-{number}: owner is live or liveness is unknown; inspect: gh pr view {number}; inspect train lease with aida integrate from {}", root.display()));
        None
    }
}

// Fetch at most four job logs per alarm, only when delivery is due. Job IDs
// come from GitHub's snapshot, never from executing stored URLs or shell text.
fn failure_details(
    prs: &[(u64, serde_json::Value)],
    fetch: &mut dyn FnMut(u64) -> Option<String>,
) -> Vec<String> {
    let mut details = Vec::new();
    let mut remaining = 4;
    for (pr, row) in prs {
        for check in ci_checks(row) {
            if !check_failed(check) {
                continue;
            }
            let name = check_name(check).unwrap_or("unknown check");
            let job = check["detailsUrl"]
                .as_str()
                .and_then(|url| url.split_once("/job/"))
                .and_then(|(_, tail)| tail.split(['/', '?', '#']).next())
                .and_then(|id| id.parse::<u64>().ok());
            let lines = if remaining > 0 {
                job.and_then(|id| {
                    remaining -= 1;
                    fetch(id)
                })
                .map(|log| crate::ci_failures::extract_failure_lines(&log))
            } else {
                None
            };
            match lines.filter(|lines| !lines.is_empty()) {
                Some(lines) => details.extend(lines.into_iter().map(|line| format!("PR-{pr} {name}: {line}"))),
                None => details.push(format!("PR-{pr} {name}: failing tests unavailable (log missing, no test failures, or alarm log budget exhausted); inspect: gh pr checks {pr}")),
            }
        }
    }
    details
}

fn pr_digest(
    root: &Path,
    pr: &crate::status_cleanup::OpenPrItem,
    row: &serde_json::Value,
    ready: Option<bool>,
    rejected: bool,
) -> Vec<String> {
    let n = pr.number;
    if ready == Some(true) {
        return vec![format!("PR-{n}: ship-ready; aida pr ship {n}")];
    }
    let mut blockers = Vec::new();
    if pr.is_draft {
        blockers.push(format!("draft; gh pr ready {n}"));
    }
    if crate::pr_has_merge_hold(root, pr) || hold_gate_blocking(row) {
        blockers.push(format!("merge hold; aida merge-hold auto-release {n} (eligible Rework approval) or human/advisor: aida merge-hold clear {n}"));
    }
    let checks: Vec<_> = ci_checks(row)
        .into_iter()
        .filter(|c| check_failed(c))
        .filter_map(check_name)
        .collect();
    if rejected
        || pr.ci_rollup.as_deref() == Some("fail")
        || pr.mergeable.as_deref() == Some("CONFLICTING")
    {
        let ids = crate::pr_ship::extract_spec_ids_from_text(&pr.title);
        let rework = if ids.is_empty() {
            format!("gh pr view {n} (identify owning spec for aida queue rework)")
        } else {
            ids.iter()
                .map(|id| format!("aida queue rework {id}"))
                .collect::<Vec<_>>()
                .join("; ")
        };
        if rejected {
            blockers.push(format!("current-head request-changes; {rework}"));
        }
        if pr.ci_rollup.as_deref() == Some("fail") {
            blockers.push(format!(
                "red CI [{}]; {rework}; gh pr checks {n}",
                checks.join(", ")
            ));
        }
        if pr.mergeable.as_deref() == Some("CONFLICTING") {
            blockers.push(format!("merge conflict; {rework}"));
        }
    }
    match pr.ci_rollup.as_deref() {
        Some("pending") => blockers.push(format!("CI pending; gh pr checks {n}")),
        Some("missing" | "unknown" | "?") | None => {
            blockers.push(format!("CI evidence missing/unknown; gh pr checks {n}"))
        }
        _ => {}
    }
    if pr.mergeable.as_deref() != Some("MERGEABLE")
        && pr.mergeable.as_deref() != Some("CONFLICTING")
    {
        blockers.push(format!(
            "mergeability unknown; gh pr view {n} --json mergeable"
        ));
    }
    if blockers.is_empty() {
        blockers.push(format!("current-head review missing, active or unreadable; aida queue work PR-{n} --role reviewer"));
    }
    vec![format!(
        "PR-{n} head {}: {}",
        pr.head_sha.as_deref().unwrap_or("unknown"),
        blockers.join("; ")
    )]
}

fn current_rejection(root: &Path, pr: &crate::status_cleanup::OpenPrItem) -> bool {
    verdict_ids(pr)
        .iter()
        .filter_map(|id| crate::gate::review_verdict::read_recorded_verdict(root, id))
        .any(|v| {
            !v.is_closed()
                && matches!(
                    v.kind,
                    crate::gate::review_verdict::VerdictKind::RequestChanges
                        | crate::gate::review_verdict::VerdictKind::Rejected
                )
                && v.reviewed_sha.as_deref() == pr.head_sha.as_deref()
        })
}
fn verdict_ids(pr: &crate::status_cleanup::OpenPrItem) -> Vec<String> {
    let mut ids = crate::pr_ship::extract_spec_ids_from_text(&pr.title);
    ids.push(format!("PR-{}", pr.number));
    ids
}
fn pr_ready(root: &Path, pr: &crate::status_cleanup::OpenPrItem) -> Option<bool> {
    if pr.is_draft
        || crate::pr_has_merge_hold(root, pr)
        || pr.mergeable.as_deref() == Some("CONFLICTING")
        || matches!(pr.ci_rollup.as_deref(), Some("fail" | "pending"))
        || pr.review_decision.as_deref() == Some("CHANGES_REQUESTED")
        || current_rejection(root, pr)
    {
        return Some(false);
    }
    if pr.mergeable.as_deref() != Some("MERGEABLE") || pr.ci_rollup.as_deref() != Some("pass") {
        return None;
    }
    let head = pr.head_sha.as_deref().filter(|h| !h.is_empty())?;
    // Refused or malformed evidence cannot establish absence of approval.
    for id in verdict_ids(pr) {
        let path = root
            .join(".aida/review-verdicts")
            .join(format!("{id}.json"));
        if crate::gate::review_verdict::verdict_file_refused(&path) {
            return None;
        }
        if let Some(current) = crate::gate::review_verdict::current_verdict(root, &id) {
            crate::gate::review_verdict::parse_recorded_verdict(&current.body)?;
        }
    }
    Some(
        crate::idle_eligible::local_kind(root, pr.number, head, &pr.title, &pr.head_branch)
            .is_some_and(|(kind, _)| kind == "ship-ready"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIN: i64 = 60_000;
    fn idle() -> Snapshot {
        Snapshot {
            lanes_running: Some(false),
            train_running: Some(false),
            ready: Some(false),
            work_waiting: Some(true),
            ..Default::default()
        }
    }
    fn at(
        root: &Path,
        s: &Snapshot,
        minutes: i64,
        send: &mut dyn FnMut(&str) -> Result<SendResult>,
    ) -> Report {
        tick(root, s, minutes * MIN, true, 30, send).unwrap()
    }
    // trace:STORY-2476 | ai:codex
    #[test]
    fn nondefault_threshold_controls_due_time_and_digest() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        for minute in [0, 10, 20, 30, 40, 44] {
            assert!(tick(t.path(), &idle(), minute * MIN, true, 45, &mut |_| {
                panic!("alarm fired before configured threshold")
            })
            .unwrap()
            .due
            .is_empty());
        }
        let report = tick(t.path(), &idle(), 45 * MIN, true, 45, &mut |body| {
            assert!(body.contains(">= 45 minutes"));
            Ok(SendResult::Sent)
        })
        .unwrap();
        assert_eq!(report.accepted, ["pipeline-idle"]);
    }

    #[test]
    fn watch_reports_unwritable_alarm_state_as_unknown() {
        let t = tempfile::tempdir().unwrap();
        // A directory at the lock path refuses open even for a privileged test user.
        std::fs::create_dir_all(t.path().join(".aida/pipeline-stalls.lock")).unwrap();
        let report = run_for_watch(t.path(), true, 30, &mut |_| panic!("must not send"));
        assert!(report.accepted.is_empty());
        assert_eq!(report.unknown.len(), 1);
        assert!(report.summary().contains("observation failed"));
        assert!(!t.path().join(".aida/pipeline-stalls.json").exists());
    }

    #[test]
    fn only_implementer_and_reviewer_roles_suppress_idle() {
        for role in [None, Some("integrator"), Some("advisor"), Some("product")] {
            assert!(!lane_role(role), "{role:?}");
        }
        for role in [Some("implementer"), Some("reviewer")] {
            assert!(lane_role(role), "{role:?}");
        }
        let t = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let leases = crate::leases::leases_dir(t.path());
        std::fs::create_dir_all(&leases).unwrap();
        // A live role-less interactive lease must not mask an idle pipeline.
        let lease = format!(
            r#"
            id = "interactive"
            scope = "STORY-2476"
            slug = "interactive"
            owner = "test"
            worktree_path = {}
            branch = "interactive"
            started_at = "2026-11-04T00:00:00Z"
            hostname = "{}"
            creator_pid = {}
        "#,
            toml::Value::String(t.path().to_string_lossy().into_owned()),
            crate::coordination::hostname(),
            std::process::id()
        );
        std::fs::write(leases.join("interactive.toml"), lease).unwrap();
        let parsed = crate::leases::list_leases_strict_all(t.path()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].role.is_none());
        assert_eq!(
            observe(t.path(), Some(home.path()), 30).lanes_running,
            Some(false)
        );
    }

    #[test]
    fn boundary_dedup_retry_and_episode_reset() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut sent = 0;
        let mut send = |body: &str| {
            assert!(body.contains("pipeline-idle"));
            sent += 1;
            Ok(SendResult::Sent)
        };
        for minute in [0, 10, 20, 29] {
            assert!(at(t.path(), &idle(), minute, &mut send).due.is_empty());
        }
        let before = std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap();
        let dry = tick(t.path(), &idle(), 30 * MIN, false, 30, &mut |_| {
            panic!("dry-run sent")
        })
        .unwrap();
        assert_eq!(dry.due, ["pipeline-idle"]);
        assert_eq!(
            before,
            std::fs::read(t.path().join(".aida/pipeline-stalls.json")).unwrap()
        );
        assert!(at(t.path(), &idle(), 30, &mut |_| Ok(SendResult::Refused))
            .accepted
            .is_empty());
        assert!(at(t.path(), &idle(), 31, &mut |_| Ok(SendResult::Refused))
            .accepted
            .is_empty());
        assert_eq!(
            at(t.path(), &idle(), 32, &mut send).accepted,
            ["pipeline-idle"]
        );
        assert!(at(t.path(), &idle(), 33, &mut send).due.is_empty());
        assert_eq!(sent, 1);
        let mut s = idle();
        s.lanes_running = Some(true);
        assert!(at(t.path(), &s, 34, &mut |_| panic!()).active.is_empty());
        assert!(at(t.path(), &idle(), 35, &mut |_| panic!()).due.is_empty());
    }
    #[test]
    fn unknown_long_gap_and_clock_reversal_restart_timer() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let mut no = |_: &str| -> Result<SendResult> { panic!("not due") };
        at(t.path(), &idle(), 0, &mut no);
        let mut unknown = idle();
        unknown.lanes_running = None;
        assert_eq!(
            at(t.path(), &unknown, 10, &mut no).unknown,
            ["pipeline-idle"]
        );
        for minute in [20, 30, 40] {
            assert!(at(t.path(), &idle(), minute, &mut no).due.is_empty());
        }
        assert!(at(t.path(), &idle(), 60, &mut no).due.is_empty()); // gap >15m
        assert!(at(t.path(), &idle(), 59, &mut no).due.is_empty()); // backwards
        assert!(at(t.path(), &idle(), 70, &mut no).due.is_empty());
    }
    #[test]
    fn alarms_have_independent_suppression() {
        let mut s = idle();
        assert_eq!(
            s.predicates(),
            [
                ("pipeline-idle", Some(true)),
                ("ready-without-train", Some(false))
            ]
        );
        s.ready = Some(true);
        s.lanes_running = Some(true);
        assert_eq!(
            s.predicates(),
            [
                ("pipeline-idle", Some(false)),
                ("ready-without-train", Some(true))
            ]
        );
        s.train_running = Some(true);
        assert_eq!(s.predicates()[1].1, Some(false));
        s.train_running = None;
        assert_eq!(s.predicates()[1].1, None);
        s.ready = Some(false);
        s.lanes_running = Some(false);
        s.work_waiting = Some(false);
        assert_eq!(s.predicates()[0].1, Some(false));
        s.work_waiting = None;
        assert_eq!(s.predicates()[0].1, None);
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        s.ready = Some(true);
        s.train_running = Some(false);
        s.lanes_running = None;
        let mut send = |_: &str| Ok(SendResult::Sent);
        for m in [0, 10, 20] {
            at(t.path(), &s, m, &mut send);
        }
        assert_eq!(
            at(t.path(), &s, 30, &mut send).accepted,
            ["ready-without-train"]
        );
    }
    // trace:STORY-2476 | ai:codex
    #[test]
    fn delivery_reservation_prevents_send_on_write_failure_and_duplicates_after_send() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        for minute in [0, 10, 20] {
            at(t.path(), &idle(), minute, &mut |_| panic!());
        }
        assert!(tick_with_writer(
            t.path(),
            &idle(),
            30 * MIN,
            true,
            30,
            &mut |_| panic!("must persist before send"),
            &mut |_, _| anyhow::bail!("injected reservation write failure")
        )
        .is_err());
        let mut writes = 0;
        let mut sent = 0;
        assert!(tick_with_writer(
            t.path(),
            &idle(),
            30 * MIN,
            true,
            30,
            &mut |_| {
                sent += 1;
                Ok(SendResult::Sent)
            },
            &mut |path, bytes| {
                writes += 1;
                if writes == 2 {
                    anyhow::bail!("injected post-send write failure");
                }
                Ok(aida_core::write_atomic(path, bytes)?)
            }
        )
        .is_err());
        assert_eq!(sent, 1);
        let report = at(t.path(), &idle(), 31, &mut |_| panic!("duplicate mail"));
        assert!(report.due.is_empty());
        assert!(report
            .unknown
            .iter()
            .any(|s| s.contains("outcome unresolved")));
    }

    #[test]
    fn skipped_neutral_checks_allow_ready_but_unknown_ci_never_hides_rejection() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".aida/review-verdicts")).unwrap();
        let path = t.path().join(".aida/review-verdicts/PR-42.json");
        let raw = serde_json::json!([{
            "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
            "mergeable":"MERGEABLE","isDraft":false,
            "statusCheckRollup":[
                {"name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                {"name":"Release","status":"COMPLETED","conclusion":"SKIPPED"},
                {"name":"optional","status":"COMPLETED","conclusion":"NEUTRAL"}]
        }]);
        let mut p = parse_stall_prs(raw.as_array().unwrap(), Some(&["build".into()]))
            .by_branch
            .remove("topic")
            .unwrap();
        std::fs::write(
            &path,
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(t.path(), &p), Some(true));
        let missing = parse_stall_prs(raw.as_array().unwrap(), Some(&["missing".into()]));
        assert_eq!(
            missing.by_branch["topic"].ci_rollup.as_deref(),
            Some("missing")
        );
        assert_eq!(
            parse_stall_prs(raw.as_array().unwrap(), None).by_branch["topic"]
                .ci_rollup
                .as_deref(),
            Some("unknown")
        );
        p.ci_rollup = Some("?".into());
        std::fs::write(
            &path,
            r#"{"verdict":"request-changes","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(t.path(), &p), Some(false));
        let mut s = idle();
        s.ready = pr_ready(t.path(), &p);
        assert_eq!(s.predicates()[0].1, Some(true));
    }

    // trace:STORY-2476 | ai:codex
    #[test]
    fn held_green_pr_is_not_red_ci_and_never_fetches_hold_logs() {
        let t = tempfile::tempdir().unwrap();
        let row = serde_json::json!({
            "number":42,"title":"work (STORY-2476)","headRefName":"topic",
            "headRefOid":"abcdef1234","mergeable":"MERGEABLE","isDraft":false,
            "labels":[{"name":"aida:merge-hold-recorded"}],
            "statusCheckRollup":[
                {"name":"merge-hold-gate","status":"COMPLETED","conclusion":"FAILURE",
                 "detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"},
                {"name":"Build (ubuntu-latest)","status":"COMPLETED","conclusion":"SUCCESS"}]
        });
        let raw = vec![row.clone()];
        let required = vec!["merge-hold-gate".into(), "Build (ubuntu-latest)".into()];
        let prs = parse_stall_prs(&raw, Some(&required));
        assert_eq!(prs.by_branch["topic"].ci_rollup.as_deref(), Some("pass"));
        let mut s = idle();
        observe_prs(t.path(), Some(t.path()), &prs, &raw, &mut s);
        assert_eq!(s.ready, Some(false));
        assert_eq!(s.work_waiting, Some(false));
        assert!(s.failed_jobs.is_empty());
        let digest = s.details.join("\n");
        assert!(digest.contains("merge hold"));
        assert!(!digest.contains("queue rework"));
        assert!(!digest.contains("red CI"));
        assert!(failure_details(&[(42, row)], &mut |_| panic!("hold log fetched")).is_empty());
    }

    // trace:BUG-2492 | ai:codex
    #[test]
    fn bug2492_pending_hold_status_blocks_readiness_without_red_ci_or_logs() {
        let t = tempfile::tempdir().unwrap();
        for state in ["PENDING", "FAILURE", "SUCCESS"] {
            let row = serde_json::json!({
                "number":42,"title":"work","headRefName":"topic",
                "headRefOid":"abcdef1234","mergeable":"MERGEABLE","isDraft":false,
                "labels":[],
                "statusCheckRollup":[
                    {"__typename":"StatusContext","context":"merge-hold-gate","state":state},
                    {"name":"Build","status":"COMPLETED","conclusion":"SUCCESS"}]
            });
            assert_eq!(hold_gate_blocking(&row), state != "SUCCESS");
            let raw = vec![row.clone()];
            let prs = parse_stall_prs(&raw, Some(&["merge-hold-gate".into(), "Build".into()]));
            assert_eq!(prs.by_branch["topic"].ci_rollup.as_deref(), Some("pass"));
            let mut s = idle();
            observe_prs(t.path(), Some(t.path()), &prs, &raw, &mut s);
            if state != "SUCCESS" {
                assert_eq!(s.ready, Some(false));
                assert!(s.details.join("\n").contains("merge hold"));
            }
            assert!(s.failed_jobs.is_empty());
            assert!(failure_details(&[(42, row)], &mut |_| panic!("hold log fetched")).is_empty());
        }
    }

    // trace:TASK-3112 | ai:codex
    #[test]
    fn task3112_same_named_jobs_in_different_workflows_remain_distinct() {
        let row = serde_json::json!({"number":42,"headRefName":"topic","statusCheckRollup":[
            {"workflowName":"A","name":"test","status":"COMPLETED","conclusion":"FAILURE","completedAt":"2026-11-05T00:00:00Z"},
            {"workflowName":"B","name":"test","status":"COMPLETED","conclusion":"SUCCESS","completedAt":"2026-11-05T01:00:00Z"}
        ]});
        assert_eq!(ci_checks(&row).len(), 2);
        assert!(ci_checks(&row).iter().any(|c| check_failed(c)));
        let prs = parse_stall_prs(&[row], None);
        assert!(prs
            .by_branch
            .values()
            .any(|p| p.ci_rollup.as_deref() == Some("fail")));
    }

    #[test]
    fn latest_check_attempt_controls_readiness_digest_and_log_selection() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join(".aida/review-verdicts")).unwrap();
        std::fs::write(
            t.path().join(".aida/review-verdicts/PR-42.json"),
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        let mut row = serde_json::json!({
            "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
            "mergeable":"MERGEABLE","isDraft":false,
            "statusCheckRollup":[
                {"name":"build","status":"COMPLETED","conclusion":"SUCCESS",
                 "startedAt":"2026-11-05T05:00:00Z","completedAt":"2026-11-05T05:01:00Z"},
                {"name":"build","status":"COMPLETED","conclusion":"FAILURE",
                 "startedAt":"2026-11-05T04:00:00Z","completedAt":"2026-11-05T04:01:00Z"}]
        });
        // Timestamp order wins over array order in both directions.
        for _ in 0..2 {
            row["statusCheckRollup"].as_array_mut().unwrap().reverse();
            let prs = parse_stall_prs(&[row.clone()], Some(&["build".into()]));
            let p = &prs.by_branch["topic"];
            assert_eq!(p.ci_rollup.as_deref(), Some("pass"));
            assert_eq!(pr_ready(t.path(), p), Some(true));
            assert!(!pr_digest(t.path(), p, &row, Some(false), false)
                .join("\n")
                .contains("red CI"));
            assert!(failure_details(&[(42, row.clone())], &mut |_| panic!(
                "superseded log fetched"
            ))
            .is_empty());
        }
        row["statusCheckRollup"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name":"build","status":"IN_PROGRESS","conclusion":null,
                "startedAt":"2026-11-05T06:00:00Z","completedAt":null
            }));
        assert_eq!(
            parse_stall_prs(&[row], Some(&["build".into()])).by_branch["topic"]
                .ci_rollup
                .as_deref(),
            Some("pending")
        );
    }

    #[test]
    fn failed_status_context_gets_named_unavailable_test_evidence() {
        let t = tempfile::tempdir().unwrap();
        for state in ["FAILURE", "ERROR"] {
            let row = serde_json::json!({
                "number":42,"title":"work (STORY-2476)","headRefName":"topic",
                "statusCheckRollup":[{"context":"legacy-ci","state":state}]
            });
            let prs = parse_stall_prs(&[row.clone()], Some(&["legacy-ci".into()]));
            let p = &prs.by_branch["topic"];
            assert_eq!(p.ci_rollup.as_deref(), Some("fail"));
            assert!(pr_digest(t.path(), p, &row, Some(false), false)
                .join("\n")
                .contains("red CI [legacy-ci]"));
            let details = failure_details(&[(42, row)], &mut |_| panic!("context has no job"));
            assert_eq!(details.len(), 1);
            assert!(details[0].contains("legacy-ci: failing tests unavailable"));
        }
    }

    #[test]
    fn failure_logs_have_a_shared_four_job_budget() {
        let row = serde_json::json!({"statusCheckRollup":[{"name":"build","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"}]});
        let hold = serde_json::json!({"statusCheckRollup":[{"name":"merge-hold-gate","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/456"}]});
        let mut prs = (10..=15).map(|pr| (pr, hold.clone())).collect::<Vec<_>>();
        prs.extend((1..=6).map(|pr| (pr, row.clone())));
        let mut calls = 0;
        let lines = failure_details(&prs, &mut |id| {
            assert_eq!(id, 123, "holds must not consume log slots");
            calls += 1;
            Some("test broken ... FAILED".into())
        });
        assert_eq!(calls, 4);
        assert_eq!(lines.len(), 6);
        assert!(lines[4].contains("unavailable"));
    }

    #[test]
    fn unproven_train_owner_is_unknown_with_inspection_evidence() {
        let t = tempfile::tempdir().unwrap();
        for _body in [
            "",
            "legacy owner",
            "<!-- aida-train-owner: host=foreign pid=123 -->",
            &crate::pr_train::owner_line_for_self(),
        ] {
            let mut details = Vec::new();
            assert_eq!(train_pr_running(t.path(), 88, &mut details), None);
            assert!(details[0].contains("Train PR-88"));
            assert!(details[0].contains("gh pr view 88"));
            assert!(details[0].contains("live or liveness is unknown"));
            assert!(!details[0].contains("gh pr close"));
        }
    }

    #[test]
    fn digest_commands_match_blockers_and_include_actual_failure_lines() {
        let t = tempfile::tempdir().unwrap();
        let mut p = pr();
        let row = serde_json::json!({"statusCheckRollup":[{"name":"build","conclusion":"FAILURE","detailsUrl":"https://github.com/o/r/actions/runs/1/job/123"}]});
        let ready = pr_digest(t.path(), &p, &row, Some(true), false).join("\n");
        assert!(ready.contains("aida pr ship 42"));
        assert!(!ready.contains("rework"));
        p.is_draft = true;
        let draft = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(draft.contains("draft; gh pr ready 42"));
        assert!(!draft.contains("rework"));
        p.is_draft = false;
        p.labels.push("aida:merge-hold".into());
        let held = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(held.contains("merge hold"));
        assert!(held.contains("aida merge-hold clear 42"));
        assert!(!held.contains("queue rework"));
        p.labels.clear();
        p.title = "work (STORY-2476)".into();
        p.ci_rollup = Some("fail".into());
        let red = pr_digest(t.path(), &p, &row, Some(false), false).join("\n");
        assert!(red.contains("red CI [build]"));
        assert!(red.contains("aida queue rework STORY-2476"));
        assert!(!red.contains("aida pr ship"));
        let rejected = pr_digest(t.path(), &p, &row, Some(false), true).join("\n");
        assert!(rejected.contains("current-head request-changes"));
        let logs = failure_details(&[(42, row.clone())], &mut |id| {
            assert_eq!(id, 123);
            Some("test suite::broken ... FAILED\nthread 'x' panicked at src/x.rs:9:\nassertion failed".into())
        });
        assert!(logs.iter().any(|s| s.contains("suite::broken")));
        assert!(logs.iter().any(|s| s.contains("assertion failed")));
        assert!(
            failure_details(&[(42, row)], &mut |_| None)[0].contains("failing tests unavailable")
        );
    }

    fn pr() -> crate::status_cleanup::OpenPrItem {
        crate::status_prs::parse_open_pr_snapshot(
            &serde_json::json!([{
                "number":42,"title":"work","headRefName":"topic","headRefOid":"abcdef1234",
                "mergeable":"MERGEABLE","isDraft":false,
                "statusCheckRollup":[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS"}]
            }])
            .to_string(),
            Some(&[]),
        )
        .by_branch
        .remove("topic")
        .unwrap()
    }
    #[test]
    fn readiness_uses_current_head_verdict_without_idle_age_filter() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join(".aida/review-verdicts")).unwrap();
        let path = root.join(".aida/review-verdicts/PR-42.json");
        let mut p = pr();
        std::fs::write(
            &path,
            r#"{"verdict":"approved","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(root, &p), Some(true)); // fresh approval immediately suppresses idle
        p.labels.push("aida:merge-hold".into());
        assert_eq!(pr_ready(root, &p), Some(false));
        p.labels.clear();
        std::fs::write(&path, r#"{"verdict":"approved","reviewed_sha":"old"}"#).unwrap();
        assert_eq!(pr_ready(root, &p), Some(false));
        std::fs::write(
            &path,
            r#"{"verdict":"request-changes","reviewed_sha":"abcdef1234"}"#,
        )
        .unwrap();
        assert_eq!(pr_ready(root, &p), Some(false));
        assert!(current_rejection(root, &p));
        std::fs::write(&path, "broken").unwrap();
        assert_eq!(pr_ready(root, &p), None);
        p.ci_rollup = Some("fail".into());
        assert_eq!(pr_ready(root, &p), Some(false));
        p.ci_rollup = Some("unknown".into());
        assert_eq!(pr_ready(root, &p), None);
    }
    #[test]
    fn preview_creates_nothing_and_busy_tick_does_not_observe_or_send() {
        let t = tempfile::tempdir().unwrap();
        let report = run_with_home(t.path(), Some(t.path()), false, 30, &mut |_| {
            panic!("preview sends")
        })
        .unwrap();
        assert!(report.accepted.is_empty());
        assert!(!t.path().join(".aida").exists());
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let path = t.path().join(".aida/pipeline-stalls.lock");
        let f = std::fs::File::create(&path).unwrap();
        f.lock_exclusive().unwrap();
        assert!(
            run_with_home(t.path(), Some(t.path()), true, 30, &mut |_| panic!())
                .unwrap()
                .busy
        );
        assert!(!t.path().join(".aida/pipeline-stalls.json").exists());
        assert!(path.exists());
    }
    #[test]
    fn preview_does_not_repair_forge_configuration() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join(".aida")).unwrap();
        let path = t.path().join(".aida/config.toml");
        let body = "[forge]\nprovider = \"pure-git\"\n";
        std::fs::write(&path, body).unwrap();
        assert!(forge_snapshot(t.path()).is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), body);
        assert!(bounded_digest(&vec!["x".repeat(2048); 1000]).len() < 25_000);
    }
    #[test]
    fn unreadable_registry_and_foreign_identity_are_unknown() {
        let t = tempfile::tempdir().unwrap();
        let dir = crate::agent_registry::agents_dir(t.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.toml"), "broken [").unwrap();
        assert!(registry(t.path()).is_err());
        assert_eq!(observe(t.path(), Some(t.path()), 30).lanes_running, None);
        let observer = aida_core::pid_observer::PidObserver {
            namespace: Some(42),
            isolated: false,
        };
        assert!(!observer.can_observe(Some(43)));
        assert_eq!(any(&[Some(false), None]), None);
        assert_eq!(any(&[Some(true), None]), Some(true));
    }
}

#[cfg(test)]
#[path = "pipeline_stall_action_tests.rs"]
mod action_tests;

#[cfg(test)]
mod ownership_reader_tests {
    use super::*;
    // trace:STORY-2325 | ai:codex
    #[test]
    fn prc_train_running_uses_ownership_and_preserves_unknown() {
        use crate::ownership::{
            dual,
            store::{Expect, OwnershipStore},
            Holder, Ownership, Phase, Role, Witness, WitnessKind, WorkKey,
        };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(p)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        let store = OwnershipStore::for_project(p).unwrap().unwrap();
        let key = WorkKey::Train {
            train: dual::change(12),
        };
        assert_eq!(train_pr_running(p, 12, &mut vec![]), None);
        for state in ["unknown", "alive", "dead", "closed"] {
            let witnesses = match state {
                "alive" | "dead" => vec![Witness {
                    admission: None,
                    kind: WitnessKind::Heartbeat {
                        at: if state == "alive" {
                            chrono::Utc::now()
                        } else {
                            chrono::Utc::now() - chrono::Duration::days(1)
                        },
                        every_secs: 10,
                    },
                    recorded_at: chrono::Utc::now(),
                }],
                _ => vec![],
            };
            store
                .update(&key, "seed", Expect::Any, |_| {
                    Some((
                        Ownership {
                            key: key.clone(),
                            generation: 0,
                            holder: Some(Holder {
                                seat: "train".into(),
                                session: "t".into(),
                                role: Role::Train,
                                tree: None,
                                witnesses,
                            }),
                            phase: if state == "closed" {
                                Phase::Closed { why: "done".into() }
                            } else {
                                Phase::Leased
                            },
                        },
                        None,
                    ))
                })
                .unwrap();
            let mut details = vec![];
            assert_eq!(
                train_pr_running(p, 12, &mut details),
                if matches!(state, "dead" | "closed") {
                    Some(false)
                } else {
                    None
                },
                "{state}"
            );
        }
        std::fs::write(store.path_for(&key), "malformed").unwrap();
        assert_eq!(train_pr_running(p, 12, &mut vec![]), None);
    }
}
