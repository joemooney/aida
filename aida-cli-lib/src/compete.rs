//! `aida compete <SPEC> --vendors <csv> [--gate "<cmd>"]` — run one spec through
//! N vendors headless, in isolated worktrees, then a deterministic objective
//! gate; report a table and leave every branch in place for the advisor to pick.
//!
//! This is the flagship niche feature productizing the SPIKE-64 finding: the
//! value of running N vendors is NOT architectural diversity (vendors converge
//! on a real codebase because the substrate dictates the shape) — it is
//! execution-QUALITY variance within the converged design. So `aida compete` is
//! framed as competition-as-QA: run a spec through N vendors + an objective gate,
//! surface the one that drifted, and let a neutral substrate-owner pick the best.
//! Slice 1 = orchestration + objective gate + REPORT-ONLY. Slice 2 (STORY-660)
//! adds a JUDGE after the gate: a cheap deterministic ranking (smaller/focused
//! diff first — the gate is necessary but not sufficient) always, plus an opt-in
//! (`--judge`) rubric LLM judge. Still report-only: no synthesis, no auto-merge.
//!
//! This module holds the PURE, unit-testable parts: the vendor→command mapping
//! (the adapter table), the per-vendor branch naming, the gate-result parsing,
//! the report-table formatting, the deterministic ranking, and the judge prompt
//! builder / verdict parser / verdict renderer. The I/O orchestration (spawning
//! vendors, creating worktrees, running the gate, spawning the LLM judge) lives
//! in `main.rs::handle_compete_command` so the costly real-vendor + real-judge
//! runs never execute in tests.
//!
//! trace:STORY-659 trace:STORY-660 | ai:claude

/// How a vendor is driven in slice 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorAdapter {
    /// A vendor with a headless CLI we can spawn directly. `command` is the
    /// binary; `args_template` is the argv WITHOUT the brief (the brief is
    /// appended as the final positional arg by the caller). Keeping the brief
    /// out of the template keeps this a pure description of the adapter.
    Headless {
        command: &'static str,
        /// argv that precede the brief positional (e.g. `["-p", ...]` for claude,
        /// `["exec", "--dangerously-bypass-approvals-and-sandbox"]` for codex).
        args_template: Vec<String>,
    },
    /// A vendor with NO headless CLI (e.g. antigravity). Instead of running it we
    /// emit an `aida brief <vendor> <SPEC>` for the human to run, and report it
    /// as a "human-run arm (briefed)". This is the cross-vendor coordination
    /// path: the neutral substrate routes the work even when it can't execute it.
    HumanBriefed,
}

/// Resolve a vendor name to its adapter. Adding a vendor later is a single row.
/// Returns `None` for an unknown vendor so the caller can skip-with-a-note
/// rather than fail the whole run.
///
/// The claude argv intentionally does NOT reuse `session::claude_headless_args`:
/// that builder forces `--session-id`/`--output-format stream-json` for the
/// orchestrator's resumable, machine-parsed drains. A compete arm just needs a
/// one-shot headless run whose stdout we tee to a log, so the simpler
/// `-p --permission-mode bypassPermissions` flag set is the right surface here.
pub fn vendor_adapter(vendor: &str) -> Option<VendorAdapter> {
    match vendor {
        "claude" => Some(VendorAdapter::Headless {
            command: "claude",
            args_template: vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
        }),
        "codex" => Some(VendorAdapter::Headless {
            command: "codex",
            args_template: vec![
                "exec".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
            ],
        }),
        "antigravity" => Some(VendorAdapter::HumanBriefed),
        _ => None,
    }
}

/// The full argv for a headless vendor run: the adapter template plus the brief
/// as the final positional argument. Returns `None` for a non-headless adapter.
pub fn headless_argv(adapter: &VendorAdapter, brief: &str) -> Option<Vec<String>> {
    match adapter {
        VendorAdapter::Headless { args_template, .. } => {
            let mut argv = args_template.clone();
            argv.push(brief.to_string());
            Some(argv)
        }
        VendorAdapter::HumanBriefed => None,
    }
}

/// Per-vendor branch name: `compete/<spec-lower>-<vendor>`. Stable + collision-
/// free across vendors for the same spec, and namespaced under `compete/` so a
/// `git branch --list 'compete/*'` finds every arm.
pub fn vendor_branch(spec_id: &str, vendor: &str) -> String {
    format!("compete/{}-{}", spec_id.to_ascii_lowercase(), vendor)
}

/// The default deterministic gate for THIS repo. It MIRRORS the PR CI surface
/// (`.github/workflows/ci.yml`) so a gate-passing arm is actually mergeable —
/// fmt-check, build, the aida-cli test suite, the `clippy::correctness` lint,
/// and the glyph-lint guard. A narrower build+test-only gate gives false-green
/// "winners" that then fail CI at `cargo fmt --check` (the BUG-576 dogfood).
///
/// The aida-cli test step unsets the advisor's role env (it leaks into
/// role-gated tests and causes false failures — see the project memory on
/// `AIDA_SESSION_ROLE`). The `--gate "<cmd>"` flag overrides this wholesale for
/// non-Rust repos / custom CI surfaces.
// trace:BUG-576 | ai:claude
pub const DEFAULT_GATE: &str = "cargo fmt --all -- --check && cargo build && env -u AIDA_SESSION_ROLE cargo test -p aida-cli && cargo clippy -- -D clippy::correctness && bash scripts/glyph-lint.sh --block";

/// Outcome of one vendor arm. Pure data — assembled by the orchestrator, then
/// handed to [`render_report`] for formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmResult {
    pub vendor: String,
    /// Did the vendor actually run? `Ran::*` distinguishes the honest outcomes.
    pub ran: Ran,
    /// Did the gate's build step succeed? `None` when the gate didn't run
    /// (vendor skipped / errored / human-briefed).
    pub built: Option<bool>,
    /// Did the full gate command exit 0? `None` when the gate didn't run.
    pub gate_passed: Option<bool>,
    /// Lines changed vs the base (added + removed), if measurable.
    pub diff_lines: Option<usize>,
    /// The per-vendor branch the work landed on (empty for human-briefed/skipped).
    pub branch: String,
}

/// Honest run outcomes — partial results are still useful (competition-as-QA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// Vendor CLI ran and exited successfully.
    Ok,
    /// Vendor CLI ran but exited non-zero (we still gate + report its branch).
    Failed,
    /// Vendor CLI not found on PATH — skipped with a note, run continues.
    Skipped,
    /// Non-headless vendor: an `aida brief` was emitted; a human runs this arm.
    Briefed,
}

impl Ran {
    /// Short cell label for the `ran` column.
    pub fn label(self) -> &'static str {
        match self {
            Ran::Ok => "ok",
            Ran::Failed => "fail",
            Ran::Skipped => "skip",
            Ran::Briefed => "briefed",
        }
    }
}

/// Parse a gate run into (built?, gate_passed?). The default gate is one shell
/// command of the shape `<fmt> && cargo build && <tests> && <clippy> && <glyph>`
/// (a custom `--gate` may differ); we treat a zero overall exit as a full pass,
/// and infer the build sub-result from whether the combined stderr/stdout shows
/// a build failure. We keep this conservative and HONEST: if the overall command
/// passed, the build necessarily passed (every clause ran green); if it failed,
/// we look for the cargo build-failure marker to attribute the failure to the
/// build specifically (a fmt/clippy/glyph failure leaves `built = true`).
pub fn parse_gate_result(exit_ok: bool, combined_output: &str) -> (bool, bool) {
    if exit_ok {
        // A `cargo build && cargo test` that exits 0 means both built and tested.
        return (true, true);
    }
    // Overall failure: did the BUILD fail, or did it build and the tests fail?
    let lower = combined_output.to_ascii_lowercase();
    let build_failed = lower.contains("error: could not compile")
        || lower.contains("error[e")
        || lower.contains("build failed");
    let built = !build_failed;
    (built, false)
}

/// Count changed lines (added + removed) from a `git diff --numstat` body.
/// Each line is `<added>\t<removed>\t<path>`; binary files show `-` and are
/// skipped. Returns the summed magnitude — a coarse but honest "how much did
/// this arm change" signal for the report.
pub fn count_diff_lines(numstat: &str) -> usize {
    numstat
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let added = cols.next()?.parse::<usize>().ok()?;
            let removed = cols.next()?.parse::<usize>().ok()?;
            Some(added + removed)
        })
        .sum()
}

/// Glyphs the report renderer needs, supplied by the caller from the central
/// registry (so this pure module never embeds raw glyph literals — they route
/// through `[ui] glyphs` / `AIDA_GLYPHS`). trace:TASK-835
#[derive(Debug, Clone)]
pub struct ReportGlyphs {
    pub check: String,
    pub cross: String,
    pub pending: String,
}

/// Format the per-cell yes/no/na value with the supplied glyphs.
fn tri(value: Option<bool>, g: &ReportGlyphs) -> String {
    match value {
        Some(true) => g.check.clone(),
        Some(false) => g.cross.clone(),
        None => g.pending.clone(),
    }
}

/// Render the report: a `vendor | ran | built | gate | diff-lines | branch`
/// table plus a one-line verdict per arm. Pure — fully unit-testable.
pub fn render_report(spec_id: &str, results: &[ArmResult], g: &ReportGlyphs) -> String {
    let mut out = String::new();
    out.push_str(&format!("Competition results for {spec_id}\n\n"));

    // Column widths sized to content (with sane minimums for the headers).
    let vendor_w = results
        .iter()
        .map(|r| r.vendor.len())
        .max()
        .unwrap_or(6)
        .max("vendor".len());
    let branch_w = results
        .iter()
        .map(|r| r.branch.len())
        .max()
        .unwrap_or(6)
        .max("branch".len());

    out.push_str(&format!(
        "{:<vw$}  {:<7}  {:<5}  {:<4}  {:>10}  {:<bw$}\n",
        "vendor",
        "ran",
        "built",
        "gate",
        "diff-lines",
        "branch",
        vw = vendor_w,
        bw = branch_w,
    ));
    for r in results {
        let diff = r
            .diff_lines
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let branch = if r.branch.is_empty() { "-" } else { &r.branch };
        out.push_str(&format!(
            "{:<vw$}  {:<7}  {:<5}  {:<4}  {:>10}  {:<bw$}\n",
            r.vendor,
            r.ran.label(),
            tri(r.built, g),
            tri(r.gate_passed, g),
            diff,
            branch,
            vw = vendor_w,
            bw = branch_w,
        ));
    }

    out.push('\n');
    for line in verdict_lines(results) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// One honest one-line verdict per arm.
pub fn verdict_lines(results: &[ArmResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| match r.ran {
            Ran::Skipped => format!("{}: skipped — CLI not found on PATH", r.vendor),
            Ran::Briefed => format!(
                "{}: human-run arm — brief emitted (no headless CLI); run it, then re-gate",
                r.vendor
            ),
            Ran::Failed if r.gate_passed == Some(true) => format!(
                "{}: vendor exited non-zero but gate PASS on branch {} — inspect the log",
                r.vendor, r.branch
            ),
            Ran::Failed => format!(
                "{}: vendor run errored — gate {} on branch {} — see the log",
                r.vendor,
                gate_word(r.gate_passed),
                r.branch
            ),
            Ran::Ok => format!(
                "{}: gate {} on branch {}",
                r.vendor,
                gate_word(r.gate_passed),
                r.branch
            ),
        })
        .collect()
}

fn gate_word(passed: Option<bool>) -> &'static str {
    match passed {
        Some(true) => "PASS",
        Some(false) => "FAIL",
        None => "not run",
    }
}

// ---------------------------------------------------------------------------
// STORY-660 — the judge: a cheap deterministic ranking (always) + an opt-in
// rubric LLM judge (--judge). The gate is necessary but NOT sufficient: the
// dogfood showed BOTH arms gate-pass yet one over-reached. The value of running
// N vendors is execution-quality variance within a converged design — so after
// the gate we add a ranking step. These are the PURE, unit-testable parts:
// deterministic ranking by diff-size/files, verdict parsing, and report
// rendering. The `claude -p` judge call itself lives in main.rs (never in tests).
// trace:STORY-660 | ai:claude
// ---------------------------------------------------------------------------

/// A gate-passing candidate ranked by the cheap deterministic signal. Smaller,
/// more focused diffs rank higher (all else equal) — now that BUG-575 keeps the
/// vendor run-log out of the diff, `diff_lines` is an honest code-size signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterministicRank {
    pub vendor: String,
    pub branch: String,
    pub diff_lines: usize,
    pub files_touched: usize,
}

/// Rank the gate-passing arms by the cheap deterministic signal: fewer changed
/// lines first, ties broken by fewer files touched, then by vendor name for a
/// stable order. Only arms that actually passed the gate are ranked — a failing
/// arm is not a winner candidate regardless of how small its diff is. The
/// `files_touched` per vendor is supplied by the caller (it counts `--numstat`
/// rows); arms missing a diff measurement sort last. trace:STORY-660 | ai:claude
pub fn deterministic_ranking(
    results: &[ArmResult],
    files_touched: &[(String, usize)],
) -> Vec<DeterministicRank> {
    let mut ranked: Vec<DeterministicRank> = results
        .iter()
        .filter(|r| r.gate_passed == Some(true))
        .map(|r| DeterministicRank {
            vendor: r.vendor.clone(),
            branch: r.branch.clone(),
            diff_lines: r.diff_lines.unwrap_or(usize::MAX),
            files_touched: files_touched
                .iter()
                .find(|(v, _)| v == &r.vendor)
                .map(|(_, n)| *n)
                .unwrap_or(usize::MAX),
        })
        .collect();
    ranked.sort_by(|a, b| {
        a.diff_lines
            .cmp(&b.diff_lines)
            .then(a.files_touched.cmp(&b.files_touched))
            .then(a.vendor.cmp(&b.vendor))
    });
    ranked
}

/// Render the deterministic-rank block for the report. Pure. Shows each
/// gate-passing arm ordered smaller-diff-first, marking the leader. This is a
/// useful tie-breaker on its own (no LLM needed). trace:STORY-660 | ai:claude
pub fn render_deterministic_ranking(ranked: &[DeterministicRank]) -> String {
    if ranked.is_empty() {
        return "Deterministic ranking: no gate-passing arms to rank.\n".to_string();
    }
    let mut out = String::new();
    out.push_str("Deterministic ranking (gate-passers, smaller/focused diff first):\n");
    for (i, r) in ranked.iter().enumerate() {
        let lines = if r.diff_lines == usize::MAX {
            "?".to_string()
        } else {
            r.diff_lines.to_string()
        };
        let files = if r.files_touched == usize::MAX {
            "?".to_string()
        } else {
            r.files_touched.to_string()
        };
        let marker = if i == 0 { " <- smallest" } else { "" };
        out.push_str(&format!(
            "  {}. {} ({} lines, {} files){}\n",
            i + 1,
            r.vendor,
            lines,
            files,
            marker
        ));
    }
    out
}

/// One vendor's rubric scores from the LLM judge. Scores are 1-5 per axis.
#[derive(Debug, Clone, PartialEq)]
pub struct RubricScore {
    pub vendor: String,
    pub spec_adherence: u8,
    pub correctness: u8,
    pub simplicity: u8,
    pub test_coverage: u8,
}

impl RubricScore {
    /// Sum of the four axes — the headline rubric total (out of 20).
    pub fn total(&self) -> u32 {
        self.spec_adherence as u32
            + self.correctness as u32
            + self.simplicity as u32
            + self.test_coverage as u32
    }
}

/// The structured verdict parsed from the LLM judge's JSON output.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeVerdict {
    pub scores: Vec<RubricScore>,
    /// The vendor the judge recommends shipping.
    pub winner: String,
    /// One-line rationale for the recommended winner.
    pub reasoning: String,
}

/// The JSON shape we ask the judge to emit, then parse. Keeping the prompt's
/// contract in one place so the prompt builder and the parser can't drift.
/// trace:STORY-660 | ai:claude
pub const JUDGE_JSON_CONTRACT: &str = r#"{"scores":[{"vendor":"<name>","spec_adherence":1-5,"correctness":1-5,"simplicity":1-5,"test_coverage":1-5}],"winner":"<vendor>","reasoning":"<one line>"}"#;

/// Which vendor renders the rubric judgment. The judge PROMPT is identical for
/// both; only the executing model changes. Default `Claude` preserves the
/// pre-TASK-869 behaviour. Splitting the judge from the implementer vendor is
/// what removes the self-evaluation caveat: a Codex judge over a Claude-vs-Codex
/// bake-off is no longer Claude grading Claude. trace:TASK-869 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JudgeVendor {
    #[default]
    Claude,
    Codex,
}

impl JudgeVendor {
    /// Parse a `--judge-vendor` value. Case-insensitive. Returns `None` for an
    /// unknown vendor so the caller can reject with a legible error.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(JudgeVendor::Claude),
            "codex" => Some(JudgeVendor::Codex),
            _ => None,
        }
    }

    /// The canonical lowercase name (also the `--judge-vendor` value).
    pub fn as_str(self) -> &'static str {
        match self {
            JudgeVendor::Claude => "claude",
            JudgeVendor::Codex => "codex",
        }
    }

    /// The default binary spawned for this judge vendor, before any
    /// `AIDA_COMPETE_JUDGE` env override.
    pub fn default_binary(self) -> &'static str {
        match self {
            JudgeVendor::Claude => "claude",
            JudgeVendor::Codex => "codex",
        }
    }

    /// Whether the judge process needs stdin closed. Codex `exec` reads stdin
    /// and will hang waiting for it in a non-interactive run, so the caller must
    /// redirect `</dev/null` (`Stdio::null()`); claude `-p` does not.
    pub fn needs_stdin_closed(self) -> bool {
        matches!(self, JudgeVendor::Codex)
    }
}

/// Build the full judge argv (binary + args) for a vendor, with the prompt as
/// the final positional argument. The binary is `AIDA_COMPETE_JUDGE` if set
/// (parallels the vendor-binary override patterns), else the vendor default.
///
/// - claude: `claude -p --permission-mode bypassPermissions <prompt>`
/// - codex:  `codex exec --dangerously-bypass-approvals-and-sandbox <prompt>`
///
/// Pure + unit-tested; the I/O (spawn, stdin-redirect, parse) is the caller's.
/// `binary_override` is the resolved `AIDA_COMPETE_JUDGE` value (or `None`).
/// trace:TASK-869 | ai:claude
pub fn judge_command(
    vendor: JudgeVendor,
    binary_override: Option<&str>,
    prompt: &str,
) -> (String, Vec<String>) {
    let binary = binary_override
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(vendor.default_binary())
        .to_string();
    let mut args: Vec<String> = match vendor {
        JudgeVendor::Claude => vec![
            "-p".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ],
        JudgeVendor::Codex => vec![
            "exec".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
        ],
    };
    args.push(prompt.to_string());
    (binary, args)
}

/// Build the rubric-judge prompt: the spec context + each candidate's diff, with
/// the strict JSON contract. Pure — the I/O (gathering diffs, spawning the
/// judge) is the caller's. `candidates` is `(vendor, diff_text)`.
/// trace:STORY-660 | ai:claude
pub fn build_judge_prompt(
    spec_id: &str,
    spec_context: &str,
    candidates: &[(String, String)],
) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "You are a neutral judge scoring competing implementations of {spec_id}. \
         Each candidate is a different vendor's diff for the SAME spec. Score each \
         candidate 1-5 on four axes — spec-adherence, correctness, simplicity, \
         test-coverage — then recommend ONE winner to ship with a one-line reason. \
         A smaller, focused diff that fully meets the spec should beat an \
         over-reaching one.\n\n"
    ));
    p.push_str("## Spec\n\n");
    p.push_str(spec_context);
    p.push_str("\n\n## Candidates\n\n");
    for (vendor, diff) in candidates {
        p.push_str(&format!(
            "### Candidate: {vendor}\n\n```diff\n{diff}\n```\n\n"
        ));
    }
    p.push_str("## Output\n\n");
    p.push_str(&format!(
        "Respond with ONLY a single JSON object (no prose, no code fence) of exactly this shape:\n{JUDGE_JSON_CONTRACT}\n"
    ));
    p
}

/// Parse the judge's raw stdout into a [`JudgeVerdict`]. Tolerant of a model
/// that wraps the JSON in prose or a ```json fence: we extract the first
/// balanced `{...}` object and parse it. Returns `None` if no valid verdict is
/// found. Pure + unit-tested. trace:STORY-660 | ai:claude
pub fn parse_judge_verdict(raw: &str) -> Option<JudgeVerdict> {
    let json = extract_first_json_object(raw)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let scores_arr = v.get("scores")?.as_array()?;
    let mut scores = Vec::new();
    for s in scores_arr {
        let vendor = s.get("vendor")?.as_str()?.to_string();
        let axis = |k: &str| -> Option<u8> {
            let n = s.get(k)?.as_u64()?;
            Some(n.clamp(1, 5) as u8)
        };
        scores.push(RubricScore {
            vendor,
            spec_adherence: axis("spec_adherence")?,
            correctness: axis("correctness")?,
            simplicity: axis("simplicity")?,
            test_coverage: axis("test_coverage")?,
        });
    }
    if scores.is_empty() {
        return None;
    }
    let winner = v
        .get("winner")
        .and_then(|w| w.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // Fall back to the highest rubric total if the judge omitted a winner.
            scores
                .iter()
                .max_by_key(|s| s.total())
                .map(|s| s.vendor.clone())
                .unwrap_or_default()
        });
    let reasoning = v
        .get("reasoning")
        .and_then(|r| r.as_str())
        .unwrap_or("(no reasoning given)")
        .trim()
        .to_string();
    Some(JudgeVerdict {
        scores,
        winner,
        reasoning,
    })
}

/// Extract the first balanced `{...}` JSON object from arbitrary text (the judge
/// may emit prose or a fenced block around it). Returns the substring including
/// the braces. trace:STORY-660 | ai:claude
fn extract_first_json_object(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let c = b as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Render the judge verdict block for the report: a per-axis score table plus
/// the clearly-marked recommended winner. Pure + unit-tested. REPORT-ONLY — the
/// human/advisor still merges. trace:STORY-660 | ai:claude
pub fn render_judge_verdict(verdict: &JudgeVerdict) -> String {
    let mut out = String::new();
    out.push_str("Rubric judge (1-5 per axis; report-only, no auto-merge):\n");
    let vendor_w = verdict
        .scores
        .iter()
        .map(|s| s.vendor.len())
        .max()
        .unwrap_or(6)
        .max("vendor".len());
    out.push_str(&format!(
        "  {:<vw$}  {:>4}  {:>4}  {:>4}  {:>4}  {:>5}\n",
        "vendor",
        "spec",
        "corr",
        "simp",
        "test",
        "total",
        vw = vendor_w,
    ));
    // Show scores ordered by total descending so the strongest is on top.
    let mut ordered: Vec<&RubricScore> = verdict.scores.iter().collect();
    ordered.sort_by(|a, b| b.total().cmp(&a.total()).then(a.vendor.cmp(&b.vendor)));
    for s in ordered {
        out.push_str(&format!(
            "  {:<vw$}  {:>4}  {:>4}  {:>4}  {:>4}  {:>5}\n",
            s.vendor,
            s.spec_adherence,
            s.correctness,
            s.simplicity,
            s.test_coverage,
            s.total(),
            vw = vendor_w,
        ));
    }
    out
}

/// Render the full recommended-winner line with the resolved branch, given the
/// arm results (so the branch is accurate). Pure. trace:STORY-660 | ai:claude
pub fn render_recommended_winner(verdict: &JudgeVerdict, results: &[ArmResult]) -> String {
    let branch = results
        .iter()
        .find(|r| r.vendor == verdict.winner)
        .map(|r| r.branch.clone())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "branch unknown".to_string());
    format!(
        "recommended winner: {} ({}) — {}",
        verdict.winner, branch, verdict.reasoning
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs() -> ReportGlyphs {
        ReportGlyphs {
            check: "[x]".to_string(),
            cross: "[ ]".to_string(),
            pending: "( )".to_string(),
        }
    }

    #[test]
    fn vendor_adapter_maps_known_vendors() {
        assert!(matches!(
            vendor_adapter("claude"),
            Some(VendorAdapter::Headless {
                command: "claude",
                ..
            })
        ));
        assert!(matches!(
            vendor_adapter("codex"),
            Some(VendorAdapter::Headless {
                command: "codex",
                ..
            })
        ));
        assert_eq!(
            vendor_adapter("antigravity"),
            Some(VendorAdapter::HumanBriefed)
        );
        assert_eq!(vendor_adapter("nope"), None);
    }

    #[test]
    fn headless_argv_appends_brief_as_final_positional() {
        let adapter = vendor_adapter("claude").unwrap();
        let argv = headless_argv(&adapter, "do the thing").unwrap();
        assert_eq!(argv.first().unwrap(), "-p");
        assert_eq!(argv.last().unwrap(), "do the thing");
        assert!(argv.contains(&"bypassPermissions".to_string()));
    }

    #[test]
    fn headless_argv_codex_uses_exec_and_bypass() {
        let adapter = vendor_adapter("codex").unwrap();
        let argv = headless_argv(&adapter, "BRIEF").unwrap();
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--dangerously-bypass-approvals-and-sandbox");
        assert_eq!(argv.last().unwrap(), "BRIEF");
    }

    #[test]
    fn headless_argv_none_for_human_briefed() {
        assert!(headless_argv(&VendorAdapter::HumanBriefed, "x").is_none());
    }

    #[test]
    fn vendor_branch_is_namespaced_and_lowercased() {
        assert_eq!(
            vendor_branch("STORY-659", "claude"),
            "compete/story-659-claude"
        );
        assert_eq!(vendor_branch("BUG-12", "codex"), "compete/bug-12-codex");
    }

    #[test]
    fn parse_gate_result_pass() {
        let (built, gate) = parse_gate_result(true, "ignored");
        assert!(built);
        assert!(gate);
    }

    #[test]
    fn parse_gate_result_build_failure() {
        let (built, gate) = parse_gate_result(
            false,
            "error[E0599]: no method named foo\nerror: could not compile `aida-cli`",
        );
        assert!(!built);
        assert!(!gate);
    }

    #[test]
    fn parse_gate_result_test_failure_still_built() {
        let (built, gate) =
            parse_gate_result(false, "test some_test ... FAILED\ntest result: FAILED");
        assert!(built, "build clause passed; only tests failed");
        assert!(!gate);
    }

    #[test]
    fn count_diff_lines_sums_added_and_removed_skips_binary() {
        let numstat = "10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\n-\t-\timg.png\n";
        assert_eq!(count_diff_lines(numstat), 17);
    }

    #[test]
    fn count_diff_lines_empty_is_zero() {
        assert_eq!(count_diff_lines(""), 0);
    }

    #[test]
    fn render_report_has_header_row_and_one_line_per_arm() {
        let results = vec![
            ArmResult {
                vendor: "codex".to_string(),
                ran: Ran::Ok,
                built: Some(true),
                gate_passed: Some(true),
                diff_lines: Some(42),
                branch: "compete/story-659-codex".to_string(),
            },
            ArmResult {
                vendor: "claude".to_string(),
                ran: Ran::Failed,
                built: Some(true),
                gate_passed: Some(false),
                diff_lines: Some(7),
                branch: "compete/story-659-claude".to_string(),
            },
        ];
        let report = render_report("STORY-659", &results, &glyphs());
        assert!(report.contains("vendor"));
        assert!(report.contains("diff-lines"));
        assert!(report.contains("compete/story-659-codex"));
        assert!(report.contains("codex: gate PASS on branch compete/story-659-codex"));
        assert!(report.contains("claude: vendor run errored — gate FAIL"));
    }

    #[test]
    fn verdict_distinguishes_skipped_and_briefed() {
        let results = vec![
            ArmResult {
                vendor: "gemini".to_string(),
                ran: Ran::Skipped,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            },
            ArmResult {
                vendor: "antigravity".to_string(),
                ran: Ran::Briefed,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            },
        ];
        let lines = verdict_lines(&results);
        assert!(lines[0].contains("skipped — CLI not found"));
        assert!(lines[1].contains("human-run arm — brief emitted"));
    }

    #[test]
    fn default_gate_unsets_session_role() {
        assert!(DEFAULT_GATE.contains("env -u AIDA_SESSION_ROLE"));
        assert!(DEFAULT_GATE.contains("cargo build"));
    }

    // The default gate must MIRROR PR CI so a gate-passing arm is mergeable:
    // a build+test-only gate gives false-green winners that fail CI at
    // `cargo fmt --check` / clippy / glyph-lint (BUG-576). trace:BUG-576 | ai:claude
    #[test]
    fn default_gate_mirrors_ci_surface() {
        // fmt-check (the BUG-576 false-green step)
        assert!(
            DEFAULT_GATE.contains("cargo fmt --all -- --check"),
            "default gate must run fmt --check: {DEFAULT_GATE}"
        );
        // build + test
        assert!(DEFAULT_GATE.contains("cargo build"));
        assert!(DEFAULT_GATE.contains("cargo test -p aida-cli"));
        // clippy correctness lint (CI gate)
        assert!(
            DEFAULT_GATE.contains("cargo clippy -- -D clippy::correctness"),
            "default gate must run clippy correctness: {DEFAULT_GATE}"
        );
        // glyph-lint guard (CI gate)
        assert!(
            DEFAULT_GATE.contains("bash scripts/glyph-lint.sh --block"),
            "default gate must run glyph-lint: {DEFAULT_GATE}"
        );
        // every step is &&-chained so the gate is a single fail-fast command
        assert_eq!(DEFAULT_GATE.matches("&&").count(), 4);
    }

    // ---- STORY-660: judge + deterministic ranking ----

    fn arm(vendor: &str, gate: Option<bool>, diff: Option<usize>) -> ArmResult {
        ArmResult {
            vendor: vendor.to_string(),
            ran: Ran::Ok,
            built: Some(true),
            gate_passed: gate,
            diff_lines: diff,
            branch: format!("compete/x-{vendor}"),
        }
    }

    #[test]
    fn deterministic_ranking_smaller_diff_first_and_excludes_failures() {
        let results = vec![
            arm("codex", Some(true), Some(8178)),
            arm("claude", Some(true), Some(190)),
            arm("gemini", Some(false), Some(5)), // gate-fail: not a winner candidate
        ];
        let files = vec![
            ("codex".to_string(), 3usize),
            ("claude".to_string(), 2usize),
        ];
        let ranked = deterministic_ranking(&results, &files);
        assert_eq!(ranked.len(), 2, "gate-failing arm excluded");
        assert_eq!(ranked[0].vendor, "claude", "smaller diff ranks first");
        assert_eq!(ranked[0].diff_lines, 190);
        assert_eq!(ranked[0].files_touched, 2);
        assert_eq!(ranked[1].vendor, "codex");
    }

    #[test]
    fn deterministic_ranking_ties_break_on_files_then_name() {
        let results = vec![
            arm("zeta", Some(true), Some(100)),
            arm("alpha", Some(true), Some(100)),
            arm("beta", Some(true), Some(100)),
        ];
        let files = vec![
            ("zeta".to_string(), 5usize),
            ("alpha".to_string(), 3usize),
            ("beta".to_string(), 3usize),
        ];
        let ranked = deterministic_ranking(&results, &files);
        // alpha & beta tie on lines+files → name order; zeta last (more files).
        assert_eq!(ranked[0].vendor, "alpha");
        assert_eq!(ranked[1].vendor, "beta");
        assert_eq!(ranked[2].vendor, "zeta");
    }

    #[test]
    fn deterministic_ranking_empty_when_no_gate_passers() {
        let results = vec![arm("codex", Some(false), Some(5))];
        assert!(deterministic_ranking(&results, &[]).is_empty());
    }

    #[test]
    fn render_deterministic_ranking_marks_smallest() {
        let ranked = deterministic_ranking(
            &[
                arm("codex", Some(true), Some(8178)),
                arm("claude", Some(true), Some(190)),
            ],
            &[
                ("codex".to_string(), 3usize),
                ("claude".to_string(), 2usize),
            ],
        );
        let out = render_deterministic_ranking(&ranked);
        assert!(out.contains("1. claude (190 lines, 2 files) <- smallest"));
        assert!(out.contains("2. codex (8178 lines, 3 files)"));
    }

    #[test]
    fn render_deterministic_ranking_empty_is_honest() {
        assert!(render_deterministic_ranking(&[]).contains("no gate-passing arms"));
    }

    #[test]
    fn build_judge_prompt_includes_spec_diffs_and_contract() {
        let prompt = build_judge_prompt(
            "STORY-660",
            "the spec body",
            &[
                ("claude".to_string(), "diff a".to_string()),
                ("codex".to_string(), "diff b".to_string()),
            ],
        );
        assert!(prompt.contains("STORY-660"));
        assert!(prompt.contains("the spec body"));
        assert!(prompt.contains("Candidate: claude"));
        assert!(prompt.contains("diff a"));
        assert!(prompt.contains("Candidate: codex"));
        assert!(prompt.contains(JUDGE_JSON_CONTRACT));
    }

    #[test]
    fn parse_judge_verdict_clean_json() {
        let raw = r#"{"scores":[{"vendor":"claude","spec_adherence":5,"correctness":5,"simplicity":4,"test_coverage":5},{"vendor":"codex","spec_adherence":4,"correctness":3,"simplicity":3,"test_coverage":2}],"winner":"claude","reasoning":"reused canonical paths, smaller diff"}"#;
        let v = parse_judge_verdict(raw).expect("parse");
        assert_eq!(v.scores.len(), 2);
        assert_eq!(v.winner, "claude");
        assert!(v.reasoning.contains("reused canonical"));
        let claude = v.scores.iter().find(|s| s.vendor == "claude").unwrap();
        assert_eq!(claude.total(), 19);
    }

    #[test]
    fn parse_judge_verdict_tolerates_prose_and_fence() {
        let raw = "Here is my verdict:\n```json\n{\"scores\":[{\"vendor\":\"a\",\"spec_adherence\":3,\"correctness\":3,\"simplicity\":3,\"test_coverage\":3}],\"winner\":\"a\",\"reasoning\":\"ok\"}\n```\nThanks!";
        let v = parse_judge_verdict(raw).expect("parse around prose");
        assert_eq!(v.winner, "a");
        assert_eq!(v.scores.len(), 1);
    }

    #[test]
    fn parse_judge_verdict_clamps_out_of_range_scores() {
        let raw = r#"{"scores":[{"vendor":"a","spec_adherence":9,"correctness":0,"simplicity":3,"test_coverage":3}],"winner":"a","reasoning":"x"}"#;
        let v = parse_judge_verdict(raw).unwrap();
        let a = &v.scores[0];
        assert_eq!(a.spec_adherence, 5, "9 clamps to 5");
        assert_eq!(a.correctness, 1, "0 clamps to 1");
    }

    #[test]
    fn parse_judge_verdict_falls_back_to_highest_total_when_winner_missing() {
        let raw = r#"{"scores":[{"vendor":"a","spec_adherence":2,"correctness":2,"simplicity":2,"test_coverage":2},{"vendor":"b","spec_adherence":5,"correctness":5,"simplicity":5,"test_coverage":5}],"reasoning":"b stronger"}"#;
        let v = parse_judge_verdict(raw).unwrap();
        assert_eq!(
            v.winner, "b",
            "highest rubric total wins when winner omitted"
        );
    }

    #[test]
    fn parse_judge_verdict_rejects_garbage() {
        assert!(parse_judge_verdict("not json at all").is_none());
        assert!(parse_judge_verdict("{\"scores\":[]}").is_none());
    }

    #[test]
    fn render_judge_verdict_has_table_and_winner() {
        let verdict = JudgeVerdict {
            scores: vec![
                RubricScore {
                    vendor: "claude".to_string(),
                    spec_adherence: 5,
                    correctness: 5,
                    simplicity: 4,
                    test_coverage: 5,
                },
                RubricScore {
                    vendor: "codex".to_string(),
                    spec_adherence: 4,
                    correctness: 3,
                    simplicity: 3,
                    test_coverage: 2,
                },
            ],
            winner: "claude".to_string(),
            reasoning: "smaller, correct diff".to_string(),
        };
        let out = render_judge_verdict(&verdict);
        assert!(out.contains("vendor"));
        assert!(out.contains("total"));
        // claude (total 19) should appear before codex (total 12).
        let ci = out.find("claude").unwrap();
        let xi = out.find("codex").unwrap();
        assert!(ci < xi, "stronger arm listed first");
    }

    #[test]
    fn render_recommended_winner_resolves_branch() {
        let verdict = JudgeVerdict {
            scores: vec![RubricScore {
                vendor: "claude".to_string(),
                spec_adherence: 5,
                correctness: 5,
                simplicity: 5,
                test_coverage: 5,
            }],
            winner: "claude".to_string(),
            reasoning: "best".to_string(),
        };
        let results = vec![arm("claude", Some(true), Some(190))];
        let line = render_recommended_winner(&verdict, &results);
        assert_eq!(line, "recommended winner: claude (compete/x-claude) — best");
    }

    // ---- TASK-869: cross-vendor judge ----
    #[test]
    fn judge_vendor_parse_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(JudgeVendor::parse("claude"), Some(JudgeVendor::Claude));
        assert_eq!(JudgeVendor::parse("Codex"), Some(JudgeVendor::Codex));
        assert_eq!(JudgeVendor::parse("  CODEX  "), Some(JudgeVendor::Codex));
        assert_eq!(JudgeVendor::parse("gemini"), None);
        assert_eq!(JudgeVendor::parse(""), None);
        assert_eq!(JudgeVendor::default(), JudgeVendor::Claude);
    }

    #[test]
    fn judge_command_claude_uses_claude_p_flags() {
        let (bin, args) = judge_command(JudgeVendor::Claude, None, "JUDGE PROMPT");
        assert_eq!(bin, "claude");
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
                "JUDGE PROMPT".to_string(),
            ]
        );
        assert!(!JudgeVendor::Claude.needs_stdin_closed());
    }

    #[test]
    fn judge_command_codex_uses_exec_bypass_and_needs_stdin_closed() {
        let (bin, args) = judge_command(JudgeVendor::Codex, None, "JUDGE PROMPT");
        assert_eq!(bin, "codex");
        assert_eq!(
            args,
            vec![
                "exec".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "JUDGE PROMPT".to_string(),
            ]
        );
        // codex exec reads stdin → must be closed in a non-interactive run.
        assert!(JudgeVendor::Codex.needs_stdin_closed());
    }

    #[test]
    fn judge_command_honors_binary_override() {
        let (bin, args) = judge_command(JudgeVendor::Codex, Some("/opt/bin/codex-wrap"), "P");
        assert_eq!(bin, "/opt/bin/codex-wrap");
        // override swaps the binary only — the exec args are unchanged.
        assert_eq!(args[0], "exec");
        // an empty/whitespace override falls back to the vendor default.
        let (bin2, _) = judge_command(JudgeVendor::Claude, Some("   "), "P");
        assert_eq!(bin2, "claude");
    }

    #[test]
    fn judge_prompt_is_vendor_independent() {
        // The rubric prompt must NOT depend on which vendor renders it — only
        // the executing model changes. trace:TASK-869
        let candidates = vec![("claude".to_string(), "diff a".to_string())];
        let prompt = build_judge_prompt("TASK-869", "spec ctx", &candidates);
        let (_, claude_args) = judge_command(JudgeVendor::Claude, None, &prompt);
        let (_, codex_args) = judge_command(JudgeVendor::Codex, None, &prompt);
        // Both carry the identical prompt as the final positional arg.
        assert_eq!(claude_args.last(), codex_args.last());
    }
}
