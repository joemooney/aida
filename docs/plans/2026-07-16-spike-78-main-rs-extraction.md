# SPIKE-78 — Shrink `aida-cli`'s monolithic `main.rs`: command-handler extraction

**Date**: 2026-07-16
**Specs**: SPIKE-78
**Status**: Phase 1 complete (handler extraction). Test relocation + crate split deferred.
**Complexity**: Large (mechanical, high-volume; 61 pure-movement PRs)

## Outcome

`aida-cli/src/main.rs` shrank from **158,055 → 126,187 lines** (−31,868, **−20.1%**) by
lifting 61 command-handler clusters out of the monolith into dedicated
`aida-cli/src/*_cmd.rs` modules — **32,992 lines** now live in the extracted set.

Every extraction was **pure movement**: a handler body moved verbatim into its own
module behind a `pub(crate)` entry point, `main.rs` kept only the dispatch call, and
the combined workspace was rebuilt + tested green before each merge. No behavior
changed; no test was rewritten.

## Approach

Each cluster followed the same loop, one PR per cluster:

1. Cut the handler cluster (`handle_<area>_command` + its private helpers) into
   `aida-cli/src/<area>_cmd.rs`, exposing one `pub(crate) fn` entry point.
2. Replace the body in `main.rs` with the dispatch call; add `mod <area>_cmd;`.
3. Resolve `use cli::{...}` import churn as **union-of-removals** (drop only the
   names that left with the handler).
4. `cargo fmt -p aida-cli` **in place** (never a piped `--check`, which masks the
   exit code after a manual resolution).
5. `env -u AIDA_SESSION_ROLE cargo test -p aida-cli --bin aida` (the var
   false-fails role-gated tests) → combined-main rebuild + test → merge on CI green
   → `aida pull` (auto-bump) → rebuild combined main between batches.

## The 61 extracted modules

Largest first (lines moved): `doctor` (5,626), `init` (2,764), `config` (2,207),
`tracker` (1,870), `plan` (1,780), `scaffold` (1,619), `dev` (1,333), `usage`
(1,261), `role` (854), `trace` (711), `statusline` (695) … then the long tail:
schedule, memories, archive, defer, graph, changelog, human, rebase, lint,
ultraplan, presence, internal, config, mailbox, digest, import_export, store,
metrics, feature, field_study, report, goal, focus, comment, compete, lifecycle,
team, relationship, db, deps, load, rel_def, session_misc, type, record, sandbox,
doc, server, triage, rules, node, autonomy, health, cache, health_vitals,
import_plan, decide, lock, assign, brief.

## Deliberately deferred (supervised follow-up)

- **Inline test relocation** — a real but smaller lever than first measured.
  A string/char/comment-aware scan (an earlier naive brace-counter was fooled by
  a `{`/`}` inside a literal and wildly over-counted) puts the actual split at
  **27% tests / 72% live code**: `main.rs` holds **166 `#[cfg(test)]` modules
  totaling 34,780 lines** (already subject-organized, largest 5,229 lines — there
  is no single "monster" module), and **91,408 lines of live code** (dispatch +
  keystone handlers + helpers + `main()` glue). Relocating all 166 test modules
  via `#[path]` sibling files under `src/tests/` drops `main.rs` to **~91K**
  (not ~16K), pure movement with zero import rewrites (the `mod` decl stays at the
  crate root so `use super::*` still resolves). The payoff is maintainability +
  rust-analyzer responsiveness + **unblocking the crate split** — the actual
  CI/compile-time win lands only once the split gives the tests their own crate
  with an independent incremental cache; within one crate, `cfg(test)` code
  already costs nothing on a plain `cargo build` and compiles regardless of which
  file it lives in. **Because main.rs is 72% live code, the bigger size lever is
  the keystone-handler extraction below, not test relocation.**
- **Keystone handlers left in place**: `pr`, `mcp`, and the
  orchestrator/drain/queue/status/solo cluster — high-churn, high-blast-radius
  surfaces that should move under supervision, not in an unattended sweep.
- **Full crate split** (`aida-cli` → thin binary + handler lib crate) — the
  structural endgame; the `*_cmd.rs` split is the enabling precondition, not the
  finish line.

## Verification

- 61/61 clusters merged to `origin/main` (PRs #1422–#1482 range), each CI-green.
- Combined main rebuilt + full `aida-cli` test suite (3,747 tests) green after
  every merge.
- Synced to both hubs (github `origin` + gitlab mirror).

## Related

- Tracking spec: SPIKE-78 (logged step-by-step via `aida comment add`).
- Follow-up specs to file: inline-test relocation, keystone-handler extraction,
  crate split.
