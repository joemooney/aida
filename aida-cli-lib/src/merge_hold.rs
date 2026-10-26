//! BUG-1167: the substrate merge-hold marker (ADR-37 layer 1, client side).
//!
//! A supervised (drive/guided/operator/decide/unset) PR gets a
//! `.aida/merge-holds/PR-<n>` marker. Every AIDA merge path funnels through
//! `forge::merge_change`, which refuses fail-closed while the marker exists — so
//! a concurrent merger (an integrator sweep, a second `aida agent` session, a
//! stale-binary drain, `aida pr merge`) cannot bypass the supervised-merge hold
//! the way BUG-1167 reproduced. The marker is cleared only by an explicit
//! human/advisor review (`aida merge-hold clear <pr>`), which is what "merge
//! requires human/advisor review" means made enforceable rather than advisory.
//!
//! This is the client half. The paired server half (ADR-37 layer 2) is a GitHub
//! required-status-check that reads the equivalent PR label, catching even a raw
//! `gh pr merge` that never touches AIDA. A drain-mode PR is never marked, so its
//! auto-merge is unaffected (the granularity the branch-protection concern needs).
// trace:BUG-1167 | ai:claude

// STORY-1889: the hold record types, the hold read path, the marker write path
// and the hold decisions live in the protected gate module; this file keeps
// forge labels and the pinned-change plumbing.
// trace:STORY-1889 | ai:claude
use crate::gate::merge_hold_core::*;

use crate::process_retry::RetryEtxtbsy;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// BUG-2176: move a marker a contained reviewer left in its review `worktree`
/// into the primary checkout, composing with any marker already there (so a
/// stricter hold stays primary). Only a marker written at or after `since`
/// (the reviewer's start) is adopted, so a stale copy from an earlier round
/// cannot re-arm a hold a human has since cleared. Returns whether a marker
/// was adopted; the worktree copy is removed only after the primary write.
// trace:BUG-2176 | ai:claude
pub(crate) fn adopt_worktree_hold(
    project_root: &Path,
    worktree: &Path,
    pr: u64,
    since: std::time::SystemTime,
) -> std::io::Result<bool> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let local = local_hold_path(worktree, pr);
    if canon(&local) == canon(&hold_path(project_root, pr)) {
        return Ok(false);
    }
    let fresh = std::fs::symlink_metadata(&local)
        .and_then(|m| m.modified())
        .is_ok_and(|modified| modified >= since);
    if !fresh {
        return Ok(false);
    }
    let Some(record) = parse_hold_marker(std::fs::read_to_string(&local), pr) else {
        return Ok(false);
    };
    // TASK-2153: carry the worktree's placement event with the marker, so
    // the primary's tamper check and hold age see the original placement.
    // trace:TASK-1951 trace:TASK-2153 | ai:claude
    let placement = crate::events::read_all(worktree)
        .into_iter()
        .rev()
        .find(|e| {
            matches!(
                e.kind,
                crate::events::EventKind::MergeHoldChanged { pr: p, placed: true, .. }
                    if u64::from(p) == pr
            )
        });
    write_typed_hold_context(project_root, &record, false, placement.as_ref())?;
    std::fs::remove_file(&local)?;
    Ok(true)
}

/// BUG-2176: markers sitting in linked worktrees of `main_root`'s repository,
/// as `(pr, path)` ascending — where no merge gate reads them and removing the
/// worktree destroys them. `aida merge-hold list` warns on each.
// trace:BUG-2176 | ai:claude
pub(crate) fn stray_worktree_markers(main_root: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let primary = canon(&local_holds_dir(main_root));
    let mut found = Vec::new();
    // TASK-2053: seat clones too; removing one destroys its markers just the
    // same. A clone is seat-written, so its holds dir is read only when it is
    // a real directory, never through a symlink.
    // trace:TASK-2053 | ai:claude
    let clones = crate::clone_seat_trees(main_root)
        .into_iter()
        .map(|wt| wt.path);
    let trees = crate::worktree::parse_worktree_paths(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .map(|tree| (tree, false))
        .chain(clones.map(|tree| (tree, true)));
    for (tree, clone) in trees {
        let dir = local_holds_dir(&tree);
        if canon(&dir) == primary {
            continue;
        }
        if clone
            && ![tree.join(".aida"), dir.clone()]
                .iter()
                .all(|d| std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()))
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(n) = name
                .to_string_lossy()
                .strip_prefix("PR-")
                .and_then(|s| s.parse::<u64>().ok())
            {
                found.push((n, entry.path()));
            }
        }
    }
    found.sort();
    found
}

/// The `stray_worktree_markers` rows of `aida merge-hold list --json`.
// trace:BUG-2176 | ai:claude
pub(crate) fn stray_markers_json(
    strays: &[(u64, PathBuf)],
    label_only: &[u64],
) -> serde_json::Value {
    strays
        .iter()
        .map(|(pr, path)| {
            serde_json::json!({
                "pr": pr,
                "path": path.display().to_string(),
                "label_only": label_only.contains(pr),
            })
        })
        .collect()
}

/// The `aida merge-hold list` line for one stray marker. A label-only hold
/// with a stray marker is the BUG-2176 shape: the reviewer's recorded reason
/// and release condition exist, just not where the gates read.
// trace:BUG-2176 | ai:claude
pub(crate) fn stray_marker_warning(
    pr: u64,
    path: &Path,
    main_root: &Path,
    label_only: bool,
) -> String {
    let primary = local_hold_path(main_root, pr);
    let lead = if label_only {
        format!(
            "PR #{pr}: held by the label alone here, but a marker for it sits in a linked worktree"
        )
    } else {
        format!("PR #{pr}: a merge-hold marker sits in a linked worktree")
    };
    format!(
        "{lead} at {} — `aida pr ship` and auto-release do not read it, and removing that worktree destroys it. Copy it to {} to restore it.",
        path.display(),
        primary.display()
    )
}

pub(crate) fn reconcile_recusal_route(
    project_root: &Path,
    record: &mut MergeHoldRecord,
) -> std::io::Result<()> {
    let agents = project_root.join(".aida/agents");
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(agents) {
        for entry in entries.flatten() {
            let Ok(body) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(value) = toml::from_str::<toml::Value>(&body) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let role = value.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let agent_type = value
                .get("agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ended = value.get("ended_at").is_some();
            let paused = value
                .get("availability")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.eq_ignore_ascii_case("paused"));
            let principal = PrincipalIdentity::registered_agent(id);
            let eligible_role = matches!(
                role.to_ascii_lowercase().as_str(),
                "reviewer" | "advisor" | "integrator"
            );
            // Every cheap filter runs before the liveness probe, and the probe
            // is a single-pid check (kill(pid, 0) on Unix), never a full
            // process-table scan. trace:STORY-1397 | ai:claude
            if ended
                || paused
                || !eligible_role
                || agent_type.is_empty()
                || principal_is_recused(record, &principal)
            {
                continue;
            }
            let live = value
                .get("pid")
                .and_then(|v| v.as_integer())
                .and_then(|v| u32::try_from(v).ok())
                .is_some_and(aida_core::liveness::pid_is_alive);
            if live {
                candidates.push((principal, agent_type.to_string()));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.principal_id.cmp(&b.0.principal_id));
    let Some((principal, agent_type)) = candidates.into_iter().next() else {
        retire_stale_route_briefs(project_root, record.pr, None)?;
        record.routed_to.clear();
        record.routing_state = HoldRoutingState::NoIndependentReader;
        return Ok(());
    };
    record.routed_to = vec![principal.clone()];
    record.routing_state = HoldRoutingState::Routed;
    let head = record.target_head_sha.as_deref().unwrap_or("unknown");
    let dir = project_root.join(".aida/agent-briefs").join(&agent_type);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("PR-{}-{}.md", record.pr, head));
    retire_stale_route_briefs(project_root, record.pr, Some(&path))?;
    if !path.exists() {
        let recused = record
            .recused_principals
            .iter()
            .map(PrincipalIdentity::key)
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!("---\nspec_id: PR-{}\ntitle: Independent exact-head review\n---\n\nIndependently review PR #{} at exact head `{}`.\n\nRecused principals: {}\n\nRecord a durable verdict; acknowledging this brief does not clear or merge the hold.\n", record.pr, record.pr, head, recused);
        aida_core::fs_atomic::write_atomic(&path, body.as_bytes())?;
    }
    Ok(())
}

fn retire_stale_route_briefs(
    project_root: &Path,
    pr: u64,
    keep: Option<&Path>,
) -> std::io::Result<()> {
    let root = project_root.join(".aida/agent-briefs");
    let Ok(agent_dirs) = std::fs::read_dir(root) else {
        return Ok(());
    };
    let prefix = format!("PR-{pr}-");
    for agent_dir in agent_dirs.flatten() {
        let Ok(entries) = std::fs::read_dir(agent_dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && name.ends_with(".md") && keep != Some(path.as_path()) {
                let stale = path.with_extension("md.stale");
                if stale.exists() {
                    std::fs::remove_file(&stale)?;
                }
                std::fs::rename(path, stale)?;
            }
        }
    }
    Ok(())
}

/// The GitHub label mirroring the merge-hold marker for ADR-37 Layer 2 (a
/// required status check keyed to this label blocks a raw/stale/UI merge the
/// client chokepoint never sees). trace on the item below stays a plain comment.
// trace:BUG-1167 | ai:claude
pub(crate) const HOLD_LABEL: &str = "aida:merge-hold";
/// Permanent forge-side evidence that this PR has been held at least once.
// trace:BUG-1693 | ai:codex
pub(crate) const HOLD_RECORDED_LABEL: &str = "aida:merge-hold-recorded";
/// Forge-side mirror of the human-gated clearance record.
// trace:BUG-1693 | ai:codex
pub(crate) const HOLD_CLEARED_LABEL: &str = "aida:merge-hold-cleared";

// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergeHoldLabel {
    pub name: &'static str,
    pub color: &'static str,
    pub description: &'static str,
}

pub(crate) const MERGE_HOLD_LABELS: [MergeHoldLabel; 3] = [
    MergeHoldLabel {
        name: HOLD_LABEL,
        color: "B60205",
        description: "Supervised merge hold — merge-hold-gate fails while present",
    },
    MergeHoldLabel {
        name: HOLD_RECORDED_LABEL,
        color: "B60205",
        description: "Supervised merge hold was recorded — persists across clearance",
    },
    MergeHoldLabel {
        name: HOLD_CLEARED_LABEL,
        color: "0E8A16",
        description: "Recorded merge hold has a human-gated clearance",
    },
];

// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelDefinitions {
    Read {
        present: Vec<&'static str>,
        missing: Vec<&'static str>,
    },
    NoForge,
    Unknown(String),
}

pub(crate) fn label_definitions(project_root: &Path) -> LabelDefinitions {
    let kind = crate::forge::resolve_forge_kind(project_root);
    if kind == crate::forge::ForgeKind::None {
        return LabelDefinitions::NoForge;
    }
    let pin = match resolve_pinned_repo(project_root, kind) {
        Ok(Some(pin)) => pin,
        Ok(None) => return LabelDefinitions::NoForge,
        Err(err) => return LabelDefinitions::Unknown(err),
    };
    label_definitions_with(project_root, &pin, run_forge_cli_stdout_bounded)
}

// A doctor probe must not let a hung forge CLI stall diagnostics indefinitely.
// Keep this separate from the shared runner used by hold placement.
const LABEL_DEFINITIONS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn run_forge_cli_stdout_bounded(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    run_forge_cli_stdout_bounded_with(project_root, cli, args, LABEL_DEFINITIONS_TIMEOUT)
}

fn run_forge_cli_stdout_bounded_with(
    project_root: &Path,
    cli: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<(bool, String), String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut command = std::process::Command::new(cli);
    command
        .current_dir(project_root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout");
    let stdout_sender = sender.clone();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout.take(4 * 1024 * 1024).read_to_end(&mut output);
        let _ = stdout_sender.send((false, result, output));
    });
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_sender = sender.clone();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = stderr.take(4 * 1024 * 1024).read_to_end(&mut output);
        let _ = stderr_sender.send((true, result, output));
    });
    drop(sender);
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_read = false;
    let mut stderr_read = false;
    loop {
        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|e| format!("could not wait for {cli}: {e}"))?;
        }
        while let Ok((is_stderr, result, output)) = receiver.try_recv() {
            result.map_err(|e| format!("could not read {cli} output: {e}"))?;
            if is_stderr {
                stderr = output;
                stderr_read = true;
            } else {
                stdout = output;
                stdout_read = true;
            }
        }
        if let Some(status) = status.as_ref().filter(|_| stdout_read && stderr_read) {
            let output = if status.success() { stdout } else { stderr };
            return Ok((
                status.success(),
                String::from_utf8_lossy(&output).to_string(),
            ));
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn label_definitions_with(
    root: &Path,
    pin: &PinnedRepo,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> LabelDefinitions {
    use crate::forge::ForgeKind;
    let (cli, args) = match pin.kind {
        ForgeKind::GitHub => (
            "gh",
            vec![
                "label".into(),
                "list".into(),
                "-R".into(),
                pin.repo_arg(),
                "--json".into(),
                "name".into(),
                "--limit".into(),
                "200".into(),
            ],
        ),
        ForgeKind::GitLab => (
            "glab",
            vec!["label".into(), "list".into(), "-R".into(), pin.repo_arg()],
        ),
        ForgeKind::None => return LabelDefinitions::NoForge,
    };
    let output = match runner(root, cli, &args) {
        Ok((true, out)) => out,
        Ok((false, out)) => return LabelDefinitions::Unknown(out),
        Err(err) => return LabelDefinitions::Unknown(err),
    };
    let names: Vec<String> = if pin.kind == ForgeKind::GitHub {
        match serde_json::from_str::<Vec<serde_json::Value>>(&output) {
            Ok(rows) => match rows
                .iter()
                .map(|row| {
                    row.get("name")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<_>>>()
            {
                Some(names) => names,
                None => {
                    return LabelDefinitions::Unknown(
                        "gh label list output did not contain a name for every row".into(),
                    )
                }
            },
            Err(err) => {
                return LabelDefinitions::Unknown(format!(
                    "could not parse gh label list output: {err}"
                ))
            }
        }
    } else {
        output.lines().map(str::to_owned).collect()
    };
    let exists = |name: &&str| {
        if pin.kind == ForgeKind::GitHub {
            names.iter().any(|candidate| candidate == *name)
        } else {
            names.iter().any(|line| line_defines(line, name))
        }
    };
    let present = MERGE_HOLD_LABELS
        .iter()
        .map(|l| l.name)
        .filter(exists)
        .collect();
    let missing = MERGE_HOLD_LABELS
        .iter()
        .map(|l| l.name)
        .filter(|n| !exists(n))
        .collect();
    LabelDefinitions::Read { present, missing }
}

// GitLab emits decorated raw lines. A label match is valid only when its next
// character cannot continue a label identifier (for example, a longer label).
fn line_defines(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(start, _)| {
        line[start + name.len()..]
            .chars()
            .next()
            .is_none_or(|ch| !(ch.is_alphanumeric() || matches!(ch, '-' | '_' | ':')))
    })
}

pub(crate) fn create_label_command(
    pin: &PinnedRepo,
    label: MergeHoldLabel,
) -> (&'static str, Vec<String>) {
    use crate::forge::ForgeKind;
    let args = match pin.kind {
        ForgeKind::GitHub => vec![
            "label".into(),
            "create".into(),
            label.name.into(),
            "-R".into(),
            pin.repo_arg(),
            "--color".into(),
            label.color.into(),
            "--description".into(),
            label.description.into(),
            "--force".into(),
        ],
        ForgeKind::GitLab => vec![
            "label".into(),
            "create".into(),
            "-R".into(),
            pin.repo_arg(),
            "--name".into(),
            label.name.into(),
            "--color".into(),
            label.color.into(),
            "--description".into(),
            label.description.into(),
        ],
        ForgeKind::None => Vec::new(),
    };
    (pin.kind.cli_name(), args)
}

pub(crate) fn provision_label_definitions(
    root: &Path,
    pin: &PinnedRepo,
    missing: &[&'static str],
) -> Vec<(&'static str, Result<(), String>)> {
    MERGE_HOLD_LABELS
        .iter()
        .filter(|label| missing.contains(&label.name))
        .map(|label| {
            let (cli, args) = create_label_command(pin, *label);
            let result = match run_forge_cli(root, cli, &args) {
                Ok((true, _)) => Ok(()),
                Ok((false, output))
                    if pin.kind == crate::forge::ForgeKind::GitLab
                        && output.to_ascii_lowercase().contains("already exists") =>
                {
                    Ok(())
                }
                Ok((false, output)) => {
                    Err(output.lines().next().unwrap_or("non-zero exit").to_string())
                }
                Err(err) => Err(err),
            };
            (label.name, result)
        })
        .collect()
}

/// Whether the forge change currently carries the Layer-2 merge-hold label.
///
/// This is intentionally a live forge read rather than marker metadata: an
/// operator may add the label directly, leaving no Layer-1 marker to inspect.
/// Errors are returned so callers keep their existing fail-closed behavior.
///
/// TASK-1455: the read is pinned to the project's own forge repo (`-R`), and
/// the answer is refused if the forge reports a change from any other repo.
// trace:TASK-1287 | ai:codex
// trace:TASK-1455 | ai:claude
pub(crate) fn label_present(project_root: &Path, pr: u64) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    Ok(fetch_pinned_change(project_root, kind, pr)?.is_some_and(|c| c.hold_label))
}

/// The forge repository every merge-hold forge call is pinned to, resolved
/// once from the project's `origin` remote. A PR number is only meaningful
/// inside one repo; letting `gh`/`glab` infer the repo from the cwd or ambient
/// config means the same number can label, read or verdict a DIFFERENT repo's
/// change.
// trace:TASK-1455 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRepo {
    pub kind: crate::forge::ForgeKind,
    /// Lowercased host from `origin` (may be an ssh alias with no dot).
    pub host: String,
    /// Forge-native project path: `owner/repo` or `group/subgroup/project`.
    pub path: String,
}

impl PinnedRepo {
    /// Build the pin from an `origin` URL. `None` when the URL does not name
    /// a host AND an `owner/repo`-shaped path (a local-path remote, an empty
    /// remote, a bare host).
    // trace:TASK-1455 | ai:claude
    pub(crate) fn from_origin(kind: crate::forge::ForgeKind, origin: &str) -> Option<Self> {
        let host = crate::forge::forge_host_of(origin)?
            .trim()
            .to_ascii_lowercase();
        let path = crate::forge::project_path_of(origin)?;
        let path = path.trim_matches('/').to_string();
        let well_formed = !host.is_empty()
            && !host.contains('/')
            && !host.contains('\\')
            && path.contains('/')
            && path.split('/').all(|seg| !seg.trim().is_empty());
        well_formed.then_some(Self { kind, host, path })
    }

    /// An ssh-config alias (`git@github-work:o/r.git`) has no dot and is not a
    /// forge hostname; `gh`/`glab` resolve it themselves, so the pin names the
    /// repo path only and lets the CLI's default host apply.
    fn host_is_alias(&self) -> bool {
        !self.host.contains('.')
    }

    /// The `-R` value: `owner/repo` on the public forge (or an ssh alias),
    /// host-qualified elsewhere so a GitHub Enterprise / self-managed GitLab
    /// repo is never resolved against the public host.
    // trace:TASK-1455 | ai:claude
    pub(crate) fn repo_arg(&self) -> String {
        use crate::forge::ForgeKind;
        let public = match self.kind {
            ForgeKind::GitHub => "github.com",
            ForgeKind::GitLab => "gitlab.com",
            ForgeKind::None => "",
        };
        if self.host == public || self.host_is_alias() {
            self.path.clone()
        } else if self.kind == ForgeKind::GitLab {
            format!("https://{}/{}", self.host, self.path)
        } else {
            format!("{}/{}", self.host, self.path)
        }
    }

    /// Does a forge-reported change URL name THIS repo's change `pr`? The
    /// post-hoc check behind the pin: a response for any other repo or number
    /// is refused rather than trusted.
    // trace:TASK-1455 | ai:claude
    pub(crate) fn change_url_matches(&self, url: &str, pr: u64) -> bool {
        use crate::forge::ForgeKind;
        let Some(host) = crate::forge::forge_host_of(url) else {
            return false;
        };
        let Some(path) = crate::forge::project_path_of(url) else {
            return false;
        };
        let suffix = match self.kind {
            ForgeKind::GitHub => format!("/pull/{pr}"),
            ForgeKind::GitLab => format!("/-/merge_requests/{pr}"),
            ForgeKind::None => return false,
        };
        let Some(repo) = path.strip_suffix(&suffix) else {
            return false;
        };
        let host_ok = self.host_is_alias() || host.eq_ignore_ascii_case(&self.host);
        host_ok && repo.eq_ignore_ascii_case(&self.path)
    }
}

/// Resolve the pin for `kind` from the project's `origin`. `Ok(None)` means a
/// pure-git project (no forge, so no forge call is ever made). An unresolvable
/// repo on a real forge is an ERROR, never an unpinned call (PRIN-5: absent
/// evidence of which repo is not evidence that the ambient one is right).
// trace:TASK-1455 | ai:claude
pub(crate) fn resolve_pinned_repo(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
) -> Result<Option<PinnedRepo>, String> {
    if kind == crate::forge::ForgeKind::None {
        return Ok(None);
    }
    pin_from_origin(kind, crate::forge::origin_url(project_root).as_deref()).map(Some)
}

fn pin_from_origin(
    kind: crate::forge::ForgeKind,
    origin: Option<&str>,
) -> Result<PinnedRepo, String> {
    let cli = kind.cli_name();
    let origin = origin.map(str::trim).filter(|o| !o.is_empty()).ok_or_else(|| {
        format!(
            "refusing to call `{cli}` unpinned: this project has no `origin` remote to name the forge repo"
        )
    })?;
    PinnedRepo::from_origin(kind, origin).ok_or_else(|| {
        format!(
            "refusing to call `{cli}` unpinned: could not resolve an owner/repo from origin `{origin}`"
        )
    })
}

/// The forge facts the merge-hold paths need about one change, read in ONE
/// pinned call and verified to belong to the pinned repo.
// trace:TASK-1455 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedChange {
    pub merged: bool,
    /// BUG-1541: the full lifecycle state, so a sweep can tell an OPEN change
    /// from one CLOSED without merging. `merged` stays for existing callers
    /// and is exactly `state == ChangeState::Merged`.
    pub state: ChangeState,
    pub head_sha: Option<String>,
    pub hold_label: bool,
    /// The forge carries `aida:merge-hold-recorded`: the PR was really held.
    // trace:TASK-1951 | ai:claude
    pub recorded_label: bool,
    /// The forge carries `aida:merge-hold-cleared`.
    // trace:TASK-1951 | ai:claude
    pub cleared_label: bool,
}

/// BUG-1541: the forge's lifecycle state for a change. `Unknown` (a missing
/// or unrecognised state word) is its own answer so a sweep fails CLOSED —
/// only a definite terminal state authorises dropping a marker.
// trace:BUG-1541 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeState {
    Open,
    Merged,
    ClosedUnmerged,
    Unknown,
}

impl ChangeState {
    /// Parse GitHub (`OPEN`/`CLOSED`/`MERGED`) and GitLab
    /// (`opened`/`closed`/`merged`/`locked`) state words.
    // trace:BUG-1541 | ai:claude
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("open" | "opened" | "locked") => Self::Open,
            Some("merged") => Self::Merged,
            Some("closed") => Self::ClosedUnmerged,
            _ => Self::Unknown,
        }
    }

    /// The terminal class, when the change can never merge (again): a marker
    /// on such a change protects nothing. `None` = open or unknown → live.
    // trace:BUG-1541 | ai:claude
    pub(crate) fn terminal(self) -> Option<TerminalState> {
        match self {
            Self::Merged => Some(TerminalState::Merged),
            Self::ClosedUnmerged => Some(TerminalState::ClosedUnmerged),
            Self::Open | Self::Unknown => None,
        }
    }
}

/// BUG-1541: why a marker is a phantom. Reported distinctly — a merged PR and
/// a PR closed without merging mean different things about how a hold ended.
// trace:BUG-1541 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalState {
    Merged,
    ClosedUnmerged,
}

impl TerminalState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::ClosedUnmerged => "closed-unmerged",
        }
    }

    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Merged => "PR merged",
            Self::ClosedUnmerged => "PR closed without merging",
        }
    }
}

/// Read `pr` from the project's pinned forge repo. `Ok(None)` = pure-git.
// trace:TASK-1455 | ai:claude
pub(crate) fn fetch_pinned_change(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
    pr: u64,
) -> Result<Option<PinnedChange>, String> {
    let Some(pin) = resolve_pinned_repo(project_root, kind)? else {
        return Ok(None);
    };
    fetch_pinned_change_with(project_root, &pin, pr, run_forge_cli_stdout).map(Some)
}

/// The argv for the pinned change read, per forge — pure for testing.
// trace:TASK-1455 | ai:claude
fn pinned_change_command(pin: &PinnedRepo, pr: u64) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => Some((
            "gh",
            vec![
                "pr".into(),
                "view".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                "--json".into(),
                "url,state,headRefOid,labels".into(),
            ],
        )),
        // REST read (BUG-639: glab has no reliable `mr view --output json`).
        // The project is named IN the endpoint, so the read is pinned by
        // construction; `--hostname` pins a self-managed host.
        ForgeKind::GitLab => {
            let mut args = vec!["api".to_string()];
            if !pin.host_is_alias() {
                args.push("--hostname".into());
                args.push(pin.host.clone());
            }
            args.push(format!(
                "projects/{}/merge_requests/{pr}",
                pin.path.replace('/', "%2F")
            ));
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

fn fetch_pinned_change_with(
    project_root: &Path,
    pin: &PinnedRepo,
    pr: u64,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<PinnedChange, String> {
    let (cli, args) = pinned_change_command(pin, pr)
        .ok_or_else(|| "no forge to read a change from (pure-git)".to_string())?;
    let (ok, stdout) = runner(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    parse_pinned_change(pin, pr, &stdout)
}

fn parse_pinned_change(pin: &PinnedRepo, pr: u64, json: &str) -> Result<PinnedChange, String> {
    use crate::forge::ForgeKind;
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("could not parse forge response: {e}"))?;
    let url_key = if pin.kind == ForgeKind::GitLab {
        "web_url"
    } else {
        "url"
    };
    let url = value.get(url_key).and_then(|v| v.as_str()).unwrap_or("");
    if !pin.change_url_matches(url, pr) {
        return Err(format!(
            "refusing forge answer for #{pr}: it names `{}`, not a change in the pinned repo `{}`",
            if url.is_empty() { "<no url>" } else { url },
            pin.repo_arg()
        ));
    }
    let (state_key, head_key) = if pin.kind == ForgeKind::GitLab {
        ("state", "sha")
    } else {
        ("state", "headRefOid")
    };
    let state = ChangeState::parse(value.get(state_key).and_then(|v| v.as_str()));
    let merged = state == ChangeState::Merged;
    let head_sha = value
        .get(head_key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let hold_label = labels_json_contains(json, HOLD_LABEL)?;
    let recorded_label = labels_json_contains(json, HOLD_RECORDED_LABEL)?;
    let cleared_label = labels_json_contains(json, HOLD_CLEARED_LABEL)?;
    Ok(PinnedChange {
        merged,
        state,
        head_sha,
        hold_label,
        recorded_label,
        cleared_label,
    })
}

/// What the FORGE says about the Layer-2 label, as opposed to what the marker
/// recorded when it was written ([`LabelState`]). `Unknown` is its own state:
/// an unreachable forge is never rendered as the marker's claim.
// trace:TASK-189 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgeLabel {
    Present,
    Absent,
    /// Pure-git project: there is no forge label to have.
    NoForge,
    Unknown(String),
}

impl ForgeLabel {
    pub(crate) fn from_fetch(fetched: &Result<Option<PinnedChange>, String>) -> Self {
        match fetched {
            Ok(Some(c)) if c.hold_label => Self::Present,
            Ok(Some(_)) => Self::Absent,
            Ok(None) => Self::NoForge,
            Err(e) => Self::Unknown(e.lines().next().unwrap_or("").to_string()),
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::NoForge => "no-forge",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Marker/label divergence for a LIVE hold (the marker exists by definition).
/// `Some(true)`: the forge has no label, so the server-side gate is not armed
/// even though the client chokepoint is. `Some(false)`: the two agree.
/// `None`: the forge could not be read — divergence is UNKNOWN, never reported
/// as agreement. Pure-git has no label layer, so it cannot diverge.
// trace:TASK-189 | ai:claude
pub(crate) fn label_diverged(forge: &ForgeLabel) -> Option<bool> {
    match forge {
        ForgeLabel::Present | ForgeLabel::NoForge => Some(false),
        ForgeLabel::Absent => Some(true),
        ForgeLabel::Unknown(_) => None,
    }
}

fn labels_json_contains(json: &str, wanted: &str) -> Result<bool, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("could not parse forge labels: {e}"))?;
    let labels = value
        .get("labels")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "forge response did not contain a labels array".to_string())?;
    Ok(labels.iter().any(|label| {
        label
            .as_str()
            .or_else(|| label.get("name").and_then(|v| v.as_str()))
            == Some(wanted)
    }))
}

/// BUG-1469: every OPEN change in the pinned repo that carries the hold
/// label. The label alone blocks a merge (it drives the required
/// merge-hold-gate check), so an inventory that only reads marker files
/// under-reports. `Ok(None)` = pure-git (no forge label layer). An error —
/// unpinnable repo, forge failure, a truncated page, an answer naming another
/// repo — is returned so the listing can REPORT the gap, never render the
/// smaller marker-only set as complete.
///
/// Only called from the explicit `aida merge-hold list`; never from
/// `aida awaiting` or its per-turn notice.
// trace:BUG-1469 | ai:claude
pub(crate) fn list_labeled_open_changes(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
) -> Result<Option<Vec<u64>>, String> {
    let Some(pin) = resolve_pinned_repo(project_root, kind)? else {
        return Ok(None);
    };
    list_labeled_open_changes_with(project_root, &pin, run_forge_cli_stdout).map(Some)
}

/// Page ceiling for the labeled-change query. A full page is reported as
/// truncated rather than trusted as the whole set.
const LABELED_QUERY_LIMIT: usize = 100;

/// The argv for the pinned labeled-change query, per forge — pure for testing.
// trace:BUG-1469 | ai:claude
fn labeled_changes_command(pin: &PinnedRepo) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => Some((
            "gh",
            vec![
                "pr".into(),
                "list".into(),
                "-R".into(),
                pin.repo_arg(),
                "--state".into(),
                "open".into(),
                "--label".into(),
                HOLD_LABEL.into(),
                "--limit".into(),
                LABELED_QUERY_LIMIT.to_string(),
                "--json".into(),
                "number,url,labels".into(),
            ],
        )),
        ForgeKind::GitLab => {
            let mut args = vec!["api".to_string()];
            if !pin.host_is_alias() {
                args.push("--hostname".into());
                args.push(pin.host.clone());
            }
            args.push(format!(
                "projects/{}/merge_requests?state=opened&labels={}&per_page={}",
                pin.path.replace('/', "%2F"),
                HOLD_LABEL.replace(':', "%3A"),
                LABELED_QUERY_LIMIT
            ));
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

fn list_labeled_open_changes_with(
    project_root: &Path,
    pin: &PinnedRepo,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<Vec<u64>, String> {
    let (cli, args) = labeled_changes_command(pin)
        .ok_or_else(|| "no forge to list labeled changes from (pure-git)".to_string())?;
    let (ok, stdout) = runner(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    parse_labeled_changes(pin, &stdout)
}

/// Parse the labeled-change listing. Every entry must name a change in the
/// pinned repo AND carry the label; any entry that does not refuses the whole
/// answer (TASK-1455: a response we cannot attribute is not evidence).
// trace:BUG-1469 | ai:claude
fn parse_labeled_changes(pin: &PinnedRepo, json: &str) -> Result<Vec<u64>, String> {
    use crate::forge::ForgeKind;
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("could not parse labeled-change listing: {e}"))?;
    let entries = value
        .as_array()
        .ok_or_else(|| "labeled-change listing was not an array".to_string())?;
    if entries.len() >= LABELED_QUERY_LIMIT {
        return Err(format!(
            "labeled-change listing hit the {LABELED_QUERY_LIMIT}-row page limit; it may be truncated"
        ));
    }
    let (num_key, url_key) = if pin.kind == ForgeKind::GitLab {
        ("iid", "web_url")
    } else {
        ("number", "url")
    };
    let mut out = Vec::new();
    for entry in entries {
        let pr = entry
            .get(num_key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("labeled-change entry has no `{num_key}`"))?;
        let url = entry.get(url_key).and_then(|v| v.as_str()).unwrap_or("");
        if !pin.change_url_matches(url, pr) {
            return Err(format!(
                "refusing labeled-change listing: #{pr} names `{}`, not a change in the pinned repo `{}`",
                if url.is_empty() { "<no url>" } else { url },
                pin.repo_arg()
            ));
        }
        let labeled = labels_json_contains(&entry.to_string(), HOLD_LABEL)?;
        if !labeled {
            return Err(format!(
                "refusing labeled-change listing: #{pr} does not carry `{HOLD_LABEL}`"
            ));
        }
        if !out.contains(&pr) {
            out.push(pr);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Commit-sha-shaped tokens cited in a marker's prose: 7–40 hex chars with at
/// least one digit and one letter (so a bare PR number or an English word is
/// never read as a sha).
// trace:BUG-1562 | ai:claude
fn cited_shas(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|tok| {
            (7..=40).contains(&tok.len())
                && tok.bytes().all(|b| b.is_ascii_hexdigit())
                && tok.bytes().any(|b| b.is_ascii_digit())
                && tok.bytes().any(|b| b.is_ascii_alphabetic())
        })
        .map(str::to_ascii_lowercase)
        .collect()
}

/// BUG-1562: re-evaluate a marker's premise where it is machine-checkable,
/// at READ time — a reason is written once and the world moves on. `Some`
/// explains why the stated premise no longer holds. This only FLAGS: a hold
/// is a deliberate gate and its owner releases it; nothing here clears.
///
/// Checks, each skipped when its input is unknown (unknown is never stale):
///   - the marker cites a commit (its `target_head_sha`, else a sha in its
///     prose) and the PR head has moved past every cited commit;
///   - a REWORK hold's verdict (the typed `verdict_ref` when present, else a
///     spec named in its prose) is now APPROVED, or has been closed by a
///     merge — the refusal it quotes is no longer the reviewer's position;
///   - the spec the hold was placed for (typed `spec`, else a spec named in
///     its prose) is no longer running: its local status is terminal
///     (completed / rejected / superseded). `status_of` is a cheap, local
///     lookup (the cache), never a forge call.
// trace:BUG-1562 | ai:claude
pub(crate) fn premise_stale(
    record: &MergeHoldRecord,
    live_head: Option<&str>,
    verdict_of: impl Fn(&str) -> Option<crate::gate::review_verdict::RecordedVerdict>,
    status_of: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    use crate::gate::review_verdict::{same_reviewed_sha, short_sha};
    let mut why = Vec::new();
    let prose_specs = || crate::pr_ship::extract_spec_ids_from_text(&record.detail);
    if record.reason_kind == HoldReasonKind::Rework {
        let keys: Vec<String> = match &record.verdict_ref {
            Some(r) => vec![r.key.clone()],
            None => prose_specs(),
        };
        for spec in keys {
            let Some(verdict) = verdict_of(&spec) else {
                continue;
            };
            if let Some(merge) = verdict.closed_by_merge.as_deref() {
                why.push(format!(
                    "{spec}'s verdict was closed by merge {}",
                    short_sha(merge)
                ));
            } else if verdict.kind.approves() {
                why.push(match verdict.reviewed_sha.as_deref() {
                    Some(sha) => format!("{spec}'s verdict is now APPROVED at {}", short_sha(sha)),
                    None => format!("{spec}'s verdict is now APPROVED"),
                });
            }
        }
    }
    // BUG-1562: "is the spec this hold was placed for still running?"
    let specs: Vec<String> = match record.spec.as_deref().map(str::trim) {
        Some(spec) if !spec.is_empty() => vec![spec.to_ascii_uppercase()],
        _ => prose_specs(),
    };
    for spec in specs {
        if let Some(status) = status_of(&spec).filter(|s| crate::is_terminal_status_str(s)) {
            why.push(format!(
                "{spec} is no longer running (status {})",
                status.trim().to_ascii_lowercase()
            ));
        }
    }
    let cited: Vec<String> = match record.target_head_sha.as_deref().map(str::trim) {
        Some(sha) if !sha.is_empty() => vec![sha.to_ascii_lowercase()],
        _ => cited_shas(&record.detail),
    };
    if let Some(head) = live_head.map(str::trim).filter(|h| !h.is_empty()) {
        if !cited.is_empty() && !cited.iter().any(|sha| same_reviewed_sha(sha, head)) {
            why.push(format!(
                "marker cites {} but the PR head is now {}",
                short_sha(&cited[0]),
                short_sha(head)
            ));
        }
    }
    (!why.is_empty()).then(|| why.join("; "))
}

/// Mirror active hold state and its history/clearance proof to forge labels so
/// Layer 2 can reject label-only release. Failures are returned to callers;
/// otherwise a failed clearance-label update could look like a successful clear.
///
/// STORY-1165: forge-routed. GitHub → `gh pr edit --add-label/--remove-label`;
/// GitLab → `glab mr update --label/--unlabel`; pure-git → no-op (no forge to
/// labels. This lets both merge-hold-gate workflows require clearance after a
/// hold.
// trace:BUG-1167 | ai:claude (STORY-1165 forge-routes it)
// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelSyncError {
    /// A named label is not DEFINED in the repository. Deterministic and
    /// operator-fixable; never retried.
    DefinitionMissing {
        names: Vec<String>,
        detail: String,
    },
    Other(String),
}

impl LabelSyncError {
    pub(crate) fn is_definition_missing(&self) -> bool {
        matches!(self, Self::DefinitionMissing { .. })
    }
}

impl std::fmt::Display for LabelSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefinitionMissing { names, detail } => write!(f, "{detail}; missing label definition(s): {}. Run `aida merge-hold labels --create-missing`.", names.join(", ")),
            Self::Other(message) => f.write_str(message),
        }
    }
}

fn classify_label_sync_failure(stderr: &str, candidates: &[&str]) -> LabelSyncError {
    let names: Vec<String> = candidates
        .iter()
        .filter(|name| stderr.contains(&format!("'{}' not found", name)))
        .map(|name| (*name).to_string())
        .collect();
    if names.is_empty() {
        LabelSyncError::Other(
            stderr
                .lines()
                .next()
                .unwrap_or("label update failed")
                .trim()
                .to_string(),
        )
    } else {
        LabelSyncError::DefinitionMissing {
            names,
            detail: stderr
                .lines()
                .next()
                .unwrap_or("label definition missing")
                .trim()
                .to_string(),
        }
    }
}

pub(crate) fn sync_label(project_root: &Path, pr: u64, held: bool) -> Result<(), LabelSyncError> {
    sync_label_via(project_root, pr, held, run_forge_cli, run_forge_cli_stdout)
}

/// [`sync_label`] with both forge calls injected: `runner` writes labels and
/// `read` is the pinned change read that answers whether a hold was recorded.
// trace:TASK-1951 | ai:claude
pub(crate) fn sync_label_via(
    project_root: &Path,
    pr: u64,
    held: bool,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
    read: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<(), LabelSyncError> {
    sync_label_with_probe(project_root, pr, held, runner, |pin| {
        fetch_pinned_change_with(project_root, pin, pr, read).map(|c| c.recorded_label)
    })
}

/// BUG-1236: the real sync loop with the forge CLI injected. Retries once on
/// failure, records the outcome on the marker (`label: synced` /
/// `label: unsynced: <err>`) when holding, and RETURNS the failure instead of
/// swallowing it — every caller prints it, `aida merge-hold list` shows it,
/// and `--fix` re-syncs it. Before this the label silently never landed on
/// three supervised PRs while the required merge-hold-gate check read pass.
// trace:BUG-1236 | ai:claude
#[cfg(test)]
pub(crate) fn sync_label_with(
    project_root: &Path,
    pr: u64,
    held: bool,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<(), LabelSyncError> {
    sync_label_with_probe(project_root, pr, held, runner, |_| Ok(true))
}

/// [`sync_label_with`] plus `recorded_probe`, which reads whether the forge
/// carries `aida:merge-hold-recorded`. An unhold adds the cleared label only
/// for a recorded hold: a cleared label without a recorded one fails the
/// merge-hold gate. A never-recorded PR instead has any orphaned cleared label
/// removed. An unreadable probe changes no label at all and returns an error
/// naming the forge failure; each caller adds the retry that fits its own
/// path (TASK-1951). Dropping the hold label alone would strand the PR.
// trace:TASK-1951 | ai:claude
pub(crate) fn sync_label_with_probe(
    project_root: &Path,
    pr: u64,
    held: bool,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
    recorded_probe: impl FnOnce(&PinnedRepo) -> Result<bool, String>,
) -> Result<(), LabelSyncError> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    // TASK-1455: pin the repo; an unresolvable one refuses (and is recorded
    // as unsynced) instead of letting `gh`/`glab` guess from the cwd.
    let pin = match resolve_pinned_repo(project_root, kind) {
        Ok(Some(pin)) => pin,
        // pure-git has no forge to carry a label; the file marker still holds.
        Ok(None) => return Ok(()),
        Err(err) => {
            if held {
                let _ = record_label_state(project_root, pr, &LabelState::Unsynced(err.clone()));
            }
            return Err(LabelSyncError::Other(err));
        }
    };
    let cleared = if held {
        ClearedLabel::Remove
    } else {
        match recorded_probe(&pin) {
            Ok(true) => ClearedLabel::Add,
            Ok(false) => ClearedLabel::Remove,
            Err(err) => {
                return Err(LabelSyncError::Other(format!(
                    "could not read PR #{pr}'s labels from the forge ({err}); no label was \
                     changed"
                )))
            }
        }
    };
    let Some((cli, args)) = sync_label_command(&pin, pr, held, cleared) else {
        return Ok(());
    };
    let mut last_err = String::new();
    let mut sync_error = None;
    for attempt in 0..2 {
        match runner(project_root, cli, &args) {
            Ok((true, _)) => {
                if held {
                    let _ = record_label_state(project_root, pr, &LabelState::Synced);
                }
                return Ok(());
            }
            Ok((false, stderr)) => {
                let classified = classify_label_sync_failure(
                    &stderr,
                    &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL],
                );
                if classified.is_definition_missing() {
                    last_err = stderr
                        .lines()
                        .next()
                        .unwrap_or("label definition missing")
                        .trim()
                        .to_string();
                    sync_error = Some(classified);
                    break;
                }
                last_err = stderr
                    .lines()
                    .next()
                    .unwrap_or("non-zero exit")
                    .trim()
                    .to_string();
            }
            Err(e) => last_err = e,
        }
        if attempt == 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    if held {
        let _ = record_label_state(project_root, pr, &LabelState::Unsynced(last_err.clone()));
    }
    let legacy = format!("`{cli} {}` failed: {last_err}", args.join(" "));
    Err(sync_error.unwrap_or(LabelSyncError::Other(legacy)))
}

fn run_forge_cli(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output_retrying_etxtbsy()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

fn run_forge_cli_stdout(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output_retrying_etxtbsy()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

/// The CLI + argv for mirroring the merge-hold label on a change, per forge —
/// kept pure so the routing is unit-testable. `None` = no forge to label
/// (pure-git).
// trace:STORY-1165 | ai:claude
// trace:TASK-1455 | ai:claude
/// What a label sync does with `aida:merge-hold-cleared`.
// trace:TASK-1951 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearedLabel {
    Add,
    Remove,
}

pub(crate) fn sync_label_command(
    pin: &PinnedRepo,
    pr: u64,
    held: bool,
    cleared: ClearedLabel,
) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => {
            let (flag, label) = if held {
                ("--add-label", format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
            } else {
                ("--remove-label", HOLD_LABEL.to_string())
            };
            let mut args = vec![
                "pr".into(),
                "edit".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                flag.into(),
                label,
            ];
            // Both partial-failure interleavings fail closed: {hold, recorded,
            // cleared} fails gate rule 1; {recorded} fails rule 2. No
            // interleaving yields a passing gate.
            // trace:BUG-1693 | ai:codex
            match cleared {
                ClearedLabel::Add => args.extend(["--add-label".into(), HOLD_CLEARED_LABEL.into()]),
                ClearedLabel::Remove => {
                    args.extend(["--remove-label".into(), HOLD_CLEARED_LABEL.into()])
                }
            }
            Some(("gh", args))
        }
        ForgeKind::GitLab => {
            let (flag, label) = if held {
                ("--label", format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
            } else {
                ("--unlabel", HOLD_LABEL.to_string())
            };
            let mut args = vec![
                "mr".into(),
                "update".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                flag.into(),
                label,
            ];
            // trace:BUG-1693 | ai:codex
            match cleared {
                ClearedLabel::Add => args.extend(["--label".into(), HOLD_CLEARED_LABEL.into()]),
                ClearedLabel::Remove => {
                    args.extend(["--unlabel".into(), HOLD_CLEARED_LABEL.into()])
                }
            }
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

/// A PR the forge shows was never recorded as held (no
/// `aida:merge-hold-recorded`) must not carry `aida:merge-hold-cleared`: the
/// gate fails on that pair. Removes the cleared label (and any hold label)
/// only when the forge read succeeds and shows a cleared label with no
/// recorded hold. Returns whether it wrote anything.
// trace:TASK-1951 | ai:claude
pub(crate) fn drop_orphaned_cleared_label(project_root: &Path, pr: u64) -> Result<bool, String> {
    drop_orphaned_cleared_label_with(project_root, pr, run_forge_cli_stdout, run_forge_cli)
}

// trace:TASK-1951 | ai:claude
pub(crate) fn drop_orphaned_cleared_label_with(
    project_root: &Path,
    pr: u64,
    read: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    let Some(pin) = resolve_pinned_repo(project_root, kind)? else {
        return Ok(false);
    };
    let change = fetch_pinned_change_with(project_root, &pin, pr, read)?;
    if change.recorded_label || !change.cleared_label {
        return Ok(false);
    }
    sync_label_with_probe(project_root, pr, false, runner, |_| Ok(false))
        .map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
#[path = "tests/merge_hold_tests.rs"]
mod tests;
