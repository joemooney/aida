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

use std::path::{Path, PathBuf};

/// Machine-readable reason for a merge hold. `Legacy` is only returned for
/// markers written before typed holds existed; every newly written marker has
/// one of the three actionable kinds.
// trace:STORY-1397 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HoldKind {
    Recusal,
    Rework,
    Decision,
    Legacy,
}

impl HoldKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Recusal => "recusal",
            Self::Rework => "rework",
            Self::Decision => "decision",
            Self::Legacy => "legacy",
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "recusal" => Self::Recusal,
            "rework" => Self::Rework,
            "decision" => Self::Decision,
            _ => Self::Legacy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HoldRecord {
    pub(crate) pr: u64,
    pub(crate) reason: String,
    pub(crate) kind: HoldKind,
    pub(crate) recused: Option<String>,
}

fn holds_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("merge-holds")
}

/// The marker path for one PR. Public so callers can log it.
pub(crate) fn hold_path(project_root: &Path, pr: u64) -> PathBuf {
    holds_dir(project_root).join(format!("PR-{pr}"))
}

/// Record a supervised merge-hold for `pr` with a human-readable reason.
/// Idempotent — re-recording refreshes the reason.
pub(crate) fn write_hold(project_root: &Path, pr: u64, reason: &str) -> std::io::Result<()> {
    write_typed_hold(project_root, pr, HoldKind::Decision, None, reason)
}

/// Write a typed marker while keeping the first-line reason format understood
/// by older binaries. Metadata is line-oriented so the safety chokepoint still
/// treats even a partially written or future-version marker as held.
// trace:STORY-1397 | ai:codex
pub(crate) fn write_typed_hold(
    project_root: &Path,
    pr: u64,
    kind: HoldKind,
    recused: Option<&str>,
    reason: &str,
) -> std::io::Result<()> {
    let dir = holds_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let reason = reason.trim();
    let reason = if reason.is_empty() {
        format!("PR-{pr} is under a supervised merge-hold\n")
    } else {
        format!("{reason}\n")
    };
    let mut body = format!("{reason}kind: {}\n", kind.as_str());
    if let Some(who) = recused.map(str::trim).filter(|s| !s.is_empty()) {
        body.push_str(&format!("recused: {who}\n"));
    }
    std::fs::write(hold_path(project_root, pr), body)
}

/// Parse the complete typed record. Unknown metadata and legacy files are
/// tolerated; marker presence remains the only merge-blocking signal.
// trace:STORY-1397 | ai:codex
pub(crate) fn read_record(project_root: &Path, pr: u64) -> Option<HoldRecord> {
    let reason = read_hold(project_root, pr)?;
    let body = std::fs::read_to_string(hold_path(project_root, pr)).unwrap_or_default();
    let metadata = |key: &str| {
        body.lines().skip(1).find_map(|line| {
            line.strip_prefix(key)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    };
    Some(HoldRecord {
        pr,
        reason,
        kind: metadata("kind:")
            .map(HoldKind::parse)
            .unwrap_or(HoldKind::Legacy),
        recused: metadata("recused:").map(str::to_string),
    })
}

/// The hold reason if `pr` is under a supervised merge-hold, else `None`.
/// The merge chokepoint refuses whenever this is `Some`.
///
/// TASK-1238: fails CLOSED on a present-but-unreadable marker. `NotFound` is the
/// only "no hold" answer; ANY other read error (permissions, a directory in its
/// place, a transient IO fault) means a marker may be there but we can't confirm
/// it isn't — so we HOLD. The prior `.ok()?` collapsed every error to `None`,
/// which would let a merge through when the marker was present but unreadable —
/// the exact fail-open class this whole marker exists to prevent.
pub(crate) fn read_hold(project_root: &Path, pr: u64) -> Option<String> {
    match std::fs::read_to_string(hold_path(project_root, pr)) {
        Ok(body) => {
            // BUG-1236: the marker's first line is the reason; a second
            // `label: …` line records the Layer-2 label sync state.
            let reason = body.lines().next().unwrap_or("").trim();
            Some(if reason.is_empty() {
                format!("PR-{pr} is under a supervised merge-hold")
            } else {
                reason.to_string()
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(format!(
            "PR-{pr} merge-hold marker present but unreadable — held for safety"
        )),
    }
}

/// Clear the hold — an explicit human/advisor review. Idempotent: clearing a
/// PR with no hold is a no-op success.
pub(crate) fn clear_hold(project_root: &Path, pr: u64) -> std::io::Result<()> {
    match std::fs::remove_file(hold_path(project_root, pr)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Every active hold as `(pr_number, reason)`, ascending by PR.
// Consumed by the follow-up `aida merge-hold list` surface; kept here so the
// primitive lands with the safety core.
#[allow(dead_code)]
pub(crate) fn list_holds(project_root: &Path) -> Vec<(u64, String)> {
    list_records(project_root)
        .into_iter()
        .map(|record| (record.pr, record.reason))
        .collect()
}

// trace:STORY-1397 | ai:codex
pub(crate) fn list_records(project_root: &Path) -> Vec<HoldRecord> {
    let dir = holds_dir(project_root);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(n) = name.strip_prefix("PR-").and_then(|s| s.parse::<u64>().ok()) {
                if let Some(record) = read_record(project_root, n) {
                    out.push(record);
                }
            }
        }
    }
    out.sort_by_key(|record| record.pr);
    out
}

/// The GitHub label mirroring the merge-hold marker for ADR-37 Layer 2 (a
/// required status check keyed to this label blocks a raw/stale/UI merge the
/// client chokepoint never sees). trace on the item below stays a plain comment.
// trace:BUG-1167 | ai:claude
pub(crate) const HOLD_LABEL: &str = "aida:merge-hold";

/// Whether the forge change currently carries the Layer-2 merge-hold label.
///
/// This is intentionally a live forge read rather than marker metadata: an
/// operator may add the label directly, leaving no Layer-1 marker to inspect.
/// Errors are returned so callers keep their existing fail-closed behavior.
// trace:TASK-1287 | ai:codex
pub(crate) fn label_present(project_root: &Path, pr: u64) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    let (cli, args): (&str, Vec<String>) = match kind {
        crate::forge::ForgeKind::GitHub => (
            "gh",
            vec![
                "pr".into(),
                "view".into(),
                pr.to_string(),
                "--json".into(),
                "labels".into(),
                "--jq".into(),
                format!("any(.labels[]; .name == \"{HOLD_LABEL}\")"),
            ],
        ),
        crate::forge::ForgeKind::GitLab => (
            "glab",
            vec![
                "mr".into(),
                "view".into(),
                pr.to_string(),
                "--output".into(),
                "json".into(),
            ],
        ),
        crate::forge::ForgeKind::None => return Ok(false),
    };
    let (ok, stdout) = run_forge_cli_stdout(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    match kind {
        crate::forge::ForgeKind::GitHub => Ok(stdout.trim().eq_ignore_ascii_case("true")),
        crate::forge::ForgeKind::GitLab => labels_json_contains(&stdout, HOLD_LABEL),
        crate::forge::ForgeKind::None => Ok(false),
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

/// BUG-1236: whether the `aida:merge-hold` label on the change mirrors the
/// marker. Recorded on the marker's second line so `aida merge-hold list`
/// can show it without a network call and `--fix` can re-sync it.
// trace:BUG-1236 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelState {
    Synced,
    Unsynced(String),
    Unknown,
}

impl LabelState {
    pub(crate) fn render(&self) -> String {
        match self {
            LabelState::Synced => "label: synced".to_string(),
            LabelState::Unsynced(err) => format!("label: UNSYNCED — {err}"),
            LabelState::Unknown => "label: unknown".to_string(),
        }
    }
}

/// Record the label sync state on an existing marker (no-op without one).
// trace:BUG-1236 | ai:claude
pub(crate) fn record_label_state(
    project_root: &Path,
    pr: u64,
    state: &LabelState,
) -> std::io::Result<()> {
    let path = hold_path(project_root, pr);
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let reason = body.lines().next().unwrap_or("").trim();
    let line = match state {
        LabelState::Synced => "label: synced".to_string(),
        LabelState::Unsynced(err) => {
            format!("label: unsynced: {}", err.lines().next().unwrap_or(""))
        }
        LabelState::Unknown => String::new(),
    };
    let metadata: Vec<&str> = body
        .lines()
        .skip(1)
        .filter(|existing| !existing.trim_start().starts_with("label:"))
        .collect();
    let mut out = format!("{reason}\n");
    for existing in metadata {
        out.push_str(existing);
        out.push('\n');
    }
    if !line.is_empty() {
        out.push_str(&line);
        out.push('\n');
    }
    std::fs::write(path, out)
}

/// The recorded label state for `pr` (Unknown when the marker predates
/// BUG-1236 or carries no state line).
// trace:BUG-1236 | ai:claude
pub(crate) fn read_label_state(project_root: &Path, pr: u64) -> LabelState {
    let Ok(body) = std::fs::read_to_string(hold_path(project_root, pr)) else {
        return LabelState::Unknown;
    };
    match body
        .lines()
        .skip(1)
        .map(str::trim)
        .find(|line| line.starts_with("label:"))
    {
        Some("label: synced") => LabelState::Synced,
        Some(l) if l.starts_with("label: unsynced:") => {
            LabelState::Unsynced(l["label: unsynced:".len()..].trim().to_string())
        }
        _ => LabelState::Unknown,
    }
}

/// Best-effort mirror of the marker state to the `aida:merge-hold` label on the
/// change (PR/MR), so Layer 2 can enforce server-side. Failures are swallowed:
/// the file marker is the source of truth; the label is a convenience mirror and
/// its absence only weakens Layer 2, never the client chokepoint.
///
/// STORY-1165: forge-routed. GitHub → `gh pr edit --add-label/--remove-label`;
/// GitLab → `glab mr update --label/--unlabel`; pure-git → no-op (no forge to
/// label). This is what lets the GitLab merge-hold-gate CI job (the Layer-2
/// analog of merge-hold-gate.yml) see the label on an MR.
// trace:BUG-1167 | ai:claude (STORY-1165 forge-routes it)
pub(crate) fn sync_label(project_root: &Path, pr: u64, held: bool) -> Result<(), String> {
    sync_label_with(project_root, pr, held, run_forge_cli)
}

/// BUG-1236: the real sync loop with the forge CLI injected. Retries once on
/// failure, records the outcome on the marker (`label: synced` /
/// `label: unsynced: <err>`) when holding, and RETURNS the failure instead of
/// swallowing it — every caller prints it, `aida merge-hold list` shows it,
/// and `--fix` re-syncs it. Before this the label silently never landed on
/// three supervised PRs while the required merge-hold-gate check read pass.
// trace:BUG-1236 | ai:claude
pub(crate) fn sync_label_with(
    project_root: &Path,
    pr: u64,
    held: bool,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<(), String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    let Some((cli, args)) = sync_label_command(kind, pr, held) else {
        // pure-git has no forge to carry a label; the file marker still holds.
        return Ok(());
    };
    let mut last_err = String::new();
    for attempt in 0..2 {
        match runner(project_root, cli, &args) {
            Ok((true, _)) => {
                if held {
                    let _ = record_label_state(project_root, pr, &LabelState::Synced);
                }
                return Ok(());
            }
            Ok((false, stderr)) => {
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
    Err(format!("`{cli} {}` failed: {last_err}", args.join(" ")))
}

fn run_forge_cli(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output()
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
        .output()
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
fn sync_label_command(
    kind: crate::forge::ForgeKind,
    pr: u64,
    held: bool,
) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match kind {
        ForgeKind::GitHub => {
            let flag = if held {
                "--add-label"
            } else {
                "--remove-label"
            };
            Some((
                "gh",
                vec![
                    "pr".into(),
                    "edit".into(),
                    pr.to_string(),
                    flag.into(),
                    HOLD_LABEL.into(),
                ],
            ))
        }
        ForgeKind::GitLab => {
            let flag = if held { "--label" } else { "--unlabel" };
            Some((
                "glab",
                vec![
                    "mr".into(),
                    "update".into(),
                    pr.to_string(),
                    flag.into(),
                    HOLD_LABEL.into(),
                ],
            ))
        }
        ForgeKind::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // STORY-1165: the label mirror must route to the right forge CLI — gh for
    // GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
    #[test]
    fn sync_label_command_routes_per_forge() {
        use crate::forge::ForgeKind;
        let (cli, args) = sync_label_command(ForgeKind::GitHub, 42, true).unwrap();
        assert_eq!(cli, "gh");
        assert!(
            args.contains(&"--add-label".to_string()) && args.contains(&HOLD_LABEL.to_string())
        );

        let (cli, args) = sync_label_command(ForgeKind::GitLab, 42, true).unwrap();
        assert_eq!(cli, "glab");
        assert!(args.contains(&"mr".to_string()) && args.contains(&"update".to_string()));
        assert!(args.contains(&"--label".to_string()) && args.contains(&HOLD_LABEL.to_string()));

        let (_, args) = sync_label_command(ForgeKind::GitLab, 42, false).unwrap();
        assert!(
            args.contains(&"--unlabel".to_string()),
            "unheld → remove the label"
        );

        assert!(
            sync_label_command(ForgeKind::None, 42, true).is_none(),
            "pure-git has no forge to label"
        );
    }

    #[test]
    fn parses_gitlab_label_shapes() {
        assert!(
            labels_json_contains(r#"{"labels":["bug","aida:merge-hold"]}"#, HOLD_LABEL).unwrap()
        );
        assert!(
            labels_json_contains(r#"{"labels":[{"name":"aida:merge-hold"}]}"#, HOLD_LABEL).unwrap()
        );
        assert!(!labels_json_contains(r#"{"labels":["bug"]}"#, HOLD_LABEL).unwrap());
    }

    #[test]
    fn write_read_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // No hold initially.
        assert!(read_hold(root, 42).is_none());
        // Write a hold → read returns the reason.
        write_hold(
            root,
            42,
            "STORY-1155 is marked drive — merge requires review",
        )
        .unwrap();
        let reason = read_hold(root, 42).expect("hold must be present");
        assert!(reason.contains("drive"), "{reason}");
        // Clear → gone.
        clear_hold(root, 42).unwrap();
        assert!(read_hold(root, 42).is_none());
        // Clearing an absent hold is a no-op success.
        clear_hold(root, 42).unwrap();
    }

    // trace:STORY-1397 | ai:codex
    #[test]
    fn typed_recusal_round_trips_and_label_updates_preserve_metadata() {
        let dir = tempfile::tempdir().unwrap();
        write_typed_hold(
            dir.path(),
            1397,
            HoldKind::Recusal,
            Some("advisor-1"),
            "author must not merge",
        )
        .unwrap();
        record_label_state(dir.path(), 1397, &LabelState::Synced).unwrap();

        let record = read_record(dir.path(), 1397).unwrap();
        assert_eq!(record.kind, HoldKind::Recusal);
        assert_eq!(record.recused.as_deref(), Some("advisor-1"));
        assert_eq!(record.reason, "author must not merge");
        assert_eq!(read_label_state(dir.path(), 1397), LabelState::Synced);
    }

    #[test]
    fn legacy_marker_remains_fail_closed_and_queryable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
        std::fs::write(hold_path(dir.path(), 3), "old free-text reason\n").unwrap();
        let record = read_record(dir.path(), 3).unwrap();
        assert_eq!(record.kind, HoldKind::Legacy);
        assert_eq!(record.reason, "old free-text reason");
    }

    #[test]
    fn empty_reason_still_holds_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 7, "   ").unwrap();
        // An empty/whitespace reason must STILL register as a hold — the marker's
        // presence is the signal, never its content. A blank marker that merged
        // would be the fail-open bug this fix exists to prevent.
        assert!(
            read_hold(root, 7).is_some(),
            "a marker with a blank reason must still hold"
        );
    }

    #[test]
    fn present_but_unreadable_marker_holds_fail_closed() {
        // TASK-1238: a marker that EXISTS but can't be read as a file must HOLD,
        // not merge. Simulate it by putting a directory where the marker file
        // would be — `read_to_string` then errors with something other than
        // NotFound. The old `.ok()?` returned None here (fail-open → merge); the
        // fix must return Some.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(hold_path(root, 99)).unwrap();
        assert!(
            read_hold(root, 99).is_some(),
            "a present-but-unreadable marker must HOLD (fail closed), never merge"
        );
    }

    #[test]
    fn list_holds_enumerates_ascending() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 30, "b").unwrap();
        write_hold(root, 5, "a").unwrap();
        let holds = list_holds(root);
        assert_eq!(
            holds.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![5, 30]
        );
    }

    // trace:BUG-1236 | ai:claude
    #[test]
    fn sync_failure_is_returned_and_recorded_on_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A git repo with a GitHub origin so the forge resolves to GitHub.
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
        write_hold(root, 7, "drive").unwrap();
        let calls = std::cell::Cell::new(0);
        let err = sync_label_with(root, 7, true, |_, _, _| {
            calls.set(calls.get() + 1);
            Ok((false, "gh: HTTP 502 bad gateway\n".to_string()))
        })
        .unwrap_err();
        assert_eq!(calls.get(), 2, "one retry");
        assert!(err.contains("502"), "{err}");
        assert_eq!(
            read_label_state(root, 7),
            LabelState::Unsynced("gh: HTTP 502 bad gateway".to_string())
        );
        assert_eq!(
            read_hold(root, 7).as_deref(),
            Some("drive"),
            "reason line untouched"
        );
        // A later successful sync flips the state.
        sync_label_with(root, 7, true, |_, _, _| Ok((true, String::new()))).unwrap();
        assert_eq!(read_label_state(root, 7), LabelState::Synced);
    }

    // trace:BUG-1236 | ai:claude
    #[test]
    fn markers_without_a_state_line_read_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        write_hold(dir.path(), 9, "guided").unwrap();
        assert_eq!(read_label_state(dir.path(), 9), LabelState::Unknown);
        assert_eq!(read_label_state(dir.path(), 10), LabelState::Unknown);
    }
}
