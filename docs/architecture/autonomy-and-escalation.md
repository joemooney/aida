# Autonomy and escalation — AIDA's autonomous-collaboration architecture

*Living architecture doc · seeded 2026-05-22 · trace:TASK-339*

> **Living doc.** This describes architecture that is still evolving. Major
> pieces are shipped (the three-mode taxonomy, the advisor escalation tier,
> the file-based handshake substrate, fork-from-live); other pieces are
> designed but unshipped (subsystem-scoped advisors, persistent advisor
> entity); a few questions are genuinely open and called out as such. Dated
> claims are correct at seed time; check `aida list --type story --tags
> autonomy` and the spec graph for the current state. STORY-306 and its
> follow-ups (STORY-360 shipped 2026-05-21, STORY-362/364 approved) may
> still reshape parts of section 7.

## Audience

Two readers, one doc:

- **A human evaluating AIDA's architecture.** *What's the model?* *Where does
  it shrink the human's queue?* *Where could it silently degrade?*
- **An agent working inside AIDA.** *Which mode am I in?* *What does the
  substrate I'm reading expect me to do with a fork?* *Why is the substrate
  load-bearing?*

The two questions have the same answer, so the doc is written once for both.

---

## TL;DR

AIDA orchestrates work through a **three-mode autonomy ladder** (default →
`--zen` → `--no-human`), routes design decisions through an **escalation
cascade** (implementer/reviewer → headless advisor → human), and coordinates
all three tiers through **file-based handshakes** under `.aida/`. The advisor
tier's resolve-vs-escalate behaviour is calibrated against a **Type A/B/C
model** with a conservative-escalation bias: it resolves only what is
provably grounded in recorded substrate. The corpus that grounds it grows
with use — every escalated answer the human records converts a future Type-B
or Type-C fork into a Type-A — so autonomous operation **improves with use,
not just with bug-fixes**.

This is not a finished design. The hardest question — *who answers the
advisor's escalations, with what context?* — has a shipped v1 (cold-boot per
punt), a recently-shipped v2 (fork-from-live), and an unshipped v3
(persistent advisor entity / subsystem-scoped pack). The doc names what's
real, what's designed, and what's still open.

---

## 1. The three-mode autonomy taxonomy

AIDA's `aida queue work --auto-complete` flag drives a spec through the
six-phase lifecycle (implement → CI → review → merge → pull → build) in one
command. The user's relationship to that drain is parameterised by **two
orthogonal axes**:

1. **Is a human present?** (yes / no)
2. **What does the human want to be asked?** (every step / real questions only)

Conflating those two axes is what makes coarse "interactive vs unattended"
flags uncomfortable in practice. A user *at the keyboard* but uninterested
in clicking yes through thirty mechanical pauses is in a different posture
than a user who is *absent*. STORY-287 separated them.

### The three modes

| Mode | Flag / env | Persona | Mechanical (`kind:confirmation`) prompts | Design-fork (`kind:design-fork`) prompts |
|------|------------|---------|--------------|----------------------|
| **Default** | *(none)* | "Driving" — approves each step | Pause + ask | Pause + ask |
| **Zen** | `--zen` / `AIDA_ZEN=1` | "Advisor on standby" — consulted on real questions only | **Auto-resolve** (take option 1) | Pause + ask |
| **No-human** | `--no-human` / `AIDA_HEADLESS=1` | "Absent" — nobody reachable | Auto-resolve | **Punt** — escalation cascade fires |

The modes form a strict ladder: each rung is more autonomy than the one
above. Precedence is **`--no-human` > `--zen` > default**.

### Prompt classification — the load-bearing primitive

`--zen` works without any headless infrastructure. It is **pure prompt
classification.** Skill templates tag each `AskUserQuestion` prompt with an
HTML-comment annotation:

```markdown
<!-- kind:confirmation -->
> Open the PR? [Y/n]

<!-- kind:design-fork -->
> Scope this fix to `aida show` only, or unify status display across every
> CLI surface? [1/2]
```

Under `$AIDA_ZEN`, the runtime auto-resolves every `kind:confirmation` to
option 1 (its first / recommended choice) and still surfaces every
`kind:design-fork`. The classification rules — and the **option-1
convention** (option 1 is always the smallest-valuable-slice / lowest-risk
default) — live in `docs/aida/discipline/skill-prompt-kinds.md`. An
un-annotated prompt defaults to `design-fork` (pause-safe).

This means the *mode* is one bit of state; the *behaviour* is encoded in the
skills. New skills inherit the taxonomy by tagging their prompts correctly,
not by wiring into a runtime.

### Provenance corroboration — `--zen` and `--auto-complete` are not env-trust-only

A naïve "`AIDA_ZEN=1` → zen mode" check fails open: a stale or leaked env
var silently enables an autonomy the user did not authorize. BUG-233 caught
this for `AIDA_AUTO_COMPLETE`; BUG-237 caught the same class for
`AIDA_ZEN`. Both bugs reframed env vars from **trust signals** into **hints
that must be corroborated**.

```
                AIDA_ZEN=1 (env)
                      │
                      ▼
        ┌─────────────────────────────┐
        │ aida zen status             │
        │  ─ orchestrator run-uuid    │
        │    matches AIDA_AUTO_COMPLETE_TOKEN?
        │  ─ session lease records    │
        │    a zen-intent marker?     │
        └─────────────────────────────┘
              ┌────────┴────────┐
              ▼                 ▼
            "zen"          "interactive"
       (trust the env)   (leaked or stale — ignore)
```

Skill templates branch on `aida zen status` / `aida orchestrator status`,
not on the bare env vars. The corroboration tokens themselves are file-based
(orchestrator run-UUID in `.aida/`; zen-intent marker in the session
lease) — the same substrate as section 5's inter-agent comms.

### Failure mode the taxonomy prevents

Without the `--zen` rung: a user *at the keyboard* either grinds through
thirty mechanical pauses (default mode), or skips to fully unattended (one
risky flag), or improvises around the friction. The middle rung is the
common case; not having it pushes users toward "I'll just `--no-human` it"
when they shouldn't be.

Trace anchor: STORY-287, BUG-233, BUG-237, `docs/autonomous-drain.md`.

---

## 2. The escalation cascade

A `kind:design-fork` under `--no-human` is the load-bearing case: a real
choice with no human reachable. AIDA's response is a **three-tier
escalation cascade**, not a flat "pick the default and continue".

```
                ┌──────────────────────────────┐
   spec ──────▶ │ Tier 1 — IMPLEMENTER         │  resolves what the spec, the codebase,
                │ (or REVIEWER), headless      │  and recorded conventions decide
                └──────────────────────────────┘
                               │
                               │ design-fork it cannot safely resolve
                               ▼
                ┌──────────────────────────────┐
                │ Tier 2 — ADVISOR             │  resolves what recorded principle
                │ (headless `claude -p`,       │  or recorded preference decides
                │  /aida-advise skill)         │
                └──────────────────────────────┘
                               │
                               │ fork turns on strategy / irreversibility /
                               │ uncertainty the corpus doesn't answer
                               ▼
                ┌──────────────────────────────┐
                │ Tier 3 — HUMAN               │  the permanent tier; the queue this
                │ (morning triage, `aida       │  cascade *shrinks*, never replaces
                │  findings list`)             │
                └──────────────────────────────┘
```

Each tier **resolves what it can** and **escalates what it can't**. The
key property is *conservative escalation*: a tier that over-resolves
produces a confident-but-wrong autonomous decision, which is worse than the
cheap pause-and-ask it replaced. So the default bias at every tier is
escalate-when-uncertain.

### Tier 1 — implementer / reviewer (STORY-276, STORY-278)

The headless **implementer** (`claude -p` running `/aida-pickup`) reads the
spec, its acceptance criteria, its parent, and any owning plan file. If it
hits a design-fork it cannot safely resolve from those, it runs
`/aida-punt` rather than guessing. The orchestrator detects the punt via a
sentinel file (`AIDA_PUNT_SIGNAL_FILE`) and routes it to tier 2.

The headless **reviewer** (`/aida-review`) writes its verdict to a verdict
file (`AIDA_REVIEW_VERDICT_FILE`). When the merge decision is uncertain
(e.g. ambiguous CI signal, a non-obvious design call worth a human's eyes),
the verdict file carries `merge: escalated-to-human` — a first-class
non-failure outcome distinct from Approved / RequestChanges / Rejected
(BUG-241).

### Tier 2 — headless advisor (STORY-306)

The orchestrator spawns a fresh `claude -p` in the advisor (`advisor`) role
with the `/aida-advise` skill, the punt payload, and a path to write its
response to. The advisor does one of two things:

- **Resolve** — write the chosen answer + reasoning to the response file.
  The implementer session resumes via `claude --resume <session-id>` fed
  the advisor's decision; the drain continues with a *judged* call.
- **Escalate** — categorize the reason a human is needed (strategy /
  irreversible / unrecorded context / etc.), write a finding tagged
  `needs-human`, and either pause the spec for morning triage
  (`--escalate-blocks`, the default) or fall back to the defensible
  default (`--escalate-defaults`).

The advisor's resolve-vs-escalate calibration is section 3.

### Tier 3 — human

The morning-after surface is `aida findings list` (with `--source advisor`
to narrow to advisor-tier escalations). Each escalation carries the
advisor's *framing* of why a human was needed — strategy, irreversibility,
genuine corpus gap — which makes morning triage seconds-per-finding, not
minutes.

The cascade **never replaces** the human tier. It *shrinks* the human's
queue: the trivial forks resolved silently by tier 1, the
recorded-principle forks resolved by tier 2, and only the genuinely
human-shaped questions surface. The point is to spend the human's attention
on the questions that need it.

### Reconcile-against-reality (BUG-241)

A tier that ends *without* its expected artifact is not the same as a tier
that *failed*. A reviewer that escalates to a human and the human merges
out-of-band ends with no verdict file — but the work shipped. An
implementer whose spec is resolved-by-supersession ends with no PR — but
the spec is correctly Completed.

The orchestrator's per-phase success/failure detector reconciles against
ground truth (spec status, PR merge state, main HEAD) **before declaring
any phase failed**. False-failure crashes — "shipped 0 specs" when one
shipped, halted batch when work succeeded out-of-band — were the failure
mode BUG-241 closed. The discipline is phase-agnostic: reality wins over
"did the artifact I poll for appear?"

Trace anchor: STORY-276, STORY-278, STORY-306, BUG-241,
`aida-cli/src/auto_complete.rs`, `aida-cli/src/punt.rs`.

---

## 3. The advisor's resolve-vs-escalate calibration — Type A / B / C

Tier 2 is where autonomy *quality* either holds or silently degrades. The
advisor that over-resolves a fork it should have escalated is the failure
mode the entire cascade was built to prevent. Calibration is the
load-bearing primitive.

Every punted fork is one of three types. The advisor's core skill is
recognising which:

| Type | The answer needs… | Verdict |
|------|-------------------|---------|
| **A** | a **recorded principle** — discipline doc, spec graph, lifecycle rule, codebase convention, plan brief | **Resolve** — the corpus decides it, not the advisor |
| **B** | a **recorded user preference / intent** — memory, acceptance-criteria edit, prior decision comment | **Resolve only if the preference is actually recorded.** Unrecorded → **escalate**. |
| **C** | **synthesized in-flight context** — the working model built across a long session, threads connecting specs, judgment that lives in no single artifact | **Escalate** — a fresh advisor cannot reconstruct this |

Resolve **A** and **recorded-B**. Escalate **C** and **unrecorded-B**. When
the advisor cannot confidently place a fork in A or recorded-B, it is C by
default — escalate.

### Why the bias is conservative

A paused spec costs the human five minutes in the morning. A wrong-but-merged
overnight decision costs far more and is invisible until it bites. So
"I could probably guess" is **not** "I can resolve this": if the advisor
would be guessing, it escalates.

The advisor escalates outright when the fork turns on:

- **Strategy** — project direction, positioning, what to build next.
- **Irreversibility** — a public API shape, a data model, a release tag, a
  schema migration: anything expensive to undo.
- **Genuine uncertainty** — the recorded corpus simply doesn't answer it.

These are the categories the advisor cannot reliably ground in substrate,
so they pass straight to tier 3.

### Audit trail — every advisor decision is reviewable

The advisor records its resolve/escalate calls so the human can audit them
in the morning and catch a bad one. Each decision is logged with:

- Type classification (A / recorded-B / unrecorded-B / C)
- The recorded substrate the resolve cited (memory name, doc path, spec ID)
  — or the escalation reason category (strategy / irreversibility / etc.)
- The chosen answer (on resolve) or the framing handed to the human (on
  escalate)

`aida findings list --source advisor` surfaces these grouped by spec.
Resolved forks also leave a comment on the spec, so the resolution is
visible in the spec's own audit trail, not just in a findings index.

### Calibration mode (STORY-347)

With `[advisor] calibration_mode = "on"` in `.aida/config.toml`, every
advisor-tier punt produces **two verdicts side-by-side** — the cold-boot
verdict (drives the drain) plus a fork-from-live shadow verdict (recorded
only, no effect on the drain). When the two disagree, the disagreement is a
**substrate gap signal**: the live session knew something the cold-boot
substrate doesn't, and that something is worth writing into a memory or
acceptance criterion.

`aida findings calibration` shows the disagreements; `aida findings
calibration annotate <punt-id> "gap → wrote memory <name>"` closes the
loop. Cost is real (both runs fire) — turn it on to mine substrate gaps,
off when the substrate is mature.

Trace anchor: STORY-306, STORY-347, `.claude/skills/aida-advise.md`,
`feedback_headless_advisor_is_cold_boot.md`.

---

## 4. The corpus-growth feedback loop — why AIDA's autonomy improves with use

The Type A / B / C model has a property: **the boundary between the types
moves with the corpus.**

```
   t = 0                              t = 90 days
   ─────────                          ───────────
   ┌─────┐                            ┌────────┐
   │  A  │  (recorded principles)     │   A    │  ← grew
   ├─────┤                            ├────────┤
   │  B  │  (recorded preferences)    │   B    │  ← grew
   ├─────┤                            ├────────┤
   │  C  │  (synthesized context)     │   C    │  ← shrank
   └─────┘                            └────────┘
   most forks → tier 3                most forks → tier 2 resolves
   (human queue large)                (human queue small)
```

Every escalation the human resolves is an opportunity to **convert a future
Type-B or Type-C into a Type-A**:

- The human picks an answer → records it as a memory (`feedback_...md`),
  an acceptance-criteria edit, a comment on the spec, a discipline-doc
  paragraph.
- The next time a similar fork punts, the advisor finds the recorded
  answer and resolves it.

The escalation rate **decays over time** — not because the advisor gets
smarter, but because the substrate grows. The headless advisor and a live
one are the same Claude *model* with different *context*; the gap is
closeable by enriching the substrate, not by waiting for a smarter model.

This is the deeper reason AIDA's autonomy improves with use, not just with
bug-fixes:

- A bug-fix improves one code path.
- A substrate write improves *every future drain that touches that
  question*.

Three guardrails keep the loop healthy:

1. **The advisor never silently grows the substrate.** Memory writes are
   the human's act, not the headless advisor's. The advisor records its
   *decision* + *reasoning*; the human reads that and decides whether to
   promote it to a memory / criterion / doc. (Letting the headless advisor
   write memories would let the substrate drift toward whatever the
   advisor wanted it to be — a closed loop with no external grounding.)
2. **Substrate hygiene is real work.** Memories go stale, contradict each
   other, accumulate when a capability lands and the old workaround memory
   becomes wrong. `feedback_memory_pack_hygiene.md` documents the audit
   cycle.
3. **Calibration mode is the gap-detector.** When fork-from-live disagrees
   with cold-boot, the disagreement points at a substrate gap (section 3).

Trace anchor: `feedback_headless_advisor_is_cold_boot.md`,
`feedback_memory_pack_hygiene.md`, `feedback_capture_over_concentration.md`,
STORY-347.

---

## 5. Inter-agent communication infrastructure

The cascade above implies four kinds of inter-process communication:
implementer ↔ orchestrator, reviewer ↔ orchestrator, orchestrator ↔
advisor, advisor ↔ implementer (resume). All four use the **same primitive**:
**files under `.aida/`**.

### The pattern — file-based async handshake

```
┌─────────────────────────────────────────────────────────────┐
│                  .aida/  (filesystem-canonical)             │
│                                                             │
│   sessions/<id>.exit-requested      ← exit sentinel         │
│   sessions/<id>.review-verdict.json ← reviewer → orchestrator│
│   sessions/<id>.punt-request.json   ← implementer → orchestrator
│   sessions/<id>.punt-response.json  ← advisor → implementer │
│   punts.jsonl                       ← append-only ledger    │
│   punts/<punt-id>/calibration.yaml  ← calibration shadow    │
│   drain-state.json                  ← orchestrator state    │
│   findings (spec YAMLs in store)    ← human-tier surface    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
        ▲           ▲             ▲              ▲
        │ writes    │ writes      │ writes       │ reads
        │           │             │              │
   implementer  reviewer      advisor      orchestrator
    (claude -p)  (claude -p)   (claude -p)    (Rust)
```

### Why filesystem-canonical

The substrate could be a daemon (a message bus, a database, a long-running
process). It is **filesystem-canonical** on purpose:

| Property | Files | Daemon / bus |
|----------|-------|--------------|
| Latency (local) | µs | ms |
| Crash safety | filesystem persists | depends on server uptime |
| Debuggability | `ls .aida/ && cat ...` | requires a tools client |
| Single-machine ops | no daemon | one process to keep alive |
| Cross-machine reach | needs NFS or equivalent | trivial over HTTP |
| Deterministic inputs | yes (substrate on disk) | depends on bus state |

For a single-developer tool, *no daemon* is a huge operational win — no
"is the advisor up?" failure mode because there is no advisor *process*
between drains. Whatever richer architectures land later, the filesystem
substrate stays the **always-available fallback**, not a deprecated path.
The simpler thing earns its keep by always working when the fancy thing is
misbehaving.

### The MCP transport (STORY-361)

Filesystem-canonical does not mean *Claude Code only*. The AIDA MCP server
(`aida mcp-serve`) extends the substrate with **thin tools that read from
and write to the same files**, so any MCP-speaking agent (Codex, Cursor,
the Anthropic MCP inspector, …) can participate in an AIDA drain without
re-platforming the orchestrator.

> *The MCP server is a transport layer over filesystem state, not a
> replacement for it.* — `docs/architecture/mcp-coordination-surface.md`

The orchestrator stays the same Rust process owning the same files; the
new bit is that an agent reaching it over MCP-stdio can poll, append, or
record through tools, instead of issuing `cp` and `cat` directly.

### Channel-by-channel

| Channel | File | Producer | Consumer | Trace |
|---------|------|----------|----------|-------|
| Graceful exit | `<session-id>.exit-requested` | skill (touch) | orchestrator (poll, SIGTERM) | TASK-329 |
| Reviewer verdict | `<session-id>.review-verdict.json` | `/aida-review` | orchestrator (phase 3 outcome) | STORY-263 |
| Punt request | `<session-id>.punt-request.json` | `/aida-punt` | orchestrator (spawns advisor) | STORY-306 |
| Punt response | `<session-id>.punt-response.json` | `/aida-advise` | implementer (`claude --resume`) | STORY-306 |
| Punt ledger | `.aida/punts.jsonl` | append by all tiers | morning audit, calibration | STORY-332, STORY-325 |
| Drain state | `.aida/drain-state.json` | orchestrator | TUI status, `aida drain status` | STORY-301 |
| Calibration | `.aida/punts/<id>/calibration.yaml` | advisor (shadow run) | `aida findings calibration` | STORY-347 |

The discipline is consistent across every channel: **one-way writes, append
or single-write, no in-place mutation of the same file by multiple
producers, names that survive `ls`**.

### The orchestrator as mediator

The orchestrator (`aida queue work --auto-complete` in
`aida-cli/src/auto_complete.rs`) is the only process that **spawns** the
other tiers. It owns:

- the child PID (via `std::process::Command`)
- the per-channel file paths (via env vars exported to the child)
- the polling / reaping logic for graceful exit
- the reconcile-against-reality check before declaring a phase failed (BUG-241)

Skills do not know about each other. They know about *files*. That keeps the
contract narrow: a new tier (a critic, a security reviewer, a fork-judge)
can plug in by reading one file and writing another, with no orchestrator
patch needed beyond spawning it.

Trace anchor: TASK-329, STORY-263, STORY-306, STORY-361,
`docs/architecture/mcp-coordination-surface.md`, BUG-241.

---

## 6. The recipient question — who answers the advisor's prompts, with what context?

A punt from tier 1 hands the advisor a structured payload (the fork, the
options, the spec context, the code area, the stakes). The advisor renders
a verdict. **But where does the advisor's *own* judgment come from?**

This is the load-bearing open question of the architecture. There are three
candidate shapes, staged by evidence:

### v1 — Cold-boot advisor (fresh `claude -p` + rich payload) — **shipped**

A fresh Claude process per punt. Loads only the persistent substrate:
memory pack, CLAUDE.md, discipline docs, the spec graph, any plan that owns
the spec. Renders a verdict, exits.

- **Strength**: no daemon, no state to corrupt, deterministic in inputs,
  reproducible. Always available; the floor.
- **Weakness**: bounded by what is *written down*. In-flight judgment that
  lives in a running advisor session — design decisions made
  conversationally, framings still half-formed — never reaches the
  headless advisor unless externalized.
- **Status**: shipped (STORY-306, 2026-05-19). Today's default.

### v2 — Fork-from-live advisor (`claude --resume` of the live session's JSONL) — **shipped 2026-05-21**

When a live advisor session is running and registered (`aida advisor
register`), the orchestrator copies its JSONL transcript into the spec's
worktree project slug under a new UUID and `claude --resume <fork> -p
"<punt>"`. The fork boots with the live session's full conversation
context, renders a verdict one-shot, terminates.

- **Strength**: closes the gap from v1. The fork has the live session's
  in-flight judgment without a daemon.
- **Weakness**: cost ~$4 for the first fork (cache-creation tax), ~$0.03
  within the 5-minute cache TTL. Fork transcripts pile up; cleanup is a
  follow-up (TASK-443). The live session must be registered for fork to
  fire — falls back to v1 (cold-boot) when not registered or when the
  source JSONL exceeds the cost ceiling.
- **Status**: shipped (STORY-360, 2026-05-21). SPIKE-11
  (`docs/spikes/2026-05-20-spike-11-session-forking.md`) validated the
  mechanic: 6.7s latency, source isolation byte-clean, project-slug
  agnostic. Off by default; opt in via `aida advisor register`.

### v3 — Persistent advisor entity (long-lived process, MCP-bus or daemon) — **designed, not shipped**

A single advisor process per project that accumulates context across drains
and answers punts in-place, never restarting. The "richest" recipient
shape; closes the substrate-gap completely.

- **Strength**: full continuity. The advisor builds a working model that
  persists across days, hands off across drains, learns the project's texture.
- **Weakness**: real ops burden — server lifecycle, version skew, crash
  recovery, concurrency, "is the advisor up?" failure modes. Multi-project
  coordination becomes a sub-problem in its own right.
- **Status**: designed but unshipped. SPIKE-10's output is
  `docs/multi-advisor-coordination.md`; the first sub-tracks are STORY-362
  (subsystem-tagged memory pack + `--focus` loading) and STORY-364 (`aida
  advisor mentor --child <project>` for the parent-child relationship).
  Both are approved, not implemented. Persistent-entity territory is
  further out.

### Evidence-first staging

The order matters. AIDA shipped v1 first (the conservative baseline that
always works), then validated v2 via SPIKE-11 before implementing it, and
is gathering evidence on v3 via SPIKE-10 + the subsystem-scoping tracks
before committing to a daemon shape. **Each stage's evidence informs the
next.** A more sophisticated recipient is more powerful *and* more
operationally fragile; the evidence-first staging is what keeps the
architecture honest about that trade-off.

```
       evidence first ──────────────────────────────────────▶

   v1 cold-boot          v2 fork-from-live         v3 persistent
   ────────────          ─────────────────         ─────────────
   shipped 2026-05-19    shipped 2026-05-21        designed; SPIKE-10
   always available      opt-in; falls back        unshipped
   substrate-bounded     to v1                     subsystem-scoping
                                                   tracks first

   ▼ floor                  ▼ richer                 ▼ richest
   no daemon                no daemon                daemon-grade ops
```

The floor (v1) never goes away. v2 sits on top of v1 and gracefully
degrades to it. v3, if it lands, will sit on top of both — and the
filesystem substrate stays canonical underneath everything, so the
fallback path is always one missing daemon away from working.

Trace anchor: STORY-306 (v1), SPIKE-11 + STORY-360 (v2), SPIKE-10 +
`docs/multi-advisor-coordination.md` + STORY-362 + STORY-364 (v3),
`feedback_headless_advisor_is_cold_boot.md`.

---

## 7. Honest open questions

What this doc describes is not a finished design. Calling out what is
unsolved is part of the architecture — silent uncertainty is worse than
named uncertainty.

### Q1 — The recipient question is not closed

v1 and v2 are shipped; v3 is designed. But which mix is *right* over the
next year is not decided. Calibration mode (STORY-347) is the empirical
instrument: when fork-from-live (v2) disagrees with cold-boot (v1), the
disagreement is the signal that a substrate gap exists. Whether v3 ever
needs to ship depends on whether v1+v2+substrate growth closes the gap or
hits a structural ceiling.

The bet: substrate enrichment + fork-from-live is *probably* enough for
single-developer projects. Multi-developer / multi-project workflows
probably push toward v3. But that is a hypothesis, not a finding.

### Q2 — Calibration reliability

The Type A / B / C classification is **the advisor's own judgment of its
own ground**. An over-confident advisor will classify a fork as A or
recorded-B when it should have escalated; calibration mode (STORY-347)
catches some of those by running two verdicts, but the underlying question
— *how do we measure whether the advisor's classifications are honest?* —
is not fully answered. The rolling-disagreement rate is an empirical
proxy; whether it converges to "advisor classifies honestly" or to "advisor
and corpus drift together into shared blind spots" is open.

Stated mitigation: the advisor never writes memories; only the human
promotes a decision to substrate. That breaks the closed loop; whether it
is sufficient is something we'll learn from sustained use.

### Q3 — Multi-agent identity and coordination

AIDA today assumes one project = one strategic context = one advisor. As
multi-agent setups land (siblings working bounded acceptance work
autonomously, with one master advisor coordinating), the question becomes:
**which advisor sees which fork?** Subsystem-scoped memory packs
(STORY-362) are a partial answer — load only the memories that match the
active subsystem — but the orchestrator-side routing question (which
advisor process answers a punt from which subsystem) is the
to-be-implemented half.

### Q4 — Cross-project advisor handoff

When project B is initiated from project A, the strategic context built up
in project A's advisor doesn't transfer with `aida init`. SPIKE-10's
`docs/multi-advisor-coordination.md` Track B describes a `aida advisor
mentor --child <project>` verb for the ongoing relationship, but it is
unshipped and the right shape is still under discussion.

### Q5 — The "trust the substrate" gradient

Today: every advisor invocation loads the full memory pack. As packs grow
past ~100 memories, the signal-to-noise of "this memory feels off-topic
for the active spec" rises. STORY-362's `--focus` loading is the first
mechanism. Whether per-spec relevance scoring (rejected at SPIKE-10 time as
over-engineered) becomes necessary at higher scale is open.

### Q6 — The reviewer's role in the cascade

Section 2 describes implementer → advisor → human. The reviewer is named
but not deeply integrated into the cascade: a reviewer that escalates a
merge decision writes `merge: escalated-to-human` and the orchestrator
pauses for triage (BUG-241), but the reviewer does *not* punt to the
advisor tier today. Whether reviewer-to-advisor routing should exist is
designed at the edges of STORY-306 but not formalized.

---

## Where the pieces live

| Concept | Code / docs |
|---------|-------------|
| Three-mode taxonomy | `docs/autonomous-drain.md`, `docs/aida/discipline/skill-prompt-kinds.md`, STORY-287 |
| Provenance corroboration | `aida zen status`, `aida orchestrator status`, BUG-233, BUG-237 |
| Punt mechanics | `aida-cli/src/punt.rs`, `.claude/skills/aida-punt.md`, STORY-332 |
| Advisor tier | `aida-cli/src/auto_complete.rs::run_advisor`, `.claude/skills/aida-advise.md`, STORY-306 |
| Type A/B/C calibration | `.claude/skills/aida-advise.md`, STORY-347 |
| Fork-from-live | `aida-cli/src/advisor.rs::plan_fork`, `aida advisor register/status/unregister`, STORY-360, `docs/spikes/2026-05-20-spike-11-session-forking.md` |
| File-based comms | `.aida/` (gitignored), TASK-329, `docs/architecture/mcp-coordination-surface.md` |
| Multi-advisor coordination | `docs/multi-advisor-coordination.md`, SPIKE-10, STORY-362, STORY-364 |
| Reconcile-against-reality | `aida-cli/src/auto_complete.rs`, BUG-241 |
| Findings surfaces | `aida findings list`, `aida findings calibration`, STORY-278, STORY-285 |

For machinery vocabulary — orchestrator, phase, drain, lease, role, scope,
session, worktree, sentinel, batch, autonomy mode — see
`aida-core/templates/docs/aida/discipline/machinery-glossary.md`. For
spec-state verbs (Approved / Planned / In Progress / Done / Completed /
Released) see `docs/lifecycle.md`. For the practical user-facing guide to
`--auto-complete` and `--no-human`, see `docs/autonomous-drain.md`.

---

## Postscript — why this doc exists

The autonomy + escalation + comms layer is **fundamental architecture** —
anyone evaluating AIDA (human reviewing, agent operating within it) needs
to be able to read one document that names the model, the cascade, the
substrate, and the open questions, without reconstructing it from a dozen
specs and a long conversation history.

It is also the layer that determines whether autonomous AIDA is a *useful
collaborator* or a *confident-but-wrong automaton*. Naming the cascade,
the calibration, and the corpus-growth loop is what makes the choice
visible: substrate enrichment is the load-bearing primitive, not advisor
cleverness, and the architecture is built so substrate enrichment is the
durable lever.

When the architecture changes — when v3 lands, when the reviewer joins the
cascade, when subsystem-scoping changes the recipient question — this doc
gets updated to track it, and `OVERVIEW.md` keeps pointing here.
