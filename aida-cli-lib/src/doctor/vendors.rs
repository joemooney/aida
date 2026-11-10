// trace:TASK-3781 | ai:codex

use super::*;

// trace:BUG-2486 | ai:codex — bounded metadata-only read; no refresh or token output.
pub(super) fn codex_token_lifetime(
    env: &aida_core::exec_context::Environment,
    category: Option<&str>,
) -> Result<Option<String>> {
    if !doctor_category_selected(category, "permission-posture")? {
        return Ok(None);
    }
    let home = env
        .get(crate::vendors::codex_auth_hints::HOME_ENV)
        .map(std::path::PathBuf::from)
        .or_else(|| crate::home_dir().map(|h| h.join(crate::vendors::codex_auth_hints::HOME_DIR)));
    let Some(home) = home else {
        return Ok(None);
    };
    if !home.join("auth.json").try_exists().unwrap_or(true) {
        return Ok(None);
    }
    Ok(Some(
        match crate::launch::contained::codex_auth::token_expiry(&home, unsafe { libc::getuid() }) {
            Ok(exp) => crate::vendors::codex_auth_hints::token_lifetime(
                exp,
                exp.saturating_sub(chrono::Utc::now().timestamp()),
            ),
            Err(error) => crate::vendors::codex_auth_hints::token_unavailable(&error),
        },
    ))
}

/// TASK-1124: which deployed Codex prompts in `dir` (normally `~/.codex/prompts`)
/// have drifted from the current source templates. A prompt that EXISTS but
/// whose content differs from `expected_codex_prompts()` is rot (a stale
/// delivery — e.g. a fix that never re-scaffolded); a MISSING prompt is an
/// opt-out, not rot, so it is never flagged. Pure over `dir`, so unit-testable
/// without touching $HOME.
// trace:TASK-1124 | ai:claude
pub(super) fn codex_prompts_drift(dir: &std::path::Path) -> Vec<String> {
    let mut drifted = Vec::new();
    for (name, expected) in aida_core::scaffolding::codex_prompts::expected_codex_prompts() {
        let path = dir.join(format!("{name}.md"));
        if let Ok(actual) = std::fs::read_to_string(&path) {
            if !aida_core::scaffolding::generated_text_matches(&actual, &expected) {
                drifted.push(name);
            }
        }
    }
    drifted.sort();
    drifted
}

pub(super) fn codex_prompt_dir_has_aida_prompt_cruft(dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        let path = entry.path();
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.starts_with("aida-") && (name.ends_with(".md") || name.ends_with(".aida-bak"))
            })
    })
}

pub(super) fn codex_ignores_prompt_dir_finding_for_version(
    dir: &std::path::Path,
    version: (u64, u64, u64),
) -> Option<DoctorFinding> {
    if !codex_prompt_dir_has_aida_prompt_cruft(dir) {
        return None;
    }
    if !aida_core::scaffolding::codex_prompts::codex_prompt_dir_is_undiscoverable(version) {
        return None;
    }
    Some(DoctorFinding {
        category: "scaffold-drift".to_string(),
        id: "scaffold-drift/codex-prompts-undiscovered".to_string(),
        summary: format!(
            "~/.codex/prompts contains AIDA prompt files, but installed Codex {}.{}.{} does not discover them as `/aida-*` slash commands",
            version.0, version.1, version.2
        ),
        action: "Prune the dead prompt pack (`rm -rf ~/.codex/prompts` or delete its `aida-*.md` and `*.aida-bak` files); use the project's `.agents/skills/` via `/skills` or `$aida-*`, or run the matching `aida ...` CLI verb directly".to_string(),
        safe_heal: false,
    })
}

pub(super) fn codex_ignores_prompt_dir_finding(dir: &std::path::Path) -> Option<DoctorFinding> {
    codex_ignores_prompt_dir_finding_for_version(dir, installed_codex_version()?)
}

/// TASK-1124: rule-delivery-rot detection. Flags deployed vendor prompts/skills
/// that have drifted from the binary's embedded source templates, in two
/// places: (1) the project-local `.claude/`+`.codex/` scaffold (via
/// `check_scaffold_status`, which already ignores the dev-repo symlink layout
/// per BUG-917), and (2) the machine-global `~/.codex/prompts` (the stale-prompt
/// case that contributed to the TASK-1123 reviewer-bypass incident). Detection
/// only — each finding names the re-scaffold command, no auto-heal.
// trace:TASK-1124 | ai:claude
/// The fix for memory-lane skill drift. `aida scaffold upgrade` ignores the
/// footprint and installs the full skill set, and `aida init` on an
/// initialized project changes nothing, so the lane-safe fix is to restore the
/// skill from the embedded templates by hand.
// trace:BUG-1645 | ai:claude
pub(super) const MEMORY_LANE_SKILL_ACTION: &str = "Memory-lane project: do not run `aida scaffold upgrade` (it installs the full skill set). Restore or refresh the skill by hand: `aida scaffold extract --output <tmp-dir>`, then copy `<tmp-dir>/skills/<name>.md` to `<pack>/<name>/SKILL.md` (e.g. `.agents/skills/aida-capture/SKILL.md`).";

/// The fix for drifted memory-lane skills. Refresh keeps a memory-lane project
/// a memory lane and updates an unedited skill; an edited one is kept, so the
/// manual restore by copy stays the fallback. Upgrade installs the full skill
/// set and is never suggested here.
// trace:BUG-1653 | ai:claude
// trace:BUG-1662 | ai:claude
pub(super) const MEMORY_LANE_DRIFT_ACTION: &str = "Memory-lane project: run `aida scaffold refresh` to update unedited skills (it keeps the project a memory lane). A skill you edited is kept: inspect it with `aida scaffold diff`, and to restore it run `aida scaffold extract --output <tmp-dir>`, then copy `<tmp-dir>/skills/<name>.md` to `<pack>/<name>/SKILL.md` (e.g. `.agents/skills/aida-capture/SKILL.md`).";

/// The fix for a memory-lane project flooded with the discipline pack by an older refresh.
// trace:TASK-1536 | ai:agy
pub(super) const MEMORY_LANE_DISCIPLINE_ACTION: &str = "Memory-lane project: remove the flooded discipline pack: `rm -rf .aida/discipline` and remove any `.aida/discipline/` allow-list entries from `.gitignore`.";

/// The fix for a memory-lane project whose AGENTS.md block was replaced with the full conventions block.
// trace:TASK-1536 | ai:agy
pub(super) const MEMORY_LANE_CONVENTIONS_ACTION: &str = "Memory-lane project: restore the memory-lane block in AGENTS.md: replace the '# AIDA Conventions' section between the '<!-- AIDA-AUTOGEN-BEGIN -->' and '<!-- AIDA-AUTOGEN-END -->' markers with '# AIDA Memory Lane' and 'Storage: <path>' (e.g. copy the block from CLAUDE.md).";

pub(super) fn scan_scaffold_drift(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();

    // (1) Project-local vendor prompts/skills (Template-category, AIDA-owned).
    // The Codex/Antigravity packs are expected only when one of them is in
    // the saved agent selection, so a Claude-only project is never told
    // Codex skills are missing. trace:BUG-1639 | ai:claude
    let config = crate::init_cmd::scaffold_config_for_project(project_root);
    let portable_selected = config.generate_codex_skills || config.generate_antigravity_skills;
    // Codex silently skips symlinked SKILL.md files in the shared pack.
    // Report them without attempting a repair; they may be user-owned links.
    // trace:TASK-1519 | ai:codex
    if let Ok(entries) = std::fs::read_dir(project_root.join(".agents/skills")) {
        let links: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path().join("SKILL.md");
                std::fs::symlink_metadata(&path)
                    .ok()
                    .filter(|meta| meta.file_type().is_symlink())
                    .map(|_| {
                        path.strip_prefix(project_root)
                            .unwrap_or(&path)
                            .display()
                            .to_string()
                    })
            })
            .collect();
        if !links.is_empty() {
            findings.push(DoctorFinding {
                category: "scaffold-drift".to_string(),
                id: "scaffold-drift/agents-skill-symlink".to_string(),
                summary: format!(
                    "{} .agents/skills SKILL.md file(s) are symlinks; Codex does not load them",
                    links.len()
                ),
                action: format!(
                    "Replace these links with regular-file copies: {}",
                    links.join(", ")
                ),
                safe_heal: false,
            });
        }
    }
    let db_path = aida_core::db::project_cache_path(project_root);
    let status = check_scaffold_status(store, project_root, &config, &db_path);
    // Compare path components so Windows' backslashes do not hide scaffold
    // drift from doctor. trace:BUG-1685 | ai:codex
    let is_vendor_prompt_or_skill = |p: &std::path::Path| {
        [
            ".claude/commands",
            ".claude/skills",
            ".agents/skills",
            ".codex/skills",
            ".antigravity/skills",
        ]
        .iter()
        .any(|prefix| p.starts_with(std::path::Path::new(prefix)))
    };
    let drifted_count = status
        .modified
        .iter()
        .filter(|(p, _)| is_vendor_prompt_or_skill(p))
        .count();
    // trace:BUG-1117 | ai:codex
    // Missing `.agents/skills/*` is scaffold drift too: Codex >=0.142 does not
    // discover the old ~/.codex/prompts pack as `$aida-*`, so absence of the
    // project-local skill surface leaves a codex-vendor project with no working
    // skill entry point. Codex 0.157 and Antigravity 1.2.11 both read
    // `.agents/skills`, the pack new installs get. trace:BUG-1639 | ai:claude
    // A memory-lane project (saved footprint, or recognised from its packs
    // when it predates the saved footprint) is missing only what memory-lane
    // installs; a minimal one installs no skills. Never nudge either toward
    // the full skill set. trace:BUG-1645 | ai:claude
    let footprint = crate::init_cmd::effective_init_footprint(project_root);
    let lane = footprint == Some(crate::cli::InitFootprint::MemoryLane);
    let expected_by_footprint = |p: &std::path::Path| match footprint {
        Some(crate::cli::InitFootprint::Minimal) => false,
        Some(crate::cli::InitFootprint::MemoryLane) => {
            aida_core::scaffolding::refresh::skill_in_pack(p).is_some_and(|(_, name)| {
                crate::init_cmd::MEMORY_LANE_SKILLS.contains(&name.as_str())
            })
        }
        _ => true,
    };
    let missing_vendor_files: Vec<_> = status
        .missing
        .iter()
        .filter(|p| expected_by_footprint(p))
        .filter(|p| is_vendor_prompt_or_skill(p))
        .collect();
    let missing_portable_skill_files: Vec<_> = missing_vendor_files
        .iter()
        .filter(|p| portable_selected && p.starts_with(std::path::Path::new(".agents/skills")))
        .collect();
    if !missing_portable_skill_files.is_empty() {
        findings.push(DoctorFinding {
            category: "scaffold-drift".to_string(),
            id: "scaffold-drift/codex-skills-missing".to_string(),
            summary: format!(
                ".agents/skills is missing AIDA skill files ({} missing); Codex and Antigravity use this project-local surface for `$aida-*`",
                missing_portable_skill_files.len()
            ),
            action: if lane {
                MEMORY_LANE_SKILL_ACTION.to_string()
            } else {
                "Run `aida scaffold upgrade` to create `.agents/skills/aida-*/SKILL.md`; then reopen Codex or run `/skills` and use `$aida-capture`.".to_string()
            },
            safe_heal: false,
        });
    }
    if drifted_count > 0 {
        findings.push(DoctorFinding {
            category: "scaffold-drift".to_string(),
            id: "scaffold-drift/project".to_string(),
            summary: format!(
                "{} deployed vendor prompt/skill file(s) drifted from the source templates (stale scaffolding)",
                drifted_count
            ),
            action: if lane {
                MEMORY_LANE_DRIFT_ACTION.to_string()
            } else {
                "aida scaffold upgrade   (or `aida scaffold diff` to inspect)".to_string()
            },
            safe_heal: false,
        });
    }

    // (1b) Memory-lane project flooded by an older refresh (BUG-1662 follow-up).
    // An older refresh installed the full discipline pack (.aida/discipline/)
    // and/or replaced the memory-lane block in AGENTS.md with the full conventions
    // block. Flag each flooded artifact and give manual restore steps.
    // trace:TASK-1536 | ai:agy
    if lane {
        let discipline_dir = project_root.join(".aida/discipline");
        if discipline_dir.exists() {
            findings.push(DoctorFinding {
                category: "scaffold-drift".to_string(),
                id: "scaffold-drift/memory-lane-discipline".to_string(),
                summary: "Memory-lane project carries the full discipline pack (.aida/discipline/) installed by an older refresh".to_string(),
                action: MEMORY_LANE_DISCIPLINE_ACTION.to_string(),
                safe_heal: false,
            });
        }

        let agents_md_path = project_root.join("AGENTS.md");
        if let Ok(content) = std::fs::read_to_string(&agents_md_path) {
            let has_conventions =
                if let Some(block) = aida_core::scaffolding::extract_aida_block(&content) {
                    block.lines().any(|l| l.trim_end() == "# AIDA Conventions")
                } else {
                    content
                        .lines()
                        .any(|l| l.trim_end() == "# AIDA Conventions")
                };
            if has_conventions {
                findings.push(DoctorFinding {
                    category: "scaffold-drift".to_string(),
                    id: "scaffold-drift/memory-lane-agents-conventions".to_string(),
                    summary: "Memory-lane project AGENTS.md carries the full AIDA Conventions block instead of the memory-lane block (installed by an older refresh)".to_string(),
                    action: MEMORY_LANE_CONVENTIONS_ACTION.to_string(),
                    safe_heal: false,
                });
            }
        }
    }

    // (2) Machine-global ~/.codex/prompts — the TASK-1123 incident case.
    if let Some(dir) = crate::home_dir().map(|h| {
        h.join(crate::vendors::codex_auth_hints::HOME_DIR)
            .join("prompts")
    }) {
        // BUG-1095: Codex CLI 0.142 does not discover this generated directory
        // as an interactive custom slash-command surface. Flag the overclaimed
        // installation state separately from stale-content drift.
        // trace:BUG-1095 | ai:codex
        if let Some(finding) = codex_ignores_prompt_dir_finding(&dir) {
            findings.push(finding);
        }
        let drifted_prompts = codex_prompts_drift(&dir);
        if !drifted_prompts.is_empty() {
            findings.push(DoctorFinding {
                category: "scaffold-drift".to_string(),
                id: "scaffold-drift/codex-prompts".to_string(),
                summary: format!(
                    "~/.codex/prompts is stale — {} prompt(s) drifted from the current source templates",
                    drifted_prompts.len()
                ),
                // TASK-1170: the edit-preserving refresh is the safe delivery
                // path — --force is no longer needed to pick up a fix, and it
                // would take your own edits with it.
                action: "aida scaffold refresh".to_string(),
                safe_heal: false,
            });
        }
    }

    findings
}
