//! Review discovery and dispatch; supersession and readiness are later slices.
// trace:TASK-1945 trace:TASK-1993 | ai:codex
use anyhow::{Context, Result};
use clap::Args;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[path = "review_watch_forge.rs"]
mod forge;

#[derive(Args, Debug)]
pub struct ReviewWatchArgs {
    /// Discover and dispatch once, then leave admitted reviews running.
    #[arg(long)]
    pub once: bool,
    /// Seconds between discovery passes (default 10).
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=3600), hide_default_value = true)]
    pub interval: u64,
    /// Maximum watcher-launched reviews across instances: one or two (default two).
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2), hide_default_value = true)]
    pub max_concurrent: u8,
    /// Vendor: claude, codex, or agy; omit for ordinary queue-work selection.
    #[arg(long, value_parser = ["claude", "codex", "agy"], hide_possible_values = true)]
    pub vendor: Option<String>,
    #[arg(long, hide = true)]
    launch_reservation: Option<String>,
}

const LIMIT: u64 = 2 * 1024 * 1024;
const STATE: &str = "state.json";
const WATCH_HEAD: &str = "AIDA_REVIEW_WATCH_HEAD";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    pid: u32,
    start: String,
    boot: String,
}
impl Identity {
    fn current() -> Result<Self> {
        Ok(Self {
            pid: std::process::id(),
            start: aida_core::liveness::process_start_identity_exact(std::process::id())
                .context("cannot bind review launch to its kernel process identity")?,
            boot: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Reserved,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    head: String,
    token: String,
    vendor: Option<String>,
    phase: Phase,
    identity: Option<Identity>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    message: aida_core::mailbox::Message,
    sent: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    attempts: Vec<Attempt>,
    // Preserve observed canonical rounds before queue work clears its current verdict.
    #[serde(default)]
    historical_keys: BTreeSet<crate::review_verdict::RecordingKey>,
    notices: BTreeMap<String, Notice>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    repository: String,
    max_concurrent: u8,
    entries: BTreeMap<String, Entry>,
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no pointers and has no preconditions.
    unsafe { libc::geteuid() }
}

/// Pin runtime directories, then open files relative to the pinned handle.
/// In particular a FIFO or symlink can never make a state read block or escape.
struct Directory {
    handle: File,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        let meta = handle.metadata()?;
        anyhow::ensure!(
            meta.is_dir() && meta.uid() == effective_uid() && meta.mode() & 0o022 == 0,
            "unsafe review-watch directory {}",
            path.display()
        );
        Ok(Self { handle })
    }
    // The shared .aida root and canonical history use the repository's ordinary
    // group-writable directory policy. Watch-owned subdirectories stay strict.
    // trace:TASK-1993 | ai:codex
    fn shared(path: &Path) -> Result<Self> {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        Self::existing(path)?.context("shared review runtime directory disappeared")
    }
    fn existing(path: &Path) -> Result<Option<Self>> {
        let handle = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta = handle.metadata()?;
        anyhow::ensure!(
            meta.is_dir() && meta.uid() == effective_uid() && meta.mode() & 0o002 == 0,
            "unsafe review history directory {}",
            path.display()
        );
        Ok(Some(Self { handle }))
    }
    fn path(&self) -> PathBuf {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/proc/self/fd/{}", self.handle.as_raw_fd()))
    }
    fn file(&self, name: &str, create: bool) -> Result<File> {
        self.open_file(name, create, false)
    }
    fn open_file(&self, name: &str, create: bool, history: bool) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(create)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path().join(name))
            .with_context(|| format!("open review-watch runtime file {name}"))?;
        let meta = file.metadata()?;
        anyhow::ensure!(
            meta.is_file()
                && meta.nlink() == 1
                && if history {
                    matches!(meta.mode() & 0o777, 0o600 | 0o640 | 0o644 | 0o660 | 0o664)
                } else {
                    meta.mode() & 0o777 == 0o600
                }
                && meta.uid() == effective_uid(),
            "unsafe review-watch file {name}"
        );
        Ok(file)
    }
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.read_file(name, false)
    }
    fn read_file(&self, name: &str, history: bool) -> Result<Option<Vec<u8>>> {
        let file = match self.open_file(name, false, history) {
            Ok(file) => file,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= LIMIT,
            "review-watch file exceeds size limit"
        );
        Ok(Some(bytes))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(
            bytes.len() as u64 <= LIMIT,
            "review-watch state exceeds size limit"
        );
        let mut temp = tempfile::NamedTempFile::new_in(self.path())?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.path().join(name)).map_err(|e| e.error)?;
        self.handle.sync_all()?;
        Ok(())
    }
    fn lock(&self, name: &str) -> Result<File> {
        let file = self.file(name, true)?;
        file.try_lock_exclusive()
            .with_context(|| format!("review-watch lock busy: {name}"))?;
        Ok(file)
    }
}

struct Store {
    runtime: Directory,
    dir: Directory,
    repository: String,
}
impl Store {
    fn open(root: &Path, repository: &str) -> Result<Self> {
        let runtime = Directory::shared(&root.join(".aida"))?;
        let dir = Directory::open(&runtime.path().join("review-watch"))?;
        Ok(Self {
            runtime,
            dir,
            repository: repository.into(),
        })
    }
    fn load(&self) -> Result<State> {
        let state = match self.dir.read(STATE)? {
            Some(bytes) => serde_json::from_slice::<State>(&bytes)
                .context("invalid review-watch state; inspect .aida/review-watch/state.json")?,
            None => {
                anyhow::ensure!(self.dir.read("initialized")?.is_none(), "review-watch state is missing after initialization; inspect existing reviews before recovery");
                State {
                    version: 1,
                    repository: self.repository.clone(),
                    max_concurrent: 2,
                    entries: BTreeMap::new(),
                }
            }
        };
        validate_state(&state, &self.repository)?;
        Ok(state)
    }
    fn transaction<T>(&self, key: &str, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        parse_key(key)?;
        // Every writer uses this ordering. No forge query, child wait or mail
        // delivery occurs while either lock is held.
        let _global = self.dir.lock("admission.lock")?;
        let _change = self.dir.lock(&format!("{key}.lock"))?;
        let mut state = self.load()?;
        let initialized = self.dir.read("initialized")?.is_some();
        let before = serde_json::to_vec(&state)?;
        let result = f(&mut state)?;
        validate_state(&state, &self.repository)?;
        if before != serde_json::to_vec(&state)? || !initialized {
            self.dir.write(STATE, &serde_json::to_vec_pretty(&state)?)?;
        }
        if !initialized {
            self.dir.file("initialized", true)?.sync_all()?;
            self.dir.handle.sync_all()?;
        }
        Ok(result)
    }
    fn history(&self, key: &str) -> Result<History> {
        let mut history = History {
            keys: self
                .load()?
                .entries
                .get(key)
                .map(|e| e.historical_keys.clone())
                .unwrap_or_default(),
        };
        let Some(dir) = Directory::existing(&self.runtime.path().join("review-verdicts"))? else {
            return Ok(history);
        };
        // GitLab's ordinary standalone collector historically used PR-N too.
        let aliases = if let Some(number) = key.strip_prefix("MR-") {
            vec![key.to_string(), format!("PR-{number}")]
        } else {
            vec![key.to_string()]
        };
        for alias in aliases {
            if let Some(bytes) = dir.read_file(&format!("{alias}.json"), true)? {
                history.add(&bytes)?;
            }
            if history.keys.len() >= 2 {
                return Ok(history);
            }
            if let Some(archive) = Directory::existing(&dir.path().join(&alias))? {
                let mut names = Vec::new();
                for (index, entry) in std::fs::read_dir(archive.path())?.enumerate() {
                    anyhow::ensure!(index < 1000, "review archive exceeds admission scan limit");
                    let name = entry?
                        .file_name()
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("non-UTF8 review archive entry"))?;
                    anyhow::ensure!(
                        names.len() < 1000,
                        "review archive exceeds admission scan limit"
                    );
                    if name.ends_with(".json") {
                        names.push(name);
                    }
                }
                names.sort();
                for name in names {
                    let bytes = archive
                        .read_file(&name, true)?
                        .context("review history disappeared during admission")?;
                    history.add(&bytes)?;
                    if history.keys.len() >= 2 {
                        return Ok(history);
                    }
                }
            }
        }
        Ok(history)
    }
    fn round_for(&self, key: &str, attempt: &Attempt) -> Result<usize> {
        self.transaction(key, |state| {
            let entry = state
                .entries
                .get_mut(key)
                .context("review reservation disappeared")?;
            anyhow::ensure!(
                entry
                    .attempts
                    .iter()
                    .any(|a| a.token == attempt.token && a.head == attempt.head && active(a)),
                "review reservation changed before queue work"
            );
            let history = self.history(key)?;
            anyhow::ensure!(
                !history.contains(&attempt.head),
                "head already has a recorded review; route further work to the advisor"
            );
            let used = history.used(entry);
            anyhow::ensure!(
                used <= 2,
                "two-round review cap reached outside watcher; route further work to the advisor"
            );
            entry.historical_keys.extend(history.keys);
            Ok(used)
        })
    }
    fn transition(&self, key: &str, token: &str, phase: Phase) -> Result<Attempt> {
        let me = Identity::current()?;
        self.transaction(key, |state| {
            let attempt = state
                .entries
                .get_mut(key)
                .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
                .context("unknown review-watch reservation; no review started")?;
            if phase == Phase::Running {
                anyhow::ensure!(
                    attempt.phase == Phase::Reserved && attempt.identity.is_none(),
                    "review-watch reservation already claimed; no duplicate review started"
                );
                attempt.identity = Some(me);
            } else {
                anyhow::ensure!(
                    attempt.phase == Phase::Running && attempt.identity.as_ref() == Some(&me),
                    "review-watch completion identity changed"
                );
            }
            attempt.phase = phase;
            Ok(attempt.clone())
        })
    }
}

// Durable rounds outside this watcher count too. Deduplicate current/archive
// copies using the verdict writer's recording identity, never a separate policy.
// trace:TASK-1993 | ai:codex
#[derive(Default)]
struct History {
    keys: BTreeSet<crate::review_verdict::RecordingKey>,
}
impl History {
    fn add(&mut self, bytes: &[u8]) -> Result<()> {
        let mut object = serde_json::from_slice::<serde_json::Value>(bytes)?
            .as_object()
            .cloned()
            .context("review history is not an object")?;
        let rounds = match object.remove("rounds") {
            None => Vec::new(),
            Some(serde_json::Value::Array(rounds)) => rounds,
            Some(_) => anyhow::bail!("invalid retained review rounds"),
        };
        for value in std::iter::once(serde_json::Value::Object(object)).chain(rounds) {
            let object = value
                .as_object()
                .context("invalid historical review round")?;
            anyhow::ensure!(
                object
                    .get("unarchived_rounds")
                    .is_none_or(|v| v.as_u64() == Some(0)),
                "unidentifiable historical rounds require advisor reconciliation"
            );
            let key = crate::review_verdict::recording_key(object);
            anyhow::ensure!(
                valid_recording_key(&key),
                "unidentifiable historical review requires advisor reconciliation"
            );
            self.keys.insert(key);
        }
        Ok(())
    }
    fn contains(&self, head: &str) -> bool {
        self.keys.iter().any(|key| {
            key.0
                .as_deref()
                .is_some_and(|sha| crate::review_verdict::same_reviewed_sha(sha, head))
        })
    }
    fn used(&self, entry: &Entry) -> usize {
        self.keys.len()
            + entry
                .attempts
                .iter()
                .filter(|a| a.phase != Phase::Failed && !self.contains(&a.head))
                .count()
    }
}

// trace:TASK-1993 | ai:codex
fn valid_recording_key(key: &crate::review_verdict::RecordingKey) -> bool {
    key.0.as_deref().is_some_and(|sha| {
        (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
    }) && !key.1.is_empty()
        && crate::review_verdict::VerdictKind::parse(&key.2)
            != crate::review_verdict::VerdictKind::Unknown
}

fn validate_state(state: &State, repository: &str) -> Result<()> {
    anyhow::ensure!(
        state.version == 1
            && state.repository == repository
            && (1..=2).contains(&state.max_concurrent)
            && state.entries.len() <= 1000,
        "unsupported or mismatched review-watch state"
    );
    for (key, entry) in &state.entries {
        parse_key(key)?;
        anyhow::ensure!(
            entry.attempts.len() <= 2
                && entry.notices.len() <= 256
                && entry.historical_keys.len() <= 2
                && entry.historical_keys.iter().all(valid_recording_key),
            "invalid review-watch attempt/notice count"
        );
        let mut heads = BTreeSet::new();
        for attempt in &entry.attempts {
            anyhow::ensure!(
                valid_sha(&attempt.head)
                    && heads.insert(&attempt.head)
                    && uuid::Uuid::parse_str(&attempt.token).is_ok()
                    && attempt
                        .vendor
                        .as_deref()
                        .is_none_or(|v| matches!(v, "claude" | "codex" | "agy")),
                "invalid review-watch launch receipt"
            );
            anyhow::ensure!(
                match attempt.phase {
                    Phase::Reserved => attempt.identity.is_none(),
                    Phase::Running | Phase::Completed => attempt
                        .identity
                        .as_ref()
                        .is_some_and(|i| i.pid > 0 && !i.start.is_empty() && !i.boot.is_empty()),
                    Phase::Failed => true,
                },
                "invalid review-watch process receipt"
            );
        }
        anyhow::ensure!(
            entry.notices.keys().all(|h| valid_sha(h)),
            "invalid review-watch notice head"
        );
    }
    Ok(())
}

#[derive(Debug)]
enum Action {
    Dispatch(Attempt),
    Notify(Notice),
    Skip(String),
}

fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}
fn parse_key(key: &str) -> Result<u64> {
    let number = key
        .strip_prefix("PR-")
        .or_else(|| key.strip_prefix("MR-"))
        .context("invalid review-watch change key")?;
    let id = number
        .parse::<u64>()
        .context("invalid review-watch change number")?;
    anyhow::ensure!(
        id > 0 && id.to_string() == number,
        "invalid review-watch change number"
    );
    Ok(id)
}
fn active(attempt: &Attempt) -> bool {
    matches!(attempt.phase, Phase::Reserved | Phase::Running)
}

fn select(
    store: &Store,
    change: &forge::Change,
    vendor: Option<String>,
    max: u8,
) -> Result<Action> {
    let key = change.key();
    let overlap = change
        .overlap
        .context("default-branch overlap evidence is unknown")?;
    store.transaction(&key, |state| {
        let total = state.entries.values().flat_map(|e| &e.attempts).filter(|a| active(a)).count();
        if total == 0 { state.max_concurrent = max; }
        anyhow::ensure!(state.max_concurrent == max, "another watcher uses a different concurrency limit; use --max-concurrent {}", state.max_concurrent);
        let repository = state.repository.clone();
        let entry = state.entries.entry(key.clone()).or_default();
        let history = store.history(&key)?;
        if overlap {
            let notice = entry.notices.entry(change.head.clone()).or_insert_with(|| Notice {
                message: stale_message(&repository, &key, change), sent: false,
            });
            let mut expected = stale_message(&repository, &key, change);
            expected.timestamp = notice.message.timestamp;
            anyhow::ensure!(serde_json::to_value(&expected)? == serde_json::to_value(&notice.message)?, "notification identity changed; pending message retained");
            return Ok(if notice.sent { Action::Skip("stale overlapping head already notified".into()) } else { Action::Notify(notice.clone()) });
        }
        if let Some(attempt) = entry.attempts.iter().find(|a| a.head == change.head && a.phase != Phase::Failed) {
            return Ok(Action::Skip(format!("head already reserved ({:?}); inspect .aida/review-watch/state.json and `aida session leases --all` for unfinished attempts", attempt.phase)));
        }
        if entry.attempts.iter().any(active) {
            return Ok(Action::Skip("prior head still active or ambiguous; S1 waits without signalling processes".into()));
        }
        if history.contains(&change.head) { return Ok(Action::Skip("head already has a recorded review".into())); }
        if history.used(entry) >= 2 { return Ok(Action::Skip("two-round review cap reached; route further work to the advisor".into())); }
        if total >= usize::from(max) { return Ok(Action::Skip("global reviewer capacity is occupied or reserved".into())); }
        entry.historical_keys.extend(history.keys);
        let attempt = Attempt { head: change.head.clone(), token: uuid::Uuid::new_v4().to_string(), vendor, phase: Phase::Reserved, identity: None };
        if let Some(failed) = entry.attempts.iter_mut().find(|a| a.phase == Phase::Failed) { *failed = attempt.clone(); }
        else { entry.attempts.push(attempt.clone()); }
        Ok(Action::Dispatch(attempt))
    })
}

fn stale_message(
    repository: &str,
    key: &str,
    change: &forge::Change,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!("{repository}\0{key}\0{}\0stale", change.head))
    );
    Message { id: id.clone(), thread_id: id, from: "review-watch".into(), to: Recipient::Agent(change.author.clone()),
        timestamp: chrono::Utc::now().timestamp(), in_reply_to: None,
        body: format!("{key} at {} is behind the default branch with overlapping paths. Merge the default branch into the branch and push before review. The watcher did not update the branch.", change.head),
        subject: Some(format!("{key}: merge the default branch before review")), urgent: false, intent: Intent::Request,
        retracted: false, deleted: false, archived: false, from_source: SenderSource::Explicit,
        from_role: None, relayed_from: None }
}

/// A deterministic mailbox ID and no-clobber publication close the
/// delivery-before-sent-receipt crash window without overwriting reader state.
fn deliver(root: &Path, notice: &Notice) -> Result<()> {
    let runtime = Directory::shared(&root.join(".aida"))?;
    let mailbox = Directory::shared(&runtime.path().join("mailbox"))?;
    let name = format!("{}.json", notice.message.id);
    let mut temp = tempfile::NamedTempFile::new_in(mailbox.path())?;
    temp.write_all(&serde_json::to_vec_pretty(&notice.message)?)?;
    temp.as_file().sync_all()?;
    match temp.persist_noclobber(mailbox.path().join(&name)) {
        Ok(_) => {
            mailbox.handle.sync_all()?;
            Ok(())
        }
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = mailbox
                .read_file(&name, true)?
                .context("notification disappeared during retry")?;
            let old: aida_core::mailbox::Message = serde_json::from_slice(&bytes)?;
            anyhow::ensure!(
                old.id == notice.message.id
                    && old.thread_id == notice.message.thread_id
                    && old.from == notice.message.from
                    && old.to == notice.message.to
                    && old.body == notice.message.body
                    && old.subject == notice.message.subject,
                "notification ID collision; existing message retained"
            );
            Ok(())
        }
        Err(e) => Err(e.error.into()),
    }
}

fn queue_args(key: &str, vendor: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "queue".into(),
        "work".into(),
        key.into(),
        "--role".into(),
        "reviewer".into(),
        "--no-human".into(),
        "--fresh".into(),
    ];
    if let Some(vendor) = vendor {
        args.extend(["--vendor".into(), vendor.into()]);
    }
    args
}

// Clear inherited orchestration/gate context so queue work must take its
// ordinary standalone attested path in the same pinned repository.
// trace:TASK-1993 | ai:codex
fn watch_command(repo: &forge::Repository, root: &Path, program: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root).stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| {
            key.starts_with("AIDA_REVIEW_")
                || key.starts_with("AIDA_FROM_PR_")
                || key.starts_with("AIDA_AGENT_GATE_")
        }) {
            command.env_remove(key);
        }
    }
    command
        .env_remove("AIDA_GRADED_REVIEW_CONTEXT")
        .env_remove("AIDA_SESSION_PURPOSE")
        .env_remove("AIDA_CARGO_NO_SLOT");
    repo.pin_command(&mut command);
    command
}

fn child(root: &Path, repo: &forge::Repository, reservation: &str) -> Result<()> {
    child_with_program(root, repo, reservation, &crate::aida_exe_path())
}

// Kept separate from executable resolution so process-level fixtures exercise
// the production receipt/preflight/queue command boundary without an LLM.
// trace:TASK-1993 | ai:codex
fn child_with_program(
    root: &Path,
    repo: &forge::Repository,
    reservation: &str,
    program: &Path,
) -> Result<()> {
    let (key, token) = reservation
        .split_once(':')
        .context("invalid launch reservation")?;
    let id = parse_key(key)?;
    let store = Store::open(root, &repo.identity)?;
    let round = std::cell::Cell::new(0);
    execute_reservation(
        &store,
        key,
        token,
        |attempt| {
            let mut current = repo.change(root, id)?;
            repo.probe(root, &mut current)?;
            anyhow::ensure!(
                current.head == attempt.head
                    && current.key() == key
                    && current.overlap == Some(false),
                "head changed before review dispatch"
            );
            round.set(store.round_for(key, attempt)?);
            Ok(())
        },
        |attempt| {
            let mut cmd = watch_command(repo, root, program);
            cmd.args(queue_args(key, attempt.vendor.as_deref()))
                .env(WATCH_HEAD, &attempt.head)
                .env("AIDA_REVIEW_ROUND", round.get().to_string());
            let status = crate::process_retry::RetryEtxtbsy::status_retrying_etxtbsy(&mut cmd)?;
            anyhow::ensure!(status.success(), "attested review exited {status}; receipt retained; inspect .aida/review-watch/state.json and `aida session leases --all`");
            Ok(())
        },
    )
}

fn execute_reservation(
    store: &Store,
    key: &str,
    token: &str,
    preflight: impl FnOnce(&Attempt) -> Result<()>,
    work: impl FnOnce(&Attempt) -> Result<()>,
) -> Result<()> {
    // No work begins before the durable, process-bound receipt. A lost
    // dispatcher without a completion receipt stays ambiguous even if its
    // PID disappeared or was reused; an orphan reviewer must not free a slot.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let attempt = loop {
        match store.transition(key, token, Phase::Running) {
            Ok(attempt) => break attempt,
            Err(e)
                if e.chain().any(|e| {
                    e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
                }) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(e) => return Err(e),
        }
    };
    if let Err(error) = preflight(&attempt) {
        // No queue-work process was started, so this reservation can retry
        // without consuming a review round or claiming reviewer completion.
        store.transition(key, token, Phase::Failed)?;
        return Err(error);
    }
    work(&attempt)?;
    store.transition(key, token, Phase::Completed)?;
    Ok(())
}

/// Called at the ordinary standalone-review launch boundary. The context
/// restricts the head and prompt; it supplies no seat or verdict authority.
pub(crate) fn reviewer_launch_context(root: &Path, id: u64) -> Result<bool> {
    let Some(head) = std::env::var_os(WATCH_HEAD) else {
        return Ok(false);
    };
    let head = head.to_str().context("invalid review-watch head context")?;
    anyhow::ensure!(valid_sha(head), "invalid review-watch head context");
    let repository = forge::Repository::load(root)?;
    let mut current = repository.change(root, id)?;
    repository.probe(root, &mut current)?;
    anyhow::ensure!(
        current.head == head && current.overlap == Some(false),
        "review-watch head changed before reviewer launch; no review started"
    );
    let store = Store::open(root, &repository.identity)?;
    let key = current.key();
    let state = store.load()?;
    let attempt = state
        .entries
        .get(&key)
        .and_then(|entry| entry.attempts.iter().find(|a| active(a) && a.head == head))
        .context("missing watcher reservation at reviewer launch")?;
    store.round_for(&key, attempt)?;
    Ok(true)
}

pub(crate) fn run(args: &ReviewWatchArgs) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    let repo = forge::Repository::load(&root)?;
    crate::seat_authority::validate_child_delegation(&root, "reviewer")?;
    if let Some(reservation) = args.launch_reservation.as_deref() {
        return child(&root, &repo, reservation);
    }
    let store = Store::open(&root, &repo.identity)?;
    let mut children = Vec::<Child>::new();
    loop {
        children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
        let changes = match repo.discover(&root) {
            Ok(changes) => changes,
            Err(error) if !args.once => {
                eprintln!("review-watch discovery refused: {error:#}");
                std::thread::sleep(std::time::Duration::from_secs(args.interval));
                continue;
            }
            Err(error) => return Err(error),
        };
        for mut change in changes {
            let key = change.key();
            if let Err(error) = repo.probe(&root, &mut change) {
                eprintln!("{key}: base evidence refused: {error:#}");
                if args.once {
                    return Err(error);
                }
                continue;
            }
            let action = match select(&store, &change, args.vendor.clone(), args.max_concurrent) {
                Ok(action) => action,
                Err(error)
                    if error.chain().any(|e| {
                        e.downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
                    }) =>
                {
                    eprintln!("{key}: watcher admission is busy");
                    continue;
                }
                Err(error) => return Err(error),
            };
            match action {
                Action::Skip(reason) => eprintln!("{key}: {reason}"),
                Action::Notify(notice) => {
                    deliver(&root, &notice)?;
                    store.transaction(&key, |state| {
                        let pending = state
                            .entries
                            .get_mut(&key)
                            .and_then(|e| e.notices.get_mut(&change.head))
                            .context("notification receipt disappeared")?;
                        anyhow::ensure!(
                            pending.message.id == notice.message.id,
                            "notification receipt changed"
                        );
                        pending.sent = true;
                        Ok(())
                    })?;
                    eprintln!("{key}: merge-main notice delivered to {}", change.author);
                }
                Action::Dispatch(attempt) => {
                    use std::os::unix::process::CommandExt;
                    let logs = Directory::open(&store.dir.path().join("logs"))?;
                    let log = logs.file(&format!("{key}-{}.log", attempt.token), true)?;
                    let mut command = watch_command(&repo, &root, &crate::aida_exe_path());
                    command
                        .args([
                            "review",
                            "watch",
                            "--launch-reservation",
                            &format!("{key}:{}", attempt.token),
                        ])
                        .stdout(Stdio::from(log.try_clone()?))
                        .stderr(Stdio::from(log))
                        .process_group(0);
                    match crate::process_retry::RetryEtxtbsy::spawn_retrying_etxtbsy(&mut command) {
                        Ok(child) => {
                            children.push(child);
                            eprintln!("{key}: review admitted for {}", change.head);
                        }
                        Err(error) => {
                            store.transaction(&key, |state| {
                                let a = state
                                    .entries
                                    .get_mut(&key)
                                    .and_then(|e| {
                                        e.attempts.iter_mut().find(|a| a.token == attempt.token)
                                    })
                                    .context("launch reservation disappeared")?;
                                anyhow::ensure!(
                                    a.phase == Phase::Reserved && a.identity.is_none(),
                                    "launch reservation already claimed"
                                );
                                a.phase = Phase::Failed;
                                Ok(())
                            })?;
                            return Err(error).context("could not spawn review dispatcher");
                        }
                    }
                }
            }
        }
        if args.once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(args.interval));
    }
}

#[cfg(test)]
#[path = "tests/task_1993_review_watch_tests.rs"]
mod tests;
