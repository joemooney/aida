//! Block-allocation config — knobs read from `[block_allocation]` and
//! `[block_allocation.<type>]` sections of `.aida/config.toml`. Drives
//! `aida add`'s auto-claim behaviour (TASK-281): when the aggregate
//! remaining IDs for a (node, type) pair drops below the configured
//! threshold, the next spec-creating command quietly claims a fresh
//! block instead of nagging the user with a "claim soon" warning.
//!
//! trace:TASK-281 | ai:claude

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Global + per-type knobs governing auto-claim.
///
/// `auto_claim` defaults to true: a project that never edits
/// `.aida/config.toml` gets auto-claim out of the box. Per-type sections
/// override the global value; `None` on a per-type field means "inherit".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockAllocationConfig {
    #[serde(default = "default_auto_claim")]
    pub auto_claim: bool,
    #[serde(flatten)]
    pub per_type: HashMap<String, BlockAllocationTypeConfig>,
}

fn default_auto_claim() -> bool {
    true
}

impl Default for BlockAllocationConfig {
    fn default() -> Self {
        Self {
            auto_claim: true,
            per_type: HashMap::new(),
        }
    }
}

/// Per-type overrides. Any field left `None` falls back to the global
/// `auto_claim` value (for the bool) or the built-in defaults
/// (threshold = 20, size = 100).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockAllocationTypeConfig {
    pub auto_claim: Option<bool>,
    pub auto_claim_threshold: Option<u32>,
    pub auto_claim_size: Option<u32>,
}

impl BlockAllocationConfig {
    /// Built-in default — claim a new block when aggregate remaining drops
    /// below 20. Matches the existing `BlockRegistry::AGGREGATE_LOW_THRESHOLD`
    /// so the auto-claim trigger coincides with what *would have been* the
    /// "running low" warning.
    pub const DEFAULT_THRESHOLD: u32 = 20;

    /// Built-in default — claim 100 IDs at a time, matching the per-type
    /// block size used by `auto_allocate_initial_blocks` for the
    /// PHASE3_AUTO_ALLOC_TYPES list.
    pub const DEFAULT_SIZE: u32 = 100;

    /// True when auto-claim is on for this type. A per-type explicit
    /// false wins over the global default; otherwise inherit global.
    /// Type prefix lookup is case-insensitive.
    pub fn is_enabled_for(&self, type_prefix: &str) -> bool {
        match self.lookup(type_prefix).and_then(|t| t.auto_claim) {
            Some(v) => v,
            None => self.auto_claim,
        }
    }

    /// Threshold for this type. Per-type override beats the built-in default.
    pub fn threshold_for(&self, type_prefix: &str) -> u32 {
        self.lookup(type_prefix)
            .and_then(|t| t.auto_claim_threshold)
            .unwrap_or(Self::DEFAULT_THRESHOLD)
    }

    /// Claim size for this type. Per-type override beats the built-in default.
    pub fn size_for(&self, type_prefix: &str) -> u32 {
        self.lookup(type_prefix)
            .and_then(|t| t.auto_claim_size)
            .unwrap_or(Self::DEFAULT_SIZE)
    }

    fn lookup(&self, type_prefix: &str) -> Option<&BlockAllocationTypeConfig> {
        let lower = type_prefix.to_ascii_lowercase();
        self.per_type
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&lower))
            .map(|(_, v)| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_allocation_config_defaults_enable_with_threshold_20_size_100() {
        let cfg = BlockAllocationConfig::default();
        assert!(cfg.is_enabled_for("BUG"));
        assert!(cfg.is_enabled_for("TASK"));
        assert_eq!(cfg.threshold_for("BUG"), 20);
        assert_eq!(cfg.size_for("BUG"), 100);
    }

    #[test]
    fn block_allocation_global_opt_out_disables_all_types() {
        let cfg = BlockAllocationConfig {
            auto_claim: false,
            per_type: HashMap::new(),
        };
        assert!(!cfg.is_enabled_for("BUG"));
        assert!(!cfg.is_enabled_for("TASK"));
        assert!(!cfg.is_enabled_for("*"));
    }

    #[test]
    fn block_allocation_per_type_opt_out_disables_only_that_type() {
        let mut per_type = HashMap::new();
        per_type.insert(
            "bug".to_string(),
            BlockAllocationTypeConfig {
                auto_claim: Some(false),
                ..Default::default()
            },
        );
        let cfg = BlockAllocationConfig {
            auto_claim: true,
            per_type,
        };
        assert!(!cfg.is_enabled_for("BUG"));
        assert!(!cfg.is_enabled_for("bug"));
        assert!(cfg.is_enabled_for("TASK"));
    }

    #[test]
    fn block_allocation_per_type_override_threshold_and_size() {
        let mut per_type = HashMap::new();
        per_type.insert(
            "story".to_string(),
            BlockAllocationTypeConfig {
                auto_claim: None,
                auto_claim_threshold: Some(50),
                auto_claim_size: Some(200),
            },
        );
        let cfg = BlockAllocationConfig {
            auto_claim: true,
            per_type,
        };
        assert_eq!(cfg.threshold_for("STORY"), 50);
        assert_eq!(cfg.size_for("STORY"), 200);
        // Other types still use the built-in defaults.
        assert_eq!(cfg.threshold_for("BUG"), 20);
        assert_eq!(cfg.size_for("BUG"), 100);
    }

    #[test]
    fn block_allocation_type_prefix_case_insensitive() {
        let mut per_type = HashMap::new();
        per_type.insert(
            "BUG".to_string(),
            BlockAllocationTypeConfig {
                auto_claim_threshold: Some(7),
                ..Default::default()
            },
        );
        let cfg = BlockAllocationConfig {
            auto_claim: true,
            per_type,
        };
        assert_eq!(cfg.threshold_for("bug"), 7);
        assert_eq!(cfg.threshold_for("BuG"), 7);
        assert_eq!(cfg.threshold_for("BUG"), 7);
    }

    #[test]
    fn block_allocation_global_default_when_per_type_auto_claim_is_none() {
        // Per-type section exists with overrides but no explicit auto_claim;
        // should inherit the global flag.
        let mut per_type = HashMap::new();
        per_type.insert(
            "bug".to_string(),
            BlockAllocationTypeConfig {
                auto_claim: None,
                auto_claim_threshold: Some(5),
                auto_claim_size: None,
            },
        );
        let cfg = BlockAllocationConfig {
            auto_claim: false,
            per_type,
        };
        // Inherits the global false even though a per-type section exists.
        assert!(!cfg.is_enabled_for("BUG"));
    }
}
