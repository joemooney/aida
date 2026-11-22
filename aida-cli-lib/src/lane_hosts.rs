//! Lanes on other hosts, as each host last reported them.
//!
//! A lane's launcher records a claim on the shared store when the lane starts
//! and removes it when the lane ends. The claim's heartbeat is the only thing
//! another host can read about that lane, so it is the lane-host record: this
//! module is the one reader of it, shared by the throughput guard
//! (`aida land stalls`) and the review watcher's deferral behind a lane
//! (BUG-2706). Neither keeps a second copy of these rules.
//!
//! A record says one of three things about a lane on another host:
//!
//! - **reporting**: the heartbeat is inside its expiry. The lane is live as of
//!   that heartbeat, and the heartbeat's age is how old the observation is.
//! - **silent**: the claim was never released and the heartbeat is past its
//!   expiry. A launcher that writes its claim once and never refreshes it
//!   looks exactly like this while its lane runs, so the lane may be live:
//!   its state is unknown, and a caller must say so rather than call it live
//!   or absent.
//! - **invalid**: the record parses but its host, heartbeat or expiry cannot
//!   be used (no host, an unreadable or far-future heartbeat, an expiry no
//!   date can hold). It is kept, with the file's name and the reason: the
//!   lane it stands for may be live, so its state is unknown too.
//!
//! A record that says nothing about another host (this host's own claim, a
//! process lock) is the only kind left out. Callers take ownership and the
//! per-host counts from the same [`View`], so the two cannot disagree about a
//! record.
//!
//! Everything here is read-only. Records are read as bounded regular files; a
//! record that cannot be read is listed as unreadable, never dropped.
// trace:TASK-3827 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

use crate::coordination::Claim;

/// How far ahead of this host's clock another host's heartbeat may be and
/// still count. Past this the timestamp is not evidence of anything.
pub(crate) const CLOCK_SKEW_SECS: i64 = 300;
/// The largest lane record read. A real one is under 1 KiB.
const MAX_RECORD_BYTES: u64 = 64 * 1024;
/// Records looked at in one read; more than this is reported, not read.
const MAX_RECORDS: usize = 20_000;
/// Unreadable records named one by one; the rest are counted.
const MAX_NAMED_UNREADABLE: usize = 5;

/// What a host's record says about one of its lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// The heartbeat is inside its expiry.
    Reporting,
    /// Unreleased, heartbeat past its expiry: state unknown.
    Silent,
}

/// One lane on another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lane {
    pub claim: Claim,
    pub state: State,
    /// The host's last report of this lane.
    pub heartbeat: DateTime<Utc>,
    /// When that report stops counting as live (heartbeat + expiry).
    pub expires: DateTime<Utc>,
}

impl Lane {
    /// Minutes since the host last reported this lane; never negative.
    pub(crate) fn age_mins(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.heartbeat)
            .num_minutes()
            .max(0)
    }

    /// The lease or claim this lane runs under, for a message.
    pub(crate) fn lease(&self) -> &str {
        self.claim
            .lease_id
            .as_deref()
            .or(self.claim.claim_id.as_deref())
            .unwrap_or("no lease id")
    }

    /// A reviewer's claim on a change, not an implementer lane on a spec.
    pub(crate) fn is_review(&self) -> bool {
        self.claim.review_verb
    }
}

/// What one parsed record says about another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Judged {
    /// Not a record of a lane on another host: this host's own claim, or a
    /// process lock. Nothing is concluded from it here.
    NotForeign,
    Lane(Lane),
    /// A record of another host that cannot be used as a report, and why.
    Invalid(String),
}

/// A parsed record of another host that is not a usable report. The lane it
/// stands for may be live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invalid {
    pub claim: Claim,
    /// The record's file name, for the operator who has to look at it.
    pub file: String,
    pub why: String,
}

/// What `claim` says about a lane on another host at `now`.
pub(crate) fn judge(claim: &Claim, now: DateTime<Utc>, our_host: &str) -> Judged {
    if claim.process_backed {
        return Judged::NotForeign;
    }
    if claim.host.trim().is_empty() {
        return Judged::Invalid("it names no host".into());
    }
    if claim.host == our_host {
        return Judged::NotForeign;
    }
    let Ok(heartbeat) = DateTime::parse_from_rfc3339(claim.heartbeat_at.trim()) else {
        return Judged::Invalid("its heartbeat is not a time".into());
    };
    let heartbeat = heartbeat.with_timezone(&Utc);
    let ahead = now.checked_add_signed(Duration::seconds(CLOCK_SKEW_SECS));
    if ahead.is_none_or(|limit| heartbeat > limit) {
        return Judged::Invalid(format!(
            "its heartbeat is more than {CLOCK_SKEW_SECS}s ahead of this host's clock"
        ));
    }
    let expires = i64::try_from(claim.ttl_secs)
        .ok()
        .and_then(Duration::try_seconds)
        .and_then(|ttl| heartbeat.checked_add_signed(ttl));
    let Some(expires) = expires else {
        return Judged::Invalid("its expiry is not a time".into());
    };
    // The same expiry rule every other reader of a claim applies. A lane on
    // another host has no process this host could look at.
    let state =
        if crate::coordination::claim_staleness(claim, now, our_host, |_, _| false, |_| false)
            .is_none()
        {
            State::Reporting
        } else {
            State::Silent
        };
    Judged::Lane(Lane {
        claim: claim.clone(),
        state,
        heartbeat,
        expires,
    })
}

/// The lane `claim` is a usable report of, if it is one.
#[cfg(test)]
pub(crate) fn standing(claim: &Claim, now: DateTime<Utc>, our_host: &str) -> Option<Lane> {
    match judge(claim, now, our_host) {
        Judged::Lane(lane) => Some(lane),
        Judged::NotForeign | Judged::Invalid(_) => None,
    }
}

/// The lanes one host reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HostCount {
    /// Lanes whose record is inside its expiry.
    pub reporting: usize,
    /// Unreleased lanes whose record is past its expiry, not older than the
    /// horizon asked for.
    pub silent: usize,
    /// Records naming this host that are not a usable report.
    pub invalid: usize,
    /// Age in minutes of the host's newest report: how old this view of the
    /// host is. `None` when it has never reported inside the horizon.
    pub newest_report_mins: Option<i64>,
}

/// Who holds something on another host, as far as the records say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder<'a> {
    Lane(&'a Lane),
    Invalid(&'a Invalid),
}

/// Every lane other hosts have on record, and what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct View {
    pub lanes: Vec<Lane>,
    /// Records of other hosts that parse but are not a usable report.
    pub invalid: Vec<Invalid>,
    /// One line per problem: the store, the directory, or a record.
    pub unreadable: Vec<String>,
}

impl View {
    /// Lane counts per host. A silent lane whose last report is older than
    /// `silent_horizon` is a leftover record and is not counted.
    pub(crate) fn hosts(
        &self,
        now: DateTime<Utc>,
        silent_horizon: Duration,
    ) -> BTreeMap<String, HostCount> {
        let mut hosts: BTreeMap<String, HostCount> = BTreeMap::new();
        for lane in &self.lanes {
            let age = now.signed_duration_since(lane.heartbeat);
            if lane.state == State::Silent && age > silent_horizon {
                continue;
            }
            let host = hosts.entry(lane.claim.host.clone()).or_default();
            match lane.state {
                State::Reporting => host.reporting += 1,
                State::Silent => host.silent += 1,
            }
            let mins = lane.age_mins(now);
            host.newest_report_mins = Some(host.newest_report_mins.map_or(mins, |m| m.min(mins)));
        }
        // A record that names no host is on no host's line; it is listed
        // with the records that are not usable.
        for record in &self.invalid {
            if !record.claim.host.trim().is_empty() {
                hosts.entry(record.claim.host.clone()).or_default().invalid += 1;
            }
        }
        hosts
    }

    /// Who on another host holds what `covers` accepts (given a record's
    /// scope), reviewers' claims left out: a reporting lane first, else the
    /// silent lane that reported last (unless that was longer ago than
    /// `silent_horizon`: a leftover holds nothing), else an invalid record.
    pub(crate) fn holder(
        &self,
        now: DateTime<Utc>,
        silent_horizon: Duration,
        covers: impl Fn(&str) -> bool,
    ) -> Option<Holder<'_>> {
        let mut best: Option<&Lane> = None;
        for lane in &self.lanes {
            if lane.is_review() || !covers(&lane.claim.scope) {
                continue;
            }
            if lane.state == State::Silent
                && now.signed_duration_since(lane.heartbeat) > silent_horizon
            {
                continue;
            }
            let better = match best {
                None => true,
                Some(held) => match (lane.state, held.state) {
                    (State::Reporting, State::Silent) => true,
                    (State::Silent, State::Reporting) => false,
                    _ => lane.heartbeat > held.heartbeat,
                },
            };
            if better {
                best = Some(lane);
            }
        }
        let invalid = || {
            self.invalid
                .iter()
                .find(|record| !record.claim.review_verb && covers(&record.claim.scope))
                .map(Holder::Invalid)
        };
        best.map(Holder::Lane).or_else(invalid)
    }
}

/// The project's shared store, when it already exists as a directory store.
/// Resolving it creates nothing. A project without one (a single-file store,
/// or none yet) is refused with the reason: it has no record of other hosts.
pub(crate) fn store_root(root: &Path) -> Result<PathBuf, String> {
    crate::detect_distributed_store_from(root).ok_or_else(|| {
        "this project has no shared store directory, so lanes on other hosts are not recorded here"
            .to_string()
    })
}

/// Reads the lanes other hosts have on record in the shared store of the
/// project at `root`. Never writes and never creates a file or directory.
pub(crate) fn read(root: &Path, our_host: &str, now: DateTime<Utc>) -> View {
    match store_root(root) {
        Ok(store) => read_store(&store, our_host, now),
        Err(why) => View {
            unreadable: vec![why],
            ..View::default()
        },
    }
}

/// [`read`], for a store whose location is already known.
pub(crate) fn read_store(store: &Path, our_host: &str, now: DateTime<Utc>) -> View {
    let mut view = View::default();
    let dir = crate::coordination::leases_dir(store);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // No lane has ever been recorded.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return view,
        Err(error) => {
            view.unreadable.push(format!(
                "the lane records could not be listed ({})",
                routed(&error.to_string(), 120)
            ));
            return view;
        }
    };
    let mut bad: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                bad.push(format!(
                    "a directory entry ({})",
                    routed(&error.to_string(), 120)
                ));
                continue;
            }
        };
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        seen += 1;
        if seen > MAX_RECORDS {
            view.unreadable.push(format!(
                "more than {MAX_RECORDS} lane records: the rest were not read"
            ));
            break;
        }
        let name = routed(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            120,
        );
        let raw = match aida_core::seat_clone_detect::read_regular_file_result_with_limit(
            &path,
            MAX_RECORD_BYTES,
        ) {
            Ok(raw) => raw,
            // Released between the listing and the read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                bad.push(format!("{name} ({})", routed(&error.to_string(), 120)));
                continue;
            }
        };
        match toml::from_str::<Claim>(&raw) {
            Ok(claim) => match judge(&claim, now, our_host) {
                Judged::NotForeign => {}
                Judged::Lane(lane) => view.lanes.push(lane),
                Judged::Invalid(why) => view.invalid.push(Invalid {
                    claim,
                    file: name,
                    why,
                }),
            },
            Err(error) => bad.push(format!(
                "{name} (not a lane record: {})",
                routed(&error.message().to_string(), 120)
            )),
        }
    }
    bad.sort();
    let more = bad.len().saturating_sub(MAX_NAMED_UNREADABLE);
    if !bad.is_empty() {
        let named: Vec<&str> = bad
            .iter()
            .take(MAX_NAMED_UNREADABLE)
            .map(String::as_str)
            .collect();
        view.unreadable.push(format!(
            "lane record(s) unreadable, so a lane may be missing: {}{}",
            named.join("; "),
            if more > 0 {
                format!("; and {more} more")
            } else {
                String::new()
            }
        ));
    }
    view.lanes.sort_by(|a, b| {
        (a.claim.host.as_str(), a.claim.scope.as_str())
            .cmp(&(b.claim.host.as_str(), b.claim.scope.as_str()))
    });
    view.invalid.sort_by(|a, b| a.file.cmp(&b.file));
    if !view.invalid.is_empty() {
        let named: Vec<String> = view
            .invalid
            .iter()
            .take(MAX_NAMED_UNREADABLE)
            .map(|record| format!("{} ({})", record.file, record.why))
            .collect();
        let more = view.invalid.len().saturating_sub(MAX_NAMED_UNREADABLE);
        view.unreadable.push(format!(
            "lane record(s) not usable as a report, so the lane each stands for may be live: {}{}",
            named.join("; "),
            if more > 0 {
                format!("; and {more} more")
            } else {
                String::new()
            }
        ));
    }
    view
}

/// `text` as one bounded line that is safe to route to a seat: control and
/// invisible formatting characters are dropped, every run of white space
/// becomes one space, and what is past `limit` characters is cut with `…`.
/// Fields that come from a shared record, a log, an error or a path go
/// through this before they reach a due job, a mail or a terminal.
pub(crate) fn routed(text: &str, limit: usize) -> String {
    let visible: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !is_invisible_format(*c))
        .collect();
    let flat = visible.split(' ').filter(|w| !w.is_empty()).fold(
        String::new(),
        |mut out: String, word| {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
            out
        },
    );
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Zero-width and direction-changing characters: they print nothing and can
/// reorder or hide the text around them.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(host: &str, scope: &str, heartbeat: DateTime<Utc>) -> Claim {
        serde_json::from_value(serde_json::json!({
            "scope": scope, "node_id": "node-two", "claim_id": "claim-1", "lease_id": "lease-1",
            "clone_path": "/remote/clone", "host": host, "pid": 1, "agent": "implementer",
            "started_at": heartbeat.to_rfc3339(), "heartbeat_at": heartbeat.to_rfc3339(),
            "ttl_secs": 1800
        }))
        .unwrap()
    }

    fn store_with(records: &[(&str, String)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = crate::coordination::leases_dir(temp.path());
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in records {
            std::fs::write(dir.join(name), body).unwrap();
        }
        temp
    }

    #[test]
    fn task_3827_a_record_inside_its_expiry_reports_and_past_it_is_silent() {
        let now = Utc::now();
        let fresh = claim("lane-host", "TASK-1", now - Duration::minutes(4));
        let lane = standing(&fresh, now, "here").unwrap();
        assert_eq!(lane.state, State::Reporting);
        assert_eq!(lane.age_mins(now), 4);
        assert_eq!(lane.expires, lane.heartbeat + Duration::seconds(1800));
        assert_eq!(lane.lease(), "lease-1");

        let old = claim("lane-host", "TASK-1", now - Duration::minutes(65));
        let lane = standing(&old, now, "here").unwrap();
        assert_eq!(lane.state, State::Silent);
        assert_eq!(lane.age_mins(now), 65);
    }

    #[test]
    fn task_3827_a_record_is_not_foreign_or_invalid_and_never_silently_dropped() {
        let now = Utc::now();
        let good = claim("lane-host", "TASK-1", now);
        // This host's own lanes are read from its own lease table, and a
        // process lock is not a lane: nothing is concluded from either.
        assert_eq!(judge(&good, now, "lane-host"), Judged::NotForeign);
        let mut lock = good.clone();
        lock.process_backed = true;
        assert_eq!(judge(&lock, now, "here"), Judged::NotForeign);
        // Every other record that cannot be used is invalid, with the reason.
        let invalid = |claim: &Claim| match judge(claim, now, "here") {
            Judged::Invalid(why) => why,
            other => panic!("expected invalid, got {other:?}"),
        };
        let mut hostless = good.clone();
        hostless.host = "  ".into();
        assert_eq!(invalid(&hostless), "it names no host");
        let mut garbled = good.clone();
        garbled.heartbeat_at = "yesterday".into();
        assert_eq!(invalid(&garbled), "its heartbeat is not a time");
        // A little ahead is another host's clock; far ahead is not a report.
        let ahead = claim(
            "lane-host",
            "TASK-1",
            now + Duration::seconds(CLOCK_SKEW_SECS),
        );
        assert_eq!(standing(&ahead, now, "here").unwrap().age_mins(now), 0);
        let future = claim(
            "lane-host",
            "TASK-1",
            now + Duration::seconds(CLOCK_SKEW_SECS + 1),
        );
        assert!(invalid(&future).contains("ahead of this host's clock"));
        // An expiry no date can hold is refused, not a panic.
        let mut endless = good;
        endless.ttl_secs = u64::MAX;
        assert_eq!(invalid(&endless), "its expiry is not a time");
    }

    // An invalid record of another host is kept with its file name, counted
    // on its host, and holds its spec as a lane whose state is unknown: the
    // host cannot show "no lane" while the record decides a stall.
    #[test]
    fn task_3827_an_invalid_record_is_named_counted_and_holds_its_spec_as_unknown() {
        let now = Utc::now();
        let mut garbled = claim("dell1", "TASK-5", now);
        garbled.heartbeat_at = "soon".into();
        let future = claim("dell1", "TASK-6", now + Duration::hours(2));
        // The largest expiry a TOML record can carry.
        let mut endless = claim("dell2", "TASK-7", now);
        endless.ttl_secs = i64::MAX as u64;
        let mut review = claim("dell2", "PR-9", now);
        review.review_verb = true;
        review.heartbeat_at = "never".into();
        let store = store_with(&[
            ("garbled.toml", toml::to_string(&garbled).unwrap()),
            ("future.toml", toml::to_string(&future).unwrap()),
            ("endless.toml", toml::to_string(&endless).unwrap()),
            ("review.toml", toml::to_string(&review).unwrap()),
        ]);
        let view = read_store(store.path(), "here", now);
        assert!(view.lanes.is_empty());
        assert_eq!(view.invalid.len(), 4);
        assert_eq!(view.unreadable.len(), 1);
        let line = &view.unreadable[0];
        for name in [
            "garbled.toml (its heartbeat is not a time)",
            "future.toml",
            "endless.toml (its expiry is not a time)",
        ] {
            assert!(line.contains(name), "{name} missing from: {line}");
        }
        let hosts = view.hosts(now, Duration::hours(24));
        assert_eq!(
            hosts["dell1"],
            HostCount {
                invalid: 2,
                ..HostCount::default()
            }
        );
        assert_eq!(hosts["dell2"].invalid, 2);
        let day = Duration::hours(24);
        for (scope, file) in [
            ("TASK-5", "garbled.toml"),
            ("TASK-6", "future.toml"),
            ("TASK-7", "endless.toml"),
        ] {
            match view.holder(now, day, |held| held == scope) {
                Some(Holder::Invalid(record)) => assert_eq!(record.file, file),
                other => panic!("{scope}: {other:?}"),
            }
        }
        // A reviewer's record is not an implementer lane on the change.
        assert_eq!(view.holder(now, day, |held| held == "PR-9"), None);
        assert_eq!(view.holder(now, day, |held| held == "TASK-8"), None);
    }

    #[test]
    fn task_3827_hosts_are_counted_from_their_own_reports_with_the_reports_age() {
        let now = Utc::now();
        let mut records = Vec::new();
        for (n, mins) in [1i64, 3, 7, 12, 20, 25, 29].iter().enumerate() {
            let c = claim(
                "dell1",
                &format!("TASK-{n}"),
                now - Duration::minutes(*mins),
            );
            records.push((format!("task-{n}.toml"), toml::to_string(&c).unwrap()));
        }
        let silent = claim("dell1", "TASK-90", now - Duration::minutes(65));
        records.push(("silent.toml".into(), toml::to_string(&silent).unwrap()));
        let leftover = claim("dell1", "TASK-91", now - Duration::days(9));
        records.push(("leftover.toml".into(), toml::to_string(&leftover).unwrap()));
        let ours = claim("here", "TASK-92", now);
        records.push(("ours.toml".into(), toml::to_string(&ours).unwrap()));
        let borrowed: Vec<(&str, String)> = records
            .iter()
            .map(|(name, body)| (name.as_str(), body.clone()))
            .collect();
        let store = store_with(&borrowed);

        let view = read_store(store.path(), "here", now);
        assert!(view.unreadable.is_empty(), "{:?}", view.unreadable);
        let hosts = view.hosts(now, Duration::hours(24));
        assert_eq!(
            hosts.len(),
            1,
            "this host's own claim is not a foreign lane"
        );
        assert_eq!(
            hosts["dell1"],
            HostCount {
                reporting: 7,
                silent: 1,
                invalid: 0,
                newest_report_mins: Some(1)
            }
        );
        // The nine-day-old record is still a lane record, for a caller that
        // asks about its spec; it is only left out of the host's count.
        assert_eq!(view.lanes.len(), 9);
    }

    #[test]
    fn task_3827_the_holder_of_a_spec_is_the_reporting_lane_else_the_last_silent_one() {
        let now = Utc::now();
        let mut review = claim("dell1", "PR-7", now);
        review.review_verb = true;
        let view = View {
            lanes: [
                claim("dell1", "TASK-1", now - Duration::minutes(90)),
                claim("dell2", "TASK-1", now - Duration::minutes(2)),
                claim("dell1", "TASK-2", now - Duration::minutes(200)),
                claim("dell2", "TASK-2", now - Duration::minutes(70)),
                review,
            ]
            .iter()
            .filter_map(|c| standing(c, now, "here"))
            .collect(),
            ..View::default()
        };
        let day = Duration::hours(24);
        let lane = |scope: &'static str| match view.holder(now, day, move |held| held == scope) {
            Some(Holder::Lane(lane)) => Some(lane),
            Some(Holder::Invalid(record)) => panic!("{record:?}"),
            None => None,
        };
        let held = lane("TASK-1").unwrap();
        assert_eq!(
            (held.claim.host.as_str(), held.state),
            ("dell2", State::Reporting)
        );
        let held = lane("TASK-2").unwrap();
        assert_eq!(
            (held.claim.host.as_str(), held.state),
            ("dell2", State::Silent)
        );
        assert_eq!(held.age_mins(now), 70);
        assert!(lane("TASK-3").is_none());
        // A reviewer's claim is not an implementer lane on the change.
        assert!(lane("PR-7").is_none());
        // A silent record older than the horizon is a leftover: with only
        // such records, nobody holds the spec.
        assert_eq!(
            view.holder(now, Duration::minutes(60), |held| held == "TASK-2"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn task_3827_a_record_that_is_not_a_small_regular_lane_file_is_named_as_unreadable() {
        let now = Utc::now();
        let good = toml::to_string(&claim("dell1", "TASK-1", now)).unwrap();
        let store = store_with(&[
            ("good.toml", good.clone()),
            ("garbled.toml", "scope = [".into()),
            ("huge.toml", "#".repeat(MAX_RECORD_BYTES as usize + 1)),
            ("notes.txt", "not a record".into()),
        ]);
        let dir = crate::coordination::leases_dir(store.path());
        // A symlink to a real record is not followed.
        std::os::unix::fs::symlink(dir.join("good.toml"), dir.join("link.toml")).unwrap();
        // A FIFO must not block the read.
        let fifo = std::ffi::CString::new(dir.join("pipe.toml").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::fs::create_dir(dir.join("dir.toml")).unwrap();

        let view = read_store(store.path(), "here", now);
        assert_eq!(view.lanes.len(), 1, "only the good record is a lane");
        assert_eq!(view.unreadable.len(), 1);
        let line = &view.unreadable[0];
        for name in [
            "garbled.toml",
            "huge.toml",
            "link.toml",
            "pipe.toml",
            "dir.toml",
        ] {
            assert!(line.contains(name), "{name} missing from: {line}");
        }
        assert!(!line.contains("notes.txt") && !line.contains("good.toml"));
        assert!(!line.contains('\n'));
    }

    // trace:TASK-3827 | ai:codex
    #[test]
    fn task_3827_reads_lane_in_configured_symlink_store() {
        let now = Utc::now();
        let record = toml::to_string(&claim("remote", "TASK-1", now)).unwrap();
        let store = store_with(&[("holder.toml", record)]);
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(
            project.path().join(".aida/config.toml"),
            "store_path = 'shared'",
        )
        .unwrap();
        std::os::unix::fs::symlink(store.path(), project.path().join("shared")).unwrap();
        let view = read(project.path(), "here", now);
        assert!(view.unreadable.is_empty(), "{:?}", view.unreadable);
        assert_eq!(view.lanes.len(), 1);
    }

    #[test]
    fn task_3827_reading_creates_nothing_and_a_project_without_a_store_directory_is_refused() {
        let now = Utc::now();
        // No store at all.
        let bare = tempfile::tempdir().unwrap();
        let view = read(bare.path(), "here", now);
        assert!(view.lanes.is_empty());
        assert_eq!(view.unreadable.len(), 1);
        assert!(view.unreadable[0].contains("no shared store directory"));
        assert_eq!(std::fs::read_dir(bare.path()).unwrap().count(), 0);

        // A project whose store is one file (the single-file layout).
        let legacy = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(legacy.path().join(".aida")).unwrap();
        std::fs::write(
            legacy.path().join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::write(legacy.path().join(".aida-store"), "requirements: []\n").unwrap();
        let before = std::fs::read(legacy.path().join(".aida-store")).unwrap();
        let view = read(legacy.path(), "here", now);
        assert!(view.lanes.is_empty());
        assert_eq!(view.unreadable.len(), 1, "{:?}", view.unreadable);
        assert_eq!(
            std::fs::read(legacy.path().join(".aida-store")).unwrap(),
            before
        );
        assert!(legacy.path().join(".aida-store").is_file());

        // A store directory with no lane ever recorded: nothing to report,
        // and the leases directory is not created.
        let empty = tempfile::tempdir().unwrap();
        let view = read_store(empty.path(), "here", now);
        assert_eq!(view, View::default());
        assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
    }

    #[test]
    fn task_3827_a_routed_field_is_one_bounded_line_without_hidden_characters() {
        assert_eq!(routed("  a\tb\r\nc  ", 80), "a b c");
        assert_eq!(routed("id\u{1b}[31m red\u{0}\u{7}", 80), "id[31m red");
        assert_eq!(
            routed("ab\u{202E}dc\u{200B}e\u{2066}f\u{FEFF}", 80),
            "abdcef"
        );
        assert_eq!(
            routed("line one\u{2028}line two\u{0085}three", 80),
            "line one line two three"
        );
        assert_eq!(routed("abcdefgh", 5), "abcd…");
        assert_eq!(routed("héllo wörld", 7), "héllo …");
        assert_eq!(routed("", 5), "");
        assert_eq!(routed(" \n\t ", 5), "");
        assert_eq!(routed("x", 0), "…");
    }
}
