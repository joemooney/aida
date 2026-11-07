//! The ownership transitions (STORY-2325 PR-B), as pure functions of the
//! current record. The store runs each one under the ownership lock.
//!
//! PR-B is a dual-write (advisor amendment A6): the old records (leases,
//! review markers, merge locks, the train owner line) stay authoritative and
//! decide; these records follow them. So `acquire` never refuses here. When
//! the old system let a second holder in over a holder the judge would have
//! kept, the record follows the old system and the journal names the holder
//! it replaced, which is the evidence `aida own show --compare` needs before
//! PR-C turns that case into a refusal.
//!
//! trace:STORY-2325 | ai:claude

use super::types::{ChangeId, Holder, Ownership, Phase, WorkKey};
use chrono::{DateTime, Utc};

/// Take `key` for `holder` in `phase`. `None` when the record already says
/// exactly that (an idempotent re-write is not journaled).
// trace:STORY-2325 | ai:claude
pub(crate) fn acquire(
    current: Option<&Ownership>,
    key: &WorkKey,
    holder: Holder,
    phase: Phase,
) -> Option<(Ownership, Option<String>)> {
    if let Some(cur) = current {
        if cur.holder.as_ref() == Some(&holder) && cur.phase == phase {
            return None;
        }
    }
    let note = current.and_then(|cur| {
        let prior = cur.holder.as_ref()?;
        if prior.session == holder.session {
            return None;
        }
        match cur.phase {
            Phase::InPr { .. } | Phase::Closed { .. } => None,
            _ => Some(format!(
                "replaced holder session {} (seat {}); the old records allowed it",
                prior.session, prior.seat
            )),
        }
    });
    Some((
        Ownership {
            key: key.clone(),
            generation: 0,
            holder: Some(holder),
            phase,
        },
        note,
    ))
}

/// The holder of `key` gives the work to its PR: the record moves to `InPr`
/// with no holder. `None` when it already says so for the same change and
/// head.
// trace:STORY-2325 | ai:claude
pub(crate) fn handoff(
    current: Option<&Ownership>,
    key: &WorkKey,
    change: &ChangeId,
    head: &str,
    now: DateTime<Utc>,
) -> Option<(Ownership, Option<String>)> {
    if let Some(Phase::InPr {
        change: c, head: h, ..
    }) = current.map(|c| &c.phase)
    {
        if c == change && h == head && current.is_some_and(|c| c.holder.is_none()) {
            return None;
        }
    }
    let note = current
        .and_then(|c| c.holder.as_ref())
        .map(|h| format!("handed off by session {} (seat {})", h.session, h.seat));
    Some((
        Ownership {
            key: key.clone(),
            generation: 0,
            holder: None,
            phase: Phase::InPr {
                change: change.clone(),
                handed_off_at: now,
                handed_off_by: current
                    .and_then(|c| c.holder.as_ref())
                    .map(|h| h.session.clone()),
                head: head.to_string(),
            },
        },
        note,
    ))
}

/// `session` gives up `key`. Only the recorded holder's own session
/// releases; anything else is no change. A finished review or train closes;
/// a leased spec stays leased with no holder (free to acquire).
// trace:STORY-2325 | ai:claude
pub(crate) fn release(
    current: Option<&Ownership>,
    session: &str,
    why: &str,
) -> Option<(Ownership, Option<String>)> {
    let cur = current?;
    if cur.holder.as_ref()?.session != session {
        return None;
    }
    let phase = match &cur.phase {
        Phase::Reviewing { .. } | Phase::Training { .. } => Phase::Closed {
            why: why.to_string(),
        },
        other => other.clone(),
    };
    Some((
        Ownership {
            key: cur.key.clone(),
            generation: cur.generation,
            holder: None,
            phase,
        },
        Some(why.to_string()),
    ))
}

/// A rider leaves train `train` (the train's merge lease was released): its
/// change record moves out of `Training` to `Leased`, keeping its holder
/// (none once it is in a PR), so it is free to acquire. `None` when it no
/// longer rides that train.
// trace:STORY-2325 | ai:claude
pub(crate) fn leave_train(
    current: Option<&Ownership>,
    key: &WorkKey,
    train: &ChangeId,
    why: &str,
) -> Option<(Ownership, Option<String>)> {
    let cur = current?;
    if !matches!(&cur.phase, Phase::Training { train: t } if t == train) {
        return None;
    }
    Some((
        Ownership {
            key: key.clone(),
            generation: cur.generation,
            holder: cur.holder.clone(),
            phase: Phase::Leased,
        },
        Some(format!("left train {}-{}: {why}", train.forge, train.n)),
    ))
}

/// A rider joins train `train`: its change record says so. The rider's own
/// holder (none once it is in a PR) is kept.
// trace:STORY-2325 | ai:claude
pub(crate) fn ride(
    current: Option<&Ownership>,
    key: &WorkKey,
    train: &ChangeId,
) -> Option<(Ownership, Option<String>)> {
    if let Some(Phase::Training { train: t }) = current.map(|c| &c.phase) {
        if t == train {
            return None;
        }
    }
    Some((
        Ownership {
            key: key.clone(),
            generation: 0,
            holder: current.and_then(|c| c.holder.clone()),
            phase: Phase::Training {
                train: train.clone(),
            },
        },
        None,
    ))
}
