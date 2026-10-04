//! BUG-1671: the destructive sweeps used to re-check each candidate OUTSIDE
//! the store write lock and write it back inside. A reopen that landed in that
//! window was silently reverted (archive) or its live queue entry dropped
//! (queue-GC).
//!
//! These tests do not merely run a sweep — a sweep over a quiet store proves
//! nothing about an interleaving. Each one installs a hook at the single
//! instant between candidate selection and the write
//! (`crate::sweep_test_hook`) and performs the reopen there, from a second
//! backend handle, which is exactly the writer the window let in. Against the
//! unfixed sweeps every one of these fails: the reopen is overwritten, or the
//! live entry is removed.
//!
//! Every fixture is a throwaway git store inside a `TempDir`; nothing here
//! reads or writes a real store.
// trace:BUG-1671 | ai:claude
#![cfg(unix)]

use std::path::{Path, PathBuf};

use aida_core::{DatabaseBackend, QueueEntry, Requirement, RequirementStatus};
use tempfile::TempDir;

const USER: &str = "alice";

struct Fixture {
    tmp: TempDir,
    store_root: PathBuf,
    cache_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_root);
        assert!(
            cache_path.starts_with(tmp.path()),
            "fixture: the cache must live inside the temp dir: {}",
            cache_path.display()
        );
        Fixture {
            tmp,
            store_root,
            cache_path,
        }
    }

    fn backend(&self) -> aida_core::CachedGitBackend {
        aida_core::CachedGitBackend::open(&self.store_root, &self.cache_path).unwrap()
    }

    fn add(&self, spec_id: &str, status: RequirementStatus, modified_days_ago: i64) -> Requirement {
        let mut req = Requirement::new(format!("spec {spec_id}"), "body".into());
        req.spec_id = Some(spec_id.to_string());
        req.status = status;
        req.modified_at = chrono::Utc::now() - chrono::Duration::days(modified_days_ago);
        self.backend().add_requirement(req).unwrap()
    }

    fn enqueue(&self, req: &Requirement) {
        aida_core::Storage::new(&self.store_root)
            .queue_add(QueueEntry {
                user_id: USER.to_string(),
                requirement_id: req.id,
                position: 0,
                added_by: USER.to_string(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    }

    fn queued_ids(&self) -> Vec<uuid::Uuid> {
        aida_core::Storage::new(&self.store_root)
            .queue_list(USER, true)
            .unwrap()
            .into_iter()
            .map(|e| e.requirement_id)
            .collect()
    }

    /// Bring the cache to the store HEAD, so candidate selection sees a
    /// perfectly fresh projection: the window under test is the lock window,
    /// not a stale snapshot (that is BUG-1664's fixture).
    fn freshen_cache(&self) {
        self.backend()
            .list_summaries(&aida_core::ListFilter::default())
            .unwrap();
    }

    fn authoritative(&self, spec_id: &str) -> Requirement {
        aida_core::GitBackend::new(&self.store_root)
            .unwrap()
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .unwrap()
    }

    fn object_path(&self, spec_id: &str) -> PathBuf {
        aida_core::object_store::object_path(&self.store_root.join("objects"), spec_id).unwrap()
    }

    fn project_root(&self) -> &Path {
        self.tmp.path()
    }

    /// Install the reopen that lands in the window: it runs on the sweep's own
    /// thread, from a second backend handle, at the moment the sweep has
    /// finished selecting and has not yet taken the store write lock.
    fn reopen_between_select_and_write(
        &self,
        spec_id: &'static str,
        status: RequirementStatus,
    ) -> crate::sweep_test_hook::Installed {
        let store_root = self.store_root.clone();
        crate::sweep_test_hook::install(
            &self.store_root,
            Box::new(move || {
                let external = aida_core::GitBackend::new(&store_root).unwrap();
                let mut req = external
                    .get_requirement_by_spec_id(spec_id)
                    .unwrap()
                    .unwrap();
                req.status = status.clone();
                external.update_requirement(&req).unwrap();
            }),
        )
    }
}

/// Two Completed specs, both genuinely 90 days old, with a fresh cache. BUG-1
/// is the one a concurrent writer reopens mid-sweep; BUG-2 must still be
/// archived, so a passing test cannot pass by doing nothing.
fn archive_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed, 90);
    fx.add("BUG-2", RequirementStatus::Completed, 90);
    fx.freshen_cache();
    fx
}

fn assert_reopen_survived_and_the_other_spec_archived(fx: &Fixture) {
    let reopened = fx.authoritative("BUG-1");
    assert_eq!(
        reopened.status,
        RequirementStatus::InProgress,
        "the reopen that landed between the re-check and the write must survive"
    );
    assert!(
        !reopened.archived,
        "a spec reopened during the sweep must not be archived"
    );
    assert!(
        fx.authoritative("BUG-2").archived,
        "the candidate nobody touched is still archived"
    );
}

// trace:BUG-1671 | ai:claude
#[test]
fn archive_older_than_keeps_a_spec_reopened_between_the_recheck_and_the_write() {
    let fx = archive_fixture();
    let _hook = fx.reopen_between_select_and_write("BUG-1", RequirementStatus::InProgress);

    crate::archive_cmd::handle_archive_command(
        None,
        Some("30d"),
        None,
        /* dry_run */ false,
        /* force */ false,
        /* verbose */ false,
        &fx.backend(),
        &fx.store_root,
    )
    .unwrap();

    assert_reopen_survived_and_the_other_spec_archived(&fx);
}

// trace:BUG-1671 | ai:claude
#[test]
fn post_pull_auto_archive_keeps_a_spec_reopened_between_the_recheck_and_the_write() {
    let fx = archive_fixture();
    std::fs::write(
        fx.project_root().join(".aida").join("config.toml"),
        "[archive]\nauto_after_days = 30\n",
    )
    .unwrap();
    let _hook = fx.reopen_between_select_and_write("BUG-1", RequirementStatus::InProgress);

    crate::maybe_auto_archive_sweep(fx.project_root(), &fx.backend(), /* quiet */ true);

    assert_reopen_survived_and_the_other_spec_archived(&fx);
}

/// BUG-1671 acceptance 3: a candidate whose object cannot be read is skipped
/// and counted, and the rest of the sweep still runs. The damage is introduced
/// in the same window, so the pre-lock pass read the spec fine and only the
/// in-lock read fails.
// trace:BUG-1671 | ai:claude
#[test]
fn archive_older_than_skips_a_spec_whose_object_cannot_be_read() {
    let fx = archive_fixture();
    let corrupt_path = fx.object_path("BUG-1");
    const GARBAGE: &str = ": not yaml at all\n\t- [\n";
    let _hook = {
        let path = corrupt_path.clone();
        crate::sweep_test_hook::install(
            &fx.store_root,
            Box::new(move || std::fs::write(&path, GARBAGE).unwrap()),
        )
    };

    crate::archive_cmd::handle_archive_command(
        None,
        Some("30d"),
        None,
        false,
        false,
        false,
        &fx.backend(),
        &fx.store_root,
    )
    .expect("one unreadable spec must not abort the sweep");

    assert_eq!(
        std::fs::read_to_string(&corrupt_path).unwrap(),
        GARBAGE,
        "an unreadable object must be left alone, not rewritten from the stale copy"
    );
    assert!(
        fx.authoritative("BUG-2").archived,
        "the readable candidate is still archived"
    );
}

/// Two Completed specs, both queued, with a fresh cache. BUG-1 is reopened
/// mid-sweep and must keep its queue entry; BUG-2's entry must still go.
fn queue_fixture() -> (Fixture, Requirement, Requirement) {
    let fx = Fixture::new();
    let reopened = fx.add("BUG-1", RequirementStatus::Completed, 0);
    let dead = fx.add("BUG-2", RequirementStatus::Completed, 0);
    fx.enqueue(&reopened);
    fx.enqueue(&dead);
    fx.freshen_cache();
    (fx, reopened, dead)
}

fn assert_reopened_entry_kept_and_dead_entry_removed(
    fx: &Fixture,
    reopened: &Requirement,
    dead: &Requirement,
) {
    assert_eq!(
        fx.authoritative("BUG-1").status,
        RequirementStatus::InProgress,
        "fixture: the reopen must have landed"
    );
    let queued = fx.queued_ids();
    assert!(
        queued.contains(&reopened.id),
        "the entry of a spec reopened before the removal must survive: {queued:?}"
    );
    assert!(
        !queued.contains(&dead.id),
        "a truly dead entry is still collected: {queued:?}"
    );
}

// trace:BUG-1671 | ai:claude
#[test]
fn queue_list_self_heal_keeps_an_entry_reopened_between_the_recheck_and_the_removal() {
    let (fx, reopened, dead) = queue_fixture();
    let storage = aida_core::Storage::new(&fx.store_root);
    let _hook = fx.reopen_between_select_and_write("BUG-1", RequirementStatus::InProgress);

    let pruned = crate::queue_cmd::opportunistic_queue_gc(&storage, &fx.store_root, USER);

    assert_eq!(pruned, 1, "only the still-dead entry is pruned");
    assert_reopened_entry_kept_and_dead_entry_removed(&fx, &reopened, &dead);
}

// trace:BUG-1671 | ai:claude
#[test]
fn queue_gc_keeps_an_entry_reopened_between_the_recheck_and_the_removal() {
    let (fx, reopened, dead) = queue_fixture();
    let storage = aida_core::Storage::new(&fx.store_root);
    let _hook = fx.reopen_between_select_and_write("BUG-1", RequirementStatus::InProgress);

    crate::queue_cmd::handle_queue_command(
        &crate::cli::QueueCommand::Gc {
            user: Some(USER.to_string()),
            r#for: None,
            dry_run: false,
        },
        &storage,
        &fx.store_root,
    )
    .unwrap();

    assert_reopened_entry_kept_and_dead_entry_removed(&fx, &reopened, &dead);
}
