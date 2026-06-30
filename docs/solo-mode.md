# Solo mode — the unattended advisor+integrator loop

**Status:** MVP runbook (EPIC-43 / TASK-815). Composes individually-verified
pieces into one loop; the *end-to-end composition* is new — **run it supervised
for a cycle or two before leaving it truly unattended.**

Solo mode is AIDA working the **safe** backlog end-to-end without per-step human
input — groom → implement → integrate → loop — while **parking keystone work
(security, architecture, the autonomy machinery itself) for the human.** It is
the answer to "work through the night," with the explicit constraint that it
never *ships* keystone unattended; it surfaces it for review.

## The loop

```bash
# 1. GROOM — a cold-boot advisor proposes approve/reject/park/queue per open
#    spec, then executes (propose-by-default; --apply executes; [intake] policy
#    bounds it). This is the advisor seat, autonomous.
aida intake --apply

# 2. IMPLEMENT — drain the advisor-blessed ready set: one worktree-isolated
#    implementer per spec → PR. `supervised` specs are EXCLUDED (keystone stays
#    parked). Use --concurrency 1 if any ready specs share a `serialize:<group>`
#    tag (collision-prone work serializes; independent work still parallelizes).
aida burndown run            # add --no-human for headless; --concurrency 1 to serialize

# 3. INTEGRATE — the single serial merge authority. Polls for Done + open-PR and
#    drives reviewer→CI→merge→pull→build, one at a time.
aida queue integrate --watch
```

Run them as a loop (intake → drain → integrate, repeat). `integrate --watch`
can run continuously alongside; the others re-run per cycle.

## The solo posture: drains honor the flag (TASK-827)

`aida solo` is not just a statusline marker — when it is active
(`presence::current_solo`), a fully-headless drain (`--no-human=both`) honors
it as a **maximum-discretion safe-backlog posture**. On a punted design-fork
the headless advisor escalates, the posture decides what happens next *per
spec*:

| Spec class | Solo active | Behaviour |
|------------|-------------|-----------|
| **Safe** (ordinary task/story, no keystone tag) | yes | **Proceed on the defensible default** — `--escalate-defaults` semantics. Maximum discretion: the drain ships the safe call and keeps moving. |
| **Keystone / architecture** (epic type, or `keystone` / `architecture` / `security` / `supervised` / `needs-supervised-build` / `blast-radius:high` / `risk:high` tag) | yes | **Park for the human** — `--escalate-blocks` semantics. Never ships keystone unattended; parks `NeedsAttention` for review, reusing the EPIC-28 park path. |
| any | no | **Unchanged** — your explicit `--escalate-*` flags win; solo supplies nothing. |

The drain prints one line per spec when the posture is in effect:
`🤖 solo posture: working safe backlog, parking keystone for human`.

This reuses the existing escalate mechanism (`EscalateMode::Defaults` /
`Blocks`) and the existing park path — it does not invent a new parking
system. The posture only applies under `--no-human=both` (where the advisor
escalation tier runs); a non-headless or non-solo drain is untouched. The
keystone classifier is conservative by design: a false positive merely parks a
safe spec for human review (cheap), while shipping keystone unattended (the
expensive error) is what it guards against. The posture decision is the pure
`presence::resolve_solo_posture` / `is_keystone_class`, unit-tested in
isolation. trace:TASK-827

## Composes with: the groom step gains an `--autopilot` option

The loop's step 1 (`groom`) gains an opt-in `--autopilot` flag — a
**bounded-authority envelope** over the binary `groom --apply`. Where `--apply`
is all-or-nothing (execute *every* proposed disposition once a spec clears the
fence), `--autopilot` is governed by a per-action authority map (auto / propose
/ never) plus a grounding gate, so it is **strictly more conservative** than the
binary apply: only reversible, in-fence, substrate-grounded actions auto-execute;
approvals and rejections are held for review by default; anything uncertain
escalates. See `docs/architecture/autonomy-and-escalation.md` §8 for the full
envelope and the orthogonal grooming-vs-draining framing.

Two things make it compose cleanly with this loop:

- **It inherits the solo keystone partition.** Autopilot's keystone fence and
  the solo posture above route through the *same* classifier
  (`presence::is_keystone_class`), so a keystone spec is parked for the human at
  the grooming stage exactly as it is at the draining stage — one classifier,
  consistent across both stages.
- **A headless solo loop tightens it further.** Under `--no-human`, an uncertain
  auto-action *demotes* to escalate (it cannot pause-and-ask). The tightening is
  demote-only, so the worst case is over-conservatism, never an un-gated execute.

Note: this is the **groom step's new option**, not a change to the solo-loop
default. Flipping the loop's grooming step to `--autopilot` by default is a
later, separate, **supervised** step — prove the envelope at the keyboard before
making it the unattended default. Until then, run it explicitly
(`aida groom --autopilot`) when you want the bounded posture.

## Why it's safe to leave running (the floor)

| Guarantee | Mechanism |
|-----------|-----------|
| Only one drain/integrate drives `main` at a time | `.aida/drain.lock` — both `burndown run` and `queue integrate` acquire it (BUG-538, TASK-812); a crashed holder's lock is stale-reclaimed |
| Keystone is never auto-shipped | `supervised` specs excluded from the drain; `supervised` / `review:draft-only` PRs **parked** by the integrator for human review (TASK-813) |
| Collision-prone specs don't co-fan | `serialize:<group>` tag → drain fans ≤1 per group per wave (TASK-814) |
| Escalations reach the human | reliable inter-agent mailbox (BUG-555) — a punt/fork/conflict lands in the advisor inbox + `aida questions` |
| The operator can see it running | `aida burndown status` reads the lock (pid / started / command) — so a peer session knows a drain/integrate is live (TASK-806) |

## What it PARKS for you (never decides alone)

- **Keystone / security / architecture** — `supervised`-tagged specs, `review:draft-only` PRs, the autonomy machinery itself.
- **Design forks** — a real choice (e.g. type-vs-tag) → filed as an `aida questions` DecisionRequest, not guessed.
- **Unresolvable conflicts** — a semantic merge it can't safely do → parked `NeedsAttention` + escalated, never force-resolved.

You drain those with `aida human` (decisions / reviews / triage), then let the
loop pick the rest back up.

## Caveats (honest)

- The pieces are individually tested; the **full loop is new** — supervise a
  cycle before unattended.
- `intake --apply` blessing the wrong thing is bounded by `[intake]` policy +
  the keystone exclusion, but review its first runs.
- Cross-platform: the drain lock + `pid_is_alive` are validated on Linux (PR CI);
  the nightly cross-platform run covers the rest.

## See also

`docs/autonomous-drain.md` (the drain user guide), `docs/architecture/autonomy-and-escalation.md`
(the escalation cascade), EPIC-43 (the vision + remaining: the typed `aida solo`
command, escalation-on-stuck polish).
