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

## Review findings reach the advisor (STORY-278)

A headless reviewer surfaces non-blocking findings — clippy noise, drifted
refs, small bugs — that a human would normally hand to the `dialog`/advisor
role to file as follow-up TASKs. With no human in the loop those follow-ups
would be lost when the drain moves on. So the headless reviewer files them
itself:

- AIDA exports `AIDA_HEADLESS=1` for every headless `claude -p` launch.
- `/aida-review` step 7b keys on it: after posting the consolidated PR
  comment, it files each non-blocking finding as a draft TASK tagged
  `from-review:PR-N,severity:<cosmetic|minor|major>`. It is idempotent — a
  re-review of the same PR skips filing if `from-review:PR-N` TASKs already
  exist.
- The advisor triages on its next session. `aida findings list` shows the
  pending findings grouped by PR and severity-sorted; the `dialog`-role
  SessionStart hook and `/aida-pickup` surface a one-line pending count.
  `aida findings promote <ID>` sends one to the work queue;
  `aida findings dismiss <ID>` rejects it with an audit comment.

A finding is always a `task` — the advisor re-types it to `bug` on promote
if warranted. "Findings" is a query (`from-review:` tag), not a new
requirement type.

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
