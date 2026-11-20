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

/// Mandates belong to the operator, while AIDA_USER identifies a queue/session.
/// Keep mint, renewal and every reader in the same identity domain. An unknown
/// operator stays empty so record validation and minting refuse it.
// trace:BUG-2524 | ai:codex
pub(crate) fn operator_principal_from(env: &aida_core::exec_context::Environment) -> String {
    ["USER", "USERNAME"]
        .into_iter()
        .find_map(|key| env.var(key).ok().map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string()
}

// trace:BUG-2524 | ai:codex
pub(crate) fn operator_principal() -> String {
    operator_principal_from(&aida_core::exec_context::Environment::capture())
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
pub(crate) use crate::git_arg_guard::END_OF_OPTIONS;

/// True when `value` is an abbreviated or full commit ID: 7 to 64 hex
/// digits (SHA-1 or SHA-256). A value read from the store, a commit trailer
/// or a verdict must pass this before it becomes a git revision argument.
pub(crate) fn is_hex_sha(value: &str) -> bool {
    crate::git_arg_guard::is_hex_sha(value)
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
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(repo).args([
        "rev-parse",
        "--verify",
        "--quiet",
        END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
        &format!("{rev}^{{commit}}"),
    ]);
    // trace:TASK-3792 | ai:claude
    let out = local_output(cmd).ok()?;
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
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(repo).args([
        "merge-base",
        "--is-ancestor",
        END_OF_OPTIONS,
        ancestor,
        descendant,
    ]); // trace:BUG-1622 | ai:claude
        // trace:TASK-3792 | ai:claude
    let out = local_output(cmd).ok()?;
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
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%cI", sha, "--"]);
    // trace:TASK-3792 | ai:claude
    let out = local_output(cmd).ok().filter(|o| o.status.success())?;
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

/// Run a gate subprocess to completion, or, under a caller's `deadline`,
/// kill its process group when the deadline passes. An expired deadline
/// never spawns it. Any outcome but completion is an error: callers read
/// that as unavailable evidence, which grants nothing.
// trace:TASK-3792 | ai:claude
pub(crate) fn output_before(
    mut cmd: std::process::Command,
    deadline: Option<std::time::Instant>,
) -> std::io::Result<std::process::Output> {
    let Some(deadline) = deadline else {
        return cmd.output();
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    match crate::command_output_with_timeout_capped(cmd, remaining, u64::MAX) {
        crate::BoundedCommandOutput::Completed(out) => Ok(out),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "did not finish before the deadline",
        )),
    }
}

/// Run a local (never-prompting) gate subprocess through the bounded
/// transport with no ceiling of its own, so a scoped read deadline
/// (`aida pr train --check --timeout`) bounds it and an expired one never
/// spawns it. Any outcome but completion is an error: unavailable evidence,
/// which grants nothing.
// trace:TASK-3792 | ai:claude
pub(crate) fn local_output(
    mut cmd: std::process::Command,
) -> std::io::Result<std::process::Output> {
    cmd.stdin(std::process::Stdio::null());
    local_output_inner(cmd)
}

/// [`local_output`] feeding `input` to the child's stdin from a writer
/// thread, which ends with a broken pipe once the child is gone.
// trace:TASK-3792 | ai:claude
pub(crate) fn local_output_fed(
    mut cmd: std::process::Command,
    input: Vec<u8>,
) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    let (reader, mut writer) = std::io::pipe()?;
    cmd.stdin(reader);
    let (wrote_tx, wrote_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = wrote_tx.send(writer.write_all(&input));
    });
    // `cmd`, and with it the read end, is dropped when the run returns.
    let out = local_output_inner(cmd)?;
    // A child that did not read all of its input produced no result for it.
    match wrote_rx.recv_timeout(std::time::Duration::from_millis(500)) {
        Ok(Ok(())) => Ok(out),
        _ => Err(std::io::Error::other(
            "the child did not read all of its input",
        )),
    }
}

fn local_output_inner(cmd: std::process::Command) -> std::io::Result<std::process::Output> {
    match crate::command_output_with_timeout_capped(cmd, std::time::Duration::MAX, u64::MAX) {
        crate::BoundedCommandOutput::Completed(out) => Ok(out),
        _ => Err(std::io::Error::other(
            "could not run, or did not finish before the read deadline",
        )),
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

// trace:TASK-3814 | ai:codex — review policy and its class resolver feed verdict decisions.
pub(crate) mod review_classification {
    /// The defect-class vocabulary: `(name, meaning)`. Additive — a new class is
    /// one line here. Names are kebab-case and stable once recorded.
    pub const FINDING_CLASSES: &[(&str, &str)] = &[
        (
            "incomplete-fix",
            "fixed the cited site but left named siblings or other call sites",
        ),
        (
            "fail-open",
            "an error, missing input or unknown state is treated as success",
        ),
        (
            "absent-evidence-reads-as-good",
            "the absence of a signal is read as a pass",
        ),
        (
            "untested-path",
            "the changed behavior has no test that would fail without it",
        ),
        (
            "contract-drift",
            "a schema, API, CLI or doc contract changed without its consumers",
        ),
        ("race", "concurrency, ordering or TOCTOU defect"),
        ("perf", "avoidable cost on a hot path"),
        (
            "portability",
            "assumes one OS, shell, filesystem or toolchain",
        ),
        ("stale-base", "reviewed or built against an outdated base"),
        ("scope-creep", "changes outside what the spec asked for"),
    ];

    /// Accepted spellings that map onto a vocabulary member.
    const CLASS_ALIASES: &[(&str, &str)] = &[
        ("missing-test", "untested-path"),
        ("untested", "untested-path"),
        ("contract-break", "contract-drift"),
        ("partial-fix", "incomplete-fix"),
        ("performance", "perf"),
        ("toctou", "race"),
    ];

    /// One `--finding-class` argument, resolved.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ClassArg {
        /// A vocabulary member (canonical name).
        Class(&'static str),
        /// An explicit "no class for this finding" (`-` or `none`).
        Skip,
        /// Not in the vocabulary; carries the input as given.
        Unknown(String),
    }

    /// Resolve a class name: case-insensitive, `_`/space folded to `-`, aliases
    /// applied. Never guesses from anything but the name itself.
    pub fn parse_class(raw: &str) -> ClassArg {
        let norm = raw.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        if norm.is_empty() || norm == "-" || norm == "none" {
            return ClassArg::Skip;
        }
        if let Some((name, _)) = FINDING_CLASSES.iter().find(|(n, _)| *n == norm) {
            return ClassArg::Class(name);
        }
        if let Some((_, target)) = CLASS_ALIASES.iter().find(|(a, _)| *a == norm) {
            return ClassArg::Class(target);
        }
        ClassArg::Unknown(raw.trim().to_string())
    }
}

pub(crate) mod review_blocking {
    //! `[review] blocking_tiers`: which finding classes block a merge.
    //!
    //! The list is read ONLY from the trusted default-branch copy of
    //! `.aida/config.toml`, never from the working tree, so a branch cannot
    //! relax the policy its own review runs under; changing it means landing
    //! the edit on the default branch (a gate-protected path, so a human merge)
    //! and `aida review blocking-tiers set` records who did it under what
    //! authority. Absent, unreadable, unparseable or unknown entries all
    //! resolve to the default: every finding blocks (fail closed).
    // trace:STORY-2913 | ai:claude
    use anyhow::{bail, Context, Result};
    use std::collections::BTreeSet;
    use std::path::Path;

    /// The documented class registry: `(name, meaning)`. Names are stable.
    // trace:STORY-2913 | ai:claude
    pub(crate) const CLASSES: &[(&str, &str)] = &[
        ("p1", "priority-1 defect: wrong behaviour users will hit"),
        ("p2", "priority-2 defect: real but contained"),
        ("p3", "priority-3 observation: minor or cosmetic defect"),
        ("data-loss", "data can be lost, corrupted or silently dropped"),
        (
            "security-authority-bypass",
            "a security defect, or a path that grants, checks or bypasses authority wrongly",
        ),
        (
            "unmet-claimed-ac",
            "an acceptance criterion the change claims to meet is not met",
        ),
        (
            "test-gap",
            "changed behaviour has no test that would fail without it (untested path, surviving mutant)",
        ),
        ("style", "style, naming or readability only"),
    ];

    /// Accepted spellings of registry classes (earlier tier names and the
    /// finding-class vocabulary's test-evidence classes).
    // trace:STORY-2913 | ai:claude
    const ALIASES: &[(&str, &str)] = &[
        ("security", "security-authority-bypass"),
        ("authority-bypass", "security-authority-bypass"),
        ("acceptance", "unmet-claimed-ac"),
        ("untested-path", "test-gap"),
        ("missing-test", "test-gap"),
        ("untested", "test-gap"),
        ("mutation-kill", "test-gap"),
    ];

    /// Blocking-tier spellings that, under ADR-76, assert the finding sits in
    /// gate, sandbox, authority, seat or containment code: they block
    /// regardless of the configured list.
    const ADR76_TIER_SPELLINGS: &[&str] = &["mutation-kill", "untested-path"];

    fn normalize(raw: &str) -> String {
        raw.trim().to_ascii_lowercase().replace(['_', ' '], "-")
    }

    /// The registry name for `raw` (case-insensitive, `_`/space folded to `-`,
    /// aliases applied), or `None` when it names no registry class.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn canonical(raw: &str) -> Option<&'static str> {
        let n = normalize(raw);
        if let Some((name, _)) = CLASSES.iter().find(|(c, _)| *c == n) {
            return Some(name);
        }
        ALIASES.iter().find(|(a, _)| *a == n).map(|(_, c)| *c)
    }

    /// The configured blocking classes, or every finding blocks.
    // trace:STORY-2913 | ai:claude
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Policy {
        /// The default: no finding defers by class (no policy follow-up
        /// routing). With the list unset, a reviewer's explicit,
        /// rationale-bearing `deferred` disposition keeps its existing
        /// meaning; when the list fails closed ([`Effective::fails_closed`])
        /// that disposition is overridden too.
        AllBlock,
        /// Only findings in these registry classes block; the rest defer.
        Tiers(BTreeSet<&'static str>),
    }

    /// Where the effective policy came from.
    // trace:STORY-2913 | ai:claude
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Source {
        /// The default branch's config does not set the key.
        Unset,
        /// Set in the default branch's `.aida/config.toml`.
        DefaultBranch,
        /// The project has no default branch to read a list from (no git, or
        /// no main/master); default applies.
        NoDefaultBranch,
        /// The default branch carries a config that could not be read; every
        /// finding blocks, explicit deferrals included.
        Unavailable,
        /// Set but refused (unknown class, wrong type, empty); every finding
        /// blocks, explicit deferrals included.
        Invalid(String),
    }

    /// The effective policy with its provenance.
    // trace:STORY-2913 | ai:claude
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Effective {
        pub policy: Policy,
        pub source: Source,
    }

    impl Effective {
        /// Whether a list may exist but could not be honoured: an invalid or
        /// unreadable default-branch list. Such a policy fails closed past
        /// the default: no finding defers, not even one the reviewer marked
        /// `deferred`, so it never yields fewer blockers than any valid list.
        // trace:STORY-2913 | ai:claude
        pub(crate) fn fails_closed(&self) -> bool {
            matches!(self.source, Source::Invalid(_) | Source::Unavailable)
        }
    }

    /// Strictly parse a `blocking_tiers` value: a non-empty array of registry
    /// class names. Anything else refuses.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn parse_value(value: &toml::Value) -> Result<BTreeSet<&'static str>> {
        let items = value
            .as_array()
            .context("review.blocking_tiers must be an array of class names")?;
        let names: Vec<String> = items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .context("review.blocking_tiers entries must be strings")
            })
            .collect::<Result<_>>()?;
        parse_names(&names)
    }

    /// Strictly parse class names (the `set` command's input).
    // trace:STORY-2913 | ai:claude
    pub(crate) fn parse_names(names: &[String]) -> Result<BTreeSet<&'static str>> {
        let mut out = BTreeSet::new();
        for raw in names {
            match canonical(raw) {
                Some(c) => {
                    out.insert(c);
                }
                None => bail!(
                    "unknown blocking class `{}` (known: {})",
                    raw.trim(),
                    registry_names().join(", ")
                ),
            }
        }
        if out.is_empty() {
            bail!("review.blocking_tiers must name at least one class");
        }
        Ok(out)
    }

    pub(crate) fn registry_names() -> Vec<&'static str> {
        CLASSES.iter().map(|(n, _)| *n).collect()
    }

    /// Resolve the policy from a config body. A body that is not TOML, or a
    /// key that does not parse, is `Invalid` and resolves to `AllBlock`.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn from_config_text(text: &str) -> Effective {
        let invalid = |why: String| Effective {
            policy: Policy::AllBlock,
            source: Source::Invalid(why),
        };
        let config: toml::Value = match toml::from_str(text) {
            Ok(v) => v,
            Err(e) => return invalid(format!("config does not parse: {e}")),
        };
        let Some(value) = config.get("review").and_then(|r| r.get("blocking_tiers")) else {
            return Effective {
                policy: Policy::AllBlock,
                source: Source::Unset,
            };
        };
        match parse_value(value) {
            Ok(set) => Effective {
                policy: Policy::Tiers(set),
                source: Source::DefaultBranch,
            },
            Err(e) => invalid(format!("{e:#}")),
        }
    }

    /// The effective policy for `root`, from the trusted default-branch copy.
    /// Never fails: anything but a valid list on the default branch is the
    /// all-block default.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn effective(root: &Path) -> Effective {
        let all_block = |source| Effective {
            policy: Policy::AllBlock,
            source,
        };
        let Some(sha) = crate::trusted_config::local_trusted_default_branch_sha(root) else {
            return all_block(Source::NoDefaultBranch);
        };
        match crate::trusted_config::read_config_at_sha(root, &sha) {
            Some(text) => from_config_text(&text),
            None => match crate::trusted_config::config_present_at_sha(root, &sha) {
                Some(false) => all_block(Source::Unset),
                _ => all_block(Source::Unavailable),
            },
        }
    }

    /// [`effective`], with the fail-closed warning printed once.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn effective_warned(root: &Path) -> Effective {
        let effective = effective(root);
        warn_if_invalid(&effective);
        effective
    }

    fn warn_if_invalid(effective: &Effective) {
        match &effective.source {
            Source::Invalid(why) => eprintln!(
                "warning: [review] blocking_tiers is invalid on the default branch ({why}); every finding blocks"
            ),
            Source::Unavailable => eprintln!(
                "warning: the default branch's .aida/config.toml could not be read; every finding blocks"
            ),
            _ => {}
        }
    }

    /// Who may change `[review] blocking_tiers`: it decides what counts as a
    /// defect, so only the operator or an advisor acting under a mandate that
    /// covers `review-policy`.
    // trace:STORY-2913 | ai:claude
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum ChangeAuthority {
        /// A human at an interactive terminal, outside any agent session.
        Operator,
        /// The advisor seat under an active `review-policy` mandate.
        Mandate { id: String, principal: String },
    }

    impl ChangeAuthority {
        pub(crate) fn label(&self) -> String {
            match self {
                Self::Operator => "operator".to_string(),
                Self::Mandate { id, .. } => format!("mandate {id}"),
            }
        }
    }

    /// The pure authority decision. `grant` is the session's VALID seat grant
    /// as `(seat, principal)`; every other case refuses.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn decide_change_authority(
        direct_human: bool,
        grant: Option<(&str, &str)>,
        state: &crate::gate::mandate::MandateState,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ChangeAuthority> {
        use crate::gate::mandate::{MandateState, Scope, MANDATE_SEAT};
        if direct_human {
            return Ok(ChangeAuthority::Operator);
        }
        let refuse = |why: String| -> Result<ChangeAuthority> {
            bail!(
                "changing [review] blocking_tiers needs operator or mandate authority: {why}. \
                 Run it at a human terminal, or from the advisor seat under a mandate started with \
                 `aida mandate start --scope review-policy`"
            )
        };
        let mandate = match state {
            MandateState::Active(m) => m,
            MandateState::None => return refuse("no human terminal and no mandate".into()),
            MandateState::Expired(m) => return refuse(format!("mandate {} expired", m.id)),
            MandateState::Stopped(m) => return refuse(format!("mandate {} was stopped", m.id)),
        };
        if !mandate.is_active_at(now) {
            return refuse(format!("mandate {} is not active", mandate.id));
        }
        if !mandate.covers(Scope::ReviewPolicy) {
            return refuse(format!(
                "active mandate {} does not cover `review-policy`",
                mandate.id
            ));
        }
        match grant {
            Some((seat, principal))
                if aida_core::team::canonical_role(seat) == MANDATE_SEAT
                    && principal.trim() == mandate.principal =>
            {
                Ok(ChangeAuthority::Mandate {
                    id: mandate.id.clone(),
                    principal: mandate.principal.clone(),
                })
            }
            _ => refuse(format!(
                "this session holds no valid advisor seat grant from {}, the mandate's principal",
                mandate.principal
            )),
        }
    }

    /// Resolve the calling session's authority to change the list.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn resolve_change_authority(root: &Path) -> Result<ChangeAuthority> {
        let direct = crate::gate::seat_authority::require_direct_human().is_ok();
        let grant = crate::gate::seat_authority::current_grant(root);
        decide_change_authority(
            direct,
            grant
                .as_ref()
                .map(|g| (g.seat.as_str(), g.principal.as_str())),
            &crate::gate::mandate::current_state(),
            chrono::Utc::now(),
        )
    }

    /// Whether a blocking-tier list is in force (findings may defer to
    /// linked follow-ups). `Result` is kept for callers; it never fails.
    pub(crate) fn enabled(root: &Path) -> Result<bool> {
        Ok(matches!(effective(root).policy, Policy::Tiers(_)))
    }

    /// Use the existing structured finding format, retaining the finding evidence.
    /// Explicit classification is supplied by the reviewer, never guessed from prose.
    pub(crate) fn apply(root: &Path, findings: &[String]) -> Result<Vec<String>> {
        Ok(apply_policy(&effective(root), findings, &[]))
    }

    /// Registry class of a structured finding's `class` member, or `None`
    /// when it names none (vocabulary classes such as `perf` included).
    // trace:TASK-3814 | ai:claude
    // trace:STORY-2913 | ai:claude
    fn class_of(detail: &serde_json::Value) -> Option<&'static str> {
        canonical(detail.get("class")?.as_str()?)
    }

    /// Whether a path names gate, sandbox, authority, seat or containment code.
    /// Documentation never counts: its untested paths are ordinary follow-ups.
    // trace:TASK-3814 | ai:claude
    pub(crate) fn sensitive_path(path: &str) -> bool {
        let lower = path.trim().to_ascii_lowercase();
        if lower.is_empty()
            || [".md", ".txt", ".rst", ".adoc"]
                .iter()
                .any(|ext| lower.ends_with(ext))
        {
            return false;
        }
        lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| {
            matches!(
                t,
                "gate"
                    | "gates"
                    | "sandbox"
                    | "sandboxes"
                    | "contain"
                    | "contained"
                    | "containment"
                    | "authority"
                    | "mandate"
                    | "seat"
                    | "seats"
            )
        })
    }

    /// Whether a structured finding's own `file`/`path` member places it in
    /// sensitive code. Scope comes from the reviewer's evidence, never prose.
    // trace:TASK-3814 | ai:claude
    fn in_sensitive_code(detail: &serde_json::Value) -> bool {
        ["file", "path"].iter().any(|key| {
            detail
                .get(*key)
                .and_then(|v| v.as_str())
                .is_some_and(sensitive_path)
        })
    }

    /// How strongly a class blocks under `set`: security/authority bypass
    /// (unconditional) over test gaps (ADR-76, path-scoped) over a class the
    /// list names over anything else. `None` is no class at all.
    // trace:STORY-2913 | ai:claude
    fn rank(class: Option<&str>, set: &BTreeSet<&'static str>) -> i8 {
        match class.and_then(canonical) {
            None if class.is_none() => -1,
            Some("security-authority-bypass") => 3,
            Some("test-gap") => 2,
            Some(c) if set.contains(c) => 1,
            _ => 0,
        }
    }

    /// Fold positional `--finding-class` values into each structured finding's
    /// `class` member before any disposition is decided, so both spellings of
    /// a classification reach the same policy. The kept class is the one that
    /// actually blocks this finding under `set` (its tier and `file` scope
    /// included), so the merged finding blocks whenever either channel's
    /// class would: neither channel can demote the other to a deferral. Only
    /// when both or neither block does the static [`rank`] decide.
    // trace:TASK-3814 | ai:claude
    // trace:STORY-2913 | ai:claude
    fn merge_positional_classes(
        findings: &[String],
        raw_classes: &[String],
        set: &BTreeSet<&'static str>,
    ) -> Vec<String> {
        findings
            .iter()
            .enumerate()
            .map(|(i, raw)| {
                let Some(class) = raw_classes.get(i).and_then(|c| {
                    if canonical(c).is_some() {
                        return Some(normalize(c));
                    }
                    match super::review_classification::parse_class(c) {
                        super::review_classification::ClassArg::Class(c) => Some(c.to_string()),
                        _ => None,
                    }
                }) else {
                    return raw.clone();
                };
                let Ok(mut detail) = serde_json::from_str::<serde_json::Value>(raw) else {
                    return raw.clone();
                };
                if !detail.is_object() {
                    return raw.clone();
                }
                let existing = detail.get("class").and_then(|v| v.as_str());
                let (new_rank, old_rank) = (rank(Some(&class), set), rank(existing, set));
                let mut merged = detail.clone();
                merged["class"] = class.into();
                let keep_old = match (blocks(&detail, set), blocks(&merged, set)) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => old_rank > new_rank || (old_rank == new_rank && old_rank > 0),
                };
                if keep_old {
                    return raw.clone();
                }
                merged.to_string()
            })
            .collect()
    }

    /// Whether one structured finding blocks under `set`. Security and
    /// authority bypass block everywhere; test gaps block in gate, sandbox,
    /// authority, seat or containment code (ADR-76); a finding without a
    /// registry `blocking_tier` blocks; otherwise it blocks when its tier or
    /// its class is on the list.
    // trace:STORY-2913 | ai:claude
    fn blocks(detail: &serde_json::Value, set: &BTreeSet<&'static str>) -> bool {
        let tier_raw = detail.get("blocking_tier").and_then(|v| v.as_str());
        let tier = tier_raw.and_then(canonical);
        let class = class_of(detail);
        let either = |c: &str| tier == Some(c) || class == Some(c);
        if either("security-authority-bypass")
            || tier_raw.is_some_and(|t| ADR76_TIER_SPELLINGS.contains(&normalize(t).as_str()))
            || (either("test-gap") && in_sensitive_code(detail))
        {
            return true;
        }
        let Some(tier) = tier else {
            return true;
        };
        set.contains(tier) || class.is_some_and(|c| set.contains(c))
    }

    /// [`apply`] with the positional `--finding-class` channel folded in first.
    // trace:TASK-3814 | ai:claude
    #[cfg(test)]
    pub(crate) fn apply_with_classes(
        root: &Path,
        findings: &[String],
        raw_classes: &[String],
    ) -> Result<Vec<String>> {
        let effective = effective(root);
        warn_if_invalid(&effective);
        Ok(apply_policy(&effective, findings, raw_classes))
    }

    /// Stamp each structured finding's disposition under `effective`. With the
    /// list unset the findings pass unchanged (today's behaviour, explicit
    /// reviewer deferrals included). A list that fails closed stamps every
    /// structured finding `blocking`, overriding any reviewer deferral.
    // trace:STORY-2913 | ai:claude
    pub(crate) fn apply_policy(
        effective: &Effective,
        findings: &[String],
        raw_classes: &[String],
    ) -> Vec<String> {
        let Policy::Tiers(set) = &effective.policy else {
            if !effective.fails_closed() {
                return findings.to_vec();
            }
            return findings
                .iter()
                .map(|raw| match serde_json::from_str::<serde_json::Value>(raw) {
                    Ok(mut detail) if detail.is_object() => {
                        detail["disposition"] = "blocking".into();
                        detail.to_string()
                    }
                    _ => raw.clone(),
                })
                .collect();
        };
        merge_positional_classes(findings, raw_classes, set)
            .iter()
            .map(|raw| {
                let Ok(mut detail) = serde_json::from_str::<serde_json::Value>(raw) else {
                    return raw.clone();
                };
                if !detail
                    .get("text")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    return raw.clone();
                }
                let deferred = !blocks(&detail, set);
                detail["disposition"] = if deferred { "deferred" } else { "blocking" }.into();
                if deferred {
                    if !detail
                        .get("rationale")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.trim().is_empty() && !s.contains(['\n', '\r']))
                    {
                        detail["rationale"] =
                            "Project policy routes below-tier findings to linked follow-ups".into();
                    }
                    if !detail
                        .get("severity")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.trim().is_empty())
                    {
                        detail["severity"] = "minor".into();
                    }
                }
                detail.to_string()
            })
            .collect()
    }
}
