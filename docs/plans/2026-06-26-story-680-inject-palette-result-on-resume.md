# STORY-680 — Inject a palette result into the chat via PTY stdin on resume

- **Date:** 2026-06-26
- **Specs:** STORY-680 (EPIC-51 slice 3); parent EPIC-51; reuses STORY-679 (palette), STORY-678 (suspend/resume), STORY-136 (inject-into-focused-PTY)
- **Status:** Implemented (PR open, not merged)
- **Complexity:** Small–Medium (architecture-class seam, but the suspend→palette→resume cycle was already complete)

## Approach

EPIC-51's flow: `Ctrl-D`/`prefix p` **suspends** a hosted chat (STORY-678 SIGSTOP) →
the deterministic AIDA action palette (STORY-679) runs `aida … --json` subprocesses
inline → on **resume**, optionally inject the chosen palette result back **into** the
chat via the PTY's stdin so the conversation continues with that context. This slice =
that PTY-stdin injection on resume.

```
prefix p ──> open_paused (SIGSTOP child) ──> Mode::Paused
                                              │
            type/filter, Enter ─> run_palette_action ─> aida … --json ─> ActivityEntry
                                              │
            ┌─────────────────────────────────┴──────────────────────────────┐
          Esc                                                              Ctrl-Y  (STORY-680)
            │                                                                  │
        Resume                                                      ResumeWithInject
   SIGCONT + repaint                                  SIGCONT ─> format_injection(last entry)
                                                       ─> write_to_focused_pty (bytes + \r)
                                                       ─> full_repaint
```

The injection **shape** (from STORY-680 acceptance): a **formatted context block** —
the deterministic command's captured output, fenced and labelled as a *quoted AIDA
result* (`[AIDA palette result — \`aida queue list --json\`]` + fenced body). NOT a
directive, NOT JSONL surgery (the spec's explicit D2 exclusion). It "becomes a user
message the conversation can see."

## Decisions

- **Trigger = `Ctrl-Y`** ("yank result into chat"). The palette consumes every
  printable `Char(c)` for its filter, so a printable trigger was unavailable; `Esc`
  already = plain resume. `Ctrl-Y` is intercepted *before* the generic `Char` arm so it
  never lands as filter text. Reliably reported by crossterm (modifiers are already
  used elsewhere in `app.rs`). Degrades to a plain `Resume` when there is no result yet.
- **Reuse, not reinvent.** The STORY-136 inject mechanism (`write_input(bytes)` + `\r`)
  is the substrate the spec names. Extracted the raw write into
  `App::write_to_focused_pty` so STORY-136's drain injection and STORY-680's result
  injection share one "bytes + submit byte" path.
- **SIGCONT before write.** A `SIGSTOP`ped child can't drain its PTY input; resume the
  child *first*, then type. Ordering is load-bearing.
- **Quoted result, not command.** The block frames itself as a result, carrying no
  imperative verb / leading `/`, so the model reads it as context rather than an order.
- **Head-clip + sanitize.** Long `list --json` dumps are clipped to 40 lines with an
  elision note; embedded CR / control bytes in a captured line are replaced with spaces
  so a stray CR can't submit a partial message or smuggle an escape sequence.
- **`format_injection` is pure.** Lives in `palette.rs`, takes `&ActivityEntry`, returns
  `String` — fully unit-testable without a PTY.

## Files (build order)

1. `aida-tui/src/palette.rs` — `format_injection` + `sanitize_inject_line` + `INJECT_MAX_LINES`; `render_hints` gains a `has_result` arg for the `ctrl-y` hint; 6 new unit tests + 1 doc-test.
2. `aida-tui/src/app.rs` — `Routing::ResumeWithInject`; `route_palette_key` intercepts `Ctrl-Y`; `handle_routing` dispatches to new `resume_with_inject`; `write_to_focused_pty` extracted from `inject_to_focused`; 2 new routing tests.
3. `docs/tui/README.md` — `prefix p` row + an "Action palette" section documenting the keys incl. `Ctrl-Y`.

## Critical files

- `aida-tui/src/app.rs` — `fn resume_with_inject`, `fn write_to_focused_pty`, `fn route_palette_key`.
- `aida-tui/src/palette.rs` — `fn format_injection`.

## Reusable helpers (don't reimplement)

- `PtyHost::write_input` / `resume` (`pty.rs`) — the PTY stdin write + SIGCONT.
- `ActivityEntry` (`actions.rs`) — the captured-result struct the palette already produces.
- `inject_to_focused` (`app.rs`, STORY-136) — now layered on `write_to_focused_pty`.

## Risks & gotchas

- **Suspend-cycle dependency:** STORY-680 depends on the STORY-678 suspend / STORY-679
  palette cycle. **Both are present on `main`** (44551f526 merged STORY-679, which builds
  on STORY-678's `prefix p` SIGSTOP + `Esc` resume). The full cycle is wired, so this
  slice implements the real injection rather than a stub. **TASK-909 (wire `Ctrl-D` from
  the hosted chat as an alternate palette-open trigger) is NOT done** — today the palette
  opens via `prefix p`, not a raw `Ctrl-D` captured from the chat. That is an independent
  follow-up; STORY-680's injection works regardless of which trigger opened the palette.
- **No real PTY in unit tests.** `App::new` hosts no tabs, so the routing tests assert the
  `Routing` verdict + mode + that `Ctrl-Y` never reaches the filter; the byte-level write
  is covered by `format_injection`'s pure tests. A live end-to-end (real chat + real PTY)
  was not exercised here — flagged for the lead.

## Tests (named)

- `palette::format_injection_quotes_command_and_fences_output`
- `palette::format_injection_is_a_result_not_a_command`
- `palette::format_injection_head_clips_long_output`
- `palette::format_injection_strips_control_bytes`
- `palette::format_injection_handles_empty_output`
- `palette::format_injection_falls_back_to_label_without_command`
- `app::palette_ctrl_y_without_a_result_just_resumes`
- `app::palette_ctrl_y_with_a_result_resumes_and_injects`

## Verification

```bash
cargo build -p aida-tui
cargo fmt --all -- --check
cargo clippy -p aida-tui -p aida-cli -- -D clippy::correctness
bash scripts/glyph-lint.sh --block
env -u AIDA_SESSION_ROLE cargo test -p aida-tui
```

## Followups

- **TASK-909** — capture raw `Ctrl-D` from the hosted chat as an alternate palette-open
  trigger (the EPIC-51 `Ctrl-D` entry point; today it's `prefix p`).
- A live end-to-end manual verification (real Claude chat + inject + continue) for the
  lead to run.
- Possible: a per-action "inject this" choice (currently `Ctrl-Y` injects the *last*
  result); and a config knob for `INJECT_MAX_LINES`.

## Related

- EPIC-51, STORY-678 (suspend), STORY-679 (palette), STORY-136 (inject-into-focused-PTY).
