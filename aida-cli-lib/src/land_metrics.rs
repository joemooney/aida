//! Observational landing history. Never used by a gate.
// trace:TASK-3813 | ai:codex
use super::{Pr, Row};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const LOG: &str = ".aida/land/readiness.jsonl";
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Event {
    at: DateTime<Utc>,
    pr: u64,
    head: String,
    old: Option<String>,
    new: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    states: BTreeMap<u64, (String, String)>,
}
fn read_events(root: &Path) -> Result<Vec<Event>> {
    let path = root.join(LOG);
    match std::fs::read_to_string(&path) {
        Ok(s) => s
            .lines()
            .map(|l| serde_json::from_str(l).context("invalid READY history"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
// Explicitly unlock the open file description before closing it: another
// thread may fork while we hold it, retaining an inherited descriptor.
struct RecordingLock(std::fs::File);
impl Drop for RecordingLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
/// The cursor is disposable; the append-only log is the evidence. Recover any
/// append made before a crash could replace the cursor, under one short lock.
pub(super) fn record(root: &Path, changes: &[Pr], rows: &[Row], at: DateTime<Utc>) -> Result<()> {
    use fs2::FileExt;
    let dir = root.join(".aida/land");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("readiness.lock"))?;
    lock.try_lock_exclusive()
        .context("readiness recorder busy; retry the pass")?;
    let _lock = RecordingLock(lock);
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(root.join(LOG))?;
    let mut cursor: Cursor = std::fs::read(dir.join("readiness-cursor.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let len = log.metadata()?.len();
    if cursor.offset > len {
        bail!("READY history was truncated");
    }
    log.seek(SeekFrom::Start(cursor.offset))?;
    let mut tail = String::new();
    log.read_to_string(&mut tail)?;
    for line in tail.lines() {
        let event: Event = serde_json::from_str(line).context("invalid READY history tail")?;
        if event.pr != 0 {
            cursor.states.insert(event.pr, (event.head, event.new));
        }
    }
    let mut events = Vec::new();
    // A marker retains the recording start even if the first pass is empty.
    if len == 0 {
        events.push(Event {
            at,
            pr: 0,
            head: String::new(),
            old: None,
            new: "recording-start".into(),
        });
    }
    let mut present = BTreeSet::new();
    for row in rows {
        let pr = changes
            .iter()
            .find(|p| p.id == row.pr)
            .context("row missing change")?;
        present.insert(pr.id);
        let next = (pr.head.clone(), row.state.label().to_string());
        if cursor.states.get(&pr.id) != Some(&next) {
            events.push(Event {
                at,
                pr: pr.id,
                head: pr.head.clone(),
                old: cursor.states.get(&pr.id).map(|s| s.1.clone()),
                new: next.1.clone(),
            });
            cursor.states.insert(pr.id, next);
        }
    }
    for (&pr, (head, state)) in cursor.states.iter_mut() {
        if !present.contains(&pr) && state != "absent" {
            events.push(Event {
                at,
                pr,
                head: head.clone(),
                old: Some(state.clone()),
                new: "absent".into(),
            });
            *state = "absent".into();
        }
    }
    if events.is_empty() && cursor.offset == len {
        return Ok(());
    }
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, &event)?;
        bytes.push(b'\n');
    }
    log.write_all(&bytes)?;
    log.sync_data()?;
    cursor.offset = log.metadata()?.len();
    let temp = dir.join("readiness-cursor.tmp");
    std::fs::write(&temp, serde_json::to_vec(&cursor)?)?;
    std::fs::rename(temp, dir.join("readiness-cursor.json"))?;
    Ok(())
}
#[derive(Clone, Debug)]
struct Merge {
    pr: u64,
    at: DateTime<Utc>,
}
#[derive(Clone, Debug)]
struct Review {
    pr: u64,
    at: DateTime<Utc>,
    approved: bool,
    changes: bool,
    vendor: String,
    spec_type: String,
}
#[derive(Clone)]
struct Interval {
    pr: u64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    source: &'static str,
}
const EXACT: &str = "READY event log";
const APPROX: &str = "approx (no READY record)";
const MIXED: &str = "mixed (approval estimate + READY event log)";
fn calculate(
    events: &[Event],
    merges: &[Merge],
    reviews: &[Review],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Value {
    let began = events.first().map(|e| e.at).unwrap_or(until);
    let mut intervals = Vec::new();
    let mut active: BTreeMap<u64, DateTime<Utc>> = BTreeMap::new();
    for event in events.iter().filter(|e| e.pr != 0 && e.at <= until) {
        if let Some(start) = active.remove(&event.pr) {
            intervals.push(Interval {
                pr: event.pr,
                start,
                end: event.at,
                source: EXACT,
            });
        }
        if event.new == "READY" {
            active.insert(event.pr, event.at);
        }
    }
    for (pr, start) in active {
        intervals.push(Interval {
            pr,
            start,
            end: until,
            source: EXACT,
        });
    }
    // Only the pre-recording portion may use approval as a READY proxy.
    let mut approved = BTreeMap::new();
    let mut historical: Vec<_> = reviews
        .iter()
        .filter(|r| r.at < began && r.at <= until)
        .collect();
    historical.sort_by_key(|r| r.at);
    for review in historical {
        if review.approved {
            approved.entry(review.pr).or_insert(review.at);
            continue;
        }
        if let Some(start) = approved.remove(&review.pr) {
            intervals.push(Interval {
                pr: review.pr,
                start,
                end: review.at,
                source: APPROX,
            });
        }
    }
    for (pr, start) in approved {
        let end = merges
            .iter()
            .find(|m| m.pr == pr)
            .map(|m| m.at)
            .unwrap_or(until)
            .min(began)
            .min(until);
        if start < end {
            intervals.push(Interval {
                pr,
                start,
                end,
                source: APPROX,
            });
        }
    }
    // A merged PR stops being READY at the merge, even before the next pass.
    for interval in &mut intervals {
        if let Some(m) = merges
            .iter()
            .find(|m| m.pr == interval.pr && m.at >= interval.start)
        {
            interval.end = interval.end.min(m.at);
        }
    }
    let mut hours = Vec::new();
    let mut hour = DateTime::<Utc>::from_timestamp(since.timestamp().div_euclid(3600) * 3600, 0)
        .expect("a bounded reporting window has a representable hour");
    while hour < until {
        let end = hour + Duration::hours(1);
        let start_in_window = hour.max(since);
        let end_in_window = end.min(until);
        let ready: BTreeSet<_> = intervals
            .iter()
            .filter(|i| i.start < end_in_window && i.end > start_in_window)
            .map(|i| (i.pr, i.source))
            .collect();
        let count = merges
            .iter()
            .filter(|m| m.at >= start_in_window && m.at < end_in_window)
            .count();
        hours.push(json!({"hour": hour, "merges": count, "source": "main git history", "ready": ready.iter().map(|(pr,source)| json!({"pr":pr,"source":source,"merge_happened":count>0})).collect::<Vec<_>>() }));
        hour = end;
    }
    // Preserve hourly provenance, but join contiguous estimated/exact segments
    // for wait summaries. A gap or a blocking observation must not be bridged.
    let mut wait_intervals = intervals.clone();
    wait_intervals.sort_by_key(|i| (i.pr, i.start, i.end));
    let mut joined: Vec<Interval> = Vec::new();
    for interval in wait_intervals {
        if let Some(prior) = joined.last_mut() {
            if prior.pr == interval.pr
                && prior.end == interval.start
                && prior.source == APPROX
                && interval.source == EXACT
                && interval.start == began
            {
                prior.end = interval.end;
                prior.source = MIXED;
                continue;
            }
        }
        joined.push(interval);
    }
    let waits = [EXACT, APPROX, MIXED].map(|source| {
        let mut landed = Vec::new();
        let mut waiting = Vec::new();
        for i in joined
            .iter()
            .filter(|i| i.source == source && i.end > since && i.start < until)
        {
            let merge = merges
                .iter()
                .find(|m| m.pr == i.pr && m.at == i.end && m.at >= since && m.at < until);
            let secs = (i.end - i.start).num_seconds();
            if merge.is_some() {
                landed.push(json!({"pr":i.pr,"seconds":secs}));
            }
            if secs > 7200 && (merge.is_some() || i.end == until) {
                waiting.push(json!({"pr":i.pr,"seconds":secs,"merged":merge.is_some()}));
            }
        }
        let longest = landed.into_iter().max_by_key(|v| v["seconds"].as_i64());
        json!({"source":source,"longest_ready_to_merge":longest,"over_2h":waiting})
    });
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    let mut vendors: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut missing = Vec::new();
    for m in merges.iter().filter(|m| m.at >= since && m.at < until) {
        let approval = reviews
            .iter()
            .filter(|r| r.pr == m.pr && r.approved && r.at <= m.at)
            .max_by_key(|r| r.at);
        if let Some(a) = approval {
            let rounds = reviews
                .iter()
                .filter(|r| r.pr == m.pr && r.changes && r.at < a.at)
                .count();
            vendors.entry(a.vendor.clone()).or_default().push(rounds);
            for spec_type in a.spec_type.split('+') {
                groups
                    .entry((a.vendor.clone(), spec_type.to_string()))
                    .or_default()
                    .push(rounds);
            }
        } else {
            missing.push(m.pr);
        }
    }
    let rounds: Vec<_> = groups
        .into_iter()
        .map(|((vendor, spec_type), samples)| {
            let mut summary = round_summary(vendor, samples);
            summary["spec_type"] = json!(spec_type);
            summary
        })
        .collect();
    let by_vendor: Vec<_> = vendors
        .into_iter()
        .map(|(vendor, samples)| round_summary(vendor, samples))
        .collect();
    json!({"since":since,"until":until,"recording_began":events.first().map(|e|e.at),"hours":hours,"ready_waits":waits,"review_rounds":rounds,"review_rounds_by_vendor":by_vendor,"review_rounds_unavailable_prs":missing,"review_rounds_source":"verdict history"})
}

// trace:TASK-3817 | ai:codex
fn round_summary(vendor: String, mut samples: Vec<usize>) -> Value {
    samples.sort_unstable();
    let count = samples.len();
    let median = (samples[(count - 1) / 2] as f64 + samples[count / 2] as f64) / 2.;
    let mean = samples.iter().sum::<usize>() as f64 / count as f64;
    json!({"vendor":vendor,"prs":count,"mean":mean,"median":median,"source":"verdict history"})
}

// The recorder label's suffix is generated by review_recorded_by_from;
// only parse that suffix, never an arbitrary agent display name.
fn vendor(r: &serde_json::Map<String, Value>) -> String {
    if let Some(v) = r.get("vendor").and_then(Value::as_str) {
        return v.into();
    }
    let label = r.get("recorded_by").and_then(Value::as_str).unwrap_or("");
    if label == crate::REVIEW_OPERATOR_RECORDED_BY {
        return "human".into();
    }
    let suffix = label
        .rsplit_once(" (")
        .map(|(_, s)| s.trim_end_matches(')'))
        .unwrap_or(label);
    let parts: Vec<_> = suffix.split_whitespace().collect();
    // trace:TASK-3813 | ai:codex — use the shared identity registry so
    // reporting does not introduce a separate vendor vocabulary.
    if parts.len() == 3 && parts[2] == "seat" {
        if let Some(profile) = aida_core::vendors::PROFILES
            .iter()
            .find(|profile| profile.aliases.contains(&parts[0]))
        {
            return profile.launch_name.into();
        }
    }
    "unknown".into()
}
type Rounds = BTreeMap<String, (Review, BTreeSet<String>)>;
fn add_rounds(
    rounds: &mut Rounds,
    pr: u64,
    r: &serde_json::Map<String, Value>,
    spec: Option<&str>,
    fallback: Option<&str>,
) -> Result<()> {
    if crate::gate::review_verdict::delivery_pending(&Value::Object(r.clone())) {
        return Ok(());
    }
    if let Some(history) = r.get("rounds") {
        for round in history
            .as_array()
            .context("verdict rounds must be an array")?
        {
            add_rounds(
                rounds,
                pr,
                round.as_object().context("invalid verdict round")?,
                spec,
                fallback,
            )?;
        }
    }
    let kind = super::VerdictKind::parse(r.get("verdict").and_then(Value::as_str).unwrap_or(""));
    if !kind.approves() && !kind.blocks_done() {
        return Ok(());
    }
    let at = if r.contains_key("recorded_at") {
        timestamp(r)?
    } else {
        DateTime::parse_from_rfc3339(fallback.context("verdict missing recorded_at")?)?
            .with_timezone(&Utc)
    };
    let identity = format!("{at}|{:?}", crate::gate::review_verdict::recording_key(r));
    let spec_type = spec
        .or_else(|| r.get("spec").and_then(Value::as_str))
        .and_then(|s| s.split_once('-'))
        .map(|(prefix, _)| prefix.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let round = rounds.entry(identity).or_insert_with(|| {
        (
            Review {
                pr,
                at,
                approved: kind.approves(),
                changes: kind == super::VerdictKind::RequestChanges,
                vendor: vendor(r),
                spec_type: String::new(),
            },
            BTreeSet::new(),
        )
    });
    if spec_type != "unknown" {
        round.1.remove("unknown");
        round.1.insert(spec_type);
    } else if round.1.is_empty() {
        round.1.insert(spec_type);
    }
    Ok(())
}
fn timestamp(record: &serde_json::Map<String, Value>) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(
        record
            .get("recorded_at")
            .and_then(Value::as_str)
            .context("verdict missing recorded_at")?,
    )?
    .with_timezone(&Utc))
}
// trace:TASK-3813 | ai:codex — share the CLI grammar and pin one clock.
pub(crate) fn parse_window_at<Tz: chrono::TimeZone>(
    window: &str,
    now: DateTime<Utc>,
    tz: &Tz,
) -> Result<DateTime<Utc>> {
    let lookback = crate::queue_cmd::parse_lookback_at(window, "--since", now, tz)?;
    if lookback <= Duration::zero() || lookback > Duration::days(3660) {
        bail!("--since must be in the past and within 3660d");
    }
    Ok(now - lookback)
}
// trace:TASK-3817 | ai:codex
pub(crate) fn window_bounds_at<Tz: chrono::TimeZone>(
    since: &str,
    until: Option<&str>,
    now: DateTime<Utc>,
    tz: &Tz,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let end = match until {
        Some(raw) => crate::queue_cmd::parse_time_bound_at(raw, "--until", now, tz)?,
        None => now,
    };
    if end > now {
        bail!("--until is in the future; the reporting window has not ended");
    }
    let start = parse_window_at(since, end, tz)?;
    if now - start > Duration::days(3660) {
        bail!("--since must be within 3660d of now");
    }
    Ok((start, end))
}

pub(super) fn run(start: &Path, since: &str, until: Option<&str>, json_format: bool) -> Result<()> {
    let (since, until) = window_bounds_at(since, until, Utc::now(), &chrono::Local)?;
    let root = crate::review_watch::forge::main_root(start)?;
    // Read all main history so a later merge can close a pre-window interval.
    let text = super::git_text(
        &root,
        &["log", "main", "--first-parent", "--format=%cI%x09%s"],
    )?;
    let re = regex::Regex::new(r"(?:\(#([0-9]+)\)$|^Merge pull request #([0-9]+)\b)")?;
    let mut merges = Vec::new();
    for line in text.lines() {
        let Some((at, subject)) = line.split_once('\t') else {
            continue;
        };
        if let Some(c) = re.captures(subject) {
            merges.push(Merge {
                pr: c
                    .get(1)
                    .or_else(|| c.get(2))
                    .context("PR number")?
                    .as_str()
                    .parse()?,
                at: DateTime::parse_from_rfc3339(at)?.with_timezone(&Utc),
            });
        }
    }
    let events = read_events(&root)?;
    let mut prs: BTreeSet<_> = merges.iter().map(|m| m.pr).collect();
    prs.extend(events.iter().filter(|e| e.pr != 0).map(|e| e.pr));
    let store = crate::gate::ledger::store_root(&root)?;
    let ledger_dir = crate::gate::ledger::ledger_dir(&store);
    match std::fs::read_dir(&ledger_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if let Some(key) = entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(crate::gate::ledger::ChangeKey::parse)
                {
                    if key.forge == crate::gate::ledger::ChangeForge::Pr {
                        prs.insert(key.number);
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let legacy_dir = crate::gate::review_verdict::verdict_path(&root, "x")
        .parent()
        .context("verdict directory")?
        .to_path_buf();
    match std::fs::read_dir(legacy_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if let Some(key) = entry
                    .path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(crate::gate::ledger::ChangeKey::parse)
                {
                    if key.forge == crate::gate::ledger::ChangeForge::Pr {
                        prs.insert(key.number);
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut reviews = Vec::new();
    let _discovery = crate::gate::ledger::checked::DiscoveryScope::enter();
    let mut discovery =
        crate::gate::ledger::checked::discover(&store).map_err(|e| anyhow::anyhow!("{}", e.0))?;
    let mut budget = Default::default();
    for pr in prs {
        let records = if let Some(d) = &mut discovery {
            d.change_events(
                crate::gate::ledger::ChangeKey {
                    forge: crate::gate::ledger::ChangeForge::Pr,
                    number: pr,
                },
                &mut budget,
            )
            .map_err(|e| anyhow::anyhow!("{}", e.0))?
        } else {
            Vec::new()
        };
        // One recording is often copied under every covered spec. Count the
        // act once, retaining all its spec types for the group label.
        let mut rounds: BTreeMap<String, (Review, BTreeSet<String>)> = BTreeMap::new();
        for (r, spec, fallback) in
            crate::gate::review_verdict::verdict_history(&root, &format!("PR-{pr}"), &records)
                .map_err(|e| anyhow::anyhow!(e))?
        {
            add_rounds(&mut rounds, pr, &r, spec.as_deref(), fallback.as_deref())?;
        }
        for (_, (mut review, types)) in rounds {
            review.spec_type = types.into_iter().collect::<Vec<_>>().join("+");
            reviews.push(review);
        }
    }
    let mut report = calculate(&events, &merges, &reviews, since, until);
    // trace:STORY-2897 | ai:claude
    // trace:TASK-3228 | ai:codex
    report["red_main"] = match crate::main_watch::read_journal(&root) {
        Ok(events) => crate::main_watch::metrics(&events, since, until.min(Utc::now())),
        Err(_) => json!({
            "available": false,
            "diagnostic": "watch journal is unreadable or contains an invalid committed event"
        }),
    };
    if json_format {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Landing metrics: {since} to {until}");
        for hour in report["hours"].as_array().unwrap() {
            println!(
                "{}: {} merges (main git history); READY: {}",
                hour["hour"], hour["merges"], hour["ready"]
            );
        }
        for waits in report["ready_waits"].as_array().unwrap() {
            println!(
                "{}: longest READY-to-merge {}; waits over 2h {}",
                waits["source"], waits["longest_ready_to_merge"], waits["over_2h"]
            );
        }
        println!(
            "Review rounds (verdict history): {}",
            report["review_rounds"]
        );
        println!(
            "Review rounds by approving reviewer vendor: {}",
            report["review_rounds_by_vendor"]
        );
        println!(
            "PRs without an approval record: {}",
            report["review_rounds_unavailable_prs"]
        );
        let red = &report["red_main"];
        if let Some(reason) = red["diagnostic"].as_str() {
            println!("Red main (watch journal): unavailable ({reason})");
            return Ok(());
        }
        println!(
            "Red main (watch journal): {} episodes ({} flakes), {} min red, {} red merges, \
             fix latency median {}s max {}s, {} of {} resolved by revert",
            red["episodes"],
            red["flakes"],
            red["red_minutes"],
            red["red_merges"],
            red["fix_latency_seconds_median"],
            red["fix_latency_seconds_max"],
            red["reverted"],
            red["resolved"]
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(h: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(h * 3600, 0).unwrap()
    }
    fn event(pr: u64, h: i64, new: &str) -> Event {
        Event {
            at: at(h),
            pr,
            head: "head".into(),
            old: None,
            new: new.into(),
        }
    }
    fn review(pr: u64, h: i64, approved: bool) -> Review {
        Review {
            pr,
            at: at(h),
            approved,
            changes: !approved,
            vendor: "codex".into(),
            spec_type: "task".into(),
        }
    }
    #[test]
    fn fixture_intervals_merges_rounds_and_straddling_start() {
        let events = vec![
            event(0, 3, "recording-start"),
            event(2, 3, "READY"),
            event(2, 4, "REWORKING"),
            event(2, 5, "READY"),
            event(3, 5, "READY"),
        ];
        let merges = vec![Merge { pr: 1, at: at(2) }, Merge { pr: 2, at: at(8) }];
        let reviews = vec![
            review(1, 0, false),
            review(1, 1, true),
            review(2, 1, false),
            review(2, 2, false),
            review(2, 3, true),
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida/land")).unwrap();
        let fixture = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect::<String>();
        std::fs::write(dir.path().join(LOG), fixture).unwrap();
        let events = read_events(dir.path()).unwrap();
        let r = calculate(&events, &merges, &reviews, at(0), at(9));
        assert_eq!(r["hours"][2]["merges"], 1);
        assert_eq!(r["hours"][1]["ready"][0]["source"], APPROX);
        assert_eq!(r["hours"][3]["ready"][0]["source"], EXACT);
        assert_eq!(r["hours"][4]["ready"], json!([]));
        assert_eq!(r["hours"][7]["ready"][0]["merge_happened"], false);
        assert_eq!(
            r["ready_waits"][0]["longest_ready_to_merge"],
            json!({"pr":2,"seconds":10800})
        );
        assert_eq!(
            r["ready_waits"][1]["longest_ready_to_merge"],
            json!({"pr":1,"seconds":3600})
        );
        assert_eq!(r["ready_waits"][0]["over_2h"].as_array().unwrap().len(), 2);
        assert_eq!(r["review_rounds"][0]["mean"], 1.5);
        assert_eq!(r["review_rounds"][0]["median"], 1.5);
    }
    #[test]
    fn explicit_unlock_releases_inherited_open_description() {
        use fs2::FileExt;
        let file = tempfile::NamedTempFile::new().unwrap();
        let guard = RecordingLock(file.reopen().unwrap());
        guard.0.try_lock_exclusive().unwrap();
        let inherited = guard.0.try_clone().unwrap();
        let other = file.reopen().unwrap();
        assert!(other.try_lock_exclusive().is_err());
        drop(guard);
        other.try_lock_exclusive().unwrap();
        drop(inherited);
    }
    #[test]
    fn mixed_change_contributes_once_to_each_type() {
        let mut a = review(1, 1, true);
        a.spec_type = "bug+task".into();
        let r = calculate(
            &[],
            &[Merge { pr: 1, at: at(2) }],
            &[review(1, 0, false), a],
            at(0),
            at(3),
        );
        let groups = r["review_rounds"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups
            .iter()
            .all(|g| g["prs"] == 1 && g["mean"] == 1.0 && g["median"] == 1.0));
    }
    #[test]
    fn historical_window_counts_each_landed_pr_once_by_vendor() {
        let merges = (1..=5)
            .map(|pr| Merge {
                pr,
                at: at(pr as i64),
            })
            .collect::<Vec<_>>();
        let mut first_approval = review(2, 1, true);
        first_approval.spec_type = "bug+task".into();
        let reviews = [
            review(1, 0, true),
            first_approval,
            review(3, 0, false),
            review(3, 1, true),
            review(5, 4, true),
        ];
        let r = calculate(&[], &merges, &reviews, at(2), at(5));
        let vendors = r["review_rounds_by_vendor"].as_array().unwrap();
        assert_eq!(vendors.len(), 1);
        assert_eq!(vendors[0]["prs"], 2);
        assert_eq!(vendors[0]["mean"], 0.5);
        assert_eq!(vendors[0]["median"], 0.5);
        assert_eq!(r["review_rounds_unavailable_prs"], json!([4]));
    }

    #[test]
    fn historical_window_keeps_fractional_final_hour() {
        let end = at(2) + Duration::milliseconds(1);
        let r = calculate(&[], &[Merge { pr: 1, at: at(2) }], &[], at(1), end);
        assert_eq!(r["hours"].as_array().unwrap().len(), 2);
        assert_eq!(r["hours"][1]["merges"], 1);
    }

    #[test]
    fn historical_window_anchors_lookback_and_refuses_future_or_empty_end() {
        let cutover = "2026-11-17T12:20:18Z";
        let end = crate::queue_cmd::parse_time_bound_at(cutover, "--until", at(0), &Utc).unwrap();
        let now = end + Duration::days(2);
        assert_eq!(
            window_bounds_at("7d", Some(cutover), now, &Utc).unwrap(),
            (end - Duration::days(7), end)
        );
        assert!(window_bounds_at("7d", Some("2026-11-24T12:20:18Z"), now, &Utc).is_err());
        assert!(window_bounds_at(cutover, Some(cutover), now, &Utc).is_err());
        assert_eq!(
            window_bounds_at("7d", None, now, &Utc).unwrap(),
            (now - Duration::days(7), now)
        );
    }
    #[test]
    fn refusal_ends_pre_recording_approval_estimate() {
        let r = calculate(
            &[],
            &[],
            &[review(1, 0, true), review(1, 1, false)],
            at(0),
            at(4),
        );
        assert_eq!(r["hours"][0]["ready"].as_array().unwrap().len(), 1);
        assert_eq!(r["hours"][1]["ready"], json!([]));
        assert_eq!(r["ready_waits"][1]["over_2h"], json!([]));
    }
    // trace:TASK-3813 | ai:codex
    #[test]
    fn consecutive_approvals_preserve_first_ready_estimate() {
        let r = calculate(
            &[],
            &[Merge { pr: 1, at: at(4) }],
            &[review(1, 0, true), review(1, 2, true)],
            at(0),
            at(5),
        );
        assert_eq!(
            r["ready_waits"][1]["longest_ready_to_merge"]["seconds"],
            14400
        );
        assert_eq!(r["ready_waits"][1]["over_2h"][0]["pr"], 1);
    }
    #[test]
    fn historical_cutoff_keeps_outstanding_approximate_ready_wait() {
        let r = calculate(
            &[event(0, 10, "recording-start")],
            &[Merge { pr: 1, at: at(8) }],
            &[review(1, 0, true)],
            at(0),
            at(5),
        );
        assert_eq!(r["ready_waits"][1]["source"], APPROX);
        assert_eq!(r["ready_waits"][1]["longest_ready_to_merge"], json!(null));
        assert_eq!(
            r["ready_waits"][1]["over_2h"],
            json!([{"pr":1,"seconds":18000,"merged":false}])
        );
    }
    #[test]
    fn straddling_wait_preserves_full_elapsed_time_with_mixed_provenance() {
        let r = calculate(
            &[event(0, 3, "recording-start"), event(1, 3, "READY")],
            &[Merge { pr: 1, at: at(5) }],
            &[review(1, 0, true)],
            at(0),
            at(6),
        );
        assert_eq!(r["ready_waits"][2]["source"], MIXED);
        assert_eq!(
            r["ready_waits"][2]["longest_ready_to_merge"]["seconds"],
            18000
        );
        assert_eq!(r["ready_waits"][2]["over_2h"][0]["seconds"], 18000);
        assert_eq!(r["hours"][2]["ready"][0]["source"], APPROX);
        assert_eq!(r["hours"][3]["ready"][0]["source"], EXACT);
    }
    fn parsed_reviews(words: &[&str]) -> Vec<Review> {
        let mut rounds = BTreeMap::new();
        for (h, word) in words.iter().enumerate() {
            let record = json!({"verdict":word,"recorded_at":at(h as i64).to_rfc3339()});
            add_rounds(&mut rounds, 1, record.as_object().unwrap(), None, None).unwrap();
        }
        rounds
            .into_values()
            .map(|(mut r, types)| {
                r.spec_type = types.into_iter().collect::<Vec<_>>().join("+");
                r
            })
            .collect()
    }
    #[test]
    fn rejected_blocks_ready_but_only_request_changes_counts_as_round() {
        let reviews = parsed_reviews(&["approved", "rejected", "request-changes", "approved"]);
        let r = calculate(&[], &[Merge { pr: 1, at: at(4) }], &reviews, at(0), at(5));
        assert_eq!(r["review_rounds"][0]["mean"], 1.0);
        assert_eq!(r["review_rounds"][0]["median"], 1.0);
        assert_eq!(r["hours"][1]["ready"], json!([]));
    }
    #[test]
    fn waiting_ends_approval_estimate_without_counting_rework() {
        let reviews = parsed_reviews(&["approved", "waiting"]);
        assert_eq!(reviews.len(), 2);
        assert!(!reviews.iter().any(|r| r.changes));
        let r = calculate(&[], &[], &reviews, at(0), at(4));
        assert_eq!(r["hours"][0]["ready"].as_array().unwrap().len(), 1);
        assert_eq!(r["hours"][1]["ready"], json!([]));
        assert_eq!(r["ready_waits"][1]["over_2h"], json!([]));
    }
    #[test]
    fn archive_only_and_mixed_history_preserve_waits_and_deduplicate_rounds() {
        use crate::gate::{ledger, review_verdict};
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join(".aida/review-verdicts/PR-1");
        std::fs::create_dir_all(&archive).unwrap();
        let refusal = json!({"verdict":"request-changes","recorded_at":at(0).to_rfc3339(),"reviewed_sha":"a"});
        let approval =
            json!({"verdict":"approved","recorded_at":at(1).to_rfc3339(),"reviewed_sha":"b"});
        std::fs::write(archive.join("a.json"), refusal.to_string()).unwrap();
        std::fs::write(archive.join("b.json"), approval.to_string()).unwrap();
        let ledger_act: ledger::GateEvent = serde_json::from_value(json!({
            "v":1,"seq":1,"at":at(1).to_rfc3339(),"change":"PR-1",
            "kind":"verdict","record":approval
        }))
        .unwrap();
        for events in [vec![], vec![ledger_act]] {
            if !events.is_empty() {
                std::fs::remove_file(archive.join("b.json")).unwrap();
                std::fs::write(archive.with_extension("json"), approval.to_string()).unwrap();
            }
            let mut rounds = BTreeMap::new();
            for (r, spec, fallback) in
                review_verdict::verdict_history(dir.path(), "PR-1", &events).unwrap()
            {
                add_rounds(&mut rounds, 1, &r, spec.as_deref(), fallback.as_deref()).unwrap();
            }
            assert_eq!(rounds.len(), 2);
            let reviews: Vec<_> = rounds
                .into_values()
                .map(|(mut r, _)| {
                    r.spec_type = "task".into();
                    r
                })
                .collect();
            let r = calculate(&[], &[Merge { pr: 1, at: at(4) }], &reviews, at(0), at(5));
            assert_eq!(r["review_rounds"][0]["mean"], 1.0);
            assert_eq!(
                r["ready_waits"][1]["longest_ready_to_merge"]["seconds"],
                10800
            );
        }
    }
    #[test]
    fn duration_validation_is_bounded_and_unicode_safe() {
        let now = at(1000);
        let parse = |raw| parse_window_at(raw, now, &Utc);
        assert_eq!(parse("7d").unwrap(), now - Duration::days(7));
        assert_eq!(parse("24h").unwrap(), now - Duration::hours(24));
        assert_eq!(parse("45m").unwrap(), now - Duration::minutes(45));
        for bad in [
            "",
            "0h",
            "-1d",
            "3661d",
            "1é",
            "é",
            "999999999999999999999d",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn copied_and_retained_review_acts_count_once() {
        let refusal = json!({"verdict":"request-changes","recorded_at":"1970-01-01T01:00:00Z","recorded_by":"codex reviewer seat","reviewed_sha":"a"});
        let approval = json!({"verdict":"approved","recorded_at":"1970-01-01T02:00:00Z","recorded_by":"codex reviewer seat","reviewed_sha":"b","rounds":[refusal]});
        let mut rounds = BTreeMap::new();
        add_rounds(
            &mut rounds,
            1,
            refusal.as_object().unwrap(),
            Some("TASK-1"),
            None,
        )
        .unwrap();
        add_rounds(
            &mut rounds,
            1,
            approval.as_object().unwrap(),
            Some("TASK-1"),
            None,
        )
        .unwrap();
        add_rounds(
            &mut rounds,
            1,
            approval.as_object().unwrap(),
            Some("BUG-1"),
            None,
        )
        .unwrap();
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds.values().filter(|(r, _)| r.changes).count(), 1);
        assert!(rounds.values().all(|(_, types)| types.len() == 2));
    }
    #[test]
    fn recording_incremental_pass_stays_below_one_second() {
        let dir = tempfile::tempdir().unwrap();
        let changes: Vec<_> = (1..=500)
            .map(|id| Pr {
                id,
                head: "a".into(),
                base: "main".into(),
                branch: None,
                title: String::new(),
                updated_at: None,
                hold_label: None,
                ci: None,
            })
            .collect();
        let rows: Vec<_> = changes
            .iter()
            .map(|pr| Row {
                pr: pr.id,
                track: super::super::Track::Main,
                state: super::super::State::Ready,
                action: super::super::Action::Train,
                since: None,
                reason: String::new(),
                actor: None,
                ci: None,
            })
            .collect();
        let started = std::time::Instant::now();
        record(dir.path(), &changes, &rows, at(0)).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let started = std::time::Instant::now();
        record(dir.path(), &changes, &rows, at(1)).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
    #[test]
    fn generated_recorder_suffix_and_unknown_metadata() {
        for (label, expected) in [
            ("r7 (codex reviewer seat)", "codex"),
            ("claude reviewer seat", "claude"),
            ("agy reviewer seat", "antigravity"),
            ("unregistered reviewer seat", "unknown"),
            ("codex named-me", "unknown"),
            ("r7 (reviewer seat)", "unknown"),
        ] {
            let r = json!({"recorded_by":label});
            assert_eq!(vendor(r.as_object().unwrap()), expected);
        }
    }
    #[test]
    fn unchanged_pass_is_noop_and_cursor_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let pr = Pr {
            id: 1,
            head: "a".into(),
            base: "main".into(),
            branch: None,
            title: String::new(),
            updated_at: None,
            hold_label: None,
            ci: None,
        };
        let row = Row {
            pr: 1,
            track: super::super::Track::Main,
            state: super::super::State::Ready,
            action: super::super::Action::Train,
            since: None,
            reason: String::new(),
            actor: None,
            ci: None,
        };
        record(dir.path(), &[pr.clone()], &[row.clone()], at(0)).unwrap();
        let before = std::fs::read(dir.path().join(LOG)).unwrap();
        record(dir.path(), &[pr.clone()], &[row.clone()], at(1)).unwrap();
        assert_eq!(before, std::fs::read(dir.path().join(LOG)).unwrap());
        std::fs::remove_file(dir.path().join(".aida/land/readiness-cursor.json")).unwrap();
        record(dir.path(), &[pr], &[row], at(2)).unwrap();
        assert_eq!(before, std::fs::read(dir.path().join(LOG)).unwrap());
        record(dir.path(), &[], &[], at(3)).unwrap();
        assert_eq!(
            read_events(dir.path()).unwrap().last().unwrap().new,
            "absent"
        );
    }
}
