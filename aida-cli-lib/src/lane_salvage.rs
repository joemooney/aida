//! BUG-2781: salvage-push a lane's committed work when the lane ends non-zero.
//!
//! A lane killed after it committed (a signal mid final gate, a crash, a
//! timeout) used to leave a finished fix unpublished. Pushing before the gate
//! (the lane job contract, AC2) is the preferred fix; this is the backstop.
//! The host, from its own trusted git and never the seat clone's config,
//! bundles the claimed branch alone, quarantines it in a host ref, checks the
//! push is a fast-forward, pushes and drops the host ref. The target is the
//! claimed branch when it already has an open PR; otherwise it is
//! `salvage/<lane>`, a branch no PR, review or train reads. Uncommitted edits,
//! a non-fast-forward and any other branch are never pushed; the seat keeps
//! its work. A salvaged head carries no review: the merge train needs a fresh
//! CI run and an independent review at that exact head.
//! trace:BUG-2781 | ai:claude

use std::path::Path;

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};

use crate::headless_background::SeatState;
use crate::standalone_implementer::Outcome;

/// How every salvage is announced.
pub(crate) const UNVERIFIED: &str = "salvaged, unverified: final gate incomplete";

/// A seat's bundler: `(main, clone, branch)` to a one-ref bundle.
pub(crate) type Bundle<'a> = &'a dyn Fn(&Path, &Path, &str) -> Result<Vec<u8>>;

/// Whether a lane's end leaves committed, unpushed work to salvage: it
/// exited non-zero, was not punted or held, and its seat `HEAD` moved past
/// the launch head with commits on no remote. Anything git cannot read is no.
pub(crate) fn wanted(
    outcome: &Outcome,
    success: bool,
    seat: &SeatState,
    launch_head: Option<&str>,
) -> bool {
    !success
        && matches!(outcome, Outcome::NoPr | Outcome::PrOpened { .. })
        && seat.head.as_deref().is_some_and(|h| launch_head != Some(h))
        && seat.pushed_since(launch_head) == Some(false)
        && seat.head_has_unpushed == Some(true)
}

/// The branch a salvage push targets: the claimed branch when it has an
/// open PR, otherwise `salvage/<lane>`.
pub(crate) fn target(branch: &str, open_pr: Option<u64>, lane: &str) -> String {
    match open_pr {
        Some(_) => branch.to_owned(),
        None => format!("salvage/{lane}"),
    }
}

/// What the lane's exit status says ended it: the signal and, as far as the
/// launcher can know, its source (AC3).
pub(crate) fn exit_cause(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    // A shell wrapper reports a signal it died of as 128 + the signal.
    let signal = signal.or(status
        .code()
        .filter(|c| (129..=192).contains(c))
        .map(|c| c - 128));
    let Some(sig) = signal else {
        return format!("exited with code {}", status.code().unwrap_or(-1));
    };
    let name = match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    };
    let source = match sig {
        9 => "not recorded by the launcher; the kernel OOM killer or an operator (check `journalctl -k`)",
        2 | 15 => "not recorded by the launcher; an operator, a supervisor or a session stop",
        _ => "not recorded by the launcher",
    };
    format!("ended by {name} ({sig}); source: {source}")
}

/// Push the seat's committed `branch` to `origin/<target>` as a
/// fast-forward, from host git only; returns the pushed tip. Refuses a seat
/// this checkout did not record, a branch its lease does not own, a punt or
/// hold, and a push that is not a fast-forward. The host import ref is
/// always dropped.
pub(crate) fn push(
    main: &Path,
    clone: &Path,
    spec: &str,
    branch: &str,
    target: &str,
    bundle: Bundle<'_>,
) -> Result<String> {
    let seat = crate::ship_from_seat::recorded_seat(main, clone)?;
    let main = seat.main.clone();
    crate::ship_from_seat::require_publication_branch(&main, &seat.root, branch)?;
    crate::ship_from_seat::refuse_publication_veto(&main, &seat.root, spec)?;
    let bytes = bundle(&main, &seat.root, branch)?;
    let staged = crate::ship_from_seat::stage_bundle(&main, branch, &bytes, false)?;
    let pushed = push_fast_forward(&main, &staged.reference, &staged.tip, target);
    let dropped = crate::ship_from_seat::git_text(
        &main,
        &["update-ref", "-d", &staged.reference, &staged.tip],
    );
    pushed?;
    if let Err(e) = dropped {
        eprintln!("implementer: could not drop {}: {e:#}", staged.reference);
    }
    Ok(staged.tip)
}

fn push_fast_forward(main: &Path, import: &str, tip: &str, target: &str) -> Result<()> {
    if let Some(remote) = crate::ship_from_seat::pin_remote_branch(main, target)? {
        let ancestry = aida_core::git_ops::host_git_command(main)
            .args(["merge-base", "--is-ancestor", &remote.tip, tip])
            .status()
            .context("could not check the fast-forward; nothing pushed")?;
        match ancestry.code() {
            Some(0) => {}
            Some(1) => anyhow::bail!(
                "{tip} does not fast-forward origin/{target} ({}); nothing pushed",
                remote.tip
            ),
            _ => anyhow::bail!("could not check the fast-forward; nothing pushed"),
        }
    }
    aida_core::git_ops::clear_sandbox_config_lock(main);
    // No `+`: origin refuses a non-fast-forward that raced the check above.
    let push = aida_core::git_ops::host_git_command(main)
        .args(["push", "origin", &format!("{import}:refs/heads/{target}")])
        .remote_credentials(main, "origin")
        .status()
        .context("could not invoke `git push`")?;
    anyhow::ensure!(
        push.success(),
        "`git push origin {import}:refs/heads/{target}` failed; nothing salvaged"
    );
    Ok(())
}

/// The announcement for a salvaged head, for the spec, the PR and the log.
pub(crate) fn announcement(tip: &str, target: &str, cause: &str) -> String {
    format!(
        "{UNVERIFIED} ({cause}). The host pushed the lane's committed head {tip} to \
         `{target}`. It is not merge-train eligible until a fresh CI run and an independent \
         review at that head."
    )
}

/// The lane a failed exit may salvage.
pub(crate) struct Lane<'a> {
    pub(crate) main: &'a Path,
    pub(crate) clone: &'a Path,
    pub(crate) spec: &'a str,
    pub(crate) branch: &'a str,
    pub(crate) lane: &'a str,
    pub(crate) launch_head: Option<&'a str>,
    pub(crate) open_pr: Option<u64>,
}

/// A failed lane exit's salvage: push its committed work when [`wanted`],
/// announce it through `announce(text, open_pr)`, and return the note for
/// the lane's log and result, or `None` when there was nothing to salvage.
/// A refusal keeps the commits in the seat and says why.
pub(crate) fn on_failed_exit(
    lane: &Lane<'_>,
    outcome: &Outcome,
    seat: &SeatState,
    cause: &str,
    bundle: Bundle<'_>,
    announce: &dyn Fn(&str, Option<u64>),
) -> Option<String> {
    if !wanted(outcome, false, seat, lane.launch_head) {
        return None;
    }
    let target = target(lane.branch, lane.open_pr, lane.lane);
    match push(
        lane.main,
        lane.clone,
        lane.spec,
        lane.branch,
        &target,
        bundle,
    ) {
        Ok(tip) => {
            let text = announcement(&tip, &target, cause);
            eprintln!("implementer: {text}");
            announce(&text, lane.open_pr);
            Some(format!("{UNVERIFIED}: pushed {tip} to {target}"))
        }
        Err(e) => {
            eprintln!("implementer: salvage-push refused: {e:#}");
            Some(format!(
                "salvage-push refused, commits retained in the seat: {e:#}"
            ))
        }
    }
}

/// Post `text` on the spec and, when there is one, on its open PR. Best
/// effort: the push already happened; a failed note warns.
pub(crate) fn announce(main: &Path, spec: &str, branch: &str, text: &str, pr: Option<u64>) {
    let store =
        crate::detect_distributed_store_from(main).unwrap_or_else(|| main.join(".aida-store"));
    let storage = crate::Storage::new(store);
    if let Err(e) =
        crate::comment_cmd::add_comment_cli(&storage, spec, text, Some("aida-host"), None)
    {
        eprintln!("implementer: could not note the salvage on {spec}: {e:#}");
    }
    if let Some(id) = pr {
        let change = crate::forge::ChangeRef {
            id,
            url: String::new(),
            branch: branch.to_owned(),
            base: String::new(),
            title: None,
        };
        if let Err(e) = crate::forge::forge_for_open_change(main).comment(&change, text) {
            eprintln!("implementer: could not note the salvage on PR #{id}: {e:#}");
        }
    }
}

#[cfg(test)]
#[path = "tests/bug_2781_lane_salvage_unit_tests.rs"]
mod unit_tests;
