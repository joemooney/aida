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

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// Include the auto-generated embedded templates
include!(concat!(env!("OUT_DIR"), "/embedded_templates.rs"));

/// Information about an embedded template
#[derive(Debug, Clone)]
pub struct TemplateInfo {
    /// Key/path of the template (e.g., "skills/aida-req.md")
    pub key: String,
    /// Category (skills, commands, hooks, settings.json)
    pub category: String,
    /// Display name (derived from filename)
    pub name: String,
    /// Content of the template
    pub content: String,
    /// Source of the template
    pub source: TemplateSource,
}

/// Get all embedded templates with their info
pub fn get_embedded_templates() -> Vec<TemplateInfo> {
    EMBEDDED_TEMPLATES
        .iter()
        .map(|(key, content)| {
            let category = key.split('/').next().unwrap_or("other").to_string();
            let name = key.split('/').last().unwrap_or(key).to_string();
            TemplateInfo {
                key: key.to_string(),
                category,
                name,
                content: content.to_string(),
                source: TemplateSource::Embedded,
            }
        })
        .collect()
}

/// Get templates by category
pub fn get_templates_by_category(category: &str) -> Vec<TemplateInfo> {
    get_embedded_templates()
        .into_iter()
        .filter(|t| t.category == category || t.key == category)
        .collect()
}

/// Get template categories with descriptions
pub fn get_template_categories() -> &'static [(&'static str, &'static str)] {
    TEMPLATE_CATEGORIES
}

/// Template loader that checks external files first, then falls back to embedded.
///
/// Load order (highest priority first):
/// 1. Project-local `.aida/templates/`
/// 2. Organization `~/.config/aida/org-templates/`
/// 3. User config `~/.config/aida/templates/`
/// 4. Embedded templates (compiled into binary)
pub struct TemplateLoader {
    /// Project-local template directory (highest priority)
    project_templates: Option<PathBuf>,
    /// Organization template directory (shared org-wide policies)
    org_templates: Option<PathBuf>,
    /// User config template directory
    user_templates: Option<PathBuf>,
    /// Cache of loaded templates
    cache: HashMap<String, String>,
}

impl TemplateLoader {
    /// Create a new template loader
    pub fn new() -> Self {
        let config_dir = dirs::config_dir();
        let user_templates = config_dir.as_ref().map(|p| p.join("aida/templates"));
        let org_templates = config_dir.as_ref().map(|p| p.join("aida/org-templates"));

        Self {
            project_templates: None,
            org_templates,
            user_templates,
            cache: HashMap::new(),
        }
    }

    /// Create a template loader with a project root for local templates
    pub fn with_project_root(project_root: &Path) -> Self {
        let project_templates = Some(project_root.join(".aida/templates"));
        let config_dir = dirs::config_dir();
        let user_templates = config_dir.as_ref().map(|p| p.join("aida/templates"));
        let org_templates = config_dir.as_ref().map(|p| p.join("aida/org-templates"));

        Self {
            project_templates,
            org_templates,
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

    /// Load from external file locations (project → org → user)
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

        // Try organization templates
        if let Some(ref org_dir) = self.org_templates {
            let path = org_dir.join(key);
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
        for dir in [
            self.project_templates.as_ref(),
            self.org_templates.as_ref(),
            self.user_templates.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if dir.join(key).exists() {
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
        for dir in [
            self.project_templates.as_ref(),
            self.org_templates.as_ref(),
            self.user_templates.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
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

        // Check organization templates
        if let Some(ref org_dir) = self.org_templates {
            let path = org_dir.join(key);
            if path.exists() {
                return TemplateSource::Organization(path);
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
    /// Template from organization-wide ~/.config/aida/org-templates/
    Organization(PathBuf),
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
        assert!(EMBEDDED_TEMPLATES.contains_key("commands/aida-status.md"));
        assert!(EMBEDDED_TEMPLATES.contains_key("hooks/aida-commit-msg"));
    }

    #[test]
    fn test_template_loader_fallback() {
        let mut loader = TemplateLoader::new();
        // Should fall back to embedded
        let content = loader.load("skills/aida-req.md");
        assert!(content.is_some());
        assert!(content.unwrap().contains("AIDA"));
    }

    /// Skill/command parity invariant — every skill must have a matching
    /// slash command (with documented exceptions). Without this check,
    /// new skills can silently land without their command, leaving them
    /// undiscoverable in Claude Code. The other direction (command
    /// without skill) is also flagged. trace:TASK-18 | ai:claude
    #[test]
    fn test_skill_command_parity() {
        use std::collections::HashSet;

        // Documented exceptions: skill-only or command-only intentionally.
        // Keep this list short and re-evaluate any addition. (Today the
        // master templates have full parity, so the allowlist is empty.)
        let command_only: HashSet<&str> = HashSet::new();
        let skill_only: HashSet<&str> = ["aida-pickup"].iter().copied().collect();

        let skills: HashSet<String> = EMBEDDED_TEMPLATES
            .keys()
            .filter_map(|k| k.strip_prefix("skills/"))
            .filter_map(|k| k.strip_suffix(".md"))
            .map(|s| s.to_string())
            .collect();
        let commands: HashSet<String> = EMBEDDED_TEMPLATES
            .keys()
            .filter_map(|k| k.strip_prefix("commands/"))
            .filter_map(|k| k.strip_suffix(".md"))
            .map(|s| s.to_string())
            .collect();

        let missing_commands: Vec<&String> = skills
            .difference(&commands)
            .filter(|s| !skill_only.contains(s.as_str()))
            .collect();
        let missing_skills: Vec<&String> = commands
            .difference(&skills)
            .filter(|s| !command_only.contains(s.as_str()))
            .collect();

        assert!(
            missing_commands.is_empty(),
            "skills without matching commands (add the command file or allowlist in skill_only): {:?}",
            missing_commands
        );
        assert!(
            missing_skills.is_empty(),
            "commands without matching skills (add the skill file or allowlist in command_only): {:?}",
            missing_skills
        );
    }

    /// Glyph-convention regression guard — the skill end-of-session
    /// Path/What happens/Why tables must render the refined glyph set
    /// (`▶` primary, `⇒` alternative, `⏸` pause/stop), never the original
    /// `▶ ⏵ 🚪` set. The glyphs are visible in every skill's
    /// end-of-session menu, so a stale glyph is a wrong semantic signal
    /// (`🚪` reads "abandon"; `⏸` reads "resume later"). trace:BUG-116 | ai:claude
    #[test]
    fn test_skill_template_glyphs() {
        // The retired glyphs must not reappear in any skill template.
        const RETIRED: &[(char, &str)] = &[
            ('⏵', "alternate-path glyph ⏵ — use ⇒ instead"),
            ('🚪', "stop/exit glyph 🚪 — use ⏸ instead"),
        ];
        for skill in ["aida-pickup.md", "aida-pr.md", "aida-review.md"] {
            let key = format!("skills/{skill}");
            let content = EMBEDDED_TEMPLATES
                .get(key.as_str())
                .unwrap_or_else(|| panic!("missing embedded template: {key}"));
            for &(glyph, why) in RETIRED {
                assert!(!content.contains(glyph), "{key} still uses retired {why}");
            }
            // Every skill carries a Path/What happens/Why table, so both
            // refined glyphs must be present after the BUG-116 swap.
            assert!(
                content.contains('⇒'),
                "{key} is missing the ⇒ alternative-path glyph"
            );
            assert!(
                content.contains('⏸'),
                "{key} is missing the ⏸ pause/stop glyph"
            );
        }

        // Snapshot two concrete rendered rows so the glyphs are asserted
        // inside a real Path/What happens/Why table, not just anywhere.
        let pickup = EMBEDDED_TEMPLATES
            .get("skills/aida-pickup.md")
            .expect("aida-pickup.md embedded");
        assert!(
            pickup.contains("| ⇒ Wrap the batch as one PR |"),
            "aida-pickup.md batch-mode table should render the ⇒ alternative row"
        );
        assert!(
            pickup.contains("| ⏸ Stop here |"),
            "aida-pickup.md should render the ⏸ pause/stop row"
        );
    }

    /// BUG-280 regression guard — the `/aida-review` skill template must carry
    /// the headless-mode contract that prevents the PR-150 failure mode
    /// (reviewer posted a PASS comment then called AskUserQuestion and bailed
    /// before writing the verdict file). The contract has four invariants:
    /// (1) a top-level *Headless mode contract* section, (2) AskUserQuestion
    /// named as forbidden under headless, (3) the verdict-file write
    /// positioned before the PR comment post in document order, (4) step 8's
    /// merge confirm gated to skip under headless. The fourth is the load-
    /// bearing one — it is the prompt that crashed PR-150. trace:BUG-280 |
    /// ai:claude
    #[test]
    fn aida_review_carries_bug_280_headless_contract() {
        let review = EMBEDDED_TEMPLATES
            .get("skills/aida-review.md")
            .expect("aida-review.md embedded");

        // (1) Top-level Headless mode contract section is present.
        assert!(
            review.contains("## Headless mode contract"),
            "aida-review.md missing the top-level `## Headless mode contract` section"
        );

        // (2) AskUserQuestion is named as forbidden under headless. The
        // section's "AskUserQuestion is forbidden" phrasing is the load-
        // bearing bit — drift past it lets the skill template re-acquire
        // the BUG-280 failure mode.
        assert!(
            review.contains("AskUserQuestion is forbidden"),
            "aida-review.md must state `AskUserQuestion is forbidden` in the headless contract"
        );

        // (3) Verdict file write precedes the PR comment post in document
        // order. Step 6a (verdict file) MUST appear before step 7 (PR
        // comment) — reversing the order is the exact BUG-280 failure
        // mode (PASS comment posted, no verdict file on disk).
        let step_6a = review.find("### 6a. Write the verdict file");
        let step_7 = review.find("### 7. Post a consolidated review comment");
        let (Some(s6a), Some(s7)) = (step_6a, step_7) else {
            panic!(
                "aida-review.md must define both `### 6a. Write the verdict file` \
                 and `### 7. Post a consolidated review comment` headings — got \
                 6a={:?} 7={:?}",
                step_6a, step_7
            );
        };
        assert!(
            s6a < s7,
            "verdict file write (step 6a) must appear BEFORE the PR comment \
             post (step 7) — reversing is BUG-280's exact failure mode"
        );

        // (4) Step 8 (merge confirm) is gated to skip under headless. The
        // gate is the prompt that crashed PR-150 — without it, the model
        // calls AskUserQuestion and dies.
        let step_8_idx = review
            .find("### 8. Confirm with the user before merge")
            .expect("aida-review.md must define `### 8. Confirm with the user before merge`");
        // Find the next section after step 8 to bound the search to step 8's body.
        let step_8_end = review[step_8_idx..]
            .find("\n### ")
            .map(|n| step_8_idx + n)
            .unwrap_or(review.len());
        let step_8_body = &review[step_8_idx..step_8_end];
        assert!(
            step_8_body.contains("AIDA_HEADLESS=1"),
            "step 8 must gate on AIDA_HEADLESS=1 to skip the merge confirm — \
             otherwise the headless reviewer calls AskUserQuestion (BUG-280)"
        );
        assert!(
            step_8_body.contains("SKIP") || step_8_body.contains("skip"),
            "step 8's headless gate must SKIP the merge confirm (not just \
             warn) — only skipping prevents AskUserQuestion from firing"
        );
    }

    /// BUG-280 regression guard — the example verdict-file JSON that the
    /// `/aida-review` skill embeds must be valid JSON with the schema the
    /// orchestrator's `read_verdict_file` accepts (the `verdict` field is
    /// the only load-bearing one; everything else is metadata). A drift in
    /// the example that produces invalid JSON or omits `verdict` would
    /// silently let a real reviewer copy a broken template. trace:BUG-280
    /// | ai:claude
    #[test]
    fn aida_review_embeds_a_parseable_verdict_file_example() {
        let review = EMBEDDED_TEMPLATES
            .get("skills/aida-review.md")
            .expect("aida-review.md embedded");

        // Locate the first verdict-file write under step 6a (which lives
        // before the PR-comment section — assertion (3) above pins the
        // order). The first cat-EOF block in step 6a is the canonical
        // example; later blocks demonstrate variants like the merge
        // escalation handshake.
        let step_6a_idx = review
            .find("### 6a. Write the verdict file")
            .expect("step 6a heading present (asserted elsewhere)");
        let after_6a = &review[step_6a_idx..];

        // Extract every single-line `{"verdict": ...}` JSON literal in
        // the section and parse them all. Single-line is the right filter:
        // the canonical heredoc examples are all on one line, and a
        // multi-line `{...}` in the template is necessarily prose (e.g. a
        // pseudo-JSON comment illustrating the `findings_filed` re-write
        // shape — that block intentionally embeds shell `#` comments, so
        // it is not literal JSON).
        let mut examples = Vec::new();
        let mut rest = after_6a;
        while let Some(open) = rest.find("{\"verdict\":") {
            let body = &rest[open..];
            // A single-line literal closes with `}` before the next `\n`.
            let newline = body.find('\n').unwrap_or(body.len());
            let line = &body[..newline];
            if let Some(close) = line.find('}') {
                examples.push(&line[..=close]);
            }
            rest = &body[newline.min(body.len())..];
        }
        assert!(
            !examples.is_empty(),
            "aida-review.md step 6a must embed at least one \
             `{{\"verdict\": ...}}` example for the skill to copy"
        );
        for (i, json) in examples.iter().enumerate() {
            let parsed: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| {
                panic!(
                    "aida-review.md verdict example #{i} is not valid JSON: \
                     {e}\n  example: {json}"
                )
            });
            let verdict = parsed.get("verdict").and_then(|v| v.as_str());
            assert!(
                matches!(verdict, Some("Approved" | "RequestChanges" | "Rejected")),
                "aida-review.md verdict example #{i} must carry a \
                 `verdict` field of Approved/RequestChanges/Rejected — \
                 got {:?}",
                verdict
            );
        }
    }
}
