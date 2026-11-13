// trace:TASK-3428 | ai:codex
use anyhow::Result;
use colored::Colorize;
/// Handle requirement type commands
/// Handle `aida cache {rebuild,status}` against a CachedGitBackend.
/// trace:EPIC-1-001 | ai:claude
/// Handle `aida db block <subcommand>` — pre-allocated agreed ID blocks.
// trace:FR-2-005 | ai:claude
/// One-shot migration: collapse legacy origin spec_ids onto their agreed_ids.
/// For each requirement where spec_id ≠ agreed_id, set spec_id := agreed_id
/// and clear agreed_id (since the canonical id now lives in spec_id alone).
/// The on-disk YAML file moves from `objects/<TYPE>/000/<OLD>.yaml` to
/// `objects/<TYPE>/000/<NEW>.yaml`; that's handled implicitly by save()'s
/// "delete files that fell out of the store" pass.
///
/// Relationships use UUIDs internally so they're unaffected.
/// trace:FR-1-071 | ai:claude
pub(crate) fn handle_retire_legacy_ids(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    dry_run: bool,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    let store = backend.load()?;

    // Collect the rename plan first so we can preview before mutating.
    let mut renames: Vec<(String, String, uuid::Uuid)> = Vec::new();
    for req in &store.requirements {
        if let (Some(spec), Some(agreed)) = (req.spec_id.as_deref(), req.agreed_id.as_deref()) {
            if spec != agreed {
                renames.push((spec.to_string(), agreed.to_string(), req.id));
            }
        }
    }

    if renames.is_empty() {
        println!(
            "{} No legacy ids to retire — every requirement's spec_id already matches its agreed_id.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "Found {} requirement{} with diverging spec_id ↔ agreed_id:",
        renames.len(),
        if renames.len() == 1 { "" } else { "s" }
    );
    println!("{}", "─".repeat(60));
    let preview_limit = 20;
    for (spec, agreed, _) in renames.iter().take(preview_limit) {
        println!("  {}  →  {}", spec.dimmed(), agreed.bold());
    }
    if renames.len() > preview_limit {
        println!(
            "  {}",
            format!("… and {} more", renames.len() - preview_limit).dimmed()
        );
    }

    // trace:BUG-2017 | ai:codex — native node IDs are permanent lookup aliases.
    // Canonical display is already supplied by agreed_id; destructive collapse
    // has no compatible place to retain the old native alias in this schema.
    if !dry_run {
        aida_core::allocation_authority::require_global(store_path)?;
        anyhow::ensure!(renames.iter().all(|(old, _, _)| aida_core::git_ops::is_global_id_format(old)),
            "node-local IDs are permanent aliases; use HOME merge-gate for canonical display, without retiring old lookup IDs");
    }

    if dry_run {
        println!();
        println!("{} Dry run — no changes made.", "→".cyan());
        println!("    Re-run without --dry-run to apply.");
        return Ok(());
    }

    // update_atomically reloads canonical state and holds store-write.lock
    // through rename persistence and cache refresh. trace:TASK-1981 | ai:codex
    let mut retired_count = 0usize;
    // Admission below also rejects any concurrently introduced unsupported node rename.
    backend.update_atomically(|mutable| {
        for req in mutable.requirements.iter_mut() {
            let needs = match (req.spec_id.as_deref(), req.agreed_id.as_deref()) {
                (Some(s), Some(a)) if s != a && aida_core::git_ops::is_global_id_format(s) => {
                    Some(a.to_string())
                }
                _ => None,
            };
            if let Some(new_spec) = needs {
                req.spec_id = Some(new_spec);
                req.agreed_id = None;
                retired_count += 1;
            }
        }
    })?;

    println!();
    println!(
        "{} Retired {} legacy spec_id{}. Canonical id now lives in spec_id alone.",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        retired_count,
        if retired_count == 1 { "" } else { "s" }
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/task_1981_retire_tests.rs"]
mod task_1981_retire_tests;
