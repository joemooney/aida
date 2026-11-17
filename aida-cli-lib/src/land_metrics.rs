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
        if let Some(start) = approved.remove(&review.pr) {
            intervals.push(Interval {
                pr: review.pr,
                start,
                end: review.at,
                source: APPROX,
            });
        }
        if review.approved {
            approved.insert(review.pr, review.at);
        }
    }
    for (pr, start) in approved {
        let end = merges
            .iter()
            .find(|m| m.pr == pr)
            .map(|m| m.at)
            .unwrap_or(until)
            .min(began);
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
    let mut hour = since.timestamp().div_euclid(3600) * 3600;
    while hour < until.timestamp() {
        let end = hour + 3600;
        let ready: BTreeSet<_> = intervals
            .iter()
            .filter(|i| {
                i.start.timestamp() < end.min(until.timestamp())
                    && i.end.timestamp() > hour.max(since.timestamp())
            })
            .map(|i| (i.pr, i.source))
            .collect();
        let count = merges
            .iter()
            .filter(|m| {
                m.at >= since && m.at < until && m.at.timestamp() >= hour && m.at.timestamp() < end
            })
            .count();
        hours.push(json!({"hour": DateTime::<Utc>::from_timestamp(hour,0), "merges": count, "source": "main git history", "ready": ready.iter().map(|(pr,source)| json!({"pr":pr,"source":source,"merge_happened":count>0})).collect::<Vec<_>>() }));
        hour = end;
    }
    let waits = [EXACT, APPROX].map(|source| {
        let mut landed = Vec::new();
        let mut waiting = Vec::new();
        for i in intervals
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
    let rounds: Vec<_> = groups.into_iter().map(|((vendor,spec_type),mut n)| {
        n.sort_unstable(); let len=n.len();
        let median = (n[(len-1)/2] + n[len/2]) as f64 / 2.;
        json!({"vendor":vendor,"spec_type":spec_type,"prs":len,"mean":n.iter().sum::<usize>() as f64/len as f64,"median":median,"source":"verdict history"})
    }).collect();
    json!({"since":since,"until":until,"recording_began":events.first().map(|e|e.at),"hours":hours,"ready_waits":waits,"review_rounds":rounds,"review_rounds_unavailable_prs":missing,"review_rounds_source":"verdict history"})
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
    if !kind.approves() && !kind.requires_changes() {
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
                changes: kind.requires_changes(),
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
pub(super) fn run(start: &Path, window: &str, json_format: bool) -> Result<()> {
    let until = Utc::now();
    let since = parse_window_at(window, until, &chrono::Local)?;
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
        for event in records {
            if event.is_spec_only() {
                continue;
            }
            let Some(r) = event.verdict_record() else {
                continue;
            };
            add_rounds(&mut rounds, pr, r, event.spec.as_deref(), Some(&event.at))?;
        }
        let path = crate::gate::review_verdict::verdict_path(&root, &format!("PR-{pr}"));
        match crate::gate::review_verdict::read_verdict_body(&path) {
            Ok(body) => {
                let r: serde_json::Map<String, Value> = serde_json::from_str(&body)?;
                add_rounds(&mut rounds, pr, &r, None, None)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        for value in
            crate::gate::review_verdict::archived_recordings(&path).map_err(anyhow::Error::msg)?
        {
            add_rounds(
                &mut rounds,
                pr,
                value.as_object().context("invalid archived verdict")?,
                None,
                None,
            )?;
        }
        for (_, (mut review, types)) in rounds {
            review.spec_type = types.into_iter().collect::<Vec<_>>().join("+");
            reviews.push(review);
        }
    }
    let report = calculate(&events, &merges, &reviews, since, until);
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
            "PRs without an approval record: {}",
            report["review_rounds_unavailable_prs"]
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
