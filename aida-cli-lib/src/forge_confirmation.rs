//! Confirmation boundary for destructive forge-driven lifecycle decisions.
// trace:STORY-2401 | ai:codex
use crate::forge::{ChangeMetadata, ChangeState, Forge};
use crate::forge_observed::{confirm_snapshot, Observed};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

/// Milliseconds between the paired reads of a confirmation (default 3s).
/// Binary-level tests set it to 0 on the spawned `aida` so every confirmed
/// read does not pay a real pause; this crate's own unit tests default to 0
/// and inject explicit pauses where the pause matters. trace:STORY-2401 | ai:claude
pub(crate) const PAUSE_ENV: &str = "AIDA_FORGE_CONFIRM_PAUSE_MS";
const DEFAULT_PAUSE: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(3)
};

pub(crate) fn pause_from(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_PAUSE)
}

pub(crate) fn pause() -> Duration {
    pause_from(std::env::var(PAUSE_ENV).ok().as_deref())
}

pub(crate) fn backoff() {
    let pause = pause();
    if !pause.is_zero() {
        std::thread::sleep(pause);
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;

    // trace:STORY-2401 | ai:claude
    #[test]
    fn story_2401_confirmation_pause_is_configurable() {
        assert_eq!(pause_from(Some("0")), Duration::ZERO);
        assert_eq!(pause_from(Some(" 250 ")), Duration::from_millis(250));
        // Unset or unparsable falls back to the default (zero in unit tests).
        assert_eq!(pause_from(None), DEFAULT_PAUSE);
        assert_eq!(pause_from(Some("soon")), DEFAULT_PAUSE);
    }
}

fn git(root: &Path, args: &[&str], remote: bool) -> Result<Output> {
    // Confirmation fetches are nested train forge calls too. Persist intent
    // again before each base/head fetch, including absent-head recovery.
    // trace:STORY-2401 | ai:codex
    if remote {
        crate::pr_train::before_train_forge_call("confirmation Git fetch")?;
    }
    // Retain the private-seat clone refusal even for evidence-only fetches.
    let mut cmd = aida_core::git_ops::host_git_command(root);
    cmd.args(args).stdin(Stdio::null());
    if remote {
        aida_core::git_ops::configure_remote_credentials(&mut cmd, root, "origin");
    }
    crate::command_output_with_timeout(cmd, Duration::from_secs(30))
        .context("confirmation Git read failed or exceeded 30 seconds")
}

fn stdout(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root, args, false)?;
    anyhow::ensure!(
        out.status.success(),
        "confirmation Git evidence unavailable"
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

// A sweep owns its fetched tips. No result survives into another command/sweep.
// trace:STORY-2401 | ai:codex
type Tips = std::collections::HashMap<(std::path::PathBuf, String), Observed<String>>;
thread_local! {
    static TIPS: std::cell::RefCell<Option<Tips>> = const { std::cell::RefCell::new(None) };
}
pub(crate) struct Sweep(bool);
impl Sweep {
    pub(crate) fn new() -> Self {
        Self(TIPS.with(|tips| {
            let mut tips = tips.borrow_mut();
            if tips.is_some() {
                false
            } else {
                *tips = Some(Tips::new());
                true
            }
        }))
    }
}
impl Drop for Sweep {
    fn drop(&mut self) {
        if self.0 {
            TIPS.with(|tips| *tips.borrow_mut() = None);
        }
    }
}

fn base_tip(root: &Path, base: &str) -> Observed<String> {
    let key = (root.to_path_buf(), base.to_string());
    if let Some(tip) = TIPS.with(|tips| tips.borrow().as_ref().and_then(|t| t.get(&key).cloned())) {
        return tip;
    }
    let tip = Observed::from_result((|| -> Result<String> {
        let source = format!("refs/heads/{base}");
        anyhow::ensure!(
            git(root, &["check-ref-format", &source], false)?
                .status
                .success(),
            "invalid base branch"
        );
        let private = format!("refs/aida/confirm/{}", uuid::Uuid::new_v4());
        let spec = format!("+{source}:{private}");
        let result = (|| {
            // Disable opportunistic tracking-ref updates even when base moved.
            // trace:BUG-2463 | ai:codex
            let fetched = git(
                root,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--refmap=",
                    "origin",
                    &spec,
                ],
                true,
            )?;
            anyhow::ensure!(fetched.status.success(), "confirmation base fetch failed");
            stdout(root, &["rev-parse", "--verify", &private])
        })();
        let _ = git(root, &["update-ref", "-d", &private], false);
        result
    })());
    // Only a fetched tip is shared across the sweep: one transient blip must
    // not leave every later candidate unconfirmed. trace:STORY-2401 | ai:claude
    if matches!(tip, Observed::Known(_)) {
        TIPS.with(|tips| {
            if let Some(tips) = tips.borrow_mut().as_mut() {
                tips.insert(key, tip.clone());
            }
        });
    }
    tip
}

/// Recover only the pinned object, without touching FETCH_HEAD, tracking refs
/// or the checkout. The shared Git runner bounds the fetch to 30 seconds and
/// retains the private-seat refusal. Never substitute a moving branch head.
// trace:BUG-2463 | ai:codex
pub(crate) fn rebase_landed(root: &Path, base: &str, head: &str) -> Observed<bool> {
    base_tip(root, base).and_then(|tip| {
        Observed::from_result((|| -> Result<bool> {
            anyhow::ensure!(
                matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid pinned head"
            );
            if !git(
                root,
                &["cat-file", "-e", &format!("{head}^{{commit}}")],
                false,
            )?
            .status
            .success()
            {
                let private = format!("refs/aida/confirm/{}", uuid::Uuid::new_v4());
                let spec = format!("+{head}:{private}");
                let result = (|| -> Result<()> {
                    let fetched = git(
                        root,
                        &[
                            "fetch",
                            "--no-tags",
                            "--no-write-fetch-head",
                            "--refmap=",
                            "origin",
                            &spec,
                        ],
                        true,
                    )?;
                    anyhow::ensure!(
                        fetched.status.success(),
                        "confirmation pinned head fetch failed"
                    );
                    anyhow::ensure!(
                        stdout(
                            root,
                            &["rev-parse", "--verify", &format!("{private}^{{commit}}")]
                        )?
                        .eq_ignore_ascii_case(head),
                        "confirmation fetched head differs from pinned head"
                    );
                    Ok(())
                })();
                let _ = git(root, &["update-ref", "-d", &private], false);
                result?;
            }
            // Rebase rewrites commit identity and can include unrelated base
            // changes, both before landing and afterwards. Require every PR
            // patch to have an equivalent on the independently fetched base.
            // Unrelated histories and merge commits (which cherry omits) do
            // not supply this receipt. Changed conflict resolutions retain work.
            stdout(root, &["merge-base", &tip, head])?;
            let range = format!("{tip}..{head}");
            if !stdout(root, &["rev-list", "--min-parents=2", &range])?.is_empty() {
                return Ok(false);
            }
            let patches = stdout(root, &["cherry", &tip, head])?;
            Ok(!patches.is_empty()
                && patches.lines().all(|line| {
                    line.strip_prefix("- ").is_some_and(|sha| {
                        matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                }))
        })())
    })
}

fn receipt(root: &Path, tip: &str, head: &str, pr: u64, train_only: bool) -> Result<bool> {
    let suffix = format!(r"\((#|!){pr}\)$");
    // A GitHub merge commit names the PR in its subject instead of a squash
    // suffix; it is a receipt for an ordinary landing, never for a train
    // member (that needs the exact member trailer). trace:STORY-2401 | ai:claude
    let merge_prefix = format!("Merge pull request #{pr} from ");
    let merge_pattern = format!("^{merge_prefix}");
    let mut args: Vec<&str> = vec![
        "log",
        "--format=%H",
        "--extended-regexp",
        "--grep",
        suffix.as_str(),
    ];
    if !train_only {
        args.extend(["--grep", merge_pattern.as_str()]);
    }
    args.push(tip);
    let commits = stdout(root, &args)?;
    for commit in commits.lines() {
        let subject = stdout(root, &["show", "-s", "--format=%s", commit])?;
        let squash =
            subject.ends_with(&format!("(#{pr})")) || subject.ends_with(&format!("(!{pr})"));
        if !train_only && (squash || subject.starts_with(&merge_prefix)) {
            return Ok(true);
        }
        if !squash {
            continue;
        }
        let trailers = stdout(
            root,
            &[
                "show",
                "-s",
                "--format=%(trailers:key=Aida-Train-Member,valueonly)",
                commit,
            ],
        )?;
        if trailers
            .lines()
            .any(|line| line == format!("#{pr} at {head}"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Independently fetched base evidence, including squash receipts when the
/// original PR head is absent locally. Unexpected ancestry failures still refuse.
// trace:STORY-2401 | ai:codex
pub(crate) fn on_base(root: &Path, base: &str, head: &str, pr: u64) -> Observed<bool> {
    base_tip(root, base).and_then(|tip| {
        Observed::from_result((|| -> Result<bool> {
            anyhow::ensure!(
                matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid pinned head"
            );
            if git(
                root,
                &["cat-file", "-e", &format!("{head}^{{commit}}")],
                false,
            )?
            .status
            .success()
            {
                let ancestor = git(root, &["merge-base", "--is-ancestor", head, &tip], false)?;
                match ancestor.status.code() {
                    Some(0) => return Ok(true),
                    Some(1) => {}
                    _ => anyhow::bail!("pinned head ancestry unavailable"),
                }
                if stdout(root, &["rev-parse", &format!("{head}^{{tree}}")])?
                    == stdout(root, &["rev-parse", &format!("{tip}^{{tree}}")])?
                {
                    return Ok(true);
                }
            }
            receipt(root, &tip, head, pr, false)
        })())
    })
}

pub(crate) fn train_landed(root: &Path, base: &str, head: &str, pr: u64) -> Observed<bool> {
    base_tip(root, base).and_then(|tip| Observed::from_result(receipt(root, &tip, head, pr, true)))
}

/// A stable forge snapshot plus independently fetched Git evidence. Unknown
/// evidence leaves the caller's spec/queue/change untouched.
// trace:STORY-2401 | ai:codex
pub(crate) fn metadata(
    root: &Path,
    forge: &dyn Forge,
    id: u64,
    sink: &mut dyn crate::network_retry::RetrySink,
) -> Observed<ChangeMetadata> {
    metadata_for(root, forge, id, sink, |_| true)
}

/// Confirm only observations that would authorize the caller's transition.
// trace:STORY-2401 | ai:codex
pub(crate) fn metadata_for(
    root: &Path,
    forge: &dyn Forge,
    id: u64,
    sink: &mut dyn crate::network_retry::RetrySink,
    actionable: impl Fn(&ChangeMetadata) -> bool,
) -> Observed<ChangeMetadata> {
    let _sweep = Sweep::new();
    let first = match forge.observe_change_metadata(id, sink) {
        Observed::Known(Ok(m)) if !actionable(&m) => return Observed::Known(m),
        Observed::Known(Err(why)) => return Observed::Pending(why),
        value => value,
    };
    let mut first = Some(first);
    let landed_closed = std::cell::Cell::new(false);
    confirm_snapshot(
        || {
            first
                .take()
                .unwrap_or_else(|| forge.observe_change_metadata(id, sink))
        },
        |value| match value {
            Err(_) => Observed::Known(true),
            Ok(m) => forge
                .observe_change_ground_truth(root, id, m)
                .map(|landed| {
                    landed_closed.set(m.state == ChangeState::Closed && landed);
                    // Closed is terminal for cleanup whether abandoned or landed by
                    // train. Only an exact member receipt below grants merge credit.
                    m.state == ChangeState::Closed || landed == (m.state == ChangeState::Merged)
                }),
        },
        backoff,
    )
    .and_then(|value| match value {
        Err(why) => Observed::Pending(why),
        Ok(mut m) => {
            if landed_closed.get() {
                match train_landed(root, &m.base_ref, &m.head_sha, id) {
                    Observed::Known(true) => m.state = ChangeState::Merged,
                    Observed::Known(false) => {}
                    Observed::Pending(why) => return Observed::Pending(why),
                    Observed::Transient(why) => return Observed::Transient(why),
                }
            }
            Observed::Known(m)
        }
    })
}
