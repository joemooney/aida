// trace:FR-0269 - Template loading system | ai:claude:high
//! Template loading system for scaffolding.
//!
//! Templates can be loaded from:
//! 1. External files (development mode, user customization)
//! 2. Embedded content (release mode, self-contained binary)
//!
//! Load order:
//! 1. Project-local `.aida/templates/` directory
//! 2. User config `~/.config/aida/templates/` directory
//! 3. Embedded templates (compiled into binary)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use once_cell::sync::Lazy;

// Include the auto-generated embedded templates
include!(concat!(env!("OUT_DIR"), "/embedded_templates.rs"));

/// Template loader that checks external files first, then falls back to embedded
pub struct TemplateLoader {
    /// Project-local template directory (highest priority)
    project_templates: Option<PathBuf>,
    /// User config template directory
    user_templates: Option<PathBuf>,
    /// Cache of loaded templates
    cache: HashMap<String, String>,
}

impl TemplateLoader {
    /// Create a new template loader
    pub fn new() -> Self {
        let user_templates = dirs::config_dir().map(|p| p.join("aida/templates"));

        Self {
            project_templates: None,
            user_templates,
            cache: HashMap::new(),
        }
    }

    /// Create a template loader with a project root for local templates
    pub fn with_project_root(project_root: &Path) -> Self {
        let project_templates = Some(project_root.join(".aida/templates"));
        let user_templates = dirs::config_dir().map(|p| p.join("aida/templates"));

        Self {
            project_templates,
            user_templates,
            cache: HashMap::new(),
        }
    }

    /// Load a template by key (e.g., "skills/aida-req.md")
    pub fn load(&mut self, key: &str) -> Option<String> {
        // Check cache first
        if let Some(content) = self.cache.get(key) {
            return Some(content.clone());
        }

        // Try external files first (project-local, then user config)
        if let Some(content) = self.load_external(key) {
            self.cache.insert(key.to_string(), content.clone());
            return Some(content);
        }

        // Fall back to embedded templates
        if let Some(content) = EMBEDDED_TEMPLATES.get(key) {
            let content = content.to_string();
            self.cache.insert(key.to_string(), content.clone());
            return Some(content);
        }

        None
    }

    /// Load from external file locations
    fn load_external(&self, key: &str) -> Option<String> {
        // Try project-local first
        if let Some(ref project_dir) = self.project_templates {
            let path = project_dir.join(key);
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    return Some(content);
                }
            }
        }

        // Try user config
        if let Some(ref user_dir) = self.user_templates {
            let path = user_dir.join(key);
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    return Some(content);
                }
            }
        }

        None
    }

    /// Check if a template is available (either external or embedded)
    pub fn has_template(&self, key: &str) -> bool {
        // Check external locations
        if let Some(ref project_dir) = self.project_templates {
            if project_dir.join(key).exists() {
                return true;
            }
        }
        if let Some(ref user_dir) = self.user_templates {
            if user_dir.join(key).exists() {
                return true;
            }
        }

        // Check embedded
        EMBEDDED_TEMPLATES.contains_key(key)
    }

    /// Get all available template keys
    pub fn list_templates(&self) -> Vec<String> {
        let mut keys: Vec<String> = EMBEDDED_TEMPLATES.keys().map(|k| k.to_string()).collect();

        // Add any external templates not in embedded
        for dir in [self.project_templates.as_ref(), self.user_templates.as_ref()].into_iter().flatten() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let prefix = entry.file_name().to_string_lossy().to_string();
                        if let Ok(subentries) = fs::read_dir(entry.path()) {
                            for subentry in subentries.flatten() {
                                let key = format!(
                                    "{}/{}",
                                    prefix,
                                    subentry.file_name().to_string_lossy()
                                );
                                if !keys.contains(&key) {
                                    keys.push(key);
                                }
                            }
                        }
                    }
                }
            }
        }

        keys.sort();
        keys
    }

    /// Extract all embedded templates to a directory
    pub fn extract_to(&self, dest: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut extracted = Vec::new();

        for (key, content) in EMBEDDED_TEMPLATES.iter() {
            let path = dest.join(key);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, content)?;
            extracted.push(path);
        }

        Ok(extracted)
    }

    /// Get the source of a template (for debugging/info)
    pub fn get_source(&self, key: &str) -> TemplateSource {
        // Check project-local first
        if let Some(ref project_dir) = self.project_templates {
            let path = project_dir.join(key);
            if path.exists() {
                return TemplateSource::ProjectLocal(path);
            }
        }

        // Check user config
        if let Some(ref user_dir) = self.user_templates {
            let path = user_dir.join(key);
            if path.exists() {
                return TemplateSource::UserConfig(path);
            }
        }

        // Check embedded
        if EMBEDDED_TEMPLATES.contains_key(key) {
            return TemplateSource::Embedded;
        }

        TemplateSource::NotFound
    }
}

impl Default for TemplateLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Source of a template
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateSource {
    /// Template from project-local .aida/templates/
    ProjectLocal(PathBuf),
    /// Template from user config ~/.config/aida/templates/
    UserConfig(PathBuf),
    /// Template embedded in binary
    Embedded,
    /// Template not found
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_templates_exist() {
        // Should have at least the core templates
        assert!(EMBEDDED_TEMPLATES.contains_key("skills/aida-req.md"));
        assert!(EMBEDDED_TEMPLATES.contains_key("skills/aida-implement.md"));
        assert!(EMBEDDED_TEMPLATES.contains_key("commands/status.md"));
        assert!(EMBEDDED_TEMPLATES.contains_key("hooks/commit-msg"));
    }

    #[test]
    fn test_template_loader_fallback() {
        let mut loader = TemplateLoader::new();
        // Should fall back to embedded
        let content = loader.load("skills/aida-req.md");
        assert!(content.is_some());
        assert!(content.unwrap().contains("AIDA"));
    }
}
