//! The history index: a rebuildable SQLite copy of the events `aida history`
//! decodes from the store's git log.
//!
//! The git store stays canonical. This file is derived and disposable: it
//! answers a query only when it provably holds the whole answer, and every
//! miss or error falls back to the git walk in `history.rs`, which is also
//! the oracle the parity tests compare against.
//!
//! Layout and rules (the signed-off design, with its amendments):
//! - The file lives beside the requirements cache, but is a separate
//!   database with its own WAL and its own indexer lock. It never opens the
//!   requirements cache and never takes the store write lock.
//! - Its name carries the schema and decoder versions, so builds with
//!   different decoders never reset each other's index.
//! - The store path is canonicalized before the `.aida` walk-up, so sibling
//!   worktrees that reach the store through a symlink share one index.
//! - New commits are appended (catch-up, oldest first) when the indexed tip
//!   is an ancestor of HEAD. Older history is back-filled newest first from
//!   a pinned anchor, in small time-boxed transactions, so a first build is
//!   resumable. A rewritten history, a different store, or a version change
//!   truncates the index and starts over.
//! - A query is served only when the tip equals HEAD and the requested
//!   window provably lies inside the indexed commits.
//!
// trace:TASK-1507 | ai:claude

use anyhow::{Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use crate::history::{self, CommitMeta, Event, EventKind, HistoryOpts};

/// Bump when the tables or their meaning change.
/// v2: date-priority order (`events.commit_ts`), the `unfilled_max_ts`
/// back-fill watermark, and the `merge_paths` and `skew` tables.
/// v3: `merge_ties` (same-second commits on both sides of a merge)
/// replaces the boundary-only `merges` table.
/// v4: `merge_region_ts` (every commit time in a merge's parallel region,
/// fork point included) replaces `merge_ties`.
/// v5: `merge_paths` is gone. The `--id` walk uses `--full-history`, so it
/// no longer simplifies merged side branches away and needs no routing.
/// v6: `events.is_ship` means "a transition into Completed from any other
/// status" (BUG-1636), not only `Done → Completed`. `--shipped` narrows on
/// that column in SQL before the Rust re-check, so a v5 file would silently
/// drop every `InProgress → Completed` ship; the new file name retires it.
// trace:TASK-1507 | ai:claude
// trace:BUG-1620 | ai:claude
// trace:BUG-1636 | ai:claude
pub(crate) const HISTORY_SCHEMA_VERSION: u32 = 6;

/// Bump whenever `decode_into_events`, `diff_modified` or `EventKind`
/// changes meaning or serialized shape. A bump gives the index a new file
/// name, so the old one is simply ignored (and pruned by an explicit
/// rebuild).
/// v2: `RelationshipsChange` records the changed `edges` (BUG-1631).
/// v3: every stored `rel_type` form decodes to one key, so a format
/// rewrite is not an edge change; v2 files could hold false removals.
/// v4: edges diff as a multiset, so a changed duplicate is counted.
// trace:TASK-1507 | ai:claude
// trace:BUG-1631 | ai:claude
pub(crate) const HISTORY_DECODER_VERSION: u32 = 4;

/// Default cap on inline indexing work per query, in milliseconds.
const DEFAULT_INDEX_BUDGET_MS: u64 = 1500;

/// Back-fill chunk size (commits per transaction) under a time budget.
const BUDGETED_BACKFILL_CHUNK: usize = 128;

/// Back-fill chunk size for an explicit, unbudgeted full build.
const FULL_BUILD_CHUNK: usize = 2000;

/// How many candidate chunk boundaries a budgeted catch-up probes before
/// giving up and rolling back (only merges can make a boundary invalid).
const MAX_BOUNDARY_PROBES: usize = 32;

/// Short retry ladder for a busy database. A reader never waits long: it
/// falls back to the git walk instead.
const BUSY_LADDER_MS: [u64; 3] = [0, 50, 100];

/// WAL size past which the indexer checkpoints and truncates it (see
/// `HistoryCache::bound_wal`); about 1,000 pages, SQLite's usual
/// auto-checkpoint point.
const WAL_TRUNCATE_BYTES: u64 = 4 * 1024 * 1024;

/// Marker that starts each commit header in the batched `git log` output.
const HEADER_MARK: char = '\u{1e}';

// ---------------------------------------------------------------------------
// Switches and budget
// ---------------------------------------------------------------------------

/// `AIDA_HISTORY_CACHE=0` (or `false`/`off`/`no`) turns the index off.
// trace:TASK-1507 | ai:claude
pub(crate) fn cache_enabled_from(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// `AIDA_HISTORY_INDEX_BUDGET_MS` caps inline indexing per query.
// trace:TASK-1507 | ai:claude
pub(crate) fn budget_from(value: Option<&str>) -> Duration {
    Duration::from_millis(
        value
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_INDEX_BUDGET_MS),
    )
}

#[cfg(test)]
thread_local! {
    /// Unit tests build throwaway stores directly under a temp dir, where
    /// the fallback index file would land in the shared temp root. The
    /// env-driven entry point is therefore off in tests unless a test opts
    /// in; the index's own tests call [`serve_at`] with an explicit path.
    static TEST_SERVE_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    /// Tests: make the next reset fail, to exercise the delete fallback.
    static FAIL_NEXT_RESET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    /// Tests: how many catch-up boundary probes (`rev-list --count`) ran.
    static BOUNDARY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count one catch-up boundary probe (tests read the count). Always true,
/// so it can sit in a condition chain.
// trace:TASK-1507 | ai:claude
fn count_boundary_probe() -> bool {
    #[cfg(test)]
    BOUNDARY_PROBES.with(|c| c.set(c.get() + 1));
    true
}

/// Tests: take (and reset) the boundary-probe count.
#[cfg(test)]
pub(crate) fn take_boundary_probes() -> usize {
    BOUNDARY_PROBES.with(|c| c.replace(0))
}

#[cfg(test)]
pub(crate) fn fail_next_reset() {
    FAIL_NEXT_RESET.with(|c| c.set(true));
}

#[cfg(test)]
pub(crate) fn set_test_serve_enabled(on: bool) {
    TEST_SERVE_ENABLED.with(|c| c.set(on));
}

/// Whether the index is switched on for this process (`AIDA_HISTORY_CACHE`).
/// Callers use it to tell "switched off" from "could not answer".
// trace:TASK-1508 | ai:claude
pub(crate) fn cache_enabled() -> bool {
    #[cfg(test)]
    {
        if !TEST_SERVE_ENABLED.with(|c| c.get()) {
            return false;
        }
    }
    cache_enabled_from(std::env::var("AIDA_HISTORY_CACHE").ok().as_deref())
}

/// A wall-clock allowance for inline indexing. `None` means unbounded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    deadline: Option<Instant>,
}

impl Budget {
    pub(crate) fn unbounded() -> Self {
        Budget { deadline: None }
    }

    pub(crate) fn for_duration(d: Duration) -> Self {
        Budget {
            deadline: Some(Instant::now() + d),
        }
    }

    fn expired(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn is_unbounded(&self) -> bool {
        self.deadline.is_none()
    }
}

// ---------------------------------------------------------------------------
// Location
// ---------------------------------------------------------------------------

/// `history-v<schema>-<decoder>.db`.
// trace:TASK-1507 | ai:claude
pub(crate) fn history_db_file_name() -> String {
    format!("history-v{HISTORY_SCHEMA_VERSION}-{HISTORY_DECODER_VERSION}.db")
}

/// Where the history index for the store at `store` lives:
/// `<project>/.aida/history-v<S>-<D>.db`, found by the same guarded walk-up
/// the requirements cache uses, starting from the canonicalized store path
/// so every worktree that reaches the store through a symlink shares it.
/// With no `.aida` directory it falls back to a sibling file next to the
/// store root (never inside the store, never a stray file in its parent).
// trace:TASK-1507 | ai:claude
pub(crate) fn history_db_path(store: &Path) -> PathBuf {
    history_db_path_with_roots(store, &aida_core::store_locate::real_temp_roots())
}

fn history_db_path_with_roots(store: &Path, temp_roots: &[PathBuf]) -> PathBuf {
    let file_name = history_db_file_name();
    let store = store.canonicalize().unwrap_or_else(|_| store.to_path_buf());
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    if let Some(parent) = store.parent() {
        let mut probe = parent.to_path_buf();
        for _ in 0..6 {
            // BUG-1598: never adopt a temp root as the project root.
            if aida_core::store_locate::is_in_canonical_roots(&probe, &canonical_roots) {
                break;
            }
            if probe.join(".aida").is_dir() {
                return probe.join(".aida").join(&file_name);
            }
            match probe.parent() {
                Some(p) => probe = p.to_path_buf(),
                None => break,
            }
        }
    }
    store.with_extension(&file_name)
}

fn lock_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.lock", db_path.display()))
}

fn sidecar_paths(db_path: &Path) -> Vec<PathBuf> {
    ["-wal", "-shm"]
        .iter()
        .map(|s| PathBuf::from(format!("{}{}", db_path.display(), s)))
        .collect()
}

fn remove_db_files(db_path: &Path) {
    let _ = std::fs::remove_file(db_path);
    for p in sidecar_paths(db_path) {
        let _ = std::fs::remove_file(p);
    }
}

// ---------------------------------------------------------------------------
// Indexer lock
// ---------------------------------------------------------------------------

/// Exclusive advisory lock on `<db>.lock`. The kernel drops it if the
/// process dies, so no stale-lock healing is needed.
pub(crate) struct IndexLock {
    _file: std::fs::File,
}

impl IndexLock {
    fn open_file(db_path: &Path) -> Result<std::fs::File> {
        let path = lock_path(db_path);
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open history index lock {}", path.display()))
    }

    /// `Ok(None)` when another process holds the lock.
    fn try_acquire(db_path: &Path) -> Result<Option<IndexLock>> {
        let file = Self::open_file(db_path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(IndexLock { _file: file })),
            Err(_) => Ok(None),
        }
    }

    fn acquire_blocking(db_path: &Path) -> Result<IndexLock> {
        let file = Self::open_file(db_path)?;
        file.lock_exclusive()
            .context("cannot lock the history index")?;
        Ok(IndexLock { _file: file })
    }

    /// Whether some process currently holds the lock (diagnostic only).
    fn is_held(db_path: &Path) -> bool {
        let Ok(file) = std::fs::OpenOptions::new()
            .write(true)
            .open(lock_path(db_path))
        else {
            return false;
        };
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                false
            }
            Err(_) => true,
        }
    }
}

// ---------------------------------------------------------------------------
// SQLite plumbing
// ---------------------------------------------------------------------------

fn is_busy(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        c.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
            matches!(
                e,
                rusqlite::Error::SqliteFailure(code, _)
                    if matches!(
                        code.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    )
            )
        })
    })
}

/// The index disagrees with the store history it claims to hold (a
/// commit indexed twice, a catch-up that does not end at HEAD). It is
/// repaired by a reset, never left to fail every later query.
// trace:TASK-1507 | ai:claude
#[derive(Debug)]
struct Inconsistent(String);

impl std::fmt::Display for Inconsistent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "history index inconsistent with the store: {}", self.0)
    }
}

impl std::error::Error for Inconsistent {}

fn inconsistent(msg: String) -> anyhow::Error {
    anyhow::Error::new(Inconsistent(msg))
}

/// A constraint violation (such as a commit indexed twice) or an
/// [`Inconsistent`] error.
// trace:TASK-1507 | ai:claude
fn is_inconsistent(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        c.downcast_ref::<Inconsistent>().is_some()
            || c.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
                matches!(
                    e,
                    rusqlite::Error::SqliteFailure(code, _)
                        if code.code == rusqlite::ErrorCode::ConstraintViolation
                )
            })
    })
}

/// NotADatabase / DatabaseCorrupt: the file is not a usable database.
fn is_corrupt(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        c.downcast_ref::<rusqlite::Error>().is_some_and(|e| {
            matches!(
                e,
                rusqlite::Error::SqliteFailure(code, _)
                    if matches!(
                        code.code,
                        rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
                    )
            )
        })
    })
}

fn with_busy_retry<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last: Option<anyhow::Error> = None;
    for wait in BUSY_LADDER_MS {
        if wait > 0 {
            std::thread::sleep(Duration::from_millis(wait));
        }
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if is_busy(&e) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("history index busy")))
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS commits (
    seq        INTEGER PRIMARY KEY,
    sha        TEXT NOT NULL UNIQUE,
    commit_ts  INTEGER NOT NULL,
    author_iso TEXT NOT NULL,
    git_author TEXT NOT NULL,
    subject    TEXT NOT NULL,
    is_merge   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS commits_ts ON commits(commit_ts, seq);
CREATE INDEX IF NOT EXISTS commits_merge ON commits(seq) WHERE is_merge = 1;
CREATE TABLE IF NOT EXISTS touches (
    path       TEXT NOT NULL,
    commit_seq INTEGER NOT NULL,
    PRIMARY KEY (path, commit_seq)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS touches_seq ON touches(commit_seq);
CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY,
    commit_seq  INTEGER NOT NULL,
    commit_ts   INTEGER NOT NULL,
    ordinal     INTEGER NOT NULL,
    path        TEXT NOT NULL,
    spec_id     TEXT NOT NULL,
    req_type    TEXT NOT NULL,
    kind        TEXT NOT NULL,
    status_from TEXT,
    status_to   TEXT,
    author      TEXT NOT NULL,
    is_ship     INTEGER NOT NULL,
    is_comment  INTEGER NOT NULL,
    is_meta     INTEGER NOT NULL,
    kind_json   TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS events_order ON events(commit_seq DESC, ordinal);
CREATE INDEX IF NOT EXISTS events_time ON events(commit_ts, commit_seq, ordinal);
CREATE INDEX IF NOT EXISTS events_ship ON events(commit_ts, commit_seq) WHERE is_ship = 1;
CREATE INDEX IF NOT EXISTS events_spec ON events(spec_id COLLATE NOCASE, commit_seq);
CREATE INDEX IF NOT EXISTS events_type ON events(req_type COLLATE NOCASE, commit_seq);
CREATE INDEX IF NOT EXISTS events_kind ON events(kind, commit_seq);
CREATE INDEX IF NOT EXISTS events_author ON events(author, commit_seq);
CREATE TABLE IF NOT EXISTS merge_region_ts (
    ts INTEGER PRIMARY KEY
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS skew (
    child_sha  TEXT NOT NULL,
    parent_sha TEXT NOT NULL,
    child_ts   INTEGER NOT NULL,
    parent_ts  INTEGER NOT NULL,
    PRIMARY KEY (child_sha, parent_sha)
) WITHOUT ROWID;
";

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn kind_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Added { .. } => "added",
        EventKind::Deleted { .. } => "deleted",
        EventKind::StatusChange { .. } => "status_change",
        EventKind::PriorityChange { .. } => "priority_change",
        EventKind::TitleChange { .. } => "title_change",
        EventKind::DescriptionEdited => "description_edited",
        EventKind::OwnerChange { .. } => "owner_change",
        EventKind::FeatureChange { .. } => "feature_change",
        EventKind::TypeChange { .. } => "type_change",
        EventKind::TagsChange { .. } => "tags_change",
        EventKind::CommentsAdded { .. } => "comments_added",
        EventKind::RelationshipsChange { .. } => "relationships_change",
    }
}

/// The stored `kind` names a query's event-kind selectors admit, or empty
/// when none is set (every kind passes). A superset of what
/// `history::event_passes_filters` keeps, so narrowing on it in SQL never
/// drops an event the git walk would return.
// trace:TASK-1512 | ai:claude
fn history_kind_selection(opts: &HistoryOpts) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if opts.status_changes_only || history::has_transition_filter(opts) {
        kinds.push("status_change");
    }
    if opts.comments_only {
        kinds.push("comments_added");
    }
    if opts.opened_only {
        kinds.push("added");
    }
    kinds
}

// ---------------------------------------------------------------------------
// Batched decoder: one `git log --raw` stream + one `git cat-file --batch`
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RawChange {
    status: String,
    path: String,
    before: Option<String>,
    after: Option<String>,
}

#[derive(Debug)]
struct RawCommit {
    sha: String,
    parents: Vec<String>,
    commit_ts: i64,
    author_iso: String,
    git_author: String,
    subject: String,
    changes: Vec<RawChange>,
}

fn is_zero_oid(oid: &str) -> bool {
    !oid.is_empty() && oid.bytes().all(|b| b == b'0')
}

/// Parse one `--raw` line. Plain commits look like
/// `:100644 100644 <old> <new> M\t<path>`; merges (combined, `--cc`) carry
/// one colon, one mode and one blob per parent plus the result, and one
/// status letter per parent. The "before" side is the first parent, which
/// matches the git walk reading `<sha>^:<path>`.
fn parse_raw_line(line: &str) -> Option<RawChange> {
    let (meta, path) = line.split_once('\t')?;
    let parents = meta.bytes().take_while(|b| *b == b':').count();
    if parents == 0 {
        return None;
    }
    let fields: Vec<&str> = meta[parents..].split(' ').collect();
    let oid_count = parents + 1;
    if fields.len() < 2 * oid_count + 1 {
        return None;
    }
    let oids = &fields[oid_count..2 * oid_count];
    let status = fields[2 * oid_count].to_string();
    let to_opt = |o: &str| (!is_zero_oid(o)).then(|| o.to_string());
    Some(RawChange {
        status,
        path: path.trim().to_string(),
        before: to_opt(oids[0]),
        after: to_opt(oids[parents]),
    })
}

fn parse_header(line: &str) -> Option<RawCommit> {
    let mut parts = line.splitn(6, '\t');
    let sha = parts.next()?.to_string();
    let parents = parts
        .next()?
        .split(' ')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    let commit_ts = parts.next()?.trim().parse::<i64>().ok()?;
    let author_iso = parts.next()?.to_string();
    let git_author = parts.next()?.to_string();
    let subject = parts.next().unwrap_or("").to_string();
    Some(RawCommit {
        sha,
        parents,
        commit_ts,
        author_iso,
        git_author,
        subject,
        changes: Vec::new(),
    })
}

/// A streaming `git log --raw` reader that yields one commit at a time.
struct LogStream {
    child: Child,
    reader: BufReader<ChildStdout>,
    pending: Option<RawCommit>,
    writer: Option<std::thread::JoinHandle<()>>,
    finished: bool,
}

impl LogStream {
    fn spawn(store: &Path, extra: &[String], stdin_data: Option<String>) -> Result<Self> {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(store)
            .args([
                "log",
                "--no-abbrev",
                "--raw",
                "--no-renames",
                "--diff-merges=cc",
                "--root",
                "--no-color",
                "--format=%x1e%H%x09%P%x09%ct%x09%aI%x09%ae%x09%s",
            ])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(if stdin_data.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = cmd.spawn().context("failed to run git log")?;
        let writer = match (stdin_data, child.stdin.take()) {
            (Some(data), Some(mut stdin)) => Some(std::thread::spawn(move || {
                let _ = stdin.write_all(data.as_bytes());
            })),
            _ => None,
        };
        let stdout = child.stdout.take().context("git log has no stdout")?;
        Ok(LogStream {
            child,
            reader: BufReader::new(stdout),
            pending: None,
            writer,
            finished: false,
        })
    }

    fn next_commit(&mut self) -> Result<Option<RawCommit>> {
        let mut buf = Vec::new();
        loop {
            if self.finished {
                return Ok(self.pending.take());
            }
            buf.clear();
            let n = self.reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                self.finished = true;
                if let Some(w) = self.writer.take() {
                    let _ = w.join();
                }
                let status = self.child.wait()?;
                if !status.success() {
                    anyhow::bail!("git log exited with {status}");
                }
                continue;
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(rest) = line.strip_prefix(HEADER_MARK) {
                let header = parse_header(rest)
                    .with_context(|| format!("unparseable git log header: {rest}"))?;
                if let Some(done) = self.pending.replace(header) {
                    return Ok(Some(done));
                }
            } else if line.starts_with(':') {
                if let (Some(c), Some(change)) = (self.pending.as_mut(), parse_raw_line(line)) {
                    c.changes.push(change);
                }
            }
        }
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

/// One long-lived `git cat-file --batch` for blob contents.
struct BlobReader {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl BlobReader {
    fn spawn(store: &Path) -> Result<Self> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(store)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to run git cat-file")?;
        let stdin = child.stdin.take().context("cat-file has no stdin")?;
        let stdout = child.stdout.take().context("cat-file has no stdout")?;
        Ok(BlobReader {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// The blob's text (lossy UTF-8, as the git walk reads it), or `None`
    /// when the object is missing or not a blob.
    fn read(&mut self, oid: &str) -> Result<Option<String>> {
        writeln!(self.stdin, "{oid}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        if self.stdout.read_line(&mut header)? == 0 {
            anyhow::bail!("git cat-file ended early");
        }
        let header = header.trim_end();
        if header.ends_with(" missing") || header.ends_with(" ambiguous") {
            return Ok(None);
        }
        let mut parts = header.split(' ');
        let _oid = parts.next();
        let kind = parts.next().unwrap_or("");
        let size: usize = parts
            .next()
            .and_then(|s| s.parse().ok())
            .with_context(|| format!("unparseable cat-file header: {header}"))?;
        let mut body = vec![0u8; size + 1];
        self.stdout.read_exact(&mut body)?;
        body.truncate(size);
        if kind != "blob" {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&body).into_owned()))
    }
}

impl Drop for BlobReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A commit decoded into its touched object paths and its events (in walk
/// order), through the same `decode_into_events` the git walk uses.
struct DecodedCommit {
    touches: Vec<String>,
    events: Vec<(String, Event)>,
}

fn decode_commit(raw: &RawCommit, blobs: &mut BlobReader) -> Result<DecodedCommit> {
    let meta = CommitMeta {
        sha: raw.sha.clone(),
        iso_timestamp: raw.author_iso.clone(),
        git_author: raw.git_author.clone(),
    };
    let mut touches = Vec::new();
    let mut events = Vec::new();
    for ch in &raw.changes {
        if ch.path.is_empty() || !ch.path.starts_with("objects/") || !ch.path.ends_with(".yaml") {
            continue;
        }
        touches.push(ch.path.clone());
        let after = match &ch.after {
            Some(oid) => blobs.read(oid)?,
            None => None,
        };
        let before = match &ch.before {
            Some(oid) => blobs.read(oid)?,
            None => None,
        };
        let mut out = Vec::new();
        history::decode_into_events(
            &meta,
            &ch.status,
            &ch.path,
            before.as_deref(),
            after.as_deref(),
            &mut out,
        );
        events.extend(out.into_iter().map(|e| (ch.path.clone(), e)));
    }
    Ok(DecodedCommit { touches, events })
}

fn insert_commit(conn: &Connection, seq: i64, raw: &RawCommit, dec: &DecodedCommit) -> Result<()> {
    conn.execute(
        "INSERT INTO commits (seq, sha, commit_ts, author_iso, git_author, subject, is_merge)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            seq,
            raw.sha,
            raw.commit_ts,
            raw.author_iso,
            raw.git_author,
            raw.subject,
            (raw.parents.len() > 1) as i64
        ],
    )?;
    let mut touch =
        conn.prepare_cached("INSERT OR IGNORE INTO touches (path, commit_seq) VALUES (?1, ?2)")?;
    for path in &dec.touches {
        touch.execute(params![path, seq])?;
    }
    let mut ev = conn.prepare_cached(
        "INSERT INTO events (commit_seq, ordinal, path, spec_id, req_type, kind,
             status_from, status_to, author, is_ship, is_comment, is_meta, kind_json,
             commit_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;
    for (ordinal, (path, e)) in dec.events.iter().enumerate() {
        let (from, to) = match &e.kind {
            EventKind::StatusChange { from, to } => (Some(from.as_str()), Some(to.as_str())),
            _ => (None, None),
        };
        ev.execute(params![
            seq,
            ordinal as i64,
            path,
            e.spec_id,
            e.req_type,
            kind_name(&e.kind),
            from,
            to,
            e.author,
            history::is_ship_event(&e.kind) as i64,
            matches!(e.kind, EventKind::CommentsAdded { .. }) as i64,
            e.req_type.eq_ignore_ascii_case("meta") as i64,
            serde_json::to_string(&e.kind)?,
            raw.commit_ts,
        ])?;
    }
    Ok(())
}

fn git_lines(store: &Path, args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(store)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Whether a local branch, or some other checkout of the store (a git
/// worktree whose HEAD is not `head`), still contains the indexed `tip`. That
/// separates a reader on an older, diverged or switched checkout, which
/// must leave the shared index alone, from a real rewrite, where nothing
/// has the indexed tip any more.
/// Any doubt (git failure, unknown object) answers `false`, which keeps the
/// old reset-on-rewrite behavior.
// trace:TASK-1507 | ai:claude
fn tip_is_live_elsewhere(store: &Path, tip: &str, head: &str) -> bool {
    // trace:TASK-1507 | ai:claude
    // N2a: a local branch that still contains the tip means this checkout
    // switched away from it, not that history was rewritten. Remote-tracking
    // refs and `store compact`'s pre-squash backup branches do not count:
    // they keep a rewritten tip reachable on purpose.
    if git_lines(
        store,
        &[
            "for-each-ref",
            "--contains",
            tip,
            "--format=%(refname)",
            "refs/heads",
        ],
    )
    .is_ok_and(|refs| {
        refs.iter()
            .any(|r| !r.starts_with("refs/heads/aida-store-pre-squash-"))
    }) {
        return true;
    }
    let Ok(lines) = git_lines(store, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("HEAD "))
        .map(str::trim)
        .filter(|h| *h != head)
        .any(|h| aida_core::git_ops::is_ancestor(store, tip, h).unwrap_or(false))
}

/// Commit times for `shas`, read from git a batch at a time.
// trace:TASK-1507 | ai:claude
fn git_commit_times(store: &Path, shas: &[String]) -> Result<HashMap<String, i64>> {
    let mut out = HashMap::new();
    for chunk in shas.chunks(200) {
        let mut args: Vec<&str> = vec!["show", "-s", "--format=%H %ct"];
        args.extend(chunk.iter().map(String::as_str));
        for line in git_lines(store, &args)? {
            if let Some((sha, ts)) = line.split_once(' ') {
                if let Ok(ts) = ts.trim().parse::<i64>() {
                    out.insert(sha.to_string(), ts);
                }
            }
        }
    }
    Ok(out)
}

/// Record a merge commit for the coverage rule `merge_region_ts`: the time
/// of every commit in the merge's parallel region, meaning both sides
/// (`M^i...M^j`) and the merge bases. git's walk orders same-second commits
/// there by queue insertion, not by ancestry: a merge queues its first
/// parent first, and a fork point can come before its own side-branch
/// child. `seq` cannot express that, so a served range holding such a
/// second shared with another commit falls back (B2, B3).
///
/// It also completes the merge's `touches`. `git log --full-history --
/// <path>` (the `--id` walk) lists a merge whose path differs from ANY
/// parent, while the combined diff that fills `touches` lists only paths
/// that differ from EVERY parent. A merge that took one side's version
/// (an `-s ours` merge, for one) produces no events for the path, but the
/// walk still counts it against `--max-commits`, so it must be a touch.
/// (This replaces the former `merge_paths` routing, N1b.)
// trace:TASK-1507 | ai:claude
// trace:BUG-1620 | ai:claude
fn record_merge(tx: &Connection, store: &Path, seq: i64, raw: &RawCommit) -> Result<()> {
    if raw.parents.len() < 2 {
        return Ok(());
    }
    let mut touch =
        tx.prepare_cached("INSERT OR IGNORE INTO touches (path, commit_seq) VALUES (?1, ?2)")?;
    for parent in &raw.parents {
        let changed = git_lines(
            store,
            &[
                "diff-tree",
                "-r",
                "--no-renames",
                "--name-only",
                "--no-commit-id",
                parent,
                &raw.sha,
            ],
        )?;
        for path in changed
            .iter()
            .filter(|p| p.starts_with("objects/") && p.ends_with(".yaml"))
        {
            touch.execute(params![path, seq])?;
        }
    }
    let mut region = tx.prepare_cached("INSERT OR IGNORE INTO merge_region_ts (ts) VALUES (?1)")?;
    for (i, a) in raw.parents.iter().enumerate() {
        for b in &raw.parents[i + 1..] {
            let range = format!("{a}...{b}");
            for line in git_lines(store, &["rev-list", "--timestamp", &range])? {
                if let Some(ts) = line
                    .split_once(' ')
                    .and_then(|(t, _)| t.parse::<i64>().ok())
                {
                    region.execute([ts])?;
                }
            }
            // No merge base (joined histories) is not an error.
            let bases = git_lines(store, &["merge-base", "--all", a, b]).unwrap_or_default();
            for ts in git_commit_times(store, &bases)?.values() {
                region.execute([ts])?;
            }
        }
    }
    Ok(())
}

/// Record every parent-to-child edge where the child is older than its
/// parent (clock skew, R3). Such an edge makes git's date-priority walk
/// differ from a pure date sort, so the query falls back near it.
// trace:TASK-1507 | ai:claude
fn record_skew(tx: &Connection, edges: &[(String, String, i64, i64)]) -> Result<()> {
    let mut ins = tx.prepare_cached(
        "INSERT OR IGNORE INTO skew (child_sha, parent_sha, child_ts, parent_ts)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (child, parent, child_ts, parent_ts) in edges {
        if child_ts < parent_ts {
            ins.execute(params![child, parent, child_ts, parent_ts])?;
        }
    }
    Ok(())
}

/// Skew edges for commits appended by a catch-up: parent times come from
/// the same batch, then the index, then git.
// trace:TASK-1507 | ai:claude
fn record_skew_for(
    tx: &Connection,
    store: &Path,
    seen: &[(String, i64, Vec<String>)],
) -> Result<()> {
    let mut times: HashMap<String, i64> = seen.iter().map(|(s, t, _)| (s.clone(), *t)).collect();
    let mut missing: Vec<String> = Vec::new();
    for parent in seen.iter().flat_map(|(_, _, p)| p) {
        if times.contains_key(parent) || missing.contains(parent) {
            continue;
        }
        let known: Option<i64> = tx
            .query_row(
                "SELECT commit_ts FROM commits WHERE sha = ?1",
                [parent],
                |r| r.get(0),
            )
            .optional()?;
        match known {
            Some(t) => {
                times.insert(parent.clone(), t);
            }
            None => missing.push(parent.clone()),
        }
    }
    times.extend(git_commit_times(store, &missing)?);
    let mut edges = Vec::new();
    for (child, child_ts, parents) in seen {
        for parent in parents {
            let parent_ts = *times
                .get(parent)
                .with_context(|| format!("no commit time for parent {parent}"))?;
            edges.push((child.clone(), parent.clone(), *child_ts, parent_ts));
        }
    }
    record_skew(tx, &edges)
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A query answer served from the index.
#[derive(Debug)]
pub(crate) struct CacheAnswer {
    pub(crate) events: Vec<Event>,
    pub(crate) hidden_archived: usize,
    pub(crate) window_exhausted: bool,
    /// The store commit the answer was served from.
    pub(crate) tip: String,
}

/// What `aida cache status` shows about the history index.
#[derive(Debug, Default, Clone)]
pub(crate) struct HistoryCacheStatus {
    pub(crate) path: PathBuf,
    pub(crate) exists: bool,
    pub(crate) error: Option<String>,
    pub(crate) tip: Option<String>,
    pub(crate) head: Option<String>,
    pub(crate) complete: bool,
    pub(crate) commits: i64,
    pub(crate) events: i64,
    pub(crate) floor_commit_at: Option<String>,
    pub(crate) built_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) last_reset_reason: Option<String>,
    pub(crate) indexer_running: bool,
}

/// Outcome of an explicit full build.
#[derive(Debug)]
pub(crate) struct RebuildReport {
    pub(crate) path: PathBuf,
    pub(crate) commits: i64,
    pub(crate) events: i64,
    pub(crate) elapsed: Duration,
    pub(crate) pruned: Vec<PathBuf>,
}

pub(crate) struct HistoryCache {
    conn: Connection,
}

impl HistoryCache {
    fn open(path: &Path) -> Result<Self> {
        let conn = with_busy_retry(|| {
            let conn = Connection::open(path)
                .with_context(|| format!("cannot open history index {}", path.display()))?;
            conn.busy_timeout(Duration::from_millis(0))?;
            let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            Self::no_checkpoint_on_close(&conn)?;
            Ok(conn)
        })?;
        Ok(HistoryCache { conn })
    }

    /// Skip the WAL checkpoint (and its fsyncs) when the connection closes,
    /// and SQLite's automatic checkpoint too. Nearly every query is a small
    /// catch-up, and the close-time checkpoint dominated its cost. The
    /// automatic one is off because a fresh process re-reads a WAL it
    /// never reset as un-checkpointed, so it would copy the whole WAL
    /// again on every commit once past its threshold; instead the indexer
    /// truncates the WAL itself when it grows ([`Self::bound_wal`]).
    /// Crash safety is unchanged: every write is a WAL transaction with its
    /// meta in the same transaction.
    // trace:TASK-1507 | ai:claude
    fn no_checkpoint_on_close(conn: &Connection) -> Result<()> {
        conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        conn.pragma_update(None, "wal_autocheckpoint", 0)?;
        Ok(())
    }

    /// Fold the WAL back into the database and truncate it once it is
    /// larger than `limit` bytes (always, with `limit == 0`). Only the lock
    /// holder calls this. Best effort: a busy reader just defers it.
    // trace:TASK-1507 | ai:claude
    fn bound_wal(&self, db_path: &Path, limit: u64) {
        let wal = PathBuf::from(format!("{}-wal", db_path.display()));
        let size = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        if size > limit {
            let _ = self
                .conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
    }

    fn open_existing(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(0))?;
        Self::no_checkpoint_on_close(&conn)?;
        Ok(HistoryCache { conn })
    }

    fn ensure_schema(&self) -> Result<()> {
        with_busy_retry(|| Ok(self.conn.execute_batch(SCHEMA_SQL)?))
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn count(&self, table: &str) -> Result<i64> {
        Ok(self
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
    }

    /// Bring the index up to `head` within `budget`: validate versions,
    /// reset on a rewrite or a foreign store, append new commits, then
    /// back-fill older history while time remains.
    fn ensure_fresh(&mut self, store: &Path, head: &str, budget: Budget) -> Result<()> {
        self.ensure_schema()?;
        let versions_ok = self.meta("schema_version")?.as_deref()
            == Some(&HISTORY_SCHEMA_VERSION.to_string())
            && self.meta("decoder_version")?.as_deref()
                == Some(&HISTORY_DECODER_VERSION.to_string());
        let tip = self.meta("tip_sha")?.filter(|t| !t.is_empty());
        match tip {
            None => self.reset(store, head, None)?,
            Some(_) if !versions_ok => self.reset(store, head, Some("index format changed"))?,
            Some(tip) if tip == head => {}
            Some(tip) => {
                let recorded = self.meta("store_root_sha")?.unwrap_or_default();
                let replaced = || {
                    aida_core::git_ops::root_commits(store, head)
                        .map(|r| r.join(","))
                        .unwrap_or_default()
                        != recorded
                };
                if aida_core::git_ops::is_ancestor(store, &tip, head)? {
                    let _ = self.catch_up(store, &tip, head, budget)?;
                } else if aida_core::git_ops::is_ancestor(store, head, &tip)? {
                    // trace:TASK-1507 | ai:claude
                    // A reader behind the shared index: fall back, no reset.
                } else if replaced() {
                    // trace:TASK-1507 | ai:claude
                    // Checked before liveness: `store compact` keeps the old
                    // tip on a backup branch, but the history was replaced.
                    self.reset(
                        store,
                        head,
                        Some("the store history was replaced (its root commit changed)"),
                    )?;
                } else if tip_is_live_elsewhere(store, &tip, head) {
                    // trace:TASK-1507 | ai:claude
                    // This reader is on an older or diverged checkout (a
                    // store worktree behind the shared index, or an undo)
                    // while the indexed tip is still what another checkout
                    // has. The index stays as it is; this query falls back
                    // to the git walk because the tip is not its HEAD.
                } else {
                    self.reset(store, head, Some("store history was rewritten"))?;
                }
            }
        }
        if self.meta("complete")?.as_deref() != Some("1") && !budget.expired() {
            self.backfill(store, budget)?;
        }
        Ok(())
    }

    /// Truncate everything and start a fresh, empty index anchored at
    /// `head`. `reason` is recorded for diagnostics (`None` for a first
    /// build).
    fn reset(&mut self, store: &Path, head: &str, reason: Option<&str>) -> Result<()> {
        #[cfg(test)]
        if FAIL_NEXT_RESET.with(|c| c.replace(false)) {
            anyhow::bail!("test: reset failed");
        }
        let old_tip = self.meta("tip_sha")?.unwrap_or_default();
        let roots = aida_core::git_ops::root_commits(store, head)?.join(",");
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM events; DELETE FROM touches; DELETE FROM commits; DELETE FROM meta;
             DELETE FROM merge_region_ts; DELETE FROM skew;",
        )?;
        let now = now_rfc3339();
        for (k, v) in [
            ("schema_version", HISTORY_SCHEMA_VERSION.to_string()),
            ("decoder_version", HISTORY_DECODER_VERSION.to_string()),
            ("store_root_sha", roots),
            ("tip_sha", head.to_string()),
            ("backfill_anchor", head.to_string()),
            ("floor_sha", String::new()),
            // Unknown until the first back-fill chunk: `--since` is not
            // served from a partial index before then.
            ("unfilled_max_ts", String::new()),
            ("complete", "0".to_string()),
            ("built_at", now.clone()),
            ("updated_at", now.clone()),
        ] {
            Self::set_meta(&tx, k, &v)?;
        }
        if let Some(reason) = reason {
            Self::set_meta(&tx, "last_reset_reason", reason)?;
            Self::set_meta(&tx, "last_reset_at", &now)?;
            Self::set_meta(
                &tx,
                "last_reset_transition",
                &format!("{old_tip} -> {head}"),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Append `tip..head`, oldest first. Returns whether it reached `head`.
    /// Under a budget it commits only at a boundary commit `C` whose
    /// ancestors in the range are exactly the rows indexed so far, so the
    /// indexed set stays ancestor-closed and `tip_sha` stays meaningful.
    fn catch_up(&mut self, store: &Path, tip: &str, head: &str, budget: Budget) -> Result<bool> {
        let range = format!("{tip}..{head}");
        let total = aida_core::git_ops::rev_list_count(store, &range)?;
        let mut stream = LogStream::spawn(
            store,
            &["--reverse".into(), "--topo-order".into(), range],
            None,
        )?;
        let mut blobs = BlobReader::spawn(store)?;
        let tx = self.conn.transaction()?;
        let mut next_seq: i64 =
            tx.query_row("SELECT COALESCE(MAX(seq) + 1, 0) FROM commits", [], |r| {
                r.get(0)
            })?;
        let mut indexed = 0usize;
        let mut probes = 0usize;
        let mut new_root = false;
        // Descendants of the old tip in the range, computed once the probe
        // limit is hit (N6).
        let mut descendants: Option<HashSet<String>> = None;
        let mut seen: Vec<(String, i64, Vec<String>)> = Vec::new();
        while let Some(raw) = stream.next_commit()? {
            let dec = decode_commit(&raw, &mut blobs)?;
            insert_commit(&tx, next_seq, &raw, &dec)?;
            // trace:TASK-1507 | ai:claude
            record_merge(&tx, store, next_seq, &raw)?;
            seen.push((raw.sha.clone(), raw.commit_ts, raw.parents.clone()));
            next_seq += 1;
            indexed += 1;
            new_root |= raw.parents.is_empty();
            if indexed < total && budget.expired() {
                if probes >= MAX_BOUNDARY_PROBES {
                    // trace:TASK-1507 | ai:claude
                    // N6: no valid boundary within the probe limit (a long
                    // side branch). Rather than roll back and redo the same
                    // work next time, keep appending past the budget and
                    // probe only descendants of the old tip, the only
                    // commits that can be a boundary (normally the merge).
                    if descendants.is_none() {
                        let range = format!("{tip}..{head}");
                        descendants = Some(
                            git_lines(store, &["rev-list", "--ancestry-path", &range])?
                                .into_iter()
                                .collect(),
                        );
                    }
                    let is_desc = descendants.as_ref().is_some_and(|d| d.contains(&raw.sha));
                    // B6: probe only merges. Every earlier descendant was
                    // already probed (the budget had expired), so a valid
                    // non-merge boundary's parent, appended just before it,
                    // would have been valid first. Probing every descendant
                    // made this path quadratic.
                    if is_desc
                        && raw.parents.len() > 1
                        && count_boundary_probe()
                        && aida_core::git_ops::rev_list_count(
                            store,
                            &format!("{tip}..{}", raw.sha),
                        )? == indexed
                    {
                        record_skew_for(&tx, store, &seen)?;
                        Self::finish_append(&tx, &raw, new_root.then_some(store))?;
                        tx.commit()?;
                        return Ok(false);
                    }
                    continue;
                }
                probes += 1;
                count_boundary_probe();
                // trace:TASK-1507 | ai:claude
                // B4: the batch must also descend from the old tip, or the
                // new tip could land on a side branch whose ancestors are
                // not exactly the indexed rows.
                let closed =
                    aida_core::git_ops::rev_list_count(store, &format!("{tip}..{}", raw.sha))?
                        == indexed
                        && aida_core::git_ops::is_ancestor(store, tip, &raw.sha)?;
                if closed {
                    record_skew_for(&tx, store, &seen)?;
                    Self::finish_append(&tx, &raw, new_root.then_some(store))?;
                    tx.commit()?;
                    return Ok(false);
                }
            }
            if indexed == total {
                if raw.sha != head {
                    return Err(inconsistent(format!(
                        "catch-up ended at {} instead of HEAD {head}",
                        raw.sha
                    )));
                }
                record_skew_for(&tx, store, &seen)?;
                Self::finish_append(&tx, &raw, new_root.then_some(store))?;
                tx.commit()?;
                return Ok(true);
            }
        }
        Err(inconsistent(format!(
            "catch-up saw {indexed} of {total} commits in {tip}..{head}"
        )))
    }

    fn finish_append(tx: &Connection, newest: &RawCommit, roots_from: Option<&Path>) -> Result<()> {
        let now = now_rfc3339();
        Self::set_meta(tx, "tip_sha", &newest.sha)?;
        Self::set_meta(tx, "tip_commit_ts", &newest.commit_ts.to_string())?;
        Self::set_meta(tx, "last_append_at", &now)?;
        Self::set_meta(tx, "updated_at", &now)?;
        Self::set_meta(tx, "indexer_pid", &std::process::id().to_string())?;
        if let Some(store) = roots_from {
            // A sync joined another root into the store's history.
            let roots = aida_core::git_ops::root_commits(store, &newest.sha)?.join(",");
            Self::set_meta(tx, "store_root_sha", &roots)?;
        }
        Self::record_last_event_at(tx)?;
        Ok(())
    }

    fn record_last_event_at(tx: &Connection) -> Result<()> {
        let last: Option<String> = tx
            .query_row(
                "SELECT c.author_iso FROM events e JOIN commits c ON c.seq = e.commit_seq
                 ORDER BY e.commit_ts DESC, e.commit_seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(last) = last {
            Self::set_meta(tx, "last_event_at", &last)?;
        }
        Ok(())
    }

    /// Index older history newest first from the pinned anchor, in small
    /// transactions until the budget runs out. The enumeration is
    /// `git rev-list --topo-order <anchor>` minus the commits already
    /// indexed, so resuming never depends on following first parents.
    fn backfill(&mut self, store: &Path, budget: Budget) -> Result<()> {
        let chunk_size = if budget.is_unbounded() {
            FULL_BUILD_CHUNK
        } else {
            BUDGETED_BACKFILL_CHUNK
        };
        self.backfill_chunks(store, budget, chunk_size, None)
    }

    /// [`Self::backfill`] with an explicit chunk size and an optional cap on
    /// how many chunks to commit (tests use it to stop mid-history).
    fn backfill_chunks(
        &mut self,
        store: &Path,
        budget: Budget,
        chunk_size: usize,
        max_chunks: Option<usize>,
    ) -> Result<()> {
        let anchor = self
            .meta("backfill_anchor")?
            .filter(|a| !a.is_empty())
            .context("history index has no back-fill anchor")?;
        let indexed: HashSet<String> = {
            let mut stmt = self.conn.prepare("SELECT sha FROM commits")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<HashSet<String>, _>>()?
        };
        // trace:TASK-1507 | ai:claude
        // `--timestamp --parents` gives each commit's time and parents in
        // the same call. Topo order can emit an old side branch before
        // newer main-line commits, so the oldest indexed commit is not a
        // bound on what is missing: every chunk records the newest commit
        // still left to fill (the watermark), and every skew edge among the
        // anchor's ancestors is known before any of them is served.
        struct Rev {
            ts: i64,
            sha: String,
            parents: Vec<String>,
        }
        let order: Vec<Rev> = git_lines(
            store,
            &[
                "rev-list",
                "--topo-order",
                "--timestamp",
                "--parents",
                &anchor,
            ],
        )?
        .into_iter()
        .map(|l| {
            let mut parts = l.split(' ');
            let ts = parts.next().unwrap_or_default();
            let ts = ts
                .parse::<i64>()
                .with_context(|| format!("unexpected rev-list line {l}"))?;
            let sha = parts
                .next()
                .with_context(|| format!("unexpected rev-list line {l}"))?
                .to_string();
            Ok(Rev {
                ts,
                sha,
                parents: parts.map(String::from).collect(),
            })
        })
        .collect::<Result<_>>()?;
        let times: HashMap<&str, i64> = order.iter().map(|r| (r.sha.as_str(), r.ts)).collect();
        let mut skew_edges = Vec::new();
        for r in &order {
            for p in &r.parents {
                if let Some(&pt) = times.get(p.as_str()) {
                    if r.ts < pt {
                        skew_edges.push((r.sha.clone(), p.clone(), r.ts, pt));
                    }
                }
            }
        }
        let todo: Vec<&Rev> = order.iter().filter(|r| !indexed.contains(&r.sha)).collect();
        if todo.is_empty() {
            let tx = self.conn.transaction()?;
            record_skew(&tx, &skew_edges)?;
            Self::set_meta(&tx, "complete", "1")?;
            Self::set_meta(&tx, "unfilled_max_ts", "")?;
            tx.commit()?;
            return Ok(());
        }
        // unfilled_after[i] = newest commit time among todo[i..].
        let mut unfilled_after: Vec<Option<i64>> = vec![None; todo.len() + 1];
        for i in (0..todo.len()).rev() {
            let t = todo[i].ts;
            unfilled_after[i] = Some(unfilled_after[i + 1].map_or(t, |m| m.max(t)));
        }
        let mut blobs = BlobReader::spawn(store)?;
        let chunk_count = todo.len().div_ceil(chunk_size);
        for (ci, chunk) in todo.chunks(chunk_size).enumerate() {
            if budget.expired() || max_chunks.is_some_and(|m| ci >= m) {
                break;
            }
            let stdin: String = chunk.iter().map(|r| format!("{}\n", r.sha)).collect();
            let mut stream = LogStream::spawn(
                store,
                &["--no-walk=unsorted".into(), "--stdin".into()],
                Some(stdin),
            )?;
            let tx = self.conn.transaction()?;
            let mut next_seq: i64 =
                tx.query_row("SELECT COALESCE(MIN(seq) - 1, 0) FROM commits", [], |r| {
                    r.get(0)
                })?;
            if ci == 0 {
                record_skew(&tx, &skew_edges)?;
            }
            let mut last: Option<RawCommit> = None;
            let mut i = 0usize;
            while let Some(raw) = stream.next_commit()? {
                if chunk.get(i).map(|r| r.sha.as_str()) != Some(raw.sha.as_str()) {
                    return Err(inconsistent(format!(
                        "back-fill order mismatch at {}",
                        raw.sha
                    )));
                }
                let dec = decode_commit(&raw, &mut blobs)?;
                insert_commit(&tx, next_seq, &raw, &dec)?;
                record_merge(&tx, store, next_seq, &raw)?;
                next_seq -= 1;
                i += 1;
                last = Some(raw);
            }
            if i != chunk.len() {
                return Err(inconsistent(format!(
                    "back-fill decoded {i} of {} commits",
                    chunk.len()
                )));
            }
            let last = last.context("empty back-fill chunk")?;
            let now = now_rfc3339();
            Self::set_meta(&tx, "floor_sha", &last.sha)?;
            let rest = ((ci + 1) * chunk_size).min(todo.len());
            let unfilled = unfilled_after[rest].map(|t| t.to_string());
            Self::set_meta(&tx, "unfilled_max_ts", unfilled.as_deref().unwrap_or(""))?;
            Self::set_meta(&tx, "updated_at", &now)?;
            Self::set_meta(&tx, "indexer_pid", &std::process::id().to_string())?;
            if ci + 1 == chunk_count {
                Self::set_meta(&tx, "complete", "1")?;
            }
            Self::record_last_event_at(&tx)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Answer `opts` from the index, or `Ok(None)` when the index cannot
    /// prove it holds the whole answer in the git walk's order. Runs inside
    /// one read transaction so every read sees the same snapshot.
    ///
    /// Order (R1): commits newest first by `(commit_ts, seq)`, events by
    /// ordinal within a commit. With no commit older than its parent, git's
    /// default walk is a date-priority queue, so this is its order; `seq`
    /// (topologically consistent) only breaks same-second ties.
    ///
    /// Coverage: the tip must equal `head`, and
    /// - a partly built index serves only what lies strictly newer than the
    ///   watermark (the newest commit not yet back-filled): the boundary of
    ///   a cut (the last kept row of the `max_commits` window, or the
    ///   commit of the `limit`-th match), or else `--since` (R2);
    /// - no clock-skewed edge may reach into the served range (R3);
    /// - the served range may not hold a second, inside a merge's parallel
    ///   region (fork point included), shared by more than one commit,
    ///   where git's tie order is not `seq` (B2, B3).
    ///
    /// `--id` needs no rule of its own. The walk runs `git log
    /// --full-history -- <path>`, which visits exactly the commits of the
    /// unfiltered walk, in the same order, and prints those whose path
    /// differs from at least one parent (a root commit: holds the path).
    /// `touches` holds exactly those commits: the raw diff for a
    /// single-parent or root commit, and a per-parent diff for a merge
    /// (`record_merge`). So the `--id` walk is the unfiltered walk restricted to
    /// `touches`, and every coverage rule above, proven for the unfiltered
    /// walk, carries over to that subsequence. (Before BUG-1620 the walk
    /// simplified TREESAME side branches away, so `--id` fell back on any
    /// merge-touched path or any merge in range.)
    // trace:TASK-1507 | ai:claude
    // trace:BUG-1620 | ai:claude
    fn query(&self, opts: &HistoryOpts, head: &str) -> Result<Option<CacheAnswer>> {
        let tx = self.conn.unchecked_transaction()?;
        let versions_ok = self.meta("schema_version")?.as_deref()
            == Some(&HISTORY_SCHEMA_VERSION.to_string())
            && self.meta("decoder_version")?.as_deref()
                == Some(&HISTORY_DECODER_VERSION.to_string());
        let tip = self.meta("tip_sha")?.unwrap_or_default();
        if !versions_ok || tip != head {
            return Ok(None);
        }
        let complete = self.meta("complete")?.as_deref() == Some("1");
        // The newest commit time among commits not yet back-filled, written
        // in the same transaction as each chunk. Empty means unknown.
        let unfilled_max_ts: Option<i64> = self
            .meta("unfilled_max_ts")?
            .and_then(|v| v.trim().parse::<i64>().ok());
        if !complete && unfilled_max_ts.is_none() {
            return Ok(None);
        }

        let parse_bound = |raw: &Option<String>| -> Result<Option<i64>> {
            raw.as_deref()
                .map(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|d| d.timestamp())
                        .with_context(|| format!("unparseable window bound {s}"))
                })
                .transpose()
        };
        let since_ts = parse_bound(&opts.since)?;
        let until_ts = parse_bound(&opts.until)?;
        let id_path: Option<String> = opts
            .id_filter
            .as_deref()
            .and_then(|id| aida_core::object_store::relative_object_path(id).ok());
        // Candidate commits, newest first: (commit_ts, seq).
        let probe = i64::try_from(opts.max_commits.saturating_add(1)).unwrap_or(i64::MAX);
        let row = |r: &rusqlite::Row| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?));
        let candidates: Vec<(i64, i64)> = if let Some(path) = &id_path {
            let mut stmt = tx.prepare(
                "SELECT c.commit_ts, c.seq FROM touches t JOIN commits c ON c.seq = t.commit_seq
                 WHERE t.path = ?1
                   AND (?2 IS NULL OR c.commit_ts >= ?2)
                   AND (?3 IS NULL OR c.commit_ts <= ?3)
                 ORDER BY c.commit_ts DESC, c.seq DESC LIMIT ?4",
            )?;
            let rows = stmt.query_map(params![path, since_ts, until_ts, probe], row)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = tx.prepare(
                "SELECT commit_ts, seq FROM commits
                 WHERE (?1 IS NULL OR commit_ts >= ?1)
                   AND (?2 IS NULL OR commit_ts <= ?2)
                 ORDER BY commit_ts DESC, seq DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![since_ts, until_ts, probe], row)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let capped = candidates.len() > opts.max_commits;
        let window_len = candidates.len().min(opts.max_commits);
        let window_low = window_len.checked_sub(1).map(|i| candidates[i]);

        let mut events: Vec<Event> = Vec::new();
        let mut last_taken: Option<(i64, i64)> = None;
        if let Some((low_ts, low_seq)) = window_low {
            // Pure narrowing: every filter is re-checked exactly in Rust by
            // the shared `event_passes_filters`.
            let mut sql = String::from(
                "SELECT c.sha, c.author_iso, e.author, e.spec_id, e.req_type, e.kind_json,
                        e.commit_ts, e.commit_seq
                 FROM events e JOIN commits c ON c.seq = e.commit_seq
                 WHERE (e.commit_ts > ?1 OR (e.commit_ts = ?1 AND e.commit_seq >= ?2))
                   AND (?3 IS NULL OR e.commit_ts >= ?3)
                   AND (?4 IS NULL OR e.commit_ts <= ?4)
                   AND (?5 IS NULL OR e.path = ?5)",
            );
            // `is_ship` is written from `history::is_ship_event`, the same
            // predicate the Rust re-check uses, so this narrowing and the
            // git walk agree on `--shipped` (BUG-1636).
            // trace:BUG-1636 | ai:claude
            if opts.shipped_only {
                sql.push_str(" AND e.is_ship = 1");
            }
            if opts.exclude_meta {
                sql.push_str(" AND e.is_meta = 0");
            }
            // The event-kind selectors (`--status-changes`, `--comments`,
            // `--opened`, `--to`/`--from`) narrow on the stored `kind`
            // column; `--to`/`--from` themselves are re-checked in Rust by
            // `event_passes_filters`, the same predicate the git walk uses.
            // trace:TASK-1512 | ai:claude
            let kinds = history_kind_selection(opts);
            if !kinds.is_empty() {
                let list: Vec<String> = kinds.iter().map(|k| format!("'{k}'")).collect();
                sql.push_str(&format!(" AND e.kind IN ({})", list.join(", ")));
            }
            sql.push_str(" ORDER BY e.commit_ts DESC, e.commit_seq DESC, e.ordinal ASC");
            let mut stmt = tx.prepare(&sql)?;
            let mut rows = stmt.query(params![low_ts, low_seq, since_ts, until_ts, id_path])?;
            while let Some(row) = rows.next()? {
                if events.len() >= opts.limit {
                    break;
                }
                let author_iso: String = row.get(1)?;
                let kind_json: String = row.get(5)?;
                let e = Event {
                    sha: row.get(0)?,
                    timestamp: history::human_timestamp(&author_iso),
                    author: row.get(2)?,
                    spec_id: row.get(3)?,
                    req_type: row.get(4)?,
                    kind: serde_json::from_str(&kind_json)
                        .context("undecodable stored history event")?,
                };
                if history::event_passes_filters(&e, opts) {
                    events.push(e);
                    last_taken = Some((row.get(6)?, row.get(7)?));
                }
            }
        }

        let limit_met = events.len() >= opts.limit;
        // The cut boundary: the commit of the `limit`-th match, else the
        // last row of a capped window. `None` with a cut means nothing is
        // kept (`-n 0` / `--max-commits 0`), which needs no coverage.
        let cut = limit_met || capped;
        let boundary: Option<(i64, i64)> = if limit_met {
            last_taken
        } else if capped {
            window_low
        } else {
            None
        };

        // R2: one watermark covers every cut and `--since`; ties fall back.
        if let Some(u) = unfilled_max_ts.filter(|_| !complete) {
            let covered = if cut {
                boundary.is_none_or(|(ts, _)| ts > u)
            } else {
                since_ts.is_some_and(|s| s > u)
            };
            if !covered {
                return Ok(None);
            }
        }
        // R3: a child older than its parent reorders git's walk (and stops
        // its `--since`) anywhere in [child_ts, parent_ts].
        let lower = [boundary.map(|b| b.0), since_ts]
            .into_iter()
            .flatten()
            .min();
        let skewed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM skew WHERE ?1 IS NULL OR parent_ts >= ?1)",
            [lower],
            |r| r.get(0),
        )?;
        if skewed {
            return Ok(None);
        }
        // B2/B3: a second inside some merge's parallel region (fork point
        // included) that more than one commit shares, anywhere in the
        // served range. git orders such ties by queue insertion, which
        // `seq` need not follow. A partner not yet back-filled lies at or
        // below the watermark, which R2 already keeps out of the range.
        // trace:TASK-1507 | ai:claude
        let cross_tie: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM merge_region_ts r
                 WHERE (?1 IS NULL OR r.ts >= ?1)
                   AND (SELECT COUNT(*) FROM commits c WHERE c.commit_ts = r.ts) > 1)",
            [lower],
            |r| r.get(0),
        )?;
        if cross_tie {
            return Ok(None);
        }
        let window_exhausted = capped && events.len() < opts.limit;
        events.truncate(opts.limit);
        Ok(Some(CacheAnswer {
            events,
            hidden_archived: opts.archived_specs.len(),
            window_exhausted,
            tip,
        }))
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

fn debug_log(err: &anyhow::Error) {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if std::env::var_os("AIDA_DEBUG").is_some()
        && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        eprintln!("history index unavailable, reading git history directly: {err:#}");
    }
}

/// Serve `opts` from the history index, catching it up inline within the
/// configured budget. `None` means "use the git walk": the index is off,
/// missing, stale, incomplete for this window, busy, or failed.
// trace:TASK-1507 | ai:claude
pub(crate) fn serve(store: &Path, opts: &HistoryOpts) -> Option<CacheAnswer> {
    if !cache_enabled() {
        return None;
    }
    #[cfg(not(test))]
    let answer = serve_at(
        store,
        &history_db_path(store),
        opts,
        budget_from(
            std::env::var("AIDA_HISTORY_INDEX_BUDGET_MS")
                .ok()
                .as_deref(),
        ),
    );
    // trace:BUG-1643 | ai:claude
    // A test that opts into env-driven serving indexes a tiny throwaway
    // store; a wall-clock budget there only makes the "served from the
    // index" answer depend on machine load. Tests index it unbounded
    // (`budget_from` and the budgeted paths are tested via `serve_at`).
    #[cfg(test)]
    let answer = serve_with_budget_at(store, &history_db_path(store), opts, None);
    match answer {
        Ok(answer) => answer,
        Err(e) => {
            debug_log(&e);
            None
        }
    }
}

/// [`serve`] against an explicit index path and budget.
// trace:TASK-1507 | ai:claude
pub(crate) fn serve_at(
    store: &Path,
    db_path: &Path,
    opts: &HistoryOpts,
    budget: Duration,
) -> Result<Option<CacheAnswer>> {
    serve_with_budget_at(store, db_path, opts, Some(budget))
}

/// [`serve_at`] with an optional budget: `None` indexes inline unbounded.
/// The deadline starts right before indexing, as it always has.
// trace:BUG-1643 | ai:claude
fn serve_with_budget_at(
    store: &Path,
    db_path: &Path,
    opts: &HistoryOpts,
    budget: Option<Duration>,
) -> Result<Option<CacheAnswer>> {
    let head = aida_core::git_ops::head_sha(store)?;
    let lock = IndexLock::try_acquire(db_path)?;
    let heal = |e: anyhow::Error, locked: bool| -> anyhow::Error {
        // BUG-683 pattern: a corrupt file is deleted (only by the lock
        // holder) so the next query rebuilds; this query falls back.
        if locked && is_corrupt(&e) {
            remove_db_files(db_path);
        }
        e
    };
    let locked = lock.is_some();
    let mut cache = HistoryCache::open(db_path).map_err(|e| heal(e, locked))?;
    if locked {
        if let Err(e) = cache.ensure_fresh(
            store,
            &head,
            budget.map_or_else(Budget::unbounded, Budget::for_duration),
        ) {
            // trace:TASK-1507 | ai:claude
            // B4: an index that disagrees with the store is reset (or, if
            // that fails, deleted) so the next query rebuilds it; this
            // query falls back.
            if is_inconsistent(&e)
                && cache
                    .reset(
                        store,
                        &head,
                        Some("the index disagreed with the store history"),
                    )
                    .is_err()
            {
                drop(cache);
                remove_db_files(db_path);
                return Err(e);
            }
            drop(cache);
            return Err(heal(e, locked));
        }
    }
    let answer = cache.query(opts, &head);
    if locked {
        cache.bound_wal(db_path, WAL_TRUNCATE_BYTES);
    }
    drop(cache);
    drop(lock);
    answer.map_err(|e| heal(e, locked))
}

/// Full, unbudgeted build (`aida cache rebuild --history`): start over,
/// index the whole history, catch up to the latest HEAD, and prune index
/// files left by other schema/decoder versions.
// trace:TASK-1507 | ai:claude
pub(crate) fn rebuild_full(store: &Path) -> Result<RebuildReport> {
    rebuild_full_at(store, &history_db_path(store))
}

pub(crate) fn rebuild_full_at(store: &Path, db_path: &Path) -> Result<RebuildReport> {
    let started = Instant::now();
    let _lock = IndexLock::acquire_blocking(db_path)?;
    let mut cache = match HistoryCache::open(db_path).and_then(|c| {
        c.ensure_schema()?;
        Ok(c)
    }) {
        Ok(c) => c,
        Err(e) if is_corrupt(&e) => {
            remove_db_files(db_path);
            let c = HistoryCache::open(db_path)?;
            c.ensure_schema()?;
            c
        }
        Err(e) => return Err(e),
    };
    let head = aida_core::git_ops::head_sha(store)?;
    cache.reset(store, &head, Some("explicit rebuild"))?;
    cache.backfill(store, Budget::unbounded())?;
    loop {
        let tip = cache.meta("tip_sha")?.unwrap_or_default();
        let now_head = aida_core::git_ops::head_sha(store)?;
        if tip == now_head {
            break;
        }
        if !aida_core::git_ops::is_ancestor(store, &tip, &now_head)? {
            anyhow::bail!("the store history changed during the rebuild; run it again");
        }
        cache.catch_up(store, &tip, &now_head, Budget::unbounded())?;
    }
    let commits = cache.count("commits")?;
    let events = cache.count("events")?;
    cache.bound_wal(db_path, 0);
    let pruned = prune_other_versions(db_path);
    Ok(RebuildReport {
        path: db_path.to_path_buf(),
        commits,
        events,
        elapsed: started.elapsed(),
        pruned,
    })
}

/// Remove history index files (and their sidecars/locks) for other
/// schema/decoder versions next to `db_path`. A version whose indexer lock
/// is held by another process (an older or newer `aida` still running) is
/// left alone entirely; a free lock is taken first and its file removed
/// while held, so no running indexer loses its lock file.
// trace:TASK-1507 | ai:claude
fn prune_other_versions(db_path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (
        db_path.parent(),
        db_path.file_name().and_then(|n| n.to_str()),
    ) else {
        return Vec::new();
    };
    let Some(idx) = name.find("history-v") else {
        return Vec::new();
    };
    let family = &name[..idx + "history-v".len()];
    let mut pruned = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return pruned;
    };
    let others: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.to_string(), e.path())))
        .filter(|(n, _)| n.starts_with(family) && !n.starts_with(name))
        .collect();
    let mut taken: Vec<(std::fs::File, PathBuf)> = Vec::new();
    let mut busy: Vec<String> = Vec::new();
    for (n, p) in others.iter().filter(|(n, _)| n.ends_with(".lock")) {
        let base = n.trim_end_matches(".lock").to_string();
        let file = std::fs::OpenOptions::new().write(true).open(p);
        match file {
            Ok(f) if f.try_lock_exclusive().is_ok() => taken.push((f, p.clone())),
            _ => busy.push(base),
        }
    }
    for (n, p) in others.iter().filter(|(n, _)| !n.ends_with(".lock")) {
        if busy.iter().any(|b| n.starts_with(b.as_str())) {
            continue;
        }
        if std::fs::remove_file(p).is_ok() {
            pruned.push(p.clone());
        }
    }
    for (lock, p) in taken {
        if std::fs::remove_file(&p).is_ok() {
            pruned.push(p);
        }
        drop(lock);
    }
    pruned
}

/// Diagnostics for `aida cache status`. Never creates the index.
// trace:TASK-1507 | ai:claude
pub(crate) fn status(store: &Path) -> HistoryCacheStatus {
    status_at(store, &history_db_path(store))
}

pub(crate) fn status_at(store: &Path, db_path: &Path) -> HistoryCacheStatus {
    let mut st = HistoryCacheStatus {
        path: db_path.to_path_buf(),
        exists: db_path.is_file(),
        head: aida_core::git_ops::head_sha(store).ok(),
        indexer_running: IndexLock::is_held(db_path),
        ..Default::default()
    };
    if !st.exists {
        return st;
    }
    let mut read = || -> Result<()> {
        let cache = HistoryCache::open_existing(db_path)?;
        st.tip = cache.meta("tip_sha")?.filter(|s| !s.is_empty());
        st.complete = cache.meta("complete")?.as_deref() == Some("1");
        st.built_at = cache.meta("built_at")?;
        st.updated_at = cache.meta("updated_at")?;
        st.last_reset_reason = cache.meta("last_reset_reason")?;
        st.commits = cache.count("commits")?;
        st.events = cache.count("events")?;
        st.floor_commit_at = cache
            .conn
            .query_row(
                "SELECT author_iso FROM commits ORDER BY seq ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(())
    };
    if let Err(e) = read() {
        st.error = Some(format!("{e:#}"));
    }
    st
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Hooks for the index's own tests.
    use super::*;

    pub(crate) fn meta(db_path: &Path, key: &str) -> Option<String> {
        HistoryCache::open_existing(db_path)
            .ok()?
            .meta(key)
            .ok()
            .flatten()
    }

    pub(crate) fn set_meta(db_path: &Path, key: &str, value: &str) {
        let cache = HistoryCache::open_existing(db_path).unwrap();
        HistoryCache::set_meta(&cache.conn, key, value).unwrap();
    }

    pub(crate) fn commit_count(db_path: &Path) -> i64 {
        HistoryCache::open_existing(db_path)
            .unwrap()
            .count("commits")
            .unwrap()
    }

    pub(crate) fn path_with_roots(store: &Path, roots: &[PathBuf]) -> PathBuf {
        history_db_path_with_roots(store, roots)
    }

    /// Run the indexer only (no query) under `budget`.
    pub(crate) fn index(store: &Path, db_path: &Path, budget: Budget) -> Result<()> {
        let head = aida_core::git_ops::head_sha(store)?;
        let _lock = IndexLock::acquire_blocking(db_path)?;
        let mut cache = HistoryCache::open(db_path)?;
        cache.ensure_fresh(store, &head, budget)
    }

    /// Query only (no indexing), as a reader that lost the indexer lock.
    pub(crate) fn query_only(
        store: &Path,
        db_path: &Path,
        opts: &HistoryOpts,
    ) -> Result<Option<CacheAnswer>> {
        let head = aida_core::git_ops::head_sha(store)?;
        HistoryCache::open(db_path)?.query(opts, &head)
    }

    /// Start a fresh index at HEAD and back-fill at most `max_chunks`
    /// chunks of `chunk_size` commits.
    pub(crate) fn partial_build(
        store: &Path,
        db_path: &Path,
        chunk_size: usize,
        max_chunks: usize,
    ) -> Result<()> {
        let head = aida_core::git_ops::head_sha(store)?;
        let _lock = IndexLock::acquire_blocking(db_path)?;
        let mut cache = HistoryCache::open(db_path)?;
        cache.ensure_schema()?;
        if cache.meta("tip_sha")?.is_none() {
            cache.reset(store, &head, None)?;
        }
        cache.backfill_chunks(store, Budget::unbounded(), chunk_size, Some(max_chunks))
    }

    /// Every indexed commit SHA, newest (highest seq) first.
    pub(crate) fn indexed_shas(db_path: &Path) -> Vec<String> {
        let cache = HistoryCache::open_existing(db_path).unwrap();
        let mut stmt = cache
            .conn
            .prepare("SELECT sha FROM commits ORDER BY seq DESC")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    /// Hold the indexer lock (as another process would).
    pub(crate) fn hold_lock(db_path: &Path) -> IndexLock {
        IndexLock::acquire_blocking(db_path).unwrap()
    }
}
