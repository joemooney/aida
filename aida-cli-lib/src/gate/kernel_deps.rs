//! Small inputs the gate's decision kernels read, implemented on the floor
//! (TASK-b6b1278890774ef993d2ebc327862470-024): the shell user identity, the
//! home directory, the human-at-terminal probes, the review verdict's git
//! ancestry check and the git argument guards. Gate modules call these here;
//! the historical paths outside `gate/` delegate to them for their own
//! callers. What a decision kernel still reaches outside `gate/` is listed in
//! `gate::dispatch_deps::FACADE_EXTERNAL_LEAVES`.
// mandate-gate
// trace:TASK-b6b1278890774ef993d2ebc327862470-024 | ai:claude

use std::io::IsTerminal;

// ── Identity and home ───────────────────────────────────────────────────────

/// Resolve the queue's "current user" identifier. Used by EVERY queue path
/// (add, list, next, done, remove, move, work, role-show queue head, …) so
/// items added in one shell are immediately visible in the same shell's
/// `aida queue list` without flags.
///
/// Resolution order:
///   1. Explicit `--user <id>` flag (override)
///   2. `AIDA_USER` env var (set by sessions / agent harnesses)
///   3. `USER` env var (POSIX shell convention)
///   4. `USERNAME` env var (Windows fallback)
///   5. The literal string "default" (last-ditch)
///
/// IMPORTANT: this is the SHELL's user identity, not the node identity from
/// `~/.aida/node.toml`, the email in `[node]`, or the role's stored user_id.
/// Those are different identity domains; mixing them caused BUG-89 (items
/// invisible to their own queuer because list resolved to one identity and
/// add resolved to another).
// trace:BUG-89 | ai:claude
pub(crate) fn current_user_id(user_override: Option<&str>) -> String {
    user_override.map(str::to_string).unwrap_or_else(|| {
        std::env::var("AIDA_USER")
            .or_else(|_| std::env::var("USER"))
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default".to_string())
    })
}

/// TASK-32: cross-platform home-dir lookup with an `AIDA_HOME` override.
/// On Windows, `dirs::home_dir()` resolves via `SHGetKnownFolderPath`,
/// which ignores env vars — that breaks bg_worker tests that need to
/// isolate writes from the real user profile. Checking `AIDA_HOME` first
/// gives tests a deterministic hook on every platform without changing
/// the production lookup.
// trace:TASK-32 | ai:claude
pub(crate) fn aida_home_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AIDA_HOME") {
        if !p.is_empty() {
            let p = std::path::PathBuf::from(p);
            // trace:BUG-1642 | ai:claude
            #[cfg(test)]
            crate::test_home::assert_hermetic(&p);
            return Some(p);
        }
    }
    crate::home_dir()
}

// ── Contained launch evidence ───────────────────────────────────────────────

/// TASK-2708: the containment receipt the seat-authority kernel reads before
/// binding a launch child's grant to a contained worker
/// (`seat_authority::bind_contained_launch_worker`). It feeds that authority
/// decision (only an accepted receipt's worker pid may be bound), so the gate
/// names it here on the floor rather than reaching into `containment` itself.
// trace:TASK-2708 | ai:claude
pub(crate) use crate::containment::launch::ContainmentReceipt;

// ── Human at a terminal ─────────────────────────────────────────────────────

/// BUG-1618: whether stdin is an interactive terminal, as the advisor-authority
/// checks see it. Production reads the real stdin; a test that pinned a
/// hermetic ambient context gets its injected answer, so running `cargo test`
/// from an interactive shell cannot grant the TTY carve-out to a test that
/// asserts a refusal.
// trace:BUG-1618 | ai:claude
pub(crate) fn authority_stdin_is_terminal() -> bool {
    #[cfg(test)]
    if let Some(tty) = crate::test_ambient::stdin_is_terminal() {
        return tty;
    }
    std::io::stdin().is_terminal()
}

// ── Deputy jobs on the integrity floor ──────────────────────────────────────

/// Set by `aida deputy` on every job it runs: the job id, and the spool the
/// job came from. The deputy reads these names from here.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) const DEPUTY_JOB_ENV: &str = "AIDA_DEPUTY_JOB";
pub(crate) const DEPUTY_SPOOL_ENV: &str = "AIDA_DEPUTY_SPOOL";

/// The marker the deputy writes at `<spool>/running/<job>.tty` while a job
/// runs on a terminal its TTY policy authorized. It is bound to the claimed
/// job FILE, not only its id: `job_dev`/`job_ino` are the identity of
/// `running/<job>.job` at claim time, so a different file under the same id
/// (a forged resubmission) never inherits the marker.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct DeputyTtyMarker {
    pub job: String,
    pub entry: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub job_dev: u64,
    pub job_ino: u64,
}

/// Where a claimed job's file sits while it runs: `<spool>/running/<job>.job`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn deputy_running_job_path(spool: &std::path::Path, job: &str) -> std::path::PathBuf {
    spool.join("running").join(format!("{job}.job"))
}

/// The (device, inode) identity a terminal marker binds to.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
#[cfg(unix)]
pub(crate) fn deputy_file_identity(md: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (md.dev(), md.ino())
}

#[cfg(not(unix))]
pub(crate) fn deputy_file_identity(_md: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Where the deputy keeps job `job`'s terminal marker under `spool`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn deputy_tty_marker_path(spool: &std::path::Path, job: &str) -> std::path::PathBuf {
    spool.join("running").join(format!("{job}.tty"))
}

/// Defence in depth for the integrity floor (signed off on the spec, Q6): a
/// process that carries a deputy job's environment holds the floor only while
/// the deputy's own marker says the TTY policy authorized that job a terminal.
/// Outside a deputy job (no `AIDA_DEPUTY_JOB`) this is always true and the
/// floor is the terminal check alone.
///
/// What it guarantees: a job, or any child that keeps the job environment
/// (every `aida` command does), cannot use a terminal it found some other way
/// (`script(1)`, a pty helper) to pass the floor. What it does NOT guarantee:
/// a job that can clear its own environment escapes this check. The command
/// deputy runs no shell scripts (those belong to the script deputy, `aida
/// deputy shell`), so only an `--allow-exec` program could run
/// `env -u AIDA_DEPUTY_JOB script …`. Against that, the boundary is the
/// operator's start-time choice to name the program; the deputy still never
/// hands it its terminal. The script deputy deliberately sets no
/// `AIDA_DEPUTY_JOB`: each of its jobs is the operator at their own terminal,
/// a consent the operator gives by starting it there.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
// trace:STORY-2338 | ai:claude
pub(crate) fn deputy_job_floor_authorized() -> bool {
    let job = std::env::var(DEPUTY_JOB_ENV).ok();
    let spool = std::env::var(DEPUTY_SPOOL_ENV).ok();
    deputy_job_floor_authorized_from(job.as_deref(), spool.as_deref(), chrono::Utc::now())
}

/// Pure-input half of [`deputy_job_floor_authorized`]. Fails closed on every
/// doubt: no spool, a job id that is not a UUID, a spool or `running/`
/// directory not owned by us or open to others, or a marker that is missing,
/// linked, foreign, writable by others, unparsable, for another job, or
/// expired, or a marker bound to a different job file than the one now at
/// `running/<job>.job`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn deputy_job_floor_authorized_from(
    job: Option<&str>,
    spool: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(job) = job.filter(|j| !j.is_empty()) else {
        return true;
    };
    let Some(spool) = spool.filter(|s| !s.is_empty()) else {
        return false;
    };
    let canonical_id = uuid::Uuid::parse_str(job).is_ok_and(|u| u.hyphenated().to_string() == job);
    if !canonical_id {
        return false;
    }
    let spool = std::path::Path::new(spool);
    let private_dir = |d: &std::path::Path| {
        std::fs::symlink_metadata(d)
            .is_ok_and(|md| md.file_type().is_dir() && owned_without(&md, 0o077))
    };
    if !private_dir(spool) || !private_dir(&spool.join("running")) {
        return false;
    }
    let marker = deputy_tty_marker_path(spool, job);
    let Ok(md) = std::fs::symlink_metadata(&marker) else {
        return false;
    };
    if !md.file_type().is_file() || !owned_without(&md, 0o022) {
        return false;
    }
    let Ok(bytes) = std::fs::read(&marker) else {
        return false;
    };
    let Ok(m) = serde_json::from_slice::<DeputyTtyMarker>(&bytes) else {
        return false;
    };
    let Ok(job_md) = std::fs::symlink_metadata(deputy_running_job_path(spool, job)) else {
        return false;
    };
    if !job_md.file_type().is_file() || !owned_without(&job_md, 0o022) {
        return false;
    }
    m.job == job && m.expires_at > now && deputy_file_identity(&job_md) == (m.job_dev, m.job_ino)
}

/// Owned by this process's uid with none of the `forbidden` mode bits.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
#[cfg(unix)]
fn owned_without(md: &std::fs::Metadata, forbidden: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    md.uid() == unsafe { libc::getuid() } && md.mode() & forbidden == 0
}

#[cfg(not(unix))]
fn owned_without(_md: &std::fs::Metadata, _forbidden: u32) -> bool {
    true
}

/// Whether stdout is an interactive terminal, through the same test seam as
/// [`authority_stdin_is_terminal`]. A human-at-terminal gate checks both: an
/// agent that pipes stdout (or stdin) is not a person at a terminal.
// trace:BUG-1667 | ai:claude
pub(crate) fn authority_stdout_is_terminal() -> bool {
    #[cfg(test)]
    if let Some(tty) = crate::test_ambient::stdout_is_terminal() {
        return tty;
    }
    std::io::stdout().is_terminal()
}

/// TASK-1787: whether stderr is an interactive terminal, through the same test seam as
/// [`authority_stdin_is_terminal`].
// trace:TASK-1787 | ai:antigravity
pub(crate) fn authority_stderr_is_terminal() -> bool {
    #[cfg(test)]
    if let Some(tty) = crate::test_ambient::stderr_is_terminal() {
        return tty;
    }
    std::io::stderr().is_terminal()
}

// ── Git argument guards ─────────────────────────────────────────────────────

/// Marks the end of git's options: every later argument is a revision or a
/// path, even when it starts with `-`. Supported since git 2.24.
pub(crate) const END_OF_OPTIONS: &str = "--end-of-options";

/// True when `value` is an abbreviated or full commit ID: 7 to 64 hex
/// digits (SHA-1 or SHA-256). A value read from the store, a commit trailer
/// or a verdict must pass this before it becomes a git revision argument.
pub(crate) fn is_hex_sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

// ── Verdict freshness ───────────────────────────────────────────────────────

/// Return the branch name currently checked out at `path` (its HEAD).
/// `None` if detached or git fails.
/// Refuses a seat clone (no git runs there; BUG-2169).
// trace:BUG-76 | ai:claude
// trace:TASK-2053 | ai:claude
pub(crate) fn current_branch_at(path: &std::path::Path) -> Option<String> {
    current_branch_at_checked(path).ok()
}

/// Preserve Git failures (including seat-clone safety refusals) for gates
/// that must not treat missing branch evidence as permission to proceed.
// trace:TASK-2887 | ai:codex
pub(crate) fn current_branch_at_checked(path: &std::path::Path) -> anyhow::Result<String> {
    let o = aida_core::git_ops::host_git_command(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .map_err(|e| {
            anyhow::anyhow!("could not run git symbolic-ref in {}: {e}", path.display())
        })?;
    anyhow::ensure!(
        o.status.success(),
        "git symbolic-ref failed in {} ({}): {}",
        path.display(),
        o.status,
        String::from_utf8_lossy(&o.stderr).trim()
    );
    let branch = String::from_utf8(o.stdout)
        .map_err(|e| anyhow::anyhow!("invalid branch output in {}: {e}", path.display()))?;
    let branch = branch.trim();
    anyhow::ensure!(
        !branch.is_empty(),
        "empty branch output in {}",
        path.display()
    );
    Ok(branch.to_string())
}

/// Full object name for `rev` in `repo`, or `None` when it doesn't resolve to
/// a commit here (unknown sha, sha from another clone, garbage input).
/// Expanding both sides is what lets a short sha recorded in a review verdict
/// be compared with a branch tip.
// trace:BUG-775 | ai:claude
pub(crate) fn resolve_commit_sha(repo: &std::path::Path, rev: &str) -> Option<String> {
    let rev = rev.trim();
    if rev.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            &format!("{rev}^{{commit}}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// `git merge-base --is-ancestor <ancestor> <descendant>` as a tri-state:
/// `Some(true)` / `Some(false)` for the two clean answers, `None` when the
/// probe itself could not run.
// trace:BUG-775 | ai:claude
pub(crate) fn is_ancestor_commit(
    repo: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "merge-base",
            "--is-ancestor",
            END_OF_OPTIONS,
            ancestor,
            descendant,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Where the tip of `branch` (or HEAD when `branch` is `None`) sits relative
/// to the commit a review verdict named. All the git I/O for the review gate
/// lives here; the decision itself is the pure
/// `review_verdict::classify_tip_relation`.
// trace:BUG-775 | ai:claude
pub(crate) fn verdict_tip_relation(
    repo: &std::path::Path,
    branch: Option<&str>,
    reviewed_sha: Option<&str>,
) -> super::review_verdict::TipRelation {
    let Some(reviewed_raw) = reviewed_sha else {
        return super::review_verdict::TipRelation::Unknown;
    };
    let reviewed = resolve_commit_sha(repo, reviewed_raw);
    let tip = resolve_commit_sha(repo, branch.unwrap_or("HEAD"))
        .or_else(|| resolve_commit_sha(repo, "HEAD"));
    let ancestry = match (reviewed.as_deref(), tip.as_deref()) {
        (Some(a), Some(b)) => is_ancestor_commit(repo, a, b),
        _ => None,
    };
    super::review_verdict::classify_tip_relation(reviewed.as_deref(), tip.as_deref(), ancestry)
}

// Read-only commit evidence and ID normalization shared by the approval
// readers and shipping. Authority plumbing stays on the protected floor.
// trace:BUG-2398 | ai:codex
pub(crate) fn commit_committed_at(
    repo: &std::path::Path,
    sha: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let sha = sha.trim();
    if sha.is_empty() || sha.starts_with('-') {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%cI", sha, "--"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    chrono::DateTime::parse_from_rfc3339(String::from_utf8_lossy(&out.stdout).trim())
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

// trace:BUG-2398 | ai:codex
pub(crate) fn normalized_spec_ids(spec_ids: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = spec_ids
        .iter()
        .map(|id| id.trim().to_ascii_uppercase())
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Bounded OpenSSH/Git execution for trusted review publication. This leaf
/// supplies only process output; signature and authority decisions stay on floor.
// trace:STORY-2492 | ai:codex
pub(crate) fn bounded_review_output(
    cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: u64,
) -> Option<std::process::Output> {
    match crate::command_output_with_timeout_capped(cmd, timeout, cap) {
        crate::BoundedCommandOutput::Completed(out) => Some(out),
        _ => None,
    }
}

/// Only evidence for this marker's process may override its TTL. Closed
/// records have no holder binding and cannot end a later manual claim.
/// Outer None means there is no attributable evidence: use the marker's own
/// process identity, then its TTL. Store discovery failures and store-wide read
/// failures (including lock timeout or pending-batch recovery failure) instead
/// return Some(None): attribution cannot be established, so a pid-backed marker
/// stays retained even after process death or TTL expiry. Retry after repairing
/// the store, or explicitly use `aida review claim --release`.
// trace:STORY-2325 | ai:codex
pub(crate) fn review_ownership_ended(
    root: &std::path::Path,
    m: &super::review_marker::Marker,
) -> Option<Option<String>> {
    use crate::ownership::{store::OwnershipStore, WitnessKind, WorkKey};
    if m.pid == 0 {
        return None;
    }
    let store = match OwnershipStore::for_project(root) {
        Ok(Some(store)) => store,
        Ok(None) => return None,
        Err(_) => return Some(None),
    };
    let (records, errors) = store.list();
    // A damaged record for this PR may be this process's evidence. Retain it;
    // errors for other PRs/kinds do not suppress a manual marker's expiry.
    let prefix = format!("origin-{}-", m.pr);
    let unreadable_matching = errors.iter().any(|(path, _)| {
        path == store.dir()
            || path == &store.dir().join("review")
            || (path.parent() == Some(store.dir().join("review").as_path())
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix(&prefix))
                    .is_some_and(|head| super::review_marker::covers_head(head, Some(&m.head_sha))))
    });
    if unreadable_matching {
        return Some(None);
    }
    let matching: Vec<_> = records
        .iter()
        .filter(|r| {
            matches!(&r.key, WorkKey::Review { change, head }
            if change == &crate::ownership::dual::change(m.pr)
                && super::review_marker::covers_head(head, Some(&m.head_sha)))
                && r.holder.as_ref().is_some_and(|holder| {
                    holder.witnesses.iter().any(|w| match &w.kind {
                        WitnessKind::Process { host, id, .. } => {
                            m.pid != 0
                                && host == &m.host
                                && id.pid == m.pid
                                && m.pid_start.as_deref().is_some_and(|start| {
                                    aida_core::local_boot_id()
                                        .and_then(|boot| {
                                            crate::ownership::dual::identity_from_record(
                                                m.pid, start, &boot,
                                            )
                                        })
                                        .as_ref()
                                        == Some(id)
                                })
                        }
                        _ => false,
                    })
                })
        })
        .collect();
    if matching.is_empty() {
        return None;
    }
    let reasons: Option<Vec<_>> = matching
        .into_iter()
        .map(crate::ownership::reader::ended_record)
        .collect();
    Some(reasons.map(|r| r.join("; ")))
}
