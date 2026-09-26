//! BUG-1664: the destructive sweeps select their candidates from the cache
//! projection, which a read may serve from a stale snapshot (a live foreign
//! cache writer makes the read skip its catch-up). Each sweep must re-validate
//! every candidate against the authoritative object before it deletes a queue
//! entry or archives a spec.
//!
//! Every fixture is a temporary git-canonical store. The stale snapshot is
//! produced the way it happens in the field: the cache is current, another
//! writer commits straight to the store, and a live foreign writer (pid 1)
//! holds the cache lock sidecar, so the read path serves the old rows.
// trace:BUG-1664 | ai:claude
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

    /// Bring the cache to the store HEAD, so later external commits leave it
    /// behind.
    fn freshen_cache(&self) {
        self.backend()
            .list_summaries(&aida_core::ListFilter::default())
            .unwrap();
    }

    /// Another writer commits straight to the store, bypassing this cache.
    fn external_edit(&self, spec_id: &str, edit: impl FnOnce(&mut Requirement)) {
        let external = aida_core::GitBackend::new(&self.store_root).unwrap();
        let mut req = external
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .unwrap();
        edit(&mut req);
        external.update_requirement(&req).unwrap();
    }

    /// A live foreign process (pid 1) holds the cache write lock, so reads
    /// serve the last committed snapshot instead of catching up.
    fn hold_foreign_writer(&self) {
        let info = aida_core::CacheLockInfo {
            pid: 1,
            command: "test-foreign-writer".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "test".to_string(),
            ..Default::default()
        };
        let sidecar = PathBuf::from(format!("{}.lock-info", self.cache_path.display()));
        std::fs::write(sidecar, serde_json::to_string(&info).unwrap()).unwrap();
    }

    /// The status the (stale) cache serves for `spec_id`.
    fn cached_status(&self, spec_id: &str) -> String {
        self.backend()
            .list_summaries(&aida_core::ListFilter {
                archive: aida_core::ArchiveFilter::Both,
                defer: aida_core::DeferFilter::Both,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .find(|s| s.spec_id.as_deref() == Some(spec_id))
            .map(|s| s.status)
            .unwrap()
    }

    fn authoritative(&self, spec_id: &str) -> Requirement {
        aida_core::GitBackend::new(&self.store_root)
            .unwrap()
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .unwrap()
    }

    fn project_root(&self) -> &Path {
        self.tmp.path()
    }
}

/// A Completed spec reopened by another writer, plus a spec that really is
/// dead, both queued; then the cache goes stale.
fn queue_fixture() -> (Fixture, Requirement, Requirement) {
    let fx = Fixture::new();
    let reopened = fx.add("BUG-1", RequirementStatus::Completed, 0);
    let dead = fx.add("BUG-2", RequirementStatus::Completed, 0);
    fx.enqueue(&reopened);
    fx.enqueue(&dead);
    fx.freshen_cache();
    fx.external_edit("BUG-1", |r| r.status = RequirementStatus::Approved);
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("BUG-1"),
        "Completed",
        "fixture: the cache must serve the stale pre-reopen status"
    );
    (fx, reopened, dead)
}

// trace:BUG-1664 | ai:claude
#[test]
fn queue_list_self_heal_keeps_an_entry_reopened_behind_a_stale_snapshot() {
    let (fx, reopened, dead) = queue_fixture();
    let storage = aida_core::Storage::new(&fx.store_root);

    let pruned = crate::queue_cmd::opportunistic_queue_gc(&storage, &fx.store_root, USER);

    let queued = fx.queued_ids();
    assert!(
        queued.contains(&reopened.id),
        "the reopened spec's live entry must survive: {queued:?}"
    );
    assert!(
        !queued.contains(&dead.id),
        "a truly dead entry is still collected: {queued:?}"
    );
    assert_eq!(pruned, 1);
}

// trace:BUG-1664 | ai:claude
#[test]
fn queue_gc_keeps_an_entry_reopened_behind_a_stale_snapshot() {
    let (fx, reopened, dead) = queue_fixture();
    let storage = aida_core::Storage::new(&fx.store_root);

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

    let queued = fx.queued_ids();
    assert!(
        queued.contains(&reopened.id),
        "the reopened spec's live entry must survive: {queued:?}"
    );
    assert!(
        !queued.contains(&dead.id),
        "a truly dead entry is still collected: {queued:?}"
    );
}

/// Three Completed specs that the cache says are 90 days old: one really is
/// (eligible), one was reopened by another writer (its timestamp untouched, so
/// only the status re-check catches it), and one was edited recently (still
/// Completed, so only the age re-check catches it). Then the cache goes stale.
fn archive_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.add("BUG-1", RequirementStatus::Completed, 90);
    fx.add("BUG-2", RequirementStatus::Completed, 90);
    fx.add("BUG-3", RequirementStatus::Completed, 90);
    fx.freshen_cache();
    fx.external_edit("BUG-2", |r| r.status = RequirementStatus::Approved);
    fx.external_edit("BUG-3", |r| r.modified_at = chrono::Utc::now());
    fx.hold_foreign_writer();
    assert_eq!(
        fx.cached_status("BUG-2"),
        "Completed",
        "fixture: the cache must serve the stale pre-reopen status"
    );
    fx
}

fn assert_only_the_eligible_spec_archived(fx: &Fixture) {
    assert!(
        fx.authoritative("BUG-1").archived,
        "the truly eligible spec is archived"
    );
    let reopened = fx.authoritative("BUG-2");
    assert!(!reopened.archived, "a reopened spec must not be archived");
    assert_eq!(reopened.status, RequirementStatus::Approved);
    assert!(
        !fx.authoritative("BUG-3").archived,
        "a recently modified spec must not be archived"
    );
}

// trace:BUG-1664 | ai:claude
#[test]
fn archive_older_than_skips_reopened_and_recent_specs_behind_a_stale_snapshot() {
    let fx = archive_fixture();
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
    .unwrap();
    assert_only_the_eligible_spec_archived(&fx);
}

// trace:BUG-1664 | ai:claude
#[test]
fn post_pull_auto_archive_skips_reopened_and_recent_specs_behind_a_stale_snapshot() {
    let fx = archive_fixture();
    std::fs::write(
        fx.project_root().join(".aida").join("config.toml"),
        "[archive]\nauto_after_days = 30\n",
    )
    .unwrap();
    crate::maybe_auto_archive_sweep(fx.project_root(), &fx.backend(), true);
    assert_only_the_eligible_spec_archived(&fx);
}

// The re-check matches statuses the way the cache's status filter does, so a
// `--status in-progress --force` sweep still archives what it selected.
// trace:BUG-1664 | ai:claude
#[test]
fn archive_recheck_matches_statuses_like_the_cache_filter() {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let mut req = Requirement::new("t".into(), "b".into());
    req.status = RequirementStatus::InProgress;
    req.modified_at = cutoff - chrono::Duration::days(1);
    let eligible = |r: &Requirement, s: &[&str]| {
        crate::archive_cmd::archive_sweep_still_eligible(r, s, cutoff)
    };
    assert!(eligible(&req, &["In Progress"]));
    assert!(eligible(&req, &["in-progress", "completed"]));
    assert!(!eligible(&req, &["completed", "rejected"]));
    req.archived = true;
    assert!(!eligible(&req, &["in_progress"]), "already archived");
    req.archived = false;
    req.modified_at = cutoff + chrono::Duration::seconds(1);
    assert!(!eligible(&req, &["inprogress"]), "not old enough");
}
