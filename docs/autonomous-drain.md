# Autonomous drain — `--no-human`

`aida queue work --auto-complete` drives a spec through the full
implementer → CI → reviewer → merge → pull → build lifecycle in one command.
By default each Claude phase is **interactive**: the orchestrator launches
`claude`, you watch it work, and you press **Ctrl+D** when a phase is done so
the orchestrator advances. That is the right mode for the everyday "watch a
spec ship in ~25 min" pattern.

`--no-human` removes the Ctrl+D. It launches the phase's Claude session
**headless** (`claude -p`) — a single-turn run that exits on its own when the
work is finished. That is what makes an unattended overnight drain possible:

```bash
aida queue work --batch nightly --auto-complete --no-human
```

## What runs headless today

This is a **reviewer-first cut**. Of the two Claude phases:

| Phase | `--no-human` behaviour |
|-------|------------------------|
| 1 — implementer | **Interactive** — unchanged. The headless implementer needs a design-pause→punt path and is tracked separately (STORY-276). |
| 3 — reviewer | **Headless** — `/aida-review` runs `claude -p`, writes its verdict file, and exits. No Ctrl+D. |

The reviewer is the safe phase to run headless: `/aida-review` already writes
its verdict to a file the orchestrator reads (the `--auto-complete` handshake)
and stops before any merge — the orchestrator owns phase 4. A headless
reviewer just stops needing a keystroke.

### MODE selector

- `--no-human=reviewer-only` — headless reviewer; implementer stays interactive.
- `--no-human` / `--no-human=both` — requests both. The headless implementer
  is not wired yet, so phase 1 still runs interactively and the orchestrator
  prints a one-line note. The flag grammar will not change when the headless
  implementer lands — it just starts honouring `both`.

`--unattended` and `--headless` are accepted as aliases of `--no-human`.

### How the headless launch is configured

Headless sessions launch with the flag set verified empirically in SPIKE-7
(`docs/spikes/2026-05-16-claude-headless.md`):

- `--permission-mode bypassPermissions` — **mandatory**. `acceptEdits` leaves
  `Bash` gated and `default` auto-denies tools silently; either way the work
  does not happen. `--no-human` forces `bypassPermissions`.
- `--output-format stream-json --verbose` — newline-delimited JSON events,
  streamed to `<project>/.aida/headless-logs/<branch>-<session>.jsonl`.
- `--session-id <uuid>` — persistence stays on, so a killed run is resumable.
- Never `--bare` — it breaks OAuth/keychain auth.

## When `--no-human` is appropriate

Good candidates:

- **Small specs with clear, checkable acceptance criteria.** The implementer
  has no real decision to make; the reviewer has a concrete checklist.
- **Batches of similar, low-ambiguity work** — a `batch:NAME` of mechanical
  TASKs is the canonical overnight drain.

Use interactive (omit `--no-human`) for:

- **Specs whose description mentions a design fork / open question.** A
  headless Claude cannot pause to ask you — under `--no-human` it stops
  cleanly with the work undone rather than guessing. Decide those at the
  keyboard.
- **Anything where you want to shape the approach as it goes.**

The trade-off is simple: **interactive = better decisions, autonomous =
better throughput.** Pick per session, not once per project.

## Cost

A full implementer→reviewer lifecycle is roughly **$3 of API spend per spec**
at current Opus prices (SPIKE-7 measured ~$3.80 across its test suite). An
overnight drain of a 20-item batch is a ~$60 run. Size your batches
accordingly.

## Limits of this cut

- The implementer phase is not headless — `--no-human=both` runs it
  interactively (STORY-276).
- There is no liveness watchdog yet: a genuinely stuck headless run is not
  auto-detected. The `stream-json` log is written so the watchdog can be
  added (TASK-298). Until then, treat a drain that has not progressed for a
  long time as needing a look.
- Skill templates have not been audited for interactive prompts beyond the
  reviewer/merge path (TASK-297).

## Related

- `docs/spikes/2026-05-16-claude-headless.md` — the empirical basis for every
  flag and caveat above.
- STORY-246 — the `--auto-complete` orchestrator.
- TASK-285 — `--batch --auto-complete`; `--no-human` composes with it.
