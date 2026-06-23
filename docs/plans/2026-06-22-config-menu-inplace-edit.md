# Plan: `aida config menu` in-place toggle/edit

**Date:** 2026-06-22
**Specs:** STORY-669
**Status:** Ready for implementation
**Complexity:** Medium (TUI interaction + a generalized config writer + registry metadata)

## 1. Approach

`aida config menu` (STORY-661) is a read-only ratatui browser over `policy_registry()`.
Make boolean knobs **toggle in place** and write through to the correct config file,
preserving the rest of the file. Env-shadowed knobs stay read-only with a clear reason.

The three missing pieces:
1. **Editability metadata** — the registry carries no type info. Add a per-knob
   `EditSpec { kind, .. }` lookup mirroring the existing `config_knob_doc(section, key)`
   pattern. MVP `kind` set: `Bool`, `ReadOnly` (everything not-yet-editable).
2. **A generic section-preserving writer** — generalize `glyph_config`'s `toml_edit`
   primitives (`load_doc`/`save_doc`/`table_mut`) into `set_kv(path, section, key, value)`.
3. **The TUI edit interaction** — Enter/Space on a `Bool` row flips it, writes, and
   re-resolves the row's value+scope in place. `ReadOnly`/env-shadowed rows show why.

```
 user presses Enter on a Bool row
        │
        ▼
 ConfigMenuItem.edit == Bool ?  ──no──▶ flash "read-only: <reason>" in footer
        │ yes
        ▼
 resolve write target file from scope/source
   (ProjectConfig→.aida/config.toml, GlobalConfig→~/.aida/config.toml,
    Default→project unless --user, Env→blocked)
        │
        ▼
 config_edit::set_kv(path, section, key, !current)   (toml_edit, preserves file)
        │
        ▼
 re-resolve THIS knob (single-knob registry call) → update item.value + item.scope
        │
        ▼
 redraw — value + scope reflect new state, no menu restart
```

**Write-target rule (MVP):** write to the file the knob currently lives in
(`PolicySource` → path). A `Default` (unset) knob writes to **project** config; a
`--user` launch flag (and/or an in-TUI scope toggle, deferred) targets `~/.aida/config.toml`.
`Env`-sourced knobs are never editable (unset the var to edit).

## 2. Decisions

- **Mirror `config_knob_doc`, don't extend `PolicyRow`.** A parallel
  `config_knob_editability(section, key) -> EditSpec` keeps the registry-building code
  untouched and co-locates editability with the existing doc table. (`PolicyRow` is
  `&'static str` keyed and rebuilt per call; threading edit-kind through every `.push`
  site is more churn for no gain.)
- **MVP = booleans only.** Scalars (e.g. `archive.auto_after_days`), enums
  (`ui.theme`, `agents.bypass` native/bypass), and the separate-file `[agents]`/`[contained]`
  knobs (`~/.aida/agents.toml`, side effects) stay **read-only this slice**. They get a
  clear "edit via `aida config <subcommand>` / config.toml" footer line, not a silent dead key.
- **Reuse, don't fork, the writer.** `glyph_config` already has the `toml_edit`
  load/save/atomic-write machinery; factor the generic `set_kv` out so glyph + menu share it.
- **Re-resolve, don't restart.** After a write, recompute just that knob so the screen
  updates live — the proof the write landed and at the right scope.
- **No new always-on behavior.** Pure additive interaction on an opt-in TTY command.

## 3. Files (build order)

1. `aida-cli/src/config_edit.rs` *(new)* — `pub(crate) fn set_kv(path, section, key, value: toml_edit::Value)`
   + `EditKind` enum (`Bool`, `ReadOnly`) + `EditSpec`. Factor `load_doc`/`save_doc`/`table_mut`
   out of `glyph_config` (or call into a shared helper) so both use one writer.
2. `aida-cli/src/glyph_config.rs` — repoint its `set_override`/`set_theme` at the shared
   `set_kv` (no behavior change; keeps one writer + the 7 existing tests green).
3. `aida-cli/src/main.rs` — add `fn config_knob_editability(section, key) -> EditSpec`
   (Bool for `telemetry.enabled`, `hints.workflow_hints`, `mailbox.*`, `field_study.enabled`,
   …; ReadOnly otherwise). Add a single-knob re-resolver `fn resolve_config_knob(project_root,
   section, key) -> (value, scope)` (reuse the per-section resolution; simplest: call
   `policy_registry` and pick the row). Populate the new `ConfigMenuItem.edit` in
   `build_config_menu_items`. Pass `--user` intent down.
4. `aida-tui/src/config_menu.rs` — add `pub edit: EditKind` (mirror a tiny enum in the
   tui crate, or pass a `bool editable` + reason string to avoid a cross-crate type) to
   `ConfigMenuItem`; add Enter/Space handling that calls a write+reresolve **callback**
   passed into `run()` (keep the tui crate free of `aida-cli` deps — the callback closes
   over the cli-side writer). Footer gains an edit hint.

## 4. Critical files

- `aida-cli/src/glyph_config.rs` — the writer being generalized; its 7 preservation tests
  are the regression guard that the refactor didn't change write behavior.
- `aida-tui/src/config_menu.rs` — `run()` signature changes from `run(items)` to
  `run(items, on_edit)` (a callback), the one cross-crate seam. Keep the tui crate
  dependency-free of `aida-cli` — the edit logic lives cli-side behind the callback.
- `aida-cli/src/main.rs` `policy_registry` / `config_knob_doc` — the registry the
  editability table must stay in lockstep with (the `KNOWN_CONFIG_SECTIONS` anti-drift
  test pattern should gain a sibling: every `Bool` editability entry resolves to a real knob).

## 5. Reusable helpers (don't reimplement)

- `glyph_config::{load_doc, save_doc, table_mut, config_path_for}` — the `toml_edit`
  section-preserving write + project/user path routing. `Scope { Project, User }` already exists.
- `aida_core::write_atomic` — atomic config write (already used by both writer patterns).
- `config_knob_doc(section, key)` — the (explanation, default) table to mirror for editability.
- `PolicySource` → write-target file mapping (ProjectConfig/GlobalConfig/Env already distinguish).
- `strip_ansi_color` — already applied when building `ConfigMenuItem.value`.

## 6. Risks + gotchas

- **`PolicyRow` has no type info** → a `Bool` editability entry whose value renders as a
  decorated string (e.g. `"bypass (agents skip…)"`) must map display→`true/false` correctly.
  MVP: only mark knobs `Bool` whose stored TOML value is a real boolean (`telemetry.enabled`,
  `hints.workflow_hints`, `field_study.enabled`, `mailbox.autosync`, …) — NOT decorated
  composites like `agents.bypass` (leave ReadOnly this slice).
- **Cross-crate type leak.** Don't make `aida-tui` depend on `aida-cli`. Pass the edit
  capability as a callback `Fn(&ConfigMenuItem) -> Result<(String /*new value*/, String /*new scope*/), String /*reason*/>`.
- **Env-shadowed write is a trap** — writing `[telemetry] enabled=false` while `AIDA_TELEMETRY=0`
  is set leaves the env still winning; the row would show "value still from env." Block edit
  when `PolicySource::Env`, footer: "overridden by `$VAR` — unset it to edit."
- **`[agents]`/`[contained]` live in `agents.toml`, not config.toml**, and have side
  effects → ReadOnly this slice (a wrong write target would silently no-op).
- **Atomic write + re-resolve ordering** — re-resolve must read the file back (not trust the
  in-memory toggle) so a failed/partial write surfaces as an unchanged row.

## 7. Tests (named)

- `config_edit::set_kv_preserves_unrelated_keys_and_comments` — the core preservation guarantee.
- `config_edit::set_kv_creates_section_when_absent`.
- `config_edit::set_kv_round_trips_bool`.
- `glyph_config::*` (existing 7) — must stay green after repointing to the shared writer.
- `main::config_knob_editability_bool_entries_resolve_to_real_knobs` — anti-drift: every
  `Bool` entry names a knob `policy_registry` actually emits (sibling to
  `policy_registry_covers_known_sections`).
- `main::env_shadowed_knob_is_not_editable` — `PolicySource::Env` ⇒ `EditKind::ReadOnly`.
- `aida-tui config_menu::enter_on_readonly_row_is_noop` (pure: the row-classification half).

## 8. Verification (executable)

```bash
env -u AIDA_SESSION_ROLE cargo test -p aida-cli config_edit glyph_config config_knob_editability
env -u AIDA_SESSION_ROLE cargo test -p aida-tui config_menu
cargo clippy -p aida-cli -p aida-tui && cargo fmt --all -- --check
# Manual (TTY): aida config menu → Enter on [field_study] enabled → flips true,
#   scope flips to .aida/config.toml live; reopen `aida config show` confirms persisted;
#   rest of config.toml (comments, other keys) intact (git diff .aida/config.toml).
# Manual: AIDA_TELEMETRY=0 aida config menu → Enter on [telemetry] enabled → footer
#   shows "overridden by AIDA_TELEMETRY — unset to edit", no write.
```

## 9. Followups

- Scalar inline-edit (text prompt) for `archive.auto_after_days`, `contained.allowed_hosts`, …
- Enum pickers (`ui.theme`, `agents.bypass`) — cycle through valid variants.
- In-TUI project/user scope toggle (write to `~/.aida/config.toml` without a launch flag).
- `[agents]`/`[contained]` editing (writes `agents.toml`; needs the side-effect paths).
- An `aida config set <section.key> <value>` non-TUI generic setter (the headless twin of
  this; would also serve scripts) — file only if demand shows.

## 10. Related

- STORY-661 (the read-only menu this builds on), TASK-859 (init → menu prompt).
- `aida config glyph` (the writer being generalized), `aida config hints` (the line-rewrite
  twin — leave as-is; new writer is toml_edit-based).
- Discipline: keep SPEC-IDs out of `--help`/`///` doc comments (TASK-268) — the new clap
  surface is internal, but the lesson stands for any footer text.
