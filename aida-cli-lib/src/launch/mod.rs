//! One agent launcher (STORY-2326): the launch is resolved once into a
//! [`LaunchContext`], and [`spawn::spawn_agent`] is the constructor that turns
//! it into a process. The seat a child runs as is assigned only through
//! [`spawn::assign_seat`].
//!
//! Slice A (this module's first cut) converts the Claude contained launch
//! paths. Codex finalization, the resume paths, `dispatch.rs`, the declared
//! resource renderers and the per-vendor environment allowlist are later
//! slices; the unconverted sites are listed, exactly, in the ratchet ledger
//! (`tests/story_2326_launch_ratchet_tests.rs`). TASK-1749 C1: nothing here
//! clears the environment.
// trace:STORY-2326 | ai:claude

// TASK-2708: the contained (AIDA bubblewrap) headless launch. trace:TASK-2708 | ai:claude
pub(crate) mod contained;
pub(crate) mod context;
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
