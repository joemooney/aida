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

## Dated historical artifacts stay frozen

When refactoring across the codebase, dated historical artifacts stay
frozen at the date in their filename. Glyph swaps, vocabulary updates,
renames, and palette unifications should update *living* guidance and
code to current truth — and **leave dated records alone**. SPIKE outputs
(`docs/spikes/YYYY-MM-DD-*.md`), session logs (`PROMPT_HISTORY.md`
entries), dated competitive-analysis snapshots, spec comments, and git
commit messages are records of *what we knew at time T*. Rewriting them
erases the path taken and makes the past look like the present.

The discriminator when classifying a file mid-refactor:

| Artifact kind | Retroactive edit? |
|---|---|
| Living guidance (CLAUDE.md, skill templates, `docs/aida/discipline/`, README) | **YES** |
| Code (source, configs, templates that compile/scaffold) | **YES** |
| Plan files in `docs/plans/` | YES if active/load-bearing; NO once historical |
| Dated SPIKE outputs (`docs/spikes/YYYY-MM-DD-*.md`) | **NO** |
| Dated session logs (`PROMPT_HISTORY.md` entries) | **NO** |
| Dated competitive-analysis snapshots | **NO** |
| Spec descriptions / acceptance bullets | YES if work hasn't started; else file a follow-up |
| Spec comments | **NO** |
| Git commit messages | **NO** |

Worked example: BUG-116 (2026-05-17) propagated the `▶ ⏵ 🚪` → `▶ ⇒ ⏸`
glyph swap across skill templates. The implementer correctly left
`docs/spikes/2026-05-16-claude-headless.md` untouched, noting it as a
*"dated historical observation record, not living guidance."* That
phrasing is the rule.

A lint check is unnecessary; the filename-date convention plus this
discipline is sufficient.

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

## Finish-state communication rubric

Any time a skill *ends a phase* — the implementer wrapping up phase 1, the
reviewer writing the verdict, `/aida-pr` after opening the PR — the closing
output is finish-state communication, and it must be self-contained. The
reader (a human at the keyboard, the next session's headless advisor, or a
log-trawler tomorrow) should not have to infer current state or guess what
should happen next. Two surfaces share the same rubric: the structured
**"how should I finish?" menu** when there are real choices to pick, and
the **closing summary block** an autonomous-drive session emits when it
finishes a phase. Both surfaces must carry all six elements below.

1. **State snapshot.** A labelled section naming the load-bearing facts:
   commits, push status, PR status / URL, drain phase, test + fmt status,
   plan file. The reader should not have to run `git status` or
   `gh pr view` to know where things stand.
2. **The deciding factor.** Any load-bearing risk that frames the choice —
   a smoke-test gate, a plan deviation, an unusually large change, a
   subprocess plumbing that is mock-tested only — surfaced *next to* the
   options (or the recommendation), not buried in the upstream preamble
   where the choice can't see it.
3. **A recommendation with rationale.** Not a flat neutral menu. The
   skill has the analysis; lead with *"I recommend X because Y"* and mark
   the row `← recommended`. For a closing summary, this is the explicit
   *"→ Next: <action>"* line that names the user-action.
4. **Per-option downstream consequence + reversibility.** For a menu, each
   option's row states what the orchestrator / drain does next (*"advances
   to phase 4 → merge"*, *"halts at phase 3 with the recovery hint"*) and
   how reversible it is. For a closing summary, this is one line stating
   what happens after the user exits.
5. **An explicit `advise` escape.** Treat *route this to the advisor* as
   a first-class option — not a fallback "type something." Today the
   advisor is reached via the user relaying; once STORY-306's advisor
   tier ships the orchestrator routes punted forks automatically.
6. **Decouple coupled decisions.** Push/PR is one decision, followup-filing
   is another, merge timing a third. Bundling them locks them; ask in
   sequence — the second prompt only fires after the first resolves.

The corollary on the closing-summary side: *silence is not an acceptable
signal.* If the session auto-exits via a sentinel (`$AIDA_EXIT_SENTINEL`
under `$AIDA_ZEN` or `--no-human=both`), the summary must say so
explicitly (*"→ session will auto-exit; nothing else needed"*) rather
than leave the user wondering whether to press a key. If a key is
required, name it (*"→ Press Ctrl+D to advance the orchestrator"*) — never
end on a vague *"Session is done."* and assume the user infers the rest.

Worked examples that follow this rubric:
- The reviewer's loud exit block in `aida-review.md` step 7 (TASK-291 /
  BUG-226) — written *before* this rubric was named, but already embodies
  it: labelled verdict line, explicit drain phases that follow, named key
  to press, sentinel touch under zen.
- The implementer's orchestrator-mode templates in `aida-pickup.md` step 6
  and `aida-pr.md` (TASK-359) — the rubric's first deliberate application
  to the implementer + PR surfaces.

Origin: 2026-05-19, captured as TASK-359 and the discipline memory
`feedback_finish_checkpoint_clarity.md`. The user saw the same gap on two
different surfaces in one day — a menu with no recommendation or advise
escape (STORY-306 finish), and a closing summary that listed what shipped
but never named the next user-action (BUG-245 finish). Two surfaces, same
rubric, same fix.
