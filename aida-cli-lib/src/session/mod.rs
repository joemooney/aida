//! `aida session list / resume` — enriched wrapper around Claude Code's
//! resume-session picker.
//!
//! Claude Code stores each session as `~/.claude/projects/<encoded-cwd>/
//! <session-id>.jsonl`. Each `.jsonl` carries an `ai-title` event with
//! the auto-generated subject and (when AIDA's TASK-1-022 SessionStart
//! hook fired with an active role) the role name in a system-message
//! body. We harvest both, render an enriched picker, and exec
//! `claude --resume <id>` on the user's choice.
//!
//! Subject text isn't user-controllable in Claude Code — this module
//! exists so the AIDA role can sit alongside the auto-subject without
//! waiting for an upstream feature.
//!
//! trace:FR-1-043 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// trace:TASK-3787 | ai:codex
// Textual includes preserve the existing private scope and crate::session API
// while separating the source by concern. No item visibility is widened.
include!("list.rs");
include!("interactive.rs");
include!("headless.rs");
include!("containment.rs");
include!("lease_lock.rs");
#[cfg(test)]
include!("inline_tests.rs");

// trace:BUG-2394 | ai:claude
#[cfg(test)]
#[path = "../tests/bug_2394_codex_posture_tests.rs"]
mod bug_2394_codex_posture_tests;

// trace:TASK-3784 | ai:codex
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) fn forget_contained_export<'a>(keys: impl Iterator<Item = &'a str>) {
        if keys.into_iter().any(|key| key == "AIDA_CONTAINED") {
            super::forget_contained_env_for_children();
        }
    }
}

// trace:TASK-3784 | ai:codex
#[cfg(test)]
mod testkit_adapter_tests {
    #[test]
    fn adapter_cleans_child_export_for_keys_added_after_snapshot() {
        let mut guard = crate::test_env::EnvVarsGuard::snapshot(&[]);
        guard.set_key("AIDA_CONTAINED", "0");
        super::set_contained_env_for_children(true);
        assert!(super::CONTAINED_ENV_APPLIED.lock().unwrap().is_some());
        drop(guard);
        let _lock = aida_testkit::test_env::env_lock();
        assert!(crate::test_env::holds_env_lock());
        assert!(super::CONTAINED_ENV_APPLIED.lock().unwrap().is_none());
    }
}
