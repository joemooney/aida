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

## Verify acceptance criteria against the primary caller

Before declaring a spec's acceptance criteria done, **name the primary
caller(s)** — the feature's top 1-3 invocation paths — and verify each
criterion holds *in their environment*. A criterion is **environment-coupled**
when its truth depends on the runtime context the feature runs in:

- TTY vs piped stdout
- headless (`claude -p`) vs interactive Claude Code
- a user-typed CLI vs a skill invoked through the Bash tool (**always
  non-TTY**) vs a git/Claude Code hook vs the MCP server
- first-time vs returning user (memory + state)
- solo node vs multi-node sync
- same worktree vs cross-worktree

When an environment-coupled criterion contradicts the primary caller's
reality, the criterion is wrong even though it reads as internally
consistent. Fix it at filing time, before the implementer hits the
contradiction at the design-checkpoint pause. Walk *each* criterion against
the named caller — do not stop at "the spec is logically coherent."

Four instances surfaced this discipline (the implementer caught all four at
the design-checkpoint pause — the pushback discipline worked, but the
filing-time check would have caught them earlier):

| Spec | The criterion | The contradiction |
|---|---|---|
| TASK-260 | refinement to Path/Action/Why glyphs | lived in a comment, not acceptance — implementer correctly shipped the original glyphs |
| TASK-267 | "use the Path/Action/Why table format" | TASK-267 was sequential-steps, not parallel-choices — wrong UI shape |
| BUG-224 | leaned toward relocating a doc | rested on a flawed mental model of the scaffolding propagation channel |
| TASK-265 | "non-TTY mode degrades to a single-line summary" | the primary caller (`/aida-pickup` via the Bash tool) is **always** non-TTY, so the card would never appear in its own use case |

Before / after, using TASK-265: *before* — "non-TTY mode degrades to a
single-line summary" (named no caller; the rule kills the feature for its own
main caller). *After* — "the primary caller is the `/aida-pickup` skill, which
runs non-TTY through the Bash tool; the card must render fully there, so there
is no non-TTY degradation path." Naming the caller first turns an abstract
mode-toggle into a check the feature must pass.

This composes with the sibling rule above ("Refinements must be acceptance
criteria"): that rule says criterion fixes *belong in acceptance, not
comments*; this rule adds the upstream check — *don't file flawed criteria in
the first place*. It also reduces (without replacing) the implementer-side
"Pause for design input" pattern, which remains the last line of defense.

Origin: 2026-05-17 advisor session, captured as TASK-311 after the fourth
instance in 36 hours. Memory:
`feedback_verify_acceptance_matches_primary_caller.md`.

## Dated historical artifacts stay frozen

When refactoring across the codebase, dated historical artifacts stay
frozen at the date in their filename. Glyph swaps, vocabulary updates,
renames, and palette unifications should update *living* guidance and
code to current truth — and **leave dated records alone**. SPIKE outputs
(`docs/spikes/YYYY-MM-DD-*.md`), dated competitive-analysis snapshots,
spec comments, and git commit messages are records of *what we knew at
time T*. Rewriting them erases the path taken and makes the past look
like the present.

The discriminator when classifying a file mid-refactor:

| Artifact kind | Retroactive edit? |
|---|---|
| Living guidance (CLAUDE.md, skill templates, `docs/aida/discipline/`, README) | **YES** |
| Code (source, configs, templates that compile/scaffold) | **YES** |
| Plan files in `docs/plans/` | YES if active/load-bearing; NO once historical |
| Dated SPIKE outputs (`docs/spikes/YYYY-MM-DD-*.md`) | **NO** |
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

## Session history lives in the substrate

AIDA projects do not keep a per-session append-only log file. The durable
record is the AIDA substrate: use `aida history events` for the time
series, `aida digest` for narrative summaries, and specs, comments, PRs,
and commits for the why. Do not create or maintain an equivalent session-log
file in an AIDA-scaffolded project.

## Trust the reviewer over intuition

The reviewer role inspects the actual diff — file paths, symbols,
architecture. Other roles often reason from commit messages and design
context. When a reviewer's verdict contradicts an intuition formed without
reading the code, the reviewer is usually right. Read the reviewer's
cited evidence before pushing back; if you push back, do the diff inspection
yourself.

## Absolutes are the more fluent error

When a claim can be written in a strong form or a precise form, the
strong form usually reads better — shorter, more quotable, more
decisive. Under time pressure a writer selects for fluency without
experiencing it as a choice, so the errors that survive are not random:
they are the ones that read better than the truth. "Be more careful"
does not fix this, because the error is not what the writer failed to
notice — it is what the writer preferred.

The detectable signature is an **absolute in a statement of fact**:
"does not exist", "every", "never", "unrecoverable", "only", "all". A
status check has the same failure mode in code form — `exit=$?` read
after a pipe to `head`/`cat` reports the pipe's exit code, not the
command's, and asserts success (`exit=0`) on a failed command.

Worked instance: an advisor wrote "unrecoverable" for a lost verdict in
a finding; the reviewer read back "I checked three places I thought of"
instead. The fluent form was also the more dramatic one — it made the
finding sound more important, so the pressure ran toward significance,
not only toward brevity. It was false: the verdict turned up in a
session log twenty minutes later.

This is greppable in binding text — acceptance criteria, ADRs, review
verdicts — in a way that "check your claims" is not, but it flags
sentences worth re-reading rather than sentences that are wrong: two
correct, load-bearing absolutes ("the trailers stay", "neither PR
merges") turned up in the same sweep that found the false ones, so
expect roughly half the hits to be fine.

## Knowing what a change is for makes you worse at finding how it fails

"Trust the reviewer over intuition" says the reviewer is usually right when a
verdict contradicts an intuition formed without reading the code. This is the
mechanism underneath that rule, and it is worth understanding, because it
applies in cases the rule does not literally cover — including when the
reviewer HAS read the code and is still wrong.

A reviewer who knows what a change is **for** will check whether the code matches
that purpose. Checking for a match is a different operation from checking for a
failure, and it is the easier one — the purpose supplies a shape, and the code
either fits it or does not. Checking for a failure has no shape to work from. It
requires generating the conditions the author did not think of, and the author's
own intent is exactly what makes those conditions hard to imagine.

It follows that **you should be most suspicious where the code matches the
purpose most neatly**. A clean match is what the easier operation produces. It is
what you would see whether or not the change is correct, so it is not evidence.

This is also why a second reader who lacks the author's intent catches what the
author cannot. Not because they are more careful, and not because they are
better. They are simply doing a different operation, because they have no purpose
to check against.

Note what this does *not* say. Knowing the purpose makes you **better** at several
real checks: whether the acceptance criteria are covered, whether the change is in
scope, whether it solves the problem at all. It makes you worse at one specific
thing. The precision is the point.

### The worked example

2026-09-19, on a spec whose whole point was that a stale binary on PATH must not
be allowed to produce phantom drift. The advisor read the diff and approved it.
The orchestrator reviewer read the same diff and refused it.

The refusal was right. The guard collected only the "failed" result, while three
separate paths produced "skipped", and every one of those three let the PR
publish unchecked. The worst was the one that fires when the worktree binary
cannot be built — which is precisely the condition the spec existed to guard. So
the feature was silently absent in its own motivating scenario.

The advisor's account of the miss is the useful part:

> I had formed a view of what the PR was for and checked whether the code matched
> the view, rather than whether it could fail.

Nothing was rushed and nothing was skipped. The diff was read. The reading was
simply the wrong operation, and holding the purpose is what selected it.

### How to apply

- **Intent is acquired by READING, not only by writing.** Reading the spec before
  the diff is enough to install the purpose you will then check against — which is
  nearly every review anyone does. Writing the brief or the spec makes it
  stronger, and typing the code makes it strongest, but the weakest form is the
  one you will be in most often, and it is the one nobody guards against.
- **Never review your own implementation**, and treat writing the rework brief or
  the spec as enough involvement to weaken your reading.
- **When you must read something you shaped**, change the operation deliberately.
  Do not ask "does this do what it is supposed to do." Ask "what input makes this
  wrong", "what happens when this call fails rather than returns", "which branch
  has no test". Enumerate the ways out of a function and check each one, rather
  than following the path the purpose suggests.
- **Treat convergence between two independent readers as the real signal.** Two
  readers who did not see each other's findings arriving at the same list is
  strong evidence the sweep is complete. The same list from a reader who saw the
  first one is an echo and evidence of nothing.
- **A review that finds nothing is a result that needs explaining**, not a result.
  Say what you looked for and did not find, so the next reader knows what is still
  uncovered.

### Why this is not just about code review

The same mechanism governs verifying a claim, confirming a fix, and checking a
report from another agent. Whenever you already know what the answer is supposed
to be, you will check for the match.

Changing the reader is the primary defence and it is why the independence rule
exists. But it is not always available: sometimes you are the only reader, and
sometimes you are the second reader who has already been told what the change is
for. When you cannot change the reader, change the operation. See
`session-discipline.md`, "Confirm a claim by a different method than produced it".

## Confirm a claim by a different method than produced it

Independence between seats is a property of *method*, not a posture of
distrust. A second seat that re-reads the first seat's evidence the way the
first seat gathered it adds nothing; a second seat that derives the same
claim a different way is the only kind of check that can fail usefully.

A worked example, because the shape matters more than the moral. A reviewer
flagged that a round detector matched a marker anywhere in a comment where
the canonical rule matches it only at the start. Correct. To make the defect
concrete, the seat writing the rework brief grepped the store for comments
where the marker appeared mid-text, found what looked like three, and named
them as the negative-test fixture. A second seat checked before the brief was
acted on — by parsing each comment's stored content rather than grepping raw
file lines — and found the marker at position zero in all three. They were
positive cases. A test built on them would have broken the detector for every
real rework round.

The first seat then repeated the second seat's method rather than deferring
to its conclusion, and recorded the marker's *position* rather than its
presence. Seventy occurrences sat at position zero. Exactly one did not: the
second seat's own correction, where the marker appeared mid-sentence inside
the argument that mid-sentence occurrences do not exist. The defect was real,
the first evidence was wrong, the second was right but incomplete, and the
only true fixture in the store was created by the act of arguing it could not
exist.

Neither seat being careful produced that answer. Two derivations disagreeing
did.

In practice:

- When confirming or disputing a claim, change the method, not just the
  reader. Searching text and parsing structure are different methods. Two
  greps are one method run twice.
- Prefer a check that measures rather than one that detects. "Does this
  string appear?" and "where does it appear?" are not the same question, and
  only the second can be wrong in an informative way.
- If two methods agree, the claim is strong enough to build on. If they
  disagree, neither is trusted until a third settles it — and say so plainly
  instead of picking the more senior seat's answer.
- Evidence gathered to support a conclusion deserves more scepticism than
  evidence that arrived uninvited. The brief above was wrong precisely
  because it went looking for confirmation.

## Verify against the artifact that ships, not the step's own report

A step that performs work also reports on it, and the report is generated by
the thing you are checking. When a step is wrong in a way that does not
crash, its report is wrong in exactly the same way — so a report is never
independent evidence about its own step. Check the artifact that survives the
step instead: the commit, not the worktree; the pushed head, not the push's
output; the stored record, not the command's confirmation.

A worked example. Resolving a rebase conflict in a branch, a seat ran a
script that rewrote the conflicted file, then a second script that reordered
some rows in it. Both printed success. The row order printed afterwards was
correct. The commit contained three orphaned conflict markers.

What happened: an intervening `git checkout --merge` had re-created the
conflict with `ours` / `theirs` labels rather than the original
`HEAD` / `<sha>` ones. The resolution script's pattern was keyed to the
original labels, matched nothing, and did nothing — and its "resolved"
message was printed unconditionally. The reorder step then moved the table
rows out of a conflict block that was still open, leaving the markers behind
and the rows in the right order. Every surface agreed; only the commit
disagreed.

The check that caught it was one command:

    git show HEAD:<path> | grep -c '<<<<<<<\|^=======$\|>>>>>>>'

The script that failed to resolve the conflict is the same script that
announced the resolution. Its self-report is not a second opinion — it is the
first opinion restated. That is the general shape: whenever the instrument
and its report are the same object, the report adds no information about
whether the instrument worked.

| You want to know | Do not trust | Check |
|---|---|---|
| Did the edit land? | the tool's success message, or the worktree | `git show HEAD:<path>` |
| Did this round do work? | that the head moved | `git patch-id` across rounds |
| Did the write take? | the command's confirmation line | the stored record on disk |
| Did the check verify anything? | a green job | that the job's work actually ran |

The last row is the same rule pointed at CI. A job that skips its real work
and exits 0 reports identically to one that ran and passed; the skip is
usually a notice annotation, invisible unless someone opens the run. "Did not
fail" and "verified" are different claims, and only one of them is evidence.

Spend the check after any step whose failure mode is silent — not after every
command. Concretely: anything involving conflict markers, anything where a
pattern match decides whether work happens, anything that can legitimately do
nothing, and anything whose output you are about to quote to someone else as
evidence. If you are going to write "verified" in a comment, verify the
artifact.

This is the single-step case of the preceding section. There, two seats guard
each other by deriving a claim different ways; here there is only one step,
and the artifact it leaves behind is the only thing that can disagree with
it.

## Check for in-flight work before rejecting

Before rejecting a spec or pivoting its architecture, check whether an
implementer is actively working on it (`aida session leases`, the spec's
status). Otherwise an implementer shipping in good faith on the original
spec ends up with a branch rendered obsolete behind their back. If work is
in flight, pause the rejection and coordinate first.

## Re-read live state before acting on remembered IDs

Conversation context can hold an identifier longer than the substrate holds
the thing it names. Before acting on a remembered lease, brief, or queue item,
re-read the current lease / brief / queue state and let that fresh state
govern the next command. Treat cleanup verbs that return "not found" as
idempotent success when the current state says there is no work left to clean
up, not as evidence of a new bug.

Concrete examples:

- `release_task` returning `not-found` for a lease id from earlier context
  usually means another session or cleanup path already released it. Re-run the
  live lease listing, confirm the lease is absent, and move on.
- Brief cleanup returning `no-briefs-found` after a handoff or pickup means
  the brief queue is already empty for that agent. Re-read the brief list and
  treat the absent brief as a no-work outcome.

This is the same discipline as "verify before filing," applied to cleanup:
fresh substrate state beats stale conversational memory. trace:TASK-1203 |
ai:codex

## Prefer fresh sessions once the substrate is clean

When context is near compaction, do not assume compaction is the safest move.
In an AIDA project, the durable substrate should carry the state: specs,
comments, briefs, queue membership, leases, findings, PR links, and trace
comments. If conversation-only residue has been captured and no live
session-bound process needs continuity, start a fresh session. The new
session can rebuild from `aida status`, `aida queue next`, pending briefs,
leases, and the handoff note.

Use `/aida-handoff` or `aida session handoff --check` at the boundary.
The CLI probe can identify live drain and current-session lease pins; the
skill must also capture residue the CLI cannot know, such as undocumented
decisions, promised follow-ups, and live watchers started only in this
conversation.

Decision rule:

- **START FRESH** when capture is complete and there is no live drain,
  current-session lease, watcher, monitor, background process, or other
  session-bound state.
- **COMPACT** when live session-bound state must survive. Name every pin
  explicitly so the next turn knows what is being preserved.

Context size alone is not a compact pin. AIDA's job is to make fresh-session
restart lossless once the substrate boundary is clean.

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
