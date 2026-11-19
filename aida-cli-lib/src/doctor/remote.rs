// trace:TASK-3781 | ai:codex

use super::*;

// ============================================================================
// TASK-717 — `aida doctor`: verify-and-prune stale REMOTE branches.
//
// `aida doctor --heal --category orphan-branches` only deletes LOCAL orphan
// branches; stale `origin/*` branches accumulate and must be pruned by hand.
// This adds a `stale-remote-branches` category that surfaces remote branches
// and verify-and-prunes them with a squash-aware safety model:
//
//   safe-to-delete iff ANY of:
//     - the branch's spec is Completed/Rejected, OR
//     - the branch HEAD is an ancestor of origin/main (already merged), OR
//     - origin/main carries a commit referencing the spec `(SPEC-ID)`
//   AND none of the EXCLUDE conditions apply:
//     - the branch is a protected ref (main/master/aida-store), OR
//     - the branch has an open PR
//   AND the branch has NO genuinely-unique unmerged commits (those are KEPT
//   and flagged for the operator, never deleted).
//
// Dry-run by default (read-only list + per-branch verdict/reason); the heal
// is gated behind --yes --force (destructive remote deletion). trace:TASK-717
// ============================================================================

// The classification verdict for one remote branch. trace:TASK-717
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RemoteBranchVerdict {
    /// Protected or open-PR — excluded from any consideration.
    Excluded(String),
    /// Verified merged/terminal AND no unique unmerged commits → safe to prune.
    SafeToDelete(String),
    /// Has genuinely-unique unmerged commits → keep, flag for the operator.
    Keep(String),
}

/// Inputs to the pure remote-branch classifier — every git/gh probe result the
/// safety model needs, gathered once per branch by the scanner. Keeping the
/// classification pure makes the squash-aware safety model unit-testable
// without git or gh. trace:TASK-717
#[derive(Debug, Clone)]
pub(super) struct RemoteBranchFacts {
    /// True when the branch is a protected ref (main/master/aida-store).
    pub(super) protected: bool,
    /// True when an open PR currently has this branch as its head.
    pub(super) has_open_pr: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges).
    pub(super) ancestor_of_main: bool,
    /// True when origin/main carries a commit whose subject/trailer references
    /// the branch's spec id (covers squash merges, where the branch tip keeps a
    /// different SHA but the work shipped).
    pub(super) spec_referenced_on_main: bool,
    /// The branch's spec status, when the branch name maps to a known spec and
    /// that spec is Completed or Rejected (terminal). None otherwise.
    pub(super) spec_terminal: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no other "merged" signal means
    /// the branch carries unique unmerged work that must be KEPT.
    pub(super) unique_unmerged_commits: u32,
}

/// Pure squash-aware classification of one remote branch. No git/gh — every
/// input is pre-gathered in `RemoteBranchFacts`. This is the safety model:
/// excludes protected + open-PR branches first; declares safe-to-delete only on
/// a positive "merged/terminal" signal AND zero unique unmerged commits;
// otherwise keeps the branch and flags it. trace:TASK-717
pub(super) fn classify_stale_remote_branch(facts: &RemoteBranchFacts) -> RemoteBranchVerdict {
    // EXCLUDE first — protected and open-PR branches are never candidates.
    if facts.protected {
        return RemoteBranchVerdict::Excluded("protected ref".to_string());
    }
    if facts.has_open_pr {
        return RemoteBranchVerdict::Excluded("has an open PR".to_string());
    }

    // A positive "this work has shipped / is terminal" signal — any one suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("HEAD is an ancestor of origin/main (merged)")
    } else if facts.spec_referenced_on_main {
        Some("origin/main carries a commit referencing its spec (squash-merged)")
    } else if facts.spec_terminal {
        Some("its spec is Completed/Rejected")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged/terminal signal at all → never delete; flag for the operator.
        return RemoteBranchVerdict::Keep(
            "no merge/terminal signal — operator decision".to_string(),
        );
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. an unmerged migration-guide on a squash-merged branch) must
    // be KEPT — deleting it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash/terminal paths.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        return RemoteBranchVerdict::Keep(format!(
            "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
            facts.unique_unmerged_commits
        ));
    }

    RemoteBranchVerdict::SafeToDelete(merged_reason.to_string())
}

/// Derive the candidate spec id from a work-branch name (`task-281-foo` →
/// `TASK-281`). Returns None when the branch doesn't follow the work-branch
// convention. trace:TASK-717
// trace:BUG-888 | ai:codex
pub(super) fn spec_id_from_work_branch(branch: &str) -> Option<String> {
    crate::work_spec_id_from_branch(branch)
}

/// Do the shared branches (code trunk + orphan store) hold the same tip on
/// every configured remote? A mismatch means the substrate has forked across
/// hubs (e.g. a clone pushed the store to gitlab but not github). Uses cheap
/// `ls-remote` (refs only, no object transfer) for current truth; a remote we
/// can't reach is skipped (not a false alarm). Never writes. Emits one finding
/// per diverged branch.
// trace:TASK-1095 | ai:claude
/// STORY-781: check the checked-in project manifest.
///
/// ABSENCE PRODUCES NO FINDING, deliberately and load-bearingly. A project
/// without a manifest keeps working identically; treating its absence as a
/// defect would make every pre-existing repository suddenly "unhealthy" for a
/// file it never agreed to carry, and would turn an optional standard into a
/// nag. Only things a human can act on are reported:
///
///   - malformed  — the file exists but cannot be read
///   - unfilled   — scaffolded and never completed; the blank form this whole
///                  design exists to prevent
///   - drifted    — the recorded repository no longer matches `origin`
///
/// Read-only. Never mutates, never heals.
// trace:STORY-781 | ai:claude
pub(crate) fn scan_project_manifest(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> Vec<DoctorFinding> {
    use aida_core::project_manifest as pm;

    let mut findings = Vec::new();
    let manifest = match pm::load(project_root) {
        // Not a defect. Say nothing.
        pm::ManifestState::Absent => return findings,
        pm::ManifestState::Malformed(msg) => {
            findings.push(DoctorFinding {
                category: "project-manifest".to_string(),
                id: "project-manifest-malformed".to_string(),
                summary: format!("{} could not be read: {msg}", pm::MANIFEST_REL_PATH),
                action: format!(
                    "fix the TOML in {} (schema: docs/internal/project-manifest.md).                      Nothing else is affected — every command keeps working without it",
                    pm::MANIFEST_REL_PATH
                ),
                safe_heal: false,
            });
            return findings;
        }
        pm::ManifestState::Present(m) => m,
    };

    let real_enough_for_identity_nudge =
        project_identity_is_real_enough(project_root, store, &manifest);

    if manifest.is_unfilled() && real_enough_for_identity_nudge {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-manifest-unfilled".to_string(),
            summary: format!(
                "{} was scaffolded but never filled in — nothing is recorded that a scan could not already work out",
                pm::MANIFEST_REL_PATH
            ),
            action: "add at least `why` — the one thing no tool can derive for you".to_string(),
            safe_heal: false,
        });
    }

    // trace:STORY-789 | ai:codex — the thesis lives as a VISION, not as a
    // duplicate manifest field, and doctor only nudges once the project has
    // earned that ask.
    if real_enough_for_identity_nudge && !store.requirements.iter().any(is_project_vision) {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-thesis-missing".to_string(),
            summary: "no project thesis VISION is recorded — the governing bet is still implicit"
                .to_string(),
            action: "create one with `aida add --type vision --title \"Project thesis\"` and phrase it as the bet this project is making".to_string(),
            safe_heal: false,
        });
    }

    // Values this build does not recognise. The FILE still parsed — that is the
    // degrade-not-reject contract — but the author almost certainly believes
    // they said something meaningful, so say that we did not understand it.
    // Note this fires on a genuinely-newer schema too, which is the right
    // trade: "your AIDA is older than this manifest" is worth knowing.
    let mut unknown: Vec<String> = Vec::new();
    if let Some(l) = manifest.project.liveness.as_ref() {
        if !l.is_recognised() {
            unknown.push(format!(
                // toml-ok: diagnostic text, not a TOML writer.
                "liveness = \"{}\" (expected one of: {})",
                l.as_str(),
                pm::Liveness::ALL.join(", ")
            ));
        }
    }
    if let Some(st) = manifest.project.stage.as_ref() {
        if !st.is_recognised() {
            unknown.push(format!(
                // toml-ok: diagnostic text, not a TOML writer.
                "stage = \"{}\" (expected one of: {})",
                st.as_str(),
                pm::Stage::ALL.join(", ")
            ));
        }
    }
    if !unknown.is_empty() {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-manifest-unrecognised-value".to_string(),
            summary: format!(
                "{} uses a value this build does not recognise: {}",
                pm::MANIFEST_REL_PATH,
                unknown.join("; ")
            ),
            action: format!(
                "fix the value, or upgrade `aida` if this manifest was written by a newer one.                  The rest of {} is still being read normally",
                pm::MANIFEST_REL_PATH
            ),
            safe_heal: false,
        });
    }

    // A recorded repository that no longer matches `origin` means the project
    // moved and the manifest is now describing somewhere it does not live.
    if let Some(recorded) = manifest.project.repository.as_deref() {
        if let Some(actual) = aida_core::git_ops::remote_url(project_root, "origin") {
            if !same_remote(recorded, &actual) {
                findings.push(DoctorFinding {
                    category: "project-manifest".to_string(),
                    id: "project-manifest-repository-drift".to_string(),
                    summary: format!(
                        "{} records repository `{recorded}` but origin is `{actual}`",
                        pm::MANIFEST_REL_PATH
                    ),
                    action: format!(
                        "update `repository` in {} to match, or remove it",
                        pm::MANIFEST_REL_PATH
                    ),
                    safe_heal: false,
                });
            }
        }
    }

    findings
}

pub(super) fn is_project_vision(req: &aida_core::Requirement) -> bool {
    req.req_type == aida_core::RequirementType::Vision
}

pub(super) fn project_identity_is_real_enough(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    manifest: &aida_core::project_manifest::ProjectManifest,
) -> bool {
    use aida_core::project_manifest::Liveness;

    if matches!(
        manifest.project.liveness.as_ref(),
        Some(Liveness::Parked | Liveness::Abandoned)
    ) {
        return false;
    }

    if store.requirements.iter().any(|r| {
        r.req_type != aida_core::RequirementType::Meta
            && !r.tags.contains("from-aida-init")
            && !r.title.trim().is_empty()
    }) {
        return true;
    }

    git_commit_count(project_root) >= 3
}

pub(super) fn git_commit_count(project_root: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-list", "--count", "HEAD"])
        .output();
    let Ok(out) = out else {
        return 0;
    };
    if !out.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

// trace:BUG-2672 | ai:codex
#[derive(Default)]
pub(super) struct RemoteDriftScan {
    pub(super) findings: Vec<DoctorFinding>,
    pub(super) info: Vec<String>,
}

/// Fast-forward lag window in seconds, or `None` when this clone runs no
/// bridge. Only a clone whose local layer reads cleanly with `bridge = true`
/// earns grace: with the layer absent, unreadable or `bridge = false` nothing
/// will close the gap, so lag is reported on the first observation. The
/// window is `[doctor.remote_drift] store_lag_minutes`, else twice the
/// bridge interval.
fn lag_window(project_root: &std::path::Path) -> Option<i64> {
    let configured = crate::read_project_config_value(project_root)
        .as_ref()
        .and_then(|cfg| {
            cfg.get("doctor")?
                .get("remote_drift")?
                .get("store_lag_minutes")
        })
        .and_then(toml::Value::as_integer);
    lag_window_for(crate::store_local::load(project_root).ok(), configured)
}

/// Grace for a loaded local layer: `None` (unreadable) or a non-bridge
/// entry earns none; a bridge entry earns [`lag_window_from`].
// trace:BUG-2672 | ai:claude
pub(super) fn lag_window_for(
    local: Option<crate::store_local::StoreLocal>,
    configured_minutes: Option<i64>,
) -> Option<i64> {
    let local = local.filter(|entry| entry.bridge)?;
    Some(lag_window_from(
        configured_minutes,
        local.bridge_every.as_deref(),
    ))
}

// trace:BUG-2672 | ai:claude
pub(super) fn lag_window_from(configured_minutes: Option<i64>, cadence: Option<&str>) -> i64 {
    if let Some(seconds) = configured_minutes
        .filter(|minutes| *minutes > 0)
        .and_then(|minutes| minutes.checked_mul(60))
    {
        return seconds;
    }
    let cadence = cadence.unwrap_or(crate::store_local::DEFAULT_BRIDGE_EVERY);
    crate::maintenance_schedule::parse_duration(cadence)
        .ok()
        .map(|duration| duration.num_seconds())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(3600)
        .saturating_mul(2)
}

/// Remote-scoped settings carried into the isolated fetch.
const FORWARDED_REMOTE_VARS: &[&str] = &[
    "proxy",
    "proxyauthmethod",
    "uploadpack",
    "serveroption",
    "vcs",
];

fn forwarded_transport_key(key: &str, remote: &str, scratch_remote: &str) -> Option<String> {
    let (section, rest) = key.split_once('.')?;
    let section = section.to_ascii_lowercase();
    match section.as_str() {
        "credential" | "http" | "protocol" => Some(key.to_string()),
        "core" => matches!(
            rest.to_ascii_lowercase().as_str(),
            "sshcommand" | "gitproxy"
        )
        .then(|| key.to_string()),
        "ssh" => rest
            .eq_ignore_ascii_case("variant")
            .then(|| key.to_string()),
        "remote" => {
            let (subsection, var) = rest.rsplit_once('.')?;
            (subsection == remote
                && FORWARDED_REMOTE_VARS.contains(&var.to_ascii_lowercase().as_str()))
            .then(|| format!("remote.{scratch_remote}.{var}"))
        }
        _ => None,
    }
}

fn quote_config(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\u{8}' => quoted.push_str("\\b"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Render the repository- and worktree-scoped transport settings of `repo`
/// (SSH command, credential helpers, HTTP and protocol options, and the
/// remote's proxy/upload-pack settings) as config text for the scratch
/// repository, so the isolated fetch reaches the remote as `ls-remote` did.
/// Global and system settings already apply to the scratch fetch. Order is
/// preserved so multi-valued keys such as `credential.helper` resets keep
/// their meaning.
// trace:BUG-2672 | ai:claude
pub(super) fn forwarded_transport_config(
    repo: &std::path::Path,
    remotes: &[(&str, &str)],
) -> Option<String> {
    let output = aida_core::git_ops::host_git_command(repo)
        .args(["config", "--list", "-z", "--show-scope"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut text = String::new();
    let mut entries = output.stdout.split(|byte| *byte == 0);
    while let (Some(scope), Some(entry)) = (entries.next(), entries.next()) {
        if !matches!(scope, b"local" | b"worktree") {
            continue;
        }
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let (key, value) = match entry.split_once('\n') {
            Some((key, value)) => (key, Some(value)),
            None => (entry, None),
        };
        for (remote, scratch_remote) in remotes {
            let Some(key) = forwarded_transport_key(key, remote, scratch_remote) else {
                continue;
            };
            let (section, rest) = key.split_once('.')?;
            let (subsection, name) = match rest.rsplit_once('.') {
                Some((subsection, name)) => (Some(subsection), name),
                None => (None, rest),
            };
            if subsection.is_some_and(|sub| sub.contains('\n')) {
                continue;
            }
            text.push('[');
            text.push_str(section);
            if let Some(subsection) = subsection {
                text.push(' ');
                text.push_str(&quote_config(subsection));
            }
            text.push_str("]\n\t");
            text.push_str(name);
            if let Some(value) = value {
                text.push_str(" = ");
                text.push_str(&quote_config(value));
            }
            text.push('\n');
            // Non-remote keys apply to every fetch; write them once.
            if !key.starts_with("remote.") {
                break;
            }
        }
    }
    Some(text)
}

#[derive(Clone, Copy)]
struct UniqueHistory {
    count: usize,
    oldest: i64,
}

fn read_unique_histories(
    left: &str,
    right: &str,
    git: impl Fn() -> std::process::Command,
) -> Option<[UniqueHistory; 2]> {
    let shallow = git()
        .args(["rev-parse", "--is-shallow-repository"])
        .output()
        .ok()?;
    if !shallow.status.success() || shallow.stdout != b"false\n" {
        return None;
    }
    let range = format!("{left}...{right}");
    let mut cmd = git();
    cmd.env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .args([
            "--no-replace-objects",
            "rev-list",
            "--left-right",
            "--timestamp",
            &range,
            "--",
        ]);
    const HISTORY_OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
    let crate::BoundedCommandOutput::Completed(output) = crate::command_output_with_timeout_capped(
        cmd,
        std::time::Duration::from_secs(15),
        HISTORY_OUTPUT_LIMIT,
    ) else {
        return None;
    };
    if !output.status.success() || output.stdout.len() as u64 > HISTORY_OUTPUT_LIMIT {
        return None;
    }
    let mut histories = [UniqueHistory {
        count: 0,
        oldest: i64::MAX,
    }; 2];
    for line in std::str::from_utf8(&output.stdout).ok()?.lines() {
        let (time, commit) = line.split_once(' ')?;
        let history = match commit.as_bytes().first()? {
            b'<' => &mut histories[0],
            b'>' => &mut histories[1],
            _ => return None,
        };
        history.count += 1;
        history.oldest = history.oldest.min(time.parse().ok()?);
    }
    (histories.iter().any(|history| history.count > 0)).then_some(histories)
}

fn compare_remote_tips(
    repo: &std::path::Path,
    branch: &str,
    [(left, left_sha), (right, right_sha)]: [(&str, &str); 2],
) -> Option<[UniqueHistory; 2]> {
    use aida_core::git_ops::{host_git_command, host_remote_git_command, remote_url};
    if let Some(histories) = read_unique_histories(left_sha, right_sha, || host_git_command(repo)) {
        return Some(histories);
    }
    if branch == "aida-store" {
        let store = repo.join(".aida-store");
        if store.is_dir() {
            if let Some(histories) =
                read_unique_histories(left_sha, right_sha, || host_git_command(&store))
            {
                return Some(histories);
            }
        }
    }
    // A remote may advertise commits the checkout has never fetched. Keep the
    // fallback's objects, refs and fetch state in an invocation-private repo.
    let scratch = tempfile::tempdir().ok()?;
    let format = if left_sha.len() == 64 {
        "sha256"
    } else {
        "sha1"
    };
    let isolate = |cmd: &mut std::process::Command| {
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_SHALLOW_FILE");
    };
    let mut init = host_git_command(scratch.path());
    isolate(&mut init);
    if !init
        .args([
            "init",
            "--bare",
            "--template=",
            &format!("--object-format={format}"),
        ])
        .output()
        .ok()?
        .status
        .success()
    {
        return None;
    }
    // The scratch fetch reads only global and system config on its own;
    // carry the source repository's transport settings (an SSH identity, a
    // local credential helper, a remote's proxy or upload-pack) into it.
    // trace:BUG-2672 | ai:claude
    let scratch_remotes = [(left, "aida-drift-0"), (right, "aida-drift-1")];
    let transport = forwarded_transport_config(repo, &scratch_remotes)?;
    {
        use std::io::Write as _;
        let mut config = std::fs::OpenOptions::new()
            .append(true)
            .open(scratch.path().join("config"))
            .ok()?;
        config.write_all(transport.as_bytes()).ok()?;
    }
    for (remote, scratch_remote) in scratch_remotes {
        let mut cmd = host_remote_git_command(repo, remote);
        isolate(&mut cmd);
        cmd.arg("--git-dir")
            .arg(scratch.path())
            .env("AIDA_DRIFT_FETCH_URL", remote_url(repo, remote)?)
            .arg(format!(
                "--config-env=remote.{scratch_remote}.url=AIDA_DRIFT_FETCH_URL"
            ))
            .args([
                "fetch",
                "--no-tags",
                "--no-auto-maintenance",
                "--no-recurse-submodules",
                "--no-write-fetch-head",
                "--refmap=",
                "--",
                scratch_remote,
                &format!("refs/heads/{branch}"),
            ]);
        let output = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(30))?;
        if !output.status.success() {
            return None;
        }
    }
    // Compare the advertised SHAs. A branch move during fetch that drops an
    // advertised commit leaves the history unverifiable.
    read_unique_histories(left_sha, right_sha, || {
        let mut cmd = host_git_command(scratch.path());
        isolate(&mut cmd);
        cmd
    })
}

pub(super) fn scan_remote_drift(project_root: &std::path::Path) -> RemoteDriftScan {
    let mut scan = RemoteDriftScan::default();
    let findings = &mut scan.findings;
    let remotes: Vec<String> = aida_core::git_ops::list_remotes(project_root)
        .into_iter()
        .filter(|r| r != "all")
        .collect();
    // BUG-1796: a single origin is enough to strand canonical store commits;
    // report the count from the local origin tracking ref even when there are
    // no mirror remotes to compare.
    let store_path = project_root.join(".aida-store");
    if let Some((ahead, _)) = crate::orphan_branch_sync_state(&store_path) {
        if ahead > 0 {
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: "orphan-store-ahead-of-origin".to_string(),
                summary: format!(
                    "orphan store is {ahead} commit{} ahead of origin ({ahead} unpushed)",
                    if ahead == 1 { "" } else { "s" }
                ),
                action: "run `aida push` to publish the orphan store commits".to_string(),
                safe_heal: false,
            });
        }
    }
    if remotes.len() < 2 {
        return scan;
    }
    let window = lag_window(project_root);
    let now = chrono::Utc::now().timestamp();
    for branch in ["main", "aida-store"] {
        let tips: Vec<_> = remotes
            .iter()
            .filter_map(|remote| {
                aida_core::git_ops::remote_branch_head_sha(project_root, remote, branch)
                    .map(|sha| (remote.as_str(), sha))
            })
            .collect();
        for (index, (left, left_sha)) in tips.iter().enumerate() {
            for (right, right_sha) in &tips[index + 1..] {
                let label = format!(
                    "branch `{branch}` ({left}={}, {right}={})",
                    &left_sha[..12],
                    &right_sha[..12]
                );
                if left_sha == right_sha {
                    scan.info.push(format!("{label} is in-sync"));
                    continue;
                }

                let histories = compare_remote_tips(
                    project_root,
                    branch,
                    [(left, left_sha), (right, right_sha)],
                );
                // Divergence always fails; only fast-forward lag on either
                // branch earns the bridge window. trace:BUG-2672 | ai:claude
                let (mut summary, action, lag) = match histories {
                    Some([left_history, right_history])
                        if left_history.count > 0 && right_history.count > 0 =>
                    {
                        (
                            format!(
                                "{label} diverged ({left}: {} unique, {right}: {} unique)",
                                left_history.count, right_history.count
                            ),
                            "inspect with `aida remote status` and reconcile by hand",
                            Some((left_history.oldest.min(right_history.oldest), false)),
                        )
                    }
                    Some([left_history, right_history]) => {
                        let (behind, ahead, missing) = if left_history.count == 0 {
                            (left, right, right_history)
                        } else {
                            (right, left, left_history)
                        };
                        (
                            format!(
                                "{label}: {behind} is {} commit(s) behind {ahead} (fast-forward)",
                                missing.count,
                            ),
                            "check the bridge cron or run `aida remote mirror-sync`",
                            Some((missing.oldest, true)),
                        )
                    }
                    None => (
                        format!("{label} has different tips; could not verify their history"),
                        "check remote access and retry the drift check",
                        None,
                    ),
                };
                if let Some((oldest, fast_forward)) = lag {
                    let age = now.saturating_sub(oldest).max(0);
                    summary.push_str(&format!(
                        ", oldest missing commit {}m {}s old",
                        age / 60,
                        age % 60
                    ));
                    if let Some(window) = window.filter(|w| fast_forward && age < *w) {
                        scan.info.push(format!(
                            "{summary}; wait for the bridge ({}m window)",
                            window / 60
                        ));
                        continue;
                    }
                }
                findings.push(DoctorFinding {
                    category: "remote-drift".to_string(),
                    id: if remotes.len() == 2 {
                        format!("remote-drift-{branch}")
                    } else {
                        format!("remote-drift-{branch}:{left}:{right}")
                    },
                    summary,
                    action: action.to_string(),
                    safe_heal: false,
                });
            }
        }
    }
    // trace:STORY-1048 | ai:codex
    if let Some(tag) = crate::remote_create::latest_local_release_tag(project_root) {
        let standings =
            crate::remote_create::collect_release_standings(project_root, &remotes, &tag);
        let missing_tags: Vec<String> = standings
            .iter()
            .filter(|s| s.tag_head.is_none())
            .map(|s| s.remote.clone())
            .collect();
        if !missing_tags.is_empty() {
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-release-tag-{tag}"),
                summary: format!(
                    "release tag `{tag}` is missing on remote(s): {}",
                    missing_tags.join(", ")
                ),
                action: format!(
                    "push the release tag to every hub: {}",
                    missing_tags
                        .iter()
                        .map(|r| format!("`git push {r} {tag}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                safe_heal: false,
            });
        }

        let missing_releases: Vec<String> = standings
            .iter()
            .filter(|s| matches!(s.gitlab_release_exists, Some(false)))
            .map(|s| s.remote.clone())
            .collect();
        if !missing_releases.is_empty() {
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-gitlab-release-{tag}"),
                summary: format!(
                    "GitLab release `{tag}` is missing on remote(s): {}",
                    missing_releases.join(", ")
                ),
                action: "rerun the mirrored tag pipeline or create the GitLab release with the four uploaded assets".to_string(),
                safe_heal: false,
            });
        }
    }
    scan
}

/// TASK-717: scan stale `origin/*` branches and classify each under the
/// squash-aware safety model. Read-only — performs git ancestry/rev-list probes
/// and one `gh pr list` for the open-PR set, never mutates anything. Returns a
/// `stale-remote-branches` finding for every branch that is either SafeToDelete
/// (verdict carries the merge reason; `safe_heal=false` so deletion stays gated
/// behind --yes --force) or Keep (flagged for the operator, never auto-healed).
// Excluded branches produce no finding. trace:TASK-717
pub(super) fn scan_stale_remote_branches(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) -> Vec<DoctorFinding> {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };

    // No resolvable default branch → we cannot corroborate merges, so stay
    // silent rather than risk flagging every remote branch.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };

    // Spec ids referenced by commits on origin/main (squash-merge signal).
    let mut referenced_on_main: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    if let Some(log) = git(&["log", "--pretty=format:%s", &default_ref]) {
        for subject in log.lines() {
            let subject = subject.trim();
            if subject.is_empty() || is_plan_commit_subject(subject) {
                continue;
            }
            for id in extract_spec_ids_from_commit(subject) {
                referenced_on_main.insert(id.to_ascii_uppercase());
            }
        }
    }

    let open_pr_branches = collect_open_prs(project_root).by_branch;
    const PROTECTED: &[&str] = &["main", "master", "aida-store", "HEAD"];

    let mut findings = Vec::new();
    for rc in collect_remote_branch_commits(project_root) {
        let branch = rc.branch;
        let protected = PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(&branch));
        let has_open_pr = open_pr_branches.contains_key(&branch);

        // Ancestor-of-main check: empty `origin/main..origin/branch` rev-list.
        let ancestor_of_main = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|n| n == 0)
        .unwrap_or(false);

        // unique_unmerged_commits == ancestor-count above; reuse it.
        let unique_unmerged_commits = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

        let spec_id = spec_id_from_work_branch(&branch);
        let spec_referenced_on_main = spec_id
            .as_deref()
            .map(|id| referenced_on_main.contains(&id.to_ascii_uppercase()))
            .unwrap_or(false);
        let spec_terminal = spec_id
            .as_deref()
            .and_then(|id| store.get_requirement_by_spec_id(id))
            .map(|req| {
                matches!(
                    req.status,
                    RequirementStatus::Completed | RequirementStatus::Rejected
                )
            })
            .unwrap_or(false);

        let facts = RemoteBranchFacts {
            protected,
            has_open_pr,
            ancestor_of_main,
            spec_referenced_on_main,
            spec_terminal,
            unique_unmerged_commits,
        };

        match classify_stale_remote_branch(&facts) {
            RemoteBranchVerdict::Excluded(_) => {
                // Protected / open-PR — never surfaced as a finding.
            }
            RemoteBranchVerdict::SafeToDelete(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` is stale ({reason})"),
                    action: format!(
                        "delete remote branch (`aida doctor --heal --category \
                         stale-remote-branches --yes --force`, or `git push origin \
                         --delete {branch}`)"
                    ),
                    // Remote deletion is destructive — gate behind --yes --force.
                    safe_heal: false,
                });
            }
            RemoteBranchVerdict::Keep(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` flagged: {reason}"),
                    action: "operator decision: review and keep, open a PR, or delete by hand"
                        .to_string(),
                    // Never auto-deleted — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// TASK-717: delete one stale remote branch via `git push origin --delete`.
/// Only reachable when the finding's action surfaced it as SafeToDelete and the
/// caller passed --yes --force (see `heal_doctor_finding`). A Keep-flagged
// branch never routes here — its action string is operator-only. trace:TASK-717
pub(super) fn heal_doctor_stale_remote_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // Keep-flagged findings carry an operator-only action and must never be
    // auto-deleted, even under --yes --force.
    if !finding.action.contains("--category stale-remote-branches") {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("flagged for operator decision — not auto-deleted".to_string()),
        });
    }
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // The id is a remote branch name; end options before it.
        // trace:BUG-1622 | ai:claude
        .args([
            "push",
            "--delete",
            git_arg_guard::END_OF_OPTIONS,
            "origin",
            &finding.id,
        ])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status()
        .with_context(|| format!("deleting remote branch origin/{}", finding.id))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted stale remote branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}
