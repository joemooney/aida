//! `aida ship --from-seat <clone>`: the host publishes a contained seat's
//! committed branch without running git in the seat's clone (BUG-HOME-038).
//!
//! A seat clone's `.git/config` and hooks are seat-writable, so host git
//! never runs with the clone as its repository or as a remote (the TASK-2053
//! invariant). Instead, per the advisor's ruling:
//!
//!   1. the clone must be one this checkout recorded (its `clone` lease);
//!   2. git inside the seat sandbox bundles `refs/heads/<branch>` alone
//!      ([`crate::seat_clone::bundle_branch`]: hooks and fsmonitor off);
//!   3. the host runs `git bundle verify` and accepts exactly one ref, the
//!      lease's branch, whose prerequisites the host already has;
//!   4. the bundle is fetched into `refs/aida/seat-import/<branch>`, never
//!      into `refs/heads`; the diff stat against the default branch and the
//!      clone's config/hooks report are printed, and the STORY-469 trailer
//!      check runs on the imported commits. A dry run fetches into a scratch
//!      repository instead and stops here, leaving no ref in main. A refusal
//!      here deletes the quarantine ref;
//!   5. the quarantined commit is pushed to `origin/<branch>`, the quarantine
//!      ref is deleted, and the PR is opened, titled with the commit subject.
//!
//! Every run first prunes quarantine refs older than a day (by their reflog's
//! import time), so an interrupted or failed run leaves nothing for long.
//!
//! trace:BUG-HOME-038 trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};
use colored::Colorize as _;

use crate::seat_clone::{HostRecordedClone, TreeKind};

/// Where an imported seat branch waits before it is pushed.
// trace:BUG-HOME-038 | ai:claude
pub(crate) const QUARANTINE_PREFIX: &str = "refs/aida/seat-import/";

/// `aida ship --from-seat` options.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct FromSeatOptions {
    /// The seat clone to publish from.
    pub(crate) clone: PathBuf,
    /// Explicit spec; `None` resolves it from the lease's branch or scope.
    pub(crate) spec: Option<String>,
    /// Push without opening the PR.
    pub(crate) no_pr: bool,
    /// Verify and report in a scratch repository; import and push nothing.
    pub(crate) dry_run: bool,
    /// TASK-2712: first commit the seat's pending work with this message,
    /// host-side in AIDA's own sandbox ([`crate::seat_clone::commit_pending`]),
    /// for a native Codex seat that cannot commit in its own sandbox.
    pub(crate) commit_pending: Option<String>,
}

/// `clone` as a seat clone `main` recorded, or the refusal.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn recorded_seat(main: &Path, clone: &Path) -> Result<HostRecordedClone> {
    aida_core::seat_clone_detect::note_host_main(main);
    let main = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    // trace:TASK-2032 | ai:claude
    crate::seat_clone::host_recorded_clone(&main, clone).ok_or_else(|| {
        anyhow::anyhow!(
            "refusing to publish from {}: it is not a seat clone this checkout recorded \
                 (no `clone` session lease in {} names it)",
            clone.display(),
            main.display()
        )
    })
}

/// The branch and scope of `seat`'s `clone` lease, read from the host-side
/// lease, never from the clone. The branch must be a plain branch name.
// trace:BUG-HOME-038 | ai:claude
fn lease_branch(seat: &HostRecordedClone) -> Result<(String, String)> {
    let lease = seat_lease(seat)?;
    Ok((lease.branch, lease.scope))
}

/// `seat`'s `clone` lease, read host-side, with its branch validated as a
/// plain branch name. An ended lease is gone, so it is refused here.
// trace:BUG-HOME-038 trace:TASK-2712 | ai:claude
fn seat_lease(seat: &HostRecordedClone) -> Result<crate::SessionLease> {
    let lease = crate::list_leases(&seat.main)
        .into_iter()
        .find(|lease| {
            lease.tree_kind == TreeKind::Clone
                && lease
                    .worktree_path
                    .canonicalize()
                    .is_ok_and(|p| p == seat.root)
        })
        .with_context(|| format!("the lease for {} disappeared", seat.root.display()))?;
    crate::git_arg_guard::reject_option_like("seat branch", &lease.branch)?;
    let valid = aida_core::git_ops::host_git_command(&seat.main)
        .args(["check-ref-format", "--branch", &lease.branch])
        .output()
        .is_ok_and(|o| o.status.success());
    if !valid || lease.branch.is_empty() {
        anyhow::bail!(
            "the seat's lease names `{}`, which is not a valid branch name",
            lease.branch.escape_debug()
        );
    }
    Ok(lease)
}

/// TASK-2712: what a host commit of a seat's pending work has checked before
/// any git runs, and the clone identity it must still have when it commits.
// trace:TASK-2712 | ai:claude
pub(crate) struct PendingCommit {
    pub(crate) seat: HostRecordedClone,
    pub(crate) branch: String,
    /// `(device, inode)` of the clone root when it was checked.
    pub(crate) pinned: (u64, u64),
}

/// TASK-2712: every refusal a host commit of `clone`'s pending work runs
/// BEFORE git touches the clone, so a refusal leaves nothing committed:
///
/// - `clone` itself is a symbolic link (only the real clone path is named);
/// - `clone` is not a seat clone `main` recorded (a foreign clone, a path
///   with no `clone` lease) or its lease has ended;
/// - the lease's worker is a live process other than this one (a competing
///   owner may still be writing);
/// - `message` has no `(SPEC-ID)` / `(Refs SPEC-ID)` trailer, names a spec
///   that does not resolve, or does not name `explicit_spec`.
// trace:TASK-2712 | ai:claude
pub(crate) fn pending_commit_preflight(
    main: &Path,
    clone: &Path,
    explicit_spec: Option<&str>,
    message: &str,
) -> Result<PendingCommit> {
    let meta = std::fs::symlink_metadata(clone)
        .with_context(|| format!("could not read the seat path {}", clone.display()))?;
    anyhow::ensure!(
        !meta.file_type().is_symlink(),
        "refusing to commit the seat's pending work: {} is a symbolic link; name the seat \
         clone itself. Nothing was committed",
        clone.display()
    );
    let seat = recorded_seat(main, clone)?;
    let lease = seat_lease(&seat).map_err(|e| {
        anyhow::anyhow!(
            "refusing to commit the seat's pending work: {e:#} (an ended lease no longer \
             vouches for the clone). Nothing was committed"
        )
    })?;
    if let Some(pid) = lease.active_pid {
        let own = crate::agent_launch::agent_codex_background::own_process_chain();
        anyhow::ensure!(
            own.contains(&pid)
                || !aida_core::liveness::process_identity_is_alive(
                    pid,
                    lease.active_pid_start_time.as_deref()
                ),
            "refusing to commit the seat's pending work in {}: its lease is held by live \
             process {pid}, which may still be writing. Let that lane finish (or end its \
             lease), then rerun. Nothing was committed",
            seat.root.display()
        );
    }
    pending_message_check(&seat.main, message, explicit_spec)?;
    let pinned = crate::seat_clone::dir_identity(&seat.root)?;
    Ok(PendingCommit {
        branch: lease.branch,
        seat,
        pinned,
    })
}

/// TASK-2712: the STORY-469 trailer rules applied to a commit message that
/// does not exist yet, against `main`'s store: [`seat_trailer_check`] and
/// [`explicit_spec_check`] on the message's subject.
// trace:TASK-2712 | ai:claude
pub(crate) fn pending_message_check(
    main: &Path,
    message: &str,
    explicit_spec: Option<&str>,
) -> Result<()> {
    let subject = message.lines().next().unwrap_or("").trim().to_string();
    anyhow::ensure!(
        !subject.is_empty(),
        "refusing to commit the seat's pending work with an empty message"
    );
    let store = crate::load_store_for_lookup(main);
    let commits = vec![("(pending)".to_string(), subject)];
    let trailer_ids = trailer_ids_checked(&commits, |id| {
        crate::trace::resolve_spec_in_store(store.as_ref(), id)
    })
    .map_err(|e| anyhow::anyhow!("{e:#}. Nothing was committed"))?;
    if let Some(explicit) = explicit_spec {
        explicit_spec_check(explicit, &trailer_ids)
            .map_err(|e| anyhow::anyhow!("{e:#}. Nothing was committed"))?;
    }
    Ok(())
}

/// TASK-2712: commit `clone`'s pending work with `message` through `commit`,
/// after [`pending_commit_preflight`], and only while the clone root is still
/// the directory that was checked (a replaced path is refused).
// trace:TASK-2712 | ai:claude
pub(crate) fn commit_seat_pending(
    main: &Path,
    clone: &Path,
    explicit_spec: Option<&str>,
    message: &str,
    commit: &dyn Fn(&Path, &Path, &str, &str) -> Result<Option<String>>,
) -> Result<Option<String>> {
    let plan = pending_commit_preflight(main, clone, explicit_spec, message)?;
    commit_planned(&plan, message, commit)
}

fn commit_planned(
    plan: &PendingCommit,
    message: &str,
    commit: &dyn Fn(&Path, &Path, &str, &str) -> Result<Option<String>>,
) -> Result<Option<String>> {
    crate::seat_clone::ensure_dir_identity(&plan.seat.root, plan.pinned)?;
    commit(&plan.seat.main, &plan.seat.root, &plan.branch, message)
}

/// TASK-2712: what the lane's post-exit step did with a seat's pending work.
// trace:TASK-2712 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PostExitCommit {
    /// The tree is not a seat clone this checkout recorded: nothing to do.
    NotASeat,
    /// The seat had nothing pending.
    Clean,
    /// The host committed the pending work as this commit.
    Committed(String),
    /// The commit was refused or failed; the work stays uncommitted.
    Failed(String),
}

/// The message the host commits a native lane's leftover work with. Only a
/// clean child exit (status 0) earns a completing `(SPEC)` trailer; the
/// leftovers of a crashed or failed lane are salvage, committed with
/// `(Refs SPEC)` so they never complete the spec at merge (advisor F4).
// trace:TASK-2712 | ai:claude
pub(crate) fn post_exit_message(spec: &str, lane: &str, clean_exit: bool) -> String {
    let spec = aida_core::node_alias::display_spec_id(spec);
    if clean_exit {
        format!("[AI:codex] chore(seat): work native Codex lane {lane} left uncommitted ({spec})")
    } else {
        format!(
            "[AI:codex] chore(seat): salvage work native Codex lane {lane} left uncommitted \
             after a failed exit (Refs {spec})"
        )
    }
}

/// TASK-2712 (advisor ruling F1): after a native Codex implementer's child
/// exits, the lane's own host-side step commits whatever it left pending in
/// its seat clone, with every [`pending_commit_preflight`] refusal, through
/// `commit` ([`crate::seat_clone::commit_pending`] in production: AIDA's own
/// bwrap job, hooks off, the host's identity). A tree that is not a recorded
/// seat clone is left alone. Publication stays a separate, gated step
/// (`aida ship --from-seat`).
// trace:TASK-2712 | ai:claude
pub(crate) fn post_exit_commit(
    main: &Path,
    tree: &Path,
    spec: &str,
    lane: &str,
    clean_exit: bool,
    commit: &dyn Fn(&Path, &Path, &str, &str) -> Result<Option<String>>,
) -> PostExitCommit {
    let main = crate::main_worktree_root_from(main);
    aida_core::seat_clone_detect::note_host_main(&main);
    // TASK-2032: anchored on the host main the lane recorded, never the cwd.
    // trace:TASK-2032 | ai:claude
    if crate::seat_clone::host_recorded_clone(&main, tree).is_none() {
        return PostExitCommit::NotASeat;
    }
    match commit_seat_pending(
        &main,
        tree,
        Some(spec),
        &post_exit_message(spec, lane, clean_exit),
        commit,
    ) {
        Ok(Some(tip)) => PostExitCommit::Committed(tip),
        Ok(None) => PostExitCommit::Clean,
        Err(e) => PostExitCommit::Failed(format!("{e:#}")),
    }
}

/// The lanes' post-exit step as a test sees it: `(main, tree, spec, lane,
/// clean_exit)`. A test installs one on its own thread through
/// [`with_post_exit_step`] to observe the step the lane itself runs.
#[cfg(test)]
pub(crate) type PostExitStep = dyn Fn(&Path, &Path, &str, &str, bool) -> PostExitCommit;

#[cfg(test)]
thread_local! {
    static POST_EXIT_STEP: std::cell::RefCell<Option<Box<PostExitStep>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `body` with `step` standing in for the production post-exit step on
/// this thread (the lane drivers call [`post_exit_commit_reported`] on the
/// thread that waited for the child).
// trace:TASK-2712 | ai:claude
#[cfg(test)]
pub(crate) fn with_post_exit_step<R>(step: Box<PostExitStep>, body: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            POST_EXIT_STEP.with(|s| *s.borrow_mut() = None);
        }
    }
    POST_EXIT_STEP.with(|s| *s.borrow_mut() = Some(step));
    let _reset = Reset;
    body()
}

/// [`post_exit_commit`] with the production commit, reported on stderr.
/// `clean_exit` is whether the child exited with status 0.
// trace:TASK-2712 | ai:claude
pub(crate) fn post_exit_commit_reported(
    main: &Path,
    tree: &Path,
    spec: &str,
    lane: &str,
    clean_exit: bool,
) -> PostExitCommit {
    #[cfg(test)]
    if let Some(outcome) = POST_EXIT_STEP.with(|s| {
        s.borrow()
            .as_ref()
            .map(|step| step(main, tree, spec, lane, clean_exit))
    }) {
        return outcome;
    }
    let outcome = post_exit_commit(
        main,
        tree,
        spec,
        lane,
        clean_exit,
        &crate::seat_clone::commit_pending,
    );
    match &outcome {
        PostExitCommit::Committed(tip) => eprintln!(
            "  committed the work lane {lane} left pending in {} as {tip} (host, hooks off). \
             Publish it with `aida ship --from-seat {}`",
            tree.display(),
            tree.display()
        ),
        PostExitCommit::Failed(why) => eprintln!(
            "  {} could not commit the work lane {lane} left pending in {}: {why}. The work \
             is still there, uncommitted",
            "warning:".yellow().bold(),
            tree.display()
        ),
        PostExitCommit::NotASeat | PostExitCommit::Clean => {}
    }
    outcome
}

/// Queue-work publication runs in the host after the child and test runner
/// have stopped, while the clone lease still vouches for the seat. Explicit
/// punts and holds retain their publication veto, including in drain/resume.
// trace:STORY-2475 | ai:codex
pub(crate) fn publish_queue_exit(
    main: &Path,
    tree: &Path,
    spec: &str,
    lane: &str,
    clean_exit: bool,
) -> Result<()> {
    let main = crate::main_worktree_root_from(main);
    let publish = || {
        run_at(
            &main,
            FromSeatOptions {
                clone: tree.to_path_buf(),
                spec: Some(spec.to_string()),
                no_pr: false,
                dry_run: false,
                commit_pending: None,
            },
        )
    };
    publish_queue_exit_with(
        &main,
        spec,
        || post_exit_commit_reported(&main, tree, spec, lane, clean_exit),
        || crate::seat_clone::probe(&main, tree),
        publish,
        || {
            advisor_request(
                &main,
                "Queue-work lane exited without changes",
                &format!(
                    "Codex queue-work lane {lane} for {spec} exited with no pending changes in {} \
                 (clean_exit={clean_exit}). Check its existing PR and failing checks; a still-red \
                 PR needs a new rework assignment.",
                    tree.display()
                ),
            )
        },
    )
}

// These effects are injected at the publication boundary so tests exercise
// the same veto, probe and dispatch as all three lane exit paths.
// trace:STORY-2475 | ai:codex
fn publish_queue_exit_with(
    main: &Path,
    spec: &str,
    commit: impl FnOnce() -> PostExitCommit,
    probe: impl FnOnce() -> crate::seat_clone::Probe,
    publish: impl Fn() -> Result<()>,
    notify: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if crate::punt::read_signal(&crate::punt::signal_path(main, spec)).is_some()
        || crate::punt::read_hold_signal(&crate::punt::hold_signal_path(main, spec)).is_some()
    {
        return Ok(());
    }
    finish_queue_exit(commit(), &publish, || {
        match probe() {
            crate::seat_clone::Probe::Known(facts) if facts.unpushed > 0 => return publish(),
            crate::seat_clone::Probe::Unknown(why)
            | crate::seat_clone::Probe::HasSubmodules(why) => {
                anyhow::bail!("could not inspect clean lane for pending publication: {why}")
            }
            _ => {}
        }
        notify()
    })
}

// trace:STORY-2475 | ai:codex
fn finish_queue_exit(
    outcome: PostExitCommit,
    publish: impl FnOnce() -> Result<()>,
    notify: impl FnOnce() -> Result<()>,
) -> Result<()> {
    match outcome {
        // Failed exits retain the existing salvage-only Refs trailer, but
        // the work is still published for review rather than stranded.
        PostExitCommit::Committed(_) => publish(),
        PostExitCommit::Clean => notify(),
        PostExitCommit::NotASeat => Ok(()),
        PostExitCommit::Failed(why) => anyhow::bail!("host post-exit commit failed: {why}"),
    }
}

/// Send through the ordinary mailbox resolver, with bounded CLI execution.
/// Queue the request when the advisor has no current reader.
// trace:STORY-2475 | ai:codex
pub(crate) fn advisor_request(main: &Path, subject: &str, message: &str) -> Result<()> {
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.current_dir(main).args([
        "mailbox",
        "send",
        "--to",
        "advisor",
        "--queue-for-role",
        "--intent",
        "request",
        "--subject",
        subject,
        "--",
        message,
    ]);
    match crate::command_output_with_timeout_detail(cmd, std::time::Duration::from_secs(20)) {
        crate::BoundedCommandOutput::Completed(out) if out.status.success() => Ok(()),
        crate::BoundedCommandOutput::Completed(out) => anyhow::bail!(
            "could not notify advisor: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
        _ => anyhow::bail!("advisor request did not finish: {message}"),
    }
}

#[cfg(test)]
mod queue_exit_tests {
    use super::*;
    // trace:STORY-2475 | ai:codex
    #[test]
    fn story_2475_punt_and_hold_veto_every_publication_effect() {
        for hold in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let main = tmp.path();
            let spec = "STORY-2475";
            if hold {
                crate::punt::write_hold_signal(
                    &crate::punt::hold_signal_path(main, spec),
                    &crate::punt::HoldSignal {
                        spec: spec.into(),
                        branch: "rework".into(),
                        reason: None,
                        spec_key: None,
                    },
                )
                .unwrap();
            } else {
                crate::punt::write_signal(
                    &crate::punt::signal_path(main, spec),
                    &crate::punt::PuntSignal {
                        spec: spec.into(),
                        category: aida_core::PuntCategory::DesignFork,
                        detail: "advisor decision needed".into(),
                        lean: None,
                    },
                )
                .unwrap();
            }
            publish_queue_exit_with(
                main,
                spec,
                || panic!("veto must precede commit"),
                || panic!("veto must precede probe"),
                || panic!("veto must precede publication"),
                || panic!("veto must precede no-change mail"),
            )
            .unwrap();
            // Also cover the public boundary with actual on-disk signals.
            publish_queue_exit(main, &main.join("missing-seat"), spec, "lane", true).unwrap();
        }
    }

    // trace:STORY-2475 | ai:codex
    #[test]
    fn story_2475_clean_unpushed_publishes_without_no_change_mail() {
        use crate::seat_clone::{Probe, ProbeFacts};
        use std::cell::Cell;
        let tmp = tempfile::tempdir().unwrap();
        for unpushed in [0, 1] {
            let published = Cell::new(0);
            let notified = Cell::new(0);
            publish_queue_exit_with(
                tmp.path(),
                "STORY-2475",
                || PostExitCommit::Clean,
                || {
                    Probe::Known(ProbeFacts {
                        unpushed,
                        ..Default::default()
                    })
                },
                || {
                    published.set(published.get() + 1);
                    Ok(())
                },
                || {
                    notified.set(notified.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(published.get(), unpushed);
            assert_eq!(notified.get(), usize::from(unpushed == 0));
        }
        for probe in [
            Probe::Unknown("probe failed".into()),
            Probe::HasSubmodules("submodules".into()),
        ] {
            let why = publish_queue_exit_with(
                tmp.path(),
                "STORY-2475",
                || PostExitCommit::Clean,
                || probe,
                || panic!("unknown must not publish"),
                || panic!("unknown must not claim no changes"),
            )
            .unwrap_err();
            assert!(why.to_string().contains("could not inspect clean lane"));
        }
        assert!(publish_queue_exit_with(
            tmp.path(),
            "STORY-2475",
            || PostExitCommit::Clean,
            || Probe::Known(ProbeFacts {
                unpushed: 1,
                ..Default::default()
            }),
            || anyhow::bail!("push refused"),
            || panic!("failed publication is not no changes")
        )
        .is_err());
    }

    #[test]
    fn story_2475_publication_and_notification_are_exclusive_and_fail_closed() {
        use std::cell::Cell;
        for (outcome, expected) in [
            (PostExitCommit::Committed("tip".into()), (true, false, true)),
            (PostExitCommit::Clean, (false, true, true)),
            (PostExitCommit::NotASeat, (false, false, true)),
            (
                PostExitCommit::Failed("refused".into()),
                (false, false, false),
            ),
        ] {
            let published = Cell::new(false);
            let notified = Cell::new(false);
            let result = finish_queue_exit(
                outcome,
                || {
                    published.set(true);
                    Ok(())
                },
                || {
                    notified.set(true);
                    Ok(())
                },
            );
            assert_eq!((published.get(), notified.get(), result.is_ok()), expected);
        }
        assert!(finish_queue_exit(
            PostExitCommit::Committed("tip".into()),
            || anyhow::bail!("push refused"),
            || panic!("must not notify clean exit")
        )
        .is_err());
        assert!(finish_queue_exit(
            PostExitCommit::Clean,
            || panic!("must not publish"),
            || anyhow::bail!("mail refused")
        )
        .is_err());
    }
}

/// The one head `git bundle list-heads` printed, when it is exactly
/// `refs/heads/<branch>` at a full object id; the refusal otherwise.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn single_branch_head(list_heads: &str, branch: &str) -> Result<String> {
    let heads: Vec<&str> = list_heads
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let want = format!("refs/heads/{branch}");
    let [line] = heads.as_slice() else {
        anyhow::bail!(
            "refusing the seat bundle: it carries {} refs, and only `{want}` is accepted",
            heads.len()
        );
    };
    let (oid, name) = line
        .split_once(' ')
        .context("refusing the seat bundle: its head list is malformed")?;
    let full_hex = matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit());
    if name != want || !full_hex {
        anyhow::bail!(
            "refusing the seat bundle: it carries `{}`, and only `{want}` is accepted",
            name.escape_debug()
        );
    }
    Ok(oid.to_string())
}

/// `git bundle verify` in `main` (every prerequisite must already be there),
/// then the one-ref check. Returns the branch tip.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn verify_bundle(main: &Path, bundle: &Path, branch: &str) -> Result<String> {
    let verify = aida_core::git_ops::host_git_command(main)
        .args(["bundle", "verify", "--quiet"])
        .arg(bundle)
        .output()
        .context("could not run `git bundle verify`")?;
    if !verify.status.success() {
        anyhow::bail!(
            "refusing the seat bundle: `git bundle verify` failed (are its prerequisites in \
             this repository?): {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        );
    }
    let heads = aida_core::git_ops::host_git_command(main)
        .args(["bundle", "list-heads"])
        .arg(bundle)
        .output()
        .context("could not run `git bundle list-heads`")?;
    if !heads.status.success() {
        anyhow::bail!(
            "refusing the seat bundle: `git bundle list-heads` failed: {}",
            String::from_utf8_lossy(&heads.stderr).trim()
        );
    }
    single_branch_head(&String::from_utf8_lossy(&heads.stdout), branch)
}

/// Fetch a verified bundle's branch into its quarantine ref in `main` and
/// check the ref landed on `tip`. Returns the quarantine ref.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn quarantine_bundle(
    main: &Path,
    bundle: &Path,
    branch: &str,
    tip: &str,
) -> Result<String> {
    let quarantine = format!("{QUARANTINE_PREFIX}{branch}");
    // The reflog records when the host imported the ref, which is what
    // [`prune_stale_quarantine`] ages it by (the commit date is the seat's).
    // trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
    // remote-creds-ok(TASK-2001): fetches from a local bundle file; contacts no remote.
    let fetch = aida_core::git_ops::host_git_command(main)
        .args(["-c", "core.logAllRefUpdates=always"])
        .args(["fetch", "--quiet", "--no-tags", "--no-write-fetch-head"])
        .arg(bundle)
        .arg(format!("+refs/heads/{branch}:{quarantine}"))
        .output()
        .context("could not run `git fetch` from the seat bundle")?;
    if !fetch.status.success() {
        anyhow::bail!(
            "could not fetch the seat bundle into {quarantine}: {}",
            String::from_utf8_lossy(&fetch.stderr).trim()
        );
    }
    let landed = git_text(main, &["rev-parse", "--verify", &quarantine])?;
    if landed != tip {
        let _ = git_text(main, &["update-ref", "-d", &quarantine, &landed]);
        anyhow::bail!("{quarantine} is at {landed}, not the bundle's {tip}");
    }
    Ok(quarantine)
}

/// How long a quarantine ref may outlive its run before
/// [`prune_stale_quarantine`] deletes it.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
pub(crate) const QUARANTINE_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// When `reference` was last written in `main`: its newest reflog entry
/// (the host's import time), else, for a ref with no reflog, its commit's
/// committer date. `None` when neither can be read.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
fn quarantine_written_at(main: &Path, reference: &str) -> Option<u64> {
    let reflog = git_text(
        main,
        &[
            "reflog",
            "show",
            "--date=unix",
            "--format=%gd",
            "-n1",
            reference,
        ],
    )
    .ok()
    .and_then(|line| {
        let stamp = line.rsplit_once("@{")?.1.strip_suffix('}')?;
        stamp.parse::<u64>().ok()
    });
    reflog.or_else(|| {
        git_text(main, &["log", "-1", "--format=%ct", reference, "--"])
            .ok()?
            .parse()
            .ok()
    })
}

/// Delete every `refs/aida/seat-import/*` ref in `main` written more than
/// `max_age` seconds before `now` (unix seconds): what a refused, failed or
/// interrupted `--from-seat` run left behind. Returns the deleted refs.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
pub(crate) fn prune_stale_quarantine(main: &Path, now: u64, max_age: u64) -> Vec<String> {
    let Ok(listed) = git_text(
        main,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            QUARANTINE_PREFIX,
        ],
    ) else {
        return Vec::new();
    };
    let mut pruned = Vec::new();
    for line in listed.lines() {
        let Some((oid, reference)) = line.split_once(' ') else {
            continue;
        };
        let stale = quarantine_written_at(main, reference)
            .is_some_and(|written| now.saturating_sub(written) > max_age);
        // Delete only if it still points where it did when listed, so a
        // concurrent run's fresh import of the same branch survives.
        if stale && git_text(main, &["update-ref", "-d", reference, oid]).is_ok() {
            pruned.push(reference.to_string());
        }
    }
    pruned
}

/// Delete a real run's quarantine ref after a refusal, and pass the refusal
/// on. A dry run's ref lives in its scratch repository and goes with it.
// trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
fn drop_quarantine_on_refusal<T>(staged: &Staged, dry_run: bool, result: Result<T>) -> Result<T> {
    if result.is_err() && !dry_run {
        if let Err(err) = git_text(
            &staged.repo,
            &["update-ref", "-d", &staged.reference, &staged.tip],
        ) {
            eprintln!(
                "{} could not delete {}: {err:#}",
                "warning:".yellow().bold(),
                staged.reference
            );
        }
    }
    result
}

/// Host git in `main`, stdout trimmed; an error carries stderr.
fn git_text(main: &Path, args: &[&str]) -> Result<String> {
    let out = aida_core::git_ops::host_git_command(main)
        .args(args)
        .output()
        .with_context(|| format!("could not run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A verified seat branch fetched into a quarantine ref: in main for a real
/// run, or in a scratch repository borrowing main's objects for a dry run, so
/// a dry run leaves no ref in main.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct Staged {
    /// The repository holding `reference`.
    pub(crate) repo: PathBuf,
    /// The quarantine ref.
    pub(crate) reference: String,
    /// The branch tip the bundle carried.
    pub(crate) tip: String,
    /// The dry run's scratch repository, removed on drop.
    _scratch: Option<tempfile::TempDir>,
}

/// Verify the seat's bundle `bytes` against `main`, then fetch it into
/// quarantine: main's `refs/aida/seat-import/<branch>`, or with `dry_run` the
/// same ref in a scratch bare repository whose alternates borrow main's
/// objects (`git bundle verify` and that fetch write nothing to main).
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn stage_bundle(
    main: &Path,
    branch: &str,
    bytes: &[u8],
    dry_run: bool,
) -> Result<Staged> {
    let mut bundle = tempfile::Builder::new()
        .prefix("aida-from-seat-")
        .suffix(".bundle")
        .tempfile()
        .context("could not create a temporary bundle file")?;
    std::io::Write::write_all(&mut bundle, bytes).context("could not write the seat bundle")?;
    let tip = verify_bundle(main, bundle.path(), branch)?;
    if !dry_run {
        let reference = quarantine_bundle(main, bundle.path(), branch, &tip)?;
        return Ok(Staged {
            repo: main.to_path_buf(),
            reference,
            tip,
            _scratch: None,
        });
    }
    let scratch = tempfile::Builder::new()
        .prefix("aida-from-seat-dry-run-")
        .tempdir()
        .context("could not create the dry run's scratch repository")?;
    let init = aida_core::git_ops::host_git_command(scratch.path())
        .args(["init", "--bare", "--quiet"])
        .output()
        .context("could not run `git init` for the dry run")?;
    if !init.status.success() {
        anyhow::bail!(
            "could not create the dry run's scratch repository: {}",
            String::from_utf8_lossy(&init.stderr).trim()
        );
    }
    let objects = main
        .join(".git")
        .join("objects")
        .canonicalize()
        .context("could not resolve main's object directory")?;
    std::fs::write(
        scratch
            .path()
            .join("objects")
            .join("info")
            .join("alternates"),
        format!("{}\n", objects.display()),
    )
    .context("could not point the dry run's scratch repository at main's objects")?;
    let reference = quarantine_bundle(scratch.path(), bundle.path(), branch, &tip)?;
    Ok(Staged {
        repo: scratch.path().to_path_buf(),
        reference,
        tip,
        _scratch: Some(scratch),
    })
}

/// Import a recorded seat's branch into main's quarantine: bundle it in the
/// seat sandbox, verify it, fetch it. Returns the quarantine ref and the tip.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn import_seat_branch(
    seat: &HostRecordedClone,
    branch: &str,
) -> Result<(String, String)> {
    let bytes = crate::seat_clone::bundle_branch(&seat.main, &seat.root, branch)?;
    let staged = stage_bundle(&seat.main, branch, &bytes, false)?;
    Ok((staged.reference, staged.tip))
}

/// The STORY-469 commit-trailer check on the seat's commits in `range`
/// (read in `repo`): some commit names a spec, either in a completing
/// `(SPEC-ID)` trailer or a non-completing `(Refs SPEC-ID)` group (the shape
/// of a partial slice of a multi-PR spec, BUG-2431), and every named ID
/// resolves to a live spec through `resolve`. A seat branch with no commit in
/// `range` has nothing to publish. Returns every named ID. Accepting `Refs`
/// here never completes the spec: merge-time completion still skips `Refs`.
// trace:BUG-HOME-038 trace:STORY-469 trace:BUG-2431 | ai:claude
pub(crate) fn seat_trailer_check<F>(repo: &Path, range: &str, resolve: F) -> Result<Vec<String>>
where
    F: FnMut(&str) -> crate::trace::SpecResolution,
{
    let commits = crate::trace::read_commits_in_range(repo, range)?;
    if commits.is_empty() {
        anyhow::bail!("refusing to publish: the seat branch adds no commit ({range})");
    }
    trailer_ids_checked(&commits, resolve)
}

/// [`seat_trailer_check`]'s rules on `(sha, subject)` pairs: some subject
/// names a spec, and every named ID resolves to a live spec.
// trace:BUG-HOME-038 trace:BUG-2431 trace:TASK-2712 | ai:claude
fn trailer_ids_checked<F>(commits: &[(String, String)], mut resolve: F) -> Result<Vec<String>>
where
    F: FnMut(&str) -> crate::trace::SpecResolution,
{
    let mut trailer_ids: Vec<String> = Vec::new();
    let mut refs_only: Vec<(&String, &String, String)> = Vec::new();
    for (sha, subject) in commits {
        let completing = crate::extract_spec_ids_from_commit(subject);
        for id in crate::pr_ship::extract_authorship_spec_ids_from_subject(subject) {
            if !completing
                .iter()
                .any(|c| aida_core::id_identity::same_id(c, &id))
            {
                trailer_ids.push(id.clone());
                refs_only.push((sha, subject, id));
            }
        }
        trailer_ids.extend(completing);
    }
    if trailer_ids.is_empty() {
        anyhow::bail!(
            "refusing to publish: no seat commit (or `--commit-pending` message) carries a \
             `(SPEC-ID)` or `(Refs SPEC-ID)` trailer. Fix the message, or amend the commit \
             inside the seat, then publish again"
        );
    }
    let mut violations = crate::trace::validate_trailer_references(commits, &mut resolve);
    // The STORY-469 guard reads completing trailers only; a `Refs` ID must
    // name a live spec too. // trace:BUG-2431 | ai:claude
    for (sha, subject, id) in refs_only {
        let verdict = match resolve(&id) {
            crate::trace::SpecResolution::Live => continue,
            crate::trace::SpecResolution::Rejected => crate::trace::TrailerVerdict::Rejected,
            crate::trace::SpecResolution::Missing => crate::trace::TrailerVerdict::Nonexistent,
        };
        violations.push(crate::trace::TrailerViolation {
            sha: sha.clone(),
            subject: subject.clone(),
            spec_id: id,
            verdict,
        });
    }
    if !violations.is_empty() {
        anyhow::bail!(
            "{}",
            crate::trace::format_trailer_guard_refusal("ship --from-seat", &violations).join("\n")
        );
    }
    Ok(trailer_ids)
}

/// An explicit `aida ship <SPEC> --from-seat` spec must be one the seat's
/// commits name in their trailers (aliases expanded, case ignored); a branch
/// whose commits name another spec is refused rather than published under
/// the wrong one.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn explicit_spec_check(explicit: &str, trailer_ids: &[String]) -> Result<()> {
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — numeric identity.
    let canonical =
        |id: &str| aida_core::id_identity::id_key(&aida_core::node_alias::expand_or_keep(id));
    let want = canonical(explicit);
    if trailer_ids.iter().any(|id| canonical(id) == want) {
        return Ok(());
    }
    let mut named: Vec<String> = trailer_ids
        .iter()
        .map(|id| aida_core::node_alias::display_spec_id(id))
        .collect();
    named.sort();
    named.dedup();
    anyhow::bail!(
        "refusing to publish: you named {}, but the seat's commits name {}. Drop the spec \
         argument, or fix the commit trailer inside the seat",
        aida_core::node_alias::display_spec_id(explicit),
        named.join(", ")
    )
}

/// What `aida ship --from-seat` needs from outside the repository, so a test
/// can drive the handler without bwrap, a forge or a requirement store.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct Deps<'a> {
    /// Bundle `refs/heads/<branch>` of the clone (`main`, `clone`, `branch`).
    pub(crate) bundle: &'a dyn Fn(&Path, &Path, &str) -> Result<Vec<u8>>,
    /// Commit the clone's pending work (`main`, `clone`, `branch`,
    /// `message`); the new tip, or `None` when there was nothing to commit.
    // trace:TASK-2712 | ai:claude
    pub(crate) commit: &'a dyn Fn(&Path, &Path, &str, &str) -> Result<Option<String>>,
    /// The forge to open the PR on, for `main`.
    pub(crate) forge: &'a dyn Fn(&Path) -> Box<dyn crate::forge::Forge>,
    /// Called with `main` and the branch once the PR is open.
    pub(crate) after_open: &'a dyn Fn(&Path, &str),
}

/// The `aida ship --from-seat` handler.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn run(opts: FromSeatOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    run_at(&cwd, opts)
}

// trace:STORY-2475 | ai:codex
pub(crate) fn run_at(cwd: &Path, opts: FromSeatOptions) -> Result<()> {
    run_from(
        cwd,
        opts,
        &Deps {
            bundle: &crate::seat_clone::bundle_branch,
            commit: &crate::seat_clone::commit_pending,
            forge: &|main| crate::forge::forge_for_open_change(main),
            after_open: &|main, branch| {
                let _ = crate::queue_review_for_open_pr(main, branch);
            },
        },
    )
}

/// [`run`] from `cwd`.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn run_from(cwd: &Path, opts: FromSeatOptions, deps: &Deps<'_>) -> Result<()> {
    if let Some(clone) = aida_core::seat_clone_detect::seat_clone_root(cwd) {
        anyhow::bail!(
            "`aida ship --from-seat` runs from the main checkout, not from inside the seat \
             clone {}",
            clone.display()
        );
    }
    let project_root = crate::find_project_root_from(cwd)?;
    let main = crate::main_worktree_root_from(&project_root);
    let seat = recorded_seat(&main, &opts.clone)?;
    let main = seat.main.clone();
    let (branch, scope) = lease_branch(&seat)?;
    let spec = crate::ship::resolve_spec(opts.spec.as_deref(), &branch, Some(&scope)).ok_or_else(
        || {
            anyhow::anyhow!(
                "could not resolve a spec from the seat's branch `{branch}` — pass one \
                 explicitly: `aida ship <SPEC> --from-seat {}`",
                seat.root.display()
            )
        },
    )?;
    eprintln!(
        "{} aida ship --from-seat — {} on branch {} from {}{}",
        "→".cyan().bold(),
        aida_core::node_alias::display_spec_id(&spec).bold(),
        branch,
        seat.root.display(),
        if opts.dry_run { " (dry run)" } else { "" }
    );
    for line in crate::seat_clone::config_hooks_report(&main, &seat.root) {
        eprintln!("  seat {line}");
    }
    // trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for pruned in prune_stale_quarantine(&main, now, QUARANTINE_MAX_AGE_SECS) {
        eprintln!("  pruned stale {pruned}");
    }

    // TASK-2712: the host commits a native Codex seat's pending work first,
    // in AIDA's own sandbox with the seat's hooks off. Every refusal (seat,
    // lease, live owner, the message's trailer and the explicit spec) runs
    // BEFORE the commit, a dry run included, so a refused message leaves
    // nothing committed. The trailer check below then judges the commit like
    // any other seat commit.
    // trace:TASK-2712 | ai:claude
    if let Some(message) = &opts.commit_pending {
        let plan = pending_commit_preflight(&main, &opts.clone, opts.spec.as_deref(), message)?;
        if opts.dry_run {
            eprintln!(
                "  dry run: the message and seat checks pass; would commit the seat's pending \
                 work first; committed nothing"
            );
        } else {
            match commit_planned(&plan, message, deps.commit)? {
                Some(tip) => eprintln!("  committed the seat's pending work as {tip} (hooks off)"),
                None => eprintln!("  the seat had no pending work to commit"),
            }
        }
    }
    let bytes = (deps.bundle)(&main, &seat.root, &branch)?;
    let staged = stage_bundle(&main, &branch, &bytes, opts.dry_run)?;
    let checked = check_staged(&main, &staged, &opts);
    drop_quarantine_on_refusal(&staged, opts.dry_run, checked)?;
    let (quarantine, tip) = (&staged.reference, &staged.tip);
    if opts.dry_run {
        eprintln!(
            "  dry run: would push {tip} to origin/{branch}{}; nothing was imported",
            if opts.no_pr { "" } else { " and open the PR" }
        );
        return Ok(());
    }

    eprintln!("  pushing {tip} to origin/{branch}");
    aida_core::git_ops::clear_sandbox_config_lock(&main);
    let push = aida_core::git_ops::host_git_command(&main)
        .args([
            "push",
            "origin",
            &format!("{quarantine}:refs/heads/{branch}"),
        ])
        .remote_credentials(&main, "origin")
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!(
            "`git push origin {quarantine}:refs/heads/{branch}` failed — investigate before \
             retrying"
        );
    }
    // Pushed: the quarantine ref has done its job (a failure above keeps it
    // for investigation; a later run prunes it once stale).
    git_text(&main, &["update-ref", "-d", quarantine, tip])?;
    finish_published(&main, &seat, &branch, &spec, tip, opts.no_pr, deps)
}

/// Everything [`run_from`] checks on a staged import before it pushes: the
/// diff stat against the default branch, the STORY-469 trailer check and an
/// explicit spec's match. A refusal here deletes the quarantine ref.
// trace:BUG-HOME-038 trace:TASK-B6B1278890774EF993D2EBC327862470-138 | ai:claude
fn check_staged(main: &Path, staged: &Staged, opts: &FromSeatOptions) -> Result<()> {
    let (quarantine, tip) = (&staged.reference, &staged.tip);
    eprintln!(
        "  {} bundle verified ({tip}){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        if opts.dry_run {
            " into a scratch repository"
        } else {
            ""
        }
    );
    let base = crate::detect_default_branch_ref(main).unwrap_or_else(|| "origin/main".into());
    let base_oid = git_text(
        main,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let stat = git_text(
        &staged.repo,
        &["diff", "--stat", &format!("{base_oid}...{quarantine}")],
    )?;
    eprintln!("  changes against {base}:");
    for line in stat.lines() {
        eprintln!("    {line}");
    }
    let store = crate::load_store_for_lookup(main);
    if store.is_none() {
        eprintln!(
            "{} ship --from-seat: no requirement store reachable; trailer spec-IDs are not \
             resolved",
            "warning:".yellow().bold()
        );
    }
    let trailer_ids =
        seat_trailer_check(&staged.repo, &format!("{base_oid}..{quarantine}"), |id| {
            crate::trace::resolve_spec_in_store(store.as_ref(), id)
        })?;
    if let Some(explicit) = opts.spec.as_deref() {
        explicit_spec_check(explicit, &trailer_ids)?;
    }
    Ok(())
}

/// After the push: open (or find) the PR unless `no_pr`.
// trace:BUG-HOME-038 | ai:claude
fn finish_published(
    main: &Path,
    seat: &HostRecordedClone,
    branch: &str,
    spec: &str,
    tip: &str,
    no_pr: bool,
    deps: &Deps<'_>,
) -> Result<()> {
    if no_pr {
        eprintln!(
            "{} aida ship --from-seat — {branch} pushed (--no-pr; no PR opened)",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        return Ok(());
    }

    let forge = (deps.forge)(main);
    // Only a clean "no open change" opens one: a failed lookup could hide an
    // open PR, and opening another would duplicate it.
    let pr = match forge.change_for_branch(branch) {
        Ok(crate::forge::ChangeLookup::Found(change)) => {
            eprintln!("  found open PR-{} for branch {branch}", change.id);
            change.id
        }
        Ok(crate::forge::ChangeLookup::NoChange) => open_seat_pr(&*forge, main, seat, branch, tip)?,
        Ok(other) => anyhow::bail!(
            "{branch} was pushed, but looking up its open PR failed ({}); no PR was opened. \
             Check the forge CLI, then run `aida ship --from-seat {}` again",
            describe_lookup(&other),
            seat.root.display()
        ),
        Err(err) => {
            return Err(err.context(format!(
                "{branch} was pushed, but looking up its open PR failed; no PR was opened"
            )))
        }
    };
    (deps.after_open)(main, branch);
    // Dual-write, host side: the contained seat never writes `.aida/` itself
    // (A6); the host that verified and pushed its bundle records the
    // handoff. // trace:STORY-2325 | ai:claude
    crate::ownership::dual::handed_off_from_seat(main, spec, tip, pr);
    eprintln!(
        "{} aida ship --from-seat — PR-{pr} open for {} (not merged)",
        "⏸".yellow().bold(),
        aida_core::node_alias::display_spec_id(spec).bold()
    );
    Ok(())
}

/// Why a lookup that neither found nor ruled out an open change failed.
fn describe_lookup(lookup: &crate::forge::ChangeLookup) -> String {
    use crate::forge::ChangeLookup;
    match lookup {
        ChangeLookup::CliMissing => "the forge CLI is not on PATH".to_string(),
        ChangeLookup::CliFailed(why) => format!("the forge CLI failed: {}", why.trim()),
        ChangeLookup::Unreachable(why) => format!("the forge is unreachable: {}", why.trim()),
        ChangeLookup::Found(_) | ChangeLookup::NoChange => "no failure".to_string(),
    }
}

/// The PR body for a published seat commit: the commit body, then the note
/// that it was published from the seat by the host.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn seat_pr_body(message: &str, seat_name: &str) -> String {
    let mut body = crate::pr_ship::derive_pr_body_from_commit(message);
    if !body.trim().is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(&format!(
        "Published from seat `{seat_name}` with `aida ship --from-seat`: bundled inside the \
         seat sandbox, verified and pushed by the host; no seat hook ran."
    ));
    body
}

/// Open the PR for a published seat branch at `tip`: titled with the commit
/// subject, its body [`seat_pr_body`].
// trace:BUG-HOME-038 | ai:claude
fn open_seat_pr(
    forge: &dyn crate::forge::Forge,
    main: &Path,
    seat: &HostRecordedClone,
    branch: &str,
    tip: &str,
) -> Result<u64> {
    let message = git_text(main, &["log", "-1", "--format=%B", tip])?;
    let title = git_text(main, &["log", "-1", "--format=%s", tip])?;
    if title.is_empty() {
        anyhow::bail!("the seat's commit has an empty subject; cannot title the PR");
    }
    let seat_name = seat
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let specs = crate::extract_spec_ids_from_commit(&message);
    let req = crate::forge::OpenChange {
        branch: branch.to_string(),
        base: crate::forge::default_branch_of(main),
        title,
        body: seat_pr_body(&message, &seat_name),
        draft: false,
    };
    let change = crate::gate::ledger::open_change_bound(forge, main, req, &specs)
        .context("could not open a forge change request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the PR was created but its number could not be read: {}",
            change.url
        );
    }
    eprintln!(
        "  {} created PR-{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        change.id
    );
    Ok(change.id)
}
