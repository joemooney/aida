//! `aida pr rebase <N>` — wrap the standard "rebase a PR before review"
//! recipe into one command (TASK-308).
//!
//! Today the user runs ~6 manual git commands to rebase a PR onto its base:
//! `gh pr checkout <N>` → `git fetch origin` → `git rebase origin/<base>` →
//! resolve conflicts → smoke build → `git push --force-with-lease`. This
//! module collapses that into a single CLI call, with three modes:
//!
//! * **default** — auto-rebase if clean; abort cleanly + print recipe on
//!   conflicts.
//! * **--check** — report-only: stale-base + overlap + conflict-prediction
//!   via `git merge-tree`.
//! * **--interactive** — on conflict, drop the user into the temp worktree
//!   to resolve, then continue + push.
//!
//! Force-push always uses `--force-with-lease` — never plain `--force`.
//! Cross-fork PRs are refused (force-pushing to a fork is the
//! contributor's job, not ours).
//!
//! The module owns the **pure pieces** so they're unit-testable without
//! `git`/`gh`: PR-metadata parsing, manual-recipe formatting, config
//! reading, temp-worktree path derivation, conflict-prediction parsing.
//! The CLI handler that wires git/gh side-effects lives in `main.rs`
//! next to the other `handle_*` command dispatchers (mirrors `punt.rs`
//! and `findings.rs`).
//!
//! trace:TASK-308 | ai:claude

use std::path::{Path, PathBuf};

/// PR metadata captured from `gh pr view <N> --json ...`. Enough to
/// drive the rebase + cross-fork detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub n: u64,
    /// The PR's declared base branch (e.g. "main"). Always non-empty
    /// when `gh` succeeded.
    pub base_ref: String,
    /// The PR's head branch on its repo of origin (e.g. "task-308").
    pub head_ref: String,
    /// `headRefOid` — the current head SHA. Used to anchor
    /// force-with-lease so we don't overwrite work the user pushed
    /// from another shell while the rebase was running.
    pub head_oid: String,
    /// `isCrossRepository` — true when the PR is from a fork. We
    /// refuse to act in that case.
    pub is_cross_repository: bool,
    /// `headRepository.nameWithOwner` (e.g. "alice/aida") when
    /// cross-repository; used only to make the refusal message
    /// concrete.
    pub head_repo_owner: Option<String>,
    pub is_draft: bool,
}

/// Parse the JSON `gh pr view <N> --json baseRefName,headRefName,headRefOid,isCrossRepository,headRepository,isDraft`
/// returned. Pulled out of the side-effecting wrapper so the parse rules
/// are pinned by unit tests without invoking `gh`.
///
/// Returns `Err` with a short reason when a required field is missing
/// — `base_ref`, `head_ref`, or `head_oid` empty are all hard errors
/// (the rebase recipe can't proceed without them).
pub fn parse_pr_info(json: &serde_json::Value, n: u64) -> Result<PrInfo, String> {
    let str_field = |key: &str| -> Option<String> {
        json.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    let base_ref = str_field("baseRefName")
        .ok_or_else(|| "gh pr view did not return baseRefName".to_string())?;
    let head_ref = str_field("headRefName")
        .ok_or_else(|| "gh pr view did not return headRefName".to_string())?;
    let head_oid = str_field("headRefOid")
        .ok_or_else(|| "gh pr view did not return headRefOid".to_string())?;
    let is_cross_repository = json
        .get("isCrossRepository")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_draft = json
        .get("isDraft")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let head_repo_owner = json
        .get("headRepository")
        .and_then(|hr| hr.get("nameWithOwner").or_else(|| hr.get("owner")))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else {
                v.get("login")
                    .and_then(|l| l.as_str())
                    .map(|s| s.to_string())
            }
        });
    Ok(PrInfo {
        n,
        base_ref,
        head_ref,
        head_oid,
        is_cross_repository,
        head_repo_owner,
        is_draft,
    })
}

/// Config for `aida pr rebase`. Lives under `[pr-rebase]` in
/// `.aida/config.toml`. All fields optional — handler applies defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrRebaseConfig {
    /// Shell command to run after a clean rebase as a build sanity
    /// check. Empty string (or `"none"`/`"false"`) disables the
    /// smoke check entirely. When `None`, the handler picks a project
    /// default (`cargo build --release` for Rust projects).
    pub smoke_check: Option<String>,
}

/// Parse `[pr-rebase]` out of a `.aida/config.toml` body. Pulled out
/// of the IO wrapper so the parser is unit-testable. Returns the
/// default config when the section is missing or the body doesn't
/// parse — config reads must never break the command.
pub fn parse_pr_rebase_config(toml_body: &str) -> PrRebaseConfig {
    let Ok(parsed) = toml_body.parse::<toml::Value>() else {
        return PrRebaseConfig::default();
    };
    let section = match parsed.get("pr-rebase") {
        Some(v) => v,
        // Accept the underscore form too — both spellings show up in
        // user-written TOML and the friction of getting it wrong
        // would be silent (no smoke check, mystery default).
        None => match parsed.get("pr_rebase") {
            Some(v) => v,
            None => return PrRebaseConfig::default(),
        },
    };
    let smoke_check = section
        .get("smoke_check")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    PrRebaseConfig { smoke_check }
}

/// Read + parse `[pr-rebase]` from `<project_root>/.aida/config.toml`.
/// Soft-fails to the default config on missing file / parse error.
pub fn read_pr_rebase_config(project_root: &Path) -> PrRebaseConfig {
    let path = project_root.join(".aida").join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(body) => parse_pr_rebase_config(&body),
        Err(_) => PrRebaseConfig::default(),
    }
}

/// Pick the project-default smoke check when the user hasn't
/// configured one. `cargo build --release` for Rust projects (the
/// `Cargo.toml` at the root is the marker); empty string otherwise
/// (skip the check rather than guess wrong on a non-Rust project).
pub fn default_smoke_check(project_root: &Path) -> String {
    if project_root.join("Cargo.toml").is_file() {
        "cargo build --release".to_string()
    } else {
        String::new()
    }
}

/// Resolve the effective smoke-check command. Pulled out so the
/// precedence rules (CLI flag → config → project default → disabled
/// sentinels) are pinned by tests.
///
/// * `no_smoke` (the `--no-smoke` CLI flag) wins — always returns `None`.
/// * Otherwise the config value, if present, wins. `""`, `"none"`,
///   `"false"`, `"skip"` all mean "user explicitly disabled" → `None`.
/// * Falling all the way through, the project default applies (which
///   is `""` for non-Rust → `None`).
pub fn resolve_smoke_check(
    no_smoke: bool,
    config: &PrRebaseConfig,
    project_default: &str,
) -> Option<String> {
    if no_smoke {
        return None;
    }
    let raw: &str = config.smoke_check.as_deref().unwrap_or(project_default);
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "" | "none" | "false" | "skip" | "off" => None,
        _ => Some(trimmed.to_string()),
    }
}

/// Temporary worktree path for the rebase. Lives under
/// `.aida/worktrees/pr-rebase-<N>/` so it stays inside the project
/// (gitignored by the standard `.aida/*` deny-by-default rule) and
/// survives a crash for inspection. The handler is responsible for
/// removing it on every exit path (success / conflict / smoke-fail
/// / interrupt).
pub fn temp_worktree_path(project_root: &Path, n: u64) -> PathBuf {
    project_root
        .join(".aida")
        .join("worktrees")
        .join(format!("pr-rebase-{n}"))
}

/// Format the "do it yourself" recipe printed when the rebase aborts
/// (conflicts, cross-fork refusal). Pulled out so the recipe text is
/// pinned by unit tests — drift in this string is what makes the
/// CLI feel inconsistent.
pub fn manual_recipe(n: u64, base_ref: &str) -> String {
    format!(
        "Manual recipe:\n  \
         gh pr checkout {n}\n  \
         git fetch origin\n  \
         git rebase origin/{base_ref}\n  \
         # resolve conflicts, then `git rebase --continue`\n  \
         git push --force-with-lease"
    )
}

/// Format the refusal message for cross-fork PRs. Pulled out for the
/// same reason as `manual_recipe` — the text is contract-visible to
/// first-users and should not drift silently.
pub fn cross_fork_refusal(n: u64, head_repo: Option<&str>) -> String {
    let from = head_repo.unwrap_or("a fork");
    format!(
        "PR-{n} is from {from}; aida can't force-push to a fork's branch. \
         Run `gh pr checkout {n}` + `git rebase` + manual push there."
    )
}

/// Prediction returned by the `--check` mode's conflict probe.
/// `clean` ⇒ rebase would apply without conflict. `Conflicting` ⇒
/// `git merge-tree` reported markers (or, on older git, the overlap
/// heuristic flagged it). `Unknown` ⇒ couldn't probe (git too old,
/// merge-tree failed) — caller prints "could not predict".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictPrediction {
    Clean,
    Conflicting { files: Vec<String> },
    Unknown,
}

/// Pre-flight verdict produced before launching a headless reviewer
/// against a PR (STORY-281). The orchestrator's phase 3 entry point and
/// the direct `aida queue work PR-N --for reviewer` path both call
/// [`classify_stale_base`] over the same inputs `--check` already
/// computes; the variant decides whether reviewer launch is silent
/// (Current), warning-only (StaleNoOverlap), or refused
/// (StaleOverlap).
///
/// Reading reviewer code against a stale base is the failure mode the
/// 2026-05-17 self-test surfaced: PR-65 sat unrebased while two PRs
/// merged in the interim, one of them touching the same file. With no
/// overlap, the review is suboptimal but safe — a textual rebase will
/// land clean at merge time. With overlap, the review is against the
/// wrong version of the code and the conflict may not show up until
/// merge.
/// trace:STORY-281 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleBaseOutcome {
    /// origin/<base> has not moved since the PR forked — silent proceed.
    Current,
    /// origin/<base> has moved but no file the PR touches has been
    /// modified on the base — review-against-stale is suboptimal but
    /// safe to proceed with a warning. `behind` is the commit count.
    StaleNoOverlap { behind: u32 },
    /// origin/<base> has moved AND at least one file the PR touches has
    /// also been modified on the base since the PR forked — reviewer
    /// launch is refused unless `--allow-stale-base` is passed. The
    /// prediction surfaces whether `git merge-tree` predicts a literal
    /// conflict; either way, the human-attention threshold is met.
    StaleOverlap {
        behind: u32,
        overlap_files: Vec<String>,
        prediction: ConflictPrediction,
    },
}

/// Pure classifier: given the same inputs `pr_rebase_check_report`
/// derives (behind-count, PR-touched files, base-touched-since-fork
/// files, merge-tree prediction), pick the [`StaleBaseOutcome`] for
/// the reviewer pre-flight. trace:STORY-281 | ai:claude
pub fn classify_stale_base(
    behind: u32,
    pr_files: &[String],
    base_files: &[String],
    prediction: ConflictPrediction,
) -> StaleBaseOutcome {
    if behind == 0 {
        return StaleBaseOutcome::Current;
    }
    let pr_set: std::collections::HashSet<&String> = pr_files.iter().collect();
    let overlap: Vec<String> = base_files
        .iter()
        .filter(|f| pr_set.contains(*f))
        .cloned()
        .collect();
    if !overlap.is_empty() {
        StaleBaseOutcome::StaleOverlap {
            behind,
            overlap_files: overlap,
            prediction,
        }
    } else if let ConflictPrediction::Conflicting { files } = &prediction {
        StaleBaseOutcome::StaleOverlap {
            behind,
            overlap_files: files.clone(),
            prediction,
        }
    } else {
        StaleBaseOutcome::StaleNoOverlap { behind }
    }
}

/// Format the user-facing "reviewer refused — stale base + overlap"
/// message for [`StaleBaseOutcome::StaleOverlap`]. Pulled out so the
/// text is pinned by unit tests — first-users see this string and it
/// must name the conflicting files and the exact recovery command.
/// trace:STORY-281 | ai:claude
pub fn stale_base_block_message(n: u64, behind: u32, overlap: &[String]) -> String {
    let mut msg = format!(
        "PR-{n} base is {behind} commit{plural} behind origin and overlaps {count} \
         file{file_plural} touched on base since the PR forked — refusing to launch \
         reviewer against stale code.\n\n",
        plural = if behind == 1 { "" } else { "s" },
        count = overlap.len(),
        file_plural = if overlap.len() == 1 { "" } else { "s" },
    );
    msg.push_str("Overlapping files:\n");
    for f in overlap.iter().take(10) {
        msg.push_str(&format!("  {f}\n"));
    }
    if overlap.len() > 10 {
        msg.push_str(&format!("  … and {} more\n", overlap.len() - 10));
    }
    msg.push('\n');
    msg.push_str(&format!(
        "Recover with:\n  aida pr rebase {n}              # clean rebase + force-push-with-lease\n  \
         aida pr rebase {n} --interactive  # if conflicts need manual resolve\n\n\
         Or pass `--allow-stale-base` to proceed anyway (review will be against stale code)."
    ));
    msg
}

/// Format the stale-base warning for the human-review verb
/// (`aida review <SPEC>`). Unlike the reviewer-role pre-flight, the
/// human verb NEVER refuses — the verdict on the code is valid either
/// way — so even the overlap case is informational: name the gap, name
/// the overlapping files, and give the exact recovery command. Pinned
/// by unit tests like its siblings. trace:BUG-510 | ai:claude
pub fn stale_base_review_warn_message(n: u64, behind: u32, overlap: &[String]) -> String {
    let mut msg = format!(
        "PR-{n} is {behind} commit{plural} behind its base — this review will run \
         against stale code.\n",
        plural = if behind == 1 { "" } else { "s" },
    );
    if overlap.is_empty() {
        msg.push_str(
            "No file the PR touches has moved on the base, so a textual rebase \
             should land clean.\n",
        );
    } else {
        msg.push_str(&format!(
            "{count} file{file_plural} the PR touches also changed on the base \
             since it forked:\n",
            count = overlap.len(),
            file_plural = if overlap.len() == 1 { "" } else { "s" },
        ));
        for f in overlap.iter().take(10) {
            msg.push_str(&format!("  {f}\n"));
        }
        if overlap.len() > 10 {
            msg.push_str(&format!("  … and {} more\n", overlap.len() - 10));
        }
    }
    msg.push_str(&format!(
        "Rebase before merging: `aida pr rebase {n}` \
         (or pass `--allow-stale-base` to skip this check)."
    ));
    msg
}

/// Format the warning printed for [`StaleBaseOutcome::StaleNoOverlap`].
/// Same pinning rationale as [`stale_base_block_message`].
/// trace:STORY-281 | ai:claude
pub fn stale_base_warn_message(n: u64, behind: u32) -> String {
    format!(
        "PR-{n} base is {behind} commit{plural} behind origin (no file overlap) — \
         proceeding with reviewer; consider `aida pr rebase {n}` before merge.",
        plural = if behind == 1 { "" } else { "s" },
    )
}

// ---------------------------------------------------------------------------
// TASK-480: intermediate / generated-only diff classifier.
//
// Sibling substrate-as-bouncer gate to the STORY-281 stale-base check.
// LLMs frequently "fix" a problem by editing an intermediate build
// product (target/, build/, dist/, node_modules/, a vendored lockfile,
// generated code) instead of the source that produces it. Such a fix
// is not reproducible — it's overwritten on the next build. The
// reviewer phase is the right substrate layer to catch it because it
// already reads the diff against the base.
//
// Per the BUG-280-class lesson (feedback_substrate_as_bouncer_not_rules):
// this is a PROGRAMMATIC GATE in the reviewer code path, NOT instruction
// text in the skill template. A rule in CLAUDE.md / a memory / a skill
// does not stop a confident LLM; a gate does.
// ---------------------------------------------------------------------------

/// Per-path classification used by [`classify_intermediate_only`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathClass {
    /// Tracked-in-git, non-ignored, not in a known-generated path.
    Source,
    /// Matches a `.gitignore` pattern or a known-generated path
    /// (`target/`, `build/`, `dist/`, `node_modules/`, …) — an
    /// intermediate build product, not source.
    Intermediate,
}

/// Verdict for the reviewer's intermediate-only pre-flight, mirroring
/// the [`StaleBaseOutcome`] shape (silent / warn / refuse).
/// trace:TASK-480 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntermediateOnlyOutcome {
    /// Diff is empty, or source-majority with at most minor intermediate
    /// noise — proceed silently.
    Clean,
    /// Diff contains both source and intermediate paths, intermediate is
    /// a minority → no; actually a *majority* (more intermediate than
    /// source) → flag-but-allow. Carries the intermediate paths for the
    /// flag message.
    SourcePlusIntermediate { intermediate: Vec<String> },
    /// Diff contains ONLY intermediate paths — refuse: the fix is not
    /// reproducible. Carries the offending paths.
    IntermediateOnly { intermediate: Vec<String> },
}

/// Known-generated / intermediate path heuristics, independent of the
/// project's `.gitignore`. These are the directories and file shapes
/// that are build products across the common ecosystems even when a
/// project forgets to ignore them. Pure + unit-tested so the heuristic
/// set is pinned. trace:TASK-480 | ai:claude
pub fn path_looks_generated(path: &str) -> bool {
    // Normalise leading "./" and Windows separators so the segment
    // matching below is uniform.
    let norm = path.replace('\\', "/");
    let norm = norm.strip_prefix("./").unwrap_or(&norm);
    let segments: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();

    // Directory-anchored build outputs: a path is generated if any of
    // these names appears as a path segment (e.g. `target/`, but also a
    // nested `crate/target/debug/foo`).
    const GENERATED_DIRS: &[&str] = &[
        "target",       // Rust / sbt
        "build",        // Gradle / CMake / many
        "dist",         // JS bundlers, Python sdists
        "node_modules", // npm/yarn/pnpm
        ".next",        // Next.js
        ".nuxt",        // Nuxt
        "out",          // Next.js export / tsc out
        ".gradle",      // Gradle cache
        "__pycache__",  // CPython bytecode cache
        ".pytest_cache",
        ".mypy_cache",
        "coverage",
        ".venv",
        "venv",
        "vendor", // Go / PHP vendored deps
        ".aida-store",
    ];
    if segments.iter().any(|s| GENERATED_DIRS.contains(s)) {
        return true;
    }

    // File-shape build products by suffix / basename.
    let base = segments.last().copied().unwrap_or("");
    const GENERATED_SUFFIXES: &[&str] = &[
        ".pyc", ".pyo", ".class", ".o", ".obj", ".a", ".rlib", ".rmeta", ".bin", ".exe", ".dll",
        ".so", ".dylib", ".min.js", ".min.css", ".map", // sourcemaps
    ];
    if GENERATED_SUFFIXES.iter().any(|sfx| base.ends_with(sfx)) {
        return true;
    }

    // Lockfiles are *coincidental* regeneration, not source — they
    // count as intermediate so a lockfile-only diff is refused, but a
    // lockfile alongside a real source change is flagged-but-allowed
    // (the SourcePlusIntermediate path).
    const LOCKFILES: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "poetry.lock",
        "Pipfile.lock",
        "composer.lock",
        "Gemfile.lock",
    ];
    if LOCKFILES.contains(&base) {
        return true;
    }

    false
}

/// Classify one path. `is_gitignored` is the project's own answer
/// (`git check-ignore`) for that path; we OR it with the
/// ecosystem-wide [`path_looks_generated`] heuristic so a project that
/// forgot to ignore `target/` is still caught. trace:TASK-480 | ai:claude
pub fn classify_path(path: &str, is_gitignored: bool) -> PathClass {
    if is_gitignored || path_looks_generated(path) {
        PathClass::Intermediate
    } else {
        PathClass::Source
    }
}

/// Pure classifier: given the PR's changed paths and a predicate that
/// answers "is this path gitignored in the repo?", decide whether the
/// reviewer should proceed silently, flag, or refuse.
///
/// Rules (matching the TASK-480 acceptance):
/// * ONLY intermediate paths → [`IntermediateOnlyOutcome::IntermediateOnly`]
///   (refuse — the fix is not reproducible).
/// * BOTH source + intermediate where intermediate is the *majority*
///   (more intermediate than source) → flag-but-allow.
/// * SOURCE-majority (intermediate ≤ half), source-only, or empty → clean.
///
/// Refusal is reserved for the unambiguous "no source at all" case so we
/// never block a real fix on a heuristic; a mixed diff is at most
/// flagged. trace:TASK-480 | ai:claude
pub fn classify_intermediate_only(
    files: &[String],
    is_gitignored: impl Fn(&str) -> bool,
) -> IntermediateOnlyOutcome {
    let mut source: Vec<String> = Vec::new();
    let mut intermediate: Vec<String> = Vec::new();
    for f in files {
        match classify_path(f, is_gitignored(f)) {
            PathClass::Source => source.push(f.clone()),
            PathClass::Intermediate => intermediate.push(f.clone()),
        }
    }

    if intermediate.is_empty() {
        // Source-only (or empty diff) — nothing to flag.
        return IntermediateOnlyOutcome::Clean;
    }
    if source.is_empty() {
        return IntermediateOnlyOutcome::IntermediateOnly { intermediate };
    }
    // Mixed. Intermediate a minority (≤ half) → clean (minor noise).
    // Otherwise (intermediate is the majority) flag-but-allow.
    let total = source.len() + intermediate.len();
    if intermediate.len() * 2 <= total {
        IntermediateOnlyOutcome::Clean
    } else {
        IntermediateOnlyOutcome::SourcePlusIntermediate { intermediate }
    }
}

/// Refusal message for [`IntermediateOnlyOutcome::IntermediateOnly`].
/// Pinned by unit tests — first-users (and the headless orchestrator's
/// logs) see this string and the verdict text is contract-visible.
/// trace:TASK-480 | ai:claude
pub fn intermediate_only_block_message(n: u64, intermediate: &[String]) -> String {
    let mut msg = format!(
        "PR-{n} changes only intermediate/generated files — refusing to review: \
         this fix is not reproducible (these paths are overwritten on the next \
         build). Modify the source code or build scripts that produce them.\n\n"
    );
    msg.push_str("Intermediate/generated paths:\n");
    for f in intermediate.iter().take(10) {
        msg.push_str(&format!("  {f}\n"));
    }
    if intermediate.len() > 10 {
        msg.push_str(&format!("  … and {} more\n", intermediate.len() - 10));
    }
    msg.push_str(
        "\nIf this PR is a deliberate regeneration of checked-in build output, \
         pass `--allow-intermediate-only` to proceed.",
    );
    msg
}

/// Flag message for [`IntermediateOnlyOutcome::SourcePlusIntermediate`].
/// Same pinning rationale. trace:TASK-480 | ai:claude
pub fn intermediate_only_warn_message(n: u64, intermediate: &[String]) -> String {
    let count = intermediate.len();
    let plural = if count == 1 { "" } else { "s" };
    let shown: Vec<&str> = intermediate.iter().take(5).map(|s| s.as_str()).collect();
    let mut list = shown.join(", ");
    if count > 5 {
        list.push_str(&format!(", … (+{})", count - 5));
    }
    format!(
        "PR-{n} touches {count} intermediate/generated path{plural} alongside source \
         ({list}) — proceeding with reviewer; confirm the generated files are an \
         intended side effect of the source change, not the fix itself."
    )
}

/// Parse the stdout from `git merge-tree --write-tree --name-only
/// <base> <head>`. On a clean merge, stdout is the resulting tree's
/// SHA on a single line. On a conflict, merge-tree exits non-zero
/// (caller handles) and stdout contains a NUL-separated section with
/// the conflicting paths followed by the (junk) tree SHA. We accept
/// either NUL- or newline-separated path lists because `--name-only`
/// uses newlines and the default `-z` form uses NULs.
///
/// Pulled out so the parser is pinned by tests independently of the
/// git invocation.
pub fn parse_merge_tree_conflicts(stdout: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let split = if stdout.contains('\0') {
        stdout.split('\0').collect::<Vec<_>>()
    } else {
        stdout.lines().collect::<Vec<_>>()
    };
    // First token is the candidate tree SHA (40+ hex chars) — skip
    // it when present so we don't mistake a SHA for a path. Same for
    // any subsequent SHA-only lines.
    let is_sha = |s: &str| s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit());
    for tok in split {
        let t = tok.trim();
        if t.is_empty() || is_sha(t) {
            continue;
        }
        if !files.contains(&t.to_string()) {
            files.push(t.to_string());
        }
    }
    files
}

// ---------------------------------------------------------------------------
// BUG-640: patch-id force-push guard.
//
// `--force-with-lease` does NOT protect a commit you've already fetched
// (the lease compares against your local remote-tracking ref, which a
// prior fetch may have already advanced) — AIDA had a real incident
// where this dropped a merged PR on main. Per AIDA's own doctrine
// (feedback_substrate_as_bouncer_not_rules): convert the CLAUDE.md rule
// "never force-push over un-incorporated remote work" into a PROGRAMMATIC
// GATE, lifted from kunchenguid/no-mistakes.
//
// Before ANY force-push:
//   1. `git ls-remote` the LIVE target ref to get the current remote tip.
//   2. `git rev-list --cherry-pick --right-only HEAD...<remote-tip> ^<base>`
//      lists every commit the remote has that we DON'T, that isn't already
//      incorporated by patch-id and isn't reachable from base.
//   3. Non-empty → REFUSE (real remote work would be dropped).
//   4. FAIL CLOSED: any inconclusive result (ls-remote fails, base won't
//      resolve, rev-list errors) refuses rather than risk the overwrite.
//
// The pure pieces (ls-remote parse, rev-list classification, message)
// live here so they're unit-testable without git/network; the
// side-effecting wrapper that shells out lives in main.rs next to the
// push call site.
// ---------------------------------------------------------------------------

/// Verdict of the patch-id force-push guard.
// trace:BUG-640 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForcePushGuard {
    /// Every commit the live remote holds is already incorporated by
    /// patch-id or reachable from base (or the remote ref is absent) —
    /// the force-push will not drop any remote work. Proceed.
    Safe,
    /// The live remote tip holds at least one commit we have NOT
    /// incorporated by patch-id. Refuse — force-pushing would drop it.
    /// Carries the `git rev-list --oneline` lines for the message.
    Unincorporated { commits: Vec<String> },
    /// Could not determine the remote state (ls-remote failed, base ref
    /// didn't resolve, rev-list errored). FAIL CLOSED — refuse.
    Inconclusive { reason: String },
}

/// Parse the SHA for `expected_ref` out of `git ls-remote <remote> <ref>`
/// stdout. Each line is `<sha>\t<refname>`. Returns `None` when the ref
/// is absent (no remote branch yet) — the caller treats absence as
/// "nothing to overwrite". Pure so the parse is pinned by tests without
/// hitting the network.
// trace:BUG-640 | ai:claude
pub fn parse_ls_remote_tip(stdout: &str, expected_ref: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let sha = it.next().unwrap_or("");
        let refname = it.next().unwrap_or("");
        if refname == expected_ref && sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(sha.to_string());
        }
    }
    None
}

/// Classify the output of
/// `git rev-list --cherry-pick --right-only --oneline HEAD...<tip> ^<base>`.
///
/// Each non-empty line is a commit the remote tip has that our branch
/// does NOT — and that is neither incorporated by patch-id
/// (`--cherry-pick` drops patch-equivalent commits) nor reachable from
/// base (`^<base>` excludes shared history). Any such line means real
/// remote work would be lost ⇒ refuse. No lines ⇒ safe.
///
/// Pure function over the rev-list stdout — the load-bearing
/// incorporation logic, unit-tested independently of git.
// trace:BUG-640 | ai:claude
pub fn classify_force_push(rev_list_stdout: &str) -> ForcePushGuard {
    let commits: Vec<String> = rev_list_stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    if commits.is_empty() {
        ForcePushGuard::Safe
    } else {
        ForcePushGuard::Unincorporated { commits }
    }
}

/// Refusal message for [`ForcePushGuard::Unincorporated`]. Names the
/// un-incorporated remote commits and the recovery path. Pinned by unit
/// tests — first-users and the headless drain's logs see this string.
// trace:BUG-640 | ai:claude
pub fn force_push_block_message(head_ref: &str, commits: &[String]) -> String {
    let count = commits.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut msg = format!(
        "Refusing to force-push: origin/{head_ref} has {count} commit{plural} not \
         incorporated into this branch by patch-id. Force-pushing would DROP \
         {them}.\n\n",
        them = if count == 1 { "it" } else { "them" },
    );
    msg.push_str("Un-incorporated remote commit(s):\n");
    for c in commits.iter().take(10) {
        msg.push_str(&format!("  {c}\n"));
    }
    if commits.len() > 10 {
        msg.push_str(&format!("  … and {} more\n", commits.len() - 10));
    }
    msg.push_str(&format!(
        "\nRecover by pulling the remote work first:\n  \
         git fetch origin {head_ref}\n  \
         git rebase origin/{head_ref}   # or merge, then re-run\n\n\
         (--force-with-lease alone does NOT protect an already-fetched commit — \
         this guard does.)"
    ));
    msg
}

/// Refusal message for [`ForcePushGuard::Inconclusive`] — the fail-closed
/// path. Pinned by unit tests.
// trace:BUG-640 | ai:claude
pub fn force_push_inconclusive_message(head_ref: &str, reason: &str) -> String {
    format!(
        "Refusing to force-push origin/{head_ref}: could not verify the live remote \
         is safe to overwrite ({reason}). Failing closed — re-run once the remote is \
         reachable, or push manually after confirming no remote work would be lost."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pr_info_happy_path() {
        let j = serde_json::json!({
            "baseRefName": "main",
            "headRefName": "task-308",
            "headRefOid": "deadbeef0000000000000000000000000000beef",
            "isCrossRepository": false,
            "isDraft": false,
            "headRepository": { "nameWithOwner": "joemooney/aida" },
        });
        let info = parse_pr_info(&j, 308).unwrap();
        assert_eq!(info.n, 308);
        assert_eq!(info.base_ref, "main");
        assert_eq!(info.head_ref, "task-308");
        assert_eq!(info.head_oid, "deadbeef0000000000000000000000000000beef");
        assert!(!info.is_cross_repository);
        assert!(!info.is_draft);
        assert_eq!(info.head_repo_owner.as_deref(), Some("joemooney/aida"));
    }

    #[test]
    fn parse_pr_info_cross_fork() {
        let j = serde_json::json!({
            "baseRefName": "main",
            "headRefName": "fix-typo",
            "headRefOid": "1234567890abcdef1234567890abcdef12345678",
            "isCrossRepository": true,
            "isDraft": true,
            "headRepository": { "nameWithOwner": "alice/aida" },
        });
        let info = parse_pr_info(&j, 99).unwrap();
        assert!(info.is_cross_repository);
        assert!(info.is_draft);
        assert_eq!(info.head_repo_owner.as_deref(), Some("alice/aida"));
    }

    #[test]
    fn parse_pr_info_missing_base_ref() {
        let j = serde_json::json!({
            "headRefName": "task-308",
            "headRefOid": "deadbeef0000000000000000000000000000beef",
        });
        let err = parse_pr_info(&j, 1).unwrap_err();
        assert!(err.contains("baseRefName"), "{err}");
    }

    #[test]
    fn parse_pr_info_missing_head_oid() {
        let j = serde_json::json!({
            "baseRefName": "main",
            "headRefName": "task-308",
        });
        let err = parse_pr_info(&j, 1).unwrap_err();
        assert!(err.contains("headRefOid"), "{err}");
    }

    #[test]
    fn parse_pr_info_empty_string_is_missing() {
        let j = serde_json::json!({
            "baseRefName": "",
            "headRefName": "task-308",
            "headRefOid": "deadbeef0000000000000000000000000000beef",
        });
        let err = parse_pr_info(&j, 1).unwrap_err();
        assert!(err.contains("baseRefName"), "{err}");
    }

    #[test]
    fn parse_pr_rebase_config_section_present() {
        let body = r#"
[pr-rebase]
smoke_check = "cargo test --release"
"#;
        let c = parse_pr_rebase_config(body);
        assert_eq!(c.smoke_check.as_deref(), Some("cargo test --release"));
    }

    #[test]
    fn parse_pr_rebase_config_underscore_spelling() {
        let body = r#"
[pr_rebase]
smoke_check = "make smoke"
"#;
        let c = parse_pr_rebase_config(body);
        assert_eq!(c.smoke_check.as_deref(), Some("make smoke"));
    }

    #[test]
    fn parse_pr_rebase_config_missing_section() {
        let body = r#"
[session]
enforcement = "warn"
"#;
        let c = parse_pr_rebase_config(body);
        assert!(c.smoke_check.is_none());
    }

    #[test]
    fn parse_pr_rebase_config_garbage_is_default() {
        let c = parse_pr_rebase_config("this is not toml = [[[");
        assert_eq!(c, PrRebaseConfig::default());
    }

    #[test]
    fn resolve_smoke_check_no_smoke_wins() {
        let cfg = PrRebaseConfig {
            smoke_check: Some("cargo build".into()),
        };
        assert_eq!(
            resolve_smoke_check(true, &cfg, "cargo build --release"),
            None
        );
    }

    #[test]
    fn resolve_smoke_check_config_overrides_default() {
        let cfg = PrRebaseConfig {
            smoke_check: Some("cargo test".into()),
        };
        assert_eq!(
            resolve_smoke_check(false, &cfg, "cargo build --release"),
            Some("cargo test".into())
        );
    }

    #[test]
    fn resolve_smoke_check_falls_back_to_default() {
        let cfg = PrRebaseConfig::default();
        assert_eq!(
            resolve_smoke_check(false, &cfg, "cargo build --release"),
            Some("cargo build --release".into())
        );
    }

    #[test]
    fn resolve_smoke_check_disabled_sentinels() {
        for sentinel in ["", "none", "false", "skip", "off", "  none  ", "NONE"] {
            let cfg = PrRebaseConfig {
                smoke_check: Some(sentinel.into()),
            };
            assert_eq!(
                resolve_smoke_check(false, &cfg, "cargo build --release"),
                None,
                "sentinel {:?} should disable",
                sentinel
            );
        }
    }

    #[test]
    fn resolve_smoke_check_default_empty_skips() {
        // Non-Rust project: project_default is "" — config absent and
        // --no-smoke not passed should resolve to None (skipped), not
        // run an empty command.
        let cfg = PrRebaseConfig::default();
        assert_eq!(resolve_smoke_check(false, &cfg, ""), None);
    }

    #[test]
    fn temp_worktree_path_under_aida() {
        let p = temp_worktree_path(Path::new("/tmp/proj"), 308);
        assert_eq!(p, PathBuf::from("/tmp/proj/.aida/worktrees/pr-rebase-308"));
    }

    #[test]
    fn manual_recipe_contains_key_commands() {
        let r = manual_recipe(308, "main");
        assert!(r.contains("gh pr checkout 308"), "{r}");
        assert!(r.contains("git fetch origin"), "{r}");
        assert!(r.contains("git rebase origin/main"), "{r}");
        assert!(r.contains("git push --force-with-lease"), "{r}");
    }

    #[test]
    fn cross_fork_refusal_names_the_fork() {
        let m = cross_fork_refusal(99, Some("alice/aida"));
        assert!(m.contains("PR-99"), "{m}");
        assert!(m.contains("alice/aida"), "{m}");
        assert!(m.contains("gh pr checkout 99"), "{m}");
    }

    #[test]
    fn cross_fork_refusal_without_repo_name() {
        let m = cross_fork_refusal(99, None);
        assert!(m.contains("a fork"), "{m}");
    }

    #[test]
    fn parse_merge_tree_clean() {
        // Clean merge: stdout is just the tree SHA.
        let out = "abc123def456abc123def456abc123def456abcd\n";
        assert!(parse_merge_tree_conflicts(out).is_empty());
    }

    #[test]
    fn parse_merge_tree_newline_separated() {
        // `git merge-tree --name-only` on a conflict prints the
        // conflicting paths one-per-line.
        let out = "src/main.rs\nsrc/lib.rs\n";
        let files = parse_merge_tree_conflicts(out);
        assert_eq!(files, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn parse_merge_tree_z_separated_with_sha() {
        // `git merge-tree -z` prints `<tree>\0<file1>\0<file2>\0`.
        let out = "abc123def456abc123def456abc123def456abcd\0src/main.rs\0src/lib.rs\0";
        let files = parse_merge_tree_conflicts(out);
        assert_eq!(files, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn parse_merge_tree_dedupes_paths() {
        let out = "src/main.rs\nsrc/main.rs\nsrc/lib.rs\n";
        let files = parse_merge_tree_conflicts(out);
        assert_eq!(files, vec!["src/main.rs", "src/lib.rs"]);
    }

    /// Integration test: invoke real `git merge-tree --write-tree
    /// --name-only` against a temp repo with a known-conflict
    /// scenario, then verify our parser pulls the conflicting file out
    /// of its stdout. This is the end-to-end claim of the `--check`
    /// mode's conflict prediction. Skipped silently when git is too
    /// old to support `merge-tree --write-tree` (≥2.38).
    #[test]
    fn merge_tree_probe_detects_known_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| -> std::process::Output {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git invocation")
        };

        // Set up a repo with two divergent branches that touch the
        // same file with conflicting changes.
        assert!(git(&["init", "-q", "-b", "main"]).status.success());
        // Pin author / committer so the commits succeed in CI sandboxes
        // that don't have a global git identity configured.
        assert!(git(&["config", "user.email", "test@example.com"])
            .status
            .success());
        assert!(git(&["config", "user.name", "Test"]).status.success());

        std::fs::write(root.join("a.txt"), b"original\n").unwrap();
        assert!(git(&["add", "a.txt"]).status.success());
        assert!(git(&["commit", "-q", "-m", "init"]).status.success());

        assert!(git(&["checkout", "-q", "-b", "feature"]).status.success());
        std::fs::write(root.join("a.txt"), b"feature change\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "feature"]).status.success());

        assert!(git(&["checkout", "-q", "main"]).status.success());
        std::fs::write(root.join("a.txt"), b"main change\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "main update"])
            .status
            .success());

        let out = git(&[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "main",
            "feature",
        ]);
        // Skip silently on git versions without --write-tree.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("unknown option") || stderr.contains("usage: git merge-tree") {
            eprintln!("skipping: git too old for --write-tree");
            return;
        }
        // merge-tree exits non-zero on conflict — that's the signal.
        assert!(!out.status.success(), "expected conflict, got success");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let files = parse_merge_tree_conflicts(&stdout);
        assert!(
            files.iter().any(|f| f == "a.txt"),
            "expected a.txt in conflict list, got {:?} from stdout {:?}",
            files,
            stdout
        );
    }

    // --- STORY-281: stale-base classifier + message formatters ---

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn classify_stale_base_current_when_behind_zero() {
        let out = classify_stale_base(
            0,
            &s(&["src/a.rs"]),
            &s(&["src/a.rs"]), // overlap doesn't matter when behind == 0
            ConflictPrediction::Clean,
        );
        assert_eq!(out, StaleBaseOutcome::Current);
    }

    #[test]
    fn classify_stale_base_no_overlap() {
        let out = classify_stale_base(
            3,
            &s(&["src/a.rs", "src/b.rs"]),
            &s(&["docs/c.md", "src/d.rs"]),
            ConflictPrediction::Clean,
        );
        assert_eq!(out, StaleBaseOutcome::StaleNoOverlap { behind: 3 });
    }

    #[test]
    fn classify_stale_base_with_overlap() {
        let out = classify_stale_base(
            2,
            &s(&["src/a.rs", "src/b.rs"]),
            &s(&["src/b.rs", "src/c.rs"]),
            ConflictPrediction::Clean,
        );
        match out {
            StaleBaseOutcome::StaleOverlap {
                behind,
                overlap_files,
                prediction,
            } => {
                assert_eq!(behind, 2);
                assert_eq!(overlap_files, vec!["src/b.rs".to_string()]);
                assert_eq!(prediction, ConflictPrediction::Clean);
            }
            other => panic!("expected StaleOverlap, got {other:?}"),
        }
    }

    #[test]
    fn classify_stale_base_with_multiple_overlapping_files() {
        let out = classify_stale_base(
            4,
            &s(&["a.rs", "b.rs", "c.rs"]),
            &s(&["a.rs", "b.rs", "d.rs"]),
            ConflictPrediction::Clean,
        );
        match out {
            StaleBaseOutcome::StaleOverlap {
                behind,
                overlap_files,
                prediction,
            } => {
                assert_eq!(behind, 4);
                assert_eq!(overlap_files, vec!["a.rs".to_string(), "b.rs".to_string()]);
                assert_eq!(prediction, ConflictPrediction::Clean);
            }
            other => panic!("expected StaleOverlap, got {other:?}"),
        }
    }

    #[test]
    fn classify_stale_base_overlap_carries_conflict_prediction() {
        let out = classify_stale_base(
            1,
            &s(&["src/a.rs"]),
            &s(&["src/a.rs"]),
            ConflictPrediction::Conflicting {
                files: vec!["src/a.rs".to_string()],
            },
        );
        match out {
            StaleBaseOutcome::StaleOverlap { prediction, .. } => {
                assert_eq!(
                    prediction,
                    ConflictPrediction::Conflicting {
                        files: vec!["src/a.rs".to_string()],
                    }
                );
            }
            other => panic!("expected StaleOverlap, got {other:?}"),
        }
    }

    #[test]
    fn classify_stale_base_blocks_on_predicted_conflict_without_path_overlap() {
        let out = classify_stale_base(
            1,
            &s(&["a.rs"]),
            &s(&["b.rs"]),
            ConflictPrediction::Conflicting {
                files: vec!["a.rs".to_string(), "b.rs".to_string()],
            },
        );
        match out {
            StaleBaseOutcome::StaleOverlap {
                behind,
                overlap_files,
                prediction,
            } => {
                assert_eq!(behind, 1);
                assert_eq!(overlap_files, vec!["a.rs".to_string(), "b.rs".to_string()]);
                assert_eq!(
                    prediction,
                    ConflictPrediction::Conflicting {
                        files: vec!["a.rs".to_string(), "b.rs".to_string()],
                    }
                );
            }
            other => panic!("expected StaleOverlap, got {other:?}"),
        }
    }

    #[test]
    fn stale_base_block_message_names_pr_and_files() {
        let msg = stale_base_block_message(65, 2, &s(&["aida-cli/src/session.rs"]));
        assert!(msg.contains("PR-65"), "{msg}");
        assert!(msg.contains("2 commits behind"), "{msg}");
        assert!(msg.contains("aida-cli/src/session.rs"), "{msg}");
        assert!(msg.contains("aida pr rebase 65"), "{msg}");
        assert!(msg.contains("--allow-stale-base"), "{msg}");
    }

    #[test]
    fn stale_base_block_message_singular_behind() {
        let msg = stale_base_block_message(1, 1, &s(&["a.rs"]));
        assert!(msg.contains("1 commit behind"), "{msg}");
        // No plural "s" after "commit".
        assert!(!msg.contains("1 commits behind"), "{msg}");
    }

    #[test]
    fn stale_base_block_message_truncates_long_overlap() {
        let many: Vec<String> = (0..15).map(|i| format!("file-{i}.rs")).collect();
        let msg = stale_base_block_message(1, 1, &many);
        assert!(msg.contains("file-0.rs"), "{msg}");
        assert!(msg.contains("file-9.rs"), "{msg}");
        assert!(!msg.contains("file-10.rs"), "{msg}");
        assert!(msg.contains("and 5 more"), "{msg}");
    }

    #[test]
    fn stale_base_warn_message_proceeds() {
        let msg = stale_base_warn_message(42, 3);
        assert!(msg.contains("PR-42"), "{msg}");
        assert!(msg.contains("3 commits behind"), "{msg}");
        assert!(msg.contains("proceeding with reviewer"), "{msg}");
        assert!(msg.contains("aida pr rebase 42"), "{msg}");
    }

    // trace:BUG-510 | ai:claude — pin the human-review-verb warning text.
    #[test]
    fn stale_base_review_warn_message_no_overlap() {
        let msg = stale_base_review_warn_message(709, 14, &[]);
        assert!(msg.contains("PR-709"), "{msg}");
        assert!(msg.contains("14 commits behind"), "{msg}");
        assert!(msg.contains("stale code"), "{msg}");
        assert!(msg.contains("aida pr rebase 709"), "{msg}");
        assert!(msg.contains("--allow-stale-base"), "{msg}");
        // Informational, never a refusal.
        assert!(!msg.contains("refusing"), "{msg}");
    }

    #[test]
    fn stale_base_review_warn_message_names_overlap_files() {
        let msg = stale_base_review_warn_message(7, 1, &s(&["src/lib.rs", "src/main.rs"]));
        assert!(msg.contains("1 commit behind"), "{msg}");
        assert!(!msg.contains("1 commits behind"), "{msg}");
        assert!(msg.contains("2 files"), "{msg}");
        assert!(msg.contains("src/lib.rs"), "{msg}");
        assert!(msg.contains("src/main.rs"), "{msg}");
        assert!(msg.contains("aida pr rebase 7"), "{msg}");
    }

    #[test]
    fn stale_base_review_warn_message_truncates_long_overlap() {
        let many: Vec<String> = (0..15).map(|i| format!("file-{i}.rs")).collect();
        let msg = stale_base_review_warn_message(1, 2, &many);
        assert!(msg.contains("file-9.rs"), "{msg}");
        assert!(!msg.contains("file-10.rs"), "{msg}");
        assert!(msg.contains("and 5 more"), "{msg}");
    }

    /// Integration test: a clean rebase scenario — same temp-repo
    /// pattern, but the two branches modify *different* files. The
    /// merge-tree probe must exit zero (stdout is the tree SHA, no
    /// conflicting paths).
    #[test]
    fn merge_tree_probe_clean_when_no_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| -> std::process::Output {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git invocation")
        };

        assert!(git(&["init", "-q", "-b", "main"]).status.success());
        assert!(git(&["config", "user.email", "test@example.com"])
            .status
            .success());
        assert!(git(&["config", "user.name", "Test"]).status.success());

        std::fs::write(root.join("a.txt"), b"a\n").unwrap();
        std::fs::write(root.join("b.txt"), b"b\n").unwrap();
        assert!(git(&["add", "a.txt", "b.txt"]).status.success());
        assert!(git(&["commit", "-q", "-m", "init"]).status.success());

        assert!(git(&["checkout", "-q", "-b", "feature"]).status.success());
        std::fs::write(root.join("a.txt"), b"a-feature\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "feature touches a"])
            .status
            .success());

        assert!(git(&["checkout", "-q", "main"]).status.success());
        std::fs::write(root.join("b.txt"), b"b-main\n").unwrap();
        assert!(git(&["commit", "-q", "-am", "main touches b"])
            .status
            .success());

        let out = git(&[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "main",
            "feature",
        ]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("unknown option") || stderr.contains("usage: git merge-tree") {
            eprintln!("skipping: git too old for --write-tree");
            return;
        }
        assert!(
            out.status.success(),
            "expected clean merge, got stderr={stderr:?}"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let files = parse_merge_tree_conflicts(&stdout);
        assert!(
            files.is_empty(),
            "expected no conflicting files on clean merge, got {:?}",
            files
        );
    }

    // --- TASK-480: intermediate / generated-only diff classifier ---

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Default gitignore predicate for tests: nothing the project
    /// explicitly ignores — so the classifier relies on the
    /// ecosystem-wide `path_looks_generated` heuristic alone.
    fn no_ignores(_: &str) -> bool {
        false
    }

    #[test]
    fn path_looks_generated_catches_build_dirs() {
        assert!(path_looks_generated("target/debug/foo.bin"));
        assert!(path_looks_generated("target/release/aida"));
        assert!(path_looks_generated("./target/debug/foo"));
        assert!(path_looks_generated("crate/target/debug/x"));
        assert!(path_looks_generated("build/output.o"));
        assert!(path_looks_generated("dist/bundle.js"));
        assert!(path_looks_generated("node_modules/left-pad/index.js"));
        assert!(path_looks_generated("app/__pycache__/mod.cpython-311.pyc"));
        assert!(path_looks_generated("frontend\\dist\\app.js")); // windows sep
    }

    #[test]
    fn path_looks_generated_catches_build_suffixes_and_lockfiles() {
        assert!(path_looks_generated("a/b/thing.pyc"));
        assert!(path_looks_generated("x.class"));
        assert!(path_looks_generated("lib/foo.so"));
        assert!(path_looks_generated("bundle.min.js"));
        assert!(path_looks_generated("Cargo.lock"));
        assert!(path_looks_generated("frontend/package-lock.json"));
    }

    #[test]
    fn path_looks_generated_passes_real_source() {
        assert!(!path_looks_generated("src/main.rs"));
        assert!(!path_looks_generated("aida-cli/src/pr_rebase.rs"));
        assert!(!path_looks_generated("Cargo.toml"));
        assert!(!path_looks_generated("README.md"));
        assert!(!path_looks_generated("build.rs")); // a build *script* is source
                                                    // NOTE: a `build/` directory segment IS treated as generated
                                                    // (e.g. `docs/build/notes.md` would be flagged). That's an
                                                    // accepted false-positive risk — the classifier only *refuses*
                                                    // when the WHOLE diff is intermediate, and `git check-ignore`
                                                    // overrides via the gitignore predicate when the project tracks
                                                    // such a path. We don't try to disambiguate here.
    }

    #[test]
    fn classify_path_honors_gitignore_predicate() {
        // A path the heuristic would call source, but the project
        // gitignores → intermediate.
        assert_eq!(
            classify_path("generated/api.rs", true),
            PathClass::Intermediate
        );
        assert_eq!(classify_path("src/main.rs", false), PathClass::Source);
    }

    #[test]
    fn intermediate_only_diff_is_refused() {
        let files = v(&["target/debug/foo.bin", "target/release/aida"]);
        let out = classify_intermediate_only(&files, no_ignores);
        match out {
            IntermediateOnlyOutcome::IntermediateOnly { intermediate } => {
                assert_eq!(intermediate.len(), 2);
            }
            other => panic!("expected IntermediateOnly, got {other:?}"),
        }
    }

    #[test]
    fn gitignored_only_diff_is_refused_even_without_known_path() {
        // Project-specific generated dir not in the heuristic list, but
        // gitignored → still refused.
        let files = v(&["gen/proto/api.pb.rs"]);
        let out = classify_intermediate_only(&files, |p| p.starts_with("gen/"));
        assert!(matches!(
            out,
            IntermediateOnlyOutcome::IntermediateOnly { .. }
        ));
    }

    #[test]
    fn source_only_diff_is_clean() {
        let files = v(&["src/main.rs", "src/lib.rs"]);
        assert_eq!(
            classify_intermediate_only(&files, no_ignores),
            IntermediateOnlyOutcome::Clean
        );
    }

    #[test]
    fn empty_diff_is_clean() {
        assert_eq!(
            classify_intermediate_only(&[], no_ignores),
            IntermediateOnlyOutcome::Clean
        );
    }

    #[test]
    fn source_with_minor_lockfile_noise_is_clean() {
        // Dependency-add: real source change + regenerated lockfile.
        // Intermediate (1) is a minority of the 3 → clean.
        let files = v(&["Cargo.toml", "src/deps.rs", "Cargo.lock"]);
        assert_eq!(
            classify_intermediate_only(&files, no_ignores),
            IntermediateOnlyOutcome::Clean
        );
    }

    #[test]
    fn source_majority_with_one_artifact_is_clean() {
        // 2 source : 1 binary artifact → intermediate ≤ half → clean.
        let files = v(&["src/a.rs", "src/b.rs", "target/debug/a.bin"]);
        assert_eq!(
            classify_intermediate_only(&files, no_ignores),
            IntermediateOnlyOutcome::Clean
        );
    }

    #[test]
    fn intermediate_majority_with_one_source_is_flagged() {
        // 1 source : 3 intermediate → intermediate is the majority →
        // allow but flag.
        let files = v(&[
            "src/foo.rs",
            "target/debug/a.bin",
            "target/debug/b.bin",
            "target/release/c",
        ]);
        match classify_intermediate_only(&files, no_ignores) {
            IntermediateOnlyOutcome::SourcePlusIntermediate { intermediate } => {
                assert_eq!(intermediate.len(), 3);
            }
            other => panic!("expected SourcePlusIntermediate, got {other:?}"),
        }
    }

    #[test]
    fn block_message_names_paths_and_recovery_flag() {
        let msg = intermediate_only_block_message(42, &v(&["target/debug/foo.bin"]));
        assert!(msg.contains("PR-42"), "{msg}");
        assert!(msg.contains("not reproducible"), "{msg}");
        assert!(msg.contains("target/debug/foo.bin"), "{msg}");
        assert!(msg.contains("--allow-intermediate-only"), "{msg}");
    }

    #[test]
    fn block_message_truncates_long_lists() {
        let many: Vec<String> = (0..15).map(|i| format!("target/debug/f{i}.bin")).collect();
        let msg = intermediate_only_block_message(1, &many);
        assert!(msg.contains("… and 5 more"), "{msg}");
    }

    #[test]
    fn warn_message_names_count_and_pr() {
        let msg = intermediate_only_warn_message(7, &v(&["Cargo.lock", "target/debug/a.bin"]));
        assert!(msg.contains("PR-7"), "{msg}");
        assert!(msg.contains("2 intermediate"), "{msg}");
        assert!(msg.contains("Cargo.lock"), "{msg}");
    }

    // --- BUG-640: patch-id force-push guard ---

    #[test]
    fn classify_force_push_empty_is_safe() {
        // No un-incorporated remote commits → safe to force-push.
        assert_eq!(classify_force_push(""), ForcePushGuard::Safe);
        assert_eq!(classify_force_push("\n  \n\n"), ForcePushGuard::Safe);
    }

    #[test]
    fn classify_force_push_nonempty_refuses() {
        // The remote has a commit our branch doesn't carry by patch-id —
        // refuse, naming the offending line(s). This is the simulated
        // "remote ahead with a distinct commit" acceptance case.
        let out = "7660a0a distinct remote commit\n";
        match classify_force_push(out) {
            ForcePushGuard::Unincorporated { commits } => {
                assert_eq!(commits, vec!["7660a0a distinct remote commit".to_string()]);
            }
            other => panic!("expected Unincorporated, got {other:?}"),
        }
    }

    #[test]
    fn classify_force_push_multiple_unincorporated() {
        let out = "aaa one\nbbb two\nccc three\n";
        match classify_force_push(out) {
            ForcePushGuard::Unincorporated { commits } => assert_eq!(commits.len(), 3),
            other => panic!("expected Unincorporated, got {other:?}"),
        }
    }

    #[test]
    fn classify_force_push_ignores_blank_lines() {
        // Trailing/leading blank lines must NOT be mistaken for a commit
        // (that would falsely refuse a safe push).
        let out = "\nzzz only real commit\n\n";
        match classify_force_push(out) {
            ForcePushGuard::Unincorporated { commits } => {
                assert_eq!(commits, vec!["zzz only real commit".to_string()]);
            }
            other => panic!("expected Unincorporated, got {other:?}"),
        }
    }

    #[test]
    fn parse_ls_remote_tip_finds_matching_ref() {
        let out = "5c21b9f56e0dea549c288dd51f6fa18733782fe8\trefs/heads/feature\n";
        assert_eq!(
            parse_ls_remote_tip(out, "refs/heads/feature").as_deref(),
            Some("5c21b9f56e0dea549c288dd51f6fa18733782fe8")
        );
    }

    #[test]
    fn parse_ls_remote_tip_absent_ref_is_none() {
        // Empty output (remote branch doesn't exist) → None → caller
        // treats as "nothing to overwrite" → Safe.
        assert!(parse_ls_remote_tip("", "refs/heads/feature").is_none());
        // A different ref present, ours absent → still None.
        let out = "abc123def456abc123def456abc123def456abcd\trefs/heads/other\n";
        assert!(parse_ls_remote_tip(out, "refs/heads/feature").is_none());
    }

    #[test]
    fn parse_ls_remote_tip_picks_correct_ref_among_many() {
        let out = "1111111111111111111111111111111111111111\trefs/heads/main\n\
                   2222222222222222222222222222222222222222\trefs/heads/feature\n";
        assert_eq!(
            parse_ls_remote_tip(out, "refs/heads/feature").as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn parse_ls_remote_tip_rejects_non_hex_sha() {
        // Defensive: a malformed first column must not be returned as a SHA.
        let out = "not-a-sha\trefs/heads/feature\n";
        assert!(parse_ls_remote_tip(out, "refs/heads/feature").is_none());
    }

    #[test]
    fn force_push_block_message_names_commits_and_recovery() {
        let msg = force_push_block_message("feature", &s(&["7660a0a distinct remote commit"]));
        assert!(msg.contains("Refusing to force-push"), "{msg}");
        assert!(msg.contains("origin/feature"), "{msg}");
        assert!(msg.contains("1 commit "), "{msg}");
        assert!(!msg.contains("1 commits"), "{msg}");
        assert!(msg.contains("7660a0a distinct remote commit"), "{msg}");
        assert!(msg.contains("git rebase origin/feature"), "{msg}");
        assert!(msg.contains("force-with-lease alone does NOT"), "{msg}");
    }

    #[test]
    fn force_push_block_message_truncates_long_lists() {
        let many: Vec<String> = (0..15).map(|i| format!("sha{i} commit {i}")).collect();
        let msg = force_push_block_message("feature", &many);
        assert!(msg.contains("sha0 commit 0"), "{msg}");
        assert!(msg.contains("sha9 commit 9"), "{msg}");
        assert!(!msg.contains("sha10 commit 10"), "{msg}");
        assert!(msg.contains("… and 5 more"), "{msg}");
    }

    #[test]
    fn force_push_inconclusive_message_fails_closed_text() {
        let msg = force_push_inconclusive_message("feature", "git ls-remote exited 128");
        assert!(msg.contains("Refusing to force-push"), "{msg}");
        assert!(msg.contains("origin/feature"), "{msg}");
        assert!(msg.contains("Failing closed"), "{msg}");
        assert!(msg.contains("git ls-remote exited 128"), "{msg}");
    }
}
