# STORY-255 — `aida init` starter discipline pack

- **Date:** 2026-05-17
- **Specs:** STORY-255
- **Status:** Complete
- **Complexity:** Medium-high (new template categories, opt-in flag, refresh-overlay logic)

## Approach

Three propagation channels ship AIDA-using discipline as scaffolding so new
projects inherit it instead of re-discovering the same friction.

```
aida init
  ├─ Channel A  docs/aida-discipline/        (always)   ← embedded templates
  ├─ Channel C  CLAUDE.md discipline section (always)   ← generate_claude_md
  └─ aida init --with-memories
       └─ Channel B  ~/.claude/projects/<slug>/memory/  (opt-in) ← marker-driven
```

The starter memory pack is **marker-driven**: it ships every file carrying
`propagation: scaffolding-pack` in frontmatter (16 today). No hardcoded count.

## Decisions

- **Marker over count.** STORY-255's body names 10 files; the producer marker
  is now on 16. The `propagate-generic-discipline` memory designates the
  marker as canonical, so the pack ships the marker set; the impl never
  hardcodes a count. (User-confirmed 2026-05-17.)
- **Comment additions deferred.** STORY-255's comment adds 3 items
  (skills/local/, `.local.md` extensions, propagation-memory split). None were
  promoted to `## Acceptance`, so they are out of this PR's scope and filed
  separately by the advisor.
- **Checksum = FNV-1a, no new dep.** Refresh needs an "edited since scaffold?"
  check. A hand-rolled FNV-1a 64-bit hash over the memory body is deterministic
  forever and adds zero dependencies.
- **Channel A in `complete_init_scaffolding`.** That function is the single
  point shared by all 4 init paths (centralized, distributed-worktree,
  post-clone, sibling), so the discipline pack scaffolds from one site.
- **MEMORY.md block is generated, not templated.** The scaffold-pack index
  block is delimited by `<!-- aida:scaffold-pack:start/end -->` and generated
  from the actual memory set, so it can't drift from the marker set.

## Files (build order)

1. `aida-core/templates/docs/aida-discipline/{README,advisor-role,lifecycle-vocabulary,workflow-patterns,session-discipline}.md` — new
2. `aida-core/templates/memories/*.md` — 16 memory files + `MEMORY.md` skeleton — new
3. `aida-core/build.rs` — `embed_directory` for the two new categories
4. `aida-core/src/scaffolding/claude_md.rs` — `generate_claude_md` discipline section
5. `aida-cli/src/process_probe.rs` — `encode_cwd_for_projects` made `pub`
6. `aida-cli/src/cli.rs` — `Init { with_memories, refresh }`
7. `aida-cli/src/main.rs` — dispatch wiring, `ensure_discipline_pack_scaffold`,
   `scaffold_memory_pack`, FNV checksum + frontmatter helpers, tests
8. `CLAUDE.md` — describe the discipline-pack mechanism

## Critical Files

- `aida-cli/src/main.rs` — `complete_init_scaffolding` (fn), `Command::Init` dispatch block, `ensure_plan_template_scaffold` (pattern to mirror)
- `aida-core/build.rs` — `embed_directory`
- `aida-core/src/scaffolding/claude_md.rs` — `generate_claude_md`

## Reusable helpers

- `ensure_plan_template_scaffold` (main.rs) — the idempotent-scaffold pattern
- `encode_cwd_for_projects` (process_probe.rs) — Claude Code project-dir slug
- `EMBEDDED_TEMPLATES` (aida-core templates) — compile-time template map

## Risks + gotchas

- Embedded templates are `.trim_end()`'d — checksum must normalize trailing
  whitespace (`trim_end` the body on both sides) or refresh always mismatches.
- `originSessionId` must be a TOP-LEVEL frontmatter key (the field scanner
  ignores indented lines so it doesn't catch `metadata:`-nested keys).
- AIDA's own `CLAUDE.md` is a Seed-class user-owned file — never regenerated;
  the template change only affects newly-init'd projects.

## Tests (named)

- `discipline_pack_scaffolds_four_docs_plus_readme`
- `memory_pack_writes_marked_files_with_marker_and_checksum`
- `memory_pack_refresh_overlays_pristine_skips_edited`
- `generated_claude_md_has_discipline_section`

## Verification

```bash
cargo build --workspace
cargo test --workspace
```

## Followups

- (Filed by advisor — STORY-255 comment additions: skills/local/, `.local.md`
  extensions, propagation-memory split.)

## Related

- STORY-255, batch:alpha-docs
