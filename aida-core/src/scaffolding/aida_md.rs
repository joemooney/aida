use super::*;

/// Delimiter markers for the inlined AIDA conventions block in AGENTS.md
/// (and any other agent-context file that can't follow `@` imports). The
/// scaffold status path uses these to extract & compare just the block,
/// leaving user content outside the markers freely editable.
/// trace:FR-1-035 | ai:claude
pub(super) const AIDA_BLOCK_BEGIN: &str = "<!-- AIDA-AUTOGEN-BEGIN -->";
pub(super) const AIDA_BLOCK_END: &str = "<!-- AIDA-AUTOGEN-END -->";

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
        let req_count = store.requirements.len();

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
        s.push_str(&format!(
            "\n\nCurrently tracking **{}** requirement(s).\n\n",
            req_count
        ));
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
             ## When `aida pull` refuses (divergent branches)\n\
             \n\
             `aida pull` is two operations in one: a `git pull` of your code branch\n\
             and a `git pull --rebase` of the orphan `aida-store` branch. The two\n\
             legs are deliberately asymmetric:\n\
             \n\
             - **Code leg**: `git pull --ff-only` — refuses if the branch has\n\
             \x20  diverged from origin. Won't surprise your working tree with an\n\
             \x20  auto-rebase.\n\
             - **Store leg**: `git pull --rebase` — store conflicts are rare and\n\
             \x20  the worktree is AIDA-managed.\n\
             \n\
             When the code leg refuses (or raw `git pull` complains about divergent\n\
             branches), the recovery recipe:\n\
             \n\
             ```bash\n\
             git fetch origin \"$(git rev-parse --abbrev-ref HEAD)\"\n\
             git log --oneline @{u}..HEAD     # what you have that origin doesn't\n\
             git log --oneline HEAD..@{u}     # what origin has that you don't\n\
             git log --name-only @{u}..HEAD --pretty= | sort -u   # files you touched\n\
             git log --name-only HEAD..@{u} --pretty= | sort -u   # files they touched\n\
             # No overlap → safe: git pull --rebase\n\
             # Overlap   → inspect; rebase + resolve, or git rebase --abort\n\
             ```\n\
             \n\
             To make raw `git pull` Just Work without per-incident decisions (one-time,\n\
             machine-global):\n\
             \n\
             ```bash\n\
             git config --global pull.rebase true\n\
             git config --global rebase.autoStash true\n\
             git config --global advice.diverging false\n\
             ```\n\
             \n\
             Trade-off: silent auto-rebase for fewer manual decisions. `autoStash`\n\
             preserves uncommitted changes across the rebase. If you'd rather see the\n\
             prompt each time, leave these unset and the recipe above is your fallback.\n\
             \n\
             ## Review workflow\n\
             \n\
             `aida review prompt --pr N` (or `--specs FR-1,STORY-2,…`) generates a\n\
             markdown review prompt that lifts each linked requirement's `## Acceptance`\n\
             section verbatim — paste it into a fresh Claude Code review session, or\n\
             write it to a file with `--write`.\n\
             \n\
             - **Install `gh` or `glab` for `--pr` mode.** AIDA shells out to\n\
             \x20  [`gh pr view`](https://cli.github.com) / [`glab mr view`](https://gitlab.com/gitlab-org/cli)\n\
             \x20  to resolve the PR's base + head refs. Without them, AIDA falls back to\n\
             \x20  `base=main` and a local review branch named `pr-N` / `mr-N` — that path\n\
             \x20  works when the PR was started via `aida session start --owns PR-N`\n\
             \x20  (STORY-61), surprising otherwise.\n\
             - **Acceptance sections are the contract.** Write a `## Acceptance`,\n\
             \x20  `## Verify`, `## Tests`, `## Test cases`, or `## Verification` section\n\
             \x20  in every STORY / BUG description so the review prompt has something\n\
             \x20  concrete to lift. `aida doctor convention-check` lints for the gap.\n",
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
                 Run `ls .claude/skills/` for the full skill catalog.\n",
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
