//! One agent launcher (STORY-2326): the launch is resolved once into a
//! [`LaunchContext`], and [`spawn::spawn_agent`] is the constructor that turns
//! it into a process. The seat a child runs as is assigned only through
//! [`spawn::assign_seat`].
//!
//! Slice A converts the Claude contained launch paths. PR-B converts fresh
//! Codex session/exec, compete and dispatch plus Native/None phase builders.
//! Vendor profiles finalize commands after seat and identity setup.
//! Claude children now receive a cleared, projected environment. Other vendor
//! cutovers, resume paths and resource renderers remain later slices; the
//! ratchet ledger inventories unconverted sites.
// trace:STORY-2326 | ai:claude

// TASK-2708: the contained (AIDA bubblewrap) headless launch. trace:TASK-2708 | ai:claude
pub(crate) mod contained;
// AGY adapter policy is present, but its admission gate remains closed.
// trace:TASK-3048 | ai:codex
pub(crate) mod agy;
pub(crate) mod context;
pub(crate) mod control;
mod grant;
pub(crate) mod profile;
pub(crate) use control::control_child;
pub(crate) use grant::{
    assign_advisor_child, issue_launch_child, issue_launch_child_with_delegation,
    review_watch_controller,
};
pub(crate) mod parent;
// Consumed by slice C's renderers. trace:STORY-2326 | ai:claude
#[allow(dead_code)]
pub(crate) mod resources;
pub(crate) mod spawn;

pub(crate) use context::{LaunchContext, LaunchRefusal, LaunchRequest};
pub(crate) use spawn::{assign_seat, spawn_agent, SeatDecision};

#[cfg(test)]
#[path = "../tests/story_2326_launch_tests.rs"]
mod story_2326_launch_tests;

#[cfg(test)]
#[path = "../tests/story_2326_launch_ratchet_tests.rs"]
mod story_2326_launch_ratchet_tests;
