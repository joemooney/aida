//! Informational Claude tool-hook notices; no graph/cache/storage handle.
// trace:BUG-2014 | ai:codex
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    epoch: String,
    // Highest observed threshold: 1 = 2h, 2 = 30m, 3 = expired.
    crossed: u8,
}

fn threshold(left: Duration) -> u8 {
    if left <= Duration::zero() {
        3
    } else if left <= Duration::minutes(30) {
        2
    } else if left <= Duration::hours(2) {
        1
    } else {
        0
    }
}

fn claim(dir: &Path, id: &str, epoch: &str, crossing: u8) -> Result<bool> {
    if crossing == 0 {
        return Ok(false);
    }
    // Permanent lock inode, never renamed or unlinked. Nonblocking contention
    // defers to the next tool call; the shell hard timeout is the outer bound.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("{id}.notice.lock")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    let path = dir.join(format!("{id}.notice.json"));
    let mut ledger: Ledger = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid expiry notice ledger")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(error) => return Err(error.into()),
    };
    if ledger.epoch != epoch {
        ledger = Ledger {
            epoch: epoch.into(),
            crossed: 0,
        };
    }
    if crossing <= ledger.crossed {
        return Ok(false);
    }
    ledger.crossed = crossing;
    aida_core::write_atomic(&path, serde_json::to_vec(&ledger)?)?;
    Ok(true)
}

fn notice(root: &Path, now: DateTime<Utc>) -> Result<Option<String>> {
    let Some(grant) = crate::seat_authority::notice_grant(root, now)? else {
        return Ok(None);
    };
    let crossing = threshold(grant.expiry.expires_at - now);
    // Expiry changes also reset the epoch for an in-place renewal.
    let epoch = format!("{}:{}", grant.id, grant.expiry.expires_at);
    if !claim(
        &crate::seat_authority::grant_dir()?,
        &grant.id,
        &epoch,
        crossing,
    )? {
        return Ok(None);
    }
    let expiry = grant.expiry;
    // Existing warning's strict '<' boundary needs one millisecond at 2h.
    Ok(crate::seat_authority::expiry_warning(
        &expiry,
        now,
        Duration::hours(2) + Duration::milliseconds(1),
    ))
}

pub(crate) fn handle() -> Result<()> {
    let root = crate::find_project_root().unwrap_or_else(|_| ".".into());
    match notice(&root, Utc::now()) {
        Ok(Some(message)) => println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": { "hookEventName": "PreToolUse", "additionalContext": message }
            })
        ),
        Ok(None) => {}
        // No stdout on ledger failure, no refusal or permission decision.
        Err(error) => eprintln!("Expiry notice unavailable: {error:#}"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug_2014_grant_notice_tests.rs"]
mod tests;
