//! BUG-1918: who may approve a change.
//!
//! The review gate used to decide authorship from a free-text label. This
//! module binds both sides to identity instead:
//!
//! * a **recorder** (the process running `aida review record`) is described by
//!   identity tokens that persist across shell calls — its session ids, its
//!   process ancestry (with kernel start identities), and every lease it is
//!   bound to by working directory or `AIDA_SESSION_ID`;
//! * a spec's **authors** are its authoring leases — live ones, plus the
//!   durable authorship ledger written when a lease is taken, so releasing the
//!   lease does not erase who claimed the work;
//! * a recorded approval's **authority** (its seat grant, or the human-review
//!   receipt written when the recorder passed the human-at-TTY floor) is
//!   re-read from its durable store at ship time instead of trusting the
//!   booleans stored beside the verdict.
//!
//! Token format: `kind:value`. Process tokens are `kind:<pid>@<start>`, where
//! `<start>` is the kernel start identity (empty when unknown). Kinds:
//! `session` (a session id from the environment or a lease's manifest),
//! `lease` (a lease id), `shell` (the shell that ran the command — only the
//! immediate parent), and `proc` (any ancestor process; a lease's hosted agent
//! process is recorded under this kind).
// trace:BUG-1918 | ai:claude

use std::path::{Path, PathBuf};

use crate::SessionLease;

/// File (under the main worktree's `.aida/`) holding the authorship ledger.
const LEDGER_FILE: &str = "review-authorship.jsonl";
/// Directory (under the user's AIDA home) holding human-review receipts.
const RECEIPT_DIR: &str = "review-receipts";

// ── Identity tokens ─────────────────────────────────────────────────────────

/// `kind:<pid>@<start>`.
pub(crate) fn process_token(kind: &str, pid: u32, start: Option<&str>) -> String {
    format!("{kind}:{pid}@{}", start.unwrap_or_default())
}

/// Do two identity tokens name the same identity? Plain tokens compare
/// exactly. Process tokens compare kind and pid, and the start identities
/// when both sides recorded one — so a recycled pid of a long-dead process is
/// not mistaken for it, while a legacy record without a start identity keeps
/// the historical pid-only match.
pub(crate) fn tokens_match(a: &str, b: &str) -> bool {
    let (Some((ka, va)), Some((kb, vb))) = (a.split_once(':'), b.split_once(':')) else {
        return false;
    };
    if ka != kb || va.is_empty() || vb.is_empty() {
        return false;
    }
    match (va.split_once('@'), vb.split_once('@')) {
        (Some((pa, sa)), Some((pb, sb))) => {
            !pa.is_empty() && pa == pb && (sa.is_empty() || sb.is_empty() || sa == sb)
        }
        (None, None) => va == vb,
        _ => false,
    }
}

/// Does any token in `a` name the same identity as any token in `b`?
pub(crate) fn identities_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| tokens_match(x, y)))
}

fn push_unique(out: &mut Vec<String>, token: String) {
    if !out.contains(&token) {
        out.push(token);
    }
}

/// Session ids the environment carries. `CLAUDE_CODE_SESSION_ID` is
/// production-only: the test process may itself run under Claude Code, and
/// its ambient session must not leak into hermetic fixtures.
fn env_session_tokens() -> Vec<String> {
    #[cfg(not(test))]
    const VARS: &[&str] = &["AIDA_SESSION_ID", "CLAUDE_CODE_SESSION_ID"];
    #[cfg(test)]
    const VARS: &[&str] = &["AIDA_SESSION_ID"];
    VARS.iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(|v| format!("session:{v}"))
        .collect()
}

/// The calling process's ancestry as `shell` (immediate parent) and `proc`
/// (every ancestor) tokens. Empty under `cfg(test)`: the test runner's own
/// ancestry is ambient, not part of any fixture.
fn ancestry_tokens() -> Vec<String> {
    #[cfg(test)]
    {
        Vec::new()
    }
    #[cfg(not(test))]
    {
        // The walk stops at the nearest Claude Code harness: everything above
        // it (a launcher's session, the operator's terminal) is a different
        // session, and an agent launched from inside an advisor's tree must
        // not inherit the advisor's identity.
        let mut out = Vec::new();
        let me = std::process::id();
        let harness = crate::process_probe::nearest_claude_ancestor_pid(me);
        let chain = crate::process_probe::walk_ancestor_pids(me);
        for (i, pid) in chain.into_iter().enumerate().skip(1) {
            if pid <= 1 {
                continue;
            }
            let start = crate::process_probe::process_start_identity(pid);
            if i == 1 {
                push_unique(&mut out, process_token("shell", pid, start.as_deref()));
            }
            push_unique(&mut out, process_token("proc", pid, start.as_deref()));
            if Some(pid) == harness {
                break;
            }
        }
        out
    }
}

/// The nearest Claude Code harness above this process, as a `proc` token —
/// the identity that survives across an agent's separate shell calls.
fn harness_token() -> Option<String> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        let pid = crate::process_probe::nearest_claude_ancestor_pid(std::process::id())?;
        let start = crate::process_probe::process_start_identity(pid);
        Some(process_token("proc", pid, start.as_deref()))
    }
}

/// The identity tokens a lease binds: its id, the shell that created it, its
/// hosted agent process, and the agent conversation its manifest recorded.
pub(crate) fn lease_tokens(lease: &SessionLease, manifest_session: Option<&str>) -> Vec<String> {
    let mut out = vec![format!("lease:{}", lease.id)];
    if let Some(pid) = lease.creator_pid {
        out.push(process_token(
            "shell",
            pid,
            lease.creator_pid_start_time.as_deref(),
        ));
    }
    if let Some(pid) = lease.active_pid {
        out.push(process_token(
            "proc",
            pid,
            lease.active_pid_start_time.as_deref(),
        ));
    }
    if let Some(id) = manifest_session.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(format!("session:{id}"));
    }
    out
}

fn manifest_session_id(project_root: &Path, lease: &SessionLease) -> Option<String> {
    let root = lease
        .parent_project_root
        .clone()
        .unwrap_or_else(|| project_root.to_path_buf());
    crate::session_manifest::load(&crate::session_manifest::manifest_path(&root, &lease.id))
        .ok()
        .and_then(|m| m.claude_session_id)
}

/// Is `lease` bound to a process acting from `dirs` with `AIDA_SESSION_ID`
/// `session_id` — working inside its worktree, or carrying its id?
fn lease_bound_by_context(
    lease: &SessionLease,
    dirs: &[PathBuf],
    session_id: Option<&str>,
) -> bool {
    let in_worktree = dirs.iter().any(|d| crate::lease_covers_cwd(lease, d));
    let carries_id = session_id
        .map(str::trim)
        .filter(|s| s.len() >= 8)
        .is_some_and(|s| lease.id.starts_with(s) || s.starts_with(&lease.id));
    in_worktree || carries_id
}

/// BUG-1918: the identity of the process recording a review — what it is
/// (session ids, ancestry) plus the leases its context binds it to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecorderIdentity {
    pub(crate) tokens: Vec<String>,
}

impl RecorderIdentity {
    /// Resolve the current process's identity against `leases`.
    pub(crate) fn current(project_root: &Path, leases: &[SessionLease]) -> Self {
        let dirs: Vec<PathBuf> = [
            std::env::current_dir().ok(),
            Some(project_root.to_path_buf()),
        ]
        .into_iter()
        .flatten()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
        let session_id = std::env::var("AIDA_SESSION_ID").ok();
        let mut tokens = env_session_tokens();
        for t in ancestry_tokens() {
            push_unique(&mut tokens, t);
        }
        Self::with_context(tokens, leases, &dirs, session_id.as_deref())
    }

    /// Pure core of [`Self::current`]: `base` tokens plus a `lease:` token
    /// for every lease the context binds.
    pub(crate) fn with_context(
        base: Vec<String>,
        leases: &[SessionLease],
        dirs: &[PathBuf],
        session_id: Option<&str>,
    ) -> Self {
        let mut tokens = base;
        for lease in leases {
            if lease_bound_by_context(lease, dirs, session_id) {
                push_unique(&mut tokens, format!("lease:{}", lease.id));
            }
        }
        Self { tokens }
    }
}

// ── Authorship ──────────────────────────────────────────────────────────────

/// Is `lease` an authoring (implementer/claim) lease? Review leases and
/// PR-scoped leases belong to reviewers and never are.
pub(crate) fn is_authoring_lease(lease: &SessionLease) -> bool {
    let scope = lease.scope.trim();
    !lease.review_verb && !scope.is_empty() && !scope.to_ascii_uppercase().starts_with("PR-")
}

/// One durable authorship record: who took an authoring lease on `scope`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AuthorshipRecord {
    pub(crate) scope: String,
    pub(crate) lease_id: String,
    #[serde(default)]
    pub(crate) branch: String,
    #[serde(default)]
    pub(crate) tokens: Vec<String>,
    pub(crate) recorded_at: String,
}

fn ledger_path(project_root: &Path) -> PathBuf {
    crate::main_worktree_root_from(project_root)
        .join(".aida")
        .join(LEDGER_FILE)
}

/// Append a durable authorship record for `lease` when it is an authoring
/// lease. The record carries the lease's own tokens plus the CLAIMING
/// process's session ids and agent harness, so the claimer stays identifiable
/// after the lease (and the shell that took it) are gone. Best effort: a
/// failed write never blocks taking the lease.
pub(crate) fn record_authoring_lease(project_root: &Path, lease: &SessionLease) {
    if !is_authoring_lease(lease) {
        return;
    }
    let mut tokens = lease_tokens(lease, None);
    for t in env_session_tokens().into_iter().chain(harness_token()) {
        push_unique(&mut tokens, t);
    }
    let record = AuthorshipRecord {
        scope: lease.scope.trim().to_string(),
        lease_id: lease.id.clone(),
        branch: lease.branch.clone(),
        tokens,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let _ = append_record(&ledger_path(project_root), &record);
}

fn append_record(path: &Path, record: &AuthorshipRecord) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

/// Every parseable record in the authorship ledger.
pub(crate) fn load_ledger(project_root: &Path) -> Vec<AuthorshipRecord> {
    std::fs::read_to_string(ledger_path(project_root))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Is `lease` an authoring lease scoped to one of `spec_ids`?
pub(crate) fn is_authoring_lease_for(lease: &SessionLease, spec_ids: &[String]) -> bool {
    is_authoring_lease(lease) && scope_in(&lease.scope, spec_ids)
}

fn scope_in(scope: &str, spec_ids: &[String]) -> bool {
    let scope = scope.trim();
    !scope.is_empty()
        && spec_ids
            .iter()
            .any(|id| scope.eq_ignore_ascii_case(id.trim()))
}

fn branch_is(branch: &str, head_branch: Option<&str>) -> bool {
    head_branch
        .map(str::trim)
        .is_some_and(|b| !b.is_empty() && branch.trim() == b)
}

/// The author sessions of the work under review: every authoring lease (live,
/// or durably recorded in the ledger) scoped to one of `spec_ids` or working
/// on `head_branch`.
pub(crate) fn author_sessions(
    leases: &[SessionLease],
    ledger: &[AuthorshipRecord],
    spec_ids: &[String],
    head_branch: Option<&str>,
    manifest_session: impl Fn(&SessionLease) -> Option<String>,
) -> Vec<crate::pr_ship::AuthorSession> {
    let mut out: Vec<crate::pr_ship::AuthorSession> = Vec::new();
    for lease in leases {
        if is_authoring_lease_for(lease, spec_ids)
            || (is_authoring_lease(lease) && branch_is(&lease.branch, head_branch))
        {
            out.push(crate::pr_ship::AuthorSession {
                label: format!("lease {} on {}", lease.id, lease.scope),
                tokens: lease_tokens(lease, manifest_session(lease).as_deref()),
            });
        }
    }
    for rec in ledger {
        if scope_in(&rec.scope, spec_ids) || branch_is(&rec.branch, head_branch) {
            out.push(crate::pr_ship::AuthorSession {
                label: format!("lease {} on {} (claim record)", rec.lease_id, rec.scope),
                tokens: rec.tokens.clone(),
            });
        }
    }
    out
}

/// [`author_sessions`] read from `project_root`'s lease store (and its main
/// worktree's) and authorship ledger.
pub(crate) fn author_sessions_at(
    project_root: &Path,
    spec_ids: &[String],
    head_branch: Option<&str>,
) -> Vec<crate::pr_ship::AuthorSession> {
    author_sessions(
        &visible_leases(project_root),
        &load_ledger(project_root),
        spec_ids,
        head_branch,
        |l| manifest_session_id(project_root, l),
    )
}

/// Every lease visible from `project_root` and its main worktree,
/// deduplicated by id.
pub(crate) fn visible_leases(project_root: &Path) -> Vec<SessionLease> {
    let main = crate::main_worktree_root_from(project_root);
    let mut seen = std::collections::HashSet::new();
    crate::list_leases(&main)
        .into_iter()
        .chain(crate::list_leases(project_root))
        .filter(|l| seen.insert(l.id.clone()))
        .collect()
}

// ── Human-review receipts ──────────────────────────────────────────────────

/// BUG-1918: a durable record, in the user's AIDA home, that a human passed
/// the human-at-TTY floor when recording an approval of `sha`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HumanReviewReceipt {
    pub(crate) id: String,
    pub(crate) sha: String,
    pub(crate) subject: String,
    pub(crate) recorded_at: String,
}

fn receipt_dir() -> Option<PathBuf> {
    crate::aida_home_dir().map(|h| h.join(".aida").join(RECEIPT_DIR))
}

fn receipt_path(dir: &Path, id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(id).ok()?;
    Some(dir.join(format!("{id}.json")))
}

/// Write a receipt for a human-at-TTY approval of `sha` on `subject`;
/// returns its id.
pub(crate) fn write_human_receipt(sha: &str, subject: &str) -> anyhow::Result<String> {
    let dir = receipt_dir().ok_or_else(|| anyhow::anyhow!("cannot locate the AIDA home"))?;
    write_human_receipt_in(&dir, sha, subject)
}

pub(crate) fn write_human_receipt_in(
    dir: &Path,
    sha: &str,
    subject: &str,
) -> anyhow::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = HumanReviewReceipt {
        id: id.clone(),
        sha: sha.trim().to_string(),
        subject: subject.trim().to_string(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    std::fs::create_dir_all(dir)?;
    let path = receipt_path(dir, &id).ok_or_else(|| anyhow::anyhow!("invalid receipt id"))?;
    aida_core::write_atomic(&path, serde_json::to_string_pretty(&receipt)?)?;
    Ok(id)
}

/// Does receipt `id` exist and vouch for exactly `sha`?
pub(crate) fn human_receipt_valid(id: &str, sha: &str) -> bool {
    receipt_dir().is_some_and(|dir| human_receipt_valid_in(&dir, id, sha))
}

pub(crate) fn human_receipt_valid_in(dir: &Path, id: &str, sha: &str) -> bool {
    receipt_path(dir, id)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<HumanReviewReceipt>(&b).ok())
        .is_some_and(|r| {
            r.id == id
                && !sha.trim().is_empty()
                && crate::review_verdict::same_reviewed_sha(&r.sha, sha)
        })
}

// ── Ship-time re-validation ────────────────────────────────────────────────

/// BUG-1918: the authority keys (see
/// [`crate::review_verdict::RecorderAttestation::authority_key`]) of every
/// attested approval among `candidates` whose authority re-validates against
/// its durable store: the seat grant was valid for that (non-implementer)
/// seat at the verdict's `recorded_at`, or the human-review receipt exists
/// for the attested commit.
pub(crate) fn verified_authority_keys(
    project_root: &Path,
    candidates: &[crate::review_verdict::RecordedVerdict],
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for v in candidates {
        let Some(att) = v.attestation.as_ref() else {
            continue;
        };
        let Some(key) = att.authority_key(v.recorded_at.as_deref()) else {
            continue;
        };
        let ok = match (att.grant_id.as_deref(), att.seat.as_deref()) {
            (Some(grant), Some(seat)) => v
                .recorded_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
                .is_some_and(|at| {
                    crate::seat_authority::grant_authorized_at(
                        project_root,
                        grant,
                        seat,
                        at.with_timezone(&chrono::Utc),
                    )
                }),
            _ => att
                .receipt_id
                .as_deref()
                .is_some_and(|id| human_receipt_valid(id, &att.sha)),
        };
        if ok {
            out.insert(key);
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/bug_1918_review_authority_tests.rs"]
mod bug_1918_review_authority_tests;
