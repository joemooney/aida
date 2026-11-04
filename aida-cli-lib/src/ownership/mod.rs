//! One ownership and liveness record per piece of work (STORY-2325).
//!
//! Today a single piece of work is owned through up to six records (a session
//! lease, a review marker, a train owner line plus a merge lock, a claim, a
//! lane file, and the mailbox retention gate), and each one answers "is the
//! owner still running?" with its own rule. This module is the replacement's
//! foundation: one typed [`Ownership`] record per [`WorkKey`], and one pure
//! [`judge`] that turns a record plus injected host facts into a
//! [`Verdict`].
//!
//! PR-A added the types and the judge. PR-B adds the store
//! ([`store`]: one locked, generation-checked, journaled write path), the
//! transitions ([`ops`]), the dual-write call-site adapters ([`dual`]), the
//! one-time rewrite (`aida own migrate`, [`migrate`]) and the diagnostic
//! `aida own show --compare` ([`compare`]). No decision call site reads the
//! records yet: the cutover of every reader is PR-C.
//!
//! The judge is pure: it reads no file, process, git state or clock. The
//! caller injects the host it runs on ([`HostFacts`], including a process
//! probe), what it read from the holder's tree ([`TreeReading`]), the PR head
//! on origin, an ancestry test, and `now`.
//!
//! Binding rules from the advisor signoff:
//! - A1: witnesses are recorded from the host's view. [`process_witness`]
//!   records no process witness when the writer runs in a pid namespace other
//!   than the host's initial one (a sandboxed CLI call).
//! - A2: a process witness whose liveness cannot be read here (another host,
//!   another pid namespace, no start identity) abstains. The owner is Dead
//!   only when at least one witness is Dead and none is Alive; an Alive
//!   witness always wins.
//! - A3: a heartbeat expires only after at least three intervals and after
//!   the longest tool timeout ([`heartbeat_window`]).
//! - A7: [`Reclaim::Free`] means the work key may be acquired; it never
//!   licenses touching the old holder's tree.
//!
//! trace:STORY-2325 | ai:claude

// The readers are PR-C, so part of the judge's surface has no production
// caller yet.
#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod compare;
pub(crate) mod dual;
mod judge;
pub(crate) mod migrate;
pub(crate) mod ops;
pub(crate) mod store;
mod types;

#[allow(unused_imports)]
pub(crate) use judge::{
    heartbeat_window, judge, judge_owner, judge_witness, process_witness, LONGEST_TOOL_TIMEOUT_SECS,
};
#[allow(unused_imports)]
pub(crate) use types::{
    ChangeId, Holder, HostFacts, MailAnchorKey, OwnerVerdict, Ownership, Phase, Reclaim, Role,
    SeatTree, TreeKind, TreeReading, Verdict, Witness, WitnessKind, WitnessReading, WorkKey,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod writer_tests;
