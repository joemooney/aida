//! Binary-freshness gate for drain / wave launch (STORY-1414) and the cheap
//! point-of-use staleness warning (TASK-188).
//!
//! # Why
//!
//! A wave pins the binary that launched it (`binary_sha` in `.aida/drain.lock`)
//! and nothing compared that SHA to the default branch. Twice a merged
//! orchestrator fix was silently discarded for hours because the drain ran a
//! build older than `main`. The `aida pull` "binary is behind HEAD" warning was
//! printed and ignored both times, so this is a substrate gate, not a louder
//! message.
//!
//! # Rule
//!
//! At wave launch (`aida queue work --auto-complete`, `aida burndown run`):
//!
//! - Only a **dev build of the AIDA workspace** is checked: the running
//!   executable lives under some `<checkout>/target/` whose checkout is the
//!   AIDA workspace, AND the project being drained is the AIDA workspace. A
//!   released `aida` (installed binary) or any downstream project is never
//!   checked.
//! - The running binary's embedded build SHA (`AIDA_BUILD_GIT_SHA`, the same
//!   stamp `aida --version` prints and `drain.lock` records) is compared with
//!   the local default-branch HEAD, reusing the TASK-221 `classify_sha_match`
//!   verdict that `aida dev status` shows. Equal, or AHEAD of HEAD (a build of
//!   a branch that already contains main), is fresh. Behind, truly diverged,
//!   or unknown is stale.
//! - By DEFAULT a stale binary prints a one-line stderr warning and the wave
//!   launches; `drain.lock` records `launched_stale = true` so an audit can
//!   establish what the wave ran under. Pinning an older binary deliberately
//!   (bisecting an orchestration regression, reproducing a shelve) therefore
//!   needs no override.
//! - `--require-head` (or `[drain] require_head = true` in
//!   `.aida/config.toml`) turns the warning into a REFUSAL naming the one fix:
//!   `make build-fast`, then rerun. Unknown never passes silently under it.
//!
//! Build-and-re-exec (the spec's full shape) is DEFERRED: refusal under
//! `--require-head` is the safe minimum, and a failed build would need its own
//! refusal path.
//!
//! trace:STORY-1414 trace:TASK-188 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ShaMatch;

/// Set when this process launched a wave on a stale binary (warned, not
/// refused), so the drain-lock record can say so.
// trace:STORY-1414 | ai:claude
static LAUNCHED_STALE: AtomicBool = AtomicBool::new(false);

/// True when this process launched its wave on a stale dev binary.
pub(crate) fn launched_stale() -> bool {
    LAUNCHED_STALE.load(Ordering::SeqCst)
}

/// The facts about the running binary vs the default branch.
// trace:STORY-1414 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// Not a dev build of the AIDA workspace (released binary, downstream
    /// project) — never gated.
    NotGated,
    /// Binary SHA equals the default-branch HEAD.
    Fresh,
    /// Binary SHA is behind (ancestor) or diverged from the default-branch HEAD.
    Stale {
        binary_sha: String,
        head_sha: String,
        branch: String,
        kind: ShaMatch,
    },
    /// Could not establish the binary or HEAD SHA.
    Unknown { reason: String },
}

/// What the wave launch should do.
// trace:STORY-1414 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateDecision {
    Proceed,
    /// Launch, but print this one-line warning (default mode, stale binary).
    Warn(String),
    Refuse(String),
}

/// Is `root` a checkout of the AIDA workspace itself?
pub(crate) fn is_aida_workspace(root: &Path) -> bool {
    root.join("aida-cli-lib").join("Cargo.toml").is_file()
        && root.join("aida-core").join("Cargo.toml").is_file()
}

/// If `exe` lives under `<checkout>/target/...` where `<checkout>` is an AIDA
/// workspace, return that checkout. Any worktree of the repo counts.
pub(crate) fn dev_workspace_of(exe: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let mut cur = exe.parent();
    while let Some(dir) = cur {
        if dir.file_name().is_some_and(|n| n == "target") {
            if let Some(checkout) = dir.parent() {
                if is_aida_workspace(checkout) {
                    return Some(checkout.to_path_buf());
                }
            }
        }
        cur = dir.parent();
    }
    None
}

/// Resolve `(branch, sha)` of the local default branch. Local refs only — no
/// network.
// trace:STORY-1414 | ai:claude
fn default_branch_head(repo: &Path) -> Option<(String, String)> {
    let branch = aida_core::git_ops::default_branch_name(repo);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}^{{commit}}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some((branch, sha))
}

/// Pure-ish classification of the running binary against `project_root`'s
/// default branch. `binary_sha` is the build's embedded SHA.
// trace:STORY-1414 | ai:claude
pub(crate) fn evaluate(project_root: &Path, exe: &Path, binary_sha: &str) -> Freshness {
    if !is_aida_workspace(project_root) || dev_workspace_of(exe).is_none() {
        return Freshness::NotGated;
    }
    let bin = binary_sha.trim();
    if bin.is_empty() || bin.eq_ignore_ascii_case("unknown") {
        return Freshness::Unknown {
            reason: "this build carries no embedded git SHA".to_string(),
        };
    }
    let Some((branch, head)) = default_branch_head(project_root) else {
        return Freshness::Unknown {
            reason: "could not resolve the default branch HEAD".to_string(),
        };
    };
    match crate::classify_sha_match(project_root, bin, &head) {
        ShaMatch::Exact => Freshness::Fresh,
        // Not an ancestor of HEAD: the build may be AHEAD (HEAD is its
        // ancestor — a feature-branch build that already contains main). That
        // is fresh; only true divergence is stale.
        ShaMatch::Unrelated
            if crate::classify_sha_match(project_root, &head, bin) == ShaMatch::Ancestor =>
        {
            Freshness::Fresh
        }
        ShaMatch::Unknown => Freshness::Unknown {
            reason: format!("git could not place build {bin} relative to {branch} HEAD"),
        },
        kind => Freshness::Stale {
            binary_sha: bin.to_string(),
            head_sha: head,
            branch,
            kind,
        },
    }
}

fn short(sha: &str) -> &str {
    sha.get(..sha.len().min(10)).unwrap_or(sha)
}

/// Pure decision: freshness + `require_head` → proceed / warn / refuse.
// trace:STORY-1414 | ai:claude
pub(crate) fn decide(freshness: &Freshness, require_head: bool) -> GateDecision {
    let problem = match freshness {
        Freshness::NotGated | Freshness::Fresh => return GateDecision::Proceed,
        Freshness::Stale {
            binary_sha,
            head_sha,
            branch,
            kind,
        } => {
            let rel = if matches!(kind, ShaMatch::Ancestor) {
                "is behind"
            } else {
                "has diverged from"
            };
            format!(
                "this aida binary (build {}) {rel} {branch} HEAD ({})",
                short(binary_sha),
                short(head_sha)
            )
        }
        Freshness::Unknown { reason } => {
            format!("cannot confirm this aida binary matches the default branch HEAD: {reason}")
        }
    };
    if require_head {
        GateDecision::Refuse(format!(
            "refusing to launch the drain: {problem}.\n  \
             --require-head is set (flag or `[drain] require_head`), and a wave pins its \
             launching binary.\n  \
             Fix: run `make build-fast`, then rerun this command."
        ))
    } else {
        GateDecision::Warn(format!(
            "{problem}; the wave will run this build (run `make build-fast` to pick up HEAD, \
             or pass --require-head to refuse instead)"
        ))
    }
}

/// Check the running binary at wave launch. Call BEFORE taking the drain lock
/// so the lock records a stale launch. `require_head` is the `--require-head`
/// flag; `[drain] require_head = true` in config also enables refusal. A child
/// drive borrowing its parent's lock is not re-checked (the parent already
/// was).
// trace:STORY-1414 | ai:claude
pub(crate) fn enforce_wave_launch_gate(
    project_root: &Path,
    require_head: bool,
) -> anyhow::Result<()> {
    if crate::drain_lock::borrow_requested() {
        return Ok(());
    }
    let require_head =
        require_head || crate::read_drain_config(project_root).require_head == Some(true);
    let freshness = evaluate(
        project_root,
        &crate::aida_exe_path(),
        env!("AIDA_BUILD_GIT_SHA"),
    );
    match decide(&freshness, require_head) {
        GateDecision::Proceed => Ok(()),
        GateDecision::Warn(msg) => {
            LAUNCHED_STALE.store(true, Ordering::SeqCst);
            eprintln!(
                "  {} {msg}",
                crate::glyphs::get(crate::glyphs::Glyph::Warning, Some(project_root))
            );
            Ok(())
        }
        GateDecision::Refuse(msg) => anyhow::bail!("{msg}"),
    }
}

/// TASK-188: the one-line point-of-use warning, or `None`. Only a dev build
/// that is strictly BEHIND the default branch warns (diverged builds are
/// usually a deliberate feature-branch build). Local git only — no network.
// trace:TASK-188 | ai:claude
pub(crate) fn staleness_warning(freshness: &Freshness) -> Option<String> {
    match freshness {
        Freshness::Stale {
            binary_sha,
            head_sha,
            branch,
            kind: ShaMatch::Ancestor,
        } => Some(format!(
            "your dev aida binary (build {}) is behind {branch} HEAD ({}) — run `make build-fast`",
            short(binary_sha),
            short(head_sha)
        )),
        _ => None,
    }
}

/// TASK-188: print the staleness line to stderr when the RUNNING binary is a
/// dev build behind the current project's default branch. Best-effort: any
/// missing signal is silent.
// trace:TASK-188 | ai:claude
pub(crate) fn warn_if_running_binary_stale() {
    let Ok(root) = crate::find_project_root() else {
        return;
    };
    // Cheap filesystem check first so downstream projects pay nothing.
    if !is_aida_workspace(&root) {
        return;
    }
    let freshness = evaluate(&root, &crate::aida_exe_path(), env!("AIDA_BUILD_GIT_SHA"));
    if let Some(line) = staleness_warning(&freshness) {
        eprintln!(
            "{} {line}",
            crate::glyphs::get(crate::glyphs::Glyph::Warning, Some(&root))
        );
    }
}

#[cfg(test)]
#[path = "tests/freshness_gate_tests.rs"]
mod tests;
