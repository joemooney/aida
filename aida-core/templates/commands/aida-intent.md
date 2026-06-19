---
description: "Produce the plain-terms WHY-comprehension of a spec — read the spec plus its immediate graph neighborhood (parents, children, blockers, referenced specs, decisions, key comments) and distil its reason-for-being into two registers (layman prose + an LLM-dense brief). Distinct from `aida why` (the deterministic state classifier): this is a cached, drift-stamped LLM synthesis."
---
# AIDA Intent

Produce the plain-terms WHY-comprehension of a spec — read the spec plus its
immediate graph neighborhood (parents, children, blockers, referenced specs,
decisions, key comments) and distil its reason-for-being into two registers
(layman prose + an LLM-dense brief). Distinct from `aida why` (the deterministic
state classifier): this is a cached, drift-stamped LLM synthesis.

## Usage

```
aida intent <spec>                    Print the cached comprehension; generate on first call.
aida intent <spec> --audience layman  Plain prose for a human skimmer (default).
aida intent <spec> --audience llm     Denser/structured brief for an agent loading the spec.
aida intent <spec> --refresh          Force regeneration over the current graph.
aida intent <spec> --json             Machine-readable {spec, audience, comprehension, generated_at, model, stale}.
```

A second call prints the cache WITHOUT regenerating, UNLESS the spec or its
neighbors drifted since generation — then it shows a **STALE** marker (re-run
with `--refresh` to regenerate). Staleness is computed at read from a hash over
the neighborhood inputs; it is never stored.

This command launches `claude -p "/aida-intent <spec>"` headless. It is not the
skill itself — the skill (`.claude/skills/aida-intent.md`) is the comprehension
task the spawned agent follows.

## Instructions (for the spawned agent)

Follow the workflow in `.claude/skills/aida-intent.md`:

1. Read the spec (`aida show <SPEC> -c`) and its immediate graph neighborhood
   (`aida graph <SPEC>` + `aida show` on each one-hop neighbor).
2. Synthesize — do not transcribe — the spec's reason-for-being, grounded in the
   neighbors that give it context (the larger goal it serves, the blockers that
   shape it, the decisions that bound its solution space).
3. Write a single JSON object to the output path the prompt names, with keys
   `layman` (plain prose), `llm` (dense/structured brief), and `model` (your
   model id). Write ONLY that JSON; do not edit the spec — the launcher folds the
   sidecar onto the spec, stamping it with a timestamp + drift hash.

The load-bearing rule: this is **AI-generated comprehension, not ground truth**.
The launcher labels it as such; write what the graph supports.

ARGUMENTS: $ARGUMENTS
