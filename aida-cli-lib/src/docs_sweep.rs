//! `aida docs sweep`: the background documentation sweep (STORY-2220).
//!
//! The ADR-5 CI drift guards catch doc drift at the moment a surface changes.
//! This sweep catches the drift that has already built up. It reviews the
//! documentation touched by changes merged since a reference revision (by
//! default, the revision the last sweep recorded) and reports findings, each
//! with a file, a line, the evidence, and a proposed fix:
//!
//! - **Stale command text.** An `aida …` invocation in a code span or code
//!   block that names a subcommand or long flag this build does not have. A
//!   hidden (deprecated) alias is reported too, with its canonical name as a
//!   mechanical fix.
//! - **Glossary gaps.** A term a website page defines inline (`<dfn>`,
//!   `<dt>`, or a `<strong>Term.</strong>` lead-in) that has no entry in the
//!   glossary source.
//!
//! The scheduled `docs-sweep` job (`aida schedule`, idle-only through its
//! `while` gate) runs it daily. `[docs_sweep] mode` picks what a run does:
//! `report` (the default) sends one mail per run to the product seat; `pr`
//! batches every mechanical fix into at most one documentation-only PR,
//! reusing a sweep PR that is still open, and files a draft spec for each
//! finding that is not mechanically fixable. A run never edits code, tests,
//! spec status, or generated files, and edits at most `max_files` files; the
//! rest is reported.
//!
//! trace:STORY-2220 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Per-clone sweep state: the last swept revision and the last run's outcome.
pub(crate) const STATE_FILE: &str = ".aida/docs-sweep-state.json";
/// The one branch every sweep PR lives on, so a run can find an open one.
pub(crate) const SWEEP_BRANCH: &str = "docs-sweep/batch";
/// Tag on every spec a sweep files.
const SWEEP_TAG: &str = "docs-sweep";
const DEFAULT_MAX_FILES: usize = 20;
const DEFAULT_WEBSITE_DIR: &str = "docs/showcase";
const DEFAULT_RECIPIENT: &str = "product";
/// Glossary sources, first existing wins: the template master in AIDA's own
/// repository, then the scaffolded copy every downstream project carries.
const GLOSSARY_CANDIDATES: &[&str] = &[
    "aida-core/templates/.aida/discipline/machinery-glossary.md",
    ".aida/discipline/machinery-glossary.md",
];
/// How many findings one report mail lists before it points at the command.
const MAIL_LIST_LIMIT: usize = 40;

// ---------------------------------------------------------------------------
// Config and state
// ---------------------------------------------------------------------------

/// `[docs_sweep] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SweepMode {
    /// Findings go to the product seat; nothing is edited.
    #[default]
    Report,
    /// Mechanical fixes go into one batched documentation-only PR.
    Pr,
}

impl SweepMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SweepMode::Report => "report",
            SweepMode::Pr => "pr",
        }
    }
}

/// `[docs_sweep]` in `.aida/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SweepConfig {
    pub mode: SweepMode,
    /// Per-run cap on files a PR-mode run changes.
    pub max_files: usize,
    /// Directory holding the website pages.
    pub website_dir: String,
    /// Seat (role) the report mail goes to.
    pub recipient: String,
    /// Commands run in the sweep worktree after edits, to regenerate files
    /// derived from an edited source (e.g. `make sync-templates`).
    pub regenerate: Vec<String>,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            mode: SweepMode::Report,
            max_files: DEFAULT_MAX_FILES,
            website_dir: DEFAULT_WEBSITE_DIR.to_string(),
            recipient: DEFAULT_RECIPIENT.to_string(),
            regenerate: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawSweepConfig {
    mode: Option<String>,
    max_files: Option<usize>,
    website_dir: Option<String>,
    recipient: Option<String>,
    regenerate: Option<Vec<String>>,
}

/// Parse the `[docs_sweep]` table out of a config body. An unknown `mode` is
/// an error rather than a silent fallback, so a typo cannot turn PR mode off
/// (or on) without anyone noticing.
pub(crate) fn parse_config(body: &str) -> Result<SweepConfig> {
    #[derive(Deserialize)]
    struct File {
        docs_sweep: Option<RawSweepConfig>,
    }
    let file: File = toml::from_str(body).context("parsing the project config")?;
    let raw = file.docs_sweep.unwrap_or_default();
    let mut cfg = SweepConfig::default();
    if let Some(mode) = raw.mode {
        cfg.mode = match mode.trim() {
            "report" => SweepMode::Report,
            "pr" => SweepMode::Pr,
            other => anyhow::bail!("unknown [docs_sweep] mode '{other}' (expected report or pr)"),
        };
    }
    if let Some(n) = raw.max_files {
        cfg.max_files = n;
    }
    if let Some(dir) = raw.website_dir.filter(|d| !d.trim().is_empty()) {
        cfg.website_dir = dir.trim().trim_end_matches('/').to_string();
    }
    if let Some(r) = raw.recipient.filter(|r| !r.trim().is_empty()) {
        cfg.recipient = r.trim().to_string();
    }
    if let Some(cmds) = raw.regenerate {
        cfg.regenerate = cmds;
    }
    Ok(cfg)
}

/// `[docs_sweep]` is read from the default branch's copy of the project
/// config, never the branch-local file: `regenerate` runs shell commands, so
/// a branch must not be able to set it (BUG-1723). The table's other keys come
/// from the same copy so it stays one unit. Local refs only, so a scheduled
/// run makes no forge call just to read its settings.
// trace:STORY-2220 | ai:claude
pub(crate) fn load_config(project_root: &Path) -> Result<SweepConfig> {
    match crate::trusted_config::read_trusted_config_toml_local(project_root) {
        Some(body) => parse_config(&body),
        None => Ok(SweepConfig::default()),
    }
}

/// What the last recorded sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SweepState {
    /// The revision the last recorded sweep reviewed up to.
    pub last_swept_rev: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    /// Findings the last run produced.
    pub findings: usize,
    pub mode: Option<SweepMode>,
    /// The open sweep PR, when the last run opened or extended one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// The spec every commit on the sweep PR trails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_spec: Option<String>,
    /// Keys of the findings the last run saw, so a run mails only what is
    /// new rather than the same backlog every day.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reported: Vec<String>,
}

pub(crate) fn load_state(project_root: &Path) -> SweepState {
    std::fs::read_to_string(project_root.join(STATE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_state(project_root: &Path, state: &SweepState) -> Result<()> {
    let path = project_root.join(STATE_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(state)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

/// One line for `aida schedule status`: the last run, its findings count,
/// and the mode. `None` before the first recorded sweep.
pub(crate) fn status_detail(project_root: &Path) -> Option<String> {
    let state = load_state(project_root);
    let at = state.last_run_at?;
    let mode = state
        .mode
        .unwrap_or_else(|| load_config(project_root).unwrap_or_default().mode);
    let rev = state
        .last_swept_rev
        .as_deref()
        .map(|r| format!(" · swept to {}", &r[..r.len().min(10)]))
        .unwrap_or_default();
    let pr = state
        .pr_url
        .as_deref()
        .map(|u| format!(" · PR {u}"))
        .unwrap_or_default();
    Some(format!(
        "last sweep {} · {} finding{} · mode {}{rev}{pr}",
        at.format("%Y-%m-%d %H:%M UTC"),
        state.findings,
        if state.findings == 1 { "" } else { "s" },
        mode.as_str()
    ))
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FindingKind {
    StaleCommand,
    GlossaryGap,
}

/// A mechanical fix a PR-mode run may apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Fix {
    /// Replace `old` with `new` on the finding's line.
    Replace { old: String, new: String },
    /// Add a stub entry for `term` to the glossary `source`.
    GlossaryStub { source: String, term: String },
}

impl Fix {
    /// The file this fix edits.
    fn target<'a>(&'a self, finding_file: &'a str) -> &'a str {
        match self {
            Fix::Replace { .. } => finding_file,
            Fix::GlossaryStub { source, .. } => source,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub kind: FindingKind,
    pub file: String,
    pub line: usize,
    pub evidence: String,
    pub proposed_fix: String,
    /// Present only when the fix is mechanical and the file may be edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

impl Finding {
    /// A stable key, so a rerun does not file the same draft spec twice.
    fn key(&self) -> String {
        use sha2::{Digest, Sha256};
        // Line numbers and commit evidence move as the doc and history change;
        // the file, kind and proposed fix identify the same problem.
        let mut h = Sha256::new();
        h.update(format!(
            "{:?}\n{}\n{}",
            self.kind, self.file, self.proposed_fix
        ));
        let digest = h.finalize();
        digest[..6].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The key the sweep state remembers: `<file>#<key>`, so a run can forget
    /// exactly the findings of the files it reviewed again.
    fn state_key(&self) -> String {
        format!("{}#{}", self.file, self.key())
    }
}

/// The reported set after a run: what it found, plus what earlier runs
/// found in files this run did not review (an incremental run says nothing
/// about them, so a clean one must not make their findings new again).
pub(crate) fn next_reported(
    previous: &[String],
    scope: &[String],
    findings: &[Finding],
) -> Vec<String> {
    let scanned: BTreeSet<&str> = scope.iter().map(String::as_str).collect();
    let mut out: BTreeSet<String> = previous
        .iter()
        .filter(|k| {
            k.rsplit_once('#')
                .is_some_and(|(file, _)| !scanned.contains(file))
        })
        .cloned()
        .collect();
    out.extend(findings.iter().map(Finding::state_key));
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Whether a repository path is documentation the sweep reviews: the CLI
/// docs, the website pages, the glossary sources, and READMEs.
pub(crate) fn in_scope(path: &str, website_dir: &str) -> bool {
    if path.starts_with("target/")
        || path.contains("/node_modules/")
        || path.starts_with(".claude/")
    {
        return false;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    (path.starts_with("docs/cli/") && path.ends_with(".md"))
        || (path.starts_with(&format!("{website_dir}/")) && path.ends_with(".html"))
        || file_name.eq_ignore_ascii_case("README.md")
        || GLOSSARY_CANDIDATES.contains(&path)
}

/// A change to one of these files can make any command reference stale.
pub(crate) fn is_cli_surface(path: &str) -> bool {
    path.ends_with("_args.rs") || path.ends_with("command_registry.rs") || path.ends_with("/cli.rs")
}

/// The files to review. With no prior sweep (`changed = None`) that is every
/// tracked in-scope file; after one, the in-scope files changed since it,
/// widening to every in-scope file when the CLI surface itself changed.
pub(crate) fn select_scope(
    tracked: &[String],
    changed: Option<&[String]>,
    website_dir: &str,
) -> Vec<String> {
    let all = || -> Vec<String> {
        tracked
            .iter()
            .filter(|p| in_scope(p, website_dir))
            .cloned()
            .collect()
    };
    match changed {
        None => all(),
        Some(changed) if changed.iter().any(|p| is_cli_surface(p)) => all(),
        Some(changed) => {
            let tracked: BTreeSet<&String> = tracked.iter().collect();
            changed
                .iter()
                .filter(|p| in_scope(p, website_dir) && tracked.contains(p))
                .cloned()
                .collect()
        }
    }
}

/// A file that says it is generated is never edited; its fix belongs at the
/// source it is generated from.
pub(crate) fn is_generated(text: &str) -> bool {
    text.lines().take(20).any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("generated file") || l.contains("do not edit by hand")
    })
}

// ---------------------------------------------------------------------------
// Stale command text
// ---------------------------------------------------------------------------

/// Pull every `aida …` invocation out of one line, with `fenced` saying
/// whether the line sits inside a code block (where a bare `aida …` or
/// `$ aida …` line counts).
pub(crate) fn extract_invocations(line: &str, fenced: bool) -> Vec<String> {
    let mut out = Vec::new();
    let decoded = decode_html(&strip_tags_keep_code(line));
    // Inline code spans: `aida …`.
    let mut rest = decoded.as_str();
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let span = after[..end].trim();
        if let Some(inv) = as_invocation(span) {
            out.push(inv);
        }
        rest = &after[end + 1..];
    }
    // HTML <code>…</code> spans.
    let mut rest = line;
    while let Some(start) = rest.find("<code") {
        let Some(open_end) = rest[start..].find('>') else {
            break;
        };
        let body_start = start + open_end + 1;
        let Some(close) = rest[body_start..].find("</code>") else {
            break;
        };
        let span = decode_html(&strip_all_tags(&rest[body_start..body_start + close]));
        if let Some(inv) = as_invocation(span.trim()) {
            out.push(inv);
        }
        rest = &rest[body_start + close..];
    }
    if fenced && out.is_empty() {
        let plain = decode_html(&strip_all_tags(line));
        let t = plain.trim();
        let t = t.strip_prefix("$ ").unwrap_or(t);
        if let Some(inv) = as_invocation(t) {
            out.push(inv);
        }
    }
    out
}

fn as_invocation(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = s.strip_prefix("aida ")?;
    let first = rest.split_whitespace().next()?;
    // `aida` followed by a flag only (`aida --version`) or a placeholder
    // names no subcommand to check.
    if first.starts_with('-') || is_placeholder(first) {
        return None;
    }
    Some(s.to_string())
}

fn strip_all_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The text to scan for backtick spans: a line carrying HTML `<code>` spans
/// is scanned through those instead, so it is not scanned twice.
fn strip_tags_keep_code(s: &str) -> String {
    if s.contains("<code") {
        String::new()
    } else {
        s.to_string()
    }
}

fn decode_html(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// A token that stands for something (`<SPEC-ID>`, `[flags]`, `$VAR`,
/// `NAME`, `…`) rather than naming a real subcommand.
fn is_placeholder(tok: &str) -> bool {
    !tok.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        || tok.chars().any(|c| "<>[]{}$*…\"'()|=/".contains(c))
        || tok.contains("...")
        || (tok.chars().any(|c| c.is_ascii_alphabetic())
            && tok
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '-' || c == '_' || c.is_ascii_digit()))
}

fn is_stop(tok: &str) -> bool {
    matches!(
        tok,
        "|" | "||" | "&&" | ";" | "#" | ">" | ">>" | "<" | "\\" | "2>&1" | "&"
    ) || tok.starts_with('#')
        || tok.starts_with('>')
        || tok.ends_with(';')
}

/// What is wrong with one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandProblem {
    UnknownSubcommand {
        /// The valid command path before the unknown token (`aida queue`).
        path: String,
        token: String,
        suggestion: Option<String>,
    },
    UnknownFlag {
        path: String,
        flag: String,
        suggestion: Option<String>,
    },
    /// A hidden (deprecated) alias: `old` should read `new`.
    HiddenAlias { old: String, new: String },
}

/// Check one invocation against the CLI tree. `root` must be built
/// (`Command::build`) so global flags have propagated to every subcommand.
pub(crate) fn check_invocation(root: &clap::Command, invocation: &str) -> Option<CommandProblem> {
    let words = split_words(invocation);
    if words.first().map(String::as_str) != Some("aida") {
        return None;
    }
    // `aida help …` is answered before clap (`aida help --all`), so it has
    // no clap surface to check.
    if words.get(1).map(String::as_str) == Some("help") {
        return None;
    }
    // Check what dispatch actually parses: the built-in pre-clap aliases
    // (`aida mylist`, `aida list <lens>`, bare `aida queue`) rewrite first.
    let (rewritten, _) = crate::builtin_alias_rewrites(&words);
    let mut tokens = rewritten.iter().map(String::as_str).skip(1);
    let mut cmd = root;
    let mut path = vec!["aida".to_string()];
    let mut canonical = vec!["aida".to_string()];
    let mut alias_hit = false;
    let mut walking = true;
    let mut skip_value = false;
    for raw in tokens {
        if is_stop(raw) {
            break;
        }
        let tok = raw.trim_end_matches([',', '.', ':']);
        if skip_value {
            skip_value = false;
            continue;
        }
        if let Some(long) = tok.strip_prefix("--") {
            let (name, has_inline_value) = match long.split_once('=') {
                Some((n, _)) => (n, true),
                None => (long, false),
            };
            if name.is_empty() {
                break; // `--` ends option parsing
            }
            if is_placeholder(name) {
                continue;
            }
            match find_long(cmd, name) {
                Some(arg) => {
                    if !has_inline_value && arg.get_action().takes_values() {
                        skip_value = true;
                    }
                }
                None => {
                    let candidates: Vec<String> = cmd
                        .get_arguments()
                        .filter_map(|a| a.get_long().map(str::to_string))
                        .collect();
                    return Some(CommandProblem::UnknownFlag {
                        path: path.join(" "),
                        flag: format!("--{name}"),
                        suggestion: nearest(name, &candidates).map(|s| format!("--{s}")),
                    });
                }
            }
            continue;
        }
        if tok.starts_with('-') {
            // A short flag: its value (if any) is ambiguous, so stop walking
            // subcommands rather than misreading a value as one.
            walking = false;
            continue;
        }
        if !walking || !cmd.has_subcommands() {
            walking = false;
            continue;
        }
        if is_placeholder(tok) || tok.is_empty() {
            walking = false;
            continue;
        }
        match cmd.find_subcommand(tok) {
            Some(sub) => {
                let visible = sub.get_name() == tok || sub.get_visible_aliases().any(|a| a == tok);
                if !visible {
                    alias_hit = true;
                }
                path.push(tok.to_string());
                canonical.push(sub.get_name().to_string());
                cmd = sub;
            }
            None => {
                if cmd.get_positionals().next().is_some() {
                    walking = false;
                    continue;
                }
                let candidates: Vec<String> = cmd
                    .get_subcommands()
                    .map(|s| s.get_name().to_string())
                    .collect();
                return Some(CommandProblem::UnknownSubcommand {
                    path: path.join(" "),
                    token: tok.to_string(),
                    suggestion: nearest(tok, &candidates)
                        .map(|s| format!("{} {s}", path.join(" "))),
                });
            }
        }
    }
    alias_hit.then(|| CommandProblem::HiddenAlias {
        old: path.join(" "),
        new: canonical.join(" "),
    })
}

/// Split like a shell would for checking purposes: a quoted argument stays
/// one token (and keeps its quotes, so it reads as a placeholder).
fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn find_long<'a>(cmd: &'a clap::Command, name: &str) -> Option<&'a clap::Arg> {
    cmd.get_arguments().find(|a| {
        a.get_long() == Some(name) || a.get_all_aliases().is_some_and(|al| al.contains(&name))
    })
}

/// The closest candidate within edit distance 2 (and at most half the word).
fn nearest(word: &str, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .map(|c| (edit_distance(word, c), c))
        .filter(|(d, _)| *d <= 2 && *d * 2 <= word.len())
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c.clone())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Evidence about what changed since the reference revision.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChangeEvidence {
    /// The reference revision, when there is one.
    pub since: Option<String>,
    /// The latest commit since `since` that touched the CLI surface.
    pub cli_commit: Option<String>,
    /// Per doc file, the latest commit since `since` that touched it.
    pub doc_commits: BTreeMap<String, String>,
}

impl ChangeEvidence {
    fn suffix(&self, file: &str) -> String {
        let mut parts = Vec::new();
        if let Some(c) = self.doc_commits.get(file) {
            parts.push(format!("doc last changed in {c}"));
        }
        if let (Some(since), Some(c)) = (&self.since, &self.cli_commit) {
            parts.push(format!(
                "CLI surface changed since {} (latest: {c})",
                short(since)
            ));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("; {}", parts.join("; "))
        }
    }
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(10)]
}

/// Review one file's command references.
pub(crate) fn command_findings(
    root: &clap::Command,
    file: &str,
    text: &str,
    evidence: &ChangeEvidence,
) -> Vec<Finding> {
    let generated = is_generated(text);
    let is_html = file.ends_with(".html");
    let mut out = Vec::new();
    let mut fenced = false;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !is_html && trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let opens_pre = is_html && line.contains("<pre");
        let closes_pre = is_html && line.contains("</pre>");
        let in_block = fenced || opens_pre;
        for inv in extract_invocations(line, in_block) {
            let Some(problem) = check_invocation(root, &inv) else {
                continue;
            };
            let (evidence_text, proposed, fix) = describe(&problem, &inv);
            if !seen.insert(format!("{}:{evidence_text}", idx)) {
                continue;
            }
            let (proposed, fix) = if generated {
                (
                    format!("{proposed} (generated file: fix it at its source and regenerate)"),
                    None,
                )
            } else {
                (proposed, fix)
            };
            out.push(Finding {
                kind: FindingKind::StaleCommand,
                file: file.to_string(),
                line: idx + 1,
                evidence: format!("{evidence_text}{}", evidence.suffix(file)),
                proposed_fix: proposed,
                fix,
            });
        }
        if is_html {
            if opens_pre && !closes_pre {
                fenced = true;
            } else if closes_pre {
                fenced = false;
            }
        }
    }
    out
}

fn describe(problem: &CommandProblem, inv: &str) -> (String, String, Option<Fix>) {
    match problem {
        CommandProblem::UnknownSubcommand {
            path,
            token,
            suggestion,
        } => (
            format!("`{inv}`: `{path}` has no subcommand `{token}` in this build"),
            match suggestion {
                Some(s) => format!("replace with `{s}` if that is what is meant"),
                None => format!(
                    "update or remove the reference (`{path} --help` lists the subcommands)"
                ),
            },
            None,
        ),
        CommandProblem::UnknownFlag {
            path,
            flag,
            suggestion,
        } => (
            format!("`{inv}`: `{path}` has no flag `{flag}` in this build"),
            match suggestion {
                Some(s) => format!("replace `{flag}` with `{s}` if that is what is meant"),
                None => format!("update or remove the flag (`{path} --help` lists the flags)"),
            },
            None,
        ),
        CommandProblem::HiddenAlias { old, new } => (
            format!("`{inv}`: `{old}` is a deprecated spelling of `{new}`"),
            format!("replace `{old}` with `{new}`"),
            Some(Fix::Replace {
                old: old.clone(),
                new: new.clone(),
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Glossary gaps
// ---------------------------------------------------------------------------

/// Normalize a term for comparison: lower case, no parenthetical, no
/// trailing punctuation, single spaces.
pub(crate) fn normalize_term(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim()
        .trim_end_matches(['.', ':', ','])
        .trim_matches('`')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Every term a glossary file defines: its `###` headings and its bolded
/// list-item lead terms (the "adjacent terms" shape).
pub(crate) fn glossary_terms_from(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("### ") {
            out.insert(normalize_term(h));
        } else if let Some(rest) = t.strip_prefix("- **").or_else(|| t.strip_prefix("* **")) {
            if let Some(end) = rest.find("**") {
                for part in rest[..end].split(['/', ',']) {
                    out.insert(normalize_term(part));
                }
            }
        }
    }
    out.retain(|t| !t.is_empty());
    out
}

fn known_term(known: &BTreeSet<String>, term: &str) -> bool {
    let n = normalize_term(term);
    known.contains(&n)
        || n.strip_suffix('s').is_some_and(|s| known.contains(s))
        || known.contains(&format!("{n}s"))
        || known.iter().any(|k| k.split(" / ").any(|p| p == n))
}

/// Terms a website line defines inline.
pub(crate) fn inline_terms(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [("<dfn", "</dfn>"), ("<dt", "</dt>")] {
        let mut rest = line;
        while let Some(start) = rest.find(open) {
            let after_open = &rest[start + open.len()..];
            // `<dt` must not match `<dtx`; require `>` or whitespace next.
            if !after_open.starts_with('>') && !after_open.starts_with(' ') {
                rest = after_open;
                continue;
            }
            let Some(gt) = after_open.find('>') else {
                break;
            };
            let body = &after_open[gt + 1..];
            let Some(end) = body.find(close) else { break };
            let term = decode_html(&strip_all_tags(&body[..end]));
            if is_term_shaped(&term) {
                out.push(term.trim().trim_end_matches(['.', ':']).to_string());
            }
            rest = &body[end..];
        }
    }
    let mut rest = line;
    while let Some(start) = rest.find("<strong>") {
        let body = &rest[start + "<strong>".len()..];
        let Some(end) = body.find("</strong>") else {
            break;
        };
        let inner = decode_html(&body[..end]);
        if let Some(term) = inner.trim().strip_suffix('.') {
            if is_term_shaped(term) && is_noun_lead_in(term) {
                out.push(term.trim().to_string());
            }
        }
        rest = &body[end..];
    }
    out
}

/// A bold lead-in names a term only when it reads like a noun phrase: at
/// most two words, no function words, no adverb. "Hub." and "Store ref."
/// pass; step labels like "Discuss the outcome." and "Review
/// independently." do not.
fn is_noun_lead_in(s: &str) -> bool {
    const FUNCTION_WORDS: &[&str] = &[
        "a", "an", "and", "the", "of", "to", "for", "with", "under", "in", "on", "or", "by",
    ];
    let words: Vec<String> = s.split_whitespace().map(str::to_lowercase).collect();
    words.len() <= 2
        && !s.contains(':')
        && words
            .iter()
            .all(|w| !FUNCTION_WORDS.contains(&w.as_str()) && !w.ends_with("ly"))
}

fn is_term_shaped(s: &str) -> bool {
    let s = s.trim().trim_end_matches(['.', ':']);
    let words = s.split_whitespace().count();
    (1..=3).contains(&words)
        && s.len() <= 40
        && s.chars().next().is_some_and(|c| c.is_alphabetic())
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '\'')
}

/// Review one website page for inline-defined terms with no glossary entry.
pub(crate) fn glossary_findings(
    file: &str,
    text: &str,
    known: &BTreeSet<String>,
    source: Option<&str>,
    reported: &mut BTreeSet<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        for term in inline_terms(line) {
            if known_term(known, &term) || !reported.insert(normalize_term(&term)) {
                continue;
            }
            let (proposed, fix) = match source {
                Some(src) => (
                    format!("add a `### {}` entry to `{src}`", normalize_term(&term)),
                    Some(Fix::GlossaryStub {
                        source: src.to_string(),
                        term: normalize_term(&term),
                    }),
                ),
                None => (
                    "add a glossary entry for it (no glossary source was found)".to_string(),
                    None,
                ),
            };
            out.push(Finding {
                kind: FindingKind::GlossaryGap,
                file: file.to_string(),
                line: idx + 1,
                evidence: format!(
                    "`{term}` is defined inline on this page but has no glossary entry"
                ),
                proposed_fix: proposed,
                fix,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PR planning and edits
// ---------------------------------------------------------------------------

/// What a PR-mode run does with the findings.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Mechanical fixes to apply, within the file cap.
    pub edits: Vec<Finding>,
    /// Findings that need judgment: each becomes a draft spec.
    pub drafts: Vec<Finding>,
    /// Mechanical fixes beyond the file cap: reported, never touched.
    pub over_cap: Vec<Finding>,
    /// The files the edits change.
    pub files: BTreeSet<String>,
}

/// Only documentation is ever edited: Markdown and HTML outside the agent
/// mirrors (`.claude/`, `.aida/`), never code, tests, or the store.
pub(crate) fn path_editable(path: &str) -> bool {
    (path.ends_with(".md") || path.ends_with(".html"))
        && !path.starts_with(".claude/")
        && !path.starts_with(".aida/")
        && !path.starts_with(".aida-store/")
        && !path.contains("/tests/")
        && !path.starts_with("target/")
}

pub(crate) fn plan(findings: &[Finding], max_files: usize) -> Plan {
    let mut p = Plan::default();
    for f in findings {
        let Some(fix) = f
            .fix
            .as_ref()
            .filter(|fix| path_editable(fix.target(&f.file)))
        else {
            p.drafts.push(f.clone());
            continue;
        };
        let target = fix.target(&f.file).to_string();
        if p.files.contains(&target) || p.files.len() < max_files {
            p.files.insert(target);
            p.edits.push(f.clone());
        } else {
            p.over_cap.push(f.clone());
        }
    }
    p
}

/// Apply the planned edits under `dir`. Returns the files actually changed;
/// an edit whose line no longer matches is skipped, never forced.
pub(crate) fn apply_edits(dir: &Path, edits: &[Finding]) -> Result<BTreeSet<String>> {
    let mut changed = BTreeSet::new();
    let mut stubs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for f in edits {
        match &f.fix {
            Some(Fix::Replace { old, new }) => {
                let path = dir.join(&f.file);
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
                let Some(line) = lines.get_mut(f.line.saturating_sub(1)) else {
                    continue;
                };
                let Some(at) = find_bounded(line, old) else {
                    continue;
                };
                line.replace_range(at..at + old.len(), new);
                std::fs::write(&path, lines.join("\n"))?;
                changed.insert(f.file.clone());
            }
            Some(Fix::GlossaryStub { source, term }) => stubs
                .entry(source.clone())
                .or_default()
                .push((term.clone(), f.file.clone())),
            None => {}
        }
    }
    for (source, terms) in stubs {
        let path = dir.join(&source);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let known = glossary_terms_from(&text);
        let mut block = String::new();
        for (term, page) in terms {
            if known.contains(&term) || block.contains(&format!("### {term}\n")) {
                continue;
            }
            block.push_str(&format!(
                "### {term}\n\n_Definition pending: `{page}` uses this term without a glossary entry (added by `aida docs sweep`)._\n\n"
            ));
        }
        if block.is_empty() {
            continue;
        }
        std::fs::write(&path, insert_glossary_block(&text, &block))?;
        changed.insert(source);
    }
    Ok(changed)
}

/// Where `old` occurs in `line` as whole words: not inside a longer name, so
/// a line already fixed to `aida status` never matches `aida st` again.
pub(crate) fn find_bounded(line: &str, old: &str) -> Option<usize> {
    let word = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    line.match_indices(old).map(|(at, _)| at).find(|&at| {
        !line[..at].chars().next_back().is_some_and(word)
            && !line[at + old.len()..].chars().next().is_some_and(word)
    })
}

/// Insert new entries before the glossary's trailing non-term sections (the
/// "Adjacent terms" and "How to add a term" headings), else at the end.
fn insert_glossary_block(text: &str, block: &str) -> String {
    let markers = ["\n## Adjacent terms", "\n## How to add a term"];
    match markers.iter().filter_map(|m| text.find(m)).min() {
        Some(at) => format!("{}\n{}{}", &text[..at], block.trim_end(), &text[at..])
            .replacen("\n\n\n", "\n\n", 1),
        None => format!("{}\n\n{}", text.trim_end(), block),
    }
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// The human report: one block per finding.
pub(crate) fn render_findings(findings: &[Finding]) -> String {
    let mut s = String::new();
    for f in findings {
        let tag = match f.kind {
            FindingKind::StaleCommand => "stale command",
            FindingKind::GlossaryGap => "glossary gap",
        };
        s.push_str(&format!(
            "{}:{} [{tag}]\n  evidence: {}\n  fix: {}\n",
            f.file, f.line, f.evidence, f.proposed_fix
        ));
    }
    s
}

/// One mail per run. `None` when there is nothing to say.
pub(crate) fn report_mail_body(
    findings: &[Finding],
    mode: SweepMode,
    since: Option<&str>,
    pr_note: Option<&str>,
    over_cap: usize,
) -> Option<String> {
    if findings.is_empty() && pr_note.is_none() {
        return None;
    }
    let range = since
        .map(|s| format!(" since {}", short(s)))
        .unwrap_or_else(|| " (full sweep)".to_string());
    let mut body = format!(
        "Documentation sweep{range}: {} new finding{} (mode: {}).\n",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" },
        mode.as_str()
    );
    if let Some(note) = pr_note {
        body.push_str(&format!("{note}\n"));
    }
    if over_cap > 0 {
        body.push_str(&format!(
            "{over_cap} mechanical fix(es) were over the per-run file cap and were not applied.\n"
        ));
    }
    if mode == SweepMode::Report {
        body.push_str("Please file and order the fixes you want.\n");
    }
    body.push('\n');
    body.push_str(&render_findings(
        &findings[..findings.len().min(MAIL_LIST_LIMIT)],
    ));
    if findings.len() > MAIL_LIST_LIMIT {
        body.push_str(&format!(
            "\n… and {} more: run `aida docs sweep --report-only` for the full list.\n",
            findings.len() - MAIL_LIST_LIMIT
        ));
    }
    Some(body)
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// Options from the command line.
pub(crate) struct SweepOpts<'a> {
    pub since: Option<&'a str>,
    pub report_only: bool,
    pub json: bool,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run_git(aida_core::git_ops::host_git_command(dir), args)
}

/// [`git`] for a call that contacts `origin`, with its credentials routed.
fn git_origin(dir: &Path, args: &[&str]) -> Result<String> {
    run_git(
        aida_core::git_ops::host_remote_git_command(dir, "origin"),
        args,
    )
}

fn run_git(mut cmd: std::process::Command, args: &[&str]) -> Result<String> {
    let out = cmd
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn lines_of(s: String) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// What [`sweep`] returns: the findings, the reviewed revision, the
/// reference revision used, and the files it reviewed.
pub(crate) type SweepOutcome = (Vec<Finding>, String, Option<String>, Vec<String>);

/// Review the documentation.
pub(crate) fn sweep(
    project_root: &Path,
    cfg: &SweepConfig,
    since: Option<&str>,
) -> Result<SweepOutcome> {
    let head = git(project_root, &["rev-parse", "HEAD"])?;
    let since = match since {
        Some(rev) => Some(
            git(
                project_root,
                &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            )
            .with_context(|| format!("`{rev}` is not a revision in this repository"))?,
        ),
        None => None,
    };
    let tracked = lines_of(git(project_root, &["ls-files"])?);
    let mut evidence = ChangeEvidence {
        since: since.clone(),
        ..Default::default()
    };
    let changed = match &since {
        Some(s) => {
            let range = format!("{s}..{head}");
            let changed = lines_of(git(project_root, &["diff", "--name-only", &range])?);
            if changed.iter().any(|p| is_cli_surface(p)) {
                evidence.cli_commit = git(
                    project_root,
                    &[
                        "log",
                        "-1",
                        "--format=%h %s",
                        &range,
                        "--",
                        "*_args.rs",
                        "*command_registry.rs",
                        "*/cli.rs",
                    ],
                )
                .ok()
                .filter(|s| !s.is_empty());
            }
            for f in changed.iter().filter(|p| in_scope(p, &cfg.website_dir)) {
                if let Ok(c) = git(
                    project_root,
                    &["log", "-1", "--format=%h %s", &range, "--", f],
                ) {
                    if !c.is_empty() {
                        evidence.doc_commits.insert(f.clone(), c);
                    }
                }
            }
            Some(changed)
        }
        None => None,
    };

    let mut root = crate::command_registry::built_command();
    root.build();
    let glossary_source = GLOSSARY_CANDIDATES
        .iter()
        .find(|p| project_root.join(p).is_file())
        .map(|p| p.to_string());
    // Only the glossary source is reviewed, never a generated copy of it.
    let scope: Vec<String> = select_scope(&tracked, changed.as_deref(), &cfg.website_dir)
        .into_iter()
        .filter(|p| {
            !GLOSSARY_CANDIDATES.contains(&p.as_str())
                || glossary_source.as_deref() == Some(p.as_str())
        })
        .collect();
    let mut known = BTreeSet::new();
    if let Some(src) = &glossary_source {
        let dir = Path::new(src).parent().unwrap_or(Path::new(""));
        for name in ["machinery-glossary.md", "lifecycle-vocabulary.md"] {
            if let Ok(text) = std::fs::read_to_string(project_root.join(dir).join(name)) {
                known.extend(glossary_terms_from(&text));
            }
        }
    }
    if let Ok(page) =
        std::fs::read_to_string(project_root.join(&cfg.website_dir).join("glossary.html"))
    {
        for id in page.split("id=\"").skip(1) {
            if let Some(end) = id.find('"') {
                known.insert(normalize_term(&id[..end].replace('-', " ")));
            }
        }
    }

    let mut findings = Vec::new();
    let mut reported_terms = BTreeSet::new();
    for file in &scope {
        let Ok(text) = std::fs::read_to_string(project_root.join(file)) else {
            continue;
        };
        findings.extend(command_findings(&root, file, &text, &evidence));
        if file.starts_with(&format!("{}/", cfg.website_dir)) && file.ends_with(".html") {
            findings.extend(glossary_findings(
                file,
                &text,
                &known,
                glossary_source.as_deref(),
                &mut reported_terms,
            ));
        }
    }
    findings.sort_by(|a, b| (&a.file, a.line, a.kind).cmp(&(&b.file, b.line, b.kind)));
    Ok((findings, head, since, scope))
}

/// `aida docs sweep`.
pub(crate) fn run(opts: SweepOpts<'_>) -> Result<()> {
    let project_root = crate::find_project_root()?;
    run_in(&project_root, opts)
}

pub(crate) fn run_in(project_root: &Path, opts: SweepOpts<'_>) -> Result<()> {
    run_with(project_root, opts, &LiveHost)
}

/// The run, with the forge and the spec store behind `host`.
pub(crate) fn run_with(
    project_root: &Path,
    opts: SweepOpts<'_>,
    host: &dyn SweepHost,
) -> Result<()> {
    let cfg = load_config(project_root)?;
    let mut state = load_state(project_root);
    let since = opts
        .since
        .map(str::to_string)
        .or_else(|| state.last_swept_rev.clone());
    let (findings, head, since, scope) = sweep(project_root, &cfg, since.as_deref())?;

    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "since": since,
                "head": head,
                "mode": if opts.report_only { "report-only" } else { cfg.mode.as_str() },
                "findings": findings,
            }))?
        );
    } else if findings.is_empty() {
        println!("Docs sweep: no findings.");
    } else {
        println!(
            "Docs sweep: {} finding{}.\n",
            findings.len(),
            if findings.len() == 1 { "" } else { "s" }
        );
        print!("{}", render_findings(&findings));
    }
    if opts.report_only {
        return Ok(());
    }

    let seen: BTreeSet<&str> = state.reported.iter().map(String::as_str).collect();
    let fresh: Vec<Finding> = findings
        .iter()
        .filter(|f| !seen.contains(f.state_key().as_str()))
        .cloned()
        .collect();
    let mut notes = Vec::new();
    let mut over_cap = 0;
    if cfg.mode == SweepMode::Pr {
        let p = plan(&findings, cfg.max_files);
        over_cap = p.over_cap.len();
        let drafts = file_drafts(project_root, host, &p.drafts)?;
        if !p.edits.is_empty() {
            if let Some(pr) = open_or_extend_pr(project_root, &cfg, host, &p)? {
                notes.push(format!(
                    "Sweep PR: {} ({} file(s) changed this run).",
                    pr.url, pr.files
                ));
                state.pr_url = Some(pr.url);
                state.pr_spec = Some(pr.spec);
            }
        }
        if drafts > 0 {
            notes.push(format!(
                "{drafts} finding(s) need judgment and were filed as draft specs (tag `{SWEEP_TAG}`)."
            ));
        }
    }
    let pr_note = (!notes.is_empty()).then(|| notes.join("\n"));
    if let Some(body) = report_mail_body(
        &fresh,
        cfg.mode,
        since.as_deref(),
        pr_note.as_deref(),
        over_cap,
    ) {
        send_report(project_root, &cfg.recipient, fresh.len(), &body)?;
    }
    state.reported = next_reported(&state.reported, &scope, &findings);
    state.last_swept_rev = Some(head);
    state.last_run_at = Some(Utc::now());
    state.findings = findings.len();
    state.mode = Some(cfg.mode);
    save_state(project_root, &state)?;
    Ok(())
}

fn send_report(project_root: &Path, recipient: &str, count: usize, body: &str) -> Result<()> {
    let (from, from_source) = crate::resolve_mail_sender_identity(None);
    let msg = aida_core::mailbox::Message {
        subject: Some(format!(
            "Docs sweep: {count} finding{}",
            if count == 1 { "" } else { "s" }
        )),
        id: uuid::Uuid::new_v4().to_string(),
        thread_id: uuid::Uuid::new_v4().to_string(),
        from,
        to: aida_core::mailbox::Recipient::Agent(recipient.to_string()),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: body.to_string(),
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source,
        from_role: crate::resolve_mail_sender_role(),
        relayed_from: None,
    };
    crate::mailbox_store::write_message(project_root, &msg)
}

fn open_backend(project_root: &Path) -> Result<aida_core::CachedGitBackend> {
    let store_path = crate::store_sync::detect_distributed_store_from(project_root)
        .ok_or_else(|| anyhow::anyhow!("no distributed store found"))?;
    let dispenser = crate::rel_cards::load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    Ok(aida_core::CachedGitBackend::with_inner(inner, &cache_path)?)
}

fn add_spec(backend: &aida_core::CachedGitBackend, spec: NewSpec) -> Result<String> {
    use aida_core::DatabaseBackend;
    let mut req = aida_core::Requirement::new(spec.title, spec.description);
    req.req_type = aida_core::RequirementType::Task;
    req.status = spec.status;
    req.owner = crate::get_default_author();
    req.tags.extend(spec.tags);
    aida_core::provenance::stamp_if_absent(&mut req);
    let store = backend.update_atomically(|store| {
        let prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req.clone(), None, prefix.as_deref());
    })?;
    store
        .requirements
        .last()
        .and_then(|r| r.spec_id.clone())
        .ok_or_else(|| anyhow::anyhow!("the store did not assign an id"))
}

/// File one draft spec per finding that needs judgment, skipping findings a
/// previous run already filed. Returns how many were filed now.
fn file_drafts(project_root: &Path, host: &dyn SweepHost, drafts: &[Finding]) -> Result<usize> {
    if drafts.is_empty() {
        return Ok(0);
    }
    let existing = host.filed_keys(project_root)?;
    let mut filed = 0;
    for f in drafts {
        let key_tag = format!("{KEY_TAG_PREFIX}{}", f.key());
        if existing.contains(&key_tag) {
            continue;
        }
        host.file_spec(
            project_root,
            NewSpec {
                title: format!("Docs sweep: {}:{} needs a fix", f.file, f.line),
                description: format!(
                    "Found by `aida docs sweep`.\n\nFile: {}:{}\nEvidence: {}\nProposed fix: {}\n",
                    f.file, f.line, f.evidence, f.proposed_fix
                ),
                status: aida_core::RequirementStatus::Draft,
                tags: vec![SWEEP_TAG.to_string(), "docs-only".to_string(), key_tag],
            },
        )?;
        filed += 1;
    }
    Ok(filed)
}

/// Tag prefix carrying a finding's stable key, so a rerun files no duplicate.
const KEY_TAG_PREFIX: &str = "docs-sweep-key:";

/// Tags on the spec a sweep PR's commits trail. Documentation-only fixes
/// take the light review tier, which is the fasttrack lane: its own filing
/// tags (`batch:fasttrack`, `lifecycle:no-review`) skip the reviewer model
/// phase; CI and an independent approval still gate the merge.
pub(crate) fn batch_spec_tags() -> Vec<String> {
    let (bucket, lane_tag, _mode) = crate::leases::fasttrack_lane_filing(false);
    let mut tags = vec![SWEEP_TAG.to_string(), "docs-only".to_string()];
    tags.extend(bucket.map(|b| format!("batch:{b}")));
    tags.extend(lane_tag);
    tags
}

fn batch_spec() -> NewSpec {
    NewSpec {
        title: "Docs sweep: batched documentation fixes".to_string(),
        description: "Mechanical documentation fixes batched by `aida docs sweep`. \
                      Documentation only, so it takes the light review tier."
            .to_string(),
        status: aida_core::RequirementStatus::Approved,
        tags: batch_spec_tags(),
    }
}

// ---------------------------------------------------------------------------
// The forge and the spec store
// ---------------------------------------------------------------------------

/// An open sweep PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPr {
    pub url: String,
    pub title: String,
}

/// The spec a sweep PR's commits trail: the parenthesized ID ending its
/// title (`… batched documentation fixes (TASK-12)`).
pub(crate) fn spec_from_title(title: &str) -> Option<String> {
    let inner = title.trim_end().strip_suffix(')')?;
    let id = inner[inner.rfind('(')? + 1..].trim();
    (id.contains('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then(|| id.to_string())
}

/// A spec a sweep files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewSpec {
    pub title: String,
    pub description: String,
    pub status: aida_core::RequirementStatus,
    pub tags: Vec<String>,
}

/// What a PR-mode run needs from outside the repository: the forge (`gh`)
/// and the spec store. [`LiveHost`] is the real one; tests use a fake.
pub(crate) trait SweepHost {
    /// The open PR on [`SWEEP_BRANCH`], if there is one.
    fn open_sweep_pr(&self, project_root: &Path) -> Result<Option<OpenPr>>;
    /// Open a PR from [`SWEEP_BRANCH`] into `base`; returns its URL.
    fn create_pr(&self, dir: &Path, base: &str, title: &str, body: &str) -> Result<String>;
    /// Every `docs-sweep-key:` tag a filed spec already carries.
    fn filed_keys(&self, project_root: &Path) -> Result<BTreeSet<String>>;
    /// File a spec; returns its ID.
    fn file_spec(&self, project_root: &Path, spec: NewSpec) -> Result<String>;
}

pub(crate) struct LiveHost;

impl SweepHost for LiveHost {
    fn open_sweep_pr(&self, project_root: &Path) -> Result<Option<OpenPr>> {
        let out = std::process::Command::new("gh")
            .current_dir(project_root)
            .args([
                "pr",
                "list",
                "--head",
                SWEEP_BRANCH,
                "--state",
                "open",
                "--json",
                "url,title",
                "--jq",
                ".[0] // empty | [.url, .title] | @tsv",
            ])
            .output()
            .context("running gh pr list")?;
        if !out.status.success() {
            anyhow::bail!(
                "gh pr list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (url, title) = line.split_once('\t').unwrap_or((line.as_str(), ""));
        Ok((!url.trim().is_empty()).then(|| OpenPr {
            url: url.trim().to_string(),
            title: title.trim().to_string(),
        }))
    }

    fn create_pr(&self, dir: &Path, base: &str, title: &str, body: &str) -> Result<String> {
        let out = std::process::Command::new("gh")
            .current_dir(dir)
            .args([
                "pr",
                "create",
                "--head",
                SWEEP_BRANCH,
                "--base",
                base,
                "--title",
                title,
                "--body",
                body,
            ])
            .output()
            .context("running gh pr create")?;
        if !out.status.success() {
            anyhow::bail!(
                "gh pr create failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn filed_keys(&self, project_root: &Path) -> Result<BTreeSet<String>> {
        // trace:STORY-2220 | ai:codex — filing must see current canonical
        // objects, including writes newer than an advisory cache snapshot.
        let store = crate::store_sync::detect_distributed_store_from(project_root)
            .context("cannot resolve docs sweep dedup store")?;
        let mut keys = BTreeSet::new();
        for (_, path) in aida_core::object_store::list_objects_strict(&store.join("objects"))? {
            let req = aida_core::object_store::read_object_from_path(&path)?;
            keys.extend(
                req.tags
                    .into_iter()
                    .filter(|t| t.starts_with(KEY_TAG_PREFIX)),
            );
        }
        Ok(keys)
    }

    fn file_spec(&self, project_root: &Path, spec: NewSpec) -> Result<String> {
        add_spec(&open_backend(project_root)?, spec)
    }
}

/// Whether a path is documentation a sweep commit may carry.
fn is_doc_path(path: &str) -> bool {
    (path.ends_with(".md") || path.ends_with(".html"))
        && !path.contains("/tests/")
        && !path.starts_with("target/")
        && !path.starts_with(".aida-store/")
}

fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What a sweep commit may contain: the files its edits changed and any
/// documentation copies a `regenerate` step derived from them (same file name,
/// still Markdown or HTML outside tests and the store), together bounded by
/// the single `max_files` cap.
/// Anything else (code, a test, an unrelated file) means a regenerate step
/// reached past the docs-only, capped edit set, so nothing is committed.
pub(crate) fn check_staged(
    staged: &[String],
    changed: &BTreeSet<String>,
    max_files: usize,
) -> Result<()> {
    let names: BTreeSet<&str> = changed.iter().map(|p| file_name_of(p)).collect();
    let outside: Vec<&String> = staged
        .iter()
        .filter(|p| !changed.contains(*p))
        .filter(|p| !is_doc_path(p) || !names.contains(file_name_of(p)))
        .collect();
    if !outside.is_empty() {
        anyhow::bail!(
            "a regenerate step changed files outside the documentation edits; nothing was committed: {}",
            outside
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // trace:STORY-2220 | ai:codex
    // Refuse the entire batch before filing or committing when regeneration
    // consumes more room than the source-edit plan left available.
    if staged.len() > max_files {
        anyhow::bail!(
            "the staged documentation edits and derived copies total {} files, exceeding the per-run cap of {max_files} by {}; nothing was committed",
            staged.len(),
            staged.len() - max_files
        );
    }
    Ok(())
}

/// The sweep PR a run opened or extended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrOutcome {
    pub url: String,
    /// The spec the PR's commits trail.
    pub spec: String,
    /// Files this run's commit changed.
    pub files: usize,
}

/// Open the sweep PR, or add a commit to the one still open. `None` when no
/// edit applied (every planned line had moved), so nothing was committed,
/// pushed, or filed.
pub(crate) fn open_or_extend_pr(
    project_root: &Path,
    cfg: &SweepConfig,
    host: &dyn SweepHost,
    p: &Plan,
) -> Result<Option<PrOutcome>> {
    let existing = host.open_sweep_pr(project_root)?;
    let default_branch = aida_core::git_ops::default_branch_name(project_root);
    let _ = git_origin(project_root, &["fetch", "--quiet", "origin"]);
    let base = match &existing {
        Some(_) => format!("origin/{SWEEP_BRANCH}"),
        None => format!("origin/{default_branch}"),
    };
    let scratch = tempfile::tempdir().context("creating the sweep worktree directory")?;
    let dir = scratch.path().join("worktree");
    let dir_s = dir.to_string_lossy().to_string();
    git(
        project_root,
        &[
            "worktree",
            "add",
            "--force",
            "-B",
            SWEEP_BRANCH,
            &dir_s,
            &base,
        ],
    )?;
    let result = (|| -> Result<Option<PrOutcome>> {
        if existing.is_some() {
            // Bring the open PR up to date so a doc added since it opened can
            // be fixed too; a conflict leaves the branch as it was.
            let target = format!("origin/{default_branch}");
            if git(&dir, &["merge", "--quiet", "--no-edit", &target]).is_err() {
                let _ = git(&dir, &["merge", "--abort"]);
            }
        }
        let changed = apply_edits(&dir, &p.edits)?;
        if changed.is_empty() {
            return Ok(None);
        }
        for cmd in &cfg.regenerate {
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&dir)
                .status()
                .with_context(|| format!("running `{cmd}`"))?;
            if !status.success() {
                anyhow::bail!("regenerate command `{cmd}` failed");
            }
        }
        git(&dir, &["add", "-A"])?;
        let staged = lines_of(git(&dir, &["diff", "--cached", "--name-only"])?);
        if staged.is_empty() {
            return Ok(None);
        }
        check_staged(&staged, &changed, cfg.max_files)?;
        // The spec is filed only now that there is a commit to trail it, and
        // an open PR keeps the spec its title already names.
        let spec = match existing.as_ref().and_then(|pr| spec_from_title(&pr.title)) {
            Some(spec) => spec,
            None => host.file_spec(project_root, batch_spec())?,
        };
        let applied: Vec<Finding> = p
            .edits
            .iter()
            .filter(|f| {
                f.fix
                    .as_ref()
                    .is_some_and(|x| changed.contains(x.target(&f.file)))
            })
            .cloned()
            .collect();
        let title = format!("[AI:aida] docs(sweep): batched documentation fixes ({spec})");
        let message = format!(
            "{title}\n\n{}",
            applied
                .iter()
                .map(|f| format!("- {}:{} {}", f.file, f.line, f.proposed_fix))
                .collect::<Vec<_>>()
                .join("\n")
        );
        git(&dir, &["commit", "--quiet", "-m", &message])?;
        let url = match &existing {
            Some(pr) => {
                git_origin(&dir, &["push", "--quiet", "origin", SWEEP_BRANCH])?;
                pr.url.clone()
            }
            None => {
                git_origin(
                    &dir,
                    &["push", "--quiet", "--force", "-u", "origin", SWEEP_BRANCH],
                )?;
                let body = format!(
                    "Batched documentation fixes from `aida docs sweep`. Documentation \
                     only (light review); spec {spec}.\n\n{}",
                    render_findings(&applied)
                );
                host.create_pr(&dir, &default_branch, &title, &body)?
            }
        };
        if url.trim().is_empty() {
            anyhow::bail!("the sweep branch was pushed but the forge returned no PR URL");
        }
        Ok(Some(PrOutcome {
            url,
            spec,
            files: staged.len(),
        }))
    })();
    let _ = git(project_root, &["worktree", "remove", "--force", &dir_s]);
    let _ = git(project_root, &["worktree", "prune"]);
    result
}

#[cfg(test)]
#[path = "tests/story_2220_docs_sweep_tests.rs"]
mod tests;
