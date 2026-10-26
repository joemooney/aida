//! BUG-2221: incremental, append-only spec-id -> commit index behind `aida
//! show`'s positive linkage commit scan (SKETCH v3, advisor signoff
//! 2026-10-26 05:40 MST, `docs/plans/2026-10-26-bug-2221-sketch-v3.md`).
//!
//! The original query (`git log --all -F --date-order --grep <id>`) walks
//! every reachable commit on every show (~650 ms on the live repo). This
//! index remembers, per resolved spec id, which commits reference it, and
//! answers a show with a handful of small, bounded git calls:
//!
//! 1. T0 = the tip set `--all` admits (`rev-list --all --no-walk`), read
//!    ONCE; every later step names T0 explicitly, so the answer is for one
//!    ref state.
//! 2. Tips not in the stored frontier are walked incrementally (only the
//!    changed tips, `^frontier`), through the same BUG-546 resolver the
//!    collector applies, and appended as a new batch.
//! 3. The wanted ids' shards are read; hits are deduped by OID.
//! 4. Hits unreachable from T0 (a deleted unmerged branch) are dropped with
//!    one `rev-list --stdin <hits> ^<T0>`.
//! 5. Short SHA and subject come from Git at query time, through Git's own
//!    `-F --grep` over exactly the surviving hits, so the case-sensitive
//!    prefilter of the original query is reproduced by Git itself.
//! 6. Order (O2): committer time desc, then batch desc, then walk position
//!    asc. Each batch is walked `--date-order`, so a parent never sorts ahead
//!    of its same-second child; inside one batch the order is Git's.
//!
//! A cold index answers from ONE full `--date-order` walk over T0 and
//! writes itself from the same output. Everything else fails closed to the
//! ORIGINAL query, unchanged: unsupported history semantics (shallow,
//! grafts, replace refs or replace-control overrides), a missing or
//! mismatched `meta`, any git error or refusal (UNKNOWN, never an empty
//! answer), an unparseable shard line, or a step past its share of the
//! budget. Storage is hardened: 0700 directories and 0600 files with
//! explicit modes, owner and no-follow checks on every open, and a foreign
//! or group/world-writable entry is refused with an actionable message.
//!
//! Losing or corrupting the index changes speed only: it is derived data,
//! never canonical spec content.
// trace:BUG-2221 | ai:claude

use crate::*;
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// On-disk layout version of this module's files.
const FORMAT_VERSION: u32 = 1;

/// Behavior version of the commit-message resolver the index was built
/// with: [`crate::extract_spec_ids_from_commit`],
/// [`crate::extract_referenced_spec_ids_from_commit`],
/// [`crate::extract_trace_line_spec_ids`] and the helpers they call. BUMP
/// THIS whenever any of them changes: rows resolved under the old behavior
/// must never answer a query (a `meta` mismatch rebuilds). The
/// `parser_version_ratchet` test hashes their source and fails until this
/// const and its pinned hash are updated together.
// trace:BUG-2221 | ai:claude
pub(crate) const PARSER_VERSION: u32 = 1;

/// Namespace under the git COMMON dir, so linked worktrees share one index.
const INDEX_NAMESPACE: &str = "aida/linkage-index/v1";

/// Shards per generation, keyed by a hash of the upper-cased spec id.
const SHARD_COUNT: usize = 256;

/// Byte cap on any single index file read (a shard is ~20 KB on the live
/// repo; the frontier ~120 KB).
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// What the index made of a show's commit scan.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IndexAnswer {
    /// The complete referencing commits, (full, short, subject), O2 order.
    Answered(Vec<(String, String, String)>),
    /// The cold walk did not finish in budget: the scan is incomplete,
    /// exactly as the original query's timeout is.
    Incomplete,
    /// Fail closed: run the ORIGINAL query with whatever budget remains.
    RunOriginal,
}

/// Why a step stopped. Every variant fails closed to the original query;
/// the variant decides whether the index is also invalidated.
#[derive(Debug)]
enum Stop {
    /// Out of time for this step (no invalidation: the index is fine).
    Timeout,
    /// Unsupported history semantics, an unusable namespace, or a refused
    /// entry: the index is not consulted (no invalidation).
    Disabled,
    /// A git error or refusal, or a malformed index file: the answer is
    /// UNKNOWN and the index is rebuilt on a later show.
    Broken,
}

/// One (commit, resolved id) row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    oid: String,
    ct: i64,
    batch: u64,
    pos: u64,
    id: String,
}

impl Row {
    fn line(&self) -> String {
        format!(
            "{} {} {} {} {}\n",
            self.oid, self.ct, self.batch, self.pos, self.id
        )
    }

    /// Parse one COMPLETE shard line (without its newline).
    fn parse(line: &str) -> Option<Row> {
        let mut p = line.splitn(5, ' ');
        let oid = p.next()?;
        let ct = p.next()?.parse().ok()?;
        let batch = p.next()?.parse().ok()?;
        let pos = p.next()?.parse().ok()?;
        let id = p.next()?;
        if !git_arg_guard::is_hex_sha(oid) || id.is_empty() {
            return None;
        }
        Some(Row {
            oid: oid.to_string(),
            ct,
            batch,
            pos,
            id: id.to_string(),
        })
    }
}

/// Answer `ids` for the show path. `deadline` is the commit-scan budget
/// ([`crate::LINKAGE_COMMIT_LOG_TIMEOUT`] clipped to the show deadline).
/// The warm steps get half of it, so a [`IndexAnswer::RunOriginal`] from a
/// slow warm step leaves the original query at least the other half; the
/// cold walk, which replaces the original query, may use all of it.
// trace:BUG-2221 | ai:claude
pub(crate) fn query_linkage_index(
    project_root: &Path,
    ids: &[String],
    deadline: Instant,
) -> IndexAnswer {
    let now = Instant::now();
    let warm_deadline = now + deadline.saturating_duration_since(now) / 2;
    query_inner(project_root, ids, warm_deadline, deadline)
}

fn query_inner(
    project_root: &Path,
    ids: &[String],
    warm_deadline: Instant,
    deadline: Instant,
) -> IndexAnswer {
    let ctx = match read_context(project_root, warm_deadline) {
        Ok(ctx) => ctx,
        Err(_) => return IndexAnswer::RunOriginal,
    };
    let wanted: HashSet<String> = ids.iter().map(|s| s.to_ascii_uppercase()).collect();
    // Every stop runs the original; a broken index was already invalidated.
    warm_or_cold(project_root, ids, &wanted, &ctx, warm_deadline, deadline)
        .unwrap_or(IndexAnswer::RunOriginal)
}

fn warm_or_cold(
    project_root: &Path,
    ids: &[String],
    wanted: &HashSet<String>,
    ctx: &Context,
    warm_deadline: Instant,
    deadline: Instant,
) -> Result<IndexAnswer, Stop> {
    let Some(ns) = namespace_dir(&ctx.common_dir, false)? else {
        return cold(project_root, ids, wanted, ctx, deadline);
    };
    let meta = match read_meta(&ns) {
        Ok(Some(meta)) => meta,
        Ok(None) => return cold(project_root, ids, wanted, ctx, deadline),
        Err(Stop::Broken) => {
            invalidate(&ns, None);
            return Err(Stop::Broken);
        }
        Err(e) => return Err(e),
    };
    if meta.identity != ctx.identity() {
        invalidate(&ns, Some(&meta.generation));
        return Err(Stop::Broken);
    }
    let answer = warm(
        project_root,
        ids,
        wanted,
        ctx,
        &ns,
        &meta.generation,
        warm_deadline,
    );
    if let Err(Stop::Broken) = answer {
        invalidate(&ns, Some(&meta.generation));
    }
    answer
}

// ---------------------------------------------------------------------------
// Context: T0, unsupported semantics, meta identity
// ---------------------------------------------------------------------------

struct Context {
    common_dir: PathBuf,
    object_format: String,
    commit_encoding: String,
    log_output_encoding: String,
    /// T0: the tips `--all` admits (other worktrees' HEADs included),
    /// deduped, in Git's order.
    tips: Vec<String>,
}

impl Context {
    fn identity(&self) -> String {
        format!(
            "format={FORMAT_VERSION}\nparser={PARSER_VERSION}\nobject_format={}\n\
             commit_encoding={}\nlog_output_encoding={}\n",
            self.object_format, self.commit_encoding, self.log_output_encoding
        )
    }
}

/// Read T0 and everything the index's validity depends on, concurrently.
/// Any failed or refused read is [`Stop::Disabled`]: UNKNOWN, never empty.
fn read_context(project_root: &Path, deadline: Instant) -> Result<Context, Stop> {
    // Replace-control overrides change which objects Git serves.
    if std::env::var_os("GIT_NO_REPLACE_OBJECTS").is_some()
        || std::env::var_os("GIT_REPLACE_REF_BASE").is_some()
        || std::env::var_os("GIT_GRAFT_FILE").is_some()
    {
        return Err(Stop::Disabled);
    }
    let (dirs, config, tips, replace) = std::thread::scope(|s| {
        let dirs = s.spawn(|| {
            git_run(
                project_root,
                &[
                    "rev-parse",
                    "--git-common-dir",
                    "--show-object-format",
                    "--is-shallow-repository",
                ],
                None,
                deadline,
            )
        });
        let config = s.spawn(|| git_run(project_root, &["config", "--list", "-z"], None, deadline));
        let replace = s.spawn(|| {
            git_run(
                project_root,
                &["for-each-ref", "--format=%(refname)", "refs/replace/"],
                None,
                deadline,
            )
        });
        let tips = git_run(
            project_root,
            &["rev-list", "--all", "--no-walk=unsorted"],
            None,
            deadline,
        );
        (
            dirs.join().unwrap_or(Err(Stop::Broken)),
            config.join().unwrap_or(Err(Stop::Broken)),
            tips,
            replace.join().unwrap_or(Err(Stop::Broken)),
        )
    });
    let (dirs, config, tips, replace) = (
        dirs.map_err(|_| Stop::Disabled)?,
        config.map_err(|_| Stop::Disabled)?,
        tips.map_err(|_| Stop::Disabled)?,
        replace.map_err(|_| Stop::Disabled)?,
    );
    if !replace.trim().is_empty() {
        return Err(Stop::Disabled);
    }
    let mut lines = dirs.lines();
    let (Some(common_raw), Some(object_format), Some(shallow)) =
        (lines.next(), lines.next(), lines.next())
    else {
        return Err(Stop::Disabled);
    };
    if shallow.trim() != "false" {
        return Err(Stop::Disabled);
    }
    let common_dir = {
        let p = PathBuf::from(common_raw.trim());
        let p = if p.is_absolute() {
            p
        } else {
            project_root.join(p)
        };
        std::fs::canonicalize(&p).map_err(|_| Stop::Disabled)?
    };
    let grafts = common_dir.join("info/grafts");
    if std::fs::metadata(&grafts).is_ok_and(|m| m.len() > 0) {
        return Err(Stop::Disabled);
    }
    let mut commit_encoding = String::new();
    let mut log_output_encoding = String::new();
    for entry in config.split('\0') {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        match key.to_ascii_lowercase().as_str() {
            "i18n.commitencoding" => commit_encoding = value.to_string(),
            "i18n.logoutputencoding" => log_output_encoding = value.to_string(),
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    let mut t0 = Vec::new();
    for oid in tips.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !git_arg_guard::is_hex_sha(oid) {
            return Err(Stop::Disabled);
        }
        if seen.insert(oid) {
            t0.push(oid.to_string());
        }
    }
    Ok(Context {
        common_dir,
        object_format: object_format.trim().to_string(),
        commit_encoding,
        log_output_encoding,
        tips: t0,
    })
}

/// One bounded git call with optional stdin lines. A spawn failure, a
/// non-zero exit (a refusal included) or malformed output is
/// [`Stop::Broken`] — UNKNOWN, never an empty answer; running out of time
/// is [`Stop::Timeout`].
fn git_run(
    project_root: &Path,
    args: &[&str],
    stdin: Option<&[String]>,
    deadline: Instant,
) -> Result<String, Stop> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Stop::Timeout);
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(project_root).args(args);
    if let Some(lines) = stdin {
        let mut file = tempfile::tempfile().map_err(|_| Stop::Broken)?;
        for line in lines {
            writeln!(file, "{line}").map_err(|_| Stop::Broken)?;
        }
        file.rewind().map_err(|_| Stop::Broken)?;
        cmd.stdin(std::process::Stdio::from(file));
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    match command_output_with_timeout_detail(cmd, left) {
        // Non-UTF-8 message bytes are read lossily, as the original does.
        BoundedCommandOutput::Completed(o) if o.status.success() => {
            Ok(String::from_utf8_lossy(&o.stdout).into_owned())
        }
        BoundedCommandOutput::TimedOut => Err(Stop::Timeout),
        _ => Err(Stop::Broken),
    }
}

// ---------------------------------------------------------------------------
// Walks: the same BUG-546 resolver the collector applies
// ---------------------------------------------------------------------------

/// The upper-cased ids a commit message resolves to (trailers, referenced
/// ids, `trace:` lines) — the collector's filter, minus Git's prefilter.
fn resolve_ids(message: &str) -> Vec<String> {
    let mut resolved = extract_spec_ids_from_commit(message);
    resolved.extend(extract_referenced_spec_ids_from_commit(message));
    resolved.extend(extract_trace_line_spec_ids(message));
    let mut out: Vec<String> = Vec::new();
    for id in resolved {
        let id = id.to_ascii_uppercase();
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Walk `revs` (`--date-order`, NUL-separated records) and resolve every
/// commit into rows of batch `batch`, positions in walk order.
fn walk(
    project_root: &Path,
    revs: &[String],
    batch: u64,
    deadline: Instant,
) -> Result<Vec<Row>, Stop> {
    let out = git_run(
        project_root,
        &[
            "log",
            "--date-order",
            "-z",
            "--format=%H %ct%n%B",
            "--stdin",
        ],
        Some(revs),
        deadline,
    )?;
    let mut rows = Vec::new();
    for (pos, record) in out.split('\0').enumerate() {
        if record.is_empty() {
            continue;
        }
        let (head, message) = record.split_once('\n').unwrap_or((record, ""));
        let (oid, ct) = head.split_once(' ').ok_or(Stop::Broken)?;
        let ct: i64 = ct.parse().map_err(|_| Stop::Broken)?;
        if !git_arg_guard::is_hex_sha(oid) {
            return Err(Stop::Broken);
        }
        for id in resolve_ids(message) {
            rows.push(Row {
                oid: oid.to_string(),
                ct,
                batch,
                pos: pos as u64,
                id,
            });
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Warm and cold answers
// ---------------------------------------------------------------------------

fn warm(
    project_root: &Path,
    ids: &[String],
    wanted: &HashSet<String>,
    ctx: &Context,
    ns: &Path,
    generation: &str,
    deadline: Instant,
) -> Result<IndexAnswer, Stop> {
    let gen_dir = secure_subdir(ns, generation, false)?.ok_or(Stop::Broken)?;
    let frontier = read_frontier(&gen_dir)?;
    let known: HashSet<&str> = frontier.tips.iter().map(String::as_str).collect();
    let changed: Vec<String> = ctx
        .tips
        .iter()
        .filter(|t| !known.contains(t.as_str()))
        .cloned()
        .collect();

    // Step 2: incremental walk of the changed tips only.
    let mut fresh: Vec<Row> = Vec::new();
    if !changed.is_empty() {
        let mut revs = changed;
        revs.extend(frontier.tips.iter().map(|t| format!("^{t}")));
        let batch = frontier.batch + 1;
        fresh = match walk(project_root, &revs, batch, deadline) {
            Ok(rows) => rows,
            // A walk too large for the warm share (a long-idle index) is
            // rebuilt cold on a later show rather than retried forever.
            Err(Stop::Timeout) => return Err(Stop::Broken),
            Err(e) => return Err(e),
        };
        if let Some(_lock) = try_lock(ns)? {
            // Under the lock, re-read what decides whether this batch may be
            // appended: a concurrent rebuild or update wins, and this show
            // keeps its rows in memory only.
            let still_current = read_meta(ns)
                .ok()
                .flatten()
                .is_some_and(|m| m.generation == generation)
                && read_frontier(&gen_dir).is_ok_and(|f| f == frontier);
            if still_current {
                // The appends land past each shard's committed prefix and the
                // frontier rename commits them. A failed write loses speed
                // only: this show answers from `fresh`.
                let _ = append_rows(&gen_dir, &frontier, &fresh).and_then(|shards| {
                    write_frontier(
                        &gen_dir,
                        &Frontier {
                            batch,
                            shards,
                            tips: ctx.tips.clone(),
                        },
                    )
                });
            }
        }
    }

    // Step 3: the wanted shards' committed rows, plus this show's batch.
    let mut rows: Vec<Row> = Vec::new();
    let mut duplicated = false;
    for shard in shards_for(wanted) {
        let (shard_rows, dup) = read_shard(&gen_dir, shard, &frontier.shards[shard])?;
        duplicated |= dup;
        rows.extend(shard_rows.into_iter().filter(|r| wanted.contains(&r.id)));
    }
    rows.extend(fresh.into_iter().filter(|r| wanted.contains(&r.id)));
    let mut hits = dedupe(rows);

    // Step 4: drop hits no tip of T0 reaches any more.
    if !hits.is_empty() {
        let mut revs: Vec<String> = hits.iter().map(|r| r.oid.clone()).collect();
        revs.extend(ctx.tips.iter().map(|t| format!("^{t}")));
        let out = git_run(
            project_root,
            &["rev-list", "--stdin"],
            Some(&revs),
            deadline,
        )?;
        let unreachable: HashSet<&str> = out.lines().map(str::trim).collect();
        hits.retain(|r| !unreachable.contains(r.oid.as_str()));
    }

    let answer = finish(project_root, ids, hits, deadline)?;
    if duplicated {
        // Re-walked rows (a restored ref) now outnumber unique ones: the
        // next show rebuilds, which compacts. This answer stands.
        invalidate(ns, Some(generation));
    }
    Ok(IndexAnswer::Answered(answer))
}

/// No index yet: one full `--date-order` walk over T0 answers the query and
/// (when the lock is free) writes a new generation from the same output.
fn cold(
    project_root: &Path,
    ids: &[String],
    wanted: &HashSet<String>,
    ctx: &Context,
    deadline: Instant,
) -> Result<IndexAnswer, Stop> {
    let rows = match walk(project_root, &ctx.tips, 0, deadline) {
        Ok(rows) => rows,
        Err(Stop::Timeout) => return Ok(IndexAnswer::Incomplete),
        Err(e) => return Err(e),
    };
    // Write first: the build is cheap next to the walk, and a write failure
    // (read-only or contended repo) costs only the next show's speed.
    if let Ok(Some(ns)) = namespace_dir(&ctx.common_dir, true) {
        if let Ok(Some(_lock)) = try_lock(&ns) {
            let _ = build_generation(&ns, ctx, &rows);
        }
    }
    let hits = dedupe(
        rows.into_iter()
            .filter(|r| wanted.contains(&r.id))
            .collect(),
    );
    match finish(project_root, ids, hits, deadline) {
        Ok(answer) => Ok(IndexAnswer::Answered(answer)),
        Err(Stop::Timeout) => Ok(IndexAnswer::Incomplete),
        Err(e) => Err(e),
    }
}

/// One row per OID, keeping the earliest (batch, pos) occurrence.
fn dedupe(rows: Vec<Row>) -> Vec<Row> {
    let mut by_oid: HashMap<String, Row> = HashMap::new();
    for row in rows {
        match by_oid.get(&row.oid) {
            Some(kept) if (kept.batch, kept.pos) <= (row.batch, row.pos) => {}
            _ => {
                by_oid.insert(row.oid.clone(), row);
            }
        }
    }
    by_oid.into_values().collect()
}

/// Step 5 and 6: Git's own `-F --grep` over exactly the hits, which also
/// supplies the short SHA and subject; then the O2 order.
fn finish(
    project_root: &Path,
    ids: &[String],
    mut hits: Vec<Row>,
    deadline: Instant,
) -> Result<Vec<(String, String, String)>, Stop> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let revs: Vec<String> = hits.iter().map(|r| r.oid.clone()).collect();
    let grep: Vec<String> = ids.iter().map(|id| format!("--grep={id}")).collect();
    let mut args: Vec<&str> = vec!["rev-list", "--no-walk=unsorted", "-F"];
    args.extend(grep.iter().map(String::as_str));
    args.extend(["--format=%H %h %s", "--stdin"]);
    let out = git_run(project_root, &args, Some(&revs), deadline)?;
    // `rev-list --format` prints a `commit <oid>` header, then the format
    // line (a subject is one line).
    let requested: HashSet<&str> = revs.iter().map(String::as_str).collect();
    let mut meta: HashMap<String, (String, String)> = HashMap::new();
    let mut lines = out.lines();
    while let Some(header) = lines.next() {
        let (Some(oid), Some(line)) = (header.strip_prefix("commit "), lines.next()) else {
            return Err(Stop::Broken);
        };
        let mut p = line.splitn(3, ' ');
        let (Some(full), Some(short)) = (p.next(), p.next()) else {
            return Err(Stop::Broken);
        };
        if full != oid || !requested.contains(full) {
            return Err(Stop::Broken);
        }
        let subject = p.next().unwrap_or("").to_string();
        meta.insert(full.to_string(), (short.to_string(), subject));
    }
    hits.retain(|r| meta.contains_key(&r.oid));
    hits.sort_by(|a, b| {
        b.ct.cmp(&a.ct)
            .then(b.batch.cmp(&a.batch))
            .then(a.pos.cmp(&b.pos))
    });
    Ok(hits
        .into_iter()
        .map(|r| {
            let (short, subject) = meta.remove(&r.oid).unwrap_or_default();
            (r.oid, short, subject)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

struct Meta {
    /// Everything but `generation`, compared verbatim to [`Context::identity`].
    identity: String,
    generation: String,
}

/// A shard's COMMITTED prefix: its length and SHA-256. Readers trust only
/// this prefix; bytes past it (an uncommitted or torn append) are ignored
/// and cut back by the next writer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Committed {
    len: u64,
    sha: String,
}

/// The tip set as of the last completed batch, the batch counter, and every
/// shard's committed prefix. Renaming a new frontier into place is what
/// commits a batch.
///
/// No file is fsynced (a fsync measured ~40 ms per file on the live host;
/// a cold build writes 256 shards). Durability comes from verification
/// instead: after a crash, a lost or short shard fails its committed-prefix
/// check, and a lost frontier or meta fails to parse or is absent, so the
/// index is rebuilt; it never answers from a prefix it cannot verify.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frontier {
    batch: u64,
    shards: Vec<Committed>,
    tips: Vec<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn shard_name(shard: usize) -> String {
    format!("{shard:02x}")
}

fn shard_of(id: &str) -> usize {
    use sha2::{Digest, Sha256};
    Sha256::digest(id.to_ascii_uppercase().as_bytes())[0] as usize % SHARD_COUNT
}

fn shards_for(wanted: &HashSet<String>) -> Vec<usize> {
    let mut shards: Vec<usize> = wanted.iter().map(|id| shard_of(id)).collect();
    shards.sort_unstable();
    shards.dedup();
    shards
}

fn is_generation_name(name: &str) -> bool {
    name.strip_prefix("gen-")
        .is_some_and(|hex| hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Refusals are reported once per process: the message names the path and
/// the fix; the show itself answers through the original query.
fn refuse(path: &Path, why: &str) -> Stop {
    static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "aida: not using the commit-linkage index at {}: {why}. \
             Remove it (rm -rf '{}') and the next `aida show` rebuilds it.",
            path.display(),
            path.display()
        );
    }
    Stop::Disabled
}

/// Owned by this uid and not writable by group or others.
#[cfg(unix)]
fn trusted(md: &std::fs::Metadata) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    if md.uid() != unsafe { libc::geteuid() } {
        return Err("it is owned by another user");
    }
    if md.mode() & 0o022 != 0 {
        return Err("it is writable by group or others");
    }
    Ok(())
}

#[cfg(not(unix))]
fn trusted(_md: &std::fs::Metadata) -> Result<(), &'static str> {
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// `parent/name` as a trusted real directory (never a symlink). `create`
/// makes it 0700 with an explicit chmod, regardless of umask. `Ok(None)`:
/// absent and not created.
fn secure_subdir(parent: &Path, name: &str, create: bool) -> Result<Option<PathBuf>, Stop> {
    let dir = parent.join(name);
    match std::fs::symlink_metadata(&dir) {
        Ok(md) => {
            if !md.is_dir() {
                return Err(refuse(&dir, "it is not a plain directory"));
            }
            trusted(&md).map_err(|why| refuse(&dir, why))?;
            Ok(Some(dir))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(None);
            }
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => set_mode(&dir, 0o700).map_err(|_| Stop::Disabled)?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Stop::Disabled),
            }
            secure_subdir(parent, name, false)
        }
        Err(_) => Err(Stop::Disabled),
    }
}

/// The `aida/linkage-index/v1` namespace below the git common dir, every
/// component checked (and, with `create`, made) trusted.
fn namespace_dir(common_dir: &Path, create: bool) -> Result<Option<PathBuf>, Stop> {
    let mut dir = common_dir.to_path_buf();
    for component in INDEX_NAMESPACE.split('/') {
        match secure_subdir(&dir, component, create)? {
            Some(d) => dir = d,
            None => return Ok(None),
        }
    }
    Ok(Some(dir))
}

/// The open failed because `O_NOFOLLOW` met a symbolic link.
fn is_symlink_refusal(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Open an index file without following a final symlink, then check it is
/// a trusted regular file. `Ok(None)` only when it does not exist.
fn open_secure(
    path: &Path,
    opts: &mut std::fs::OpenOptions,
) -> Result<Option<std::fs::File>, Stop> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if is_symlink_refusal(&e) => {
            return Err(refuse(path, "it is a symbolic link"));
        }
        Err(_) => return Err(Stop::Broken),
    };
    let md = file.metadata().map_err(|_| Stop::Broken)?;
    if !md.is_file() {
        return Err(refuse(path, "it is not a regular file"));
    }
    trusted(&md).map_err(|why| refuse(path, why))?;
    Ok(Some(file))
}

fn read_secure(path: &Path) -> Result<Option<Vec<u8>>, Stop> {
    let Some(file) = open_secure(path, std::fs::OpenOptions::new().read(true))? else {
        return Ok(None);
    };
    let mut buf = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|_| Stop::Broken)?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Err(Stop::Broken);
    }
    Ok(Some(buf))
}

fn read_secure_text(path: &Path) -> Result<Option<String>, Stop> {
    read_secure(path)?
        .map(|bytes| String::from_utf8(bytes).map_err(|_| Stop::Broken))
        .transpose()
}

/// Create a NEW file (never an existing one or a link) with mode 0600 set
/// explicitly, and write `bytes`.
fn create_new(path: &Path, bytes: &[u8]) -> Result<(), Stop> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts.open(path).map_err(|_| Stop::Broken)?;
    set_mode(path, 0o600).map_err(|_| Stop::Broken)?;
    file.write_all(bytes).map_err(|_| Stop::Broken)
}

/// Replace `dir/name` atomically: a 0600 temp file in `dir`, renamed over
/// the target (see [`Frontier`] for why nothing is fsynced).
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), Stop> {
    let mut tmp = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempfile_in(dir)
        .map_err(|_| Stop::Broken)?;
    set_mode(tmp.path(), 0o600).map_err(|_| Stop::Broken)?;
    tmp.write_all(bytes).map_err(|_| Stop::Broken)?;
    tmp.persist(dir.join(name)).map_err(|_| Stop::Broken)?;
    Ok(())
}

/// The writer lock: a 0600 `lock` file, non-blocking exclusive `flock`
/// (released by the kernel if the holder dies). `Ok(None)` = held.
fn try_lock(ns: &Path) -> Result<Option<std::fs::File>, Stop> {
    let path = ns.join("lock");
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let lock = open_secure(&path, &mut opts)?.ok_or(Stop::Disabled)?;
    set_mode(&path, 0o600).map_err(|_| Stop::Disabled)?;
    Ok(fs2::FileExt::try_lock_exclusive(&lock)
        .is_ok()
        .then_some(lock))
}

fn read_meta(ns: &Path) -> Result<Option<Meta>, Stop> {
    let Some(text) = read_secure_text(&ns.join("meta"))? else {
        return Ok(None);
    };
    let mut identity = String::new();
    let mut generation = None;
    for line in text.lines() {
        match line.strip_prefix("generation=") {
            Some(g) => generation = Some(g.to_string()),
            None => {
                identity.push_str(line);
                identity.push('\n');
            }
        }
    }
    let generation = generation
        .filter(|g| is_generation_name(g))
        .ok_or(Stop::Broken)?;
    Ok(Some(Meta {
        identity,
        generation,
    }))
}

/// `batch=N`, then one `shard=<len> <sha256>` line per shard in order, then
/// one tip OID per line.
fn read_frontier(gen_dir: &Path) -> Result<Frontier, Stop> {
    let text = read_secure_text(&gen_dir.join("frontier"))?.ok_or(Stop::Broken)?;
    let mut lines = text.lines();
    let batch = lines
        .next()
        .and_then(|l| l.strip_prefix("batch="))
        .and_then(|b| b.parse().ok())
        .ok_or(Stop::Broken)?;
    let mut shards = Vec::with_capacity(SHARD_COUNT);
    for line in lines.by_ref().take(SHARD_COUNT) {
        let (len, sha) = line
            .strip_prefix("shard=")
            .and_then(|rest| rest.split_once(' '))
            .ok_or(Stop::Broken)?;
        let len = len.parse().map_err(|_| Stop::Broken)?;
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Stop::Broken);
        }
        shards.push(Committed {
            len,
            sha: sha.to_string(),
        });
    }
    if shards.len() != SHARD_COUNT {
        return Err(Stop::Broken);
    }
    let mut tips = Vec::new();
    for tip in lines {
        if !git_arg_guard::is_hex_sha(tip) {
            return Err(Stop::Broken);
        }
        tips.push(tip.to_string());
    }
    Ok(Frontier {
        batch,
        shards,
        tips,
    })
}

fn write_frontier(gen_dir: &Path, frontier: &Frontier) -> Result<(), Stop> {
    let mut text = format!("batch={}\n", frontier.batch);
    for c in &frontier.shards {
        text.push_str(&format!("shard={} {}\n", c.len, c.sha));
    }
    for tip in &frontier.tips {
        text.push_str(tip);
        text.push('\n');
    }
    write_atomic(gen_dir, "frontier", text.as_bytes())
}

fn shard_path(gen_dir: &Path, shard: usize) -> PathBuf {
    gen_dir.join("shards").join(shard_name(shard))
}

/// The verified committed prefix of a shard's bytes.
fn committed_prefix<'a>(bytes: &'a [u8], committed: &Committed) -> Result<&'a [u8], Stop> {
    let len = usize::try_from(committed.len).map_err(|_| Stop::Broken)?;
    let prefix = bytes.get(..len).ok_or(Stop::Broken)?;
    if sha256_hex(prefix) != committed.sha {
        return Err(Stop::Broken);
    }
    Ok(prefix)
}

/// Read one shard's committed rows. Every shard of a generation exists, so
/// a missing one, a prefix that fails verification, or an unparseable line
/// is [`Stop::Broken`]; bytes past the prefix (a torn or uncommitted
/// append) are ignored. The flag is true when duplicate rows outnumber
/// unique ones.
fn read_shard(
    gen_dir: &Path,
    shard: usize,
    committed: &Committed,
) -> Result<(Vec<Row>, bool), Stop> {
    let bytes = read_secure(&shard_path(gen_dir, shard))?.ok_or(Stop::Broken)?;
    let prefix = committed_prefix(&bytes, committed)?;
    let text = std::str::from_utf8(prefix).map_err(|_| Stop::Broken)?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(Stop::Broken);
    }
    let mut rows = Vec::new();
    let mut unique: HashSet<(String, String)> = HashSet::new();
    for line in text.lines() {
        let row = Row::parse(line).ok_or(Stop::Broken)?;
        unique.insert((row.oid.clone(), row.id.clone()));
        rows.push(row);
    }
    let duplicated = rows.len() > 2 * unique.len();
    Ok((rows, duplicated))
}

/// Append rows past each touched shard's committed prefix (cutting back any
/// torn or uncommitted tail first) and return the new committed prefixes.
/// Caller holds the lock and commits them with the frontier.
fn append_rows(gen_dir: &Path, frontier: &Frontier, rows: &[Row]) -> Result<Vec<Committed>, Stop> {
    let mut by_shard: HashMap<usize, String> = HashMap::new();
    for row in rows {
        by_shard
            .entry(shard_of(&row.id))
            .or_default()
            .push_str(&row.line());
    }
    let mut shards = frontier.shards.clone();
    for (shard, text) in by_shard {
        let path = shard_path(gen_dir, shard);
        let mut file = open_secure(&path, std::fs::OpenOptions::new().read(true).write(true))?
            .ok_or(Stop::Broken)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|_| Stop::Broken)?;
        let mut content = committed_prefix(&bytes, &shards[shard])?.to_vec();
        file.set_len(content.len() as u64)
            .map_err(|_| Stop::Broken)?;
        file.seek(std::io::SeekFrom::Start(content.len() as u64))
            .map_err(|_| Stop::Broken)?;
        file.write_all(text.as_bytes()).map_err(|_| Stop::Broken)?;
        content.extend_from_slice(text.as_bytes());
        shards[shard] = Committed {
            len: content.len() as u64,
            sha: sha256_hex(&content),
        };
    }
    Ok(shards)
}

/// Write a complete new generation (all shards, then the frontier), then
/// point `meta` at it, then remove other generations. Caller holds the lock.
fn build_generation(ns: &Path, ctx: &Context, rows: &[Row]) -> Result<(), Stop> {
    let seed = format!(
        "{}-{:?}-{:?}",
        std::process::id(),
        std::time::SystemTime::now(),
        Instant::now()
    );
    let generation = format!("gen-{}", &sha256_hex(seed.as_bytes())[..16]);
    let gen_dir = secure_subdir(ns, &generation, true)?.ok_or(Stop::Broken)?;
    let shard_dir = secure_subdir(&gen_dir, "shards", true)?.ok_or(Stop::Broken)?;
    let mut texts: Vec<String> = vec![String::new(); SHARD_COUNT];
    for row in rows {
        texts[shard_of(&row.id)].push_str(&row.line());
    }
    let mut shards = Vec::with_capacity(SHARD_COUNT);
    for (shard, text) in texts.iter().enumerate() {
        create_new(&shard_dir.join(shard_name(shard)), text.as_bytes())?;
        shards.push(Committed {
            len: text.len() as u64,
            sha: sha256_hex(text.as_bytes()),
        });
    }
    write_frontier(
        &gen_dir,
        &Frontier {
            batch: 0,
            shards,
            tips: ctx.tips.clone(),
        },
    )?;
    let meta = format!("{}generation={generation}\n", ctx.identity());
    write_atomic(ns, "meta", meta.as_bytes())?;
    remove_other_generations(ns, &generation);
    Ok(())
}

/// Remove stale generation directories (strict names, trusted real
/// directories only). Caller holds the lock.
fn remove_other_generations(ns: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(ns) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == keep || !is_generation_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if md.is_dir() && trusted(&md).is_ok() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Drop `meta` so the next show rebuilds cold. With `generation`, only
/// while `meta` still names it: a reader that lost a race with a rebuild
/// never discards the NEW generation. Readers that already resolved a
/// generation finish against it; a rebuild writes a new directory.
fn invalidate(ns: &Path, generation: Option<&str>) {
    if let Some(generation) = generation {
        if !read_meta(ns).is_ok_and(|m| m.is_some_and(|m| m.generation == generation)) {
            return;
        }
    }
    let meta = ns.join("meta");
    if std::fs::symlink_metadata(&meta).is_ok_and(|md| md.is_file() && trusted(&md).is_ok()) {
        let _ = std::fs::remove_file(&meta);
    }
}

#[cfg(test)]
#[path = "tests/bug_2221_linkage_index_tests.rs"]
mod bug_2221_linkage_index_tests;
