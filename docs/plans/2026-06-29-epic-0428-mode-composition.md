# Plan: TASK-0432 — Autopilot vs zen / solo / intake / no-human composition

Date: 2026-06-29
Specs: TASK-0432 (parent EPIC-0428) — depends on TASK-0429 (envelope)
Status: Draft — **design only, needs master-advisor sign-off before any code**
Complexity: ~60 prod LOC + ~80 test LOC + doc edits when built, 0 commits now, risk medium (precedence bugs are silent-degradation bugs)

<!-- Do NOT implement. This plan RATIFIES the surface the other three assumed
     (config posture + one flag) and defines precedence. -->

## Approach

EPIC-0428 lists four existing autonomy surfaces autopilot must compose with —
`--zen`, `--no-human`, solo mode, and `intake --apply` (now `groom --apply`) —
and asks the structural question: **is autopilot a new mode, a config posture, a
role-presence flag, or a command family?** This plan answers it and pins
precedence.

The key clarifying insight: autopilot operates on a **different axis** than the
three-mode ladder. The ladder (`default → --zen → --no-human`,
`docs/architecture/autonomy-and-escalation.md` §1) parameterises a *drain* —
how the implement→CI→review→merge pipeline handles `kind:confirmation` vs
`kind:design-fork` prompts. Autopilot parameterises the *advisor disposition
pass* — how `groom` decides a spec's fate. They are **orthogonal stages of the
loop**: groom (advisor) decides what enters the ready set; the drain
(implementer) works it. So autopilot is **not a fourth rung on the ladder** —
it is the *grooming-stage analog* of what the ladder is to the draining stage.

Therefore: **autopilot is a config posture over `groom` plus one activation
flag (`--autopilot`)** — exactly as TASK-0429/0430/0431 assumed. It is *not* a
new top-level mode, *not* a new command family (`groom` already owns the verb),
and *not* a role-presence flag (it does not change *who* is present; it changes
*how much the advisor seat may auto-dispose*). The existing `solo`
presence-marker is the closest analog and the right composition partner.

### Diagram — orthogonal axes of the loop

```
   GROOMING STAGE                        DRAINING STAGE
   (advisor: what enters ready set)      (implementer: work the ready set)
   ────────────────────────────────      ──────────────────────────────────
   groom (propose-by-default)            default   (pause every step)
   groom --apply (binary execute)        --zen     (auto-resolve mechanical)
   groom --autopilot (envelope) ◄─NEW    --no-human (punt → advisor cascade)
                                         solo posture (safe-backlog discretion)

   autopilot governs the LEFT column.   The ladder governs the RIGHT column.
   They compose; they do not conflict — different stages, different prompts.
```

## Decisions

- **Decision: autopilot is a config posture (`[autopilot]`) + one flag
  (`groom --autopilot`).** Not a new mode, not a command family, not a
  role-presence flag. **Rationale**: `groom` already owns disposition; the
  three-mode ladder already owns drain autonomy; a new top-level mode would
  straddle both and force users to reason about a 4×3 matrix. A posture-over-an-
  existing-verb is the minimal surface and mirrors how `[intake]` already tunes
  `groom`. (This ratifies the surface TASK-0429/0430/0431 built against.)

- **Decision: precedence is layered, not ranked — autopilot governs grooming,
  the ladder governs draining, and they apply at different stages.**
  **Rationale**: there is no "which wins" between `--autopilot` and `--zen`
  because they never decide the same thing. The only real composition question is
  what happens in `groom --autopilot --then-drain` (groom under the envelope,
  then a drain under whatever ladder mode the drain flags specify) — and the
  answer is "each stage uses its own setting", which the existing `on_apply`/
  `--then-drain` plumbing already expresses.

- **Decision: `--no-human` chains autopilot at the grooming stage.**
  **Rationale**: a fully-headless solo loop (`groom --apply` →
  `burndown run --no-human` → `queue integrate`) currently runs the *binary*
  groom. Under autopilot, the headless grooming step runs *under the envelope* —
  the cold-boot advisor's auto-dispositions are bounded by the action-authority
  map + grounding gate instead of "execute everything proposed". This is strictly
  *more* conservative than today's binary `--apply`, so it is safe to make
  autopilot the default grooming posture *inside* a headless solo loop once the
  envelope ships. Precedence: a headless context (`AIDA_HEADLESS`) tightens
  defaults — uncertain → escalate (it cannot pause-and-ask).

- **Decision: solo posture and autopilot compose via the SAME keystone
  classifier; solo's safe-vs-keystone partition becomes autopilot's gate-1/gate-3
  bias.** **Rationale**: `presence::resolve_solo_posture` already maps
  (solo_active, is_keystone) → ProceedOnDefault / ParkForHuman. Autopilot reuses
  `is_keystone_class` for gate 1 (TASK-0429) — so when solo is active, autopilot
  inherits the *exact same* "ship safe / park keystone" posture the drain uses.
  No new posture system; autopilot's grounding gate is the grooming-stage mirror
  of solo's escalate-defaults-vs-blocks on the drain side. One classifier, two
  stages, consistent behavior.

- **Decision: implementation reuses intake policy, solo posture, the advisor
  escalation tier, and findings/punts wholesale — autopilot adds only the
  action-authority map + the four-gate `evaluate`.** **Rationale**: identified
  reuse (acceptance criterion). Nothing here is greenfield except the envelope
  itself (TASK-0429) and the audit record shape (TASK-0430).

- **Decision: update `docs/architecture/autonomy-and-escalation.md` (new §8) AND
  `docs/solo-mode.md` (compose-with note).** **Rationale**: autopilot is an
  autonomy-architecture concept (belongs in the autonomy doc as the grooming-
  stage analog of the cascade) and a solo-loop participant (belongs in the solo
  runbook as the grooming step's new posture). Both, per acceptance.

## The composition matrix (the deliverable)

| Context | Grooming stage (`groom`) | Draining stage | Autopilot's effect |
|---------|--------------------------|----------------|--------------------|
| **default** (operator at keyboard) | propose-by-default; operator confirms | default ladder (pause every step) | autopilot off — operator drives disposition |
| **`groom --autopilot`** | envelope auto-executes in-fence, grounded, in-authority actions; rest held/escalated | n/a (grooming only) | the new posture |
| **`--zen` drain** | unchanged (grooming is a separate stage) | mechanical auto-resolve, design-fork pause | independent — zen is drain-side |
| **`--no-human` solo loop** | `groom --autopilot` (envelope, headless-tightened) | burndown `--no-human` → advisor cascade | autopilot bounds the headless groom (more conservative than binary `--apply`) |
| **solo posture active** | autopilot inherits safe/keystone partition via `is_keystone_class` | drain uses ProceedOnDefault/ParkForHuman | one classifier, consistent across stages |
| **`groom --apply` (no `--autopilot`)** | **unchanged** binary execute (back-compat) | n/a | autopilot opt-in; existing behavior preserved |

## Files (in build-order)

### `aida-cli/src/autopilot.rs` (new) — headless tightening + solo inheritance (created by TASK-0429)

- `fn effective_envelope(base: AutopilotEnvelope, headless: bool, solo: SoloPosture) -> AutopilotEnvelope` — pure composition: `headless` demotes any `auto` that would require a pause-on-uncertainty to `escalate`; `solo` keystone-parks. **Unit-testable** — the precedence table is the test surface.

### `aida-cli/src/main.rs` — launcher wiring

- `handle_intake_command` (`main.rs:88127`): when `--autopilot` is set, build the envelope, apply `effective_envelope` with the resolved `AIDA_HEADLESS` + `presence::current_solo`, and route dispositions through `autopilot::evaluate` instead of the binary apply. `--then-drain` is unchanged (it chains the existing burndown).

### `aida-cli/src/cli.rs` — the flag

- `--autopilot` on the `Groom` variant (next to `--apply`), documented as "execute dispositions under the `[autopilot]` envelope instead of all-or-nothing".

### `docs/architecture/autonomy-and-escalation.md` — new §8

- "Advisor autopilot — the grooming-stage analog of the cascade." The orthogonal-axes diagram, the composition matrix, and the statement that autopilot reuses the §3 Type A/B/C calibration as its grounding gate.

### `docs/solo-mode.md` — compose-with note

- The solo loop's groom step gains an `--autopilot` option; document that it is *more* conservative than the binary `--apply` and inherits the solo keystone partition.

### `aida-core/templates/docs/aida/discipline/` — vocabulary

- Add "autopilot" to `machinery-glossary.md` (the orthogonal-axis framing) so downstream projects inherit the precise mental model.

## Critical Files

- `aida-cli/src/autopilot.rs` (new) — created by TASK-0429
- `aida-cli/src/main.rs`, `aida-cli/src/cli.rs`
- `docs/architecture/autonomy-and-escalation.md`
- `docs/solo-mode.md`
- `aida-core/templates/docs/aida/discipline/machinery-glossary.md`

## Reusable helpers (do not reimplement)

- `presence::resolve_solo_posture` / `SoloPosture` / `current_solo` (`aida-cli/src/presence.rs:390-428`) — solo composition; autopilot's `effective_envelope` takes a `SoloPosture`, never re-derives it.
- `presence::is_keystone_class` (`aida-cli/src/presence.rs`) — the single keystone classifier shared across groom fence, solo posture, drain, and `queue integrate`. Autopilot's gate 1 routes through it (TASK-0429) — guarantees no stage disagrees.
- `IntakeConfig` / `OnApply` / `--then-drain` plumbing (`aida-cli/src/intake.rs`, `main.rs:88206`) — the existing groom→drain chain; autopilot does not touch it.
- The advisor escalation tier (`/aida-advise`, STORY-306) + `punt.rs` + `findings.rs` — autopilot's "escalate" outcome reuses the cascade's tier-2→tier-3 path verbatim.
- `AIDA_HEADLESS` / `AIDA_ZEN` resolution (the three-mode ladder, autonomy doc §1) — read, don't reinvent; autopilot only *reads* headlessness to tighten its own grooming defaults.

## Risks + gotchas

1. **Risk: users expect `--autopilot` and `--zen` to be the same axis and set
   both expecting compounding autonomy.** **Mitigation**: the docs lead with the
   orthogonal-axes framing; the glossary entry and the composition matrix make
   "grooming stage vs draining stage" explicit. `--zen` on a `groom` command is a
   no-op (different stage) and should warn-and-ignore, not silently accept.

2. **Risk: precedence bug — a headless context fails to tighten autopilot, so an
   uncertain auto-action executes unattended.** **Mitigation**: `effective_envelope`
   is pure and unit-tested with the full (headless × solo × base-authority)
   cross-product. The headless tightening is *demote-only* (never widens), so the
   worst-case bug is over-conservatism (a held action), not an un-gated execute.

3. **Risk: back-compat — making autopilot the default grooming posture inside
   the solo loop changes existing `--no-human` behavior.** **Mitigation**:
   autopilot is **opt-in** (`--autopilot` / `[autopilot]` config). The binary
   `groom --apply` path is untouched until a project explicitly adopts the
   envelope. The solo-loop default flips to autopilot only in a *later, separate*
   spec after the envelope is proven (flagged as a followup, not this work).

4. **Risk: two keystone classifiers drift (groom fence vs solo posture).**
   **Mitigation**: this is exactly why TASK-0429 mandates routing gate 1 through
   `is_keystone_class` — there is *one* classifier. A test asserts the groom
   fence and `resolve_solo_posture` agree on a keystone fixture.

## Tests (named)

- `effective_envelope_headless_demotes_uncertain_auto_to_escalate` — headless tightening.
- `effective_envelope_headless_never_widens` — demote-only invariant.
- `effective_envelope_solo_keystone_parks` — solo inheritance.
- `effective_envelope_default_context_is_base_envelope` — no-op composition.
- `groom_apply_without_autopilot_is_unchanged_binary` — back-compat.
- `zen_flag_on_groom_warns_and_noops` — orthogonal-axis guard.
- `keystone_fence_and_solo_posture_agree` — one-classifier invariant.

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && "$AIDA_BIN" init >/dev/null
printf '\n[autopilot]\ntag = "auto"\napprove = "propose"\n' >> .aida/config.toml
"$AIDA_BIN" add --title "safe tidy" --type task --status draft --tags "risk:low"
"$AIDA_BIN" add --title "keystone work" --type task --status draft --tags "architecture"

# Back-compat: binary groom --apply unchanged (no --autopilot).
"$AIDA_BIN" groom --apply --dry-run | grep -iv 'envelope'   # legacy path, no envelope mention

# Autopilot grooming is its own stage; --zen (drain axis) is a no-op here.
"$AIDA_BIN" groom --autopilot --zen --dry-run 2>&1 | grep -i 'zen.*ignored\|no-op'

# Headless tightens: uncertain auto-actions demote to escalate.
AIDA_HEADLESS=1 "$AIDA_BIN" groom --autopilot --dry-run | grep -i 'headless'

# Solo posture parks keystone even under autopilot.
"$AIDA_BIN" solo on >/dev/null 2>&1 || true
"$AIDA_BIN" groom --autopilot --dry-run | grep -i 'keystone.*park\|fenced.*architecture'
```

## Followups

- Followup TASK (file at sign-off): flip the *solo-loop default* groom step to `--autopilot` once the envelope is proven in supervised use (separate, later — `feedback_reliability_fixes_use_keyboard_not_drain`: prove the autonomy keystone at the keyboard first).
- Followup: the §8 autonomy-doc edit is a *living-doc* update — leave dated SPIKE/snapshot artifacts frozen (`feedback_dated_artifacts_immutable`).

## Related

- TASK-0429 (envelope), TASK-0430 (audit), TASK-0431 (product), `docs/architecture/autonomy-and-escalation.md` §1–§4, `docs/solo-mode.md`, `feedback_three_mode_autonomy_taxonomy`, TASK-827 (solo posture), BUG-594 (keystone fence).

## Recommendation + smallest first slice

**Recommendation**: declare autopilot a **config posture over `groom` plus one
opt-in flag** — explicitly *not* a fourth rung on the three-mode ladder, because
it governs the orthogonal grooming stage, not the draining stage. Make the one
keystone classifier (`is_keystone_class`) the shared invariant across groom
fence, solo posture, drain, and autopilot so no stage can disagree. Keep the
binary `groom --apply` path untouched (opt-in), and flip solo-loop defaults to
autopilot only later, after supervised proof.

**Smallest first slice**: write the **doc deliverable first** — the new §8 in
`docs/architecture/autonomy-and-escalation.md` (orthogonal-axes framing +
composition matrix) and the `docs/solo-mode.md` compose-with note — *before any
code*. This is a design task whose highest-value output is the shared mental
model that prevents the precedence bugs; ratifying "autopilot is a grooming-stage
posture, the ladder is a draining-stage axis, they compose via one keystone
classifier" is what unblocks TASK-0429's implementation cleanly. The pure
`effective_envelope` composition function + its cross-product tests are the
second slice, landing alongside TASK-0429's `evaluate`.
