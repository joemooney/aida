//! `aida history <SPEC> --timeline` — one spec's elapsed time split into
//! explicit work / wait / unknown spans.
//!
//! Three read-only sources are merged, in this order of authority over a
//! boundary:
//!
//! 1. the store's own git log for that spec's object (filing + every status
//!    transition, at full `%aI` precision) — it owns the lifecycle envelope;
//! 2. the local drain feed (`.aida/events.jsonl` and its rotated archive) —
//!    it owns phase and park boundaries;
//! 3. forge PR/check timestamps, when a forge CLI is on PATH — detail inside
//!    a CI interval, never a lifecycle boundary.
//!
//! The defining rule: **nothing is interpolated.** Every instant inside the
//! reported envelope is covered by exactly one span, and any instant no
//! source explains becomes a visible `unknown` span rather than being folded
//! into whichever labelled span happens to sit next to it. `work + wait +
//! unknown == elapsed` holds exactly, so the share of the window that is
//! merely unaccounted for is impossible to hide.
// trace:STORY-1478 | ai:claude

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::events::{Event as FeedEvent, EventKind};

/// Frozen wire version of the span model. The aggregate cycle-time views
/// consume these rows, so any shape change bumps this.
// trace:STORY-1478 | ai:claude
// The aggregate breakdown siblings are STORY-1479 and STORY-1480.
pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const VIEW_NAME: &str = "history-timeline";

/// `aida history`'s own `--limit` default. `--timeline` ignores windowing
/// flags outright (a partial window would silently produce partial totals
/// that still read as complete), so a caller who moved `--limit` off this
/// value is refused rather than quietly served.
// trace:STORY-1478 | ai:claude
pub(crate) const HISTORY_DEFAULT_LIMIT: usize = 20;

/// Drain phase slugs that count as somebody/something actively working.
/// A slug outside this list produces no work span — it produces a note.
// trace:STORY-1478 | ai:claude
const WORK_PHASES: &[&str] = &["implementer", "ci", "reviewer", "merge", "pull", "build"];

/// Precedence over a disputed instant. Higher wins; an equal-precedence
/// disagreement is resolved to `unknown`, never by source order.
const PREC_STORE_STATE: u8 = 1;
const PREC_DERIVED: u8 = 2;
const PREC_PHASE: u8 = 3;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpanClass {
    Work,
    Wait,
    Unknown,
}

impl SpanClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SpanClass::Work => "work",
            SpanClass::Wait => "wait",
            SpanClass::Unknown => "unknown",
        }
    }
}

/// Which source supplied the span's decisive boundaries. `None` is reserved
/// for `unknown` spans: saying `store` there would imply the store explained
/// the interval when it only bounded the envelope around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SpanSource {
    None,
    Store,
    Events,
    Github,
    Mixed,
}

impl SpanSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SpanSource::None => "none",
            SpanSource::Store => "store",
            SpanSource::Events => "events",
            SpanSource::Github => "github",
            SpanSource::Mixed => "mixed",
        }
    }

    fn combine(self, other: SpanSource) -> SpanSource {
        match (self, other) {
            (a, b) if a == b => a,
            (SpanSource::None, b) => b,
            (a, SpanSource::None) => a,
            _ => SpanSource::Mixed,
        }
    }
}

/// How the reported window ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointKind {
    /// A store transition into Completed, with no later reopening.
    Completed,
    /// The last boundary any source observed — NOT wall-clock now.
    LastObserved,
}

impl EndpointKind {
    fn as_str(self) -> &'static str {
        match self {
            EndpointKind::Completed => "completed",
            EndpointKind::LastObserved => "last-observed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    /// Display activity, including the `#N` suffix on a repeated phase.
    pub(crate) activity: String,
    pub(crate) class: SpanClass,
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
    pub(crate) source: SpanSource,
    /// `rework` / `re-review` / `repeat` on a second-or-later occurrence.
    pub(crate) repeat: Option<&'static str>,
    /// Which piece of evidence this span came from. Two abutting spans join
    /// only when they share it, so a retry that re-enters the same phase
    /// immediately stays two spans (and so reads as `ci #1` / `ci #2`)
    /// instead of silently becoming one long one.
    // trace:STORY-1478 | ai:claude
    merge_key: Option<String>,
    /// What bounded it: store SHAs, event file:line, forge identifiers.
    /// Empty on an `unknown` span — by construction nothing supports it.
    pub(crate) evidence: Vec<String>,
}

impl Span {
    pub(crate) fn duration_s(&self) -> f64 {
        duration_ns(self.start, self.end).max(0) as f64 / 1_000_000_000.0
    }
}

fn duration_ns(start: DateTime<Utc>, end: DateTime<Utc>) -> i128 {
    let d = end - start;
    d.num_seconds() as i128 * 1_000_000_000 + d.subsec_nanos() as i128
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Totals {
    pub(crate) work: f64,
    pub(crate) wait: f64,
    pub(crate) unknown: f64,
    pub(crate) elapsed: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct Timeline {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) current_status: Option<String>,
    pub(crate) observed_start: DateTime<Utc>,
    pub(crate) observed_end: DateTime<Utc>,
    pub(crate) endpoint_kind: EndpointKind,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) spans: Vec<Span>,
    /// Bounded work that happened after the envelope closed (post-merge
    /// `pull`/`build`). Never folded into the primary totals: doing so would
    /// make `elapsed` shorter than the spans it contains.
    pub(crate) post_completion_spans: Vec<Span>,
    pub(crate) post_completion_s: f64,
    pub(crate) totals: Totals,
    pub(crate) incomplete: bool,
    pub(crate) coverage_notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreMarkerKind {
    /// The spec's object was created, at this status.
    Filed {
        status: String,
    },
    Status {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreMarker {
    pub(crate) at: DateTime<Utc>,
    pub(crate) sha: String,
    pub(crate) kind: StoreMarkerKind,
}

impl StoreMarker {
    fn status_after(&self) -> &str {
        match &self.kind {
            StoreMarkerKind::Filed { status } => status,
            StoreMarkerKind::Status { to, .. } => to,
        }
    }
}

/// One drain-feed line, kept with where it was read from so a span can cite
/// it precisely rather than gesturing at the file.
#[derive(Debug, Clone)]
pub(crate) struct DrainRecord {
    pub(crate) ev: FeedEvent,
    pub(crate) origin: String,
}

/// A forge check run with BOTH endpoints verified. A verdict without times
/// is not admitted: it cannot bound anything.
#[derive(Debug, Clone)]
pub(crate) struct ForgeCheck {
    pub(crate) pr: u32,
    pub(crate) name: String,
    pub(crate) started: DateTime<Utc>,
    pub(crate) completed: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineInput {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) current_status: Option<String>,
    pub(crate) store: Vec<StoreMarker>,
    pub(crate) events: Vec<DrainRecord>,
    pub(crate) checks: Vec<ForgeCheck>,
    pub(crate) notes: Vec<String>,
    pub(crate) as_of: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct Candidate {
    label: String,
    class: SpanClass,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    source: SpanSource,
    precedence: u8,
    evidence: Vec<String>,
    /// Identity for joining abutting pieces (see [`Span::merge_key`]). A
    /// distinct value per drain phase entry keeps separate attempts separate;
    /// one shared value across forge checks lets parallel jobs of the same CI
    /// run read as a single interval rather than as repeated attempts.
    merge_key: String,
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

fn norm_status(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

// Shared with the aggregate cycle-time view (STORY-1479), which must agree
// with this module about what "Completed" is.
pub(crate) fn is_status(s: &str, want: &str) -> bool {
    norm_status(s) == norm_status(want)
}

// trace:STORY-1478 | ai:codex
fn transitions_form_chain(transitions: &[(&str, &str, &str)]) -> bool {
    if transitions.is_empty() {
        return false;
    }
    let mut degree: BTreeMap<String, i32> = BTreeMap::new();
    let mut adjacency: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (from, to, _) in transitions {
        let from = norm_status(from);
        let to = norm_status(to);
        *degree.entry(from.clone()).or_default() -= 1;
        *degree.entry(to.clone()).or_default() += 1;
        adjacency.entry(from.clone()).or_default().push(to.clone());
        adjacency.entry(to).or_default().push(from);
    }
    let starts = degree.values().filter(|&&d| d == -1).count();
    let ends = degree.values().filter(|&&d| d == 1).count();
    if !((starts == 1 && ends == 1)
        || (starts == 0 && ends == 0 && degree.values().all(|&d| d == 0)))
        || degree.values().any(|&d| d.abs() > 1)
    {
        return false;
    }
    let Some(root) = degree.keys().next() else {
        return false;
    };
    let mut seen = BTreeSet::from([root.clone()]);
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        for neighbor in &adjacency[&node] {
            if seen.insert(neighbor.clone()) {
                pending.push(neighbor.clone());
            }
        }
    }
    seen.len() == degree.len()
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Build the timeline from already-collected evidence. Pure: no git, no
/// filesystem, no network, no clock — every test drives this directly.
// trace:STORY-1478 | ai:claude
pub(crate) fn build_timeline(input: TimelineInput) -> Result<Timeline> {
    let mut notes = input.notes.clone();

    let mut store = input.store.clone();
    store.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.sha.cmp(&b.sha)));
    let mut contradictory: BTreeSet<String> = BTreeSet::new();
    let transitions: Vec<(&str, &str, &str)> = store
        .iter()
        .filter_map(|marker| match &marker.kind {
            StoreMarkerKind::Status { from, to } => {
                Some((from.as_str(), to.as_str(), marker.sha.as_str()))
            }
            _ => None,
        })
        .collect();
    let ordered_consistently = transitions
        .windows(2)
        .all(|pair| is_status(pair[0].1, pair[1].0));
    if !ordered_consistently {
        if transitions_form_chain(&transitions) {
            contradictory.extend(transitions.iter().map(|(_, _, sha)| (*sha).to_string()));
        }
    }
    if !contradictory.is_empty() {
        notes.push(format!("{} store transition(s) contradict the preceding state after timestamp ordering; affected lifecycle intervals are unknown", contradictory.len()));
    }

    let mut events = input.events.clone();
    events.sort_by(|a, b| a.ev.ts.cmp(&b.ev.ts).then_with(|| a.origin.cmp(&b.origin)));

    let filed = store
        .iter()
        .find(|m| matches!(m.kind, StoreMarkerKind::Filed { .. }))
        .cloned();
    if filed.is_none() {
        notes.push(
            "no creation record for this spec is reachable in the store history \
             (compacted, imported, or rewritten); the window below starts at the \
             earliest boundary that IS observed, so it is not a filing-to-completion measure"
                .to_string(),
        );
    }

    // --- envelope -------------------------------------------------------
    let earliest_store = store.first().map(|m| m.at);
    let earliest_event = events.first().map(|r| r.ev.ts);
    let observed_start = match (filed.as_ref().map(|m| m.at), earliest_store, earliest_event) {
        (Some(t), _, _) => t,
        (None, Some(t), _) => t,
        (None, None, Some(t)) => t,
        (None, None, None) => anyhow::bail!(
            "no store history or drain events recorded for {} — nothing to build a timeline from",
            input.id
        ),
    };

    // The last transition INTO Completed owns the end, unless the spec was
    // reopened after it: then the run is still live and the honest end is
    // the last boundary observed, flagged incomplete.
    let last_completed_idx = store
        .iter()
        .rposition(|m| is_status(m.status_after(), "Completed"));
    let (observed_end, endpoint_kind) = match last_completed_idx {
        Some(i) if i + 1 == store.len() => (store[i].at, EndpointKind::Completed),
        Some(i) => {
            notes.push(format!(
                "reopened after completing at {}; the window ends at the last observed \
                 boundary instead and the totals are partial",
                rfc3339(store[i].at)
            ));
            let last = store.last().map(|m| m.at).unwrap_or(store[i].at);
            let last = events.last().map(|r| r.ev.ts.max(last)).unwrap_or(last);
            (last, EndpointKind::LastObserved)
        }
        None => {
            let last = store.last().map(|m| m.at).unwrap_or(observed_start);
            let last = events.last().map(|r| r.ev.ts.max(last)).unwrap_or(last);
            (last, EndpointKind::LastObserved)
        }
    };

    let mut incomplete = filed.is_none()
        || endpoint_kind == EndpointKind::LastObserved
        || notes
            .iter()
            .any(|n| n.contains("store git-log record(s) could not be parsed"));
    incomplete |= !contradictory.is_empty();
    if endpoint_kind == EndpointKind::LastObserved {
        notes.push(format!(
            "not completed in the store, so the window ends at the last boundary any \
             source recorded ({}) — not at the current time",
            rfc3339(observed_end)
        ));
    }

    if observed_end <= observed_start {
        notes.push(
            "every observed boundary falls at the same instant, so there is no measurable \
             window; no spans are reported rather than manufacturing one"
                .to_string(),
        );
        return Ok(Timeline {
            id: input.id,
            title: input.title,
            current_status: input.current_status,
            observed_start,
            observed_end: observed_start,
            endpoint_kind,
            as_of: input.as_of,
            spans: Vec::new(),
            post_completion_spans: Vec::new(),
            post_completion_s: 0.0,
            totals: Totals {
                work: 0.0,
                wait: 0.0,
                unknown: 0.0,
                elapsed: 0.0,
            },
            incomplete: true,
            coverage_notes: notes,
        });
    }

    // --- candidates -----------------------------------------------------
    // Evidenced activity first: the store's coarse states are then fitted
    // around it, never over it. In particular `queued` has to stop at the
    // first work anyone can show, even while the stored status is still
    // Approved.
    let mut cands: Vec<Candidate> = Vec::new();
    cands.extend(phase_candidates(&events, &mut notes));
    cands.extend(shelve_candidates(&events, &store, &mut notes));
    cands.extend(awaiting_merge_candidates(&events, &mut notes));
    cands.extend(check_candidates(&input.checks, &mut notes));
    let first_work = cands
        .iter()
        .filter(|c| c.class == SpanClass::Work)
        .map(|c| c.start)
        .min();
    cands.extend(store_wait_candidates(
        &store,
        filed.as_ref(),
        observed_end,
        first_work,
        &contradictory,
        &mut notes,
    ));

    // --- split at the envelope end, then partition ----------------------
    let mut post: Vec<Candidate> = Vec::new();
    let mut main: Vec<Candidate> = Vec::new();
    for c in cands {
        if c.end <= c.start {
            continue;
        }
        if c.start >= observed_end {
            post.push(c);
            continue;
        }
        if c.end > observed_end {
            let mut tail = c.clone();
            tail.start = observed_end;
            post.push(tail);
            let mut head = c;
            head.end = observed_end;
            main.push(head);
            continue;
        }
        main.push(c);
    }
    for c in main.iter_mut() {
        if c.start < observed_start {
            c.start = observed_start;
        }
    }
    main.retain(|c| c.end > c.start);

    let mut spans = partition(&main, observed_start, observed_end, &mut notes);
    number_repeats(&mut spans);

    let mut post_spans = partition_known_only(&post);
    number_repeats(&mut post_spans);
    let post_completion_s = union_seconds(&post_spans);

    let elapsed_ns = duration_ns(observed_start, observed_end);
    let work_ns = class_total_ns(&spans, SpanClass::Work);
    let wait_ns = class_total_ns(&spans, SpanClass::Wait);
    let unknown_ns = class_total_ns(&spans, SpanClass::Unknown);
    let totals = Totals {
        work: work_ns as f64 / 1_000_000_000.0,
        wait: wait_ns as f64 / 1_000_000_000.0,
        unknown: unknown_ns as f64 / 1_000_000_000.0,
        elapsed: elapsed_ns as f64 / 1_000_000_000.0,
    };
    // Conservation is structural (the partition covers the envelope exactly
    // once), but a drift here would mean a silently wrong breakdown, so it
    // is reported rather than trusted.
    let sum = totals.work + totals.wait + totals.unknown;
    if work_ns + wait_ns + unknown_ns != elapsed_ns {
        notes.push(format!(
            "internal accounting drift: spans total {:.3}s against a {:.3}s window; \
             treat this breakdown as unreliable",
            sum, totals.elapsed
        ));
        incomplete = true;
    }

    if totals.unknown > 0.0 {
        let share = (totals.unknown / totals.elapsed.max(1e-9)) * 100.0;
        notes.push(format!(
            "{:.0}% of the window ({}) is unaccounted for: no source recorded what happened. \
             It is shown as its own unknown span(s) and is NOT distributed into the spans \
             around it",
            share,
            fmt_dur(totals.unknown)
        ));
    }

    Ok(Timeline {
        id: input.id,
        title: input.title,
        current_status: input.current_status,
        observed_start,
        observed_end,
        endpoint_kind,
        as_of: input.as_of,
        spans,
        post_completion_spans: post_spans,
        post_completion_s,
        totals,
        incomplete,
        coverage_notes: notes,
    })
}

/// Contiguous status intervals from the store markers, as
/// `(status, start, end, sha)`.
fn status_intervals(
    store: &[StoreMarker],
    envelope_end: DateTime<Utc>,
) -> Vec<(String, DateTime<Utc>, DateTime<Utc>, String)> {
    let mut out = Vec::new();
    for (i, m) in store.iter().enumerate() {
        let start = m.at;
        let end = store
            .get(i + 1)
            .map(|n| n.at)
            .unwrap_or(envelope_end)
            .max(start);
        out.push((m.status_after().to_string(), start, end, m.sha.clone()));
    }
    out
}

/// The only three waits the store alone can support, each needing both of
/// its endpoints to be a real recorded state — never a single transition
/// plus an assumption about what came before or after it.
// trace:STORY-1478 | ai:claude
fn store_wait_candidates(
    store: &[StoreMarker],
    filed: Option<&StoreMarker>,
    envelope_end: DateTime<Utc>,
    first_work: Option<DateTime<Utc>>,
    contradictory: &BTreeSet<String>,
    notes: &mut Vec<String>,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let filed_draft = filed
        .map(|m| is_status(m.status_after(), "Draft"))
        .unwrap_or(false);
    if let Some(m) = filed {
        if !filed_draft {
            notes.push(format!(
                "filed straight at {} rather than Draft, so no approval wait is claimed",
                m.status_after()
            ));
        }
    }
    for (status, start, mut end, sha) in status_intervals(store, envelope_end) {
        if contradictory.contains(&sha) {
            continue;
        }
        // A Draft interval is a recorded state, so it counts wherever it
        // appears; only the "it must have started as a Draft" assumption is
        // refused, above.
        let label = if is_status(&status, "Draft") {
            "awaiting approval"
        } else if is_status(&status, "Approved") {
            // Still sitting at Approved after work demonstrably began is not
            // queueing; it is a stale status. Cut the wait at the work and
            // let the remainder fall through to unknown.
            if let Some(w) = first_work {
                if w <= start {
                    continue;
                }
                end = end.min(w);
            }
            "queued"
        } else if is_status(&status, "Needs Attention") {
            "parked"
        } else {
            // In Progress / Planned / Done / Completed say nothing about
            // whether anybody was working or waiting. Deliberately no
            // candidate: the gap surfaces as unknown.
            continue;
        };
        out.push(Candidate {
            label: label.to_string(),
            class: SpanClass::Wait,
            start,
            end,
            source: SpanSource::Store,
            precedence: PREC_STORE_STATE,
            evidence: vec![format!("store:{}", short_sha(&sha))],
            merge_key: format!("store/{}/{}", label, rfc3339(start)),
        });
    }
    out
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// Work spans from drain phase entries. A phase is closed only by evidence
/// keyed to the SAME run (and, for CI/merge, the matching terminal kind); a
/// phase with no such closer yields no span at all — its start becomes a
/// note and the time after it stays unknown.
// trace:STORY-1478 | ai:claude
fn phase_candidates(events: &[DrainRecord], notes: &mut Vec<String>) -> Vec<Candidate> {
    let recs = events;
    let mut out = Vec::new();
    for (i, rec) in recs.iter().enumerate() {
        let EventKind::PhaseEntered { slug, attempt, .. } = &rec.ev.kind else {
            continue;
        };
        let slug_l = slug.to_ascii_lowercase();
        if !WORK_PHASES.contains(&slug_l.as_str()) {
            notes.push(format!(
                "phase `{}` entered at {} is not a recognised work phase, so no work span \
                 is claimed for it",
                slug,
                rfc3339(rec.ev.ts)
            ));
            continue;
        }
        let run = rec.ev.run_uuid.clone();
        let mut end: Option<(DateTime<Utc>, String)> = None;
        for later in recs.iter().skip(i + 1) {
            if later.ev.ts <= rec.ev.ts {
                continue;
            }
            let same_run = later.ev.run_uuid == run;
            let closes = match &later.ev.kind {
                // Any next phase entry in the same run ends this one.
                EventKind::PhaseEntered { .. } => same_run,
                EventKind::CiTerminal { .. } => same_run && slug_l == "ci",
                EventKind::PrMerged { .. } => same_run && slug_l == "merge",
                // A park ends work whoever recorded it; the shelve path does
                // not always carry the run UUID.
                EventKind::SpecShelved { .. } => same_run || later.ev.run_uuid.is_empty(),
                EventKind::RunCompleted {
                    pull_completed,
                    build_completed,
                } => {
                    same_run
                        && match slug_l.as_str() {
                            "pull" => *pull_completed,
                            "build" => *build_completed,
                            _ => false,
                        }
                }
                _ => false,
            };
            if closes {
                end = Some((later.ev.ts, later.origin.clone()));
                break;
            }
        }
        // A store completion cannot close a phase either: `pull` and `build`
        // routinely run past it.
        match end {
            Some((at, origin)) => out.push(Candidate {
                label: slug_l.clone(),
                class: SpanClass::Work,
                start: rec.ev.ts,
                end: at,
                source: SpanSource::Events,
                precedence: PREC_PHASE,
                evidence: vec![
                    format!("events:{}", rec.origin),
                    format!("events:{}", origin),
                ],
                // One key per phase ENTRY, so a retry re-entering the same
                // phase back-to-back stays a separate span.
                merge_key: format!("phase/{}/{}/{}/{}", run, slug_l, attempt, rec.origin),
            }),
            None => notes.push(format!(
                "phase `{}` (attempt {}) started at {} has no recorded end, so its duration \
                 is unknown rather than run to the end of the window",
                slug,
                attempt,
                rfc3339(rec.ev.ts)
            )),
        }
    }
    out
}

/// Park spans from a shelve. The store owns when the spec stopped being
/// parked, so a shelve interval is cut at the first store transition to a
/// status other than Needs Attention — a later drain restart does not get
/// to claim the gap between them.
// trace:STORY-1478 | ai:claude
fn shelve_candidates(
    events: &[DrainRecord],
    store: &[StoreMarker],
    notes: &mut Vec<String>,
) -> Vec<Candidate> {
    let recs = events;
    let mut out = Vec::new();
    for (i, rec) in recs.iter().enumerate() {
        if !matches!(rec.ev.kind, EventKind::SpecShelved { .. }) {
            continue;
        }
        let mut end: Option<(DateTime<Utc>, String)> = None;
        for later in recs.iter().skip(i + 1) {
            if later.ev.ts <= rec.ev.ts {
                continue;
            }
            let resumes = matches!(
                later.ev.kind,
                EventKind::SpecRequeued { .. }
                    | EventKind::SpecReDriven { .. }
                    | EventKind::RunStarted
                    | EventKind::PhaseEntered { .. }
            );
            if resumes {
                end = Some((later.ev.ts, format!("events:{}", later.origin)));
                break;
            }
        }
        let store_exit = store
            .iter()
            .find(|m| m.at > rec.ev.ts && !is_status(m.status_after(), "Needs Attention"))
            .map(|m| (m.at, format!("store:{}", short_sha(&m.sha))));
        let chosen = match (end, store_exit) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        match chosen {
            Some((at, ev)) => out.push(Candidate {
                label: "parked".to_string(),
                class: SpanClass::Wait,
                start: rec.ev.ts,
                end: at,
                source: SpanSource::Events,
                precedence: PREC_PHASE,
                evidence: vec![format!("events:{}", rec.origin), ev],
                merge_key: format!("park/{}", rec.origin),
            }),
            None => notes.push(format!(
                "parked at {} with nothing recording a resume, so the park's length is unknown",
                rfc3339(rec.ev.ts)
            )),
        }
    }
    out
}

/// `awaiting merge` needs BOTH a still-valid work-done boundary and a
/// credited merge, with no rework, retry, re-drive or shelve between them.
/// Reaching `Done` proves nothing on its own — CI and review often run past
/// it — so `Done` is not used here.
// trace:STORY-1478 | ai:claude
fn awaiting_merge_candidates(events: &[DrainRecord], notes: &mut Vec<String>) -> Vec<Candidate> {
    let recs = events;
    let Some(merge) = recs
        .iter()
        .rev()
        .find(|r| matches!(r.ev.kind, EventKind::PrMerged { .. }))
    else {
        return Vec::new();
    };
    let done = recs
        .iter()
        .filter(|r| r.ev.ts < merge.ev.ts)
        .filter(|r| {
            matches!(
                &r.ev.kind,
                // A green CI run is not a work-done record: review, and
                // often more implementation, still follow it.
                EventKind::PhaseDonePr { .. } | EventKind::ReviewVerdictRecorded { .. }
            )
        })
        .next_back();
    let Some(done) = done else {
        return Vec::new();
    };
    let invalidated = recs.iter().any(|r| {
        r.ev.ts > done.ev.ts
            && r.ev.ts < merge.ev.ts
            && matches!(
                r.ev.kind,
                EventKind::SpecShelved { .. }
                    | EventKind::SpecRetried { .. }
                    | EventKind::SpecReDriven { .. }
                    | EventKind::SpecRequeued { .. }
            )
    });
    if invalidated {
        notes.push(
            "work was reworked between the last work-done record and the merge, so the gap \
             before the merge is not reported as an await-merge wait"
                .to_string(),
        );
        return Vec::new();
    }
    vec![Candidate {
        label: "awaiting merge".to_string(),
        class: SpanClass::Wait,
        start: done.ev.ts,
        end: merge.ev.ts,
        source: SpanSource::Events,
        precedence: PREC_DERIVED,
        evidence: vec![
            format!("events:{}", done.origin),
            format!("events:{}", merge.origin),
        ],
        merge_key: format!("await-merge/{}", merge.origin),
    }]
}

/// Forge check runs become CI detail. Parallel jobs are unioned by the
/// partition (never summed), and a reversed or zero-length interval is
/// dropped with a note instead of being repaired.
// trace:STORY-1478 | ai:claude
fn check_candidates(checks: &[ForgeCheck], notes: &mut Vec<String>) -> Vec<Candidate> {
    let mut out = Vec::new();
    for c in checks {
        if c.completed <= c.started {
            notes.push(format!(
                "forge check `{}` on PR #{} reports an end at or before its start, so it is \
                 discarded rather than corrected",
                c.name, c.pr
            ));
            continue;
        }
        out.push(Candidate {
            label: "ci".to_string(),
            class: SpanClass::Work,
            start: c.started,
            end: c.completed,
            source: SpanSource::Github,
            precedence: PREC_DERIVED,
            evidence: vec![format!("github:pr/{}/check/{}", c.pr, c.name)],
            // Shared across a PR's checks: parallel jobs of one CI run are
            // one interval, not repeated attempts.
            merge_key: format!("forge-ci/{}", c.pr),
        });
    }
    out
}

/// Turn overlapping candidates into one non-overlapping, contiguous
/// partition of `[start, end)`. Every elementary interval gets exactly one
/// label: the highest-precedence candidate covering it, `unknown` when none
/// does, and `unknown` again when two equally-authoritative candidates
/// disagree — the one place where picking a winner would be a guess.
// trace:STORY-1478 | ai:claude
fn partition(
    cands: &[Candidate],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    notes: &mut Vec<String>,
) -> Vec<Span> {
    let mut points: BTreeSet<DateTime<Utc>> = BTreeSet::new();
    points.insert(start);
    points.insert(end);
    for c in cands {
        if c.start > start && c.start < end {
            points.insert(c.start);
        }
        if c.end > start && c.end < end {
            points.insert(c.end);
        }
    }
    let pts: Vec<DateTime<Utc>> = points.into_iter().collect();
    let mut raw: Vec<Span> = Vec::new();
    let mut disputed = 0usize;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let covering: Vec<&Candidate> = cands
            .iter()
            .filter(|c| c.start <= a && c.end >= b)
            .collect();
        let top_prec = covering.iter().map(|c| c.precedence).max();
        let top: Vec<&&Candidate> = match top_prec {
            Some(p) => covering.iter().filter(|c| c.precedence == p).collect(),
            None => Vec::new(),
        };
        let distinct: BTreeSet<(&str, &str)> = top
            .iter()
            .map(|c| (c.label.as_str(), c.class.as_str()))
            .collect();
        if top.is_empty() {
            raw.push(Span {
                activity: "unexplained".into(),
                class: SpanClass::Unknown,
                start: a,
                end: b,
                source: SpanSource::None,
                repeat: None,
                merge_key: None,
                evidence: Vec::new(),
            });
        } else if distinct.len() > 1 {
            disputed += 1;
            raw.push(Span {
                activity: "disputed".into(),
                class: SpanClass::Unknown,
                start: a,
                end: b,
                source: SpanSource::None,
                repeat: None,
                merge_key: None,
                evidence: Vec::new(),
            });
        } else {
            let first = top[0];
            let mut source = SpanSource::None;
            let mut evidence: Vec<String> = Vec::new();
            for c in &top {
                source = source.combine(c.source);
                evidence.extend(c.evidence.iter().cloned());
            }
            evidence.sort();
            evidence.dedup();
            raw.push(Span {
                activity: first.label.clone(),
                class: first.class,
                start: a,
                end: b,
                source,
                repeat: None,
                merge_key: Some(first.merge_key.clone()),
                evidence,
            });
        }
    }
    if disputed > 0 {
        notes.push(format!(
            "{} interval(s) had two sources claiming different activities with equal \
             authority; they are reported as unknown rather than resolved by guessing",
            disputed
        ));
    }
    merge_adjacent(raw)
}

/// The same partition for the post-completion pool, minus the unknown fill:
/// there is no envelope after the end, so an uncovered instant there is
/// simply not part of the timeline.
fn partition_known_only(cands: &[Candidate]) -> Vec<Span> {
    if cands.is_empty() {
        return Vec::new();
    }
    let start = cands.iter().map(|c| c.start).min().unwrap();
    let end = cands.iter().map(|c| c.end).max().unwrap();
    let mut sink = Vec::new();
    let spans = partition(cands, start, end, &mut sink);
    spans
        .into_iter()
        .filter(|s| s.class != SpanClass::Unknown)
        .collect()
}

fn merge_adjacent(raw: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for s in raw {
        match out.last_mut() {
            Some(prev)
                if prev.end == s.start
                    && prev.activity == s.activity
                    && prev.class == s.class
                    && prev.merge_key == s.merge_key =>
            {
                prev.end = s.end;
                prev.source = prev.source.combine(s.source);
                prev.evidence.extend(s.evidence);
                prev.evidence.sort();
                prev.evidence.dedup();
            }
            _ => out.push(s),
        }
    }
    out
}

/// Number the second-and-later occurrence of the same activity so a rework
/// loop reads as `implementer #2` / `reviewer #2` rather than as two
/// indistinguishable rows. Unknown spans are never numbered — they are not
/// a repeat of anything.
// trace:STORY-1478 | ai:claude
fn number_repeats(spans: &mut [Span]) {
    let mut totals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in spans.iter() {
        if s.class == SpanClass::Unknown {
            continue;
        }
        *totals.entry(s.activity.clone()).or_insert(0) += 1;
    }
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in spans.iter_mut() {
        if s.class == SpanClass::Unknown {
            continue;
        }
        let total = totals.get(&s.activity).copied().unwrap_or(1);
        if total < 2 {
            continue;
        }
        let n = seen.entry(s.activity.clone()).or_insert(0);
        *n += 1;
        let nth = *n;
        if nth > 1 {
            s.repeat = Some(match s.activity.as_str() {
                "implementer" => "rework",
                "reviewer" => "re-review",
                _ => "repeat",
            });
        }
        s.activity = format!("{} #{}", s.activity, nth);
    }
}

fn class_total_ns(spans: &[Span], class: SpanClass) -> i128 {
    spans
        .iter()
        .filter(|s| s.class == class)
        .map(|s| duration_ns(s.start, s.end).max(0))
        .sum()
}

/// Union (not sum) of a span list's wall-clock coverage.
fn union_seconds(spans: &[Span]) -> f64 {
    let mut iv: Vec<(DateTime<Utc>, DateTime<Utc>)> =
        spans.iter().map(|s| (s.start, s.end)).collect();
    iv.sort();
    let mut total = 0f64;
    let mut cur: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for (a, b) in iv {
        match cur {
            Some((cs, ce)) if a <= ce => cur = Some((cs, ce.max(b))),
            Some((cs, ce)) => {
                total += (ce - cs).num_milliseconds() as f64 / 1000.0;
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += (ce - cs).num_milliseconds() as f64 / 1000.0;
    }
    total
}

/// Compact duration: `5h47m57s`, `3m42s`, `12s`.
pub(crate) fn fmt_dur(secs: f64) -> String {
    let total = secs.round().max(0.0) as i64;
    let d = total / 86_400;
    let h = (total % 86_400) / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    let mut out = String::new();
    if d > 0 {
        out.push_str(&format!("{}d", d));
    }
    if d > 0 || h > 0 {
        out.push_str(&format!("{}h", h));
    }
    if d > 0 || h > 0 || m > 0 {
        out.push_str(&format!("{}m", m));
    }
    out.push_str(&format!("{}s", s));
    out
}

fn pct(part: f64, whole: f64) -> String {
    if whole <= 0.0 {
        return "—".into();
    }
    format!("{:.0}%", part / whole * 100.0)
}

// ---------------------------------------------------------------------------
// Rendering — one model, three surfaces, identical classification and totals
// ---------------------------------------------------------------------------

/// The ordered row fields every surface shows. Frozen with `SCHEMA_VERSION`.
const ROW_FIELDS: &[&str] = &[
    "activity",
    "class",
    "start",
    "end",
    "duration_s",
    "source",
    "repeat",
];

fn row_cells(s: &Span) -> Vec<String> {
    vec![
        s.activity.clone(),
        s.class.as_str().to_string(),
        rfc3339(s.start),
        rfc3339(s.end),
        format!("{:.3}", s.duration_s()),
        s.source.as_str().to_string(),
        s.repeat.unwrap_or("").to_string(),
    ]
}

// trace:STORY-1478 | ai:claude
pub(crate) fn render_human(t: &Timeline) -> String {
    use colored::Colorize;
    let mut out = String::new();
    let title = if t.title.is_empty() {
        String::new()
    } else {
        format!(" — {}", t.title)
    };
    let status = match &t.current_status {
        Some(s) => format!("  [current: {}]", s.green()),
        None => "  [not currently in the store]".dimmed().to_string(),
    };
    out.push_str(&format!("{}{}{}\n", t.id.bold(), title, status));
    out.push_str(&format!(
        "{}\n",
        format!(
            "Timeline {} → {} ({}, {})",
            rfc3339(t.observed_start),
            rfc3339(t.observed_end),
            match t.endpoint_kind {
                EndpointKind::Completed => "filed → completed",
                EndpointKind::LastObserved => "ends at the last observed boundary",
            },
            if t.incomplete {
                "PARTIAL COVERAGE"
            } else {
                "complete coverage"
            }
        )
        .dimmed()
    ));
    out.push('\n');

    if t.spans.is_empty() {
        out.push_str("  (no measurable spans)\n");
    }
    for s in &t.spans {
        let label = match s.repeat {
            Some(r) => format!("{} ({})", s.activity, r),
            None => s.activity.clone(),
        };
        let class_cell = format!("{:<7}", s.class.as_str());
        let painted = match s.class {
            SpanClass::Work => class_cell.green().to_string(),
            SpanClass::Wait => class_cell.yellow().to_string(),
            SpanClass::Unknown => class_cell.red().to_string(),
        };
        out.push_str(&format!(
            "  {}  {}  {:>9}  {}  {}\n",
            rfc3339(s.start).dimmed(),
            painted,
            fmt_dur(s.duration_s()),
            label,
            format!("[{}]", s.source.as_str()).dimmed(),
        ));
    }

    out.push('\n');
    out.push_str(&format!(
        "  work     {:>9}  {}\n",
        fmt_dur(t.totals.work),
        pct(t.totals.work, t.totals.elapsed)
    ));
    out.push_str(&format!(
        "  wait     {:>9}  {}\n",
        fmt_dur(t.totals.wait),
        pct(t.totals.wait, t.totals.elapsed)
    ));
    out.push_str(&format!(
        "  {}  {:>9}  {}\n",
        "unknown".red().bold(),
        fmt_dur(t.totals.unknown),
        pct(t.totals.unknown, t.totals.elapsed)
    ));
    out.push_str(&format!("  elapsed  {:>9}\n", fmt_dur(t.totals.elapsed)));

    if !t.post_completion_spans.is_empty() {
        out.push_str(&format!(
            "\n{}\n",
            format!(
                "After the window closed (excluded from the totals above; {} in all):",
                fmt_dur(t.post_completion_s)
            )
            .dimmed()
        ));
        for s in &t.post_completion_spans {
            out.push_str(&format!(
                "  {}  {:<7}  {:>9}  {}\n",
                rfc3339(s.start).dimmed(),
                s.class.as_str(),
                fmt_dur(s.duration_s()),
                s.activity
            ));
        }
    }

    if !t.coverage_notes.is_empty() {
        out.push_str("\nCoverage:\n");
        for n in &t.coverage_notes {
            out.push_str(&format!("  - {}\n", n));
        }
    }
    out
}

// trace:STORY-1478 | ai:claude
pub(crate) fn render_toon(t: &Timeline) -> String {
    let mut out = format!("view: {}\n", VIEW_NAME);
    out.push_str(&format!("schema_version: {}\n", SCHEMA_VERSION));
    out.push_str(&format!("{}\n", crate::toon::scalar("id", &t.id)));
    out.push_str(&format!("{}\n", crate::toon::scalar("title", &t.title)));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("current", t.current_status.as_deref().unwrap_or(""))
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("observed_start", &rfc3339(t.observed_start))
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("observed_end", &rfc3339(t.observed_end))
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("endpoint_kind", t.endpoint_kind.as_str())
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("as_of", &rfc3339(t.as_of))
    ));
    out.push_str("order: oldest-first\n");
    out.push_str(&format!("incomplete: {}\n", t.incomplete));
    let rows: Vec<Vec<String>> = t.spans.iter().map(row_cells).collect();
    out.push_str(&crate::toon::table_raw("spans", ROW_FIELDS, &rows));
    out.push('\n');
    out.push_str(&format!("work_s: {:.3}\n", t.totals.work));
    out.push_str(&format!("wait_s: {:.3}\n", t.totals.wait));
    out.push_str(&format!("unknown_s: {:.3}\n", t.totals.unknown));
    out.push_str(&format!("elapsed_s: {:.3}\n", t.totals.elapsed));
    out.push_str(&format!("post_completion_s: {:.3}\n", t.post_completion_s));
    let post: Vec<Vec<String>> = t.post_completion_spans.iter().map(row_cells).collect();
    out.push_str(&crate::toon::table_raw(
        "post_completion_spans",
        ROW_FIELDS,
        &post,
    ));
    out.push('\n');
    let notes: Vec<Vec<String>> = t
        .coverage_notes
        .iter()
        .map(|n| vec![n.replace('\n', " ")])
        .collect();
    out.push_str(&crate::toon::table_raw("coverage_notes", &["note"], &notes));
    out.push('\n');
    out
}

fn span_json(s: &Span) -> JsonValue {
    json!({
        "activity": s.activity,
        "class": s.class.as_str(),
        "start": rfc3339(s.start),
        "end": rfc3339(s.end),
        "duration_s": s.duration_s(),
        "source": s.source.as_str(),
        "repeat": s.repeat,
        "evidence": s.evidence,
    })
}

// trace:STORY-1478 | ai:claude
pub(crate) fn render_json(t: &Timeline) -> JsonValue {
    json!({
        "schema_version": SCHEMA_VERSION,
        "view": VIEW_NAME,
        "id": t.id,
        "title": if t.title.is_empty() { JsonValue::Null } else { json!(t.title) },
        "current_status": t.current_status,
        "order": "oldest-first",
        "observed_start": rfc3339(t.observed_start),
        "observed_end": rfc3339(t.observed_end),
        "endpoint_kind": t.endpoint_kind.as_str(),
        "as_of": rfc3339(t.as_of),
        "incomplete": t.incomplete,
        "spans": t.spans.iter().map(span_json).collect::<Vec<_>>(),
        "totals_s": {
            "work": t.totals.work,
            "wait": t.totals.wait,
            "unknown": t.totals.unknown,
            "elapsed": t.totals.elapsed,
        },
        "post_completion_spans": t.post_completion_spans.iter().map(span_json).collect::<Vec<_>>(),
        "post_completion_s": t.post_completion_s,
        "coverage_notes": t.coverage_notes,
    })
}

// ---------------------------------------------------------------------------
// Collectors
// ---------------------------------------------------------------------------

/// Filing + every status transition for one spec, at full `%aI` precision.
///
/// This walks the spec's own object path with no commit cap, deliberately
/// separate from the display collector's `--limit`/`--max-commits` window: a
/// capped walk would hand back a partial lifecycle that still renders as a
/// whole one. The per-commit YAML decoding itself is `history`'s decoder, so
/// the two views cannot disagree about what a transition is.
// trace:STORY-1478 | ai:claude
pub(crate) fn collect_store_markers(
    store_path: &Path,
    spec_id: &str,
) -> Result<(Vec<StoreMarker>, Vec<String>)> {
    let mut notes = Vec::new();
    let Ok(rel) = aida_core::object_store::relative_object_path(spec_id) else {
        anyhow::bail!("{} is not a resolvable requirement id", spec_id);
    };
    let log = crate::history::run_git(
        store_path,
        &[
            "log".into(),
            "--pretty=format:%H%x09%aI%x09%ae".into(),
            "--full-history".into(),
            "--".into(),
            rel.clone(),
        ],
    )?;
    let commits = parse_store_log(&log, &mut notes);
    if commits.is_empty() {
        anyhow::bail!(
            "no recorded history for {} — nothing to build a timeline from",
            spec_id
        );
    }
    let mut touches: Vec<(crate::history::CommitMeta, String, String)> = Vec::new();
    // Oldest first.
    for commit in commits.iter().rev() {
        let changed = crate::history::run_git(
            store_path,
            &[
                "show".into(),
                "--name-status".into(),
                "--no-renames".into(),
                "--format=".into(),
                commit.sha.clone(),
                "--".into(),
                rel.clone(),
            ],
        )?;
        for line in changed.lines() {
            let mut parts = line.splitn(2, '\t');
            let status = parts.next().unwrap_or("").trim();
            let path = parts.next().unwrap_or("").trim();
            if path.is_empty() || !path.starts_with("objects/") || !path.ends_with(".yaml") {
                continue;
            }
            touches.push((commit.clone(), status.to_string(), path.to_string()));
        }
    }
    let mut fetch =
        |rev: &str, path: &str| crate::history::git_show_blob(store_path, rev, path).ok();
    let markers = markers_from_touches(&touches, &mut fetch, &mut notes);
    Ok((markers, notes))
}

/// Decode store markers from an already-known, oldest-first list of
/// `(commit, name-status letter, object path)` touches. Shared by the
/// per-spec collector above and the aggregate cycle-time view (STORY-1479),
/// whose single whole-branch `--name-status` walk supplies the touches for
/// many specs without a per-spec `git log`/`git show` pair — the decoding
/// itself stays this one function, so the two views cannot disagree about
/// what a filing or a transition is. `fetch_blob` is `(rev, path) → body`
/// so the aggregate view can serve blobs from one long-lived
/// `git cat-file --batch` instead of a process per touch.
// trace:STORY-1479 | ai:claude
pub(crate) fn markers_from_touches(
    touches: &[(crate::history::CommitMeta, String, String)],
    fetch_blob: &mut dyn FnMut(&str, &str) -> Option<String>,
    notes: &mut Vec<String>,
) -> Vec<StoreMarker> {
    let mut markers: Vec<StoreMarker> = Vec::new();
    let mut unparsed = 0usize;
    for (commit, status, path) in touches {
        let after = fetch_blob(&commit.sha, path);
        let before = fetch_blob(&format!("{}^", commit.sha), path);
        let Ok(at) = DateTime::parse_from_rfc3339(&commit.iso_timestamp) else {
            unparsed += 1;
            continue;
        };
        let at = at.with_timezone(&Utc);
        let mut decoded = Vec::new();
        crate::history::decode_into_events(
            commit,
            status,
            path,
            before.as_deref(),
            after.as_deref(),
            &mut decoded,
        );
        for ev in decoded {
            match ev.kind {
                crate::history::EventKind::Added { .. } => {
                    // The filed status comes from the added revision's own
                    // YAML in this same snapshot; it is never assumed to
                    // have been Draft.
                    let filed = after
                        .as_deref()
                        .and_then(yaml_status)
                        .unwrap_or_else(|| "(unrecorded)".to_string());
                    markers.push(StoreMarker {
                        at,
                        sha: commit.sha.clone(),
                        kind: StoreMarkerKind::Filed { status: filed },
                    });
                }
                crate::history::EventKind::StatusChange { from, to } => {
                    markers.push(StoreMarker {
                        at,
                        sha: commit.sha.clone(),
                        kind: StoreMarkerKind::Status { from, to },
                    });
                }
                _ => {}
            }
        }
    }
    if unparsed > 0 {
        notes.push(format!(
            "{} store commit(s) carried a timestamp this build could not read; those \
             boundaries are missing from the timeline",
            unparsed
        ));
    }
    markers
}

fn parse_store_log(log: &str, notes: &mut Vec<String>) -> Vec<crate::history::CommitMeta> {
    let mut commits = Vec::new();
    let mut malformed = 0usize;
    for line in log.lines() {
        match crate::history::parse_log_line(line) {
            Some(commit) => commits.push(commit),
            None => malformed += 1,
        }
    }
    if malformed > 0 {
        notes.push(format!(
            "{} store git-log record(s) could not be parsed; their boundaries are missing and coverage is incomplete",
            malformed
        ));
    }
    commits
}

fn yaml_status(yaml: &str) -> Option<String> {
    let v: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    for key in ["custom_status", "status"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return Some(s.trim().to_string());
            }
        }
    }
    None
}

/// Drain events for one spec from the live feed and its rotated archive.
///
/// Deliberately not `events::read_all_with_archive`: that reader drops an
/// unparseable line silently, which for a timeline means quietly losing a
/// boundary and reporting the resulting hole as though the drain had simply
/// been idle. Here a bad line is counted and reported.
// trace:STORY-1478 | ai:claude
pub(crate) fn collect_drain_events(
    project_root: &Path,
    spec_id: &str,
) -> (Vec<DrainRecord>, Vec<String>) {
    let mut out: Vec<DrainRecord> = Vec::new();
    let mut notes = Vec::new();
    let mut bad = 0usize;
    let mut any_file = false;
    for path in [
        crate::events::events_archive_path(project_root),
        crate::events::events_path(project_root),
    ] {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "events".into());
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        any_file = true;
        for (i, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<FeedEvent>(line) {
                Ok(ev) => {
                    if ev
                        .spec
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
                    {
                        out.push(DrainRecord {
                            ev,
                            origin: format!("{}:{}", name, i + 1),
                        });
                    }
                }
                Err(_) => bad += 1,
            }
        }
    }
    if !any_file {
        notes.push(
            "no local drain event feed is readable here, so nothing below is attributed to a \
             drain phase; time a drain may have spent working shows as unknown"
                .to_string(),
        );
    }
    if bad > 0 {
        notes.push(format!(
            "{} drain event line(s) could not be read; any boundary they held is missing, so \
             some spans below may be unknown that were in fact instrumented",
            bad
        ));
    }
    // Exact duplicates (the same event written twice, or present in both the
    // live feed and the archive) would otherwise double a boundary.
    let before = out.len();
    out.sort_by(|a, b| a.ev.ts.cmp(&b.ev.ts).then_with(|| a.origin.cmp(&b.origin)));
    out.dedup_by(|a, b| a.ev == b.ev);
    if out.len() < before {
        notes.push(format!(
            "{} duplicate drain event line(s) collapsed",
            before - out.len()
        ));
    }
    (out, notes)
}

/// The spec's published timing record (STORY-1480 / ADR-64) as feed-shaped
/// records, so cross-machine evidence replays through exactly the decoder
/// the local feed uses. Origins name the record so a rendered boundary is
/// attributable to "published at completion" rather than this clone's feed.
// trace:STORY-1480 | ai:claude
pub(crate) fn collect_published_events(
    store_path: &Path,
    spec_id: &str,
) -> (Vec<DrainRecord>, Vec<String>) {
    let (events, notes) = crate::timing_record::load_published(store_path, spec_id);
    let recs = events
        .into_iter()
        .enumerate()
        .map(|(i, ev)| DrainRecord {
            ev,
            origin: format!("timing-record:{}", i + 1),
        })
        .collect();
    (recs, notes)
}

/// Union the local feed with published-record events, in feed order, exact
/// duplicates collapsed (the same semantics `collect_drain_events` applies
/// across the live feed and its archive — local origins sort first, so the
/// local line wins a tie). Returns how many published events survived
/// beyond the local feed.
// trace:STORY-1480 | ai:claude
pub(crate) fn merge_records(
    mut local: Vec<DrainRecord>,
    published: Vec<DrainRecord>,
) -> (Vec<DrainRecord>, usize) {
    if published.is_empty() {
        return (local, 0);
    }
    let before = local.len();
    local.extend(published);
    local.sort_by(|a, b| a.ev.ts.cmp(&b.ev.ts).then_with(|| a.origin.cmp(&b.origin)));
    local.dedup_by(|a, b| a.ev == b.ev);
    let contributed = local.len().saturating_sub(before);
    (local, contributed)
}

/// PR numbers this spec is actually credited with, from its own drain
/// events. A PR is never matched by title or branch guesswork.
// trace:STORY-1478 | ai:claude
pub(crate) fn credited_prs(events: &[DrainRecord]) -> Vec<u32> {
    let mut prs: Vec<u32> = Vec::new();
    for r in events {
        let pr = match &r.ev.kind {
            EventKind::PrMerged { pr } => Some(*pr),
            EventKind::PhaseDonePr { pr } => Some(*pr),
            EventKind::SpecCompleted { pr: Some(pr), .. } => u32::try_from(*pr).ok(),
            _ => None,
        };
        if let Some(pr) = pr {
            if !prs.contains(&pr) {
                prs.push(pr);
            }
        }
    }
    prs
}

/// Read-only forge timestamps. Injectable so tests never touch a network or
/// a real `gh`.
// trace:STORY-1478 | ai:claude
pub(crate) trait ForgeTimeSource {
    /// Check runs with both endpoints present for one credited PR, or a
    /// one-line reason the evidence is unavailable.
    fn pr_checks(&self, pr: u32) -> std::result::Result<Vec<ForgeCheck>, String>;
}

pub(crate) struct GhTimeSource {
    project_root: std::path::PathBuf,
}

impl GhTimeSource {
    pub(crate) fn new(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
        }
    }
}

impl ForgeTimeSource for GhTimeSource {
    fn pr_checks(&self, pr: u32) -> std::result::Result<Vec<ForgeCheck>, String> {
        if !crate::forge::ForgeKind::GitHub.cli_on_path() {
            return Err("`gh` is not on PATH, so no PR or CI timestamps were read".into());
        }
        let out = bounded_output(
            &self.project_root,
            &[
                "pr",
                "view",
                &pr.to_string(),
                "--json",
                "createdAt,mergedAt,statusCheckRollup",
            ],
            std::time::Duration::from_secs(10),
        )?;
        parse_gh_checks(pr, &out)
    }
}

/// Run `gh` with a hard wall-clock bound; a hung call must not hang the
/// whole view.
fn bounded_output(
    cwd: &Path,
    args: &[&str],
    limit: std::time::Duration,
) -> std::result::Result<String, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run `gh`: {}", e))?;
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "`gh` did not answer within {}s, so PR/CI timestamps were skipped",
                        limit.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("could not wait for `gh`: {}", e)),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("could not read `gh` output: {}", e))?;
    if !out.status.success() {
        return Err("`gh` refused the request (auth, network, or unknown PR)".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse `gh pr view --json …`. Pure, so the malformed-payload cases are
/// testable without a forge.
// trace:STORY-1478 | ai:claude
pub(crate) fn parse_gh_checks(pr: u32, body: &str) -> std::result::Result<Vec<ForgeCheck>, String> {
    let v: JsonValue = serde_json::from_str(body)
        .map_err(|_| "`gh` returned something that is not JSON, so it was ignored".to_string())?;
    let rollup = v
        .get("statusCheckRollup")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for c in rollup {
        let name = c
            .get("name")
            .or_else(|| c.get("context"))
            .and_then(|x| x.as_str())
            .unwrap_or("check")
            .to_string();
        let started = c
            .get("startedAt")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let completed = c
            .get("completedAt")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        // A verdict with no usable interval cannot bound anything, so it is
        // dropped rather than given an invented endpoint.
        if let (Some(started), Some(completed)) = (started, completed) {
            out.push(ForgeCheck {
                pr,
                name,
                started,
                completed,
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

/// `aida history <SPEC> --timeline`.
// trace:STORY-1478 | ai:claude
pub(crate) fn run(
    store_path: &Path,
    spec_id: &str,
    output: crate::history::HistoryOutput,
    forge_enabled: bool,
) -> Result<()> {
    let project_root = store_path.parent().unwrap_or(store_path).to_path_buf();
    let (store, mut notes) = collect_store_markers(store_path, spec_id)?;
    let (local_events, ev_notes) = collect_drain_events(&project_root, spec_id);
    notes.extend(ev_notes);
    // STORY-1480: work done on another machine reaches this view through the
    // published timing record on the store branch, not the local feed.
    // trace:STORY-1480 | ai:claude
    let (published, pub_notes) = collect_published_events(store_path, spec_id);
    notes.extend(pub_notes);
    let (events, published_contributed) = merge_records(local_events, published);
    if published_contributed > 0 {
        notes.push(format!(
            "the store's published timing record contributed {published_contributed} event(s) \
             beyond the local feed (work recorded on another clone)"
        ));
    }

    let mut checks: Vec<ForgeCheck> = Vec::new();
    if forge_enabled {
        let prs = credited_prs(&events);
        if prs.is_empty() {
            notes.push(
                "no pull request is credited to this spec in the local event feed, so no forge \
                 timestamps were fetched"
                    .to_string(),
            );
        } else {
            let source = GhTimeSource::new(&project_root);
            let mut failures: Vec<String> = Vec::new();
            for pr in prs {
                match source.pr_checks(pr) {
                    Ok(mut c) => checks.append(&mut c),
                    Err(e) => failures.push(e),
                }
            }
            if !failures.is_empty() {
                failures.sort();
                failures.dedup();
                notes.push(format!(
                    "forge timestamps unavailable ({}); the store and event evidence below is \
                     unaffected",
                    failures.join("; ")
                ));
            }
        }
    } else {
        notes.push("forge lookup was switched off for this run".to_string());
    }

    let (current_status, title) = crate::history::current_snapshot_for(store_path, spec_id);
    let timeline = build_timeline(TimelineInput {
        id: spec_id.to_string(),
        title,
        current_status,
        store,
        events,
        checks,
        notes,
        as_of: Utc::now(),
    })?;

    match output {
        crate::history::HistoryOutput::Json => {
            println!("{}", serde_json::to_string_pretty(&render_json(&timeline))?);
        }
        crate::history::HistoryOutput::Toon => print!("{}", render_toon(&timeline)),
        // `--oneline` is refused for this view at the CLI boundary; a single
        // line cannot carry a span table without dropping the unknown rows.
        _ => print!("{}", render_human(&timeline)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — fixture evidence only. No live store, no `gh`, no network.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event as Ev;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .unwrap_or_else(|e| panic!("bad fixture timestamp {s}: {e}"))
            .with_timezone(&Utc)
    }

    fn filed(ts: &str, status: &str, sha: &str) -> StoreMarker {
        StoreMarker {
            at: at(ts),
            sha: sha.into(),
            kind: StoreMarkerKind::Filed {
                status: status.into(),
            },
        }
    }

    fn tr(ts: &str, from: &str, to: &str, sha: &str) -> StoreMarker {
        StoreMarker {
            at: at(ts),
            sha: sha.into(),
            kind: StoreMarkerKind::Status {
                from: from.into(),
                to: to.into(),
            },
        }
    }

    fn rec(ts: &str, run: &str, line: usize, kind: EventKind) -> DrainRecord {
        DrainRecord {
            ev: Ev {
                ts: at(ts),
                spec: Some("SPEC-1".into()),
                run_uuid: run.into(),
                seat: None,
                kind,
            },
            origin: format!("events.jsonl:{line}"),
        }
    }

    fn phase(idx: i32, slug: &str, attempt: u32) -> EventKind {
        EventKind::PhaseEntered {
            idx,
            slug: slug.into(),
            vendor: None,
            seat: None,
            model: None,
            effort: None,
            attempt,
        }
    }

    fn shelved(phase: &str) -> EventKind {
        EventKind::SpecShelved {
            phase: phase.into(),
            kind: "review-changes".into(),
            detail: None,
            recovery_hint: None,
        }
    }

    fn input(store: Vec<StoreMarker>, events: Vec<DrainRecord>) -> TimelineInput {
        TimelineInput {
            id: "SPEC-1".into(),
            title: "fixture".into(),
            current_status: Some("Completed".into()),
            store,
            events,
            checks: Vec::new(),
            notes: Vec::new(),
            as_of: at("2026-09-21T00:00:00Z"),
        }
    }

    /// The partition invariant every other assertion rests on: the spans
    /// tile the window exactly once, and the three classes add up to it.
    fn assert_partition(t: &Timeline) {
        let mut cursor = t.observed_start;
        for s in &t.spans {
            assert_eq!(s.start, cursor, "span {} is not contiguous", s.activity);
            assert!(s.end > s.start, "span {} is not positive", s.activity);
            cursor = s.end;
        }
        assert_eq!(cursor, t.observed_end, "spans do not reach the window end");
        let sum = t.totals.work + t.totals.wait + t.totals.unknown;
        assert!(
            sum == t.totals.elapsed,
            "work+wait+unknown ({sum}) != elapsed ({})",
            t.totals.elapsed
        );
    }

    fn find<'a>(t: &'a Timeline, activity: &str) -> Option<&'a Span> {
        t.spans.iter().find(|s| s.activity == activity)
    }

    /// A real two-run drain with a review shelve, taking its anchors from the
    /// recorded BUG-1424 evidence: filing 07:08:02Z, approval 07:11:44Z, the
    /// store re-entry at 12:28:53Z that precedes the second run's start, the
    /// merge at 12:55:51.296998292Z, and completion at 12:55:59Z.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_drained_bug_1424_two_runs_review_shelve() {
        let store = vec![
            filed("2026-09-20T07:08:02Z", "Draft", "526462ba"),
            tr("2026-09-20T07:11:44Z", "Draft", "Approved", "aaa11111"),
            tr(
                "2026-09-20T07:30:00Z",
                "Approved",
                "In Progress",
                "aaa22222",
            ),
            tr(
                "2026-09-20T11:41:00Z",
                "In Progress",
                "Needs Attention",
                "aaa33333",
            ),
            tr("2026-09-20T12:28:53Z", "Done", "In Progress", "aaa44444"),
            tr("2026-09-20T12:50:00Z", "In Progress", "Done", "aaa55555"),
            tr("2026-09-20T12:55:59Z", "Done", "Completed", "8f076f99"),
        ];
        let events = vec![
            rec("2026-09-20T07:35:00Z", "runA", 4779, EventKind::RunStarted),
            rec(
                "2026-09-20T07:35:10Z",
                "runA",
                4780,
                phase(1, "implementer", 1),
            ),
            rec("2026-09-20T08:20:00Z", "runA", 4781, phase(2, "ci", 1)),
            rec(
                "2026-09-20T08:35:00Z",
                "runA",
                4782,
                EventKind::CiTerminal { green: true },
            ),
            rec(
                "2026-09-20T08:40:00Z",
                "runA",
                4783,
                phase(3, "reviewer", 1),
            ),
            rec("2026-09-20T11:41:00Z", "runA", 4784, shelved("reviewer")),
            rec(
                "2026-09-20T12:34:39.985486329Z",
                "runB",
                4800,
                EventKind::RunStarted,
            ),
            rec(
                "2026-09-20T12:34:45Z",
                "runB",
                4801,
                phase(1, "implementer", 1),
            ),
            rec("2026-09-20T12:44:00Z", "runB", 4802, phase(2, "ci", 1)),
            rec(
                "2026-09-20T12:48:00Z",
                "runB",
                4803,
                EventKind::CiTerminal { green: true },
            ),
            rec(
                "2026-09-20T12:49:00Z",
                "runB",
                4804,
                phase(3, "reviewer", 1),
            ),
            rec("2026-09-20T12:52:00Z", "runB", 4805, phase(4, "merge", 1)),
            rec(
                "2026-09-20T12:55:51.296998292Z",
                "runB",
                4806,
                EventKind::PrMerged { pr: 2015 },
            ),
            rec("2026-09-20T12:55:52Z", "runB", 4807, phase(5, "pull", 1)),
            rec("2026-09-20T12:58:00Z", "runB", 4808, phase(6, "build", 1)),
            // The real feed's trailing PhaseDonePr carries no run UUID and no
            // RunCompleted follows it, so `build` never closes. Preserved.
            rec(
                "2026-09-20T12:58:30Z",
                "",
                4815,
                EventKind::PhaseDonePr { pr: 2015 },
            ),
        ];
        let t = build_timeline(input(store, events)).expect("timeline");

        assert_eq!(t.endpoint_kind, EndpointKind::Completed);
        assert!(!t.incomplete, "a fully recorded lifecycle is not partial");
        // 07:08:02Z → 12:55:59Z, the store-observed elapsed.
        assert_eq!(t.totals.elapsed, 20_877.0);
        assert_partition(&t);

        assert_eq!(
            find(&t, "awaiting approval").map(|s| s.end),
            Some(at("2026-09-20T07:11:44Z"))
        );
        assert_eq!(
            find(&t, "queued").map(|s| (s.start, s.end)),
            Some((at("2026-09-20T07:11:44Z"), at("2026-09-20T07:30:00Z")))
        );

        // The park ends where the STORE says the spec left Needs Attention,
        // not where the next drain run happened to start ~6 minutes later.
        let parked = find(&t, "parked").expect("a park span");
        assert_eq!(parked.start, at("2026-09-20T11:41:00Z"));
        assert_eq!(parked.end, at("2026-09-20T12:28:53Z"));

        // The uninstrumented gap between the store re-entry and the second
        // run's first phase is unknown, not park and not work.
        let gap = t
            .spans
            .iter()
            .find(|s| s.start == at("2026-09-20T12:28:53Z"))
            .expect("a span at the re-entry");
        assert_eq!(gap.class, SpanClass::Unknown);
        assert_eq!(gap.source, SpanSource::None);
        assert_eq!(gap.end, at("2026-09-20T12:34:45Z"));

        // Repeated phases are numbered globally and labelled.
        assert_eq!(
            find(&t, "implementer #2").and_then(|s| s.repeat),
            Some("rework")
        );
        assert_eq!(
            find(&t, "reviewer #2").and_then(|s| s.repeat),
            Some("re-review")
        );
        assert!(find(&t, "implementer #1").is_some());

        // `pull` straddles completion and is split exactly there.
        let pull_main = find(&t, "pull").expect("the pre-completion part of pull");
        assert_eq!(pull_main.end, at("2026-09-20T12:55:59Z"));
        let post_pull = t
            .post_completion_spans
            .iter()
            .find(|s| s.activity == "pull")
            .expect("the post-completion part of pull");
        assert_eq!(post_pull.start, at("2026-09-20T12:55:59Z"));
        assert_eq!(post_pull.end, at("2026-09-20T12:58:00Z"));
        assert!(t.post_completion_s > 0.0);

        // `build` has no terminal, so it becomes a note and NOT a span that
        // runs to the end of the feed.
        assert!(t
            .post_completion_spans
            .iter()
            .all(|s| s.activity != "build"));
        assert!(
            t.coverage_notes
                .iter()
                .any(|n| n.contains("`build`") && n.contains("no recorded end")),
            "notes: {:?}",
            t.coverage_notes
        );

        // A green CI verdict alone never becomes an await-merge wait.
        assert!(find(&t, "awaiting merge").is_none());
        assert!(t.totals.unknown > 0.0);
    }

    /// A spec worked outside a drain: the store transitions and credited
    /// forge evidence carry the whole timeline, and the uninstrumented bulk
    /// of it stays unknown instead of being folded into a neighbour. Anchors
    /// are TASK-1507's recorded times.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_no_drain_task_1507() {
        let store = vec![
            filed("2026-09-25T08:15:03Z", "Approved", "bbb00000"),
            tr(
                "2026-09-25T10:52:03Z",
                "Approved",
                "In Progress",
                "bbb11111",
            ),
            tr(
                "2026-09-25T23:48:38Z",
                "In Progress",
                "Completed",
                "bbb22222",
            ),
        ];
        let events = vec![
            rec(
                "2026-09-25T13:19:11Z",
                "",
                7140,
                EventKind::UnshippedWorkDetected {
                    spec: "TASK-1507".into(),
                    branch: "task-1507".into(),
                    first_seen: "2026-09-25T13:19:11Z".into(),
                },
            ),
            // Emission time, not completion time: it must not move the end.
            rec(
                "2026-09-25T23:48:40Z",
                "",
                7203,
                EventKind::SpecCompleted {
                    commit: "bbb22222".into(),
                    pr: Some(2190),
                    closed_by: "reconcile-status".into(),
                },
            ),
        ];
        let mut inp = input(store, events);
        inp.checks = vec![
            ForgeCheck {
                pr: 2190,
                name: "test".into(),
                started: at("2026-09-25T23:32:00Z"),
                completed: at("2026-09-25T23:40:00Z"),
            },
            // Reversed (a skipped check): discarded, not repaired.
            ForgeCheck {
                pr: 2190,
                name: "skipped".into(),
                started: at("2026-09-25T23:50:00Z"),
                completed: at("2026-09-25T23:45:00Z"),
            },
            // Finishes after the spec completed: post-completion only.
            ForgeCheck {
                pr: 2190,
                name: "late".into(),
                started: at("2026-09-25T23:50:00Z"),
                completed: at("2026-09-25T23:55:00Z"),
            },
        ];
        let t = build_timeline(inp).expect("timeline");

        assert_eq!(t.endpoint_kind, EndpointKind::Completed);
        assert_eq!(t.totals.elapsed, 56_015.0);
        assert_partition(&t);

        // Filed already approved, so no approval wait is invented.
        assert!(find(&t, "awaiting approval").is_none());
        assert!(t
            .coverage_notes
            .iter()
            .any(|n| n.contains("filed straight at Approved")));

        // No drain phase events, so no implementer work is claimed anywhere.
        assert!(
            t.spans
                .iter()
                .all(|s| !s.activity.starts_with("implementer")),
            "invented implementer work: {:?}",
            t.spans.iter().map(|s| &s.activity).collect::<Vec<_>>()
        );
        // The Approved interval is queued; In Progress claims nothing.
        assert_eq!(
            find(&t, "queued").map(|s| (s.start, s.end)),
            Some((at("2026-09-25T08:15:03Z"), at("2026-09-25T10:52:03Z")))
        );

        // The verified check bounds a CI span; everything around it is unknown.
        let ci = find(&t, "ci").expect("a ci span from the check");
        assert_eq!(ci.source, SpanSource::Github);
        assert_eq!(ci.start, at("2026-09-25T23:32:00Z"));
        assert_eq!(ci.end, at("2026-09-25T23:40:00Z"));
        let unknown: Vec<&Span> = t
            .spans
            .iter()
            .filter(|s| s.class == SpanClass::Unknown)
            .collect();
        assert!(
            unknown
                .iter()
                .any(|s| s.start == at("2026-09-25T10:52:03Z")
                    && s.end == at("2026-09-25T23:32:00Z")),
            "the uninstrumented working day must be one visible unknown span"
        );
        assert!(t.totals.unknown > t.totals.work);
        assert!(t
            .coverage_notes
            .iter()
            .any(|n| n.contains("`skipped`") && n.contains("discarded")));
        assert!(t
            .post_completion_spans
            .iter()
            .any(|s| s.start == at("2026-09-25T23:50:00Z")));
    }

    /// An in-phase retry is not a park, repeated phases are numbered, and
    /// `awaiting merge` needs a real work-done record.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_rework_retry_and_rereview() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Draft", "c0"),
            tr("2026-09-20T00:10:00Z", "Draft", "Approved", "c1"),
            tr("2026-09-20T00:20:00Z", "Approved", "In Progress", "c2"),
            tr("2026-09-20T02:00:00Z", "In Progress", "Completed", "c3"),
        ];
        let events = vec![
            rec("2026-09-20T00:20:00Z", "r1", 1, EventKind::RunStarted),
            rec("2026-09-20T00:20:05Z", "r1", 2, phase(1, "implementer", 1)),
            rec("2026-09-20T00:40:00Z", "r1", 3, phase(2, "ci", 1)),
            rec(
                "2026-09-20T00:50:00Z",
                "r1",
                4,
                EventKind::SpecRetried {
                    phase: "ci".into(),
                    cause: "watchdog".into(),
                    attempt: 2,
                    max: 3,
                    model_before: None,
                    model_after: None,
                    detail: None,
                },
            ),
            rec("2026-09-20T00:50:01Z", "r1", 5, phase(2, "ci", 2)),
            rec(
                "2026-09-20T01:00:00Z",
                "r1",
                6,
                EventKind::CiTerminal { green: true },
            ),
            rec("2026-09-20T01:05:00Z", "r1", 7, phase(3, "reviewer", 1)),
            rec(
                "2026-09-20T01:30:00Z",
                "r1",
                8,
                EventKind::ReviewVerdictRecorded {
                    pr: Some(9),
                    verdict: "approved".into(),
                    reviewed_sha: None,
                },
            ),
            rec(
                "2026-09-20T01:55:00Z",
                "r1",
                9,
                EventKind::PrMerged { pr: 9 },
            ),
        ];
        let t = build_timeline(input(store, events)).expect("timeline");
        assert_partition(&t);

        // A retry marker must never manufacture parked time.
        assert!(
            t.spans.iter().all(|s| s.activity != "parked"),
            "a retry was mistaken for a park"
        );
        assert_eq!(
            find(&t, "ci #1").map(|s| s.end),
            Some(at("2026-09-20T00:50:01Z"))
        );
        assert_eq!(find(&t, "ci #2").and_then(|s| s.repeat), Some("repeat"));

        // Review recorded → merge is a real await-merge wait.
        let aw = find(&t, "awaiting merge").expect("awaiting merge");
        assert_eq!(aw.class, SpanClass::Wait);
        assert_eq!(aw.start, at("2026-09-20T01:30:00Z"));
        assert_eq!(aw.end, at("2026-09-20T01:55:00Z"));
    }

    /// A shelve with no store record resumes on the event that ends it, and a
    /// phase whose only candidate terminal belongs to another run stays
    /// unbounded rather than borrowing it.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_shelve_resume_without_phase_terminal() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "d0"),
            tr("2026-09-20T04:00:00Z", "Approved", "Completed", "d1"),
        ];
        let events = vec![
            rec("2026-09-20T00:30:00Z", "r1", 1, phase(1, "implementer", 1)),
            rec("2026-09-20T01:00:00Z", "r1", 2, shelved("implementer")),
            rec(
                "2026-09-20T02:00:00Z",
                "r1",
                3,
                EventKind::SpecRequeued {
                    via: "queue-rework".into(),
                    actor: None,
                    from: "Needs Attention".into(),
                    to: "Approved".into(),
                    cleared_tags: Vec::new(),
                    kept_tags: Vec::new(),
                },
            ),
            rec("2026-09-20T02:30:00Z", "r2", 4, phase(1, "reviewer", 1)),
            // A DIFFERENT run's completion: it cannot close r2's reviewer.
            rec(
                "2026-09-20T03:00:00Z",
                "r9",
                5,
                EventKind::RunCompleted {
                    pull_completed: true,
                    build_completed: true,
                },
            ),
        ];
        let t = build_timeline(input(store, events)).expect("timeline");
        assert_partition(&t);

        assert_eq!(
            find(&t, "implementer").map(|s| (s.start, s.end)),
            Some((at("2026-09-20T00:30:00Z"), at("2026-09-20T01:00:00Z")))
        );
        assert_eq!(
            find(&t, "parked").map(|s| (s.start, s.end)),
            Some((at("2026-09-20T01:00:00Z"), at("2026-09-20T02:00:00Z")))
        );
        // The orphaned reviewer phase produces no span at all.
        assert!(find(&t, "reviewer").is_none());
        assert!(t
            .coverage_notes
            .iter()
            .any(|n| n.contains("`reviewer`") && n.contains("no recorded end")));
        let covering = t
            .spans
            .iter()
            .find(|s| s.start <= at("2026-09-20T02:30:00Z") && s.end > at("2026-09-20T02:30:00Z"))
            .expect("some span must cover the orphaned phase's start");
        assert_eq!(covering.class, SpanClass::Unknown);
        assert_eq!(covering.start, at("2026-09-20T02:00:00Z"));
    }

    /// Sub-second store precision survives, and the filed status is read from
    /// the added revision rather than assumed to be Draft.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_precise_store_clock_and_initial_status() {
        assert_eq!(
            yaml_status("status: Approved\ntitle: x\n").as_deref(),
            Some("Approved")
        );
        assert_eq!(
            yaml_status("status: Draft\ncustom_status: Blocked\n").as_deref(),
            Some("Blocked"),
            "the user-visible custom status wins, as it does elsewhere in history"
        );
        assert_eq!(yaml_status("title: x\n"), None);

        let store = vec![
            filed("2026-09-20T00:00:00.250Z", "Approved", "e0"),
            tr("2026-09-20T00:00:10.750Z", "Approved", "Completed", "e1"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert_eq!(
            t.totals.elapsed, 10.5,
            "fractional seconds are not rounded away"
        );
        assert!(find(&t, "awaiting approval").is_none());
        assert_partition(&t);
    }

    /// Equal-authority disagreement resolves to unknown, a reopen after
    /// completion ends the window at the last observed boundary, and post
    /// spans are unioned rather than summed.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_partition_overlap_reopen_and_post_completion() {
        // Two concurrent runs claiming different activities over the same
        // instant, both at phase authority.
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "f0"),
            tr("2026-09-20T01:00:00Z", "Approved", "Completed", "f1"),
            // Reopened after completing.
            tr("2026-09-20T02:00:00Z", "Completed", "In Progress", "f2"),
        ];
        let events = vec![
            rec("2026-09-20T00:10:00Z", "rA", 1, phase(1, "implementer", 1)),
            rec("2026-09-20T00:40:00Z", "rA", 2, phase(2, "pull", 1)),
            rec(
                "2026-09-20T00:50:00Z",
                "rA",
                3,
                EventKind::RunCompleted {
                    pull_completed: true,
                    build_completed: true,
                },
            ),
            rec("2026-09-20T00:10:00Z", "rB", 3, phase(1, "reviewer", 1)),
            rec("2026-09-20T00:40:00Z", "rB", 4, phase(2, "pull", 1)),
            rec(
                "2026-09-20T00:50:00Z",
                "rB",
                5,
                EventKind::RunCompleted {
                    pull_completed: true,
                    build_completed: true,
                },
            ),
        ];
        let t = build_timeline(input(store, events)).expect("timeline");
        assert_eq!(t.endpoint_kind, EndpointKind::LastObserved);
        assert!(t.incomplete);
        assert_eq!(t.observed_end, at("2026-09-20T02:00:00Z"));
        assert!(t.coverage_notes.iter().any(|n| n.contains("reopened")));
        assert_partition(&t);

        let disputed = find(&t, "disputed").expect("a disputed interval");
        assert_eq!(disputed.class, SpanClass::Unknown);
        assert_eq!(disputed.source, SpanSource::None);
        assert!(disputed.evidence.is_empty());
        assert!(t
            .coverage_notes
            .iter()
            .any(|n| n.contains("equal") && n.contains("unknown")));

        // Overlapping post-completion work is unioned, not added up.
        let spans = vec![
            Span {
                activity: "pull".into(),
                class: SpanClass::Work,
                start: at("2026-09-20T03:00:00Z"),
                end: at("2026-09-20T03:10:00Z"),
                source: SpanSource::Events,
                repeat: None,
                merge_key: None,
                evidence: Vec::new(),
            },
            Span {
                activity: "build".into(),
                class: SpanClass::Work,
                start: at("2026-09-20T03:05:00Z"),
                end: at("2026-09-20T03:20:00Z"),
                source: SpanSource::Events,
                repeat: None,
                merge_key: None,
                evidence: Vec::new(),
            },
        ];
        assert_eq!(union_seconds(&spans), 1200.0);
    }

    /// Parallel forge checks fill one CI interval by union, and a check that
    /// merely has a verdict contributes nothing.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_forge_repo_head_attempt_and_parallel_checks() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "g0"),
            tr("2026-09-20T01:00:00Z", "Approved", "Completed", "g1"),
        ];
        let mut inp = input(store, Vec::new());
        inp.checks = vec![
            ForgeCheck {
                pr: 7,
                name: "unit".into(),
                started: at("2026-09-20T00:10:00Z"),
                completed: at("2026-09-20T00:30:00Z"),
            },
            ForgeCheck {
                pr: 7,
                name: "clippy".into(),
                started: at("2026-09-20T00:20:00Z"),
                completed: at("2026-09-20T00:40:00Z"),
            },
        ];
        let t = build_timeline(inp).expect("timeline");
        assert_partition(&t);
        // 00:10 → 00:40 is 1800s of wall clock; summing the two jobs would
        // have produced 2400s and overstated the work.
        assert_eq!(t.totals.work, 1800.0);
        let ci = find(&t, "ci").expect("ci");
        assert_eq!(ci.source, SpanSource::Github);
        assert!(ci.evidence.iter().any(|e| e.contains("pr/7/check/unit")));
        assert!(ci.evidence.iter().any(|e| e.contains("pr/7/check/clippy")));

        // A verdict with no usable interval is not admitted.
        let parsed = parse_gh_checks(
            7,
            r#"{"statusCheckRollup":[{"name":"verdict-only","conclusion":"SUCCESS"}]}"#,
        )
        .expect("valid json");
        assert!(parsed.is_empty());
    }

    /// Every forge-degradation path leaves the store and event evidence
    /// intact and produces exactly one aggregated note.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_missing_gh_and_bad_network_json() {
        struct Failing(&'static str);
        impl ForgeTimeSource for Failing {
            fn pr_checks(&self, _pr: u32) -> std::result::Result<Vec<ForgeCheck>, String> {
                Err(self.0.to_string())
            }
        }
        for reason in [
            "`gh` is not on PATH, so no PR or CI timestamps were read",
            "`gh` did not answer within 10s, so PR/CI timestamps were skipped",
            "`gh` refused the request (auth, network, or unknown PR)",
        ] {
            let src = Failing(reason);
            assert_eq!(src.pr_checks(1).unwrap_err(), reason);
        }
        assert!(parse_gh_checks(1, "not json at all").is_err());
        assert!(parse_gh_checks(1, "{}").expect("empty payload").is_empty());

        // With no forge evidence the store/event spans are unchanged.
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Draft", "h0"),
            tr("2026-09-20T00:30:00Z", "Draft", "Approved", "h1"),
            tr("2026-09-20T01:00:00Z", "Approved", "Completed", "h2"),
        ];
        let mut inp = input(store, Vec::new());
        inp.notes
            .push("forge timestamps unavailable (`gh` is not on PATH)".into());
        let t = build_timeline(inp).expect("timeline");
        assert_partition(&t);
        assert_eq!(
            find(&t, "awaiting approval").map(|s| s.duration_s()),
            Some(1800.0)
        );
        assert_eq!(
            t.coverage_notes
                .iter()
                .filter(|n| n.contains("forge timestamps unavailable"))
                .count(),
            1
        );
        // JSON stays valid and complete with no forge leg.
        let doc = render_json(&t);
        assert!(doc["post_completion_spans"].is_array());
        assert!(doc["post_completion_s"].is_number());
    }

    /// The live feed and its rotated archive are both read, a duplicate is
    /// collapsed, an unreadable line is reported rather than silently
    /// dropped, and an absent feed is reported as lost coverage, not as zero
    /// drain time.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_rotated_malformed_and_missing_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let (empty, notes) = collect_drain_events(root, "SPEC-1");
        assert!(empty.is_empty());
        assert!(notes
            .iter()
            .any(|n| n.contains("no local drain event feed")));

        std::fs::create_dir_all(root.join(".aida")).expect("mkdir");
        let line = |ts: &str| {
            format!(
                r#"{{"ts":"{ts}","spec":"SPEC-1","run_uuid":"r1","kind":{{"event":"RunStarted"}}}}"#
            )
        };
        // The archive holds one line; the live feed repeats it exactly (a
        // rotation overlap) and adds an unreadable line and another spec's.
        std::fs::write(
            crate::events::events_archive_path(root),
            format!("{}\n", line("2026-09-20T00:00:00Z")),
        )
        .expect("write archive");
        std::fs::write(
            crate::events::events_path(root),
            format!(
                "{}\n{{ not json\n{}\n",
                line("2026-09-20T00:00:00Z"),
                r#"{"ts":"2026-09-20T00:05:00Z","spec":"OTHER-2","run_uuid":"r1","kind":{"event":"RunStarted"}}"#
            ),
        )
        .expect("write live");

        let before_live = std::fs::read(crate::events::events_path(root)).expect("read");
        let before_archive = std::fs::read(crate::events::events_archive_path(root)).expect("read");

        let (recs, notes) = collect_drain_events(root, "SPEC-1");
        assert_eq!(recs.len(), 1, "the duplicated line must collapse to one");
        assert!(notes.iter().any(|n| n.contains("could not be read")));
        assert!(notes.iter().any(|n| n.contains("duplicate")));

        // Reading is read-only: neither file changed.
        assert_eq!(
            before_live,
            std::fs::read(crate::events::events_path(root)).expect("read")
        );
        assert_eq!(
            before_archive,
            std::fs::read(crate::events::events_archive_path(root)).expect("read")
        );
    }

    /// A spec completed on machine X shows its boundaries on machine Y: the
    /// record X published to the store supplies the events Y's feed never
    /// saw, through the same decoder, and the union never doubles a line
    /// on X itself.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn story_1480_published_record_supplies_cross_machine_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("store");
        std::fs::create_dir_all(store.join("objects")).expect("mkdir");

        // Machine X: a feed with two lifecycle lines, published at done.
        let mx = dir.path().join("mx");
        std::fs::create_dir_all(mx.join(".aida")).expect("mkdir");
        std::fs::write(
            crate::events::events_path(&mx),
            concat!(
                r#"{"ts":"2026-09-20T00:00:00Z","spec":"SPEC-9","run_uuid":"r1","kind":{"event":"RunStarted"}}"#,
                "\n",
                r#"{"ts":"2026-09-20T01:00:00Z","spec":"SPEC-9","run_uuid":"r1","kind":{"event":"PrMerged","pr":7}}"#,
                "\n"
            ),
        )
        .expect("write feed");
        crate::timing_record::publish_to_store(&store, &mx, "SPEC-9", "queue-done")
            .expect("published");

        // Machine Y: no feed at all — everything comes from the record.
        let my = dir.path().join("my");
        std::fs::create_dir_all(&my).expect("mkdir");
        let (local, _) = collect_drain_events(&my, "SPEC-9");
        assert!(local.is_empty());
        let (published, notes) = collect_published_events(&store, "SPEC-9");
        assert!(
            notes.is_empty(),
            "readable record carries no note: {notes:?}"
        );
        let (merged, contributed) = merge_records(local, published);
        assert_eq!(contributed, 2);
        assert_eq!(merged.len(), 2);
        assert!(matches!(merged[1].ev.kind, EventKind::PrMerged { pr: 7 }));
        assert!(merged
            .iter()
            .all(|r| r.origin.starts_with("timing-record:")));

        // Machine X itself: the union must not double a boundary, and the
        // local line wins the tie so origins keep naming the feed.
        let (local_x, _) = collect_drain_events(&mx, "SPEC-9");
        let (published_x, _) = collect_published_events(&store, "SPEC-9");
        let (merged_x, contributed_x) = merge_records(local_x, published_x);
        assert_eq!(merged_x.len(), 2);
        assert_eq!(contributed_x, 0, "nothing beyond the local feed");
        assert!(merged_x.iter().all(|r| r.origin.starts_with("events")));
    }

    /// The three surfaces carry one model: the same classes, bounds and
    /// totals, and the unknown rows appear in all of them.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_human_toon_json_contract() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Draft", "i0"),
            tr("2026-09-20T00:30:00Z", "Draft", "Approved", "i1"),
            tr("2026-09-20T01:00:00Z", "Approved", "In Progress", "i2"),
            tr("2026-09-20T02:00:00Z", "In Progress", "Completed", "i3"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert_partition(&t);
        assert!(t.totals.unknown > 0.0);

        let human = render_human(&t);
        assert!(
            human.contains("unknown"),
            "human view must name unknown time"
        );
        assert!(human.contains("elapsed"));

        let toon = render_toon(&t);
        assert!(toon.contains(&format!("view: {VIEW_NAME}")));
        assert!(toon.contains("order: oldest-first"));
        assert!(toon.contains("spans["));
        assert!(toon.contains("post_completion_spans["));
        assert!(toon.contains(&format!("unknown_s: {:.3}", t.totals.unknown)));

        let doc = render_json(&t);
        assert_eq!(doc["schema_version"], json!(SCHEMA_VERSION));
        assert_eq!(doc["view"], json!(VIEW_NAME));
        assert_eq!(doc["order"], json!("oldest-first"));
        assert_eq!(doc["endpoint_kind"], json!("completed"));
        assert_eq!(doc["incomplete"], json!(false));
        assert!(doc["coverage_notes"].is_array());
        let rows = doc["spans"].as_array().expect("spans array");
        assert_eq!(rows.len(), t.spans.len());
        for row in rows {
            for key in ["activity", "class", "start", "end", "duration_s", "source"] {
                assert!(row.get(key).is_some(), "span row is missing {key}");
            }
            let class = row["class"].as_str().unwrap();
            assert!(
                matches!(class, "work" | "wait" | "unknown"),
                "class {class}"
            );
            let source = row["source"].as_str().unwrap();
            assert!(
                matches!(source, "store" | "events" | "github" | "mixed" | "none"),
                "source {source}"
            );
            // An unknown span never cites a source that did not explain it.
            if class == "unknown" {
                assert_eq!(source, "none");
            }
        }
        let sum: f64 = rows.iter().map(|r| r["duration_s"].as_f64().unwrap()).sum();
        assert!((sum - doc["totals_s"]["elapsed"].as_f64().unwrap()).abs() < 0.002);

        // Every TOON span row round-trips with the same field order.
        // The spans table decodes back to the same field order and row count.
        let spans_block = toon
            .split_once("spans[")
            .map(|(_, rest)| format!("spans[{rest}"))
            .expect("a spans table");
        let table = crate::toon::parse_table(&spans_block).expect("spans table parses");
        let want: Vec<String> = ROW_FIELDS.iter().map(|f| f.to_string()).collect();
        assert_eq!(table.fields, want);
        assert_eq!(table.rows.len(), t.spans.len());
        assert_eq!(table.rows[0][1], t.spans[0].class.as_str());
    }

    /// A spec with neither store history nor events is refused rather than
    /// rendered as an empty-but-confident timeline.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_no_evidence_is_refused() {
        let err = build_timeline(input(Vec::new(), Vec::new())).expect_err("must refuse");
        assert!(err.to_string().contains("nothing to build a timeline from"));
    }

    /// A one-instant lifecycle reports no spans instead of a zero-length or
    /// invented one.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_degenerate_window_reports_no_spans() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "j0"),
            tr("2026-09-20T00:00:00Z", "Approved", "Completed", "j1"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(t.spans.is_empty());
        assert!(t.incomplete);
        assert_eq!(t.totals.elapsed, 0.0);
        assert!(t
            .coverage_notes
            .iter()
            .any(|n| n.contains("no measurable window")));
    }

    #[test]
    fn story_1478_duration_formatting_is_readable() {
        assert_eq!(fmt_dur(0.0), "0s");
        assert_eq!(fmt_dur(42.4), "42s");
        assert_eq!(fmt_dur(222.0), "3m42s");
        assert_eq!(fmt_dur(20_877.0), "5h47m57s");
        assert_eq!(fmt_dur(90_061.0), "1d1h1m1s");
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_submillisecond_spans_conserve_exactly() {
        let start = at("2026-09-20T00:00:00Z");
        let mut store = vec![filed("2026-09-20T00:00:00Z", "Draft", "n0")];
        let mut previous = "Draft";
        for i in 1..20 {
            let next = if i % 2 == 0 { "Draft" } else { "Approved" };
            store.push(StoreMarker {
                at: start + chrono::Duration::nanoseconds(i * 250_000),
                sha: format!("n{i}"),
                kind: StoreMarkerKind::Status {
                    from: previous.into(),
                    to: next.into(),
                },
            });
            previous = if next == "Draft" { "Draft" } else { "Approved" };
        }
        store.push(StoreMarker {
            at: start + chrono::Duration::milliseconds(5),
            sha: "n20".into(),
            kind: StoreMarkerKind::Status {
                from: previous.into(),
                to: "Completed".into(),
            },
        });
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert_partition(&t);
        assert_eq!(t.totals.elapsed, 0.005);
        assert_eq!(
            t.totals.work + t.totals.wait + t.totals.unknown,
            t.totals.elapsed
        );
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_skewed_store_transition_is_unknown() {
        let store = vec![
            filed("2026-09-20T00:00:01Z", "Approved", "s0"),
            tr("2026-09-20T00:00:03Z", "Approved", "In Progress", "s1"),
            tr("2026-09-20T00:00:02Z", "In Progress", "Completed", "s2"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(t.incomplete);
        assert!(t.spans.iter().any(|s| s.class == SpanClass::Unknown));
        assert!(t.coverage_notes.iter().any(|n| n.contains("contradict")));
    }

    /// The continuity guard must not over-narrow the skew detector. Here the
    /// true order is Approved -> In Progress -> In Review -> Completed, but
    /// clock skew timestamps the In Review -> Completed transition BEFORE the
    /// In Progress -> In Review one, so timestamp ordering interleaves them.
    /// The transition under test starts from neither the preceding state nor a
    /// gap: the FOLLOWING transition is the true successor, which is exactly
    /// what proves the ordering is wrong rather than merely incomplete.
    // trace:STORY-1478 | ai:claude
    #[test]
    fn story_1478_skew_three_transitions_deep_is_still_contradictory() {
        let store = vec![
            filed("2026-09-20T00:00:01Z", "Approved", "d0"),
            tr("2026-09-20T00:00:02Z", "Approved", "In Progress", "d1"),
            // Skewed: recorded before the transition that must precede it.
            tr("2026-09-20T00:00:03Z", "In Review", "Completed", "d2"),
            tr("2026-09-20T00:00:04Z", "In Progress", "In Review", "d3"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(
            t.incomplete,
            "skew that is only visible three transitions deep must still be caught"
        );
        assert!(t.spans.iter().any(|s| s.class == SpanClass::Unknown));
        assert!(t.coverage_notes.iter().any(|n| n.contains("contradict")));
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_skew_after_two_later_transitions_is_contradictory() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "x0"),
            tr("2026-09-20T00:00:02Z", "A", "B", "x1"),
            tr("2026-09-20T00:00:04Z", "C", "D", "x2"),
            tr("2026-09-20T00:00:05Z", "D", "Completed", "x3"),
            tr("2026-09-20T00:00:06Z", "B", "C", "x4"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(t.incomplete);
        assert!(t.spans.iter().any(|s| s.class == SpanClass::Unknown));
        assert!(t.coverage_notes.iter().any(|n| n.contains("contradict")));
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_skew_after_three_later_transitions_is_contradictory() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "y0"),
            tr("2026-09-20T00:00:02Z", "A", "B", "y1"),
            tr("2026-09-20T00:00:04Z", "C", "D", "y2"),
            tr("2026-09-20T00:00:05Z", "D", "E", "y3"),
            tr("2026-09-20T00:00:06Z", "E", "F", "y4"),
            tr("2026-09-20T00:00:07Z", "B", "C", "y5"),
            tr("2026-09-20T00:00:08Z", "F", "Completed", "y6"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(t.incomplete);
        assert!(t.spans.iter().any(|s| s.class == SpanClass::Unknown));
        assert!(t.coverage_notes.iter().any(|n| n.contains("contradict")));
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_consistent_rework_chain_is_not_skew() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "z0"),
            tr("2026-09-20T00:00:02Z", "Approved", "In Progress", "z1"),
            tr("2026-09-20T00:00:03Z", "In Progress", "Done", "z2"),
            tr("2026-09-20T00:00:04Z", "Done", "In Progress", "z3"),
            tr("2026-09-20T00:00:05Z", "In Progress", "Done", "z4"),
            tr("2026-09-20T00:00:06Z", "Done", "Completed", "z5"),
        ];
        let t = build_timeline(input(store, Vec::new())).expect("timeline");
        assert!(!t.incomplete);
        assert!(!t.coverage_notes.iter().any(|n| n.contains("contradict")));
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_unfinished_endpoint_is_last_observed_boundary() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "u0"),
            tr("2026-09-20T00:00:02Z", "Approved", "In Progress", "u1"),
        ];
        let last = at("2026-09-20T00:00:04.123456789Z");
        let event = rec(
            "2026-09-20T00:00:04.123456789Z",
            "run",
            1,
            EventKind::RunStarted,
        );
        let mut fixture = input(store, vec![event]);
        fixture.as_of = last;
        let t = build_timeline(fixture.clone()).expect("timeline");
        assert_eq!(t.endpoint_kind, EndpointKind::LastObserved);
        assert_eq!(t.observed_end, last);
        fixture.as_of += chrono::Duration::days(10);
        assert_eq!(build_timeline(fixture).unwrap().observed_end, last);
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_run_completed_requires_matching_success_flag() {
        let store = vec![
            filed("2026-09-20T00:00:00Z", "Approved", "c0"),
            tr("2026-09-20T00:02:00Z", "Approved", "Completed", "c1"),
        ];
        let events = vec![
            rec("2026-09-20T00:00:10Z", "run", 1, phase(1, "pull", 1)),
            rec(
                "2026-09-20T00:01:00Z",
                "run",
                2,
                EventKind::RunCompleted {
                    pull_completed: false,
                    build_completed: true,
                },
            ),
        ];
        let t = build_timeline(input(store, events)).expect("timeline");
        assert!(find(&t, "pull").is_none());
        assert!(t
            .spans
            .iter()
            .all(|s| s.class != SpanClass::Work || s.activity != "pull"));
    }

    // trace:STORY-1478 | ai:codex
    #[test]
    fn story_1478_malformed_store_log_marks_coverage_incomplete() {
        let mut notes = Vec::new();
        let parsed = parse_store_log("broken record\n", &mut notes);
        assert!(parsed.is_empty());
        assert!(notes
            .iter()
            .any(|n| n.contains("could not be parsed") && n.contains("incomplete")));
        let mut fixture = input(
            vec![
                filed("2026-09-20T00:00:00Z", "Approved", "m0"),
                tr("2026-09-20T00:01:00Z", "Approved", "Completed", "m1"),
            ],
            Vec::new(),
        );
        fixture.notes = notes;
        assert!(build_timeline(fixture).unwrap().incomplete);
    }
}
