//! Common publication implementation, not yet connected to production callers.
//!
//! Records and durable files are evidence, never launch permits. In particular,
//! this module deliberately has no vendor release API while the independently
//! reviewed transport and all enlisted writers remain integration dependencies.
// trace:BUG-1808 | ai:codex

#[cfg(target_os = "linux")]
mod durable;
#[cfg(target_os = "linux")]
mod effects;
#[cfg(target_os = "linux")]
mod observations;
mod preflight;
mod records;
