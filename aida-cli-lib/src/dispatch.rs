//! Typed dispatch requests (`aida dispatch`).
//!
//! A sandboxed advisor seat queues closed, validated requests (`flow`,
//! `review`, `take`, `impl`, `train`) under `<main_worktree>/.aida/dispatch/`.
//! Request write time enforces an active Mandate covering [`Scope::Dispatch`]
//! (plus [`Scope::ShipHeld`] for shipping `flow` and `train`), [`HumanGates`],
//! the `operator-only` / `ExecutionMode::Decide` always-human floor, and a
//! process/session-key-bound `advisor` seat grant ([`grant_check`]).
//!
//! Per the signed SKETCH v1 and ADVISOR SIGNOFF:
//! - No self-binding grant guard: no code path ever appends `proc:` tokens to
//!   an advisor grant outside `aida role enter` / `--bind-pid`.
//! - The cron/schedule `dispatch tick` executes only `take` and `impl`;
//!   `review`, `flow`, and `train` require `aida dispatch serve` started by the
//!   human from an advisor terminal after `eval "$(aida role enter advisor ...)"`,
//!   and `aida dispatch list` surfaces them as `waiting for serve` while
//!   `Pending`.
// trace:STORY-1917 | ai:agy
// mandate-gate

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[path = "dispatch_args.rs"]
pub(crate) mod dispatch_args;
pub(crate) use dispatch_args::*;

use crate::gate::seat_authority::{self, GrantCheck, GrantRejection};
use crate::mandate::{self, Mandate, MandateState, Scope};
use crate::mandate_drive::{ActionRow, Outcome};
use crate::mandate_gates::{GateOp, HumanGates};

pub(crate) const DISPATCH_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_BRIEF_BYTES: usize = 64 * 1024;
pub(crate) const MAX_TRAIN_PRS: usize = 16;
pub(crate) const MAX_PR_NUMBER: u64 = 999_999;

const DISPATCH_DIR: &str = "dispatch";
const REQUESTS_SUBDIR: &str = "requests";
const BRIEFS_SUBDIR: &str = "briefs";
const RESULTS_SUBDIR: &str = "results";
const LOGS_SUBDIR: &str = "logs";
const TICK_LOCK_FILE: &str = "tick.lock";

/// On-disk request record (`requests/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchRequest {
    pub schema_version: u32,
    pub id: String,
    pub requested_at: DateTime<Utc>,
    pub principal: String,
    pub requester_seat: String,
    pub requester_grant_id: String,
    pub matched_binding: Vec<String>,
    pub mandate_id: String,
    pub dedupe_key: String,
    pub target_key: String,
    pub action: DispatchVerb,
}

/// Closed verbs persisted in [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum DispatchVerb {
    #[serde(alias = "pr-flow")]
    Flow {
        pr: u64,
        #[serde(default)]
        no_ship: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Review {
        pr: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "takeover")]
    Take {
        pr: u64,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "lane")]
    Impl {
        name: String,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Train {
        prs: Vec<u64>,
    },
}

impl DispatchVerb {
    pub(crate) fn verb_str(&self) -> &'static str {
        match self {
            Self::Flow { .. } => "flow",
            Self::Review { .. } => "review",
            Self::Take { .. } => "take",
            Self::Impl { .. } => "impl",
            Self::Train { .. } => "train",
        }
    }

    /// True when this verb requires the advisor seat grant at execution time
    /// and therefore runs only under `aida dispatch serve`, never under the
    /// cron/schedule `dispatch tick`.
    // trace:STORY-1917 | ai:agy
    pub(crate) fn requires_serve(&self) -> bool {
        matches!(
            self,
            Self::Flow { .. } | Self::Review { .. } | Self::Train { .. }
        )
    }

    /// True when this verb ships held PRs (`flow` without `--no-ship`, or `train`).
    pub(crate) fn ships_pr(&self) -> bool {
        matches!(self, Self::Flow { no_ship: false, .. } | Self::Train { .. })
    }

    /// True when this verb delegates a reviewer seat (`review`, or `flow`).
    pub(crate) fn delegates_reviewer(&self) -> bool {
        matches!(self, Self::Flow { .. } | Self::Review { .. })
    }

    pub(crate) fn target_key(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("pr:{pr}")
            }
            Self::Impl { name, spec, .. } => {
                format!("lane:{}:spec:{}", name, spec.to_ascii_uppercase())
            }
            Self::Train { .. } => "train".to_string(),
        }
    }

    pub(crate) fn dedupe_key(&self) -> String {
        match self {
            Self::Flow { pr, no_ship, .. } => format!("flow:pr:{pr}:noship:{no_ship}"),
            // Request queueing runs offline/store-less inside a sandboxed
            // advisor seat where forge/network queries for the current PR head
            // OID may be unavailable, so review deduplicates on active
            // (`Pending`/`Running`) requests for `pr:<pr>`; once a review
            // finishes (`Done`/`Failed`/`Refused`/`Cancelled`), a new review
            // request for a newer head queues immediately.
            Self::Review { pr, .. } => format!("review:pr:{pr}"),
            Self::Take {
                pr,
                branch,
                spec,
                brief_sha256,
                ..
            } => format!(
                "take:pr:{pr}:branch:{branch}:spec:{}:brief:{brief_sha256}",
                spec.to_ascii_uppercase()
            ),
            Self::Impl {
                name, branch, spec, ..
            } => format!(
                "impl:name:{name}:branch:{branch}:spec:{}",
                spec.to_ascii_uppercase()
            ),
            Self::Train { prs } => {
                let mut sorted = prs.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let csv = sorted
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("train:prs:{csv}")
            }
        }
    }

    pub(crate) fn subject_label(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("PR #{pr}")
            }
            Self::Impl { name, spec, .. } => format!("{name} ({})", spec.to_ascii_uppercase()),
            Self::Train { prs } => {
                let list = prs
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("PRs {list}")
            }
        }
    }
}

/// Execution status of a dispatch request.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DispatchStatus {
    Pending,
    Running,
    Done,
    Refused,
    Failed,
    Cancelled,
}

impl DispatchStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    pub(crate) fn matches_filter(self, filter: DispatchStatusFilter) -> bool {
        matches!(
            (self, filter),
            (Self::Pending, DispatchStatusFilter::Pending)
                | (Self::Running, DispatchStatusFilter::Running)
                | (Self::Done, DispatchStatusFilter::Done)
                | (Self::Refused, DispatchStatusFilter::Refused)
                | (Self::Failed, DispatchStatusFilter::Failed)
                | (Self::Cancelled, DispatchStatusFilter::Cancelled)
        )
    }
}

/// On-disk result record (`results/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchResult {
    pub id: String,
    pub verb: String,
    pub target: String,
    pub dedupe_key: String,
    pub status: DispatchStatus,
    pub requested_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub requester_grant_prefix: String,
    pub mandate_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<DateTime<Utc>>,
}

// ── Paths & Private File I/O ────────────────────────────────────────────────

pub(crate) fn dispatch_root(main_worktree: &Path) -> PathBuf {
    main_worktree.join(".aida").join(DISPATCH_DIR)
}

pub(crate) fn requests_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(REQUESTS_SUBDIR)
}

pub(crate) fn briefs_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(BRIEFS_SUBDIR)
}

pub(crate) fn results_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(RESULTS_SUBDIR)
}

pub(crate) fn logs_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(LOGS_SUBDIR)
}

pub(crate) fn request_path(main_worktree: &Path, id: &str) -> PathBuf {
    requests_dir(main_worktree).join(format!("{id}.json"))
}

pub(crate) fn result_path(main_worktree: &Path, id: &str) -> PathBuf {
    results_dir(main_worktree).join(format!("{id}.json"))
}

pub(crate) fn brief_path(main_worktree: &Path, id: &str) -> PathBuf {
    briefs_dir(main_worktree).join(format!("{id}.md"))
}

/// Exclusive flock over `.aida/dispatch/tick.lock`, explicitly unlocked on
/// Drop before closing the fd (BUG-2160).
struct DispatchLock(std::fs::File);

impl Drop for DispatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn acquire_dispatch_lock(main_worktree: &Path) -> Result<DispatchLock> {
    use fs2::FileExt;
    let dir = dispatch_root(main_worktree);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(TICK_LOCK_FILE))?;
    file.lock_exclusive()
        .context("failed to acquire .aida/dispatch/tick.lock")?;
    Ok(DispatchLock(file))
}

/// Write `body` to `path` as a 0600 regular file, atomically (temp + rename).
// trace:STORY-1917 | ai:agy
pub(crate) fn write_private_atomic(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("dispatch path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("dispatch"),
        Uuid::new_v4()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp)?;
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Open `path` with `O_NOFOLLOW | O_NONBLOCK`, verify it is a regular file
/// owned by the current uid with mode 0600 (when `require_0600` is true) and
/// size `<= max_bytes`, and read its bytes.
// trace:STORY-1917 | ai:agy
pub(crate) fn read_bounded_regular_file(
    path: &Path,
    max_bytes: usize,
    require_0600: bool,
) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                anyhow::anyhow!("{} is a symlink (refused)", path.display())
            } else {
                anyhow::anyhow!("cannot open {}: {e}", path.display())
            }
        })?;
    let meta = file
        .metadata()
        .with_context(|| format!("cannot stat {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            path.display(),
            meta.uid()
        );
    }
    if require_0600 {
        let mode = meta.mode() & 0o7777;
        if mode != 0o600 {
            bail!("{} has mode {:04o}, expected 0600", path.display(), mode);
        }
    }
    if meta.len() > max_bytes as u64 {
        bail!(
            "{} exceeds maximum size ({} bytes > {} bytes)",
            path.display(),
            meta.len(),
            max_bytes
        );
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take((max_bytes as u64) + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("cannot read {}", path.display()))?;
    if buf.len() > max_bytes {
        bail!(
            "{} exceeds maximum size ({} bytes)",
            path.display(),
            max_bytes
        );
    }
    Ok(buf)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ── Dual-End Validation ─────────────────────────────────────────────────────

/// Validate a PR number (`1..=999_999`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_pr(pr: u64) -> Result<u64> {
    if (1..=MAX_PR_NUMBER).contains(&pr) {
        Ok(pr)
    } else {
        bail!("invalid PR number `{pr}`: must be between 1 and {MAX_PR_NUMBER}")
    }
}

/// Validate a `train` PR list (`1..=16` items, each valid, deduplicated in order).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_prs(prs: &[u64]) -> Result<Vec<u64>> {
    if prs.is_empty() {
        bail!("train requires at least 1 PR number");
    }
    if prs.len() > MAX_TRAIN_PRS {
        bail!(
            "train accepts at most {MAX_TRAIN_PRS} PR numbers (got {})",
            prs.len()
        );
    }
    let mut out = Vec::with_capacity(prs.len());
    let mut seen = BTreeSet::new();
    for &pr in prs {
        validate_pr(pr)?;
        if seen.insert(pr) {
            out.push(pr);
        }
    }
    Ok(out)
}

/// Validate a branch name (`^[A-Za-z0-9._/-]{1,80}$` plus git check-ref-format
/// rules, forbidding `main` and `master`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_branch(branch: &str) -> Result<String> {
    let b = branch.trim();
    if b.is_empty() || b.len() > 80 {
        bail!("invalid branch `{branch}`: length must be 1..=80 characters");
    }
    if matches!(b, "main" | "master" | "HEAD") {
        bail!("invalid branch `{b}`: cannot target `{b}` directly");
    }
    if b.starts_with('/')
        || b.ends_with('/')
        || b.starts_with('.')
        || b.ends_with('.')
        || b.starts_with('-')
    {
        bail!("invalid branch `{b}`: cannot start or end with `/`, `.`, or `-`");
    }
    if b.contains("..") || b.contains("//") || b.contains("@{") || b.ends_with(".lock") {
        bail!("invalid branch `{b}`: contains forbidden git ref sequence");
    }
    for segment in b.split('/') {
        if segment.is_empty()
            || segment.starts_with('.')
            || segment.ends_with('.')
            || segment.ends_with(".lock")
        {
            bail!("invalid branch `{b}`: invalid path component `{segment}`");
        }
    }
    if !b
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        bail!("invalid branch `{b}`: only `[A-Za-z0-9._/-]` characters are allowed");
    }
    Ok(b.to_string())
}

/// Validate an `impl` lane / worktree name (`^[A-Za-z0-9._-]{1,60}$`, no `/`,
/// no leading `.` or `-`, no `..`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_lane_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 60 {
        bail!("invalid lane name `{name}`: length must be 1..=60 characters");
    }
    if n.starts_with('.') || n.starts_with('-') || n.ends_with('.') || n.contains("..") {
        bail!("invalid lane name `{n}`: cannot start with `.`/`-` or contain `..`");
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("invalid lane name `{n}`: only `[A-Za-z0-9._-]` characters are allowed (no `/`)");
    }
    Ok(n.to_string())
}

/// Validate a spec ID syntax (`^[A-Z][A-Z0-9]*-(?:[0-9]+-)?[0-9]{1,6}$`),
/// confirm it exists in the project's store, and verify it is not
/// `operator-only` or `ExecutionMode::Decide` (in the current store or sticky
/// exclusion ledger). Also checks `include_supervised` when the active mandate
/// excludes supervised specs.
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_spec_in_store(
    main_worktree: &Path,
    mandate_dir: &Path,
    raw_spec: &str,
    include_supervised: bool,
) -> Result<String> {
    let spec = raw_spec.trim().to_ascii_uppercase();
    let Some((prefix, rest)) = spec.split_once('-') else {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    };
    if prefix.is_empty()
        || !prefix.chars().next().unwrap().is_ascii_uppercase()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        bail!("invalid spec ID `{raw_spec}`: invalid prefix `{prefix}`");
    }
    let valid_number = match rest.split_once('-') {
        None => !rest.is_empty() && rest.len() <= 6 && rest.chars().all(|c| c.is_ascii_digit()),
        Some((node, seq)) => {
            !node.is_empty()
                && node.len() <= 4
                && node.chars().all(|c| c.is_ascii_digit())
                && !seq.is_empty()
                && seq.len() <= 6
                && seq.chars().all(|c| c.is_ascii_digit())
        }
    };
    if !valid_number {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    }

    let ledger = mandate::read_exclusions(mandate_dir)
        .map_err(|why| anyhow::anyhow!("mandate exclusion ledger is unreadable: {why}"))?;
    if let Some(entry) = ledger.get(&spec) {
        if entry.always_human {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger and always requires a human",
                entry.class
            );
        }
        if !include_supervised {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                entry.class
            );
        }
    }

    let store = crate::load_store_for_lookup(main_worktree)
        .with_context(|| format!("cannot load requirement store to validate `{spec}`"))?;
    let Some(req) = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
    }) else {
        bail!("spec `{spec}` was not found in the requirement store");
    };

    // Record any exclusions seen on this requirement into the sticky ledger.
    mandate::note_exclusions(mandate_dir, &spec, std::iter::once(req));

    if let Some(class) = mandate::always_human_class(req) {
        bail!("spec `{spec}` is `{class}` and always requires a human");
    }
    for alias in [req.spec_id.as_deref(), req.agreed_id.as_deref()]
        .into_iter()
        .flatten()
    {
        let key = alias.trim().to_ascii_uppercase();
        if let Some(entry) = ledger.get(&key) {
            if entry.always_human {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger and always requires a human",
                    entry.class
                );
            }
            if !include_supervised {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                    entry.class
                );
            }
        }
    }
    if !include_supervised {
        if let Some(why) = mandate::excluded_class(req) {
            bail!(
                "spec `{spec}` is in a supervised class ({why}); start the mandate with `--include-supervised` to cover it"
            );
        }
    }
    Ok(spec)
}

/// Read, validate, and freeze `--brief <path>` into `.aida/dispatch/briefs/<id>.md`
/// (mode 0600). Returns `(brief_relpath, brief_sha256)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn freeze_brief(
    main_worktree: &Path,
    request_id: &str,
    source_path: &Path,
) -> Result<(String, String)> {
    let bytes = read_bounded_regular_file(source_path, MAX_BRIEF_BYTES, false)?;
    if bytes.is_empty() {
        bail!("brief file {} is empty", source_path.display());
    }
    if bytes.contains(&0) {
        bail!("brief file {} contains NUL bytes", source_path.display());
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("brief file {} is not valid UTF-8", source_path.display()))?;
    if text.trim().is_empty() {
        bail!("brief file {} is blank", source_path.display());
    }
    let digest = sha256_hex(&bytes);
    let relpath = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    let dest = brief_path(main_worktree, request_id);
    write_private_atomic(&dest, &bytes)?;
    Ok((relpath, digest))
}

/// Execution-time re-validation of a frozen brief file on disk (`O_NOFOLLOW |
/// O_NONBLOCK`, mode 0600, owned by current uid, `<= 64 KiB`, non-empty UTF-8,
/// SHA-256 matches `expected_sha256`).
// trace:STORY-1917 | ai:agy
pub(crate) fn verify_frozen_brief(
    main_worktree: &Path,
    request_id: &str,
    brief_relpath: &str,
    expected_sha256: &str,
) -> Result<String> {
    let expected_rel = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    if brief_relpath != expected_rel {
        bail!("invalid brief_relpath `{brief_relpath}` (expected `{expected_rel}`)");
    }
    if expected_sha256.len() != 64
        || !expected_sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("invalid brief_sha256 `{expected_sha256}`");
    }
    let path = brief_path(main_worktree, request_id);
    let bytes = read_bounded_regular_file(&path, MAX_BRIEF_BYTES, true)?;
    if bytes.is_empty() || bytes.contains(&0) {
        bail!(
            "frozen brief {} is empty or contains NUL bytes",
            path.display()
        );
    }
    let actual_sha = sha256_hex(&bytes);
    if actual_sha != expected_sha256 {
        bail!(
            "frozen brief {} SHA-256 mismatch (expected {expected_sha256}, got {actual_sha})",
            path.display()
        );
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("frozen brief {} is not valid UTF-8", path.display()))?;
    Ok(text.to_string())
}

/// Execution-time re-validation of an on-disk [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
pub(crate) fn revalidate_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    include_supervised: bool,
) -> Result<()> {
    if req.schema_version != DISPATCH_SCHEMA_VERSION {
        bail!(
            "unsupported dispatch request schema_version {} (expected {})",
            req.schema_version,
            DISPATCH_SCHEMA_VERSION
        );
    }
    if Uuid::parse_str(&req.id).is_err() {
        bail!("invalid dispatch request id `{}`", req.id);
    }
    if req.requester_seat != mandate::MANDATE_SEAT {
        bail!(
            "dispatch request `{}` has requester_seat `{}`, expected `{}`",
            req.id,
            req.requester_seat,
            mandate::MANDATE_SEAT
        );
    }
    if req.matched_binding.is_empty() {
        bail!("dispatch request `{}` has an empty matched_binding", req.id);
    }
    match &req.action {
        DispatchVerb::Flow { pr, .. } | DispatchVerb::Review { pr, .. } => {
            validate_pr(*pr)?;
        }
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_pr(*pr)?;
            validate_branch(branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_lane_name(name)?;
            validate_branch(branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Train { prs } => {
            validate_prs(prs)?;
        }
    }
    let expected_target = req.action.target_key();
    if req.target_key != expected_target {
        bail!(
            "dispatch request `{}` target_key `{}` does not match `{expected_target}`",
            req.id,
            req.target_key
        );
    }
    let expected_dedupe = req.action.dedupe_key();
    if req.dedupe_key != expected_dedupe {
        bail!(
            "dispatch request `{}` dedupe_key `{}` does not match `{expected_dedupe}`",
            req.id,
            req.dedupe_key
        );
    }
    Ok(())
}

/// Render the `take` or `impl` prompt wrapper from the embedded template in
/// `aida-core/templates/dispatch/`.
// trace:STORY-1917 | ai:agy
pub(crate) fn render_lane_prompt(
    main_worktree: &Path,
    worktree: &Path,
    action: &DispatchVerb,
    brief_body: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let ts = now.format("%Y-%m-%d %H:%M").to_string();
    match action {
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/take-prompt.md")
                .context("missing embedded template dispatch/take-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{PR}}", &pr.to_string())
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        DispatchVerb::Impl {
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/impl-prompt.md")
                .context("missing embedded template dispatch/impl-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        other => bail!(
            "verb `{}` does not use a lane prompt template",
            other.verb_str()
        ),
    }
}

// ── Authority Verification at Request Write Time ────────────────────────────

/// Validated caller & mandate context for `aida dispatch request`.
#[derive(Debug, Clone)]
pub(crate) struct AuthorizedDispatchCaller {
    pub mandate: Mandate,
    pub grant_id: String,
    pub matched_binding: Vec<String>,
}

/// Check Mandate (`Scope::Dispatch` + `Scope::ShipHeld` when shipping),
/// `HumanGates` (`GateOp::Dispatch` + `GateOp::Ship`), and `advisor` seat grant
/// (bound to the calling process or its inherited session keyring).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_request_authority(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<AuthorizedDispatchCaller> {
    let (valid, _) = mandate::load_valid(mandate_dir, principal, now);
    let mandate = match mandate::state_at(&valid, now) {
        MandateState::None => {
            bail!(
                "no active mandate; start one at a human terminal with `aida mandate start --ship-held --scope dispatch` (or `--drive`)"
            )
        }
        MandateState::Expired(m) => {
            bail!(
                "mandate {} expired at {}; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id,
                m.expires_at.to_rfc3339()
            )
        }
        MandateState::Stopped(m) => {
            bail!(
                "mandate {} was stopped; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id
            )
        }
        MandateState::Active(m) => m,
    };

    if !mandate.covers(Scope::Dispatch) {
        bail!(
            "active mandate {} does not cover `dispatch`; stop it and start one with `aida mandate start --ship-held --scope dispatch` (or `--drive`)",
            mandate.id
        );
    }

    let ships_pr = matches!(
        verb,
        DispatchRequestVerb::Flow { no_ship: false, .. } | DispatchRequestVerb::Train { .. }
    );
    if ships_pr && !mandate.covers(Scope::ShipHeld) {
        bail!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            mandate.id,
            match verb {
                DispatchRequestVerb::Train { .. } => "train",
                _ => "flow",
            }
        );
    }

    let gates = crate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let msg = format!("dispatch is gated by human gates ({why})");
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Gated,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }
    if ships_pr {
        if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let msg = format!("shipping is gated by human gates ({why})");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Gated,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    }

    let grant = match seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(grant) => grant,
        GrantCheck::Absent => {
            let msg = "dispatch requests require a bound `advisor` seat grant (no session grant is set); enter the role before starting the advisor session with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`".to_string();
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let fix = match reason {
                GrantRejection::Unbound
                | GrantRejection::PresenterNotBound { .. }
                | GrantRejection::ForeignNamespace { .. }
                | GrantRejection::KeyringUnavailable { .. } => {
                    "A `--bind-pid` grant cannot present inside a bwrap tool call because the PID namespace hides the harness PID and no session key was inherited. Start the advisor session after running `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in the launching terminal so the kernel session keyring (`key:<sha256>`) is inherited into the sandbox."
                }
                _ => "Re-enter the advisor seat at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`.",
            };
            let seat_str = seat.as_deref().unwrap_or("unknown");
            let msg = format!("advisor seat grant {id} ({seat_str}) was rejected: {reason}. {fix}");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    };

    if crate::canonical_role_name(&grant.seat) != mandate::MANDATE_SEAT {
        let msg = format!(
            "dispatch requests require the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id, grant.seat
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    if grant.principal.trim() != mandate.principal.trim() {
        let msg = format!(
            "advisor grant principal `{}` does not match mandate principal `{}`",
            grant.principal, mandate.principal
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let delegates_reviewer = matches!(
        verb,
        DispatchRequestVerb::Flow { .. } | DispatchRequestVerb::Review { .. }
    );
    if delegates_reviewer
        && !grant
            .delegable_seats
            .iter()
            .any(|s| crate::canonical_role_name(s) == "reviewer")
    {
        let msg = format!(
            "advisor grant {} does not delegate the `reviewer` seat required by `{}`; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id,
            verb_name_of(verb)
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let presenter = seat_authority::presenter_tokens_for(&grant);
    let matched_binding = seat_authority::matched_binding(&grant, &presenter);
    if matched_binding.is_empty() {
        let msg = format!(
            "advisor grant {} has no presenter binding matching this process; start the advisor session after `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    Ok(AuthorizedDispatchCaller {
        mandate,
        grant_id: grant.id,
        matched_binding,
    })
}

fn verb_name_of(verb: &DispatchRequestVerb) -> &'static str {
    match verb {
        DispatchRequestVerb::Flow { .. } => "flow",
        DispatchRequestVerb::Review { .. } => "review",
        DispatchRequestVerb::Take { .. } => "take",
        DispatchRequestVerb::Impl { .. } => "impl",
        DispatchRequestVerb::Train { .. } => "train",
    }
}

fn verb_subject_of(verb: &DispatchRequestVerb) -> String {
    match verb {
        DispatchRequestVerb::Flow { pr, .. }
        | DispatchRequestVerb::Review { pr, .. }
        | DispatchRequestVerb::Take { pr, .. } => format!("PR #{pr}"),
        DispatchRequestVerb::Impl { name, spec, .. } => {
            format!("{name} ({})", spec.trim().to_ascii_uppercase())
        }
        DispatchRequestVerb::Train { prs } => {
            let list = prs
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("PRs {list}")
        }
    }
}

fn emit_dispatch_event(
    main_worktree: &Path,
    mandate_id: &str,
    verb: &str,
    subject: &str,
    outcome: Outcome,
    why: Option<String>,
) {
    let row = ActionRow::new(&format!("dispatch:{verb}"), subject, verb, outcome, why);
    crate::mandate_drive::emit_action(main_worktree, mandate_id, "session", &row);
}

// ── Store Operations (Submit / List / Show / Cancel) ────────────────────────

/// Outcome of `submit_request`: either a newly created request or an existing
/// active (`Pending`/`Running`) request with the same `dedupe_key`.
#[derive(Debug, Clone)]
pub(crate) struct SubmittedDispatch {
    pub request: DispatchRequest,
    pub result: DispatchResult,
    pub deduplicated: bool,
}

/// Validate and persist a dispatch request under `.aida/dispatch/`.
// trace:STORY-1917 | ai:agy
pub(crate) fn submit_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<SubmittedDispatch> {
    let caller = check_request_authority(main_worktree, mandate_dir, principal, verb, now)?;
    let _lock = acquire_dispatch_lock(main_worktree)?;

    let request_id = Uuid::new_v4().to_string();
    let action_res: Result<DispatchVerb> = (|| match verb {
        DispatchRequestVerb::Flow {
            pr,
            no_ship,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Flow {
                pr,
                no_ship: *no_ship,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Review { pr, vendor } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Review {
                pr,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Take {
            pr,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            let branch = validate_branch(branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate.include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            Ok(DispatchVerb::Take {
                pr,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Impl {
            name,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let name = validate_lane_name(name)?;
            let branch = validate_branch(branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate.include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            Ok(DispatchVerb::Impl {
                name,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Train { prs } => {
            let prs = validate_prs(prs)?;
            Ok(DispatchVerb::Train { prs })
        }
    })();

    let action = match action_res {
        Ok(action) => action,
        Err(err) => {
            let _ = std::fs::remove_file(brief_path(main_worktree, &request_id));
            emit_dispatch_event(
                main_worktree,
                &caller.mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(format!("{err:#}")),
            );
            return Err(err);
        }
    };

    let dedupe_key = action.dedupe_key();
    let target_key = action.target_key();

    // Check existing active requests for idempotency or target collision.
    for (existing_req, existing_res) in load_all_pairs(main_worktree)? {
        if !existing_res.status.is_active() {
            continue;
        }
        if existing_req.dedupe_key == dedupe_key {
            // Clean up the newly frozen brief if we deduplicate onto an existing request.
            let _ = std::fs::remove_file(brief_path(main_worktree, &request_id));
            return Ok(SubmittedDispatch {
                request: existing_req,
                result: existing_res,
                deduplicated: true,
            });
        }
        if existing_req.target_key == target_key {
            let _ = std::fs::remove_file(brief_path(main_worktree, &request_id));
            let msg = format!(
                "target `{target_key}` already has an active dispatch request {} ({}, status: {}); cancel it first with `aida dispatch cancel {}`",
                existing_req.id,
                existing_req.action.verb_str(),
                existing_res.status.as_str(),
                existing_req.id
            );
            emit_dispatch_event(
                main_worktree,
                &caller.mandate.id,
                action.verb_str(),
                &action.subject_label(),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    }

    let req = DispatchRequest {
        schema_version: DISPATCH_SCHEMA_VERSION,
        id: request_id.clone(),
        requested_at: now,
        principal: principal.trim().to_string(),
        requester_seat: mandate::MANDATE_SEAT.to_string(),
        requester_grant_id: caller.grant_id.clone(),
        matched_binding: caller.matched_binding,
        mandate_id: caller.mandate.id.clone(),
        dedupe_key: dedupe_key.clone(),
        target_key: target_key.clone(),
        action: action.clone(),
    };

    // Verify dual-end validator agrees with what we are about to write.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        &req,
        caller.mandate.include_supervised,
    ) {
        let _ = std::fs::remove_file(brief_path(main_worktree, &request_id));
        bail!("internal validation check failed: {err:#}");
    }

    let grant_prefix: String = caller.grant_id.chars().take(8).collect();
    let summary = if action.requires_serve() {
        format!(
            "queued {} {} (waiting for serve)",
            action.verb_str(),
            action.subject_label()
        )
    } else {
        format!("queued {} {}", action.verb_str(), action.subject_label())
    };
    let res = DispatchResult {
        id: request_id.clone(),
        verb: action.verb_str().to_string(),
        target: target_key,
        dedupe_key,
        status: DispatchStatus::Pending,
        requested_at: now,
        started_at: None,
        finished_at: None,
        requester_grant_prefix: grant_prefix,
        mandate_id: caller.mandate.id.clone(),
        worker_pid: None,
        worker_start: None,
        spawned_session: None,
        exit_code: None,
        summary: summary.clone(),
        refusal_reason: None,
        log_path: format!("{LOGS_SUBDIR}/{request_id}.log"),
        notified_at: None,
    };

    write_private_atomic(
        &request_path(main_worktree, &request_id),
        &serde_json::to_vec_pretty(&req)?,
    )?;
    write_private_atomic(
        &result_path(main_worktree, &request_id),
        &serde_json::to_vec_pretty(&res)?,
    )?;

    emit_dispatch_event(
        main_worktree,
        &caller.mandate.id,
        action.verb_str(),
        &action.subject_label(),
        Outcome::Routed,
        Some(summary),
    );

    Ok(SubmittedDispatch {
        request: req,
        result: res,
        deduplicated: false,
    })
}

/// Read one `(DispatchRequest, DispatchResult)` pair by exact ID.
// trace:STORY-1917 | ai:agy
pub(crate) fn load_pair(
    main_worktree: &Path,
    id: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }
    let req_bytes =
        read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let req: DispatchRequest = serde_json::from_slice(&req_bytes)
        .with_context(|| format!("malformed request JSON for {id}"))?;
    if req.id != id {
        bail!("request file {id}.json has mismatched id `{}`", req.id);
    }
    let res_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let res: DispatchResult = serde_json::from_slice(&res_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if res.id != id {
        bail!("result file {id}.json has mismatched id `{}`", res.id);
    }
    Ok((req, res))
}

/// Resolve an exact UUID or unique prefix to `(DispatchRequest, DispatchResult)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn resolve_pair(
    main_worktree: &Path,
    id_or_prefix: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    let needle = id_or_prefix.trim();
    if Uuid::parse_str(needle).is_ok() {
        return load_pair(main_worktree, needle);
    }
    let mut matches: Vec<(DispatchRequest, DispatchResult)> = load_all_pairs(main_worktree)?
        .into_iter()
        .filter(|(req, _)| req.id.starts_with(needle))
        .collect();
    match matches.len() {
        0 => bail!("no dispatch request matching `{needle}`"),
        1 => Ok(matches.remove(0)),
        _ => bail!("ambiguous dispatch request prefix `{needle}`"),
    }
}

/// Load all valid `(DispatchRequest, DispatchResult)` pairs sorted by
/// `requested_at` ascending (then `id`).
// trace:STORY-1917 | ai:agy
pub(crate) fn load_all_pairs(
    main_worktree: &Path,
) -> Result<Vec<(DispatchRequest, DispatchResult)>> {
    let req_dir = requests_dir(main_worktree);
    let entries = match std::fs::read_dir(&req_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", req_dir.display())),
    };
    let mut pairs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "warning: ignoring unreadable entry in {}: {err}",
                    req_dir.display()
                );
                continue;
            }
        };
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        if ext != "json" || stem.starts_with('.') || Uuid::parse_str(stem).is_err() {
            continue;
        }
        match load_pair(main_worktree, stem) {
            Ok(pair) => pairs.push(pair),
            Err(err) => {
                eprintln!("warning: ignoring malformed dispatch record {stem}: {err:#}");
            }
        }
    }
    pairs.sort_by(|(a, _), (b, _)| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
    Ok(pairs)
}

/// Cancel a `Pending` dispatch request.
// trace:STORY-1917 | ai:agy
pub(crate) fn cancel_request(
    main_worktree: &Path,
    id_or_prefix: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let (req, mut res) = resolve_pair(main_worktree, id_or_prefix)?;
    if res.status != DispatchStatus::Pending {
        bail!(
            "cannot cancel dispatch request {}: status is `{}` (only `pending` requests can be cancelled)",
            req.id,
            res.status.as_str()
        );
    }
    res.status = DispatchStatus::Cancelled;
    res.finished_at = Some(now);
    res.summary = format!(
        "cancelled {} {}",
        req.action.verb_str(),
        req.action.subject_label()
    );
    write_private_atomic(
        &result_path(main_worktree, &req.id),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id,
        req.action.verb_str(),
        &req.action.subject_label(),
        Outcome::Skipped,
        Some("cancelled".to_string()),
    );
    Ok(res)
}

/// Human-facing status label for `aida dispatch list`: per ADVISOR SIGNOFF,
/// `Pending` requests for `review`, `flow`, and `train` display as
/// `"pending (waiting for serve)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn display_status(req: &DispatchRequest, res: &DispatchResult) -> String {
    if res.status == DispatchStatus::Pending && req.action.requires_serve() {
        "pending (waiting for serve)".to_string()
    } else {
        res.status.as_str().to_string()
    }
}

pub(crate) fn format_list_text(pairs: &[(DispatchRequest, DispatchResult)]) -> String {
    if pairs.is_empty() {
        return "No dispatch requests.\n".to_string();
    }
    let mut out = String::new();
    for (req, res) in pairs {
        let short_id: String = req.id.chars().take(8).collect();
        let status = display_status(req, res);
        let ts = req.requested_at.format("%Y-%m-%d %H:%M:%S");
        out.push_str(&format!(
            "{short_id}  {:<28}  {:<6}  {:<18}  {}  {}\n",
            status,
            req.action.verb_str(),
            req.target_key,
            ts,
            res.summary
        ));
    }
    out
}

pub(crate) fn list_json(pairs: &[(DispatchRequest, DispatchResult)]) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(req, res)| {
            serde_json::json!({
                "id": req.id,
                "verb": req.action.verb_str(),
                "target": req.target_key,
                "dedupe_key": req.dedupe_key,
                "status": res.status.as_str(),
                "display_status": display_status(req, res),
                "waiting_for_serve": res.status == DispatchStatus::Pending && req.action.requires_serve(),
                "requested_at": req.requested_at.to_rfc3339(),
                "started_at": res.started_at.map(|t| t.to_rfc3339()),
                "finished_at": res.finished_at.map(|t| t.to_rfc3339()),
                "mandate_id": req.mandate_id,
                "requester_grant_prefix": res.requester_grant_prefix,
                "summary": res.summary,
                "refusal_reason": res.refusal_reason,
                "request": req,
                "result": res,
            })
        })
        .collect();
    serde_json::Value::Array(rows)
}

// ── CLI Entrypoint ──────────────────────────────────────────────────────────

pub(crate) fn handle_dispatch(action: &DispatchAction) -> Result<()> {
    let main_worktree = crate::find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .context("cannot locate main worktree root")?;
    let mandate_dir = mandate::mandate_dir()?;
    let now = Utc::now();

    match action {
        DispatchAction::Request { verb, json } => {
            let principal = crate::current_user_id(None);
            let submitted = submit_request(&main_worktree, &mandate_dir, &principal, verb, now)?;
            if *json {
                let payload = serde_json::json!({
                    "deduplicated": submitted.deduplicated,
                    "waiting_for_serve": submitted.result.status == DispatchStatus::Pending
                        && submitted.request.action.requires_serve(),
                    "display_status": display_status(&submitted.request, &submitted.result),
                    "request": submitted.request,
                    "result": submitted.result,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            if submitted.deduplicated {
                println!(
                    "already active: {} ({})",
                    submitted.request.id,
                    display_status(&submitted.request, &submitted.result)
                );
            } else {
                println!(
                    "queued: {} ({} {} — {})",
                    submitted.request.id,
                    submitted.request.action.verb_str(),
                    submitted.request.action.subject_label(),
                    display_status(&submitted.request, &submitted.result)
                );
            }
            Ok(())
        }
        DispatchAction::List { status, json } => {
            let mut pairs = load_all_pairs(&main_worktree)?;
            if let Some(filter) = status {
                pairs.retain(|(_, res)| res.status.matches_filter(*filter));
            }
            if *json {
                println!("{}", serde_json::to_string_pretty(&list_json(&pairs))?);
            } else {
                print!("{}", format_list_text(&pairs));
            }
            Ok(())
        }
        DispatchAction::Show { id, json } => {
            let (req, res) = resolve_pair(&main_worktree, id)?;
            if *json {
                let payload = serde_json::json!({
                    "display_status": display_status(&req, &res),
                    "waiting_for_serve": res.status == DispatchStatus::Pending && req.action.requires_serve(),
                    "request": req,
                    "result": res,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("id:             {}", req.id);
                println!("verb:           {}", req.action.verb_str());
                println!("target:         {}", req.target_key);
                println!("dedupe_key:     {}", req.dedupe_key);
                println!("status:         {}", display_status(&req, &res));
                println!("requested_at:   {}", req.requested_at.to_rfc3339());
                if let Some(started) = res.started_at {
                    println!("started_at:     {}", started.to_rfc3339());
                }
                if let Some(finished) = res.finished_at {
                    println!("finished_at:    {}", finished.to_rfc3339());
                }
                println!("mandate_id:     {}", req.mandate_id);
                println!("grant_prefix:   {}", res.requester_grant_prefix);
                println!("summary:        {}", res.summary);
                if let Some(reason) = &res.refusal_reason {
                    println!("refusal_reason: {reason}");
                }
                println!("log_path:       {}", res.log_path);
            }
            Ok(())
        }
        DispatchAction::Cancel { id } => {
            let res = cancel_request(&main_worktree, id, now)?;
            println!("cancelled: {} ({})", res.id, res.summary);
            Ok(())
        }
        DispatchAction::Tick => {
            let summary = run_dispatch_tick(&main_worktree, &mandate_dir, now)?;
            if summary.spawned > 0 || summary.reaped > 0 {
                println!(
                    "dispatch tick: spawned={}, reaped={}, waiting_for_serve={}",
                    summary.spawned, summary.reaped, summary.waiting_for_serve
                );
            }
            Ok(())
        }
        DispatchAction::Serve { once, poll_ms } => {
            run_dispatch_serve(&main_worktree, &mandate_dir, *once, *poll_ms)
        }
        DispatchAction::Run { id, via_serve } => {
            let res = execute_one_request(&main_worktree, &mandate_dir, id, *via_serve, now)?;
            println!("{}: {} ({})", res.status.as_str(), res.id, res.summary);
            Ok(())
        }
    }
}

// ── Slice 2: Executor Core, Serve, and Review / Take / Impl Runners ─────────

pub(crate) const MAX_CONCURRENT_REVIEWS: usize = 2;
pub(crate) const MAX_CONCURRENT_FLOWS: usize = 4;
pub(crate) const MAX_CONCURRENT_TRAINS: usize = 1;
pub(crate) const MAX_DISPATCH_LOG_BYTES: usize = 1024 * 1024;
pub(crate) const STALE_GIT_CONFIG_LOCK_AGE_SECS: u64 = 120;

/// Summary of one `dispatch tick` or `dispatch serve` claim/reap pass.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DispatchPassSummary {
    pub reaped: usize,
    pub spawned: usize,
    pub claimed_ids: Vec<String>,
    pub waiting_for_serve: usize,
}

/// Safe stale `.git/config.lock` cleanup: unlinks `<main_worktree>/.git/config.lock`
/// ONLY when `symlink_metadata` confirms a regular file (never a symlink),
/// `len() == 0`, mode `0o444`, and `mtime >= 120s` in the past relative to `now`.
/// Returns `Ok(true)` when a stale lock was removed, `Ok(false)` otherwise.
// trace:STORY-1917 | ai:agy
pub(crate) fn clean_stale_git_config_lock(
    main_worktree: &Path,
    now: std::time::SystemTime,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let lock_path = main_worktree.join(".git").join("config.lock");
    let meta = match std::fs::symlink_metadata(&lock_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("cannot stat {}", lock_path.display())),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Ok(false);
    }
    if meta.len() != 0 {
        return Ok(false);
    }
    if (meta.mode() & 0o777) != 0o444 {
        return Ok(false);
    }
    let Ok(mtime) = meta.modified() else {
        return Ok(false);
    };
    let Ok(age) = now.duration_since(mtime) else {
        return Ok(false);
    };
    if age < std::time::Duration::from_secs(STALE_GIT_CONFIG_LOCK_AGE_SECS) {
        return Ok(false);
    }
    match std::fs::remove_file(&lock_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot unlink {}", lock_path.display())),
    }
}

/// Send one best-effort mailbox notification to `advisor` when a request
/// reaches a terminal state (`Done`, `Refused`, or `Failed`), and return the
/// notification timestamp if delivery succeeded.
// trace:STORY-1917 | ai:agy
pub(crate) fn notify_advisor_outcome(
    main_worktree: &Path,
    req: &DispatchRequest,
    res: &DispatchResult,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let subject = format!(
        "dispatch {} {}: {} ({})",
        req.action.verb_str(),
        req.action.subject_label(),
        res.status.as_str(),
        &req.id[..8.min(req.id.len())]
    );
    let body = match &res.refusal_reason {
        Some(reason) => format!(
            "Dispatch request {} ({} {}) {}: {}\nreason: {}\nlog: {}",
            req.id,
            req.action.verb_str(),
            req.action.subject_label(),
            res.status.as_str(),
            res.summary,
            reason,
            res.log_path
        ),
        None => format!(
            "Dispatch request {} ({} {}) {}: {}\nlog: {}",
            req.id,
            req.action.verb_str(),
            req.action.subject_label(),
            res.status.as_str(),
            res.summary,
            res.log_path
        ),
    };
    let msg_id = Uuid::new_v4().to_string();
    let msg = Message {
        id: msg_id.clone(),
        thread_id: msg_id,
        from: "dispatch".to_string(),
        to: Recipient::Agent("advisor".to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body,
        subject: Some(subject),
        urgent: matches!(res.status, DispatchStatus::Refused | DispatchStatus::Failed),
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: Some("dispatch".to_string()),
        relayed_from: None,
    };
    if crate::mailbox_store::write_message(main_worktree, &msg).is_ok() {
        crate::events::emit(
            main_worktree,
            &crate::events::Event::new(
                None,
                "",
                crate::events::EventKind::MailReceived {
                    to: "advisor".to_string(),
                },
            ),
        );
        Some(now)
    } else {
        None
    }
}

/// Append bounded log text to `.aida/dispatch/logs/<id>.log` (mode 0600, capped
/// at [`MAX_DISPATCH_LOG_BYTES`]).
// trace:STORY-1917 | ai:agy
pub(crate) fn append_dispatch_log(main_worktree: &Path, id: &str, text: &str) -> Result<()> {
    let dir = logs_dir(main_worktree);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.log"));
    let mut existing = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    existing.extend_from_slice(text.as_bytes());
    if existing.len() > MAX_DISPATCH_LOG_BYTES {
        let drop_len = existing.len() - MAX_DISPATCH_LOG_BYTES;
        existing.drain(..drop_len);
    }
    write_private_atomic(&path, &existing)
}

/// Verify that `aida dispatch serve` is running with a valid, bound `advisor`
/// seat grant (`GrantCheck::Valid` with `seat == "advisor"`).
///
/// Per ADVISOR SIGNOFF: `serve` never self-binds or mutates
/// `grant.binding`; it inherits the advisor terminal's `proc:`/`sid:`/`key:`
/// binding the ordinary way. If the grant is absent, rejected, expired, or
/// revoked, `serve` refuses to start (or stops mid-run and leaves unclaimed
/// requests `Pending`).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_serve_grant(
    main_worktree: &Path,
    now: DateTime<Utc>,
) -> Result<seat_authority::SeatGrant> {
    let grant = match seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(g) => g,
        GrantCheck::Absent => {
            bail!(
                "`aida dispatch serve` requires a bound `advisor` seat grant (no session grant is set); enter the role at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` before starting `aida dispatch serve`"
            )
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let seat_str = seat.as_deref().unwrap_or("unknown");
            bail!(
                "`aida dispatch serve` advisor seat grant {id} ({seat_str}) is not valid: {reason}; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`"
            )
        }
    };
    if crate::canonical_role_name(&grant.seat) != mandate::MANDATE_SEAT {
        bail!(
            "`aida dispatch serve` requires the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id,
            grant.seat
        );
    }
    let presenter = seat_authority::presenter_tokens_for(&grant);
    if seat_authority::matched_binding(&grant, &presenter).is_empty() {
        bail!(
            "`aida dispatch serve` grant {} has no presenter binding matching this process; run `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in this terminal first",
            grant.id
        );
    }
    Ok(grant)
}

/// Reap any `Running` request whose recorded `(worker_pid, worker_start)` is
/// provably dead without having recorded completion. Caller must hold
/// `acquire_dispatch_lock`.
// trace:STORY-1917 | ai:agy
fn reap_dead_workers_locked(main_worktree: &Path, now: DateTime<Utc>) -> Result<usize> {
    let mut reaped = 0usize;
    for (req, mut res) in load_all_pairs(main_worktree)? {
        if res.status != DispatchStatus::Running {
            continue;
        }
        let alive = match res.worker_pid {
            Some(pid) => {
                crate::process_probe::process_identity_is_alive(pid, res.worker_start.as_deref())
            }
            None => false,
        };
        if alive {
            continue;
        }
        let reason = format!(
            "worker (pid {:?}) exited unexpectedly without recording completion",
            res.worker_pid
        );
        res.status = DispatchStatus::Failed;
        res.finished_at = Some(now);
        res.summary = format!(
            "failed {} {}: {reason}",
            req.action.verb_str(),
            req.action.subject_label()
        );
        res.refusal_reason = Some(reason.clone());
        res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
        write_private_atomic(
            &result_path(main_worktree, &req.id),
            &serde_json::to_vec_pretty(&res)?,
        )?;
        emit_dispatch_event(
            main_worktree,
            &req.mandate_id,
            req.action.verb_str(),
            &req.action.subject_label(),
            Outcome::Skipped,
            Some(reason),
        );
        reaped += 1;
    }
    Ok(reaped)
}

/// Spawn a detached `aida dispatch run <id> [--via-serve]` worker process,
/// redirecting stdout/stderr to `.aida/dispatch/logs/<id>.log` (mode 0600).
// trace:STORY-1917 | ai:agy
fn spawn_detached_worker(
    main_worktree: &Path,
    id: &str,
    via_serve: bool,
) -> Result<(u32, Option<String>)> {
    let aida_bin = resolve_aida_bin()?;
    let log_dir = logs_dir(main_worktree);
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{id}.log"));
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let log_out = opts.open(&log_path)?;
    let log_err = log_out.try_clone()?;

    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree).args(["dispatch", "run", id]);
    if via_serve {
        cmd.arg("--via-serve");
    } else {
        // Cron/schedule tick workers never inherit ambient session grants/roles.
        for key in crate::mandate_drive::SESSION_ENV {
            cmd.env_remove(key);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_out))
        .stderr(std::process::Stdio::from(log_err));

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `aida dispatch run {id}`"))?;
    let pid = child.id();
    let start = crate::process_probe::process_start_identity(pid);
    Ok((pid, start))
}

/// Claim eligible `Pending` requests under `tick.lock` (moving `Pending ->
/// Running` and enforcing concurrency caps: 2 reviews, 4 flows, 1 train), and
/// either spawn detached `aida dispatch run <id>` workers (`spawn_detached ==
/// true`) or return their IDs for in-process execution (used by deterministic
/// unit tests).
// trace:STORY-1917 | ai:agy
pub(crate) fn claim_and_dispatch_pass(
    main_worktree: &Path,
    via_serve: bool,
    spawn_detached: bool,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let reaped = reap_dead_workers_locked(main_worktree, now)?;
    let pairs = load_all_pairs(main_worktree)?;

    let mut running_reviews = 0usize;
    let mut running_flows = 0usize;
    let mut running_trains = 0usize;
    for (req, res) in &pairs {
        if res.status == DispatchStatus::Running {
            match req.action {
                DispatchVerb::Review { .. } => running_reviews += 1,
                DispatchVerb::Flow { .. } => running_flows += 1,
                DispatchVerb::Train { .. } => running_trains += 1,
                DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
            }
        }
    }

    let mut summary = DispatchPassSummary {
        reaped,
        ..Default::default()
    };

    for (req, mut res) in pairs {
        if res.status != DispatchStatus::Pending {
            continue;
        }
        if req.action.requires_serve() && !via_serve {
            summary.waiting_for_serve += 1;
            continue;
        }
        // In Slice 2, `flow` and `train` runners land in Slice 3; under `serve`
        // we only claim verbs whose runner is supported or let `execute_one_request`
        // handle them. Check concurrency caps first:
        match req.action {
            DispatchVerb::Review { .. } => {
                if running_reviews >= MAX_CONCURRENT_REVIEWS {
                    continue;
                }
                running_reviews += 1;
            }
            DispatchVerb::Flow { .. } => {
                if running_flows >= MAX_CONCURRENT_FLOWS {
                    continue;
                }
                running_flows += 1;
            }
            DispatchVerb::Train { .. } => {
                if running_trains >= MAX_CONCURRENT_TRAINS {
                    continue;
                }
                running_trains += 1;
            }
            DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
        }

        res.status = DispatchStatus::Running;
        res.started_at = Some(now);
        if spawn_detached {
            match spawn_detached_worker(main_worktree, &req.id, via_serve) {
                Ok((pid, start)) => {
                    res.worker_pid = Some(pid);
                    res.worker_start = start;
                    res.summary = format!(
                        "running {} {} (pid {pid})",
                        req.action.verb_str(),
                        req.action.subject_label()
                    );
                }
                Err(err) => {
                    let reason = format!("failed to spawn dispatch worker: {err:#}");
                    res.status = DispatchStatus::Failed;
                    res.finished_at = Some(now);
                    res.summary = reason.clone();
                    res.refusal_reason = Some(reason);
                    res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
                    write_private_atomic(
                        &result_path(main_worktree, &req.id),
                        &serde_json::to_vec_pretty(&res)?,
                    )?;
                    continue;
                }
            }
        } else {
            let pid = std::process::id();
            res.worker_pid = Some(pid);
            res.worker_start = crate::process_probe::process_start_identity(pid);
            res.summary = format!(
                "running {} {}",
                req.action.verb_str(),
                req.action.subject_label()
            );
        }

        write_private_atomic(
            &result_path(main_worktree, &req.id),
            &serde_json::to_vec_pretty(&res)?,
        )?;
        summary.spawned += 1;
        summary.claimed_ids.push(req.id);
    }

    Ok(summary)
}

/// Run one substrate `aida dispatch tick` pass: executes only `take` and `impl`
/// requests (leaves `review`, `flow`, and `train` `Pending` as `waiting for serve`).
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick(
    main_worktree: &Path,
    _mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    claim_and_dispatch_pass(main_worktree, false, true, now)
}

/// Synchronous test/in-process tick helper: claims eligible `take`/`impl`
/// requests (`via_serve = false`) and runs them in-process, leaving `review`,
/// `flow`, and `train` `Pending`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let summary = claim_and_dispatch_pass(main_worktree, false, false, now)?;
    for id in &summary.claimed_ids {
        let _ = execute_one_request(main_worktree, mandate_dir, id, false, now);
    }
    Ok(summary)
}

/// Synchronous `serve --once` helper: verifies the caller's bound `advisor`
/// grant first (refusing to start without one), then claims and executes
/// eligible requests.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve_once_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _grant = check_serve_grant(main_worktree, now)?;
    let summary = claim_and_dispatch_pass(main_worktree, true, false, now)?;
    for id in &summary.claimed_ids {
        // Re-check the advisor grant before each request so mid-pass expiry
        // stops immediately and reverts unclaimed requests back to Pending.
        if check_serve_grant(main_worktree, Utc::now()).is_err() {
            if let Ok((_req, mut res)) = load_pair(main_worktree, id) {
                if res.status == DispatchStatus::Running {
                    res.status = DispatchStatus::Pending;
                    res.started_at = None;
                    res.worker_pid = None;
                    res.worker_start = None;
                    let _ = write_private_atomic(
                        &result_path(main_worktree, id),
                        &serde_json::to_vec_pretty(&res)?,
                    );
                }
            }
            continue;
        }
        let _ = execute_one_request(main_worktree, mandate_dir, id, true, now);
    }
    Ok(summary)
}

/// Long-running `aida dispatch serve` loop started from a human advisor
/// terminal after `eval "$(aida role enter advisor ...)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve(
    main_worktree: &Path,
    _mandate_dir: &Path,
    once: bool,
    poll_ms: u64,
) -> Result<()> {
    let now = Utc::now();
    let grant = check_serve_grant(main_worktree, now)?;
    println!(
        "dispatch serve: bound advisor grant {} active until {}",
        &grant.id[..8.min(grant.id.len())],
        grant.expires_at.to_rfc3339()
    );
    loop {
        let tick_now = Utc::now();
        if let Err(err) = check_serve_grant(main_worktree, tick_now) {
            eprintln!("dispatch serve stopping (pending requests left pending): {err:#}");
            return Ok(());
        }
        let summary = claim_and_dispatch_pass(main_worktree, true, true, tick_now)?;
        if summary.spawned > 0 || summary.reaped > 0 {
            println!(
                "dispatch serve: spawned={}, reaped={}",
                summary.spawned, summary.reaped
            );
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(poll_ms.clamp(100, 60_000)));
    }
}

/// Execute a single dispatch request (`aida dispatch run <id> [--via-serve]`).
///
/// Enforces:
/// 1. Dual-end re-validation of `requests/<id>.json` and any frozen brief file
///    (`O_NOFOLLOW | O_NONBLOCK`, mode 0600, current uid, SHA-256 match).
/// 2. Hard Boundary B rule (ADVISOR SIGNOFF): if `req.action.requires_serve()`
///    (`review`, `flow`, `train`) and `!via_serve`, refuses immediately without
///    touching any grant. When `via_serve` is true, verifies `check_serve_grant`
///    (inherited binding; never self-binds the grant).
/// 3. Mandate re-check (`mandate::recheck`), `HumanGates` (`GateOp::Dispatch` /
///    `GateOp::Ship`), and requester grant re-check on disk
///    (`seat_authority::grant_authorized_at`).
// trace:STORY-1917 | ai:agy
pub(crate) fn execute_one_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    id: &str,
    via_serve: bool,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));

    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }

    // Load the result record first so we can record a refusal even if the
    // request JSON on disk was tampered with after submission.
    let res_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let mut res: DispatchResult = serde_json::from_slice(&res_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if res.id != id {
        bail!("result file {id}.json has mismatched id `{}`", res.id);
    }
    if matches!(
        res.status,
        DispatchStatus::Done
            | DispatchStatus::Refused
            | DispatchStatus::Failed
            | DispatchStatus::Cancelled
    ) {
        return Ok(res);
    }

    let req: DispatchRequest =
        match read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)
            .and_then(|bytes| {
                serde_json::from_slice::<DispatchRequest>(&bytes)
                    .with_context(|| format!("malformed request JSON for {id}"))
            }) {
            Ok(req) if req.id == id => req,
            Ok(req) => {
                let reason = format!(
                    "tampered request JSON for {id}: inner id `{}` does not match `{id}`",
                    req.id
                );
                return record_tampered_refusal(main_worktree, id, &mut res, &reason, now);
            }
            Err(err) => {
                let reason = format!("request validation failed on disk: {err:#}");
                return record_tampered_refusal(main_worktree, id, &mut res, &reason, now);
            }
        };

    // Ensure status is Running while we execute.
    if res.status == DispatchStatus::Pending {
        res.status = DispatchStatus::Running;
        res.started_at = Some(now);
        let pid = std::process::id();
        res.worker_pid = Some(pid);
        res.worker_start = crate::process_probe::process_start_identity(pid);
        write_private_atomic(
            &result_path(main_worktree, id),
            &serde_json::to_vec_pretty(&res)?,
        )?;
    }

    // 1. Boundary B check: `review`, `flow`, and `train` may ONLY execute under
    // `aida dispatch serve` (never under `dispatch tick`).
    if req.action.requires_serve() && !via_serve {
        let reason = format!(
            "verb `{}` requires an advisor seat grant and can only run under `aida dispatch serve` (refused by `dispatch tick`)",
            req.action.verb_str()
        );
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    if via_serve {
        if let Err(err) = check_serve_grant(main_worktree, now) {
            let reason = format!("serve advisor grant check failed at execution time: {err:#}");
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 2. Re-validate the mandate on disk (`mandate::recheck` + active mandate scopes).
    let (valid_mandates, _) = mandate::load_valid(mandate_dir, &req.principal, now);
    let active_mandate = match mandate::state_at(&valid_mandates, now) {
        MandateState::Active(m) => m,
        MandateState::None => {
            let reason = "no active mandate at execution time".to_string();
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
        MandateState::Expired(m) => {
            let reason = format!(
                "mandate {} expired at {} before execution",
                m.id,
                m.expires_at.to_rfc3339()
            );
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
        MandateState::Stopped(m) => {
            let reason = format!("mandate {} was stopped before execution", m.id);
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    };

    if !active_mandate.covers(Scope::Dispatch) {
        let reason = format!(
            "active mandate {} no longer covers `dispatch`",
            active_mandate.id
        );
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.ships_pr() && !active_mandate.covers(Scope::ShipHeld) {
        let reason = format!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            active_mandate.id,
            req.action.verb_str()
        );
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    // 3. Re-validate HumanGates at execution time.
    let gates = crate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let reason = format!("dispatch is gated by human gates at execution time ({why})");
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Gated,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.ships_pr() {
        if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let reason = format!("shipping is gated by human gates at execution time ({why})");
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Gated,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 4. Re-validate the requester's advisor grant on disk (`grant_authorized_at`),
    //    and treat any `revoked_at` stamp as immediate revocation regardless of
    //    clock skew.
    let grant_unrevoked = seat_authority::read_grant(&req.requester_grant_id)
        .map(|g| g.revoked_at.is_none())
        .unwrap_or(false);
    if !grant_unrevoked
        || !seat_authority::grant_authorized_at(
            main_worktree,
            &req.requester_grant_id,
            mandate::MANDATE_SEAT,
            now,
            &req.matched_binding,
        )
    {
        let reason = format!(
            "requester advisor grant {} is no longer valid (expired, revoked, or binding mismatch)",
            req.requester_grant_id
        );
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.delegates_reviewer() {
        let delegable_ok = seat_authority::read_grant(&req.requester_grant_id)
            .map(|g| {
                g.delegable_seats
                    .iter()
                    .any(|s| crate::canonical_role_name(s) == "reviewer")
            })
            .unwrap_or(false);
        if !delegable_ok {
            let reason = format!(
                "requester advisor grant {} does not delegate `reviewer`",
                req.requester_grant_id
            );
            return finalize_request(
                main_worktree,
                &req,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 5. Dual-end validation of request fields + frozen brief SHA-256 + spec store checks.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        &req,
        active_mandate.include_supervised,
    ) {
        let reason = format!("execution-time validation refused request: {err:#}");
        return finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    // 6. Execute the verb's runner.
    let runner_outcome = match &req.action {
        DispatchVerb::Review { pr, vendor } => run_review_verb(main_worktree, &req, *pr, *vendor),
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_take_verb(
            main_worktree,
            &req,
            *pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_impl_verb(
            main_worktree,
            &req,
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Flow { .. } | DispatchVerb::Train { .. } => RunnerOutcome::Refused {
            reason: format!(
                "verb `{}` runner is not enabled in this build (scheduled for Slice 3)",
                req.action.verb_str()
            ),
        },
    };

    match runner_outcome {
        RunnerOutcome::Done {
            summary,
            spawned_session,
            exit_code,
        } => finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Done,
            Outcome::Done,
            spawned_session,
            exit_code,
            summary,
            None,
            now,
        ),
        RunnerOutcome::Refused { reason } => finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        ),
        RunnerOutcome::Failed { summary, exit_code } => finalize_request(
            main_worktree,
            &req,
            res,
            DispatchStatus::Failed,
            Outcome::Skipped,
            None,
            exit_code,
            summary.clone(),
            Some(summary),
            now,
        ),
    }
}

#[derive(Debug)]
enum RunnerOutcome {
    Done {
        summary: String,
        spawned_session: Option<String>,
        exit_code: Option<i32>,
    },
    Refused {
        reason: String,
    },
    Failed {
        summary: String,
        exit_code: Option<i32>,
    },
}

fn record_tampered_refusal(
    main_worktree: &Path,
    id: &str,
    res: &mut DispatchResult,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _ = append_dispatch_log(main_worktree, id, &format!("refused: {reason}\n"));
    res.status = DispatchStatus::Refused;
    res.finished_at = Some(now);
    res.summary = reason.to_string();
    res.refusal_reason = Some(reason.to_string());
    emit_dispatch_event(
        main_worktree,
        &res.mandate_id,
        &res.verb,
        &res.target,
        Outcome::Refused,
        Some(reason.to_string()),
    );
    write_private_atomic(
        &result_path(main_worktree, id),
        &serde_json::to_vec_pretty(res)?,
    )?;
    Ok(res.clone())
}

#[allow(clippy::too_many_arguments)]
fn finalize_request(
    main_worktree: &Path,
    req: &DispatchRequest,
    mut res: DispatchResult,
    status: DispatchStatus,
    event_outcome: Outcome,
    spawned_session: Option<String>,
    exit_code: Option<i32>,
    summary: String,
    refusal_reason: Option<String>,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _ = append_dispatch_log(
        main_worktree,
        &req.id,
        &format!("{}: {}\n", status.as_str(), summary),
    );
    res.status = status;
    res.finished_at = Some(now);
    res.spawned_session = spawned_session;
    res.exit_code = exit_code;
    res.summary = summary.clone();
    res.refusal_reason = refusal_reason.clone();
    res.notified_at = notify_advisor_outcome(main_worktree, req, &res, now);
    write_private_atomic(
        &result_path(main_worktree, &req.id),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id,
        req.action.verb_str(),
        &req.action.subject_label(),
        event_outcome,
        refusal_reason.or(Some(summary)),
    );
    Ok(res)
}

/// Resolve the sibling `<project>-worktrees` root directory next to `main_worktree`.
// trace:STORY-1917 | ai:agy
pub(crate) fn worktrees_root(main_worktree: &Path) -> PathBuf {
    let parent = main_worktree.parent().unwrap_or(main_worktree);
    let repo_name = main_worktree
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aida");
    parent.join(format!("{repo_name}-worktrees"))
}

/// Check if any live session lease in `.aida/sessions/` owns `worktree` or
/// `branch` (or `scope`). Returns a descriptive refusal reason if occupied by a
/// live process.
// trace:STORY-1917 | ai:agy
pub(crate) fn check_live_worktree_or_branch_occupant(
    main_worktree: &Path,
    worktree: &Path,
    branch: Option<&str>,
    scope: Option<&str>,
) -> Option<String> {
    let live_sessions = crate::process_probe::probe_live_claude_sessions();
    let now = Utc::now();
    let target_canon = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());

    for lease in crate::leases::list_leases(main_worktree) {
        let state = crate::leases::lease_state_for(&lease, &live_sessions, now);
        let is_live = matches!(state, crate::LeaseState::Live)
            || crate::agent_registry::live_agents_covering_cwd(main_worktree, &lease.worktree_path)
                > 0;
        if !is_live {
            continue;
        }
        let lease_canon = lease
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| lease.worktree_path.clone());
        if !lease.worktree_path.as_os_str().is_empty() && lease_canon == target_canon {
            return Some(format!(
                "worktree {} is currently occupied by live session lease {} (scope `{}`, role `{}`)",
                worktree.display(),
                lease.id,
                lease.scope,
                lease.role.as_deref().unwrap_or("unspecified")
            ));
        }
        if let Some(b) = branch {
            if !lease.branch.is_empty() && lease.branch == b {
                return Some(format!(
                    "branch `{b}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
        if let Some(s) = scope {
            if lease.scope.eq_ignore_ascii_case(s) {
                return Some(format!(
                    "scope `{s}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
    }
    None
}

/// Prepare a dead reviewer worktree (`aida-pr-<pr>`) before launching a fresh
/// review:
/// 1. Refuses if a live reviewer lease currently occupies `rev_wt` or `PR-<pr>`.
/// 2. Copies any `<rev_wt>/.aida/merge-holds/PR-<pr>` marker into
///    `<main_worktree>/.aida/merge-holds/PR-<pr>` (no-clobber) so removing the
///    dead reviewer worktree never loses a merge hold.
/// 3. Removes `rev_wt` via `git worktree remove --force`.
// trace:STORY-1917 | ai:agy
pub(crate) fn cleanup_dead_reviewer_worktree(main_worktree: &Path, pr: u64) -> Result<()> {
    let pr_scope = format!("PR-{pr}");
    let candidates = [
        worktrees_root(main_worktree).join(format!("aida-pr-{pr}")),
        main_worktree
            .parent()
            .unwrap_or(main_worktree)
            .join(format!("aida-pr-{pr}")),
    ];
    for rev_wt in candidates {
        if let Some(busy) =
            check_live_worktree_or_branch_occupant(main_worktree, &rev_wt, None, Some(&pr_scope))
        {
            bail!("refusing to clean up reviewer worktree: {busy}");
        }
        if !rev_wt.exists() {
            continue;
        }
        // Preserve any merge-hold marker recorded inside the reviewer worktree
        // (no-clobber copy into `<main_worktree>/.aida/merge-holds/PR-<pr>`).
        let wt_hold = rev_wt.join(".aida").join("merge-holds").join(&pr_scope);
        let main_hold_dir = main_worktree.join(".aida").join("merge-holds");
        let main_hold = main_hold_dir.join(&pr_scope);
        if wt_hold.is_file() && !main_hold.exists() {
            std::fs::create_dir_all(&main_hold_dir)?;
            let _ = std::fs::copy(&wt_hold, &main_hold);
        }
        let out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "remove", "--force"])
            .arg(&rev_wt)
            .output()
            .with_context(|| {
                format!("failed to run git worktree remove on {}", rev_wt.display())
            })?;
        if !out.status.success() && rev_wt.exists() {
            // If git did not know it as a linked worktree, leave any non-empty
            // untracked directory alone only if git worktree prune doesn't clear it.
            let _ = aida_core::git_ops::host_git_command(main_worktree)
                .args(["worktree", "prune"])
                .output();
        }
    }
    // Best-effort cleanup of a leftover `pr-<pr>` branch after removing any dead
    // reviewer worktree, matching `review-one.sh`.
    let _ = aida_core::git_ops::host_git_command(main_worktree)
        .args(["branch", "-D", &format!("pr-{pr}")])
        .output();
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_AIDA_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_AGENT_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn resolve_aida_bin() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(p) = TEST_AIDA_BIN.with(|c| c.borrow().clone()) {
        return Ok(p);
    }
    crate::aida_bin::process()
        .map(|r| r.path)
        .or_else(|_| Ok(std::env::current_exe()?))
}

fn resolve_vendor_bin(vendor: DispatchVendor) -> PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_AGENT_BIN.with(|c| c.borrow().clone()) {
        return p;
    }
    let name = match vendor {
        DispatchVendor::Claude => "claude",
        DispatchVendor::Codex => "codex",
        DispatchVendor::Agy => "agy",
    };
    crate::find_executable_on_path(name).unwrap_or_else(|| PathBuf::from(name))
}

/// Run the `review` verb (`aida queue work PR-<pr> --role reviewer --no-human [--vendor <v>]`).
/// Runs only under `aida dispatch serve` so it inherits the human advisor's
/// bound seat grant directly.
// trace:STORY-1917 | ai:agy
fn run_review_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    pr: u64,
    vendor: Option<DispatchVendor>,
) -> RunnerOutcome {
    if let Err(err) = cleanup_dead_reviewer_worktree(main_worktree, pr) {
        return RunnerOutcome::Refused {
            reason: format!("{err:#}"),
        };
    }

    let aida_bin = match resolve_aida_bin() {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Failed {
                summary: format!("cannot locate aida binary: {err:#}"),
                exit_code: None,
            }
        }
    };

    let pr_arg = format!("PR-{pr}");
    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree).args([
        "queue",
        "work",
        &pr_arg,
        "--role",
        "reviewer",
        "--no-human",
    ]);
    if let Some(v) = vendor {
        cmd.args(["--vendor", v.as_str()]);
    }
    cmd.env_remove("AIDA_SESSION_PURPOSE");

    match cmd.output() {
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let _ = append_dispatch_log(main_worktree, &req.id, &combined);
            let code = out.status.code();
            if out.status.success() {
                RunnerOutcome::Done {
                    summary: format!("completed review of PR #{pr}"),
                    spawned_session: None,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!("review of PR #{pr} exited with {:?}", code),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn `aida queue work PR-{pr}`: {err:#}"),
            exit_code: None,
        },
    }
}

/// If `refs/heads/<branch>` is already checked out in a linked worktree, return
/// that worktree's path so `take` can reuse it rather than failing `git worktree add`.
// trace:STORY-1917 | ai:agy
fn find_worktree_for_branch(main_worktree: &Path, branch: &str) -> Option<PathBuf> {
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let want_branch = format!("branch refs/heads/{branch}");
    let main_canon = main_worktree
        .canonicalize()
        .unwrap_or_else(|_| main_worktree.to_path_buf());
    let mut current_wt: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            current_wt = Some(PathBuf::from(rest.trim()));
        } else if line.trim() == want_branch {
            if let Some(wt) = current_wt.take() {
                let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.clone());
                if wt_canon != main_canon {
                    return Some(wt);
                }
            }
        } else if line.trim().is_empty() {
            current_wt = None;
        }
    }
    None
}

fn git_ref_exists(repo: &Path, git_ref: &str) -> bool {
    aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", git_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Prepare or reuse a worktree at `wt_path` on `branch` tracking `start_ref`
/// (e.g. `origin/<branch>` for `take`, or `origin/main` for `impl`) without
/// ever force-pushing.
// trace:STORY-1917 | ai:agy
pub(crate) fn prepare_lane_worktree(
    main_worktree: &Path,
    wt_path: &Path,
    branch: &str,
    fetch_ref: &str,
    start_ref: &str,
) -> Result<()> {
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Best-effort fetch of the base/branch from origin if `origin` remote is configured.
    let has_origin = aida_core::git_ops::host_git_command(main_worktree)
        .args(["remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_origin {
        let fetch_out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["fetch", "origin", fetch_ref])
            .output()
            .with_context(|| format!("failed to run git fetch origin {fetch_ref}"))?;
        if !fetch_out.status.success() {
            bail!(
                "git fetch origin {fetch_ref} failed: {}",
                String::from_utf8_lossy(&fetch_out.stderr).trim()
            );
        }
    }

    let resolved_start = if git_ref_exists(main_worktree, start_ref) {
        start_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{fetch_ref}")) {
        fetch_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{branch}")) {
        branch.to_string()
    } else {
        "HEAD".to_string()
    };

    if wt_path.exists() {
        let out = aida_core::git_ops::host_git_command(wt_path)
            .args(["checkout", "-B", branch, &resolved_start])
            .output()
            .with_context(|| format!("failed to checkout {branch} in {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git checkout -B {branch} {resolved_start} in {} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    } else {
        let out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "add", "-B", branch])
            .arg(wt_path)
            .arg(&resolved_start)
            .output()
            .with_context(|| format!("failed to add worktree {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git worktree add -B {branch} {} {resolved_start} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    // Ensure `.aida` inside the worktree links back to `main_worktree/.aida`
    // so the implementer sees the project config, sessions, and mailbox.
    let wt_aida = wt_path.join(".aida");
    if !wt_aida.exists() && std::fs::symlink_metadata(&wt_aida).is_err() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(main_worktree.join(".aida"), &wt_aida);
    }
    Ok(())
}

/// Build the scrubbed `Command` that launches an implementer worker in `wt_path`
/// with the session grant and purpose environment variables cleared and
/// `AIDA_SESSION_ROLE="implementer"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn build_implementer_launch_command(
    agent_bin: &Path,
    main_worktree: &Path,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    name: &str,
    vendor: DispatchVendor,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(agent_bin);
    cmd.current_dir(wt_path);
    match vendor {
        DispatchVendor::Claude => {
            cmd.args(["--bg", "-n", name, prompt_text]);
        }
        DispatchVendor::Codex => {
            cmd.args(["exec", "--full-auto", "-C"])
                .arg(wt_path)
                .arg(prompt_text);
        }
        DispatchVendor::Agy => {
            cmd.args(["--prompt", prompt_text]);
        }
    }
    // Critical Boundary B requirement: implementer lanes never inherit or hold
    // any session grant or purpose.
    cmd.env(seat_authority::GRANT_ENV, "")
        .env(seat_authority::GRANT_FILE_ENV, "")
        .env("AIDA_SESSION_PURPOSE", "")
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_SESSION_SCOPE", spec)
        .env("AIDA_PROJECT_ROOT", main_worktree)
        .env("AIDA_AGENT_NAME", name)
        .env("AIDA_USER", name);
    cmd
}

#[allow(clippy::too_many_arguments)]
fn run_take_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    pr: u64,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body = match verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)
    {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("frozen brief verification failed: {err:#}"),
            }
        }
    };

    let wt_name = format!("take-{pr}");
    let default_wt = worktrees_root(main_worktree).join(&wt_name);
    // Reuse an existing worktree that already has `branch` checked out when
    // `take-<pr>` does not exist yet (matching `take.sh`).
    let wt_path = if !default_wt.exists() {
        find_worktree_for_branch(main_worktree, branch).unwrap_or(default_wt)
    } else {
        default_wt
    };
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch takeover on PR #{pr}: {busy}"),
        };
    }

    let start_ref = format!("origin/{branch}");
    if let Err(err) = prepare_lane_worktree(main_worktree, &wt_path, branch, branch, &start_ref) {
        return RunnerOutcome::Failed {
            summary: format!("takeover worktree setup failed for PR #{pr}: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action, &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render takeover prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        &wt_name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_impl_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    name: &str,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body = match verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)
    {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("frozen brief verification failed: {err:#}"),
            }
        }
    };

    let wt_path = worktrees_root(main_worktree).join(name);
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch impl lane `{name}` ({spec}): {busy}"),
        };
    }

    if let Err(err) = prepare_lane_worktree(main_worktree, &wt_path, branch, "main", "origin/main")
    {
        return RunnerOutcome::Failed {
            summary: format!("impl worktree setup failed for `{name}`: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action, &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render impl prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

fn parse_spawned_bg_session_id(stdout: &str) -> Option<String> {
    if let Some(id) = crate::agent_launch::parse_bg_session_id(stdout) {
        return Some(id);
    }
    for line in stdout.lines() {
        if let Some((_, rest)) = line.split_once("claude logs ") {
            let token = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn launch_implementer_lane(
    main_worktree: &Path,
    req: &DispatchRequest,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    lane_name: &str,
    vendor: DispatchVendor,
) -> RunnerOutcome {
    let agent_bin = resolve_vendor_bin(vendor);

    let mut cmd = build_implementer_launch_command(
        &agent_bin,
        main_worktree,
        wt_path,
        spec,
        prompt_text,
        lane_name,
        vendor,
    );

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            let _ = append_dispatch_log(main_worktree, &req.id, &combined);
            let code = out.status.code();
            if out.status.success() {
                let bg_id = parse_spawned_bg_session_id(&stdout);
                RunnerOutcome::Done {
                    summary: format!(
                        "launched {} {} in {}",
                        req.action.verb_str(),
                        req.action.subject_label(),
                        wt_path.display()
                    ),
                    spawned_session: bg_id,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!(
                        "{} {} launcher exited with {:?}: {}",
                        req.action.verb_str(),
                        req.action.subject_label(),
                        code,
                        stderr.lines().next().unwrap_or("launch failed").trim()
                    ),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn implementer launcher: {err:#}"),
            exit_code: None,
        },
    }
}

#[cfg(test)]
#[path = "tests/story_1917_dispatch_request_tests.rs"]
mod story_1917_dispatch_request_tests;

#[cfg(test)]
#[path = "tests/story_1917_dispatch_exec_tests.rs"]
mod story_1917_dispatch_exec_tests;

/// Queue and inspect typed dispatch requests (`flow`, `review`, `take`,
/// `impl`, `train`) from a sandboxed advisor seat for unsandboxed execution.
// trace:STORY-1917 | ai:agy
#[derive(clap::Parser, Debug)]
#[clap(name = "dispatch")]
pub(crate) struct DispatchArgs {
    #[clap(subcommand)]
    pub action: crate::cli::DispatchAction,
}

fn run_dispatch(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <DispatchArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_dispatch(&args.action)
}

// trace:STORY-1917 | ai:agy
pub(crate) static DISPATCH: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "dispatch",
        help_after: "fleet",
        command: <DispatchArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_dispatch),
    };
