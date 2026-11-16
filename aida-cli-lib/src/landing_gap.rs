//! Shared landing-gap arithmetic for live health and historical throughput.
// trace:STORY-2849 | ai:codex
use chrono::{DateTime, Duration, Utc};

/// Elapsed time since the last landing. Future timestamps have zero age;
/// absent evidence remains unknown rather than inventing an idle interval.
pub fn landing_gap(last_landing: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<Duration> {
    last_landing.map(|last| (now - last).max(Duration::zero()))
}
