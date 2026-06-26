# STORY-679 — deterministic AIDA action palette while the chat is suspended

- **Date:** 2026-06-26
- **Specs:** STORY-679 (EPIC-51 slice 2; parent EPIC-51, sibling EPIC-26)
- **Status:** implemented
- **Complexity:** medium

## Approach

EPIC-51's vision: from a live hosted chat, `Ctrl-D` suspends it → a deterministic
TUI AIDA action surface (no LLM) → act instantly → resume. The three pieces are
(1) the deterministic palette, (2) PTY suspend/resume of the chat, (3) run-and-inject
result back into the chat JSONL. **Slice 1 (suspend/resume) already shipped** as
STORY-678: `Ctrl-a p` SIGSTOPs the focused child's process group, paints a minimal
"paused" overlay, and any key SIGCONTs + repaints. **This slice (STORY-679) replaces
that static paused overlay with the deterministic action palette.**

```
Ctrl-a p (STORY-678 suspend)         +---------------------------------+
   chat SIGSTOPped  ---------------> |  [paused] AIDA action palette    |
                                     |  : queue                         |  type to filter
                                     |  > queue   show the work queue   |
                                     |    punts   open punts            |  fuzzy-ranked
                                     |    findings list findings        |  curated actions
                                     |    status  project snapshot      |
                                     |  ------------------------------- |
                                     |  ok 12:01:33  queue (inline JSON) |  result pane
                                     +---------------------------------+
   Esc  ---------------------------> chat SIGCONT + repaint (resume)
```

The palette is **deterministic**: every action maps to a fixed `aida ... --json`
(or `gh`) argv, run as a captured subprocess via the existing `actions::run_argv`
machinery (STORY-133) — zero LLM round-trip. The action set is curated (the spec's
`:queue :punts :findings :spec <ID> :run <cmd>`), fuzzy-filtered by the typed query
using the **existing `cmd_palette::fuzzy_score`** matcher (STORY-682).

## Decisions

- **Replace, not add a 6th mode.** Slice 1 already owns `Mode::Paused`. STORY-679
  enriches what `Mode::Paused` *shows*: the palette, not a static line. The
  suspend/resume lifecycle, key routing entry (`Ctrl-a p`), and SIGSTOP/SIGCONT are
  unchanged — only `open_paused` / `draw_paused` and the paused key routing change.
- **Curated action set, not full clap enumeration.** The spec names a small,
  high-signal set (`queue / punts / findings / spec / run`). `aida-tui` does not
  depend on `aida-cli`, so `cmd_palette::enumerate(Cli::command())` can't run here
  anyway. A curated `PaletteAction` enum keeps the surface deterministic and the
  argv explicit. We still reuse `cmd_palette::fuzzy_score` for ranking, so the
  filter behaves like the STORY-682 palette.
- **Parametric actions** (`spec <ID>`, `run <cmd>`): when the typed query starts
  with `spec ` / `run `, the palette dispatches the parametric form directly
  (`aida show <ID> --json`, or the raw `<cmd>` argv) — guarded by the same
  `intent::is_safe_payload` allow-list so no shell metacharacters reach a child.
- **No LLM, no injection (this slice).** Results render *inline* in the palette's
  result pane. Injecting the result back into the chat JSONL is EPIC-51 piece (3),
  a separate slice — flagged as the dependency, not built here.

## Files (build order)

1. `aida-tui/src/palette.rs` (new) — pure `PaletteState` + `PaletteAction` +
   deterministic `dispatch` (action/query -> argv) + `rank` over the curated set.
   Fully unit-testable, no terminal.
2. `aida-tui/src/lib.rs` — register `mod palette;` and re-export the testable core.
3. `aida-tui/src/app.rs` — carry `PaletteState`; route paused-mode keys through it
   (type/filter/select/run/esc-resume); `open_paused` seeds it, `draw_paused`
   renders it; the run path runs the captured subprocess and lands the result in
   the palette pane.

## Critical files

- `aida-tui/src/app.rs` — `Mode::Paused` routing (`route_key`), `open_paused`,
  `draw_paused`, the new `route_palette_key` + `run_palette_action`.
- `aida-tui/src/palette.rs` — the deterministic core.
- `aida-tui/src/actions.rs` — `run_argv` reused unchanged for capture.
- `aida-tui/src/cmd_palette.rs` — `fuzzy_score` reused for ranking.

## Reusable helpers

- `actions::run_argv(label, &argv) -> ActivityEntry` — deterministic capture.
- `cmd_palette::fuzzy_score(query, candidate) -> Option<i32>` — fuzzy ranking.
- `intent::is_safe_payload(&str) -> bool` — metacharacter gate for `run`/`spec`.
- `app::aida_exe()` — the running binary, so a dev build drives a dev build.

## Risks + gotchas

- **`run <cmd>` injection.** Mitigated: the raw command is tokenised by whitespace
  and each token must pass `is_safe_payload`; argv is spawned directly (no shell),
  exactly like `dispatch::run_child` (STORY-681).
- **Long output.** The result pane shows the tail, clipped per line (mirrors the
  overlay's activity log).
- **Paused child must stay frozen.** Palette subprocesses are independent `aida`
  invocations; they never touch the suspended PTY. Esc/resume is the only path that
  SIGCONTs the child.

## Tests (named, in `palette.rs` + `app.rs`)

- `fuzzy_filters_actions_by_query`, `empty_query_shows_all_actions_in_order`
- `dispatch_queue_action_is_aida_queue_list_json`
- `dispatch_findings_is_json`, `dispatch_punts_is_json`, `dispatch_status_is_json`
- `dispatch_spec_param_builds_show_json`, `dispatch_run_param_passes_through`
- `dispatch_run_rejects_metacharacters`, `dispatch_spec_rejects_empty_id`
- `selection_wraps`, `dispatch_is_deterministic_no_llm` (argv only, no `claude`)
- app: `prefix_p_opens_palette`, `palette_typing_filters`, `palette_enter_runs_action`,
  `palette_esc_resumes`

## Verification

```
cargo build -p aida-tui
cargo fmt --all -- --check
cargo clippy -p aida-tui -p aida-cli -- -D clippy::correctness
bash scripts/glyph-lint.sh --block
env -u AIDA_SESSION_ROLE cargo test -p aida-tui
```

## Followups

- EPIC-51 piece (3): inject a palette action's result back into the suspended chat's
  JSONL on resume (separate slice).
- Wire `Ctrl-D` capture from the hosted chat as an alternate palette-open trigger
  (the epic's literal framing). Today the open key is `Ctrl-a p` (STORY-678).

## Related

- STORY-678 (slice 1, suspend/resume) — the substrate this builds on.
- STORY-682 / `cmd_palette.rs` — the fuzzy matcher reused here.
- STORY-681 / `dispatch.rs` — the direct-spawn-no-shell pattern mirrored here.
- STORY-133 / `actions.rs`, `overlay.rs` — the captured-subprocess + result-pane model.
