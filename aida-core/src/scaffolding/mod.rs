// trace:FR-0152,FR-0226 | ai:claude:high
//! AI Project Scaffolding Module
//!
//! Provides functionality to generate AI coding agent integration artifacts:
//! - CLAUDE.md project instructions
//! - AGENTS.md project instructions
//! - .claude/commands/ directory with project-specific slash commands
//! - .claude/skills/ directory with requirements-driven development skills
//! - .codex/skills/ directory with requirements-driven development skills
//! - .git/hooks/ directory with traceability validation hooks
//! - Code traceability configuration

mod aida_md;
mod claude_md;
mod codex_md;
mod hooks;
mod managed_merge;
mod settings;

pub use aida_md::{aida_md_matches, extract_aida_block};
pub use claude_md::{claude_md_has_import, insert_claude_md_import, CLAUDE_AIDA_IMPORT};
pub use managed_merge::{slot_merge, slots_for_file, SlotChange, SlotChangeKind};

/// Slot-level diff for ManagedMerge files. Parses both sides as JSON, walks
/// the AIDA-owned slot list, and reports only slots whose parsed sub-value
/// differs. Falls back to a full-text diff if either side fails to parse.
///
/// JSON object key ordering is invisible at the parsed-value level, so this
/// path treats `{"a":1,"b":2}` and `{"b":2,"a":1}` as identical regardless
/// of how they look in serialized form.
/// trace:BUG-1-066 | ai:claude
fn diff_slice_managed_merge(expected: &str, actual: &str, slots: &[&str]) -> DiffSlice {
    let expected_v: serde_json::Value = match serde_json::from_str(expected) {
        Ok(v) => v,
        Err(_) => {
            return DiffSlice::FullDiff {
                expected: expected.to_string(),
                actual: actual.to_string(),
            };
        }
    };
    let actual_v: serde_json::Value = match serde_json::from_str(actual) {
        Ok(v) => v,
        Err(_) => {
            return DiffSlice::FullDiff {
                expected: expected.to_string(),
                actual: actual.to_string(),
            };
        }
    };

    // Find which slots actually differ (by parsed value, not text).
    let mut differing: Vec<&str> = Vec::new();
    for slot in slots {
        if expected_v.pointer(slot) != actual_v.pointer(slot) {
            differing.push(slot);
        }
    }
    if differing.is_empty() {
        return DiffSlice::Match;
    }

    // Render only the affected slots, pretty-printed, on both sides.
    let render = |doc: &serde_json::Value| -> String {
        let mut s = String::new();
        for slot in &differing {
            s.push_str(&format!("slot: {}\n", slot));
            let v = doc
                .pointer(slot)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let pretty =
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "<unrenderable>".to_string());
            for line in pretty.lines() {
                s.push_str("  ");
                s.push_str(line);
                s.push('\n');
            }
            s.push('\n');
        }
        s
    };

    DiffSlice::SliceDiff {
        expected: render(&expected_v),
        actual: render(&actual_v),
        note: format!(
            "diff scoped to AIDA-managed JSON slots ({} differ; key-ordering noise hidden)",
            differing.len()
        ),
    }
}

/// Result of computing what to diff for a scaffold artifact, given its
/// AIDA-managed scope. trace:FR-1-027 | ai:claude
#[derive(Debug, Clone)]
pub enum DiffSlice {
    /// AIDA-managed portion matches on-disk content — nothing to show.
    Match,
    /// Full content differs — render `expected` vs `actual` as-is.
    FullDiff { expected: String, actual: String },
    /// Only a sub-slice differs (CLAUDE.md presence-check or AGENTS.md
    /// AUTOGEN block). The renderer should diff `expected` vs `actual`,
    /// and the (optional) `note` shown above the diff explains scope.
    SliceDiff {
        expected: String,
        actual: String,
        note: String,
    },
    /// Required marker / line is missing entirely from the on-disk file.
    /// Surfaced as a one-line warning (no diff body) so the user knows
    /// to restore it. trace:FR-1-027 | ai:claude
    MarkerMissing { message: String },
}

/// Compute the diff slice for a scaffold artifact. Three files have AIDA-
/// managed sub-portions; full-content drift on these is mostly noise:
///
/// - `CLAUDE.md` (Seed): only the `@.claude/AIDA.md` import line is AIDA-
///   managed; everything else is user-owned.
/// - `AGENTS.md` (Seed): only the content between AIDA-AUTOGEN markers.
/// - `.claude/settings.json` and `.mcp.json` (ManagedMerge): only the
///   declared JSON Pointer slots from `slots_for_file`. JSON-key-order
///   differences are not drift.
///
/// All other files are diffed whole.
/// trace:FR-1-027 | ai:claude
pub fn aida_managed_diff_slice(path: &Path, expected: &str, actual: &str) -> DiffSlice {
    // ManagedMerge files (slot-shared JSON): compare only declared slots,
    // ignore user-owned keys and JSON object-key ordering noise.
    // trace:BUG-1-066 | ai:claude
    let slots = managed_merge::slots_for_file(path);
    if !slots.is_empty() {
        return diff_slice_managed_merge(expected, actual, slots);
    }

    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    match name {
        "CLAUDE.md" => {
            // CLAUDE.md is mostly user-owned; the only thing AIDA cares
            // about is the `@.claude/AIDA.md` import line that pulls in
            // the conventions. If that's present, no drift to surface.
            const KEY: &str = "@.claude/AIDA.md";
            if actual.contains(KEY) {
                DiffSlice::Match
            } else {
                DiffSlice::MarkerMissing {
                    message: format!(
                        "CLAUDE.md does not import `{}`. Add the line to pull in AIDA conventions, or run `aida scaffold upgrade`.",
                        KEY
                    ),
                }
            }
        }
        "AGENTS.md" => {
            // AGENTS.md: compare only what's between AIDA-AUTOGEN markers.
            // If markers are absent on disk, the user opted out — treat as
            // matching (consistent with `seed_matches` in report.rs).
            match extract_aida_block(actual) {
                None => DiffSlice::Match,
                Some(actual_block) => match extract_aida_block(expected) {
                    None => DiffSlice::Match,
                    Some(expected_block) => {
                        if actual_block.trim() == expected_block.trim() {
                            DiffSlice::Match
                        } else {
                            DiffSlice::SliceDiff {
                                expected: expected_block.to_string(),
                                actual: actual_block.to_string(),
                                note: "diff scoped to <!-- AIDA-AUTOGEN-BEGIN --> ... <!-- AIDA-AUTOGEN-END --> block".to_string(),
                            }
                        }
                    }
                },
            }
        }
        "AIDA.md" => {
            // .claude/AIDA.md's "Claude Code skills" section is gated by
            // generate_skills; an `aida init --no-skills` project drops it,
            // but the status path always regenerates with skills on. Compare
            // tolerant of that section so a clean --no-skills init doesn't
            // report STALE-on-arrival. trace:TASK-125 | ai:claude
            if aida_md_matches(actual, expected) {
                DiffSlice::Match
            } else {
                DiffSlice::FullDiff {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                }
            }
        }
        _ => {
            if actual.trim() == expected.trim() {
                DiffSlice::Match
            } else {
                DiffSlice::FullDiff {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                }
            }
        }
    }
}

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::RequirementsStore;
use crate::templates::TemplateLoader;

/// Current scaffolding version - increment when templates change significantly
pub const SCAFFOLD_VERSION: &str = "2.0.0";

/// Compute a simple checksum for content (first 8 chars of hex-encoded hash)
fn compute_checksum(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())[..8].to_string()
}

/// Generate the AIDA header for a markdown file
fn generate_aida_header(content: &str) -> String {
    let checksum = compute_checksum(content);
    format!(
        "<!-- AIDA Generated: v{} | checksum:{} | DO NOT EDIT DIRECTLY -->\n\
         <!-- To customize: copy this file and modify the copy -->\n\n",
        SCAFFOLD_VERSION, checksum
    )
}

/// Generate the AIDA header for a shell script file
fn generate_aida_header_shell(content: &str) -> String {
    let checksum = compute_checksum(content);
    format!(
        "# AIDA Generated: v{} | checksum:{}\n\
         # To customize: copy this file and modify the copy\n",
        SCAFFOLD_VERSION, checksum
    )
}

/// Wrap raw embedded-template bytes with the appropriate AIDA-Generated
/// header so the result matches what `scaffold apply` would write to disk.
/// Public so callers like `aida scaffold extract` can produce files that
/// round-trip cleanly through `scaffold status` (BUG-1-034).
///
/// Header form depends on file type:
/// - `.json`             → no header (JSON has no comment syntax; an HTML
///   comment breaks parsers like Claude Code's settings reader).
/// - shell scripts (`.sh`) and TOML files → `# AIDA Generated: ...`
///   prepended.
/// - markdown with YAML frontmatter (starts with `---\n` and has a
///   closing `---\n`) → header inserted *after* the frontmatter so YAML
///   parsers still work.
/// - everything else (plain markdown, HTML) → `<!-- ... -->` HTML-comment
///   header at the top.
///
/// trace:BUG-1-034 | ai:claude
pub fn wrap_with_aida_header(path: &std::path::Path, raw_content: &str) -> String {
    // Recognize shell scripts by .sh extension, by living under
    // .git/hooks/ or .claude/hooks/ (extensionless hook files), or by
    // a `#!` shebang at line 1. The shebang must remain line 1 or the
    // OS won't honor it. trace:BUG-21 | ai:claude
    let path_str = path.to_string_lossy();
    let is_shell = path.extension().and_then(|e| e.to_str()) == Some("sh")
        || path_str.starts_with(".git/hooks/")
        || path_str.starts_with(".claude/hooks/")
        || raw_content.starts_with("#!");
    let is_json = path.extension().and_then(|e| e.to_str()) == Some("json");
    let is_toml = path.extension().and_then(|e| e.to_str()) == Some("toml");

    if is_json {
        raw_content.to_string()
    } else if is_shell || is_toml {
        // The shebang (if present) must remain on line 1 or the OS won't
        // honor it. Inject the AIDA-Generated comment block AFTER the
        // shebang line. trace:BUG-21 | ai:claude
        let header = generate_aida_header_shell(raw_content);
        if raw_content.starts_with("#!") {
            // First line is a shebang; preserve it as-is, inject header after.
            let split = raw_content
                .find('\n')
                .map(|nl| nl + 1)
                .unwrap_or(raw_content.len());
            let (shebang_line, body) = raw_content.split_at(split);
            format!("{}{}{}", shebang_line, header, body)
        } else {
            format!("{}{}", header, raw_content)
        }
    } else if raw_content.starts_with("---\n") {
        let after_open = 4; // past "---\n"
        if let Some(close_pos) = raw_content[after_open..].find("\n---\n") {
            let fm_end = after_open + close_pos + 5; // past "\n---\n"
            let (frontmatter, body) = raw_content.split_at(fm_end);
            format!("{}{}{}", frontmatter, generate_aida_header(body), body)
        } else {
            format!("{}{}", generate_aida_header(raw_content), raw_content)
        }
    } else {
        format!("{}{}", generate_aida_header(raw_content), raw_content)
    }
}

/// Compute the checksum that `wrap_with_aida_header` would have stored in
/// the AIDA header for `expected_content`. Mirrors the frontmatter-aware
/// scope rule used at write time so `check_file_status` compares apples to
/// apples — at write we hash the post-frontmatter body, at read we must
/// hash the same thing. Without this, every YAML-frontmatter file (skills,
/// commands) reports `Modified` even when byte-identical on disk.
/// trace:BUG-29 | ai:claude
fn checksum_for_stored_header(expected_content: &str) -> String {
    if expected_content.starts_with("---\n") {
        let after_open = 4;
        if let Some(close_pos) = expected_content[after_open..].find("\n---\n") {
            let fm_end = after_open + close_pos + 5;
            return compute_checksum(&expected_content[fm_end..]);
        }
    }
    compute_checksum(expected_content)
}

/// Find the AIDA header line in file content, skipping YAML frontmatter if present
fn find_aida_header_line(content: &str) -> Option<&str> {
    // If content starts with YAML frontmatter, skip past it
    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        // Find the closing --- after the opening one
        let after_open = if content.starts_with("---\r\n") { 5 } else { 4 };
        if let Some(close_pos) = content[after_open..].find("\n---") {
            let after_close = after_open + close_pos + 4; // past "\n---"
                                                          // Skip the newline after closing ---
            let rest = content[after_close..]
                .trim_start_matches('\r')
                .trim_start_matches('\n');
            return rest.lines().next();
        }
    }

    // No frontmatter - header should be on the first line
    content.lines().next()
}

/// Parse an existing file to determine its status relative to expected content
fn check_file_status(file_path: &PathBuf, expected_content: &str) -> FileStatus {
    let content = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return FileStatus::New,
    };

    // JSON files are headerless (JSON has no comment syntax). Compare by
    // raw content equality. trace:EPIC-1-001 | ai:claude
    if file_path.extension().and_then(|e| e.to_str()) == Some("json") {
        if content == expected_content {
            return FileStatus::Unmodified;
        }
        return FileStatus::Modified {
            expected_checksum: compute_checksum(expected_content),
            actual_checksum: compute_checksum(&content),
        };
    }

    // Try to parse AIDA header (markdown format)
    // Format: <!-- AIDA Generated: v{version} | checksum:{hash} | DO NOT EDIT DIRECTLY -->
    let md_header_pattern = regex::Regex::new(
        r"^<!-- AIDA Generated: v([0-9.]+) \| checksum:([a-f0-9]+) \| DO NOT EDIT DIRECTLY -->",
    )
    .unwrap();

    // Try to parse AIDA header (shell format)
    // Format: # AIDA Generated: v{version} | checksum:{hash}
    let shell_header_pattern =
        regex::Regex::new(r"^# AIDA Generated: v([0-9.]+) \| checksum:([a-f0-9]+)").unwrap();

    // Find the header line, skipping frontmatter if present
    let header_line = find_aida_header_line(&content).unwrap_or("");

    // Check markdown header
    if let Some(caps) = md_header_pattern.captures(header_line) {
        let file_version = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let stored_checksum = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        // Check version first
        if file_version != SCAFFOLD_VERSION {
            return FileStatus::OlderVersion {
                file_version: file_version.to_string(),
            };
        }

        // Compute checksum of the expected content using the same scope
        // (post-frontmatter body when applicable) the writer used. Without
        // this, every YAML-frontmatter file reports false Modified.
        // trace:BUG-29 | ai:claude
        let expected_checksum = checksum_for_stored_header(expected_content);

        if stored_checksum == expected_checksum {
            return FileStatus::Unmodified;
        } else {
            return FileStatus::Modified {
                expected_checksum,
                actual_checksum: stored_checksum.to_string(),
            };
        }
    }

    // Check shell header
    if let Some(caps) = shell_header_pattern.captures(header_line) {
        let file_version = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let stored_checksum = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        // Check version first
        if file_version != SCAFFOLD_VERSION {
            return FileStatus::OlderVersion {
                file_version: file_version.to_string(),
            };
        }

        // Shell scripts don't have YAML frontmatter, so checksum_for_stored_header
        // collapses to compute_checksum(expected_content) — matching the writer.
        let expected_checksum = checksum_for_stored_header(expected_content);

        if stored_checksum == expected_checksum {
            return FileStatus::Unmodified;
        } else {
            return FileStatus::Modified {
                expected_checksum,
                actual_checksum: stored_checksum.to_string(),
            };
        }
    }

    // No AIDA header found - file exists but wasn't generated by AIDA
    FileStatus::NoHeader
}

/// Status of an existing scaffolded file
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    /// File doesn't exist yet
    New,
    /// File exists and matches expected checksum (safe to overwrite)
    Unmodified,
    /// File exists but checksum differs (user modified)
    Modified {
        expected_checksum: String,
        actual_checksum: String,
    },
    /// File exists but has no AIDA header (unknown origin)
    NoHeader,
    /// File exists with older version (can be upgraded)
    OlderVersion { file_version: String },
}

/// Configuration for what scaffolding artifacts to generate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldConfig {
    /// Generate CLAUDE.md project instructions
    pub generate_claude_md: bool,
    /// Generate CLAUDE.local.md — per-machine, gitignored companion to
    /// CLAUDE.md. Scaffolded with a two-section template (project
    /// review feedback + personal habits to correct). Never overwrites
    /// an existing CLAUDE.local.md. trace:TASK-572 | ai:claude
    pub generate_claude_local_md: bool,
    /// Generate AGENTS.md project instructions for Codex-compatible agents
    pub generate_agents_md: bool,
    /// Generate .claude/commands/ directory with slash commands
    pub generate_commands: bool,
    /// Generate .claude/skills/ directory with skills
    pub generate_skills: bool,
    /// Generate .codex/skills/ directory with Codex-compatible skills
    pub generate_codex_skills: bool,
    /// Include aida-req skill for requirement creation
    pub include_aida_req_skill: bool,
    /// Include aida-plan skill for implementation planning
    pub include_aida_plan_skill: bool,
    /// Include aida-implement skill for requirement implementation
    pub include_aida_implement_skill: bool,
    /// Include aida-capture skill for session review
    pub include_aida_capture_skill: bool,
    /// Include aida-docs skill for documentation management
    pub include_aida_docs_skill: bool,
    /// Include aida-docs-review skill for exhaustive documentation quality review
    pub include_aida_docs_review_skill: bool,
    /// Include aida-release skill for release management
    pub include_aida_release_skill: bool,
    /// Include aida-evaluate skill for requirement quality evaluation
    pub include_aida_evaluate_skill: bool,
    /// Include aida-commit skill for commit with requirement linking
    pub include_aida_commit_skill: bool,
    /// Include aida-sync skill for template synchronization
    pub include_aida_sync_skill: bool,
    /// Include aida-test skill for test generation linked to requirements
    pub include_aida_test_skill: bool,
    /// Include aida-review skill for code review against specs
    pub include_aida_review_skill: bool,
    /// Include aida-onboard skill for project onboarding
    pub include_aida_onboard_skill: bool,
    /// Include aida-sprint skill for sprint planning
    pub include_aida_sprint_skill: bool,
    /// Include aida-search skill for unified search
    pub include_aida_search_skill: bool,
    /// Include aida-standup skill for daily standup generation
    pub include_aida_standup_skill: bool,
    /// Include aida-import-plan skill for importing saved /ultraplan output
    pub include_aida_import_plan_skill: bool,
    /// Include aida-digest skill for advisor-curated narrative work reports.
    /// trace:STORY-252
    pub include_aida_digest_skill: bool,
    /// Include aida-backlog-groom skill for curating Approved work onto the
    /// queue with risk + conflict heuristics. trace:STORY-444
    pub include_aida_backlog_groom_skill: bool,
    /// Generate git hooks for traceability validation
    pub generate_git_hooks: bool,
    /// Include commit-msg hook for AI attribution validation
    pub include_commit_msg_hook: bool,
    /// Include pre-commit hook for trace comment validation
    pub include_pre_commit_hook: bool,
    /// Include prepare-commit-msg hook that pins the orphan-store HEAD
    /// SHA into every code commit's `Aida-Store:` trailer. Default true
    /// — enables `aida store status` time-travel semantics.
    /// trace:EPIC-21 | ai:claude
    pub include_store_pair_hook: bool,
    /// Generate Claude Code hooks for AIDA integration
    pub generate_claude_code_hooks: bool,
    /// Include commit validation hook (PreToolUse)
    pub include_validate_commit_hook: bool,
    /// Include commit tracking hook (PostToolUse)
    pub include_track_commits_hook: bool,
    /// Include role-context hook (SessionStart) — surfaces `(role:<name>)`
    /// state to Claude when a session starts. The hook itself is a no-op
    /// when AIDA_SESSION_ROLE is unset, so it's safe even for users who
    /// don't run roles. trace:TASK-20 | ai:claude
    pub include_role_context_hook: bool,
    /// Include git-guardrails hook (PreToolUse) — blocks risky git commands
    /// like `push --force` to main without explicit confirmation.
    /// trace:TASK-27 | ai:claude
    pub include_git_guardrails_hook: bool,
    /// Custom project type for specialized scaffolding
    pub project_type: ProjectType,
    /// Tech stack hints for context generation
    pub tech_stack: Vec<String>,
}

impl Default for ScaffoldConfig {
    fn default() -> Self {
        Self {
            generate_claude_md: true,
            // trace:TASK-572 | ai:claude
            generate_claude_local_md: true,
            generate_agents_md: true,
            generate_commands: true,
            generate_skills: true,
            generate_codex_skills: true,
            include_aida_req_skill: true,
            include_aida_plan_skill: true,
            include_aida_implement_skill: true,
            include_aida_capture_skill: true,
            include_aida_docs_skill: true,
            include_aida_docs_review_skill: true,
            include_aida_release_skill: true,
            include_aida_evaluate_skill: true,
            include_aida_commit_skill: true,
            include_aida_sync_skill: true,
            include_aida_test_skill: true,
            include_aida_review_skill: true,
            include_aida_onboard_skill: true,
            include_aida_sprint_skill: true,
            include_aida_search_skill: true,
            include_aida_standup_skill: true,
            include_aida_import_plan_skill: true,
            // trace:STORY-252
            include_aida_digest_skill: true,
            // trace:STORY-444
            include_aida_backlog_groom_skill: true,
            generate_git_hooks: true,
            include_commit_msg_hook: true,
            include_pre_commit_hook: true, // Enabled by default
            include_store_pair_hook: true,
            generate_claude_code_hooks: true,
            include_validate_commit_hook: true,
            include_track_commits_hook: true,
            include_role_context_hook: true,
            include_git_guardrails_hook: true,
            project_type: ProjectType::Generic,
            tech_stack: Vec::new(),
        }
    }
}

/// Project type for specialized scaffolding
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectType {
    #[default]
    Generic,
    Rust,
    Python,
    TypeScript,
    Web,
    Api,
    Cli,
}

impl ProjectType {
    /// Get all project types for UI selection
    pub fn all() -> &'static [ProjectType] {
        &[
            ProjectType::Generic,
            ProjectType::Rust,
            ProjectType::Python,
            ProjectType::TypeScript,
            ProjectType::Web,
            ProjectType::Api,
            ProjectType::Cli,
        ]
    }

    /// Get display label for the project type
    pub fn label(&self) -> &'static str {
        match self {
            ProjectType::Generic => "Generic",
            ProjectType::Rust => "Rust",
            ProjectType::Python => "Python",
            ProjectType::TypeScript => "TypeScript",
            ProjectType::Web => "Web Application",
            ProjectType::Api => "API/Backend",
            ProjectType::Cli => "CLI Tool",
        }
    }
}

/// Ownership category for a scaffolded file. Determines per-file upgrade
/// semantics for `aida scaffold upgrade` — the design rationale is
/// `docs/plans/2026-05-04-scaffold-categorization.md` (SPIKE-1-029).
///
/// trace:FR-1-028 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    /// Pure embedded tooling (skills, commands, hooks, .claude/AIDA.md).
    /// AIDA owns these; drift = stale on-disk copy. `upgrade` overwrites
    /// without prompting.
    Template,
    /// Scaffolded once at `init`, then user-owned (CLAUDE.md, AGENTS.md).
    /// Drift is *expected* after first apply. `upgrade` leaves alone.
    /// AGENTS.md is special-cased: a delimited AIDA-AUTOGEN block inside
    /// the seed file IS auto-upgraded (see FR-1-035 + report.rs).
    Seed,
    /// Slot-shared (settings.json, .mcp.json). AIDA owns specific JSONPath
    /// slots, user owns the rest. v1 of `upgrade` treats this like Seed
    /// (don't touch existing files); slot-merge semantics are deferred
    /// — see SPIKE-1-029 §Q3 for the design.
    ManagedMerge,
}

impl FileCategory {
    /// Path-based categorization. Hard-coded today; the spike's
    /// `manifest.toml` design (Q2) is the long-term home but adding it
    /// would dwarf the actual upgrade-behavior work for v1.
    /// trace:FR-1-028 | ai:claude
    pub fn from_path(path: &Path) -> FileCategory {
        let s = path.to_string_lossy();
        // ManagedMerge: JSON config files where AIDA + user share keys.
        if s == ".claude/settings.json" || s == ".mcp.json" {
            return FileCategory::ManagedMerge;
        }
        // Seed: top-level project docs the user is expected to tailor.
        if s == "CLAUDE.md" || s == "AGENTS.md" {
            return FileCategory::Seed;
        }
        // Everything else under templates/ control surfaces is Template.
        // That's `.claude/AIDA.md`, all `.claude/skills/*`, `.claude/commands/*`,
        // `.claude/hooks/*`, `.codex/skills/**`, and the git commit-msg hook.
        FileCategory::Template
    }

    pub fn label(self) -> &'static str {
        match self {
            FileCategory::Template => "template",
            FileCategory::Seed => "seed",
            FileCategory::ManagedMerge => "managed-merge",
        }
    }
}

/// Represents a scaffolding artifact to be generated
#[derive(Debug, Clone)]
pub struct ScaffoldArtifact {
    /// Relative path from project root
    pub path: PathBuf,
    /// Content of the artifact (with AIDA header)
    pub content: String,
    /// Description of what this artifact does
    pub description: String,
    /// Whether the file already exists
    pub exists: bool,
    /// Status of existing file (if any)
    pub file_status: FileStatus,
}

impl ScaffoldArtifact {
    /// Ownership category for this file (Template / Seed / ManagedMerge).
    /// Drives `aida scaffold upgrade`'s per-file strategy.
    /// trace:FR-1-028 | ai:claude
    pub fn category(&self) -> FileCategory {
        FileCategory::from_path(&self.path)
    }
}

/// Result of scaffolding preview
#[derive(Debug, Clone)]
pub struct ScaffoldPreview {
    /// Artifacts to be generated
    pub artifacts: Vec<ScaffoldArtifact>,
    /// Files that would be overwritten
    pub overwrites: Vec<PathBuf>,
    /// New files that would be created
    pub new_files: Vec<PathBuf>,
    /// Directories that would be created
    pub new_dirs: Vec<PathBuf>,
    /// Files that have been modified by user (need confirmation to overwrite)
    pub modified_files: Vec<PathBuf>,
    /// Files with older AIDA versions (safe to upgrade)
    pub upgradeable_files: Vec<PathBuf>,
}

/// Options for applying scaffolding
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Force overwrite of modified files (ignores user modifications)
    pub force: bool,
}

/// Scaffolding generator
pub struct Scaffolder {
    /// Project root directory
    project_root: PathBuf,
    /// Scaffolding configuration
    config: ScaffoldConfig,
    /// Database path (to determine backend type)
    database_path: Option<PathBuf>,
    /// Template loader for external/embedded templates (used for customization fallback chain)
    #[allow(dead_code)]
    template_loader: TemplateLoader,
}

impl Scaffolder {
    /// Create a new scaffolder for the given project directory
    pub fn new(project_root: PathBuf, config: ScaffoldConfig) -> Self {
        let template_loader = TemplateLoader::with_project_root(&project_root);
        Self {
            project_root,
            config,
            database_path: None,
            template_loader,
        }
    }

    /// Create a new scaffolder with database path for backend-aware scaffolding
    pub fn with_database(
        project_root: PathBuf,
        config: ScaffoldConfig,
        database_path: PathBuf,
    ) -> Self {
        let template_loader = TemplateLoader::with_project_root(&project_root);
        Self {
            project_root,
            config,
            database_path: Some(database_path),
            template_loader,
        }
    }

    /// Check if the database is SQLite based on path extension
    fn is_sqlite_database(&self) -> bool {
        self.database_path
            .as_ref()
            .map(|p| p.extension().map(|e| e == "db").unwrap_or(false))
            .unwrap_or(false)
    }

    /// Get the database filename for display
    fn database_filename(&self) -> String {
        self.database_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "requirements.yaml".to_string())
    }

    /// Load a template from external sources or embedded, with fallback chain
    #[allow(dead_code)]
    fn load_template(&mut self, key: &str) -> Option<String> {
        self.template_loader.load(key)
    }

    /// Helper to create an artifact with version/checksum header and file status checking
    fn create_artifact(
        &self,
        path: PathBuf,
        raw_content: String,
        description: String,
        is_shell: bool,
    ) -> ScaffoldArtifact {
        let full_path = self.project_root.join(&path);
        let exists = full_path.exists();

        // Check file status against the raw content (what we're comparing against)
        let file_status = if exists {
            check_file_status(&full_path, &raw_content)
        } else {
            FileStatus::New
        };

        // Wrap with the appropriate AIDA-Generated header. The same helper
        // is used by `aida scaffold extract` so the two paths can't drift
        // apart again (BUG-1-034). The `is_shell` parameter is now ignored
        // here — wrap_with_aida_header derives it from the file extension.
        // trace:BUG-1-034 | ai:claude
        let _ = is_shell; // silence unused-arg warning; semantic is in helper
        let content = wrap_with_aida_header(&path, &raw_content);

        ScaffoldArtifact {
            path,
            content,
            description,
            exists,
            file_status,
        }
    }

    /// Generate a preview of what would be scaffolded
    pub fn preview(&mut self, store: &RequirementsStore) -> ScaffoldPreview {
        let mut artifacts = Vec::new();
        let mut overwrites = Vec::new();
        let mut new_files = Vec::new();
        let mut new_dirs = HashSet::new();
        let mut modified_files = Vec::new();
        let mut upgradeable_files = Vec::new();

        // CLAUDE.md - Note: CLAUDE.md is user-edited, so no AIDA header
        if self.config.generate_claude_md {
            let path = PathBuf::from("CLAUDE.md");
            let full_path = self.project_root.join(&path);
            let exists = full_path.exists();
            let content = self.generate_claude_md(store);

            if exists {
                overwrites.push(path.clone());
            } else {
                new_files.push(path.clone());
            }

            artifacts.push(ScaffoldArtifact {
                path,
                content,
                description: "Project instructions for Claude Code".to_string(),
                exists,
                file_status: if exists {
                    FileStatus::NoHeader
                } else {
                    FileStatus::New
                },
            });
        }

        // CLAUDE.local.md — per-machine, gitignored. Two-section starter
        // template (project review feedback + personal habits). NEVER
        // overwrites an existing file — it's personal notes accumulated
        // over time. Operator can delete to re-scaffold or use --force
        // (the global ScaffoldConfig.force isn't plumbed for this file
        // since the protection is the whole point — personal notes
        // shouldn't be silently overwritten). trace:TASK-572 | ai:claude
        if self.config.generate_claude_local_md {
            let path = PathBuf::from("CLAUDE.local.md");
            let full_path = self.project_root.join(&path);
            let exists = full_path.exists();
            if !exists {
                let content = generate_claude_local_md();
                new_files.push(path.clone());
                artifacts.push(ScaffoldArtifact {
                    path,
                    content,
                    description: "Personal Claude Code notes (gitignored, per-machine)".to_string(),
                    exists: false,
                    file_status: FileStatus::New,
                });
            }
        }

        // AGENTS.md - Note: AGENTS.md is user-edited, so no AIDA header
        if self.config.generate_agents_md {
            let path = PathBuf::from("AGENTS.md");
            let full_path = self.project_root.join(&path);
            let exists = full_path.exists();
            let content = self.generate_agents_md(store);

            if exists {
                overwrites.push(path.clone());
            } else {
                new_files.push(path.clone());
            }

            artifacts.push(ScaffoldArtifact {
                path,
                content,
                description: "Project instructions for Codex-compatible agents".to_string(),
                exists,
                file_status: if exists {
                    FileStatus::NoHeader
                } else {
                    FileStatus::New
                },
            });
        }

        // .aida/reserved-paths.toml — hand-curated project reservations
        // injected into `aida ultraplan` prompts. trace:TASK-517 | ai:codex
        if let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get("reserved-paths.toml") {
            new_dirs.insert(PathBuf::from(".aida"));
            let path = PathBuf::from(".aida/reserved-paths.toml");
            let artifact = self.create_artifact(
                path.clone(),
                body.to_string(),
                "Project reserved namespaces for /ultraplan collision avoidance".to_string(),
                false,
            );
            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }
            artifacts.push(artifact);
        }

        // .aida/agents.toml — commented per-agent launch defaults for
        // `aida agent new`. trace:TASK-557 | ai:codex
        if let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get("agents.toml") {
            new_dirs.insert(PathBuf::from(".aida"));
            let path = PathBuf::from(".aida/agents.toml");
            let artifact = self.create_artifact(
                path.clone(),
                body.to_string(),
                "Per-agent launch defaults for supervised AIDA agents".to_string(),
                false,
            );
            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }
            artifacts.push(artifact);
        }

        // .claude/AIDA.md — single source of truth for AIDA conventions.
        // Imported by CLAUDE.md via `@.claude/AIDA.md`, inlined into
        // AGENTS.md inside AIDA-AUTOGEN delimiters. Template-class:
        // auto-upgrades on `scaffold apply`, AIDA-Generated header
        // attached so drift detection works.
        // trace:FR-1-035 | ai:claude
        if self.config.generate_claude_md {
            new_dirs.insert(PathBuf::from(".claude"));
            let path = PathBuf::from(".claude/AIDA.md");
            let artifact = self.create_artifact(
                path.clone(),
                self.generate_aida_md(store),
                "AIDA conventions (single source of truth, imported by CLAUDE.md)".to_string(),
                false,
            );

            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }
            artifacts.push(artifact);
        }

        // .claude/commands/ directory
        if self.config.generate_commands {
            new_dirs.insert(PathBuf::from(".claude/commands"));

            // Add default commands
            let commands = self.generate_commands(store);
            for (name, content, desc) in commands {
                let path = PathBuf::from(format!(".claude/commands/{}.md", name));
                let artifact = self.create_artifact(path.clone(), content, desc, false);

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // .claude/skills/ directory
        if self.config.generate_skills {
            new_dirs.insert(PathBuf::from(".claude/skills"));

            // Add aida-req skill
            if self.config.include_aida_req_skill {
                let path = PathBuf::from(".claude/skills/aida-req.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_req_skill(),
                    "Skill for adding requirements with AI evaluation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-plan skill
            if self.config.include_aida_plan_skill {
                let path = PathBuf::from(".claude/skills/aida-plan.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_plan_skill(),
                    "Skill for planning requirement implementation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-implement skill
            if self.config.include_aida_implement_skill {
                let path = PathBuf::from(".claude/skills/aida-implement.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_implement_skill(),
                    "Skill for implementing requirements with traceability".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-capture skill
            if self.config.include_aida_capture_skill {
                let path = PathBuf::from(".claude/skills/aida-capture.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_capture_skill(),
                    "Skill for capturing missed requirements from session".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-docs skill
            if self.config.include_aida_docs_skill {
                let path = PathBuf::from(".claude/skills/aida-docs.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_docs_skill(),
                    "Skill for documentation management and generation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-docs-review skill
            if self.config.include_aida_docs_review_skill {
                let path = PathBuf::from(".claude/skills/aida-docs-review.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_docs_review_skill(),
                    "Skill for exhaustive documentation quality review".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-release skill
            if self.config.include_aida_release_skill {
                let path = PathBuf::from(".claude/skills/aida-release.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_release_skill(),
                    "Skill for release management and version bumping".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-evaluate skill
            if self.config.include_aida_evaluate_skill {
                let path = PathBuf::from(".claude/skills/aida-evaluate.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_evaluate_skill(),
                    "Skill for evaluating requirement quality".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-commit skill
            if self.config.include_aida_commit_skill {
                let path = PathBuf::from(".claude/skills/aida-commit.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_commit_skill(),
                    "Skill for committing with requirement linking".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-sync skill
            if self.config.include_aida_sync_skill {
                let path = PathBuf::from(".claude/skills/aida-sync.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_sync_skill(),
                    "Skill for template synchronization".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-test skill
            if self.config.include_aida_test_skill {
                let path = PathBuf::from(".claude/skills/aida-test.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_test_skill(),
                    "Skill for generating tests linked to requirements".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-review skill
            if self.config.include_aida_review_skill {
                let path = PathBuf::from(".claude/skills/aida-review.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_review_skill(),
                    "Skill for reviewing code changes against specs".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-onboard skill
            if self.config.include_aida_onboard_skill {
                let path = PathBuf::from(".claude/skills/aida-onboard.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_onboard_skill(),
                    "Skill for project onboarding".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-sprint skill
            if self.config.include_aida_sprint_skill {
                let path = PathBuf::from(".claude/skills/aida-sprint.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_sprint_skill(),
                    "Skill for sprint planning".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-search skill
            if self.config.include_aida_search_skill {
                let path = PathBuf::from(".claude/skills/aida-search.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_search_skill(),
                    "Skill for unified search across requirements and code".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-standup skill
            if self.config.include_aida_standup_skill {
                let path = PathBuf::from(".claude/skills/aida-standup.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_standup_skill(),
                    "Skill for daily standup generation".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-import-plan skill. trace:TASK-114 | ai:claude
            if self.config.include_aida_import_plan_skill {
                let path = PathBuf::from(".claude/skills/aida-import-plan.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_import_plan_skill(),
                    "Skill for importing saved /ultraplan plan files into AIDA".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-digest skill. trace:STORY-252
            if self.config.include_aida_digest_skill {
                let path = PathBuf::from(".claude/skills/aida-digest.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_digest_skill(),
                    "Skill for curated narrative work digests".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Add aida-backlog-groom skill. trace:STORY-444
            if self.config.include_aida_backlog_groom_skill {
                let path = PathBuf::from(".claude/skills/aida-backlog-groom.md");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_aida_backlog_groom_skill(),
                    "Skill for curating the Approved backlog onto the queue".to_string(),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // .claude/skills/local/README.md — per-project skill extensions
            // (STORY-305). The `local/` directory hosts project-owned skills
            // AIDA never manages; sibling `<skill>.local.md` files extend a
            // stock skill via append-merge. Both survive `aida scaffold
            // apply` and `make sync-templates` because neither path enters
            // `local/` and no stock skill's filename ends in `.local.md`.
            // The README is template-class so its explanation stays
            // canonical across upgrades. trace:STORY-305 | ai:claude
            new_dirs.insert(PathBuf::from(".claude/skills/local"));
            let local_readme_path = PathBuf::from(".claude/skills/local/README.md");
            let local_readme_artifact = self.create_artifact(
                local_readme_path.clone(),
                generate_local_skills_readme(),
                "Per-project skill extensions (STORY-305)".to_string(),
                false,
            );
            match &local_readme_artifact.file_status {
                FileStatus::New => new_files.push(local_readme_path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(local_readme_artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => {
                    upgradeable_files.push(local_readme_artifact.path.clone())
                }
                FileStatus::Unmodified => overwrites.push(local_readme_artifact.path.clone()),
            }
            artifacts.push(local_readme_artifact);

            // BUG-386: catch-all loop for skill templates that don't have a
            // dedicated `include_aida_<name>_skill` flag + handwritten block
            // above. Without this, new templates added to
            // `aida-core/templates/skills/` never reach .claude/skills/ until
            // someone adds matching boilerplate. The handwritten flag-gated
            // blocks above still control the original 20 daily-driver skills
            // for backward compat; this loop fills in anything else
            // (aida-pickup, aida-pr, aida-doctor, aida-drain-queue, etc.)
            // trace:BUG-386 | ai:claude
            {
                use crate::templates::{classify_skill_key, EMBEDDED_TEMPLATES};
                // Sort for deterministic artifact ordering across calls
                let mut keys: Vec<&&str> = EMBEDDED_TEMPLATES.keys().collect();
                keys.sort();
                for key in keys {
                    // Folder-form skills (`<name>/SKILL.md` plus `templates/` and
                    // `examples/` helpers) and flat skills (`<name>.md`) both flow
                    // through here. The relative path is preserved so the whole
                    // subfolder tree lands under `.claude/skills/`; apply()
                    // create_dir_all's parents. trace:TASK-574
                    let skill = match classify_skill_key(key) {
                        Some(s) if s.rel_path.ends_with(".md") => s,
                        _ => continue,
                    };
                    let path = PathBuf::from(format!(".claude/skills/{}", skill.rel_path));
                    // Skip if a handwritten block already scaffolded this skill
                    if artifacts.iter().any(|a| a.path == path) {
                        continue;
                    }
                    let content = EMBEDDED_TEMPLATES
                        .get(key.as_ref() as &str)
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let desc = if skill.is_prompt {
                        format!("AIDA skill: {}", skill.name)
                    } else {
                        format!("AIDA skill helper: {}", skill.rel_path)
                    };
                    let artifact = self.create_artifact(path.clone(), content, desc, false);
                    match &artifact.file_status {
                        FileStatus::New => new_files.push(path),
                        FileStatus::Modified { .. } | FileStatus::NoHeader => {
                            modified_files.push(artifact.path.clone())
                        }
                        FileStatus::OlderVersion { .. } => {
                            upgradeable_files.push(artifact.path.clone())
                        }
                        FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                    }
                    artifacts.push(artifact);
                }
            }
        }

        // .codex/skills/ directory
        if self.config.generate_codex_skills {
            new_dirs.insert(PathBuf::from(".codex/skills"));

            let codex_skill_defs = [
                ("aida-req", self.config.include_aida_req_skill),
                ("aida-plan", self.config.include_aida_plan_skill),
                ("aida-implement", self.config.include_aida_implement_skill),
                ("aida-capture", self.config.include_aida_capture_skill),
                ("aida-docs", self.config.include_aida_docs_skill),
                (
                    "aida-docs-review",
                    self.config.include_aida_docs_review_skill,
                ),
                ("aida-release", self.config.include_aida_release_skill),
                ("aida-evaluate", self.config.include_aida_evaluate_skill),
                ("aida-commit", self.config.include_aida_commit_skill),
                ("aida-sync", self.config.include_aida_sync_skill),
                ("aida-test", self.config.include_aida_test_skill),
                ("aida-review", self.config.include_aida_review_skill),
                ("aida-onboard", self.config.include_aida_onboard_skill),
                ("aida-sprint", self.config.include_aida_sprint_skill),
                ("aida-search", self.config.include_aida_search_skill),
                ("aida-standup", self.config.include_aida_standup_skill),
                (
                    "aida-import-plan",
                    self.config.include_aida_import_plan_skill,
                ),
                // trace:STORY-252
                ("aida-digest", self.config.include_aida_digest_skill),
                // trace:STORY-444
                (
                    "aida-backlog-groom",
                    self.config.include_aida_backlog_groom_skill,
                ),
            ];

            for (name, enabled) in codex_skill_defs {
                if !enabled {
                    continue;
                }
                let path = PathBuf::from(format!(".codex/skills/{}/SKILL.md", name));
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_codex_skill(name),
                    format!("Codex-compatible skill {}", name),
                    false,
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }
        }

        // .mcp.json — MCP server configuration for Claude Code
        {
            let mcp_content = r#"{
  "mcpServers": {
    "aida": {
      "type": "stdio",
      "command": "aida",
      "args": ["mcp-serve"]
    }
  }
}"#
            .to_string();
            let path = PathBuf::from(".mcp.json");
            let artifact = self.create_artifact(
                path.clone(),
                mcp_content,
                "MCP server configuration for Claude Code".to_string(),
                false,
            );

            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }

            artifacts.push(artifact);
        }

        // Cross-agent onboarding docs. These are template-class files:
        // new AIDA projects inherit the current MCP setup and discipline
        // guidance for non-Claude agents. trace:TASK-485 | ai:codex
        {
            let agent_docs = [
                (
                    "cross-agent-onboarding.md",
                    "Cross-agent MCP onboarding guide",
                ),
                ("codex-brief-pickup.md", "Codex brief pickup guide"),
                ("codex-mcp-setup.md", "Codex MCP setup guide"),
                (
                    "codex-mcp-roundtrip-verdict.md",
                    "Codex MCP roundtrip verdict",
                ),
                ("antigravity-mcp-setup.md", "Antigravity MCP setup guide"),
                (
                    "antigravity-brief-pickup.md",
                    "Antigravity brief pickup guide",
                ),
                ("per-agent-config.md", "Per-agent launch config guide"),
                (
                    "aida-mcp-install-matrix.md",
                    "AIDA MCP client install matrix",
                ),
                (
                    "session-communication.md",
                    "Agent session communication guide",
                ),
            ];
            for (name, desc) in agent_docs {
                let key = format!("docs/agents/{name}");
                let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get(key.as_str()) else {
                    continue;
                };
                let path = PathBuf::from(format!("docs/agents/{name}"));
                let artifact =
                    self.create_artifact(path.clone(), body.to_string(), desc.to_string(), false);
                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }
                artifacts.push(artifact);
            }
        }

        // Per-project skill extension guide referenced by scaffolded
        // skill-local guidance. trace:TASK-482 | ai:codex
        if let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get("docs/extending-skills.md") {
            let path = PathBuf::from("docs/extending-skills.md");
            let artifact = self.create_artifact(
                path.clone(),
                body.to_string(),
                "Guide for extending AIDA skills per project".to_string(),
                false,
            );
            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }
            artifacts.push(artifact);
        }

        // .git/hooks/ directory (only if .git exists)
        if self.config.generate_git_hooks && self.project_root.join(".git").exists() {
            new_dirs.insert(PathBuf::from(".git/hooks"));

            // commit-msg hook
            if self.config.include_commit_msg_hook {
                let path = PathBuf::from(".git/hooks/commit-msg");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_commit_msg_hook(),
                    "Git hook for validating AI attribution in commit messages".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // pre-commit hook
            if self.config.include_pre_commit_hook {
                let path = PathBuf::from(".git/hooks/pre-commit");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_pre_commit_hook(),
                    "Git hook for validating trace comments before commit".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // prepare-commit-msg hook — pins the orphan store SHA into
            // every code commit's `Aida-Store:` trailer. Pairs with
            // `aida store status`. trace:EPIC-21 | ai:claude
            if self.config.include_store_pair_hook {
                let body = crate::templates::EMBEDDED_TEMPLATES
                    .get("hooks/aida-store-pair.sh")
                    .copied()
                    .unwrap_or("")
                    .to_string();
                if !body.is_empty() {
                    let path = PathBuf::from(".git/hooks/prepare-commit-msg");
                    let artifact = self.create_artifact(
                        path.clone(),
                        body,
                        "Git hook pinning orphan-store HEAD into commit trailer".to_string(),
                        true, // shell script
                    );
                    match &artifact.file_status {
                        FileStatus::New => new_files.push(path),
                        FileStatus::Modified { .. } | FileStatus::NoHeader => {
                            modified_files.push(artifact.path.clone())
                        }
                        FileStatus::OlderVersion { .. } => {
                            upgradeable_files.push(artifact.path.clone())
                        }
                        FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                    }
                    artifacts.push(artifact);
                }
            }
        }

        // Claude Code hooks (in .claude/hooks/)
        if self.config.generate_claude_code_hooks {
            new_dirs.insert(PathBuf::from(".claude/hooks"));

            // Validate commit hook (PreToolUse)
            if self.config.include_validate_commit_hook {
                let path = PathBuf::from(".claude/hooks/aida-validate-commit.sh");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_validate_commit_hook(),
                    "Claude Code hook for validating commit messages reference requirements"
                        .to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Track commits hook (PostToolUse)
            if self.config.include_track_commits_hook {
                let path = PathBuf::from(".claude/hooks/aida-track-commits.sh");
                let artifact = self.create_artifact(
                    path.clone(),
                    self.generate_track_commits_hook(),
                    "Claude Code hook for updating requirement status after commits".to_string(),
                    true, // shell script
                );

                match &artifact.file_status {
                    FileStatus::New => new_files.push(path),
                    FileStatus::Modified { .. } | FileStatus::NoHeader => {
                        modified_files.push(artifact.path.clone())
                    }
                    FileStatus::OlderVersion { .. } => {
                        upgradeable_files.push(artifact.path.clone())
                    }
                    FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                }

                artifacts.push(artifact);
            }

            // Role-context hook (SessionStart) — sourced from embedded
            // template. trace:TASK-27 | ai:claude
            if self.config.include_role_context_hook {
                let path = PathBuf::from(".claude/hooks/aida-role-context.sh");
                let body = self.generate_role_context_hook();
                if !body.is_empty() {
                    let artifact = self.create_artifact(
                        path.clone(),
                        body,
                        "Claude Code SessionStart hook surfacing AIDA role context".to_string(),
                        true, // shell script
                    );
                    match &artifact.file_status {
                        FileStatus::New => new_files.push(path),
                        FileStatus::Modified { .. } | FileStatus::NoHeader => {
                            modified_files.push(artifact.path.clone())
                        }
                        FileStatus::OlderVersion { .. } => {
                            upgradeable_files.push(artifact.path.clone())
                        }
                        FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                    }
                    artifacts.push(artifact);
                }
            }

            // Git-guardrails hook (PreToolUse) — sourced from embedded
            // template. trace:TASK-27 | ai:claude
            if self.config.include_git_guardrails_hook {
                let path = PathBuf::from(".claude/hooks/aida-git-guardrails.sh");
                let body = self.generate_git_guardrails_hook();
                if !body.is_empty() {
                    let artifact = self.create_artifact(
                        path.clone(),
                        body,
                        "Claude Code PreToolUse hook blocking risky git commands".to_string(),
                        true, // shell script
                    );
                    match &artifact.file_status {
                        FileStatus::New => new_files.push(path),
                        FileStatus::Modified { .. } | FileStatus::NoHeader => {
                            modified_files.push(artifact.path.clone())
                        }
                        FileStatus::OlderVersion { .. } => {
                            upgradeable_files.push(artifact.path.clone())
                        }
                        FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
                    }
                    artifacts.push(artifact);
                }
            }

            // Generate settings.json with hook configuration
            let path = PathBuf::from(".claude/settings.json");
            let artifact = self.create_artifact(
                path.clone(),
                self.generate_claude_settings_json(),
                "Claude Code settings with AIDA hook configuration".to_string(),
                false, // JSON file
            );

            match &artifact.file_status {
                FileStatus::New => new_files.push(path),
                FileStatus::Modified { .. } | FileStatus::NoHeader => {
                    modified_files.push(artifact.path.clone())
                }
                FileStatus::OlderVersion { .. } => upgradeable_files.push(artifact.path.clone()),
                FileStatus::Unmodified => overwrites.push(artifact.path.clone()),
            }

            artifacts.push(artifact);
        }

        // Filter new_dirs to only include those that don't exist
        let new_dirs: Vec<PathBuf> = new_dirs
            .into_iter()
            .filter(|d| !self.project_root.join(d).exists())
            .collect();

        ScaffoldPreview {
            artifacts,
            overwrites,
            new_files,
            new_dirs,
            modified_files,
            upgradeable_files,
        }
    }

    /// Apply the scaffolding (write files) - writes all files regardless of status
    /// For more control, use `apply_with_options`
    pub fn apply(&self, preview: &ScaffoldPreview) -> Result<Vec<PathBuf>, ScaffoldError> {
        self.apply_with_options(preview, &ApplyOptions::default())
    }

    /// Apply the scaffolding with options to control behavior for modified files
    pub fn apply_with_options(
        &self,
        preview: &ScaffoldPreview,
        options: &ApplyOptions,
    ) -> Result<Vec<PathBuf>, ScaffoldError> {
        let mut written_files = Vec::new();
        let mut skipped_files = Vec::new();

        // Create directories first
        for dir in &preview.new_dirs {
            let full_path = self.project_root.join(dir);
            fs::create_dir_all(&full_path).map_err(|e| ScaffoldError::IoError {
                path: full_path.clone(),
                message: e.to_string(),
            })?;
        }

        // Also ensure parent directories exist for all artifacts
        for artifact in &preview.artifacts {
            if let Some(parent) = artifact.path.parent() {
                let full_parent = self.project_root.join(parent);
                if !full_parent.exists() {
                    fs::create_dir_all(&full_parent).map_err(|e| ScaffoldError::IoError {
                        path: full_parent.clone(),
                        message: e.to_string(),
                    })?;
                }
            }
        }

        // Write artifacts based on their status, category, and options.
        // BUG-43: Template-category files (.claude/AIDA.md, skills,
        // commands, hooks) are AIDA-owned — drift on disk means a stale
        // copy left over from a previous binary version, not a user
        // customization. Always overwrite for that category. Seed +
        // ManagedMerge keep the prior "respect user edits unless --force"
        // semantics. trace:BUG-43 | ai:claude
        for artifact in &preview.artifacts {
            let category = FileCategory::from_path(&artifact.path);
            let should_write = match &artifact.file_status {
                FileStatus::New => true,
                FileStatus::Unmodified => true,
                FileStatus::OlderVersion { .. } => true, // Always upgrade
                FileStatus::Modified { .. } => match category {
                    FileCategory::Template => true,
                    FileCategory::Seed | FileCategory::ManagedMerge => options.force,
                },
                FileStatus::NoHeader => match category {
                    FileCategory::Template => true,
                    FileCategory::Seed | FileCategory::ManagedMerge => options.force,
                },
            };

            if !should_write {
                skipped_files.push(artifact.path.clone());
                continue;
            }

            let full_path = self.project_root.join(&artifact.path);
            fs::write(&full_path, &artifact.content).map_err(|e| ScaffoldError::IoError {
                path: full_path.clone(),
                message: e.to_string(),
            })?;

            // Make git hooks and Claude Code hooks executable on Unix
            #[cfg(unix)]
            if artifact.path.starts_with(".git/hooks/")
                || artifact.path.starts_with(".claude/hooks/")
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&full_path)
                    .map_err(|e| ScaffoldError::IoError {
                        path: full_path.clone(),
                        message: e.to_string(),
                    })?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&full_path, perms).map_err(|e| ScaffoldError::IoError {
                    path: full_path.clone(),
                    message: e.to_string(),
                })?;
            }

            written_files.push(artifact.path.clone());
        }

        Ok(written_files)
    }

    /// Generate slash commands
    fn generate_commands(&self, _store: &RequirementsStore) -> Vec<(String, String, String)> {
        use crate::templates::EMBEDDED_TEMPLATES;

        // Command definitions: (template_key, output_name, description)
        let command_defs = [
            (
                "commands/aida-status.md",
                "aida-status",
                "Show project requirements status",
            ),
            (
                "commands/aida-review.md",
                "aida-review",
                "Review a requirement for quality",
            ),
            (
                "commands/aida-req.md",
                "aida-req",
                "Add a new requirement with AI evaluation",
            ),
            (
                "commands/aida-implement.md",
                "aida-implement",
                "Implement a requirement with traceability",
            ),
            (
                "commands/aida-capture.md",
                "aida-capture",
                "Capture missed requirements from session",
            ),
            (
                "commands/aida-evaluate.md",
                "aida-evaluate",
                "Evaluate requirement quality with AI",
            ),
            (
                "commands/aida-commit.md",
                "aida-commit",
                "Commit with requirement linking",
            ),
            (
                "commands/aida-sync.md",
                "aida-sync",
                "Sync templates and scaffolding",
            ),
            (
                "commands/aida-test.md",
                "aida-test",
                "Generate tests linked to requirements",
            ),
            (
                "commands/aida-onboard.md",
                "aida-onboard",
                "Project onboarding for new team members",
            ),
            (
                "commands/aida-sprint.md",
                "aida-sprint",
                "Sprint planning from approved requirements",
            ),
            (
                "commands/aida-search.md",
                "aida-search",
                "Unified search across requirements and code",
            ),
            (
                "commands/aida-standup.md",
                "aida-standup",
                "Daily standup summary from recent activity",
            ),
            (
                "commands/aida-docs-review.md",
                "aida-docs-review",
                "Exhaustive documentation quality review",
            ),
            (
                "commands/aida-import-plan.md",
                "aida-import-plan",
                "Import a saved /ultraplan plan file into AIDA conventions",
            ),
            // trace:STORY-252
            (
                "commands/aida-digest.md",
                "aida-digest",
                "Curated narrative work digest for a time window",
            ),
            // trace:STORY-444
            (
                "commands/aida-backlog-groom.md",
                "aida-backlog-groom",
                "Curate Approved backlog into the queue with risk + conflict analysis",
            ),
        ];

        let mut commands: Vec<(String, String, String)> = command_defs
            .iter()
            .filter_map(|(key, name, desc)| {
                EMBEDDED_TEMPLATES
                    .get(key)
                    .map(|content| (name.to_string(), content.to_string(), desc.to_string()))
            })
            .collect();

        // BUG-386: append any command templates not in the hand-written
        // command_defs list. Without this, new templates added to
        // `aida-core/templates/commands/` never reach .claude/commands/
        // until someone adds matching entries above. The hand-written
        // entries retain their curated descriptions for backward-compat;
        // this catch-all fills in /aida-pickup, /aida-pr, /aida-doctor, etc.
        // with auto-generated descriptions.
        // trace:BUG-386 | ai:claude
        let already_listed: std::collections::HashSet<String> =
            commands.iter().map(|(name, _, _)| name.clone()).collect();
        let mut catch_all_keys: Vec<&&str> = EMBEDDED_TEMPLATES.keys().collect();
        catch_all_keys.sort();
        for key in catch_all_keys {
            let filename = match key.strip_prefix("commands/") {
                Some(f) if f.ends_with(".md") => f,
                _ => continue,
            };
            let name = filename.trim_end_matches(".md").to_string();
            if already_listed.contains(&name) {
                continue;
            }
            if let Some(content) = EMBEDDED_TEMPLATES.get(key.as_ref() as &str) {
                let desc = format!("AIDA slash command: /{}", name);
                commands.push((name, content.to_string(), desc));
            }
        }

        commands
    }

    /// Generate aida-req skill content (loads from embedded template)
    fn generate_aida_req_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-req.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Requirement Creation Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-implement skill content (loads from embedded template)
    fn generate_aida_implement_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-implement.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Implementation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-plan skill content (loads from embedded template)
    fn generate_aida_plan_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-plan.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Planning Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-capture skill content (loads from embedded template)
    fn generate_aida_capture_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-capture.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Session Capture Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-docs skill content (loads from embedded template)
    fn generate_aida_docs_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-docs.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Documentation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-docs-review skill content (loads from embedded template)
    fn generate_aida_docs_review_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-docs-review.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Documentation Review Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-release skill content (loads from embedded template)
    fn generate_aida_release_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-release.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Release Management Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-evaluate skill content (loads from embedded template)
    fn generate_aida_evaluate_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-evaluate.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Requirement Evaluation Skill

## Purpose

Evaluate a requirement's quality using AI analysis.

## When to Use

Use this skill when:
- User wants to evaluate a specific requirement's quality
- User asks to "evaluate", "assess", or "review" a requirement

## Workflow

1. Load the requirement from database: `aida show <SPEC-ID>`
2. Run AI evaluation for clarity, testability, completeness, consistency
3. Display quality score and issues found
4. Offer follow-up actions: improve, split, or accept
"#
                .to_string()
            })
    }

    /// Generate aida-commit skill content (loads from embedded template)
    fn generate_aida_commit_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-commit.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Commit Skill

## Purpose

Create git commits with automatic requirement linkage.

## When to Use

Use this skill when:
- User wants to commit changes with requirement traceability
- User says "commit" after implementing features

## Workflow

1. Analyze staged changes and extract requirement traces
2. Check for untraced implementation code
3. Offer to create requirements for untraced work
4. Create commit with requirement links
5. Update linked requirement statuses
"#
                .to_string()
            })
    }

    /// Generate aida-sync skill content (loads from embedded template)
    fn generate_aida_sync_skill(&self) -> String {
        // Load from embedded templates at compile time
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-sync.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                r#"# AIDA Sync Skill

## Purpose

Maintain consistency between AIDA templates and scaffolded projects.

## When to Use

Use this skill when:
- You've modified templates in `aida-core/templates/`
- You want to check scaffold status
- At the end of an AIDA development session

## Workflow

1. Detect environment (AIDA repo vs scaffolded project)
2. For AIDA repo: Check template integrity
3. For other projects: Check scaffold status
4. Ensure templates and skills are consistent
"#
                .to_string()
            })
    }

    /// Generate aida-test skill content (loads from embedded template)
    fn generate_aida_test_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-test.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Test Generation Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-review skill content (loads from embedded template)
    fn generate_aida_review_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-review.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Code Review Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-onboard skill content (loads from embedded template)
    fn generate_aida_onboard_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-onboard.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                "# AIDA Project Onboarding Skill\n\n(template not found)".to_string()
            })
    }

    /// Generate aida-sprint skill content (loads from embedded template)
    fn generate_aida_sprint_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-sprint.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Sprint Planning Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-search skill content (loads from embedded template)
    fn generate_aida_search_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-search.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Unified Search Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-standup skill content (loads from embedded template)
    fn generate_aida_standup_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-standup.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Standup Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-import-plan skill content (loads from embedded
    /// template). trace:TASK-114 | ai:claude
    fn generate_aida_import_plan_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-import-plan.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Import Plan Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-digest skill content (loads from embedded template).
    /// trace:STORY-252
    fn generate_aida_digest_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-digest.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Digest Skill\n\n(template not found)".to_string())
    }

    /// Generate aida-backlog-groom skill content (loads from embedded
    /// template). trace:STORY-444
    fn generate_aida_backlog_groom_skill(&self) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;
        EMBEDDED_TEMPLATES
            .get("skills/aida-backlog-groom.md")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "# AIDA Backlog Groom Skill\n\n(template not found)".to_string())
    }

    /// Generate Codex skill content from an embedded Claude skill template.
    /// Codex requires YAML frontmatter in SKILL.md; keep the embedded
    /// template frontmatter intact so Codex loads the scaffolded skills.
    /// trace:BUG-375 | ai:codex
    fn generate_codex_skill(&self, skill_name: &str) -> String {
        use crate::templates::EMBEDDED_TEMPLATES;

        let key = format!("skills/{}.md", skill_name);
        EMBEDDED_TEMPLATES
            .get(key.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("# {}\n\n(template not found)", skill_name))
    }
}

/// README scaffolded into `.claude/skills/local/` so a new project sees the
/// per-project skill-extension contract the first time it pokes around in
/// `.claude/skills/`. Kept verbatim in sync with `docs/extending-skills.md`
/// so the README is a TL;DR pointer, not a competing source of truth.
/// trace:STORY-305 | ai:claude
fn generate_local_skills_readme() -> String {
    String::from(
        "# Per-project skill extensions\n\
         \n\
         This directory and the `*.local.md` convention let a project extend\n\
         AIDA's stock skills without forking them. AIDA never writes inside\n\
         `local/`, never writes a `*.local.md` file, and `make sync-templates`\n\
         never touches either — both survive every upgrade and re-scaffold.\n\
         \n\
         ## Two mechanisms, one rule\n\
         \n\
         1. **New skill** — drop `local/<my-skill>.md` here. Claude Code\n\
            discovers it the same way it discovers stock skills. AIDA will\n\
            never overwrite it.\n\
         2. **Extend a stock skill** — alongside `.claude/skills/<name>.md`\n\
            (one level up from this directory), add `<name>.local.md`. When\n\
            `/aida-<name>` is invoked, the stock skill runs first and the\n\
            `.local.md` content is **appended** as project-specific guidance\n\
            with last-word authority — later instructions override earlier\n\
            ones in normal markdown precedence.\n\
         \n\
         ## Tracked, not ignored\n\
         \n\
         Both `local/<my-skill>.md` and `<name>.local.md` are project assets:\n\
         they are intentionally **checked into git** so the whole team picks\n\
         up the project's skill customizations on `git pull`. The scaffolded\n\
         `.gitignore` makes no exception for them; they fall under `.claude/`\n\
         which is tracked by default.\n\
         \n\
         ## Worked example\n\
         \n\
         See `docs/extending-skills.md` for two end-to-end examples (a\n\
         brand-new project-owned skill, and a `<skill>.local.md` extension\n\
         to `/aida-pr`). trace:STORY-305\n",
    )
}

/// Errors that can occur during scaffolding
#[derive(Debug)]
pub enum ScaffoldError {
    /// IO error while reading/writing files
    IoError { path: PathBuf, message: String },
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::IoError { path, message } => {
                write!(f, "IO error at {}: {}", path.display(), message)
            }
        }
    }
}

impl std::error::Error for ScaffoldError {}

/// Generate the starter CLAUDE.local.md template. Two sections (project
/// review feedback + personal habits) with brief inline guidance so the
/// operator sees the shape. Inspired by Arpan Patel's "Beyond the
/// Prompt: Claude Code" article (2026-05-26). trace:TASK-572 | ai:claude
pub(crate) fn generate_claude_local_md() -> String {
    r#"# CLAUDE.local.md — Personal notes for Claude Code

This file is **per-machine** and **gitignored**. It loads at the start of every
Claude Code session in this project, just like `CLAUDE.md`, but it never leaves
your machine. Use it for project-specific rules and personal-habit reminders
that belong to you, not the team.

If you're new to this file: the pattern comes from the Claude Code
community. After every PR review, dump the feedback here. Over time it
becomes a personalized rule file capturing exactly the kind of mistakes
you make most often. Reviewer nits drop noticeably within a couple
weeks of consistent use.

---

## Project review feedback (private)

Rules you've learned from reviewer comments on YOUR PRs in this repo.
Add new lines as you see the feedback. Phrase each as an imperative rule.

<!-- Examples — replace with your own:
- New SQS consumers need a DLQ and alarms in the same PR
- Use `Optional<T>` over null returns
- Tests for new endpoints must include the auth-failure case
- Prefer named tuples over plain dicts for return types with 3+ fields
-->

## Personal habits to correct

Things YOU keep doing that you want Claude to remind you about (or just
do for you). These are about your interaction style and shortcuts, not
project conventions.

<!-- Examples — replace with your own:
- Stop using `console.log`; use the project logger instead
- Always update the OpenAPI spec when adding endpoints
- Run `bun run typecheck` before claiming done (Claude does this for you)
-->

---

## Tips for maintaining this file

- **Keep sections clearly separated.** Project feedback ≠ personal habits.
- **Prune every few weeks.** Things that have become muscle memory can go.
  The file should capture what you're still learning, not what you already
  do automatically.
- **For PROJECT-WIDE rules** (everyone on the team needs them) — those
  belong in `CLAUDE.md` (which is committed), not here.
- **For GENERAL AIDA patterns** that apply across all your AIDA projects —
  those belong in `~/.claude/projects/<slug>/memory/feedback_*.md`, not
  here.
- **`aida findings add`** is the right verb for a pattern observation that
  hasn't earned its rule status yet. Let recurrence promote it.

The `/aida-learn` skill (scaffolded by `aida init`) walks through the
routing decision when you don't know which substrate a rule belongs in.
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn yaml_frontmatter_round_trips_through_check_file_status() {
        // trace:BUG-29 | ai:claude
        // Regression test: at write time wrap_with_aida_header hashes only
        // the post-frontmatter body; check_file_status must use the same
        // scope when re-hashing expected content. Otherwise byte-identical
        // skill files always report `Modified`.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("aida-req.md");
        let raw = "---\nname: aida-req\ndescription: file a requirement\n---\n\
                   # File a requirement\n\nBody text.\n";
        let wrapped = wrap_with_aida_header(std::path::Path::new("aida-req.md"), raw);
        std::fs::write(&file_path, &wrapped).unwrap();
        // The expected content the scaffolder hands to check_file_status is
        // the embedded template (the *raw* form, no header).
        let status = check_file_status(&file_path, raw);
        assert_eq!(
            status,
            FileStatus::Unmodified,
            "byte-identical YAML-frontmatter file should report Unmodified, got {:?}",
            status
        );
    }

    #[test]
    fn checksum_for_stored_header_matches_writer_scope() {
        // trace:BUG-29 | ai:claude
        // With frontmatter: must hash post-frontmatter body only.
        let with_fm = "---\nname: x\n---\n\nbody text\n";
        let body_only = "\nbody text\n";
        assert_eq!(
            checksum_for_stored_header(with_fm),
            compute_checksum(body_only)
        );
        // Without frontmatter: must hash the whole content.
        let no_fm = "# heading\n\nbody\n";
        assert_eq!(checksum_for_stored_header(no_fm), compute_checksum(no_fm));
    }

    fn create_test_store() -> RequirementsStore {
        RequirementsStore {
            name: "test-project".to_string(),
            title: "Test Project".to_string(),
            description: "A test project for scaffolding".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_default_config() {
        let config = ScaffoldConfig::default();
        assert!(config.generate_claude_md);
        assert!(config.generate_agents_md);
        assert!(config.generate_commands);
        assert!(config.generate_skills);
        assert!(config.generate_codex_skills);
        assert!(config.include_aida_req_skill);
        assert!(config.include_aida_implement_skill);
        assert!(config.include_aida_capture_skill);
        assert_eq!(config.project_type, ProjectType::Generic);
    }

    #[test]
    fn test_preview_generates_expected_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);

        // Should have CLAUDE.md, 2 commands, and 2 skills
        assert!(!preview.artifacts.is_empty());

        // Check that CLAUDE.md is generated
        let claude_md = preview
            .artifacts
            .iter()
            .find(|a| a.path == PathBuf::from("CLAUDE.md"));
        assert!(claude_md.is_some());
        assert!(claude_md.unwrap().content.contains("Test Project"));
    }

    #[test]
    fn test_apply_creates_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        let result = scaffolder.apply(&preview);

        assert!(result.is_ok());

        // Check that CLAUDE.md was created
        assert!(temp_dir.path().join("CLAUDE.md").exists());
        assert!(temp_dir.path().join("AGENTS.md").exists());
        // trace:TASK-572 | ai:claude — CLAUDE.local.md scaffolds the
        // structured personal-notes template.
        let claude_local = temp_dir.path().join("CLAUDE.local.md");
        assert!(
            claude_local.exists(),
            "CLAUDE.local.md should be scaffolded by default"
        );
        let claude_local_content = std::fs::read_to_string(&claude_local).unwrap();
        assert!(
            claude_local_content.contains("Project review feedback"),
            "CLAUDE.local.md should have the PR-feedback section"
        );
        assert!(
            claude_local_content.contains("Personal habits to correct"),
            "CLAUDE.local.md should have the personal-habits section"
        );
        assert!(temp_dir.path().join(".aida/reserved-paths.toml").exists());
        assert!(temp_dir.path().join(".aida/agents.toml").exists());
        let reserved_paths =
            std::fs::read_to_string(temp_dir.path().join(".aida/reserved-paths.toml")).unwrap();
        toml::from_str::<toml::Value>(&reserved_paths)
            .expect("scaffolded reserved-paths.toml must be valid TOML");
        let agents = std::fs::read_to_string(temp_dir.path().join(".aida/agents.toml")).unwrap();
        toml::from_str::<toml::Value>(&agents).expect("scaffolded agents.toml must be valid TOML");
        assert!(
            !agents.lines().any(|line| line.starts_with("default_flags")),
            "scaffolded agents.toml should not enable live defaults"
        );

        // Check that .claude directories were created
        assert!(temp_dir.path().join(".claude/commands").exists());
        assert!(temp_dir.path().join(".claude/skills").exists());
        assert!(temp_dir.path().join(".codex/skills").exists());
        // BUG-375: Codex skips scaffolded skills without YAML frontmatter.
        // The AIDA header must be inserted after the frontmatter, not before
        // it, so Codex sees `---` at byte 0 of SKILL.md.
        for entry in std::fs::read_dir(temp_dir.path().join(".codex/skills")).unwrap() {
            let skill_path = entry.unwrap().path().join("SKILL.md");
            let content = std::fs::read_to_string(&skill_path).unwrap();
            assert!(
                content.starts_with("---\nname:"),
                "{} should start with Codex-readable YAML frontmatter",
                skill_path.display()
            );
            assert!(
                content.contains("\n---\n<!-- AIDA Generated:"),
                "{} should keep the AIDA header after YAML frontmatter",
                skill_path.display()
            );
        }
        assert!(temp_dir
            .path()
            .join("docs/agents/cross-agent-onboarding.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/codex-brief-pickup.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/codex-mcp-setup.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/antigravity-brief-pickup.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/per-agent-config.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/aida-mcp-install-matrix.md")
            .exists());
        assert!(temp_dir
            .path()
            .join("docs/agents/session-communication.md")
            .exists());
        assert!(temp_dir.path().join("docs/extending-skills.md").exists());
    }

    /// trace:TASK-572 | ai:claude — CLAUDE.local.md must never overwrite
    /// existing personal notes. The whole point of the file is that
    /// operators accumulate rules in it over time; silently dropping
    /// that on a re-run would be catastrophic.
    #[test]
    fn claude_local_md_never_overwrites_existing_personal_notes() {
        let temp_dir = TempDir::new().unwrap();
        let existing_path = temp_dir.path().join("CLAUDE.local.md");
        let personal_notes = "# My accumulated rules\n\n- One: never push to main directly\n";
        std::fs::write(&existing_path, personal_notes).unwrap();

        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();
        let preview = scaffolder.preview(&store);
        let result = scaffolder.apply(&preview);
        assert!(result.is_ok());

        // The existing CLAUDE.local.md should be UNCHANGED — the scaffold
        // path takes the not-exists branch only.
        let after = std::fs::read_to_string(&existing_path).unwrap();
        assert_eq!(
            after, personal_notes,
            "scaffold must not overwrite existing CLAUDE.local.md personal notes"
        );
    }

    #[test]
    fn preview_includes_cross_agent_onboarding_docs() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        let paths: Vec<PathBuf> = preview.artifacts.iter().map(|a| a.path.clone()).collect();

        assert!(paths.contains(&PathBuf::from("docs/agents/cross-agent-onboarding.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/codex-brief-pickup.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/codex-mcp-setup.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/antigravity-mcp-setup.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/antigravity-brief-pickup.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/per-agent-config.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/aida-mcp-install-matrix.md")));
        assert!(paths.contains(&PathBuf::from("docs/agents/session-communication.md")));
        assert!(paths.contains(&PathBuf::from("docs/extending-skills.md")));
        assert!(paths.contains(&PathBuf::from(".aida/reserved-paths.toml")));
        assert!(paths.contains(&PathBuf::from(".aida/agents.toml")));
    }

    /// STORY-305: `aida scaffold apply` must create `.claude/skills/local/`
    /// with a README so the project sees the per-project skill-extension
    /// surface the first time they look at `.claude/skills/`.
    /// trace:STORY-305 | ai:claude
    #[test]
    fn test_folder_form_skill_scaffolded() {
        // A folder-form skill scaffolds its whole subfolder tree: the prompt
        // body AND its helper files. trace:TASK-574
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();
        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).expect("scaffolding apply");

        let skills_dir = temp_dir.path().join(".claude/skills");
        assert!(
            skills_dir.join("aida-pr/SKILL.md").exists(),
            "folder-form skill prompt should scaffold to <name>/SKILL.md"
        );
        assert!(
            skills_dir
                .join("aida-pr/examples/pr-description-template.md")
                .exists(),
            "folder-form skill helper files should scaffold under the skill folder"
        );
        assert!(
            !skills_dir.join("aida-pr.md").exists(),
            "migrated skill should not also scaffold a flat <name>.md"
        );
    }

    #[test]
    fn test_local_skills_dir_and_readme_scaffolded() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).expect("scaffolding apply");

        let local_dir = temp_dir.path().join(".claude/skills/local");
        let readme = local_dir.join("README.md");
        assert!(local_dir.is_dir(), ".claude/skills/local/ should be a dir");
        assert!(readme.is_file(), "README.md should be scaffolded");

        let body = std::fs::read_to_string(&readme).unwrap();
        // The README must teach both mechanisms and the append-merge rule.
        assert!(
            body.contains("local/<my-skill>.md"),
            "README should document the project-owned new-skill path"
        );
        assert!(
            body.contains("<name>.local.md"),
            "README should document the stock-skill extension path"
        );
        assert!(
            body.to_lowercase().contains("append"),
            "README should document append-merge semantics"
        );
    }

    /// STORY-481: the `/aida-techdebt` skill (end-of-session duplication
    /// scan) must be scaffolded by `aida init` — both the skill and its
    /// matching slash command. It rides the BUG-386 catch-all loop (it's
    /// not a flag-gated daily driver), so this test also guards that loop
    /// for a Claude-only skill. trace:STORY-481 | ai:claude
    #[test]
    fn test_techdebt_skill_and_command_scaffolded() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).expect("scaffolding apply");

        let skill = temp_dir.path().join(".claude/skills/aida-techdebt.md");
        let command = temp_dir.path().join(".claude/commands/aida-techdebt.md");
        assert!(skill.is_file(), "aida-techdebt skill should be scaffolded");
        assert!(
            command.is_file(),
            "aida-techdebt command should be scaffolded"
        );

        // The skill must keep its acceptance contract: a read-only scan
        // that composes with `aida findings add`.
        let body = std::fs::read_to_string(&skill).unwrap();
        assert!(
            body.contains("aida findings add"),
            "techdebt skill should compose with `aida findings add`"
        );
        assert!(
            body.to_lowercase().contains("read-only"),
            "techdebt skill should describe a read-only scan"
        );
    }

    /// STORY-305: applying the scaffolder a second time MUST NOT touch a
    /// pre-existing project-owned local skill under `.claude/skills/local/`,
    /// nor a pre-existing `<name>.local.md` extension alongside a stock
    /// skill. They're project assets — AIDA never overwrites them.
    /// trace:STORY-305 | ai:claude
    #[test]
    fn test_resync_preserves_local_skill_extensions() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config.clone());
        let store = create_test_store();

        // First apply lays down the skills + the local/ dir + README.
        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).expect("first apply");

        // Project drops in two extensions: a brand-new project-owned skill
        // and an extension to the (stock) aida-pr skill.
        let project_skill = temp_dir.path().join(".claude/skills/local/my-deploy.md");
        let project_skill_body = "# /my-deploy\n\nProject-owned skill body.\n";
        std::fs::write(&project_skill, project_skill_body).unwrap();

        let pr_local = temp_dir.path().join(".claude/skills/aida-pr.local.md");
        let pr_local_body = "## Project addendum\n\nTitle must start with [SPEC-ID].\n";
        std::fs::write(&pr_local, pr_local_body).unwrap();

        // Second apply (the "re-sync" path). Must leave both files alone.
        let mut scaffolder2 = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let preview2 = scaffolder2.preview(&store);
        scaffolder2.apply(&preview2).expect("second apply");

        // Neither file should have been deleted, renamed, or rewritten.
        assert!(
            project_skill.is_file(),
            "project-owned local/ skill must survive re-apply"
        );
        assert_eq!(
            std::fs::read_to_string(&project_skill).unwrap(),
            project_skill_body,
            "project-owned local/ skill body must be byte-identical after re-apply"
        );
        assert!(
            pr_local.is_file(),
            "<skill>.local.md extension must survive re-apply"
        );
        assert_eq!(
            std::fs::read_to_string(&pr_local).unwrap(),
            pr_local_body,
            "<skill>.local.md extension body must be byte-identical after re-apply"
        );

        // The preview must not even *list* these files as artifacts —
        // structurally proving the apply loop never had them to touch.
        assert!(
            !preview2
                .artifacts
                .iter()
                .any(|a| a.path == PathBuf::from(".claude/skills/local/my-deploy.md")),
            "scaffolder must never own a project-owned local/ skill"
        );
        assert!(
            !preview2
                .artifacts
                .iter()
                .any(|a| a.path == PathBuf::from(".claude/skills/aida-pr.local.md")),
            "scaffolder must never own a *.local.md extension"
        );
    }

    /// STORY-305: the always-imported `.claude/AIDA.md` conventions file
    /// MUST teach Claude Code the append-merge rule for `<name>.local.md`,
    /// so the merge happens for every session without per-skill changes.
    /// trace:STORY-305 | ai:claude
    #[test]
    fn test_aida_md_documents_local_extension_rule() {
        let temp_dir = TempDir::new().unwrap();
        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);
        let aida_md = preview
            .artifacts
            .iter()
            .find(|a| a.path == PathBuf::from(".claude/AIDA.md"))
            .expect(".claude/AIDA.md should be scaffolded");

        let body = &aida_md.content;
        assert!(
            body.contains("Per-project skill extensions"),
            "AIDA.md should document the per-project extension section"
        );
        assert!(
            body.contains(".local.md"),
            "AIDA.md should name the .local.md convention so Claude Code reads it"
        );
        assert!(
            body.to_lowercase().contains("append"),
            "AIDA.md should specify append-merge semantics"
        );
    }

    #[test]
    fn test_project_type_labels() {
        assert_eq!(ProjectType::Rust.label(), "Rust");
        assert_eq!(ProjectType::Python.label(), "Python");
        assert_eq!(ProjectType::Generic.label(), "Generic");
    }

    #[test]
    fn test_diff_slice_claude_md_present_import() {
        // trace:FR-1-027 | ai:claude
        let actual = "# CLAUDE.md\n\nFoo bar.\n\n@.claude/AIDA.md\n\n## Project\n";
        let expected = "# CLAUDE.md\n\n@.claude/AIDA.md\n\n## Project overview\n";
        let slice = aida_managed_diff_slice(Path::new("CLAUDE.md"), expected, actual);
        assert!(matches!(slice, DiffSlice::Match));
    }

    #[test]
    fn test_diff_slice_claude_md_missing_import() {
        let actual = "# CLAUDE.md\n\nFoo bar.\n\n## Project\n";
        let expected = "# CLAUDE.md\n\n@.claude/AIDA.md\n";
        let slice = aida_managed_diff_slice(Path::new("CLAUDE.md"), expected, actual);
        match slice {
            DiffSlice::MarkerMissing { message } => {
                assert!(message.contains("@.claude/AIDA.md"));
            }
            other => panic!("expected MarkerMissing, got {:?}", other),
        }
    }

    #[test]
    fn test_diff_slice_agents_md_block_match() {
        let block = "<!-- AIDA-AUTOGEN-BEGIN -->\nfoo\n<!-- AIDA-AUTOGEN-END -->";
        let actual = format!("# user content above\n{}\n# user content below", block);
        let expected = format!("# different preamble\n{}\n# different postamble", block);
        let slice = aida_managed_diff_slice(Path::new("AGENTS.md"), &expected, &actual);
        assert!(matches!(slice, DiffSlice::Match));
    }

    #[test]
    fn test_diff_slice_agents_md_block_diverged() {
        let actual_block = "<!-- AIDA-AUTOGEN-BEGIN -->\nold body\n<!-- AIDA-AUTOGEN-END -->";
        let expected_block = "<!-- AIDA-AUTOGEN-BEGIN -->\nnew body\n<!-- AIDA-AUTOGEN-END -->";
        let actual = format!("user1\n{}\nuser2", actual_block);
        let expected = format!("DIFFERENT\n{}\nUSER STUFF", expected_block);
        let slice = aida_managed_diff_slice(Path::new("AGENTS.md"), &expected, &actual);
        match slice {
            DiffSlice::SliceDiff {
                expected: e,
                actual: a,
                ..
            } => {
                assert!(e.contains("new body") && !e.contains("DIFFERENT"));
                assert!(a.contains("old body") && !a.contains("user1"));
            }
            other => panic!("expected SliceDiff, got {:?}", other),
        }
    }

    #[test]
    fn test_diff_slice_agents_md_user_opted_out() {
        // No AIDA-AUTOGEN markers on disk → user opted out, treat as match.
        let actual = "fully user-owned, no markers";
        let expected = "<!-- AIDA-AUTOGEN-BEGIN -->\nfoo\n<!-- AIDA-AUTOGEN-END -->";
        let slice = aida_managed_diff_slice(Path::new("AGENTS.md"), expected, actual);
        assert!(matches!(slice, DiffSlice::Match));
    }

    #[test]
    fn test_diff_slice_settings_json_key_reorder_is_match() {
        // trace:BUG-1-066 | ai:claude
        let expected = r#"{
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": []}],
                "PostToolUse": [{"matcher": "Bash", "hooks": []}]
            },
            "statusLine": {
                "type": "command",
                "command": "aida statusline"
            }
        }"#;
        // Same semantic content, different JSON key ordering everywhere.
        let actual = r#"{
            "statusLine": {
                "command": "aida statusline",
                "type": "command"
            },
            "hooks": {
                "PostToolUse": [{"hooks": [], "matcher": "Bash"}],
                "PreToolUse": [{"hooks": [], "matcher": "Bash"}]
            }
        }"#;
        let slice = aida_managed_diff_slice(Path::new(".claude/settings.json"), expected, actual);
        assert!(
            matches!(slice, DiffSlice::Match),
            "key reordering must not surface as drift"
        );
    }

    #[test]
    fn test_diff_slice_settings_json_real_drift() {
        // Real semantic drift: user changed the statusLine command.
        let expected = r#"{
            "hooks": {
                "PreToolUse": [],
                "PostToolUse": [],
                "SessionStart": []
            },
            "statusLine": {"type": "command", "command": "aida statusline"}
        }"#;
        let actual = r#"{
            "hooks": {
                "PreToolUse": [],
                "PostToolUse": [],
                "SessionStart": []
            },
            "statusLine": {"type": "command", "command": "echo hi"}
        }"#;
        let slice = aida_managed_diff_slice(Path::new(".claude/settings.json"), expected, actual);
        match slice {
            DiffSlice::SliceDiff {
                expected,
                actual,
                note,
            } => {
                assert!(note.contains("AIDA-managed JSON slots"));
                assert!(expected.contains("aida statusline"));
                assert!(actual.contains("echo hi"));
                // Only the differing slot should appear, not the others.
                assert!(expected.contains("/statusLine"));
                assert!(!expected.contains("/hooks/PreToolUse"));
            }
            other => panic!("expected SliceDiff, got {:?}", other),
        }
    }

    #[test]
    fn test_diff_slice_settings_json_invalid_falls_back() {
        let slice = aida_managed_diff_slice(
            Path::new(".claude/settings.json"),
            "{ valid: \"json\" }",
            "not json at all",
        );
        assert!(matches!(slice, DiffSlice::FullDiff { .. }));
    }

    #[test]
    fn test_diff_slice_other_files_full_diff() {
        let actual = "line one\n";
        let expected = "line one\nline two\n";
        let slice = aida_managed_diff_slice(Path::new(".claude/skills/foo.md"), expected, actual);
        match slice {
            DiffSlice::FullDiff { .. } => {}
            other => panic!("expected FullDiff, got {:?}", other),
        }
    }

    #[test]
    fn test_pre_commit_hook_scaffolded_and_works() {
        use std::process::Command;
        let temp_dir = TempDir::new().unwrap();

        // Helper to run git commands with cleared parent git environment variables
        let run_git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.args(args);
            cmd.current_dir(temp_dir.path());
            for (key, _) in std::env::vars() {
                if key.starts_with("GIT_") {
                    cmd.env_remove(&key);
                }
            }
            let status = cmd.status().unwrap();
            assert!(status.success(), "git command {:?} failed", args);
        };

        // Initialize git
        run_git(&["init"]);

        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();

        let preview = scaffolder.preview(&store);

        // Ensure .git/hooks/pre-commit is in the preview artifacts
        let pre_commit_art = preview
            .artifacts
            .iter()
            .find(|a| a.path == PathBuf::from(".git/hooks/pre-commit"));
        assert!(
            pre_commit_art.is_some(),
            "pre-commit hook should be scaffolded"
        );

        // Apply the scaffolding
        scaffolder.apply(&preview).unwrap();

        let hook_path = temp_dir.path().join(".git/hooks/pre-commit");
        assert!(hook_path.exists(), "pre-commit hook file should exist");

        // Setup dummy git configs for commit
        run_git(&["config", "user.email", "test@aida.dev"]);
        run_git(&["config", "user.name", "AIDA Test"]);

        // Write a test .gitignore file
        std::fs::write(
            temp_dir.path().join(".gitignore"),
            "target/\n.aida-store/\n",
        )
        .unwrap();

        // Make it executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Helper to run hook with cleared parent git environment variables
        let run_hook = |env_var: Option<(&str, &str)>| -> std::process::Output {
            // Windows cannot execute POSIX hook scripts directly as Win32 programs;
            // Git for Windows runs them through its shell. trace:BUG-346 | ai:codex
            let mut cmd = if cfg!(windows) {
                let mut cmd = Command::new("sh");
                cmd.arg(&hook_path);
                cmd
            } else {
                Command::new(&hook_path)
            };
            cmd.current_dir(temp_dir.path());
            for (key, _) in std::env::vars() {
                if key.starts_with("GIT_") {
                    cmd.env_remove(&key);
                }
            }
            if let Some((k, v)) = env_var {
                cmd.env(k, v);
            }
            cmd.output().unwrap()
        };

        // Verify that running the hook with no staged files exits 0
        let output = run_hook(None);
        assert!(
            output.status.success(),
            "hook should succeed when no files staged"
        );

        // 1. Stage a normal (non-ignored) file
        let source_path = temp_dir.path().join("main.rs");
        std::fs::write(&source_path, "fn main() {}").unwrap();

        run_git(&["add", "main.rs", ".gitignore"]);

        // Run the hook, should succeed!
        let output = run_hook(None);
        assert!(
            output.status.success(),
            "hook should succeed on non-ignored source file"
        );

        // 2. Stage a gitignored file
        let ignored_dir = temp_dir.path().join("target");
        std::fs::create_dir_all(&ignored_dir).unwrap();
        let ignored_file = ignored_dir.join("debug.log");
        std::fs::write(&ignored_file, "build log").unwrap();

        run_git(&["add", "-f", "target/debug.log"]);

        // Run the hook, should fail!
        let output = run_hook(None);
        println!("HOOK ATTEMPT STATUS: {:?}", output.status.code());
        println!(
            "HOOK ATTEMPT STDOUT: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        println!(
            "HOOK ATTEMPT STDERR: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success(),
            "hook should refuse gitignored file"
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("is gitignored"),
            "error message should say file is gitignored"
        );
        assert!(
            stderr.contains("substrate-as-bouncer.md"),
            "error message should refer to discipline guide"
        );

        // 3. Test bypass via environment variable
        let output = run_hook(Some(("AIDA_ALLOW_INTERMEDIATE", "1")));
        assert!(
            output.status.success(),
            "hook should succeed when AIDA_ALLOW_INTERMEDIATE=1 is set"
        );

        // 4. Test bypass via .aida-store branch exemption
        // Create an aida-store branch
        run_git(&["checkout", "-b", "aida-store"]);

        let output = run_hook(None);
        assert!(
            output.status.success(),
            "hook should succeed when on aida-store branch"
        );
    }

    // trace:TASK-503 | ai:antigravity
    // Verifies the pre-commit hook's auto-fmt section reformats and re-stages
    // a fmt-drifty Rust file so CI's `cargo fmt --all -- --check` cannot fail
    // on local commits. Origin: three fmt-CI failures in one session on
    // 2026-05-23 (BUG-360, TASK-497, TASK-490) — the substrate-as-bouncer fix.
    #[test]
    fn test_pre_commit_hook_runs_cargo_fmt_on_staged_rust_files() {
        use std::process::Command;
        let temp_dir = TempDir::new().unwrap();

        let run_git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.args(args);
            cmd.current_dir(temp_dir.path());
            for (key, _) in std::env::vars() {
                if key.starts_with("GIT_") {
                    cmd.env_remove(&key);
                }
            }
            let status = cmd.status().unwrap();
            assert!(status.success(), "git command {:?} failed", args);
        };

        run_git(&["init"]);

        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();
        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).unwrap();

        let hook_path = temp_dir.path().join(".git/hooks/pre-commit");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Minimal Cargo project so `cargo fmt --all` has something to format.
        std::fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[package]\nname = \"fmt-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"fmt-test\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp_dir.path().join("src")).unwrap();

        // Deliberately fmt-drifty: missing spaces, packed statements.
        let messy = "fn main(){let x=1;let y=2;println!(\"{}\",x+y);}\n";
        std::fs::write(temp_dir.path().join("src/main.rs"), messy).unwrap();
        std::fs::write(temp_dir.path().join(".gitignore"), "target/\n").unwrap();

        run_git(&["config", "user.email", "test@aida.dev"]);
        run_git(&["config", "user.name", "AIDA Test"]);
        run_git(&["add", "src/main.rs", "Cargo.toml", ".gitignore"]);

        // Run the hook directly.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("sh");
            c.arg(&hook_path);
            c
        } else {
            Command::new(&hook_path)
        };
        cmd.current_dir(temp_dir.path());
        for (key, _) in std::env::vars() {
            if key.starts_with("GIT_") {
                cmd.env_remove(&key);
            }
        }
        let output = cmd.output().unwrap();

        // Skip the test (don't fail) if cargo or rustfmt aren't on PATH —
        // the hook's --check branch returns non-zero, but if cargo itself
        // can't run we shouldn't claim a regression.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("command not found") || stderr.contains("rustfmt") {
            eprintln!("skipping: cargo/rustfmt unavailable in test env: {stderr}");
            return;
        }

        assert!(
            output.status.success(),
            "hook should succeed (cargo fmt re-formats and re-stages). stderr: {stderr}"
        );

        let after = std::fs::read_to_string(temp_dir.path().join("src/main.rs")).unwrap();
        assert_ne!(
            after, messy,
            "src/main.rs should have been reformatted by the hook"
        );
        // rustfmt produces multi-line output for this packed code.
        assert!(
            after.lines().count() > 1,
            "reformatted file should be multi-line, got: {after:?}"
        );
    }

    // trace:TASK-135 | ai:claude
    // Verifies the pre-commit hook's substrate-as-bouncer gate rejects a
    // SPEC-ID trace marker on a `///` doc comment (which clap would leak into
    // `--help`), and lets a plain `//` trace comment through. Catches the
    // "both-at-once trap" (TASK-268 / BUG-227) at the moment of writing rather
    // than minutes later in CI (source_doc_comments_carry_no_trace_token).
    #[test]
    fn test_pre_commit_hook_rejects_trace_marker_on_doc_comment() {
        use std::process::Command;
        let temp_dir = TempDir::new().unwrap();

        let run_git = |args: &[&str]| {
            let mut cmd = Command::new("git");
            cmd.args(args);
            cmd.current_dir(temp_dir.path());
            for (key, _) in std::env::vars() {
                if key.starts_with("GIT_") {
                    cmd.env_remove(&key);
                }
            }
            let status = cmd.status().unwrap();
            assert!(status.success(), "git command {:?} failed", args);
        };

        run_git(&["init"]);

        let config = ScaffoldConfig::default();
        let mut scaffolder = Scaffolder::new(temp_dir.path().to_path_buf(), config);
        let store = create_test_store();
        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).unwrap();

        let hook_path = temp_dir.path().join(".git/hooks/pre-commit");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::fs::create_dir_all(temp_dir.path().join("src")).unwrap();
        std::fs::write(temp_dir.path().join(".gitignore"), "target/\n").unwrap();
        run_git(&["config", "user.email", "test@aida.dev"]);
        run_git(&["config", "user.name", "AIDA Test"]);

        let run_hook = || {
            let mut cmd = if cfg!(windows) {
                let mut c = Command::new("sh");
                c.arg(&hook_path);
                c
            } else {
                Command::new(&hook_path)
            };
            cmd.current_dir(temp_dir.path());
            for (key, _) in std::env::vars() {
                if key.starts_with("GIT_") {
                    cmd.env_remove(&key);
                }
            }
            cmd.output().unwrap()
        };

        // Offending: a `trace:` marker on a `///` doc comment leaks into --help.
        let bad = "/// trace:STORY-1 | ai:claude\npub fn foo() {}\n";
        std::fs::write(temp_dir.path().join("src/leaky.rs"), bad).unwrap();
        run_git(&["add", "src/leaky.rs", ".gitignore"]);

        let output = run_hook();
        assert!(
            !output.status.success(),
            "hook must reject a `///` doc comment carrying a trace marker"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("src/leaky.rs:1:"),
            "rejection should name the offending file:line, got: {stderr}"
        );

        // Fix: a plain `//` trace comment is the correct form — must pass the gate.
        let good = "// trace:STORY-1 | ai:claude\npub fn foo() {}\n";
        std::fs::write(temp_dir.path().join("src/leaky.rs"), good).unwrap();
        run_git(&["add", "src/leaky.rs"]);

        let output = run_hook();
        let stderr = String::from_utf8_lossy(&output.stderr);
        // The fmt step may warn if cargo/rustfmt is unavailable; the trace gate
        // is independent of it, so assert only that no trace offender fired.
        assert!(
            !stderr.contains("SPEC-ID trace marker") && !stderr.contains("Offending lines"),
            "plain `//` trace comment must not trip the doc-comment gate, got: {stderr}"
        );
    }
}
