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
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use crate::history::{self, CommitMeta, Event, EventKind, HistoryOpts};

/// Bump when the tables or their meaning change.
// trace:TASK-1507 | ai:claude
pub(crate) const HISTORY_SCHEMA_VERSION: u32 = 1;

/// Bump whenever `decode_into_events`, `diff_modified` or `EventKind`
/// changes meaning or serialized shape. A bump gives the index a new file
/// name, so the old one is simply ignored (and pruned by an explicit
/// rebuild).
// trace:TASK-1507 | ai:claude
pub(crate) const HISTORY_DECODER_VERSION: u32 = 1;

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
pub(crate) fn set_test_serve_enabled(on: bool) {
    TEST_SERVE_ENABLED.with(|c| c.set(on));
}

fn cache_enabled() -> bool {
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
    subject    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS commits_ts ON commits(commit_ts, seq);
CREATE TABLE IF NOT EXISTS touches (
    path       TEXT NOT NULL,
    commit_seq INTEGER NOT NULL,
    PRIMARY KEY (path, commit_seq)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS touches_seq ON touches(commit_seq);
CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY,
    commit_seq  INTEGER NOT NULL,
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
CREATE INDEX IF NOT EXISTS events_ship ON events(commit_seq) WHERE is_ship = 1;
CREATE INDEX IF NOT EXISTS events_spec ON events(spec_id COLLATE NOCASE, commit_seq);
CREATE INDEX IF NOT EXISTS events_type ON events(req_type COLLATE NOCASE, commit_seq);
CREATE INDEX IF NOT EXISTS events_kind ON events(kind, commit_seq);
CREATE INDEX IF NOT EXISTS events_author ON events(author, commit_seq);
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
        "INSERT INTO commits (seq, sha, commit_ts, author_iso, git_author, subject)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            seq,
            raw.sha,
            raw.commit_ts,
            raw.author_iso,
            raw.git_author,
            raw.subject
        ],
    )?;
    let mut touch =
        conn.prepare_cached("INSERT OR IGNORE INTO touches (path, commit_seq) VALUES (?1, ?2)")?;
    for path in &dec.touches {
        touch.execute(params![path, seq])?;
    }
    let mut ev = conn.prepare_cached(
        "INSERT INTO events (commit_seq, ordinal, path, spec_id, req_type, kind,
             status_from, status_to, author, is_ship, is_comment, is_meta, kind_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
    #[allow(dead_code)]
    pub(crate) tip: String,
}

/// What `aida cache status` shows about the history index.
#[derive(Debug, Default)]
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
            Ok(conn)
        })?;
        Ok(HistoryCache { conn })
    }

    fn open_existing(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(0))?;
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
                if aida_core::git_ops::is_ancestor(store, &tip, head)? {
                    let _ = self.catch_up(store, &tip, head, budget)?;
                } else {
                    let recorded = self.meta("store_root_sha")?.unwrap_or_default();
                    let current = aida_core::git_ops::root_commits(store, head)
                        .map(|r| r.join(","))
                        .unwrap_or_default();
                    let reason = if recorded != current {
                        "the store history was replaced (its root commit changed)"
                    } else {
                        "store history was rewritten"
                    };
                    self.reset(store, head, Some(reason))?;
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
        let old_tip = self.meta("tip_sha")?.unwrap_or_default();
        let roots = aida_core::git_ops::root_commits(store, head)?.join(",");
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM events; DELETE FROM touches; DELETE FROM commits; DELETE FROM meta;",
        )?;
        let now = now_rfc3339();
        for (k, v) in [
            ("schema_version", HISTORY_SCHEMA_VERSION.to_string()),
            ("decoder_version", HISTORY_DECODER_VERSION.to_string()),
            ("store_root_sha", roots),
            ("tip_sha", head.to_string()),
            ("backfill_anchor", head.to_string()),
            ("floor_sha", String::new()),
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
        while let Some(raw) = stream.next_commit()? {
            let dec = decode_commit(&raw, &mut blobs)?;
            insert_commit(&tx, next_seq, &raw, &dec)?;
            next_seq += 1;
            indexed += 1;
            new_root |= raw.parents.is_empty();
            if indexed < total && budget.expired() {
                if probes >= MAX_BOUNDARY_PROBES {
                    // No ancestor-closed boundary found in time: keep the
                    // old tip; this query falls back to the git walk.
                    return Ok(false);
                }
                probes += 1;
                let closed =
                    aida_core::git_ops::rev_list_count(store, &format!("{tip}..{}", raw.sha))?
                        == indexed;
                if closed {
                    Self::finish_append(&tx, &raw, new_root.then_some(store))?;
                    tx.commit()?;
                    return Ok(false);
                }
            }
            if indexed == total {
                if raw.sha != head {
                    anyhow::bail!("catch-up ended at {} instead of HEAD {head}", raw.sha);
                }
                Self::finish_append(&tx, &raw, new_root.then_some(store))?;
                tx.commit()?;
                return Ok(true);
            }
        }
        anyhow::bail!("catch-up saw {indexed} of {total} commits in {tip}..{head}")
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
                 ORDER BY e.commit_seq DESC LIMIT 1",
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
        let order = git_lines(store, &["rev-list", "--topo-order", &anchor])?;
        let todo: Vec<&String> = order.iter().filter(|s| !indexed.contains(*s)).collect();
        if todo.is_empty() {
            let tx = self.conn.transaction()?;
            Self::set_meta(&tx, "complete", "1")?;
            tx.commit()?;
            return Ok(());
        }
        let mut blobs = BlobReader::spawn(store)?;
        let chunk_count = todo.len().div_ceil(chunk_size);
        for (ci, chunk) in todo.chunks(chunk_size).enumerate() {
            if budget.expired() || max_chunks.is_some_and(|m| ci >= m) {
                break;
            }
            let stdin: String = chunk.iter().map(|s| format!("{s}\n")).collect();
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
            let mut last: Option<RawCommit> = None;
            let mut i = 0usize;
            while let Some(raw) = stream.next_commit()? {
                if chunk.get(i).map(|s| s.as_str()) != Some(raw.sha.as_str()) {
                    anyhow::bail!("back-fill order mismatch at {}", raw.sha);
                }
                let dec = decode_commit(&raw, &mut blobs)?;
                insert_commit(&tx, next_seq, &raw, &dec)?;
                next_seq -= 1;
                i += 1;
                last = Some(raw);
            }
            if i != chunk.len() {
                anyhow::bail!("back-fill decoded {i} of {} commits", chunk.len());
            }
            let last = last.context("empty back-fill chunk")?;
            let now = now_rfc3339();
            Self::set_meta(&tx, "floor_sha", &last.sha)?;
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
    /// prove it holds the whole answer. Runs inside one read transaction
    /// so every read sees the same snapshot.
    ///
    /// Coverage rules: the tip must equal `head`, and one of these holds:
    /// the index reaches the root; the window of `max_commits` commits (and
    /// one more, to tell a capped walk from a finished one) lies inside the
    /// index; `limit` matches were found after every filter; or `--since`
    /// is newer than the oldest indexed commit. `--since`/`--until` are
    /// treated as pure commit-time filters (git's `--since` stops its walk
    /// at the first older commit, which can differ only under clock skew).
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
        let floor_ts: Option<i64> = tx
            .query_row(
                "SELECT commit_ts FROM commits ORDER BY seq ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;

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

        let probe = i64::try_from(opts.max_commits.saturating_add(1)).unwrap_or(i64::MAX);
        let candidates: Vec<i64> = if let Some(path) = &id_path {
            let mut stmt = tx.prepare(
                "SELECT c.seq FROM touches t JOIN commits c ON c.seq = t.commit_seq
                 WHERE t.path = ?1
                   AND (?2 IS NULL OR c.commit_ts >= ?2)
                   AND (?3 IS NULL OR c.commit_ts <= ?3)
                 ORDER BY t.commit_seq DESC LIMIT ?4",
            )?;
            let rows = stmt.query_map(params![path, since_ts, until_ts, probe], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()?
        } else {
            let mut stmt = tx.prepare(
                "SELECT seq FROM commits
                 WHERE (?1 IS NULL OR commit_ts >= ?1)
                   AND (?2 IS NULL OR commit_ts <= ?2)
                 ORDER BY seq DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![since_ts, until_ts, probe], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()?
        };
        let capped = candidates.len() > opts.max_commits;
        let window_len = candidates.len().min(opts.max_commits);

        let mut events: Vec<Event> = Vec::new();
        if let Some(&low) = candidates[..window_len].last() {
            // Pure narrowing: every filter is re-checked exactly in Rust by
            // the shared `event_passes_filters`.
            let mut sql = String::from(
                "SELECT c.sha, c.author_iso, e.author, e.spec_id, e.req_type, e.kind_json
                 FROM events e JOIN commits c ON c.seq = e.commit_seq
                 WHERE e.commit_seq >= ?1
                   AND (?2 IS NULL OR c.commit_ts >= ?2)
                   AND (?3 IS NULL OR c.commit_ts <= ?3)
                   AND (?4 IS NULL OR e.path = ?4)",
            );
            if opts.shipped_only {
                sql.push_str(" AND e.is_ship = 1");
            }
            if opts.exclude_meta {
                sql.push_str(" AND e.is_meta = 0");
            }
            sql.push_str(" ORDER BY e.commit_seq DESC, e.ordinal ASC");
            let mut stmt = tx.prepare(&sql)?;
            let mut rows = stmt.query(params![low, since_ts, until_ts, id_path])?;
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
                }
            }
        }

        let limit_met = events.len() >= opts.limit;
        let older_than_since = matches!((since_ts, floor_ts), (Some(s), Some(f)) if f < s);
        if !(complete || capped || limit_met || older_than_since) {
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
    let budget = budget_from(
        std::env::var("AIDA_HISTORY_INDEX_BUDGET_MS")
            .ok()
            .as_deref(),
    );
    match serve_at(store, &history_db_path(store), opts, budget) {
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
        if let Err(e) = cache.ensure_fresh(store, &head, Budget::for_duration(budget)) {
            drop(cache);
            return Err(heal(e, locked));
        }
    }
    let answer = cache.query(opts, &head);
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
/// schema/decoder versions next to `db_path`.
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
    for entry in entries.flatten() {
        let Some(other) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        if other.starts_with(family) && !other.starts_with(name) {
            let p = entry.path();
            if std::fs::remove_file(&p).is_ok() {
                pruned.push(p);
            }
        }
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
