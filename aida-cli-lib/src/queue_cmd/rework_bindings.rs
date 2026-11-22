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
//!
//! One rule governs that recovery and the enumeration before it: a source
//! that could not be listed, read, parsed or verified is unknown, never
//! absent, and an unknown source refuses rework by name. Only a file or
//! directory proved not to exist is empty, and only a verified publication
//! takes a signed record's precedence.
// trace:BUG-2629 | ai:claude

use crate::gate::ledger::event::EVENT_VERSION;
use crate::gate::ledger::{self, bindings, checked, project, store, ChangeKey, GateEvent};
use crate::gate::review_publication::ReviewStoreGeneration;
use crate::gate::review_verdict::{self as rv, JsonObj, RecordedVerdict, VerdictKind};
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One spec's change-keyed review inventory.
// trace:BUG-2629 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct ReworkBindings {
    /// The store root the inventory was read from; `None` when it cannot be
    /// resolved (then nothing is bound, as before).
    root: Option<PathBuf>,
    /// Every change that may hold a review of the spec. `true` when the
    /// change is proved to concern the spec without reading its events: the
    /// by-spec index names it, or it is the spec's open change.
    inventory: Vec<(ChangeKey, bool)>,
    positions: HashMap<ChangeKey, usize>,
    hints: HashMap<String, Vec<ChangeKey>>,
    selected_only_extra: Option<ChangeKey>,
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
    /// The first source of the spec's reviews that could not be read, parsed
    /// or verified. What it holds is unknown, a waiting review included, so
    /// nothing read beside it admits rework: `findings` are not actionable
    /// and the absence of a `wait` proves nothing.
    pub(crate) unknown: Option<String>,
    /// The record bodies behind `findings`: one review read under its
    /// change key and under its spec key is imported once.
    bodies: HashSet<String>,
}

impl Recovered {
    /// Keep what `key` established. Findings are retained past their key's
    /// turn, body and parsed record both, so they are charged to `budget`
    /// before they are kept.
    fn take(
        &mut self,
        key: &str,
        standing: Standing,
        budget: &mut ReadBudget,
    ) -> Result<(), String> {
        match standing {
            Standing::Wait(wait) => {
                self.wait.get_or_insert(wait);
            }
            Standing::Findings(body, verdict) => {
                if !self.bodies.contains(&body) {
                    budget.charge(
                        &format_args!("the findings recorded for {key}"),
                        2 * body.len() as u64,
                    )?;
                    self.bodies.insert(body);
                    self.findings.push((key.to_string(), verdict));
                }
            }
            Standing::Nothing => {}
        }
        Ok(())
    }
}

/// What one key's reviews establish while ownership is unknown.
enum Standing {
    Wait(RecordedVerdict),
    /// The record body and its parsed verdict.
    Findings(String, RecordedVerdict),
    Nothing,
}

/// Aggregate bytes one recovery may read and retain: a selection's budget.
/// The spec's own reviews fit it many times over; a recovery that would
/// cross it stops and names where.
// trace:BUG-2629 | ai:claude
const MAX_RECOVERY_BYTES: u64 = checked::MAX_SELECTION_BYTES;

#[cfg(test)]
thread_local! {
    /// A byte budget for every [`ReadBudget`] in place of its production one.
    pub(crate) static READ_BUDGET: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
    /// A parse-attempt ceiling for every [`ReadBudget`] in place of
    /// [`MAX_PARSE_ATTEMPTS`].
    pub(crate) static PARSE_BUDGET: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
    /// Called at every step of a discovery or recovery that touches the
    /// ledger: a probe of what the command holds while it reads.
    pub(crate) static ON_LEDGER_STEP: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
    /// Answers for the signed publication of a key in place of the store.
    #[allow(clippy::type_complexity)]
    pub(crate) static PUBLISHED: std::cell::RefCell<
        Option<Box<dyn Fn(&str) -> anyhow::Result<Option<rv::CurrentVerdict>>>>,
    > = const { std::cell::RefCell::new(None) };
}

fn step() {
    #[cfg(test)]
    ON_LEDGER_STEP.with(|probe| {
        if let Some(probe) = probe.borrow().as_ref() {
            probe()
        }
    });
}

/// The verified signed publication that answers for `key`. `Err` when one
/// may exist but cannot be verified: that is no record at all, and never the
/// request-changes diagnostic the ordinary projection renders for it.
// trace:BUG-2629 | ai:claude
fn published(project_root: &Path, key: &str) -> anyhow::Result<Option<rv::CurrentVerdict>> {
    #[cfg(test)]
    if let Some(answer) = PUBLISHED.with(|stub| stub.borrow().as_ref().map(|stub| stub(key))) {
        return answer;
    }
    crate::gate::review_publication::current_verified(project_root, &[key])
}

/// Why a bounded read did not return a file's text.
// trace:BUG-2629 | ai:claude
#[derive(Clone)]
enum Unread {
    /// The byte budget ran out: the pass stops here.
    Exhausted(String),
    /// The file exists and could not be read as a ledger: its content is
    /// unknown.
    Failed(String),
}

impl Unread {
    fn why(self) -> String {
        match self {
            Unread::Exhausted(why) | Unread::Failed(why) => why,
        }
    }
}

/// Lines one bounded pass may try to parse, events or not: the selection's
/// ceiling. Many short lines fit the byte budget many times over.
// trace:BUG-2629 | ai:claude
const MAX_PARSE_ATTEMPTS: usize = checked::MAX_SELECTION_EVENTS;

/// The budget of one bounded pass: every byte it reads and every payload it
/// retains is charged here, and so is every line it tries to parse. A read
/// is capped at what remains, on the open handle, so a file that grows
/// between its listing and its read cannot take the pass past the bound.
// trace:BUG-2629 | ai:claude
struct ReadBudget {
    total: u64,
    left: u64,
    attempts: usize,
}

impl ReadBudget {
    fn new(total: u64) -> Self {
        #[cfg(test)]
        let total = READ_BUDGET.with(|b| b.get()).unwrap_or(total);
        #[allow(unused_mut)]
        let mut attempts = MAX_PARSE_ATTEMPTS;
        #[cfg(test)]
        if let Some(limit) = PARSE_BUDGET.with(|b| b.get()) {
            attempts = limit;
        }
        Self {
            total,
            left: total,
            attempts,
        }
    }

    fn exhausted(&self, what: &dyn std::fmt::Display) -> String {
        format!(
            "{what} does not fit the {} bytes that remain of the {}-byte read budget",
            self.left, self.total
        )
    }

    /// Charge `bytes` held for `what`, or refuse them.
    fn charge(&mut self, what: &dyn std::fmt::Display, bytes: u64) -> Result<(), String> {
        if bytes > self.left {
            return Err(self.exhausted(what));
        }
        self.left -= bytes;
        Ok(())
    }

    /// Count one line of `what` about to be parsed, or refuse it: the pass
    /// has then tried as many lines as it may.
    fn attempt(&mut self, what: &dyn std::fmt::Display) -> Result<(), String> {
        if self.attempts == 0 {
            return Err(format!(
                "{what} holds more lines than remain of the pass's parse budget"
            ));
        }
        self.attempts -= 1;
        #[cfg(test)]
        checked::discovery_work_tests::PARSE_ATTEMPTS.with(|n| n.set(n.get() + 1));
        Ok(())
    }

    /// The text of the ledger entry `name` (of the pinned ledger directory,
    /// or of its `by-spec` child), charged as read; `shown` names it in a
    /// refusal. `Ok(None)` only when no such entry exists. The entry is
    /// opened once, beneath the pinned directory, without following a link
    /// and without blocking, and that handle is the one validated and read
    /// ([`checked::Pinned::open`]): a link, a FIFO, a device or a hard link
    /// in a ledger's place is [`Unread::Failed`], as is text that is not
    /// UTF-8. Bytes read are charged whether or not they decode.
    fn read(
        &mut self,
        pinned: &checked::Pinned,
        by_spec: bool,
        name: &str,
        shown: &Path,
    ) -> Result<Option<String>, Unread> {
        use std::io::Read as _;
        step();
        let failed = |what: &dyn std::fmt::Display| {
            Unread::Failed(format!("{} cannot be read: {what}", shown.display()))
        };
        let (handle, meta) = match pinned.open(by_spec, name) {
            Ok(Some(opened)) => opened,
            Ok(None) => return Ok(None),
            Err(why) => return Err(failed(&why)),
        };
        if meta.len() > checked::MAX_LEDGER_BYTES {
            return Err(failed(&"it exceeds the per-file gate-ledger size cap"));
        }
        if meta.len() > self.left {
            return Err(Unread::Exhausted(self.exhausted(&shown.display())));
        }
        let mut bytes = Vec::new();
        let cap = self.left.min(checked::MAX_LEDGER_BYTES);
        let read = handle.take(cap.saturating_add(1)).read_to_end(&mut bytes);
        if bytes.len() as u64 > self.left {
            return Err(Unread::Exhausted(self.exhausted(&shown.display())));
        }
        // What a failed read had read is spent as well.
        self.left -= bytes.len() as u64;
        #[cfg(test)]
        checked::discovery_work_tests::LOSSY_READ_BYTES
            .with(|n| n.set(n.get() + bytes.len() as u64));
        if bytes.len() as u64 > cap {
            return Err(failed(&"it grew past the per-file gate-ledger size cap"));
        }
        read.map_err(|e| failed(&e))?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| failed(&"not UTF-8"))
    }
}

/// Hand `each` every non-blank line of the ledger text `body` (which
/// `shown` names) with its line number and, when the line is an event, the
/// event. Each line is counted against the pass's parse ceiling before it is
/// parsed, and no line is kept: `Err` when the ceiling is reached.
// trace:BUG-2629 | ai:claude
fn scan_ledger<'a>(
    budget: &mut ReadBudget,
    shown: &Path,
    body: &'a str,
    mut each: impl FnMut(usize, &'a str, Option<GateEvent>),
) -> Result<(), String> {
    for (n, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        budget.attempt(&shown.display())?;
        let event = (line.len() <= checked::MAX_LINE_BYTES)
            .then(|| serde_json::from_str(line).ok())
            .flatten();
        each(n + 1, line, event);
    }
    Ok(())
}

impl ReworkBindings {
    fn note(&mut self, change: ChangeKey, proved: bool) {
        if let Some(&position) = self.positions.get(&change) {
            self.inventory[position].1 |= proved;
        } else {
            self.positions.insert(change, self.inventory.len());
            self.inventory.push((change, proved));
        }
    }

    fn fail(&mut self, why: String) {
        self.failure.get_or_insert(why);
    }

    /// Whether the verdict stored under `key` is bound to the spec.
    #[cfg(test)]
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
    /// full read is not repeated per key), and the bytes read from ledgers
    /// and legacy files, the signed bodies received and the findings kept
    /// are all charged to one [`MAX_RECOVERY_BYTES`] budget. Only the events
    /// recorded for a label are kept past their ledger's turn, within the
    /// bytes their ledger was charged. `Err` names where the budget ran
    /// out: the search for a wait is then incomplete, and the caller refuses.
    ///
    /// A source that exists and cannot be read, parsed or verified does not
    /// end the pass (a wait elsewhere is still worth naming); it is reported
    /// as [`Recovered::unknown`], and the caller refuses on it.
    // trace:BUG-2629 | ai:claude
    pub(crate) fn recover(
        &self,
        project_root: &Path,
        labels: &[String],
        cache: &mut DiscoveryCache,
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
            let key = change.as_key();
            let events = cache
                .recovery_ledger(
                    false,
                    &store::ledger_file(root, change),
                    &key,
                    &mut budget,
                    &mut out.unknown,
                )?
                .into_iter()
                .filter(|e| !e.is_spec_only())
                .collect::<Vec<_>>();
            let view: Vec<&GateEvent> = events
                .iter()
                .filter(|e| project::terminal_annotation_matches(e, &key))
                .collect();
            // A change the bindings merely list (a write-ahead binding
            // precedes its append, and may outlive one that never came)
            // answers for the spec only when its answering review is
            // recorded for the spec, as the ordinary binding check has it.
            // A round whose verdict cannot be read is judged too when it is
            // recorded for the spec: it is unknown evidence about it.
            let concerns = proved
                || newest(&standing(&view)).is_some_and(|e| labels.iter().any(|l| names(e, l)))
                || view
                    .iter()
                    .any(|e| unreadable(e) && labels.iter().any(|l| names(e, l)));
            if concerns {
                let found = judge(project_root, &key, &view, &mut budget, &mut out.unknown)?;
                out.take(&key, found, &mut budget)?;
            }
            for event in events {
                if let Some(at) = labels.iter().position(|label| names(&event, label)) {
                    by_label[at].push(event);
                }
            }
        }
        for (label, mut bound) in labels.iter().zip(by_label) {
            let mut unbound = Vec::new();
            if let Some(spec) = project::spec_key(label) {
                let file = store::unbound_file(root, &spec);
                // An unbound record is keyed by its spec.
                let protected =
                    record_identities(bound.iter().filter(|event| project::ownership_free(event)));
                unbound =
                    cache.recovery_ledger(true, &file, &spec, &mut budget, &mut out.unknown)?;
                // An ownership-free bound copy can answer. An unverified
                // bound copy is replaced by its independent unbound copy.
                unbound.retain(|event| !has_record(&protected, event));
                let replaced = record_identities(&unbound);
                bound.retain(|event| !has_record(&replaced, event));
            }
            let merged = project::merge_streams(bound.into_iter().chain(unbound));
            let view: Vec<&GateEvent> = merged
                .iter()
                .filter(|e| project::terminal_annotation_matches(e, label))
                .collect();
            let found = judge(project_root, label, &view, &mut budget, &mut out.unknown)?;
            out.take(label, found, &mut budget)?;
        }
        Ok(out)
    }
}

// The JSON record's Hash and Eq use the same identity. Retain references
// while comparing streams so deduplication does not copy verdict payloads.
// trace:BUG-2629 | ai:codex
fn record_identities<'a>(events: impl IntoIterator<Item = &'a GateEvent>) -> HashSet<&'a JsonObj> {
    events
        .into_iter()
        .filter_map(GateEvent::verdict_record)
        .collect()
}

fn has_record(records: &HashSet<&JsonObj>, event: &GateEvent) -> bool {
    event
        .verdict_record()
        .is_some_and(|record| records.contains(record))
}

/// The verdict events of one ledger for a recovery: the entry `entry`
/// (`by-spec` or not, and its name) of the pinned ledger, which `file`
/// names, read under `budget`. `Err` when the budget ran out.
///
/// Every line is held to the envelope the strict reader requires of the
/// ledger it is read from ([`checked`]): it parses, its version is
/// supported, it is bound to `source` (the change the ledger belongs to, or
/// the spec of an unbound ledger) and its sequence number advances. A line
/// that is not is no evidence about this key: a record misplaced here, which
/// strict discovery refuses, answers nothing, clears no wait and supplies no
/// findings. It is noted in `unknown` by name, as is a ledger that exists
/// and cannot be read or ends in a torn line; the events that do belong are
/// still returned, so a wait among them is named.
// trace:BUG-2629 | ai:claude
#[derive(Default)]
struct RawLedger {
    events: Vec<GateEvent>,
    // The label scan may ignore malformed lines that cannot name its spec.
    malformed: Vec<String>,
    labels: HashSet<String>,
    issue: Option<String>,
    bytes: u64,
    attempts: usize,
}

fn read_raw_ledger(
    budget: &mut ReadBudget,
    pinned: Option<&checked::Pinned>,
    entry: (bool, &str),
    file: &Path,
    source: &str,
) -> Result<RawLedger, Unread> {
    let Some(pinned) = pinned else {
        return Ok(RawLedger::default());
    };
    let Some(body) = budget.read(pinned, entry.0, entry.1, file)? else {
        return Ok(RawLedger::default());
    };
    let mut raw = RawLedger {
        bytes: body.len() as u64,
        ..RawLedger::default()
    };
    if !body.is_empty() && !body.ends_with('\n') {
        raw.issue = Some(format!("{} ends in a torn line", file.display()));
    }
    let mut seq: Option<u64> = None;
    scan_ledger(budget, file, &body, |n, line, event| {
        raw.attempts += 1;
        if let Some(label) = event.as_ref()
            .filter(|event| !event.is_spec_only() && event.verdict_record().is_some())
            .and_then(|event| event.spec.as_deref())
        {
            raw.labels.insert(label.trim().to_ascii_uppercase());
        }
        let refused = match &event {
            None => Some("cannot be parsed".to_string()),
            Some(e) if e.v == 0 || e.v > EVENT_VERSION => {
                Some(format!("has unsupported event version {}", e.v))
            }
            Some(e) if !e.change.eq_ignore_ascii_case(source) => {
                Some(format!("is bound to {}, not {source}", e.change))
            }
            Some(e) if seq.is_some_and(|prev| prev >= e.seq) => {
                Some("does not advance the ledger's sequence".to_string())
            }
            Some(_) => None,
        };
        match (refused, event) {
            (Some(why), event) => {
                if event.is_none() {
                    raw.malformed.push(line.to_ascii_lowercase());
                }
                raw.issue.get_or_insert_with(|| format!(
                    "{} holds a record that cannot be parsed or does not belong to it: line {n} {why}",
                    file.display()
                ));
            }
            (None, Some(event)) => {
                seq = Some(event.seq);
                if event.verdict_record().is_some() {
                    raw.events.push(event);
                }
            }
            (None, None) => {}
        }
    }).map_err(Unread::Exhausted)?;
    Ok(raw)
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

/// A review round whose verdict cannot be read (the record has no verdict
/// field, say): what it decided is unknown. It is never "no review".
// trace:BUG-2629 | ai:claude
fn unreadable(event: &GateEvent) -> bool {
    annotation(event).is_none()
        && event
            .verdict_record()
            .and_then(rv::parse_ledger_record)
            .is_none()
}

/// The readable review rounds of `view`, oldest first: no annotations, and
/// no round whose verdict cannot be read ([`unreadable`]).
fn readable<'a>(view: &[&'a GateEvent]) -> Vec<&'a GateEvent> {
    view.iter()
        .copied()
        .filter(|e| annotation(e).is_none() && !unreadable(e))
        .collect()
}

/// The review rounds of `view` that stand while ownership is unknown: the
/// waits, withheld or not, and the rounds no witness could disqualify. A
/// round whose ownership is unverified clears nothing, so a later approval
/// or refusal that a foreign change might own does not hide an earlier wait.
/// A round whose verdict cannot be read stands for nothing and clears
/// nothing either: the wait before it is kept.
fn standing<'a>(view: &[&'a GateEvent]) -> Vec<&'a GateEvent> {
    readable(view)
        .into_iter()
        .filter(|e| is_wait(e) || project::ownership_free(e))
        .collect()
}

/// What the answering record `body` of `key` establishes: an open wait,
/// current open findings, or nothing; `None` when the record cannot be read
/// as a verdict, which is unknown, never nothing. `closure` is the view's
/// latest terminal annotation; one that cannot be read closes nothing.
// trace:BUG-2629 | ai:claude
fn settle(key: &str, closure: Option<&JsonObj>, body: String, current: bool) -> Option<Standing> {
    let own = rv::parse_recorded_verdict(&body)?;
    let closed = closure
        .and_then(|record| rv::terminal_wait_overlay_body(key, &body, record))
        .and_then(|over| rv::parse_recorded_verdict(&over).map(|verdict| (over, verdict)));
    let (body, verdict) = closed.unwrap_or((body, own));
    Some(match verdict {
        v if v.kind == VerdictKind::Waiting && !v.is_closed() => Standing::Wait(v),
        v if current && !v.is_closed() && v.kind.requires_changes() => Standing::Findings(body, v),
        _ => Standing::Nothing,
    })
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
/// 1. a verified signed publication answers alone, whatever the raw ledger
///    holds;
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
///
/// A publication that cannot be verified or read, a legacy file that is
/// refused, unreadable or malformed, or a ledger round whose verdict cannot
/// be read, is not absent: it is noted in `unknown` (the caller refuses on
/// it), takes no precedence and clears nothing, and the readable rounds are
/// still judged so a wait behind it is named. The legacy file is read within
/// `budget`, which is charged every byte read from it whether or not it
/// parses, and the signed body is charged to it; `Err` when it runs out.
// trace:BUG-2629 | ai:claude
fn judge(
    project_root: &Path,
    key: &str,
    view: &[&GateEvent],
    budget: &mut ReadBudget,
    unknown: &mut Option<String>,
) -> Result<Standing, String> {
    let closure = view.iter().rev().find_map(|e| annotation(e));
    match published(project_root, key) {
        Ok(Some(signed)) => {
            budget.charge(
                &format_args!("the signed review of {key}"),
                signed.body.len() as u64,
            )?;
            match settle(key, closure, signed.body, true) {
                Some(found) => return Ok(found),
                None => {
                    unknown.get_or_insert_with(|| {
                        format!("the signed review publication of {key} cannot be read")
                    });
                }
            }
        }
        Ok(None) => {}
        Err(e) => {
            unknown.get_or_insert_with(|| {
                format!("the signed review publication of {key} cannot be verified: {e:#}")
            });
        }
    }
    let path = rv::verdict_path(project_root, key);
    step();
    // Every byte read from the legacy file is spent, parsed or not: a
    // malformed file must not leave the allowance whole for the next key.
    let mut consumed = 0;
    let read = rv::checked_legacy_record(project_root, key, budget.left, &mut consumed);
    budget.charge(&path.display(), consumed)?;
    let (file_body, legacy) = match read {
        Ok(Some((body, verdict))) => (Some(body), Some(verdict)),
        Ok(None) => (None, None),
        Err(e)
            if e.get_ref()
                .is_some_and(|inner| inner.is::<rv::OverReadLimit>()) =>
        {
            return Err(budget.exhausted(&path.display()));
        }
        Err(e) => {
            unknown.get_or_insert_with(|| format!("{} cannot be read: {e}", path.display()));
            (None, None)
        }
    };
    let supersedes = |event: &GateEvent| {
        legacy
            .as_ref()
            .is_none_or(|file| rv::ledger_supersedes(&path, Some(file), event))
    };
    if view.iter().any(|e| unreadable(e)) {
        unknown.get_or_insert_with(|| {
            format!("a review round recorded for {key} holds no readable verdict")
        });
    }
    let rounds = readable(view);
    let mut unsettled = || {
        unknown.get_or_insert_with(|| format!("the review recorded for {key} cannot be read"));
        Standing::Nothing
    };
    if let Some(event) = newest(&standing(view)).filter(|e| supersedes(e)) {
        let current = project::ownership_free(event)
            && newest(&rounds).is_some_and(|last| std::ptr::eq(last, event));
        return Ok(event
            .verdict_record()
            .and_then(|record| serde_json::to_string(record).ok())
            .and_then(|body| settle(key, closure, body, current))
            .unwrap_or_else(&mut unsettled));
    }
    Ok(match file_body {
        Some(body) => {
            let current = !rounds.iter().any(|e| supersedes(e));
            settle(key, closure, body, current).unwrap_or_else(&mut unsettled)
        }
        None => Standing::Nothing,
    })
}

/// Review evidence owned by one queue invocation. Failed and absent reads
/// stay pinned too, so another consumer cannot retry discovery implicitly.
// trace:BUG-2629 | ai:codex
#[derive(Default)]
pub(crate) struct DiscoveryCache {
    root: Option<PathBuf>,
    discovery: Option<Result<Option<checked::Discovery>, String>>,
    budget: checked::Budget,
    evidence: checked::QueueEvidence,
    change_events: HashMap<ChangeKey, Result<Vec<GateEvent>, String>>,
    label_events: HashMap<String, Result<Vec<GateEvent>, String>>,
    listed: HashMap<String, Result<Option<Vec<ChangeKey>>, String>>,
    inventories: HashMap<Vec<String>, ReworkBindings>,
    current: HashMap<String, Result<Option<rv::CurrentVerdict>, String>>,
    closures: HashMap<String, JsonObj>,
    signed_generation: Option<Result<Option<ReviewStoreGeneration>, String>>,
    signed: HashMap<Vec<String>, Result<Option<rv::CurrentVerdict>, String>>,
    fallback: Option<Result<Option<checked::Pinned>, String>>,
    fallback_hints: HashMap<String, Result<Option<Vec<String>>, String>>,
    raw: HashMap<(bool, String), Result<RawLedger, Unread>>,
    raw_budget: Option<ReadBudget>,
    verdict_budget: Option<ReadBudget>,
    pub(super) pickups: HashMap<
        (String, String, Option<String>),
        Result<crate::rework_direction::ReviewResult<String>, super::work::ReworkReviewError>,
    >,
}

impl DiscoveryCache {
    fn discovery(
        &mut self,
        project_root: &Path,
    ) -> Result<Option<&mut checked::Discovery>, String> {
        step();
        self.discovery
            .get_or_insert_with(|| {
                let root = ledger::store_root(project_root).map_err(|e| e.to_string())?;
                self.evidence.watch(ledger::ledger_dir(&root));
                let discovery = checked::discover_for_queue(&root).map_err(|e| e.0);
                self.root = Some(root);
                discovery
            })
            .as_mut()
            .map(Option::as_mut)
            .map_err(|why| why.clone())
    }

    fn listed(
        &mut self,
        project_root: &Path,
        spec: &str,
    ) -> Result<Option<Vec<ChangeKey>>, String> {
        if let Some(listed) = self.listed.get(spec) {
            return listed.clone();
        }
        let listed = match self.discovery(project_root) {
            Ok(Some(discovery)) => discovery
                .listed(spec)
                .map(|listed| {
                    listed.map(|keys| {
                        keys.iter()
                            .filter_map(|key| ChangeKey::parse(key))
                            .collect()
                    })
                })
                .map_err(|why| why.0),
            // Uncertified ledgers are enumerated by label. Hints may also
            // tie a legacy verdict file to an otherwise unrecorded change.
            _ => Ok(None),
        };
        self.listed.insert(spec.to_string(), listed.clone());
        listed
    }

    // A certified discovery shares this exact hint with its ownership folds.
    // An uncertified one can still read hints through pinned fallback handles.
    // trace:BUG-2629 | ai:codex
    fn index_hint(
        &mut self,
        project_root: &Path,
        spec: &str,
    ) -> Result<Option<Vec<ChangeKey>>, String> {
        let mut budget = std::mem::take(&mut self.budget);
        let found = match self.discovery(project_root) {
            Ok(Some(discovery)) => discovery
                .queue_index_hint(spec, &mut budget)
                .map_err(|e| e.0),
            Ok(None) => Ok(None),
            Err(why) => self
                .fallback_hints
                .entry(spec.to_string())
                .or_insert_with(|| {
                    let root = self.root.as_deref().ok_or(why)?;
                    let by_spec = ledger::ledger_dir(root).join("by-spec");
                    self.evidence.watch(by_spec.clone());
                    self.evidence.watch(by_spec.join(format!("{spec}.json")));
                    let pinned = self
                        .fallback
                        .get_or_insert_with(|| checked::pin(root).map_err(|e| e.0));
                    match pinned.as_ref().map_err(Clone::clone)? {
                        Some(pinned) => pinned.index_hint(spec, &mut budget).map_err(|e| e.0),
                        None => Ok(None),
                    }
                })
                .clone(),
        };
        self.budget = budget;
        found.map(|hint| {
            hint.map(|keys| {
                keys.iter()
                    .filter_map(|key| ChangeKey::parse(key))
                    .collect()
            })
        })
    }

    fn ownership_checked(
        &mut self,
        project_root: &Path,
        change: ChangeKey,
    ) -> Result<&[GateEvent], String> {
        if !self.change_events.contains_key(&change) {
            let mut budget = std::mem::take(&mut self.budget);
            let found = match self.discovery(project_root) {
                Ok(Some(discovery)) => discovery
                    .queue_change_events(change, &mut budget)
                    .map_err(|why| why.0),
                Ok(None) => Ok(Vec::new()),
                Err(why) => {
                    let Some(root) = self.root.as_deref() else {
                        return Err(why);
                    };
                    let file = store::ledger_file(root, change);
                    match self.raw_ledger(false, &file, &change.as_key()) {
                        Ok(raw)
                            if raw.issue.is_none() && !project::needs_witnesses(&raw.events) =>
                        {
                            Ok(raw
                                .events
                                .iter()
                                .filter(|event| !event.is_spec_only())
                                .cloned()
                                .collect())
                        }
                        Ok(raw) => Err(raw.issue.clone().unwrap_or(why)),
                        Err(error) => Err(error.why()),
                    }
                }
            }
            .and_then(|events| {
                if events
                    .iter()
                    .any(|event| event.verdict_record().is_some() && unreadable(event))
                {
                    Err(format!(
                        "{} holds a review round with no readable verdict",
                        change.as_key()
                    ))
                } else {
                    Ok(events)
                }
            });
            self.budget = budget;
            self.change_events.insert(change, found);
        }
        self.change_events[&change].as_deref().map_err(Clone::clone)
    }

    fn inventory(&mut self, project_root: &Path, labels: &[String]) -> ReworkBindings {
        if let Some(inventory) = self.inventories.get(labels) {
            return inventory.clone();
        }
        let inventory = discover_inventory(self, project_root, labels);
        self.inventories.insert(labels.to_vec(), inventory.clone());
        inventory
    }

    fn raw_ledger(
        &mut self,
        by_spec: bool,
        file: &Path,
        source: &str,
    ) -> Result<&RawLedger, Unread> {
        let key = (by_spec, source.to_string());
        if !self.raw.contains_key(&key) {
            self.evidence.watch(file.to_path_buf());
            let root = self.root.as_deref().expect("inventory has a store root");
            let pinned = self
                .fallback
                .get_or_insert_with(|| checked::pin(root).map_err(|why| why.0));
            let found = match pinned {
                Ok(pinned) => read_raw_ledger(
                    self.raw_budget
                        .get_or_insert_with(|| ReadBudget::new(checked::MAX_DISCOVERY_BYTES)),
                    pinned.as_ref(),
                    (
                        by_spec,
                        file.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default(),
                    ),
                    file,
                    source,
                ),
                Err(why) => Err(Unread::Failed(why.clone())),
            };
            self.raw.insert(key.clone(), found);
        }
        self.raw[&key].as_ref().map_err(Clone::clone)
    }

    fn cached_change(&self, change: ChangeKey) -> Option<(&[GateEvent], u64)> {
        self.discovery
            .as_ref()
            .and_then(|found| found.as_ref().ok().and_then(Option::as_ref))
            .and_then(|discovery| discovery.cached_change(change))
    }

    fn recovery_ledger(
        &mut self,
        by_spec: bool,
        file: &Path,
        source: &str,
        budget: &mut ReadBudget,
        unknown: &mut Option<String>,
    ) -> Result<Vec<GateEvent>, String> {
        let (events, bytes, attempts) = match ChangeKey::parse(source)
            .filter(|_| !by_spec)
            .and_then(|change| self.cached_change(change))
        {
            Some((events, bytes)) => (events, bytes, events.len()),
            None => match self.raw_ledger(by_spec, file, source) {
                Ok(raw) => {
                    if let Some(why) = &raw.issue {
                        unknown.get_or_insert_with(|| why.clone());
                    }
                    (&raw.events[..], raw.bytes, raw.attempts)
                }
                Err(Unread::Failed(why)) => {
                    unknown.get_or_insert(why);
                    return Ok(Vec::new());
                }
                Err(Unread::Exhausted(why)) => return Err(why),
            },
        };
        budget.charge(&file.display(), bytes)?;
        for _ in 0..attempts {
            budget.attempt(&file.display())?;
        }
        Ok(events
            .iter()
            .filter(|event| event.verdict_record().is_some())
            .cloned()
            .collect())
    }
    pub(super) fn events(
        &mut self,
        project_root: &Path,
        bindings: &ReworkBindings,
        key: &str,
    ) -> Result<&[GateEvent], String> {
        if let Some(change) = ChangeKey::parse(key) {
            return self.ownership_checked(project_root, change);
        }
        let Some(spec) = project::spec_key(key) else {
            return Ok(&[]);
        };
        if !self.label_events.contains_key(&spec) {
            let view = self.label_view(project_root, bindings, &spec);
            self.label_events.insert(spec.clone(), view);
        }
        self.label_events[&spec].as_deref().map_err(Clone::clone)
    }

    fn label_view(
        &mut self,
        project_root: &Path,
        bindings: &ReworkBindings,
        spec: &str,
    ) -> Result<Vec<GateEvent>, String> {
        let mut bound = Vec::new();
        for (change, _) in &bindings.inventory {
            if bindings.selected_only_extra == Some(*change) {
                continue;
            }
            bound.extend(
                self.ownership_checked(project_root, *change)?
                    .iter()
                    .filter(|event| event.verdict_record().is_some() && names(event, spec))
                    .cloned(),
            );
        }
        let mut budget = std::mem::take(&mut self.budget);
        let unbound = match self.discovery(project_root) {
            Ok(Some(discovery)) => discovery.unbound(spec, &mut budget).map_err(|why| why.0),
            Ok(None) => Ok(Vec::new()),
            Err(why) => {
                let Some(root) = self.root.as_deref() else {
                    self.budget = budget;
                    return Err(why);
                };
                let file = store::unbound_file(root, spec);
                self.raw_ledger(true, &file, spec)
                    .map_err(Unread::why)
                    .and_then(|raw| {
                        if let Some(why) = &raw.issue {
                            Err(why.clone())
                        } else {
                            Ok(raw.events.clone())
                        }
                    })
            }
        };
        self.budget = budget;
        let identities = record_identities(&bound);
        let mut unbound = unbound?;
        if unbound.iter().any(unreadable) {
            return Err(format!(
                "the unbound ledger of {spec} contains no readable verdict"
            ));
        }
        unbound.retain(|event| !has_record(&identities, event));
        bound.extend(unbound);
        Ok(project::merge_streams(bound)
            .into_iter()
            .filter(|event| project::terminal_annotation_matches(event, spec))
            .collect())
    }

    // Gate-ledger metadata cannot detect changes to signed lookup authority.
    // trace:BUG-2629 | ai:codex
    pub(super) fn check_signed_generation(&mut self, root: &Path) -> std::io::Result<()> {
        self.refresh_signed_generation(root, false)
    }

    pub(super) fn carry_signed_generation_across_store_writes(
        &mut self,
        root: &Path,
    ) -> std::io::Result<()> {
        self.refresh_signed_generation(root, true)
    }

    fn refresh_signed_generation(
        &mut self,
        root: &Path,
        allow_metadata_changes: bool,
    ) -> std::io::Result<()> {
        if let Some(Err(reason)) = &self.signed_generation {
            return Err(std::io::Error::other(reason.clone()));
        }
        let checked = || -> anyhow::Result<_> {
            let live = crate::gate::review_publication::review_store_generation(root)
                .context("canonical review store generation could not be read")?;
            if let Some(Ok(pinned)) = &self.signed_generation {
                let unchanged = pinned == &live
                    || (allow_metadata_changes
                        && match (pinned, &live) {
                            (Some(pinned), Some(live)) => pinned.same_signed_authority(live)?,
                            _ => false,
                        });
                anyhow::ensure!(
                    unchanged,
                    "canonical review store generation changed during review selection"
                );
            }
            Ok(live)
        };
        let live = checked().map_err(|error| format!("{error:#}"));
        let checked = self.signed_generation.insert(live);
        checked
            .as_ref()
            .map(|_| ())
            .map_err(|reason| std::io::Error::other(reason.clone()))
    }

    fn signed(&mut self, root: &Path, ids: &[&str]) -> Result<Option<rv::CurrentVerdict>, String> {
        let keys = ids.iter().map(|key| key.to_string()).collect();
        self.signed
            .entry(keys)
            .or_insert_with(|| {
                let found = match ids {
                    [key] => published(root, key),
                    _ => crate::gate::review_publication::current_verified(root, ids),
                }
                .map_err(|error| {
                    format!("signed review publication could not be verified: {error:#}")
                })?;
                if let Some(found) = &found {
                    self.verdict_budget
                        .get_or_insert_with(|| ReadBudget::new(MAX_RECOVERY_BYTES))
                        .charge(&"signed review publications", found.body.len() as u64)?;
                }
                Ok(found)
            })
            .clone()
    }

    fn legacy_body(&mut self, root: &Path, key: &str) -> Result<Option<String>, String> {
        let budget = self
            .verdict_budget
            .get_or_insert_with(|| ReadBudget::new(MAX_RECOVERY_BYTES));
        let path = rv::verdict_path(root, key);
        self.evidence.watch(path.clone());
        let mut consumed = 0;
        let found = rv::checked_legacy_record(root, key, budget.left, &mut consumed);
        budget.charge(&path.display(), consumed)?;
        found
            .map(|found| found.map(|(body, _)| body))
            .map_err(|why| format!("{} cannot be read: {why}", path.display()))
    }

    pub(super) fn current_verdict(
        &mut self,
        root: &Path,
        bindings: &ReworkBindings,
        key: &str,
    ) -> Result<Option<rv::CurrentVerdict>, String> {
        if !self.current.contains_key(key) {
            let store = self.root.clone().unwrap_or_else(|| root.to_path_buf());
            let current = (|| {
                let signed = self.signed(root, &[key])?;
                // A verified publication answers for this key before local files.
                // trace:BUG-2629 | ai:codex
                let legacy = if signed.is_some() {
                    None
                } else {
                    self.legacy_body(root, key)?
                };
                let events = self.events(root, bindings, key)?;
                if ChangeKey::parse(key).is_some() {
                    let verdicts: Vec<_> = events
                        .iter()
                        .filter(|event| {
                            event.verdict_record().is_some()
                                && project::terminal_annotation_matches(event, key)
                        })
                        .cloned()
                        .collect();
                    Ok(rv::current_verdict_from_events(
                        root, &store, key, &verdicts, legacy, signed,
                    ))
                } else {
                    Ok(rv::current_verdict_from_events(
                        root, &store, key, events, legacy, signed,
                    ))
                }
            })();
            self.current.insert(key.to_string(), current);
        }
        let mut current = self.current[key].clone()?;
        if let (Some(source), Some(record)) = (&mut current, self.closures.get(key)) {
            if let Some(body) = rv::terminal_wait_overlay_body(key, &source.body, record) {
                source.body = body;
            }
        }
        Ok(current)
    }

    pub(super) fn select_verdict(
        &mut self,
        root: &Path,
        bindings: &ReworkBindings,
        ids: &[&str],
    ) -> crate::gate::review_alias::AliasSelection {
        use crate::gate::review_alias::{self, AliasSelection, Selected};
        let labels = review_alias::spec_labels(ids);
        let signed = if labels.len() == 1 {
            self.signed(root, &[&labels[0]])
        } else {
            self.signed(root, ids)
        };
        if let Err(reason) = signed.as_ref() {
            return AliasSelection::Unsafe {
                reason: reason.clone(),
                refusal: None,
            };
        }
        if let Ok(Some(mut remote)) = signed {
            for key in &labels {
                let stored = self.events(root, bindings, key).ok().and_then(|events| {
                    events
                        .iter()
                        .rev()
                        .filter_map(GateEvent::verdict_record)
                        .find(|record| record.contains_key("terminal_wait_source"))
                        .cloned()
                });
                if let Some(body) = self
                    .closures
                    .get(key)
                    .or(stored.as_ref())
                    .and_then(|record| rv::terminal_wait_overlay_body(key, &remote.body, record))
                {
                    remote.body = body;
                }
            }
            return AliasSelection::Selected(Box::new(Selected {
                verdict: rv::parse_recorded_verdict(&remote.body).expect("verified publication"),
                body: remote.body,
                label: ids.first().unwrap_or(&"").to_string(),
                source: remote.source,
                order_time: None,
            }));
        }
        match labels.as_slice() {
            [] => AliasSelection::Absent,
            [label] => match self.current_verdict(root, bindings, label) {
                Ok(Some(current)) => match rv::parse_recorded_verdict(&current.body) {
                    Some(verdict) => AliasSelection::Selected(Box::new(Selected {
                        verdict,
                        label: label.clone(),
                        body: current.body,
                        source: current.source,
                        order_time: None,
                    })),
                    None => AliasSelection::Unsafe {
                        reason: format!("the review of {label} does not parse"),
                        refusal: None,
                    },
                },
                Ok(None) => AliasSelection::Absent,
                Err(reason) => AliasSelection::Unsafe {
                    reason,
                    refusal: None,
                },
            },
            _ => {
                let snapshots = labels
                    .into_iter()
                    .map(|label| {
                        let events = self.events(root, bindings, &label)?.to_vec();
                        Ok((label, events))
                    })
                    .collect::<Result<Vec<_>, String>>();
                match snapshots {
                    Ok(events) => review_alias::choose_cached(
                        root,
                        self.root.as_deref().unwrap_or(root),
                        events,
                    ),
                    Err(reason) => AliasSelection::Unsafe {
                        reason,
                        refusal: None,
                    },
                }
            }
        }
    }

    // Reuse the checked discovery to fence cached evidence after forge lookup.
    // trace:BUG-2629 | ai:codex
    fn revalidate_wait_evidence(&mut self, root: &Path) -> std::io::Result<()> {
        self.check_signed_generation(root)?;
        self.evidence.revalidate()?;
        if let Some(Ok(Some(discovery))) = &self.discovery {
            discovery.revalidate_queue_evidence()?;
        }
        Ok(())
    }

    // Hold the ledger write lock across revalidation and annotation appends.
    // trace:BUG-2629 | ai:codex
    pub(super) fn close_waits(
        &mut self,
        root: &Path,
        waits: &[(String, ChangeKey, rv::CurrentVerdict)],
    ) -> std::io::Result<()> {
        if waits.is_empty() || ledger::contained() {
            return self.revalidate_wait_evidence(root);
        }
        let store_root = self.root.clone().unwrap_or_else(|| root.to_path_buf());
        store::with_exclusive(&store_root, || {
            self.revalidate_wait_evidence(root)?;
            for (key, change, source) in waits {
                let annotation = rv::append_terminal_wait(root, &store_root, key, *change, source)?;
                self.closures.insert(key.clone(), annotation);
            }
            Ok(())
        })
    }
}

pub(crate) struct WaitingReviews<'a> {
    pub(super) root: &'a Path,
    pub(super) cache: &'a mut DiscoveryCache,
    pub(super) bindings: &'a ReworkBindings,
    pub(super) terminal: Vec<(String, ChangeKey, rv::CurrentVerdict)>,
}

impl crate::review_waiting::WaitReviews for WaitingReviews<'_> {
    fn current(&mut self, key: &str) -> Option<rv::CurrentVerdict> {
        self.cache
            .current_verdict(self.root, self.bindings, key)
            .ok()
            .flatten()
    }

    fn signed(&mut self, key: &str) -> Option<rv::CurrentVerdict> {
        self.cache.signed(self.root, &[key]).ok().flatten()
    }

    fn changes(&self, label: &str) -> Vec<ChangeKey> {
        self.bindings.hints.get(label).cloned().unwrap_or_default()
    }

    fn close(
        &mut self,
        key: &str,
        change: ChangeKey,
        source: &rv::CurrentVerdict,
    ) -> std::io::Result<()> {
        self.terminal
            .push((key.to_string(), change, source.clone()));
        Ok(())
    }
}

/// Every change whose ledger records a review for one of `labels`, from one
/// bounded pass over the change ledgers: what the per-key check found by
/// each event's label, for a ledger whose bindings certify nothing or whose
/// strict full read refused. No payload is kept.
///
/// `Err`, naming where, when the pass cannot establish that it covered the
/// ledger: it would cross its bounds, the directory cannot be listed, or a
/// ledger in it cannot be read. Only a ledger directory that does not exist
/// is empty. A line that is not an event is passed over unless it carries a
/// label's text: an event recorded for a label names it, so a line without
/// it is no review of the spec, and one with it may be.
///
/// The bounds are bytes read ([`checked::MAX_DISCOVERY_BYTES`]) and lines
/// tried ([`MAX_PARSE_ATTEMPTS`]), shared by every ledger of the pass; each
/// ledger is opened beneath the pinned ledger directory, without following a
/// link ([`ReadBudget::read`]).
// trace:BUG-2629 | ai:claude
fn labelled_changes_with_cache(
    cache: &mut DiscoveryCache,
    root: &Path,
    labels: &[String],
) -> Result<Vec<ChangeKey>, String> {
    step();
    let dir = ledger::ledger_dir(root);
    let unlisted = |e: &str| format!("{} cannot be listed: {e}", dir.display());
    let pin = cache
        .fallback
        .get_or_insert_with(|| checked::pin(root).map_err(|why| why.0));
    let Some(pinned) = pin.as_ref().map_err(|why| unlisted(why))? else {
        return Ok(Vec::new());
    };
    let changes = pinned.names().map_err(|why| unlisted(&why.0))?;
    let wanted: Vec<_> = labels
        .iter()
        .map(|label| label.trim().to_ascii_lowercase())
        .filter(|label| !label.is_empty())
        .collect();
    let mut found = Vec::new();
    for (index, change) in changes
        .iter()
        .filter_map(|name| {
            bindings::change_ledger_name(name).and_then(|key| ChangeKey::parse(&key))
        })
        .enumerate()
    {
        if index >= checked::MAX_CHANGE_LEDGERS {
            return Err(format!(
                "{} holds more than {} change ledgers",
                dir.display(),
                checked::MAX_CHANGE_LEDGERS
            ));
        }
        let relevant = if let Some((events, _)) = cache.cached_change(change) {
            events.iter().any(|event| {
                !event.is_spec_only()
                    && event.verdict_record().is_some()
                    && labels.iter().any(|label| names(event, label))
            })
        } else {
            let file = store::ledger_file(root, change);
            let raw = cache
                .raw_ledger(false, &file, &change.as_key())
                .map_err(Unread::why)?;
            if let Some(label) = wanted.iter().find(|label| {
                raw.malformed
                    .iter()
                    .any(|line| line.contains(label.as_str()))
            }) {
                return Err(format!(
                    "{} holds a record that cannot be parsed and may review {}",
                    file.display(),
                    label.to_ascii_uppercase()
                ));
            }
            labels.iter().any(|label| raw.labels.contains(label))
        };
        if relevant {
            found.push(change);
        }
    }
    Ok(found)
}

#[cfg(test)]
fn labelled_changes(root: &Path, labels: &[String]) -> Result<Vec<ChangeKey>, String> {
    let mut cache = DiscoveryCache {
        root: Some(root.to_path_buf()),
        ..DiscoveryCache::default()
    };
    labelled_changes_with_cache(&mut cache, root, labels)
}

/// Build the spec inventory once, independently of selecting a change.
// trace:BUG-2629 | ai:codex
fn discover_inventory(
    cache: &mut DiscoveryCache,
    project_root: &Path,
    labels: &[String],
) -> ReworkBindings {
    let mut out = ReworkBindings::default();
    let uncertified = cache.discovery(project_root).is_err();
    let Some(root) = cache.root.clone() else {
        return out;
    };
    let mut scan = uncertified;
    for label in labels {
        let Some(spec) = project::spec_key(label) else {
            continue;
        };
        let hint = match cache.index_hint(project_root, &spec) {
            Ok(hint) => hint,
            Err(why) => {
                out.refusal = Some(why);
                break;
            }
        };
        let listed = match cache.listed(project_root, &spec) {
            Ok(listed) => listed,
            Err(why) => {
                out.fail(why);
                scan = true;
                None
            }
        };
        let lost = checked::binding_lost(hint.as_deref(), listed.as_deref());
        if let Some(hint) = &hint {
            out.hints.insert(spec.clone(), hint.clone());
        }
        for change in hint.into_iter().flatten() {
            out.note(change, true);
        }
        for change in listed.into_iter().flatten() {
            out.note(change, false);
        }
        if lost && !uncertified {
            match cache.discovery(project_root).and_then(|discovery| {
                discovery
                    .map(|d| d.queue_recorded_changes(&spec).map_err(|e| e.0))
                    .unwrap_or_else(|| Ok(Vec::new()))
            }) {
                Ok(found) => found
                    .iter()
                    .filter_map(|key| ChangeKey::parse(key))
                    .for_each(|change| out.note(change, false)),
                Err(why) => {
                    out.fail(why);
                    scan = true;
                }
            }
        }
    }
    if scan && out.refusal.is_none() {
        match labelled_changes_with_cache(cache, &root, labels) {
            Ok(found) => found.into_iter().for_each(|change| out.note(change, false)),
            Err(why) => out.refusal = Some(why),
        }
    }
    out.root = Some(root);
    out
}

impl ReworkBindings {
    pub(super) fn bound_keys(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.bound.iter().map(|change| change.as_key()).collect();
        keys.sort();
        keys
    }
}

pub(super) fn discover_with_cache(
    cache: &mut DiscoveryCache,
    project_root: &Path,
    labels: &[String],
    open_change: Option<&str>,
    selected_only: bool,
) -> ReworkBindings {
    let selected = open_change.and_then(ChangeKey::parse);
    let mut selected_check = None;
    if let Some(change) = selected {
        selected_check = cache.ownership_checked(project_root, change).err();
    }
    let mut out = cache.inventory(project_root, labels);
    if let Some(why) = selected_check {
        out.fail(why);
    }
    if out.refusal.is_none() && out.failure.is_none() && !selected_only {
        for (change, proved) in &out.inventory {
            let events = match cache.ownership_checked(project_root, *change) {
                Ok(events) => events,
                Err(why) => {
                    out.fail(why);
                    break;
                }
            };
            let key = change.as_key();
            let bound = *proved
                || project::newest_of(
                    events
                        .iter()
                        .filter(|event| {
                            event.verdict_record().is_some()
                                && project::terminal_annotation_matches(event, &key)
                        })
                        .cloned()
                        .collect(),
                )
                .is_some_and(|event| labels.iter().any(|label| names(&event, label)));
            if bound {
                out.bound.push(*change);
            }
        }
    } else if !selected_only {
        out.bound = out
            .inventory
            .iter()
            .filter_map(|(c, proved)| proved.then_some(*c))
            .collect();
    }
    if let Some(change) = selected {
        if !out.positions.contains_key(&change) {
            out.selected_only_extra = Some(change);
        }
        out.note(change, true);
    }
    out
}

#[cfg(test)]
fn discover(
    project_root: &Path,
    labels: &[String],
    open_change: Option<&str>,
    selected_only: bool,
) -> ReworkBindings {
    discover_with_cache(
        &mut DiscoveryCache::default(),
        project_root,
        labels,
        open_change,
        selected_only,
    )
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

#[cfg(test)]
#[path = "rework_discovery_ratchet.rs"]
mod discovery_ratchet;
