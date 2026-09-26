//! The derived portable skill inventory: which skill masters ship to the
//! non-Claude skill packs (`.agents/skills/`, read by Codex and Antigravity,
//! plus any legacy `.codex/skills/` / `.antigravity/skills/` pack that
//! already exists).
//!
//! The inventory is the Claude skill set minus a commented exclusion list
//! (`templates/skill-inventory.toml`), so a new skill master reaches every
//! vendor by default instead of waiting for someone to add it to a
//! hand-kept list. Every per-skill `include_aida_*_skill` flag in
//! [`ScaffoldConfig`] is honoured through one shared table.
// trace:BUG-1639 | ai:claude

use std::collections::{BTreeMap, BTreeSet};

use once_cell::sync::Lazy;
use serde::Deserialize;

use super::ScaffoldConfig;
use crate::templates::{classify_skill_key, EMBEDDED_TEMPLATES};

/// The exclusion list, embedded at compile time.
// trace:BUG-1639 | ai:claude
const INVENTORY_TOML: &str = include_str!("../../templates/skill-inventory.toml");

/// The shared non-Claude skill pack, discovered by Codex and Antigravity.
// trace:BUG-1639 | ai:claude
pub const PORTABLE_PACK: &str = ".agents/skills";

/// Legacy per-vendor packs. AIDA keeps maintaining one only when it already
/// exists as a real directory; new installs never create them.
// trace:BUG-1639 | ai:claude
pub const LEGACY_CODEX_PACK: &str = ".codex/skills";
/// See [`LEGACY_CODEX_PACK`].
// trace:BUG-1639 | ai:claude
pub const LEGACY_ANTIGRAVITY_PACK: &str = ".antigravity/skills";

/// Prefix of every skill name AIDA owns. AIDA never plans, writes, refreshes
/// or prunes any other name in a shared skill directory.
// trace:BUG-1639 | ai:claude
pub const AIDA_SKILL_PREFIX: &str = "aida-";

#[derive(Debug, Deserialize)]
struct InventoryEntry {
    name: String,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
struct InventoryFile {
    #[serde(default)]
    claude_only: Vec<InventoryEntry>,
    #[serde(default)]
    not_a_skill: Vec<InventoryEntry>,
}

static INVENTORY: Lazy<InventoryFile> = Lazy::new(|| {
    // The file is a compile-time constant; the unit tests parse it, so a
    // malformed edit fails CI before it can ship.
    toml::from_str(INVENTORY_TOML).expect("templates/skill-inventory.toml must parse")
});

/// Skills that never ship to a non-Claude pack, with the reason each is
/// Claude-only.
// trace:BUG-1639 | ai:claude
pub fn claude_only_skills() -> BTreeMap<&'static str, &'static str> {
    INVENTORY
        .claude_only
        .iter()
        .map(|e| (e.name.as_str(), e.reason.as_str()))
        .collect()
}

/// Skill templates that are never scaffolded as skills into any pack.
// trace:BUG-1639 | ai:claude
pub fn not_a_skill() -> BTreeSet<&'static str> {
    INVENTORY
        .not_a_skill
        .iter()
        .map(|e| e.name.as_str())
        .collect()
}

/// Is `name` excluded from every skill pack?
// trace:BUG-1639 | ai:claude
pub fn is_not_a_skill(name: &str) -> bool {
    INVENTORY.not_a_skill.iter().any(|e| e.name == name)
}

/// Which packs a per-skill include flag gates.
// trace:BUG-1639 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagScope {
    /// The Claude pack and every portable pack.
    AllPacks,
    /// Only the portable packs; the Claude pack keeps its own version.
    PortableOnly,
}

/// Which kind of pack a skill is being planned for.
// trace:BUG-1639 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    /// `.claude/skills/`.
    Claude,
    /// `.agents/skills/` and the legacy `.codex/skills/` / `.antigravity/skills/`.
    Portable,
}

type IncludeFlag = fn(&ScaffoldConfig) -> bool;

/// Every per-skill `include_aida_*_skill` flag, by skill name. One table so
/// the Claude pack and the portable packs cannot disagree about which flag
/// gates which skill; a unit test fails when a flag field is missing here.
// trace:BUG-1639 | ai:claude
pub const INCLUDE_FLAGS: &[(&str, IncludeFlag, FlagScope)] = &[
    (
        "aida-req",
        |c| c.include_aida_req_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-plan",
        |c| c.include_aida_plan_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-implement",
        |c| c.include_aida_implement_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-capture",
        |c| c.include_aida_capture_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-learn",
        |c| c.include_aida_learn_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-memory-query",
        |c| c.include_aida_memory_query_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-memory-capture",
        |c| c.include_aida_memory_capture_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-docs",
        |c| c.include_aida_docs_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-docs-review",
        |c| c.include_aida_docs_review_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-release",
        |c| c.include_aida_release_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-evaluate",
        |c| c.include_aida_evaluate_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-commit",
        |c| c.include_aida_commit_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-sync",
        |c| c.include_aida_sync_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-test",
        |c| c.include_aida_test_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-review",
        |c| c.include_aida_review_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-onboard",
        |c| c.include_aida_onboard_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-sprint",
        |c| c.include_aida_sprint_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-search",
        |c| c.include_aida_search_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-standup",
        |c| c.include_aida_standup_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-import-plan",
        |c| c.include_aida_import_plan_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-digest",
        |c| c.include_aida_digest_skill,
        FlagScope::AllPacks,
    ),
    (
        "aida-backlog-groom",
        |c| c.include_aida_backlog_groom_skill,
        FlagScope::AllPacks,
    ),
    // The Claude pack keeps its subagent-tool version whatever this flag
    // says (STORY-1475); the flag gates only the portable body.
    (
        "aida-orchestrate",
        |c| c.include_aida_orchestrate_skill,
        FlagScope::PortableOnly,
    ),
];

/// Does `config` include skill `name` in a pack of kind `pack`? A skill with
/// no include flag is always included.
// trace:BUG-1639 | ai:claude
pub fn skill_enabled(config: &ScaffoldConfig, name: &str, pack: PackKind) -> bool {
    INCLUDE_FLAGS
        .iter()
        .find(|(n, _, _)| *n == name)
        .is_none_or(|(_, flag, scope)| {
            (pack == PackKind::Claude && *scope == FlagScope::PortableOnly) || flag(config)
        })
}

/// One file of a portable skill.
// trace:BUG-1639 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableSkillFile {
    /// Path under `<pack>/<name>/`: `SKILL.md`, or a folder-form helper such
    /// as `examples/pr-description-template.md`.
    pub rel_path: String,
    /// Embedded template key the file's body comes from.
    pub source_key: String,
}

/// A skill that ships to the portable packs.
// trace:BUG-1639 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableSkill {
    pub name: String,
    /// `SKILL.md` first is not guaranteed; callers order writes themselves.
    pub files: Vec<PortableSkillFile>,
}

impl PortableSkill {
    /// The embedded template key of the skill's `SKILL.md` body.
    pub fn skill_md_source(&self) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.rel_path == "SKILL.md")
            .map(|f| f.source_key.as_str())
    }
}

/// Every skill master that is a real skill (not on the `not_a_skill` list),
/// with each of its template keys, in sorted order.
fn skill_masters() -> BTreeMap<&'static str, Vec<(&'static str, &'static str, bool)>> {
    let mut masters: BTreeMap<&'static str, Vec<(&'static str, &'static str, bool)>> =
        BTreeMap::new();
    for key in EMBEDDED_TEMPLATES.keys() {
        let Some(skill) = classify_skill_key(key) else {
            continue;
        };
        if !skill.rel_path.ends_with(".md") || is_not_a_skill(skill.name) {
            continue;
        }
        masters
            .entry(skill.name)
            .or_default()
            .push((key, skill.rel_path, skill.is_prompt));
    }
    for files in masters.values_mut() {
        files.sort();
    }
    masters
}

/// The skills `config` ships to the portable packs: every skill master,
/// minus `not_a_skill`, minus `claude_only`, minus any skill whose include
/// flag is off. Only `aida-*` names are ever returned. A `SKILL.md` body
/// comes from `skills-portable/<name>.md` when that exists, else from the
/// master; a folder-form skill also carries its helper files.
// trace:BUG-1639 | ai:claude
pub fn portable_skill_inventory(config: &ScaffoldConfig) -> BTreeMap<String, PortableSkill> {
    let claude_only = claude_only_skills();
    let mut out = BTreeMap::new();
    for (name, keys) in skill_masters() {
        if !name.starts_with(AIDA_SKILL_PREFIX)
            || claude_only.contains_key(name)
            || !skill_enabled(config, name, PackKind::Portable)
        {
            continue;
        }
        let portable = format!("skills-portable/{name}.md");
        let portable_key = EMBEDDED_TEMPLATES
            .get_key_value(portable.as_str())
            .map(|(k, _)| *k);
        let mut files = Vec::new();
        for (key, rel_path, is_prompt) in keys {
            let (rel, source) = if is_prompt {
                ("SKILL.md".to_string(), portable_key.unwrap_or(key))
            } else {
                // Folder-form helper: `<name>/<sub>` keeps its sub-path.
                let sub = rel_path
                    .split_once('/')
                    .map_or(rel_path, |(_, sub)| sub)
                    .to_string();
                (sub, key)
            };
            files.push(PortableSkillFile {
                rel_path: rel,
                source_key: source.to_string(),
            });
        }
        if files.iter().any(|f| f.rel_path == "SKILL.md") {
            out.insert(
                name.to_string(),
                PortableSkill {
                    name: name.to_string(),
                    files,
                },
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_inventory() -> BTreeMap<String, PortableSkill> {
        portable_skill_inventory(&ScaffoldConfig::default())
    }

    /// The acceptance guard: a skill master is either shipped to the
    /// portable packs or excluded on purpose, so a new skill can never
    /// silently miss Codex and Antigravity.
    // trace:BUG-1639 | ai:claude
    #[test]
    fn every_skill_master_is_portable_or_claude_only() {
        let inventory = default_inventory();
        let claude_only = claude_only_skills();
        let masters = skill_masters();
        assert!(masters.len() > 40, "{} masters", masters.len());
        for name in masters.keys() {
            let shipped = inventory.contains_key(*name);
            let excluded = claude_only.contains_key(name);
            assert!(
                shipped ^ excluded,
                "{name}: shipped={shipped} claude_only={excluded}; a skill master must \
                 ship to the portable packs or be listed under [[claude_only]] in \
                 templates/skill-inventory.toml"
            );
        }
        assert!(
            inventory.keys().all(|n| masters.contains_key(n.as_str())),
            "the inventory names only real masters"
        );
    }

    // trace:BUG-1639 | ai:claude
    #[test]
    fn claude_only_entries_name_real_masters_and_have_reasons() {
        let file: InventoryFile = toml::from_str(INVENTORY_TOML).expect("inventory parses");
        let prompt_masters: BTreeSet<&str> = EMBEDDED_TEMPLATES
            .keys()
            .filter_map(|k| classify_skill_key(k))
            .filter(|s| s.is_prompt)
            .map(|s| s.name)
            .collect();
        assert!(!file.claude_only.is_empty());
        for entry in file.claude_only.iter().chain(&file.not_a_skill) {
            assert!(
                prompt_masters.contains(entry.name.as_str()),
                "{} names no skill master",
                entry.name
            );
            assert!(
                entry.reason.trim().len() > 10,
                "{} needs a reason",
                entry.name
            );
        }
        let claude_only: BTreeSet<&str> =
            file.claude_only.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(claude_only, BTreeSet::from(["aida-burndown", "aida-solo"]));
        assert_eq!(
            not_a_skill(),
            BTreeSet::from(["aida-advise", "aida-assess", "aida-intent"])
        );
        // A skill is not on both lists, and no portable body exists for a
        // skill that never ships.
        for entry in &file.claude_only {
            assert!(!is_not_a_skill(&entry.name), "{}", entry.name);
            assert!(
                !EMBEDDED_TEMPLATES
                    .contains_key(format!("skills-portable/{}.md", entry.name).as_str()),
                "{} has a portable body; remove it from [[claude_only]]",
                entry.name
            );
        }
    }

    // trace:BUG-1639 | ai:claude
    #[test]
    fn portable_inventory_prefers_skills_portable_body() {
        let inventory = default_inventory();
        let orchestrate = &inventory["aida-orchestrate"];
        assert_eq!(
            orchestrate.skill_md_source(),
            Some("skills-portable/aida-orchestrate.md")
        );
        assert_eq!(
            inventory["aida-req"].skill_md_source(),
            Some("skills/aida-req.md")
        );
        // Every portable body belongs to a shipped skill.
        for key in EMBEDDED_TEMPLATES.keys() {
            if let Some(name) = key
                .strip_prefix("skills-portable/")
                .and_then(|f| f.strip_suffix(".md"))
            {
                assert_eq!(
                    inventory[name].skill_md_source(),
                    Some(*key),
                    "{name} must ship its portable body"
                );
            }
        }
    }

    // trace:BUG-1639 | ai:claude
    #[test]
    fn portable_inventory_includes_aida_handoff() {
        let inventory = default_inventory();
        for name in [
            "aida-handoff",
            "aida-human-audit",
            "aida-fleet-watch",
            "aida-grill",
            "aida-triage",
            "aida-guided-implement",
            "aida-review",
        ] {
            assert!(inventory.contains_key(name), "{name} ships");
        }
        for name in [
            "aida-burndown",
            "aida-solo",
            "aida-advise",
            "aida-assess",
            "aida-intent",
        ] {
            assert!(!inventory.contains_key(name), "{name} does not ship");
        }
        assert!(inventory.keys().all(|n| n.starts_with(AIDA_SKILL_PREFIX)));
    }

    // trace:BUG-1639 | ai:claude
    #[test]
    fn folder_form_skill_carries_its_helpers() {
        let inventory = default_inventory();
        let pr = &inventory["aida-pr"];
        let rels: BTreeSet<&str> = pr.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert!(rels.contains("SKILL.md"));
        assert!(
            rels.contains("examples/pr-description-template.md"),
            "{rels:?}"
        );
        assert_eq!(pr.skill_md_source(), Some("skills/aida-pr/SKILL.md"));
    }

    /// Every `include_aida_*_skill` field of [`ScaffoldConfig`] is in the
    /// shared table, and turning it off drops exactly that skill from the
    /// portable inventory (and from the Claude pack unless the flag is
    /// portable-only).
    // trace:BUG-1639 | ai:claude
    #[test]
    fn inventory_honours_every_include_flag() {
        let defaults = serde_json::to_value(ScaffoldConfig::default()).unwrap();
        let fields: Vec<String> = defaults
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with("include_aida_") && k.ends_with("_skill"))
            .cloned()
            .collect();
        assert_eq!(fields.len(), INCLUDE_FLAGS.len(), "{fields:?}");
        let all = default_inventory();
        for field in fields {
            let name = format!(
                "aida-{}",
                field
                    .trim_start_matches("include_aida_")
                    .trim_end_matches("_skill")
                    .replace('_', "-")
            );
            let (_, _, scope) = INCLUDE_FLAGS
                .iter()
                .find(|(n, _, _)| *n == name)
                .unwrap_or_else(|| panic!("{field} has no INCLUDE_FLAGS entry for {name}"));
            assert!(all.contains_key(&name), "{name} ships by default");
            let mut value = defaults.clone();
            value[field.as_str()] = serde_json::Value::Bool(false);
            let config: ScaffoldConfig = serde_json::from_value(value).unwrap();
            let off = portable_skill_inventory(&config);
            assert!(!off.contains_key(&name), "{field}=false must drop {name}");
            assert_eq!(off.len() + 1, all.len(), "{field} drops only {name}");
            assert!(!skill_enabled(&config, &name, PackKind::Portable));
            assert_eq!(
                skill_enabled(&config, &name, PackKind::Claude),
                *scope == FlagScope::PortableOnly,
                "{name}"
            );
        }
    }
}
