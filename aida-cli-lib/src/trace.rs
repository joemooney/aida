//! git linkage, trailer gate, trace check, diff coverage, and doc suggest (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Parse the spec id out of a `git grep -o` hit on the trace regex. The line is
/// the matched text (`trace:SPEC-ID`), possibly carrying a `ref:file:` prefix
/// git adds when grepping a tree; we locate the last `trace:` and read the id
/// after it. Returns the id upper-cased, or None if the token is malformed.
/// trace:TASK-673 | ai:claude
pub(crate) fn parse_trace_id_token(line: &str) -> Option<String> {
    let idx = line.rfind("trace:")?;
    let rest = &line[idx + "trace:".len()..];
    let spec_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(rest.len());
    let spec = &rest[..spec_end];
    // Shape: <ALPHA>+ '-' <DIGITS> (optional '-<DIGITS>'), optionally followed
    // by a criterion suffix (`.A1`, `.AC3`, `.ac1a2b3`). Guard against a bare
    // word or a leading digit so non-spec `trace:` strings don't pollute the set.
    // trace:TASK-1246 | ai:codex
    let first_alpha = spec.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    if !first_alpha || !spec.contains('-') {
        return None;
    }
    if rest[spec_end..].starts_with('.') {
        let suffix_rest = &rest[spec_end + 1..];
        let suffix_end = suffix_rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(suffix_rest.len());
        let suffix = &suffix_rest[..suffix_end];
        if criterion_trace_suffix(suffix) {
            return Some(format!("{spec}.{suffix}").to_ascii_uppercase());
        }
    }
    Some(spec.to_ascii_uppercase())
}

/// Recurse into source files looking for trace comments. Skips the
/// usual "don't grep here" directories.
// trace:EPIC-19 | ai:claude
pub(crate) fn walk_source_for_traces(
    root: &std::path::Path,
    re: &regex::Regex,
    out: &mut std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // Skip noise dirs.
        if path.is_dir() {
            if matches!(
                name,
                ".git"
                    | ".aida-store"
                    | ".aida"
                    | "target"
                    | "node_modules"
                    | "dist"
                    | "build"
                    | ".cache"
                    | ".venv"
                    | "venv"
            ) {
                continue;
            }
            walk_source_for_traces(&path, re, out);
            continue;
        }
        // Read text-like files only. trace:EPIC-19 | ai:claude
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let probably_text = matches!(
            ext,
            "rs" | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "sh"
                | "md"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
        );
        if !probably_text {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            for cap in re.captures_iter(line) {
                if let Some(m) = cap.get(1) {
                    out.entry(m.as_str().to_string())
                        .or_default()
                        .push((path.clone(), lineno + 1));
                }
            }
        }
    }
}

/// One harvested trace comment: a related spec touches `file`, and (when a
/// definition sits on or just below the comment) the named `symbol`.
pub(crate) struct TraceHit {
    pub(crate) file: String,
    pub(crate) symbol: Option<String>,
}

/// Recursively collect source files under `dir`, skipping hidden dirs and
/// the usual build/vendor trees.
pub(crate) fn collect_source_files(
    dir: &std::path::Path,
    exts: &[&str],
    out: &mut Vec<std::path::PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.')
                || name == "target"
                || name == "node_modules"
                || name == "vendor"
            {
                continue;
            }
        }
        if path.is_dir() {
            collect_source_files(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.contains(&ext) {
                out.push(path);
            }
        }
    }
}

/// Scan the codebase for `trace:<SPEC-ID>` comments whose id is in `wanted`,
/// returning spec_id → the trace hits found. The symbol on each hit is the
/// first definition on the trace line or the two lines below it (comment
/// lines are skipped so the comment's own prose can't false-match).
/// trace:TASK-94 | ai:claude
pub(crate) fn scan_trace_graph(
    project_root: &std::path::Path,
    wanted: &HashSet<String>,
) -> std::collections::HashMap<String, Vec<TraceHit>> {
    scan_trace_graph_bounded(project_root, wanted, None).0
}

/// BUG-1594: [`scan_trace_graph`] with an optional wall-clock `budget`.
/// Returns the hits plus `complete` — `false` when the budget ran out before
/// every source file was read, so a caller can say the file list is partial
/// instead of presenting it as complete (PRIN-5). A file is only line-split
/// and regex-matched when its raw text contains `trace:` and one of the
/// wanted ids — the common case (a file that never mentions the spec) now
/// costs one substring search instead of a regex pass over every line.
// trace:BUG-1594 | ai:claude
pub(crate) fn scan_trace_graph_bounded(
    project_root: &std::path::Path,
    wanted: &HashSet<String>,
    budget: Option<std::time::Duration>,
) -> (std::collections::HashMap<String, Vec<TraceHit>>, bool) {
    use regex::Regex;
    let started = std::time::Instant::now();
    // Capture the full spec id — including the optional third segment of a
    // node-aware id (`FR-1-042`), or `trace:FR-1-042` would bucket under a
    // bogus `FR-1`.
    let trace_re =
        Regex::new(r"\btrace:([A-Z]+-[A-Z0-9]+(?:-[A-Z0-9]+)*-[0-9]+|[A-Z]+-[0-9]+)").unwrap();
    let sym_re = Regex::new(
        r"\b(?:fn|struct|enum|trait|type|mod|const|static|function|class|interface)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .unwrap();
    // BUG-568: trace comments are indexed only from the LOCAL source tree, so
    // trace markers in sibling repos of a shared-store workspace are never
    // found. Warn loudly (deduped per-process). trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "trace-comment scan");

    let exts = ["rs", "ts", "tsx", "js", "jsx", "py", "sh"];
    let mut files = Vec::new();
    collect_source_files(project_root, &exts, &mut files);

    let mut out: std::collections::HashMap<String, Vec<TraceHit>> =
        std::collections::HashMap::new();
    let mut complete = true;
    for path in &files {
        if budget.is_some_and(|b| started.elapsed() >= b) {
            complete = false;
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        // BUG-1594: cheap prefilter before the per-line regex pass.
        if !content.contains("trace:") || !wanted.iter().any(|id| content.contains(id.as_str())) {
            continue;
        }
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            for cap in trace_re.captures_iter(line) {
                let id = cap[1].to_string();
                if !wanted.contains(&id) {
                    continue;
                }
                // Symbol: first definition on this line or the next two,
                // skipping pure comment lines.
                let mut symbol = None;
                for probe in lines.iter().skip(i).take(3) {
                    let t = probe.trim_start();
                    if t.starts_with("//") || t.starts_with('#') || t.starts_with('*') {
                        continue;
                    }
                    if let Some(m) = sym_re.captures(probe) {
                        symbol = Some(m[1].to_string());
                        break;
                    }
                }
                let rel = path
                    .strip_prefix(project_root)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                out.entry(id)
                    .or_default()
                    .push(TraceHit { file: rel, symbol });
            }
        }
    }
    (out, complete)
}

/// STORY-511: the resolved state of the change-request (PR/MR) line in
/// `aida show`'s git-linkage section, decoupled from the git/forge probes
/// so the *rendering* is a pure function of (forge, state) — and thus
/// unit-testable with no git repo and no `gh`/`glab` on PATH. EPIC-35
/// slice 5 makes this section forge-aware: a GitLab repo reads "MR-47"
/// and "glab not installed", not the GitHub-only "PR-47"/"gh" wording.
/// trace:STORY-511 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangeLinkageState {
    /// Shipped (merged to the default branch); carries the squash-merge
    /// change number when one was parsed from the subject.
    Shipped { number: Option<u64> },
    /// In flight, an open change was found. `number` + `url`.
    InFlightFound { number: u64, url: String },
    /// In flight, the forge CLI ran cleanly and reported no open change.
    InFlightNoChange,
    /// In flight, the forge CLI is not on PATH.
    CliMissing,
    /// In flight, the forge CLI ran but errored (auth / parse).
    CliFailed,
    /// In flight, the forge API was unreachable (transient).
    Unreachable,
    /// Work is committed but no local branch holds it.
    BranchNotFound,
    /// BUG-1594: the forge lookup was killed at its per-call ceiling.
    // trace:BUG-1594 | ai:claude
    TimedOut,
    /// BUG-1594: the shared forge-lookup budget was already spent, so this
    /// branch's lookup never ran.
    // trace:BUG-1594 | ai:claude
    LookupSkipped,
}

/// BUG-1594: map a forge lookup onto the rendered linkage state (plus the
/// open change's url), distinguishing a ceiling timeout from a generic
/// unreachable API. Shared by the primary and sibling-branch lookups.
// trace:BUG-1594 | ai:claude
pub(crate) fn change_linkage_state_for(
    lookup: crate::forge::ChangeLookup,
) -> (ChangeLinkageState, Option<String>) {
    match lookup {
        crate::forge::ChangeLookup::Found(c) => (
            ChangeLinkageState::InFlightFound {
                number: c.id,
                url: c.url.clone(),
            },
            Some(c.url),
        ),
        crate::forge::ChangeLookup::NoChange => (ChangeLinkageState::InFlightNoChange, None),
        crate::forge::ChangeLookup::CliMissing => (ChangeLinkageState::CliMissing, None),
        crate::forge::ChangeLookup::CliFailed(_) => (ChangeLinkageState::CliFailed, None),
        crate::forge::ChangeLookup::Unreachable(msg) if msg == FORGE_LOOKUP_TIMED_OUT => {
            (ChangeLinkageState::TimedOut, None)
        }
        crate::forge::ChangeLookup::Unreachable(_) => (ChangeLinkageState::Unreachable, None),
    }
}

/// BUG-1594: the note `aida show` prints when the in-flight branch search
/// stopped before covering every referencing commit.
// trace:BUG-1594 | ai:claude
pub(crate) fn format_branch_scan_truncated_note(scanned: usize, total: usize) -> String {
    format!(
        "branch search incomplete: checked the newest {scanned} of {total} commits \
         — other branches may also reference this spec"
    )
}

/// STORY-511: render the change-request linkage lines for `aida show`'s
/// git-linkage section, forge-aware. Returns `(label, value)` pairs of
/// **plain, uncolored** text in render order (caller applies styling).
/// The change noun comes from [`crate::forge::ForgeKind::change_noun`]
/// ("PR" / "MR" / "change") and the CLI name from
/// [`crate::forge::ForgeKind::cli_name`] ("gh" / "glab" / "the forge CLI")
/// so a GitLab project reads "MR-47" and "glab not installed" rather than
/// the GitHub-only wording. Pure: no git, no forge CLI, no I/O — fully
/// unit-testable in isolation. trace:STORY-511 | ai:claude
pub(crate) fn format_change_linkage(
    forge: crate::forge::ForgeKind,
    state: &ChangeLinkageState,
) -> Vec<(String, String)> {
    let noun = forge.change_noun(); // "PR" | "MR" | "change"
    let cli = forge.cli_name(); // "gh" | "glab" | ""
                                // Pure-git has no forge CLI binary; name it generically rather than
                                // printing an empty token in "<cli> not installed".
    let cli_label = if cli.is_empty() { "the forge CLI" } else { cli };
    let mut out: Vec<(String, String)> = Vec::new();
    match state {
        ChangeLinkageState::Shipped { number } => {
            out.push(("Branch".to_string(), "merged to main".to_string()));
            if let Some(num) = number {
                out.push((noun.to_string(), format!("{noun}-{num}")));
            }
        }
        ChangeLinkageState::InFlightFound { number, url } => {
            out.push((noun.to_string(), format!("{noun}-{number} {url}")));
        }
        ChangeLinkageState::InFlightNoChange => {
            out.push((noun.to_string(), format!("no {noun} opened yet")));
        }
        ChangeLinkageState::CliMissing => {
            out.push((
                noun.to_string(),
                format!("{cli_label} not installed — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::CliFailed => {
            out.push((
                noun.to_string(),
                format!("{cli_label} lookup failed — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::Unreachable => {
            out.push((
                noun.to_string(),
                format!("{noun} API unreachable — {noun} state unknown (transient)"),
            ));
        }
        ChangeLinkageState::TimedOut => {
            // trace:BUG-1594 | ai:claude
            out.push((
                noun.to_string(),
                format!(
                    "{cli_label} lookup timed out after {}s — {noun} state unknown",
                    FORGE_CLI_CALL_TIMEOUT.as_secs()
                ),
            ));
        }
        ChangeLinkageState::LookupSkipped => {
            // trace:BUG-1594 | ai:claude
            out.push((
                noun.to_string(),
                format!("{noun} lookup skipped (time budget spent) — {noun} state unknown"),
            ));
        }
        ChangeLinkageState::BranchNotFound => {
            // BUG-1528 (PRIN-5): a failed branch lookup must never silently
            // suppress the PR line. Before this fix, `BranchNotFound` was
            // the one arm of this enum that emitted no PR/MR line at all —
            // every other "in-flight but uncertain" arm (CliMissing /
            // CliFailed / Unreachable) already says "state unknown" instead
            // of going quiet. Match that convention here too, distinctly
            // from "no PR was ever opened" (`InFlightNoChange`).
            // trace:BUG-1528 | ai:claude
            out.push((
                "Branch".to_string(),
                "work committed but branch not found locally".to_string(),
            ));
            out.push((
                noun.to_string(),
                format!("{noun} state unknown — branch not found locally"),
            ));
        }
    }
    out
}

/// TASK-241: the git-linkage data for a spec — extracted from
/// [`print_git_linkage`] so the collection (git-only, never gh) is
/// unit-testable against a temp repo. trace:TASK-241 | ai:claude
// trace:STORY-82 | ai:claude — `pub(crate)` so the MCP `show_requirement`
// tool can reuse the same git-linkage collection `aida show` renders.
pub(crate) struct GitLinkage {
    /// (full_sha, short_sha, subject), newest first.
    pub(crate) commits: Vec<(String, String, String)>,
    /// (file, symbol) per trace comment — deduped and sorted.
    pub(crate) files: Vec<(String, Option<String>)>,
    /// The newest referencing commit is an ancestor of main.
    pub(crate) shipped: bool,
    /// Feature branch holding the work (in-flight case only).
    pub(crate) branch: Option<String>,
    /// BUG-1528: other branches (besides `branch`) whose name matches the
    /// spec id and that also carry a referencing commit — a branch-crossing
    /// signal (e.g. `bug-1420-work` + `bug-1420-round2` both open). Rendered
    /// as a note so a reviewer sees the fan-out instead of one branch picked
    /// silently. Empty in the common single-branch case.
    // trace:BUG-1528 | ai:claude
    pub(crate) other_branches: Vec<String>,
    /// BUG-1594: `Some((scanned, total))` when the in-flight branch search
    /// stopped before walking every referencing commit (commit cap or wall-
    /// clock budget hit), so `branch`/`other_branches` may be incomplete.
    /// Rendered as an explicit note — partial linkage is never shown as
    /// complete (PRIN-5). `None` when the search covered every commit.
    // trace:BUG-1594 | ai:claude
    pub(crate) branch_scan_truncated: Option<(usize, usize)>,
    /// BUG-1594: the trace-comment walk hit its time budget, so `files` may
    /// be missing entries. Rendered as an explicit note (PRIN-5).
    // trace:BUG-1594 | ai:claude
    pub(crate) files_scan_incomplete: bool,
    /// BUG-1993: the referencing-commit `git log --all` hit its time budget,
    /// so `commits` is empty and the shipped state is unknown. Rendered as an
    /// explicit note (PRIN-5), never as "no commits" or "not shipped".
    // trace:BUG-1993 | ai:claude
    pub(crate) commits_scan_incomplete: bool,
    /// BUG-1993: the shipped state is unknown — the commit scan or the
    /// `merge-base --is-ancestor` check did not finish in budget. `shipped`
    /// is `false` then, but renderers must read it as "unknown".
    // trace:BUG-1993 | ai:claude
    pub(crate) shipped_unknown: bool,
    /// Worktree path checked out at `branch`, if any.
    pub(crate) worktree: Option<String>,
    /// PR number parsed from a squash-merge subject (shipped case only).
    pub(crate) shipped_pr: Option<u64>,
    /// Workspace repo slug this linkage was scanned from (ADR-12 D5: the
    /// `origin.repo` join key qualifying commits/branch/PR). `None` in the
    /// single-repo case — rendering stays unqualified, unchanged.
    // trace:STORY-634 | ai:claude
    pub(crate) repo: Option<String>,
}

impl GitLinkage {
    /// PRIN-5: one note per bounded scan that stopped early, in render
    /// order. Empty when every scan finished, so `is_empty()` is the
    /// "linkage is complete" test the JSON and Markdown surfaces share.
    // trace:BUG-1993 | ai:claude
    pub(crate) fn incomplete_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.commits_scan_incomplete {
            notes.push(LINKAGE_COMMIT_SCAN_INCOMPLETE_NOTE.to_string());
        } else if self.shipped_unknown {
            notes.push(LINKAGE_SHIPPED_UNKNOWN_NOTE.to_string());
        }
        if let Some((scanned, total)) = self.branch_scan_truncated {
            notes.push(format_branch_scan_truncated_note(scanned, total));
        }
        if self.files_scan_incomplete {
            notes.push(LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE.to_string());
        }
        notes
    }
}

/// BUG-1993: the budget for the referencing-commit `git log --all --grep`.
/// Measured on this repo (73k commits, 5k refs, idle host, 2026-10-14):
/// 0.6-1.1 s. The headroom is for a loaded host; past it the commit list is
/// reported incomplete and the shipped state unknown, never "not shipped".
/// (#2620 set budgets below the measured cost and showed shipped specs as
/// unshipped.)
// trace:BUG-1993 | ai:claude
pub(crate) const LINKAGE_COMMIT_LOG_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(3);

/// BUG-1993: the budget for the `merge-base --is-ancestor` shipped check.
/// Measured on this repo: ~22 ms. Past it the shipped state is unknown.
// trace:BUG-1993 | ai:claude
pub(crate) const LINKAGE_SHIPPED_CHECK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(1);

/// BUG-1594: the most referencing commits the in-flight branch search walks
/// (newest first) before it stops and reports itself partial.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_BRANCH_SCAN_MAX_COMMITS: usize = 20;

/// BUG-1594: the wall-clock budget for the in-flight branch search.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_BRANCH_SCAN_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(2);

/// BUG-1594: the wall-clock budget for the trace-comment source walk.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_TRACE_SCAN_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// BUG-1594: the total wall-clock budget `aida show` spends on forge
/// (PR/MR) lookups across the primary branch and any sibling branches; each
/// single call is additionally capped at [`FORGE_CLI_CALL_TIMEOUT`].
// trace:BUG-1594 | ai:claude
pub(crate) const SHOW_FORGE_LOOKUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

/// TASK-1475: wall-clock ceiling for EACH bounded git call the filing-drift
/// hint makes (at most two: the linked-commits file-touch lookup, and the
/// `code_sha..HEAD` log walk). Local git only, no network — this is a
/// safety net against a pathological huge history, not the expected cost.
// trace:TASK-1475 | ai:claude
pub(crate) const DRIFT_HINT_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// TASK-1475 (follow-up to CR-8 acceptance 6): a cheap filing-provenance
/// staleness signal — how many commits since `filed_at.code_sha` touched a
/// file this spec's `trace:` comments or linked commits point at. Local git
/// only (no network), every subprocess bounded via
/// [`command_output_with_timeout`] so a slow/huge repo can only ever cost
/// this function its fixed per-call budget, never hang `aida show`/`aida
/// why`.
///
/// `linkage_files`/`linkage_commits` are the already-collected
/// [`GitLinkage::files`]/[`GitLinkage::commits`] — this never re-runs the
/// trace-comment tree walk or the referencing-commit log.
///
/// Returns `None` when there is nothing to say: no provenance, no
/// `code_sha`, an unresolvable sha (shallow clone / a different repo —
/// TASK-1475 AC2), no traced files at all, or a genuinely driftless zero
/// (no commits since filing touched any traced file — not worth a line).
/// When the sha resolves but the bounded walk can't finish in budget,
/// returns `Some(..)` naming the state "unknown" rather than silently
/// reporting zero — a truncated scan must never be mistaken for a complete,
/// driftless one (PRIN-5).
// trace:TASK-1475 | ai:claude
pub(crate) fn filing_drift_hint(
    project_root: &std::path::Path,
    filed_at: Option<&aida_core::FilingProvenance>,
    linkage_files: &[(String, Option<String>)],
    linkage_commits: &[(String, String, String)],
) -> Option<String> {
    let code_sha = filed_at.and_then(|p| p.code_sha.as_deref())?.trim();
    // `code_sha` comes from the shared store, which another writer controls:
    // only a commit ID may reach `git log`. trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(code_sha) {
        return None;
    }

    // Traced files: every location a `trace:` comment names this spec, plus
    // every file the spec's own linked (referencing) commits touched — one
    // bounded `git show` covering all of them at once.
    let mut traced: std::collections::BTreeSet<String> =
        linkage_files.iter().map(|(f, _)| f.clone()).collect();
    if !linkage_commits.is_empty() {
        let shas: Vec<&str> = linkage_commits
            .iter()
            .take(LINKAGE_BRANCH_SCAN_MAX_COMMITS)
            .map(|(full, _, _)| full.as_str())
            .collect();
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C")
            .arg(project_root)
            .args(["show", "--format=", "--name-only"])
            // trace:BUG-1622 | ai:claude
            .arg(git_arg_guard::END_OF_OPTIONS)
            .args(&shas);
        if let Some(out) = command_output_with_timeout(cmd, DRIFT_HINT_GIT_TIMEOUT) {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        traced.insert(line.to_string());
                    }
                }
            }
        }
    }
    if traced.is_empty() {
        return None;
    }

    // One bounded `git log <code_sha>..HEAD --name-only -- <traced files>`
    // does double duty: a non-zero exit means the sha didn't resolve
    // (shallow clone / different repo — AC2, degrade to silence); a
    // timeout means the walk itself couldn't finish (degrade to "unknown",
    // never a guessed zero); success parses into a commit count plus the
    // distinct traced files those commits actually touched (pathspec-
    // restricted, so this is always <= traced.len()).
    let mut log_cmd = std::process::Command::new("git");
    log_cmd
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--name-only",
            "--pretty=format:\u{1}",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{code_sha}..HEAD"),
        ])
        .arg("--")
        .args(traced.iter());
    match command_output_with_timeout(log_cmd, DRIFT_HINT_GIT_TIMEOUT) {
        Some(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut commit_count = 0usize;
            let mut touched: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for chunk in text.split('\u{1}') {
                let chunk = chunk.trim();
                if chunk.is_empty() {
                    continue;
                }
                commit_count += 1;
                for line in chunk.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        touched.insert(line);
                    }
                }
            }
            if commit_count == 0 {
                // Genuinely driftless — nothing to hint at.
                None
            } else {
                Some(format!(
                    "{commit_count} commit{} since filing touched {} traced file{}",
                    if commit_count == 1 { "" } else { "s" },
                    touched.len(),
                    if touched.len() == 1 { "" } else { "s" },
                ))
            }
        }
        Some(_) => None,
        None => Some("commits since filing: unknown (scan didn't finish in budget)".to_string()),
    }
}

/// TASK-241: collect the git linkage for `ids` — commits referencing the
/// AIDA `(SPEC-ID)` format, files carrying `trace:` comments, and
/// branch/worktree/shipped state. gh-free by design: the in-flight
/// open-PR lookup happens later, in [`print_git_linkage`].
/// trace:TASK-241 | ai:claude
pub(crate) fn collect_git_linkage(project_root: &std::path::Path, ids: &[String]) -> GitLinkage {
    collect_git_linkage_opts(project_root, ids, true)
}

/// The orphan git-canonical requirements-store branch name (see CLAUDE.md
/// "Storage model"). Its `update SPEC-NNN` / `add SPEC-NNN` bookkeeping
/// commits — and cross-node store-lineage merges — mention a bare spec id in
/// the subject, which is indistinguishable at the grep layer from a real
/// code commit. Any scan that resolves a spec id to a CODE branch/commit
/// range must exclude this branch, or it mistakes the requirements store
/// itself for the spec's feature branch (`aida review` / `aida human review`
/// offered to open a PR from `aida-store`). `scan_completed_without_commit`
/// avoids the same trap structurally, by only ever walking the resolved
/// default CODE branch.
///
/// Accepts either a bare branch name (`aida-store`) or a remote-qualified
/// short ref (`origin/aida-store`, `gitlab/aida-store`, …) — a multi-hub
/// project (STORY-760) can have the orphan branch mirrored under several
/// remote prefixes, and `git branch --all --contains` reports it under
/// each, so matching only the bare literal would miss every remote but one.
// trace:BUG-720 | ai:claude
pub(crate) fn is_orphan_store_branch(branch: &str) -> bool {
    let name = branch.rsplit('/').next().unwrap_or(branch);
    name.eq_ignore_ascii_case("aida-store")
}

/// BUG-550: variant of [`collect_git_linkage`] with the full source-tree walk
/// (`scan_trace_graph`, which populates `GitLinkage::files`) gated behind
/// `scan_trace`. Surfaces that classify a spec's *review state* — the `aida
/// human` reviews bucket runs this once per candidate spec — only read
/// `commits`/`branch`/`shipped`/`shipped_pr`, never `files`, so they pass
/// `false` to skip the per-spec tree scan. That scan reads every source file
/// in the repo; multiplying it by a widened candidate set is the dominant
/// cost on this hot, offline command. With `scan_trace = false`, `files` is
/// always empty. trace:BUG-550 | ai:claude
pub(crate) fn collect_git_linkage_opts(
    project_root: &std::path::Path,
    ids: &[String],
    scan_trace: bool,
) -> GitLinkage {
    use std::process::Command as PCmd;

    // BUG-568: linkage (commits/files/PR) is collected only from the LOCAL
    // repo. In a shared-store multi-repo workspace, work done in a sibling repo
    // shows zero linkage here — warn loudly so the gap is visible rather than
    // read as "no work done". Suppressible via AIDA_QUIET for hot batch callers
    // (e.g. the `aida human` reviews sweep). trace:BUG-568 | ai:claude
    warn_multi_repo_scan_limited(project_root, "git-linkage scan");

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
    };
    // BUG-380: capture stderr via output() so an invalid `target` ref
    // cannot leak `fatal: Not a valid object name <X>` to the user's
    // terminal — belt-and-braces alongside `resolve_default_branch_ref`,
    // which already filters `target` to a ref that exists.
    // BUG-1993: bounded; `None` means the check did not finish in budget,
    // so the shipped state is unknown rather than false.
    // trace:BUG-1993 | ai:claude
    let is_ancestor = |sha: &str, target: &str| -> Option<bool> {
        let mut cmd = PCmd::new("git");
        cmd.arg("-C")
            .arg(project_root)
            .args(["merge-base", "--is-ancestor", sha, target]);
        match command_output_with_timeout_detail(cmd, LINKAGE_SHIPPED_CHECK_TIMEOUT) {
            BoundedCommandOutput::Completed(o) => Some(o.status.success()),
            BoundedCommandOutput::SpawnFailed => Some(false),
            BoundedCommandOutput::TimedOut => None,
        }
    };

    // ---- Commits referencing the spec (across all refs) ----
    // BUG-546: grep the BARE id across all refs, then keep only commits whose
    // FULL message resolves to the wanted id through the same robust parser
    // the auto-bump / reconcile surfaces use (`(SPEC-ID)` trailers — including
    // multi-spec / prose parens like `(TASK-800 / STORY-610 slice 1a)` — plus
    // `trace:SPEC-ID` lines in the body). The old approach grepped the fixed
    // string `(SPEC-ID)`, so a real PR whose subject paren named the spec
    // alongside prose was invisible to `aida review <spec>` and the `aida
    // human` reviews bucket. The post-grep resolver replaces the parens-only
    // filter that previously excluded the orphan `aida-store` bookkeeping
    // commits ("update TASK-92"): those carry the bare id but neither a
    // trailer nor a `trace:` line, so the resolver drops them. trace:BUG-546
    let wanted_lc: HashSet<String> = ids.iter().map(|s| s.to_ascii_uppercase()).collect();
    let mut args: Vec<&str> = vec![
        "log",
        "--all",
        "-F",
        "--date-order",
        "--pretty=format:%H\u{1f}%h\u{1f}%s\u{1f}%B%x1e",
    ];
    for id in ids {
        args.push("--grep");
        args.push(id.as_str());
    }
    // BUG-1993: the `--all` walk is the costliest linkage call (0.6-1.1 s
    // idle on this repo), so it is bounded. A timeout yields no commits and
    // marks the scan incomplete; a spawn failure or git error stays the old
    // silent empty result. trace:BUG-1993 | ai:claude
    let mut log_cmd = PCmd::new("git");
    log_cmd.arg("-C").arg(project_root).args(&args);
    let (log, commits_scan_incomplete) =
        match command_output_with_timeout_detail(log_cmd, LINKAGE_COMMIT_LOG_TIMEOUT) {
            BoundedCommandOutput::Completed(o) if o.status.success() => (
                String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
                false,
            ),
            BoundedCommandOutput::TimedOut => (String::new(), true),
            _ => (String::new(), false),
        };
    let commits: Vec<(String, String, String)> = log
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            if record.is_empty() {
                return None;
            }
            let mut p = record.splitn(4, '\u{1f}');
            let full = p.next()?.to_string();
            let short = p.next()?.to_string();
            let subject = p.next()?.to_string();
            let body = p.next().unwrap_or("");
            // Resolve the commit's full message and keep it only when it
            // actually references one of the wanted ids via a trailer or a
            // `trace:` line — not just a bare mention. trace:BUG-546
            let mut resolved = extract_spec_ids_from_commit(body);
            resolved.extend(extract_referenced_spec_ids_from_commit(body));
            resolved.extend(extract_trace_line_spec_ids(body));
            if resolved
                .iter()
                .any(|r| wanted_lc.contains(&r.to_ascii_uppercase()))
            {
                Some((full, short, subject))
            } else {
                None
            }
        })
        .collect();

    // ---- Files carrying trace comments for the spec ----
    // BUG-550: gated — the per-spec source-tree walk only matters to callers
    // that render `files`; the reviews-bucket classifier skips it.
    // BUG-1594: bounded — a cold page cache made this walk alone take tens
    // of seconds; past the budget the list is flagged partial, not complete.
    let mut files_scan_incomplete = false;
    let files: Vec<(String, Option<String>)> = if scan_trace {
        let wanted: HashSet<String> = ids.iter().cloned().collect();
        let (trace_hits, complete) =
            scan_trace_graph_bounded(project_root, &wanted, Some(LINKAGE_TRACE_SCAN_BUDGET));
        files_scan_incomplete = !complete;
        let mut f: Vec<(String, Option<String>)> = trace_hits
            .values()
            .flatten()
            .map(|h| (h.file.clone(), h.symbol.clone()))
            .collect();
        f.sort();
        f.dedup();
        f
    } else {
        Vec::new()
    };

    // ---- Branch / worktree / shipped state (anchored on newest commit) ----
    let mut shipped = false;
    let mut branch: Option<String> = None;
    let mut other_branches: Vec<String> = Vec::new();
    let mut branch_scan_truncated: Option<(usize, usize)> = None;
    let mut worktree: Option<String> = None;
    let mut shipped_pr: Option<u64> = None;
    let mut shipped_unknown = commits_scan_incomplete;
    if let Some((full, _, _)) = commits.first() {
        // BUG-380: resolve a ref that exists rather than passing a
        // literal "main"; on master-default repos the literal leaks
        // `fatal: Not a valid object name 'main'` from git through
        // is_ancestor's inherited stderr.
        let verdict = resolve_default_branch_ref(project_root)
            .map(|main_ref| is_ancestor(full, &main_ref))
            .unwrap_or(Some(false));
        shipped = verdict == Some(true);
        // trace:BUG-1993 | ai:claude
        shipped_unknown = verdict.is_none();
        if shipped_unknown {
            // BUG-1993: unknown is neither shipped nor in flight; skip the
            // in-flight branch walk rather than present a guessed branch.
        } else if shipped {
            // A squash-merge subject ends with `(#NN)` — the cheapest,
            // most reliable PR pointer for shipped work.
            shipped_pr = commits
                .iter()
                .find_map(|(_, _, s)| parse_squash_pr_number(s));
        } else {
            // In flight: find the feature branch that holds the work.
            //
            // BUG-1528: don't anchor solely on the SINGLE newest referencing
            // commit. When a spec has multiple branches in flight (a
            // branch-crossing — e.g. `bug-1420-work` plus a later
            // `bug-1420-round2`), the newest commit across ALL of them can
            // live on a branch that gets filtered out below (main/master/
            // orphan-store) or simply isn't the spec's own branch, leaving
            // the spec's actual local branch entirely unreachable via
            // `--contains <newest-sha>` even though `git branch --list`
            // plainly shows it. Union the `--contains` result over EVERY
            // referencing commit (still newest-first, so ties still prefer
            // recency) so a branch holding an older-but-still-relevant
            // commit is found too. trace:BUG-1528 | ai:claude
            let norm_id = |s: &str| s.to_ascii_lowercase().replace([' ', '_'], "-");
            let mut candidates: Vec<String> = Vec::new();
            let mut local_candidates: Vec<String> = Vec::new();
            // BUG-1594: `git branch --contains` is one subprocess per
            // referencing commit, and a spec with a long commit trail (or
            // commits old enough that every agent branch contains them) made
            // this loop dominate `aida show`. Walk newest-first under a
            // commit cap AND a wall-clock budget; if either stops the walk,
            // record it so the renderer says the branch search is partial.
            // trace:BUG-1594 | ai:claude
            let scan_started = std::time::Instant::now();
            let mut scanned = 0usize;
            for (commit_full, _, _) in &commits {
                if scanned >= LINKAGE_BRANCH_SCAN_MAX_COMMITS
                    || scan_started.elapsed() >= LINKAGE_BRANCH_SCAN_BUDGET
                {
                    break;
                }
                scanned += 1;
                let Some(contains) = git(&[
                    "branch",
                    "--all",
                    "--contains",
                    commit_full,
                    "--format=%(refname:short)",
                ]) else {
                    continue;
                };
                // BUG-553: a commit can be reachable from MULTIPLE branches when a
                // later spec's branch was stacked on this one's unmerged commit
                // (the BUG-554 anti-pattern). Picking the first arbitrarily then
                // mis-attributes the spec to a sibling's branch (e.g. TASK-806
                // shown on `task-805`). Prefer the branch whose name matches one of
                // the spec ids being resolved (the spec's OWN branch, `TASK-806` →
                // `task-806`); fall back to the first only when none matches.
                // trace:BUG-553 | ai:claude
                //
                // BUG-720: also exclude the orphan `aida-store` branch. A commit
                // can be reachable ONLY from `aida-store` (its own bookkeeping
                // commit, or a cross-node store-lineage merge that names the spec
                // in parens) — never offer it as the spec's review branch, or
                // `aida review`/`aida human review` prompts to PR the entire
                // requirements store as a code change.
                for raw in contains.lines() {
                    let raw = raw.trim();
                    // BUG-1591: remember which candidates exist as a LOCAL
                    // branch — stripping `origin/` erased that, so selection
                    // fell to commit recency and a newer remote-only branch
                    // beat the spec's own local branch. trace:BUG-1591 | ai:claude
                    let is_remote = raw.starts_with("origin/") || raw.starts_with("remotes/");
                    let b = raw
                        .trim_start_matches("remotes/")
                        .trim_start_matches("origin/");
                    if !is_remote && !b.is_empty() && !local_candidates.iter().any(|c| c == b) {
                        local_candidates.push(b.to_string());
                    }
                    if !b.is_empty()
                        && b != "HEAD"
                        && b != "main"
                        && b != "master"
                        && !is_orphan_store_branch(b)
                        && !candidates.iter().any(|c| c == b)
                    {
                        candidates.push(b.to_string());
                    }
                }
            }
            if scanned < commits.len() {
                branch_scan_truncated = Some((scanned, commits.len()));
            }
            // BUG-1528: every candidate whose name matches the spec's own
            // id, in first-seen (recency) order. The first becomes `branch`;
            // any rest are a branch-crossing signal surfaced as
            // `other_branches` rather than silently dropped. trace:BUG-1528
            let mut id_matches: Vec<String> = candidates
                .iter()
                .filter(|b| {
                    let bl = b.to_ascii_lowercase();
                    ids.iter().any(|id| {
                        let nid = norm_id(id);
                        !nid.is_empty() && (bl == nid || bl.contains(&nid))
                    })
                })
                .cloned()
                .collect();
            // BUG-1591: a spec's own LOCAL branch outranks a remote-only one;
            // recency order is kept within each group (stable sort), so the
            // choice no longer depends on which commit git lists first.
            // trace:BUG-1591 | ai:claude
            id_matches.sort_by_key(|b| !local_candidates.iter().any(|c| c == b));
            if id_matches.is_empty() {
                // No candidate matches the spec's own id by name — fall back
                // to the first (newest) candidate found, as before.
                branch = candidates.into_iter().next();
            } else {
                branch = Some(id_matches.remove(0));
                other_branches = id_matches;
            }
            if let (Some(b), Some(wt)) =
                (branch.as_deref(), git(&["worktree", "list", "--porcelain"]))
            {
                let mut cur_path: Option<String> = None;
                for line in wt.lines() {
                    if let Some(p) = line.strip_prefix("worktree ") {
                        cur_path = Some(p.to_string());
                    } else if let Some(r) = line.strip_prefix("branch ") {
                        if r.trim_start_matches("refs/heads/") == b {
                            worktree = cur_path.clone();
                        }
                    }
                }
            }
        }
    }

    GitLinkage {
        commits,
        files,
        shipped,
        branch,
        other_branches,
        branch_scan_truncated,
        files_scan_incomplete,
        commits_scan_incomplete,
        shipped_unknown,
        worktree,
        shipped_pr,
        // trace:STORY-634 | ai:claude — the scanned repo's workspace slug.
        repo: workspace_repo_slug(project_root),
    }
}

pub(crate) fn print_git_linkage(
    project_root: &std::path::Path,
    ids: &[String],
    verbose: bool,
    filed_at: Option<&aida_core::FilingProvenance>,
) {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
    };

    let GitLinkage {
        commits,
        files,
        shipped,
        branch,
        other_branches,
        branch_scan_truncated,
        files_scan_incomplete,
        commits_scan_incomplete,
        shipped_unknown,
        worktree,
        shipped_pr,
        // trace:STORY-634 | ai:claude
        repo,
    } = collect_git_linkage(project_root, ids);

    if commits.is_empty() && files.is_empty() && commits_scan_incomplete {
        // BUG-1993 (PRIN-5): the commit scan timed out — the linkage is
        // unknown, not absent. trace:BUG-1993 | ai:claude
        println!(
            "\n{}: {}",
            "Git linkage".green().bold(),
            LINKAGE_COMMIT_SCAN_INCOMPLETE_NOTE.yellow()
        );
        if files_scan_incomplete {
            println!("  {}", LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE.yellow());
        }
        return;
    }

    if commits.is_empty() && files.is_empty() && files_scan_incomplete {
        // BUG-1594 (PRIN-5): an empty result from a scan that did not finish
        // is "unknown", not "no linkage" — never print the newcomer hint.
        // trace:BUG-1594 | ai:claude
        println!(
            "\n{}: {}",
            "Git linkage".green().bold(),
            "no referencing commits; trace-comment scan incomplete (time budget) — \
             traced files unknown"
                .yellow()
        );
        return;
    }
    if commits.is_empty() && files.is_empty() {
        // TASK-726: the link is AIDA's whole magic — it's what makes "recall why"
        // work — but a newcomer has no idea how to create one. When there's no
        // linkage yet, say exactly how, using their real spec id. Found by
        // running a novice's first session: `aida show` reported "no linkage"
        // and left them at a dead end. trace:TASK-726 | ai:claude
        let example_id = ids.first().map(|s| s.as_str()).unwrap_or("TASK-1");
        println!(
            "\n{}: {}",
            "Git linkage".green().bold(),
            "no commits or trace comments reference this spec yet".dimmed()
        );
        println!(
            "  {} Link your code to it: add a {} comment where you implement it, \
             or name {} in a commit message — then this fills in on its own.",
            crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
            format!("// trace:{example_id}").cyan(),
            format!("({example_id})").cyan(),
        );
        return;
    }

    println!("\n{}:", "Git linkage".green().bold());

    // ---- Branch / worktree / PR-or-MR (anchored on the newest commit) ----
    // STORY-511: resolve the forge once so the change-request line uses the
    // right noun ("PR"/"MR") and CLI ("gh"/"glab") — a GitLab repo now reads
    // "MR-47" + "glab not installed", not the GitHub-only wording. The
    // (label, value) text is produced by the pure `format_change_linkage`;
    // this block only adds color + the worktree/branch decoration that
    // depends on live data. trace:STORY-511 | ai:claude
    if !commits.is_empty() {
        let forge = crate::forge::resolve_forge_kind(project_root);
        // Helper: print a formatted change-linkage line list with styling.
        let render = |lines: Vec<(String, String)>, found_url: Option<&str>| {
            for (label, value) in lines {
                if label == "Branch" {
                    // "merged to main" is the only Branch line this path
                    // produces (in-flight Branch is rendered below with
                    // worktree decoration).
                    println!("  {}     {}", "Branch".bold(), value.green());
                } else if let Some(url) = found_url {
                    // An open change: split the trailing url so it can be dimmed.
                    let id_part = value.strip_suffix(url).map(|s| s.trim_end());
                    match id_part {
                        Some(id) => {
                            println!("  {}         {} {}", label.bold(), id.cyan(), url.dimmed())
                        }
                        None => println!("  {}         {}", label.bold(), value.cyan()),
                    }
                } else {
                    // Shipped change number, or a dimmed "unknown" diagnostic.
                    let is_diag = value.contains("unknown")
                        || value.starts_with("no ")
                        || value.contains("not installed");
                    if is_diag {
                        println!("  {}         {}", label.bold(), value.dimmed());
                    } else {
                        println!("  {}         {}", label.bold(), value.cyan());
                    }
                }
            }
        };

        if shipped_unknown {
            // trace:BUG-1993 | ai:claude
            println!(
                "  {}     {}",
                "Branch".bold(),
                LINKAGE_SHIPPED_UNKNOWN_NOTE.yellow()
            );
        } else if shipped {
            let mut lines =
                format_change_linkage(forge, &ChangeLinkageState::Shipped { number: shipped_pr });
            // STORY-634 (ADR-12 D5): in a multi-repo workspace a bare change
            // number / "merged to main" is ambiguous — qualify both with the
            // scanned repo's slug ("api#141"). Single-repo (`repo` = None)
            // renders exactly as before.
            if let Some(slug) = repo.as_deref() {
                for (label, value) in lines.iter_mut() {
                    if label == "Branch" {
                        *value = format!("{value} ({slug})");
                    } else if let Some(num) = shipped_pr {
                        *value = format!("{slug}#{num}");
                    }
                }
            }
            render(lines, None);
        } else {
            match branch.as_deref() {
                Some(b) => {
                    let wt = worktree
                        .map(|p| format!(" (worktree: {})", p))
                        .unwrap_or_default();
                    // STORY-634: name the repo the branch lives in when the
                    // store spans a multi-repo workspace; empty otherwise.
                    let repo_note = repo
                        .as_deref()
                        .map(|slug| format!(" · repo: {slug}"))
                        .unwrap_or_default();
                    println!(
                        "  {}     {}{}{} · {}",
                        "Branch".bold(),
                        b.cyan(),
                        wt,
                        repo_note,
                        "in flight".yellow()
                    );
                    // STORY-516: forge-routed lookup → STORY-511 forge-aware
                    // rendering. trace:STORY-516 trace:STORY-511 | ai:claude
                    // BUG-1594: every lookup is capped per call and the whole
                    // set shares one budget; a lookup the budget no longer
                    // covers is rendered as skipped, never as "no PR".
                    // trace:BUG-1594 | ai:claude
                    let forge_started = std::time::Instant::now();
                    let bounded_lookup = |branch_name: &str| {
                        if forge_started.elapsed() >= SHOW_FORGE_LOOKUP_BUDGET {
                            return (ChangeLinkageState::LookupSkipped, None);
                        }
                        change_linkage_state_for(with_forge_lookup_timeout(
                            FORGE_CLI_CALL_TIMEOUT,
                            || change_lookup_for_branch(project_root, branch_name),
                        ))
                    };
                    let (state, url) = bounded_lookup(b);
                    render(format_change_linkage(forge, &state), url.as_deref());
                    // BUG-1528 (AC3/AC4): more than one branch references
                    // this spec — a branch-crossing. Say so, with each
                    // sibling's own PR/MR state, rather than silently
                    // picking `b` above and leaving the rest invisible.
                    // trace:BUG-1528 | ai:claude
                    for other in &other_branches {
                        let (ostate, ourl) = bounded_lookup(other);
                        println!(
                            "  {}     {} {}",
                            "Branch".bold(),
                            other.cyan(),
                            "also references this spec".yellow()
                        );
                        render(format_change_linkage(forge, &ostate), ourl.as_deref());
                    }
                }
                None => render(
                    format_change_linkage(forge, &ChangeLinkageState::BranchNotFound),
                    None,
                ),
            }
            // BUG-1594 (PRIN-5): the branch search stopped early — say so,
            // so the branch lines above are never read as complete.
            // trace:BUG-1594 | ai:claude
            if let Some((scanned, total)) = branch_scan_truncated {
                println!(
                    "  {}",
                    format_branch_scan_truncated_note(scanned, total).yellow()
                );
            }
        }
    }

    // ---- Commits list ----
    if !commits.is_empty() {
        let cap = if verbose {
            commits.len()
        } else {
            5.min(commits.len())
        };
        println!(
            "  {} ({}{})",
            "Commits".bold(),
            commits.len(),
            if !verbose && commits.len() > cap {
                format!(", showing {}", cap)
            } else {
                String::new()
            }
        );
        for (full, short, subject) in commits.iter().take(cap) {
            println!("    {} {}", short.yellow(), subject);
            if verbose {
                if let Some(stat) =
                    git(&["show", "--shortstat", "--format=", full]).filter(|s| !s.is_empty())
                {
                    if let Some(last) = stat.lines().last() {
                        println!("      {}", last.trim().dimmed());
                    }
                }
            }
        }
        if !verbose && commits.len() > cap {
            println!(
                "    {}",
                format!("… {} more (--verbose to expand)", commits.len() - cap).dimmed()
            );
        }
    }

    // ---- Files traced ----
    if !files.is_empty() {
        println!("  {} ({})", "Files traced".bold(), files.len());
        for (file, symbol) in &files {
            match symbol {
                Some(sym) => println!("    {} — {}", file, sym.dimmed()),
                None => println!("    {}", file),
            }
        }
    }
    // BUG-1594 (PRIN-5): a partial trace walk is labeled, never passed off
    // as the complete file list. trace:BUG-1594 | ai:claude
    if files_scan_incomplete {
        println!("  {}", LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE.yellow());
    }
    // BUG-1993: traced files rendered, but the commit scan timed out.
    // trace:BUG-1993 | ai:claude
    if commits_scan_incomplete {
        println!("  {}", LINKAGE_COMMIT_SCAN_INCOMPLETE_NOTE.yellow());
    }
    // TASK-1475 (CR-8 acceptance 6 follow-up): a one-line filing-drift
    // signal — how much of the traced surface has moved since the code
    // state this spec was filed against. Silent when there's nothing to
    // say (no provenance, unresolvable sha, no traced files, or no drift).
    // trace:TASK-1475 | ai:claude
    if let Some(hint) = filing_drift_hint(project_root, filed_at, &files, &commits) {
        println!("  {}", hint.dimmed());
    }
}

/// BUG-1594: the note `aida show` prints when the trace-comment walk stopped
/// at its time budget.
// trace:BUG-1594 | ai:claude
pub(crate) const LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE: &str =
    "trace-comment scan incomplete (time budget) — more files may reference this spec";

/// BUG-1993: the note rendered when the referencing-commit scan stopped at
/// its time budget.
// trace:BUG-1993 | ai:claude
pub(crate) const LINKAGE_COMMIT_SCAN_INCOMPLETE_NOTE: &str =
    "commit scan incomplete (time budget) — referencing commits and shipped state unknown";

/// BUG-1993: the note rendered when the shipped check stopped at its time
/// budget.
// trace:BUG-1993 | ai:claude
pub(crate) const LINKAGE_SHIPPED_UNKNOWN_NOTE: &str =
    "shipped state unknown (merge check did not finish in time budget)";

/// Why a trailer reference fails the validity gate. trace:STORY-498 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TrailerVerdict {
    /// The id does not resolve to any spec in the requirement graph (a
    /// hallucinated or typo'd id, or a since-deleted spec).
    Nonexistent,
    /// The id resolves but the spec is rejected — a dead provenance link.
    Rejected,
}

impl TrailerVerdict {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            TrailerVerdict::Nonexistent => "does not exist in the requirement graph",
            TrailerVerdict::Rejected => "resolves to a rejected spec",
        }
    }
}

/// One offending `(SPEC-ID)` reference found by the gate. trace:STORY-498 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TrailerViolation {
    /// Short SHA of the offending commit.
    pub(crate) sha: String,
    /// The commit subject (for the human-readable failure message).
    pub(crate) subject: String,
    /// The id that failed to resolve / resolved to a dead spec.
    pub(crate) spec_id: String,
    pub(crate) verdict: TrailerVerdict,
}

/// How a SPEC-ID resolves against the live requirement graph — the result the
/// pure validation core consumes so it stays independent of the store/git.
/// trace:STORY-498 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecResolution {
    /// Resolves to a live (non-rejected) spec — passes.
    Live,
    /// Resolves to a rejected spec — a dead reference.
    Rejected,
    /// Resolves to nothing.
    Missing,
}

/// Pure, testable core of the validity gate: given commits as
/// `(short_sha, subject)` pairs and a resolver that classifies each SPEC-ID
/// against the graph, return every offending reference.
///
/// Exemptions baked in here (matching the auto-bump scan + TASK-488):
/// - Plan commits (`docs(plans): …`) are skipped — their trailer names what is
///   planned, not what shipped, so a planned-but-not-yet-real id is legitimate.
/// - A commit with no `(SPEC-ID)` trailer is skipped — there is nothing to
///   validate (mechanical / release / docs commits with "no REQ-ID needed").
///
/// trace:STORY-498 | ai:claude
pub(crate) fn validate_trailer_references<F>(
    commits: &[(String, String)],
    mut resolve: F,
) -> Vec<TrailerViolation>
where
    F: FnMut(&str) -> SpecResolution,
{
    let mut violations = Vec::new();
    for (sha, subject) in commits {
        if is_plan_commit_subject(subject) {
            continue;
        }
        for id in extract_spec_ids_from_commit(subject) {
            let verdict = match resolve(&id) {
                SpecResolution::Live => continue,
                SpecResolution::Rejected => TrailerVerdict::Rejected,
                SpecResolution::Missing => TrailerVerdict::Nonexistent,
            };
            violations.push(TrailerViolation {
                sha: sha.clone(),
                subject: subject.clone(),
                spec_id: id,
                verdict,
            });
        }
    }
    violations
}

/// Resolve the commit range to scan. Explicit `--range` wins; otherwise scan
/// the commits this branch adds over the default branch
/// (`<default-branch>..HEAD`), falling back to `HEAD~20..HEAD` when no default
/// branch resolves (e.g. a shallow CI checkout or a repo with no main).
/// trace:STORY-498 | ai:claude
pub(crate) fn resolve_gate_range(project_root: &std::path::Path, range: Option<&str>) -> String {
    if let Some(r) = range {
        return r.to_string();
    }
    if let Some(default_ref) = resolve_default_branch_ref(project_root) {
        // `<base>..HEAD`: the commits HEAD has that the default branch doesn't.
        return format!("{default_ref}..HEAD");
    }
    "HEAD~20..HEAD".to_string()
}

/// TASK-1444: resolve the commit range for reviewer-verdict shelve
/// attribution against the **PR's own branch**, not the drain's main
/// checkout `HEAD`. `resolve_gate_range(.., None)` scans
/// `<default>..HEAD`, which is the drain's own worktree/checkout — for the
/// orchestrator's phase driver that is NOT necessarily the branch the PR
/// under review is on. Prefers `origin/<branch>` (what CI and the reviewer
/// actually saw) and falls back to the local `<branch>` ref when the origin
/// ref hasn't been fetched; returns `None` when neither resolves (or the
/// default branch itself can't be resolved) so the caller can treat
/// attribution as `Uncertain` instead of silently reading the wrong range.
// trace:TASK-1444 | ai:claude
pub(crate) fn resolve_shelve_gate_range(
    project_root: &std::path::Path,
    branch: Option<&str>,
) -> Option<String> {
    let branch = branch?;
    let default_ref = resolve_default_branch_ref(project_root)?;
    use std::process::Command as PCmd;
    let ref_exists = |r: &str| -> bool {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", "--verify", "--quiet", r])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let origin_branch = format!("origin/{branch}");
    let branch_ref = if ref_exists(&origin_branch) {
        origin_branch
    } else if ref_exists(branch) {
        branch.to_string()
    } else {
        return None;
    };
    Some(format!("{default_ref}..{branch_ref}"))
}

/// Resolve a single SPEC-ID against a loaded store, mirroring the trace-gate
/// resolver. When `store` is `None` (no requirement store reachable) every id
/// resolves `Live` — failing every id would block legitimate ships on a
/// checkout without store access, so the safe default is to surface the
/// missing-store condition separately (callers warn) rather than refuse.
/// Shared by `aida trace gate` (STORY-498) and the client-side ship/done guard.
/// trace:STORY-498 trace:STORY-469 | ai:claude
pub(crate) fn resolve_spec_in_store(
    store: Option<&aida_core::RequirementsStore>,
    id: &str,
) -> SpecResolution {
    let Some(store) = store else {
        return SpecResolution::Live;
    };
    let want = id.to_ascii_uppercase();
    let found = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(&want))
            .unwrap_or(false)
            || r.agreed_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
    });
    match found {
        None => SpecResolution::Missing,
        Some(r) if matches!(r.status, RequirementStatus::Rejected) => SpecResolution::Rejected,
        Some(_) => SpecResolution::Live,
    }
}

/// How a single inline `// trace:SPEC-ID` marker resolves. A superset of
/// `SpecResolution` that also splits out the archived-but-live case so the
/// report can call it cheaply (an archived target is a softer rot signal than a
/// deleted one — the spec still exists, just hidden). trace:TASK-868
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceRotVerdict {
    /// Resolves to a live, non-archived, non-rejected spec — healthy.
    Live,
    /// Resolves to an archived spec — exists but hidden; soft rot.
    Archived,
    /// Resolves to a rejected spec — a dead provenance link.
    Rejected,
    /// Resolves to nothing — deleted / renumbered / typo'd target; hard rot.
    Unknown,
}

impl TraceRotVerdict {
    /// Whether this verdict counts as HARD trace-rot — a genuinely dead
    /// provenance link (deleted/renumbered target or a rejected spec). This is
    /// what `--block` fails on. An archived target is NOT hard rot: the spec
    /// still exists in the graph, it's just hidden from default views, so its
    /// trace still resolves — it's reported as a soft signal, not a failure.
    pub(crate) fn is_rot(self) -> bool {
        matches!(self, TraceRotVerdict::Rejected | TraceRotVerdict::Unknown)
    }

    /// Whether this verdict is worth surfacing at all (anything but healthy).
    pub(crate) fn is_flagged(self) -> bool {
        !matches!(self, TraceRotVerdict::Live)
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            TraceRotVerdict::Live => "resolves to a live spec",
            TraceRotVerdict::Archived => "target is archived",
            TraceRotVerdict::Rejected => "target is rejected",
            TraceRotVerdict::Unknown => "target does not exist (deleted/renumbered)",
        }
    }
}

/// Resolve a SPEC-ID against the store for the rot check, distinguishing
/// archived from rejected from missing. When the store is unreachable, every id
/// resolves `Live` (callers warn separately) so a checkout without store access
/// doesn't flag the whole tree. trace:TASK-868
pub(crate) fn resolve_trace_rot(
    store: Option<&aida_core::RequirementsStore>,
    id: &str,
) -> TraceRotVerdict {
    let Some(store) = store else {
        return TraceRotVerdict::Live;
    };
    let want = id.to_ascii_uppercase();
    let found = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(&want))
            .unwrap_or(false)
            || r.agreed_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
    });
    match found {
        None => TraceRotVerdict::Unknown,
        Some(r) if matches!(r.status, RequirementStatus::Rejected) => TraceRotVerdict::Rejected,
        Some(r) if r.archived => TraceRotVerdict::Archived,
        Some(_) => TraceRotVerdict::Live,
    }
}

/// One dangling (rotted) inline trace marker. trace:TASK-868
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TraceRot {
    /// The referenced SPEC-ID that rotted.
    pub(crate) spec_id: String,
    /// Project-relative file path.
    pub(crate) file: String,
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) verdict: TraceRotVerdict,
}

/// CLI handler for `aida trace check`. Scans inline `// trace:SPEC-ID` markers,
/// resolves each against the live graph, reports the rot, and exits non-zero
/// (code 1) under `--block` when any dangling trace exists. trace:TASK-868
pub(crate) fn handle_trace_check(path: Option<&str>, json: bool, block: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let scan_root = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => project_root.clone(),
    };

    let store = load_store_for_lookup(&project_root);
    if store.is_none() && !json {
        eprintln!(
            "{} trace check: no requirement store reachable from {} — cannot resolve trace \
             targets, so nothing can be flagged as rotted. Run where the store is attached \
             (`aida cache rebuild` / fresh-clone auto-attach).",
            crate::glyph(crate::glyphs::Glyph::Warning),
            project_root.display()
        );
    }

    // Same marker grammar + scanner the doctor trace check uses.
    let trace_re = regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();
    walk_source_for_traces(&scan_root, &trace_re, &mut by_spec);

    // Resolve each unique id once; expand to per-location rows for the flagged ones.
    let mut total_markers = 0usize;
    let mut unique_ids = 0usize;
    let mut resolved_markers = 0usize;
    let mut flagged: Vec<TraceRot> = Vec::new();
    let mut ids: Vec<&String> = by_spec.keys().collect();
    ids.sort();
    for id in ids {
        unique_ids += 1;
        let locations = &by_spec[id];
        total_markers += locations.len();
        let verdict = resolve_trace_rot(store.as_ref(), id);
        if verdict.is_flagged() {
            for (p, line) in locations {
                let rel = p.strip_prefix(&project_root).unwrap_or(p);
                flagged.push(TraceRot {
                    spec_id: id.clone(),
                    file: rel.display().to_string(),
                    line: *line,
                    verdict,
                });
            }
        } else {
            resolved_markers += locations.len();
        }
    }
    flagged.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));

    // Hard rot (dead links) is what `--block` fails on; archived is a soft signal.
    let hard: Vec<&TraceRot> = flagged.iter().filter(|r| r.verdict.is_rot()).collect();
    let archived = flagged.len() - hard.len();
    let dangling = hard.len();
    let unknown = hard
        .iter()
        .filter(|r| matches!(r.verdict, TraceRotVerdict::Unknown))
        .count();
    let rejected = dangling - unknown;
    let rot_rate = if total_markers == 0 {
        0.0
    } else {
        (dangling as f64) * 100.0 / (total_markers as f64)
    };

    if json {
        let payload = serde_json::json!({
            "scan_root": scan_root.display().to_string(),
            "store_reachable": store.is_some(),
            "total_traces": total_markers,
            "unique_spec_ids": unique_ids,
            "resolved": resolved_markers,
            "dangling": dangling,
            "dangling_unknown": unknown,
            "dangling_rejected": rejected,
            "archived": archived,
            "rot_rate_pct": (rot_rate * 100.0).round() / 100.0,
            "flagged": flagged,
            "ok": dangling == 0,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if dangling == 0 && archived == 0 {
        println!(
            "{} trace check: {} inline trace(s) across {} spec-id(s) — every marker resolves to a \
             live spec (0 rotted)",
            crate::glyph(crate::glyphs::Glyph::Check),
            total_markers,
            unique_ids
        );
    } else {
        if dangling > 0 {
            eprintln!(
                "{} trace check: {} dangling/rotted trace marker(s) ({:.1}% rot rate — {} unknown, \
                 {} rejected; {} of {} resolve):",
                crate::glyph(crate::glyphs::Glyph::Cross),
                dangling,
                rot_rate,
                unknown,
                rejected,
                resolved_markers,
                total_markers
            );
            for r in &hard {
                eprintln!(
                    "  {}:{} → {} ({})",
                    r.file,
                    r.line,
                    r.spec_id,
                    r.verdict.reason()
                );
            }
        } else {
            println!(
                "{} trace check: 0 dead trace links ({} of {} resolve)",
                crate::glyph(crate::glyphs::Glyph::Check),
                resolved_markers,
                total_markers
            );
        }
        if archived > 0 {
            println!(
                "\n{} {} trace(s) point at archived specs (still resolve; soft signal, not \
                 blocking). Re-target or accept.",
                crate::glyph(crate::glyphs::Glyph::Warning),
                archived
            );
        }
        if dangling > 0 {
            eprintln!(
                "\nA `// trace:SPEC-ID` marker must name a live requirement. Update the marker to \
                 the spec's current id (after a merge-gate renumber the id changes), point it at a \
                 real spec, or delete the stale comment. `aida doctor validate-trace-comments \
                 --strip-dangling` can sweep the unknown ones in bulk."
            );
        }
    }

    if block && dangling > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// CLI handler for `aida trace gate`. Reads the commit range from git, runs the
/// pure validator against the live store, prints the result, and exits non-zero
/// (code 1) when any commit references a dead/dangling SPEC-ID.
/// trace:STORY-498 | ai:claude
/// Read `(short_sha, subject)` rows for a git range. A unit separator keeps
/// subjects with arbitrary punctuation intact. Returns an error only when git
/// itself fails (bad range, no git). Shared by the trace gate and the
/// client-side ship/done guard. trace:STORY-498 trace:STORY-469 | ai:claude
pub(crate) fn read_commits_in_range(
    project_root: &std::path::Path,
    range: &str,
) -> Result<Vec<(String, String)>> {
    use std::process::Command as PCmd;
    let output = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--no-merges",
            "--pretty=format:%h\x1f%s",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|line| {
                let (sha, subject) = line.split_once('\x1f')?;
                let subject = subject.trim();
                if subject.is_empty() {
                    return None;
                }
                Some((sha.to_string(), subject.to_string()))
            })
            .collect()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!("git log failed for range `{range}`: {}", err.trim());
        }
        Err(e) => anyhow::bail!("could not run git: {e}"),
    }
}

/// Format the human-readable refusal lines for client-side trailer violations.
/// Pure (takes pre-computed violations) so it's unit-testable in isolation.
/// `surface` names the command for the message ("pr ship" / "queue done").
/// trace:STORY-469 | ai:claude
pub(crate) fn format_trailer_guard_refusal(
    surface: &str,
    violations: &[TrailerViolation],
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {surface}: {} commit trailer reference(s) name a spec that does not resolve:",
        crate::glyph(crate::glyphs::Glyph::Cross),
        violations.len()
    ));
    for v in violations {
        lines.push(format!(
            "  {} ({}) {} — {}",
            v.sha,
            v.spec_id,
            v.verdict.reason(),
            v.subject
        ));
    }
    lines.push(String::new());
    lines.push(
        "A commit's `(SPEC-ID)` trailer must name a live requirement — typo? hallucination? \
         since-rejected? Check `aida show <SPEC-ID>` and fix the trailer (`git commit --amend` / \
         interactive rebase) before retrying, or re-run with --force to bypass."
            .to_string(),
    );
    lines
}

/// Guard 1: validate that every `(SPEC-ID)` trailer on the commits this branch
/// adds over the default branch resolves to a live (non-rejected) spec. Refuses
/// (exits 1) on any dead/dangling reference unless `force` is set. A no-op when
/// the branch is the default branch (nothing to ship) or no store is reachable
/// (cannot corroborate — surfaced as a soft warning, never a hard refusal, so a
/// store-less checkout can still ship). `surface` names the calling command.
/// trace:STORY-469 | ai:claude
pub(crate) fn run_client_trailer_guard(project_root: &std::path::Path, surface: &str, force: bool) {
    if force {
        return;
    }
    let range = resolve_gate_range(project_root, None);
    let commits = match read_commits_in_range(project_root, &range) {
        Ok(c) => c,
        Err(e) => {
            // Git read failed (e.g. shallow checkout, unresolved range). The
            // guard cannot corroborate; surface softly and proceed — failing
            // the ship/done on a git hiccup would be worse than the miss.
            eprintln!(
                "{} {surface}: trailer spec-ID check skipped — {}",
                "warning:".yellow().bold(),
                e
            );
            return;
        }
    };
    if commits.is_empty() {
        return;
    }
    let store = load_store_for_lookup(project_root);
    if store.is_none() {
        eprintln!(
            "{} {surface}: trailer spec-ID check skipped — no requirement store reachable. \
             Run where the store is attached (`aida cache rebuild`) to enforce.",
            "warning:".yellow().bold()
        );
        return;
    }
    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let violations = validate_trailer_references(&commits, resolve);
    if violations.is_empty() {
        return;
    }
    for line in format_trailer_guard_refusal(surface, &violations) {
        eprintln!("{line}");
    }
    std::process::exit(1);
}

/// Pure, testable core: given commits as `(sha, subject)` pairs and the spec
/// the branch is leased for, return `None` when some commit's `(SPEC-ID)`
/// trailer names `expected_spec` (attribution OK), or `Some(other_ids)` — the
/// distinct spec ids the trailers DO name — when none does (refuse). Reuses
/// the same trailer extractor + plan-commit exemption as Guard 1
/// (`validate_trailer_references`) so the two guards agree on what a
/// "trailer" is.
// trace:TASK-1442 | ai:claude
pub(crate) fn pr_open_spec_guard_violation(
    commits: &[(String, String)],
    expected_spec: &str,
) -> Option<Vec<String>> {
    let mut other_ids: Vec<String> = Vec::new();
    for (_, subject) in commits {
        if is_plan_commit_subject(subject) {
            continue;
        }
        for id in extract_spec_ids_from_commit(subject) {
            if id.eq_ignore_ascii_case(expected_spec) {
                return None; // found a matching trailer — attributed correctly
            }
            if !other_ids
                .iter()
                .any(|s: &String| s.eq_ignore_ascii_case(&id))
            {
                other_ids.push(id);
            }
        }
    }
    Some(other_ids)
}

/// Which spec a reviewer-verdict shelve should be recorded against.
// trace:TASK-1444 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShelveAttribution {
    /// A commit trailer confirms the verdict is about the lease's own spec.
    Confirmed(String),
    /// Every non-plan commit trailer names exactly one spec, and it is NOT
    /// the lease's — the shelve should target THAT spec, not the lease.
    Reattributed(String),
    /// The commits don't confirm a single spec (none named, or more than
    /// one) — the attribution can't be safely resolved either way.
    Uncertain(String),
}

/// Pure, testable core: decide which spec a reviewer-verdict shelve is
/// actually about, from what the PR's own commits credit — never silently
/// `lease_spec`. Reuses `pr_open_spec_guard_violation`'s trailer extraction
/// (TASK-1442) so the two guards agree on what a trailer is: `None` there
/// means some commit trailers `lease_spec` itself (`Confirmed`); `Some(ids)`
/// means none does, and `ids` is what the non-plan commits DO name — exactly
/// one other id means the verdict is confidently about that spec instead
/// (`Reattributed`), while zero or several distinct ids means the commits
/// don't settle it (`Uncertain`).
// trace:TASK-1444 | ai:claude
pub(crate) fn decide_shelve_attribution(
    commits: &[(String, String)],
    lease_spec: &str,
) -> ShelveAttribution {
    match pr_open_spec_guard_violation(commits, lease_spec) {
        None => ShelveAttribution::Confirmed(lease_spec.to_string()),
        Some(other_ids) => match other_ids.as_slice() {
            [only] => ShelveAttribution::Reattributed(only.clone()),
            [] => ShelveAttribution::Uncertain(
                "no commit on this PR carries a spec-ID trailer".to_string(),
            ),
            many => ShelveAttribution::Uncertain(format!(
                "commits name multiple specs: {}",
                many.join(", ")
            )),
        },
    }
}

/// Pure, testable core: given the spec a PR's lease attributes it to and
/// that PR's own commits, decide whether the two attribution sources agree.
/// Reuses `decide_shelve_attribution` (TASK-1444) so this and the
/// reviewer-verdict shelve guard agree on what "trailer evidence" means:
/// `Confirmed` (a trailer names the lease spec) and `Uncertain` (the
/// trailers don't settle it — zero or several distinct ids) are both
/// non-disagreements; only a confident `Reattributed` to a DIFFERENT spec is
/// a disagreement worth surfacing. Never silently prefers one source over
/// the other — both claimed owners are returned.
// trace:TASK-1445 | ai:claude
pub(crate) fn pr_attribution_disagreement(
    pr: u64,
    commits: &[(String, String)],
    lease_spec: &str,
) -> Option<awaiting_you::PrAttributionDisagreementItem> {
    match decide_shelve_attribution(commits, lease_spec) {
        ShelveAttribution::Reattributed(trailer_spec) => {
            Some(awaiting_you::PrAttributionDisagreementItem {
                pr,
                lease_spec: lease_spec.to_string(),
                trailer_spec,
            })
        }
        ShelveAttribution::Confirmed(_) | ShelveAttribution::Uncertain(_) => None,
    }
}

/// Collect every attribution split for the currently live drain's in-flight
/// members. Local-only: a `.aida/drain-state.json` read, the local lease
/// list, and one `git log` per in-flight member with a recorded PR — no
/// network call. Returns nothing when no drain is active, a member has no
/// PR yet, or its lease's branch can't be resolved/read (fails open — a git
/// hiccup here must not manufacture a false disagreement).
// trace:TASK-1445 | ai:claude
pub(crate) fn collect_pr_attribution_disagreements(
    project_root: &std::path::Path,
) -> Vec<awaiting_you::PrAttributionDisagreementItem> {
    let state = match drain_state::probe(project_root) {
        drain_state::DrainStatus::Active(state) => state,
        drain_state::DrainStatus::None
        | drain_state::DrainStatus::Stale(_)
        | drain_state::DrainStatus::Stopped(_) => return Vec::new(),
    };
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };
    let leases = list_leases(project_root);
    let mut out = Vec::new();
    for member in state.members.iter().filter(|m| m.is_running()) {
        let Some(pr) = member.pr else { continue };
        let Some(lease) = leases
            .iter()
            .find(|l| l.scope.eq_ignore_ascii_case(&member.spec))
        else {
            continue;
        };
        let range = format!("{default_ref}..{}", lease.branch);
        let Ok(commits) = read_commits_in_range(project_root, &range) else {
            continue;
        };
        if let Some(disagreement) = pr_attribution_disagreement(pr as u64, &commits, &member.spec) {
            out.push(disagreement);
        }
    }
    out
}

/// Refuse (exit 1) to open a PR when no commit the branch adds over the
/// default branch carries a `(SPEC-ID)` trailer naming `expected_spec`. A
/// no-op when the commit range can't be read (soft warning — a git hiccup
/// shouldn't block shipping) or when the branch has no commits to ship.
// trace:TASK-1442 | ai:claude
pub(crate) fn run_pr_open_spec_guard(
    project_root: &std::path::Path,
    branch: &str,
    expected_spec: &str,
) {
    let range = resolve_gate_range(project_root, None);
    let commits = match read_commits_in_range(project_root, &range) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{} pr ship: PR-spec attribution check skipped — {}",
                "warning:".yellow().bold(),
                e
            );
            return;
        }
    };
    if commits.is_empty() {
        return;
    }

    let other_ids = match pr_open_spec_guard_violation(&commits, expected_spec) {
        None => return,
        Some(ids) => ids,
    };

    eprintln!(
        "{} pr ship: refusing to open a PR — branch `{}` is leased for {} but no commit on \
         it carries a `({})` trailer.",
        crate::glyph(crate::glyphs::Glyph::Cross),
        branch,
        expected_spec,
        expected_spec
    );
    if other_ids.is_empty() {
        eprintln!("  no commit on this branch carries a (SPEC-ID) trailer at all.");
    } else {
        eprintln!(
            "  commit trailer(s) instead name: {} — a mismatch against the leased spec {}.",
            other_ids.join(", "),
            expected_spec
        );
    }
    eprintln!(
        "  Fix the trailer(s) (`git commit --amend` / interactive rebase) to reference {}, or \
         end this lease and open the PR from the branch that actually owns {}.",
        expected_spec, expected_spec
    );
    std::process::exit(1);
}

/// TASK-1457 (BUG-1527 follow-up): the same three-way classification
/// `decide_shelve_attribution` (TASK-1444) gives a reviewer-verdict shelve,
/// but for the branch-swap seam `RealPhaseDriver::run_implementer` gates on
/// before it will accept a mid-phase branch change. `ensure_pr_open_spec_attribution`
/// collapses "a trailer names a different spec" and "no commit carries any
/// spec-ID trailer" into the same `Err` — which is why a same-spec rename
/// (BUG-223) whose commits simply have not been trailered yet used to read
/// as a confidently-worded "swap". PRIN-5 requires these stay distinct
/// outcomes: `Reattributed` is a spec-ID trailer actively pointing somewhere
/// else (the confident swap case); `Uncertain` is absent evidence, not
/// contrary evidence, and must never be reported the same way. Fails open
/// (`Confirmed`) exactly where `ensure_pr_open_spec_attribution` does — an
/// unresolvable default branch, an unreadable range, or no commits at all —
/// so a git hiccup here can't manufacture a false swap report either.
// trace:BUG-1527 trace:TASK-1457 | ai:claude
pub(crate) fn classify_branch_swap_attribution(
    repo: &std::path::Path,
    branch_ref: &str,
    expected_spec: &str,
) -> ShelveAttribution {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return ShelveAttribution::Confirmed(expected_spec.to_string());
    };
    let range = format!("{default_ref}..{branch_ref}");
    let commits = match read_commits_in_range(repo, &range) {
        Ok(c) => c,
        Err(_) => return ShelveAttribution::Confirmed(expected_spec.to_string()),
    };
    if commits.is_empty() {
        return ShelveAttribution::Confirmed(expected_spec.to_string());
    }
    decide_shelve_attribution(&commits, expected_spec)
}

/// TASK-1442 follow-up: the same PR-spec attribution check as
/// `run_pr_open_spec_guard`, but as a `Result` instead of an exiting side
/// effect. `run_pr_open_spec_guard` is only safe at `aida pr ship`'s own
/// top level; the autonomous drain's PR-open recovery paths
/// (`open_orchestrator_pr_for_implementer_worktree`,
/// `open_orchestrator_pr_for_pushed_branch`) run INSIDE the orchestrator
/// process, where `std::process::exit` would kill the whole drain instead of
/// failing just the one phase. `repo` is the directory to run git in (the
/// implementer worktree, or the main project root for a pushed branch);
/// `branch_ref` is whatever ref names the branch's commits from there (a
/// local branch name or `origin/<branch>`). A no-op when the default branch
/// or commit range can't be resolved (soft — a git hiccup shouldn't block
/// recovery) or when there are no commits to check.
///
/// TASK-1457: this collapses `Reattributed` and `Uncertain` (see
/// `classify_branch_swap_attribution` above) into the same `Err` — a
/// deliberate, kept decision for THIS gate. `ensure_spec_done_after_pr`
/// (its only status-writing caller) must not flip a spec to Done on
/// EITHER absent or contrary trailer evidence; per PRIN-5, "cannot
/// determine" is not license to proceed on a Done write any more than
/// "determined otherwise" is. The two PR-open recovery callers
/// (`open_orchestrator_pr_for_implementer_worktree`,
/// `open_orchestrator_pr_for_pushed_branch`) inherit the same fail-safe
/// for the same reason: opening a PR under an unconfirmed identity is a
/// second write worth refusing on absent evidence too. Only the
/// branch-swap seam needs the finer three-way split, because only it has
/// to tell an operator whether to look for a swap or a missing trailer.
// trace:TASK-1442 trace:TASK-1457 | ai:claude
pub(crate) fn ensure_pr_open_spec_attribution(
    repo: &std::path::Path,
    branch_ref: &str,
    expected_spec: &str,
) -> Result<()> {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return Ok(());
    };
    let range = format!("{default_ref}..{branch_ref}");
    let commits = match read_commits_in_range(repo, &range) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    if commits.is_empty() {
        return Ok(());
    }
    if let Some(other_ids) = pr_open_spec_guard_violation(&commits, expected_spec) {
        let named = if other_ids.is_empty() {
            "no commit carries a (SPEC-ID) trailer at all".to_string()
        } else {
            format!("commit trailer(s) instead name: {}", other_ids.join(", "))
        };
        anyhow::bail!(
            "refusing to open a PR for {expected_spec} on `{branch_ref}` — no commit on it \
             carries a `({expected_spec})` trailer; {named}"
        );
    }
    Ok(())
}

/// CLI handler for `aida trace gate`. Reads the commit range from git, runs the
/// pure validator against the live store, prints the result, and exits non-zero
/// (code 1) when any commit references a dead/dangling SPEC-ID.
/// trace:STORY-498 | ai:claude
pub(crate) fn handle_trace_gate(range: Option<&str>, json: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let commits = read_commits_in_range(&project_root, &range)?;

    // Load the live requirement graph once, then resolve each id against it.
    let store = load_store_for_trace_gate(&project_root)?;
    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(Some(&store), id) };

    let violations = validate_trailer_references(&commits, resolve);

    if json {
        let payload = serde_json::json!({
            "range": range,
            "commits_scanned": commits.len(),
            "violations": violations,
            "ok": violations.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if violations.is_empty() {
        println!(
            "{} trace gate: {} commit(s) in `{}` — every (SPEC-ID) trailer resolves to a live spec",
            crate::glyph(crate::glyphs::Glyph::Check),
            commits.len(),
            range
        );
    } else {
        eprintln!(
            "{} trace gate: {} dead/dangling SPEC-ID reference(s) in `{}`:",
            crate::glyph(crate::glyphs::Glyph::Cross),
            violations.len(),
            range
        );
        for v in &violations {
            eprintln!(
                "  {} ({}) {} — {}",
                v.sha,
                v.spec_id,
                v.verdict.reason(),
                v.subject
            );
        }
        eprintln!(
            "\nA commit's `(SPEC-ID)` trailer must name a live requirement. Fix the trailer to a \
             real, non-rejected id (or file the missing spec), then amend/re-commit."
        );
    }

    if !violations.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn load_store_for_trace_gate(
    project_root: &std::path::Path,
) -> Result<aida_core::RequirementsStore> {
    // trace:TASK-1206 | ai:codex
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(store) = aida_core::DatabaseBackend::load(&backend) {
                return Ok(store);
            }
        }
    }

    for legacy_name in ["requirements.db", "requirements.yaml"] {
        let legacy_path = project_root.join(legacy_name);
        if legacy_path.exists() {
            return Storage::new(legacy_path).load();
        }
    }

    anyhow::bail!(
        "{} trace gate: no requirement store reachable from {} — cannot validate references. \
         Ensure the gate runs where the store is attached (`aida cache rebuild` / fresh-clone \
         auto-attach).",
        crate::glyph(crate::glyphs::Glyph::Cross),
        project_root.display()
    )
}

/// Why a coverable hunk is exempt from the coverage denominator. Every
/// exemption is reported (never silently dropped) per SPIKE-47 §3.
/// trace:STORY-499 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageExemption {
    /// File path matched a test glob (SPIKE-47 F1).
    TestFile,
    /// File path / generated-marker matched (SPIKE-47 F2).
    Generated,
    /// Non-source / docs / prose extension (SPIKE-47 F3).
    DocOrProse,
    /// Config / data / lockfile (SPIKE-47 F4).
    ConfigData,
    /// Vendored / third-party path (SPIKE-47 F5).
    Vendored,
    /// Hunk with only deletions (SPIKE-47 H1).
    PureDeletion,
    /// Hunk whose added lines are only comments / blank (SPIKE-47 H2/H4).
    CommentOrBlankOnly,
    /// Hunk net change ≤ trivial threshold (SPIKE-47 H4).
    Trivial,
}

impl CoverageExemption {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            CoverageExemption::TestFile => "test file",
            CoverageExemption::Generated => "generated code",
            CoverageExemption::DocOrProse => "docs / non-source",
            CoverageExemption::ConfigData => "config / data / lockfile",
            CoverageExemption::Vendored => "vendored / third-party",
            CoverageExemption::PureDeletion => "pure deletion",
            CoverageExemption::CommentOrBlankOnly => "comment / blank only",
            CoverageExemption::Trivial => "trivial change",
        }
    }
}

/// How a coverable hunk earned its coverage (SPIKE-47 §4.2). Cheapest-first.
/// trace:STORY-499 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageSource {
    /// A `// trace:<live-id>` appears in the hunk's added lines (§4.2.1).
    InHunkAnchor,
    /// A `// trace:<live-id>` sits within N lines above the hunk (§4.2.2).
    ProximityAnchor,
    /// A module-level `//! trace:<live-id>` covers the whole file (§4.2.3).
    FileAnchor,
    /// The hunk's commit carries a live `(SPEC-ID)` trailer (§4.2.4 — the floor).
    CommitTrailer,
}

/// The classification of one changed hunk. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HunkVerdict {
    /// File the hunk lives in (post-change path).
    pub(crate) file: String,
    /// 1-based start line of the hunk in the post-change file.
    pub(crate) new_start: usize,
    /// Number of added/modified lines in the hunk.
    pub(crate) added_lines: usize,
    /// `Some(reason)` when the hunk is exempt from the denominator.
    pub(crate) exempt: Option<CoverageExemption>,
    /// `Some(source)` when the hunk is covered. `None` + not-exempt = a
    /// reported uncovered coverable hunk.
    pub(crate) covered: Option<CoverageSource>,
}

impl HunkVerdict {
    /// A coverable hunk that is NOT covered — the thing the gate reports.
    pub(crate) fn is_uncovered_coverable(&self) -> bool {
        self.exempt.is_none() && self.covered.is_none()
    }
    pub(crate) fn is_coverable(&self) -> bool {
        self.exempt.is_none()
    }
}

/// One parsed changed hunk fed into the pure core. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedHunk {
    /// Post-change file path.
    pub(crate) file: String,
    /// 1-based start line of the hunk in the post-change file.
    pub(crate) new_start: usize,
    /// The `+` (added) line bodies, in order, with the leading `+` stripped.
    pub(crate) added: Vec<String>,
    /// The `-` (removed) line bodies, with the leading `-` stripped.
    pub(crate) removed: Vec<String>,
}

/// Aggregate coverage result returned by the pure core. trace:STORY-499 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct DiffCoverage {
    pub(crate) hunks: Vec<HunkVerdict>,
}

impl DiffCoverage {
    pub(crate) fn coverable(&self) -> usize {
        self.hunks.iter().filter(|h| h.is_coverable()).count()
    }
    pub(crate) fn covered(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| h.is_coverable() && h.covered.is_some())
            .count()
    }
    pub(crate) fn exempt(&self) -> usize {
        self.hunks.iter().filter(|h| h.exempt.is_some()).count()
    }
    pub(crate) fn uncovered(&self) -> Vec<&HunkVerdict> {
        self.hunks
            .iter()
            .filter(|h| h.is_uncovered_coverable())
            .collect()
    }
    /// `covered / coverable`. 1.0 when there is nothing coverable (vacuously
    /// fully covered — an all-exempt diff passes). trace:STORY-499 | ai:claude
    pub(crate) fn ratio(&self) -> f64 {
        let coverable = self.coverable();
        if coverable == 0 {
            return 1.0;
        }
        self.covered() as f64 / coverable as f64
    }
}

/// Default source extensions that REQUIRE coverage. Mirrors `aida trace scan
/// --extensions` default (`rs`); extensible per-repo (SPIKE-47 §2.1.2).
/// trace:STORY-499 | ai:claude
pub(crate) const COVERAGE_SOURCE_EXTENSIONS: &[&str] =
    &["rs", "py", "ts", "tsx", "js", "jsx", "go", "java"];

/// SPIKE-47 §3.2 file/path-level exemption classifier. Returns `Some(reason)`
/// when the whole file is exempt, `None` when its changed source hunks are
/// coverable. Deterministic — globs/extensions only, no parsing. PURE.
/// trace:STORY-499 | ai:claude
pub(crate) fn classify_coverage_file(
    path: &str,
    generated_header: Option<&str>,
) -> Option<CoverageExemption> {
    let lower = path.to_ascii_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();

    // F5: vendored / third-party.
    if segments
        .iter()
        .any(|s| matches!(*s, "vendor" | "node_modules" | "third_party" | "target"))
    {
        return Some(CoverageExemption::Vendored);
    }

    // F2: generated — path globs OR an in-file generated marker.
    if segments.contains(&"generated") || lower.contains(".gen.") || lower.ends_with(".gen.rs") {
        return Some(CoverageExemption::Generated);
    }
    if let Some(header) = generated_header {
        let h = header.to_ascii_lowercase();
        if h.contains("@generated") || h.contains("do not edit") || h.contains("auto-generated") {
            return Some(CoverageExemption::Generated);
        }
    }

    // F1: tests — `tests/` segment, or a test-named file.
    let file_name = segments.last().copied().unwrap_or("");
    if segments.contains(&"tests")
        || file_name.starts_with("test_")
        || file_name.contains("_test.")
        || file_name.contains(".test.")
        || file_name.ends_with("_test")
    {
        return Some(CoverageExemption::TestFile);
    }

    // Extension-based classification.
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    // F3: docs / prose — non-source extensions, or anything under `docs/`.
    if segments.contains(&"docs")
        || matches!(ext, "md" | "txt" | "rst" | "adoc" | "html" | "css" | "svg")
    {
        return Some(CoverageExemption::DocOrProse);
    }

    // F4: config / data / lockfiles.
    if lower.ends_with(".lock")
        || file_name == ".gitignore"
        || matches!(
            ext,
            "toml" | "yaml" | "yml" | "json" | "ini" | "cfg" | "lock"
        )
    {
        return Some(CoverageExemption::ConfigData);
    }

    // Coverable only when the extension is in the source set.
    if COVERAGE_SOURCE_EXTENSIONS.contains(&ext) {
        return None;
    }
    // Unknown / non-source extension → not coverable (treated as prose).
    Some(CoverageExemption::DocOrProse)
}

/// True when a single source line is comment-only or blank, for the common
/// comment leaders (`//`, `///`, `//!`, `#`, `*`, `/*`, `*/`, `<!--`). PURE.
/// Used by the comment-only hunk exemption (SPIKE-47 H4). trace:STORY-499 | ai:claude
pub(crate) fn coverage_line_is_comment_or_blank(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    t.starts_with("//")
        || t.starts_with('#')
        || t.starts_with('*')
        || t.starts_with("/*")
        || t.starts_with("*/")
        || t.starts_with("<!--")
        || t.starts_with("-->")
}

/// SPIKE-47 §3.3 hunk-level exemption classifier. Returns `Some(reason)` when
/// a hunk (in an already-coverable file) is exempt. `trivial_max_lines` is the
/// configurable trivial-change floor (default 1). PURE. trace:STORY-499 | ai:claude
pub(crate) fn classify_coverage_hunk(
    hunk: &ParsedHunk,
    trivial_max_lines: usize,
) -> Option<CoverageExemption> {
    // H1: pure deletion — removed lines, no additions.
    if hunk.added.is_empty() {
        return Some(CoverageExemption::PureDeletion);
    }

    // H2 (fmt/whitespace) collapses into H1 once whitespace is normalized: a
    // whitespace-only reflow has `+`/`-` lines equal after trimming, so it
    // carries no net coverable addition. Detect by comparing the whitespace-
    // normalized added vs removed multisets.
    let norm = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect()
    };
    let mut added_norm = norm(&hunk.added);
    let mut removed_norm = norm(&hunk.removed);
    added_norm.sort();
    removed_norm.sort();
    if !added_norm.is_empty() && added_norm == removed_norm {
        // Whitespace-only reflow (no net content change) → fmt-only.
        return Some(CoverageExemption::CommentOrBlankOnly);
    }

    // H4 (comment-only): every added line is a comment or blank.
    if hunk
        .added
        .iter()
        .all(|l| coverage_line_is_comment_or_blank(l))
    {
        return Some(CoverageExemption::CommentOrBlankOnly);
    }

    // H4 (trivial): net effective added lines (non-blank, non-comment) at or
    // below the threshold.
    let effective_added = hunk
        .added
        .iter()
        .filter(|l| !coverage_line_is_comment_or_blank(l))
        .count();
    if effective_added <= trivial_max_lines {
        return Some(CoverageExemption::Trivial);
    }

    None
}

/// SPIKE-47 trace-anchor grammar (reused verbatim from `aida trace gate`):
/// matches `trace:<SPEC-ID>` regardless of comment leader. PURE compile.
/// trace:STORY-499 | ai:claude
pub(crate) fn coverage_anchor_re() -> regex::Regex {
    regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap()
}

/// True when `line` carries a `// trace:<id>` whose id resolves Live via
/// `resolve`. Module-level `//!` anchors and `///` doc anchors all match (the
/// regex keys off the `trace:` token, not the leader). PURE (resolver injected).
/// trace:STORY-499 | ai:claude
pub(crate) fn coverage_line_has_live_anchor<F>(
    line: &str,
    re: &regex::Regex,
    resolve: &mut F,
) -> bool
where
    F: FnMut(&str) -> SpecResolution,
{
    re.captures_iter(line).any(|cap| {
        let id = &cap[1];
        matches!(resolve(id), SpecResolution::Live)
    })
}

/// Attribute one hunk to a coverage source, per SPIKE-47 §4.2 (cheapest-first):
///   1. in-hunk anchor (added lines)
///   2. proximity anchor (≤ `proximity_lines` ABOVE the hunk in the post-change file)
///   3. file-level `//!` anchor anywhere in the file
///   4. commit-trailer fallback (the floor)
///      `post_change_lines` is the full post-change file (1-based logical, passed as a
///      slice). PURE — resolver + content injected, no I/O. trace:STORY-499 | ai:claude
pub(crate) fn attribute_hunk_coverage<F>(
    hunk: &ParsedHunk,
    post_change_lines: &[String],
    commit_trailer_covered: bool,
    proximity_lines: usize,
    re: &regex::Regex,
    resolve: &mut F,
) -> Option<CoverageSource>
where
    F: FnMut(&str) -> SpecResolution,
{
    // 1. In-hunk anchor.
    if hunk
        .added
        .iter()
        .any(|l| coverage_line_has_live_anchor(l, re, resolve))
    {
        return Some(CoverageSource::InHunkAnchor);
    }

    // 2. Proximity anchor — scan ABOVE-only (§4.2 rule 2 + risk #2). The hunk
    // starts at 1-based `new_start`; lines above are indices [start-1-N, start-1)
    // in the 0-based file vector.
    if hunk.new_start >= 1 {
        let start_idx = hunk.new_start.saturating_sub(1); // 0-based first hunk line
        let from = start_idx.saturating_sub(proximity_lines);
        for line in post_change_lines.iter().take(start_idx).skip(from) {
            if coverage_line_has_live_anchor(line, re, resolve) {
                return Some(CoverageSource::ProximityAnchor);
            }
        }
    }

    // 3. File-level `//!` module anchor anywhere in the file.
    if post_change_lines
        .iter()
        .any(|l| l.trim_start().starts_with("//!") && coverage_line_has_live_anchor(l, re, resolve))
    {
        return Some(CoverageSource::FileAnchor);
    }

    // 4. Commit-trailer fallback — the floor.
    if commit_trailer_covered {
        return Some(CoverageSource::CommitTrailer);
    }

    None
}

/// The PURE, fully-isolated core of the coverage gate. Given parsed hunks, the
/// post-change content of each touched file, a per-file generated-header probe,
/// a per-hunk commit-trailer-covered flag, and an anchor resolver, classify
/// every hunk (exempt / covered / uncovered-coverable) per the SPIKE-47
/// definition. NO git, NO store, NO I/O — every dependency is an argument.
///
/// `files`: post-change line content keyed by path (the file the hunk lives in).
/// `generated_headers`: first-lines blob per path (for the §3.2 generated-marker
///   probe); absent ⇒ probe skipped.
/// `commit_trailer_covered`: per-file flag — does the file's commit carry a live
///   `(SPEC-ID)` trailer? (the §4.2.4 floor).
/// trace:STORY-499 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_diff_coverage<F>(
    hunks: &[ParsedHunk],
    files: &std::collections::HashMap<String, Vec<String>>,
    generated_headers: &std::collections::HashMap<String, String>,
    commit_trailer_covered: &std::collections::HashMap<String, bool>,
    proximity_lines: usize,
    trivial_max_lines: usize,
    mut resolve: F,
) -> DiffCoverage
where
    F: FnMut(&str) -> SpecResolution,
{
    let re = coverage_anchor_re();
    let mut out = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let added_lines = hunk.added.len();

        // File-level exemption first (cheapest; whole file dropped).
        if let Some(reason) = classify_coverage_file(
            &hunk.file,
            generated_headers.get(&hunk.file).map(|s| s.as_str()),
        ) {
            out.push(HunkVerdict {
                file: hunk.file.clone(),
                new_start: hunk.new_start,
                added_lines,
                exempt: Some(reason),
                covered: None,
            });
            continue;
        }

        // Hunk-level exemption.
        if let Some(reason) = classify_coverage_hunk(hunk, trivial_max_lines) {
            out.push(HunkVerdict {
                file: hunk.file.clone(),
                new_start: hunk.new_start,
                added_lines,
                exempt: Some(reason),
                covered: None,
            });
            continue;
        }

        // Coverable — attribute coverage.
        let empty = Vec::new();
        let post = files.get(&hunk.file).unwrap_or(&empty);
        let trailer_covered = commit_trailer_covered
            .get(&hunk.file)
            .copied()
            .unwrap_or(false);
        let covered = attribute_hunk_coverage(
            hunk,
            post,
            trailer_covered,
            proximity_lines,
            &re,
            &mut resolve,
        );
        out.push(HunkVerdict {
            file: hunk.file.clone(),
            new_start: hunk.new_start,
            added_lines,
            exempt: None,
            covered,
        });
    }
    DiffCoverage { hunks: out }
}

/// Parse `git diff --unified=0` output into `ParsedHunk`s. PURE (takes the raw
/// diff text). Tracks the current `+++ b/<path>` file and each `@@ -a,b +c,d @@`
/// header to assign `new_start` and split `+`/`-` line bodies. Skips
/// `/dev/null` (deleted-file) targets. trace:STORY-499 | ai:claude
pub(crate) fn parse_unified_diff_hunks(diff: &str) -> Vec<ParsedHunk> {
    let hunk_re = regex::Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap();
    let mut hunks: Vec<ParsedHunk> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut cur: Option<ParsedHunk> = None;

    let flush = |cur: &mut Option<ParsedHunk>, hunks: &mut Vec<ParsedHunk>| {
        if let Some(h) = cur.take() {
            if !h.added.is_empty() || !h.removed.is_empty() {
                hunks.push(h);
            }
        }
    };

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            flush(&mut cur, &mut hunks);
            let path = rest.trim();
            current_file = if path == "/dev/null" {
                None
            } else {
                // Strip the `b/` prefix git prepends.
                Some(path.strip_prefix("b/").unwrap_or(path).to_string())
            };
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff --git") {
            continue;
        }
        if let Some(cap) = hunk_re.captures(line) {
            flush(&mut cur, &mut hunks);
            if let Some(file) = &current_file {
                let new_start: usize = cap[1].parse().unwrap_or(1);
                cur = Some(ParsedHunk {
                    file: file.clone(),
                    new_start,
                    added: Vec::new(),
                    removed: Vec::new(),
                });
            }
            continue;
        }
        if let Some(h) = cur.as_mut() {
            if let Some(body) = line.strip_prefix('+') {
                h.added.push(body.to_string());
            } else if let Some(body) = line.strip_prefix('-') {
                h.removed.push(body.to_string());
            }
        }
    }
    flush(&mut cur, &mut hunks);
    hunks
}

/// Read the unified diff for a range, `git diff -M -w --unified=0 <base>..HEAD`.
/// `-M` enables rename detection (renamed files emit no content hunks → §3.3 H3),
/// `-w` ignores whitespace so fmt-only reflows produce no hunks (§3.3 H2).
/// trace:STORY-499 | ai:claude
pub(crate) fn read_diff_for_range(project_root: &std::path::Path, range: &str) -> Result<String> {
    use std::process::Command as PCmd;
    let output = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "-M",
            "-w",
            "--unified=0",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).to_string()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            anyhow::bail!("git diff failed for range `{range}`: {}", err.trim());
        }
        Err(e) => anyhow::bail!("could not run git: {e}"),
    }
}

/// Read the post-change content of a file at HEAD (or the working tree) as a
/// line vector, for the proximity / file-level anchor scan (§4.2). Tries the
/// on-disk file first (the merged tree in CI checkout), falling back to
/// `git show HEAD:<path>`. Returns an empty vec when neither resolves (the hunk
/// can still be covered by an in-hunk anchor or the commit trailer).
/// trace:STORY-499 | ai:claude
pub(crate) fn read_post_change_file(project_root: &std::path::Path, path: &str) -> Vec<String> {
    let on_disk = project_root.join(path);
    if let Ok(content) = std::fs::read_to_string(&on_disk) {
        return content.lines().map(|l| l.to_string()).collect();
    }
    use std::process::Command as PCmd;
    if let Ok(o) = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["show", &format!("HEAD:{path}")])
        .output()
    {
        if o.status.success() {
            return String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect();
        }
    }
    Vec::new()
}

/// For each touched file, determine whether ANY commit in `range` that touches
/// it carries a live `(SPEC-ID)` trailer (the §4.2.4 floor). Conservative and
/// cheap: if any non-plan commit referencing a live spec touched the file, its
/// hunks get the trailer-floor. Returns a per-file flag map. trace:STORY-499 | ai:claude
pub(crate) fn compute_trailer_floor<F>(
    project_root: &std::path::Path,
    range: &str,
    files: &[String],
    resolve: &mut F,
) -> std::collections::HashMap<String, bool>
where
    F: FnMut(&str) -> SpecResolution,
{
    use std::process::Command as PCmd;
    let mut out = std::collections::HashMap::new();
    for file in files {
        // Commits in range that touched this file, with subjects.
        let covered = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            // trace:BUG-1622 | ai:claude
            .args([
                "log",
                "--no-merges",
                "--pretty=format:%s",
                git_arg_guard::END_OF_OPTIONS,
                range,
                "--",
                file,
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout).lines().any(|subject| {
                    if is_plan_commit_subject(subject) {
                        return false;
                    }
                    extract_spec_ids_from_commit(subject)
                        .iter()
                        .any(|id| matches!(resolve(id), SpecResolution::Live))
                })
            })
            .unwrap_or(false);
        out.insert(file.clone(), covered);
    }
    out
}

/// CLI handler for `aida trace coverage`. Gathers the diff + post-change files +
/// commit-trailer floor + live-graph resolver from git/the store, runs the pure
/// `compute_diff_coverage` core, prints a report, and exits non-zero ONLY when
/// `block` posture is active and a coverable hunk is uncovered. Default posture
/// is report-only (CI-success regardless) per SPIKE-47 §5/§6. trace:STORY-499 | ai:claude
pub(crate) fn handle_trace_coverage(range: Option<&str>, json: bool, block: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let diff = read_diff_for_range(&project_root, &range)?;
    let hunks = parse_unified_diff_hunks(&diff);

    // The live requirement graph (for anchor/trailer resolution).
    let store = load_store_for_lookup(&project_root);
    if store.is_none() {
        eprintln!(
            "{} trace coverage: no requirement store reachable from {} — anchor/trailer ids cannot \
             be confirmed live (every id is treated as live; coverage may be over-counted). Run \
             where the store is attached (`aida cache rebuild`).",
            crate::glyph(crate::glyphs::Glyph::Warning),
            project_root.display()
        );
    }

    // Distinct touched files (post-change paths).
    let mut touched: Vec<String> = hunks.iter().map(|h| h.file.clone()).collect();
    touched.sort();
    touched.dedup();

    // Post-change content + generated-marker headers per file.
    let mut files = std::collections::HashMap::new();
    let mut generated_headers = std::collections::HashMap::new();
    for path in &touched {
        let lines = read_post_change_file(&project_root, path);
        let header: String = lines.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
        generated_headers.insert(path.clone(), header);
        files.insert(path.clone(), lines);
    }

    // Commit-trailer floor per file.
    let mut floor_resolve =
        |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let trailer_floor = compute_trailer_floor(&project_root, &range, &touched, &mut floor_resolve);

    let resolve = |id: &str| -> SpecResolution { resolve_spec_in_store(store.as_ref(), id) };
    let coverage = compute_diff_coverage(
        &hunks,
        &files,
        &generated_headers,
        &trailer_floor,
        5, // proximity_lines (SPIKE-47 §4.2 default N=5)
        1, // trivial_max_lines (SPIKE-47 §3.3 H4 default)
        resolve,
    );

    let uncovered = coverage.uncovered();
    let posture = if block { "block" } else { "report" };

    // Audited exemption breakdown — SPIKE-47 §3 requires every exemption to be
    // reported (never silently dropped). trace:STORY-499 | ai:claude
    let mut exempt_breakdown: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for h in &coverage.hunks {
        if let Some(reason) = h.exempt {
            *exempt_breakdown.entry(reason.reason()).or_insert(0) += 1;
        }
    }

    if json {
        let payload = serde_json::json!({
            "range": range,
            "posture": posture,
            "hunks_total": coverage.hunks.len(),
            "coverable": coverage.coverable(),
            "covered": coverage.covered(),
            "exempt": coverage.exempt(),
            "exempt_breakdown": exempt_breakdown,
            "uncovered": uncovered.len(),
            "ratio": coverage.ratio(),
            "uncovered_hunks": uncovered,
            "ok": uncovered.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "trace coverage [{posture}]: {} hunk(s) in `{}` — {} coverable, {} covered, {} exempt \
             (ratio {:.0}%)",
            coverage.hunks.len(),
            range,
            coverage.coverable(),
            coverage.covered(),
            coverage.exempt(),
            coverage.ratio() * 100.0,
        );
        if !exempt_breakdown.is_empty() {
            let summary = exempt_breakdown
                .iter()
                .map(|(reason, n)| format!("{n} {reason}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  exempt: {summary}");
        }
        if uncovered.is_empty() {
            println!(
                "{} every coverable changed hunk carries trace coverage",
                crate::glyph(crate::glyphs::Glyph::Check)
            );
        } else {
            eprintln!(
                "{} {} uncovered coverable hunk(s):",
                crate::glyph(crate::glyphs::Glyph::Warning),
                uncovered.len()
            );
            for h in &uncovered {
                eprintln!(
                    "  {}:{} (+{} line(s)) — no // trace: anchor and no live commit trailer",
                    h.file, h.new_start, h.added_lines
                );
            }
            eprintln!(
                "\nAdd a `// trace:<SPEC-ID>` near the change (or ensure the commit carries a live \
                 `(SPEC-ID)` trailer). Tests / generated / docs / config / trivial changes are \
                 exempt automatically."
            );
        }
    }

    // Report-only by default (CI succeeds); only `block` posture exits non-zero.
    if block && !uncovered.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// A kind of newly-added public CLI/MCP surface detected in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicSurfaceKind {
    /// A new clap flag (an `#[arg(long…)]` / `#[clap(long…)]` attribute).
    CliFlag,
    /// A new named clap subcommand (an `#[command(name = …)]` attribute).
    CliSubcommand,
    /// A new MCP tool descriptor (`"name": "<snake_case>"` in `mcp.rs`).
    McpTool,
}

impl PublicSurfaceKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PublicSurfaceKind::CliFlag => "CLI flag",
            PublicSurfaceKind::CliSubcommand => "CLI subcommand",
            PublicSurfaceKind::McpTool => "MCP tool",
        }
    }
}

/// One added line the classifier flagged as new public surface.
// trace:TASK-939 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct SurfaceHit {
    pub(crate) file: String,
    /// 1-based start line of the containing hunk in the post-change file.
    pub(crate) new_start: usize,
    pub(crate) kind: PublicSurfaceKind,
    /// The trimmed added line that matched (for the human report).
    pub(crate) snippet: String,
}

/// True when `word` occurs in `hay` bounded by non-identifier chars on both
/// sides (so `long` matches in `#[clap(long)]` but NOT inside `long_help`).
// trace:TASK-939 | ai:claude
pub(crate) fn contains_ident_word(hay: &str, word: &str) -> bool {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// An added line that declares a new clap long flag: an `#[arg(…)]` /
/// `#[clap(…)]` attribute carrying a word-boundaried `long`. Excludes
/// `long_help` / `long_about` (not a flag) and `#[clap(subcommand)]` (no
/// `long`). PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn is_clap_long_flag(line: &str) -> bool {
    let t = line.trim_start();
    if !t.starts_with("#[") {
        return false;
    }
    if !(t.contains("clap(") || t.contains("arg(")) {
        return false;
    }
    contains_ident_word(t, "long")
}

/// An added line that declares a *named* clap subcommand: an `#[command(…)]` /
/// `#[clap(…)]` attribute carrying `name = "…"`. (Unit-variant subcommands
/// that derive their name from the variant carry no attribute and are not
/// detected here — flags added alongside them usually are.) PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn is_clap_named_subcommand(line: &str) -> bool {
    let t = line.trim_start();
    if !t.starts_with("#[") {
        return false;
    }
    if !(t.contains("clap(") || t.contains("command(")) {
        return false;
    }
    contains_ident_word(t, "name") && t.contains('=')
}

/// Extract the tool name from an MCP tool descriptor line — `"name": "<value>"`
/// where `<value>` is a snake_case identifier. Returns `None` for Title-case
/// resource titles (`"Project Summary"`) or non-descriptor lines. PURE.
// trace:TASK-939 | ai:claude
pub(crate) fn mcp_tool_name_in_line(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("\"name\"")?;
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    let val = &rest[..end];
    let ident = !val.is_empty()
        && val.starts_with(|c: char| c.is_ascii_lowercase())
        && val
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    ident.then_some(val)
}

/// Classify a single ADDED diff line as new public surface, or `None`. PURE —
/// the `file` path only selects the MCP-tool probe (limited to `mcp.rs`).
// trace:TASK-939 | ai:claude
pub(crate) fn classify_surface_line(file: &str, line: &str) -> Option<PublicSurfaceKind> {
    // MCP tool descriptor — only in the MCP surface file. The server-name line
    // (`"name": "aida"`) is excluded so it never reads as a new tool.
    if file.ends_with("mcp.rs") {
        if let Some(name) = mcp_tool_name_in_line(line) {
            if name != "aida" {
                return Some(PublicSurfaceKind::McpTool);
            }
        }
    }
    if is_clap_named_subcommand(line) {
        return Some(PublicSurfaceKind::CliSubcommand);
    }
    if is_clap_long_flag(line) {
        return Some(PublicSurfaceKind::CliFlag);
    }
    None
}

/// Scan parsed diff hunks for newly-added public surface. PURE — the testable
/// core of `aida doc suggest`.
// trace:TASK-939 | ai:claude
pub(crate) fn detect_new_public_surface(hunks: &[ParsedHunk]) -> Vec<SurfaceHit> {
    let mut hits = Vec::new();
    for h in hunks {
        for line in &h.added {
            if let Some(kind) = classify_surface_line(&h.file, line) {
                hits.push(SurfaceHit {
                    file: h.file.clone(),
                    new_start: h.new_start,
                    kind,
                    snippet: line.trim().to_string(),
                });
            }
        }
    }
    hits
}

/// Collect the spec ids this PR references — commit-trailer ids across `range`
/// (plan commits skipped) plus inline `trace:` anchors in the added lines.
/// These are the ids a Doc entry would be `--about`.
// trace:TASK-939 | ai:claude
pub(crate) fn specs_referenced_in_range(
    project_root: &std::path::Path,
    range: &str,
    hunks: &[ParsedHunk],
) -> Vec<String> {
    use std::process::Command as PCmd;
    let mut ids: Vec<String> = Vec::new();

    if let Ok(out) = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        // trace:BUG-1622 | ai:claude
        .args([
            "log",
            "--no-merges",
            "--pretty=format:%s",
            git_arg_guard::END_OF_OPTIONS,
            range,
            "--",
        ])
        .output()
    {
        if out.status.success() {
            for subject in String::from_utf8_lossy(&out.stdout).lines() {
                if is_plan_commit_subject(subject) {
                    continue;
                }
                ids.extend(extract_spec_ids_from_commit(subject));
            }
        }
    }

    let re = coverage_anchor_re();
    for h in hunks {
        for line in &h.added {
            for cap in re.captures_iter(line) {
                ids.push(cap[1].to_string());
            }
        }
    }

    ids.sort();
    ids.dedup();
    ids
}

/// CLI handler for `aida doc suggest`. Reads the branch diff, detects new public
/// surface, resolves the referenced specs against the live graph, and nudges
/// `aida doc add --about <ID>` for any that carry no Doc entry. Warn-only —
/// always exits 0.
// trace:TASK-939 | ai:claude
pub(crate) fn handle_doc_suggest(range: Option<&str>, json: bool) -> Result<()> {
    use aida_core::models::{RelationshipType, RequirementStatus, RequirementType};

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    // trace:BUG-1622 | ai:claude
    if let Some(r) = range {
        git_arg_guard::reject_option_like("--range", r)?;
    }
    let range = resolve_gate_range(&project_root, range);

    let diff = read_diff_for_range(&project_root, &range)?;
    let hunks = parse_unified_diff_hunks(&diff);
    let surface = detect_new_public_surface(&hunks);
    let referenced = specs_referenced_in_range(&project_root, &range, &hunks);

    // The live requirement graph — for the documented-set check.
    let store = load_store_for_lookup(&project_root);

    // Set of spec uuids referenced by at least one Doc.
    let mut documented: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    if let Some(s) = store.as_ref() {
        for doc in s
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Doc)
        {
            for rel in &doc.relationships {
                if rel.rel_type == RelationshipType::References {
                    documented.insert(rel.target_id);
                }
            }
        }
    }

    // Partition the referenced specs into gaps (undocumented, live, non-Doc)
    // and already-documented.
    let mut gaps: Vec<(String, String)> = Vec::new();
    let mut documented_ids: Vec<String> = Vec::new();
    if let Some(s) = store.as_ref() {
        for cid in &referenced {
            let want = cid.to_ascii_uppercase();
            let found = s.requirements.iter().find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(&want))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(&want))
            });
            match found {
                // A Doc, a rejected spec, or an unresolvable id is not a target.
                Some(r) if r.req_type == RequirementType::Doc => {}
                Some(r) if matches!(r.status, RequirementStatus::Rejected) => {}
                Some(r) if r.archived => {}
                Some(r) => {
                    if documented.contains(&r.id) {
                        documented_ids.push(r.display_id());
                    } else {
                        gaps.push((r.display_id(), r.title.clone()));
                    }
                }
                None => {}
            }
        }
    }

    let has_surface = !surface.is_empty();
    // "ok" = nothing to nudge: either no new surface, or every referenced spec
    // already carries a doc.
    let ok = !has_surface || gaps.is_empty();

    if json {
        let surface_rows: Vec<serde_json::Value> = surface
            .iter()
            .map(|h| {
                serde_json::json!({
                    "file": h.file,
                    "line": h.new_start,
                    "kind": h.kind,
                    "snippet": h.snippet,
                })
            })
            .collect();
        let gap_rows: Vec<serde_json::Value> = gaps
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
            .collect();
        let payload = serde_json::json!({
            "range": range,
            "surface_count": surface.len(),
            "surface": surface_rows,
            "referenced_specs": referenced,
            "documented_specs": documented_ids,
            "gaps": gap_rows,
            "ok": ok,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if !has_surface {
        println!(
            "{} No new CLI/MCP surface added in `{}` — nothing to capture.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            range
        );
        return Ok(());
    }

    println!(
        "{} New public surface added in `{}`:",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        range
    );
    for h in &surface {
        println!(
            "  {} · {}:{} · {}",
            h.kind.label().cyan(),
            h.file,
            h.new_start,
            h.snippet
        );
    }
    println!();

    if !gaps.is_empty() {
        println!(
            "{} Referenced spec(s) with no doc entry:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        for (id, title) in &gaps {
            println!("  {} · {}", id.cyan(), title);
        }
        println!();
        println!("Capture the \"why\" while it's fresh:");
        for (id, _) in &gaps {
            println!("  aida doc add --title \"…\" --about {}", id);
        }
        println!("(warn-only — this nudge does not block the PR)");
    } else if !documented_ids.is_empty() {
        println!(
            "{} Referenced spec(s) already have doc entries — nothing to capture.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        println!(
            "{} Couldn't resolve which spec this surface belongs to (no live commit trailer or \
             `trace:` anchor). Capture docs with:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        println!("  aida doc add --title \"…\" --about <SPEC-ID>");
        println!("(warn-only — this nudge does not block the PR)");
    }

    Ok(())
}

#[cfg(test)]
mod show_forge_budget_tests {
    use super::SHOW_FORGE_LOOKUP_BUDGET;
    use crate::FORGE_CLI_CALL_TIMEOUT;
    use std::time::Duration;

    /// BUG-1594's integration test reports its wall-clock only as a soft
    /// budget, so this pins the worst case structurally: the last forge
    /// call may start just before the lookup budget expires and then run to
    /// its own per-call timeout.
    // trace:BUG-1995 | ai:claude
    #[test]
    fn show_forge_lookup_worst_case_stays_within_ten_seconds() {
        assert!(
            SHOW_FORGE_LOOKUP_BUDGET + FORGE_CLI_CALL_TIMEOUT <= Duration::from_secs(10),
            "SHOW_FORGE_LOOKUP_BUDGET ({SHOW_FORGE_LOOKUP_BUDGET:?}) + FORGE_CLI_CALL_TIMEOUT \
             ({FORGE_CLI_CALL_TIMEOUT:?}) must stay within show's 10 s forge ceiling"
        );
        assert!(FORGE_CLI_CALL_TIMEOUT > Duration::ZERO);
    }
}
