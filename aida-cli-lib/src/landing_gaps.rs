//! Shared inter-landing gap metrics for throughput and integration readers.
// trace:STORY-2844 | ai:codex
use chrono::{DateTime, Duration, Utc};

/// Elapsed time since the last landing, for live health checks. Future
/// timestamps have zero age; absent evidence remains unknown rather than
/// inventing an idle interval.
// trace:STORY-2849 | ai:claude
pub(crate) fn since_last(
    last_landing: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<Duration> {
    last_landing.map(|last| (now - last).max(Duration::zero()))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LandingGaps {
    pub landings: usize,
    pub longest_gap_seconds: Option<i64>,
    pub gaps_over_60_minutes: usize,
}

/// Include both window endpoints and count only gaps between included landings.
/// Input order is arbitrary; quiet time at the window edges is excluded.
pub(crate) fn summarize(
    times: impl IntoIterator<Item = DateTime<Utc>>,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> LandingGaps {
    let mut times: Vec<_> = times
        .into_iter()
        .filter(|t| *t >= since && *t <= until)
        .collect();
    times.sort_unstable();
    let mut result = LandingGaps {
        landings: times.len(),
        ..Default::default()
    };
    for pair in times.windows(2) {
        let seconds = (pair[1] - pair[0]).num_seconds();
        result.longest_gap_seconds = Some(
            result
                .longest_gap_seconds
                .map_or(seconds, |old| old.max(seconds)),
        );
        result.gaps_over_60_minutes += usize::from(seconds > 3600);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_shared_gaps_filter_sort_and_preserve_strict_threshold() {
        let since = DateTime::from_timestamp(0, 0).unwrap();
        let until = since + Duration::hours(4);
        let at = |seconds| since + Duration::seconds(seconds);
        let result = summarize(
            [until, at(-1), at(3600), since, at(3600), at(14401)],
            since,
            until,
        );
        assert_eq!(
            result,
            LandingGaps {
                landings: 4,
                longest_gap_seconds: Some(10800),
                gaps_over_60_minutes: 1
            }
        );
        assert_eq!(summarize([], since, until), LandingGaps::default());
        assert_eq!(
            summarize([at(3600)], since, until).longest_gap_seconds,
            None
        );
        assert_eq!(
            summarize([since, at(3600)], since, until).gaps_over_60_minutes,
            0
        );
    }
}
