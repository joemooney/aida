//! `aida autoprogress` — the robust unattended autonomous-progress entry point.
//!
//! One tested command that safely drains the approved ready set from ANYWHERE,
//! with none of the hand-rolled-cron footguns (TASK-1231): it resolves the
//! project explicitly (cwd/wrong-store-proof), skips if a drain is already
//! running (single-drain-lock), and drains via the reliable single-spec claim
//! form — never the count form (grabs archived defaults) or the batch form
//! (role-queue mismatch). `--groom` additionally approves the safe fence first.
//!
//! Drain-only by default; the durable cron line is `aida autoprogress --groom
//! --project <path>`.
// trace:TASK-1231 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(crate) struct AutoprogressOpts {
    /// Explicit project path (the cron uses this — no cwd dependency). When
    /// None, resolve from the current directory (interactive use).
    pub(crate) project: Option<String>,
    /// Approve the safe fence (advisor intake) before draining.
    pub(crate) groom: bool,
    /// Drain at most this many ready specs this pass.
    pub(crate) max: usize,
    /// Report what it WOULD do without draining.
    pub(crate) dry_run: bool,
}

pub(crate) fn handle_autoprogress(opts: AutoprogressOpts) -> Result<()> {
    // 1. Resolve the project EXPLICITLY — never groom a random store. An
    //    explicit --project wins; otherwise walk up from cwd. Either way we
    //    verify it is a git-canonical AIDA project before touching anything.
    let root = resolve_project(opts.project.as_deref())?;

    // 2. Single-drain-lock: skip cleanly if a drain is already running. A stale
    //    lock (crashed drain) is left for queue-work's own stale-reclaim.
    if let crate::drain_lock::LockStatus::Running(_) = crate::drain_lock::probe_lock(&root) {
        println!("autoprogress: a drain is already running — skipping this pass");
        return Ok(());
    }

    // 3. Optional groom: approve the safe fence (do-not-approve classes,
    //    needs-human, keystones, deferred, risk>medium are all fenced by groom
    //    itself — it can never bless them).
    if opts.groom && !opts.dry_run {
        if let Err(e) = run_aida(&root, &["groom", "--apply", "--risk", "medium"]) {
            eprintln!("autoprogress: groom pass failed ({e}) — continuing to drain what is already approved");
        }
    }

    // 4. Select the approved ready set (implementable types only, bounded).
    let specs = ready_specs(&root, opts.max)?;
    if specs.is_empty() {
        println!("autoprogress: no approved ready work to drain");
        return Ok(());
    }

    if opts.dry_run {
        println!(
            "autoprogress (dry-run): would drain {} ready spec(s): {}",
            specs.len(),
            specs.join(", ")
        );
        return Ok(());
    }

    // 5. Drain each via the reliable SINGLE-SPEC claim form, serially. The
    //    single-drain-lock already serialises them; running sequentially keeps
    //    it explicit and lets one failure not abort the rest.
    let mut drained = 0usize;
    for spec in &specs {
        match run_aida(
            &root,
            &[
                "queue",
                "work",
                spec,
                "--auto-complete",
                "--no-human=both",
                "--force-claim",
            ],
        ) {
            Ok(()) => drained += 1,
            Err(e) => eprintln!("autoprogress: {spec} did not complete ({e})"),
        }
    }
    println!(
        "autoprogress: drained {} of {} ready spec(s)",
        drained,
        specs.len()
    );
    Ok(())
}

/// Resolve + validate the project root. An explicit path is canonicalised and
/// checked; otherwise we walk up from cwd. Refuses anything that is not a
/// git-canonical AIDA project (`.aida/config.toml`) — this is the wrong-store
/// guard that the hand-rolled cron lacked.
pub(crate) fn resolve_project(explicit: Option<&str>) -> Result<PathBuf> {
    let root = match explicit {
        Some(p) => {
            let path = PathBuf::from(p);
            let canon = path
                .canonicalize()
                .with_context(|| format!("--project path `{p}` does not exist"))?;
            crate::main_worktree_root_from(&canon)
        }
        None => crate::find_main_worktree_root().context(
            "no AIDA project found from the current directory — pass --project <path> \
             (autoprogress refuses to groom a random store)",
        )?,
    };
    if !root.join(".aida").join("config.toml").exists() {
        anyhow::bail!(
            "{} is not a git-canonical AIDA project (.aida/config.toml missing) — refusing to run",
            root.display()
        );
    }
    Ok(root)
}

/// Run the `aida` binary (self) with the project as cwd. Reuses the tested CLI
/// behaviour instead of reimplementing groom/drive.
fn run_aida(root: &Path, args: &[&str]) -> Result<()> {
    let exe = crate::aida_exe_path();
    let status = std::process::Command::new(exe)
        .current_dir(root)
        .args(args)
        .status()
        .with_context(|| format!("failed to run `aida {}`", args.join(" ")))?;
    anyhow::ensure!(
        status.success(),
        "`aida {}` exited non-zero",
        args.join(" ")
    );
    Ok(())
}

/// The approved ready set via `aida list` (tested), filtered + bounded here.
fn ready_specs(root: &Path, max: usize) -> Result<Vec<String>> {
    let exe = crate::aida_exe_path();
    let out = std::process::Command::new(exe)
        .current_dir(root)
        .args(["list", "--status", "approved", "--format", "json"])
        .output()
        .context("failed to run `aida list`")?;
    anyhow::ensure!(out.status.success(), "`aida list --status approved` failed");
    Ok(select_ready_from_json(
        &String::from_utf8_lossy(&out.stdout),
        max,
    ))
}

/// PURE: from `aida list --format json`, keep only the READY SET and bound to
/// `max`. A spec is ready when it is (1) an IMPLEMENTABLE type (a
/// decision/epic/folder/meta/principle/vision/constraint/term/doc is never
/// drained), (2) NOT fenced to a supervised execution_mode (an explicit
/// non-drain mode fences the *implementation*, not just the merge, so an
/// unattended pass must never headless-drive it; explicit drain stays
/// drainable), and (3) for ungroomed/unset mode only, NOT keystone-tagged
/// (defense-in-depth). Unit-tested directly.
// trace:TASK-1231 BUG-1157 | ai:codex
pub(crate) fn select_ready_from_json(json: &str, max: usize) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    // The list surface emits either a bare array or `{ "specs": [...] }`.
    let items = value
        .as_array()
        .cloned()
        .or_else(|| value.get("specs").and_then(|s| s.as_array()).cloned())
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|it| {
            // `aida list --format json` emits `spec_id` + `req_type`; tolerate
            // the alternate keys too.
            let id = it
                .get("spec_id")
                .or_else(|| it.get("agreed_id"))
                .or_else(|| it.get("id"))
                .and_then(|v| v.as_str())?;
            let ty = it
                .get("req_type")
                .or_else(|| it.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // Individual tags — the list surface space-joins them into array
            // elements, so split each element back out.
            let tags: Vec<String> = it
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .flat_map(|s| s.split_whitespace())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            // The groomed execution mode is the authoritative routing signal.
            // Explicit drain is drainable even with descriptive keystone-class
            // tags; unset mode still gets the conservative tag heuristic.
            // trace:TASK-1231 BUG-1157 | ai:codex
            let mode = it
                .get("execution_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let mode_is_unset = mode.is_empty();
            let mode_holds_for_supervision = !mode_is_unset && mode != "drain";
            let release_operator = tags
                .iter()
                .any(|tag| crate::presence::is_release_operator_tag(tag));
            let tag_fences_unset_mode = mode_is_unset
                && crate::presence::is_keystone_class(ty, tags.iter().map(|s| s.as_str()));
            // Fence: a drainable type AND not an explicit supervised mode AND
            // never a release-operator task. The keystone-tag net is a fallback
            // only for unset/ungroomed specs; an explicit drain decision wins.
            // trace:STORY-1125 BUG-1157 | ai:codex
            if is_drainable_type(ty)
                && !mode_holds_for_supervision
                && !release_operator
                && !tag_fences_unset_mode
            {
                Some(id.to_string())
            } else {
                None
            }
        })
        .take(max)
        .collect()
}

/// Whether a requirement type is drainable implementer work. Deliberately
/// STRICTER than `is_work_item_type` (which counts an Epic as agile work):
/// autoprogress QUEUES work to an implementer, so it must never target an
/// epic rollup, an accepted decision, an authored constraint/vision/doc, etc.
/// Mirrors BUG-1130's `is_realignable_type`. Case-insensitive; an unrecognized
/// token is treated as drainable (never silently drop real work).
fn is_drainable_type(ty: &str) -> bool {
    !matches!(
        ty.trim().to_ascii_lowercase().as_str(),
        "epic"
            | "decision"
            | "folder"
            | "meta"
            | "principle"
            | "vision"
            | "constraint"
            | "term"
            | "doc"
    )
}

#[cfg(test)]
#[path = "tests/task_1231_autoprogress_tests.rs"]
mod task_1231_autoprogress_tests;
