use super::*;

/// Delimiter markers for the inlined AIDA conventions block in AGENTS.md
/// (and any other agent-context file that can't follow `@` imports). The
/// scaffold status path uses these to extract & compare just the block,
/// leaving user content outside the markers freely editable.
/// trace:FR-1-035 | ai:claude
pub(super) const AIDA_BLOCK_BEGIN: &str = "<!-- AIDA-AUTOGEN-BEGIN -->";
pub(super) const AIDA_BLOCK_END: &str = "<!-- AIDA-AUTOGEN-END -->";

/// Heading that opens the Claude-Code-skills documentation section of
/// `.claude/AIDA.md`. The whole section is gated by `generate_skills`, so a
/// project initialized with `aida init --no-skills` writes an AIDA.md that
/// stops *before* this heading. The drift check must tolerate that — see
/// `aida_md_matches`. trace:TASK-125 | ai:claude
const AIDA_MD_SKILLS_HEADING: &str = "## Claude Code skills";

/// Compare an on-disk `.claude/AIDA.md` against the freshly-regenerated
/// expected content, tolerant of the one legitimately-optional section.
///
/// `generate_aida_md` appends a "Claude Code skills (slash commands)"
/// section only when `config.generate_skills` is true. The `aida status`
/// scaffold drift check always regenerates with the default config
/// (`generate_skills = true`) — it has no record of an `aida init
/// --no-skills` having dropped the section — so a clean `--no-skills` init
/// would otherwise report `.claude/AIDA.md` as STALE the instant it was
/// written. We split both sides at the skills heading and:
///
/// - always require the pre-skills *body* to match (after stripping the
///   AIDA-Generated header lines, which carry a content checksum), and
/// - require the skills section to match only when *both* sides have it.
///   When the on-disk file omits it (the `--no-skills` case), the absence
///   is treated as a deliberate opt-out, not drift.
///
/// trace:TASK-125 | ai:claude
pub fn aida_md_matches(actual: &str, expected: &str) -> bool {
    let a = strip_aida_header(actual);
    let e = strip_aida_header(expected);

    let (a_body, a_skills) = split_at_skills(a);
    let (e_body, e_skills) = split_at_skills(e);

    if a_body.trim() != e_body.trim() {
        return false;
    }

    match (a_skills, e_skills) {
        // Both carry the skills section — it must match.
        (Some(a_s), Some(e_s)) => a_s.trim() == e_s.trim(),
        // On-disk file omits the section (opted out via --no-skills). The
        // expected content may or may not include it; either way the absence
        // on disk is deliberate, not drift.
        (None, _) => true,
        // On-disk has the section but expected doesn't — unusual, but lean
        // toward "matching" rather than flagging drift on a section the
        // current template no longer emits.
        (Some(_), None) => true,
    }
}

/// Split AIDA.md content into (body-before-skills, optional-skills-section).
fn split_at_skills(content: &str) -> (&str, Option<&str>) {
    match content.find(AIDA_MD_SKILLS_HEADING) {
        Some(idx) => (&content[..idx], Some(&content[idx..])),
        None => (content, None),
    }
}

/// Drop the leading `<!-- AIDA Generated: ... -->` / `<!-- To customize:
/// ... -->` header lines so two AIDA.md bodies can be compared on their
/// substance, not the embedded per-content checksum. Non-header content is
/// returned unchanged.
fn strip_aida_header(content: &str) -> &str {
    let mut rest = content;
    loop {
        let trimmed = rest.trim_start_matches(['\n', '\r']);
        if let Some(after) = trimmed.strip_prefix("<!-- AIDA Generated:") {
            // Skip to the end of this comment's line.
            rest = match after.find('\n') {
                Some(nl) => &after[nl + 1..],
                None => "",
            };
            continue;
        }
        if let Some(after) = trimmed.strip_prefix("<!-- To customize:") {
            rest = match after.find('\n') {
                Some(nl) => &after[nl + 1..],
                None => "",
            };
            continue;
        }
        return trimmed;
    }
}

impl Scaffolder {
    /// Generate the canonical AIDA conventions content. Single source of
    /// truth for trace format, commit format, daily commands, and capture
    /// rules — replaces the duplicated chunks that lived in claude_md.rs
    /// and codex_md.rs. Written verbatim to `.claude/AIDA.md` (Claude
    /// Code expands `@.claude/AIDA.md` on its side) and inlined into
    /// AGENTS.md inside delimiters for agents that don't follow imports.
    /// trace:FR-1-035 | ai:claude
    pub(super) fn generate_aida_md(&self, store: &RequirementsStore) -> String {
        // Built up via push_str rather than format!() because the body has
        // code examples with `{` and `}` that would need to be doubled to
        // survive a format string. Pieces with substitution use format!()
        // locally, then get concatenated.
        let db_filename = self.database_filename();
        let _ = store; // TASK-1-098: req count removed from AIDA.md; `store`
                       // is still in the signature for forward-compat (other
                       // generators may need it).

        let storage_note = if self.is_sqlite_database() {
            format!(
                "Requirements database: `{}` (SQLite — legacy mode; new projects use the \
                 distributed git-canonical store at `.aida-store/`).",
                db_filename
            )
        } else {
            "Requirements database: distributed git-canonical store at `.aida-store/` \
             (orphan branch `aida-store`, plus a rebuildable SQLite cache at `.aida/cache.db`)."
                .to_string()
        };

        let mut s = String::with_capacity(4096);
        s.push_str(
            "# AIDA Conventions\n\
             \n\
             This file is the single source of truth for AIDA's coding conventions in\n\
             this project. CLAUDE.md imports it via `@.claude/AIDA.md`; AGENTS.md\n\
             inlines a copy inside auto-generated delimiters. Edit this file to change\n\
             the conventions for both.\n\
             \n\
             ## Requirements management\n\
             \n\
             This project tracks requirements with [AIDA](https://github.com/joemooney/aida).\n\
             **Do not maintain a separate `REQUIREMENTS.md`** — the requirements DB is\n\
             canonical.\n\
             \n",
        );
        s.push_str(&storage_note);
        // TASK-1-098 (BUG-386 sibling): don't embed a derived 'Currently
        // tracking N requirements' count into .claude/AIDA.md. The count
        // changes every time the substrate grows, which triggered STALE
        // drift on every scaffold-status check (compared against a
        // freshly-regenerated reference with the CURRENT count). Use
        // `aida list` for project size; this file is conventions, not
        // state. trace:TASK-1-098 | ai:claude
        s.push_str("\n\n");
        s.push_str(
            "### Daily commands\n\
             \n\
             ```bash\n\
             aida list                              # list all requirements (cache-backed)\n\
             aida list --status draft               # filter by status\n\
             aida show <ID>                         # show details (e.g. `aida show FR-0042`)\n\
             aida search \"<query>\"                  # full-text search\n\
             aida add --title \"...\" --type <type> --status draft\n\
             aida edit <ID> --status in-progress\n\
             aida edit <ID> --status completed\n\
             aida comment add <ID> \"implementation note...\"\n\
             aida rel add --from <ID> --to <ID> --type <Parent|Verifies|References>\n\
             aida history                           # what was touched recently (digest)\n\
             aida statusline                        # one-line: project · role · queue · cache\n\
             ```\n\
             \n\
             ### Requirement-first development\n\
             \n\
             1. **Before coding:** check whether the work has a SPEC-ID. If not, create one\n\
             \x20  (`aida add --type <task|story|bug|...> --status approved --title \"...\"`).\n\
             2. **During coding:** add inline trace comments referencing the SPEC-ID.\n\
             3. **Before committing:** mark the requirement `completed` (or `in-progress`\n\
             \x20  if work continues), and ensure the commit message references it.\n\
             \n\
             ## Inline trace comments\n\
             \n\
             Add a comment near the code that implements (or fixes, or verifies) a\n\
             requirement:\n\
             \n\
             ```rust\n\
             // trace:FR-0042 | ai:claude\n\
             fn implement_feature() { /* ... */ }\n\
             ```\n\
             \n\
             Format: `// trace:<SPEC-ID> | ai:<tool>[:<confidence>]`\n\
             \n\
             - `<SPEC-ID>` — e.g. `FR-0042`, `BUG-1-017`, `TASK-0344`\n\
             - `<tool>` — `claude`, `codex`, `copilot`, `human`, `aider`, …\n\
             - `<confidence>` — optional: `high` (implied), `med` (40-80% AI), `low` (<40% AI)\n\
             \n\
             ## Commit message format\n\
             \n\
             ```\n\
             [AI:tool] type(scope): description (REQ-ID)\n\
             ```\n\
             \n\
             Examples:\n\
             \n\
             ```\n\
             [AI:claude] feat(auth): add login validation (FR-0042)\n\
             [AI:claude:med] fix(api): handle null response (BUG-0023)\n\
             [AI:antigravity+claude] test(hooks): accept mixed authorship (TASK-509)\n\
             chore(deps): update dependencies        # no REQ-ID needed\n\
             docs: update README                     # no REQ-ID needed\n\
             ```\n\
             \n\
             Rules:\n\
             \n\
             - `[AI:tool]` required when commit includes AI-assisted code (any file with a\n\
             \x20  `// trace:... | ai:tool` comment changed). Use `[AI:tool1+tool2]` for\n\
             \x20  mixed-agent authorship, with optional confidence on the whole commit\n\
             \x20  (`[AI:tool1+tool2:med]`).\n\
             - `type` required: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,\n\
             \x20  `build`, `ci`, `chore`, `revert`.\n\
             - `(scope)` optional — component or area affected.\n\
             - `(REQ-ID)` required for `feat`/`fix`; optional for `chore`/`docs`.\n\
             \n\
             Set `AIDA_COMMIT_STRICT=true` (or commit through the `/aida-commit` skill) to\n\
             enforce; otherwise the commit-msg hook just warns on non-conforming messages.\n\
             \n\
             ## Capture proactively, not reactively\n\
             \n\
             The requirements DB is only valuable when it stays in sync with reality.\n\
             Treat `/aida-capture` as a habit, not a safety net:\n\
             \n\
             1. **Spec-first when introducing a new theme.** New command, new field on a\n\
             \x20  core model, new skill, new architectural pattern — pause and `aida add`\n\
             \x20  *before* the implementation commits. ~2 min cost; saves backfill later.\n\
             2. **Don't reuse one EPIC as a catchall.** When the work has drifted from\n\
             \x20  what the EPIC was originally about, that's a signal to create a new EPIC,\n\
             \x20  not stretch the existing one.\n\
             3. **Run `/aida-capture` at natural pauses.** End of focused work, before\n\
             \x20  compaction, when stepping away. Five-minute pass that catches missed reqs.\n\
             4. **Yellow flag at >5 untracked commits.** Five+ feat/fix commits without a\n\
             \x20  matching requirement → offer to capture before continuing.\n\
             5. **Trace comments must match reality.** A `// trace:EPIC-N` on code that\n\
             \x20  has nothing to do with EPIC-N is misinformation that compounds. If you're\n\
             \x20  unsure which spec a piece of work belongs to, that's the signal it needs\n\
             \x20  its own.\n\
             \n\
             ## Glance at the statusbar\n\
             \n\
             `.claude/settings.json` wires `aida statusline` into Claude Code's status\n\
             bar. It shows project · active role · queue depth · cache freshness. If the\n\
             role you expect isn't there, you forgot to `aida role enter <name>` before\n\
             starting the session.\n\
             \n\
             ## Git sync & review workflow\n\
             \n\
             - **`aida pull` refusing (divergent branches)?** The code leg is\n\
             \x20  `git pull --ff-only` (won't auto-rebase your tree); the store leg\n\
             \x20  is `--rebase`. Recovery recipe + the one-time `git config` to make\n\
             \x20  raw `git pull` Just Work: `docs/aida/discipline/git-sync-and-review.md`.\n\
             - **Reviewing a PR?** `aida review prompt --pr N` lifts each linked\n\
             \x20  spec's `## Acceptance` into a review prompt. Needs `gh`/`glab` for\n\
             \x20  `--pr` mode; write a `## Acceptance` section in every STORY/BUG so\n\
             \x20  there's something to lift. Detail: same discipline doc.\n",
        );

        if self.config.generate_skills {
            s.push_str(
                "\n## Claude Code skills (slash commands)\n\
                 \n\
                 This project ships a curated set of `/aida-*` skills under `.claude/skills/`,\n\
                 each with a matching slash command in `.claude/commands/`. Daily drivers:\n\
                 \n\
                 - `/aida-req` — add a new requirement with AI evaluation\n\
                 - `/aida-implement` — implement a requirement with trace comments + status updates\n\
                 - `/aida-plan` — decompose a requirement into an implementation plan\n\
                 - `/aida-evaluate` — score a requirement on clarity / testability / completeness\n\
                 - `/aida-capture` — review the current session and capture missed requirements\n\
                 - `/aida-commit` — commit with automatic requirement linking\n\
                 - `/aida-pickup` — peek at the next item routed to your active role and start work\n\
                 - `/aida-queue` — read-only queue inspection (counterpart to `/aida-pickup`)\n\
                 - `/aida-search` — unified search across requirements + code\n\
                 \n\
                 Run `ls .claude/skills/` for the full skill catalog.\n\
                 \n\
                 ### Per-project skill extensions (STORY-305)\n\
                 \n\
                 Two mechanisms let a project extend skills without forking them.\n\
                 AIDA never overwrites either, and `make sync-templates` never\n\
                 touches them:\n\
                 \n\
                 1. **Project-owned new skills** live under\n\
                 \x20  `.claude/skills/local/<my-skill>.md`. Claude Code discovers\n\
                 \x20  them the same way it discovers stock skills.\n\
                 2. **Stock-skill extensions** live alongside the stock skill as\n\
                 \x20  `.claude/skills/<name>.local.md`. When `/aida-<name>` is\n\
                 \x20  invoked, treat the stock skill as the base and **append**\n\
                 \x20  the `<name>.local.md` content as project-specific guidance\n\
                 \x20  with last-word authority (later instructions override\n\
                 \x20  earlier ones — normal markdown precedence).\n\
                 \n\
                 **For Claude Code**: whenever you load a stock skill from\n\
                 `.claude/skills/<name>.md`, also read `.claude/skills/<name>.local.md`\n\
                 if it exists in the same directory and treat its contents as\n\
                 appended to the stock skill. Both files together are the\n\
                 effective skill. See `docs/extending-skills.md` for examples.\n",
            );
        }

        s
    }

    /// AIDA conventions inlined into AGENTS.md, wrapped in the AUTOGEN
    /// delimiters that scaffold status uses for drift detection. Drops the
    /// Claude-specific slash-command section since non-Claude agents don't
    /// have those — but otherwise mirrors `.claude/AIDA.md` so the rules
    /// the agent sees match what the user maintains.
    /// trace:FR-1-035 | ai:claude
    pub(super) fn generate_aida_md_for_agents(&self, store: &RequirementsStore) -> String {
        // Reuse the canonical generator and chop off the Claude-skills
        // tail so AGENTS.md doesn't tell Codex about slash commands it
        // can't run.
        let full = self.generate_aida_md(store);
        let body = match full.find("## Claude Code skills") {
            Some(idx) => full[..idx].trim_end().to_string(),
            None => full.trim_end().to_string(),
        };
        format!("{}\n{}\n{}\n", AIDA_BLOCK_BEGIN, body, AIDA_BLOCK_END)
    }
}

/// Pull the AIDA-AUTOGEN block out of an AGENTS.md (or similar) file. Used
/// by `scaffold status` to compare just the autogenerated block against
/// the embedded template, leaving user-owned content outside the
/// delimiters untouched.
///
/// Markers MUST appear on their own line (line-anchored) so they don't
/// collide with intro text that mentions the marker names in backticks.
/// Returns `None` when either delimiter is missing.
/// trace:FR-1-035 | ai:claude
pub fn extract_aida_block(content: &str) -> Option<&str> {
    let begin = find_marker_at_line_start(content, AIDA_BLOCK_BEGIN)?;
    let after_begin = begin + AIDA_BLOCK_BEGIN.len();
    let end_rel = find_marker_at_line_start(&content[after_begin..], AIDA_BLOCK_END)?;
    Some(content[after_begin..after_begin + end_rel].trim_matches(['\n', '\r']))
}

/// Find a marker that's at the start of a line (or start of buffer), with
/// only optional whitespace before it on that line. Returns the byte
/// offset of the marker itself, not the line start.
fn find_marker_at_line_start(haystack: &str, marker: &str) -> Option<usize> {
    let mut search_from = 0usize;
    while let Some(rel) = haystack[search_from..].find(marker) {
        let abs = search_from + rel;
        let line_start = haystack[..abs].rfind('\n').map(|i| i + 1).unwrap_or(0);
        // Everything between line_start and abs must be whitespace.
        if haystack[line_start..abs].chars().all(char::is_whitespace) {
            return Some(abs);
        }
        search_from = abs + marker.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "<!-- AIDA Generated: v2.0.0 | checksum:deadbeef | DO NOT EDIT DIRECTLY -->\n\
                          <!-- To customize: copy this file and modify the copy -->\n\n";

    /// TASK-125: a `--no-skills` AIDA.md (no skills section) must NOT be
    /// reported as drift against the default-config (skills-on) regeneration.
    #[test]
    fn aida_md_matches_tolerates_missing_skills_section() {
        let body = "# AIDA Conventions\n\nshared body.\n";
        let with_skills =
            format!("{HEADER}{body}\n## Claude Code skills (slash commands)\n\nstuff\n");
        let without_skills = format!("{HEADER}{body}");
        // On-disk lacks the section, expected has it → still a match.
        assert!(aida_md_matches(&without_skills, &with_skills));
        // Both have the section → match.
        assert!(aida_md_matches(&with_skills, &with_skills));
        // Header checksum differing must not matter.
        let with_skills_other_header = with_skills.replace("deadbeef", "cafef00d");
        assert!(aida_md_matches(&with_skills_other_header, &with_skills));
    }

    /// Real drift in the shared body must still be reported.
    #[test]
    fn aida_md_matches_flags_body_drift() {
        let expected = format!("{HEADER}# AIDA Conventions\n\nshared body.\n");
        let actual = format!("{HEADER}# AIDA Conventions\n\nUSER EDITED body.\n");
        assert!(!aida_md_matches(&actual, &expected));
    }

    /// Drift inside the skills section (when both have it) must be reported.
    #[test]
    fn aida_md_matches_flags_skills_section_drift() {
        let body = "# AIDA Conventions\n\nshared body.\n";
        let expected =
            format!("{HEADER}{body}\n## Claude Code skills (slash commands)\n\noriginal\n");
        let actual =
            format!("{HEADER}{body}\n## Claude Code skills (slash commands)\n\nTAMPERED\n");
        assert!(!aida_md_matches(&actual, &expected));
    }

    /// End-to-end regression for TASK-125: the AIDA.md that
    /// `aida init --no-skills` writes (skills section dropped, AIDA-Generated
    /// header attached) must compare clean against the content the status
    /// drift check regenerates with the default (skills-on) config. Before
    /// the fix this reported `.claude/AIDA.md` STALE-on-arrival.
    #[test]
    fn no_skills_init_aida_md_is_not_stale_vs_default_regeneration() {
        use crate::models::RequirementsStore;
        use crate::scaffolding::{ScaffoldConfig, Scaffolder};
        use std::path::PathBuf;

        let store = RequirementsStore::default();

        // What `aida init --no-skills` writes to disk.
        let mut no_skills_cfg = ScaffoldConfig::default();
        no_skills_cfg.generate_skills = false;
        let no_skills = Scaffolder::new(PathBuf::from("/tmp/aida-task125"), no_skills_cfg);
        let on_disk_raw = no_skills.generate_aida_md(&store);
        let on_disk =
            super::super::wrap_with_aida_header(&PathBuf::from(".claude/AIDA.md"), &on_disk_raw);

        // What `aida status` regenerates to compare against (default config).
        let default = Scaffolder::new(
            PathBuf::from("/tmp/aida-task125"),
            ScaffoldConfig::default(),
        );
        let expected_raw = default.generate_aida_md(&store);
        let expected =
            super::super::wrap_with_aida_header(&PathBuf::from(".claude/AIDA.md"), &expected_raw);

        // Sanity: the two genuinely differ (skills section present in one).
        assert_ne!(
            on_disk, expected,
            "test precondition: outputs should differ"
        );
        assert!(
            aida_md_matches(&on_disk, &expected),
            "a fresh --no-skills AIDA.md must not be reported as drift"
        );
    }

    #[test]
    fn extract_aida_block_returns_inner_content() {
        let s = format!(
            "intro\n\n{}\n# AIDA stuff\nbody line\n{}\n\nuser tail",
            AIDA_BLOCK_BEGIN, AIDA_BLOCK_END
        );
        let block = extract_aida_block(&s).expect("delimiters present");
        assert!(block.contains("# AIDA stuff"));
        assert!(block.contains("body line"));
        assert!(!block.contains("user tail"));
        assert!(!block.contains(AIDA_BLOCK_BEGIN));
    }

    #[test]
    fn extract_aida_block_returns_none_when_missing() {
        assert_eq!(extract_aida_block("no delimiters here"), None);
        assert_eq!(extract_aida_block(AIDA_BLOCK_BEGIN), None); // missing END
    }

    /// Regression: intro paragraph mentions the marker names in backticks
    /// for documentation purposes. Without line-anchored matching the
    /// extractor would pull a tiny garbage region out of the intro
    /// instead of the actual block. trace:FR-1-035 | ai:claude
    #[test]
    fn extract_aida_block_ignores_inline_marker_mentions() {
        let s = format!(
            "Intro mentions `{begin}` and `{end}` in backticks for documentation.\n\
             \n\
             {begin}\n\
             real body\n\
             {end}\n\
             \n\
             User tail.\n",
            begin = AIDA_BLOCK_BEGIN,
            end = AIDA_BLOCK_END
        );
        let block = extract_aida_block(&s).expect("delimiters present");
        assert_eq!(block.trim(), "real body");
    }

    #[test]
    fn extract_aida_block_handles_leading_whitespace_on_marker_line() {
        // Indented marker should still match (some markdown editors auto-indent).
        let s = format!(
            "intro\n  {}\nbody\n  {}\ntail",
            AIDA_BLOCK_BEGIN, AIDA_BLOCK_END
        );
        let block = extract_aida_block(&s).expect("delimiters present");
        assert!(block.contains("body"));
    }
}
