//! A headless agent's usage limit as a typed outage: recorded on the host
//! with its reset time and refused at launch until then (then ignored).
// trace:STORY-3134 | ai:claude
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, Utc};
use std::path::{Path, PathBuf};

/// How long an outage without a vendor-given reset time is held.
const UNKNOWN_RESET_HOLD_MINUTES: i64 = 60;
/// Only the log's tail is read; a usage-limit failure ends the run.
const TAIL_BYTES: u64 = 256 * 1024;

/// `VendorUnavailable { vendor, until }`, as recorded on the host.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Outage {
    pub(crate) vendor: String,
    pub(crate) until: DateTime<Utc>,
    /// Whether `until` came from the vendor rather than the default hold.
    pub(crate) until_known: bool,
    pub(crate) message: String,
    /// The lane whose run hit the limit, when a lane recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lane: Option<String>,
}

impl Outage {
    pub(crate) fn new(vendor: &str, until: Option<DateTime<Utc>>, message: String) -> Self {
        let hold = Utc::now() + Duration::minutes(UNKNOWN_RESET_HOLD_MINUTES);
        let (vendor, until_known) = (vendor.to_string(), until.is_some());
        let until = until.unwrap_or(hold);
        Self {
            vendor,
            until,
            until_known,
            message,
            lane: None,
        }
    }

    fn until_text(&self) -> String {
        let until = self.until.with_timezone(&Local).format("%Y-%m-%d %H:%M %Z");
        let hold = if self.until_known {
            ""
        } else {
            " (no reset time given)"
        };
        format!("{until}{hold}")
    }

    /// The lane exit's lead: `agent usage limit: <vendor> until <reset>`.
    pub(crate) fn exit_note(&self) -> String {
        let (vendor, until) = (&self.vendor, self.until_text());
        format!(
            "agent usage limit: {vendor} until {until} ({})",
            self.message
        )
    }

    pub(crate) fn describe(&self) -> String {
        let (vendor, until, message) = (&self.vendor, self.until_text(), &self.message);
        format!("`{vendor}` is unavailable until {until} ({message})")
    }
}

/// The JSON events in the tail of a headless `log`; other lines are skipped.
pub(crate) fn tail_events(log: &Path) -> Option<Vec<serde_json::Value>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(log).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    // The seek can land inside a UTF-8 sequence: read bytes, decode lossily.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(
        text.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
    )
}

/// The record path for `vendor` (its session spelling).
fn path(main: &Path, vendor: &str) -> PathBuf {
    main.join(format!(".aida/vendor-outages/{vendor}.json"))
}

/// Record `outage` on the main checkout `main`, replacing any earlier one.
pub(crate) fn record(main: &Path, outage: &Outage) -> Result<()> {
    let path = path(main, &outage.vendor);
    std::fs::create_dir_all(path.parent().context("no record directory")?)?;
    let body = serde_json::to_vec_pretty(outage)?;
    aida_core::write_atomic(&path, body).with_context(|| format!("write {}", path.display()))
}

/// Refuse a launch on `vendor` while it has an unexpired outage recorded on
/// the main checkout of `root`. An unreadable record never refuses.
pub(crate) fn refuse_launch(root: &Path, vendor: &str) -> Result<()> {
    let path = path(&crate::main_worktree_root_from(root), vendor);
    let read = std::fs::read(&path).ok();
    let outage = read.and_then(|raw| serde_json::from_slice::<Outage>(&raw).ok());
    match outage.filter(|o| o.until > Utc::now()) {
        Some(o) => anyhow::bail!(
            "{}. Launches and reviews on `{vendor}` are refused until then and resume on their \
             own after the reset. To retry earlier, remove {}.",
            o.describe(),
            path.display()
        ),
        None => Ok(()),
    }
}

/// Every unexpired outage recorded on the main checkout of `root`, by vendor.
/// Unreadable records are skipped, as [`refuse_launch`] skips them.
// trace:STORY-3134 | ai:claude
pub(crate) fn active(root: &Path) -> Vec<Outage> {
    let dir = crate::main_worktree_root_from(root).join(".aida/vendor-outages");
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let read = entries.filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok());
    let mut out: Vec<Outage> = read.filter(|o: &Outage| o.until > Utc::now()).collect();
    out.sort_by(|a, b| a.vendor.cmp(&b.vendor));
    out
}

/// The `vendor-outage` findings `aida doctor` and `aida status` show: each
/// active outage with its reset time and the lane it came from.
// trace:STORY-3134 | ai:claude
pub(crate) fn doctor_findings(root: &Path) -> Vec<crate::doctor_root::DoctorFinding> {
    let main = crate::main_worktree_root_from(root);
    let finding = |o: Outage| {
        let lane = o.lane.as_deref().map(|l| format!(", hit by lane {l}"));
        let until = o.until_text();
        crate::doctor_root::DoctorFinding {
            category: "vendor-outage".into(),
            id: o.vendor.clone(),
            summary: format!(
                "`{}` hit its usage limit{}; launches and reviews on it are refused until {until}",
                o.vendor,
                lane.unwrap_or_default()
            ),
            action: format!(
                "none: it clears on its own at {until}; to retry earlier, remove {}",
                path(&main, &o.vendor).display()
            ),
            safe_heal: false,
        }
    };
    active(root).into_iter().map(finding).collect()
}

/// End a lane stopped by a usage limit with no retained lease, once its
/// committed work is on a remote (`end` is `session end`, whose own guard
/// refuses commits no remote has). Uncommitted edits keep the lease, so they
/// are never discarded. Returns whether the lease ended, and the note.
// trace:STORY-3134 | ai:claude
pub(crate) fn end_lane(uncommitted: Option<usize>, end: &dyn Fn() -> Result<()>) -> (bool, String) {
    match uncommitted {
        Some(0) => match end() {
            Ok(()) => (true, "lease released".into()),
            Err(e) => (false, format!("lease kept: {e:#}")),
        },
        Some(n) => (
            false,
            format!("lease kept: {n} uncommitted file(s) in the seat"),
        ),
        None => (false, "lease kept: the seat could not be read".into()),
    }
}

/// `message` led by the lane's usage-limit note, when it has one, so a stop
/// on a usage limit always names itself.
// trace:STORY-3134 | ai:claude
pub(crate) fn exit_message(note: Option<&str>, message: String) -> String {
    match note {
        Some(note) => format!("{note}; {message}"),
        None => message,
    }
}

#[cfg(test)]
#[path = "tests/story_3134_vendor_outage_tests.rs"]
mod story_3134_vendor_outage_tests;
