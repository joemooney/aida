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
//! Antigravity skills and the machine-global Codex custom prompts through one
//! mechanism instead of one per vendor.
// trace:TASK-1170 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::scaffolding::refresh::{refresh_file, RefreshReport};
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

/// Refresh every installed agent pack under `project_root`, plus the
/// machine-global Codex custom prompts. Only files that already exist are
/// touched — installing a pack the project opted out of stays `aida init`'s
/// job, so a Claude-only project never grows a `.codex/` tree from a refresh.
///
/// `codex_prompts_dest` overrides the machine-global `~/.codex/prompts`
/// location (mirrors `scaffold codex-prompts --dest`).
pub(crate) fn refresh_agent_packs(
    project_root: &Path,
    codex_prompts_dest: Option<&Path>,
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
        selection.apply_to_scaffold_config(&mut config);
    }
    let db_path = project_root.join(".aida").join("cache.db");
    let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
        project_root.to_path_buf(),
        config,
        db_path,
    );
    let store = aida_core::RequirementsStore::default();
    let preview = scaffolder.preview(&store);

    for artifact in &preview.artifacts {
        let rel = artifact.path.to_string_lossy().replace('\\', "/");
        let Some(idx) = PROJECT_PACKS.iter().position(|(p, _)| rel.starts_with(p)) else {
            continue;
        };
        let dest = project_root.join(&artifact.path);
        match refresh_file(&dest, &artifact.content, false) {
            Ok(outcome) => packs[idx].report.record(&artifact.path, outcome),
            Err(e) => eprintln!(
                "  {} could not refresh {}: {}",
                "Warning:".yellow(),
                artifact.path.display(),
                e
            ),
        }
    }

    if let Some(prompts) = codex_prompts_refresh(codex_prompts_dest) {
        packs.push(prompts);
    }
    packs.retain(|p| p.report != RefreshReport::default());
    packs
}

/// Refresh `~/.codex/prompts` when the machine actually has it installed.
///
/// These files shipped before they carried a scaffold marker, so an unmarked
/// prompt here is adopted into edit-tracking rather than skipped — the previous
/// content is saved to a `.aida-bak` sibling first, so nothing is lost, and
/// every later refresh follows the strict marked/edited contract.
fn codex_prompts_refresh(dest: Option<&Path>) -> Option<PackRefresh> {
    let dir = match dest {
        Some(d) => d.to_path_buf(),
        None => dirs::home_dir()?.join(".codex").join("prompts"),
    };
    if !dir.is_dir() {
        return None;
    }
    let mut report = RefreshReport::default();
    for (name, expected) in aida_core::scaffolding::codex_prompts::expected_codex_prompts() {
        let file = format!("{name}.md");
        let dest = dir.join(&file);
        match refresh_file(&dest, &expected, true) {
            Ok(outcome) => report.record(&PathBuf::from(&file), outcome),
            Err(e) => eprintln!(
                "  {} could not refresh {}: {}",
                "Warning:".yellow(),
                file,
                e
            ),
        }
    }
    Some(PackRefresh {
        label: "Codex prompts".to_string(),
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
    if !total.adopted.is_empty() {
        println!(
            "    {} file(s) that predate edit-tracking were brought current; the previous copies are saved alongside as .aida-bak",
            total.adopted.len()
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
        let stale = wrap_with_aida_header(
            Path::new(".claude/skills/aida-req.md"),
            "---\nname: aida-req\n---\n# Old\n\nstale body\n",
        );
        std::fs::write(skills.join("aida-req.md"), &stale).unwrap();

        // An edited copy of another skill must survive untouched.
        let edited = wrap_with_aida_header(
            Path::new(".claude/skills/aida-commit.md"),
            "---\nname: aida-commit\n---\n# Mine\n\nbody\n",
        )
        .replace("body", "MY OWN body");
        std::fs::write(skills.join("aida-commit.md"), &edited).unwrap();

        let packs = refresh_agent_packs(root, None);
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
            std::fs::read_to_string(skills.join("aida-req.md")).unwrap(),
            stale
        );
        assert_eq!(
            std::fs::read_to_string(skills.join("aida-commit.md")).unwrap(),
            edited,
            "an edited pack file is never overwritten"
        );

        // Nothing that was not already installed gets created.
        assert!(!root.join(".codex").exists());
        assert!(!root.join(".antigravity").exists());
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

        let packs = refresh_agent_packs(root, None);
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

    /// The delivery gap this closes: a `~/.codex/prompts` deployed by an older
    /// binary carries no marker and is stale (no `$ARGUMENTS`, Claude-only
    /// language). One refresh brings it level with the embedded template,
    /// keeps the old copy alongside, and leaves it precisely tracked after.
    #[test]
    fn stale_codex_prompt_is_brought_level_with_the_embedded_template() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("aida-guided-implement.md");
        std::fs::write(
            &dest,
            "# Guided implement\n\nAsk via `AskUserQuestion`. No arguments placeholder here.\n",
        )
        .unwrap();

        let expected = aida_core::scaffolding::codex_prompts::expected_codex_prompts()
            .into_iter()
            .find(|(n, _)| n == "aida-guided-implement")
            .map(|(_, body)| body)
            .expect("guided-implement ships as a Codex prompt");

        refresh_file(&dest, &expected, true).unwrap();

        let now = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(now, expected);
        assert!(now.contains("$ARGUMENTS"), "{now}");
        assert!(!now.contains("AskUserQuestion"), "{now}");
        assert!(tmp
            .path()
            .join("aida-guided-implement.md.aida-bak")
            .exists());

        // Now tracked: a second refresh is a no-op, and a user edit sticks.
        assert_eq!(
            refresh_file(&dest, &expected, true).unwrap(),
            aida_core::scaffolding::refresh::RefreshOutcome::Unchanged
        );
        let mine = expected.replace("$ARGUMENTS", "$ARGUMENTS # mine");
        std::fs::write(&dest, &mine).unwrap();
        assert_eq!(
            refresh_file(&dest, &expected, true).unwrap(),
            aida_core::scaffolding::refresh::RefreshOutcome::KeptEdited
        );
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), mine);
    }

    /// BUG-718: a symlinked pack file (the AIDA dev-repo layout) is skipped,
    /// and the master it points at is byte-identical afterwards.
    #[cfg(unix)]
    #[test]
    fn symlinked_pack_file_leaves_its_master_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let skills = root.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();

        let master = root.join("master-aida-req.md");
        let original = wrap_with_aida_header(
            Path::new(".claude/skills/aida-req.md"),
            "---\nname: aida-req\n---\n# Master\n\nsource of truth\n",
        );
        std::fs::write(&master, &original).unwrap();
        std::os::unix::fs::symlink(&master, skills.join("aida-req.md")).unwrap();

        let packs = refresh_agent_packs(root, None);
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
}
