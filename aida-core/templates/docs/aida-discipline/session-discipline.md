# Session discipline

Per-session habits that keep AIDA work honest. None of these need the AIDA
codebase to apply — they are about how an AI session reasons and acts.

## Verify before filing

When the user reports friction ("I had to do X manually", "this didn't fire
automatically", "why doesn't AIDA have Y?"), the first instinct is to file a
TASK proposing a new capability. **Pause and diagnose first.** The friction
may be timing, visibility, or state confusion — not a missing capability.

Ten-second checks beat thirty minutes of speculative design:

- `gh pr view <N> --json state,mergedAt` — is the PR actually unmerged?
- `aida show <SPEC>` — is the spec actually in the status you assume?
- `git log -1 --oneline origin/main` — has the change already landed?

A subagent's claim about "what is wrong" — especially about its own
environment — is a *hypothesis to verify*, not a fact to file on.

## Run `--help` before suggesting flags

Do not pattern-match CLI flags from mental models or analogous tools. Run
`<command> --help` before recommending a flag, or ask the user to paste it.
Guessing creates UX friction by suggesting flags that do not exist. The same
discipline generalizes: read the actual skill template before specifying a
skill's UX; run the actual diagnostic before asserting state. Verify the
artifact; don't reason from analogy.

## Pause for design input

When picking up work with real UX / design latitude (empty-state UX, copy,
layout, interaction model), pause and present the concrete design decisions
as explicit options *before* writing code. Read enough of the code first to
make the options concrete and to surface forks the spec did not anticipate.
Keep it tight: one batched question, recommend a default. Then ship the
*minimal* fix and let the bigger vision be a follow-up.

## Failed flag attempts are signals

When an agent tries a flag that does not exist and gets `unexpected
argument`, that error is diagnostic signal, not noise — the agent's mental
model of the surface diverged from reality, and the command's output did not
redirect it. Default to filing it as a discoverability finding. Ask "should
we file this?", not "does this bother you?" — the former respects that it is
already evidence.

## Refinements must be acceptance criteria

After a spec is filed, refinements arise — clearer wording, tweaked design,
corrected detail. If a refinement is captured only as a **comment** on the
spec, it is **not binding on the implementer**. The implementer's contract is
the spec's `## Acceptance` list. For a refinement to ship, it must become an
acceptance bullet — edit the acceptance list, or file a follow-up that
supersedes the original. Comments are background context; the acceptance
list is the contract.

## Trust the reviewer over intuition

The reviewer role inspects the actual diff — file paths, symbols,
architecture. Other roles often reason from commit messages and design
context. When a reviewer's verdict contradicts an intuition formed without
reading the code, the reviewer is usually right. Read the reviewer's
cited evidence before pushing back; if you push back, do the diff inspection
yourself.

## Check for in-flight work before rejecting

Before rejecting a spec or pivoting its architecture, check whether an
implementer is actively working on it (`aida session leases`, the spec's
status). Otherwise an implementer shipping in good faith on the original
spec ends up with a branch rendered obsolete behind their back. If work is
in flight, pause the rejection and coordinate first.

## Ship infrastructure fixes through the system they fix

When fixing the project's own automation (a merge hook, a status auto-bump,
a CI workflow), the merge of the fix itself often exercises the new code
path. That is the strongest possible validation — the fix tests itself in
its own end-to-end cycle. Prefer shipping such fixes through the very
plumbing they repair, and note the dogfood moment in the PR description.

## Capture is durable; analysis is a living document

Some artifacts (a competitive analysis, an architecture overview) go stale
fast. Treat them as living documents with a refresh cadence and dated
snapshots, not one-shot outputs — each refresh adds a delta rather than
re-doing the work from scratch.
