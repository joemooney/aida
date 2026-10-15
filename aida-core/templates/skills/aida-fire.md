---
name: aida-fire
description: Fire mode — under an operator mandate, the advisor fans out parallel fixers and independent reviewers, gates each PR by risk tier, and ships on green, one at a time.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
  - Agent
  - SendMessage
  - TaskStop
  - PushNotification
---

# AIDA Fire Skill

<!-- trace:STORY-1692 | ai:claude -->

## Purpose

Fire mode is the advisor-driven parallel burndown: the advisor surveys open
work, fans out **fixers** (one per spec, each in its own worktree) and
**reviewers** (separate agents that never review their own work), gates each
PR by risk, and ships green PRs itself. It runs on an **operator mandate**,
which is a time-boxed grant of merge authority. It never replaces review.

The authority model, risk tiers, two-round cap and hold-clear conditions are
in `.aida/discipline/fire-mode.md`. Read that first. This skill is the
procedure.

## When to use

- The operator says "fire mode", "burn it all down, you merge", or a seat
  handoff says a fire-mode mandate is still in force.
- Many independent PRs and specs are open, and review or merge latency (not
  implementation) is the bottleneck.

## Skip if

- No mandate is in force (see step 1). Use `/aida-burndown` for a hands-off
  drain of the blessed queue, or `/aida-orchestrate` for full-backlog
  proxy work with batched integration.
- One spec is in play. Use `/aida-pickup`.
- The work is an architecture change. It needs a sketch and advisor signoff
  first, not a fan-out.
- The session is headless. Background CI watchers (step 6) need an
  interactive harness. Use `/aida-burndown`, whose launcher waits on CI for you.

## Procedure

### 1. Entry: confirm the mandate, or refuse

Fire mode does not start on inference. Before any dispatch, find the mandate
in one of these places:

1. The operator said so **in this session**.
2. The seat handoff you resumed from records it, with an end time that has
   not passed.
3. A mandate note on the mailbox or on a spec comment that you can quote.

The mandate must name its **scope** (which seats, queue, tags or PRs), its
**authority** (the advisor merges on green, whoever authored the PR) and its
**end time**. If any of the three is missing, or the end time has passed,
**refuse to start**. Print:

```text
Fire mode needs an operator mandate: scope, merge authority, end time.
Ask the operator to state it in this session, e.g.
  "Fire mode until 18:00: open PRs + the approved queue; you merge on green."
```

Once confirmed, record it so the next seat can find it:

```bash
aida mailbox send --to advisor --subject "fire-mode mandate" \
  "Mandate (operator, <date time>): scope=<...>; authority=merge on green; ends=<time>. Quote: \"<operator words>\""
```

The mandate never waives the floor in `fire-mode.md`: security, sandbox,
authority and architecture review; a green Build; no weakened tests; no
`--no-verify`; merge-hold clears by the operator.

### 2. Survey, then de-duplicate

```bash
aida awaiting                 # everything gated on the advisor
aida pr list                  # open PRs, spec, CI and rebase state
aida queue list               # blessed work
aida list --status needs-attention
aida merge-hold list          # holds; each one routes to the operator
```

Before dispatching **anything**, check each candidate for existing work.
Fanning out a spec that already has a branch, PR or claim duplicates effort
and races at merge time:

```bash
gh pr list --state all --search "<SPEC-ID>" --json number,state,headRefName
git branch -r | grep -i "<spec-id>"
aida session leases           # spec-scoped claims and leases
aida status <SPEC-ID>
```

Drop anything already in flight. Sort the rest into **ship now** (approved
at head, green), **review needed**, **rework needed** and **fix needed**.

### 3. Fan out, in one message, within the cap

Send all spawns for a wave in **one** message so they run concurrently. Keep
the wave to **about 10 agents in total** (fixers plus reviewers). More than
that costs more coordination than it saves.

- **Fixers:** one per spec, or a tight pair that shares files. Never two
  fixers on files that overlap. Serialize those.
- **Reviewers:** a separate agent per PR, never the fixer. Pick the tier
  by risk (see `fire-mode.md`): FASTTRACK for docs/test-only, STANDARD for
  ordinary code, STRICT for authority, gates, sandbox, containment and seats.

Each fixer claims its spec (`aida claim <SPEC-ID>`) so a later survey sees it.

#### Fixer prompt template

```text
You are a fixer in <repo path>. First read CLAUDE.md and
.aida/discipline/README.md. Export AIDA_AGENT_TYPE=<tool> AIDA_AGENT_NAME=fixer-<id>.
Rules:
- Work in an isolated sibling worktree, never the main checkout: create it
  with `aida worktree add <SPEC-ID>` (off origin/main; prints the path).
- Claim the spec first (`aida claim <SPEC-ID>`). If it is already claimed or
  has a branch or PR, STOP and report.
- Commit format: `[AI:<tool>] type(scope): summary (<SPEC-ID>)` + co-author
  trailer. Never use --no-verify.
- Template masters only; never edit generated mirrors.
- Testing tier for YOUR work (the full workspace suite runs only in CI; never
  run `cargo test --workspace` locally). <Pick one:>
  FASTTRACK (docs, templates, one line): `make check-ci-fast` only, no cargo.
  STANDARD: `make test-touched` (targeted `cargo test --profile agent -p
  <crate> <filter>` for the touched modules) plus `make check-ci-compile`.
  STRICT or heft >= 7: STANDARD plus the area's integration test binaries
  (`cargo test --profile agent -p <crate> --test <name>`).
  Always `make check-ci-fast` before pushing, and `make check-ci-compile`
  after ANY merge from main: the fast gate does not compile tests.
- Open a PR. Do NOT merge.
- STOP clause: if the fix needs an architecture change, a new authority path,
  or a design choice the spec does not settle, do not implement it. Report the
  design fork and the options instead.
Task: <SPEC-ID>. Read `aida show <SPEC-ID> --comments` in full.
<Two to five lines of specific direction: files, root cause, acceptance.>
Report the PR number, head SHA, files changed and check results in under 150 words.
```

#### Reviewer prompt template: STANDARD

```text
You are an independent reviewer for PR #<N> (<SPEC-ID>) in <repo path>.
You did not write it. Read-only: do not edit, commit, push or record verdicts.
Read `aida show <SPEC-ID> --comments`, then `gh pr diff <N>` at head <SHA>.
Check: does it meet every acceptance criterion; is it correct on the edge
cases; are there tests that would fail without the change; trace comments
and commit format; no generated mirror edited by hand. Rely on GitHub CI,
do not run the local test suite.
Return: VERDICT: APPROVE | CHANGES. For each finding: file:line, severity
(blocking / non-blocking), and a concrete failure scenario. Under 250 words.
Non-blocking findings are listed separately; they become follow-ups, not rework.
```

#### Reviewer prompt template: STRICT (authority, gates, sandbox, seats)

```text
<STANDARD template>, plus:
- Enumerate every path that grants, checks or bypasses authority in the diff.
  For each, name who can reach it and what stops a seat without the grant.
- Look for fail-open: an error, a missing file, an env var, or a timeout that
  turns a refusal into an allow.
- Check that no existing test was weakened, deleted or marked ignored to pass.
- Name the test that proves the refusal path, not only the happy path.
Under 400 words. Default to CHANGES when the refusal path is untested.
```

#### Functional verification: security, sandbox and containment PRs

A security APPROVE reads the code. It does not prove the feature works. A
containment feature can pass review and still refuse every legitimate write.
For any sandbox, containment or permission-boundary PR, add a second agent
before shipping or clearing its hold:

```text
Functionally verify PR #<N> at head <SHA> in an isolated worktree. Build it
and run the REAL end-to-end path, not unit tests: launch the contained agent
(e.g. the real-agent opt-in test such as AIDA_TEST_REAL_CLAUDE=1), perform a
write that MUST succeed and one that MUST be refused, and paste both outputs.
Report WORKS / BROKEN with the evidence, under 200 words. Do not edit code.
```

### 4. Gate each PR

**APPROVE:** ship it with the FAQ-57 recipe, at the exact head the reviewer saw:

```bash
read H B < <(gh pr view N --json headRefOid,headRefName --jq '"\(.headRefOid) \(.headRefName)"')
aida review record PR-N --pr N --sha "$H" --branch "$B" --verdict approved --summary "<TIER>: <why>"
gh pr ready N
aida pr ship N    # --override-stale-check ONLY when the ci.yml delta on main is unrelated
```

If the head moved after review (rework push, merge from main), record again
at the new head. The approval binds to a SHA.

**CHANGES:** resume the **same** fixer (`SendMessage` to its agent id) with
a rework brief per `.aida/discipline/rework-brief-craft.md`: each blocking
finding, its file:line, the failure scenario, and what "fixed" looks like. If
the PR belongs to another seat, mail the owning seat instead.

**Two-round cap.** After the second review round, rule. Either merge and file
the remaining findings as follow-ups, or park the PR with a reason on the spec.
There is no round three. Non-blocking findings are **always** filed as
follow-up specs (`aida add ... --status draft`), never as another round.

**Merge holds:** the advisor does not clear them. When the release condition
is met (approved at head, green Build, current with main, and functionally
verified where step 3 requires it), ask the operator:

```text
PR #N's hold is ready to release: approved at <SHA>, Build green, up to date.
Please run: ! aida merge-hold clear N
```

### 5. Ship serially after any merge wave

Two PRs that are each green can still break main together: one adds a required
field, the other adds a literal without it. Neither run saw the other.
`--override-stale-check` does not protect against this. Therefore:

1. Ship **one** PR.
2. For the next PR: update it from main (`gh pr update-branch N` or a merge
   from main), let a **fresh** Build run on the new head, re-record the
   approval at that head, then ship.
3. Repeat. Never ship back to back on greens computed against an older main.

### 6. Wait without polling

For each PR in flight, start **one background watcher** that exits on a
terminal state, then act on its notification. Do not loop `gh pr checks` in
the foreground:

```bash
# run_in_background: true
gh pr checks N --watch --fail-fast >/dev/null 2>&1; gh pr view N --json number,mergeStateStatus,statusCheckRollup --jq '{n:.number,state:.mergeStateStatus}'
```

After every merge wave, also watch **main**:

```bash
# run_in_background: true
gh run watch "$(gh run list --branch main --workflow CI --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```

After each merge, expect conflicts or compile drift in the remaining PRs.
Update the branch, or resume the fixer.

**Main-red stop rule.** If main's Build goes red, **stop shipping**. Every open
PR's green is now meaningless. Diagnose the break (usually the fixer that hit
it already knows), fix it forward with one small FASTTRACK PR through the
normal gate (no `--no-verify`, even for one line), wait for main to go green,
then resume serial shipping from step 5.

### 7. Respect spec-level gates

The mandate does not override a gate written on the spec. The two kinds of
gate are handled differently:

- **"Operator merges"** (or a supervised hold): this gate **always** goes to
  the operator. Ask. Ship only after the operator explicitly authorizes that
  merge in this session, then record the authorization on the spec before
  shipping. Your own comment never satisfies this gate.

  ```bash
  aida comment add <SPEC-ID> "Operator authorized merge of #N in-session (<time>): \"<operator words>\"."
  ```

- **"Needs advisor signoff"** (e.g. on a sketch): only this kind of gate
  may be recorded by the advisor. Record the signoff and its reasoning on the
  spec **before** shipping:

  ```bash
  aida comment add <SPEC-ID> "Advisor signoff (fire mode): <decision> because <reasoning>."
  ```

Architecture changes still need a sketch and signoff. A fixer that hits its
STOP clause hands you a design fork. Take it to the operator, do not rule
alone.

### 8. Capture

- Rulings and signoffs: spec comments (`aida comment add`).
- Follow-ups and non-blocking findings: draft specs (`aida add ... --status draft`).
- Process lessons, e.g. a new failure mode: a comment on the relevant
  discipline spec, or `aida findings add`.
- At the end: run `/aida-capture` over the session.

### 9. Exit and hand off

Stop when the mandate ends, the scope is drained, or main is red and you
cannot fix it forward. Release your claims (`aida unclaim <SPEC-ID>`), then
write the seat handoff (`/aida-handoff`) in this shape:

```text
FIRE MODE <in force until <time> | ended <time>> — scope: <...>
Shipped:      #N (<SPEC-ID>) <one line> ...
In flight:    #N — <state> — next: <exact action>
Held:         #N — <why> — releases when: <condition> (operator clears)
Next wave:    <SPEC-IDs, ordered, with overlap notes>
Owed to the operator: <decisions, hold clears, design forks>
Main:         <green | red since <sha>, fix-forward #N>
```

## Report

End each wave with one status line: shipped / in flight / held / parked
counts, main's state, and the next action. Use the seat status contract in
`.aida/discipline/advisor-role.md`. Once your turn ends, never claim you are
"still watching".

## See also

- `.aida/discipline/fire-mode.md`: the authority model this procedure runs under.
- `.aida/discipline/rework-brief-craft.md`: CHANGES briefs.
- `.aida/discipline/advisor-role.md`: the advisor seat.
- `/aida-burndown`, `/aida-orchestrate`, `/aida-fasttrack`: the neighbouring lanes.
- FAQ-57: the ship recipe.

ARGUMENTS: $ARGUMENTS
