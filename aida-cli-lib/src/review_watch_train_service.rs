//! Train service using the same binding and supervision as review-watch.
// trace:STORY-2691 | ai:codex
use super::service::HostAuthority;
const TRAIN: bool = true;
include!("advisor_loop_service.rs");
