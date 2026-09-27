//! Per-spec timing records published to the git-canonical store (ADR-60).
//!
//! # Why this exists
//!
//! The live phase-event stream (`.aida/events.jsonl`) is per-clone, local-only
//! and rotated, so work performed on one machine leaves no evidence on any
//! other. A spec completed on machine X therefore has no measurable cycle time
//! on machine Y: every span reads as `unknown`.
//!
//! ADR-60 keeps the live log local and authoritative for in-flight detail, and
//! publishes ONE small record per spec to the store when the spec reaches a
//! terminal state — the finished span list, filtered to lifecycle activities.
//! Mail, cron and shift-tick noise is never published.
//!
//! # Cross-machine clock contract — read this before adding a field
//!
//! The store offers two merge classes, and this record deliberately sits in
//! the safer one:
//!
//! - **Append-only, id-keyed collections** (a spec's `history`, `comments`,
//!   `processing_record`, the oplog) are merged by UNION on a stable id.
//!   Whether an entry survives does not depend on any clock.
//! - **Scalar fields** are merged last-write-wins on wall-clock `modified_at`
//!   with a content-hash tie-break (BUG-578). Under clock skew the
//!   wall-clock-later write wins even when it is causally earlier. There is no
//!   hybrid logical clock in this codebase — the HLC module was removed rather
//!   than wired in.
//!
//! So a [`TimingRecord`] is a **union-by-id span set**, never a
//! last-write-wins document. A span's [`Span::id`] is a pure function of its
//! own content, so two machines that both publish for the same spec keep both
//! sets and no span can be lost because one machine's clock is behind.
//!
//! What that does and does not buy:
//!
//! - A span's `duration_s` is measured start-to-end **by one host's clock**
//!   ([`Span::node`] names it), so it stays meaningful under skew.
//! - A gap BETWEEN two spans recorded by DIFFERENT nodes is **not a
//!   measurement**. It mixes two clocks and can even be negative.
//!   [`cross_node_gap`] returns such a gap as unmeasured rather than clamping
//!   it and presenting it as a measured wait.
//!
//! trace:STORY-1480 trace:ADR-60 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Directory under the store worktree root holding one record per spec.
pub const TIMINGS_DIR: &str = "timings";

/// Record schema version. Bumped only on a breaking shape change; a reader
/// that does not recognize the version reports the record as unreadable rather
/// than guessing at its meaning.
pub const SCHEMA: u32 = 1;

/// How a span's time should be counted when a cycle-time breakdown adds it up.
///
/// The three-way split is deliberate: `Unknown` is a first-class outcome so a
/// gap no source explains is reported, never folded into a neighbour.
// trace:STORY-1480 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpanClass {
    /// Someone or something was working: implementer, ci, reviewer, merge,
    /// pull, build, plan, integrate.
    Work,
    /// Nothing was progressing the spec: awaiting approval, queued, parked on
    /// a human or the advisor, awaiting merge, between runs.
    Wait,
    /// Time no source accounts for. Reported explicitly.
    Unknown,
}

impl SpanClass {
    /// Stable machine label.
    pub fn as_str(self) -> &'static str {
        match self {
            SpanClass::Work => "work",
            SpanClass::Wait => "wait",
            SpanClass::Unknown => "unknown",
        }
    }
}

/// Classify an activity name. The vocabulary matches the per-spec timeline's
/// span model so a timeline and an aggregate breakdown can consume a published
/// record without a second translation table.
///
/// An activity this table does not know is [`SpanClass::Unknown`] — an
/// unrecognized activity must never be silently counted as work.
// trace:STORY-1480 | ai:claude
pub fn classify_activity(activity: &str) -> SpanClass {
    match activity {
        // Drain pipeline phases, the planning prelude, the in-implementer
        // compile step, an integrate member, and the interactive seats that
        // hold a session worktree.
        "implementer" | "ci" | "reviewer" | "merge" | "pull" | "build" | "plan" | "plan-verify"
        | "compile" | "integrate" | "advisor" | "integrator" => SpanClass::Work,
        "awaiting-approval" | "queued" | "parked" | "shelved" | "awaiting-merge"
        | "between-runs" => SpanClass::Wait,
        _ => SpanClass::Unknown,
    }
}

/// One measured interval in a spec's life.
///
/// `start` and `end` are both stamped by the host named in `node`, so
/// `duration_s` is a single-clock measurement. Comparing a span's `end` to a
/// different node's `start` is NOT a measurement — see [`cross_node_gap`].
// trace:STORY-1480 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Activity name, e.g. `implementer`, `ci`, `parked`.
    pub activity: String,
    /// How the activity's time counts.
    pub class: SpanClass,
    /// Start, on the recording host's clock.
    pub start: DateTime<Utc>,
    /// End, on the SAME host's clock. `None` when the span was never closed
    /// (the run died mid-phase) — an open span is published with no duration
    /// rather than being given an invented end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    /// `end - start` in whole seconds. `None` whenever `end` is `None`, and
    /// also when the pair is non-monotonic (the host's own clock stepped
    /// backwards mid-span) — a negative duration is reported as unmeasured,
    /// never clamped to zero and presented as measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<i64>,
    /// Where the span came from, e.g. `events` or `store-history`.
    pub source: String,
    /// Node id of the host that recorded both stamps.
    pub node: String,
    /// Attempt number when the activity repeated (rework / re-review). `1` for
    /// a first pass.
    #[serde(default = "one")]
    pub attempt: u32,
}

fn one() -> u32 {
    1
}

impl Span {
    /// Build a span, deriving its class and its duration.
    ///
    /// The duration is computed here and nowhere else, so the "negative means
    /// unmeasured" rule cannot be bypassed by a caller filling the field in.
    // trace:STORY-1480 | ai:claude
    pub fn new(
        activity: impl Into<String>,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        source: impl Into<String>,
        node: impl Into<String>,
        attempt: u32,
    ) -> Self {
        let activity = activity.into();
        let class = classify_activity(&activity);
        let duration_s = end.and_then(|e| {
            let secs = (e - start).num_seconds();
            if secs < 0 {
                None
            } else {
                Some(secs)
            }
        });
        Self {
            activity,
            class,
            start,
            end,
            duration_s,
            source: source.into(),
            node: node.into(),
            attempt: attempt.max(1),
        }
    }

    /// The span's stable union key — a pure function of its own content, so
    /// two clones merging the same published record compute the same key and
    /// the union is deterministic regardless of which side runs the merge.
    ///
    /// It is a readable composite rather than a hash so a conflicted record
    /// stays auditable by eye in a store diff.
    // trace:STORY-1480 | ai:claude
    pub fn id(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.node,
            self.source,
            self.activity,
            self.attempt,
            self.start.to_rfc3339(),
            self.end.map(|e| e.to_rfc3339()).unwrap_or_default()
        )
    }
}

/// One machine's act of publishing. Kept as a list, not a scalar, so a record
/// carries WHICH hosts contributed to it instead of the last one silently
/// overwriting the others.
// trace:STORY-1480 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    /// Node id of the publishing clone.
    pub node: String,
    /// Hostname of the publishing clone, best-effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// When it published, on the publishing host's clock.
    pub at: DateTime<Utc>,
    /// How many spans that publish contributed.
    pub spans: usize,
}

/// The published per-spec timing record: `timings/<SPEC-ID>.yaml` on the
/// `aida-store` branch.
// trace:STORY-1480 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingRecord {
    /// Schema version — see [`SCHEMA`].
    pub schema: u32,
    /// Display id of the spec this record describes.
    pub spec: String,
    /// Every span any machine has published for this spec, unioned by
    /// [`Span::id`] and ordered by `(start, id)` for a stable diff.
    #[serde(default)]
    pub spans: Vec<Span>,
    /// Which machines contributed, oldest first.
    #[serde(default)]
    pub published_by: Vec<Publication>,
}

impl TimingRecord {
    /// An empty record for `spec`.
    pub fn new(spec: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA,
            spec: spec.into(),
            spans: Vec::new(),
            published_by: Vec::new(),
        }
    }

    /// Nodes that contributed spans, sorted and deduped.
    pub fn nodes(&self) -> Vec<String> {
        let mut v: Vec<String> = self.spans.iter().map(|s| s.node.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Total measured seconds per class. Spans with no `duration_s` contribute
    /// nothing — an unmeasured span must not be counted as zero work.
    /// The second element of each pair is how many spans of that class were
    /// unmeasured, so a caller can report coverage instead of over-claiming.
    // trace:STORY-1480 | ai:claude
    pub fn totals(&self) -> BTreeMap<&'static str, (i64, usize)> {
        let mut out: BTreeMap<&'static str, (i64, usize)> = BTreeMap::new();
        for span in &self.spans {
            let entry = out.entry(span.class.as_str()).or_insert((0, 0));
            match span.duration_s {
                Some(d) => entry.0 += d,
                None => entry.1 += 1,
            }
        }
        out
    }
}

/// Union two records by [`Span::id`].
///
/// Pure and commutative in survival: `union(a, b)` and `union(b, a)` hold the
/// same span set and the same publication set, so two clones resolving the
/// same conflict converge. Ordering is `(start, id)`, a data-function, not the
/// order the sides were supplied in.
///
/// The schema is the MAXIMUM of the two — a record written by a newer binary
/// is not silently downgraded to this binary's version by the merge.
// trace:STORY-1480 | ai:claude
pub fn union(a: &TimingRecord, b: &TimingRecord) -> TimingRecord {
    let mut spans: BTreeMap<String, Span> = BTreeMap::new();
    for span in a.spans.iter().chain(b.spans.iter()) {
        spans.insert(span.id(), span.clone());
    }
    let mut spans: Vec<Span> = spans.into_values().collect();
    spans.sort_by(|x, y| x.start.cmp(&y.start).then_with(|| x.id().cmp(&y.id())));

    let mut pubs: BTreeMap<String, Publication> = BTreeMap::new();
    for p in a.published_by.iter().chain(b.published_by.iter()) {
        pubs.insert(format!("{}|{}", p.at.to_rfc3339(), p.node), p.clone());
    }
    let published_by: Vec<Publication> = pubs.into_values().collect();

    TimingRecord {
        schema: a.schema.max(b.schema),
        // Both sides describe the same spec (same path); prefer a non-empty id.
        spec: if a.spec.is_empty() {
            b.spec.clone()
        } else {
            a.spec.clone()
        },
        spans,
        published_by,
    }
}

/// The gap between `earlier` ending and `later` starting.
///
/// Returns `Ok(seconds)` only when BOTH spans were recorded by the same node,
/// so the two stamps come off one clock and the difference is a measurement.
/// Otherwise — a different node, a missing end, or a negative result — returns
/// `Err(reason)`: the caller must report the interval as `unknown` and name the
/// reason rather than clamping it and presenting it as a measured wait.
// trace:STORY-1480 | ai:claude
pub fn cross_node_gap(earlier: &Span, later: &Span) -> Result<i64, String> {
    let Some(end) = earlier.end else {
        return Err(format!(
            "`{}` was never closed, so the interval before `{}` is unmeasured",
            earlier.activity, later.activity
        ));
    };
    if earlier.node != later.node {
        return Err(format!(
            "`{}` was recorded on node {} and `{}` on node {}; the interval between them \
             spans two clocks and is unmeasured",
            earlier.activity, earlier.node, later.activity, later.node
        ));
    }
    let secs = (later.start - end).num_seconds();
    if secs < 0 {
        return Err(format!(
            "node {} recorded `{}` starting before `{}` ended; the interval is unmeasured",
            later.node, later.activity, earlier.activity
        ));
    }
    Ok(secs)
}

/// File-system-safe form of a spec id.
fn sanitize(spec: &str) -> String {
    spec.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `timings/<SPEC-ID>.yaml` under the store worktree root.
pub fn record_path(store_root: &Path, spec: &str) -> PathBuf {
    store_root
        .join(TIMINGS_DIR)
        .join(format!("{}.yaml", sanitize(spec)))
}

/// The same path relative to the store root, for `git add`.
pub fn record_rel(spec: &str) -> String {
    format!("{TIMINGS_DIR}/{}.yaml", sanitize(spec))
}

/// Read one spec's published record. Missing, unreadable or malformed → `None`
/// (a record that cannot be read is "not published", never an error on a read
/// path). A record whose `schema` is newer than [`SCHEMA`] also reads as
/// `None`: this binary does not know what its fields mean and must not guess.
// trace:STORY-1480 | ai:claude
pub fn load(store_root: &Path, spec: &str) -> Option<TimingRecord> {
    let text = std::fs::read_to_string(record_path(store_root, spec)).ok()?;
    let record: TimingRecord = serde_yaml::from_str(&text).ok()?;
    if record.schema > SCHEMA {
        return None;
    }
    Some(record)
}

/// Write one record file. No git — the caller owns add/commit/push.
// trace:STORY-1480 | ai:claude
pub fn save(store_root: &Path, record: &TimingRecord) -> std::io::Result<()> {
    let path = record_path(store_root, &record.spec);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml = serde_yaml::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::write_atomic(&path, yaml.as_bytes())
}

#[cfg(test)]
mod story_1480_spec_timing_tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// A duration is only reported when one host measured both ends.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn negative_and_open_spans_report_no_duration() {
        let open = Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            None,
            "events",
            "1",
            1,
        );
        assert_eq!(open.duration_s, None);

        let backwards = Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T09:00:00Z")),
            "events",
            "1",
            1,
        );
        assert_eq!(
            backwards.duration_s, None,
            "a backwards clock must not produce a clamped, measured-looking zero"
        );

        let good = Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T10:05:00Z")),
            "events",
            "1",
            1,
        );
        assert_eq!(good.duration_s, Some(300));
    }

    /// An activity the table does not know is never counted as work.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn unknown_activity_classifies_unknown() {
        assert_eq!(classify_activity("implementer"), SpanClass::Work);
        assert_eq!(classify_activity("parked"), SpanClass::Wait);
        assert_eq!(classify_activity("mail-received"), SpanClass::Unknown);
        assert_eq!(classify_activity("compile"), SpanClass::Work);
        assert_eq!(classify_activity("plan-verify"), SpanClass::Work);
    }

    /// Two machines publishing the same spec keep BOTH span sets, and both
    /// sides converge on the same result whichever runs the merge — the store
    /// has no causal clock, so survival must not depend on a wall clock.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn union_is_order_independent_and_loses_nothing() {
        let mut x = TimingRecord::new("STORY-1");
        x.spans.push(Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T11:00:00Z")),
            "events",
            "nodeX",
            1,
        ));
        x.published_by.push(Publication {
            node: "nodeX".into(),
            host: Some("x".into()),
            at: ts("2026-09-01T11:00:01Z"),
            spans: 1,
        });

        let mut y = TimingRecord::new("STORY-1");
        // A node whose clock is an hour BEHIND still contributes its span.
        y.spans.push(Span::new(
            "reviewer",
            ts("2026-09-01T09:30:00Z"),
            Some(ts("2026-09-01T09:40:00Z")),
            "events",
            "nodeY",
            1,
        ));
        y.published_by.push(Publication {
            node: "nodeY".into(),
            host: Some("y".into()),
            at: ts("2026-09-01T09:40:01Z"),
            spans: 1,
        });

        let a = union(&x, &y);
        let b = union(&y, &x);
        assert_eq!(a, b, "union must converge regardless of which side merges");
        assert_eq!(a.spans.len(), 2, "no span may be dropped");
        assert_eq!(a.nodes(), vec!["nodeX".to_string(), "nodeY".to_string()]);
        assert_eq!(a.published_by.len(), 2);
    }

    /// Re-publishing the same spans is idempotent: the union key is content.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn republishing_the_same_spans_is_idempotent() {
        let mut x = TimingRecord::new("STORY-1");
        x.spans.push(Span::new(
            "ci",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T10:02:00Z")),
            "events",
            "nodeX",
            1,
        ));
        let merged = union(&x, &x.clone());
        assert_eq!(merged.spans.len(), 1);
    }

    /// A rework pass is a distinct span even when it repeats the activity.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn attempt_distinguishes_rework_from_a_duplicate() {
        let first = Span::new(
            "reviewer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T10:10:00Z")),
            "events",
            "n",
            1,
        );
        let second = Span::new(
            "reviewer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T10:10:00Z")),
            "events",
            "n",
            2,
        );
        assert_ne!(first.id(), second.id());
    }

    /// A gap between two nodes' spans is refused as a measurement and names
    /// both nodes, instead of being clamped into a plausible-looking wait.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_gap_across_two_nodes_is_not_a_measurement() {
        let a = Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T11:00:00Z")),
            "events",
            "nodeA",
            1,
        );
        let b = Span::new(
            "reviewer",
            ts("2026-09-01T12:00:00Z"),
            Some(ts("2026-09-01T12:10:00Z")),
            "events",
            "nodeB",
            1,
        );
        let err = cross_node_gap(&a, &b).expect_err("two clocks cannot be differenced");
        assert!(err.contains("nodeA"), "{err}");
        assert!(err.contains("nodeB"), "{err}");

        let same_node = Span::new(
            "reviewer",
            ts("2026-09-01T12:00:00Z"),
            Some(ts("2026-09-01T12:10:00Z")),
            "events",
            "nodeA",
            1,
        );
        assert_eq!(cross_node_gap(&a, &same_node), Ok(3600));
    }

    /// An unmeasured span is reported as unmeasured, not summed as zero.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn totals_separate_measured_from_unmeasured() {
        let mut r = TimingRecord::new("STORY-1");
        r.spans.push(Span::new(
            "implementer",
            ts("2026-09-01T10:00:00Z"),
            Some(ts("2026-09-01T10:10:00Z")),
            "events",
            "n",
            1,
        ));
        r.spans.push(Span::new(
            "build",
            ts("2026-09-01T11:00:00Z"),
            None,
            "events",
            "n",
            1,
        ));
        let totals = r.totals();
        assert_eq!(totals.get("work"), Some(&(600, 1)));
    }

    /// A record from a newer schema reads as absent rather than misread.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_future_schema_record_is_not_guessed_at() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = TimingRecord::new("STORY-1");
        r.schema = SCHEMA + 1;
        save(dir.path(), &r).unwrap();
        assert!(load(dir.path(), "STORY-1").is_none());

        r.schema = SCHEMA;
        save(dir.path(), &r).unwrap();
        assert!(load(dir.path(), "STORY-1").is_some());
    }

    /// A spec id can never escape the timings directory.
    // trace:STORY-1480 | ai:claude
    #[test]
    fn a_hostile_spec_id_cannot_escape_the_timings_dir() {
        // Only `/` is replaced; a `.` is kept, so the result is still ONE
        // filename under `timings/` with no traversable component.
        assert_eq!(
            record_rel("../../etc/passwd"),
            "timings/.._.._etc_passwd.yaml"
        );
        assert_eq!(record_path(Path::new("/s"), "a/b").components().count(), 4);
        let dir = tempfile::tempdir().unwrap();
        let p = record_path(dir.path(), "../x");
        assert!(p.starts_with(dir.path().join(TIMINGS_DIR)));
    }
}
