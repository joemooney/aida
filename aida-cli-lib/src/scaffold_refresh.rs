//! Edit-preserving refresh of every agent pack AIDA ships.
//!
//! `aida init --refresh` (and the direct `aida scaffold refresh`) brings the
//! installed packs level with the binary's embedded templates *without*
//! `--force`: a pack file whose body still hashes to its recorded scaffold
//! checksum is overlaid; one the user has edited, one with no marker, and one
//! that is a symlink are all left exactly as they are.
//!
//! This is the same contract the starter memory pack has had since STORY-255,
//! generalized so a template fix reaches Claude skills/commands, Codex skills,
//! and Antigravity skills through one mechanism instead of one per vendor.
// trace:TASK-1170 | ai:claude
// trace:BUG-1118 | ai:codex

use std::path::{Path, PathBuf};

use std::collections::{BTreeMap, BTreeSet};

use aida_core::scaffolding::refresh::{
    plan_skill_pack, refresh_file, skill_in_pack, ManifestMode, RefreshOutcome, RefreshReport,
};
use colored::Colorize;

/// One pack's refresh result, labelled for the summary.
pub(crate) struct PackRefresh {
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) report: RefreshReport,
}

/// The project-local packs a refresh covers, longest-prefix-first so the
/// labels stay stable. Everything else the scaffolder renders (CLAUDE.md,
/// AGENTS.md, settings.json, hooks, docs) is out of scope: those are Seed /
/// ManagedMerge files with their own merge rules.
const PROJECT_PACKS: &[(&str, &str)] = &[
    (".claude/skills/", "Claude skills"),
    (".claude/commands/", "Claude commands"),
    (".codex/skills/", "Codex skills"),
    (".antigravity/skills/", "Antigravity skills"),
];

/// Decide, per installed skill pack, which newly shipped skills this refresh
/// delivers, and record them in the pack's delivered-skills manifest BEFORE
/// anything is written (so a failed record creates nothing).
///
/// A skill is delivered only when the pack's manifest has never recorded its
/// name; a delivered skill that is now missing is recorded as an opt-out and
/// stays deleted. A pack without a complete manifest (installed before it
/// existed) is seeded from the directories on disk and receives nothing this
/// pass. An unreadable manifest fails closed with a warning. A pack directory
/// that does not exist is never created.
// trace:TASK-1503 | ai:claude
fn plan_refresh_deliveries(
    project_root: &Path,
    preview: &aida_core::scaffolding::ScaffoldPreview,
) -> BTreeMap<PathBuf, BTreeSet<String>> {
    let mut deliveries = BTreeMap::new();
    for install_plan in &preview.skill_packs {
        if !project_root.join(&install_plan.pack).is_dir() {
            continue;
        }
        let plan = plan_skill_pack(
            project_root,
            &install_plan.pack,
            install_plan.shipped.clone(),
            ManifestMode::Refresh,
        );
        if let Some(warning) = &plan.warning {
            eprintln!("  {} {}", "Warning:".yellow(), warning);
            continue;
        }
        let deliver = plan.deliverable(project_root);
        if let Err(e) = plan.record(project_root, &deliver) {
            eprintln!(
                "  {} could not record delivered skills in {} ({}); nothing created",
                "Warning:".yellow(),
                plan.pack.display(),
                e
            );
            continue;
        }
        deliveries.insert(plan.pack.clone(), deliver);
    }
    deliveries
}

/// Write a newly shipped skill file into an installed pack.
// trace:TASK-1503 | ai:claude
fn install_new_skill(dest: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, content)
}

/// Refresh every installed agent pack under `project_root`. Only files that already exist are
/// touched — installing a pack the project opted out of stays `aida init`'s
/// job, so a Claude-only project never grows a `.codex/` tree from a refresh.
/// The one exception: an installed skill pack receives each skill AIDA has
/// started shipping since the pack was installed, once, tracked by the pack's
/// [`aida_core::scaffolding::refresh::DELIVERED_MANIFEST`]; a delivered skill
/// the user deletes stays deleted (TASK-1503).
///
/// `codex_prompts_dest` overrides the machine-global `~/.codex/prompts`
/// location for the deprecation notice (mirrors `scaffold codex-prompts --dest`).
pub(crate) fn refresh_agent_packs(
    project_root: &Path,
    codex_prompts_dest: Option<&Path>,
) -> Vec<PackRefresh> {
    let roles_dir = crate::global_roles_dir();
    refresh_agent_packs_at(project_root, codex_prompts_dest, roles_dir.as_deref())
}

/// Dependency-injected refresh core. Tests pass an explicit starter-role
/// destination (or `None`) so parallel pack tests never consult another
/// test's process-global HOME override.
// trace:BUG-1579 | ai:codex
fn refresh_agent_packs_at(
    project_root: &Path,
    codex_prompts_dest: Option<&Path>,
    roles_dir: Option<&Path>,
) -> Vec<PackRefresh> {
    let mut packs: Vec<PackRefresh> = PROJECT_PACKS
        .iter()
        .map(|(_, label)| PackRefresh {
            label: (*label).to_string(),
            location: project_root.display().to_string(),
            report: RefreshReport::default(),
        })
        .collect();

    // The scaffolder is the single source of "what should this file contain",
    // so refresh can never drift from what `init` / `scaffold apply` write.
    // An empty store is fine: none of the pack files interpolate requirements
    // (only CLAUDE.md / AGENTS.md / the tree docs do, and those are excluded).
    let mut config = aida_core::scaffolding::ScaffoldConfig::default();
    // trace:STORY-807 | ai:codex
    if let Some(selection) = crate::init_cmd::read_enabled_agent_selection(project_root) {
        selection.apply_to_scaffold_config(&mut config, false);
    }
    let db_path = project_root.join(".aida").join("cache.db");
    let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
        project_root.to_path_buf(),
        config,
        db_path,
    );
    let store = aida_core::RequirementsStore::default();
    let preview = scaffolder.preview(&store);

    // trace:TASK-1503 | ai:claude
    let deliveries = plan_refresh_deliveries(project_root, &preview);

    for artifact in &preview.artifacts {
        let rel = artifact.path.to_string_lossy().replace('\\', "/");
        let Some(idx) = PROJECT_PACKS.iter().position(|(p, _)| rel.starts_with(p)) else {
            continue;
        };
        let dest = project_root.join(&artifact.path);
        match refresh_file(&dest, &artifact.content, false) {
            // trace:TASK-1503 | ai:claude
            Ok(RefreshOutcome::Missing) => {
                let deliver = skill_in_pack(&artifact.path).is_some_and(|(pack, name)| {
                    deliveries.get(&pack).is_some_and(|d| d.contains(&name))
                });
                let outcome = if !deliver {
                    RefreshOutcome::Missing
                } else {
                    match install_new_skill(&dest, &artifact.content) {
                        Ok(()) => RefreshOutcome::Installed,
                        Err(e) => {
                            eprintln!(
                                "  {} could not install {}: {}",
                                "Warning:".yellow(),
                                artifact.path.display(),
                                e
                            );
                            RefreshOutcome::Missing
                        }
                    }
                };
                packs[idx].report.record(&artifact.path, outcome);
            }
            Ok(outcome) => packs[idx].report.record(&artifact.path, outcome),
            Err(e) => eprintln!(
                "  {} could not refresh {}: {}",
                "Warning:".yellow(),
                artifact.path.display(),
                e
            ),
        }
    }

    if let Some(agents) = agents_md_block_refresh(project_root, &preview) {
        packs.push(agents);
    }

    if let Ok(report) = crate::ensure_discipline_pack_scaffold(project_root, true) {
        if report.relocated > 0 || report.written > 0 {
            let mut refresh = RefreshReport::default();
            refresh.refreshed = report
                .relocated_paths
                .into_iter()
                .chain(report.written_paths)
                .collect();
            if let Ok(true) = crate::ensure_discipline_pack_gitignore_allow_list(project_root) {
                refresh.refreshed.push(PathBuf::from(".gitignore"));
            }
            packs.push(PackRefresh {
                label: "Discipline pack".to_string(),
                location: project_root.display().to_string(),
                report: refresh,
            });
        }
    }

    if let Some(prompts) = codex_prompts_refresh(codex_prompts_dest) {
        packs.push(prompts);
    }
    if let Some(roles) = roles_dir.and_then(starter_roles_refresh) {
        packs.push(roles);
    }
    packs.retain(|p| p.report != RefreshReport::default());
    packs
}

/// Add newly shipped guidance to an already-installed starter role without
/// rewriting anything the operator owns. Role files predate scaffold checksum
/// markers, so the safe migration is deliberately narrower than `refresh_file`:
/// append only an absent `system_prompt`; an existing value wins, and all other
/// bytes (including comments and custom purpose text) remain untouched.
// trace:BUG-1464 | ai:codex
fn starter_roles_refresh(roles_dir: &Path) -> Option<PackRefresh> {
    let mut report = RefreshReport::default();

    for (name, _, shipped_prompt) in crate::STARTER_ROLES {
        let Some(shipped_prompt) = shipped_prompt else {
            continue;
        };
        let path = roles_dir.join(format!("{name}.toml"));
        if path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            report
                .skipped_symlink
                .push(PathBuf::from(format!("{name}.toml")));
            continue;
        }
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(role) = toml::from_str::<crate::RoleState>(&existing) else {
            report
                .kept_unmarked
                .push(PathBuf::from(format!("{name}.toml")));
            continue;
        };
        if role.system_prompt.is_some() {
            continue;
        }

        let encoded = toml::Value::String((*shipped_prompt).to_string()).to_string();
        let separator = if existing.ends_with('\n') { "" } else { "\n" };
        let updated = format!("{existing}{separator}system_prompt = {encoded}\n");
        if let Err(error) = crate::write_atomic(&path, &updated) {
            eprintln!(
                "  {} could not refresh starter role {}: {}",
                "Warning:".yellow(),
                path.display(),
                error
            );
            continue;
        }
        report.refreshed.push(PathBuf::from(format!("{name}.toml")));
    }

    (report.changed() > 0 || !report.kept_unmarked.is_empty() || !report.skipped_symlink.is_empty())
        .then(|| PackRefresh {
            label: "Starter roles".to_string(),
            location: roles_dir.display().to_string(),
            report,
        })
}

/// `[scaffold] agents_md_block` in `.aida/config.toml` — the opt-out for
/// AIDA's block injection into a user-owned AGENTS.md. Missing file, section,
/// key, or a parse error all mean enabled: a config problem never blocks init
/// or refresh, and inject-by-default is the documented policy.
// trace:BUG-838 | ai:claude
pub(crate) fn agents_md_block_enabled(project_root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
    else {
        return true;
    };
    let Ok(doc) = content.parse::<toml::Value>() else {
        return true;
    };
    doc.get("scaffold")
        .and_then(|s| s.get("agents_md_block"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// The AGENTS.md AIDA-AUTOGEN block is part of the refresh contract: an
/// EXISTING AGENTS.md gets the block appended when it has no markers and the
/// block content refreshed when it does — user-owned content outside the
/// delimiters is preserved byte-for-byte. Refresh never installs: a project
/// with no AGENTS.md, an agent profile that doesn't generate one, a symlinked
/// destination, or the `[scaffold] agents_md_block = false` opt-out all leave
/// the file exactly as it is.
// trace:BUG-838 | ai:claude
fn agents_md_block_refresh(
    project_root: &Path,
    preview: &aida_core::scaffolding::ScaffoldPreview,
) -> Option<PackRefresh> {
    if !agents_md_block_enabled(project_root) {
        return None;
    }
    let artifact = preview
        .artifacts
        .iter()
        .find(|a| a.path == Path::new("AGENTS.md"))?;
    let dest = project_root.join("AGENTS.md");
    let mut report = RefreshReport::default();
    if dest
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        report.record(
            Path::new("AGENTS.md"),
            aida_core::scaffolding::refresh::RefreshOutcome::SkippedSymlink(dest),
        );
    } else {
        let existing = std::fs::read_to_string(&dest).ok()?;
        let (merged, _) =
            aida_core::scaffolding::merge_agents_md_aida_block(&existing, &artifact.content);
        if aida_core::scaffolding::generated_text_matches(&merged, &existing) {
            report.record(
                Path::new("AGENTS.md"),
                aida_core::scaffolding::refresh::RefreshOutcome::Unchanged,
            );
        } else {
            match std::fs::write(&dest, &merged) {
                Ok(()) => report.record(
                    Path::new("AGENTS.md"),
                    aida_core::scaffolding::refresh::RefreshOutcome::Refreshed,
                ),
                Err(e) => {
                    eprintln!(
                        "  {} could not refresh AGENTS.md: {}",
                        "Warning:".yellow(),
                        e
                    );
                    return None;
                }
            }
        }
    }
    Some(PackRefresh {
        label: "AGENTS.md AIDA block".to_string(),
        location: project_root.display().to_string(),
        report,
    })
}

/// Report an installed `~/.codex/prompts` pack without writing to it.
///
/// Codex >=0.142 does not discover this directory as a slash-command surface,
/// so refresh no longer adopts unmarked prompt files into edit-tracking. That
/// old adoption path produced one `.aida-bak` file per prompt while keeping a
/// dead global pack alive.
fn codex_prompts_refresh(dest: Option<&Path>) -> Option<PackRefresh> {
    let dir = match dest {
        Some(d) => d.to_path_buf(),
        None => dirs::home_dir()?.join(".codex").join("prompts"),
    };
    if !dir.is_dir() {
        return None;
    }
    let mut report = RefreshReport::default();
    for (name, _) in aida_core::scaffolding::codex_prompts::expected_codex_prompts() {
        let file = format!("{name}.md");
        let dest = dir.join(&file);
        if dest.exists() {
            report.kept_unmarked.push(PathBuf::from(&file));
        }
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if name.starts_with("aida-") && name.ends_with(".aida-bak") {
                report.kept_unmarked.push(PathBuf::from(name));
            }
        }
    }
    Some(PackRefresh {
        label: "Codex prompts (deprecated)".to_string(),
        location: dir.display().to_string(),
        report,
    })
}

/// Print the per-pack summary. Silent about packs that had nothing installed.
pub(crate) fn print_refresh_summary(packs: &[PackRefresh]) {
    let mut total = RefreshReport::default();
    for pack in packs {
        total.absorb(&pack.report);
    }
    let installed: Vec<&PackRefresh> = packs
        .iter()
        .filter(|p| p.report.unchanged + p.report.changed() > 0 || !p.report.kept_edited.is_empty())
        .collect();

    println!();
    println!("  {}:", "Agent packs refreshed".bold());
    if installed.is_empty() {
        println!("    no installed agent packs found — nothing to refresh");
    }
    for pack in installed {
        let r = &pack.report;
        println!(
            "    {:<20} {} updated · {} unchanged · {} kept (edited) · {} kept (yours)",
            pack.label,
            r.changed().to_string().blue(),
            r.unchanged,
            r.kept_edited.len().to_string().yellow(),
            r.kept_unmarked.len(),
        );
        println!("      {}", pack.location.dimmed());
    }
    // trace:STORY-1475 | ai:claude
    if !total.installed.is_empty() {
        println!(
            "    {} newly shipped skill(s) added to installed packs: {}",
            total.installed.len(),
            total
                .installed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !total.adopted.is_empty() {
        println!(
            "    {} file(s) that predate edit-tracking were brought current; the previous copies are saved alongside as .aida-bak",
            total.adopted.len()
        );
    }
    if packs
        .iter()
        .any(|p| p.label == "Codex prompts (deprecated)")
    {
        println!(
            "    {} ~/.codex/prompts is not discoverable on Codex >=0.142; refresh left it untouched. Prune it with `rm -rf ~/.codex/prompts` or delete its `aida-*.md` and `*.aida-bak` files.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    }
    if !total.skipped_symlink.is_empty() {
        // BUG-718: expected in the AIDA dev repo, where the pack files are
        // per-file symlinks into the template masters.
        println!(
            "    {} {} symlinked file(s) skipped so their targets stay intact",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            total.skipped_symlink.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::scaffolding::wrap_with_aida_header;

    #[test]
    fn starter_role_refresh_adds_missing_prompt_and_preserves_user_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let roles = tmp.path().join(".aida/roles");
        std::fs::create_dir_all(&roles).unwrap();
        let advisor = roles.join("advisor.toml");
        let legacy = r#"# operator comment
name = "advisor"
purpose = "My edited purpose"
created_at = "2026-09-01T00:00:00Z"
last_active_at = "2026-09-01T00:00:00Z"
global = true
"#;
        std::fs::write(&advisor, legacy).unwrap();
        let packs = refresh_agent_packs_at(
            tmp.path(),
            Some(&tmp.path().join("no-prompts")),
            Some(&roles),
        );
        let role_pack = packs
            .iter()
            .find(|pack| pack.label == "Starter roles")
            .expect("legacy role should be refreshed");
        assert_eq!(
            role_pack.report.refreshed,
            vec![PathBuf::from("advisor.toml")]
        );
        let refreshed = std::fs::read_to_string(&advisor).unwrap();
        assert!(refreshed.starts_with(legacy), "existing role bytes changed");
        assert!(refreshed.contains("system_prompt ="));
        assert!(refreshed.contains("rework-brief-craft.md"));

        let prompt_line = refreshed
            .lines()
            .find(|line| line.starts_with("system_prompt ="))
            .unwrap();
        let custom = refreshed.replace(
            prompt_line,
            "system_prompt = \"My custom operating instructions\"",
        );
        std::fs::write(&advisor, &custom).unwrap();
        let packs = refresh_agent_packs_at(
            tmp.path(),
            Some(&tmp.path().join("no-prompts")),
            Some(&roles),
        );
        assert!(packs.iter().all(|pack| pack.label != "Starter roles"));
        assert_eq!(std::fs::read_to_string(&advisor).unwrap(), custom);
    }

    /// BUG-1579: every sibling pack-refresh test uses the injected core with
    /// starter roles disabled. It therefore cannot consume this fixture's
    /// migration before the owning test reaches it.
    #[test]
    fn unrelated_pack_refresh_cannot_consume_starter_role_fixture() {
        let fixture = tempfile::tempdir().unwrap();
        let roles = fixture.path().join("roles");
        std::fs::create_dir_all(&roles).unwrap();
        std::fs::write(
            roles.join("advisor.toml"),
            "name = \"advisor\"\npurpose = \"fixture\"\ncreated_at = \"2026-09-01T00:00:00Z\"\nlast_active_at = \"2026-09-01T00:00:00Z\"\nglobal = true\n",
        )
        .unwrap();

        let unrelated = tempfile::tempdir().unwrap();
        let _ = refresh_agent_packs_at(unrelated.path(), None, None);
        let packs = refresh_agent_packs_at(fixture.path(), None, Some(&roles));

        let role_pack = packs
            .iter()
            .find(|pack| pack.label == "Starter roles")
            .expect("only the owning refresh consumes the role migration");
        assert_eq!(
            role_pack.report.refreshed,
            vec![PathBuf::from("advisor.toml")]
        );
    }

    /// A refresh over a project with an installed, pristine-but-stale Claude
    /// skill overlays it; an edited sibling and a symlinked sibling survive.
    #[test]
    fn project_pack_refresh_overlays_pristine_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skills = root.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();

        // Write a deliberately stale-but-pristine copy of a real skill: same
        // marker shape, different body → the checksum still matches its OWN
        // body, so refresh must overlay it with the embedded master.
        // BUG-1135: skills scaffold in directory form (`<name>/SKILL.md`), so
        // the installed fixture lives there too.
        let stale = wrap_with_aida_header(
            Path::new(".claude/skills/aida-req/SKILL.md"),
            "---\nname: aida-req\n---\n# Old\n\nstale body\n",
        );
        let req_dir = skills.join("aida-req");
        std::fs::create_dir_all(&req_dir).unwrap();
        std::fs::write(req_dir.join("SKILL.md"), &stale).unwrap();

        // An edited copy of another skill must survive untouched.
        let edited = wrap_with_aida_header(
            Path::new(".claude/skills/aida-commit/SKILL.md"),
            "---\nname: aida-commit\n---\n# Mine\n\nbody\n",
        )
        .replace("body", "MY OWN body");
        let commit_dir = skills.join("aida-commit");
        std::fs::create_dir_all(&commit_dir).unwrap();
        std::fs::write(commit_dir.join("SKILL.md"), &edited).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        let claude = packs
            .iter()
            .find(|p| p.label == "Claude skills")
            .expect("claude skills pack");
        assert_eq!(
            claude.report.refreshed.len(),
            1,
            "exactly the pristine file is overlaid: {:?}",
            claude.report
        );
        assert_eq!(claude.report.kept_edited.len(), 1);
        assert_ne!(
            std::fs::read_to_string(req_dir.join("SKILL.md")).unwrap(),
            stale
        );
        assert_eq!(
            std::fs::read_to_string(commit_dir.join("SKILL.md")).unwrap(),
            edited,
            "an edited pack file is never overwritten"
        );

        // Nothing that was not already installed gets created.
        assert!(!root.join(".codex").exists());
        assert!(!root.join(".antigravity").exists());
    }

    use aida_core::scaffolding::refresh::{
        read_skill_manifest, write_skill_manifest, DELIVERED_MANIFEST,
    };

    const SKILL_PACKS: [(&str, &str); 3] = [
        (".claude/skills", "Claude skills"),
        (".codex/skills", "Codex skills"),
        (".antigravity/skills", "Antigravity skills"),
    ];

    /// `aida init` / `scaffold apply` into an empty project.
    fn install_all_packs(root: &Path) {
        let mut scaffolder = aida_core::scaffolding::Scaffolder::new(
            root.to_path_buf(),
            aida_core::scaffolding::ScaffoldConfig::default(),
        );
        let preview = scaffolder.preview(&aida_core::RequirementsStore::default());
        scaffolder.apply(&preview).expect("apply");
    }

    /// Make `name` look like a skill AIDA started shipping after `pack` was
    /// installed: absent on disk and never recorded in the manifest.
    fn forget_delivery(pack_dir: &Path, name: &str) {
        std::fs::remove_dir_all(pack_dir.join(name)).unwrap();
        let mut m = read_skill_manifest(pack_dir).unwrap().unwrap();
        assert!(m.delivered.remove(name), "{name} was delivered by apply");
        write_skill_manifest(pack_dir, &m).unwrap();
    }

    fn installed_in<'a>(packs: &'a [PackRefresh], label: &str) -> &'a [PathBuf] {
        packs
            .iter()
            .find(|p| p.label == label)
            .map(|p| p.report.installed.as_slice())
            .unwrap_or(&[])
    }

    /// A skill new since the pack was installed is delivered on refresh into
    /// the Claude, Codex and Antigravity packs, and recorded in each pack's
    /// own manifest; a second refresh installs nothing.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn refresh_delivers_skill_new_since_install_into_every_skill_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        for (pack, _) in SKILL_PACKS {
            forget_delivery(&root.join(pack), "aida-orchestrate");
        }

        let packs = refresh_agent_packs_at(root, None, None);
        for (pack, label) in SKILL_PACKS {
            let rel = PathBuf::from(pack).join("aida-orchestrate/SKILL.md");
            assert!(root.join(&rel).is_file(), "{pack} receives the new skill");
            assert_eq!(installed_in(&packs, label), [rel], "{pack}");
            let m = read_skill_manifest(&root.join(pack)).unwrap().unwrap();
            assert!(
                m.complete && m.delivered.contains("aida-orchestrate"),
                "{pack}"
            );
        }

        let again = refresh_agent_packs_at(root, None, None);
        assert!(
            again.iter().all(|p| p.report.installed.is_empty()),
            "a second refresh installs nothing new"
        );
    }

    /// A delivered skill the user deletes stays deleted, and the deletion is
    /// recorded in the manifest as an opt-out.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn refresh_keeps_deleted_skill_deleted_and_records_the_opt_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        for (pack, _) in SKILL_PACKS {
            std::fs::remove_dir_all(root.join(pack).join("aida-commit")).unwrap();
        }

        let packs = refresh_agent_packs_at(root, None, None);
        assert!(packs.iter().all(|p| p.report.installed.is_empty()));
        for (pack, _) in SKILL_PACKS {
            let dir = root.join(pack);
            assert!(!dir.join("aida-commit").exists(), "{pack}: not resurrected");
            let m = read_skill_manifest(&dir).unwrap().unwrap();
            assert!(
                m.opted_out.contains("aida-commit"),
                "{pack}: opt-out recorded"
            );
            assert!(!m.delivered.contains("aida-commit"), "{pack}");
            assert!(m.delivered.contains("aida-req"), "{pack}");
        }
    }

    /// `aida init` / `scaffold apply` writes one manifest per pack directory,
    /// and a re-run of apply (the init / upgrade path) never recreates a
    /// delivered skill the user deleted.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn apply_writes_one_manifest_per_pack_and_reapply_respects_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        for (pack, _) in SKILL_PACKS {
            let dir = root.join(pack);
            assert!(dir.join(DELIVERED_MANIFEST).is_file(), "{pack}");
            let m = read_skill_manifest(&dir).unwrap().unwrap();
            assert!(
                m.complete && m.delivered.contains("aida-orchestrate"),
                "{pack}"
            );
            std::fs::remove_dir_all(dir.join("aida-orchestrate")).unwrap();
        }
        let manifests = walkdir_manifests(root);
        assert_eq!(manifests.len(), SKILL_PACKS.len(), "{manifests:?}");

        install_all_packs(root);
        refresh_agent_packs_at(root, None, None);
        for (pack, _) in SKILL_PACKS {
            let dir = root.join(pack);
            assert!(!dir.join("aida-orchestrate").exists(), "{pack}");
            let m = read_skill_manifest(&dir).unwrap().unwrap();
            assert!(m.opted_out.contains("aida-orchestrate"), "{pack}");
        }
    }

    fn walkdir_manifests(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                out.extend(walkdir_manifests(&path));
            } else if entry.file_name() == DELIVERED_MANIFEST {
                out.push(path);
            }
        }
        out
    }

    /// A pack installed before the manifest existed (no manifest, or a
    /// STORY-1475 allow-list manifest) fails closed: the first refresh seeds
    /// the manifest from the skill directories on disk and creates nothing,
    /// and the next refresh creates nothing either.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn legacy_pack_is_seeded_from_disk_and_refresh_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let codex = root.join(".codex/skills");
        let req = wrap_with_aida_header(
            Path::new(".codex/skills/aida-req/SKILL.md"),
            "---\nname: aida-req\n---\n# Req\n",
        );
        std::fs::create_dir_all(codex.join("aida-req")).unwrap();
        std::fs::write(codex.join("aida-req/SKILL.md"), &req).unwrap();
        // An Antigravity pack carrying a STORY-1475 allow-list manifest.
        let agy = root.join(".antigravity/skills");
        std::fs::create_dir_all(agy.join("aida-req")).unwrap();
        std::fs::write(agy.join("aida-req/SKILL.md"), &req).unwrap();
        std::fs::write(
            agy.join(DELIVERED_MANIFEST),
            "# Skills AIDA has delivered into this pack.\naida-orchestrate\n",
        )
        .unwrap();
        // An emptied pack with no manifest at all.
        std::fs::create_dir_all(root.join(".claude/skills")).unwrap();

        for _ in 0..2 {
            let packs = refresh_agent_packs_at(root, None, None);
            assert!(
                packs.iter().all(|p| p.report.installed.is_empty()),
                "a legacy pack receives nothing"
            );
        }
        for dir in [&codex, &agy] {
            let entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| !n.starts_with('.'))
                .collect();
            assert_eq!(entries, vec!["aida-req".to_string()], "{}", dir.display());
            let m = read_skill_manifest(dir).unwrap().unwrap();
            assert!(m.complete);
            assert_eq!(m.delivered.iter().collect::<Vec<_>>(), ["aida-req"]);
            assert!(m.opted_out.contains("aida-orchestrate"));
            assert!(m.opted_out.contains("aida-commit"));
        }
        let claude: Vec<_> = std::fs::read_dir(root.join(".claude/skills"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(claude, vec![DELIVERED_MANIFEST.to_string()]);
    }

    /// An unreadable delivered manifest fails closed: refresh creates nothing
    /// and leaves the manifest untouched.
    // trace:STORY-1475 | ai:claude
    // trace:TASK-1503 | ai:claude
    #[test]
    fn unreadable_delivered_manifest_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        let codex = root.join(".codex/skills");
        forget_delivery(&codex, "aida-orchestrate");
        let manifest = codex.join(DELIVERED_MANIFEST);
        std::fs::write(&manifest, [0xff, 0xfe, 0x00]).unwrap();
        // A directory in the manifest's place fails closed too.
        let agy = root.join(".antigravity/skills");
        forget_delivery(&agy, "aida-orchestrate");
        std::fs::remove_file(agy.join(DELIVERED_MANIFEST)).unwrap();
        std::fs::create_dir_all(agy.join(DELIVERED_MANIFEST)).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        assert!(!codex.join("aida-orchestrate").exists());
        assert!(!agy.join("aida-orchestrate").exists());
        assert!(installed_in(&packs, "Codex skills").is_empty());
        assert!(installed_in(&packs, "Antigravity skills").is_empty());
        assert_eq!(std::fs::read(&manifest).unwrap(), vec![0xff, 0xfe, 0x00]);
    }

    /// The delivery is recorded before `SKILL.md` is written: if the manifest
    /// cannot be written, nothing is created.
    // trace:STORY-1475 | ai:claude
    // trace:TASK-1503 | ai:claude
    #[cfg(unix)]
    #[test]
    fn failed_delivery_record_writes_no_skill() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        let codex = root.join(".codex/skills");
        forget_delivery(&codex, "aida-orchestrate");
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = codex.join("probe");
        let writable = std::fs::write(&probe, "x").is_ok();
        let packs = refresh_agent_packs_at(root, None, None);
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            return; // running as root: permissions are not enforced
        }
        assert!(!codex.join("aida-orchestrate").exists());
        assert!(installed_in(&packs, "Codex skills").is_empty());
    }

    /// A symlinked skill directory the user owns is never written through,
    /// and counts as installed (recorded as delivered).
    // trace:TASK-1503 | ai:claude
    #[cfg(unix)]
    #[test]
    fn symlinked_skill_is_left_untouched_by_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        install_all_packs(root);
        let codex = root.join(".codex/skills");
        forget_delivery(&codex, "aida-orchestrate");
        let target = root.join("my-orchestrate");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, codex.join("aida-orchestrate")).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        assert!(installed_in(&packs, "Codex skills").is_empty());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        let m = read_skill_manifest(&codex).unwrap().unwrap();
        assert!(m.delivered.contains("aida-orchestrate"));
    }

    #[test]
    fn project_pack_refresh_respects_enabled_agent_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skills = root.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[agents]\nenabled = [\"codex\"]\n",
        )
        .unwrap();

        let stale = wrap_with_aida_header(
            Path::new(".claude/skills/aida-req.md"),
            "---\nname: aida-req\n---\n# Old\n\nstale body\n",
        );
        std::fs::write(skills.join("aida-req.md"), &stale).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        assert!(
            packs.iter().all(|p| p.label != "Claude skills"),
            "disabled Claude pack should not refresh: {:?}",
            packs.iter().map(|p| &p.label).collect::<Vec<_>>()
        );
        assert_eq!(
            std::fs::read_to_string(skills.join("aida-req.md")).unwrap(),
            stale
        );
    }

    /// BUG-1118: `~/.codex/prompts` is not a discoverable custom-command
    /// surface on modern Codex, so refresh reports an installed pack without
    /// rewriting it or creating `.aida-bak` siblings.
    #[test]
    fn stale_codex_prompt_is_reported_but_not_rewritten_or_backed_up() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("aida-guided-implement.md");
        let stale =
            "# Guided implement\n\nAsk via `AskUserQuestion`. No arguments placeholder here.\n";
        std::fs::write(&dest, stale).unwrap();

        let packs = refresh_agent_packs_at(tmp.path(), Some(tmp.path()), None);
        let prompts = packs
            .iter()
            .find(|p| p.label == "Codex prompts (deprecated)")
            .expect("legacy prompt pack is reported");
        assert_eq!(
            prompts.report.kept_unmarked.len(),
            1,
            "{:?}",
            prompts.report
        );
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), stale);
        assert!(
            !tmp.path()
                .join("aida-guided-implement.md.aida-bak")
                .exists(),
            "refresh must not accumulate backup files for the dead prompt surface"
        );
    }

    /// BUG-718: a symlinked pack file (the AIDA dev-repo layout) is skipped,
    /// and the master it points at is byte-identical afterwards.
    #[cfg(unix)]
    #[test]
    fn symlinked_pack_file_leaves_its_master_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skills = root.join(".claude/skills");
        let req_dir = skills.join("aida-req");
        std::fs::create_dir_all(&req_dir).unwrap();

        let master = root.join("master-aida-req.md");
        let original = wrap_with_aida_header(
            Path::new(".claude/skills/aida-req/SKILL.md"),
            "---\nname: aida-req\n---\n# Master\n\nsource of truth\n",
        );
        std::fs::write(&master, &original).unwrap();
        std::os::unix::fs::symlink(&master, req_dir.join("SKILL.md")).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        let claude = packs
            .iter()
            .find(|p| p.label == "Claude skills")
            .expect("claude skills pack");
        assert_eq!(
            claude.report.skipped_symlink.len(),
            1,
            "the symlink must be skipped: {:?}",
            claude.report
        );
        assert!(claude.report.refreshed.is_empty());
        assert_eq!(
            std::fs::read_to_string(&master).unwrap(),
            original,
            "writing through the symlink would have corrupted the master"
        );
    }

    /// BUG-838: refresh appends the AIDA-AUTOGEN block to an existing
    /// AGENTS.md that has no markers — user content preserved byte-for-byte,
    /// only the delimited block (never the generated seed's framing) added,
    /// and a second refresh converges to unchanged.
    #[test]
    fn refresh_appends_aida_block_to_unmarked_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let original = "# My project agents\n\nHouse rules the team wrote.\n";
        std::fs::write(root.join("AGENTS.md"), original).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        let pack = packs
            .iter()
            .find(|p| p.label == "AGENTS.md AIDA block")
            .expect("agents-md pack in refresh report");
        assert_eq!(pack.report.refreshed.len(), 1, "{:?}", pack.report);

        let content = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(content.starts_with(original), "user content must lead");
        assert_eq!(content.matches("<!-- AIDA-AUTOGEN-BEGIN -->").count(), 1);
        assert_eq!(content.matches("<!-- AIDA-AUTOGEN-END -->").count(), 1);
        assert!(content.contains("# AIDA Conventions"));
        assert!(
            !content.contains("Guidance for Codex and MCP-compatible coding agents"),
            "the generated seed's framing must never be spliced into a user file"
        );

        // Idempotent: the second refresh reports unchanged, content is stable.
        let packs2 = refresh_agent_packs_at(root, None, None);
        if let Some(pack2) = packs2.iter().find(|p| p.label == "AGENTS.md AIDA block") {
            assert!(pack2.report.refreshed.is_empty(), "{:?}", pack2.report);
            assert_eq!(pack2.report.unchanged, 1);
        }
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            content
        );
    }

    /// BUG-838: no AGENTS.md means refresh installs nothing — the file must
    /// not appear, and no pack row is reported for it.
    #[test]
    fn refresh_never_creates_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let packs = refresh_agent_packs_at(root, None, None);
        assert!(packs.iter().all(|p| p.label != "AGENTS.md AIDA block"));
        assert!(!root.join("AGENTS.md").exists());
    }

    #[test]
    fn refresh_relocates_legacy_discipline_pack() {
        // trace:STORY-829 | ai:codex
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let old = root.join("docs/aida/discipline");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("README.md"), "edited legacy readme\n").unwrap();
        std::fs::write(
            root.join(".gitignore"),
            ".aida-store/\n.aida/*\n!.aida/config.toml\n",
        )
        .unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        let discipline = packs
            .iter()
            .find(|p| p.label == "Discipline pack")
            .expect("discipline pack refresh row");
        assert!(
            discipline
                .report
                .refreshed
                .contains(&PathBuf::from(".aida/discipline/README.md")),
            "{:?}",
            discipline.report
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".aida/discipline/README.md")).unwrap(),
            "edited legacy readme\n"
        );
        assert!(
            !root.join("docs/aida/discipline").exists(),
            "empty legacy tree should be removed"
        );
        let gitignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains("!.aida/discipline/"), "{gitignore}");
        assert!(gitignore.contains("!.aida/discipline/**"), "{gitignore}");
    }

    #[test]
    fn refresh_installs_current_session_discipline_template_when_missing() {
        // trace:TASK-1203 | ai:codex
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida/discipline")).unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        let discipline = packs
            .iter()
            .find(|p| p.label == "Discipline pack")
            .expect("discipline pack refresh row");
        assert!(
            discipline
                .report
                .refreshed
                .contains(&PathBuf::from(".aida/discipline/session-discipline.md")),
            "{:?}",
            discipline.report
        );

        let body =
            std::fs::read_to_string(root.join(".aida/discipline/session-discipline.md")).unwrap();
        assert!(
            body.contains("Re-read live state before acting on remembered IDs"),
            "{body}"
        );
        assert!(
            body.contains("release_task") && body.contains("not-found"),
            "{body}"
        );
        assert!(body.contains("no-briefs-found"), "{body}");
    }

    /// BUG-838: the `[scaffold] agents_md_block = false` opt-out leaves an
    /// unmarked AGENTS.md byte-identical through a refresh.
    #[test]
    fn refresh_honors_agents_md_block_opt_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let original = "# Agents\n\nNo AIDA here, thanks.\n";
        std::fs::write(root.join("AGENTS.md"), original).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[scaffold]\nagents_md_block = false\n",
        )
        .unwrap();

        let packs = refresh_agent_packs_at(root, None, None);
        assert!(packs.iter().all(|p| p.label != "AGENTS.md AIDA block"));
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            original
        );
    }

    /// BUG-838: knob parsing — missing file/section/key and parse errors all
    /// mean enabled; only an explicit `false` disables.
    #[test]
    fn agents_md_block_enabled_defaults_and_opt_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(agents_md_block_enabled(root), "no config file → enabled");

        std::fs::create_dir_all(root.join(".aida")).unwrap();
        let cfg = root.join(".aida/config.toml");
        std::fs::write(&cfg, "[archive]\nauto_after_days = 30\n").unwrap();
        assert!(
            agents_md_block_enabled(root),
            "no [scaffold] section → enabled"
        );

        std::fs::write(&cfg, "[scaffold]\nagents_md_block = true\n").unwrap();
        assert!(agents_md_block_enabled(root));

        std::fs::write(&cfg, "[scaffold]\nagents_md_block = false\n").unwrap();
        assert!(!agents_md_block_enabled(root));

        std::fs::write(&cfg, "not [ valid toml").unwrap();
        assert!(agents_md_block_enabled(root), "parse error → enabled");
    }
}
