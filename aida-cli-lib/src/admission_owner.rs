//! Cooperative admission exclusion. Kernel flock is the exclusion authority.
//! Review controllers and cooperative trains share a mode; land excludes both.
// trace:STORY-2857 | ai:codex
use aida_core::proc_identity::{ProcIdentity, Verdict};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    identity: ProcIdentity,
    purpose: String,
    token: String,
    /// When the claim was taken; `advisor loops status` shows its age. Older
    /// receipts omit it, and older readers ignore it.
    // trace:BUG-2703 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acquired_at: Option<DateTime<Utc>>,
}
#[derive(Debug)]
pub(crate) struct AdmissionBusy(pub(crate) String);
impl std::fmt::Display for AdmissionBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "admission busy: {}", self.0)
    }
}
impl std::error::Error for AdmissionBusy {}

pub(crate) struct Claim {
    lock: File,
    receipt: PathBuf,
}
impl Drop for Claim {
    fn drop(&mut self) {
        // Match acquire's receipts -> mode ordering: while waiting for receipts,
        // retain our mode lock. Readers only try (never wait for) the mode lock.
        // trace:STORY-2857 | ai:codex
        // The serializer wait is bounded (BUG-2703): a stalled holder cannot
        // keep this claim alive. The receipt is removed either way; readers
        // skip a receipt that vanishes mid-scan, and the flock is the authority.
        // trace:BUG-2703 | ai:claude
        if let Some(dir) = self.receipt.parent() {
            let _serial = lock_serial(dir, std::time::Instant::now() + CLEANUP_GRACE);
            let _ = std::fs::remove_file(&self.receipt);
        }
        let _ = FileExt::unlock(&self.lock);
    }
}
fn open(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}

/// The least time any admission step waits for the receipt serializer, even
/// under a zero bound: other holders keep it for milliseconds, so a refusal
/// must not come from ordinary contention. Never an extension of a bound of
/// a second or more.
// trace:BUG-2703 | ai:claude
const SERIAL_GRACE: Duration = Duration::from_secs(1);
/// How long cleanup (a dropped claim or intent) waits for the serializer
/// before removing its own file without it.
const CLEANUP_GRACE: Duration = Duration::from_millis(500);

/// Take `receipts.lock` by bounded try-lock retries, never a blocking wait:
/// a holder that stalls while holding it (paused, wedged, stopped under a
/// debugger) cannot make an admission step outlive `until`. At `until` the
/// refusal is a typed [`AdmissionBusy`] naming the serializer's holder.
// trace:BUG-2703 | ai:claude
fn lock_serial(dir: &Path, until: std::time::Instant) -> Result<File> {
    let path = dir.join("receipts.lock");
    let serial = open(&path)?;
    loop {
        match serial.try_lock_exclusive() {
            Ok(()) => return Ok(serial),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let now = std::time::Instant::now();
                if now >= until {
                    return Err(AdmissionBusy(format!(
                        "the admission receipt serializer is held by {}",
                        serial_holder(&path)
                    ))
                    .into());
                }
                std::thread::sleep((until - now).min(Duration::from_millis(10)));
            }
            Err(error) => return Err(error).context("lock the admission receipt serializer"),
        }
    }
}

/// Who holds the flock on `path`, from `/proc/locks` (best effort: a lock
/// inherited across fork names the process that took it). Never trusted for
/// any decision; only reported.
// trace:BUG-2703 | ai:claude
fn serial_holder(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let unknown = || "a process whose PID is unknown".to_string();
    let Ok(meta) = std::fs::metadata(path) else {
        return unknown();
    };
    let Ok(locks) = std::fs::read_to_string("/proc/locks") else {
        return unknown();
    };
    let pids = flock_holders(&locks, meta.dev(), meta.ino());
    if pids.is_empty() {
        return unknown();
    }
    pids.iter()
        .map(
            |pid| match std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                Ok(comm) => format!("PID {pid} ({})", comm.trim()),
                Err(_) => format!("PID {pid}"),
            },
        )
        .collect::<Vec<_>>()
        .join(", ")
}

/// PIDs holding (not waiting for) a flock on device `dev`, inode `ino`, as
/// `/proc/locks` lists them: `N: FLOCK ADVISORY WRITE <pid> <maj>:<min>:<ino> ...`
/// with major/minor in hex. Blocked waiters (`N: -> FLOCK ...`) are skipped.
// trace:BUG-2703 | ai:claude
fn flock_holders(locks: &str, dev: u64, ino: u64) -> Vec<u32> {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff);
    let minor = (dev & 0xff) | ((dev >> 12) & !0xff);
    let mut pids = Vec::new();
    for line in locks.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 || fields[1] != "FLOCK" {
            continue;
        }
        let mut id = fields[5].split(':');
        let (Some(maj), Some(min), Some(inode), None) =
            (id.next(), id.next(), id.next(), id.next())
        else {
            continue;
        };
        if u64::from_str_radix(maj, 16) == Ok(major)
            && u64::from_str_radix(min, 16) == Ok(minor)
            && inode.parse::<u64>() == Ok(ino)
        {
            if let Ok(pid) = fields[4].parse::<u32>() {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
    }
    pids
}
fn directory(root: &Path) -> PathBuf {
    root.join(".aida/admission-owner")
}

/// Write a receipt or intent through a temporary file and a rename, so no
/// reader (several read without the serializer) ever sees it empty or half
/// written, and a failed or killed write leaves nothing under `path`. The
/// temporary name has no extension, so no reader takes it for a record.
// trace:BUG-2703 | ai:claude
fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("admission record has no directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(body)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Remove a dead holder's receipt or a stale intent. Its owner's bounded
/// cleanup removes the same file without the serializer, so finding it
/// already gone is success, not an error.
// trace:BUG-2703 | ai:claude
fn remove_stale(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("remove stale admission record {}", path.display()))
        }
        _ => Ok(()),
    }
}
fn receipts(dir: &Path) -> Result<Vec<Receipt>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            // A claim whose bounded cleanup removed its receipt without the
            // serializer is simply gone. trace:BUG-2703 | ai:claude
            let body = match aida_core::seat_clone_detect::read_regular_file_result_with_limit(
                &path, 16384,
            ) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                body => body.context("admission holder receipt is unreadable")?,
            };
            anyhow::ensure!(body.len() <= 16384, "admission holder receipt is too large");
            let row: Receipt = serde_json::from_str(&body)?;
            anyhow::ensure!(row.version == 1, "unsupported admission holder receipt");
            if matches!(
                aida_core::proc_identity::observe(&row.identity, true),
                Verdict::Dead(_)
            ) {
                remove_stale(&path)?;
            } else {
                rows.push(row);
            }
        }
    }
    Ok(rows)
}
fn busy(rows: &[Receipt]) -> anyhow::Error {
    AdmissionBusy(if rows.is_empty() {
        "holder PID/start identity/purpose unknown".into()
    } else {
        rows.iter()
            .map(|r| {
                format!(
                    "PID {}, start {}:{} (pidns {}), purpose {}",
                    r.identity.pid,
                    r.identity.boot_id,
                    r.identity.starttime,
                    r.identity.pidns_inode,
                    r.purpose
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
    .into()
}

/// Holder correlation for a contended per-service runtime lock.
// trace:BUG-2621 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ServiceHolder {
    /// Another live process holds a receipt naming this exact service purpose.
    Verified(u32),
    /// Anything else: missing, malformed, foreign, dead, recycled or unknown.
    Unverified(String),
}

/// Re-read holder receipts at decision time; never trust a cached copy.
/// Read-only: never unlinks, steals or signals from receipt evidence, and
/// never waits unboundedly for the receipt serializer. Only a receipt whose
/// purpose names `purpose` and whose PID/start identity is observed alive
/// (and is not this process) verifies; every other row is diagnostic only.
// trace:BUG-2621 | ai:claude
pub(crate) fn service_holder(root: &Path, purpose: &str) -> ServiceHolder {
    match service_holder_inner(root, purpose) {
        Ok(holder) => holder,
        Err(error) => ServiceHolder::Unverified(format!("holder evidence unreadable: {error:#}")),
    }
}

fn service_holder_inner(root: &Path, purpose: &str) -> Result<ServiceHolder> {
    let dir = directory(root);
    if !dir.is_dir() {
        return Ok(ServiceHolder::Unverified(
            "no admission holder receipt; holder PID/start identity/purpose unknown".into(),
        ));
    }
    let serial = open(&dir.join("receipts.lock"))?;
    let mut attempts = 0;
    loop {
        match serial.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e).context("admission holder receipts are busy"),
        }
    }
    let own = std::process::id();
    let mut verified = None;
    let mut evidence = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let row = aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, 16384)
            .map_err(anyhow::Error::from)
            .and_then(|body| {
                anyhow::ensure!(body.len() <= 16384, "too large");
                let row: Receipt = serde_json::from_str(&body)?;
                anyhow::ensure!(row.version == 1, "unsupported version {}", row.version);
                Ok(row)
            });
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                evidence.push(format!("malformed receipt {name}: {error:#}"));
                continue;
            }
        };
        let verdict = aida_core::proc_identity::observe(&row.identity, true);
        let state = match &verdict {
            Verdict::Alive => "alive",
            Verdict::Dead(_) => "dead or PID reused",
            Verdict::ForeignUnknown(_) => "unverifiable",
        };
        evidence.push(format!(
            "PID {}, start {}:{} (pidns {}), purpose {}: {state}",
            row.identity.pid,
            row.identity.boot_id,
            row.identity.starttime,
            row.identity.pidns_inode,
            row.purpose
        ));
        if verified.is_none()
            && row.purpose == purpose
            && row.identity.pid != own
            && matches!(verdict, Verdict::Alive)
        {
            verified = Some(row.identity.pid);
        }
    }
    let _ = FileExt::unlock(&serial);
    Ok(match verified {
        Some(pid) => ServiceHolder::Verified(pid),
        None if evidence.is_empty() => ServiceHolder::Unverified(
            "no admission holder receipt; holder PID/start identity/purpose unknown".into(),
        ),
        None => ServiceHolder::Unverified(format!(
            "no live {purpose} holder verified; receipts: {}",
            evidence.join("; ")
        )),
    })
}

/// Write a holder receipt for an arbitrary identity (test fixtures only).
// trace:BUG-2621 | ai:claude
#[cfg(test)]
pub(crate) fn write_test_receipt(root: &Path, identity: ProcIdentity, purpose: &str) -> PathBuf {
    let dir = directory(root);
    std::fs::create_dir_all(&dir).unwrap();
    let row = Receipt {
        version: 1,
        identity,
        purpose: purpose.into(),
        token: uuid::Uuid::new_v4().to_string(),
        acquired_at: None,
    };
    let path = dir.join(format!("{}.json", row.token));
    std::fs::write(&path, serde_json::to_vec(&row).unwrap()).unwrap();
    path
}

/// Acquire before discovery/launch, retain until all owned work has settled.
/// Shared locks admit only legacy controllers; every other caller is exclusive.
pub(crate) fn acquire(root: &Path, purpose: &str, legacy: bool) -> Result<Claim> {
    acquire_mode(
        root,
        purpose,
        if legacy {
            Mode::Legacy
        } else {
            Mode::Exclusive
        },
        Deadline::start(Duration::ZERO),
    )
}

/// The one deadline an admission attempt carries through its preflight, its
/// acquisition retries, its intent registration and cleanup: nothing in it
/// waits past [`Deadline::serial_until`] for the receipt serializer.
// trace:BUG-2703 | ai:claude
#[derive(Clone, Copy)]
struct Deadline {
    started: std::time::Instant,
    bound: Duration,
}
impl Deadline {
    fn start(bound: Duration) -> Self {
        Self {
            started: std::time::Instant::now(),
            bound,
        }
    }
    /// When the wait for holders to yield ends.
    fn until(&self) -> std::time::Instant {
        self.started + self.bound
    }
    /// When a serializer wait ends: the bound, floored at [`SERIAL_GRACE`]
    /// from the start so a zero bound still rides out ordinary contention.
    fn serial_until(&self) -> std::time::Instant {
        self.started + self.bound.max(SERIAL_GRACE)
    }
}

// Explicitly separate manual cooperation from the legacy controller option.
// trace:BUG-2650 | ai:codex
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Exclusive,
    Legacy,
    CooperativeTrain,
}
fn acquire_mode(root: &Path, purpose: &str, mode: Mode, deadline: Deadline) -> Result<Claim> {
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    // trace:BUG-2703 | ai:claude
    let _serial = lock_serial(&dir, deadline.serial_until())?;
    let rows = receipts(&dir)?;
    // Serialize trains from discovery onward, even before run.lock exists.
    // Review controllers alone may coexist with a manual train.
    // trace:BUG-2650 | ai:codex
    if (mode == Mode::CooperativeTrain || (mode == Mode::Legacy && purpose == "legacy auto-train"))
        && rows.iter().any(|r| {
            matches!(
                r.purpose.as_str(),
                "legacy auto-train" | "cooperative manual train"
            )
        })
    {
        return Err(busy(&rows));
    }
    // Writer preference: a waiting land/ship (or manual train) stops new
    // shared claims that would keep it waiting, and an audited drain stops
    // every loop controller's claim. Unreadable evidence refuses.
    // trace:BUG-2703 | ai:claude
    if mode == Mode::Legacy {
        if let Some(drain) = crate::admission_drain::active(root)? {
            return Err(AdmissionBusy(drain.to_string()).into());
        }
    }
    if let Some(intent) = intents(&dir, Utc::now())?
        .iter()
        .find(|intent| yields_to(mode, purpose, intent.kind))
    {
        return Err(AdmissionBusy(intent.describe()).into());
    }
    let lock = open(&dir.join("mode.lock"))?;
    let result = if mode != Mode::Exclusive {
        FileExt::try_lock_shared(&lock)
    } else {
        lock.try_lock_exclusive()
    };
    if let Err(error) = result {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Err(busy(&rows));
        }
        return Err(error.into());
    }
    // A dead controller may have surviving children. Unknown is not quiescent.
    if mode == Mode::Exclusive {
        crate::review_watch::admission_quiescent(root)?;
    }
    let book = crate::land::run::Book::read(root)?;
    let children: Vec<_> = book
        .children
        .iter()
        .filter(|c| crate::land::run::alive(c))
        .map(|c| Receipt {
            version: 1,
            identity: c.identity.clone(),
            purpose: format!("prior land {:?}", c.kind),
            token: String::new(),
            acquired_at: None,
        })
        .collect();
    if !children.is_empty() {
        return Err(busy(&children));
    }
    // A joining legacy controller may share the current auto-train's work.
    // Unknown/orphan trains still refuse, including when only a watcher remains.
    // trace:STORY-2857 | ai:codex
    // A review controller likewise joins a live manual cooperative train: it
    // gives its claim up to a waiting ship or a drain, and must be able to
    // take it back while that train still runs (the BUG-2650 coexistence).
    // The auto-train never reaches here beside a manual train: trains were
    // serialized above. trace:BUG-2703 | ai:claude
    let joining_train = mode == Mode::Legacy
        && rows.iter().any(|r| {
            matches!(
                r.purpose.as_str(),
                "legacy auto-train" | "cooperative manual train"
            ) && matches!(
                aida_core::proc_identity::observe(&r.identity, true),
                Verdict::Alive
            )
        });
    if !joining_train && crate::pr_train::admission_train_running(root)? {
        return Err(AdmissionBusy(
            "prior train is not quiescent; holder identity unknown (inspect pr train --status)"
                .into(),
        )
        .into());
    }
    let row = Receipt {
        version: 1,
        identity: ProcIdentity::capture(std::process::id()).map_err(anyhow::Error::msg)?,
        purpose: purpose.into(),
        token: uuid::Uuid::new_v4().to_string(),
        acquired_at: Some(Utc::now()),
    };
    let receipt = dir.join(format!("{}.json", row.token));
    // trace:BUG-2703 | ai:claude
    if let Err(error) = write_atomic(&receipt, &serde_json::to_vec(&row)?) {
        let _ = FileExt::unlock(&lock);
        return Err(error.context("write the admission holder receipt"));
    }
    Ok(Claim { lock, receipt })
}

/// Only a land controller's direct train child gets its cooperative handoff.
/// This conveys no grant or merge authority.
pub(crate) fn train(root: &Path, automatic: bool) -> Result<Option<Claim>> {
    train_mode(root, automatic, Mode::Exclusive, Duration::ZERO)
}

// Manual merge trains alone opt into member-scoped review coexistence. Direct
// ship retains its existing exclusive/quiescent admission policy.
// trace:BUG-2650 | ai:codex
pub(crate) fn cooperative_train(root: &Path, automatic: bool) -> Result<Option<Claim>> {
    train_mode(root, automatic, Mode::CooperativeTrain, Duration::ZERO)
}

/// `aida pr ship`'s admission: [`train`], waiting up to the configured bound
/// (`[admission] wait_secs`) while shared holders yield.
// trace:BUG-2703 | ai:claude
pub(crate) fn ship_admission(root: &Path) -> Result<Option<Claim>> {
    train_mode(root, false, Mode::Exclusive, wait_bound(root)?)
}

/// A manual `aida pr train`'s admission: [`cooperative_train`], waiting up
/// to the configured bound while the auto-train yields between its passes.
/// The auto-train's own trains never wait: they run on their own claim.
// trace:BUG-2703 | ai:claude
pub(crate) fn manual_train_admission(root: &Path, automatic: bool) -> Result<Option<Claim>> {
    let bound = if automatic {
        Duration::ZERO
    } else {
        wait_bound(root)?
    };
    train_mode(root, automatic, Mode::CooperativeTrain, bound)
}

/// `aida land run`'s exclusive admission, waiting up to the configured bound.
// trace:BUG-2703 | ai:claude
pub(crate) fn land_admission(root: &Path) -> Result<Claim> {
    acquire_waiting(
        root,
        "land run",
        Mode::Exclusive,
        Deadline::start(wait_bound(root)?),
    )
}

fn train_mode(root: &Path, automatic: bool, mode: Mode, bound: Duration) -> Result<Option<Claim>> {
    // One deadline from here on: the handoff preflight below shares it with
    // the acquisition, so a stalled serializer holder cannot stretch either.
    // trace:BUG-2703 | ai:claude
    let deadline = Deadline::start(bound);
    let dir = directory(root);
    if dir.exists() {
        let _serial =
            lock_serial(&dir, deadline.serial_until()).map_err(|error| gave_up(error, deadline))?;
        let rows = receipts(&dir)?;
        let parent = unsafe { libc::getppid() } as u32;
        if rows.iter().any(|r| {
            matches!(
                aida_core::proc_identity::observe(&r.identity, true),
                Verdict::Alive
            ) && ((automatic
                && r.identity.pid == std::process::id()
                && r.purpose == "legacy auto-train")
                || (r.identity.pid == parent && r.purpose == "land run"))
        }) {
            return Ok(None);
        }
    }
    let purpose = if mode == Mode::CooperativeTrain {
        "cooperative manual train"
    } else {
        "manual train/ship"
    };
    acquire_waiting(root, purpose, mode, deadline).map(Some)
}

/// Default and ceiling for `[admission] wait_secs`.
// trace:BUG-2703 | ai:claude
const DEFAULT_WAIT_SECS: u64 = 180;
const MAX_WAIT_SECS: u64 = 3600;
/// Slack on an intent's expiry beyond its waiter's bound, so a waiter that is
/// between retries never loses its place.
const INTENT_SLACK_SECS: i64 = 30;
const INTENT_EXTENSION: &str = "intent";
/// How long an intent that cannot be parsed is left in place before it is
/// removed. It is ignored from the first read; the grace only keeps a reader
/// from deleting a file an older binary is still writing in place.
// trace:BUG-2703 | ai:claude
const UNPARSABLE_INTENT_GRACE: Duration = Duration::from_secs(30);

/// How long a land, ship or manual train waits for admission:
/// `[admission] wait_secs`, default 180, 0 refuses at once. An unreadable or
/// invalid setting refuses rather than guessing.
// trace:BUG-2703 | ai:claude
pub(crate) fn wait_bound(root: &Path) -> Result<Duration> {
    match crate::review_helpers::project_config_text(root) {
        Ok(text) => parse_wait_bound(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Duration::from_secs(DEFAULT_WAIT_SECS))
        }
        Err(error) => Err(error).context("read .aida/config.toml for `[admission] wait_secs`"),
    }
}

fn parse_wait_bound(text: &str) -> Result<Duration> {
    let config: toml::Value =
        toml::from_str(text).context("parse .aida/config.toml for `[admission] wait_secs`")?;
    let Some(value) = config.get("admission").and_then(|a| a.get("wait_secs")) else {
        return Ok(Duration::from_secs(DEFAULT_WAIT_SECS));
    };
    let seconds = value
        .as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n <= MAX_WAIT_SECS)
        .with_context(|| {
            format!("`[admission] wait_secs` must be an integer from 0 to {MAX_WAIT_SECS}")
        })?;
    Ok(Duration::from_secs(seconds))
}

/// What a waiting requester asks shared holders to give up.
// trace:BUG-2703 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IntentKind {
    /// A land or ship: every shared claim yields.
    Exclusive,
    /// A manual train: only the auto-train yields (trains serialize).
    Train,
}

/// A live requester waiting for admission. Never a claim: it grants nothing,
/// it only stops new shared claims that would keep its requester waiting.
// trace:BUG-2703 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    identity: ProcIdentity,
    purpose: String,
    kind: IntentKind,
    token: String,
    registered_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}
impl Intent {
    fn describe(&self) -> String {
        format!(
            "{} admission requested by PID {} (purpose {}), waiting since {}",
            match self.kind {
                IntentKind::Exclusive => "exclusive",
                IntentKind::Train => "train",
            },
            self.identity.pid,
            self.purpose,
            self.registered_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
    }
}

/// Whether a `mode` acquisition for `purpose` must defer to an intent.
/// Exclusive requesters never defer; review controllers coexist with a
/// manual train; the auto-train defers to any waiting land, ship or train.
// trace:BUG-2703 | ai:claude
fn yields_to(mode: Mode, purpose: &str, kind: IntentKind) -> bool {
    match mode {
        Mode::Exclusive => false,
        Mode::CooperativeTrain => kind == IntentKind::Exclusive,
        Mode::Legacy => kind == IntentKind::Exclusive || purpose == "legacy auto-train",
    }
}

/// A well-formed intent, or `None` for an empty, truncated, malformed or
/// invalid one (an unsupported version, or an expiry past any bound).
// trace:BUG-2703 | ai:claude
fn parse_intent(body: &str) -> Option<Intent> {
    let row: Intent = serde_json::from_str(body).ok()?;
    (row.version == 1
        && row.expires_at
            <= row.registered_at
                + chrono::Duration::seconds(MAX_WAIT_SECS as i64 + INTENT_SLACK_SECS))
    .then_some(row)
}

/// Live, unexpired intents. Callers hold receipts.lock. Dead or expired
/// intents are removed; an unknown liveness keeps the intent (it expires).
/// An intent that does not parse is stale, never a wedge: it has no expiry
/// and no requester to outlive, so it is ignored at once and removed after
/// [`UNPARSABLE_INTENT_GRACE`]. Intents are written atomically, so a live
/// requester's intent always parses; an intent grants nothing, so ignoring
/// one can only cost a waiter its preference, never admit anyone.
// trace:BUG-2703 | ai:claude
fn intents(dir: &Path, now: DateTime<Utc>) -> Result<Vec<Intent>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != INTENT_EXTENSION) {
            continue;
        }
        // An intent its requester's bounded cleanup removed mid-scan is gone.
        let body =
            match aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, 16384) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                body => body.with_context(|| {
                    format!("admission intent {} is unreadable", path.display())
                })?,
            };
        let Some(row) = parse_intent(&body) else {
            let aged = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .is_ok_and(|at| at.elapsed().is_ok_and(|age| age >= UNPARSABLE_INTENT_GRACE));
            if aged && std::fs::remove_file(&path).is_ok() {
                eprintln!("admission: discarded unparsable intent {}", path.display());
            }
            continue;
        };
        if row.expires_at <= now
            || matches!(
                aida_core::proc_identity::observe(&row.identity, true),
                Verdict::Dead(_)
            )
        {
            remove_stale(&path)?;
        } else {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Removes its intent once the requester holds admission or gives up. The
/// serializer wait is bounded; past it the intent file is removed anyway
/// (readers skip a vanished intent), so a stalled serializer holder cannot
/// hold the requester in its own cleanup. trace:BUG-2703 | ai:claude
struct IntentGuard(PathBuf);
impl Drop for IntentGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _serial = lock_serial(dir, std::time::Instant::now() + CLEANUP_GRACE);
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

fn register_intent(
    root: &Path,
    purpose: &str,
    kind: IntentKind,
    deadline: Deadline,
) -> Result<IntentGuard> {
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    // trace:BUG-2703 | ai:claude
    let _serial = lock_serial(&dir, deadline.serial_until())?;
    let remaining = deadline
        .until()
        .saturating_duration_since(std::time::Instant::now());
    let now = Utc::now();
    let row = Intent {
        version: 1,
        identity: ProcIdentity::capture(std::process::id()).map_err(anyhow::Error::msg)?,
        purpose: purpose.into(),
        kind,
        token: uuid::Uuid::new_v4().to_string(),
        registered_at: now,
        expires_at: now + chrono::Duration::seconds(remaining.as_secs() as i64 + INTENT_SLACK_SECS),
    };
    let path = dir.join(format!("{}.{INTENT_EXTENSION}", row.token));
    // Atomic, and nothing is left behind when the write fails: the guard
    // exists only for an intent that is fully written.
    write_atomic(&path, &serde_json::to_vec(&row)?).context("write the admission intent")?;
    Ok(IntentGuard(path))
}

/// Acquire `mode`, retrying an [`AdmissionBusy`] refusal once a second for up
/// to `bound`. While it waits it registers an intent, so loop controllers
/// yield their shared claims at their next tick boundary and no new shared
/// claim starts ahead of it. Running reviews and trains are never cut short:
/// the quiescence and train checks still refuse until they finish. Giving up
/// names the holders and the time waited. Any other error refuses at once.
// trace:BUG-2703 | ai:claude
// Every serializer wait inside (preflight, acquisition, intent registration
// and cleanup) ends by `deadline`, so a process stalled while holding
// receipts.lock cannot hold the requester past its bound either.
// trace:BUG-2703 | ai:claude
fn acquire_waiting(root: &Path, purpose: &str, mode: Mode, deadline: Deadline) -> Result<Claim> {
    let kind = if mode == Mode::Exclusive {
        IntentKind::Exclusive
    } else {
        IntentKind::Train
    };
    let mut intent: Option<IntentGuard> = None;
    loop {
        let error = match acquire_mode(root, purpose, mode, deadline) {
            Ok(claim) => return Ok(claim),
            Err(error) => error,
        };
        let Some(busy) = error.downcast_ref::<AdmissionBusy>() else {
            return Err(error);
        };
        let now = std::time::Instant::now();
        if now >= deadline.until() {
            return Err(gave_up(error, deadline));
        }
        if intent.is_none() {
            eprintln!(
                "admission busy for {purpose} ({}); waiting up to {}s for holders to yield",
                busy.0,
                deadline.bound.as_secs()
            );
            intent = Some(
                register_intent(root, purpose, kind, deadline)
                    .map_err(|error| gave_up(error, deadline))?,
            );
        }
        std::thread::sleep((deadline.until() - now).min(Duration::from_secs(1)));
    }
}

/// A busy refusal at the end of a bounded wait names the holders and how
/// long it waited; a zero bound keeps the original holder evidence. Other
/// errors pass through unchanged.
// trace:BUG-2703 | ai:claude
fn gave_up(error: anyhow::Error, deadline: Deadline) -> anyhow::Error {
    let Some(busy) = error.downcast_ref::<AdmissionBusy>() else {
        return error;
    };
    if deadline.bound.is_zero() {
        return error;
    }
    AdmissionBusy(format!(
        "{}; gave up after waiting {}s (bound {}s, `[admission] wait_secs`)",
        busy.0,
        deadline.started.elapsed().as_secs(),
        deadline.bound.as_secs()
    ))
    .into()
}

/// Why a loop controller holding (or about to take) a shared claim for
/// `purpose` must give it up at its next tick boundary: an active drain, or
/// a waiting requester it defers to. Unreadable evidence is a reason too.
// trace:BUG-2703 | ai:claude
pub(crate) fn yield_reason(root: &Path, purpose: &str) -> Option<String> {
    let inner = || -> Result<Option<String>> {
        if let Some(drain) = crate::admission_drain::active(root)? {
            return Ok(Some(drain.to_string()));
        }
        let dir = directory(root);
        if !dir.is_dir() {
            return Ok(None);
        }
        // Bounded: a stalled serializer holder makes the controller yield
        // (fail closed), never hang between passes. trace:BUG-2703 | ai:claude
        let _serial = lock_serial(&dir, std::time::Instant::now() + SERIAL_GRACE)?;
        Ok(intents(&dir, Utc::now())?
            .iter()
            .find(|intent| yields_to(Mode::Legacy, purpose, intent.kind))
            .map(Intent::describe))
    };
    match inner() {
        Ok(reason) => reason,
        Err(error) => Some(format!("admission evidence unreadable: {error:#}")),
    }
}

/// One admission claim or waiting intent, for status display. Read-only.
// trace:BUG-2703 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HolderView {
    pub(crate) pid: u32,
    pub(crate) purpose: String,
    /// `claim` or `waiting`.
    pub(crate) state: &'static str,
    pub(crate) since: Option<DateTime<Utc>>,
}

/// Every claim receipt and waiting intent, as recorded. Read-only and
/// lock-free: never unlinks, and a row it cannot parse (a leftover from an
/// interrupted write) is skipped, so one bad file never hides the rest.
// trace:BUG-2703 | ai:claude
pub(crate) fn holders(root: &Path) -> Result<Vec<HolderView>> {
    let dir = directory(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "json" && ext != INTENT_EXTENSION {
            continue;
        }
        // Lock-free: a claim dropped mid-scan is simply gone.
        let body =
            match aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, 16384) {
                Ok(body) => body,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
        if ext == "json" {
            let Ok(row) = serde_json::from_str::<Receipt>(&body) else {
                continue;
            };
            if !matches!(
                aida_core::proc_identity::observe(&row.identity, true),
                Verdict::Dead(_)
            ) {
                rows.push(HolderView {
                    pid: row.identity.pid,
                    purpose: row.purpose,
                    state: "claim",
                    since: row.acquired_at,
                });
            }
        } else {
            let Some(row) = parse_intent(&body) else {
                continue;
            };
            if row.expires_at > Utc::now()
                && !matches!(
                    aida_core::proc_identity::observe(&row.identity, true),
                    Verdict::Dead(_)
                )
            {
                rows.push(HolderView {
                    pid: row.identity.pid,
                    purpose: row.purpose,
                    state: "waiting",
                    since: Some(row.registered_at),
                });
            }
        }
    }
    rows.sort_by(|a, b| a.since.cmp(&b.since));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_files(root: &Path) -> usize {
        std::fs::read_dir(directory(root))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == INTENT_EXTENSION)
            })
            .count()
    }

    /// A waiting exclusive requester stops new shared claims (writer
    /// preference), a holder sees why it must yield, and once it does the
    /// requester gets in within its bound; its intent is then gone.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac6ca3ea | ai:claude
    #[test]
    fn bug2703_waiting_exclusive_gets_in_once_shared_holders_yield() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let mut watcher = Some(acquire(&root, "legacy review-watch", true).unwrap());
        assert_eq!(yield_reason(&root, "legacy review-watch"), None);
        let waiter = {
            let root = root.clone();
            std::thread::spawn(move || {
                acquire_waiting(
                    &root,
                    "land run",
                    Mode::Exclusive,
                    Deadline::start(Duration::from_secs(20)),
                )
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let reason = loop {
            if let Some(reason) = yield_reason(&root, "legacy review-watch") {
                break reason;
            }
            assert!(std::time::Instant::now() < deadline, "no intent registered");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            reason.contains("exclusive admission requested by PID"),
            "{reason}"
        );
        assert!(reason.contains("purpose land run"), "{reason}");
        // No new shared claim starts ahead of the waiting requester.
        for (purpose, legacy) in [("legacy review-watch", true), ("legacy auto-train", true)] {
            let error = acquire(&root, purpose, legacy).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
            assert!(error.to_string().contains("purpose land run"), "{error}");
        }
        assert!(cooperative_train(&root, false).is_err());
        // The holder yields at its boundary; the requester then gets in.
        watcher.take();
        let land = waiter.join().unwrap().expect("exclusive within the bound");
        assert_eq!(intent_files(&root), 0, "the intent is removed");
        assert!(acquire(&root, "legacy review-watch", true).is_err());
        drop(land);
        assert!(acquire(&root, "legacy review-watch", true).is_ok());
    }

    /// A requester whose holders never yield gives up at its bound, naming
    /// the holders and the time it waited; its intent does not linger.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac336622 | ai:claude
    #[test]
    fn bug2703_bounded_wait_gives_up_naming_holders_and_wait() {
        let temp = tempfile::tempdir().unwrap();
        let _watcher = acquire(temp.path(), "legacy review-watch", true).unwrap();
        let started = std::time::Instant::now();
        let error = acquire_waiting(
            temp.path(),
            "manual train/ship",
            Mode::Exclusive,
            Deadline::start(Duration::from_secs(2)),
        )
        .err()
        .unwrap();
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        let text = error.to_string();
        assert!(text.contains("purpose legacy review-watch"), "{text}");
        // The seconds waited depend on the runner's load; the bound does not.
        assert!(text.contains("gave up after waiting "), "{text}");
        assert!(text.contains("s (bound 2s, "), "{text}");
        assert_eq!(intent_files(temp.path()), 0);
        // A zero bound refuses at once with the original holder evidence.
        let error = acquire_waiting(
            temp.path(),
            "manual train/ship",
            Mode::Exclusive,
            Deadline::start(Duration::ZERO),
        )
        .err()
        .unwrap();
        assert!(!error.to_string().contains("gave up"));
        assert!(error.to_string().contains("purpose legacy review-watch"));
    }

    /// A waiting manual train asks only the auto-train to yield; review
    /// controllers keep their claim and coexist with it.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac6ca3ea | ai:claude
    #[test]
    fn bug2703_manual_train_waits_for_the_auto_train_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let _watcher = acquire(&root, "legacy review-watch", true).unwrap();
        let mut worker = Some(acquire(&root, "legacy auto-train", true).unwrap());
        let waiter = {
            let root = root.clone();
            std::thread::spawn(move || {
                acquire_waiting(
                    &root,
                    "cooperative manual train",
                    Mode::CooperativeTrain,
                    Deadline::start(Duration::from_secs(20)),
                )
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while yield_reason(&root, "legacy auto-train").is_none() {
            assert!(std::time::Instant::now() < deadline, "no train intent");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(yield_reason(&root, "legacy review-watch"), None);
        assert!(acquire(&root, "legacy review-watch", true).is_ok());
        worker.take();
        let train = waiter
            .join()
            .unwrap()
            .expect("manual train within the bound");
        assert!(acquire(&root, "legacy auto-train", true).is_err());
        drop(train);
    }

    /// An active drain withholds every loop controller's claim and tells
    /// holders to yield; it never blocks a land or ship.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_drain_withholds_loop_claims_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let watcher = acquire(root, "legacy review-watch", true).unwrap();
        crate::admission_drain::fixture_drain(root, "joe (uid 1)", "land #4324", 600, Utc::now())
            .unwrap();
        for purpose in ["legacy review-watch", "legacy auto-train"] {
            let reason = yield_reason(root, purpose).unwrap();
            assert!(reason.contains("drained by joe (uid 1)"), "{reason}");
            let error = acquire(root, purpose, true).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        }
        drop(watcher);
        assert!(acquire(root, "land run", false).is_ok());
    }

    /// Expired or dead intents are discarded. An intent that does not parse
    /// (zero-length, truncated, or claiming to outlive any bound) is stale:
    /// it blocks no admission and makes no holder yield, and it is removed
    /// once it is older than the grace (review round 3, finding 2).
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac6ca3ea | ai:claude
    #[test]
    fn bug2703_stale_intents_expire_and_unparsable_intents_never_wedge() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let guard = register_intent(
            root,
            "land run",
            IntentKind::Exclusive,
            Deadline::start(Duration::ZERO),
        )
        .unwrap();
        let mut row: Intent = serde_json::from_slice(&std::fs::read(&guard.0).unwrap()).unwrap();
        row.registered_at = Utc::now() - chrono::Duration::seconds(120);
        row.expires_at = Utc::now() - chrono::Duration::seconds(1);
        std::fs::write(&guard.0, serde_json::to_vec(&row).unwrap()).unwrap();
        assert!(acquire(root, "legacy review-watch", true).is_ok());
        assert!(!guard.0.exists(), "an expired intent is removed");
        let mut dead = row.clone();
        dead.identity.boot_id = uuid::Uuid::new_v4().to_string();
        dead.expires_at = Utc::now() + chrono::Duration::seconds(60);
        dead.registered_at = Utc::now();
        std::fs::write(&guard.0, serde_json::to_vec(&dead).unwrap()).unwrap();
        assert_eq!(yield_reason(root, "legacy auto-train"), None);
        assert!(!guard.0.exists(), "a dead requester's intent is removed");
        // What an interrupted in-place write left behind, and an intent
        // claiming to outlive any bound: none is trusted, none wedges.
        let mut forever = row;
        forever.registered_at = Utc::now();
        forever.expires_at = Utc::now() + chrono::Duration::days(30);
        let forever = serde_json::to_vec(&forever).unwrap();
        let bodies: [&[u8]; 4] = [b"", b"{broken", &forever[..forever.len() / 2], &forever];
        for body in bodies {
            std::fs::write(&guard.0, body).unwrap();
            assert_eq!(yield_reason(root, "legacy auto-train"), None);
            assert_eq!(yield_reason(root, "legacy review-watch"), None);
            for purpose in ["legacy auto-train", "legacy review-watch"] {
                assert!(acquire(root, purpose, true).is_ok(), "{purpose}");
            }
            assert!(ship_admission(root).unwrap().is_some());
            assert!(cooperative_train(root, false).unwrap().is_some());
            drop(land_admission(root).unwrap());
            assert!(holders(root).unwrap().is_empty());
            assert!(guard.0.exists(), "kept within the grace");
            // Once older than the grace it is removed by the next reader.
            std::fs::File::options()
                .write(true)
                .open(&guard.0)
                .unwrap()
                .set_modified(
                    std::time::SystemTime::now() - UNPARSABLE_INTENT_GRACE - Duration::from_secs(1),
                )
                .unwrap();
            assert_eq!(yield_reason(root, "legacy auto-train"), None);
            assert!(!guard.0.exists(), "an aged unparsable intent is removed");
        }
        drop(guard);
        assert!(acquire(root, "legacy auto-train", true).is_ok());
    }

    /// Review round 3, finding 2: an intent is written by rename, never in
    /// place, and a failed write leaves no `.intent` behind to wedge anyone.
    // trace:BUG-2703 | ai:claude
    #[test]
    fn bug2703_intent_and_receipt_writes_leave_no_partial_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let guard = register_intent(
            root,
            "land run",
            IntentKind::Exclusive,
            Deadline::start(Duration::from_secs(5)),
        )
        .unwrap();
        assert!(parse_intent(&std::fs::read_to_string(&guard.0).unwrap()).is_some());
        drop(guard);
        let claim = acquire(root, "legacy review-watch", true).unwrap();
        drop(claim);
        // Nothing but the lock files remains: no temporary, no record.
        let mut left: Vec<String> = std::fs::read_dir(directory(root))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["mode.lock", "receipts.lock"]);
        // A write that cannot complete (its target is a directory) fails
        // and leaves neither the record nor a temporary behind.
        let blocked = directory(root).join("blocked.intent");
        std::fs::create_dir(&blocked).unwrap();
        assert!(write_atomic(&blocked, b"{}").is_err());
        std::fs::remove_dir(&blocked).unwrap();
        assert_eq!(intent_files(root), 0);
        assert_eq!(std::fs::read_dir(directory(root)).unwrap().count(), 2);
    }

    /// Review round 3, finding 4: a dead holder's receipt or an expired
    /// intent that its owner's own cleanup removes first is not an error.
    // trace:BUG-2703 | ai:claude
    #[test]
    fn bug2703_racing_cleanup_of_a_stale_record_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let gone = temp.path().join("gone.json");
        remove_stale(&gone).expect("already removed is success");
        std::fs::write(&gone, b"x").unwrap();
        remove_stale(&gone).unwrap();
        assert!(!gone.exists());
        // Any other failure is still reported.
        let dir = temp.path().join("dir.json");
        std::fs::create_dir(&dir).unwrap();
        assert!(remove_stale(&dir).is_err());
    }

    /// Review round 3, finding 5: the lock-free status listing skips a row
    /// it cannot parse (an empty file from an interrupted write) and still
    /// lists every other claim and waiter.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_holders_skips_an_unparsable_row() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _watcher = acquire(root, "legacy review-watch", true).unwrap();
        let dir = directory(root);
        std::fs::write(dir.join("empty.json"), b"").unwrap();
        std::fs::write(dir.join("cut.json"), b"{\"version\":1,").unwrap();
        std::fs::write(dir.join("empty.intent"), b"").unwrap();
        let rows = holders(root).expect("one bad row does not fail the listing");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].purpose, "legacy review-watch");
        assert_eq!(rows[0].state, "claim");
    }

    /// Review round 3, finding 3: a review controller that gave its claim up
    /// takes it back while a manual cooperative train is still running, and
    /// never beside a train whose owner is gone.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac6ca3ea | ai:claude
    #[test]
    fn bug2703_review_watch_rejoins_a_running_manual_train() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let watcher = acquire(root, "legacy review-watch", true).unwrap();
        let train = cooperative_train(root, false).unwrap().unwrap();
        let run_dir = root.join(".aida/train");
        std::fs::create_dir_all(&run_dir).unwrap();
        let run = open(&run_dir.join("run.lock")).unwrap();
        run.lock_exclusive().unwrap();
        assert!(crate::pr_train::admission_train_running(root).unwrap());
        drop(watcher);
        let watcher = acquire(root, "legacy review-watch", true)
            .expect("the watcher rejoins the live manual train");
        // Trains still serialize, and the exclusive mode is still refused.
        assert!(acquire(root, "legacy auto-train", true).is_err());
        assert!(acquire(root, "land run", false).is_err());
        drop(watcher);
        // An orphan run (its train's claim is gone) is still refused.
        drop(train);
        let error = acquire(root, "legacy review-watch", true).err().unwrap();
        assert!(error.to_string().contains("not quiescent"), "{error}");
        FileExt::unlock(&run).unwrap();
        assert!(acquire(root, "legacy review-watch", true).is_ok());
    }

    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac336622 | ai:claude
    #[test]
    fn bug2703_wait_bound_config() {
        assert_eq!(parse_wait_bound("").unwrap(), Duration::from_secs(180));
        assert_eq!(
            parse_wait_bound("[admission]\nwait_secs = 0\n").unwrap(),
            Duration::ZERO
        );
        assert_eq!(
            parse_wait_bound("[admission]\nwait_secs = 600 # ten minutes\n").unwrap(),
            Duration::from_secs(600)
        );
        for bad in ["-1", "3601", "\"5m\"", "true"] {
            assert!(parse_wait_bound(&format!("[admission]\nwait_secs = {bad}\n")).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(wait_bound(temp.path()).unwrap(), Duration::from_secs(180));
    }

    /// Child fixture: a separate process that takes the receipt serializer,
    /// says so, and then stalls holding it. A no-op unless the parent test
    /// below launches it.
    // trace:BUG-2703 | ai:claude
    #[test]
    fn bug2703_serializer_holder_child() {
        let inputs = aida_core::exec_context::Environment::capture();
        let Some(dir) = inputs.get("AIDA_TEST_SERIALIZER_HOLD_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let serial = open(&dir.join("receipts.lock")).unwrap();
        serial.lock_exclusive().unwrap();
        std::fs::write(dir.join("serializer-held"), std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    /// Kills the stalled serializer holder when a test ends, pass or fail.
    struct StalledHolder(std::process::Child);
    impl Drop for StalledHolder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn stall_serializer(dir: &Path) -> StalledHolder {
        let marker = dir.join("serializer-held");
        let _ = std::fs::remove_file(&marker);
        let child = std::process::Command::new(crate::aida_bin::current_executable_path().unwrap())
            .args([
                "--exact",
                "admission_owner::tests::bug2703_serializer_holder_child",
                "--nocapture",
            ])
            .env("AIDA_TEST_SERIALIZER_HOLD_DIR", dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let holder = StalledHolder(child);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the holder never took the serializer"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        holder
    }

    /// Review finding 1 (round 2): a separate process stalled while holding
    /// receipts.lock cannot hold `pr ship`'s admission past its bound, in
    /// the preflight, the acquisition or the intent cleanup. The refusal is
    /// typed busy, names the serializer's holder and the wait; a zero bound
    /// still returns within the serializer grace; loop controllers neither
    /// hang nor take a claim (they fail closed and yield).
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac336622 | ai:claude
    #[test]
    fn bug2703_stalled_serializer_holder_cannot_outlast_the_bound() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let dir = directory(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[admission]\nwait_secs = 1\n",
        )
        .unwrap();

        // Preflight + acquisition: the serializer is stalled from the start.
        let holder = stall_serializer(&dir);
        let pid = holder.0.id();
        let started = std::time::Instant::now();
        let error = ship_admission(root)
            .err()
            .expect("ship refuses at its bound");
        let waited = started.elapsed();
        // The holder stalls for 30s: any return well short of that is the
        // bound at work. The margins allow for a loaded runner.
        let prompt = Duration::from_secs(15);
        assert!(waited >= Duration::from_secs(1), "{waited:?}");
        assert!(waited < prompt, "{waited:?}");
        assert!(error.downcast_ref::<AdmissionBusy>().is_some(), "{error:#}");
        let text = error.to_string();
        assert!(text.contains("receipt serializer is held by"), "{text}");
        // The holder's PID comes from /proc/locks, best effort: where the
        // directory's device differs from the one listed there (a btrfs
        // subvolume, an overlay) it is reported as unknown. The parser is
        // asserted exactly in bug2703_flock_holders_parses_proc_locks.
        let names_holder = |text: &str| {
            text.contains(&format!("PID {pid}")) || text.contains("a process whose PID is unknown")
        };
        assert!(names_holder(&text), "{text}");
        assert!(text.contains("gave up after waiting "), "{text}");
        assert!(text.contains("s (bound 1s, "), "{text}");
        // A zero bound refuses within the serializer grace, naming the holder.
        std::fs::write(
            root.join(".aida/config.toml"),
            "[admission]\nwait_secs = 0\n",
        )
        .unwrap();
        let started = std::time::Instant::now();
        let error = ship_admission(root).err().unwrap();
        assert!(started.elapsed() < prompt, "{:?}", started.elapsed());
        let text = error.to_string();
        assert!(text.contains("receipt serializer is held by"), "{text}");
        assert!(names_holder(&text), "{text}");
        // Loop controllers neither hang nor claim: they fail closed.
        let started = std::time::Instant::now();
        assert!(acquire(root, "legacy review-watch", true).is_err());
        let reason = yield_reason(root, "legacy review-watch").expect("yields");
        assert!(reason.contains("serializer is held"), "{reason}");
        assert!(started.elapsed() < prompt, "{:?}", started.elapsed());
        drop(holder);

        // Intent registration and cleanup: the ship is already waiting (its
        // intent registered) when the serializer stalls.
        std::fs::write(
            root.join(".aida/config.toml"),
            "[admission]\nwait_secs = 3\n",
        )
        .unwrap();
        let watcher = acquire(root, "legacy review-watch", true).unwrap();
        let ship = {
            let root = root.to_path_buf();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                (
                    ship_admission(&root).map(|c| c.is_some()),
                    started.elapsed(),
                )
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while intent_files(root) == 0 {
            assert!(std::time::Instant::now() < deadline, "no intent registered");
            std::thread::sleep(Duration::from_millis(20));
        }
        let holder = stall_serializer(&dir);
        let (result, waited) = ship.join().unwrap();
        let error = result.err().expect("ship gives up at its bound");
        assert!(waited < prompt, "{waited:?}");
        assert!(
            error.to_string().contains("gave up after waiting"),
            "{error}"
        );
        assert_eq!(
            intent_files(root),
            0,
            "the intent is removed despite the stall"
        );
        drop(holder);
        drop(watcher);
        assert!(acquire(root, "legacy review-watch", true).is_ok());
    }

    #[test]
    fn bug2703_flock_holders_parses_proc_locks() {
        let dev: u64 = (0x103 << 8) | 0x05;
        let locks = "1: FLOCK  ADVISORY  WRITE 4242 103:05:77 0 EOF\n\
                     1: -> FLOCK  ADVISORY  WRITE 5151 103:05:77 0 EOF\n\
                     2: POSIX  ADVISORY  WRITE 6161 103:05:77 0 EOF\n\
                     3: FLOCK  ADVISORY  READ 7171 103:05:78 0 EOF\n";
        assert_eq!(flock_holders(locks, dev, 77), vec![4242]);
        assert!(flock_holders(locks, dev, 79).is_empty());
    }
    // trace:BUG-2650 | ai:codex
    #[test]
    fn bug2650_cooperative_trains_serialize_and_refuse_orphan_runs() {
        let root = tempfile::tempdir().unwrap();
        let _watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        assert!(train(root.path(), false).is_err());
        let first = cooperative_train(root.path(), false).unwrap();
        assert!(cooperative_train(root.path(), false).is_err());
        assert!(acquire(root.path(), "land run", false).is_err());
        drop(first);
        assert!(cooperative_train(root.path(), false).is_ok());
        let dir = root.path().join(".aida/train");
        std::fs::create_dir_all(&dir).unwrap();
        let orphan = open(&dir.join("run.lock")).unwrap();
        orphan.lock_exclusive().unwrap();
        assert!(cooperative_train(root.path(), false).is_err());
    }

    // Trains exclude each other before either has a journal; watchers coexist.
    // trace:BUG-2650 | ai:codex
    #[test]
    fn bug2650_legacy_auto_train_serializes_before_journal() {
        let root = tempfile::tempdir().unwrap();
        let manual = cooperative_train(root.path(), false).unwrap();
        assert!(!root.path().join(".aida/train/run.lock").exists());
        assert!(acquire(root.path(), "legacy auto-train", true).is_err());
        assert!(acquire(root.path(), "legacy review-watch", true).is_ok());
        drop(manual);
        let automatic = acquire(root.path(), "legacy auto-train", true).unwrap();
        assert!(cooperative_train(root.path(), false).is_err());
        drop(automatic);
        assert!(cooperative_train(root.path(), false).is_ok());
    }

    // trace:STORY-2857 | ai:codex
    #[test]
    fn story2857_legacy_watcher_joins_active_train_but_not_orphan() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let dir = root.path().join(".aida/train");
        std::fs::create_dir_all(&dir).unwrap();
        let run = open(&dir.join("run.lock")).unwrap();
        run.lock_exclusive().unwrap();
        let original = std::fs::read(&worker.receipt).unwrap();
        let mut unknown: Receipt = serde_json::from_slice(&original).unwrap();
        unknown.identity.pidns_inode = u64::MAX;
        std::fs::write(&worker.receipt, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        std::fs::write(&worker.receipt, original).unwrap();
        let watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        for purpose in ["land run", "manual train/ship"] {
            let error = acquire(root.path(), purpose, false).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        }
        drop(worker);
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        drop(watcher);
        assert!(acquire(root.path(), "legacy review-watch", true).is_err());
        FileExt::unlock(&run).unwrap();
        assert!(acquire(root.path(), "legacy review-watch", true).is_ok());
    }

    // Hold the reader lock across a receipt snapshot and opening every row.
    // Drop must block until that snapshot has been consumed.
    // trace:STORY-2857 | ai:codex
    #[test]
    fn story2857_drop_serializes_with_receipt_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let _watcher = acquire(root.path(), "legacy review-watch", true).unwrap();
        let dir = directory(root.path());
        let serial = open(&dir.join("receipts.lock")).unwrap();
        serial.lock_exclusive().unwrap();
        let paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        let receipt = worker.receipt.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let dropping = std::thread::spawn(move || {
            tx.send(()).unwrap();
            drop(worker);
            done_tx.send(()).unwrap();
        });
        rx.recv().unwrap();
        assert!(
            matches!(
                done_rx.recv_timeout(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "drop must wait for the reader's snapshot lock"
        );
        // A conflicting lock proves the reader excludes receipt deletion.
        let contender = open(&dir.join("receipts.lock")).unwrap();
        assert_eq!(
            contender.try_lock_exclusive().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        for path in paths {
            std::fs::read(path).unwrap();
        }
        assert_eq!(receipts(&dir).unwrap().len(), 2);
        FileExt::unlock(&serial).unwrap();
        dropping.join().unwrap();
        assert!(!receipt.exists());
        let error = acquire(root.path(), "manual train/ship", false)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        assert!(error.to_string().contains("legacy review-watch"));
    }
    #[test]
    fn story2857_modes_and_manual_train_exclude_each_other() {
        let root = tempfile::tempdir().unwrap();
        let worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        let review = acquire(root.path(), "legacy review-watch", true).unwrap();
        for purpose in ["land run", "manual train/ship"] {
            let error = acquire(root.path(), purpose, false).err().unwrap();
            assert!(error.downcast_ref::<AdmissionBusy>().is_some());
            let text = error.to_string();
            assert!(text.contains(&format!("PID {}", std::process::id())));
            assert!(text.contains("start ") && text.contains("purpose legacy auto-train"));
        }
        let error = train(root.path(), false).err().unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        drop(worker);
        assert!(acquire(root.path(), "land run", false).is_err());
        drop(review);
        let land = acquire(root.path(), "land run", false).unwrap();
        assert!(acquire(root.path(), "legacy auto-train", true).is_err());
        drop(land);
        assert!(acquire(root.path(), "manual train/ship", false).is_ok());
    }
    #[test]
    fn story2857_process_contender() {
        // trace:STORY-2857 | ai:codex
        let inputs = aida_core::exec_context::Environment::capture();
        let Some(root) = inputs.get("AIDA_TEST_ADMISSION_ROOT") else {
            return;
        };
        let root = Path::new(root);
        if inputs.get("AIDA_TEST_ADMISSION_HANDOFF").is_some() {
            assert!(train(root, false).unwrap().is_none());
            return;
        }
        let error = if inputs.get("AIDA_TEST_ADMISSION_MANUAL").is_some() {
            crate::pr_train::pr_train_handler(&crate::pr_train::TrainOptions {
                prs: &[42],
                all_ready: false,
                dry_run: false,
                local_ci: None,
                max: 10,
                no_pull: true,
            })
            .err()
            .expect("manual train must refuse")
        } else {
            acquire(root, "land run", false)
                .err()
                .expect("land must refuse")
        };
        assert!(error.downcast_ref::<AdmissionBusy>().is_some(), "{error:#}");
        assert!(error.to_string().contains("purpose legacy auto-train"));
        assert!(error.to_string().contains("start "));
    }
    #[test]
    fn story2857_cross_process_worker_blocks_land_and_manual() {
        let root = tempfile::tempdir().unwrap();
        let _worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        // A real repository with a resolvable default branch: the manual
        // train resolves the landing policy fail-closed before admission
        // (STORY-2905), so an unresolvable branch would refuse first.
        // trace:STORY-2905 | ai:claude
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            let out = crate::test_git::git_command()
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        }
        // The manual train waits a bounded time before refusing (BUG-2703);
        // keep that wait short here. trace:BUG-2703 | ai:claude
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[admission]\nwait_secs = 1\n",
        )
        .unwrap();
        for manual in [false, true] {
            // Resolve the running test image, preserving the exact child fixture.
            // trace:STORY-2857 | ai:codex
            let mut command =
                std::process::Command::new(crate::aida_bin::current_executable_path().unwrap());
            command
                .args([
                    "--exact",
                    "admission_owner::tests::story2857_process_contender",
                    "--nocapture",
                ])
                .env("AIDA_TEST_ADMISSION_ROOT", root.path())
                .current_dir(root.path());
            if manual {
                command.env("AIDA_TEST_ADMISSION_MANUAL", "1");
            }
            let status = command.status().unwrap();
            assert!(status.success());
        }
    }
    #[test]
    fn story2857_admission_does_not_hold_train_run_lock() {
        let root = tempfile::tempdir().unwrap();
        let _worker = acquire(root.path(), "legacy auto-train", true).unwrap();
        // Admission is retained through scans/idle; train journal exclusion is separate.
        assert!(!crate::pr_train::admission_train_running(root.path()).unwrap());
        assert!(train(root.path(), true).unwrap().is_none());
        assert!(!root.path().join(".aida/train/run.lock").exists());
    }
    #[test]
    fn story2857_transition_requires_quiescent_children_and_readable_evidence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aida/cache")).unwrap();
        let path = root.path().join(".aida/cache/land-children.json");
        let book = crate::land::run::Book {
            children: vec![crate::land::run::Child {
                kind: crate::land::run::Kind::Train,
                prs: vec![42],
                head: String::new(),
                spec: None,
                merge_tree: None,
                identity: ProcIdentity::capture(std::process::id()).unwrap(),
                started_at: chrono::Utc::now().to_rfc3339(),
                log: String::new(),
            }],
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
        let error = acquire(root.path(), "legacy auto-train", true)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<AdmissionBusy>().is_some());
        assert!(error.to_string().contains("prior land Train"));
        std::fs::write(&path, b"broken").unwrap();
        assert!(acquire(root.path(), "land run", false).is_err());
        std::fs::remove_file(path).unwrap();
        assert!(acquire(root.path(), "land run", false).is_ok());
    }
    #[test]
    fn story2857_land_hands_admission_to_its_direct_train_child() {
        let root = tempfile::tempdir().unwrap();
        let _land = acquire(root.path(), "land run", false).unwrap();
        // trace:STORY-2857 | ai:codex
        let status =
            std::process::Command::new(crate::aida_bin::current_executable_path().unwrap())
                .args([
                    "--exact",
                    "admission_owner::tests::story2857_process_contender",
                    "--nocapture",
                ])
                .env("AIDA_TEST_ADMISSION_ROOT", root.path())
                .env("AIDA_TEST_ADMISSION_HANDOFF", "1")
                .status()
                .unwrap();
        assert!(status.success());
    }
}
