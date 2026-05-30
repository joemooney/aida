# TASK-574 — Skill helper subfolders

- **Date:** 2026-05-28
- **Specs:** TASK-574
- **Status:** In progress
- **Complexity:** Medium

## Approach

Claude Code skills may be *folders* — `<name>/SKILL.md` plus supporting files
(`templates/`, `examples/`, helper scripts) — not only flat `<name>.md` files.
AIDA's embedding + scaffolding pipeline assumes flat `.md` files end-to-end.
This change teaches the whole pipeline the folder-form while keeping every
existing flat skill working unchanged, then migrates one skill (`aida-pr`) to
folder-form as the proof and documents the pattern.

```
templates/skills/aida-pr/SKILL.md ──build.rs(recurse)──▶ EMBEDDED_TEMPLATES["skills/aida-pr/SKILL.md"]
templates/skills/aida-pr/examples/… ─────────────────▶ EMBEDDED_TEMPLATES["skills/aida-pr/examples/…"]
                                                              │
                                       classify_skill_key()   │  (name="aida-pr", rel_path, is_prompt)
                                                              ▼
                          scaffolding loop ──▶ .claude/skills/aida-pr/{SKILL.md,examples/…}
                          codex loop      ──▶ .codex/skills/aida-pr/SKILL.md  (prompt only)
                          parity test     ──▶ skill name "aida-pr"
```

## Decisions

- **One classifier, many call-sites.** A single `classify_skill_key()` in
  `templates.rs` is the canonical "what skill does this embedded key belong to,
  and is it the prompt body?" so the scaffolding loop, codex generation, and the
  parity test never re-derive the convention divergently.
- **`build.rs` recursion is generic**, not skills-only — every `embed_directory`
  call now recurses, preserving relative paths in keys. Safe because no existing
  template dir has subdirectories (verified).
- **Migrate `aida-pr`, not `aida-plan`.** `aida-pr` is scaffolded by the generic
  loop (not a hardcoded `generate_*` method), so migrating it exercises the new
  folder path with the least churn. The spec names both as candidates.
- **Codex gets the prompt body only** for now; folder helper-file parity on the
  `.codex/` surface is a follow-up, not this task.
- **Dogfood sync uses a directory symlink** for folder-form skills (the flat
  `*.md` per-file convention is unchanged for flat skills).

## Files (build order)

1. `aida-core/build.rs` — make `embed_directory` recurse into subdirs.
2. `aida-core/src/templates.rs` — add `SkillKey` + `classify_skill_key`; rewrite
   `test_skill_command_parity` to use it; add classifier + folder-form tests.
3. `aida-core/src/scaffolding/mod.rs` — generic skill loop uses
   `classify_skill_key` (name-based `handled` set, write `rel_path`);
   `generate_codex_skills` emits only `is_prompt` entries keyed by `name`.
4. `aida-core/templates/skills/aida-pr.md` → `…/aida-pr/SKILL.md` (git mv) +
   new `…/aida-pr/examples/pr-description-template.md`; SKILL.md references it.
5. `.claude/skills/aida-pr.md` symlink → `.claude/skills/aida-pr` dir symlink.
6. `Makefile` — `sync-templates` / `check-templates` handle folder-form skills.
7. `aida-core/templates/docs/aida/discipline/skill-prompt-kinds.md` — document
   the helper-folder pattern.

## Critical Files

- `aida-core/build.rs` — `embed_directory` (compile-time embedding).
- `aida-core/src/templates.rs` — `classify_skill_key`, parity invariant test.
- `aida-core/src/scaffolding/mod.rs` — generic skill loop (~L1551),
  `generate_codex_skills` (~L2257).

## Reusable helpers

- `EMBEDDED_TEMPLATES` (`aida-core/src/templates.rs`) — `HashMap<&str,&str>` of
  `category/relpath → contents`.
- `ClaudeScaffolder::create_artifact` / `apply` — `apply` already
  `create_dir_all`s each artifact's parent, so subfolder files need no extra
  directory bookkeeping.

## Risks + gotchas

- The generic loop's `handled` set was `.md`-suffixed; switching to name-based
  must still exclude the hardcoded skills and the generated `local/README.md`.
- `classify_skill_key` must return `None` for `skills/local/README.md`.
- Parity test extracts skill names from `is_prompt` entries only, else
  `examples/*.md` helper files would masquerade as skills.

## Tests (named)

- `templates::tests::test_classify_skill_key` — flat, folder SKILL.md, folder
  support file, local/README rejection.
- `templates::tests::test_skill_command_parity` — still green with `aida-pr`
  folder-form.
- `templates::tests::test_embedded_templates_exist` — add folder-form key
  assertion (`skills/aida-pr/SKILL.md`, `skills/aida-pr/examples/…`).
- `scaffolding::tests::test_folder_form_skill_scaffolded` — apply writes
  `.claude/skills/aida-pr/SKILL.md` + `examples/pr-description-template.md`.

## Verification

```bash
cargo build -p aida-core -p aida-cli
cargo test -p aida-core templates::tests
cargo test -p aida-core scaffolding::tests
make check-templates
```

## Followups

- Codex `.codex/skills/<name>/` helper-file parity (templates/examples copied to
  the codex surface too).
- Migrate further flat skills to folder-form where a template/example helps
  (e.g. `aida-plan` shipping `templates/plan-skeleton.md`).

## Related

- TASK-482 (generic skill loop keeps scaffolding in lockstep with templates).
- TASK-419 (codex skill folder form).
- STORY-305 (`.claude/skills/local/` extension namespace).
