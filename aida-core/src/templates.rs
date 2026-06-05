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

/// A classified `EMBEDDED_TEMPLATES` entry under the `skills/` prefix.
///
/// Skills come in two on-disk shapes (TASK-574):
///   * flat:        `skills/<name>.md`        — the prompt body itself.
///   * folder-form: `skills/<name>/SKILL.md`  — the prompt body, plus
///                  `skills/<name>/<support>` — helper files (`templates/`,
///                  `examples/`, scripts) shipped alongside the prompt.
///
/// Centralizing this mapping keeps the scaffolding loop and the skill↔command
/// parity invariant from re-deriving the convention divergently. trace:TASK-574
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillKey<'a> {
    /// Canonical skill name (folder name, or flat file stem), e.g. `aida-pr`.
    pub name: &'a str,
    /// Path relative to `.claude/skills/` the entry scaffolds to,
    /// e.g. `aida-pr/examples/pr-description-template.md`.
    pub rel_path: &'a str,
    /// True when this entry is the skill's prompt body (flat `.md` or
    /// folder-form `SKILL.md`); false for helper files.
    pub is_prompt: bool,
}

/// Classify an `EMBEDDED_TEMPLATES` key as a skill entry.
///
/// Returns `None` for keys outside the `skills/` prefix and for the
/// per-project local-extensions helper (`skills/local/README.md`), which is
/// scaffolding furniture, not a skill. trace:TASK-574
pub fn classify_skill_key(key: &str) -> Option<SkillKey<'_>> {
    let rest = key.strip_prefix("skills/")?;
    // The local/ extensions README is generated separately and is not a skill.
    if rest == "local/README.md" {
        return None;
    }
    match rest.split_once('/') {
        // Folder-form: skills/<name>/<subpath>
        Some((name, sub)) => Some(SkillKey {
            name,
            rel_path: rest,
            is_prompt: sub == "SKILL.md",
        }),
        // Flat: skills/<name>.md
        None => rest.strip_suffix(".md").map(|name| SkillKey {
            name,
            rel_path: rest,
            is_prompt: true,
        }),
    }
}

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
    fn test_folder_form_skill_embedded() {
        // aida-pr is the folder-form proof: SKILL.md plus an examples/ helper.
        // The recursive embed (build.rs) must preserve the subfolder structure
        // in the key. trace:TASK-574
        assert!(
            EMBEDDED_TEMPLATES.contains_key("skills/aida-pr/SKILL.md"),
            "folder-form skill prompt must embed as skills/aida-pr/SKILL.md"
        );
        assert!(
            EMBEDDED_TEMPLATES.contains_key("skills/aida-pr/examples/pr-description-template.md"),
            "folder-form skill helper files must embed under the skill folder"
        );
        // The flat key must NOT exist after migration.
        assert!(!EMBEDDED_TEMPLATES.contains_key("skills/aida-pr.md"));
    }

    #[test]
    fn test_classify_skill_key() {
        // Flat skill → prompt body, name == stem.
        let flat = classify_skill_key("skills/aida-req.md").expect("flat skill");
        assert_eq!(flat.name, "aida-req");
        assert_eq!(flat.rel_path, "aida-req.md");
        assert!(flat.is_prompt);

        // Folder-form prompt body.
        let prompt = classify_skill_key("skills/aida-pr/SKILL.md").expect("folder prompt");
        assert_eq!(prompt.name, "aida-pr");
        assert_eq!(prompt.rel_path, "aida-pr/SKILL.md");
        assert!(prompt.is_prompt);

        // Folder-form helper file → belongs to the skill, not a prompt.
        let helper = classify_skill_key("skills/aida-pr/examples/pr-description-template.md")
            .expect("folder helper");
        assert_eq!(helper.name, "aida-pr");
        assert_eq!(
            helper.rel_path,
            "aida-pr/examples/pr-description-template.md"
        );
        assert!(!helper.is_prompt);

        // Non-skill keys and the local/ README are rejected.
        assert!(classify_skill_key("commands/aida-status.md").is_none());
        assert!(classify_skill_key("skills/local/README.md").is_none());
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

        // Folder-form skills (`skills/<name>/SKILL.md`) and flat skills
        // (`skills/<name>.md`) both reduce to their canonical name via the
        // classifier; helper files (examples/, templates/) are not prompts and
        // must not masquerade as skills. trace:TASK-574
        let skills: HashSet<String> = EMBEDDED_TEMPLATES
            .keys()
            .filter_map(|k| classify_skill_key(k))
            .filter(|s| s.is_prompt)
            .map(|s| s.name.to_string())
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
        // TASK-320: the retired glyphs must not reappear in ANY embedded skill
        // prompt, not just the three originally spot-checked — a new or edited
        // skill that reintroduces ⏵/🚪 should fail CI. Enumerate every prompt
        // skill template via the classifier (folder + flat forms; helper files
        // excluded). trace:TASK-320 | ai:claude
        let skill_keys: Vec<String> = EMBEDDED_TEMPLATES
            .keys()
            .filter(|k| classify_skill_key(k).map(|s| s.is_prompt).unwrap_or(false))
            .map(|k| k.to_string())
            .collect();
        assert!(
            skill_keys.len() >= 3,
            "expected to enumerate embedded skill prompts, found {}",
            skill_keys.len()
        );
        for key in &skill_keys {
            let content = EMBEDDED_TEMPLATES
                .get(key.as_str())
                .unwrap_or_else(|| panic!("missing embedded template: {key}"));
            for &(glyph, why) in RETIRED {
                assert!(!content.contains(glyph), "{key} still uses retired {why}");
            }
        }

        // The refined glyphs (⇒ / ⏸) must be present in the skills that carry a
        // Path/What happens/Why table — kept scoped, since not every skill has
        // one. trace:BUG-116 | ai:claude
        for key in &[
            "skills/aida-pickup.md",
            "skills/aida-pr/SKILL.md",
            "skills/aida-review.md",
        ] {
            let content = EMBEDDED_TEMPLATES
                .get(*key)
                .unwrap_or_else(|| panic!("missing embedded template: {key}"));
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

    /// TASK-333 regression guard — the `/aida-review` skill template must
    /// carry an explicit `## Fix-forward policy` section with the scoped
    /// allowance + guardrails so reviewers stop improvising inconsistently
    /// (PR-64 refused, PR-91 fix-forwarded; same skill, opposite behavior).
    /// The policy has five load-bearing pieces: (1) a top-level section
    /// header, (2) both PERMITTED and FORBIDDEN buckets named with the
    /// build/test-behavior discriminator, (3) a worked-examples table with
    /// rows for at least one fix-forward and one RequestChanges case so the
    /// discriminator is concrete, (4) the procedure names mergeStateStatus
    /// CLEAN re-verification, (5) a `kind:reviewer-fix-forward` finding tag
    /// for the STORY-285 surface. Step 5 must point at the policy section
    /// rather than carrying its own (formerly contradictory) examples.
    /// trace:TASK-333 | ai:claude
    #[test]
    fn aida_review_carries_task_333_fix_forward_policy() {
        let review = EMBEDDED_TEMPLATES
            .get("skills/aida-review.md")
            .expect("aida-review.md embedded");

        // (1) Top-level Fix-forward policy section is present.
        let policy_idx = review.find("## Fix-forward policy").expect(
            "aida-review.md must define a top-level `## Fix-forward policy` \
             section (TASK-333 acceptance criterion 1)",
        );

        // The policy must appear BEFORE the Workflow header so reviewers
        // read it before walking the steps that reference it.
        let workflow_idx = review
            .find("\n## Workflow")
            .expect("aida-review.md must define a `## Workflow` section");
        assert!(
            policy_idx < workflow_idx,
            "the `## Fix-forward policy` section must appear before \
             `## Workflow` so step 5's pointer resolves to context already \
             on screen"
        );

        // Scope subsequent assertions to the policy section's body
        // (between its header and the next top-level `## ` header).
        let policy_body = {
            let after_header = &review[policy_idx..];
            let end = after_header[2..]
                .find("\n## ")
                .map(|n| n + 2)
                .unwrap_or(after_header.len());
            &after_header[..end]
        };

        // (2) Both PERMITTED and FORBIDDEN buckets named with the
        // build/test-behavior discriminator — improvisation comes from a
        // missing discriminator, so the explicit one is load-bearing.
        assert!(
            policy_body.contains("PERMITTED") && policy_body.contains("FORBIDDEN"),
            "policy section must name both PERMITTED and FORBIDDEN buckets \
             (acceptance criterion 2: doc-only-vs-logic discriminator)"
        );
        assert!(
            policy_body.contains("build or test behavior"),
            "policy section must name the `build or test behavior` \
             discriminator that decides PERMITTED vs FORBIDDEN — without \
             the rule the buckets are vibe-checks"
        );

        // (3) Worked-examples table with at least one ✅ row and one ❌ row
        // so the discriminator is concrete enough to apply without
        // judgment-call ambiguity (acceptance criterion 2's "worked
        // examples"). The table also makes the PR-91 / PR-64 inconsistency
        // re-derivable: a reviewer looking up "is this doc-only?" reads
        // the row, not their own intuition.
        let table_marker = "| Diff | Verdict | Reason |";
        assert!(
            policy_body.contains(table_marker),
            "policy section must include the worked-examples table header \
             `{table_marker}` so the discriminator carries concrete rows"
        );
        let table_idx = policy_body.find(table_marker).unwrap();
        let table_body = &policy_body[table_idx..];
        let table_end = table_body
            .find("\n\n")
            .map(|n| table_idx + n)
            .unwrap_or(policy_body.len());
        let table = &policy_body[table_idx..table_end];
        let permitted_rows = table.matches("✅").count();
        let forbidden_rows = table.matches("❌").count();
        assert!(
            permitted_rows >= 1 && forbidden_rows >= 1,
            "worked-examples table must include ≥1 ✅ Fix-forward row and \
             ≥1 ❌ RequestChanges row — got {permitted_rows} ✅ and \
             {forbidden_rows} ❌"
        );

        // (4) Procedure names mergeStateStatus CLEAN re-verification — the
        // CI-desync guard that bounds the doc-only allowance's first risk.
        assert!(
            policy_body.contains("mergeStateStatus") && policy_body.contains("CLEAN"),
            "policy procedure must require `mergeStateStatus: CLEAN` \
             re-verification on the fix-forward commit before Approving \
             (acceptance criterion 4)"
        );

        // (5) `kind:reviewer-fix-forward` finding tag is named so the
        // STORY-285 surface picks it up (acceptance criterion 3, the
        // self-grading guardrail).
        assert!(
            policy_body.contains("kind:reviewer-fix-forward"),
            "policy section must name the `kind:reviewer-fix-forward` \
             finding tag for the STORY-285 surface (acceptance criterion 3)"
        );
        assert!(
            policy_body.contains("from-review:PR-"),
            "policy section must name the `from-review:PR-<N>` finding tag \
             so the finding rides the same STORY-285 surface that step 7b \
             uses — the advisor's `aida findings list` groups by it"
        );

        // (6) Logic-touching changes route to RequestChanges — acceptance
        // criterion 5. Search both the policy section and Step 5 to be
        // robust against later prose reorgs.
        let logic_to_rc = policy_body.contains("RequestChanges instead")
            || policy_body.contains("return RequestChanges");
        assert!(
            logic_to_rc,
            "policy section must explicitly route logic-touching changes \
             to RequestChanges (acceptance criterion 5)"
        );

        // Step 5 must point at the policy section rather than carry its
        // own (formerly contradictory) inline examples. The pre-TASK-333
        // text used the heading `Mechanical fix-forward`; the new heading
        // mentions the doc-only policy explicitly.
        let step_5_idx = review
            .find("### 5. ")
            .expect("aida-review.md must define a `### 5.` step");
        let step_5_end = review[step_5_idx..]
            .find("\n### ")
            .map(|n| step_5_idx + n)
            .unwrap_or(review.len());
        let step_5 = &review[step_5_idx..step_5_end];
        assert!(
            step_5.contains("Fix-forward policy"),
            "step 5 must reference the `Fix-forward policy` section so the \
             reviewer reads the policy before acting (acceptance \
             criterion 6: consistent behavior)"
        );

        // Step 5 must NOT re-introduce a `#[cfg(unix)]` test gate as a
        // permitted fix-forward example — the pre-TASK-333 text did, and
        // it directly contradicts the new policy (test attributes change
        // which tests CI runs).
        let cfg_unix_as_permitted = step_5.contains("gate USERPROFILE assertion on #[cfg(unix)]")
            && !step_5.contains("Forbidden");
        assert!(
            !cfg_unix_as_permitted,
            "step 5 must not surface `#[cfg(unix)]` test-gating as a \
             permitted fix-forward — the pre-TASK-333 example contradicts \
             the new policy"
        );
    }
}
