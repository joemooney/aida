use super::*;

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

    // Merge (never clobber) the roster ceiling for `subject`.
    let seat = aida_core::team::canonical_role(seat);
    let mut members: std::collections::BTreeMap<String, Vec<String>> =
        aida_core::team::TeamRoster::load(&store)
            .entries()
            .into_iter()
            .collect();
    let seats = members.entry(subject.to_string()).or_default();
    for requested in std::iter::once(seat.as_str()).chain(delegable.iter().copied()) {
        let canonical = aida_core::team::canonical_role(requested);
        if !seats.iter().any(|have| *have == canonical) {
            seats.push(canonical);
        }
    }
    #[derive(serde::Serialize)]
    struct RosterFile {
        members: std::collections::BTreeMap<String, Vec<String>>,
    }
    std::fs::write(
        registry.join("team.toml"),
        toml::to_string(&RosterFile { members }).unwrap(),
    )
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
    };
    write_grant(&grant).unwrap();
    grant.id
}
