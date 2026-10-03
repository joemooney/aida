//! `aida metrics cycle-time` — the aggregate sibling of `aida history <SPEC>
//! --timeline` (STORY-1478): every spec completed in a window, its span model
//! rebuilt from the same two local sources (store transitions + drain feed),
//! rolled up into per-activity percentiles, the overall work/wait/unknown
//! split, rework signals, and the slowest specs with their dominant span.
//!
//! The per-spec model's defining rule carries over: nothing is interpolated,
//! so the aggregate's `unknown` share is real unaccounted time, never noise
//! folded into a labelled bucket. Forge timestamps are deliberately not
//! fetched here — one `gh` call per credited PR across a whole window would
//! make the view minutes-slow and network-dependent; CI rework signals come
//! from the local `CiTerminal` drain events instead.
// trace:STORY-1479 | ai:claude

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;
use std::path::Path;

use crate::events::EventKind;
use crate::history_timeline::{
    build_timeline, fmt_dur, is_status, markers_from_touches, SpanClass, StoreMarkerKind, Timeline,
    TimelineInput,
};

/// Frozen wire version of the aggregate view.
// trace:STORY-1479 | ai:claude
pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const VIEW_NAME: &str = "metrics-cycle-time";

/// The span label the shelve path produces (`history_timeline`'s
/// `shelve_candidates` and the store's Needs Attention wait both emit it);
/// its total is the "total shelved time" rework signal.
const PARKED_LABEL: &str = "parked";

// ---------------------------------------------------------------------------
// Per-spec digest — the pure aggregation input
// ---------------------------------------------------------------------------

/// One included spec, digested to exactly what the aggregate needs. Pure
/// data, so every arithmetic test drives [`aggregate`] with literal numbers.
#[derive(Debug, Clone)]
pub(crate) struct SpecCycle {
    pub(crate) id: String,
    pub(crate) title: String,
    /// `(base activity, class, duration_s)` per span, the repeat suffix
    /// (`ci #2`) already stripped so attempts aggregate under one activity.
    pub(crate) spans: Vec<(String, SpanClass, f64)>,
    pub(crate) work_s: f64,
    pub(crate) wait_s: f64,
    pub(crate) unknown_s: f64,
    pub(crate) elapsed_s: f64,
    pub(crate) incomplete: bool,
    /// Local `CiTerminal` verdicts for this spec: green / red counts.
    pub(crate) ci_green: usize,
    pub(crate) ci_red: usize,
    /// Distinct `PhaseEntered` events per phase, over the spec's whole life.
    pub(crate) reviewer_passes: usize,
    pub(crate) implementer_passes: usize,
    pub(crate) shelves: usize,
}

/// `ci #2` → `ci`; labels without a numeric `#N` suffix pass through
/// untouched (an activity legitimately containing `#` is not mangled).
// trace:STORY-1479 | ai:claude
pub(crate) fn base_activity(label: &str) -> &str {
    if let Some((head, tail)) = label.rsplit_once(" #") {
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
            return head;
        }
    }
    label
}

/// Digest one built timeline + its spec's drain-feed rework counters into
/// the aggregate's input row.
// trace:STORY-1479 | ai:claude
pub(crate) fn digest(
    timeline: &Timeline,
    events: &[crate::history_timeline::DrainRecord],
) -> SpecCycle {
    let mut ci_green = 0usize;
    let mut ci_red = 0usize;
    let mut reviewer_passes = 0usize;
    let mut implementer_passes = 0usize;
    let mut shelves = 0usize;
    for r in events {
        match &r.ev.kind {
            EventKind::CiTerminal { green } => {
                if *green {
                    ci_green += 1;
                } else {
                    ci_red += 1;
                }
            }
            EventKind::PhaseEntered { slug, .. } => match slug.to_ascii_lowercase().as_str() {
                "reviewer" => reviewer_passes += 1,
                "implementer" => implementer_passes += 1,
                _ => {}
            },
            EventKind::SpecShelved { .. } => shelves += 1,
            _ => {}
        }
    }
    SpecCycle {
        id: timeline.id.clone(),
        title: timeline.title.clone(),
        spans: timeline
            .spans
            .iter()
            .map(|s| {
                (
                    base_activity(&s.activity).to_string(),
                    s.class,
                    s.duration_s(),
                )
            })
            .collect(),
        work_s: timeline.totals.work,
        wait_s: timeline.totals.wait,
        unknown_s: timeline.totals.unknown,
        elapsed_s: timeline.totals.elapsed,
        incomplete: timeline.incomplete,
        ci_green,
        ci_red,
        reviewer_passes,
        implementer_passes,
        shelves,
    }
}

// ---------------------------------------------------------------------------
// Aggregate model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActivityStat {
    pub(crate) activity: String,
    pub(crate) class: SpanClass,
    pub(crate) count: usize,
    pub(crate) p50_s: f64,
    pub(crate) p90_s: f64,
    pub(crate) total_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SlowSpec {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) elapsed_s: f64,
    pub(crate) dominant_activity: String,
    /// Dominant activity's share of the spec's elapsed window, in percent.
    pub(crate) dominant_pct: f64,
}

/// A rate kept as `numerator / denominator` so a consumer can see what a
/// percentage is a percentage OF; a zero denominator renders as "no data",
/// never as 0%.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rate {
    pub(crate) num: usize,
    pub(crate) den: usize,
}

impl Rate {
    pub(crate) fn pct(&self) -> Option<f64> {
        if self.den == 0 {
            None
        } else {
            Some(self.num as f64 / self.den as f64 * 100.0)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ReworkSignals {
    /// Red CI verdicts over all CI verdicts (local `CiTerminal` events).
    pub(crate) ci_failure: Rate,
    /// Specs with more than one reviewer pass over specs with at least one.
    pub(crate) reviewer_bounce: Rate,
    /// Mean `implementer` phase entries per included spec. `None` with no
    /// included specs.
    pub(crate) implementer_passes_avg: Option<f64>,
    pub(crate) shelves: usize,
    pub(crate) shelved_total_s: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct CycleReport {
    pub(crate) since: DateTime<Utc>,
    pub(crate) until: Option<DateTime<Utc>>,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) completed_in_window: usize,
    pub(crate) included: usize,
    /// `(spec id, reason)` per excluded spec, in id order.
    pub(crate) excluded: Vec<(String, String)>,
    pub(crate) activities: Vec<ActivityStat>,
    pub(crate) work_s: f64,
    pub(crate) wait_s: f64,
    pub(crate) unknown_s: f64,
    pub(crate) elapsed_s: f64,
    pub(crate) rework: ReworkSignals,
    pub(crate) slowest: Vec<SlowSpec>,
    /// How many included timelines carry partial coverage — their own view
    /// flags it, so the aggregate says so too rather than reading as exact.
    pub(crate) partial_coverage: usize,
    pub(crate) notes: Vec<String>,
}

/// Nearest-rank percentile: the value at rank `ceil(q·n)` (1-based) of the
/// sorted samples. `p50` of `[1,2,3,4]` is `2`, `p90` of ten values is the
/// 9th — always a real observed duration, never an interpolated one,
/// matching the span model's no-interpolation rule.
// trace:STORY-1479 | ai:claude
pub(crate) fn percentile(durations: &[f64], q: f64) -> f64 {
    if durations.is_empty() {
        return 0.0;
    }
    let mut sorted = durations.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("span durations are finite"));
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Share of `part` in `whole`, in percent; `None` when there is no whole.
fn share_pct(part: f64, whole: f64) -> Option<f64> {
    if whole <= 0.0 {
        None
    } else {
        Some(part / whole * 100.0)
    }
}

fn class_order(class: SpanClass) -> u8 {
    match class {
        SpanClass::Work => 0,
        SpanClass::Wait => 1,
        SpanClass::Unknown => 2,
    }
}

/// Pure rollup. Everything the renders show is computed here, so the
/// arithmetic is testable with literal fixture numbers.
// trace:STORY-1479 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn aggregate(
    included: &[SpecCycle],
    excluded: Vec<(String, String)>,
    completed_in_window: usize,
    since: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    as_of: DateTime<Utc>,
    slowest_n: usize,
    notes: Vec<String>,
) -> CycleReport {
    // Per-activity duration samples, keyed by (class order, activity) so the
    // rendered table groups work, then wait, then unknown.
    let mut buckets: BTreeMap<(u8, String), Vec<f64>> = BTreeMap::new();
    for spec in included {
        for (activity, class, dur) in &spec.spans {
            buckets
                .entry((class_order(*class), activity.clone()))
                .or_default()
                .push(*dur);
        }
    }
    let mut activities: Vec<ActivityStat> = buckets
        .into_iter()
        .map(|((class_key, activity), durs)| ActivityStat {
            activity,
            class: match class_key {
                0 => SpanClass::Work,
                1 => SpanClass::Wait,
                _ => SpanClass::Unknown,
            },
            count: durs.len(),
            p50_s: percentile(&durs, 0.50),
            p90_s: percentile(&durs, 0.90),
            total_s: durs.iter().sum(),
        })
        .collect();
    // Within a class, biggest time sink first.
    activities.sort_by(|a, b| {
        class_order(a.class)
            .cmp(&class_order(b.class))
            .then(b.total_s.partial_cmp(&a.total_s).expect("finite totals"))
            .then_with(|| a.activity.cmp(&b.activity))
    });

    let work_s: f64 = included.iter().map(|s| s.work_s).sum();
    let wait_s: f64 = included.iter().map(|s| s.wait_s).sum();
    let unknown_s: f64 = included.iter().map(|s| s.unknown_s).sum();
    let elapsed_s: f64 = included.iter().map(|s| s.elapsed_s).sum();

    let ci_red: usize = included.iter().map(|s| s.ci_red).sum();
    let ci_runs: usize = included.iter().map(|s| s.ci_green + s.ci_red).sum();
    let reviewed = included.iter().filter(|s| s.reviewer_passes >= 1).count();
    let bounced = included.iter().filter(|s| s.reviewer_passes > 1).count();
    let implementer_passes_avg = if included.is_empty() {
        None
    } else {
        Some(
            included.iter().map(|s| s.implementer_passes).sum::<usize>() as f64
                / included.len() as f64,
        )
    };
    let shelves: usize = included.iter().map(|s| s.shelves).sum();
    let shelved_total_s: f64 = included
        .iter()
        .flat_map(|s| &s.spans)
        .filter(|(a, _, _)| a == PARKED_LABEL)
        .map(|(_, _, d)| d)
        .sum();

    let mut slow: Vec<&SpecCycle> = included.iter().collect();
    slow.sort_by(|a, b| {
        b.elapsed_s
            .partial_cmp(&a.elapsed_s)
            .expect("finite elapsed")
            .then_with(|| a.id.cmp(&b.id))
    });
    let slowest = slow
        .into_iter()
        .take(slowest_n)
        .map(|s| {
            // Dominant span: the single biggest time sink in this spec's
            // window, unknown included — "51% unknown" is exactly the kind
            // of thing this list exists to surface.
            let mut by_activity: BTreeMap<&str, f64> = BTreeMap::new();
            for (activity, _, dur) in &s.spans {
                *by_activity.entry(activity.as_str()).or_default() += *dur;
            }
            let (dominant_activity, dominant_s) = by_activity
                .into_iter()
                .max_by(|a, b| {
                    a.1.partial_cmp(&b.1)
                        .expect("finite totals")
                        .then(b.0.cmp(a.0))
                })
                .unwrap_or(("(no spans)", 0.0));
            SlowSpec {
                id: s.id.clone(),
                title: s.title.clone(),
                elapsed_s: s.elapsed_s,
                dominant_activity: dominant_activity.to_string(),
                dominant_pct: share_pct(dominant_s, s.elapsed_s).unwrap_or(0.0),
            }
        })
        .collect();

    let mut excluded = excluded;
    excluded.sort();

    CycleReport {
        since,
        until,
        as_of,
        completed_in_window,
        included: included.len(),
        excluded,
        activities,
        work_s,
        wait_s,
        unknown_s,
        elapsed_s,
        rework: ReworkSignals {
            ci_failure: Rate {
                num: ci_red,
                den: ci_runs,
            },
            reviewer_bounce: Rate {
                num: bounced,
                den: reviewed,
            },
            implementer_passes_avg,
            shelves,
            shelved_total_s,
        },
        slowest,
        partial_coverage: included.iter().filter(|s| s.incomplete).count(),
        notes,
    }
}

// ---------------------------------------------------------------------------
// Rendering — one model, three surfaces
// ---------------------------------------------------------------------------

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn fmt_pct(v: Option<f64>) -> String {
    match v {
        Some(p) => format!("{:.0}%", p),
        None => "—".into(),
    }
}

/// Exclusion reasons folded to `count× reason`, most frequent first.
fn excluded_reason_counts(excluded: &[(String, String)]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, reason) in excluded {
        *counts.entry(reason.as_str()).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(r, c)| (r.to_string(), c))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

// trace:STORY-1479 | ai:claude
pub(crate) fn render_human(r: &CycleReport) -> String {
    use colored::Colorize;
    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        "Cycle time — specs completed in the window".bold()
    ));
    out.push_str(&format!(
        "{}\n",
        format!(
            "Window: {} → {}",
            rfc3339(r.since),
            r.until.map(rfc3339).unwrap_or_else(|| "now".into())
        )
        .dimmed()
    ));
    out.push_str(&format!(
        "Completed in window: {} · included {} · excluded {}\n",
        r.completed_in_window,
        r.included,
        r.excluded.len()
    ));
    for (reason, count) in excluded_reason_counts(&r.excluded) {
        out.push_str(&format!("  excluded {}× — {}\n", count, reason));
    }
    if r.included == 0 {
        out.push_str("\n  (nothing to aggregate)\n");
        for n in &r.notes {
            out.push_str(&format!("  - {}\n", n));
        }
        return out;
    }

    out.push_str(&format!(
        "\nOverall: work {} ({}) · wait {} ({}) · {} {} ({}) — {} elapsed across {} spec(s)\n",
        fmt_dur(r.work_s),
        fmt_pct(share_pct(r.work_s, r.elapsed_s)),
        fmt_dur(r.wait_s),
        fmt_pct(share_pct(r.wait_s, r.elapsed_s)),
        "unknown".red().bold(),
        fmt_dur(r.unknown_s),
        fmt_pct(share_pct(r.unknown_s, r.elapsed_s)),
        fmt_dur(r.elapsed_s),
        r.included,
    ));
    if r.partial_coverage > 0 {
        out.push_str(&format!(
            "{}\n",
            format!(
                "  {} included timeline(s) have partial coverage; the totals understate them",
                r.partial_coverage
            )
            .dimmed()
        ));
    }

    out.push('\n');
    out.push_str(&format!(
        "  {:<20} {:<8} {:>5}  {:>9}  {:>9}  {:>10}\n",
        "activity", "class", "count", "p50", "p90", "total"
    ));
    for a in &r.activities {
        let class_cell = format!("{:<8}", a.class.as_str());
        let painted = match a.class {
            SpanClass::Work => class_cell.green().to_string(),
            SpanClass::Wait => class_cell.yellow().to_string(),
            SpanClass::Unknown => class_cell.red().to_string(),
        };
        out.push_str(&format!(
            "  {:<20} {} {:>5}  {:>9}  {:>9}  {:>10}\n",
            a.activity,
            painted,
            a.count,
            fmt_dur(a.p50_s),
            fmt_dur(a.p90_s),
            fmt_dur(a.total_s),
        ));
    }

    out.push_str(&format!(
        "\nRework: CI failure {} ({} red / {} runs) · reviewer bounce {} ({} of {} reviewed specs) · avg implementer passes {} · {} shelve(s) totalling {}\n",
        fmt_pct(r.rework.ci_failure.pct()),
        r.rework.ci_failure.num,
        r.rework.ci_failure.den,
        fmt_pct(r.rework.reviewer_bounce.pct()),
        r.rework.reviewer_bounce.num,
        r.rework.reviewer_bounce.den,
        r.rework
            .implementer_passes_avg
            .map(|v| format!("{:.1}", v))
            .unwrap_or_else(|| "—".into()),
        r.rework.shelves,
        fmt_dur(r.rework.shelved_total_s),
    ));

    if !r.slowest.is_empty() {
        out.push_str("\nSlowest:\n");
        for s in &r.slowest {
            out.push_str(&format!(
                "  {:<12} {:>9}   {:.0}% {}\n",
                s.id,
                fmt_dur(s.elapsed_s),
                s.dominant_pct,
                s.dominant_activity,
            ));
        }
    }

    if !r.notes.is_empty() {
        out.push_str("\nCoverage:\n");
        for n in &r.notes {
            out.push_str(&format!("  - {}\n", n));
        }
    }
    out
}

const ACTIVITY_FIELDS: &[&str] = &["activity", "class", "count", "p50_s", "p90_s", "total_s"];
const SLOWEST_FIELDS: &[&str] = &["id", "elapsed_s", "dominant_activity", "dominant_pct"];
const EXCLUDED_FIELDS: &[&str] = &["id", "reason"];

// trace:STORY-1479 | ai:claude
pub(crate) fn render_toon(r: &CycleReport) -> String {
    let mut out = format!("view: {}\n", VIEW_NAME);
    out.push_str(&format!("schema_version: {}\n", SCHEMA_VERSION));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("since", &rfc3339(r.since))
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("until", &r.until.map(rfc3339).unwrap_or_default())
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar("as_of", &rfc3339(r.as_of))
    ));
    out.push_str(&format!("completed_in_window: {}\n", r.completed_in_window));
    out.push_str(&format!("included: {}\n", r.included));
    out.push_str(&format!("partial_coverage: {}\n", r.partial_coverage));
    out.push_str(&format!("work_s: {:.3}\n", r.work_s));
    out.push_str(&format!("wait_s: {:.3}\n", r.wait_s));
    out.push_str(&format!("unknown_s: {:.3}\n", r.unknown_s));
    out.push_str(&format!("elapsed_s: {:.3}\n", r.elapsed_s));
    let acts: Vec<Vec<String>> = r
        .activities
        .iter()
        .map(|a| {
            vec![
                a.activity.clone(),
                a.class.as_str().to_string(),
                a.count.to_string(),
                format!("{:.3}", a.p50_s),
                format!("{:.3}", a.p90_s),
                format!("{:.3}", a.total_s),
            ]
        })
        .collect();
    out.push_str(&crate::toon::table_raw(
        "activities",
        ACTIVITY_FIELDS,
        &acts,
    ));
    out.push('\n');
    out.push_str(&format!("ci_red: {}\n", r.rework.ci_failure.num));
    out.push_str(&format!("ci_runs: {}\n", r.rework.ci_failure.den));
    out.push_str(&format!(
        "reviewer_bounced: {}\n",
        r.rework.reviewer_bounce.num
    ));
    out.push_str(&format!(
        "reviewer_reviewed: {}\n",
        r.rework.reviewer_bounce.den
    ));
    out.push_str(&format!(
        "{}\n",
        crate::toon::scalar(
            "implementer_passes_avg",
            &r.rework
                .implementer_passes_avg
                .map(|v| format!("{:.3}", v))
                .unwrap_or_default()
        )
    ));
    out.push_str(&format!("shelves: {}\n", r.rework.shelves));
    out.push_str(&format!(
        "shelved_total_s: {:.3}\n",
        r.rework.shelved_total_s
    ));
    let slow: Vec<Vec<String>> = r
        .slowest
        .iter()
        .map(|s| {
            vec![
                s.id.clone(),
                format!("{:.3}", s.elapsed_s),
                s.dominant_activity.clone(),
                format!("{:.1}", s.dominant_pct),
            ]
        })
        .collect();
    out.push_str(&crate::toon::table_raw("slowest", SLOWEST_FIELDS, &slow));
    out.push('\n');
    let exc: Vec<Vec<String>> = r
        .excluded
        .iter()
        .map(|(id, reason)| vec![id.clone(), reason.replace('\n', " ")])
        .collect();
    out.push_str(&crate::toon::table_raw("excluded", EXCLUDED_FIELDS, &exc));
    out.push('\n');
    let notes: Vec<Vec<String>> = r.notes.iter().map(|n| vec![n.replace('\n', " ")]).collect();
    out.push_str(&crate::toon::table_raw("notes", &["note"], &notes));
    out.push('\n');
    out
}

// trace:STORY-1479 | ai:claude
pub(crate) fn render_json(r: &CycleReport) -> JsonValue {
    json!({
        "schema_version": SCHEMA_VERSION,
        "view": VIEW_NAME,
        "window": {
            "since": rfc3339(r.since),
            "until": r.until.map(rfc3339),
        },
        "as_of": rfc3339(r.as_of),
        "completed_in_window": r.completed_in_window,
        "included": r.included,
        "partial_coverage": r.partial_coverage,
        "excluded": {
            "count": r.excluded.len(),
            "specs": r.excluded.iter().map(|(id, reason)| json!({
                "id": id,
                "reason": reason,
            })).collect::<Vec<_>>(),
        },
        "totals_s": {
            "work": r.work_s,
            "wait": r.wait_s,
            "unknown": r.unknown_s,
            "elapsed": r.elapsed_s,
        },
        "shares_pct": {
            "work": share_pct(r.work_s, r.elapsed_s),
            "wait": share_pct(r.wait_s, r.elapsed_s),
            "unknown": share_pct(r.unknown_s, r.elapsed_s),
        },
        "activities": r.activities.iter().map(|a| json!({
            "activity": a.activity,
            "class": a.class.as_str(),
            "count": a.count,
            "p50_s": a.p50_s,
            "p90_s": a.p90_s,
            "total_s": a.total_s,
        })).collect::<Vec<_>>(),
        "rework": {
            "ci_red": r.rework.ci_failure.num,
            "ci_runs": r.rework.ci_failure.den,
            "ci_failure_pct": r.rework.ci_failure.pct(),
            "reviewer_bounced": r.rework.reviewer_bounce.num,
            "reviewer_reviewed": r.rework.reviewer_bounce.den,
            "reviewer_bounce_pct": r.rework.reviewer_bounce.pct(),
            "implementer_passes_avg": r.rework.implementer_passes_avg,
            "shelves": r.rework.shelves,
            "shelved_total_s": r.rework.shelved_total_s,
        },
        "slowest": r.slowest.iter().map(|s| json!({
            "id": s.id,
            "title": if s.title.is_empty() { JsonValue::Null } else { json!(s.title) },
            "elapsed_s": s.elapsed_s,
            "dominant_activity": s.dominant_activity,
            "dominant_pct": s.dominant_pct,
        })).collect::<Vec<_>>(),
        "notes": r.notes,
    })
}

// ---------------------------------------------------------------------------
// Batched evidence transport — the aggregate's performance seams
// ---------------------------------------------------------------------------

/// One long-lived `git cat-file --batch` serving every blob the marker
/// decoder asks for. The per-spec timeline view spawns a `git show` per
/// blob, which is fine for one spec; across a whole window that is tens of
/// thousands of process spawns and minutes of wall clock, where one batch
/// pipe answers in seconds.
// trace:STORY-1479 | ai:claude
struct BatchBlobs {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl BatchBlobs {
    fn new(store_path: &Path) -> Result<Self> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("git")
            .arg("-C")
            .arg(store_path)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    /// `<rev>:<path>` → blob body, or `None` for a missing/unreadable
    /// object (a filing commit has no parent-side blob, exactly as the
    /// per-spec view's `git show` returns an error there).
    fn get(&mut self, rev: &str, path: &str) -> Option<String> {
        use std::io::{BufRead, Read, Write};
        writeln!(self.stdin, "{rev}:{path}").ok()?;
        self.stdin.flush().ok()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header).ok()?;
        let fields: Vec<&str> = header.trim_end().split(' ').collect();
        // `<oid> <type> <size>` on success; `<input> missing` (or another
        // two-field error) when the revision or path does not resolve.
        let (kind, size) = match fields.as_slice() {
            [_oid, kind, size] => (*kind, size.parse::<usize>().ok()?),
            _ => return None,
        };
        let mut buf = vec![0u8; size + 1]; // body + trailing newline
        self.stdout.read_exact(&mut buf).ok()?;
        buf.pop();
        if kind != "blob" {
            return None;
        }
        String::from_utf8(buf).ok()
    }
}

impl Drop for BatchBlobs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every spec's drain records in ONE pass over the live feed and its
/// archive, bucketed by (upper-cased) spec id — the aggregate's counterpart
/// of `collect_drain_events`, which re-reads both files per spec. Same
/// admission semantics: unparseable lines are counted, not dropped
/// silently; exact duplicates across live feed and archive collapse.
// trace:STORY-1479 | ai:claude
fn drain_events_by_spec(
    project_root: &Path,
) -> (
    BTreeMap<String, Vec<crate::history_timeline::DrainRecord>>,
    Vec<String>,
) {
    use crate::events::Event as FeedEvent;
    let mut by_spec: BTreeMap<String, Vec<crate::history_timeline::DrainRecord>> = BTreeMap::new();
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
                    let Some(spec) = ev.spec.as_deref() else {
                        continue;
                    };
                    by_spec.entry(spec.to_ascii_uppercase()).or_default().push(
                        crate::history_timeline::DrainRecord {
                            ev,
                            origin: format!("{}:{}", name, i + 1),
                        },
                    );
                }
                Err(_) => bad += 1,
            }
        }
    }
    if !any_file {
        notes.push(
            "no local drain event feed is readable here, so nothing is attributed to a drain \
             phase; time drains may have spent working shows as unknown"
                .to_string(),
        );
    }
    if bad > 0 {
        notes.push(format!(
            "{} drain event line(s) could not be read; any boundary they held is missing, so \
             some spans may be unknown that were in fact instrumented",
            bad
        ));
    }
    for records in by_spec.values_mut() {
        records.sort_by(|a, b| a.ev.ts.cmp(&b.ev.ts).then_with(|| a.origin.cmp(&b.origin)));
        records.dedup_by(|a, b| a.ev == b.ev);
    }
    (by_spec, notes)
}

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

/// `aida metrics cycle-time`.
// trace:STORY-1479 | ai:claude
pub(crate) fn run(
    store_path: &Path,
    since_raw: &str,
    until_raw: Option<&str>,
    type_filter: Option<&str>,
    tags: &[String],
    slowest_n: usize,
    output: crate::history::HistoryOutput,
) -> Result<()> {
    let now = Utc::now();
    let since_at = crate::history::parse_history_bound(since_raw, "--since", now, &chrono::Local)?;
    let until_at = until_raw
        .map(|u| crate::history::parse_history_bound(u, "--until", now, &chrono::Local))
        .transpose()?;
    if let Some(u) = until_at {
        if u < since_at {
            anyhow::bail!(
                "--until resolves earlier than --since; that window can never match anything"
            );
        }
    }
    let wanted_type = type_filter.map(crate::parse_type).transpose()?;

    let objects_root = store_path.join("objects");
    let all = aida_core::object_store::load_all_objects(&objects_root)?;
    let candidates: Vec<&aida_core::Requirement> = all
        .iter()
        .filter(|r| r.status == aida_core::RequirementStatus::Completed)
        .filter(|r| r.spec_id.is_some())
        .filter(|r| wanted_type.as_ref().map_or(true, |t| &r.req_type == t))
        .filter(|r| {
            tags.iter().all(|want| {
                r.tags
                    .iter()
                    .any(|have| have.eq_ignore_ascii_case(want.trim()))
            })
        })
        .collect();

    // ONE whole-branch `--name-status` walk replaces a per-spec `git log
    // --full-history -- <path>` + per-commit `git show` pair: on a store
    // with tens of thousands of commits, a per-candidate walk costs minutes
    // of wall clock where this walk costs one git process. It yields every
    // candidate's full touch list (a timeline needs the whole lifecycle,
    // not just the window) AND the cheap touched-in-window pre-filter; the
    // expensive blob decoding below runs only for specs that pass it.
    // trace:STORY-1479 | ai:claude
    let candidate_paths: BTreeMap<String, String> = candidates
        .iter()
        .filter_map(|r| {
            let spec_id = r.spec_id.as_deref()?;
            let rel = aida_core::object_store::relative_object_path(spec_id).ok()?;
            Some((rel, spec_id.to_string()))
        })
        .collect();
    let log = crate::history::run_git(
        store_path,
        &[
            "log".into(),
            "--pretty=format:%H%x09%aI%x09%ae".into(),
            "--name-status".into(),
            "--no-renames".into(),
        ],
    )?;
    // The walk is newest-first; each spec's touches are reversed afterwards
    // so the shared decoder sees them oldest-first, exactly as the per-spec
    // timeline view feeds it.
    let mut touches: BTreeMap<String, Vec<(crate::history::CommitMeta, String, String)>> =
        BTreeMap::new();
    let mut current: Option<crate::history::CommitMeta> = None;
    for line in log.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let first = line.split('\t').next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            current = crate::history::parse_log_line(line);
            continue;
        }
        let Some(commit) = current.as_ref() else {
            continue;
        };
        let mut parts = line.splitn(2, '\t');
        let status = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim();
        if candidate_paths.contains_key(path) {
            touches.entry(path.to_string()).or_default().push((
                commit.clone(),
                status.to_string(),
                path.to_string(),
            ));
        }
    }
    for list in touches.values_mut() {
        list.reverse();
    }
    // Pre-filter bounds widened a day each side: markers carry `%aI` author
    // dates and the authoritative in-window check below uses the decoded
    // completion marker itself, so this only has to be a superset.
    let prefilter_since = since_at - chrono::Duration::days(1);
    let prefilter_until = until_at.map(|u| u + chrono::Duration::days(1));

    let project_root = store_path.parent().unwrap_or(store_path);
    let mut completed_in_window = 0usize;
    let mut included: Vec<SpecCycle> = Vec::new();
    let mut excluded: Vec<(String, String)> = Vec::new();
    let mut notes = vec![
        "forge timestamps are not consulted by this aggregate view; CI signals come from the \
         local drain feed"
            .to_string(),
    ];
    let (feed_by_spec, feed_notes) = drain_events_by_spec(project_root);
    notes.extend(feed_notes);
    let mut blobs = BatchBlobs::new(store_path)?;

    for req in candidates {
        let spec_id = req.spec_id.as_deref().expect("filtered to Some above");
        let Ok(rel) = aida_core::object_store::relative_object_path(spec_id) else {
            continue;
        };
        let Some(spec_touches) = touches.get(&rel) else {
            continue;
        };
        let touched_in_window = spec_touches.iter().any(|(c, _, _)| {
            DateTime::parse_from_rfc3339(&c.iso_timestamp)
                .map(|t| {
                    let t = t.with_timezone(&Utc);
                    t >= prefilter_since && prefilter_until.map_or(true, |u| t <= u)
                })
                .unwrap_or(false)
        });
        if !touched_in_window {
            continue;
        }
        let mut spec_notes = Vec::new();
        let mut fetch = |rev: &str, path: &str| blobs.get(rev, path);
        let markers = markers_from_touches(spec_touches, &mut fetch, &mut spec_notes);
        if markers.is_empty() {
            excluded.push((
                spec_id.to_string(),
                "no decodable lifecycle record in the store history".to_string(),
            ));
            continue;
        }
        // The spec's CURRENT status is Completed (filtered above), so its
        // completion instant is the last recorded arrival at Completed.
        let completed_at = markers.iter().rev().find_map(|m| match &m.kind {
            StoreMarkerKind::Status { to, .. } if is_status(to, "Completed") => Some(m.at),
            StoreMarkerKind::Filed { status } if is_status(status, "Completed") => Some(m.at),
            _ => None,
        });
        let Some(completed_at) = completed_at else {
            // Completed now, but with no reachable record of WHEN (compacted
            // or rewritten history) — it cannot be dated into any window.
            excluded.push((
                spec_id.to_string(),
                "completed, but no completion transition is reachable in the store history, so \
                 it cannot be dated into a window"
                    .to_string(),
            ));
            continue;
        };
        if completed_at < since_at || until_at.is_some_and(|u| completed_at > u) {
            continue;
        }
        completed_in_window += 1;

        let events: Vec<crate::history_timeline::DrainRecord> = feed_by_spec
            .get(&spec_id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default();
        if events.is_empty() {
            excluded.push((
                spec_id.to_string(),
                "no drain events on this machine (work likely ran elsewhere)".to_string(),
            ));
            continue;
        }
        let tl_notes = spec_notes;
        let timeline = match build_timeline(TimelineInput {
            id: spec_id.to_string(),
            title: req.title.clone(),
            current_status: Some(req.status.to_string()),
            store: markers,
            events: events.clone(),
            checks: Vec::new(),
            notes: tl_notes,
            as_of: now,
        }) {
            Ok(t) => t,
            Err(e) => {
                excluded.push((spec_id.to_string(), format!("timeline unbuildable: {e}")));
                continue;
            }
        };
        if timeline.totals.elapsed <= 0.0 {
            excluded.push((
                spec_id.to_string(),
                "no measurable window (every observed boundary falls at one instant)".to_string(),
            ));
            continue;
        }
        included.push(digest(&timeline, &events));
    }

    let report = aggregate(
        &included,
        excluded,
        completed_in_window,
        since_at,
        until_at,
        now,
        slowest_n,
        notes,
    );

    match output {
        crate::history::HistoryOutput::Json => {
            println!("{}", serde_json::to_string_pretty(&render_json(&report))?);
        }
        crate::history::HistoryOutput::Toon => print!("{}", render_toon(&report)),
        _ => print!("{}", render_human(&report)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — fixture numbers only. No store, no git, no feed files.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event as Ev, EventKind};
    use crate::history_timeline::DrainRecord;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .unwrap_or_else(|e| panic!("bad fixture timestamp {s}: {e}"))
            .with_timezone(&Utc)
    }

    fn spec(id: &str, spans: &[(&str, SpanClass, f64)]) -> SpecCycle {
        let work_s = spans
            .iter()
            .filter(|(_, c, _)| *c == SpanClass::Work)
            .map(|(_, _, d)| d)
            .sum();
        let wait_s = spans
            .iter()
            .filter(|(_, c, _)| *c == SpanClass::Wait)
            .map(|(_, _, d)| d)
            .sum();
        let unknown_s = spans
            .iter()
            .filter(|(_, c, _)| *c == SpanClass::Unknown)
            .map(|(_, _, d)| d)
            .sum();
        SpecCycle {
            id: id.into(),
            title: format!("{id} title"),
            spans: spans
                .iter()
                .map(|(a, c, d)| (a.to_string(), *c, *d))
                .collect(),
            work_s,
            wait_s,
            unknown_s,
            elapsed_s: work_s + wait_s + unknown_s,
            incomplete: false,
            ci_green: 0,
            ci_red: 0,
            reviewer_passes: 0,
            implementer_passes: 0,
            shelves: 0,
        }
    }

    fn report(included: &[SpecCycle]) -> CycleReport {
        aggregate(
            included,
            Vec::new(),
            included.len(),
            at("2026-09-01T00:00:00Z"),
            None,
            at("2026-10-01T00:00:00Z"),
            5,
            Vec::new(),
        )
    }

    // --- percentile arithmetic (nearest-rank) ---------------------------

    #[test]
    fn story_1479_percentile_nearest_rank() {
        let v = [4.0, 1.0, 3.0, 2.0];
        assert_eq!(percentile(&v, 0.50), 2.0); // ceil(0.5*4)=2nd of [1,2,3,4]
        assert_eq!(percentile(&v, 0.90), 4.0); // ceil(0.9*4)=4th
        let ten: Vec<f64> = (1..=10).map(|i| i as f64 * 10.0).collect();
        assert_eq!(percentile(&ten, 0.50), 50.0); // 5th of 10
        assert_eq!(percentile(&ten, 0.90), 90.0); // 9th of 10
        assert_eq!(percentile(&[7.5], 0.50), 7.5);
        assert_eq!(percentile(&[7.5], 0.90), 7.5);
        assert_eq!(percentile(&[], 0.50), 0.0);
    }

    // --- activity grouping + per-activity stats --------------------------

    #[test]
    fn story_1479_activities_group_across_specs_and_strip_repeat_suffix() {
        // `ci #2` must land in the same bucket as `ci`.
        assert_eq!(base_activity("ci #2"), "ci");
        assert_eq!(base_activity("awaiting approval"), "awaiting approval");
        assert_eq!(base_activity("phase #x"), "phase #x"); // non-numeric: kept

        let a = spec(
            "SPEC-1",
            &[
                ("ci", SpanClass::Work, 100.0),
                ("ci", SpanClass::Work, 300.0),
                ("queued", SpanClass::Wait, 50.0),
            ],
        );
        let b = spec(
            "SPEC-2",
            &[
                ("ci", SpanClass::Work, 200.0),
                ("unknown", SpanClass::Unknown, 400.0),
            ],
        );
        let r = report(&[a, b]);
        let ci = r
            .activities
            .iter()
            .find(|x| x.activity == "ci")
            .expect("ci row");
        assert_eq!(ci.count, 3);
        assert_eq!(ci.p50_s, 200.0); // 2nd of [100,200,300]
        assert_eq!(ci.p90_s, 300.0); // 3rd of [100,200,300]
        assert_eq!(ci.total_s, 600.0);
        // Order: work rows before wait rows before unknown rows.
        let classes: Vec<&str> = r.activities.iter().map(|a| a.class.as_str()).collect();
        let first_wait = classes.iter().position(|c| *c == "wait").unwrap();
        let first_unknown = classes.iter().position(|c| *c == "unknown").unwrap();
        assert!(classes.iter().position(|c| *c == "work").unwrap() < first_wait);
        assert!(first_wait < first_unknown);
    }

    // --- share arithmetic -------------------------------------------------

    #[test]
    fn story_1479_overall_shares_sum_from_spec_totals() {
        let a = spec(
            "SPEC-1",
            &[
                ("implementer", SpanClass::Work, 600.0),
                ("queued", SpanClass::Wait, 300.0),
                ("unknown", SpanClass::Unknown, 100.0),
            ],
        );
        let b = spec(
            "SPEC-2",
            &[
                ("implementer", SpanClass::Work, 400.0),
                ("unknown", SpanClass::Unknown, 600.0),
            ],
        );
        let r = report(&[a, b]);
        assert_eq!(r.work_s, 1000.0);
        assert_eq!(r.wait_s, 300.0);
        assert_eq!(r.unknown_s, 700.0);
        assert_eq!(r.elapsed_s, 2000.0);
        let j = render_json(&r);
        assert_eq!(j["shares_pct"]["work"].as_f64().unwrap(), 50.0);
        assert_eq!(j["shares_pct"]["wait"].as_f64().unwrap(), 15.0);
        assert_eq!(j["shares_pct"]["unknown"].as_f64().unwrap(), 35.0);
        // Shares cover the window exactly — nothing redistributed.
        assert_eq!(r.work_s + r.wait_s + r.unknown_s, r.elapsed_s);
    }

    // --- rework signals ----------------------------------------------------

    #[test]
    fn story_1479_rework_rates_carry_their_denominators() {
        let mut a = spec("SPEC-1", &[("implementer", SpanClass::Work, 10.0)]);
        a.ci_green = 3;
        a.ci_red = 1;
        a.reviewer_passes = 2; // bounced
        a.implementer_passes = 2;
        a.shelves = 1;
        let mut b = spec("SPEC-2", &[("parked", SpanClass::Wait, 500.0)]);
        b.ci_green = 1;
        b.ci_red = 0;
        b.reviewer_passes = 1; // clean single pass
        b.implementer_passes = 1;
        let mut c = spec("SPEC-3", &[("implementer", SpanClass::Work, 20.0)]);
        c.reviewer_passes = 0; // never reviewed: not in the bounce denominator
        c.implementer_passes = 0;
        c.shelves = 2;

        let r = report(&[a, b, c]);
        assert_eq!(r.rework.ci_failure.num, 1);
        assert_eq!(r.rework.ci_failure.den, 5);
        assert_eq!(r.rework.ci_failure.pct().unwrap(), 20.0);
        assert_eq!(r.rework.reviewer_bounce.num, 1);
        assert_eq!(r.rework.reviewer_bounce.den, 2);
        assert_eq!(r.rework.reviewer_bounce.pct().unwrap(), 50.0);
        assert_eq!(r.rework.implementer_passes_avg.unwrap(), 1.0); // (2+1+0)/3
        assert_eq!(r.rework.shelves, 3);
        assert_eq!(r.rework.shelved_total_s, 500.0);
    }

    #[test]
    fn story_1479_zero_denominator_rates_are_no_data_not_zero_pct() {
        let a = spec("SPEC-1", &[("implementer", SpanClass::Work, 10.0)]);
        let r = report(&[a]);
        assert_eq!(r.rework.ci_failure.pct(), None);
        assert_eq!(r.rework.reviewer_bounce.pct(), None);
        let j = render_json(&r);
        assert!(j["rework"]["ci_failure_pct"].is_null());
        assert!(j["rework"]["reviewer_bounce_pct"].is_null());
        assert!(render_human(&r).contains("CI failure — (0 red / 0 runs)"));
    }

    // --- slowest specs + dominant span --------------------------------------

    #[test]
    fn story_1479_slowest_sorted_with_dominant_span() {
        let a = spec(
            "BUG-1",
            &[
                ("parked", SpanClass::Wait, 510.0),
                ("implementer", SpanClass::Work, 490.0),
            ],
        );
        let b = spec(
            "BUG-2",
            &[
                ("implementer", SpanClass::Work, 100.0),
                ("unknown", SpanClass::Unknown, 1900.0),
            ],
        );
        let c = spec("BUG-3", &[("ci", SpanClass::Work, 10.0)]);
        let r = aggregate(
            &[a, b, c],
            Vec::new(),
            3,
            at("2026-09-01T00:00:00Z"),
            None,
            at("2026-10-01T00:00:00Z"),
            2, // cap below the included count
            Vec::new(),
        );
        assert_eq!(r.slowest.len(), 2);
        assert_eq!(r.slowest[0].id, "BUG-2");
        assert_eq!(r.slowest[0].dominant_activity, "unknown");
        assert_eq!(r.slowest[0].dominant_pct, 95.0);
        assert_eq!(r.slowest[1].id, "BUG-1");
        assert_eq!(r.slowest[1].dominant_activity, "parked");
        assert_eq!(r.slowest[1].dominant_pct, 51.0);
        let human = render_human(&r);
        assert!(
            human.contains("51% parked"),
            "human render says the dominant span:\n{human}"
        );
    }

    // --- exclusions ----------------------------------------------------------

    #[test]
    fn story_1479_exclusions_reported_with_reason_counts() {
        let a = spec("SPEC-1", &[("implementer", SpanClass::Work, 10.0)]);
        let r = aggregate(
            &[a],
            vec![
                (
                    "SPEC-9".into(),
                    "no drain events on this machine (work likely ran elsewhere)".into(),
                ),
                (
                    "SPEC-8".into(),
                    "no drain events on this machine (work likely ran elsewhere)".into(),
                ),
                ("SPEC-7".into(), "timeline unbuildable: boom".into()),
            ],
            4,
            at("2026-09-01T00:00:00Z"),
            None,
            at("2026-10-01T00:00:00Z"),
            5,
            Vec::new(),
        );
        assert_eq!(r.completed_in_window, 4);
        assert_eq!(r.included, 1);
        assert_eq!(r.excluded.len(), 3);
        // Sorted by id for a stable wire order.
        assert_eq!(r.excluded[0].0, "SPEC-7");
        let human = render_human(&r);
        assert!(human.contains("excluded 2× — no drain events on this machine"));
        assert!(human.contains("excluded 1× — timeline unbuildable: boom"));
        let j = render_json(&r);
        assert_eq!(j["excluded"]["count"], 3);
        assert_eq!(j["completed_in_window"], 4);
    }

    // --- digest: timeline + feed → SpecCycle ---------------------------------

    #[test]
    fn story_1479_digest_counts_rework_events_and_strips_span_suffixes() {
        use crate::history_timeline::{StoreMarker, StoreMarkerKind};
        let rec = |ts: &str, line: usize, kind: EventKind| DrainRecord {
            ev: Ev {
                ts: at(ts),
                spec: Some("SPEC-1".into()),
                run_uuid: "runA".into(),
                seat: None,
                kind,
            },
            origin: format!("events.jsonl:{line}"),
        };
        let phase = |idx: i32, slug: &str, attempt: u32| EventKind::PhaseEntered {
            idx,
            slug: slug.into(),
            vendor: None,
            seat: None,
            model: None,
            effort: None,
            attempt,
        };
        let store = vec![
            StoreMarker {
                at: at("2026-09-20T10:00:00Z"),
                sha: "aaaa1111".into(),
                kind: StoreMarkerKind::Filed {
                    status: "Draft".into(),
                },
            },
            StoreMarker {
                at: at("2026-09-20T13:00:00Z"),
                sha: "cccc3333".into(),
                kind: StoreMarkerKind::Status {
                    from: "In Progress".into(),
                    to: "Completed".into(),
                },
            },
        ];
        let events = vec![
            rec("2026-09-20T10:30:00Z", 1, phase(1, "implementer", 1)),
            rec("2026-09-20T11:00:00Z", 2, phase(2, "ci", 1)),
            rec(
                "2026-09-20T11:10:00Z",
                3,
                EventKind::CiTerminal { green: false },
            ),
            rec("2026-09-20T11:20:00Z", 4, phase(1, "implementer", 2)),
            rec("2026-09-20T11:40:00Z", 5, phase(2, "ci", 2)),
            rec(
                "2026-09-20T11:50:00Z",
                6,
                EventKind::CiTerminal { green: true },
            ),
            rec("2026-09-20T12:00:00Z", 7, phase(3, "reviewer", 1)),
        ];
        let timeline = build_timeline(TimelineInput {
            id: "SPEC-1".into(),
            title: "fixture".into(),
            current_status: Some("Completed".into()),
            store,
            events: events.clone(),
            checks: Vec::new(),
            notes: Vec::new(),
            as_of: at("2026-09-21T00:00:00Z"),
        })
        .expect("fixture timeline builds");
        let d = digest(&timeline, &events);
        assert_eq!(d.ci_green, 1);
        assert_eq!(d.ci_red, 1);
        assert_eq!(d.implementer_passes, 2);
        assert_eq!(d.reviewer_passes, 1);
        assert_eq!(d.shelves, 0);
        // Both ci attempts digest under one base activity.
        let ci_spans: Vec<&(String, SpanClass, f64)> =
            d.spans.iter().filter(|(a, _, _)| a == "ci").collect();
        assert_eq!(ci_spans.len(), 2);
        // Work + wait + unknown still covers the whole envelope.
        assert!((d.work_s + d.wait_s + d.unknown_s - d.elapsed_s).abs() < 1e-6);
    }

    // --- render contracts -----------------------------------------------------

    #[test]
    fn story_1479_toon_and_json_carry_schema_and_view() {
        let r = report(&[spec("SPEC-1", &[("ci", SpanClass::Work, 5.0)])]);
        let toon = render_toon(&r);
        assert!(toon.starts_with("view: metrics-cycle-time\nschema_version: 1\n"));
        let j = render_json(&r);
        assert_eq!(j["view"], "metrics-cycle-time");
        assert_eq!(j["schema_version"], 1);
        assert_eq!(j["included"], 1);
    }
}
