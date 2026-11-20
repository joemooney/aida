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
//! failure is reported instead, and [`ReworkBindings::recover`] reads the
//! inventory's raw ledgers once, under one byte budget and without opening
//! another discovery, for what the withheld copies still establish: an open
//! wait, and the request-changes reviews that stay readable beside it.
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
    /// Why the inventory itself is not known to be complete: the changes
    /// that review the spec could not be enumerated within the read bound.
    refusal: Option<String>,
}

/// What a failed ownership discovery still establishes about a spec's
/// reviews ([`ReworkBindings::recover`]).
// trace:BUG-2629 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct Recovered {
    /// An open WAITING review, withheld or still readable.
    pub(crate) wait: Option<RecordedVerdict>,
    /// The open request-changes reviews that answer as current for their
    /// key without an ownership index, each under the key it answers for.
    pub(crate) findings: Vec<(String, RecordedVerdict)>,
    /// The record bodies behind `findings`: one review read under its
    /// change key and under its spec key is imported once.
    bodies: Vec<String>,
}

impl Recovered {
    fn take(&mut self, key: &str, standing: Standing) {
        match standing {
            Standing::Wait(wait) => {
                self.wait.get_or_insert(wait);
            }
            Standing::Findings(body, verdict) => {
                if !self.bodies.contains(&body) {
                    self.bodies.push(body);
                    self.findings.push((key.to_string(), verdict));
                }
            }
            Standing::Nothing => {}
        }
    }
}

/// What one key's reviews establish while ownership is unknown.
enum Standing {
    Wait(RecordedVerdict),
    /// The record body and its parsed verdict.
    Findings(String, RecordedVerdict),
    Nothing,
}

/// Aggregate bytes one recovery may read from the inventory's ledgers: a
/// selection's budget. The spec's own ledgers fit it many times over; a
/// recovery that would cross it stops and names the file.
// trace:BUG-2629 | ai:claude
const MAX_RECOVERY_BYTES: u64 = checked::MAX_SELECTION_BYTES;

#[cfg(test)]
thread_local! {
    /// A byte budget for every [`ReadBudget`] in place of its production one.
    pub(crate) static READ_BUDGET: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// The byte budget of one bounded read pass. A file is charged its size
/// before it is read, so a pass never starts a read it cannot afford.
// trace:BUG-2629 | ai:claude
struct ReadBudget {
    total: u64,
    left: u64,
}

impl ReadBudget {
    fn new(total: u64) -> Self {
        #[cfg(test)]
        let total = READ_BUDGET.with(|b| b.get()).unwrap_or(total);
        Self { total, left: total }
    }

    fn spend(&mut self, file: &Path) -> Result<(), String> {
        let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        if size > self.left {
            return Err(format!(
                "{} is {size} bytes and only {} of the {}-byte read budget remain",
                file.display(),
                self.left,
                self.total
            ));
        }
        self.left -= size;
        Ok(())
    }
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

    /// Why the changes that review the spec could not be enumerated, if
    /// they could not: no review can then be ruled out, a waiting one
    /// included, so admission is refused.
    pub(crate) fn refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// What the spec's reviews establish after a failed ownership
    /// discovery, read from the raw ledgers of its inventory, the legacy
    /// files and the signed publications. Each key is judged in the view its
    /// ordinary reader has ([`judge`]): a change key over that change's own
    /// ledger, a spec label over the events recorded for it on every
    /// inventoried change and on its unbound ledger. It can preserve a
    /// refusal and import findings that are readable regardless; it never
    /// grants admission.
    ///
    /// One pass: each ledger is read once, no discovery is opened (a failed
    /// full read is not repeated per key), and the bytes read are bounded by
    /// [`MAX_RECOVERY_BYTES`]. Only the events recorded for a label are kept
    /// past their ledger's turn. `Err` names the file the budget ran out
    /// at: the search for a wait is then incomplete, and the caller refuses.
    // trace:BUG-2629 | ai:claude
    pub(crate) fn recover(
        &self,
        project_root: &Path,
        labels: &[String],
    ) -> Result<Recovered, String> {
        let mut out = Recovered::default();
        let Some(root) = self.root.as_deref() else {
            return Ok(out);
        };
        let mut budget = ReadBudget::new(MAX_RECOVERY_BYTES);
        let mut by_label: Vec<Vec<GateEvent>> = labels.iter().map(|_| Vec::new()).collect();
        let mut inventory = self.inventory.clone();
        inventory.sort_by_key(|(change, _)| change.as_key());
        for (change, proved) in inventory {
            budget.spend(&store::ledger_file(root, change))?;
            let events: Vec<GateEvent> = store::read_file(root, change)
                .into_iter()
                .filter(|e| !e.is_spec_only() && e.verdict_record().is_some())
                .collect();
            let key = change.as_key();
            let view: Vec<&GateEvent> = events
                .iter()
                .filter(|e| project::terminal_annotation_matches(e, &key))
                .collect();
            // A change the bindings merely list (a write-ahead binding
            // precedes its append, and may outlive one that never came)
            // answers for the spec only when its answering review is
            // recorded for the spec, as the ordinary binding check has it.
            let concerns = proved
                || newest(&standing(&view)).is_some_and(|e| labels.iter().any(|l| names(e, l)));
            if concerns {
                out.take(&key, judge(project_root, &key, &view));
            }
            for event in events {
                if let Some(at) = labels.iter().position(|label| names(&event, label)) {
                    by_label[at].push(event);
                }
            }
        }
        for (label, bound) in labels.iter().zip(by_label) {
            let mut unbound = Vec::new();
            if let Some(spec) = project::spec_key(label) {
                budget.spend(&store::unbound_file(root, &spec))?;
                unbound = store::read_unbound(root, &spec);
                // A record already bound to a change is read once, from the
                // change, as in the projection.
                unbound.retain(|u| {
                    u.verdict_record().is_some()
                        && !bound
                            .iter()
                            .any(|b| b.verdict_record() == u.verdict_record())
                });
            }
            let merged = project::merge_streams(bound.into_iter().chain(unbound));
            let view: Vec<&GateEvent> = merged
                .iter()
                .filter(|e| project::terminal_annotation_matches(e, label))
                .collect();
            out.take(label, judge(project_root, label, &view));
        }
        Ok(out)
    }
}

/// Whether `event` is recorded for the spec `label`.
fn names(event: &GateEvent, label: &str) -> bool {
    event
        .spec
        .as_deref()
        .is_some_and(|spec| spec.trim().eq_ignore_ascii_case(label.trim()))
}

fn is_wait(event: &GateEvent) -> bool {
    event
        .verdict_record()
        .and_then(rv::parse_ledger_record)
        .is_some_and(|verdict| verdict.kind == VerdictKind::Waiting)
}

/// A terminal-wait annotation: a lifecycle overlay, not a review round.
fn annotation(event: &GateEvent) -> Option<&JsonObj> {
    event
        .verdict_record()
        .filter(|record| record.contains_key("terminal_wait_source"))
}

/// The review rounds of `view` that stand while ownership is unknown: the
/// waits, withheld or not, and the rounds no witness could disqualify. A
/// round whose ownership is unverified clears nothing, so a later approval
/// or refusal that a foreign change might own does not hide an earlier wait.
fn standing<'a>(view: &[&'a GateEvent]) -> Vec<&'a GateEvent> {
    view.iter()
        .copied()
        .filter(|e| annotation(e).is_none())
        .filter(|e| is_wait(e) || project::ownership_free(e))
        .collect()
}

/// The answering round of `rounds` (oldest first, no annotations), as
/// [`project::newest_review_of`] picks it: the last, or the ordinary origin
/// a proved phase-3 overlay duplicates.
fn newest<'a>(rounds: &[&'a GateEvent]) -> Option<&'a GateEvent> {
    let (last, rest) = rounds.split_last()?;
    Some(match rest.last() {
        Some(origin) if project::proved_phase3_origin(origin, last) => origin,
        _ => last,
    })
}

/// What `key`'s reviews establish while ownership is unknown, from `view`:
/// the key's verdict events in its reader's order, with the terminal
/// annotations of other keys already left out.
///
/// The sources keep the precedence the projection gives them
/// ([`rv::current_verdict`]), so a wait a newer authoritative round has
/// superseded does not refuse:
///
/// 1. a signed publication answers alone, whatever the raw ledger holds;
/// 2. otherwise the newest standing ledger round ([`standing`]) answers
///    when it supersedes the legacy file ([`rv::ledger_supersedes`]), and
///    the legacy file answers when it does not.
///
/// The answering wait is open unless the view's own latest terminal
/// annotation closes exactly it: same key, same source, and a closed
/// record. An annotation on another change's ledger is not in a change
/// key's view and closes nothing there.
///
/// A request-changes answer is reported as findings only when it is the
/// review the projection would also present as current: a publication, a
/// ledger round no witness could disqualify with no withheld round after
/// it, or a legacy file no ledger round supersedes.
// trace:BUG-2629 | ai:claude
fn judge(project_root: &Path, key: &str, view: &[&GateEvent]) -> Standing {
    let closure = view.iter().rev().find_map(|e| annotation(e));
    let settle = |body: String, current: bool| {
        let body = closure
            .and_then(|record| rv::terminal_wait_overlay_body(key, &body, record))
            .unwrap_or(body);
        match rv::parse_recorded_verdict(&body) {
            Some(v) if v.kind == VerdictKind::Waiting && !v.is_closed() => Standing::Wait(v),
            Some(v) if current && !v.is_closed() && v.kind.requires_changes() => {
                Standing::Findings(body, v)
            }
            _ => Standing::Nothing,
        }
    };
    if let Some(signed) = crate::gate::review_publication::current_unreported(project_root, &[key])
    {
        return settle(signed.body, true);
    }
    let path = rv::verdict_path(project_root, key);
    // A legacy file that cannot be read or parsed outranks nothing here:
    // the raw wait behind it still refuses.
    let (file_body, legacy) = match rv::checked_legacy_record(project_root, key) {
        Ok(Some((body, verdict))) => (Some(body), Some(verdict)),
        _ => (None, None),
    };
    let supersedes = |event: &GateEvent| {
        legacy
            .as_ref()
            .is_none_or(|file| rv::ledger_supersedes(&path, Some(file), event))
    };
    let rounds: Vec<&GateEvent> = view
        .iter()
        .copied()
        .filter(|e| annotation(e).is_none())
        .collect();
    if let Some(event) = newest(&standing(view)).filter(|e| supersedes(e)) {
        let Some(body) = event
            .verdict_record()
            .and_then(|record| serde_json::to_string(record).ok())
        else {
            return Standing::Nothing;
        };
        let current = project::ownership_free(event)
            && newest(&rounds).is_some_and(|last| std::ptr::eq(last, event));
        return settle(body, current);
    }
    match file_body {
        Some(body) => settle(body, !rounds.iter().any(|e| supersedes(e))),
        None => Standing::Nothing,
    }
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

/// Whether the bindings of the ledger under `root` are certified complete,
/// as a strict selection requires: a supported marker with older writers
/// fenced out, or no change ledger at all. Otherwise a change may hold a
/// review of a spec that neither a binding nor a hint records.
// trace:BUG-2629 | ai:claude
fn bindings_certified(root: &Path) -> bool {
    use bindings::MarkerState;
    match bindings::marker_state(root) {
        Ok(MarkerState::Complete) => store::writer_fenced(root),
        Ok(MarkerState::NoLedger | MarkerState::Trivial) => true,
        _ => false,
    }
}

/// Every change whose ledger records a review for one of `labels`, from one
/// bounded lossy pass over the change ledgers: what the per-key check found
/// by each event's label, for a ledger whose bindings certify nothing. No
/// payload is kept. `Err` when the pass would cross its bounds, naming where.
// trace:BUG-2629 | ai:claude
fn labelled_changes(root: &Path, labels: &[String]) -> Result<Vec<ChangeKey>, String> {
    let dir = ledger::ledger_dir(root);
    let mut changes: Vec<ChangeKey> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            bindings::change_ledger_name(name.to_str()?).and_then(|key| ChangeKey::parse(&key))
        })
        .collect();
    if changes.len() > checked::MAX_CHANGE_LEDGERS {
        return Err(format!(
            "{} holds more than {} change ledgers",
            dir.display(),
            checked::MAX_CHANGE_LEDGERS
        ));
    }
    changes.sort_by_key(|change| change.as_key());
    let mut budget = ReadBudget::new(checked::MAX_DISCOVERY_BYTES);
    let mut found = Vec::new();
    for change in changes {
        budget.spend(&store::ledger_file(root, change))?;
        let reviews_spec = store::read_file(root, change).iter().any(|e| {
            !e.is_spec_only()
                && e.verdict_record().is_some()
                && labels.iter().any(|label| names(e, label))
        });
        if reviews_spec {
            found.push(change);
        }
    }
    Ok(found)
}

/// The change-keyed review inventory of the spec known by `labels`, read
/// from `project_root`'s gate ledger.
///
/// A change is bound when the spec's by-spec index names it, or when its
/// newest ledger verdict is recorded for the spec; only a change the ledger
/// records against the spec can be the latter, so no other ledger is read.
/// That holds for a ledger whose bindings are certified complete. For one
/// that is not (never bootstrapped, or open to older writers) the changes
/// are enumerated by the label their events carry instead, in one bounded
/// pass ([`labelled_changes`]); crossing its bounds is a
/// [`ReworkBindings::refusal`].
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
        if !bindings_certified(&root) {
            match labelled_changes(&root, labels) {
                Ok(found) => found.into_iter().for_each(|change| out.note(change, false)),
                Err(why) => out.refusal = Some(why),
            }
        }
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
            // A deleted binding must not hide a review: every ledger that
            // carries the label, from one full read. A full read that
            // refused is not repeated for the next label: it reads the same
            // ledgers and refuses the same way.
            if lost && out.failure.is_none() {
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
