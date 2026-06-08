# Drain the Merge / Integration Queue

Land finished work — specs that are Done with an open PR — on the default branch,
one PR at a time, in dependency order. The integrator seat: rebase stale
branches, watch CI, squash-merge the green-and-reviewed ones, pull to auto-bump,
and escalate (never resolve) anything that needs a judgment call.

## Instructions

Follow the workflow in `.claude/skills/aida-integrate.md`:

1. Resolve the integration set + conflict forecast first:
   `aida queue integrate --dry-run`. Cross-check dependency order with
   `aida graph <SPEC> --blocked-by` and `gh pr list --state open`; land
   independent PRs before their dependents.
2. Per PR, in order: read state (`gh pr view <n>`) → gate on the reviewer
   verdict → gate on CI (re-trigger flaky, brief the implementer on a real
   failure) → rebase if behind (mechanical conflicts only) → squash-merge once
   green AND approved → delete branch → `aida pull` (auto-bump) → re-trigger
   stale CI on the others. The serial loop is `aida queue integrate`
   (`--watch` / `--rebase` / `--max <N>` / `--strategy per-item`).
3. Escalate, do NOT resolve: semantic conflict → advisor; missing verdict →
   reviewer (`aida queue add --for reviewer <SPEC>`); real test failure →
   original implementer (`aida brief <implementer> <SPEC>`); any design call →
   advisor. Punt-and-continue: one blocked PR is escalated and skipped, it never
   halts the cascade.
4. Per-agent dispatch: route one integration to a named agent with
   `aida brief <agent> <SPEC>` (role routing via `aida queue add --for
   integrator` also works).
5. Report (`PushNotification`): PRs merged, what was escalated and to whom, and
   what's still waiting.

Guardrails: CI + a reviewer verdict gate every merge; never merge into red or
without a verdict; never resolve a semantic conflict silently. The producer half
is `/aida-burndown`; this is the single serial consumer that lands its PRs.

ARGUMENTS: $ARGUMENTS
