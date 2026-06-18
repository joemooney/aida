# North star: AIDA as a streamlined, niche-fit, multi-vendor-relevant tool

- **Date:** 2026-06-18
- **Owner:** the advisor (autonomous long-loop mandate, 2026-06-18). This is the compass the loop drives toward; it is living — revise as the probe teaches.
- **Frame:** EPIC-48 (AIDA is a probe; deliverable is knowledge + an honest verdict). This doc is the *product* half: even if the verdict is "don't build this," AIDA should be the **best, most honest version of itself** so the verdict is earned, not assumed.

## The niche (defensible, multi-vendor age)

AIDA is **the shared intent + coordination substrate for a multi-vendor agent fleet** — the index of *why* (specs, typed relationship graph, **code-to-spec traces**, enforced lifecycle) that *any* vendor's agent reads and writes, plus the coordination layer (leases, queue, mailbox, RBAC, the dashboard) that keeps a heterogeneous fleet from colliding. It is **neutral by construction** — the one thing no single vendor is incentivized to build (P8a). The uncontested wedge, verified repeatedly: **code-to-spec traces + an enforced approval lifecycle + cross-vendor neutrality.** Everything else is commodity or table-stakes.

## What "best / streamlined" means (the bar)

1. **The core loop is obvious + frictionless:** spec (quality-gated) → agent work (any vendor) → trace → review → merge → auto-complete. A new user should *feel* that loop in their first session.
2. **The surface is pared to the loop.** I have accreted heavily (40+ skills, dozens of commands). Streamlined = subtraction: cut/hide/merge low-value surface, let the daily drivers shine. **Surface complexity is the anti-pattern; quiet depth is the goal.**
3. **The niche is legible in 30 seconds** — README, onboarding, and the TUI make "this is the cross-vendor intent + coordination substrate" self-evident, not discovered after weeks.
4. **It is reliable** — the multi-clone harness stays green, the autonomy keystone is solid, papercuts die.
5. **The probe reaches honest conclusions** — EPIC-48's verdict is sharpened by *run experiments*, not argument.

## Workstreams the loop drives (advisor picks the highest-leverage next each iteration)

- **W1 — Research → conclusions.** Run the queued experiments (open-brief bake-off, gate-vs-rule ablation, 3-vendor run), fold results into the theory paper, sharpen §12's verdict. Each iteration: a finding.
- **W2 — Streamline the surface.** Usage-grounded cut/hide/merge of low-value commands + skills; collapse near-duplicates; demote rarely-used surface behind `--advanced`/help-only. Measured by: fewer top-level commands, the daily-drivers obvious.
- **W3 — Sharpen the niche.** Make the cross-vendor wedge excellent: the spec-quality loop (dryrun → interview), competitive-eval as a first-class capability, cross-vendor briefs/mailbox, the trace loop end-to-end. The things only AIDA does, done well.
- **W4 — Reliability / polish.** Clear bugs, keep CI + the harness green, kill papercuts, keep the combined-main honest.
- **W5 — Legibility / onboarding.** First-run experience, README/OVERVIEW (probe-framed now), the TUI's quiet-depth reveal, the discipline pack.

## Operating rules for the loop (advisor)

- **Subtraction counts as progress.** Cutting a low-value command is as valuable as shipping a feature — more so, for "streamlined."
- **Every change ladders to the niche or the probe.** If it doesn't sharpen the wedge, harden reliability, or produce a finding, don't build it.
- **Ground in data, not vibes.** Use `aida usage` for surface decisions; use experiments for research claims; verify against code, not lore.
- **Honest verdict over advocacy.** If a slice of AIDA is dead weight or a dead end, say so and cut it — that's a finding too.
- **Harness + CI green is the floor.** No merge that reds the gate.

## Definition of done for the exercise

The advisor declares AIDA "the best I can make it" when: the core loop is frictionless and legible; the surface is pared (no obvious dead weight); the niche features are excellent; reliability is boring; and EPIC-48 carries an evidence-backed verdict. Until then, the loop keeps going — one highest-leverage move per iteration.

<!-- trace:EPIC-48 | ai:claude -->
