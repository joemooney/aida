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
        }
    }

    pub(crate) fn describe(&self) -> String {
        let until = self.until.with_timezone(&Local).format("%Y-%m-%d %H:%M %Z");
        let hold = (!self.until_known).then_some(" (no reset time given)");
        let hold = hold.unwrap_or_default();
        let (vendor, message) = (&self.vendor, &self.message);
        format!("`{vendor}` is unavailable until {until}{hold} ({message})")
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

#[cfg(test)]
#[path = "tests/story_3134_vendor_outage_tests.rs"]
mod story_3134_vendor_outage_tests;
