# I bet my agent system on AI agents ignoring the rules. They didn't.

*A negative result on rule-vs-gate enforcement for coding agents — and why the field may be over-buying enforcement machinery. Hypothesis-grade (n=1); the point is that it's cheap to check for yourself.*

---

When I started building a coordination layer for fleets of AI coding agents, I took one thing as self-evident: **you cannot govern a capable agent with a written rule.**

A line in a project-instructions file that says *"every commit message must follow this format"* is a sign on the wall — *"Employees must wash hands."* Agents, like people, ignore signs. So I built turnstiles: programmatic gates that mechanically reject the malformed commit, block the merge, refuse the out-of-format action. The whole architecture rested on the belief that enforcement had to be *structural*, because prose couldn't be trusted.

Then I tried to prove it, and I couldn't.

## The setup

I pre-registered five hypotheses about *when* agents drop written rules — short tasks vs. long, simple vs. complex, one vendor vs. another. Then I built a harness to test them, with two design choices that matter:

1. **Deterministic grading.** Compliance was checked by a script, not by an LLM judging another LLM. The moment you let a model grade a model, you've reintroduced exactly the softness the experiment is trying to squeeze out. Every result here is a string match, not a vibe.
2. **A real gate arm.** Each policy ran two ways — (a) stated as a written rule the agent reads, and (b) enforced by a gate that blocks violations. The gate arm existed for one purpose: to catch whatever the rule arm let slip through.

Two vendors' current models (Anthropic's Claude, OpenAI's Codex), tasks ranging from trivial to genuinely complex, ten runs per cell.

## The result

Across all five cells, the rule arm hit **100% compliance**. The gates never fired — not once. There was nothing for them to catch. Every hypothesis about rule-dropping failed to reproduce.

It wasn't only the lab — with one caveat I have to make against my own data. In my repository's commit history, commits from *orchestrated agent runs* carried a stated format rule far more reliably (~2% miss, n=85 drained commits) than *interactive* sessions (~17%). But I discount that gap myself: the orchestrated path formats that trailer *by construction* — it commits through a helper that adds it — so part of the difference is mechanical, not behavioral, and the attribution is coarse (spec-level, not per-commit). The controlled experiments, not this field number, are the spine. I include it only because it points the same way, not because it proves anything alone.

I call this **governance inversion**. The founding intuition — *agents need turnstiles because they ignore signs* — inverted under measurement. Current models honor the sign at ceiling. And the implication has a price tag: a lot of teams are budgeting for hard enforcement machinery around well-scoped agent tasks that the evidence says the models will simply comply with unprompted.

## The caveats, which are the actual finding

If I stopped there this would be a vendor whitepaper. Here is where it earns the word "honest."

**This is n=1.** One system, essentially one operator, ten runs a cell, single-task pilots, and field data drawn from one unusually disciplined repo. It is hypothesis-grade, not conclusion-grade. I am not telling you to rip out your gates. I am telling you the *assumption underneath them* may be wrong, and that it is remarkably cheap to check.

**And the rules broke exactly once.** In the whole program, an agent dropped a stated rule a single time — on a large, messy, real codebase, under long unattended autonomy: hours of hands-off queue-draining with no human watching. That is precisely the regime the clean experiments *couldn't* reach. As tasks stretch, context crowds, and supervision disappears, the ceiling may crack.

So the finding is not "gates are useless." It's that **gates are probably miscategorized** — they are insurance for the long-autonomy, large-blast-radius tail, not a tax to levy on every well-scoped task. The single failure is a signpost pointing at exactly where structural enforcement still earns its keep.

## What I'd do with this

- **Don't pay the gate tax where it buys nothing.** For bounded, well-specified agent tasks, a clear written rule appears to be enough on 2026 models. Reserve structural enforcement for the long-autonomy, high-blast-radius regime — the place the one real failure actually occurred.
- **Run the cheap replication before you trust me.** This is two afternoons, not a research program. Take one policy you currently enforce in code, *also* state it as a plain written rule, and measure compliance across your real pipeline — rule-on vs. gate-on. If your agents comply at ceiling too, you've just found budget. If they don't, you've learned your regime is the hard one, which is worth knowing.
- **Grade deterministically, or don't bother.** The instant an LLM judges compliance, your result is noise wearing a lab coat.

## Why I'm publishing a result that killed my own thesis

The founding premise of the tool I spent a year building died in these five ablations. I'm reporting that as the headline, not burying it in an appendix, because negative results are chronically underpublished in agent engineering — and the reason is structural. Most of us writing about this are building or selling something a result like this would undercut. I have no such incentive: the tool was always a probe, and a probe that falsifies its own hypothesis has done its job.

Two things follow. First, the experiment is open and deterministic — the runner scripts are in the repo and grading is a string match, not a model's opinion. They're fork-and-adapt reference implementations rather than a turnkey harness (each cell hardcodes its own task, rule, and grader), but the design is simple enough to rebuild in an afternoon, and I'd much rather the community stand it up against their own pipelines and pressure-test governance inversion than take one builder's word for it. Second, the finding is a fork, not a dead end: if it holds at scale, the enforcement-machinery line item is where the savings are; if it breaks in the long-autonomy regime — my bet for where it breaks — then we've localized precisely where gates are worth building, and that's worth just as much as the savings.

The expensive mistake the field is currently making is treating "agents ignore rules" as a settled premise and spending accordingly. It isn't settled. On current models, for the tasks most teams are actually automating, the agents are reading the sign — and washing their hands.

---

*The full design-science study — the five pre-registered hypotheses, the harness, the field data, and the ~dozen other assumptions this project falsified about coordinating multi-vendor agent fleets — is [here]. Corrections and replications welcome; that's the point.*
