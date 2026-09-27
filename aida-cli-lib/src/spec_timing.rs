//! Publish a per-spec timing record to the store at terminal state (ADR-60).
//!
//! This module is the bridge between the LOCAL live event stream
//! (`.aida/events.jsonl`, [`crate::events`]) and the CROSS-MACHINE record
//! ([`aida_core::spec_timing`]). It does two things and nothing else:
//!
//! 1. [`spans_for_spec`] folds the local stream's lifecycle events into a span
//!    list. It is pure, so what gets published is testable from a fixture
//!    stream with no store and no git.
//! 2. [`publish`] writes that list into `timings/<SPEC-ID>.yaml` on the
//!    `aida-store` branch through a CAS push-wins loop, unioning with whatever
//!    other machines already published.
//!
//! # What is and is not published
//!
//! Only the lifecycle kinds the fold reads (`PhaseEntered`/`PhaseEnded`,
//! `ActivitySpan`, `SpecParked`/`SpecRequeued`) become spans. `MailReceived`,
//! `CronJobFired`, `ShiftTick` and the rest of the local feed are structurally
//! excluded — not filtered out downstream, never read in the first place.
//!
//! # What a published span does and does not prove
//!
//! A span's `duration_s` is a single host's start-to-end measurement. A gap
//! between two spans is NOT: see [`aida_core::spec_timing::cross_node_gap`] and
//! the module docs there for the clock contract this record is built around.
//! An activity that was still open when the spec completed publishes with no
//! end and no duration rather than being given an invented one.
//!
//! trace:STORY-1480 trace:ADR-60 | ai:claude

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use aida_core::spec_timing::{self, Publication, Span, TimingRecord};

use crate::events::{Event, EventKind};

/// Source label stamped on every span this module derives, so a reader can tell
/// a span folded from the live event log from one a later story derives out of
/// store history or a forge API.
pub(crate) const SOURCE_EVENTS: &str = "events";

/// The activity name a park interval is published under. It is in the `wait`
/// class, which is the whole point: before this, a park showed as unknown time.
pub(crate) const ACTIVITY_PARKED: &str = "parked";

/// A phase entry still waiting for its end.
struct OpenPhase {
    slug: String,
    attempt: u32,
    start: DateTime<Utc>,
}

/// Fold a local event stream into the span list for one spec.
///
/// Pure: `node` is supplied by the caller rather than resolved here, and no
/// path is touched, so a fixture stream produces an exactly predictable record.
///
/// Pairing rules:
/// - `PhaseEntered` opens a span keyed by `(idx, attempt)`; the matching
///   `PhaseEnded` closes it. A second `PhaseEntered` for the same key without
///   an intervening end replaces the open span's start — the earlier entry was
///   an announce this stream never saw the end of, and inventing an end for it
///   would fabricate a duration.
/// - `ActivitySpan` is already complete: it carries its own start and the
///   event's `ts` is its end.
/// - `SpecParked` opens a wait; `SpecRequeued` closes it, preferring the
///   requeue's own `parked_since` (recorded at the clearing site) and falling
///   back to the open marker.
/// - Anything still open when the stream ends is published with no end, so it
///   reads as unmeasured rather than as zero.
// trace:STORY-1480 | ai:claude
pub(crate) fn spans_for_spec(events: &[Event], spec: &str, node: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut open_phases: HashMap<(i32, u32), OpenPhase> = HashMap::new();
    let mut open_park: Option<DateTime<Utc>> = None;

    for ev in events {
        if !ev
            .spec
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(spec))
        {
            continue;
        }
        match &ev.kind {
            EventKind::PhaseEntered {
                idx, slug, attempt, ..
            } => {
                open_phases.insert(
                    (*idx, *attempt),
                    OpenPhase {
                        slug: slug.clone(),
                        attempt: *attempt,
                        start: ev.ts,
                    },
                );
            }
            EventKind::PhaseEnded {
                idx,
                slug,
                attempt,
                outcome: _,
            } => {
                if let Some(open) = open_phases.remove(&(*idx, *attempt)) {
                    spans.push(Span::new(
                        open.slug,
                        open.start,
                        Some(ev.ts),
                        SOURCE_EVENTS,
                        node,
                        open.attempt,
                    ));
                } else {
                    // An end with no recorded entry (the stream was rotated
                    // between them, or the entry predates this
                    // instrumentation). Record the fact, not a guessed start:
                    // a zero-length span at the end stamp keeps the activity
                    // visible while its duration stays unmeasured.
                    spans.push(Span::new(
                        slug.clone(),
                        ev.ts,
                        None,
                        SOURCE_EVENTS,
                        node,
                        *attempt,
                    ));
                }
            }
            EventKind::ActivitySpan {
                activity,
                started_at,
                ..
            } => {
                spans.push(Span::new(
                    activity.clone(),
                    *started_at,
                    Some(ev.ts),
                    SOURCE_EVENTS,
                    node,
                    1,
                ));
            }
            EventKind::SpecParked { .. } => {
                open_park = Some(ev.ts);
            }
            EventKind::SpecRequeued { parked_since, .. } => {
                if let Some(start) = parked_since.or(open_park) {
                    spans.push(Span::new(
                        ACTIVITY_PARKED,
                        start,
                        Some(ev.ts),
                        SOURCE_EVENTS,
                        node,
                        1,
                    ));
                }
                open_park = None;
            }
            _ => {}
        }
    }

    // Still-open activities: published without an end so they report as
    // unmeasured. Sorted for a deterministic record.
    let mut leftovers: Vec<OpenPhase> = open_phases.into_values().collect();
    leftovers.sort_by_key(|p| p.start);
    for open in leftovers {
        spans.push(Span::new(
            open.slug,
            open.start,
            None,
            SOURCE_EVENTS,
            node,
            open.attempt,
        ));
    }
    if let Some(start) = open_park {
        spans.push(Span::new(
            ACTIVITY_PARKED,
            start,
            None,
            SOURCE_EVENTS,
            node,
            1,
        ));
    }

    spans.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id().cmp(&b.id())));
    spans
}

/// STORY-1480: the activity name for an interactive session held under `role`.
///
/// The role is used verbatim so the record says which seat actually sat there,
/// rather than labelling every interactive session `implementer`. A role the
/// span vocabulary does not know classifies as unknown time — visibly
/// unattributed, which is the honest outcome, not silently counted as work.
// trace:STORY-1480 | ai:claude
pub(crate) fn interactive_activity_for_role(role: Option<&str>) -> String {
    match role.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => r.to_ascii_lowercase(),
        None => crate::events::ACTIVITY_IMPLEMENTER.to_string(),
    }
}

/// STORY-1480: is a live drain already accounting for `spec`'s phases?
///
/// An interactive span and a drain phase span would otherwise double-count the
/// same wall time. The drain's own `PhaseEntered`/`PhaseEnded` pair is the more
/// precise record, so the interactive span stands down when a drain holds the
/// spec.
// trace:STORY-1480 | ai:claude
pub(crate) fn drain_owns_spec(project_root: &Path, spec: &str) -> bool {
    // A drain launched from the main checkout writes its state file there, so
    // a `session end` run from inside a linked worktree must look at the main
    // root or it would see no drain and double-count the same wall time.
    // Resolved through the same seam the event stream uses.
    let roots = [
        project_root.to_path_buf(),
        crate::events::stream_root(project_root),
    ];
    roots.iter().any(|root| {
        crate::drain_state::DrainState::read(root)
            .and_then(|s| s.current)
            .is_some_and(|c| c.eq_ignore_ascii_case(spec))
    })
}

/// The store worktree for a project, or `None` when no store is attached.
fn store_root(project_root: &Path) -> Option<PathBuf> {
    let root = project_root.join(".aida-store");
    root.join("objects").is_dir().then_some(root)
}

/// This clone's node id — the authoritative per-clone discriminator is the
/// `registry/nodes.toml` entry keyed by `clone_path`, because two clones
/// sharing one store inherit the same `.aida/node.toml` id (it rides the
/// branch). Falls back to the store node id, then `"1"`, so a span always
/// names SOME recording host rather than an empty string.
// trace:STORY-1480 | ai:claude
fn recording_node(store_root: &Path, clone_path: &Path) -> String {
    let canon = clone_path
        .canonicalize()
        .unwrap_or_else(|_| clone_path.to_path_buf());
    let registry_path = store_root.join("registry").join("nodes.toml");
    if let Ok(registry) = aida_core::NodeRegistry::load(&registry_path) {
        for node in &registry.nodes {
            if let Some(p) = &node.clone_path {
                let np = p.canonicalize().unwrap_or_else(|_| p.clone());
                if np == canon {
                    return node.id.clone();
                }
            }
        }
    }
    aida_core::NodeConfig::load(&store_root.join(".aida").join("node.toml"))
        .map(|c| c.node_id)
        .unwrap_or_else(|_| "1".to_string())
}

/// Hostname, best-effort, honoring the same override the coordination claims
/// use so a test fixture never leaks a real machine name.
fn hostname() -> Option<String> {
    std::env::var("AIDA_HOST_OVERRIDE")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

/// Is `store_root` a git checkout (a worktree `.git` file or a repo `.git` dir)?
fn is_git_checkout(store_root: &Path) -> bool {
    store_root.join(".git").exists()
}

/// Build the record this clone wants to publish for `spec` — pure given the
/// stream and the identity, so a test asserts the published shape without git.
// trace:STORY-1480 | ai:claude
pub(crate) fn record_to_publish(
    events: &[Event],
    spec: &str,
    node: &str,
    host: Option<String>,
    at: DateTime<Utc>,
) -> TimingRecord {
    let spans = spans_for_spec(events, spec, node);
    let mut record = TimingRecord::new(spec);
    record.published_by.push(Publication {
        node: node.to_string(),
        host,
        at,
        spans: spans.len(),
    });
    record.spans = spans;
    record
}

/// Publish `spec`'s timing record to the store, unioning with whatever is
/// already there.
///
/// Returns `Ok(false)` when there was nothing to publish: no store attached, or
/// the local stream holds no lifecycle span for this spec (a spec completed on a
/// machine whose event log was rotated away publishes nothing rather than an
/// empty record that would read as "measured, and it took no time").
///
/// # Why this commits LOCALLY and never pushes
///
/// ADR-60 rejected making the store the live event log because that puts a git
/// commit on the drain's hot path. A push would put a NETWORK round trip there
/// instead, and `aida pull`'s auto-bump can complete many specs in one run, so
/// this would be one push per spec. The record is committed locally and the
/// next `aida push` / `aida db sync` carries it — the same "a failed push is
/// not an error" posture the schedule ledger takes, made unconditional.
///
/// A local commit cannot lose a push race, so there is no CAS retry loop here.
/// Two machines that publish the same spec converge when their stores merge:
/// `timings/<SPEC>.yaml` is registered with the store's conflict machinery and
/// resolved by span-set UNION, not last-writer-wins.
///
/// Best-effort by contract, like every other write on the completion path.
// trace:STORY-1480 | ai:claude
pub(crate) fn publish(project_root: &Path, spec: &str) -> Result<bool> {
    let Some(store) = store_root(project_root) else {
        return Ok(false);
    };
    let events = crate::events::read_all(project_root);
    let node = recording_node(&store, project_root);
    let mine = record_to_publish(&events, spec, &node, hostname(), Utc::now());
    if mine.spans.is_empty() {
        return Ok(false);
    }

    // Union with whatever this clone already holds — its own earlier publish,
    // or another machine's that a previous pull brought in.
    let merged = match spec_timing::load(&store, spec) {
        Some(prev) => spec_timing::union(&prev, &mine),
        None => mine,
    };

    if !is_git_checkout(&store) {
        // A fixture store with no git: write the file and stop there.
        spec_timing::save(&store, &merged)
            .with_context(|| format!("writing timing record for {spec}"))?;
        return Ok(true);
    }

    // Never commit onto a store that is mid-rebase or detached (the BUG-1229
    // data-loss class). Skipping loses nothing permanently: the record is
    // re-derivable from the local stream until it is rotated away.
    if let Err(err) = aida_core::git_ops::ensure_store_write_safe(&store) {
        eprintln!("  timing record for {spec} not published: {err}");
        return Ok(false);
    }
    spec_timing::save(&store, &merged)
        .with_context(|| format!("writing timing record for {spec}"))?;
    let rel = spec_timing::record_rel(spec);
    aida_core::git_ops::add(&store, &[rel.as_str()])?;
    // `commit` returning false means nothing was staged — the record already
    // held exactly these spans, which is a successful publish.
    aida_core::git_ops::commit(&store, &format!("timings: {spec}"))?;
    Ok(true)
}

/// Publish and swallow the outcome — the shape the completion path uses, where
/// a timing record must never be able to fail a spec's completion.
// trace:STORY-1480 | ai:claude
pub(crate) fn publish_best_effort(project_root: &Path, spec: &str) {
    if let Err(err) = publish(project_root, spec) {
        eprintln!("  timing record for {spec} not published: {err}");
    }
}

#[cfg(test)]
mod story_1480_publish_tests {
    use super::*;
    use crate::events::Event;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn at(spec: &str, when: &str, kind: EventKind) -> Event {
        let mut ev = Event::new(Some(spec.to_string()), "", kind);
        ev.ts = ts(when);
        ev
    }

    fn entered(idx: i32, slug: &str, attempt: u32) -> EventKind {
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

    fn ended(idx: i32, slug: &str, attempt: u32, outcome: &str) -> EventKind {
        EventKind::PhaseEnded {
            idx,
            slug: slug.into(),
            attempt,
            outcome: outcome.into(),
        }
    }

    /// A drain phase pair becomes one measured work span.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_phase_pair_is_one_measured_span() {
        let evs = vec![
            at("S-1", "2026-09-01T10:00:00Z", entered(1, "implementer", 1)),
            at(
                "S-1",
                "2026-09-01T10:30:00Z",
                ended(1, "implementer", 1, crate::events::PHASE_ADVANCED),
            ),
        ];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].activity, "implementer");
        assert_eq!(spans[0].duration_s, Some(1800));
        assert_eq!(spans[0].node, "n1");
    }

    /// A phase entered but never ended publishes with NO duration — the run
    /// died mid-phase and that time is genuinely unmeasured.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn an_unclosed_phase_is_unmeasured_not_zero() {
        let evs = vec![at("S-1", "2026-09-01T10:00:00Z", entered(3, "reviewer", 1))];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].duration_s, None);
        assert_eq!(spans[0].end, None);
    }

    /// A rework loop yields one span per attempt, so `reviewer #2` is not
    /// collapsed into the first review.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn rework_yields_one_span_per_attempt() {
        let evs = vec![
            at("S-1", "2026-09-01T10:00:00Z", entered(3, "reviewer", 1)),
            at(
                "S-1",
                "2026-09-01T10:10:00Z",
                ended(3, "reviewer", 1, crate::events::PHASE_REENTERED),
            ),
            at("S-1", "2026-09-01T11:00:00Z", entered(3, "reviewer", 2)),
            at(
                "S-1",
                "2026-09-01T11:05:00Z",
                ended(3, "reviewer", 2, crate::events::PHASE_ADVANCED),
            ),
        ];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].attempt, 1);
        assert_eq!(spans[1].attempt, 2);
        assert_eq!(spans[1].duration_s, Some(300));
    }

    /// A park is attributed as `parked` (class wait), which is exactly the time
    /// that used to be reported as unknown.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_park_resume_pair_is_attributed_as_wait() {
        let evs = vec![
            at(
                "S-1",
                "2026-09-01T10:00:00Z",
                EventKind::SpecParked {
                    on: crate::events::PARKED_ON_HUMAN.into(),
                    reason: "ci red".into(),
                    via: "shelve".into(),
                },
            ),
            at(
                "S-1",
                "2026-09-02T10:00:00Z",
                EventKind::SpecRequeued {
                    via: "queue-rework".into(),
                    actor: None,
                    from: "Needs Attention".into(),
                    to: "Approved".into(),
                    cleared_tags: vec![],
                    kept_tags: vec![],
                    parked_since: Some(ts("2026-09-01T10:00:00Z")),
                },
            ),
        ];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].activity, ACTIVITY_PARKED);
        assert_eq!(
            spans[0].class,
            aida_core::spec_timing::SpanClass::Wait,
            "a park must count as wait, not unknown"
        );
        assert_eq!(spans[0].duration_s, Some(86_400));
    }

    /// A park never resumed is still published, as unmeasured wait.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_park_never_resumed_is_published_open() {
        let evs = vec![at(
            "S-1",
            "2026-09-01T10:00:00Z",
            EventKind::SpecParked {
                on: crate::events::PARKED_ON_ADVISOR.into(),
                reason: "design fork".into(),
                via: "punt".into(),
            },
        )];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].duration_s, None);
    }

    /// A self-contained activity span (interactive session, integrate member,
    /// plan, compile) publishes as one measured span.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn an_activity_span_publishes_as_measured() {
        let evs = vec![at(
            "S-1",
            "2026-09-01T12:00:00Z",
            EventKind::ActivitySpan {
                activity: crate::events::ACTIVITY_COMPILE.into(),
                started_at: ts("2026-09-01T11:58:00Z"),
                outcome: crate::events::OUTCOME_COMPLETED.into(),
                detail: None,
            },
        )];
        let spans = spans_for_spec(&evs, "S-1", "n1");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].activity, "compile");
        assert_eq!(spans[0].duration_s, Some(120));
    }

    /// Mail, cron and shift noise is structurally excluded — the fold never
    /// reads those kinds, so they cannot reach the store.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn non_lifecycle_noise_is_never_published() {
        let evs = vec![
            at(
                "S-1",
                "2026-09-01T10:00:00Z",
                EventKind::MailReceived {
                    to: "advisor".into(),
                },
            ),
            at(
                "S-1",
                "2026-09-01T10:01:00Z",
                EventKind::CronJobFired {
                    job: "mailbox-triage".into(),
                    seat: "advisor".into(),
                },
            ),
            at(
                "S-1",
                "2026-09-01T10:02:00Z",
                EventKind::ShiftTick {
                    launched: None,
                    reaped: 0,
                    recovered_stale_pid: None,
                    refused: vec![],
                    breaker: None,
                    escalated: vec![],
                    redriven: vec![],
                    reclassified: vec![],
                    mail_escalated: vec![],
                    redrive_held: None,
                },
            ),
        ];
        assert!(spans_for_spec(&evs, "S-1", "n1").is_empty());
    }

    /// Another spec's events never leak into this spec's record.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn other_specs_events_are_not_published() {
        let evs = vec![
            at("S-2", "2026-09-01T10:00:00Z", entered(1, "implementer", 1)),
            at(
                "S-2",
                "2026-09-01T10:30:00Z",
                ended(1, "implementer", 1, crate::events::PHASE_ADVANCED),
            ),
        ];
        assert!(spans_for_spec(&evs, "S-1", "n1").is_empty());
    }

    /// A spec with nothing measured publishes NO record, rather than an empty
    /// one that would read as "measured, and it took no time".
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_spec_with_no_spans_publishes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida-store/objects")).unwrap();
        assert!(!publish(dir.path(), "S-1").unwrap());
        assert!(aida_core::spec_timing::load(&dir.path().join(".aida-store"), "S-1").is_none());
    }

    /// A publish with no store attached is a clean no-op, never an error on the
    /// completion path.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn publishing_without_a_store_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!publish(dir.path(), "S-1").unwrap());
    }

    /// The end-to-end local shape: a fixture stream plus a fixture (non-git)
    /// store yields a readable record whose spans came only from this spec.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_published_record_is_readable_from_the_store() {
        let _off = crate::test_env::EnvVarGuard::unset(crate::events::EVENTS_DISABLE_ENV);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        for ev in [
            at("S-1", "2026-09-01T10:00:00Z", entered(1, "implementer", 1)),
            at(
                "S-1",
                "2026-09-01T10:30:00Z",
                ended(1, "implementer", 1, crate::events::PHASE_ADVANCED),
            ),
        ] {
            crate::events::emit(dir.path(), &ev);
        }
        assert!(publish(dir.path(), "S-1").unwrap());
        let record = aida_core::spec_timing::load(&store, "S-1").expect("published record");
        assert_eq!(record.spec, "S-1");
        assert_eq!(record.spans.len(), 1);
        assert_eq!(record.published_by.len(), 1);
        assert_eq!(record.totals().get("work"), Some(&(1800, 0)));

        // Publishing again is idempotent: the union key is span content.
        assert!(publish(dir.path(), "S-1").unwrap());
        let again = aida_core::spec_timing::load(&store, "S-1").expect("record");
        assert_eq!(again.spans.len(), 1);
    }
}

/// STORY-1480: print one spec's PUBLISHED timing record — the cross-machine
/// read surface. Deliberately narrow: it reports what the store holds, and
/// nothing derived, interpolated or joined with any other source.
///
/// Three things it states explicitly rather than smoothing over, because each
/// one is the difference between a measurement and a guess:
/// - which node measured each span,
/// - that a span with no duration is UNMEASURED, not zero,
/// - that an interval between two different nodes' spans is not a measurement.
// trace:STORY-1480 | ai:claude
pub(crate) fn report(spec: &str, json: bool) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let Some(store) = store_root(&project_root) else {
        anyhow::bail!("no AIDA store is attached to this project, so no timing record can be read");
    };
    let Some(record) = spec_timing::load(&store, spec) else {
        if json {
            println!(
                "{}",
                serde_json::json!({ "spec": spec, "published": false })
            );
        } else {
            println!(
                "no published timing record for {spec} — it publishes when the spec reaches a \
                 terminal state, so an in-flight spec has none yet, and one completed before \
                 this was instrumented has none either"
            );
        }
        return Ok(());
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }

    println!("timing record: {}", record.spec);
    let nodes = record.nodes();
    println!("measured by:   {}", nodes.join(", "));
    println!();
    for span in &record.spans {
        let dur = match span.duration_s {
            Some(d) => format!("{d}s"),
            None => "unmeasured".to_string(),
        };
        println!(
            "  {:<14} {:<8} {:>12}  node {}  (attempt {})",
            span.activity,
            span.class.as_str(),
            dur,
            span.node,
            span.attempt
        );
    }
    println!();
    for (class, (measured, unmeasured)) in record.totals() {
        if unmeasured > 0 {
            println!("  {class}: {measured}s measured, {unmeasured} span(s) unmeasured");
        } else {
            println!("  {class}: {measured}s measured");
        }
    }
    if nodes.len() > 1 {
        println!();
        println!(
            "  note: spans came from {} machines, whose clocks are independent. Each span's own \
             duration is one machine's measurement; the interval BETWEEN two machines' spans is \
             not.",
            nodes.len()
        );
    }
    Ok(())
}
