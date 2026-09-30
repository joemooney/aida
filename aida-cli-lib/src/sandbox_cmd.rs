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

/// How many passes a sandbox removal makes before it gives up on ENOTEMPTY.
// trace:BUG-1695 | ai:claude
const SANDBOX_REMOVE_ATTEMPTS: u32 = 3;

/// How long to pause between those passes.
// trace:BUG-1695 | ai:claude
const SANDBOX_REMOVE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

/// How many surviving paths a removal diagnostic names. Enough to identify a
/// writer, few enough that the line stays readable.
// trace:BUG-1695 | ai:claude
const SANDBOX_REMOVE_REPORT_ENTRIES: usize = 10;

/// Recursively remove `dir`, retrying ONLY on `ENOTEMPTY`, and naming what
/// survived when it finally gives up.
///
/// BUG-1695: `aida sandbox destroy` failed a REQUIRED CI check with
/// `Directory not empty (os error 39)`. The bug was filed on the theory that
/// something was still writing under the store at teardown. That theory is
/// wrong, and the measurements are recorded here so a later reader neither
/// "simplifies" this retry away nor pays for the same investigation twice:
///
/// - Nothing outlives create+seed. Every git call in `aida_core::git_ops`
///   goes through `Command::output()`, which waits for the child. Snapshotting
///   the whole store (path + inode + mtime ns) the instant `sandbox_create`
///   returned and again 250ms later, under eight-thread contention, measured
///   zero drift across 80 stores.
/// - Nothing writes into an idle store. A built store parked under the shared
///   temp root for 900s while the full `aida-cli-lib` suite ran beside it
///   recorded no mutation at all.
/// - Nothing reappears in the race window. A teardown that empties a directory
///   level, pauses 30ms, then looks again before the `rmdir` -- run back to
///   back for 900s against that same suite -- never once saw an entry return.
///
/// So there is no writer to close; a later pass finds the tree empty. That is
/// why the remedy is a retry rather than a leak fix. It is deliberately narrow
/// in two ways. It retries only `ENOTEMPTY`, so an unrelated failure (a
/// permission error, a busy mount) still fails on the first pass instead of
/// being swallowed three times over. And because a bare retry WOULD hide a
/// genuine leak if one ever did appear, it is never silent: a retry that
/// eventually SUCCEEDS still reports the pass count and what had blocked the
/// failing pass, and a retry that gives up names the entries that survived.
/// Those two lists answer different questions -- "what blocked the removal"
/// versus "what is left now" -- and the difference is the tell between this
/// benign race and a real leak. Without the success report the common case
/// would pass silently, which is precisely the masking this bug's acceptance
/// rules out.
///
/// `remove`, `backoff`, and `report` are injected so the retry, its bound, its
/// diagnostics, and its error text are testable from fixtures, with no sleeping
/// and no polling.
// trace:BUG-1695 | ai:claude
fn remove_tree_retrying_dir_not_empty(
    dir: &std::path::Path,
    attempts: u32,
    backoff: std::time::Duration,
    mut remove: impl FnMut(&std::path::Path) -> std::io::Result<()>,
    mut report: impl FnMut(String),
) -> std::io::Result<()> {
    let attempts = attempts.max(1);
    let mut last: Option<std::io::Error> = None;
    let mut blocked_by: Option<Vec<String>> = None;
    for attempt in 0..attempts {
        match remove(dir) {
            Ok(()) => {
                // A silent successful retry is the masking BUG-1695's
                // acceptance forbids, so say what the failing pass hit.
                if let Some(blocked) = blocked_by {
                    let named = if blocked.is_empty() {
                        "an entry it could no longer see".to_string()
                    } else {
                        blocked.join(", ")
                    };
                    report(format!(
                        "sandbox removal at {} needed {} of {attempts} passes -- \
                         the failed pass was blocked by: {named}",
                        dir.display(),
                        attempt + 1,
                    ));
                }
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                // Snapshot at the FIRST failure: that is what actually blocked
                // the removal, which a later pass would no longer show.
                if blocked_by.is_none() {
                    blocked_by = Some(surviving_entries(dir, SANDBOX_REMOVE_REPORT_ENTRIES));
                }
                last = Some(e);
                if attempt + 1 < attempts && !backoff.is_zero() {
                    std::thread::sleep(backoff);
                }
            }
            Err(e) => return Err(e),
        }
    }
    let survivors = surviving_entries(dir, SANDBOX_REMOVE_REPORT_ENTRIES);
    let detail = if survivors.is_empty() {
        "nothing is left under it now".to_string()
    } else {
        format!("still present: {}", survivors.join(", "))
    };
    let last = last.expect("the loop only falls through after a DirectoryNotEmpty");
    Err(std::io::Error::other(format!(
        "{last} after {attempts} attempts -- {detail}"
    )))
}

/// Up to `limit` paths still under `dir`, for the give-up diagnostic. Relative
/// to `dir` so the message stays readable, and best-effort: a directory we
/// cannot read contributes nothing rather than masking the real error.
// trace:BUG-1695 | ai:claude
fn surviving_entries(dir: &std::path::Path, limit: usize) -> Vec<String> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>, limit: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= limit {
                return;
            }
            let path = entry.path();
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
            if path.is_dir() && !path.is_symlink() {
                walk(&path, root, out, limit);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out, limit);
    out
}

/// [`remove_tree_retrying_dir_not_empty`] with this module's production
/// settings: the real recursive remove, three passes, a 50ms backoff.
// trace:BUG-1695 | ai:claude
fn remove_sandbox_tree(dir: &std::path::Path) -> std::io::Result<()> {
    remove_tree_retrying_dir_not_empty(
        dir,
        SANDBOX_REMOVE_ATTEMPTS,
        SANDBOX_REMOVE_BACKOFF,
        |p| std::fs::remove_dir_all(p),
        |msg| eprintln!("{} {msg}", "warning:".yellow().bold()),
    )
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
        remove_sandbox_tree(&store)
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
        remove_sandbox_tree(&store)
            .with_context(|| format!("failed to reset sandbox at {}", store.display()))?;
    }
    sandbox_create(store, seed, true)
}

/// Delete the sandbox store directory entirely. Idempotent.
// trace:SPIKE-48 | ai:claude
fn sandbox_destroy(store: std::path::PathBuf) -> Result<()> {
    if store.exists() {
        remove_sandbox_tree(&store)
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
/// a short lifecycle walk plus a blocked-by chain (so `aida graph blocked-by`
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
        "This task blocks the dependent below. Try `aida graph blocked-by <dependent-id>`."
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

    /// BUG-1695: the retry exists for the ENOTEMPTY teardown race, so a pass
    /// that fails with it must be followed by another pass -- and the moment
    /// one succeeds, no further pass is made.
    // trace:BUG-1695 | ai:claude
    #[test]
    fn remove_tree_retries_dir_not_empty_and_stops_at_the_first_success() {
        let tmp = TempDir::new().unwrap();
        let mut calls = 0u32;
        let result = remove_tree_retrying_dir_not_empty(
            tmp.path(),
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| {
                calls += 1;
                if calls == 1 {
                    Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty))
                } else {
                    Ok(())
                }
            },
            |_| {},
        );

        assert!(result.is_ok(), "the second pass succeeded: {result:?}");
        assert_eq!(
            calls, 2,
            "one retry after the ENOTEMPTY, then it stops -- not {calls} passes"
        );
    }

    /// BUG-1695: the retry is scoped to ENOTEMPTY on purpose. Any other
    /// failure -- a permission error, a busy mount -- must surface on the
    /// first pass rather than be attempted three times and reported late.
    // trace:BUG-1695 | ai:claude
    #[test]
    fn remove_tree_does_not_retry_an_unrelated_error() {
        let tmp = TempDir::new().unwrap();
        let mut calls = 0u32;
        let result = remove_tree_retrying_dir_not_empty(
            tmp.path(),
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| {
                calls += 1;
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            },
            |_| {},
        );

        let err = result.expect_err("a permission error is not retried away");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::PermissionDenied,
            "the caller still sees the real error kind, not a wrapped one"
        );
        assert_eq!(calls, 1, "it gave up immediately, not after {calls} passes");
    }

    /// BUG-1695: a bare retry would hide a genuine leak, so the give-up error
    /// names what survived. Pins the bound as well: exactly `attempts` passes,
    /// never an unbounded loop.
    // trace:BUG-1695 | ai:claude
    #[test]
    fn remove_tree_is_bounded_and_names_what_survived() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("sb");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("objects/leftover.yaml"), "x").unwrap();

        let mut calls = 0u32;
        let result = remove_tree_retrying_dir_not_empty(
            &store,
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| {
                calls += 1;
                Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty))
            },
            |_| {},
        );

        let err = result.expect_err("an ENOTEMPTY that never clears must fail");
        assert_eq!(
            calls, SANDBOX_REMOVE_ATTEMPTS,
            "bounded at {SANDBOX_REMOVE_ATTEMPTS} passes, not {calls}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("still present:"),
            "the give-up error must diagnose itself, got: {msg}"
        );
        assert!(
            msg.contains("objects/leftover.yaml"),
            "it must name the entry that survived, got: {msg}"
        );
    }

    /// BUG-1695: and when nothing survived, the diagnostic says so rather than
    /// printing an empty list -- that difference is the tell between "a writer
    /// is leaking" and "the removal itself needed another pass".
    // trace:BUG-1695 | ai:claude
    #[test]
    fn remove_tree_says_so_when_the_tree_is_already_gone() {
        let tmp = TempDir::new().unwrap();
        let absent = tmp.path().join("gone");
        let result = remove_tree_retrying_dir_not_empty(
            &absent,
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty)),
            |_| {},
        );

        let msg = result
            .expect_err("still an error -- the remove never succeeded")
            .to_string();
        assert!(msg.contains("nothing is left under it now"), "got: {msg}");
    }
    /// BUG-1695 acceptance: "a bare retry loop ... would equally mask a genuine
    /// leak". A retry that GIVES UP already diagnoses itself, but the common
    /// case is a retry that succeeds -- and if that were silent, a real leak
    /// that happened to clear within three passes would leave no trace at all.
    /// So a successful retry reports the pass count and names what blocked the
    /// pass that failed.
    // trace:BUG-1695 | ai:claude
    #[test]
    fn a_successful_retry_is_not_silent_and_names_what_blocked_it() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("sb");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("objects/leftover.yaml"), "x").unwrap();

        let mut calls = 0u32;
        let mut reports: Vec<String> = Vec::new();
        let result = remove_tree_retrying_dir_not_empty(
            &store,
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| {
                calls += 1;
                if calls == 1 {
                    Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty))
                } else {
                    Ok(())
                }
            },
            |msg| reports.push(msg),
        );

        assert!(result.is_ok(), "the second pass succeeded: {result:?}");
        assert_eq!(
            reports.len(),
            1,
            "exactly one diagnostic for the one race, got {reports:?}"
        );
        let msg = &reports[0];
        assert!(
            msg.contains("needed 2 of 3 passes"),
            "it must state how many passes the removal took, got: {msg}"
        );
        assert!(
            msg.contains("objects/leftover.yaml"),
            "it must name what blocked the failed pass, got: {msg}"
        );
    }

    /// BUG-1695: the flip side -- a removal that succeeds on its FIRST pass is
    /// the overwhelmingly common path, and it must stay silent. A warning on
    /// every `sandbox destroy` would train readers to ignore the one that
    /// matters.
    // trace:BUG-1695 | ai:claude
    #[test]
    fn a_first_pass_success_reports_nothing() {
        let tmp = TempDir::new().unwrap();
        let mut reports: Vec<String> = Vec::new();
        let result = remove_tree_retrying_dir_not_empty(
            tmp.path(),
            SANDBOX_REMOVE_ATTEMPTS,
            std::time::Duration::ZERO,
            |_| Ok(()),
            |msg| reports.push(msg),
        );

        assert!(result.is_ok());
        assert!(
            reports.is_empty(),
            "no race, so nothing to report -- got {reports:?}"
        );
    }
}
