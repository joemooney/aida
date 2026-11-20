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
    /// The first source of the spec's reviews that could not be read, parsed
    /// or verified. What it holds is unknown, a waiting review included, so
    /// nothing read beside it admits rework: `findings` are not actionable
    /// and the absence of a `wait` proves nothing.
    pub(crate) unknown: Option<String>,
    /// The record bodies behind `findings`: one review read under its
    /// change key and under its spec key is imported once.
    bodies: Vec<String>,
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
                    self.bodies.push(body);
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

/// The byte budget of one bounded pass: every byte it reads and every
/// payload it retains is charged here. A read is capped at what remains, on
/// the open handle, so a file that grows between its listing and its read
/// cannot take the pass past the bound.
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

    /// The text of the regular file at `file`, charged as read. `Ok(None)`
    /// only when no such file exists; a file that exists and cannot be read
    /// (not regular, unreadable, not UTF-8) is [`Unread::Failed`].
    fn read(&mut self, file: &Path) -> Result<Option<String>, Unread> {
        use std::io::Read as _;
        step();
        let failed = |what: &dyn std::fmt::Display| {
            Unread::Failed(format!("{} cannot be read: {what}", file.display()))
        };
        // A regular-file check comes before the open, so a FIFO in a
        // ledger's place never hangs the pass. trace:BUG-2543 | ai:claude
        match std::fs::metadata(file) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(failed(&"not a regular file")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return match std::fs::symlink_metadata(file) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    _ => Err(failed(&"a link to nothing")),
                };
            }
            Err(e) => return Err(failed(&e)),
        }
        let handle = std::fs::File::open(file).map_err(|e| failed(&e))?;
        let meta = handle.metadata().map_err(|e| failed(&e))?;
        if !meta.is_file() {
            return Err(failed(&"not a regular file"));
        }
        if meta.len() > self.left {
            return Err(Unread::Exhausted(self.exhausted(&file.display())));
        }
        let mut bytes = Vec::new();
        handle
            .take(self.left.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| failed(&e))?;
        if bytes.len() as u64 > self.left {
            return Err(Unread::Exhausted(self.exhausted(&file.display())));
        }
        self.left -= bytes.len() as u64;
        #[cfg(test)]
        checked::discovery_work_tests::LOSSY_READ_BYTES
            .with(|n| n.set(n.get() + bytes.len() as u64));
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| failed(&"not UTF-8"))
    }
}

/// The events of a ledger's text in file order, and its non-blank lines
/// that are not events.
// trace:BUG-2629 | ai:claude
fn parse_ledger(body: &str) -> (Vec<GateEvent>, Vec<&str>) {
    let (mut events, mut torn) = (Vec::new(), Vec::new());
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str(line) {
            Ok(event) => events.push(event),
            Err(_) => torn.push(line),
        }
    }
    (events, torn)
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
            let events = recovery_events(
                &mut budget,
                &store::ledger_file(root, change),
                &mut out.unknown,
            )?
            .into_iter()
            .filter(|e| !e.is_spec_only())
            .collect::<Vec<_>>();
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
                for event in recovery_events(
                    &mut budget,
                    &store::unbound_file(root, &spec),
                    &mut out.unknown,
                )? {
                    let copies = |b: &GateEvent| b.verdict_record() == event.verdict_record();
                    // A record bound to a change is read once, from the
                    // change, as in the projection: when that copy can
                    // answer. A copy whose ownership is unverified answers
                    // nothing here, so the unbound record, which no foreign
                    // change can own, stands in its place.
                    if bound
                        .iter()
                        .any(|b| copies(b) && project::ownership_free(b))
                    {
                        continue;
                    }
                    bound.retain(|b| !copies(b));
                    unbound.push(event);
                }
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

/// The verdict events of the ledger at `file` for a recovery, read under
/// `budget`. `Err` when the budget ran out. A ledger that exists and cannot
/// be read, or holds a line that is not an event, is noted in `unknown`:
/// what it records is not known, and the events that did parse are still
/// returned so a wait among them is named.
// trace:BUG-2629 | ai:claude
fn recovery_events(
    budget: &mut ReadBudget,
    file: &Path,
    unknown: &mut Option<String>,
) -> Result<Vec<GateEvent>, String> {
    let body = match budget.read(file) {
        Ok(body) => body.unwrap_or_default(),
        Err(Unread::Exhausted(why)) => return Err(why),
        Err(Unread::Failed(why)) => {
            unknown.get_or_insert(why);
            String::new()
        }
    };
    let (events, torn) = parse_ledger(&body);
    if !torn.is_empty() {
        unknown.get_or_insert_with(|| {
            format!("{} holds a record that cannot be parsed", file.display())
        });
    }
    Ok(events
        .into_iter()
        .filter(|e| e.verdict_record().is_some())
        .collect())
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
/// A publication that cannot be verified, or a legacy file that is refused,
/// unreadable or malformed, is not absent: it is noted in `unknown` (the
/// caller refuses on it), takes no precedence, and the raw rounds are still
/// judged so a wait behind it is named. The legacy file is read within
/// `budget` and the signed body is charged to it; `Err` when it runs out.
// trace:BUG-2629 | ai:claude
fn judge(
    project_root: &Path,
    key: &str,
    view: &[&GateEvent],
    budget: &mut ReadBudget,
    unknown: &mut Option<String>,
) -> Result<Standing, String> {
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
    match published(project_root, key) {
        Ok(Some(signed)) => {
            budget.charge(
                &format_args!("the signed review of {key}"),
                signed.body.len() as u64,
            )?;
            return Ok(settle(signed.body, true));
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
    let (file_body, legacy) = match rv::checked_legacy_record(project_root, key, budget.left) {
        Ok(Some((body, verdict))) => {
            budget.charge(&path.display(), body.len() as u64)?;
            (Some(body), Some(verdict))
        }
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
            return Ok(Standing::Nothing);
        };
        let current = project::ownership_free(event)
            && newest(&rounds).is_some_and(|last| std::ptr::eq(last, event));
        return Ok(settle(body, current));
    }
    Ok(match file_body {
        Some(body) => settle(body, !rounds.iter().any(|e| supersedes(e))),
        None => Standing::Nothing,
    })
}

/// Whether the projection readers can establish ownership of `change`'s
/// review copies: the condition under which they present them rather than
/// withhold them ([`checked::filter_miskeyed_or_fail_closed`]).
fn ownership_checked(root: &Path, change: ChangeKey) -> Result<(), String> {
    step();
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
// trace:BUG-2629 | ai:claude
fn labelled_changes(root: &Path, labels: &[String]) -> Result<Vec<ChangeKey>, String> {
    use std::io::ErrorKind::NotFound;
    step();
    let dir = ledger::ledger_dir(root);
    let unlisted = |e: std::io::Error| format!("{} cannot be listed: {e}", dir.display());
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e)
            if e.kind() == NotFound
                && matches!(std::fs::symlink_metadata(&dir), Err(e) if e.kind() == NotFound) =>
        {
            return Ok(Vec::new())
        }
        Err(e) => return Err(unlisted(e)),
    };
    let mut changes = Vec::new();
    for entry in entries {
        let name = entry.map_err(&unlisted)?.file_name();
        let Some(change) = name
            .to_str()
            .and_then(bindings::change_ledger_name)
            .and_then(|key| ChangeKey::parse(&key))
        else {
            continue;
        };
        if changes.len() >= checked::MAX_CHANGE_LEDGERS {
            return Err(format!(
                "{} holds more than {} change ledgers",
                dir.display(),
                checked::MAX_CHANGE_LEDGERS
            ));
        }
        changes.push(change);
    }
    changes.sort_by_key(|change| change.as_key());
    let wanted: Vec<String> = labels
        .iter()
        .map(|label| label.trim().to_ascii_lowercase())
        .filter(|label| !label.is_empty())
        .collect();
    let mut budget = ReadBudget::new(checked::MAX_DISCOVERY_BYTES);
    let mut found = Vec::new();
    for change in changes {
        let file = store::ledger_file(root, change);
        // A ledger removed since the listing holds no review.
        let Some(body) = budget.read(&file).map_err(Unread::why)? else {
            continue;
        };
        let (events, torn) = parse_ledger(&body);
        if let Some(label) = wanted.iter().find(|label| {
            torn.iter()
                .any(|line| line.to_ascii_lowercase().contains(label.as_str()))
        }) {
            return Err(format!(
                "{} holds a record that cannot be parsed and may review {}",
                file.display(),
                label.to_ascii_uppercase()
            ));
        }
        let reviews_spec = events.iter().any(|e| {
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
/// pass ([`labelled_changes`]). So are they when a lost binding's strict
/// full read refuses: that read returned no inventory, so the spec's changes
/// are not known from it. A pass that cannot cover the ledger is a
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
        // At most one label pass per inventory: it covers every label.
        let scan_by_label = |out: &mut ReworkBindings| match labelled_changes(&root, labels) {
            Ok(found) => found.into_iter().for_each(|change| out.note(change, false)),
            Err(why) => out.refusal = Some(why),
        };
        let uncertified = !bindings_certified(&root);
        if uncertified {
            scan_by_label(&mut out);
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
                    Err(e) => {
                        out.fail(e.0);
                        // The refused read enumerated nothing, so the
                        // change whose binding was lost is still unknown:
                        // find it by label, or refuse when that cannot
                        // cover the ledger either.
                        if !uncertified {
                            scan_by_label(&mut out);
                        }
                    }
                }
            }
        }
        // An incomplete inventory already fails the read; the index still
        // binds what it names, and no further ledger is read.
        for (change, proved) in out.inventory.clone() {
            step();
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
