//! Host-side, dry-run-first lifecycle GC for linked worktrees and ended seat clones.
// trace:TASK-1222 | ai:codex
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
struct Candidate {
    path: PathBuf,
    clone: bool,
    branch: Option<String>,
    head: Option<String>,
    bytes: u64,
    bytes_complete: bool,
    eligible: bool,
    reason: String,
    removed: bool,
    #[serde(skip)]
    identity: Option<(u64, u64)>,
}

/// Bound Git commands and preserve diagnostics; an error is never a clean tree.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = aida_core::git_ops::host_git_command(root);
    cmd.args(args);
    let out = crate::command_output_with_timeout(cmd, Duration::from_secs(10))
        .context("GC Git command timed out")?;
    anyhow::ensure!(
        out.status.success(),
        "GC Git command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}

/// Logical bytes, without following symlinks (including a symlinked root).
fn bytes(path: &Path) -> Result<u64> {
    bytes_until(path, Instant::now() + Duration::from_secs(5))
}

// Bound large build-directory walks as well as the outer inventory scan.
fn bytes_until(path: &Path, deadline: Instant) -> Result<u64> {
    anyhow::ensure!(
        Instant::now() < deadline,
        "byte estimate budget exhausted; retained"
    );
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_symlink() {
        return Ok(0);
    }
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        total = total.saturating_add(bytes_until(&entry?.path(), deadline)?);
    }
    Ok(total)
}

// Runtime noise is shared with seat probes; tracked changes always retain work.
// trace:TASK-1222 | ai:codex
fn linked_clean(path: &Path) -> Result<bool> {
    let status = git(
        path,
        &[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "-z",
        ],
    )?;
    for entry in status.split('\0').filter(|s| !s.is_empty()) {
        if !crate::seat_clone::is_runtime_noise_entry(path, entry) {
            return Ok(false);
        }
    }
    let ignored = git(
        path,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )?;
    Ok(ignored
        .split('\0')
        .filter(|s| !s.is_empty())
        .all(|entry| crate::seat_clone::is_runtime_noise_entry(path, &format!("? {entry}"))))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap_or_else(|_| a.to_owned())
        == b.canonicalize().unwrap_or_else(|_| b.to_owned())
}

/// An ended lease is absent, not merely old or attached to an exited process.
/// Require explicit session retirement; GC never releases someone else's lease.
fn ownership_clear(main: &Path, path: &Path, branch: &str) -> Result<()> {
    let leases = crate::leases::list_leases_strict_all(main).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        !leases
            .iter()
            .any(|l| same_path(&l.worktree_path, path) || l.branch == branch),
        "session lease still exists; end/reap the session first"
    );
    let dir = aida_core::liveness::leases_dir(main);
    // exists() also returns false on permission errors and dangling links.
    // Only an actually absent directory proves the lock inventory empty.
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && matches!(std::fs::symlink_metadata(&dir), Err(missing)
                if missing.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error).context("unreadable session ownership directory"),
    };
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(crate::worktree_lock::LOCK_ONLY_PREFIX)
        {
            continue;
        }
        let body = aida_core::seat_clone_detect::read_regular_file(&entry.path())
            .context("unreadable worktree lock")?;
        let lock = crate::worktree_lock::parse_lock_only_entry(&body)?;
        anyhow::ensure!(
            !same_path(Path::new(&lock.worktree_path), path),
            "operator worktree lock exists"
        );
    }
    Ok(())
}

fn inspect(main: &Path, path: &Path, is_clone: bool, default: &str) -> Candidate {
    inspect_with(
        main,
        path,
        is_clone,
        default,
        &mut crate::forge::observe_gc_branch_closed,
    )
}

fn inspect_with(
    main: &Path,
    path: &Path,
    is_clone: bool,
    default: &str,
    closed: &mut impl FnMut(&Path, &str, &str) -> crate::forge_observed::Observed<bool>,
) -> Candidate {
    let mut row = Candidate {
        path: path.to_owned(),
        clone: is_clone,
        branch: None,
        head: None,
        bytes: 0,
        bytes_complete: true,
        eligible: false,
        reason: String::new(),
        removed: false,
        identity: None,
    };
    let result = (|| -> Result<()> {
        row.identity = Some(crate::seat_clone::dir_identity(path)?);
        anyhow::ensure!(
            !same_path(main, path) && !main.starts_with(path),
            "protected main checkout"
        );
        if is_clone {
            anyhow::ensure!(
                crate::seat_clone::is_seat_clone_location(main, path),
                "outside this project's seat parent"
            );
            anyhow::ensure!(
                aida_core::seat_clone_detect::seat_clone_root(path).as_deref() == Some(path),
                "not a recognized seat clone"
            );
            // A shared seat parent can contain several projects. Require this
            // host's base pin plus matching marker and object-store binding.
            let body = aida_core::seat_clone_detect::read_regular_file(
                &path.join(aida_core::seat_clone_detect::MARKER_RELATIVE),
            )
            .context("unreadable seat marker")?;
            let marker: crate::seat_clone::SeatCloneMarker = toml::from_str(&body)?;
            anyhow::ensure!(
                same_path(&marker.main, main),
                "seat belongs to another project"
            );
            let pin =
                crate::seat_clone::pinned_base(main, path).context("missing host seat pin")?;
            anyhow::ensure!(
                pin == marker.base_sha,
                "host seat pin does not match marker"
            );
            let alternates = aida_core::seat_clone_detect::read_regular_file(
                &path.join(".git/objects/info/alternates"),
            )
            .context("unreadable seat alternates")?;
            anyhow::ensure!(
                alternates.lines().count() == 1
                    && same_path(Path::new(alternates.trim()), &main.join(".git/objects")),
                "seat object store does not match host"
            );
            row.branch = crate::seat_clone::checked_out_branch(path);
        } else {
            row.branch = Some(git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?);
        }
        let branch = row
            .branch
            .as_deref()
            .context("detached or unreadable branch")?;
        anyhow::ensure!(
            !["main", "master", "aida-store"].contains(&branch)
                && branch != default.trim_start_matches("origin/"),
            "protected branch"
        );
        ownership_clear(main, path, branch)?;
        // Git worktree locks live in its administrative gitdir.
        if !is_clone {
            let dir = git(path, &["rev-parse", "--absolute-git-dir"])?;
            anyhow::ensure!(
                !Path::new(&dir).join("locked").try_exists()?,
                "Git worktree lock exists"
            );
            anyhow::ensure!(linked_clean(path)?, "uncommitted work");
            anyhow::ensure!(
                !path.join(".gitmodules").try_exists()?
                    || (crate::seat_clone::is_empty_gitmodules_placeholder(path)
                        && git(path, &["ls-files", "--", ".gitmodules"])?.is_empty()),
                "submodules require operator inspection"
            );
            row.head = Some(git(path, &["rev-parse", "HEAD^{commit}"])?);
        } else {
            match crate::seat_clone::probe(main, path) {
                crate::seat_clone::Probe::Known(facts) => {
                    anyhow::ensure!(facts.dirty.is_empty(), "uncommitted work in seat clone");
                    anyhow::ensure!(
                        facts.branch == row.branch,
                        "seat branch changed during inspection"
                    );
                    row.head = facts.head;
                }
                crate::seat_clone::Probe::Unknown(why)
                | crate::seat_clone::Probe::HasSubmodules(why) => {
                    bail!("seat inspection refused: {why}")
                }
            }
            anyhow::ensure!(
                crate::seat_clone::origin_missing_commits(main, path, branch)? == 0,
                "seat clone has commits missing from origin"
            );
        }
        let head = row.head.as_deref().context("missing HEAD")?;
        // Linked ancestry needs no network. Squashes/closed PRs require the
        // forge's exact head, never just a reused branch name.
        let ancestor = !is_clone
            && git(
                main,
                &["rev-list", "--count", &format!("{default}..{head}")],
            )? == "0";
        if !ancestor {
            match closed(main, branch, head) {
                crate::forge_observed::Observed::Known(true) => {}
                crate::forge_observed::Observed::Known(false) => {
                    bail!("PR is open, absent, or has a different head")
                }
                crate::forge_observed::Observed::Pending(why)
                | crate::forge_observed::Observed::Transient(why) => {
                    bail!("PR state unknown: {why}")
                }
            }
        }
        aida_core::worktree_holders::check_worktree_removal(path)?;
        estimate_bytes(&mut row, bytes(path));
        if is_clone {
            let target = main
                .join("target/worktrees")
                .join(path.file_name().context("seat name")?);
            match target.try_exists() {
                Ok(true) => estimate_bytes(&mut row, bytes(&target)),
                Ok(false) => {}
                Err(_) => row.bytes_complete = false,
            }
        }
        row.reason = if ancestor {
            "branch merged into default branch"
        } else {
            "matching PR merged or closed"
        }
        .into();
        row.eligible = true;
        Ok(())
    })();
    if let Err(error) = result {
        row.reason = format!("{error:#}");
    }
    row
}

// Accounting is informational: incomplete walks must never veto safe cleanup.
// trace:TASK-1222 | ai:codex
fn estimate_bytes(row: &mut Candidate, estimate: Result<u64>) {
    match estimate {
        Ok(size) => row.bytes = row.bytes.saturating_add(size),
        Err(_) => row.bytes_complete = false,
    }
}

fn inventory(main: &Path) -> Result<Vec<(PathBuf, bool)>> {
    // Read the actual registration list; a failed listing must not mean empty.
    let raw = git(main, &["worktree", "list", "--porcelain"])?;
    let mut paths: Vec<_> = crate::worktree_reclaim::parse_worktree_entries(&raw)
        .into_iter()
        .filter(|w| !same_path(main, &w.path) && w.branch.as_deref() != Some("aida-store"))
        .map(|w| (w.path, false))
        .collect();
    let configured = crate::worktree_gc::worktree_pool_config_worktree_parent(main);
    let sample = aida_core::worktree_pool::worktree_placement_path(
        main,
        "aida-seat-placeholder",
        configured.as_deref(),
    );
    let parent = sample.parent().context("no seat parent")?;
    if parent.try_exists()? {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name();
            if !name
                .to_str()
                .is_some_and(|name| name.starts_with("aida-seat-"))
                || !entry.file_type()?.is_dir()
            {
                continue;
            }
            let path = entry.path();
            if !paths.iter().any(|(p, _)| same_path(p, &path)) {
                paths.push((path, true));
            }
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(paths)
}

fn remove(main: &Path, row: &Candidate, default: &str) -> Result<bool> {
    remove_with(
        main,
        row,
        default,
        &crate::worktree_gc::worktree_pool_global_hooks("pre_destroy"),
        &mut crate::forge::observe_gc_branch_closed,
    )
}

fn remove_with(
    main: &Path,
    row: &Candidate,
    default: &str,
    pre_destroy_hooks: &[String],
    closed: &mut impl FnMut(&Path, &str, &str) -> crate::forge_observed::Observed<bool>,
) -> Result<bool> {
    // Reread ownership, clean state, remote coverage, forge status, HEAD and
    // directory identity immediately before the existing guarded teardown.
    crate::seat_clone::ensure_dir_identity(
        &row.path,
        row.identity.context("missing scan identity")?,
    )?;
    let fresh = inspect_with(main, &row.path, row.clone, default, closed);
    anyhow::ensure!(
        fresh.eligible
            && fresh.head == row.head
            && fresh.branch == row.branch
            && fresh.identity == row.identity,
        "candidate changed or became unsafe: {}",
        fresh.reason
    );
    if row.clone {
        // Ended clones no longer have a lease for verified_clone_main.
        // The fresh host pin + marker + alternates proof above anchors their
        // mailbox to this host without manufacturing a replacement lease.
        let retired = aida_core::mailbox_lifecycle::guard::retire_clone_checkout(
            Some(main),
            &row.path,
            "ended seat lifecycle GC",
        )
        .map_err(|refusal| {
            anyhow::anyhow!(crate::mail_retention::refusal_lines(&refusal).join("; "))
        })?;
        let final_check = crate::seat_clone::ensure_dir_identity(
            &row.path,
            row.identity.context("missing scan identity")?,
        )
        .and_then(|_| ownership_clear(main, &row.path, row.branch.as_deref().unwrap_or_default()));
        if let Err(error) = final_check {
            crate::mail_retention::reopen_quietly(retired);
            return Err(error);
        }
        // The strict origin check proved this exact HEAD remotely covered,
        // including an advertised PR ref after source-branch deletion. Carry
        // only that pinned proof into the existing salvage-aware teardown.
        let remotely_covered_head: Vec<String> = row.head.iter().cloned().collect();
        match crate::seat_clone::destroy_retained(
            main,
            &row.path,
            crate::seat_clone::DestroyMode::KeepUnshipped,
            row.head.as_deref(),
            &remotely_covered_head,
            retired,
        )? {
            crate::seat_clone::Destroyed::Removed { .. } => Ok(true),
            crate::seat_clone::Destroyed::Kept { reason, .. } => bail!("{reason}"),
        }
    } else {
        crate::seat_clone::ensure_dir_identity(
            &row.path,
            row.identity.context("missing scan identity")?,
        )?;
        ownership_clear(
            main,
            &row.path,
            row.branch.as_deref().context("missing branch")?,
        )?;
        // Forge and byte-size reads can take time. Reread the local mutation
        // evidence after those reads, directly before shared teardown.
        anyhow::ensure!(
            Some(git(&row.path, &["rev-parse", "HEAD^{commit}"])?).as_ref() == row.head.as_ref()
                && Some(git(
                    &row.path,
                    &["symbolic-ref", "--quiet", "--short", "HEAD"]
                )?)
                .as_ref()
                    == row.branch.as_ref()
                && linked_clean(&row.path)?,
            "linked worktree changed before teardown"
        );
        let removed = aida_core::worktree_pool_destroy::teardown_worktree_path(
            main,
            &row.path,
            pre_destroy_hooks,
        )?;
        // Keep the ref if it moved or another checkout adopted it; cleanup
        // must not delete a new branch incarnation after removing an old tree.
        if removed {
            let branch = row.branch.as_deref().context("missing branch")?;
            let registered = git(main, &["worktree", "list", "--porcelain"])?;
            if !crate::worktree_reclaim::parse_worktree_entries(&registered)
                .iter()
                .any(|w| w.branch.as_deref() == Some(branch))
            {
                git(
                    main,
                    &[
                        "update-ref",
                        "-d",
                        &format!("refs/heads/{branch}"),
                        row.head.as_deref().context("missing head")?,
                    ],
                )?;
            }
        }
        Ok(removed)
    }
}

fn freed_estimate(main: &Path, row: &Candidate) -> u64 {
    if !row.clone {
        return row.bytes;
    }
    let Some(name) = row.path.file_name() else {
        return 0;
    };
    let target = main.join("target/worktrees").join(name);
    match target.try_exists() {
        Ok(false) => row.bytes,
        Ok(true) => row
            .bytes
            .saturating_sub(bytes(&target).unwrap_or(row.bytes)),
        Err(_) => 0,
    }
}

// Keep the existing front door's global --format and agent-mode behavior.
fn render_toon(report: &serde_json::Value) -> String {
    use crate::toon::{scalar, table_raw};
    let mut blocks: Vec<String> = [
        "partial",
        "remaining",
        "dry_run",
        "scanned",
        "eligible",
        "kept",
        "removed",
        "failures",
        "reclaimable_bytes",
        "freed_bytes",
        "bytes_complete",
    ]
    .into_iter()
    .map(|key| scalar(key, &report[key].to_string()))
    .collect();
    let fields = [
        "path",
        "clone",
        "branch",
        "head",
        "bytes",
        "eligible",
        "removed",
        "reason",
        "bytes_complete",
    ];
    let rows: Vec<Vec<String>> = report["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            fields
                .iter()
                .map(|field| match &item[*field] {
                    serde_json::Value::String(value) => value.clone(),
                    serde_json::Value::Null => String::new(),
                    value => value.to_string(),
                })
                .collect()
        })
        .collect();
    blocks.push(table_raw("items", &fields, &rows));
    blocks.join("\n")
}

// Shared by dispatch and authority regressions, including legacy flags.
// trace:TASK-1222 | ai:codex
pub(crate) fn apply_requested(apply: bool, yes: bool, force: bool) -> bool {
    apply || (yes && force)
}

// trace:TASK-1222 | ai:codex
fn authorize_apply(apply: bool, seat: bool, autonomous: bool) -> Result<()> {
    if apply && seat {
        bail!("worktree gc --apply must run on the host, outside a seat clone");
    }
    if apply && autonomous {
        bail!("worktree gc --apply requires an interactive operator terminal; scheduled/headless cleanup is refused");
    }
    Ok(())
}

// Leave enough of the scheduler's 300s ceiling for a final bounded inspection
// and report delivery. Never silently claim a complete inventory.
fn scan_budgeted<T, R>(
    items: impl IntoIterator<Item = T>,
    budget: Duration,
    mut inspect: impl FnMut(T) -> R,
) -> Vec<R> {
    let deadline = Instant::now() + budget;
    items
        .into_iter()
        .take_while(|_| Instant::now() < deadline)
        .map(&mut inspect)
        .collect()
}

pub(crate) fn run(apply: bool, json: bool, fail_on_findings: bool) -> Result<()> {
    let invoking = crate::find_project_root()?;
    let main = crate::worktree_gc::main_worktree_root_from(&invoking);
    authorize_apply(
        apply,
        aida_core::seat_clone_detect::seat_clone_root(&invoking).is_some(),
        crate::doctor_cmd::doctor_running_autonomously(&main),
    )?;
    let default =
        crate::resolve_default_branch_ref(&main).context("cannot resolve default branch")?;
    let inventory = inventory(&main)?;
    let total = inventory.len();
    let mut rows = scan_budgeted(inventory, Duration::from_secs(60), |(p, c)| {
        inspect(&main, &p, c, &default)
    });
    let remaining = total - rows.len();
    let eligible = rows.iter().filter(|r| r.eligible).count();
    let reclaimable_bytes: u64 = rows.iter().filter(|r| r.eligible).map(|r| r.bytes).sum();
    let mut freed_bytes = 0u64;
    let mut failures = 0usize;
    for row in rows.iter_mut().filter(|r| r.eligible && apply) {
        match remove(&main, row, &default) {
            Ok(true) => {
                row.removed = true;
                freed_bytes += freed_estimate(&main, row);
            }
            Ok(false) => {
                failures += 1;
                row.reason = "teardown kept worktree".into();
            }
            Err(error) => {
                failures += 1;
                // A compare-delete of the branch can fail after tree teardown.
                row.reason = format!("cleanup incomplete: {error:#}");
                match row.path.try_exists() {
                    Ok(exists) => row.removed = !exists,
                    Err(stat_error) => {
                        row.removed = false;
                        row.reason
                            .push_str(&format!("; removal unconfirmed: {stat_error}"));
                    }
                }
                if row.removed {
                    freed_bytes += freed_estimate(&main, row);
                }
            }
        }
    }
    let removed = rows.iter().filter(|r| r.removed).count();
    let kept = rows
        .iter()
        .filter(|r| !r.eligible || (apply && !r.removed))
        .count();
    let report = serde_json::json!({
        "partial": remaining > 0, "remaining": remaining, "dry_run": !apply, "eligible": eligible, "kept": kept, "scanned": rows.len(), "failures": failures,
        "bytes_complete": rows.iter().all(|r| r.bytes_complete),
        "removed": removed, "reclaimable_bytes": reclaimable_bytes, "freed_bytes": freed_bytes, "items": &rows
    });
    use crate::doctor_cmd::DoctorRenderMode;
    match crate::doctor_cmd::doctor_render_mode_from(
        json,
        crate::output_format_override(),
        crate::agent_output_mode(),
    ) {
        DoctorRenderMode::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        DoctorRenderMode::Toon => println!("{}", render_toon(&report)),
        DoctorRenderMode::Human => {
            println!(
                "Scan: {} inspected; {remaining} deferred by budget.",
                rows.len()
            );
            for row in &rows {
                println!(
                    "{} {} ({} bytes{}): {}",
                    if row.removed {
                        "removed"
                    } else if row.eligible && !apply {
                        "eligible"
                    } else {
                        "kept"
                    },
                    row.path.display(),
                    row.bytes,
                    if row.bytes_complete {
                        ""
                    } else {
                        "+; estimate incomplete"
                    },
                    row.reason
                );
            }
            println!("{} eligible; {kept} kept; {removed} removed; {reclaimable_bytes} bytes reclaimable; {freed_bytes} bytes freed (estimated logical size).", eligible);
        }
    }
    if apply && failures > 0 {
        bail!("some GC candidates were retained; inspect item reasons");
    }
    if fail_on_findings && (eligible > 0 || remaining > 0) {
        bail!(
            "{}",
            findings_summary(
                rows.len(),
                eligible,
                kept,
                remaining,
                reclaimable_bytes,
                rows.iter().all(|r| r.bytes_complete)
            )
        );
    }
    Ok(())
}

// The scheduler routes only this final error line to the advisor.
// trace:TASK-1222 | ai:codex
fn findings_summary(
    scanned: usize,
    eligible: usize,
    kept: usize,
    remaining: usize,
    bytes: u64,
    bytes_complete: bool,
) -> String {
    format!("lifecycle GC: scanned={scanned}; eligible={eligible}; kept={kept}; remaining={remaining}; reclaimable_bytes={bytes}; bytes_complete={bytes_complete}; operator review required")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge_observed::Observed;

    #[test]
    fn task_1222_runtime_layout_and_placeholders_are_disposable() {
        // trace:TASK-1222.acfbfd06 | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "runtime");
        std::fs::write(tree.join(".gitignore"), ".aida/\ntarget/\n.aida-store\n.codegraph\n.claude/\n.bashrc\n.gitconfig\n.gitmodules\n.env\nnotes\n").unwrap();
        git_ok(&tree, &["add", ".gitignore"]);
        git_ok(&tree, &["commit", "-qm", "runtime ignores"]);
        for directory in [
            ".aida/agents",
            ".aida/cache-db",
            ".aida/sessions",
            "target",
            ".claude",
        ] {
            std::fs::create_dir_all(tree.join(directory)).unwrap();
        }
        for file in [
            ".aida/agents/registry",
            ".aida/cache-db/cache",
            ".aida/sessions/ended",
            ".aida/session-env.sh",
            "target/cache",
        ] {
            std::fs::write(tree.join(file), "runtime").unwrap();
        }
        for file in [".bashrc", ".gitconfig", ".gitmodules"] {
            std::fs::write(tree.join(file), "").unwrap();
        }
        for link in [".aida-store", ".codegraph", ".claude/skills"] {
            std::os::unix::fs::symlink(main.join("missing-runtime"), tree.join(link)).unwrap();
        }
        let row = inspect_with(&main, &tree, false, "origin/main", &mut closed);
        assert!(row.eligible, "{}", row.reason);
        for file in [
            ".aida/merge-holds",
            ".aida/review-verdicts",
            ".env",
            "notes",
            ".claude/local-notes",
            ".gitmodules",
            ".bashrc",
        ] {
            std::fs::write(tree.join(file), "precious").unwrap();
            assert!(
                !inspect_with(&main, &tree, false, "origin/main", &mut closed).eligible,
                "{file}"
            );
            std::fs::remove_file(tree.join(file)).unwrap();
        }
        std::fs::write(tree.join(".gitmodules"), "").unwrap();
        git_ok(&tree, &["add", "-f", ".gitmodules"]);
        git_ok(&tree, &["commit", "-qm", "tracked modules"]);
        assert!(!inspect_with(&main, &tree, false, "origin/main", &mut closed).eligible);
    }

    #[test]
    fn task_1222_partial_advisor_summary_has_all_counts() {
        // trace:TASK-1222.ac14b158 | ai:codex
        assert_eq!(findings_summary(3, 0, 3, 12, 0, false),
            "lifecycle GC: scanned=3; eligible=0; kept=3; remaining=12; reclaimable_bytes=0; bytes_complete=false; operator review required");
    }

    #[test]
    fn task_1222_byte_budget_does_not_veto_eligibility() {
        // trace:TASK-1222.ac683e0c | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        let mut row = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(row.eligible, "{}", row.reason);
        let known = row.bytes;
        estimate_bytes(&mut row, bytes_until(&tree, Instant::now()));
        assert!(row.eligible);
        assert!(!row.bytes_complete);
        assert_eq!(row.bytes, known);
        assert!(tree.exists());
    }

    #[test]
    fn task_1222_budget_and_apply_authority() {
        assert!(scan_budgeted([1, 2], Duration::ZERO, |_| panic!("must defer")).is_empty());
        assert_eq!(
            scan_budgeted([1, 2], Duration::from_secs(1), |v| v),
            vec![1, 2]
        );
        for apply in [
            apply_requested(true, false, false),
            apply_requested(false, true, true),
        ] {
            assert!(authorize_apply(apply, true, false).is_err());
            assert!(authorize_apply(apply, false, true).is_err());
        }
        assert!(!apply_requested(false, true, false));
        assert!(!apply_requested(false, false, true));
        assert!(authorize_apply(false, true, true).is_ok());
        assert!(authorize_apply(true, false, false).is_ok());
    }

    #[test]
    fn task_1222_ignored_local_state_is_retained() {
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        std::fs::create_dir_all(tree.join(".aida")).unwrap();
        std::fs::write(tree.join(".aida/merge-hold"), "retain").unwrap();
        let row = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(
            !row.eligible && row.reason.contains("uncommitted"),
            "{}",
            row.reason
        );
        assert!(tree.join(".aida/merge-hold").exists());
        std::fs::remove_dir_all(tree.join(".aida")).unwrap();
        std::fs::create_dir_all(tree.join("target")).unwrap();
        std::fs::write(tree.join("target/cache"), "regenerable").unwrap();
        assert!(linked_clean(&tree).unwrap());
        git_ok(&tree, &["add", "-f", "target/cache"]);
        git_ok(&tree, &["commit", "-qm", "tracked target"]);
        std::fs::write(tree.join("target/cache"), "modified tracked work").unwrap();
        assert!(!linked_clean(&tree).unwrap());
    }

    #[test]
    fn task_1222_live_holder_retains_linked_tree() {
        // trace:TASK-1222.acfbfd06 | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        let _held_file = std::fs::File::open(tree.join("seed")).unwrap();
        // The caller is intentionally exempt; a separate process must hold it.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&tree)
            .spawn()
            .unwrap();
        let result = aida_core::worktree_holders::check_worktree_removal(&tree);
        let row = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(result.is_err());
        assert!(!row.eligible, "{}", row.reason);
        assert!(tree.exists());
    }

    fn git_ok(root: &Path, args: &[&str]) -> String {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        std::fs::create_dir(&main).unwrap();
        git_ok(&main, &["init", "-q", "-b", "main"]);
        git_ok(&main, &["config", "user.email", "test@example.com"]);
        git_ok(&main, &["config", "user.name", "Test"]);
        std::fs::write(main.join(".gitignore"), ".aida/\ntarget/\n").unwrap();
        std::fs::write(main.join("seed"), "seed\n").unwrap();
        git_ok(&main, &["add", "."]);
        git_ok(&main, &["commit", "-qm", "seed"]);
        let origin = temp.path().join("origin.git");
        git_ok(
            temp.path(),
            &[
                "clone",
                "-q",
                "--bare",
                main.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git_ok(
            &main,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_ok(&main, &["fetch", "-q", "origin"]);
        (temp, main)
    }
    fn linked(main: &Path, name: &str) -> PathBuf {
        let path = main.parent().unwrap().join(name);
        git_ok(
            main,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }
    fn closed(_: &Path, _: &str, _: &str) -> Observed<bool> {
        Observed::Known(true)
    }
    fn absent(_: &Path, _: &str, _: &str) -> Observed<bool> {
        Observed::Known(false)
    }

    #[test]
    fn task_1222_toon_report_preserves_counts_bytes_and_quoted_paths() {
        let report = serde_json::json!({
            "dry_run":true, "scanned":1, "eligible":0, "kept":1, "removed":0,
            "failures":0, "reclaimable_bytes":0, "freed_bytes":0, "bytes_complete":false,
            "items":[{"path":"seat,one", "clone":true, "branch":null,
                "head":null, "bytes":42, "eligible":false, "removed":false,
                "reason":"retained, dirty", "bytes_complete":false}]
        });
        let output = render_toon(&report);
        assert!(output.contains("kept: 1"));
        assert!(output.contains("freed_bytes: 0"));
        let table = crate::toon::parse_table(&output[output.find("items[").unwrap()..]).unwrap();
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][0], "seat,one");
        assert_eq!(table.rows[0][4], "42");
        assert_eq!(table.rows[0][7], "retained, dirty");
        assert_eq!(table.rows[0][8], "false");
        assert!(output.contains("bytes_complete: false"));
    }

    #[test]
    fn task_1222_linked_preview_and_removal_revalidate_dirty_and_locks() {
        // trace:TASK-1222.acfbfd06 | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        let plan = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(plan.eligible, "{}", plan.reason);
        assert!(plan.bytes > 0);
        assert!(tree.exists(), "preview must not remove");
        std::fs::write(tree.join("untracked"), "precious").unwrap();
        assert!(remove_with(&main, &plan, "origin/main", &[], &mut absent).is_err());
        assert!(tree.exists());
        std::fs::remove_file(tree.join("untracked")).unwrap();
        git_ok(&main, &["worktree", "lock", tree.to_str().unwrap()]);
        let locked = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(
            !locked.eligible && locked.reason.contains("lock"),
            "{}",
            locked.reason
        );
        git_ok(&main, &["worktree", "unlock", tree.to_str().unwrap()]);
        assert!(remove_with(&main, &plan, "origin/main", &[], &mut absent).unwrap());
        assert!(!tree.exists());
        assert!(git(&main, &["rev-parse", "--verify", "refs/heads/feature"]).is_err());
        assert!(main.join("seed").exists());
    }

    #[test]
    fn task_1222_replaced_directory_is_retained() {
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        let row = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(row.eligible, "{}", row.reason);
        let moved = tree.with_extension("original");
        std::fs::rename(&tree, &moved).unwrap();
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("precious"), "replacement").unwrap();
        assert!(remove_with(&main, &row, "origin/main", &[], &mut absent).is_err());
        assert!(tree.join("precious").exists());
        assert!(moved.join("seed").exists());
    }

    #[test]
    fn task_1222_linked_closed_pr_and_unknown_or_reopened_pr() {
        // trace:TASK-1222.acfbfd06 | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "unmerged");
        std::fs::write(tree.join("new"), "published work").unwrap();
        git_ok(&tree, &["add", "."]);
        git_ok(&tree, &["commit", "-qm", "new"]);
        assert!(!inspect_with(&main, &tree, false, "origin/main", &mut absent).eligible);
        let row = inspect_with(&main, &tree, false, "origin/main", &mut closed);
        assert!(row.eligible, "{}", row.reason);
        assert!(
            remove_with(&main, &row, "origin/main", &[], &mut absent).is_err(),
            "reopened PR must keep tree"
        );
        let unknown = inspect_with(&main, &tree, false, "origin/main", &mut |_, _, _| {
            Observed::Transient("offline".into())
        });
        assert!(!unknown.eligible);
        assert!(remove_with(&main, &row, "origin/main", &[], &mut closed).unwrap());
    }

    #[test]
    fn task_1222_ownership_unknown_and_locks_keep_linked_tree() {
        // trace:TASK-1222.acfbfd06 | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        crate::worktree_lock::acquire(&main, &tree, "advisor").unwrap();
        let locked = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(!locked.eligible, "{}", locked.reason);
        crate::worktree_lock::release(&main, &tree).unwrap();
        let dir = aida_core::liveness::leases_dir(&main);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.toml"), "not a valid lease").unwrap();
        assert!(!inspect_with(&main, &tree, false, "origin/main", &mut absent).eligible);
        assert!(tree.exists());
    }

    #[test]
    fn task_1222_unreadable_ownership_directory_retains_worktree() {
        let (_tmp, main) = fixture();
        let tree = linked(&main, "feature");
        let dir = aida_core::liveness::leases_dir(&main);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(main.join("missing-sessions"), &dir).unwrap();
        let row = inspect_with(&main, &tree, false, "origin/main", &mut absent);
        assert!(
            !row.eligible && row.reason.contains("unreadable session ownership"),
            "{}",
            row.reason
        );
        assert!(tree.join("seed").exists());
    }

    #[test]
    fn task_1222_ended_clone_requires_origin_coverage_and_closed_pr() {
        // trace:TASK-1222.ac683e0c | ai:codex
        // trace:TASK-1222.ac5418e8 | ai:codex
        let (_tmp, main) = fixture();
        let clone = main.parent().unwrap().join("aida-seat-project-test");
        crate::seat_clone::create(&main, &clone, "feature", "origin/main").unwrap();
        let target = main
            .join("target/worktrees")
            .join(clone.file_name().unwrap());
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("cache"), "build cache").unwrap();
        let row = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(row.eligible, "{}", row.reason);
        assert!(row.bytes >= bytes(&clone).unwrap() + 11);
        assert!(inventory(&main)
            .unwrap()
            .iter()
            .any(|(p, c)| p == &clone && *c));
        assert!(!inspect_with(&main, &clone, true, "origin/main", &mut absent).eligible);
        // A lease is still owning even when its PID has exited.
        let dir = aida_core::liveness::leases_dir(&main);
        std::fs::create_dir_all(&dir).unwrap();
        let lease = format!("id = 'gc-test'\nscope = 'TASK-1222'\nslug = 'test'\nowner = 'test'\nworktree_path = '{}'\nbranch = 'feature'\nstarted_at = '2026-10-01T00:00:00Z'\nhostname = 'test'\ntree_kind = 'clone'\nactive_pid = 4294967294\n", clone.display());
        let lease_path = dir.join("gc-test.toml");
        std::fs::write(&lease_path, lease).unwrap();
        let held = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(
            !held.eligible && held.reason.contains("lease"),
            "{}",
            held.reason
        );
        std::fs::remove_file(lease_path).unwrap();
        std::fs::write(clone.join("precious"), "uncommitted").unwrap();
        assert!(remove_with(&main, &row, "origin/main", &[], &mut closed).is_err());
        std::fs::remove_file(clone.join("precious")).unwrap();
        // Use the accessor's sandboxed commit interface in production; fixture
        // Git is deliberately allowed by the shared test command builder.
        git_ok(&clone, &["config", "user.email", "test@example.com"]);
        git_ok(&clone, &["config", "user.name", "Test"]);
        std::fs::write(clone.join("new"), "local-only commit").unwrap();
        git_ok(&clone, &["add", "."]);
        git_ok(&clone, &["commit", "-qm", "local"]);
        let missing = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(
            !missing.eligible && missing.reason.contains("missing from origin"),
            "{}",
            missing.reason
        );
        // A second remote's ref is insufficient; only origin grants coverage.
        let head = git_ok(&clone, &["rev-parse", "HEAD"]);
        git_ok(
            &clone,
            &["update-ref", "refs/remotes/backup/feature", &head],
        );
        assert!(!inspect_with(&main, &clone, true, "origin/main", &mut closed).eligible);
        // A forged seat-local origin ref cannot supply host origin evidence.
        git_ok(&clone, &["update-ref", "refs/remotes/origin/forged", &head]);
        assert!(!inspect_with(&main, &clone, true, "origin/main", &mut closed).eligible);
        git_ok(&clone, &["push", "-q", "origin", "HEAD:refs/heads/feature"]);
        let published = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(published.eligible, "{}", published.reason);
        // A squash merge / closed PR can leave only its origin PR ref. The
        // source head is not an ancestor of main, and no tracking ref in the
        // clone or host may serve as a substitute for that advertisement.
        let origin = main.parent().unwrap().join("origin.git");
        git_ok(
            &main,
            &[
                "fetch",
                "-q",
                "--no-write-fetch-head",
                "origin",
                "refs/heads/feature:refs/aida/gc-fixture-source",
            ],
        );
        git_ok(&main, &["merge", "--squash", "refs/aida/gc-fixture-source"]);
        git_ok(&main, &["commit", "-qm", "squash"]);
        git_ok(&main, &["push", "-q", "origin", "main"]);
        git_ok(&main, &["update-ref", "-d", "refs/aida/gc-fixture-source"]);
        git_ok(&main, &["update-ref", "-d", "refs/remotes/origin/feature"]);
        assert_ne!(
            git_ok(
                &main,
                &["rev-list", "--count", &format!("origin/main..{head}")]
            ),
            "0"
        );
        git_ok(&origin, &["update-ref", "refs/pull/1/head", &head]);
        git_ok(&clone, &["push", "-q", "origin", ":refs/heads/feature"]);
        git_ok(&clone, &["update-ref", "-d", "refs/remotes/origin/feature"]);
        git_ok(&clone, &["update-ref", "-d", "refs/remotes/origin/forged"]);
        git_ok(&clone, &["update-ref", "-d", "refs/remotes/backup/feature"]);
        let published = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(published.eligible, "{}", published.reason);
        std::fs::write(clone.join("new"), "stashed work").unwrap();
        git_ok(&clone, &["stash", "push", "-qm", "keep"]);
        let stashed = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(
            !stashed.eligible && stashed.reason.contains("missing from origin"),
            "{}",
            stashed.reason
        );
        git_ok(&clone, &["stash", "drop", "-q"]);
        git_ok(&clone, &["checkout", "-qb", "spare"]);
        std::fs::write(clone.join("side"), "work on another branch").unwrap();
        git_ok(&clone, &["add", "."]);
        git_ok(&clone, &["commit", "-qm", "side"]);
        git_ok(&clone, &["checkout", "-q", "feature"]);
        let side = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(
            !side.eligible && side.reason.contains("missing from origin"),
            "{}",
            side.reason
        );
        git_ok(&clone, &["branch", "-D", "spare"]);
        assert!(remove_with(&main, &published, "origin/main", &[], &mut closed).unwrap());
        assert!(!clone.exists());
        assert!(!target.exists());
        assert!(crate::seat_clone::pinned_base(&main, &clone).is_none());
    }

    #[test]
    fn task_1222_never_counts_symlink_targets_or_accepts_foreign_clone() {
        let (_tmp, main) = fixture();
        let link = main.parent().unwrap().join("aida-seat-foreign");
        std::os::unix::fs::symlink(&main, &link).unwrap();
        assert_eq!(bytes(&link).unwrap(), 0);
        assert!(!inventory(&main).unwrap().iter().any(|(p, _)| p == &link));
        assert!(!inspect_with(&main, &main, false, "origin/main", &mut closed).eligible);
        let clone = main.parent().unwrap().join("aida-seat-project-test");
        crate::seat_clone::create(&main, &clone, "feature", "origin/main").unwrap();
        let marker_path = clone.join(aida_core::seat_clone_detect::MARKER_RELATIVE);
        let mut marker: crate::seat_clone::SeatCloneMarker =
            toml::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
        marker.main = main.parent().unwrap().to_owned();
        std::fs::write(marker_path, toml::to_string(&marker).unwrap()).unwrap();
        let row = inspect_with(&main, &clone, true, "origin/main", &mut closed);
        assert!(
            !row.eligible && row.reason.contains("another project"),
            "{}",
            row.reason
        );
    }
}

#[cfg(test)]
mod task_1222_cli_tests {
    use crate::worktree_gc::{WorktreeArgs, WorktreeCommand};
    #[test]
    fn task_1222_gc_cli_defaults_to_preview_and_rejects_apply_guard_combo() {
        let cli = crate::command_registry::try_parse_from(["aida", "worktree", "gc"]).unwrap();
        let WorktreeArgs {
            inner:
                WorktreeCommand::Gc {
                    apply,
                    yes,
                    force,
                    fail_on_findings,
                    ..
                },
            ..
        } = cli.command.registered::<WorktreeArgs>()
        else {
            panic!("expected gc");
        };
        assert!(!apply && !yes && !force && !fail_on_findings);
        assert!(
            crate::command_registry::try_parse_from(["aida", "worktree", "gc", "--apply"]).is_ok()
        );
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "worktree",
            "gc",
            "--apply",
            "--fail-on-findings"
        ])
        .is_err());
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "worktree",
            "gc",
            "--force",
            "--fail-on-findings"
        ])
        .is_err());
    }
}
