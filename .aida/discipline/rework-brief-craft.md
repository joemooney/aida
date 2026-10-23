# Rework-brief craft

A rework brief preserves a review finding across a cold session. It is not
"fix the review" and it is not a paraphrase written from memory.
<!-- trace:STORY-1351 | ai:codex -->

1. Open the reviewer verdict file named by the review result.
2. Extract each blocking finding and verify it against the diff and linked
   acceptance criterion.
3. Name the affected file and symbol, the failing or missing test, the observed
   behavior, and the concrete change required. Preserve the verdict path so the
   next reviewer can trace the handoff.
4. Write the brief for the implementation agent and requeue the same spec on
   its implementation role. Do not create an unrelated replacement spec.
5. Keep judgment in the finding and mechanics in the requested change. If the
   remedy requires a product/design choice, route that choice to the advisor
   instead of pretending it is implementation detail.
6. State the local gate, and only the local gate: ONE targeted
   `cargo test --profile agent -p <crate> <module filter>` run plus
   `make check-ci-fast`, then push. `make test-touched` and
   `make check-ci-compile` only when the change touches shared test infra
   (`test_env`, `test_git`, ratchet baselines) or more than 10 files. A brief
   never orders `make check-ci`, `make check`, `cargo test --workspace` or an
   unfiltered `--lib` run; the full suite is CI's job. It never sets
   `AIDA_CARGO_NO_SLOT` and never wraps cargo in `timeout N`. It tells the
   implementer to run long cargo in the background. On 2026-10-17 advisor
   briefs ordered `make check-ci`, and fix-2725 ran `make check` with the slot
   limiter bypassed: 1h20m on a host where the targeted gate takes minutes.
   <!-- trace:STORY-1853 | ai:claude -->
7. When the brief names a cargo command, write its budget as
   `AIDA_CARGO_TIMEOUT=<dur> cargo ...` and tell the implementer to run anything
   that can exceed about 10 minutes in the tool's background mode. Never stack
   an outer `timeout <N>` inside the tool's own cap: slot-queue wait counts
   against it. Tell them to check `cargo ps` after a timeout or KILLED run
   instead of retrying blindly. Until aida-host STORY-100 ships, the wrapper
   ignores the variable and background mode is the only protection. See "Cargo time budgets" in
   `implementer-discipline.md`. <!-- trace:STORY-1854 | ai:claude -->

A sufficient brief lets a fresh implementer answer: what failed, where, how to
reproduce it, what outcome is required, and which evidence will prove it fixed.
After writing it, verify the brief appears in `aida brief list --for-agent
<agent>` and that the spec is back on the correct role queue.
