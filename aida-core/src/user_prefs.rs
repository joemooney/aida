// trace:STORY-44 | ai:claude
//! User-level preferences at `~/.aida/preferences.toml`.
//!
//! These are machine-wide defaults that `aida init` consults so a user can
//! say "always try `JM` as my node id" without retyping it per project. Per-
//! project values still win — the prefs file only seeds the prompt.
//!
//! Layout:
//! ```toml
//! preferred_node_id = "JM"      # tried first by `aida init`
//! email = "joe@example.com"     # default if `git config user.email` is unset
//! ```
//!
//! Missing file → empty defaults. We don't auto-create the file; the user
//! either edits it or runs `aida config user --node-id ...`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// On-disk shape of `~/.aida/preferences.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserPreferences {
    /// The node id `aida init` should try to claim by default. Validated
    /// with `node::validate_node_id` before writing. None means "fall
    /// through to sequential numeric (legacy behavior)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_node_id: Option<String>,

    /// Email stamp used when `git config user.email` is unset. None means
    /// "no fallback".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl UserPreferences {
    /// `~/.aida/preferences.toml`. Returns None if the home dir is unknown
    /// (e.g., HOME unset on a CI runner).
    pub fn path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".aida").join("preferences.toml"))
    }

    /// Load from `~/.aida/preferences.toml`, returning defaults when the
    /// file is missing. A malformed file is an error — better to surface
    /// it loudly than silently lose the user's settings.
    pub fn load() -> Result<Self> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// Write to `~/.aida/preferences.toml`, creating the parent dir if
    /// needed. Caller is responsible for validating fields (e.g.,
    /// `node::validate_node_id`).
    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::path()
            .context("Cannot determine home directory for preferences file")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self)
            .context("serializing preferences")?;
        std::fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// True when the prefs file has no fields set — equivalent to "no
    /// preferences saved." Used by `aida config user` to decide whether
    /// to print "(no preferences set)" vs the field list.
    pub fn is_empty(&self) -> bool {
        self.preferred_node_id.is_none() && self.email.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serialization() {
        let prefs = UserPreferences {
            preferred_node_id: Some("JM".into()),
            email: Some("joe@example.com".into()),
        };
        let s = toml::to_string_pretty(&prefs).unwrap();
        assert!(s.contains("preferred_node_id = \"JM\""));
        assert!(s.contains("email = \"joe@example.com\""));
        let back: UserPreferences = toml::from_str(&s).unwrap();
        assert_eq!(back.preferred_node_id.as_deref(), Some("JM"));
        assert_eq!(back.email.as_deref(), Some("joe@example.com"));
    }

    #[test]
    fn empty_prefs_serialize_to_blank() {
        let prefs = UserPreferences::default();
        assert!(prefs.is_empty());
        let s = toml::to_string_pretty(&prefs).unwrap();
        assert!(!s.contains("preferred_node_id"));
        assert!(!s.contains("email"));
    }

    #[test]
    fn missing_file_yields_default() {
        // Can't reliably stub the home dir, but loading without writing
        // anywhere should at worst return defaults if the test runner's
        // home doesn't have a prefs file. trace:STORY-44 | ai:claude
        let path = UserPreferences::path();
        if let Some(p) = path {
            if !p.exists() {
                let prefs = UserPreferences::load().unwrap();
                assert!(prefs.is_empty());
            }
        }
    }
}
