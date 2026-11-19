//! FAST source guards reuse the FULL shard tests without application dependencies.
// trace:TASK-3791 | ai:codex
#![cfg(test)]
#![allow(dead_code)]
extern crate self as aida_core;
#[path = "../aida-core/src/launch_env_keys.rs"]
pub mod launch_policy;
pub mod seat_clone {
    pub use crate::task_2143_clone_git_ratchet_tests as clone_git_ratchet_tests;
}
#[path = "../aida-cli-lib/src/tests/bug_1723_config_trust_tests.rs"]
pub mod bug_1723_config_trust_tests;
#[path = "../aida-cli-lib/src/tests/node_alias_display_ratchet_tests.rs"]
pub mod node_alias_display_ratchet_tests;
#[path = "../aida-cli-lib/src/tests/story_2326_launch_ratchet_tests.rs"]
pub mod story_2326_launch_ratchet_tests;
#[path = "../aida-cli-lib/src/tests/story_2401_forge_read_ratchet_tests.rs"]
pub mod story_2401_forge_read_ratchet_tests;
#[path = "../aida-cli-lib/src/tests/story_2539_vendor_literal_ratchet_tests.rs"]
pub mod story_2539_vendor_literal_ratchet_tests;
#[path = "../aida-cli-lib/src/tests/task_1555_ci_gate_tiers_tests.rs"]
pub mod task_1555_ci_gate_tiers_tests;
#[path = "../aida-cli-lib/src/tests/task_1750_liveness_primitive_guard_tests.rs"]
pub mod task_1750_liveness_primitive_guard_tests;
#[path = "../aida-cli-lib/src/gate/tests/task_1752_grant_env_guard_tests.rs"]
pub mod task_1752_grant_env_guard_tests;
#[path = "../aida-cli-lib/src/tests/task_2032_launcher_main_ratchet_tests.rs"]
pub mod task_2032_launcher_main_ratchet_tests;
#[path = "../aida-cli-lib/src/tests/task_2143_clone_git_ratchet_tests.rs"]
pub mod task_2143_clone_git_ratchet_tests;

#[path = "../aida-cli-lib/src/tests/resolve_aida_exe_ratchet_tests.rs"]
pub mod resolve_aida_exe_ratchet_tests;
