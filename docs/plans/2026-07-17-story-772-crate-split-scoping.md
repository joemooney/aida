# STORY-772 — Crate-split scoping (aida-cli → thin bin + handler lib)

**Date**: 2026-07-17
**Specs**: STORY-772 (BlockedBy STORY-770, now merged)
**Status**: Scoping complete — awaiting go/no-go + acceptance re-scope
**Complexity**: Large (structural), but the *move* is a near-mechanical 2-PR sequence

## Headline finding (correcting the earlier build-time premise)

A simple **bin + one-lib** split delivers only **~0–5% clean-build improvement and
nothing for incremental rebuilds** — the lib is still a single ~274K-line crate,
and `codegen-units` + incremental compilation already capture the intra-crate
parallelism/caching a bin/lib boundary would supposedly add. The earlier framing
(SPIKE-78 summary, STORY-770) that the crate split is "where the compile-time win
lands" is **wrong for this shape of split**. The real build lever would be
splitting the **lib itself into multiple crates**, which STORY-772 as written does
not do and does not unblock (it's gated by the `pub(crate)` shared-helper web).

**So STORY-772's value is testability + a clean entry-point boundary + a
precondition for a future multi-crate lib split — not build time.** Its acceptance
criteria should be re-scoped accordingly.

## Recommended topology

Thin bin, everything else into ONE lib; tests stay *inside* the lib.

```
aida-cli/            (bin crate, ~3 lines)
  src/main.rs        → fn main() { std::process::exit(aida_cli_lib::main_entry()) }
aida-cli-lib/        (library crate — the actual code)
  src/lib.rs         → pub fn main_entry() / pub fn run()
  src/cli.rs, *_cmd.rs, mcp.rs, ... (all 175 modules move verbatim)
  src/tests/*.rs     (all 164 test modules move verbatim, stay #[cfg(test)])
  build.rs           → moves here (owns the src/generated gRPC output + version stamps)
```

Keeping `run()`/dispatch in the **lib** (not the bin) is the key call: only **one
symbol** (`main_entry`) becomes `pub`; all 661 `pub(crate) fn` stay `pub(crate)`
because their callers move with them. Putting `run()` in the bin instead would
force ~660+ `pub(crate)` fns + many private helpers across the boundary — a
visibility explosion that also leaks internal API as public surface.

## The test decision (the crux)

The 164 relocated test modules use `use super::*` (124 files) / `use crate::…`
(27 files) to reach `pub(crate)`/private items. A **standalone test crate is not
viable** — it would require promoting essentially the whole `pub(crate)` surface
to `pub`. **Tests ride inside `aida-cli-lib`** exactly as today (`#[cfg(test)]
#[path="tests/NAME.rs"] mod NAME;` in `lib.rs`). Net test visibility churn: zero.
The 7 black-box integration tests in `aida-cli/tests/*.rs` spawn the binary and
are unaffected — they stay on the bin.

## Sequencing (green at every step)

1. **PR-1 (prep, no new crate):** add `src/lib.rs` to `aida-cli` that `mod`s
   everything `main.rs` does + `pub fn run`, reduce `main.rs` to a 3-line
   `fn main()` calling the same-package lib target (`[lib]` + `[[bin]]`). Move the
   164 `#[path]` test decls from `main.rs` to `lib.rs`. Proves the boundary
   compiles *within one package* — trivially revertible.
2. **PR-2 (carve the package):** create `aida-cli-lib/`, `git mv` all of
   `aida-cli/src/*` (except the 3-line `main.rs` + black-box `tests/`), move
   `build.rs`, split manifests (feature pass-through: `tui`/`remote`/`postgres`),
   add workspace member. Mechanical relocation — no visibility edits.
3. **PR-3+ (optional, independent):** relocate the ~101 inline `handle_*` still in
   the lib's `main.rs`/`lib.rs` into the `*_cmd.rs` pattern. Within-lib, unrelated
   to the split.

The move can't be subdivided module-by-module across the boundary — the
`crate::glyphs`/`crate::glyph` web (500+ refs) means it's a single clean move, not
N boundary negotiations.

## Risks / gotchas

- **`include_str!("../main.rs")` in `tests/headless_hint_tests.rs`** (scans main.rs
  source) breaks after the move — `../main.rs` becomes the 3-line stub. Retarget to
  `../lib.rs` (wherever the scanned env-setter code lands) and re-verify the
  `bug_376` `../../../aida-core/templates/...` depth from the new tests dir.
- **`build.rs` must move with the source it generates** (`src/generated` gRPC);
  leaving it on the bin breaks `remote` builds. `aida-core/build.rs` (template
  embedding) is separate and stays put.
- **Cross-worktree cargo cache:** the new crate name invalidates every cached
  `aida-cli` artifact fleet-wide on first build — one-time rebuild storm.
- **Feature pass-through:** bin must forward `tui`/`remote`/`postgres` or
  `-p aida-cli --features remote` silently drops them. Add a CI matrix check.
- **MCP self-respawn:** benign — re-execs the installed binary by path; ensure the
  moved `build.rs` still emits `AIDA_BUILD_*` stamps (respawn compares `--version`).

## Recommendation

- **GO** on topology if the goal is testability / clean boundary / enabling a
  future multi-crate lib split. Two-PR, stays green, no visibility explosion.
- **Re-scope STORY-772's acceptance** away from build-time (it delivers ~0–5%),
  and file a follow-up for the *lib-internal multi-crate split* if build time is
  the actual pain — that's the real lever, and it's the harder, `pub(crate)`-web-gated job.

## Related

- STORY-770 (test relocation, merged — the enabling precondition)
- SPIKE-78 (the parent refactor)
- Follow-up to file: lib-internal multi-crate split (the actual build-time lever)
