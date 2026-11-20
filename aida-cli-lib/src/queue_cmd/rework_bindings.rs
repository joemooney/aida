//! Which change-keyed reviews concern a spec, for rework admission, decided
//! from the spec's own inventory instead of every key in the verdict store.
//!
//! The previous check asked each stored key in turn whether its newest
//! ledger verdict named the spec. Every question read that key's ledger and
//! opened an ownership discovery of its own, so one `queue rework` read the
//! whole gate ledger, and a discovery that had to fall back to a full read
//! which then refused (one unreadable ledger is enough) repeated that full
//! read once per key. Here the candidates are the changes the ledger records
//! against the spec (its by-spec index, its write-ahead bindings and, on
//! evidence of a lost binding, one full read), and only those are read,
//! under one shared discovery.
//!
//! When ownership discovery fails for one of those changes, the projection
//! readers withhold every review copy a foreign change could own, which
//! used to read as "no review" and let a waiting review through. The
//! failure is reported instead, and [`ReworkBindings::withheld_wait`] finds
//! the open wait among the withheld copies so the refusal is preserved.
// trace:BUG-2629 | ai:claude

use crate::gate::ledger::{self, bindings, checked, project, store, ChangeKey, GateEvent};
use crate::gate::review_verdict::{self as rv, JsonObj, RecordedVerdict, VerdictKind};
use std::path::{Path, PathBuf};

/// One spec's change-keyed review inventory.
// trace:BUG-2629 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct ReworkBindings {
    /// The store root the inventory was read from; `None` when it cannot be
    /// resolved (then nothing is bound, as before).
    root: Option<PathBuf>,
    /// Every change that may hold a review of the spec. `true` when the
    /// change is proved to concern the spec without reading its events: the
    /// by-spec index names it, or it is the spec's open change.
    inventory: Vec<(ChangeKey, bool)>,
    /// The changes whose verdicts are bound to the spec.
    bound: Vec<ChangeKey>,
    /// Why ownership of the inventory's reviews could not be established.
    failure: Option<String>,
}

impl ReworkBindings {
    fn note(&mut self, change: ChangeKey, proved: bool) {
        match self
            .inventory
            .iter_mut()
            .find(|(known, _)| *known == change)
        {
            Some((_, was)) => *was |= proved,
            None => self.inventory.push((change, proved)),
        }
    }

    fn fail(&mut self, why: String) {
        self.failure.get_or_insert(why);
    }

    /// Whether the verdict stored under `key` is bound to the spec.
    pub(crate) fn binds(&self, key: &str) -> bool {
        ChangeKey::parse(key).is_some_and(|change| self.bound.contains(&change))
    }

    /// Why the spec's reviews cannot be read completely, if they cannot.
    pub(crate) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The open WAITING review among the copies a failed discovery withheld,
    /// read from the raw ledgers of the spec's inventory and from the legacy
    /// files. It can only preserve a refusal, never grant admission:
    ///
    /// - a round whose ownership is unverified clears nothing, so a later
    ///   approval or refusal that a foreign change might own does not hide
    ///   an earlier wait (a later round no witness could disqualify does);
    /// - a wait closed by its terminal annotation stays closed: the
    ///   annotation is applied before the wait is judged open.
    // trace:BUG-2629 | ai:claude
    pub(crate) fn withheld_wait(
        &self,
        project_root: &Path,
        labels: &[String],
    ) -> Option<RecordedVerdict> {
        let root = self.root.as_deref()?;
        let raw: Vec<(ChangeKey, bool, Vec<GateEvent>)> = self
            .inventory
            .iter()
            .map(|&(change, proved)| {
                let events = store::read_file(root, change)
                    .into_iter()
                    .filter(|e| !e.is_spec_only() && e.verdict_record().is_some())
                    .collect();
                (change, proved, events)
            })
            .collect();
        let closures: Vec<&JsonObj> = raw
            .iter()
            .flat_map(|(_, _, events)| events)
            .filter_map(|e| e.verdict_record())
            .filter(|record| record.contains_key("terminal_wait_source"))
            .collect();
        let open_wait = |key: &str, body: &str| {
            rv::parse_recorded_verdict(body).filter(|verdict| {
                verdict.kind == VerdictKind::Waiting
                    && !verdict.is_closed()
                    && !closures
                        .iter()
                        .any(|record| rv::terminal_wait_overlay_body(key, body, record).is_some())
            })
        };
        // What the projection still answers with: legacy files, and ledger
        // evidence no witness could disqualify.
        let keys = labels
            .iter()
            .cloned()
            .chain(raw.iter().map(|(change, ..)| change.as_key()));
        for key in keys {
            let wait = rv::current_verdict(project_root, &key)
                .and_then(|current| open_wait(&key, &current.body));
            if wait.is_some() {
                return wait;
            }
        }
        // The withheld ledger copies, each ledger read as its readers read
        // it: under the change key (every review on it), and under each
        // label (the reviews recorded for that label). A terminal annotation
        // closes only the key it names, exactly as in the projection.
        for (change, proved, events) in &raw {
            let change_key = change.as_key();
            let views = std::iter::once((change_key.as_str(), None))
                .chain(labels.iter().map(|label| (label.as_str(), Some(label))));
            for (key, label) in views {
                let named = |spec: Option<&str>, label: &String| {
                    spec.is_some_and(|spec| label.eq_ignore_ascii_case(spec.trim()))
                };
                let standing: Vec<GateEvent> = events
                    .iter()
                    .filter(|e| label.is_none_or(|label| named(e.spec.as_deref(), label)))
                    .filter(|e| project::terminal_annotation_matches(e, key))
                    .filter(|e| is_wait(e) || ownership_free(e))
                    .cloned()
                    .collect();
                let Some(newest) = project::newest_of(standing) else {
                    continue;
                };
                // A change the bindings merely list concerns the spec only
                // when the answering review is recorded for it.
                let concerns = *proved
                    || label.is_some()
                    || labels.iter().any(|l| named(newest.spec.as_deref(), l));
                if !concerns {
                    continue;
                }
                let wait = newest
                    .verdict_record()
                    .and_then(|record| serde_json::to_string(record).ok())
                    .and_then(|body| open_wait(key, &body));
                if wait.is_some() {
                    return wait;
                }
            }
        }
        None
    }
}

fn is_wait(event: &GateEvent) -> bool {
    event
        .verdict_record()
        .and_then(rv::parse_ledger_record)
        .is_some_and(|verdict| verdict.kind == VerdictKind::Waiting)
}

/// Whether `event` stands without an ownership index: no witness could
/// disqualify it, so a failed discovery does not withhold it.
fn ownership_free(event: &GateEvent) -> bool {
    let mut one = vec![event.clone()];
    project::MiskeyOwners::fail_closed(&mut one);
    !one.is_empty()
}

/// Whether the projection readers can establish ownership of `change`'s
/// review copies: the condition under which they present them rather than
/// withhold them ([`checked::filter_miskeyed_or_fail_closed`]).
fn ownership_checked(root: &Path, change: ChangeKey) -> Result<(), String> {
    let mut events: Vec<GateEvent> = store::read_file(root, change)
        .into_iter()
        .filter(|e| !e.is_spec_only())
        .collect();
    if !project::needs_witnesses(&events) {
        return Ok(());
    }
    match checked::discover(root) {
        Ok(Some(mut discovery)) => discovery.filter_events(&mut events).map_err(|e| e.0),
        Ok(None) => Err("the gate ledger disappeared while it was read".into()),
        Err(e) => Err(e.0),
    }
}

/// The change-keyed review inventory of the spec known by `labels`, read
/// from `project_root`'s gate ledger.
///
/// A change is bound when the spec's by-spec index names it, or when its
/// newest ledger verdict is recorded for the spec; only a change the ledger
/// records against the spec can be the latter, so no other ledger is read.
/// With `selected_only` (branch selection supplied the change) no
/// spec-bound change is looked for. `open_change`, the spec's open or
/// selected change, is always checked: its reviews answer for the spec.
// trace:BUG-2629 | ai:claude
pub(crate) fn discover(
    project_root: &Path,
    labels: &[String],
    open_change: Option<&str>,
    selected_only: bool,
) -> ReworkBindings {
    let mut out = ReworkBindings::default();
    let Ok(root) = ledger::store_root(project_root) else {
        return out;
    };
    // One ownership inventory for every read below. trace:BUG-2543 | ai:claude
    let _discovery = checked::DiscoveryScope::enter();
    if !selected_only {
        for label in labels {
            let Some(spec) = project::spec_key(label) else {
                continue;
            };
            let hint = project::index_changes(&root, &spec);
            let listed = bindings::bound_changes_lossy(&root, &spec);
            let lost = checked::binding_lost(hint.as_deref(), listed.as_deref());
            for change in hint.into_iter().flatten() {
                out.note(change, true);
            }
            for change in listed.into_iter().flatten() {
                out.note(change, false);
            }
            if lost {
                // A deleted binding must not hide a review: every ledger
                // that carries the label, from one full read.
                match checked::recorded_changes(&root, &spec) {
                    Ok(found) => found
                        .iter()
                        .filter_map(|key| ChangeKey::parse(key))
                        .for_each(|change| out.note(change, false)),
                    Err(e) => out.fail(e.0),
                }
            }
        }
        // An incomplete inventory already fails the read; the index still
        // binds what it names, and no further ledger is read.
        for (change, proved) in out.inventory.clone() {
            if out.failure.is_none() {
                if let Err(why) = ownership_checked(&root, change) {
                    out.fail(why);
                }
            }
            let bound = proved
                || (out.failure.is_none()
                    && project::newest_verdict_event(project_root, &change.as_key()).is_some_and(
                        |event| {
                            event.spec.as_deref().is_some_and(|spec| {
                                labels.iter().any(|l| l.eq_ignore_ascii_case(spec))
                            })
                        },
                    ));
            if bound {
                out.bound.push(change);
            }
        }
    }
    if let Some(open) = open_change.and_then(ChangeKey::parse) {
        if out.failure.is_none() {
            if let Err(why) = ownership_checked(&root, open) {
                out.fail(why);
            }
        }
        out.note(open, true);
    }
    out.root = Some(root);
    out
}

/// How long a rework lookup runs before it first names itself.
pub(crate) const SLOW_PHASE_NOTICE: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `work`, and while it is still running after each `every`, tell `say`
/// which phase the command is in and for how long. The reported spin showed
/// nothing for nine minutes; a slow phase now says what it is.
// trace:BUG-2629 | ai:claude
pub(crate) fn announce_slow_phase<T>(
    phase: &str,
    every: std::time::Duration,
    say: impl Fn(String) + Send,
    work: impl FnOnce() -> T,
) -> T {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    std::thread::scope(|scope| {
        // Dropped when `work` returns or unwinds, which ends the watcher.
        let (done, finished) = channel::<()>();
        scope.spawn(move || {
            let started = std::time::Instant::now();
            while matches!(finished.recv_timeout(every), Err(RecvTimeoutError::Timeout)) {
                say(format!(
                    "still {phase} ({}s elapsed)",
                    started.elapsed().as_secs()
                ));
            }
        });
        let out = work();
        drop(done);
        out
    })
}

#[cfg(test)]
#[path = "rework_bindings_tests.rs"]
mod tests;
