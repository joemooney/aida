use super::*;

thread_local! {
    /// BUG-1934: force [`in_foreign_pid_namespace`] for this test thread
    /// (`None` = probe for real). Unit tests default to "not foreign" so
    /// a test suite run inside a sandbox keeps its expected sentences.
    pub(crate) static FOREIGN_NAMESPACE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(Some(false)) };
}

/// Panic unless `store` is a scratch store a test may write: under a
/// system temp root, or under this workspace's `target/` (fixtures that
/// must live outside `/tmp`). A test that resolves a real checkout's store
/// fails here instead of rewriting it (BUG-2209).
// trace:BUG-2209 | ai:claude
pub(crate) fn assert_scratch_store(store: &Path) {
    if aida_core::team::is_under_temp_root(store) {
        return;
    }
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|workspace| workspace.join("target"));
    let canonical = store.canonicalize().unwrap_or_else(|_| store.to_path_buf());
    let in_target = target
        .and_then(|t| t.canonicalize().ok())
        .is_some_and(|t| canonical.starts_with(t));
    assert!(
        in_target,
        "test wrote a roster to `{}`, which is neither under a temp directory nor under \
         the workspace target/: the fixture resolved a real store",
        store.display()
    );
}

/// Write the store plumbing (`.aida/config.toml`, `objects/`) plus a
/// roster ceiling allowing `seat` ∪ `delegable` for `subject`, then mint
/// a direct grant bound to `subject` and return its handle. The caller
/// exports it via `AIDA_SESSION_GRANT` through the shared env guards
/// (`test_env::AmbientGuard::hermetic_with_seat` bundles all of it).
pub(crate) fn mint_grant_for(
    project_root: &Path,
    subject: &str,
    seat: &str,
    delegable: &[&str],
) -> String {
    mint_grant_for_with_ttl(project_root, subject, seat, delegable, None)
}

// trace:TASK-1787 | ai:antigravity
pub(crate) fn mint_grant_for_with_ttl(
    project_root: &Path,
    subject: &str,
    seat: &str,
    delegable: &[&str],
    ttl: Option<Duration>,
) -> String {
    // The mint resolves env-derived paths (the grant-store home via
    // `$AIDA_HOME`/`$HOME`); hold the process env lock for the duration
    // unless the caller already does, so a sibling test's guard cannot
    // swap those variables mid-mint and strand the record in a home this
    // mint never prepared. trace:STORY-1473 | ai:claude
    let _lock = (!crate::test_env::holds_env_lock()).then(crate::test_env::env_lock);
    let aida_dir = project_root.join(".aida");
    std::fs::create_dir_all(&aida_dir).unwrap();
    let config = aida_dir.join("config.toml");
    if !config.exists() {
        std::fs::write(&config, "store_path = \".aida-store\"\n").unwrap();
    }
    let store = crate::detect_distributed_store_from(project_root).unwrap_or_else(|| {
        // The fixture config exists but names no reachable store (e.g. an
        // `[agents]`-only config): append the default so the validation
        // path detects the same store this fixture seeds.
        let store = project_root.join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let content = std::fs::read_to_string(&config).unwrap();
        if !content.contains("store_path") {
            // Prepended so the key stays top-level (appending after a
            // `[table]` header would land inside that table).
            std::fs::write(&config, format!("store_path = \".aida-store\"\n{content}")).unwrap();
        }
        store
    });
    std::fs::create_dir_all(store.join("objects")).unwrap();
    let registry = store.join("registry");
    std::fs::create_dir_all(&registry).unwrap();

    // Merge (never clobber) the roster ceiling for `subject`, and only
    // into a scratch store: a fixture whose walk-up escaped to a real
    // checkout once wrote this roster over the operator's (BUG-2209).
    // trace:BUG-2209 | ai:claude
    assert_scratch_store(&store);
    let seat = aida_core::team::canonical_role(seat);
    aida_core::team::write_member_seats(&store, subject, None, |current| {
        let mut seats = current.map(<[String]>::to_vec).unwrap_or_default();
        for requested in std::iter::once(seat.as_str()).chain(delegable.iter().copied()) {
            let canonical = aida_core::team::canonical_role(requested);
            if !seats.iter().any(|have| *have == canonical) {
                seats.push(canonical);
            }
        }
        Some(seats)
    })
    .unwrap();

    let lifetime = ttl.unwrap_or(MAX_GRANT_AGE);
    let now = Utc::now();
    let grant = SeatGrant {
        id: Uuid::new_v4().to_string(),
        principal: subject.to_string(),
        subject: subject.to_string(),
        session_id: Uuid::new_v4().to_string(),
        seat,
        tty_issued_at: now,
        delegable_seats: delegable
            .iter()
            .map(|role| aida_core::team::canonical_role(role))
            .collect(),
        parent_grant_id: None,
        issued_at: now,
        expires_at: now + lifetime,
        revoked_at: None,
        // BUG-1934: bound to the test process, the presenter of every
        // in-process check and an ancestor of every binary a test spawns.
        // trace:BUG-1934 | ai:claude
        binding: own_process_token().into_iter().collect(),
    };
    write_grant(&grant).unwrap();
    grant.id
}
