//! LANE-OUTCOME: a lane that exited failed, punted, with no change or
//! refused before it started, and that nobody followed up.
//!
//! A follow-up is anything dated after the exit that takes the spec on: a
//! later lane attempt, a queue entry, a requeue, a re-drive or an escalation
//! to a human. A live lane or claim on the spec, and a spec that is closed,
//! count too. Receipts are host-local (`.aida/lane-results/`), so a run
//! reports the lanes launched from the checkout it runs in.
// trace:TASK-3827 | ai:claude

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use super::{mins_since, Class, Item, Policy, OPERATOR_SEAT};
use crate::events::EventKind;

/// The seat a lane exit goes to when its receipt names no launcher.
const LAUNCH_SEAT: &str = "product";
/// Exits older than this are history, not something a wake would change.
pub(crate) const HORIZON_HOURS: i64 = 24;

/// A finished lane attempt and what took its spec on afterwards, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneOutcomeFact {
    pub attempt: String,
    pub spec: String,
    /// The receipt's result word, as in the lane's exit line.
    pub result: &'static str,
    pub reason_code: String,
    pub detail: String,
    pub finished_at: DateTime<Utc>,
    pub launched_by: Option<String>,
    pub followed_up: Option<String>,
}

/// A registered seat name, or the launch seat when the receipt names none.
fn launch_seat(launched_by: Option<&str>) -> String {
    launched_by
        .map(|seat| crate::canonical_role_name(seat.trim()).to_ascii_lowercase())
        .filter(|seat| {
            !seat.is_empty()
                && seat != OPERATOR_SEAT
                && seat.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        })
        .unwrap_or_else(|| LAUNCH_SEAT.to_string())
}

pub(crate) fn classify_lane_outcomes(
    outcomes: &[LaneOutcomeFact],
    policy: &Policy,
    now: DateTime<Utc>,
) -> Vec<Item> {
    outcomes
        .iter()
        .filter(|outcome| outcome.followed_up.is_none())
        .filter(|outcome| {
            let age = now - outcome.finished_at;
            age > Duration::minutes(policy.lane_outcome_mins)
                && age <= Duration::hours(HORIZON_HOURS)
        })
        .map(|outcome| Item {
            key: format!("{}:{}", Class::LaneOutcome.label(), outcome.attempt),
            class: Class::LaneOutcome,
            pr: None,
            seat: launch_seat(outcome.launched_by.as_deref()),
            age_mins: mins_since(Some(outcome.finished_at), now),
            summary: format!(
                "the lane for {} exited `{} reason={}` ({}) and nothing has relaunched, queued or routed it since",
                crate::land::one_line(&outcome.spec, 80),
                outcome.result,
                crate::land::one_line(&outcome.reason_code, 80),
                crate::land::one_line(&outcome.detail, 300)
            ),
            action: format!(
                "relaunch the lane for {}, or route the spec (attempt {})",
                crate::land::one_line(&outcome.spec, 80),
                outcome.attempt
            ),
        })
        .collect()
}

/// What can take a spec on after a lane exits. Specs are named by key: one
/// spec has one key whatever label a record used for it.
#[derive(Debug, Clone, Default)]
pub(crate) struct FollowUps {
    /// Dated follow-ups: a later attempt, a queue entry, a routing event.
    pub dated: Vec<(String, DateTime<Utc>, String)>,
    /// A live lane or claim on the spec, described.
    pub live: Vec<(String, String)>,
    /// A closed spec and its status.
    pub closed: BTreeMap<String, String>,
}

impl FollowUps {
    /// What followed the exit of `spec` at `finished_at`, if anything.
    pub(crate) fn after(&self, spec: &str, finished_at: DateTime<Utc>) -> Option<String> {
        if let Some((_, at, what)) = self
            .dated
            .iter()
            .filter(|(key, at, _)| key == spec && *at > finished_at)
            .min_by_key(|(_, at, _)| *at)
        {
            return Some(format!("{what} at {}", at.format("%Y-%m-%d %H:%M UTC")));
        }
        if let Some((_, what)) = self.live.iter().find(|(key, _)| key == spec) {
            return Some(what.clone());
        }
        self.closed
            .get(spec)
            .map(|status| format!("the spec is {status}"))
    }
}

/// The routing an event records for its spec, when it is one.
fn routing(kind: &EventKind) -> Option<String> {
    match kind {
        EventKind::SpecRequeued { via, .. } => Some(format!("requeued ({via})")),
        EventKind::SpecReDriven { .. } => Some("re-driven by the supervisor".into()),
        EventKind::AdvisorEscalated { .. } => Some("escalated to a human".into()),
        _ => None,
    }
}

/// Every queue entry on the store: the queued spec, when, and for whom.
fn queue_entries(store_root: &Path) -> Vec<aida_core::QueueEntry> {
    let Ok(files) = std::fs::read_dir(store_root.join("registry/queues")) else {
        return Vec::new();
    };
    files
        .flatten()
        .map(|file| file.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("yaml"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&text).ok())
        .flatten()
        .collect()
}

/// The finished lane attempts of `root` inside the horizon whose result asks
/// for a follow-up, each with what followed it. `local` and `foreign` are the
/// live lanes of this clone and the live claims of others, as scope and
/// description.
pub(crate) fn lane_outcome_facts(
    root: &Path,
    store_root: &Path,
    store: &aida_core::RequirementsStore,
    local: &[(String, String)],
    foreign: &[(String, String)],
    now: DateTime<Utc>,
    unknown: &mut Vec<String>,
) -> Vec<LaneOutcomeFact> {
    use crate::lane_result::Kind;
    let (receipts, unreadable) = crate::lane_result::recorded(root);
    if unreadable > 0 {
        unknown.push(format!("{unreadable} lane receipt(s) could not be read"));
    }
    let horizon = now - Duration::hours(HORIZON_HOURS);
    // One key per spec: its record's id when the store knows the label.
    let key = |label: &str| {
        store.get_requirement_by_spec_id(label.trim()).map_or_else(
            || label.trim().to_ascii_uppercase(),
            |spec| spec.id.to_string(),
        )
    };
    let mut follow = FollowUps::default();
    for later in &receipts {
        follow.dated.push((
            key(&later.spec),
            later.created_at,
            format!("attempt {} was launched", later.attempt),
        ));
    }
    for entry in queue_entries(store_root) {
        follow.dated.push((
            entry.requirement_id.to_string(),
            entry.added_at,
            match &entry.for_role {
                Some(role) => format!("queued for {role}"),
                None => "queued".to_string(),
            },
        ));
    }
    for event in crate::events::read_all(root) {
        if event.ts < horizon {
            continue;
        }
        if let (Some(spec), Some(what)) = (&event.spec, routing(&event.kind)) {
            follow.dated.push((key(spec), event.ts, what));
        }
    }
    for (scope, what) in local.iter().chain(foreign) {
        follow.live.push((key(scope), what.clone()));
    }
    receipts
        .iter()
        .filter_map(|receipt| {
            let kind = receipt.result?;
            let finished_at = receipt.finished_at.filter(|at| *at >= horizon)?;
            if receipt.spec.trim().is_empty()
                || !matches!(
                    kind,
                    Kind::FailedAfterStart
                        | Kind::Punted
                        | Kind::NoChangeWithReason
                        | Kind::RefusedBeforeStart
                )
            {
                return None;
            }
            let spec_key = key(&receipt.spec);
            if let Some(spec) = store.get_requirement_by_spec_id(receipt.spec.trim()) {
                use aida_core::RequirementStatus as Status;
                if matches!(
                    spec.status,
                    Status::Completed | Status::Rejected | Status::Superseded
                ) {
                    follow
                        .closed
                        .insert(spec_key.clone(), spec.status.to_string());
                }
            }
            Some(LaneOutcomeFact {
                attempt: receipt.attempt.clone(),
                spec: receipt.spec.clone(),
                result: kind.label(),
                reason_code: receipt
                    .reason_code
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                detail: receipt.detail.clone().unwrap_or_default(),
                finished_at,
                launched_by: receipt.launched_by.clone(),
                followed_up: follow.after(&spec_key, finished_at),
            })
        })
        .collect()
}
