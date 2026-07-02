# The one per-spec engine invariant (ADR-7 / ADR-9)

**Status:** enforced by CI (`aida-cli/tests/adr9_one_engine_guardrail.rs`)
**Decisions:** ADR-7 (the invariant), ADR-9 (the enforcement), ADR-10 (autonomy mode as a typed engine parameter)

## The invariant (ADR-7)

Every per-spec driver runs the **same** implement → CI → review → merge → pull
lifecycle. That lifecycle lives in **one** place — `auto_complete::orchestrate_with_resume`
(the "engine"). `aida zen`, `aida integrate`, and `aida queue work --auto-complete`
differ only in **scope** (which spec/specs) + **lifetime** (one-shot vs watched) +
**autonomy mode** (see ADR-10), *never* in the per-spec lifecycle itself.

How each driver reaches the engine:

| Driver | How it routes through the engine |
|---|---|
| `aida queue work --auto-complete` | Calls `orchestrate_with_resume` **in-process** (the one production call site, in `main.rs`). |
| `aida zen <spec>` | **Self-invokes** `aida queue work <spec> --auto-complete --no-human <mode>` — a subprocess that re-enters the engine. It does NOT reimplement the phases. |
| `aida integrate` | Re-enters the engine via `queue work --from-pr` (phases 3–6 for already-Done PRs). |

## The single sanctioned exception (fleet layer)

`aida burndown` is the **one** allow-listed exception. It orchestrates a *fleet*
of implementer subagents (native Task-tool fan-out driven by the `/aida-burndown`
skill) — one level **above** the per-spec lifecycle. It deliberately does **not**
route each spec through `orchestrate_with_resume`; it is a fleet-layer orchestrator,
not a per-spec driver. This is a conscious, rationale'd divergence, not drift.

## Why it's a CI gate, not a rule (ADR-9)

A prose rule ("please route through the one engine") does not hold against a
confident agent: at zen slice-1 (PR #1231) a per-spec driver reimplemented a
truncated flow that stopped at implement+PR with **zero review**. Substrate-as-
bouncer: the invariant is now a **CI test**, not a convention.

`aida-cli/tests/adr9_one_engine_guardrail.rs` asserts that the engine
(`orchestrate_with_resume`) is called in production only from an explicit
allow-list of source files (the engine module + the `queue work` handler). A new
per-spec driver:

- **routes through the engine** → the right thing; if it adds a new *in-process*
  entry point, the author consciously adds its file to `ENGINE_CALLER_ALLOWLIST`
  (an acknowledged engine entry, not silent drift); or
- **reimplements the lifecycle** → the wrong thing; caught here (if it touches the
  engine) or at PR review against this document (if it re-forks like burndown did).
  A genuinely new fleet-layer exception must be added consciously and documented
  here — burndown is the only one today.

## Related

- `docs/architecture/autonomy-and-escalation.md` — the autonomy ladder + escalation cascade.
- `docs/autonomous-drain.md` — the practical drain/burndown user guide.
