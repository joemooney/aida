//! `aida internal ...` — hidden internal-helper subcommands invoked by git hooks
//! (advisor code gate, no-verify-bypass recorder, automatic advisor-lock gate).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure-movement refactor).

use crate::cli;
use crate::{advisor_code_gate, build_sha_short, find_project_root, locking_gate, rule_violation};
use anyhow::Result;

// trace:STORY-684 | ai:claude
/// Dispatch for the hidden `aida internal <…>` family — substrate machinery
/// invoked by hooks/scaffolding, not by humans. Today it carries the
/// vendor-agnostic advisor-code-write gate (STORY-684).
pub(crate) fn handle_internal_command(command: &cli::InternalCommand) -> Result<()> {
    match command {
        // Called by the git pre-commit hook: enforce the advisor-no-code-write
        // invariant at the commit boundary for ANY vendor. Exits non-zero (the
        // bail) when an advisor session stages code with no sanctioned context,
        // which fails the pre-commit hook and aborts the commit. git has already
        // staged everything by the time the hook runs, so we read the staged
        // index (include_unstaged = false). trace:STORY-684
        cli::InternalCommand::AdvisorCodeGate => {
            let root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            advisor_code_gate::enforce_at_commit(&root, false)
        }
        // Called by the git post-commit hook AFTER a commit lands (TASK-917).
        // The pre-commit hook can't observe its own bypass (--no-verify is "don't
        // run the pre-commit hook"), so detection is inverted to here: the
        // pre-commit hook wrote the staged-tree SHA into a sentinel; if it does
        // NOT match the committed tree, the pre-commit hook was skipped — a
        // --no-verify bypass. We re-confirm the sentinel check in-binary (don't
        // trust the hook to have classified it) and record a direct rule
        // violation. Always exits 0 — the commit already happened; instrumentation
        // must never fail it.
        cli::InternalCommand::RecordNoVerifyBypass => {
            record_no_verify_bypass_for_head();
            Ok(())
        }
        // Called by the git pre-commit hook: enforce the automatic
        // advisor-lock gate (STORY-711 slice 2) at the commit boundary for
        // ANY vendor. Silent no-op under the default `[locking] posture =
        // "off"`; a `Refused` verdict warns under `warn` and blocks under
        // `enforce`, naming the authorizing advisor.
        // trace:TASK-1140 | ai:claude
        cli::InternalCommand::LockingGate => {
            let root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            locking_gate::enforce_at_commit(&root)
        }
    }
}

// trace:TASK-917 | ai:claude
/// Resolve the pre-commit sentinel path for a repo. The pre-commit hook writes
/// the staged-tree SHA here when it runs; the post-commit detector compares it to
/// the committed tree to spot a `--no-verify` bypass. Lives under `.git/` (local,
/// never committed).
fn precommit_sentinel_path(git_dir: &std::path::Path) -> std::path::PathBuf {
    git_dir.join("aida-precommit-sentinel")
}

// trace:TASK-917 | ai:claude
/// Post-commit `--no-verify` detector. Re-confirms the bypass from the pre-commit
/// sentinel, then — if confirmed and the field study is on — records a
/// `no-verify-bypass` rule violation. Best-effort throughout: any git/IO hiccup
/// is treated as "can't confirm a bypass" and recording is skipped, so a clean
/// commit is never mislabeled. The sentinel is consumed (removed) either way so a
/// later non-bypassing commit that happens to reuse the tree can't false-positive.
fn record_no_verify_bypass_for_head() {
    let root = match find_project_root() {
        Ok(r) => r,
        Err(_) => return,
    };
    let git_dir = match git_common_dir(&root) {
        Some(d) => d,
        None => return,
    };
    let sentinel = precommit_sentinel_path(&git_dir);

    // The tree the just-made commit actually points at.
    let committed_tree = match git_rev_parse(&root, "HEAD^{tree}") {
        Some(t) => t,
        None => return,
    };
    // What the pre-commit hook recorded (if it ran at all). A missing sentinel
    // reads as "no pre-commit ran" — the bypass case.
    let sentinel_tree = std::fs::read_to_string(&sentinel)
        .ok()
        .map(|s| s.trim().to_string());
    // Consume the sentinel so it can't linger and mis-classify a later commit.
    let _ = std::fs::remove_file(&sentinel);

    let matched = sentinel_tree.as_deref() == Some(committed_tree.as_str());
    if !rule_violation::detect_no_verify_bypass(matched) {
        return; // pre-commit ran for this tree — not a bypass.
    }

    // A bypass. Infer the single spec from HEAD's trailers/traces (an identifier
    // breadcrumb, never message content); `unknown` when ambiguous/absent.
    let spec_id = infer_head_spec(&root).unwrap_or_else(|| "unknown".to_string());
    let headless = std::env::var("AIDA_AUTO_COMPLETE").is_ok()
        || matches!(
            std::env::var("AIDA_NO_HUMAN").ok().as_deref(),
            Some("1") | Some("true") | Some("both")
        );
    rule_violation::record_no_verify_bypass(&root, &spec_id, headless, build_sha_short());
}

// trace:TASK-917 | ai:claude
/// `git rev-parse <rev>` → trimmed stdout, or `None` on failure.
fn git_rev_parse(root: &std::path::Path, rev: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", rev])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// trace:TASK-917 | ai:claude
/// The repo's common git dir (`git rev-parse --git-common-dir`, made absolute) —
/// so the sentinel lives in the shared `.git` even from a worktree.
fn git_common_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let p = std::path::PathBuf::from(&raw);
    Some(if p.is_absolute() { p } else { root.join(p) })
}

// trace:TASK-917 | ai:claude
/// Infer the single SPEC-ID referenced by HEAD's commit message (the spec-id
/// trailer / any trace markers in the subject+body), when unambiguous. An
/// identifier breadcrumb already public in the commit — never message content.
/// `None` when zero or multiple distinct specs are referenced.
fn infer_head_spec(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["show", "-s", "--format=%B", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let re = regex::Regex::new(r"(?i)\b([A-Z]+(?:-[A-Z0-9_]+)?-[0-9]+)\b").ok()?;
    let mut specs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cap in re.captures_iter(&body) {
        if let Some(m) = cap.get(1) {
            specs.insert(m.as_str().to_uppercase());
        }
    }
    if specs.len() == 1 {
        specs.into_iter().next()
    } else {
        None
    }
}
