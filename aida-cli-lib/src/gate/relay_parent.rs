//! Mandate-derived relay parent replacement. Same-uid authority trust as mandates.
// trace:TASK-3119 | ai:codex
// mandate-gate
use super::{mandate, seat_authority as seat};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Closed refusal codes for controllers; stderr text is never a protocol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RefusalCode {
    KnobOff,
    InvalidPolicy,
    InvalidDuration,
    AuthorityBusy,
    MissingParent,
    InvalidParent,
    IdentityMismatch,
    PresenterNotBound,
    ParentNotExpiring,
    ParentSuperseded,
    NoActiveMandate,
    MandateOutOfScope,
    InsufficientLifetime,
    UnsafeRunDir,
    HandoffExists,
    EvidenceUnreadable,
    AuditFailed,
}
pub(crate) fn refusal_code(error: &anyhow::Error) -> RefusalCode {
    use RefusalCode::*;
    let text = error.to_string();
    match text.as_str() {
        "knob-off" => KnobOff,
        "invalid-policy" => InvalidPolicy,
        "invalid-duration" => InvalidDuration,
        "authority-busy" | "parent-busy" => AuthorityBusy,
        "missing-parent" => MissingParent,
        "not-renewable-parent" => InvalidParent,
        "identity-mismatch" => IdentityMismatch,
        "presenter-not-bound" => PresenterNotBound,
        "parent-not-expiring" => ParentNotExpiring,
        "parent-superseded" => ParentSuperseded,
        "no-active-mandate" => NoActiveMandate,
        "mandate-out-of-scope" | "missing-renewal-authorization" => MandateOutOfScope,
        "insufficient-lifetime" => InsufficientLifetime,
        "unsafe-run-dir" => UnsafeRunDir,
        "handoff-exists" => HandoffExists,
        "audit-failed" => AuditFailed,
        _ => EvidenceUnreadable,
    }
}

const ORIGIN: &str = "mandate-relay-renewal:";
const LIMIT: u64 = 65536;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authorization {
    pub version: u32,
    pub project: PathBuf,
    pub seats: Vec<String>,
}

pub(crate) fn authorization_for_seats(seats: &[String]) -> Result<Authorization> {
    let defaults = ["advisor".to_string()];
    authorization(
        &crate::statusline_project_root(),
        if seats.is_empty() { &defaults } else { seats },
    )
}
pub(crate) fn authorization(root: &Path, seats: &[String]) -> Result<Authorization> {
    let principal = mandate::operator_principal();
    let seats: Vec<_> = seats
        .iter()
        .map(|s| aida_core::team::canonical_role(s))
        .collect();
    anyhow::ensure!(
        !seats.is_empty()
            && seats
                .iter()
                .all(|s| seat::roster_allows(root, &principal, s)),
        "relay target seats must be in the operator roster"
    );
    Ok(Authorization {
        version: 1,
        project: seat::roster_store(root)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .canonicalize()?,
        seats,
    })
}

/// One permanent lock shared by mandate start/renew/stop and replacements.
/// Bounded acquisition: callers refuse rather than hang behind a stalled writer.
pub(crate) fn authority_lock(dir: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("relay-parent.lock"))?;
    // A concurrent fork can briefly inherit a lock until exec closes the fd.
    // Tolerate that short handoff, but never wait indefinitely for authority.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => return Err(e).context("authority-busy"),
        }
    }
    Ok(lock)
}

pub(crate) fn enabled() -> Result<bool> {
    let path = super::kernel_deps::aida_home_dir()
        .context("missing home")?
        .join(".aida")
        .join("config.toml");
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let value: toml::Value = toml::from_str(&text)?;
    match value
        .get("seat")
        .and_then(|s| s.get("mandate_relay_parent_renewal"))
    {
        None => Ok(false),
        Some(v) => v
            .as_bool()
            .context("mandate_relay_parent_renewal must be boolean"),
    }
}

/// Safe bounded file reader shared by handoffs and receipts, including FIFOs.
fn private_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let m = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    check_private_metadata(&m, unsafe { libc::geteuid() })?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= LIMIT, "oversized record");
    Ok(bytes)
}
fn check_private_metadata(m: &std::fs::Metadata, uid: u32) -> Result<()> {
    anyhow::ensure!(
        m.is_file()
            && m.mode() & 0o7777 == 0o600
            && m.uid() == uid
            && m.nlink() == 1
            && m.len() <= LIMIT,
        "unsafe private record"
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelayGrant {
    pub version: u32,
    pub grant: String,
    pub expires_at: DateTime<Utc>,
    pub mandate: String,
    pub supersedes: String,
}

pub(crate) fn read_relay_grant(run_dir: &Path) -> Result<RelayGrant> {
    let bytes = private_bytes(&run_dir.join("relay.grant"))?;
    let text = std::str::from_utf8(&bytes)?;
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once('=').context("malformed handoff")?;
        anyhow::ensure!(fields.insert(k, v).is_none(), "duplicate field");
    }
    anyhow::ensure!(
        fields.len() == 5 && fields.get("version") == Some(&"1"),
        "unsupported handoff"
    );
    let get = |key| fields.get(key).copied().context("missing field");
    let record = RelayGrant {
        version: 1,
        grant: get("grant")?.into(),
        expires_at: DateTime::parse_from_rfc3339(get("expires_at")?)?.with_timezone(&Utc),
        mandate: get("mandate")?.into(),
        supersedes: get("supersedes")?.into(),
    };
    for id in [&record.grant, &record.mandate, &record.supersedes] {
        Uuid::parse_str(id)?;
    }
    anyhow::ensure!(
        record.grant != record.supersedes,
        "grant must replace its predecessor"
    );
    Ok(record)
}

fn write_handoff(dir: &Path, r: &RelayGrant) -> Result<()> {
    let path = dir.join("relay.grant");
    // Never overwrite stale success, a link or an unreadable record on refusal.
    anyhow::ensure!(
        std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "handoff-exists"
    );
    let tmp = dir.join(format!(".relay.grant.{}", Uuid::new_v4()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        write!(
            f,
            "version=1\ngrant={}\nexpires_at={}\nmandate={}\nsupersedes={}\n",
            r.grant,
            r.expires_at.to_rfc3339(),
            r.mandate,
            r.supersedes
        )?;
        f.sync_all()?;
        // link is atomic and cannot replace a concurrent handoff.
        std::fs::hard_link(&tmp, &path)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(tmp);
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Receipt {
    pub version: u32,
    pub id: String,
    pub at: DateTime<Utc>,
    pub outcome: String,
    pub reason: Option<String>,
    pub refusal_code: Option<RefusalCode>,
    pub mandate: Option<String>,
    pub supersedes: Option<String>,
    pub grant: Option<String>,
    pub authorization: Option<Authorization>,
    pub effective: Option<seat::SeatGrant>,
    pub requested_expires_at: DateTime<Utc>,
    pub previous_expires_at: Option<DateTime<Utc>>,
    pub clamps: Vec<String>,
}
fn receipt_dir() -> Result<PathBuf> {
    Ok(seat::grant_dir()?.with_file_name("relay-parent-renewals"))
}
fn save(r: &Receipt) -> Result<()> {
    let dir = receipt_dir()?;
    mandate::write_private(
        &dir.join(format!("{}.json", r.id)),
        &serde_json::to_vec_pretty(r)?,
    )?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

fn checked_mandates(
    dir: &Path,
    principal: &str,
    now: DateTime<Utc>,
) -> Result<Vec<mandate::Mandate>> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if matches!(
            path.extension().and_then(|x| x.to_str()),
            Some("json" | "stopped")
        ) {
            let stem = path
                .file_stem()
                .and_then(|x| x.to_str())
                .context("invalid mandate path")?;
            if Uuid::parse_str(stem).is_ok() {
                private_bytes(&path)?;
            }
        }
    }
    let (valid, warnings) = mandate::load_valid(dir, principal, now);
    anyhow::ensure!(warnings.is_empty(), "unreadable-mandate-evidence");
    Ok(valid)
}

/// Every use of a replacement (and its descendants) rechecks the mandate and
/// exact durable success receipt; a stopped or changed mandate grants nothing.
pub(crate) fn validate_origin(
    root: &Path,
    grant: &seat::SeatGrant,
    now: DateTime<Utc>,
) -> Result<()> {
    let Some(id) = grant.origin.as_deref().and_then(|s| s.strip_prefix(ORIGIN)) else {
        return Ok(());
    };
    Uuid::parse_str(id)?;
    let r: Receipt =
        serde_json::from_slice(&private_bytes(&receipt_dir()?.join(format!("{id}.json")))?)?;
    anyhow::ensure!(
        r.version == 1 && r.outcome == "success" && r.grant.as_deref() == Some(&grant.id),
        "missing successful renewal receipt"
    );
    // Validate today's complete lineage, then project only temporal evidence
    // back to the recorded instant. Later issues must not shadow this mandate;
    // later renewals and stops must not invalidate an earlier approval.
    let mut valid = checked_mandates(
        &mandate::mandate_dir()?,
        &grant.principal,
        Utc::now().max(now),
    )?;
    valid.retain(|m| m.issued_at <= now);
    for m in &mut valid {
        if let Some(next) = m.renewals.iter().find(|r| r.renewed_at > now) {
            m.expires_at = next.previous_expires_at;
        }
        if m.revoked_at.is_some_and(|at| at > now) {
            m.revoked_at = None;
        }
    }
    let mandate::MandateState::Active(m) = mandate::state_at(&valid, now) else {
        bail!("mandate inactive");
    };
    anyhow::ensure!(
        Some(m.id.as_str()) == r.mandate.as_deref(),
        "mandate inactive"
    );
    let auth = r.authorization.as_ref().context("missing authorization")?;
    anyhow::ensure!(
        m.covers(mandate::Scope::RelayParentRenewal)
            && m.relay_parent.as_ref() == Some(auth)
            && auth.version == 1
            && auth.project
                == seat::roster_store(root)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?
                    .canonicalize()?
            && grant.expires_at <= m.expires_at
            && auth.seats.contains(&grant.seat)
            && grant.delegable_seats.iter().all(|s| auth.seats.contains(s)),
        "mandate scope changed"
    );
    let effective = r.effective.context("missing effective grant")?;
    anyhow::ensure!(
        effective.principal == grant.principal
            && effective.subject == grant.subject
            && effective.seat == grant.seat
            && effective.delegable_seats == grant.delegable_seats
            && effective.expires_at == grant.expires_at
            && effective.binding == grant.binding,
        "grant differs from receipt"
    );
    Ok(())
}

pub(crate) fn renew(
    root: &Path,
    run_dir: &Path,
    requested: Duration,
    rotation: Duration,
) -> Result<RelayGrant> {
    anyhow::ensure!(enabled().context("invalid-policy")?, "knob-off");
    let mut receipt = Receipt {
        version: 1,
        id: Uuid::new_v4().to_string(),
        at: Utc::now(),
        outcome: "refused".into(),
        reason: None,
        refusal_code: None,
        mandate: None,
        supersedes: seat::current_grant_id(),
        grant: None,
        authorization: None,
        effective: None,
        requested_expires_at: Utc::now()
            .checked_add_signed(requested)
            .unwrap_or(Utc::now()),
        previous_expires_at: None,
        clamps: Vec::new(),
    };
    let result = (|| {
        anyhow::ensure!(
            requested > Duration::zero() && rotation > Duration::zero(),
            "invalid-duration"
        );
        // Pin the owned directory; neither a rename nor a symlink swap can
        // redirect the eventual handoff to a different directory.
        let run_handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(run_dir)?;
        let meta = run_handle.metadata()?;
        anyhow::ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
            "unsafe-run-dir"
        );
        use std::os::fd::AsRawFd;
        let pinned_run = PathBuf::from(format!("/proc/self/fd/{}", run_handle.as_raw_fd()));
        let run_dir = pinned_run.as_path();
        anyhow::ensure!(
            std::fs::symlink_metadata(run_dir.join("relay.grant"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "handoff-exists"
        );
        let dir = mandate::mandate_dir()?;
        let _lock = authority_lock(&dir)?;
        let old_id = receipt.supersedes.as_deref().context("missing-parent")?;
        let _parent_lock = seat::lock_parent(old_id)?;
        let old = seat::renewable_parent(root, old_id, rotation)?;
        let entries = match std::fs::read_dir(receipt_dir()?) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(entries) = entries {
            for entry in entries {
                let path = entry?.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    let previous: Receipt = serde_json::from_slice(&private_bytes(&path)?)?;
                    anyhow::ensure!(
                        previous.outcome != "success"
                            || previous.supersedes.as_deref() != Some(old_id),
                        "parent-superseded"
                    );
                }
            }
        }
        let now = Utc::now();
        let valid = checked_mandates(&dir, &old.principal, now)?;
        let mandate::MandateState::Active(m) = mandate::state_at(&valid, now) else {
            bail!("no-active-mandate");
        };
        receipt.mandate = Some(m.id.clone());
        let auth = m
            .relay_parent
            .as_ref()
            .context("missing-renewal-authorization")?;
        anyhow::ensure!(
            m.covers(mandate::Scope::RelayParentRenewal)
                && auth.version == 1
                && auth.project
                    == seat::roster_store(root)
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?
                        .canonicalize()?
                && auth.seats.contains(&old.seat),
            "mandate-out-of-scope"
        );
        receipt.previous_expires_at = Some(old.expires_at);
        receipt.requested_expires_at = now
            .checked_add_signed(requested)
            .context("invalid-duration")?;
        let expires = receipt
            .requested_expires_at
            .min(m.expires_at)
            .min(now + Duration::hours(seat::resolve_max_grant_hours(root) as i64));
        anyhow::ensure!(
            expires > now && expires - now >= rotation,
            "insufficient-lifetime"
        );
        if expires == m.expires_at && expires < receipt.requested_expires_at {
            receipt.clamps.push("mandate-end".into());
        }
        if expires < receipt.requested_expires_at && expires < m.expires_at {
            receipt.clamps.push("grant-cap".into());
        }
        let mut new = old.clone();
        new.id = Uuid::new_v4().to_string();
        new.session_id = Uuid::new_v4().to_string();
        new.issued_at = now;
        new.expires_at = expires;
        new.origin = Some(format!("{ORIGIN}{}", receipt.id));
        new.delegable_seats
            .retain(|s| auth.seats.contains(s) && seat::roster_allows(root, &old.principal, s));
        receipt.outcome = "success".into();
        receipt.grant = Some(new.id.clone());
        receipt.authorization = Some(auth.clone());
        receipt.effective = Some(new.clone());
        save(&receipt)?; // durable receipt precedes activation
        seat::persist_replacement(&new)?;
        let handoff = RelayGrant {
            version: 1,
            grant: new.id.clone(),
            expires_at: expires,
            mandate: m.id,
            supersedes: old.id,
        };
        if let Err(e) = write_handoff(run_dir, &handoff) {
            let _ = seat::revoke_by_id(&new.id);
            return Err(e);
        }
        Ok(handoff)
    })();
    if let Err(ref err) = result {
        receipt.outcome = "refused".into();
        receipt.reason = Some(err.to_string());
        receipt.refusal_code = Some(refusal_code(err));
        save(&receipt).context("audit-failed")?;
    }
    // The canonical receipt is mandatory; the event feed remains a mirror.
    let event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::RelayParentGrantRenewal {
            receipt: serde_json::to_value(&receipt)?,
        },
    );
    crate::events::emit(root, &event);
    result
}

pub(crate) fn summary(mandate_id: &str) -> serde_json::Value {
    let mut receipts = Vec::new();
    if let Ok(entries) = receipt_dir().and_then(|d| Ok(std::fs::read_dir(d)?)) {
        for e in entries.flatten() {
            if let Ok(r) =
                private_bytes(&e.path()).and_then(|b| Ok(serde_json::from_slice::<Receipt>(&b)?))
            {
                if r.mandate.as_deref() == Some(mandate_id) {
                    receipts.push(r);
                }
            }
        }
    }
    receipts.sort_by_key(|r| r.at);
    serde_json::json!({"count": receipts.iter().filter(|r| r.outcome == "success").count(), "refusal_count": receipts.iter().filter(|r| r.outcome != "success").count(), "last": receipts.last()})
}

#[cfg(test)]
mod task_3119_tests {
    use super::*;
    use crate::gate::test_support::test_env::EnvVarsGuard;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn handoff() -> RelayGrant {
        RelayGrant {
            version: 1,
            grant: Uuid::new_v4().to_string(),
            expires_at: Utc::now() + Duration::hours(3),
            mandate: Uuid::new_v4().to_string(),
            supersedes: Uuid::new_v4().to_string(),
        }
    }
    #[test]
    fn task_3119_handoff_reader_refuses_unsafe_files() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("relay.grant");
        assert!(read_relay_grant(d.path()).is_err());
        let r = handoff();
        write_handoff(d.path(), &r).unwrap();
        assert_eq!(read_relay_grant(d.path()).unwrap(), r);
        let meta = std::fs::metadata(&path).unwrap();
        assert!(check_private_metadata(&meta, meta.uid().wrapping_add(1)).is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
        assert!(write_handoff(d.path(), &handoff()).is_err());
        for mode in [0o644, 0o400, 0o700, 0o1600] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(read_relay_grant(d.path()).is_err(), "mode {mode:o}");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let backup = d.path().join("backup");
        std::fs::rename(&path, &backup).unwrap();
        symlink(&backup, &path).unwrap();
        assert!(read_relay_grant(d.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&backup, &path).unwrap();
        assert!(read_relay_grant(d.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read_relay_grant(d.path()).is_err());
        std::fs::remove_dir(&path).unwrap();
        let fifo = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // Nonblocking open must refuse the FIFO immediately, even with no writer.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(read_relay_grant(d.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        std::fs::write(&path, vec![b'x'; LIMIT as usize + 1]).unwrap();
        assert!(read_relay_grant(d.path()).is_err());
    }
    #[test]
    fn task_3119_handoff_reader_refuses_malformed_records() {
        let d = tempfile::tempdir().unwrap();
        let r = handoff();
        write_handoff(d.path(), &r).unwrap();
        let path = d.path().join("relay.grant");
        let good = std::fs::read_to_string(&path).unwrap();
        for bad in [
            good.replace("version=1", "version=2"),
            good.replace("version=1\n", ""),
            format!("{good}version=1\n"),
            format!("{good}unknown=x\n"),
            good.replace(&r.grant, "bogus"),
            good.replace(&r.mandate, "bogus"),
            good.replace(&r.supersedes, "bogus"),
            good.replace(&r.expires_at.to_rfc3339(), "tomorrow"),
            good.replace(&r.grant, &r.supersedes),
            "broken".into(),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(read_relay_grant(d.path()).is_err());
        }
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_relay_grant(d.path()).is_err());
    }

    struct Fixture {
        _env: EnvVarsGuard,
        _tmp: tempfile::TempDir,
        root: PathBuf,
        run: PathBuf,
        parent: seat::SeatGrant,
        mandate: mandate::Mandate,
        dir: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let mut env =
                EnvVarsGuard::snapshot(&["AIDA_USER", "USER", "AIDA_HOME", seat::GRANT_ENV]);
            env.set_key("AIDA_USER", "relay-test");
            env.set_key("USER", "relay-test");
            env.set_key("AIDA_HOME", tmp.path().join("home"));
            let root = tmp.path().join("project");
            std::fs::create_dir(&root).unwrap();
            let id =
                seat::test_support::mint_grant_for(&root, "relay-test", "advisor", &["reviewer"]);
            env.set_key(seat::GRANT_ENV, &id);
            let mut parent = seat::read_grant(&id).unwrap();
            parent.expires_at = Utc::now() - Duration::seconds(1);
            seat::persist_replacement(&parent).unwrap();
            let dir = mandate::mandate_dir().unwrap();
            let now = Utc::now();
            let mut m = mandate::start(
                &dir,
                &|| Ok(()),
                "relay-test",
                Duration::hours(3),
                None,
                false,
                now,
            )
            .unwrap();
            m.scopes.insert(mandate::Scope::RelayParentRenewal);
            m.relay_parent =
                Some(authorization(&root, &["advisor".into(), "reviewer".into()]).unwrap());
            let run = tmp.path().join("run");
            std::fs::create_dir(&run).unwrap();
            let f = Self {
                _env: env,
                _tmp: tmp,
                root,
                run,
                parent,
                mandate: m,
                dir,
            };
            f.save();
            f
        }
        fn save(&self) {
            mandate::write_private(
                &self.dir.join(format!("{}.json", self.mandate.id)),
                &serde_json::to_vec(&self.mandate).unwrap(),
            )
            .unwrap();
        }
        fn knob(&self, text: &str) {
            std::fs::write(
                super::super::kernel_deps::aida_home_dir()
                    .unwrap()
                    .join(".aida/config.toml"),
                text,
            )
            .unwrap();
        }
        fn renew(&self) -> Result<RelayGrant> {
            renew(
                &self.root,
                &self.run,
                Duration::hours(24),
                Duration::hours(2),
            )
        }
        fn refuses(&self) {
            assert!(self.renew().is_err());
            assert!(!self.run.join("relay.grant").exists());
        }
    }
    #[test]
    fn task_3119_knob_and_mandate_refusals_have_no_handoff() {
        let mut f = Fixture::new();
        let events_path = f.root.join(".aida/events.jsonl");
        let events_before = std::fs::read(&events_path).ok();
        let disabled = || {
            assert_eq!(refusal_code(&f.renew().unwrap_err()), RefusalCode::KnobOff);
            assert!(!f.run.join("relay.grant").exists());
            assert!(!receipt_dir().unwrap().exists());
            assert_eq!(std::fs::read(&events_path).ok(), events_before);
            assert_eq!(
                std::fs::read_dir(seat::grant_dir().unwrap())
                    .unwrap()
                    .count(),
                1
            );
        };
        disabled();
        std::fs::write(
            f.root.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n[seat]\nmandate_relay_parent_renewal=true\n",
        )
        .unwrap();
        disabled(); // project policy cannot enable authority
        f.knob("[seat]\nmandate_relay_parent_renewal=false\n");
        disabled();
        f.knob("[seat]\nmandate_relay_parent_renewal='yes'\n");
        f.refuses();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\n");
        let original = f.mandate.clone();
        f.mandate.scopes.clear();
        f.save();
        f.refuses();
        f.mandate = original.clone();
        f.mandate.relay_parent = None;
        f.save();
        f.refuses();
        f.mandate = original.clone();
        f.mandate.relay_parent.as_mut().unwrap().seats = vec!["reviewer".into()];
        f.save();
        f.refuses();
        f.mandate = original.clone();
        f.mandate.relay_parent.as_mut().unwrap().project = PathBuf::from("/elsewhere");
        f.save();
        f.refuses();
        f.mandate = original.clone();
        f.mandate.expires_at = Utc::now() + Duration::minutes(30);
        f.save();
        f.refuses();
        f.mandate = original.clone();
        f.mandate.issued_at = Utc::now() - Duration::hours(4);
        f.mandate.expires_at = Utc::now() - Duration::seconds(1);
        f.save();
        f.refuses();
        f.mandate = original;
        f.save();
        f.parent.revoked_at = Some(Utc::now());
        seat::persist_replacement(&f.parent).unwrap();
        f.refuses();
    }
    #[test]
    fn task_3119_replacement_clamps_audits_and_revokes_descendants() {
        let f = Fixture::new();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\n");
        let old_bytes = std::fs::read(
            seat::grant_dir()
                .unwrap()
                .join(format!("{}.json", f.parent.id)),
        )
        .unwrap();
        let handoff = f.renew().unwrap();
        assert_eq!(read_relay_grant(&f.run).unwrap(), handoff);
        assert_eq!(handoff.expires_at, f.mandate.expires_at);
        assert_eq!(handoff.supersedes, f.parent.id);
        assert_ne!(handoff.grant, f.parent.id);
        assert_eq!(
            std::fs::read(
                seat::grant_dir()
                    .unwrap()
                    .join(format!("{}.json", f.parent.id))
            )
            .unwrap(),
            old_bytes
        );
        assert!(matches!(
            seat::grant_check_for(&f.root, &handoff.grant, "relay-test"),
            seat::GrantCheck::Valid(_)
        ));
        let mut child = seat::read_grant(&handoff.grant).unwrap();
        child.id = Uuid::new_v4().to_string();
        child.parent_grant_id = Some(handoff.grant.clone());
        child.origin = None;
        child.seat = "reviewer".into();
        child.delegable_seats.clear();
        seat::persist_replacement(&child).unwrap();
        assert!(matches!(
            seat::grant_check_for(&f.root, &child.id, "relay-test"),
            seat::GrantCheck::Valid(_)
        ));
        let summary = summary(&f.mandate.id);
        assert_eq!(summary["count"], 1);
        assert_eq!(summary["last"]["outcome"], "success");
        mandate::stop(&f.dir, &|| Ok(()), "relay-test", Utc::now()).unwrap();
        assert!(!matches!(
            seat::grant_check_for(&f.root, &child.id, "relay-test"),
            seat::GrantCheck::Valid(_)
        ));
    }
    // trace:TASK-3119 | ai:codex
    #[test]
    fn task_3119_recorded_approval_survives_mandate_expiry_but_live_use_refuses() {
        use crate::gate::review_authority::verified_authority_keys;
        use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

        let mut f = Fixture::new();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\n");
        let handoff = f.renew().unwrap();
        let mut parent = seat::read_grant(&handoff.grant).unwrap();
        let mut child = parent.clone();
        child.id = Uuid::new_v4().to_string();
        child.parent_grant_id = Some(parent.id.clone());
        child.origin = None;
        child.seat = "reviewer".into();
        child.delegable_seats.clear();
        seat::persist_replacement(&child).unwrap();
        let mut approval = RecordedVerdict {
            kind: VerdictKind::Approved,
            recorded_at: Some(Utc::now().to_rfc3339()),
            attestation: Some(RecorderAttestation {
                sha: "reviewed-head".into(),
                seat: Some("reviewer".into()),
                grant_id: Some(child.id.clone()),
                grant_binding: child.binding.clone(),
                identity: child.binding.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            verified_authority_keys(&f.root, &[approval.clone()]).len(),
            1
        );

        // Move the entire issued timeline into the past, without sleeping or
        // changing its intervals. The approval predates expiry; verification
        // now occurs after expiry, as when CI outlasts the mandate.
        let elapsed = Duration::hours(4);
        let recorded_at =
            chrono::DateTime::parse_from_rfc3339(approval.recorded_at.as_deref().unwrap())
                .unwrap()
                .with_timezone(&Utc)
                - elapsed;
        approval.recorded_at = Some(recorded_at.to_rfc3339());
        f.mandate.issued_at -= elapsed;
        f.mandate.expires_at -= elapsed;
        f.save();
        parent.issued_at -= elapsed;
        parent.expires_at -= elapsed;
        seat::persist_replacement(&parent).unwrap();
        child.issued_at -= elapsed;
        child.expires_at -= elapsed;
        seat::persist_replacement(&child).unwrap();
        let receipt_id = parent
            .origin
            .as_deref()
            .unwrap()
            .strip_prefix(ORIGIN)
            .unwrap();
        let mut receipt: Receipt = serde_json::from_slice(
            &private_bytes(&receipt_dir().unwrap().join(format!("{receipt_id}.json"))).unwrap(),
        )
        .unwrap();
        receipt.at -= elapsed;
        receipt.requested_expires_at -= elapsed;
        receipt.previous_expires_at = receipt.previous_expires_at.map(|at| at - elapsed);
        receipt.effective = Some(parent.clone());
        save(&receipt).unwrap();

        let recorded = recorded_at;
        let later = recorded + Duration::minutes(10);
        let original = f.mandate.clone();
        mandate::renew(
            &f.dir,
            &|| Ok(()),
            "relay-test",
            Some(Duration::hours(3)),
            later,
        )
        .unwrap();
        assert_eq!(
            verified_authority_keys(&f.root, &[approval.clone()]).len(),
            1
        );
        // An unrelated later issuance must not shadow the recorded mandate.
        let mut newer = original.clone();
        newer.id = Uuid::new_v4().to_string();
        newer.issued_at = later;
        newer.expires_at = later + Duration::hours(1);
        mandate::write_private(
            &f.dir.join(format!("{}.json", newer.id)),
            &serde_json::to_vec(&newer).unwrap(),
        )
        .unwrap();
        assert_eq!(
            verified_authority_keys(&f.root, &[approval.clone()]).len(),
            1
        );
        std::fs::remove_file(f.dir.join(format!("{}.json", newer.id))).unwrap();
        mandate::stop(&f.dir, &|| Ok(()), "relay-test", later).unwrap();
        assert_eq!(
            verified_authority_keys(&f.root, &[approval.clone()]).len(),
            1
        );
        assert!(validate_origin(&f.root, &parent, later).is_err());
        assert!(
            validate_origin(&f.root, &parent, original.issued_at - Duration::seconds(1)).is_err()
        );
        std::fs::remove_file(f.dir.join(format!("{}.stopped", original.id))).unwrap();
        f.mandate = original;
        f.save();

        assert!(recorded_at < f.mandate.expires_at);
        assert!(f.mandate.expires_at < Utc::now());
        assert_eq!(
            verified_authority_keys(&f.root, &[approval.clone()]).len(),
            1
        );
        // Isolate mandate revalidation as well as the ordinary live-use gate.
        assert!(validate_origin(&f.root, &parent, recorded_at).is_ok());
        assert!(validate_origin(&f.root, &parent, Utc::now()).is_err());
        assert!(!matches!(
            seat::grant_check_for(&f.root, &child.id, "relay-test"),
            seat::GrantCheck::Valid(_)
        ));
        approval.recorded_at = Some(Utc::now().to_rfc3339());
        assert!(verified_authority_keys(&f.root, &[approval]).is_empty());
    }
    #[test]
    fn task_3119_scope_and_cap_intersections_and_duplicate_refusal() {
        let mut f = Fixture::new();
        f.mandate.relay_parent.as_mut().unwrap().seats = vec!["advisor".into()];
        f.save();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\nmax_grant_hours=1\n");
        let before = Utc::now();
        let r = renew(&f.root, &f.run, Duration::hours(24), Duration::minutes(30)).unwrap();
        let new = seat::read_grant(&r.grant).unwrap();
        assert!(new.delegable_seats.is_empty());
        assert!(r.expires_at >= before + Duration::hours(1));
        assert!(r.expires_at <= Utc::now() + Duration::hours(1));
        // Issuance has not installed the handle in the controller. Exercise
        // the normal child-launch preflight only after the caller installs it.
        assert_eq!(
            seat::current_grant_id().as_deref(),
            Some(f.parent.id.as_str())
        );
        f._env.set_key(seat::GRANT_ENV, &r.grant);
        assert!(seat::validate_child_delegation(&f.root, "reviewer").is_err());
        f._env.set_key(seat::GRANT_ENV, &f.parent.id);
        std::fs::remove_file(f.run.join("relay.grant")).unwrap();
        f.refuses(); // consumed handoff must not allow another mint from the predecessor
        assert_eq!(summary(&f.mandate.id)["count"], 1);
    }
    #[test]
    fn task_3119_identity_binding_and_missing_mandate_refuse() {
        let mut f = Fixture::new();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\n");
        let original = f.parent.clone();
        f.parent.subject = "impostor".into();
        seat::persist_replacement(&f.parent).unwrap();
        f.refuses();
        f.parent = original.clone();
        f.parent.binding = vec!["proc:0@0".into()];
        seat::persist_replacement(&f.parent).unwrap();
        f.refuses();
        f.parent = original;
        seat::persist_replacement(&f.parent).unwrap();
        std::fs::remove_file(f.dir.join(format!("{}.json", f.mandate.id))).unwrap();
        f.refuses();
    }
    #[test]
    fn task_3119_authority_lock_contention_then_stop_refuses_renewal() {
        let f = Fixture::new();
        f.knob("[seat]\nmandate_relay_parent_renewal=true\n");
        let lock = authority_lock(&f.dir).unwrap();
        f.refuses();
        assert!(mandate::stop(&f.dir, &|| Ok(()), "relay-test", Utc::now()).is_err());
        drop(lock);
        mandate::stop(&f.dir, &|| Ok(()), "relay-test", Utc::now()).unwrap();
        f.refuses();
    }
}

#[cfg(test)]
mod task_3119_extra_tests {
    use super::*;
    #[test]
    fn task_3119_refusal_codes_are_closed_json_values() {
        for (reason, expected) in [
            ("knob-off", "knob-off"),
            ("parent-superseded", "parent-superseded"),
            ("authority-busy", "authority-busy"),
            ("missing-renewal-authorization", "mandate-out-of-scope"),
            ("unrecognized error", "evidence-unreadable"),
        ] {
            assert_eq!(
                serde_json::to_value(refusal_code(&anyhow::anyhow!(reason))).unwrap(),
                expected
            );
        }
    }
}
