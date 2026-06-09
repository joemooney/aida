// trace:ARCH-distributed-hlc | ai:claude
//! Hybrid Logical Clock (HLC) implementation for distributed timestamp ordering.
//!
//! HLC combines wall clock time with a logical counter to provide timestamps that:
//! - Stay close to wall time (unlike pure Lamport clocks)
//! - Preserve causality (unlike raw wall clocks under clock skew)
//! - Are totally ordered (wall_time, counter, node_id)
//!
//! Reference: "Logical Physical Clocks and Consistent Snapshots in Globally
//! Distributed Databases" (Kulkarni et al., 2014)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// A Hybrid Logical Clock timestamp.
///
/// Ordering: (wall_time, counter, node_id) — fully deterministic.
/// Two HLC timestamps from different nodes are never equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlcTimestamp {
    /// Wall clock component (milliseconds since Unix epoch)
    pub wall_time_ms: i64,
    /// Logical counter — incremented when wall clock hasn't advanced
    pub counter: u32,
    /// Node that generated this timestamp (0 for centralized mode)
    pub node_id: u32,
}

impl HlcTimestamp {
    /// Create a timestamp from components.
    pub fn new(wall_time_ms: i64, counter: u32, node_id: u32) -> Self {
        Self {
            wall_time_ms,
            counter,
            node_id,
        }
    }

    /// Convert to a chrono DateTime (loses counter and node_id precision).
    pub fn to_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.wall_time_ms).unwrap_or_else(Utc::now)
    }

    /// Create from a chrono DateTime (sets counter=0, node_id=0).
    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Self {
            wall_time_ms: dt.timestamp_millis(),
            counter: 0,
            node_id: 0,
        }
    }
}

impl PartialOrd for HlcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HlcTimestamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.wall_time_ms
            .cmp(&other.wall_time_ms)
            .then(self.counter.cmp(&other.counter))
            .then(self.node_id.cmp(&other.node_id))
    }
}

impl std::fmt::Display for HlcTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.wall_time_ms, self.counter, self.node_id)
    }
}

/// The HLC clock instance. Thread-safe via internal Mutex.
///
/// Usage:
/// ```
/// use aida_core::hlc::Hlc;
///
/// let clock = Hlc::new(7); // node_id = 7
/// let ts1 = clock.now();
/// let ts2 = clock.now();
/// assert!(ts2 > ts1);
/// ```
pub struct Hlc {
    node_id: u32,
    state: Mutex<HlcState>,
}

struct HlcState {
    last_wall_time_ms: i64,
    last_counter: u32,
}

impl Hlc {
    /// Create a new HLC for the given node.
    /// Use node_id=0 for centralized (single-server) mode.
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            state: Mutex::new(HlcState {
                last_wall_time_ms: 0,
                last_counter: 0,
            }),
        }
    }

    /// Generate a new timestamp guaranteed to be greater than any previous
    /// timestamp from this clock or any received timestamp.
    pub fn now(&self) -> HlcTimestamp {
        let physical = Utc::now().timestamp_millis();
        let mut state = self.state.lock().unwrap();

        if physical > state.last_wall_time_ms {
            // Wall clock advanced — reset counter
            state.last_wall_time_ms = physical;
            state.last_counter = 0;
        } else {
            // Wall clock hasn't advanced (same ms or skew) — increment counter.
            // saturating_add so a pathological burst (>u32::MAX events in one ms)
            // can never panic (debug) or wrap to 0 (release) and break
            // monotonicity. trace:TASK-712
            state.last_counter = state.last_counter.saturating_add(1);
        }

        HlcTimestamp {
            wall_time_ms: state.last_wall_time_ms,
            counter: state.last_counter,
            node_id: self.node_id,
        }
    }

    /// Update the clock after receiving a remote timestamp.
    /// Returns a new timestamp that is causally after both the local clock
    /// and the received timestamp.
    pub fn receive(&self, remote: &HlcTimestamp) -> HlcTimestamp {
        let physical = Utc::now().timestamp_millis();
        let mut state = self.state.lock().unwrap();

        if physical > state.last_wall_time_ms && physical > remote.wall_time_ms {
            // Physical clock is ahead of everything — use it
            state.last_wall_time_ms = physical;
            state.last_counter = 0;
        } else if remote.wall_time_ms > state.last_wall_time_ms {
            // Remote is ahead — adopt its wall time. saturating_add guards
            // against a remote counter == u32::MAX. trace:TASK-712
            state.last_wall_time_ms = remote.wall_time_ms;
            state.last_counter = remote.counter.saturating_add(1);
        } else if state.last_wall_time_ms > remote.wall_time_ms {
            // We're ahead — just increment our counter. trace:TASK-712
            state.last_counter = state.last_counter.saturating_add(1);
        } else {
            // Same wall time — take max counter + 1 (saturating). trace:TASK-712
            state.last_counter =
                std::cmp::max(state.last_counter, remote.counter).saturating_add(1);
        }

        HlcTimestamp {
            wall_time_ms: state.last_wall_time_ms,
            counter: state.last_counter,
            node_id: self.node_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monotonic_ordering() {
        let clock = Hlc::new(7);
        let ts1 = clock.now();
        let ts2 = clock.now();
        let ts3 = clock.now();
        assert!(ts2 > ts1);
        assert!(ts3 > ts2);
    }

    #[test]
    fn test_receive_advances_past_remote() {
        let clock_a = Hlc::new(7);
        let clock_b = Hlc::new(11);

        let ts_a = clock_a.now();
        let ts_b = clock_b.receive(&ts_a);

        // ts_b must be after ts_a
        assert!(ts_b > ts_a);
        // ts_b should have node_id 11
        assert_eq!(ts_b.node_id, 11);
    }

    #[test]
    fn test_total_ordering_different_nodes() {
        // Two timestamps with same wall_time and counter but different node_id
        let ts_a = HlcTimestamp::new(1000, 0, 7);
        let ts_b = HlcTimestamp::new(1000, 0, 11);
        // They should not be equal
        assert_ne!(ts_a, ts_b);
        // One must be less than the other (deterministic)
        assert!(ts_a < ts_b); // node 7 < node 11
    }

    #[test]
    fn test_display() {
        let ts = HlcTimestamp::new(1710000000000, 5, 7);
        assert_eq!(ts.to_string(), "1710000000000:5:7");
    }

    #[test]
    fn test_datetime_roundtrip() {
        let now = Utc::now();
        let ts = HlcTimestamp::from_datetime(now);
        let back = ts.to_datetime();
        // Within 1ms due to millisecond truncation
        assert!((now - back).num_milliseconds().abs() <= 1);
    }

    #[test]
    fn test_centralized_mode() {
        let clock = Hlc::new(0); // centralized mode
        let ts = clock.now();
        assert_eq!(ts.node_id, 0);
    }

    // trace:TASK-712 — receiving a remote whose counter is at u32::MAX must not
    // panic (debug overflow) nor wrap to 0 (release) and break monotonicity.
    #[test]
    fn test_receive_remote_counter_at_max_does_not_overflow() {
        let clock = Hlc::new(7);
        // Force the local clock's wall time to match the remote so we take the
        // "same wall time — max(counter)+1" branch with a maxed remote counter.
        let now_ms = Utc::now().timestamp_millis();
        let remote = HlcTimestamp::new(now_ms, u32::MAX, 11);
        let ts = clock.receive(&remote);
        // saturating_add clamps at u32::MAX rather than wrapping/panicking.
        assert_eq!(ts.counter, u32::MAX);
        assert!(ts.wall_time_ms >= remote.wall_time_ms);
    }

    // trace:TASK-712 — the "remote is ahead" branch also guards a maxed counter.
    #[test]
    fn test_receive_future_remote_counter_at_max_does_not_overflow() {
        let clock = Hlc::new(7);
        // Remote wall time well in the future so we take the "adopt remote wall
        // time" branch, with a maxed remote counter.
        let future_ms = Utc::now().timestamp_millis() + 1_000_000;
        let remote = HlcTimestamp::new(future_ms, u32::MAX, 11);
        let ts = clock.receive(&remote);
        assert_eq!(ts.wall_time_ms, future_ms);
        assert_eq!(ts.counter, u32::MAX);
    }
}
