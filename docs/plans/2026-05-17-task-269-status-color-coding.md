# Plan: TASK-269 — color-code Status + reprint as last line

Date: 2026-05-17
Specs: TASK-269
Status: In Progress
Complexity: ~110 prod LOC, ~70 test LOC, 1 commit, risk low

## Approach

`aida show <ID>` prints the Status field uncolored near the top, where it
scrolls off-screen on long specs. TASK-269 asks for two compounding fixes:
color-code Status inline, and reprint it as the last line after a rule so the
answer to "what state is this in?" sits at the cursor when the command
returns. The user picked the *full unification* scope: every requirement-status
display in the CLI shares one palette.

Strategy: introduce a single `status_display` module holding the spec's
glyph + colour palette, then route every status renderer through it. Glyph +
colour go in the prominent single-status displays (`aida show` top + reprint,
the spec card one-liner, the `[badge]` chip in `aida queue list` and the
`aida status` overlay). Fixed-width table columns (`aida list`, `aida history`)
adopt the unified *colour* only — a 2-char glyph prefix would break column
alignment and the column header already labels the field. `colored` degrades
to plain text under `NO_COLOR` / non-TTY automatically, so the `NO_COLOR`
criterion is satisfied for free; glyphs are plain Unicode and always print.

```
                       status_display
   ┌──────────────────────┬───────────────────────┐
   status_glyph(&str)   paint_status(text,&str)  status_badge(&str)
        │                      │                      │
        │  glyph+colour ───────────────────────────────┤
        │   aida show (top + reprint), card, queue list, status overlay
        │                      │
        └ colour only ─────────┘
            aida list, aida history (colorize_status)
```

## Decisions

- **One module, three functions.** `status_glyph` (Unicode glyph),
  `paint_status` (apply palette colour to arbitrary text — lets table cells
  be padded *then* coloured), `status_badge` (`"<glyph> <coloured status>"`).
- **Spec's proposed palette is authoritative** — the user authored it in the
  spec on 2026-05-16: Draft=dim ◯, Approved=cyan ▸, Planned=blue ▷,
  InProgress=yellow ◐, Done=bright-green-bold ◉, Completed=green ✓,
  Rejected=red ✗. Done stays bold-bright-green to keep the STORY-86
  "done on a branch" vs plain-green "merged" distinction.
- **Glyphs in detail/chip views, colour-only in fixed-width tables.**
  `aida list` / `aida history` columns are width-budgeted; a glyph prefix
  misaligns them. Unified colour is the consistency win the user asked for.
- **`aida session leases` is excluded.** Its status column renders
  `LeaseState` (Live/Dormant/Stale) — a different enum that already has its
  own glyph + colour. Not a requirement status; out of scope by nature.
- **Legacy SQLite paths skipped** (`list_requirements`, `show_requirement`,
  `handle_feature_command` at the `main.rs:5000-5800` band) — CLAUDE.md says
  not to invest in the deprecated backend; they print a deprecation warning.
- **gRPC `client.rs` skipped** — separate display surface over `proto::`
  enums, rarely exercised; noted for a possible followup.
- **Unknown/custom statuses** get a neutral `·` glyph + `normal()` colour so
  project-specific `custom_status` values never panic the match.

## Files (build order)

1. `aida-cli/src/status_display.rs` — **new.** The palette module:
   `status_glyph`, `paint_status`, `status_badge`, private `normalize`
   (lowercases, strips whitespace/`-`/`_` so "In Progress", "in-progress" and
   padded cells all match). Plus `#[cfg(test)]` unit tests.
2. `aida-cli/src/main.rs` — `mod status_display;` declaration; then:
   - `aida show` inline Status line (`Command::Show` arm, `effective_status()`
     print) → `status_badge`.
   - reprint block after `print_git_linkage` in the same arm → rule +
     `Status: <badge>`.
   - `render_spec_card` — Brief one-liner and Balanced one-liner → `status_badge`.
   - `handle_queue_command` — grouped view and flat view status chips →
     `status_badge` inside `[...]`.
   - `aida status` overlay — queue-head chip and recent-activity row → palette.
   - `aida list` (`Command::List` git-backend arm) — both layouts: pad the
     status cell, then `paint_status`.
3. `aida-cli/src/history.rs` — `colorize_status` becomes a thin wrapper over
   `status_display::paint_status` (keeps the `(deleted)` arm).

## Critical Files

- `aida-cli/src/status_display.rs` — the single source of palette truth; every
  other change is a call into it.
- `aida-cli/src/main.rs` `Command::Show` arm (~line 2613-2730) — the core
  deliverable: inline colour + the bottom reprint.
- `aida-cli/src/history.rs` `colorize_status` — the existing helper being
  subsumed; getting its signature wrapper right keeps the history table
  aligned (it is called with a pre-padded cell).

## Reusable helpers

- `colored` crate (`Colorize`, `ColoredString`, `colored::control`) — already
  the project's colour primitive; auto-respects `NO_COLOR` + TTY.
- Existing pattern `colorize_status` in `history.rs` — the prior art being
  generalised; its "pad-then-colour" comment is the alignment lesson reused.
- `req.effective_status()` (`aida-core/src/models.rs`) — the canonical status
  string, already used by `aida show` / the card.

## Risks + gotchas

- **Column alignment.** `{:<width}` counts bytes including ANSI escapes — must
  pad the plain string *before* colouring (the FR-1-070 / history.rs lesson).
  Applies to `aida list` and `aida history`.
- **`In Progress` already overflows** `aida list`'s `{:<10}` Status column
  (11 chars) — pre-existing; colour-only change does not worsen it.
- **`colored` global override** — `colored::control::set_override` is process
  global; NO_COLOR unit tests must not run order-dependent. Keep colour tests
  self-contained / assert on glyph + text rather than escape codes where
  possible.
- **`effective_status()` may return a `custom_status`** — `normalize` + the
  `_ =>` arm keep that safe.

## Tests (named)

In `status_display.rs` `#[cfg(test)]`:
- `glyph_for_each_canonical_status` — all 7 statuses map to the spec glyphs.
- `glyph_normalizes_spacing` — "In Progress", "InProgress", "in-progress",
  and a padded "Approved   " all resolve.
- `glyph_unknown_status_is_neutral_bullet` — custom status → `·`.
- `badge_contains_glyph_and_label` — `status_badge("Done")` contains `◉`
  and `Done`.
- `paint_status_plain_under_no_color` — with `set_override(false)`,
  `paint_status` output has no `\x1b` escape.

## Verification

```bash
cargo build -p aida-cli
cargo test -p aida-cli status_display
cargo test -p aida-cli colorize_status
# manual: status visible top + bottom, colour on, glyphs present
./target/debug/aida show TASK-269 | tail -3
NO_COLOR=1 ./target/debug/aida show TASK-269 | tail -3   # plain + glyph, no escapes
./target/debug/aida list | head -8
./target/debug/aida queue list
cargo fmt --all -- --check
```

## Followups

- Retrofit the gRPC `client.rs` status display onto `status_display` once the
  server/client path has users.
- Consider widening `aida list`'s Status column and adding the glyph there too
  if the colour-only table treatment proves insufficient for colourblind users.

## Related

- TASK-265 — `aida show --card`; the card one-liner is one of the call sites.
- STORY-86 — Done vs Completed; the bold-bright-green Done colour is preserved.
- TASK-256 — TUI theming; palette uses semantic colours stable on dark + light.
- `user_likes_emoji.md` — glyphs are explicitly wanted, never stripped.
