//! `aida scaffold` command cluster (FR-0260 / FR-1-027 / FR-1-028 / FR-1-047 /
//! BUG-1-065 / BUG-718 / FR-0315).
//!
//! Scaffolding management: re-scaffold skills/commands/hooks and the discipline
//! pack, `scaffold status` / `scaffold diff` (drift report + HTML report), and
//! `scaffold upgrade` (idempotent apply with `--refresh` / `--force` / `--prune`
//! and AIDA-block / managed-slot merges). The shared scaffold helpers
//! (`ensure_discipline_pack_scaffold`, `scaffold_memory_pack`,
//! `check_scaffold_status`, `add_aida_gitignore_entries`, …) stay in `main.rs`
//! and are reached via `crate::`. Extracted verbatim from `main.rs` (SPIKE-78);
//! no behavior change.

use crate::*;
use anyhow::Result;
use colored::Colorize;

// trace:FR-0260 | ai:claude:high
pub(crate) fn handle_scaffold_command(
    cmd: &ScaffoldCommand,
    storage: &Storage,
    db_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        ScaffoldCommand::Status {
            project_root,
            verbose,
            report,
            output,
        } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            if !root.exists() {
                return Err(anyhow::anyhow!(
                    "Project root does not exist: {}",
                    root.display()
                ));
            }

            let config = ScaffoldConfig::default();
            let status = check_scaffold_status(&store, &root, &config, db_path);

            // Generate HTML report if requested
            if *report {
                let html = generate_scaffold_html_report(&store, &root, &config, db_path, &status)?;
                if let Some(output_path) = output {
                    std::fs::write(output_path, &html)?;
                    println!(
                        "{} Scaffold report generated: {}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        output_path.display()
                    );
                } else {
                    println!("{}", html);
                }
                return Ok(());
            }

            if status.is_current {
                println!(
                    "{} Scaffold is up to date",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
            } else {
                println!(
                    "{} Scaffold drift detected",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }

            println!();
            println!(
                "  {} matching, {} modified, {} missing, {} extra",
                status.matching.len().to_string().green(),
                status.modified.len().to_string().yellow(),
                status.missing.len().to_string().red(),
                status.extra.len().to_string().blue()
            );

            if *verbose {
                if !status.matching.is_empty() {
                    println!();
                    println!("{}:", "Matching".green());
                    for path in &status.matching {
                        println!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::Check),
                            path.display()
                        );
                    }
                }

                if !status.modified.is_empty() {
                    println!();
                    println!("{}:", "Modified".yellow());
                    for (path, file_status) in &status.modified {
                        match file_status {
                            FileStatus::Modified {
                                expected_lines,
                                actual_lines,
                            } => {
                                println!(
                                    "  ~ {} (expected {} lines, found {})",
                                    path.display(),
                                    expected_lines,
                                    actual_lines
                                );
                            }
                            _ => {
                                println!("  ~ {}", path.display());
                            }
                        }
                    }
                }

                if !status.missing.is_empty() {
                    println!();
                    println!("{}:", "Missing".red());
                    for path in &status.missing {
                        println!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::Cross),
                            path.display()
                        );
                    }
                }

                if !status.extra.is_empty() {
                    println!();
                    println!("{} (not from scaffold):", "Extra".blue());
                    for path in &status.extra {
                        println!("  + {}", path.display());
                    }
                }
            }
        }

        ScaffoldCommand::Preview { project_root } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            let config = ScaffoldConfig::default();
            let mut scaffolder =
                Scaffolder::with_database(root.clone(), config, db_path.to_path_buf());
            let preview = scaffolder.preview(&store);

            println!("{} Scaffold preview for: {}", "📁".blue(), root.display());
            println!();

            for artifact in &preview.artifacts {
                let exists = root.join(&artifact.path).exists();
                let status = if exists { "exists" } else { "new" };
                println!(
                    "  {} {} ({})",
                    if exists { "~" } else { "+" },
                    artifact.path.display(),
                    status
                );
            }

            println!();
            println!(
                "Total: {} files ({} new, {} existing)",
                preview.artifacts.len(),
                preview
                    .artifacts
                    .iter()
                    .filter(|a| !root.join(&a.path).exists())
                    .count(),
                preview
                    .artifacts
                    .iter()
                    .filter(|a| root.join(&a.path).exists())
                    .count()
            );
        }

        ScaffoldCommand::Apply {
            project_root,
            force,
            dry_run,
            prune,
        } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            let config = ScaffoldConfig::default();
            let mut scaffolder =
                Scaffolder::with_database(root.clone(), config, db_path.to_path_buf());

            if *dry_run {
                println!(
                    "{} Dry run - no files will be modified",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).blue()
                );
                println!();
            }

            let preview = scaffolder.preview(&store);

            let mut created = 0usize;
            let mut updated = 0usize;
            let mut unchanged = 0usize;
            let mut skipped = 0usize;
            let mut would_create = 0usize;
            let mut would_update = 0usize;

            for artifact in &preview.artifacts {
                let full_path = root.join(&artifact.path);

                // BUG-718: never write through a symlink — in the AIDA dev repo
                // the scaffold files are symlinks into aida-core/templates/ and
                // std::fs::write would corrupt the source master. Skip + warn.
                // trace:BUG-718 | ai:claude
                if let Some(target) = aida_core::scaffolding::symlink_target(&full_path) {
                    println!(
                        "  {} {} → {} (skipped — symlink; writing would corrupt the target)",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        artifact.path.display(),
                        target.display()
                    );
                    skipped += 1;
                    continue;
                }

                let exists = full_path.exists();

                if exists && !force && !dry_run {
                    if artifact.path == std::path::Path::new(".git/hooks/pre-commit") {
                        let is_managed = if let Ok(content) = std::fs::read_to_string(&full_path) {
                            content.contains("AIDA Generated")
                                || content.contains("Generated by AIDA")
                        } else {
                            false
                        };
                        if is_managed {
                            // It is AIDA-managed, so do NOT skip. We want to update it!
                        } else {
                            println!(
                                "  {} {} (skipped - exists and contains custom user edits, use --force to overwrite)",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                artifact.path.display()
                            );
                            skipped += 1;
                            continue;
                        }
                    } else {
                        println!(
                            "  {} {} (skipped - exists, use --force to overwrite)",
                            "~".yellow(),
                            artifact.path.display()
                        );
                        skipped += 1;
                        continue;
                    }
                }

                // Detect "no-op" updates: file exists and content already
                // matches what we'd write. Lets us tell the user "0 files
                // needed updating" instead of "all files updated".
                let already_matches = exists
                    && std::fs::read(&full_path)
                        .map(|bytes| bytes == artifact.content.as_bytes())
                        .unwrap_or(false);

                if already_matches {
                    unchanged += 1;
                    continue;
                }

                if !*dry_run {
                    if let Some(parent) = full_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&full_path, &artifact.content)?;
                }

                let action = if exists { "updated" } else { "created" };
                println!(
                    "  {} {} ({})",
                    if exists { "~" } else { "+" },
                    artifact.path.display(),
                    action
                );
                if *dry_run {
                    if exists {
                        would_update += 1;
                    } else {
                        would_create += 1;
                    }
                } else if exists {
                    updated += 1;
                } else {
                    created += 1;
                }
            }

            println!();
            if *dry_run {
                let total_changes = would_create + would_update;
                if total_changes == 0 {
                    println!(
                        "{} Already up to date — {} file(s) match templates exactly, nothing would change.",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        unchanged
                    );
                } else {
                    println!(
                        "{} Dry run: would create {}, update {} ({} unchanged, {} skipped).",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).blue(),
                        would_create,
                        would_update,
                        unchanged,
                        skipped
                    );
                }
            } else {
                let total_changes = created + updated;
                if total_changes == 0 && skipped == 0 {
                    println!(
                        "{} Already up to date — {} file(s) match templates exactly, nothing changed.",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        unchanged
                    );
                } else {
                    println!(
                        "{} Scaffold applied: {} created, {} updated, {} unchanged{}.",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        created,
                        updated,
                        unchanged,
                        if skipped > 0 {
                            format!(", {} skipped (use --force)", skipped)
                        } else {
                            String::new()
                        }
                    );
                }
            }

            // BUG-298 / BUG-719: surface (and with --prune, remove) obsolete
            // `aida-*` files this AIDA version no longer ships. Symlinks and
            // non-`aida-` files are never touched (see the detector).
            report_and_prune_obe_scaffold(&root, *prune, *dry_run, "aida scaffold apply --prune");
        }

        // trace:FR-0269 - Template extraction command | ai:claude:high
        ScaffoldCommand::Extract { output, force } => {
            use aida_core::templates::TemplateLoader;

            let dest = output.clone().unwrap_or_else(|| {
                dirs::config_dir()
                    .map(|p| p.join("aida/templates"))
                    .unwrap_or_else(|| std::path::PathBuf::from("templates"))
            });

            println!(
                "{} Extracting embedded templates to: {}",
                "📦".blue(),
                dest.display()
            );

            // Create the destination directory if it doesn't exist
            if !dest.exists() {
                std::fs::create_dir_all(&dest)?;
            }

            let loader = TemplateLoader::new();
            let templates = loader.list_templates();

            let mut extracted = 0;
            let mut skipped = 0;

            for key in &templates {
                let full_path = dest.join(key);

                // Check if file exists and skip unless force
                if full_path.exists() && !force {
                    println!("  {} {} (skipped - exists)", "~".yellow(), key);
                    skipped += 1;
                    continue;
                }

                // Create parent directories if needed
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Load from embedded and write to disk WITH the AIDA-Generated
                // header so the file round-trips cleanly via `scaffold status`
                // (i.e. user can extract → cp into project → status reports
                // clean, instead of "modified" because the bare embedded bytes
                // have no header). trace:BUG-1-034 | ai:claude
                let mut temp_loader = TemplateLoader::new();
                if let Some(content) = temp_loader.load(key) {
                    let wrapped = aida_core::scaffolding::wrap_with_aida_header(
                        std::path::Path::new(key),
                        &content,
                    );
                    std::fs::write(&full_path, &wrapped)?;
                    println!("  {} {} (extracted)", "+".green(), key);
                    extracted += 1;
                }
            }

            println!();
            println!(
                "{} Extracted {} templates ({} skipped)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                extracted,
                skipped
            );

            if skipped > 0 && !force {
                println!("  Use --force to overwrite existing files");
            }
        }
        ScaffoldCommand::CodexPrompts { dest, force } => {
            // STORY-763: the slash-command parity piece — Codex reads custom
            // prompts from ~/.codex/prompts; each file becomes an invokable
            // /aida-... prompt inside a Codex session.
            let dest_dir = match dest {
                Some(d) => d.clone(),
                None => dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?
                    .join(".codex")
                    .join("prompts"),
            };
            let outcome =
                aida_core::scaffolding::codex_prompts::scaffold_codex_prompts(&dest_dir, *force)?;
            println!(
                "{} Codex custom prompts at {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                dest_dir.display()
            );
            println!(
                "  written: {}   skipped (already present): {}",
                outcome.written.len(),
                outcome.skipped_existing.len()
            );
            if !outcome.excluded.is_empty() {
                println!("  excluded (Claude-specific — not ported):");
                for (name, reason) in &outcome.excluded {
                    println!("    - {} — {}", name.cyan(), reason.dimmed());
                }
            }
            if !outcome.skipped_existing.is_empty() {
                println!(
                    "  {} re-run with --force to overwrite existing prompt files",
                    "·".dimmed()
                );
            }
        }

        ScaffoldCommand::Upgrade {
            project_root,
            dry_run,
            force,
            prune,
        } => {
            // trace:FR-1-028 | ai:claude
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if !root.exists() {
                anyhow::bail!("Project root does not exist: {}", root.display());
            }

            let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
                root.clone(),
                ScaffoldConfig::default(),
                db_path.to_path_buf(),
            );
            let preview = scaffolder.preview(&store);
            run_scaffold_upgrade(&root, &preview, *dry_run, *force, *prune)?;
        }
        ScaffoldCommand::Diff {
            path,
            project_root,
            no_color,
            context,
            list,
        } => {
            // trace:FR-1-027 | ai:claude
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if !root.exists() {
                anyhow::bail!("Project root does not exist: {}", root.display());
            }

            let config = ScaffoldConfig::default();
            // Use with_database so our preview matches what `scaffold status`
            // produces — without the db_path the scaffolder renders CLAUDE.md
            // / AGENTS.md against legacy defaults, which then "drifts" against
            // a fresh `aida init` for purely cosmetic reasons.
            let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
                root.clone(),
                config.clone(),
                db_path.to_path_buf(),
            );
            let preview = scaffolder.preview(&store);

            // Resolve which artifacts to diff. When `path` is given, restrict
            // to that one entry (error if not in the manifest); else walk all
            // artifacts and diff any whose on-disk content differs.
            // Exit codes per FR-1-027: 0=clean, 1=drift, 2=usage error.
            let targets: Vec<&aida_core::scaffolding::ScaffoldArtifact> = match path {
                Some(p) => {
                    let needle = p.clone();
                    match preview.artifacts.iter().find(|a| a.path == needle) {
                        Some(matched) => vec![matched],
                        None => {
                            eprintln!(
                                "Error: {} is not a scaffolded file (run `aida scaffold status` to see what is)",
                                needle.display()
                            );
                            std::process::exit(2);
                        }
                    }
                }
                None => preview.artifacts.iter().collect(),
            };

            let any_drift = print_scaffold_diffs(&root, &targets, *context, *no_color, *list)?;
            if any_drift {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// What `scaffold upgrade` should do for a single artifact, computed from
/// its category + drift state.
// trace:FR-1-028 | ai:claude
enum UpgradeAction {
    /// File missing on disk — create it.
    Create,
    /// File exists, drifted — full overwrite. Templates by default;
    /// anything when `--force`.
    Overwrite,
    /// File exists with AIDA-AUTOGEN markers and the block content is
    /// drifted — rewrite just the marked block, preserve user content
    /// outside the markers.
    RewriteAidaBlock,
    /// CLAUDE.md exists but is missing the `@.claude/AIDA.md` import line.
    /// Insert it, preserving everything else.
    // trace:BUG-1-065 | ai:claude
    InsertClaudeImport,
    /// ManagedMerge file with AIDA-owned slot drift — replace just the
    /// declared slots, preserve everything else verbatim. The `Vec`
    /// records what changed for the per-row UI.
    // trace:FR-1-047 | ai:claude
    SlotMerge {
        changes: Vec<aida_core::SlotChange>,
        merged: serde_json::Value,
    },
    /// File exists, drifted, but the file is user-owned (Seed without
    /// markers) — log and skip.
    LeaveAlone,
    /// File exists and matches — silent count.
    None,
}

/// Pick an upgrade action for a managed-merge file by parsing both the
/// on-disk JSON and the AIDA-rendered template, then running them
/// through `slot_merge`. Falls back to `LeaveAlone` if either side fails
/// to parse — bad JSON should be a user-facing error from elsewhere
/// (e.g. `scaffold status` / `scaffold diff`), not a silent overwrite.
// trace:FR-1-047 | ai:claude
fn decide_managed_merge(
    relative_path: &std::path::Path,
    on_disk_path: &std::path::Path,
    expected_content: &str,
) -> UpgradeAction {
    let actual_text = match std::fs::read_to_string(on_disk_path) {
        Ok(s) => s,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let actual_json: serde_json::Value = match serde_json::from_str(&actual_text) {
        Ok(v) => v,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let expected_json: serde_json::Value = match serde_json::from_str(expected_content) {
        Ok(v) => v,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let slots = aida_core::scaffolding::slots_for_file(relative_path);
    let (merged, changes) = aida_core::scaffolding::slot_merge(&actual_json, &expected_json, slots);
    if changes.is_empty() {
        UpgradeAction::None
    } else {
        UpgradeAction::SlotMerge { changes, merged }
    }
}

/// Replace the content between `<!-- AIDA-AUTOGEN-BEGIN -->` and
/// `<!-- AIDA-AUTOGEN-END -->` in `actual` with the corresponding block
/// from `expected`. Preserves everything outside the markers verbatim.
/// Falls back to `actual` if either side is missing markers (defensive
/// — caller should only invoke this when both have markers).
// trace:FR-1-028 | ai:claude
fn rewrite_aida_block(actual: &str, expected: &str) -> String {
    use aida_core::scaffolding::extract_aida_block;
    let Some(actual_block) = extract_aida_block(actual) else {
        return actual.to_string();
    };
    let Some(expected_block) = extract_aida_block(expected) else {
        return actual.to_string();
    };
    // Replace the first occurrence of the actual block with the expected
    // block. Both extract_aida_block returns are slices of the original
    // strings (between markers), so we can match-replace inside `actual`.
    actual.replacen(actual_block, expected_block, 1)
}

/// Mirror of `report.rs::file_matches_for_status`. Seeds (CLAUDE.md,
/// AGENTS.md) use marker-presence semantics — CLAUDE.md is matching if
/// the file exists at all; AGENTS.md only needs block-content comparison
/// when AIDA-AUTOGEN markers are present (user can opt out by removing
/// them). Templates and managed-merge use whole-content equality.
// trace:FR-1-028 | ai:claude
fn file_matches_artifact(path: &std::path::Path, actual: &str, expected: &str) -> bool {
    use aida_core::FileCategory;
    match FileCategory::from_path(path) {
        FileCategory::Seed => {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            match name {
                // CLAUDE.md drift = missing the @.claude/AIDA.md import line.
                // Mirrors `seed_matches` in aida-core/src/report.rs.
                // trace:BUG-1-065 | ai:claude
                "CLAUDE.md" => aida_core::scaffolding::claude_md_has_import(actual),
                "AGENTS.md" => {
                    match aida_core::scaffolding::extract_aida_block(actual) {
                        // markers present → AIDA owns the block content
                        Some(a) => match aida_core::scaffolding::extract_aida_block(expected) {
                            Some(e) => a.trim() == e.trim(),
                            None => true,
                        },
                        // markers absent → user opted out, fully their file
                        None => true,
                    }
                }
                _ => actual.trim() == expected.trim(),
            }
        }
        FileCategory::Template => actual.trim() == expected.trim(),
        FileCategory::ManagedMerge => {
            // Slot-equality: parse both sides as JSON and compare just the
            // AIDA-owned slots. User keys outside the slots don't trigger
            // drift. Mirrors `report.rs::managed_merge_matches` and what
            // `scaffold upgrade` actually applies. trace:FR-1-047
            use serde_json::Value;
            let Ok(av): Result<Value, _> = serde_json::from_str(actual) else {
                return actual.trim() == expected.trim();
            };
            let Ok(ev): Result<Value, _> = serde_json::from_str(expected) else {
                return actual.trim() == expected.trim();
            };
            let slots = aida_core::scaffolding::slots_for_file(path);
            if slots.is_empty() {
                actual.trim() == expected.trim()
            } else {
                slots.iter().all(|s| av.pointer(s) == ev.pointer(s))
            }
        }
    }
}

/// Category-aware scaffold upgrade. For each artifact, decide what to do
/// based on its `FileCategory` and current drift state, then either
/// write or leave alone. Output is grouped by category with per-file
/// detail only for files that actually need attention or changed.
///
/// Strategies:
///   - Template + drifted/missing → overwrite/create
///   - Template + matching        → no-op (no message)
///   - Seed + missing             → create
///   - Seed + drifted             → leave alone (user owns; drift expected)
///   - Seed + matching            → no-op
///   - ManagedMerge + missing     → create
///   - ManagedMerge + drifted     → v1: leave alone with a "deferred" note
///   - ManagedMerge + matching    → no-op
///
/// `--force` overrides the per-category strategy and overwrites every
/// drifted file regardless of category (parity with `apply --force`,
/// just with cleaner output).
///
// trace:FR-1-028 | ai:claude
fn run_scaffold_upgrade(
    project_root: &std::path::Path,
    preview: &aida_core::ScaffoldPreview,
    dry_run: bool,
    force: bool,
    prune: bool,
) -> Result<()> {
    use aida_core::FileCategory;
    use std::path::PathBuf;

    #[derive(Default)]
    struct CategoryStats {
        upgraded: Vec<PathBuf>,
        created: Vec<PathBuf>,
        left_alone: Vec<PathBuf>,
        // BUG-718: files skipped because they are symlinks — writing would
        // follow the link and corrupt the source-of-truth master (the dev
        // repo dogfoods its templates this way). (on-disk path, link target).
        symlinked: Vec<(PathBuf, PathBuf)>,
        unchanged: usize,
    }

    let mut by_cat: std::collections::BTreeMap<&str, CategoryStats> =
        std::collections::BTreeMap::new();

    for artifact in &preview.artifacts {
        let cat = artifact.category();
        let cat_label = cat.label();
        let stats = by_cat.entry(cat_label).or_default();

        let on_disk_path = project_root.join(&artifact.path);

        // BUG-718: never write *through* a symlink. In the AIDA dev repo the
        // scaffold files under .claude/ are per-file symlinks into
        // aida-core/templates/ (the source-of-truth masters); std::fs::write
        // follows the link and would corrupt the master. Skip + warn instead,
        // for every category/action, in dry-run and for real.
        // trace:BUG-718 | ai:claude
        if let Some(target) = aida_core::scaffolding::symlink_target(&on_disk_path) {
            stats.symlinked.push((artifact.path.clone(), target));
            continue;
        }

        let exists = on_disk_path.exists();
        // Use content-equality directly rather than `artifact.file_status`
        // — there's a pre-existing bug in `check_file_status` where files
        // with YAML frontmatter (skills, commands) report Modified even
        // when they're byte-identical, because the header's stored
        // checksum is computed against the post-frontmatter body but the
        // expected checksum is computed against the full raw_content.
        // `aida scaffold status` already sidesteps this with content
        // equality; matching that behavior here. The underlying bug is
        // tracked separately so file_status can be made trustworthy.
        // trace:FR-1-028 | ai:claude
        let drifted = if exists {
            match std::fs::read_to_string(&on_disk_path) {
                Ok(actual) => !file_matches_artifact(&artifact.path, &actual, &artifact.content),
                Err(_) => true,
            }
        } else {
            // Missing isn't drift — handled separately as "create".
            false
        };

        // Decide action per category. Two special cases for v1.1+ work:
        // - AGENTS.md (Seed) with AIDA-AUTOGEN markers gets a block-only
        //   rewrite (FR-1-035) — preserves user content outside the
        //   block.
        // - Managed-merge files (settings.json, .mcp.json) with drift
        //   in any AIDA-owned slot get a slot-merge (FR-1-047) — replace
        //   only the declared slots, preserve every other key verbatim.
        let action = if !exists {
            UpgradeAction::Create
        } else if !drifted {
            UpgradeAction::None
        } else if force {
            UpgradeAction::Overwrite
        } else {
            match cat {
                FileCategory::Template => UpgradeAction::Overwrite,
                FileCategory::Seed => {
                    let name = artifact
                        .path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    let actual_text = std::fs::read_to_string(&on_disk_path).ok();
                    match name {
                        "AGENTS.md"
                            if actual_text
                                .as_deref()
                                .and_then(aida_core::scaffolding::extract_aida_block)
                                .is_some() =>
                        {
                            UpgradeAction::RewriteAidaBlock
                        }
                        "CLAUDE.md"
                            if !actual_text
                                .as_deref()
                                .map(aida_core::scaffolding::claude_md_has_import)
                                .unwrap_or(true) =>
                        {
                            // The only AIDA-managed bit of CLAUDE.md is the
                            // import line; if it's missing we can fix that
                            // surgically without touching anything else.
                            // trace:BUG-1-065 | ai:claude
                            UpgradeAction::InsertClaudeImport
                        }
                        _ => UpgradeAction::LeaveAlone,
                    }
                }
                FileCategory::ManagedMerge => {
                    decide_managed_merge(&artifact.path, &on_disk_path, &artifact.content)
                }
            }
        };

        match action {
            UpgradeAction::Create => {
                if !dry_run {
                    if let Some(parent) = on_disk_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&on_disk_path, &artifact.content)?;
                }
                stats.created.push(artifact.path.clone());
            }
            UpgradeAction::Overwrite => {
                if !dry_run {
                    if let Some(parent) = on_disk_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&on_disk_path, &artifact.content)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::RewriteAidaBlock => {
                if !dry_run {
                    let actual = std::fs::read_to_string(&on_disk_path)?;
                    let merged = rewrite_aida_block(&actual, &artifact.content);
                    std::fs::write(&on_disk_path, merged)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::InsertClaudeImport => {
                // trace:BUG-1-065 | ai:claude
                if !dry_run {
                    let actual = std::fs::read_to_string(&on_disk_path)?;
                    let updated = aida_core::scaffolding::insert_claude_md_import(&actual);
                    std::fs::write(&on_disk_path, updated)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::SlotMerge { changes, merged } => {
                // trace:FR-1-047 | ai:claude
                if !dry_run {
                    let pretty = serde_json::to_string_pretty(&merged)?;
                    std::fs::write(&on_disk_path, pretty + "\n")?;
                }
                stats.upgraded.push(artifact.path.clone());
                // Surface the per-slot diff inline since it's the most
                // useful signal for a managed-merge upgrade.
                for ch in &changes {
                    let kind = match ch.kind {
                        aida_core::SlotChangeKind::Replaced => {
                            crate::glyph(crate::glyphs::Glyph::FlowQueued)
                                .cyan()
                                .to_string()
                        }
                        aida_core::SlotChangeKind::Added => "+".green().to_string(),
                    };
                    eprintln!(
                        "      {}   {}: {} {}",
                        " ".repeat(0),
                        artifact.path.display().to_string().dimmed(),
                        kind,
                        ch.slot
                    );
                }
            }
            UpgradeAction::LeaveAlone => {
                stats.left_alone.push(artifact.path.clone());
            }
            UpgradeAction::None => {
                stats.unchanged += 1;
            }
        }
    }

    // Render. One block per category, in the same order as the SPIKE
    // doc + the FileCategory enum (template → seed → managed-merge).
    let order = ["template", "seed", "managed-merge"];
    let mut total_changes = 0usize;
    for cat in order {
        let Some(stats) = by_cat.get(cat) else {
            continue;
        };
        let header = match cat {
            "template" => "Templates (AIDA-owned)".cyan().bold(),
            "seed" => "Seed (user-owned post-init)".yellow().bold(),
            "managed-merge" => "Managed-merge (slot-shared)".magenta().bold(),
            _ => cat.normal().bold(),
        };
        println!("\n{}", header);
        if !stats.created.is_empty() {
            println!("  {} {} created:", "+".green().bold(), stats.created.len());
            for p in &stats.created {
                println!("      + {}", p.display());
            }
            total_changes += stats.created.len();
        }
        if !stats.upgraded.is_empty() {
            let verb = if force { "overwritten" } else { "upgraded" };
            println!(
                "  {} {} {}:",
                crate::glyph(crate::glyphs::Glyph::FlowQueued).cyan().bold(),
                stats.upgraded.len(),
                verb
            );
            for p in &stats.upgraded {
                println!(
                    "      {} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowQueued),
                    p.display()
                );
            }
            total_changes += stats.upgraded.len();
        }
        if !stats.left_alone.is_empty() {
            let why = match cat {
                "seed" => "user-owned; drift expected. Edit by hand or `apply --force`",
                "managed-merge" => {
                    "slot-merge deferred (FR-1-028 v2). Edit by hand or `apply --force`"
                }
                _ => "left alone",
            };
            println!(
                "  {} {} drifted, left alone ({}):",
                "·".yellow(),
                stats.left_alone.len(),
                why
            );
            for p in &stats.left_alone {
                println!("      · {}", p.display());
            }
        }
        if !stats.symlinked.is_empty() {
            // BUG-718: these were skipped to protect a source-of-truth master.
            println!(
                "  {} {} skipped — symlink into another tree; writing would corrupt the target (NOT written):",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                stats.symlinked.len()
            );
            for (path, target) in &stats.symlinked {
                println!(
                    "      {} {} → {}",
                    "⤳".yellow(),
                    path.display(),
                    target.display()
                );
            }
        }
        if stats.unchanged > 0 {
            println!(
                "  {} {} matching (no action needed)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                stats.unchanged
            );
        }
    }

    let total_symlinked: usize = by_cat.values().map(|s| s.symlinked.len()).sum();

    println!();
    if dry_run {
        println!(
            "{} Dry run — {} file(s) would change. Re-run without --dry-run to apply.",
            "→".cyan().bold(),
            total_changes
        );
    } else if total_changes == 0 {
        println!(
            "{} Scaffold up to date — nothing to do.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
    } else {
        println!(
            "{} {} file(s) changed.",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            total_changes
        );
    }
    if total_symlinked > 0 {
        println!(
            "{} {} symlinked file(s) skipped to protect their targets (this is expected in the AIDA dev repo).",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            total_symlinked
        );
    }

    // BUG-719: surface (and with --prune, remove) obsolete `aida-*` files a
    // prior binary may have left behind or resurrected — matches `scaffold
    // apply`, so upgrade is also a complete drift-fixing path.
    report_and_prune_obe_scaffold(
        project_root,
        prune,
        dry_run,
        "aida scaffold upgrade --prune",
    );
    Ok(())
}

/// Walk the resolved artifact set, diffing each against its on-disk copy.
/// Returns true if any drift was emitted (so the caller can set exit code).
/// Files that are missing on disk are reported as a single header + note,
/// not as a full diff (the unified-diff format isn't useful when actual is
/// empty / nonexistent — `aida scaffold status` already covers that case).
// trace:FR-1-027 | ai:claude
fn print_scaffold_diffs(
    project_root: &std::path::Path,
    artifacts: &[&aida_core::scaffolding::ScaffoldArtifact],
    context_lines: usize,
    no_color: bool,
    list_only: bool,
) -> Result<bool> {
    use aida_core::DiffSlice;

    if no_color {
        colored::control::set_override(false);
    }

    let mut any_drift = false;
    let mut printed_count = 0;
    for artifact in artifacts {
        let full_path = project_root.join(&artifact.path);
        let actual_result = std::fs::read_to_string(&full_path);

        // Resolve drift state via the slice helper so CLAUDE.md (presence-
        // only) and AGENTS.md (AUTOGEN-block-only) get scoped properly.
        // Missing-file handling stays in this layer because the slice helper
        // doesn't see the filesystem. trace:FR-1-027 | ai:claude
        let slice = match &actual_result {
            Ok(actual) => {
                aida_core::aida_managed_diff_slice(&artifact.path, &artifact.content, actual)
            }
            Err(_) => {
                // Single-file mode: explicit user asked for this path → surface.
                // Bulk mode: only surface for known-required files (Template
                // category — AIDA owns those).
                let category = artifact.category();
                let surface =
                    artifacts.len() == 1 || matches!(category, aida_core::FileCategory::Template);
                if !surface {
                    continue;
                }
                if list_only {
                    println!("{}", artifact.path.display());
                } else {
                    println!(
                        "{}",
                        format!("# {} is missing on disk", artifact.path.display()).yellow()
                    );
                }
                any_drift = true;
                continue;
            }
        };

        match slice {
            DiffSlice::Match => continue,
            DiffSlice::MarkerMissing { message } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                } else {
                    if printed_count > 0 {
                        println!();
                    }
                    printed_count += 1;
                    println!(
                        "{}",
                        format!("# {}: {}", artifact.path.display(), message).yellow()
                    );
                }
            }
            DiffSlice::FullDiff { expected, actual } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                    continue;
                }
                if printed_count > 0 {
                    println!();
                }
                printed_count += 1;

                println!(
                    "{}",
                    format!("--- a/{}  (template)", artifact.path.display()).red()
                );
                println!(
                    "{}",
                    format!("+++ b/{}  (on disk)", artifact.path.display()).green()
                );
                render_unified_diff(&expected, &actual, context_lines);
            }
            DiffSlice::SliceDiff {
                expected,
                actual,
                note,
            } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                    continue;
                }
                if printed_count > 0 {
                    println!();
                }
                printed_count += 1;

                println!(
                    "{}",
                    format!("--- a/{}  (template)", artifact.path.display()).red()
                );
                println!(
                    "{}",
                    format!("+++ b/{}  (on disk)", artifact.path.display()).green()
                );
                println!("{}", format!("# {}", note).dimmed());
                render_unified_diff(&expected, &actual, context_lines);
            }
        }
    }

    if !any_drift && !list_only {
        eprintln!("{}", "No drift — scaffold matches on-disk files.".dimmed());
    }
    Ok(any_drift)
}

/// Render a unified diff of two strings to stdout with git-style coloring.
fn render_unified_diff(expected: &str, actual: &str, context_lines: usize) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .diff_lines(expected, actual);
    for hunk in diff
        .unified_diff()
        .context_radius(context_lines)
        .iter_hunks()
    {
        println!("{}", format!("{}", hunk.header()).cyan());
        for change in hunk.iter_changes() {
            let line = change.value();
            let line = line.strip_suffix('\n').unwrap_or(line);
            match change.tag() {
                ChangeTag::Delete => println!("{}", format!("-{}", line).red()),
                ChangeTag::Insert => println!("{}", format!("+{}", line).green()),
                ChangeTag::Equal => println!(" {}", line),
            }
        }
    }
}

// trace:FR-0315 | ai:claude:high
/// Generate HTML report for scaffold status with diffs
fn generate_scaffold_html_report(
    store: &RequirementsStore,
    root: &std::path::Path,
    config: &ScaffoldConfig,
    db_path: &std::path::Path,
    status: &aida_core::ScaffoldStatus,
) -> Result<String> {
    use std::fmt::Write;

    let mut scaffolder =
        Scaffolder::with_database(root.to_path_buf(), config.clone(), db_path.to_path_buf());
    let preview = scaffolder.preview(store);

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");

    let mut html = String::new();

    // HTML header with inline styles
    writeln!(
        html,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIDA Scaffold Status Report</title>
    <style>
        :root {{
            --bg-primary: #1a1a2e;
            --bg-secondary: #16213e;
            --bg-tertiary: #0f3460;
            --text-primary: #e4e4e7;
            --text-secondary: #a1a1aa;
            --accent-green: #22c55e;
            --accent-yellow: #eab308;
            --accent-red: #ef4444;
            --accent-blue: #3b82f6;
            --border-color: #27272a;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: 'Segoe UI', system-ui, -apple-system, sans-serif;
            background: var(--bg-primary);
            color: var(--text-primary);
            line-height: 1.6;
            padding: 2rem;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        header {{
            background: linear-gradient(135deg, var(--bg-secondary), var(--bg-tertiary));
            padding: 2rem;
            border-radius: 12px;
            margin-bottom: 2rem;
            border: 1px solid var(--border-color);
        }}
        h1 {{ color: var(--accent-blue); font-size: 1.75rem; margin-bottom: 0.5rem; }}
        .meta {{ color: var(--text-secondary); font-size: 0.875rem; }}
        .summary {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        .summary-card {{
            background: var(--bg-secondary);
            padding: 1.5rem;
            border-radius: 8px;
            border: 1px solid var(--border-color);
            text-align: center;
        }}
        .summary-card .count {{ font-size: 2.5rem; font-weight: bold; }}
        .summary-card .label {{ color: var(--text-secondary); font-size: 0.875rem; }}
        .count.green {{ color: var(--accent-green); }}
        .count.yellow {{ color: var(--accent-yellow); }}
        .count.red {{ color: var(--accent-red); }}
        .count.blue {{ color: var(--accent-blue); }}
        section {{
            background: var(--bg-secondary);
            border-radius: 12px;
            border: 1px solid var(--border-color);
            margin-bottom: 1.5rem;
            overflow: hidden;
        }}
        section h2 {{
            padding: 1rem 1.5rem;
            background: var(--bg-tertiary);
            font-size: 1.1rem;
            border-bottom: 1px solid var(--border-color);
        }}
        .file-list {{ list-style: none; }}
        .file-list li {{
            padding: 0.75rem 1.5rem;
            border-bottom: 1px solid var(--border-color);
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.875rem;
        }}
        .file-list li:last-child {{ border-bottom: none; }}
        .file-list .icon {{ margin-right: 0.5rem; }}
        details {{
            border-bottom: 1px solid var(--border-color);
        }}
        details:last-child {{ border-bottom: none; }}
        summary {{
            padding: 0.75rem 1.5rem;
            cursor: pointer;
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.875rem;
            background: var(--bg-secondary);
            transition: background 0.2s;
        }}
        summary:hover {{ background: var(--bg-tertiary); }}
        summary .icon {{ margin-right: 0.5rem; }}
        .diff {{
            padding: 1rem 1.5rem;
            background: #0d1117;
            overflow-x: auto;
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.8rem;
            line-height: 1.4;
        }}
        .diff-line {{ white-space: pre; }}
        .diff-line.add {{ color: #3fb950; background: rgba(46, 160, 67, 0.15); }}
        .diff-line.remove {{ color: #f85149; background: rgba(248, 81, 73, 0.15); }}
        .diff-line.context {{ color: #8b949e; }}
        .diff-line.header {{ color: #79c0ff; font-weight: bold; }}
        .status-ok {{ color: var(--accent-green); }}
        .status-warn {{ color: var(--accent-yellow); }}
        .status-error {{ color: var(--accent-red); }}
        .status-info {{ color: var(--accent-blue); }}
        .empty {{ padding: 2rem; text-align: center; color: var(--text-secondary); }}
    </style>
</head>
<body>
<div class="container">"#
    )?;

    // Header
    writeln!(
        html,
        r#"<header>
    <h1>📊 AIDA Scaffold Status Report</h1>
    <p class="meta">Project: {} {} Generated: {}</p>
</header>"#,
        root.display(),
        crate::glyph(crate::glyphs::Glyph::Bullet),
        timestamp
    )?;

    // Summary cards
    writeln!(
        html,
        r#"<div class="summary">
    <div class="summary-card">
        <div class="count green">{}</div>
        <div class="label">Matching</div>
    </div>
    <div class="summary-card">
        <div class="count yellow">{}</div>
        <div class="label">Modified</div>
    </div>
    <div class="summary-card">
        <div class="count red">{}</div>
        <div class="label">Missing</div>
    </div>
    <div class="summary-card">
        <div class="count blue">{}</div>
        <div class="label">Extra</div>
    </div>
</div>"#,
        status.matching.len(),
        status.modified.len(),
        status.missing.len(),
        status.extra.len()
    )?;

    // Overall status
    let overall_status = if status.is_current {
        format!(
            r#"<p class="status-ok">{} Scaffold is up to date</p>"#,
            crate::glyph(crate::glyphs::Glyph::Check)
        )
    } else {
        format!(
            r#"<p class="status-warn">{} Scaffold drift detected</p>"#,
            crate::glyph(crate::glyphs::Glyph::Warning)
        )
    };
    writeln!(
        html,
        r#"<section><h2>Status</h2><div style="padding: 1rem 1.5rem;">{}</div></section>"#,
        overall_status
    )?;

    // Matching files section
    if !status.matching.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-ok">{} Matching Files ({})</h2>
    <ul class="file-list">"#,
            crate::glyph(crate::glyphs::Glyph::Check),
            status.matching.len()
        )?;
        for path in &status.matching {
            writeln!(
                html,
                r#"        <li><span class="icon">{}</span>{}</li>"#,
                crate::glyph(crate::glyphs::Glyph::Check),
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Modified files section with diffs
    if !status.modified.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-warn">~ Modified Files ({})</h2>"#,
            status.modified.len()
        )?;

        for (path, file_status) in &status.modified {
            // Get expected content from scaffold preview
            let expected_content = preview
                .artifacts
                .iter()
                .find(|a| &a.path == path)
                .map(|a| a.content.as_str())
                .unwrap_or("");

            // Get actual content from disk
            let full_path = root.join(path);
            let actual_content = std::fs::read_to_string(&full_path).unwrap_or_default();

            // Generate diff
            let diff = generate_unified_diff(
                path.to_string_lossy().as_ref(),
                expected_content,
                &actual_content,
            );

            let status_info = match file_status {
                FileStatus::Modified {
                    expected_lines,
                    actual_lines,
                } => {
                    format!(
                        " (expected {} lines, found {})",
                        expected_lines, actual_lines
                    )
                }
                _ => String::new(),
            };

            writeln!(
                html,
                r#"    <details>
        <summary><span class="icon status-warn">~</span>{}{}</summary>
        <div class="diff">"#,
                path.display(),
                status_info
            )?;

            for line in diff.lines() {
                let class = if line.starts_with('+') && !line.starts_with("+++") {
                    "add"
                } else if line.starts_with('-') && !line.starts_with("---") {
                    "remove"
                } else if line.starts_with("@@")
                    || line.starts_with("---")
                    || line.starts_with("+++")
                {
                    "header"
                } else {
                    "context"
                };
                writeln!(
                    html,
                    r#"<div class="diff-line {}">{}</div>"#,
                    class,
                    html_escape(line)
                )?;
            }

            writeln!(html, "        </div>\n    </details>")?;
        }
        writeln!(html, "</section>")?;
    }

    // Missing files section
    if !status.missing.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-error">{} Missing Files ({})</h2>
    <ul class="file-list">"#,
            crate::glyph(crate::glyphs::Glyph::Cross),
            status.missing.len()
        )?;
        for path in &status.missing {
            writeln!(
                html,
                r#"        <li><span class="icon status-error">{}</span>{}</li>"#,
                crate::glyph(crate::glyphs::Glyph::Cross),
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Extra files section
    if !status.extra.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-info">+ Extra Files ({})</h2>
    <ul class="file-list">"#,
            status.extra.len()
        )?;
        for path in &status.extra {
            writeln!(
                html,
                r#"        <li><span class="icon status-info">+</span>{}</li>"#,
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Footer
    writeln!(
        html,
        r#"</div>
</body>
</html>"#
    )?;

    Ok(html)
}

/// Generate a unified diff between expected and actual content
fn generate_unified_diff(filename: &str, expected: &str, actual: &str) -> String {
    use std::fmt::Write;

    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    let mut diff = String::new();
    writeln!(diff, "--- expected/{}", filename).ok();
    writeln!(diff, "+++ actual/{}", filename).ok();

    // Simple line-by-line diff with context
    let max_len = expected_lines.len().max(actual_lines.len());
    let context_size = 3;
    let mut in_hunk = false;
    let mut hunk_start_expected = 0;
    let mut hunk_start_actual = 0;
    let mut hunk_lines: Vec<String> = Vec::new();
    let mut last_change = 0;

    for i in 0..max_len {
        let exp_line = expected_lines.get(i).copied();
        let act_line = actual_lines.get(i).copied();

        let is_same = exp_line == act_line;

        if !is_same {
            // Start a new hunk if needed
            if !in_hunk {
                in_hunk = true;
                hunk_start_expected = i.saturating_sub(context_size);
                hunk_start_actual = i.saturating_sub(context_size);
                // Add context before
                for j in hunk_start_expected..i {
                    if let Some(line) = expected_lines.get(j) {
                        hunk_lines.push(format!(" {}", line));
                    }
                }
            }
            last_change = i;

            // Add the diff lines
            if let Some(line) = exp_line {
                hunk_lines.push(format!("-{}", line));
            }
            if let Some(line) = act_line {
                hunk_lines.push(format!("+{}", line));
            }
        } else if in_hunk {
            // We have a matching line in a hunk
            if i <= last_change + context_size {
                // Still within context after
                if let Some(line) = exp_line {
                    hunk_lines.push(format!(" {}", line));
                }
            } else {
                // End the hunk
                let exp_count = hunk_lines.iter().filter(|l| !l.starts_with('+')).count();
                let act_count = hunk_lines.iter().filter(|l| !l.starts_with('-')).count();
                writeln!(
                    diff,
                    "@@ -{},{} +{},{} @@",
                    hunk_start_expected + 1,
                    exp_count,
                    hunk_start_actual + 1,
                    act_count
                )
                .ok();
                for line in &hunk_lines {
                    writeln!(diff, "{}", line).ok();
                }
                hunk_lines.clear();
                in_hunk = false;
            }
        }
    }

    // Flush any remaining hunk
    if !hunk_lines.is_empty() {
        let exp_count = hunk_lines.iter().filter(|l| !l.starts_with('+')).count();
        let act_count = hunk_lines.iter().filter(|l| !l.starts_with('-')).count();
        writeln!(
            diff,
            "@@ -{},{} +{},{} @@",
            hunk_start_expected + 1,
            exp_count,
            hunk_start_actual + 1,
            act_count
        )
        .ok();
        for line in &hunk_lines {
            writeln!(diff, "{}", line).ok();
        }
    }

    if diff.lines().count() <= 2 {
        // No actual differences found, show a note
        diff.push_str("@@ -1,1 +1,1 @@\n");
        diff.push_str(" (Files appear identical or differ only in whitespace)\n");
    }

    diff
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
