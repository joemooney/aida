//! Host-owned fast-gate receipts and the single repair round.
// trace:STORY-2640 | ai:codex
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Receipt {
    branch: String,
    head: Option<String>,
    marker: String,
    pending: bool,
    repair_used: bool,
    #[serde(default)]
    native_ready: bool,
    #[serde(default)]
    repair_queue: Option<aida_core::QueueEntry>,
    #[serde(default)]
    owner: Option<aida_core::proc_identity::ProcIdentity>,
    #[serde(default)]
    reserved_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn path(main: &Path, branch: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    main.join(".aida/fast-gates")
        .join(format!("{:x}.json", Sha256::digest(branch.as_bytes())))
}
fn read(main: &Path, branch: &str) -> Result<Option<Receipt>> {
    match std::fs::read(path(main, branch)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn write(main: &Path, receipt: &Receipt) -> Result<()> {
    let path = path(main, &receipt.branch);
    std::fs::create_dir_all(path.parent().context("fast gate directory")?)?;
    aida_core::write_atomic(&path, serde_json::to_vec(receipt)?)?;
    Ok(())
}

// Serialize reservation replacement with stale-owner cleanup. The sidecar is
// permanent: replacing the receipt must never replace the lock inode.
// trace:STORY-2640 | ai:codex
fn receipt_lock(main: &Path, branch: &str) -> Result<std::fs::File> {
    use fs2::FileExt;
    let lock = path(main, branch).with_extension("lock");
    std::fs::create_dir_all(lock.parent().context("fast gate lock directory")?)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock)?;
    file.lock_exclusive()?;
    Ok(file)
}

/// Forget settled receipts after their local and origin branches disappear.
/// Keep lock sidecars permanently: an existing waiter may still hold their inode.
/// Unknown Git evidence and pending repairs retain their receipt.
// trace:STORY-2640 | ai:codex
fn settled_owner_ended(receipt: &Receipt) -> bool {
    !receipt.pending
        && receipt.owner.as_ref().is_none_or(|owner| {
            matches!(
                aida_core::proc_identity::observe(owner, true),
                aida_core::proc_identity::Verdict::Dead(_)
            )
        })
}

fn prune_settled(main: &Path) -> Result<()> {
    let directory = main.join(".aida/fast-gates");
    if !directory.is_dir() {
        return Ok(());
    }
    let refs = std::process::Command::new("git")
        .arg("-C")
        .arg(main)
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ])
        .output()?;
    anyhow::ensure!(
        refs.status.success(),
        "fast gate branch inventory unavailable"
    );
    let refs = String::from_utf8(refs.stdout)?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let candidate: Receipt = serde_json::from_slice(&std::fs::read(entry.path())?)?;
        if !settled_owner_ended(&candidate) {
            continue;
        }
        let local = format!("refs/heads/{}", candidate.branch);
        let origin = format!("refs/remotes/origin/{}", candidate.branch);
        if refs.lines().any(|r| r == local || r == origin) {
            continue;
        }
        let _lock = receipt_lock(main, &candidate.branch)?;
        // A concurrent pickup may have replaced the settled record while we
        // waited. It must retain both its reservation and its repair credit.
        if let Some(current) = read(main, &candidate.branch)? {
            if settled_owner_ended(&current) && path(main, &current.branch) == entry.path() {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Reserve before the vendor can push. Existing retry credit survives relaunches.
#[cfg(test)]
pub(crate) fn begin(main: &Path, branch: &str) -> Result<()> {
    let _lock = receipt_lock(main, branch)?;
    begin_locked(main, branch)
}

fn begin_locked(main: &Path, branch: &str) -> Result<()> {
    let mut receipt = read(main, branch)?.unwrap_or(Receipt {
        branch: branch.into(),
        head: None,
        marker: "skipped(pending)".into(),
        pending: true,
        repair_used: false,
        native_ready: false,
        repair_queue: None,
        owner: None,
        reserved_at: None,
    });
    if !receipt.pending {
        receipt.repair_used = false;
    }
    receipt.pending = true;
    receipt.owner = aida_core::proc_identity::ProcIdentity::capture(std::process::id()).ok();
    receipt.reserved_at = Some(chrono::Utc::now());
    receipt.native_ready = false;
    receipt.head = None;
    receipt.repair_queue = None;
    receipt.marker = "skipped(pending)".into();
    write(main, &receipt)
}

/// Settle abandoned reservations on every exit, including a missing PR lookup.
/// A queued repair has a bound head and stays pending until its one retry.
// trace:STORY-2640 | ai:codex
pub(crate) struct Reservation {
    main: PathBuf,
    branch: String,
    report_result: bool,
    reserved_at: Option<chrono::DateTime<chrono::Utc>>,
}
impl Reservation {
    /// Bind post-exit PR discovery and review admission to the actual branch,
    /// just as the drain reconciles HEAD after an implementer returns.
    // trace:STORY-2640 | ai:codex
    pub(crate) fn reconcile_branch(&mut self, tree: &Path) -> Result<String> {
        let branch = if crate::seat_clone::host_recorded_clone(&self.main, tree).is_some() {
            match crate::seat_clone::probe(&self.main, tree) {
                crate::seat_clone::Probe::Known(facts) => {
                    facts.branch.context("fast gate seat is detached")?
                }
                _ => anyhow::bail!("cannot resolve fast gate seat branch"),
            }
        } else {
            aida_core::git_ops::attached_branch(tree)?
        };
        self.rebind(&branch)?;
        Ok(branch)
    }

    /// Exit reporting and lease cleanup survive unavailable branch evidence.
    // trace:STORY-2640 | ai:codex
    pub(crate) fn exit_branch(&mut self, tree: &Path) -> String {
        match self.reconcile_branch(tree) {
            Ok(branch) => branch,
            Err(error) => {
                eprintln!("fast_gate=skipped(branch): {error:#}");
                if let Ok(Some(mut r)) = read(&self.main, &self.branch) {
                    if r.marker != "fail" {
                        r.marker = "skipped(branch)".into();
                    }
                    if let Err(error) = write(&self.main, &r) {
                        eprintln!("fast gate branch marker: {error:#}");
                    }
                }
                self.branch.clone()
            }
        }
    }

    fn rebind(&mut self, branch: &str) -> Result<()> {
        if branch == self.branch {
            return Ok(());
        }
        // All migrations lock names in the same order. Keep the new branch
        // reserved before settling the old one, and preserve the retry budget.
        let (first, second) = if branch < self.branch.as_str() {
            (branch, self.branch.as_str())
        } else {
            (self.branch.as_str(), branch)
        };
        let _first = receipt_lock(&self.main, first)?;
        let _second = receipt_lock(&self.main, second)?;
        let mut source =
            read(&self.main, &self.branch)?.context("fast gate reservation disappeared")?;
        anyhow::ensure!(
            source.reserved_at == self.reserved_at,
            "fast gate reservation was replaced"
        );
        if let Some(destination) = read(&self.main, branch)? {
            anyhow::ensure!(
                !destination.pending,
                "actual branch already has a pending fast gate"
            );
            source.repair_used |= destination.repair_used;
        }
        source.branch = branch.into();
        write(&self.main, &source)?;
        release_locked(&self.main, &self.branch)?;
        self.branch = branch.into();
        Ok(())
    }

    pub(crate) fn with_result_line(mut self) -> Self {
        self.report_result = true;
        self
    }
}
pub(crate) fn reserve(main: &Path, branch: &str) -> Result<Reservation> {
    if let Err(error) = prune_settled(main) {
        eprintln!("fast gate receipt cleanup deferred: {error:#}");
    }
    let _lock = receipt_lock(main, branch)?;
    begin_locked(main, branch)?;
    Ok(Reservation {
        main: main.into(),
        branch: branch.into(),
        report_result: false,
        reserved_at: read(main, branch)?.and_then(|r| r.reserved_at),
    })
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let result = (|| -> Result<()> {
            let _lock = receipt_lock(&self.main, &self.branch)?;
            if let Some(mut r) = read(&self.main, &self.branch)? {
                if r.reserved_at == self.reserved_at
                    && r.owner
                        == aida_core::proc_identity::ProcIdentity::capture(std::process::id()).ok()
                    && r.repair_queue.is_none()
                {
                    r.pending = false;
                    r.native_ready = false;
                    r.head = None;
                    if r.marker == "skipped(pending)" {
                        r.marker = "skipped(lane-ended)".into();
                    }
                    write(&self.main, &r)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("fast gate reservation cleanup: {e:#}");
        }
        if self.report_result {
            println!("{}", result_line(&self.main, &self.branch, "implementer:"));
        }
    }
}

/// Infrastructure trouble is visible but must not bypass lane reporting/cleanup.
// trace:STORY-2640 | ai:codex
pub(crate) fn infrastructure(main: &Path, branch: &str, _lease: &str, error: &anyhow::Error) {
    let result = (|| -> Result<()> {
        let marker = read(main, branch)?
            .map(|r| {
                if r.marker == "pass" || r.marker == "fail" {
                    r.marker
                } else if r.repair_used {
                    "fail".into()
                } else {
                    "skipped(infrastructure)".into()
                }
            })
            .unwrap_or_else(|| "skipped(infrastructure)".into());
        eprintln!("fast_gate={marker}: {error:#}");
        if let Some(mut r) = read(main, branch)? {
            r.marker = marker;
            write(main, &r)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("could not persist fast gate infrastructure marker: {e:#}");
    }
}

pub(crate) fn result_line(main: &Path, branch: &str, line: &str) -> String {
    let marker = marker(main, branch);
    let marker = if marker == "skipped(pending)" {
        "skipped(lane-ended)"
    } else {
        &marker
    };
    format!("{line} fast_gate={marker}")
}

/// An absent receipt is an ordinary non-lane PR. Corrupt state fails closed.
pub(crate) fn blocks_review(main: &Path, branch: &str) -> Result<bool> {
    if read(main, branch)?.is_none() {
        return Ok(false);
    }
    let _lock = receipt_lock(main, branch)?;
    let Some(r) = read(main, branch)? else {
        return Ok(false);
    };
    if !r.pending {
        return Ok(false);
    }
    // A bounded backstop also covers legacy receipts and unobservable owners.
    // trace:STORY-2640 | ai:codex
    let reserved_at = r.reserved_at.or_else(|| {
        std::fs::metadata(path(main, branch))
            .and_then(|m| m.modified())
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from)
    });
    if reserved_at.is_some_and(|at| chrono::Utc::now() - at >= chrono::Duration::hours(24)) {
        release_locked(main, branch)?;
        return Ok(false);
    }
    if let Some(entry) = &r.repair_queue {
        let storage = crate::Storage::new(
            crate::detect_distributed_store_from(main)
                .context("fast gate rework store unavailable")?,
        );
        return repair_queue_live(&storage, entry);
    }
    if r.owner.as_ref().is_some_and(|owner| {
        matches!(
            aida_core::proc_identity::observe(owner, true),
            aida_core::proc_identity::Verdict::Dead(_)
        )
    }) {
        release_locked(main, branch)?;
        return Ok(false);
    }
    Ok(true)
}

/// Operator recovery and abandoned-owner cleanup release admission, never grant
/// publication authority or erase a known gate verdict.
// trace:STORY-2640 | ai:codex
pub(crate) fn release(main: &Path, branch: &str) -> Result<()> {
    let _lock = receipt_lock(main, branch)?;
    release_locked(main, branch)
}

// trace:STORY-2640 | ai:codex
pub(crate) fn operator_release(main: &Path, branch: &str, lane_context: bool) -> Result<()> {
    anyhow::ensure!(
        !lane_context,
        "fast-gate release requires an operator outside a seat or lane"
    );
    let _lock = receipt_lock(main, branch)?;
    anyhow::ensure!(
        read(main, branch)?.is_some(),
        "no fast-gate receipt for branch"
    );
    release_locked(main, branch)
}

pub(crate) fn missing_lease(main: &Path, branch: Option<&str>) {
    if let Some(branch) = branch {
        let result = (|| -> Result<()> {
            let _lock = receipt_lock(main, branch)?;
            if let Some(mut r) = read(main, branch)? {
                if r.marker != "fail" {
                    r.marker = "skipped(missing-lease)".into();
                }
                write(main, &r)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("fast gate missing-lease marker: {error:#}");
        }
    }
    let marker = branch
        .map(|branch| marker(main, branch))
        .unwrap_or_else(|| "skipped(missing-lease)".into());
    eprintln!("fast_gate={marker}");
}

fn release_locked(main: &Path, branch: &str) -> Result<()> {
    if let Some(mut r) = read(main, branch)? {
        r.pending = false;
        r.native_ready = false;
        if r.marker == "skipped(pending)" {
            r.marker = "skipped(lane-ended)".into();
        }
        write(main, &r)?;
    }
    Ok(())
}

/// Publication errors remain errors, with their gate evidence intact.
// trace:STORY-2640 | ai:codex
pub(crate) fn publication_result<T>(main: &Path, branch: &str, result: Result<T>) -> Result<T> {
    if result.is_err() {
        if let Err(error) = release(main, branch) {
            eprintln!("fast gate publication cleanup: {error:#}");
        }
        eprintln!("{}", result_line(main, branch, "publication failed:"));
    }
    result
}

// Only the exact queued repair owns this review hold. A removed/replaced row,
// terminal spec or parked spec releases it; unreadable evidence still refuses.
// trace:STORY-2640 | ai:codex
fn repair_queue_live(storage: &crate::Storage, entry: &aida_core::QueueEntry) -> Result<bool> {
    let store = storage.load()?;
    let Some(req) = store
        .requirements
        .iter()
        .find(|r| r.id == entry.requirement_id)
    else {
        return Ok(false);
    };
    if req.archived
        || req.deferred
        || !matches!(
            req.status,
            aida_core::RequirementStatus::Approved
                | aida_core::RequirementStatus::Planned
                | aida_core::RequirementStatus::InProgress
        )
    {
        return Ok(false);
    }
    Ok(storage
        .queue_list(&entry.user_id, true)?
        .iter()
        .any(|live| {
            live.requirement_id == entry.requirement_id
                && live.added_at == entry.added_at
                && live.added_by == entry.added_by
                && live.for_role == entry.for_role
        }))
}

pub(crate) fn marker(main: &Path, branch: &str) -> String {
    read(main, branch)
        .ok()
        .flatten()
        .map(|r| r.marker)
        .unwrap_or_else(|| "skipped(no-result)".into())
}

pub(crate) fn veto(main: &Path, spec: &str) -> bool {
    crate::punt::read_signal(&crate::punt::signal_path(main, spec)).is_some()
        || crate::punt::read_hold_signal(&crate::punt::hold_signal_path(main, spec)).is_some()
}

#[derive(Debug)]
pub(crate) struct NativeCommitError(String);
impl std::fmt::Display for NativeCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host post-exit commit failed: {}", self.0)
    }
}
impl std::error::Error for NativeCommitError {}

fn commit(main: &Path, tree: &Path, spec: &str, lease: &str, clean: bool) -> Result<bool> {
    match crate::ship_from_seat::post_exit_commit_reported(main, tree, spec, lease, clean) {
        crate::ship_from_seat::PostExitCommit::Failed(why) => Err(NativeCommitError(why).into()),
        crate::ship_from_seat::PostExitCommit::NotASeat => Ok(false),
        _ => Ok(true),
    }
}

/// Probe clone Git only through its existing sandbox. Linked worktrees use
/// the ordinary sanitized host reader, which refuses private clones.
fn snapshot(main: &Path, tree: &Path, branch: &str) -> Result<String> {
    if crate::seat_clone::host_recorded_clone(main, tree).is_some() {
        match crate::seat_clone::probe(main, tree) {
            crate::seat_clone::Probe::Known(facts) => {
                anyhow::ensure!(
                    facts.dirty.is_empty(),
                    "fast gate needs a clean committed seat"
                );
                facts.head.context("fast gate seat has no head")
            }
            _ => anyhow::bail!("cannot bind fast gate to the seat head"),
        }
    } else {
        let state = crate::headless_background::SeatState::read(tree, branch);
        anyhow::ensure!(
            state.uncommitted == Some(0),
            "fast gate needs a clean committed seat"
        );
        crate::headless_background::seat_head(tree).context("fast gate seat head unavailable")
    }
}

fn attempt(
    main: &Path,
    tree: &Path,
    lease: &str,
    branch: &str,
) -> Result<(String, String, String)> {
    // A skipped check still needs the committed head for native publication.
    // trace:BUG-2512 | ai:codex
    let head = snapshot(main, tree, branch)?;
    let Some(command) = crate::project_checks::load(main)?.fast_check else {
        let reason = "No trusted [validation].fast_check command configured";
        eprintln!("fast_gate=skipped(unconfigured): {reason}");
        return Ok((head, "skipped(unconfigured)".into(), reason.into()));
    };
    if crate::session::bwrap_availability() != crate::session::BwrapAvailability::Ok {
        eprintln!("fast_gate=skipped(no-sandbox)");
        return Ok((String::new(), "skipped(no-sandbox)".into(), String::new()));
    }
    let (marker, output) = gate_result(crate::lane_job::run_fast_gate(main, tree, lease, command)?);
    anyhow::ensure!(
        snapshot(main, tree, branch)? == head,
        "seat changed during fast gate; refusing stale evidence"
    );
    eprintln!("fast_gate={marker} head={head}\n{output}");
    Ok((head, marker, output))
}

// A timeout gives no source-failure evidence and must not consume a repair.
pub(crate) fn gate_result(result: Option<(i32, String)>) -> (String, String) {
    match result {
        None => ("skipped(no-sandbox)".into(), String::new()),
        Some((0, output)) => ("pass".into(), output),
        Some((crate::lane_job::TIMEOUT_RC, output)) => ("skipped(timeout)".into(), output),
        Some((crate::lane_job::REFUSED_RC, output)) => ("skipped(refused)".into(), output),
        Some((crate::lane_job::STOPPED_RC, output)) => ("skipped(stopped)".into(), output),
        Some((_, output)) => ("fail".into(), output),
    }
}

/// Host-published lanes commit locally, gate, resume once on red, and gate
/// the repaired commit before any publication. Signals remain publication vetoes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn native(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    clean: bool,
    mut repair: impl FnMut(&str) -> Result<bool>,
) -> Result<bool> {
    match native_inner(main, tree, spec, lease, branch, clean, &mut repair) {
        Ok(ready) => Ok(ready),
        Err(error) if error.is::<NativeCommitError>() => {
            publication_result(main, branch, Err(error))
        }
        Err(error) => native_error(main, branch, lease, &error, || snapshot(main, tree, branch)),
    }
}

// Only a clean, inspectable committed seat can still be published. Keep known
// red evidence when repair startup or a subsequent head check fails.
// trace:STORY-2640 | ai:codex
fn native_error(
    main: &Path,
    branch: &str,
    lease: &str,
    error: &anyhow::Error,
    snapshot: impl FnOnce() -> Result<String>,
) -> Result<bool> {
    infrastructure(main, branch, lease, error);
    let ready = (|| -> Result<()> {
        let head = snapshot()?;
        let mut r = read(main, branch)?.context("fast gate receipt missing")?;
        r.head = Some(head);
        r.native_ready = true;
        write(main, &r)
    })();
    publication_result(
        main,
        branch,
        ready.context("native publication cannot recover a clean committed seat"),
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn native_inner(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    clean: bool,
    mut repair: impl FnMut(&str) -> Result<bool>,
) -> Result<bool> {
    if veto(main, spec) {
        return Ok(true);
    }
    if crate::session::bwrap_availability() != crate::session::BwrapAvailability::Ok {
        skip_native_without_sandbox(main, branch, lease)?;
        return Ok(false);
    }
    let clean_exit = std::cell::Cell::new(clean);
    let publishable = std::cell::Cell::new(true);
    native_rounds(
        main,
        branch,
        || {
            if veto(main, spec) {
                return Ok(None);
            }
            if !commit(main, tree, spec, lease, clean_exit.get())? {
                publishable.set(false);
                return Ok(None);
            }
            attempt(main, tree, lease, branch).map(Some)
        },
        |prompt| {
            clean_exit.set(repair(prompt)?);
            Ok(())
        },
    )?;
    Ok(publishable.get())
}

// Native seat Git itself requires bubblewrap: no commit or publication can
// safely run here. Settle rather than reserving review forever.
fn skip_native_without_sandbox(main: &Path, branch: &str, _lease: &str) -> Result<()> {
    let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
    receipt.marker = "skipped(no-sandbox)".into();
    receipt.pending = false;
    receipt.head = None;
    receipt.native_ready = false;
    write(main, &receipt)?;
    eprintln!(
        "fast_gate=skipped(no-sandbox); native commit/publication still requires its sandbox"
    );
    Ok(())
}

// The native publication driver, with only the sandbox/commit effects injected.
// The durable reservation and one-round limit are identical in production/tests.
pub(crate) fn native_rounds(
    main: &Path,
    branch: &str,
    mut attempt: impl FnMut() -> Result<Option<(String, String, String)>>,
    mut repair: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
    let Some((mut head, mut result, output)) = attempt()? else {
        return Ok(());
    };
    if receipt.repair_used {
        result = retry_marker("fail", &result).into();
    }
    if result == "fail" && !receipt.repair_used {
        receipt.repair_used = true;
        receipt.marker = result.clone();
        write(main, &receipt)?;
        repair(&repair_prompt(&output))?;
        let Some((next_head, next_result, _)) = attempt()? else {
            return Ok(());
        };
        head = next_head;
        result = retry_marker(&result, &next_result).into();
    }
    receipt.head = Some(head);
    receipt.marker = result;
    receipt.pending = true;
    receipt.native_ready = true;
    write(main, &receipt)
}

// A retry without a verdict cannot erase the first observed source failure.
// trace:STORY-2640 | ai:codex
fn retry_marker<'a>(previous: &'a str, next: &'a str) -> &'a str {
    if previous == "fail" && next != "pass" && next != "fail" {
        previous
    } else {
        next
    }
}

fn repair_prompt(output: &str) -> String {
    // Bound the entire failure excerpt well below Linux's single-argv limit.
    let mut start = output.len().saturating_sub(16 * 1024);
    while !output.is_char_boundary(start) {
        start += 1;
    }
    let output = &output[start..];
    format!("The host's sandboxed configured fast-check command failed. This is your ONE fast-gate fix round. Fix the failing gates below and finish. Native seats must leave edits uncommitted for host publication; retain all punt/hold instructions.\n{output}")
}

/// A self-published lane queues its one repair before review admission. The
/// retry receipt is branch-scoped, so an unchanged or still-red retry terminates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pushed(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    pr: u64,
    vendor: &str,
) -> Result<bool> {
    match pushed_inner(main, tree, spec, lease, branch, pr, vendor) {
        Ok(queued) => Ok(queued),
        Err(error) => {
            infrastructure(main, branch, lease, &error);
            Ok(false)
        }
    }
}

// trace:STORY-2640 | ai:codex
fn pushed_head_attempt(
    local: &str,
    published: &str,
    run: impl FnOnce() -> Result<(String, String, String)>,
) -> Result<(String, String, String)> {
    if local != published {
        return Ok((
            published.into(),
            "skipped(unpushed-head)".into(),
            String::new(),
        ));
    }
    run()
}

#[allow(clippy::too_many_arguments)]
fn pushed_inner(
    main: &Path,
    tree: &Path,
    spec: &str,
    lease: &str,
    branch: &str,
    pr: u64,
    vendor: &str,
) -> Result<bool> {
    if veto(main, spec) || marker(main, branch) == "skipped(branch)" {
        return Ok(false);
    }
    let published_head = crate::standalone_implementer::lookup_pr(main, branch)
        .filter(|(id, _)| *id == pr)
        .and_then(|(_, head)| head)
        .context("cannot bind fast gate to the published PR head")?;
    let local_head = snapshot(main, tree, branch)?;
    let checked = pushed_head_attempt(&local_head, &published_head, || {
        attempt(main, tree, lease, branch)
    });
    let (mut head, result, output) = match checked {
        Ok(result) => result,
        Err(error) => {
            infrastructure(main, branch, lease, &error);
            (String::new(), marker(main, branch), String::new())
        }
    };
    if head.is_empty() {
        head = crate::standalone_implementer::lookup_pr(main, branch)
            .filter(|(id, _)| *id == pr)
            .and_then(|(_, head)| head)
            .context("cannot bind skipped fast gate to the published PR head")?;
    }
    let prior = read(main, branch)?.context("fast gate was not reserved")?;
    let result = if prior.repair_used {
        retry_marker("fail", &result).to_string()
    } else {
        result
    };
    pushed_round(
        main,
        branch,
        &head,
        &result,
        || annotate(main, pr, &head, &result),
        || {
            let storage = crate::Storage::new(
                crate::detect_distributed_store_from(main)
                    .context("fast gate rework store unavailable")?,
            );
            let store = storage.load()?;
            let req = store
                .get_requirement_unambiguous(spec)?
                .context("fast gate spec unavailable")?;
            let note = repair_prompt(&output);
            let vendor = crate::brief::validate_brief_agent(vendor)?;
            let dir = main.join(".aida/agent-briefs").join(vendor);
            std::fs::create_dir_all(&dir)?;
            let stamp = chrono::Utc::now()
                .format("%Y-%m-%dT%H%M%S%.9fZ")
                .to_string();
            let brief = dir.join(format!("{}-{stamp}.md", req.display_id())); // canonical-id: durable brief filename
            aida_core::write_atomic(
                &brief,
                crate::brief::render_agent_brief(
                    main,
                    vendor,
                    &req.display_id(), // canonical-id: persisted brief spec identity
                    req,
                    &store,
                    &stamp,
                    Some(&note),
                    None,
                    None,
                ),
            )?;
            crate::brief::add_pending_brief(main, vendor, &brief)?;
            let mut retryable = false;
            storage.update_atomically(|store| {
                if let Some(live) = store.requirements.iter_mut().find(|r| r.id == req.id) {
                    retryable = prepare_queued_repair(live);
                }
            })?;
            anyhow::ensure!(
                retryable,
                "fast gate spec disappeared or became terminal; no repair queued"
            );
            for user in storage.queue_users()? {
                if user != "role:implementer" {
                    storage.queue_remove_for_role(&user, &req.id, Some("implementer"))?;
                }
            }
            let entry = aida_core::QueueEntry {
                user_id: "role:implementer".into(),
                requirement_id: req.id,
                position: 0,
                added_by: "fast-gate".into(),
                note: Some(note),
                added_at: chrono::Utc::now(),
                for_role: Some("implementer".into()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            };
            storage.queue_add(entry.clone())?;
            Ok(Some(entry))
        },
    )
}

// A queued repair has no new worker yet. Match metadata-only queue rework:
// leave it Approved so drains can claim it, then let pickup set InProgress.
// Terminal decisions remain untouched.
// trace:STORY-2640 | ai:codex
fn prepare_queued_repair(req: &mut aida_core::Requirement) -> bool {
    use aida_core::RequirementStatus;
    if matches!(
        req.status,
        RequirementStatus::Completed | RequirementStatus::Rejected | RequirementStatus::Superseded
    ) {
        return false;
    }
    if let Some(target) = crate::queue_cmd::rework_target_for_mode(&req.status, false) {
        aida_core::conflict::set_status_recorded(req, target, "fast-gate");
    }
    true
}

// Keep review blocked through annotation and queue insertion; failed insertion
// settles the red receipt so infrastructure cannot wedge review forever.
fn pushed_round(
    main: &Path,
    branch: &str,
    head: &str,
    result: &str,
    annotate: impl FnOnce() -> Result<()>,
    queue_repair: impl FnOnce() -> Result<Option<aida_core::QueueEntry>>,
) -> Result<bool> {
    let mut receipt = read(main, branch)?.context("fast gate was not reserved")?;
    receipt.head = Some(head.into());
    receipt.marker = retry_marker(
        if receipt.repair_used {
            "fail"
        } else {
            &receipt.marker
        },
        result,
    )
    .into();
    let queue = result == "fail" && !receipt.repair_used;
    receipt.pending = true;
    write(main, &receipt)?;
    if let Err(error) = annotate() {
        if receipt.marker != "fail" && receipt.marker != "pass" {
            receipt.marker = "skipped(annotation)".into();
        }
        eprintln!("fast_gate={}: annotation failed: {error:#}", receipt.marker);
        if !queue {
            // An unpushed seat head may differ from the PR. A skipped gate
            // must not leave an obsolete head fence after the lane exits.
            receipt.head = None;
        }
        write(main, &receipt)?;
    }
    if queue {
        receipt.repair_queue = match queue_repair() {
            Ok(entry) => entry,
            Err(error) => {
                receipt.pending = false;
                write(main, &receipt)?;
                return Err(error);
            }
        };
        receipt.repair_used = true;
        write(main, &receipt)?;
    } else {
        receipt.pending = false;
        write(main, &receipt)?;
    }
    Ok(queue)
}

// trace:STORY-2640 | ai:codex
pub(crate) use crate::forge::annotate_fast_gate as annotate;

// trace:STORY-2640 | ai:codex
pub(crate) fn head(main: &Path, branch: &str) -> Option<String> {
    read(main, branch).ok().flatten().and_then(|r| r.head)
}

/// Publication must still name the commit which the sandbox checked.
// trace:STORY-2640 | ai:codex
pub(crate) fn publication_ready(main: &Path, tree: &Path, branch: &str) -> Result<()> {
    let r = read(main, branch)?.context("host publication has no fast-gate receipt")?;
    anyhow::ensure!(
        r.native_ready && r.head.as_deref() == Some(snapshot(main, tree, branch)?.as_str()),
        "host publication fast-gate evidence is pending or stale"
    );
    Ok(())
}

/// Annotation is informational: a forge flake cannot invalidate a checked
/// publication or strand its lease. Diagnostics record the annotation failure;
/// a known gate verdict remains intact.
// trace:STORY-2640 | ai:codex
pub(crate) fn finish_publication(
    main: &Path,
    branch: &str,
    annotate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if let Err(error) = annotate() {
        let mut r = read(main, branch)?.context("fast gate receipt disappeared")?;
        if r.marker != "fail" && r.marker != "pass" {
            r.marker = "skipped(annotation)".into();
        }
        eprintln!("fast_gate={}: annotation failed: {error:#}", r.marker);
        r.head = None;
        write(main, &r)?;
    }
    published(main, branch)
}

/// Release review admission only after the host published and annotated its PR.
// trace:STORY-2640 | ai:codex
pub(crate) fn published(main: &Path, branch: &str) -> Result<()> {
    let mut receipt = read(main, branch)?.context("fast gate receipt disappeared")?;
    receipt.pending = false;
    receipt.native_ready = false;
    write(main, &receipt)
}

// Reconciliation is a host-controlled merge after the gate. Preserve red
// evidence; otherwise explicitly report that the published merge was not gated.
// trace:STORY-2640 | ai:codex
pub(crate) fn reconciled_import(main: &Path, branch: &str, tip: &str) -> Result<()> {
    let _lock = receipt_lock(main, branch)?;
    let mut r = read(main, branch)?.context("host publication has no fast-gate receipt")?;
    anyhow::ensure!(
        r.native_ready,
        "host publication has no ready fast-gate receipt"
    );
    if r.head.as_deref() != Some(tip) {
        r.head = Some(tip.into());
        if r.marker != "fail" {
            r.marker = "skipped(reconciled)".into();
        }
        write(main, &r)?;
    }
    Ok(())
}

/// Enforce the checked tip after explicitly accounting for host reconciliation.
// trace:STORY-2640 | ai:codex
pub(crate) fn check_import(main: &Path, branch: &str, tip: &str, automatic: bool) -> Result<()> {
    if !automatic {
        return Ok(());
    }
    let r = read(main, branch)?.context("host publication has no fast-gate receipt")?;
    anyhow::ensure!(
        r.native_ready && r.head.as_deref() == Some(tip),
        "imported lane head differs from the host fast gate; rerun the lane before publication"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    // trace:STORY-2640 | ai:codex
    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_repair_prompt_bounds_multibyte_output_below_argv_limit() {
        let output = "é".repeat(1024 * 1024) + "trailing failure";
        let prompt = repair_prompt(&output);
        assert!(prompt.len() < 20 * 1024);
        assert!(prompt.ends_with("trailing failure"));
        assert!(prompt.contains("ONE fast-gate fix round"));
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_exit_branch_error_keeps_result_and_settles_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        let head = crate::test_git::commit_fixture(tree.path());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(tree.path())
            .args(["checkout", "--detach", &head])
            .status()
            .unwrap()
            .success());
        let mut reservation = reserve(tmp.path(), "lane").unwrap();
        assert_eq!(reservation.exit_branch(tree.path()), "lane");
        assert!(result_line(tmp.path(), "lane", "implementer: no-pr")
            .ends_with("fast_gate=skipped(branch)"));
        drop(reservation);
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
    }

    #[test]
    fn fast_gate_prunes_only_settled_receipts_for_deleted_branches() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-b", "main"])
            .status()
            .unwrap()
            .success());
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(tmp.path())
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial"
            ])
            .status()
            .unwrap()
            .success());
        for branch in ["main", "deleted", "repair", "remote", "reporting"] {
            begin(tmp.path(), branch).unwrap();
            if branch != "repair" {
                release(tmp.path(), branch).unwrap();
                if branch != "reporting" {
                    let mut receipt = read(tmp.path(), branch).unwrap().unwrap();
                    receipt.owner = None;
                    write(tmp.path(), &receipt).unwrap();
                }
            }
        }
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(tmp.path())
            .args(["update-ref", "refs/remotes/origin/remote", "HEAD"])
            .status()
            .unwrap()
            .success());
        prune_settled(tmp.path()).unwrap();
        assert!(read(tmp.path(), "deleted").unwrap().is_none());
        assert!(path(tmp.path(), "deleted").with_extension("lock").exists());
        for branch in ["main", "repair", "remote", "reporting"] {
            assert!(read(tmp.path(), branch).unwrap().is_some(), "{branch}");
        }
        // Cleanup must not unlink an inode another caller may be waiting on.
        begin(tmp.path(), "deleted").unwrap();
        assert!(blocks_review(tmp.path(), "deleted").unwrap());
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_branch_move_transfers_hold_retry_and_result_to_actual_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("seat");
        std::fs::create_dir(&tree).unwrap();
        for args in [
            vec!["init", "-b", "fresh"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        ] {
            assert!(crate::test_git::git_command()
                .arg("-C")
                .arg(&tree)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let mut reservation = reserve(tmp.path(), "recorded").unwrap();
        let mut receipt = read(tmp.path(), "recorded").unwrap().unwrap();
        receipt.repair_used = true;
        write(tmp.path(), &receipt).unwrap();
        assert_eq!(reservation.reconcile_branch(&tree).unwrap(), "fresh");
        assert!(!blocks_review(tmp.path(), "recorded").unwrap());
        assert!(blocks_review(tmp.path(), "fresh").unwrap());
        assert!(read(tmp.path(), "fresh").unwrap().unwrap().repair_used);
        // A still-red rework on a fresh branch cannot buy another repair.
        pushed_round(
            tmp.path(),
            "fresh",
            "head",
            "fail",
            || Ok(()),
            || panic!("second repair"),
        )
        .unwrap();
        assert!(result_line(tmp.path(), "fresh", "implementer:").ends_with("fast_gate=fail"));
        drop(reservation);
        assert!(!blocks_review(tmp.path(), "fresh").unwrap());
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_branch_move_refuses_to_replace_another_pending_lane() {
        let tmp = tempfile::tempdir().unwrap();
        let mut first = reserve(tmp.path(), "first").unwrap();
        let _other = reserve(tmp.path(), "other").unwrap();
        assert!(first.rebind("other").is_err());
        assert!(blocks_review(tmp.path(), "first").unwrap());
        assert!(blocks_review(tmp.path(), "other").unwrap());
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_drop_preserves_new_reservation_and_ordinary_branch_has_no_lock() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!blocks_review(tmp.path(), "ordinary").unwrap());
        assert!(!path(tmp.path(), "ordinary").with_extension("lock").exists());
        let old = reserve(tmp.path(), "lane").unwrap();
        let newer = reserve(tmp.path(), "lane").unwrap();
        drop(old);
        assert!(read(tmp.path(), "lane").unwrap().unwrap().pending);
        drop(newer);
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().pending);
        let stale = reserve(tmp.path(), "lane").unwrap();
        let mut replacement = read(tmp.path(), "lane").unwrap().unwrap();
        replacement.owner.as_mut().unwrap().starttime += 1;
        write(tmp.path(), &replacement).unwrap();
        drop(stale);
        assert!(read(tmp.path(), "lane").unwrap().unwrap().pending);
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_operator_release_refuses_lane_and_missing_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(operator_release(tmp.path(), "unknown", false).is_err());
        let reservation = reserve(tmp.path(), "lane").unwrap();
        assert!(operator_release(tmp.path(), "lane", true).is_err());
        assert!(read(tmp.path(), "lane").unwrap().unwrap().pending);
        operator_release(tmp.path(), "lane", false).unwrap();
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().pending);
        drop(reservation);
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_unpushed_head_never_claims_pr_pass() {
        let tmp = tempfile::tempdir().unwrap();
        for prior_red in [false, true] {
            super::begin(tmp.path(), "push").unwrap();
            if prior_red {
                let mut r = super::read(tmp.path(), "push").unwrap().unwrap();
                r.marker = "fail".into();
                r.repair_used = true;
                super::write(tmp.path(), &r).unwrap();
            }
            let (head, marker, _) = super::pushed_head_attempt("local", "published", || {
                panic!("unpublished head must not run the gate")
            })
            .unwrap();
            super::pushed_round(
                tmp.path(),
                "push",
                &head,
                &marker,
                || Ok(()),
                || panic!("mismatch is not source repair"),
            )
            .unwrap();
            assert_eq!(
                super::marker(tmp.path(), "push"),
                if prior_red {
                    "fail"
                } else {
                    "skipped(unpushed-head)"
                }
            );
        }
    }

    use super::*;
    use crate::lane_job::{self, JobRequest};
    use std::collections::BTreeMap;
    use std::time::Duration;

    // trace:STORY-2640 | ai:codex
    #[test]
    fn killed_host_releases_pending_review_hold_without_drop() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let owner = aida_core::proc_identity::ProcIdentity::capture(child.id());
        // Always reap the bounded child even when identity capture fails.
        child.kill().unwrap();
        child.wait().unwrap();
        let mut r = read(tmp.path(), "lane").unwrap().unwrap();
        r.owner = Some(owner.unwrap());
        r.head = Some("published".into());
        r.marker = "fail".into();
        write(tmp.path(), &r).unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        assert_eq!(marker(tmp.path(), "lane"), "fail");
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().pending);
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn expired_or_operator_released_receipt_cannot_wedge_review() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let mut r = read(tmp.path(), "lane").unwrap().unwrap();
        r.reserved_at = Some(chrono::Utc::now() - chrono::Duration::hours(25));
        write(tmp.path(), &r).unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        // Legacy receipts have neither timestamp nor owner: use file age.
        std::fs::write(
            path(tmp.path(), "lane"),
            r#"{"branch":"lane","head":null,"marker":"fail","pending":true,"repair_used":false}"#,
        )
        .unwrap();
        std::fs::File::options()
            .write(true)
            .open(path(tmp.path(), "lane"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(25 * 3600))
            .unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        begin(tmp.path(), "lane").unwrap();
        assert!(blocks_review(tmp.path(), "lane").unwrap());
        release(tmp.path(), "lane").unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().native_ready);
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn passing_gate_publication_failure_returns_error_and_preserves_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = reserve(tmp.path(), "lane").unwrap();
        native_rounds(
            tmp.path(),
            "lane",
            || Ok(Some(("head".into(), "pass".into(), String::new()))),
            |_| panic!("green must not repair"),
        )
        .unwrap();
        let result: Result<()> =
            publication_result(tmp.path(), "lane", Err(anyhow::anyhow!("push refused")));
        assert!(result.unwrap_err().to_string().contains("push refused"));
        assert_eq!(marker(tmp.path(), "lane"), "pass");
        assert!(result_line(tmp.path(), "lane", "publication failed:").contains("fast_gate=pass"));
        infrastructure(
            tmp.path(),
            "lane",
            "lease",
            &anyhow::anyhow!("unrelated error"),
        );
        assert_eq!(marker(tmp.path(), "lane"), "pass");
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        drop(guard);
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn native_commit_failure_is_a_publication_error() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let error = crate::ship_from_seat::with_post_exit_step(
            Box::new(|_, _, _, _, _| {
                crate::ship_from_seat::PostExitCommit::Failed("commit refused".into())
            }),
            || commit(tmp.path(), tmp.path(), "STORY-2640", "lease", true),
        )
        .unwrap_err();
        assert!(error.is::<NativeCommitError>());
        let result: Result<bool> = publication_result(tmp.path(), "lane", Err(error));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("host post-exit commit failed"));
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        if crate::session::bwrap_availability() == crate::session::BwrapAvailability::Ok {
            begin(tmp.path(), "lane").unwrap();
            let result = crate::ship_from_seat::with_post_exit_step(
                Box::new(|_, _, _, _, _| {
                    crate::ship_from_seat::PostExitCommit::Failed("commit refused".into())
                }),
                || {
                    native(
                        tmp.path(),
                        tmp.path(),
                        "STORY-2640",
                        "lease",
                        "lane",
                        true,
                        |_| panic!("commit failure must not start a repair"),
                    )
                },
            );
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("host post-exit commit failed"));
            assert!(!blocks_review(tmp.path(), "lane").unwrap());
        }
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_retry_timeout_preserves_red_in_both_modes() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "native").unwrap();
        let mut round = 0;
        native_rounds(
            tmp.path(),
            "native",
            || {
                round += 1;
                Ok(Some((
                    format!("head-{round}"),
                    if round == 1 {
                        "fail"
                    } else {
                        "skipped(timeout)"
                    }
                    .into(),
                    String::new(),
                )))
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(marker(tmp.path(), "native"), "fail");
        begin(tmp.path(), "push").unwrap();
        let mut r = read(tmp.path(), "push").unwrap().unwrap();
        r.marker = "fail".into();
        r.repair_used = true;
        write(tmp.path(), &r).unwrap();
        begin(tmp.path(), "push").unwrap();
        pushed_round(
            tmp.path(),
            "push",
            "retry",
            "skipped(timeout)",
            || Ok(()),
            || panic!("no second repair"),
        )
        .unwrap();
        assert_eq!(marker(tmp.path(), "push"), "fail");
        assert_eq!(retry_marker("fail", "pass"), "pass");
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_cleanup_serializes_with_new_reservation() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let lock = receipt_lock(tmp.path(), "lane").unwrap();
        let main = tmp.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let child = std::thread::spawn(move || {
            begin(&main, "lane").unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        release_locked(tmp.path(), "lane").unwrap();
        drop(lock);
        child.join().unwrap();
        assert!(blocks_review(tmp.path(), "lane").unwrap());
        assert!(
            result_line(tmp.path(), "lane", "implementer: held").contains("skipped(lane-ended)")
        );
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_infrastructure_skips_without_consuming_the_repair_round() {
        for (rc, expected) in [
            (lane_job::TIMEOUT_RC, "skipped(timeout)"),
            (lane_job::REFUSED_RC, "skipped(refused)"),
            (lane_job::STOPPED_RC, "skipped(stopped)"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            begin(tmp.path(), "lane").unwrap();
            native_rounds(
                tmp.path(),
                "lane",
                || {
                    let (result, output) =
                        gate_result(Some((rc, "runner infrastructure failure".into())));
                    Ok(Some(("head".into(), result, output)))
                },
                |_| panic!("timeout must not prompt for a source repair"),
            )
            .unwrap();
            let r = read(tmp.path(), "lane").unwrap().unwrap();
            assert_eq!(r.marker, expected);
            assert!(!r.repair_used);
            assert!(r.native_ready);
            begin(tmp.path(), "pushed").unwrap();
            assert!(!pushed_round(
                tmp.path(),
                "pushed",
                "head",
                expected,
                || Ok(()),
                || panic!("timeout must not queue a repair")
            )
            .unwrap());
            assert!(!read(tmp.path(), "pushed").unwrap().unwrap().repair_used);
            assert!(!blocks_review(tmp.path(), "pushed").unwrap());
        }
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac433911 | ai:codex
    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.acb1c30b | ai:codex
    #[test]
    fn native_fail_then_fixed_round_binds_repaired_head() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut attempts = 0;
        let mut repairs = 0;
        native_rounds(
            root,
            "lane",
            || {
                attempts += 1;
                assert!(blocks_review(root, "lane").unwrap());
                Ok(Some(if attempts == 1 {
                    (
                        "old".into(),
                        "fail".into(),
                        "portability gate FAILED".into(),
                    )
                } else {
                    ("fixed".into(), "pass".into(), String::new())
                }))
            },
            |prompt| {
                repairs += 1;
                assert!(prompt.contains("portability gate FAILED"));
                assert!(read(root, "lane")?.unwrap().repair_used);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!((attempts, repairs), (2, 1));
        assert_eq!(marker(root, "lane"), "pass");
        assert!(blocks_review(root, "lane").unwrap());
        published(root, "lane").unwrap();
        assert!(!blocks_review(root, "lane").unwrap());
        assert!(!blocks_review(root, "lane").unwrap());
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.acb1c30b | ai:codex
    #[test]
    fn native_persistent_failure_releases_for_publication_without_third_round() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut repairs = 0;
        for _ in 0..2 {
            native_rounds(
                root,
                "lane",
                || Ok(Some(("same".into(), "fail".into(), "gate red".into()))),
                |_| {
                    repairs += 1;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(marker(root, "lane"), "fail");
            assert!(read(root, "lane").unwrap().unwrap().native_ready);
        }
        assert_eq!(repairs, 1);
        published(root, "lane").unwrap();
        assert!(!blocks_review(root, "lane").unwrap());
        assert_eq!(marker(root, "lane"), "fail");
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_native_uninspectable_seat_is_a_publication_error() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let error = native_error(
            tmp.path(),
            "lane",
            "lease",
            &anyhow::anyhow!("seat changed during fast gate"),
            || anyhow::bail!("dirty seat"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("dirty seat"));
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().native_ready);
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        assert!(result_line(tmp.path(), "lane", "publication failed:")
            .contains("fast_gate=skipped(infrastructure)"));
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_native_errors_preserve_first_red() {
        for fail_repair in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            begin(tmp.path(), "lane").unwrap();
            let mut attempts = 0;
            let error = native_rounds(
                tmp.path(),
                "lane",
                || {
                    attempts += 1;
                    if attempts == 2 {
                        anyhow::bail!("seat changed during fast gate");
                    }
                    Ok(Some(("old".into(), "fail".into(), "red".into())))
                },
                |_| {
                    if fail_repair {
                        anyhow::bail!("vendor spawn failed");
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(
                native_error(tmp.path(), "lane", "lease", &error, || Ok("new".into())).unwrap()
            );
            assert_eq!(marker(tmp.path(), "lane"), "fail");
            assert!(read(tmp.path(), "lane").unwrap().unwrap().native_ready);
            assert!(result_line(tmp.path(), "lane", "implementer:").contains("fast_gate=fail"));
            finish_publication(tmp.path(), "lane", || anyhow::bail!("forge unavailable")).unwrap();
            assert_eq!(marker(tmp.path(), "lane"), "fail");
            assert!(!blocks_review(tmp.path(), "lane").unwrap());
        }
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_self_published_settled_result_allows_later_head() {
        let tmp = tempfile::tempdir().unwrap();
        for result in ["pass", "fail"] {
            begin(tmp.path(), result).unwrap();
            // A red second round settles with its failure marker intact.
            let mut receipt = read(tmp.path(), result).unwrap().unwrap();
            receipt.repair_used = true;
            write(tmp.path(), &receipt).unwrap();
            assert!(!pushed_round(
                tmp.path(),
                result,
                "published",
                result,
                || Ok(()),
                || panic!("a settled round needs no repair"),
            )
            .unwrap());
            assert!(!blocks_review(tmp.path(), result).unwrap());
            assert_eq!(marker(tmp.path(), result), result);
        }
    }

    // trace:STORY-2640 AC1 | ai:codex
    #[test]
    fn fast_gate_self_push_repair_is_drain_claimable() {
        use aida_core::RequirementStatus;
        for status in [
            RequirementStatus::InProgress,
            RequirementStatus::Done,
            RequirementStatus::Planned,
            RequirementStatus::Approved,
            RequirementStatus::NeedsAttention,
        ] {
            let mut req = aida_core::Requirement::new("Repair".into(), String::new());
            req.status = status;
            assert!(prepare_queued_repair(&mut req));
            assert_eq!(req.status, RequirementStatus::Approved);
            assert!(crate::queue_cmd::auto_complete_head_drivable(&req.status));
        }
        for status in [
            RequirementStatus::Completed,
            RequirementStatus::Rejected,
            RequirementStatus::Superseded,
        ] {
            let mut req = aida_core::Requirement::new("Terminal".into(), String::new());
            req.status = status.clone();
            assert!(!prepare_queued_repair(&mut req));
            assert_eq!(req.status, status);
        }
    }

    // trace:STORY-2640 | ai:codex
    #[test]
    fn fast_gate_queued_repair_releases_when_removed_replaced_or_parked() {
        use aida_core::db::DatabaseBackend as _;
        let tmp = tempfile::tempdir().unwrap();
        let backend =
            crate::test_env::home_fixture_backend(&tmp.path().join(".aida-store")).unwrap();
        // This fixture must satisfy allocation trust even with a shared-write
        // launcher umask; never weaken the production reader for a test.
        // trace:STORY-2640 | ai:codex
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let setting =
                aida_core::allocation_authority::common_dir(&tmp.path().join(".aida-store"))
                    .unwrap()
                    .join(aida_core::allocation_authority::CONFIG_FILE);
            std::fs::set_permissions(setting, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        let storage = crate::Storage::new(tmp.path().join(".aida-store"));
        let mut req = aida_core::Requirement::new("Repair".into(), String::new());
        req.status = aida_core::RequirementStatus::InProgress;
        let req = backend.add_requirement(req).unwrap();
        let entry = aida_core::QueueEntry {
            user_id: "role:implementer".into(),
            requirement_id: req.id,
            position: 0,
            added_by: "fast-gate".into(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("implementer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        begin(tmp.path(), "lane").unwrap();
        pushed_round(
            tmp.path(),
            "lane",
            "published",
            "fail",
            || Ok(()),
            || {
                storage.update_atomically(|store| {
                    assert!(prepare_queued_repair(&mut store.requirements[0]));
                })?;
                storage.queue_add(entry.clone())?;
                Ok(Some(entry.clone()))
            },
        )
        .unwrap();
        assert!(blocks_review(tmp.path(), "lane").unwrap());
        let queued = storage.load().unwrap();
        assert!(crate::queue_cmd::auto_complete_head_drivable(
            &queued.requirements[0].status
        ));
        storage.queue_remove(&entry.user_id, &req.id).unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        let mut replacement = entry.clone();
        replacement.added_at += chrono::Duration::seconds(1);
        storage.queue_add(replacement).unwrap();
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        storage.queue_remove(&entry.user_id, &req.id).unwrap();
        storage.queue_add(entry).unwrap();
        for (status, deferred) in [
            (aida_core::RequirementStatus::Superseded, false),
            (aida_core::RequirementStatus::NeedsAttention, false),
            (aida_core::RequirementStatus::InProgress, true),
        ] {
            let mut store = backend.load().unwrap();
            store.requirements[0].status = status;
            store.requirements[0].deferred = deferred;
            backend.save(&store).unwrap();
            assert!(!blocks_review(tmp.path(), "lane").unwrap());
        }
    }

    #[test]
    fn native_repair_veto_keeps_publication_and_review_pending() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        let mut attempts = 0;
        native_rounds(
            tmp.path(),
            "lane",
            || {
                attempts += 1;
                Ok((attempts == 1).then(|| ("old".into(), "fail".into(), "failure".into())))
            },
            |_| Ok(()),
        )
        .unwrap();
        assert!(blocks_review(tmp.path(), "lane").unwrap());
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac486725 | ai:codex
    #[test]
    fn exact_make_job_has_fixed_timeout_and_never_counts_as_cargo_tests() {
        let mut job = JobRequest {
            argv: vec!["make".into(), "check-ci-fast".into()],
            env: BTreeMap::new(),
        };
        assert!(lane_job::validate(&job).is_ok());
        assert_eq!(lane_job::timeout_for(&job, true), Duration::from_secs(600));
        assert!(!lane_job::runs_tests(&job));
        let tally = lane_job::Tally::default();
        tally.record_exit(&job, "test result: ok. 1 passed; 0 failed;".as_bytes());
        assert!(tally.untested());
        job.env.insert("AIDA_CARGO_TIMEOUT".into(), "3600".into());
        assert!(lane_job::validate(&job).is_err());
        job.env.clear();
        job.argv.push("SHELL=evil".into());
        assert!(lane_job::validate(&job).is_err());
    }

    #[test]
    fn host_receipt_is_required_only_for_lane_branches_and_corruption_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!blocks_review(tmp.path(), "ordinary").unwrap());
        begin(tmp.path(), "lane").unwrap();
        assert!(blocks_review(tmp.path(), "lane").unwrap());
        std::fs::write(path(tmp.path(), "lane"), b"invalid").unwrap();
        assert!(blocks_review(tmp.path(), "lane").is_err());
    }
    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.acb1c30b | ai:codex
    #[test]
    fn native_publication_refuses_pending_and_changed_heads() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("seat");
        let main = tmp.path().join("host");
        std::fs::create_dir_all(&tree).unwrap();
        let first = crate::test_git::commit_fixture(&tree);
        begin(&main, "main").unwrap();
        assert!(publication_ready(&main, &tree, "main").is_err());
        native_rounds(
            &main,
            "main",
            || {
                Ok(Some((
                    snapshot(&main, &tree, "main")?,
                    "pass".into(),
                    String::new(),
                )))
            },
            |_| panic!("green gates need no repair"),
        )
        .unwrap();
        publication_ready(&main, &tree, "main").unwrap();
        check_import(&main, "main", &first, true).unwrap();
        assert!(check_import(&main, "main", "different-import", true).is_err());
        let next = crate::test_git::commit_fixture(&tree);
        assert_ne!(first, next);
        assert!(publication_ready(&main, &tree, "main").is_err());
    }

    #[test]
    fn self_pushed_failure_queues_before_review_and_retry_can_stay_red() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        begin(root, "lane").unwrap();
        let mut queued = 0;
        let first = pushed_round(
            root,
            "lane",
            "first",
            "fail",
            || {
                assert!(blocks_review(root, "lane")?);
                Ok(())
            },
            || {
                queued += 1;
                assert!(blocks_review(root, "lane")?);
                Ok(None)
            },
        )
        .unwrap();
        assert!(first);
        assert!(blocks_review(root, "lane").unwrap());
        begin(root, "lane").unwrap();
        assert!(!pushed_round(
            root,
            "lane",
            "second",
            "fail",
            || Ok(()),
            || {
                queued += 1;
                Ok(None)
            }
        )
        .unwrap());
        assert_eq!(queued, 1);
        assert_eq!(marker(root, "lane"), "fail");
        assert!(!blocks_review(root, "lane").unwrap());
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac897346 | ai:codex
    #[test]
    fn annotation_flake_does_not_abort_green_or_red_lane() {
        let tmp = tempfile::tempdir().unwrap();
        for result in ["pass", "fail"] {
            let guard = reserve(tmp.path(), result).unwrap();
            let mut queued = false;
            assert_eq!(
                pushed_round(
                    tmp.path(),
                    result,
                    "head",
                    result,
                    || anyhow::bail!("forge unavailable"),
                    || {
                        queued = true;
                        Ok(None)
                    },
                )
                .unwrap(),
                result == "fail"
            );
            assert_eq!(queued, result == "fail");
            assert_eq!(marker(tmp.path(), result), result);
            drop(guard);
            assert!(!blocks_review(tmp.path(), result).unwrap());
        }
    }

    #[test]
    fn queue_error_settles_red_without_consuming_repair() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = reserve(tmp.path(), "lane").unwrap();
        assert!(pushed_round(
            tmp.path(),
            "lane",
            "head",
            "fail",
            || Ok(()),
            || anyhow::bail!("queue unavailable"),
        )
        .is_err());
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        assert_eq!(marker(tmp.path(), "lane"), "fail");
        drop(guard);
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
        assert!(!read(tmp.path(), "lane").unwrap().unwrap().repair_used);
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac486725 | ai:codex
    #[test]
    fn held_lane_later_opened_by_operator_is_reviewable() {
        let tmp = tempfile::tempdir().unwrap();
        for outcome in ["held", "punted", "no-pr", "lookup-lag"] {
            {
                let _reservation = reserve(tmp.path(), outcome).unwrap();
                assert!(blocks_review(tmp.path(), outcome).unwrap());
            }
            assert!(!blocks_review(tmp.path(), outcome).unwrap());
        }
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac486725 | ai:codex
    #[test]
    fn infrastructure_error_settles_reservation_and_result_line_has_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = reserve(tmp.path(), "lane").unwrap();
        infrastructure(tmp.path(), "lane", "lease", &anyhow::anyhow!("dirty seat"));
        let line = result_line(tmp.path(), "lane", "implementer: pr=77");
        let outcome = crate::standalone_implementer::Outcome::PrOpened {
            pr: 77,
            head: Some("head".into()),
        };
        let line = crate::standalone_implementer::with_untested(line, &outcome, true);
        assert!(line.contains("fast_gate=skipped(infrastructure)"));
        assert!(line.contains("untested=true"));
        drop(guard);
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac897346 | ai:codex
    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.acb1c30b | ai:codex
    #[test]
    fn native_publication_annotation_flake_releases_review_with_known_result() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("host");
        let tree = tmp.path().join("seat");
        std::fs::create_dir_all(&tree).unwrap();
        let head = crate::test_git::commit_fixture(&tree);
        let guard = reserve(&main, "main").unwrap();
        native_rounds(
            &main,
            "main",
            || Ok(Some((head.clone(), "pass".into(), String::new()))),
            |_| panic!("green needs no repair"),
        )
        .unwrap();
        publication_ready(&main, &tree, "main").unwrap();
        check_import(&main, "main", &head, true).unwrap();
        finish_publication(&main, "main", || anyhow::bail!("gh flake")).unwrap();
        drop(guard);
        assert!(!blocks_review(&main, "main").unwrap());
        assert_eq!(
            result_line(&main, "main", "implementer: pr=77"),
            "implementer: pr=77 fast_gate=pass"
        );
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.acc30b8a | ai:codex
    #[test]
    fn advisor_step_six_documents_host_sandbox_and_one_repair() {
        for doc in [
            include_str!("../../.aida/discipline/rework-brief-craft.md"),
            include_str!("../../aida-core/templates/.aida/discipline/rework-brief-craft.md"),
        ] {
            let step = doc.split("6.").nth(1).unwrap().split("7.").next().unwrap();
            assert!(step.contains("host"));
            assert!(step.contains("sandbox"));
            assert!(step.contains("one") || step.contains("ONE"));
            assert!(step.contains("fast_gate="));
            assert!(step.contains("skipped(<reason>)"));
        }
    }

    // trace:STORY-b6b1278890774ef993d2ebc327862470-563.ac486725 | ai:codex
    #[test]
    fn native_no_sandbox_settles_without_publication_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = reserve(tmp.path(), "lane").unwrap();
        skip_native_without_sandbox(tmp.path(), "lane", "lease").unwrap();
        let r = read(tmp.path(), "lane").unwrap().unwrap();
        assert!(!r.pending);
        assert!(!r.native_ready);
        assert_eq!(r.marker, "skipped(no-sandbox)");
        drop(guard);
        assert!(!blocks_review(tmp.path(), "lane").unwrap());
    }

    #[test]
    fn skipped_gate_needs_no_repair_and_does_not_claim_a_pass() {
        let tmp = tempfile::tempdir().unwrap();
        begin(tmp.path(), "lane").unwrap();
        native_rounds(
            tmp.path(),
            "lane",
            || {
                Ok(Some((
                    "head".into(),
                    "skipped(no-sandbox)".into(),
                    String::new(),
                )))
            },
            |_| panic!("no sandbox must never run a repair or an unsandboxed fallback"),
        )
        .unwrap();
        assert_eq!(marker(tmp.path(), "lane"), "skipped(no-sandbox)");
    }
}

// trace:BUG-2512 | ai:codex
#[cfg(test)]
mod bug_2512_tests {
    use super::*;

    #[test]
    fn bug_2512_unconfigured_gate_skips_with_reason_and_committed_head() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = crate::test_git::git_command()
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::write(root.path().join("example.py"), "print('hello')\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "Python project without validation config",
        ]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        let head = git(&["rev-parse", "HEAD"]);
        let (checked_head, marker, reason) =
            attempt(root.path(), root.path(), "fixture", "main").unwrap();
        assert_eq!(checked_head, head);
        assert_eq!(marker, "skipped(unconfigured)");
        assert!(reason.contains("No trusted [validation].fast_check command configured"));
    }
}
