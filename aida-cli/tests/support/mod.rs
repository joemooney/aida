//! STORY-1473 / ADR-66 shared fixture: establish a validated, session-bound
//! seat grant for binary-driving tests.
//!
//! ADR-66 removed every ambient authority path — `AIDA_SESSION_ROLE`, a bare
//! TTY, and roster-membership-as-seat — so a test that drives the real `aida`
//! binary through an authorized operation must establish what `aida role
//! enter` would at a human TTY: a roster ceiling in the project store plus a
//! grant record under the (hermetic) test HOME, exported to the child through
//! `AIDA_SESSION_GRANT`. This writes exactly the records production
//! validates; nothing here opens a production bypass.
//!
//! The record shape mirrors `aida-cli-lib/src/seat_authority.rs`'s
//! `SeatGrant`; a drifting field fails these tests closed (the grant stops
//! validating), never open.
// trace:STORY-1473 | ai:claude

use std::path::{Path, PathBuf};

/// Mint a `seat` grant for the identity the spawned `aida` binary resolves
/// (`AIDA_USER` / `USER` / `USERNAME`, as `current_user_id` does), rostered in
/// `repo`'s store, with its record under `home`. Returns the grant handle to
/// pass as `AIDA_SESSION_GRANT`. `delegable` lists child seats this session
/// may issue (empty in production by default).
#[allow(dead_code)] // each test binary compiles its own copy of this module
pub fn grant_seat(home: &Path, repo: &Path, seat: &str, delegable: &[&str]) -> String {
    let subject = std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    grant_seat_for(home, repo, &subject, seat, delegable)
}

/// [`grant_seat`] for command-builder call sites: a no-op before `aida init`
/// has scaffolded `repo/.aida/config.toml` (init itself needs no seat), and a
/// stable per-home grant afterwards, so a builder can call it on every
/// command without churning records.
#[allow(dead_code)]
pub fn ensure_seat(home: &Path, repo: &Path, seat: &str, delegable: &[&str]) -> Option<String> {
    let subject = std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    ensure_seat_for(home, repo, &subject, seat, delegable)
}

/// [`ensure_seat`] with an explicit grant subject, for builders that pin
/// `AIDA_USER` on the spawned command.
#[allow(dead_code)]
pub fn ensure_seat_for(
    home: &Path,
    repo: &Path,
    subject: &str,
    seat: &str,
    delegable: &[&str],
) -> Option<String> {
    if !repo.join(".aida").join("config.toml").exists() {
        return None;
    }
    let id = stable_grant_id(home, subject, seat);
    if !home
        .join(".aida")
        .join("session-grants")
        .join(format!("{id}.json"))
        .exists()
    {
        grant_seat_with_id(home, repo, subject, seat, delegable, &id);
    }
    Some(id)
}

/// A deterministic, well-formed UUID string derived from `home` + `seat`, so
/// repeated `ensure_seat` calls in one fixture reuse one record.
#[allow(dead_code)]
fn stable_grant_id(home: &Path, subject: &str, seat: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut a = std::collections::hash_map::DefaultHasher::new();
    (home, subject, seat, "lo").hash(&mut a);
    let lo = a.finish();
    let mut b = std::collections::hash_map::DefaultHasher::new();
    (home, subject, seat, "hi").hash(&mut b);
    let hi = b.finish();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (hi >> 32) as u32,
        (hi >> 16) as u16,
        hi as u16,
        (lo >> 48) as u16,
        lo & 0xffff_ffff_ffff
    )
}

/// [`grant_seat`] with an explicit grant subject, for tests that pin
/// `AIDA_USER` on the spawned command.
#[allow(dead_code)]
pub fn grant_seat_for(
    home: &Path,
    repo: &Path,
    subject: &str,
    seat: &str,
    delegable: &[&str],
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    grant_seat_with_id(home, repo, subject, seat, delegable, &id);
    id
}

fn grant_seat_with_id(
    home: &Path,
    repo: &Path,
    subject: &str,
    seat: &str,
    delegable: &[&str],
    id: &str,
) {
    // Roster ceiling: `subject` may take `seat` plus every delegable seat.
    // Merged (never clobbered) so one fixture can seat several roles/users.
    let store = store_root(repo);
    std::fs::create_dir_all(store.join("objects")).expect("create store objects dir");
    let registry = store.join("registry");
    std::fs::create_dir_all(&registry).expect("create store registry dir");
    let roster_path = registry.join("team.toml");
    let mut table: toml::Table = std::fs::read_to_string(&roster_path)
        .ok()
        .and_then(|content| content.parse().ok())
        .unwrap_or_default();
    let members = table
        .entry("members")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .expect("members is a table");
    let mut seats: Vec<toml::Value> = match members.get(subject) {
        Some(toml::Value::Array(existing)) => existing.clone(),
        Some(toml::Value::String(existing)) => vec![toml::Value::String(existing.clone())],
        _ => Vec::new(),
    };
    for s in std::iter::once(seat).chain(delegable.iter().copied()) {
        if !seats.iter().any(|have| have.as_str() == Some(s)) {
            seats.push(toml::Value::String(s.to_string()));
        }
    }
    members.insert(subject.to_string(), toml::Value::Array(seats));
    std::fs::write(&roster_path, toml::to_string(&table).unwrap()).expect("write roster ceiling");

    // Grant record, as `aida role enter` writes it at a TTY.
    let now = chrono::Utc::now();
    let expires = now + chrono::Duration::hours(23);
    let record = serde_json::json!({
        "id": id,
        "principal": subject,
        "subject": subject,
        "session_id": uuid::Uuid::new_v4().to_string(),
        "seat": seat,
        "tty_issued_at": now,
        "delegable_seats": delegable,
        "parent_grant_id": null,
        "issued_at": now,
        "expires_at": expires,
        "revoked_at": null,
    });
    let grants = home.join(".aida").join("session-grants");
    std::fs::create_dir_all(&grants).expect("create session-grants dir");
    std::fs::write(
        grants.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .expect("write grant record");
}

/// The store the spawned binary will detect for `repo`: the first
/// `store_path` named by `repo/.aida/config.toml`, else `.aida-store`.
fn store_root(repo: &Path) -> PathBuf {
    let config = repo.join(".aida").join("config.toml");
    if let Ok(content) = std::fs::read_to_string(&config) {
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("store_path") {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        return repo.join(value);
                    }
                }
            }
        }
        // Config exists but names no store: append the default top-level key
        // (prepended so it cannot land inside a `[table]`).
        if !content.contains("store_path") {
            std::fs::write(&config, format!("store_path = \".aida-store\"\n{content}"))
                .expect("add default store_path to fixture config");
        }
    } else {
        std::fs::create_dir_all(repo.join(".aida")).expect("create .aida dir");
        std::fs::write(&config, "store_path = \".aida-store\"\n").expect("write fixture config");
    }
    repo.join(".aida-store")
}
