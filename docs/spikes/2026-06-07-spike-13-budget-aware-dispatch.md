# Spike: multi-agent budget-aware dispatching surface

**Spec:** SPIKE-13 · **Date:** 2026-06-07 · **Status:** Done · **Mirrors:** SPIKE-7 / SPIKE-8 format

## TL;DR — verdict

**Recommend the portable, fused Path 2+3+4 (operator hints + observed-availability + heartbeat
`paused` state). Defer Path 1 (per-provider budget APIs) indefinitely. Build is still gated on data
recurrence (≥3–5 events; we have 2) — so this spike de-risks the design and files ONE minimal
follow-up, rather than shipping the surface now.**

Three findings:

1. **The right surface is one `paused` dimension on the agent registry, not a budget integration.**
   What dispatch actually needs is not "how many tokens does agent X have left" (unknowable
   portably) but "is agent X currently unavailable, and roughly when does it recover." That is a
   *state bit + a timestamp*, which the EPIC-31 registry and STORY-435 heartbeat already have the
   shape for. Budget exhaustion, rate-limit pause, and manual "operator took this agent offline" all
   collapse to the same `unavailable since T, expected back ~T'` record.

2. **Per-provider budget APIs (Path 1) are a trap.** Anthropic / OpenAI / Antigravity each expose
   different, changing, auth-coupled usage surfaces; Antigravity's is unknown; web `/ultraplan`
   budget is opaque (same finding SPIKE-8 reached about web-flow cost). Integrating them is
   non-portable, brittle against upstream change, and — critically — *pre-evidence*. The spec's own
   operator decision says "don't over-invest pre-evidence." Path 1 is the over-investment.

3. **The evidence bar isn't met yet, and that's fine — the manual discipline already covers us.**
   We have exactly 2 budget-pause observations (2026-05-23: Antigravity hourly limit; web
   `/ultraplan` usage cap). The spec self-gates at ≥3–5. The manual discipline
   (`feedback_multi_agent_budget_dispatching`) already prescribes the dispatch behavior; the
   substrate surface is an *amortization* of that discipline, worth building only once the event
   rate justifies the maintenance. This spike's job is to have the design ready when event #3–5 lands.

## How to read this doc

This is an **investigation spike** — no implementation, per its "Out of scope" (this is a SPIKE —
investigate, then file follow-up specs). Verified inputs: the 2 empirical observations from
`feedback_multi_agent_budget_dispatching` (2026-05-23), the spec's four candidate paths, and the
composes-with surfaces (EPIC-31 registry, STORY-435 heartbeat). The output is a recommendation + a
single drafted follow-up spec, ready to pull when the data recurs.

---

## The four paths, scored

| Path | Portable? | Cost | Pre-evidence risk | Verdict |
|---|---|---|---|---|
| **1. Per-provider budget API** | ✗ (per-provider, auth-coupled, changes upstream; Antigravity unknown) | High (N integrations + maintenance) | High — couples to APIs before we know the surface earns it | **Defer indefinitely** |
| **2. Operator-supplied budget hints** | ✓ (just data the operator declares) | Low | Low | **Adopt** (input layer) |
| **3. Observed-availability inference** | ✓ (advisor records pause events against agent identity) | Low | Low | **Adopt** (auto-capture layer) |
| **4. Heartbeat-based `paused` state** | ✓ (extends STORY-435 `last_active_at`) | Low–Med (one field + surfacing) | Low | **Adopt** (storage + display layer) |

Paths 2, 3, and 4 are not competitors — they are the **input**, **auto-capture**, and
**storage/display** layers of one surface. Path 1 is a different, heavier bet that the cheap surface
makes unnecessary for the dispatch use case.

## Recommended MVP design (the fused surface)

One state extension to the agent registry record (EPIC-31), reusing STORY-435's heartbeat fields:

```
agent record (existing: id, role, last_active_at, ...)
  + availability: Available | Paused
  + paused_since:  <timestamp>        # set when paused
  + paused_reason: budget | rate-limit | manual | unknown
  + expected_back: <timestamp|null>   # operator hint OR inferred from a known reset cadence
```

- **Input (Path 2):** `aida agent pause <agent> --reason budget [--resets <when>]` and
  `aida agent resume <agent>` — the operator/advisor declares the state. Manual but portable; this
  is the load-bearing dispatch information the memory already says to track mentally.
- **Auto-capture (Path 3):** when the advisor observes a mid-task pause (the "agent dropped out"
  shape), it records `paused_since = now` against that agent identity — turning a surprise into a
  durable record so the *next* brief can see "paused 40 min ago."
- **Storage/display (Path 4):** `aida status` / `aida agent status` shows a `⏸ paused (budget, ~back
  14:00)` line per agent. `last_active_at` already exists; `availability` is the natural extension.
- **Brief-time guard (the actual payoff):** `aida brief <agent> <SPEC>` emits a *warning* (not a
  refusal) when the target agent is `Paused` — "claude is paused (budget, expected back ~14:00);
  brief anyway? consider <sibling>." A warning, not a block, keeps it advisory (per
  `feedback_substrate_as_bouncer_not_rules`, a hard gate is for invariants; budget headroom is a
  heuristic, so warn).

This is ~one enum + three fields + two small commands + one surfacing point + one brief-time check.
It composes cleanly with what exists and adds zero upstream coupling.

## Why not build it now

The spec gates itself ("revisit when budget-pause events recur, ≥3–5 data points") and the operator's
2026-06-06 disposition reaffirmed "KEEP PARKED, recurrence-triggered." With 2 observations, building
the surface risks polishing a coordination feature whose event rate doesn't yet justify its
maintenance — the over-engineering caution (`feedback_pushback_on_overengineering`). The manual
discipline already prescribes the right dispatch behavior. So: **design ready, build deferred.** When
event #3 (and ideally #4–5) lands, pull the filed follow-up and ship — no re-investigation needed.

## Deliverables produced

- **Recommendation:** fused Path 2+3+4 surface above; Path 1 explicitly out.
- **Follow-up spec filed (draft, recurrence-gated):** STORY for the minimal `paused`-state registry
  surface + brief-time warning (see Followups).
- **Discipline alignment:** the memory's "Substrate enhancements that would help" section already
  lists exactly these three (per-agent budget headroom in status; brief-time availability check;
  calibration pairing). This spike confirms that list and narrows it to the portable subset — no
  discipline-doc rewrite needed beyond pointing at this report.

## Followups

- File a draft STORY (recurrence-gated, `deferred:post-stability`): "Agent registry `paused`
  availability state + brief-time budget warning" — the fused Path 2+3+4 MVP above. Tag
  `from-spike:SPIKE-13`, link to EPIC-31 (registry) and STORY-435 (heartbeat). Pull when budget-pause
  events reach ≥3.
- Do NOT file a Path-1 (per-provider API) spec — recorded here as explicitly rejected so a future
  session doesn't re-propose it without new portability evidence.
- Each future budget-pause observation: record it against the agent identity (even before the surface
  exists) so the ≥3–5 gate is measurable rather than vibes-based.

## Related

- **`feedback_multi_agent_budget_dispatching`** — the manual discipline this surface would amortize;
  its empirical instances (2026-05-23 ×2) are this spike's entire evidence base.
- **EPIC-31** — agent registry + launcher + state; budget/availability is the missing dimension and
  the natural host for the `paused` field.
- **STORY-435** — MCP heartbeat (`last_active_at`); `availability: Available | Paused` is the direct
  extension.
- **STORY-447 / TASK-513** — effort-estimation calibration + credit-burn workflow; "expected duration
  vs agent availability" is a future calibration pairing, out of scope for the MVP.
- **SPIKE-8** (`2026-06-07-spike-8-ultraplan-comparison.md`) — reached the same "web-flow budget is
  opaque" finding that helps sink Path 1 for the `/ultraplan` agent specifically.
- **`feedback_pushback_on_overengineering`** — why this spike files a design and defers the build
  rather than shipping a coordination surface on 2 data points.
