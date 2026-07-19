//! `aida sandbox` command cluster (SPIKE-48) — the throwaway-store playground:
//! create / reset / destroy / path a per-user sandbox `GitBackend` store so a
//! user can exercise `aida list` / `queue work` / `graph` against curated
//! scenario specs without touching their real project.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78; pure movement, no behavior
//! change). Note: the OS-level `bwrap`/firecracker sandbox detection
//! (`bwrap_status_line`) is a *separate* concern that stays in `main.rs` — it is
//! shared by `aida doctor` and `aida init`, and is unrelated to this
//! sandbox-store playground. This module reaches shared helpers (e.g.
//! `aida_store_override_from`) via `crate::`.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::*;

/// Default sandbox store location: a stable per-user dir under the system temp
/// directory, so repeated `aida sandbox create` / `path` point at the same
/// playground without the user tracking a path. Per-user so two accounts on one
/// box don't collide.
// trace:SPIKE-48 | ai:claude
fn default_sandbox_path() -> std::path::PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir().join(format!("aida-sandbox-{user}"))
}

fn sandbox_path_or_default(path: Option<&std::path::Path>) -> std::path::PathBuf {
    path.map(|p| p.to_path_buf())
        .unwrap_or_else(default_sandbox_path)
}

pub(crate) fn handle_sandbox_command(cmd: &cli::SandboxCommand) -> Result<()> {
    match cmd {
        cli::SandboxCommand::Create { path, seed, force } => {
            sandbox_create(sandbox_path_or_default(path.as_deref()), *seed, *force)
        }
        cli::SandboxCommand::Reset { path, seed } => {
            sandbox_reset(sandbox_path_or_default(path.as_deref()), *seed)
        }
        cli::SandboxCommand::Destroy { path } => {
            sandbox_destroy(sandbox_path_or_default(path.as_deref()))
        }
        cli::SandboxCommand::Path { path, export } => {
            sandbox_path(sandbox_path_or_default(path.as_deref()), *export)
        }
    }
}

/// Print the shell line that retargets `aida` at the sandbox. Shown by
/// `create` / `reset` / `path --export` so the user can copy-paste-activate.
fn sandbox_export_line(store: &std::path::Path) -> String {
    format!("export AIDA_STORE={}", store.display())
}

/// Is `dir` already a usable sandbox store (a git repo holding `objects/`)?
fn sandbox_is_populated(dir: &std::path::Path) -> bool {
    aida_core::git_ops::is_git_repo(dir) && dir.join("objects").is_dir()
}

/// Create (or no-op on) the sandbox store at `store`. Initializes a git repo +
/// `GitBackend` (which lays down `objects/` and `metadata.yaml`), optionally
/// seeds curated scenario specs, and prints the activation export line.
// trace:SPIKE-48 | ai:claude
fn sandbox_create(store: std::path::PathBuf, seed: bool, force: bool) -> Result<()> {
    use aida_core::git_ops;

    if sandbox_is_populated(&store) && !force {
        println!(
            "{} sandbox store already exists at {}",
            "Note:".dimmed(),
            store.display().to_string().white().bold()
        );
        println!("  Use it:        {}", sandbox_export_line(&store).cyan());
        println!("  Re-seed:       {}", "aida sandbox reset --seed".cyan());
        println!("  Recreate:      {}", "aida sandbox create --force".cyan());
        return Ok(());
    }

    if force && store.exists() {
        std::fs::remove_dir_all(&store)
            .with_context(|| format!("failed to remove existing sandbox at {}", store.display()))?;
    }

    std::fs::create_dir_all(&store)
        .with_context(|| format!("failed to create sandbox dir {}", store.display()))?;

    if !git_ops::is_git_repo(&store) {
        git_ops::init(&store)?;
    }
    // A throwaway store still needs an identity for commits to succeed.
    let git_name =
        git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA Sandbox".to_string());
    let git_email =
        git_ops::git_config_get("user.email").unwrap_or_else(|_| "sandbox@localhost".to_string());
    git_ops::configure_user(&store, &git_name, &git_email)?;

    // Initialize the backend (creates objects/ + metadata.yaml) and seed META
    // prompts so the playground behaves like a real project.
    let backend = aida_core::GitBackend::new(&store)?;
    let mut rs = aida_core::models::RequirementsStore::new();
    rs.name = "AIDA Sandbox".to_string();
    rs.title = "AIDA Sandbox".to_string();
    aida_core::meta::seed_meta_requirements(&mut rs)?;
    backend.save(&rs)?;

    std::fs::write(store.join("objects/.gitkeep"), "")?;
    git_ops::add_all(&store, "objects")?;
    if store.join("metadata.yaml").exists() {
        git_ops::add(&store, &["metadata.yaml"])?;
    }
    let _ = git_ops::commit(&store, "chore: initialize AIDA sandbox store");

    println!(
        "{} sandbox store at {}",
        "Created".green(),
        store.display().to_string().white().bold()
    );

    if seed {
        sandbox_seed(&store)?;
    }

    println!();
    println!("Activate it in this shell:");
    println!("  {}", sandbox_export_line(&store).cyan());
    println!(
        "Then `aida list` / `aida queue work` operate on the sandbox. {} when done.",
        "aida sandbox destroy".cyan()
    );
    Ok(())
}

/// Wipe the sandbox's contents and re-create it empty (or `--seed`-ed). The
/// directory path is reused so an exported `AIDA_STORE` stays valid.
// trace:SPIKE-48 | ai:claude
fn sandbox_reset(store: std::path::PathBuf, seed: bool) -> Result<()> {
    if store.exists() {
        std::fs::remove_dir_all(&store)
            .with_context(|| format!("failed to reset sandbox at {}", store.display()))?;
    }
    sandbox_create(store, seed, true)
}

/// Delete the sandbox store directory entirely. Idempotent.
// trace:SPIKE-48 | ai:claude
fn sandbox_destroy(store: std::path::PathBuf) -> Result<()> {
    if store.exists() {
        std::fs::remove_dir_all(&store)
            .with_context(|| format!("failed to destroy sandbox at {}", store.display()))?;
        println!(
            "{} sandbox store at {}",
            "Removed".green(),
            store.display().to_string().white().bold()
        );
        println!("  Unset the override: {}", "unset AIDA_STORE".cyan());
    } else {
        println!(
            "{} no sandbox store at {}",
            "Note:".dimmed(),
            store.display()
        );
    }
    Ok(())
}

/// Print the sandbox path (and existence), or the `export AIDA_STORE=...` line
/// with `--export`.
// trace:SPIKE-48 | ai:claude
fn sandbox_path(store: std::path::PathBuf, export: bool) -> Result<()> {
    if export {
        println!("{}", sandbox_export_line(&store));
        return Ok(());
    }
    let state = if sandbox_is_populated(&store) {
        "exists".green()
    } else {
        "not created".dimmed()
    };
    println!("{} ({})", store.display(), state);
    Ok(())
}

/// Seed a small, deterministic set of curated scenario specs into the sandbox:
/// a short lifecycle walk plus a blocked-by chain (so `aida graph --blocked-by`
/// and a drain have something to chew on). Deterministic + offline by design —
/// AI-generated scenarios are a deferred nicety.
// trace:SPIKE-48 | ai:claude
fn sandbox_seed(store: &std::path::Path) -> Result<()> {
    use aida_core::models::{RelationshipType, Requirement, RequirementStatus, RequirementType};

    let backend = aida_core::GitBackend::new(store)?;

    // Build the scenario specs in memory first so we can wire a blocked-by edge
    // by UUID (each Requirement gets its id at construction).
    let mut lifecycle = Requirement::new(
        "Sandbox: a lifecycle walk".to_string(),
        "Approved task to walk Draft -> Approved -> Planned -> In Progress -> Done. \
         Edit its status with `aida edit <id> --status ...` to feel the state machine."
            .to_string(),
    );
    lifecycle.req_type = RequirementType::Task;
    lifecycle.status = RequirementStatus::Approved;
    lifecycle.tags.insert("sandbox".to_string());

    let mut blocker = Requirement::new(
        "Sandbox: the blocker".to_string(),
        "This task blocks the dependent below. Try `aida graph <dependent-id> --blocked-by`."
            .to_string(),
    );
    blocker.req_type = RequirementType::Task;
    blocker.status = RequirementStatus::Approved;
    blocker.tags.insert("sandbox".to_string());

    let mut dependent = Requirement::new(
        "Sandbox: the dependent".to_string(),
        "Blocked by 'the blocker' — it won't be pickable until the blocker is Done.".to_string(),
    );
    dependent.req_type = RequirementType::Task;
    dependent.status = RequirementStatus::Approved;
    dependent.tags.insert("sandbox".to_string());
    dependent
        .relationships
        .push(aida_core::models::Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id: blocker.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some("sandbox".to_string()),
        });
    // Inverse edge on the blocker so the graph reads cleanly both ways.
    blocker.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::Blocks,
        target_id: dependent.id,
        created_at: Some(chrono::Utc::now()),
        created_by: Some("sandbox".to_string()),
    });

    let mut writer = backend.bulk_writer()?;
    writer.add(lifecycle)?;
    writer.add(blocker)?;
    writer.add(dependent)?;
    let count = writer.finish("chore: seed sandbox scenario specs")?;

    println!("  {} {} scenario specs", "Seeded".green(), count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// SPIKE-48: `sandbox_is_populated` is true only for a git repo holding
    /// `objects/`; the seed/scenario round-trip produces an override-acceptable
    /// store.
    // trace:SPIKE-48 | ai:claude
    #[test]
    fn sandbox_create_and_seed_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("sb");

        // Fresh dir is not yet a populated sandbox.
        assert!(!sandbox_is_populated(&store));

        sandbox_create(store.clone(), true, false).expect("create+seed");

        // Now it is a populated, override-acceptable store.
        assert!(sandbox_is_populated(&store));
        assert!(crate::aida_store_override_from(&store).is_some());
        assert!(store.join("objects").is_dir());

        // Destroy removes it; override no longer resolves.
        sandbox_destroy(store.clone()).expect("destroy");
        assert!(!store.exists());
        assert!(crate::aida_store_override_from(&store).is_none());
    }
}
