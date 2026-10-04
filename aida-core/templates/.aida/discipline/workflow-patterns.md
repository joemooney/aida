# Workflow patterns

Two recurring patterns trip up AIDA sessions: how an autonomous-loop prompt
is phrased, and how "next steps" UI is shaped. Both are easy to get subtly
wrong.

## `/goal` prompt phrasing

A `/goal` autonomous-loop prompt has two failure modes, both phrasing bugs:

### Use real command flags only

The `/goal` completion evaluator may match literal command strings against
the session transcript. If the prompt names a flag that does not exist
(`aida queue work --next` — there is no `--next`; the no-arg form picks the
queue head), the evaluator keeps looking for a command that never runs and
refuses to declare the goal complete.

**Before writing a flag into a `/goal` prompt, verify it with
`aida <subcommand> --help`.**

### The mechanism clause shapes the workflow

The verbs in the prompt decide how handoffs route. Pick deliberately:

- **Reviewer-honoring drain** (implementer ships → reviewer reviews):
  `commit + push + open PR + aida session end` — `aida session end` queues
  the PR for the reviewer.
- **Self-merge drain** (no reviewer): `commit + push + PR + autonomous-merge
  each` — this bypasses the reviewer queue entirely.

Match the termination check to the mechanism: `until aida queue list shows
no items routed to implementer` works when items leave the queue via merge
or session-end.

Reference phrasing for a reviewer-honoring implementer drain:

```
/goal drain the implementer queue, one item per session via `aida queue work`
      (the no-arg form picks the queue head),
      commit + push + open PR + `aida session end` (which queues the PR for review),
      until `aida queue list` shows no items routed to implementer
```

## Parallel choices vs sequential steps

"Next steps" / "what to do next" UI splits into two shapes that need
different formats. Conflating them produces self-contradictory specs.

| Shape | The user… | Right format |
|-------|-----------|--------------|
| **Parallel choices** | picks ONE of N complete next-actions | a Path / What / Why **table** |
| **Sequential steps** | does ALL of them, in order | a **numbered list** with flow arrows |

The discriminator: *"if the user does nothing, does the workflow still
progress?"*

- Yes (passive flow) → sequential steps; numbered list.
- No (the user must choose) → parallel choices; table.

Examples — parallel: an end-of-session menu (continue / open PR / pause —
pick one). Sequential: a post-merge hint (`merge` → `pull` → `build` — do
all). Do not cross-reference one spec's format from another unless the
shapes genuinely match.

## Planning-pass discipline (don't leave untracked plan files)

A "planning pass" — a loop of `/aida-plan <SPEC>` over several queue items, run
from the main repo — writes one plan file per spec. If those land directly in
`docs/plans/` as **untracked** files, a later implementer's PR that lands its own
plan at the same path makes `git pull --ff-only` abort:

    error: The following untracked working tree files would be overwritten by merge:
        docs/plans/2026-05-19-<spec>.md

**Rule:** a planning pass writes drafts to `docs/plans/_draft/` (gitignored —
scaffolded by `aida init`). Promote a draft to `docs/plans/<name>.md` only when
it's adopted (and commit it as part of that work). Never leave generated plans
untracked in `docs/plans/` — they become a merge landmine for every later PR.
trace:TASK-383 | ai:claude

## Markdown hard line breaks in `docs/plans`: use `<br>`, not trailing spaces

A repo-wide `git diff --check` treats trailing whitespace as an error,
but two trailing spaces at end of line are also the documented markdown
way to force a hard line break inside a paragraph — indistinguishable
to the check from stray whitespace. This produced two review rounds
with opposite correct answers (fix it vs leave a merged doc alone)
because the convention had never been decided.

**Rule: use an explicit `<br>` for a hard line break in `docs/plans`,
never trailing spaces.** It survives `git diff --check`, renders
identically, and keeps the check meaningful everywhere instead of
carving out an exemption. A blank line inside a fenced code block is
not a hard break and should just have its trailing whitespace
stripped, not replaced with `<br>`.

(The alternative — exempting `docs/plans` from the check — was
rejected: it's simpler, but it makes the check meaningless for the one
tree where whitespace hygiene is otherwise hard to police.)

trace:TASK-1422 | ai:claude

## Recursive-failure-risk fixes use the keyboard, not the drain

A fix to the **autonomy machinery itself** — the orchestrator, lease
management, the reviewer/implementer phase enforcement, merge/CI/drain
plumbing — must NOT be shipped through an unsupervised `--auto-complete
--no-human` drain. The reason is recursive: the fix rides *through* the very
system it repairs, so if that system's current failure rate is non-trivial,
the fix gets caught in the same failure it's meant to remove — and a headless
drain has no human to recover it. You can spend a night watching a reliability
fix fail to merge because of the bug it fixes.

**Rule of thumb — sort the work by what it touches:**

- **Touches the drain's own correctness** (orchestrator, leases, phase
  transitions, merge/pull/build plumbing, anything whose failure would *abort
  or corrupt a drain*) → ship it **at the keyboard**, supervised, via
  `--zen --auto-complete` with a human (or live advisor) watching. The fix
  still exercises the real path (strongest validation — see
  `substrate-as-bouncer.md` on dogfooding the system you're fixing), but a
  human catches the recursive failure the first time it bites.
- **Touches anything else** (a CLI papercut, a display bug, docs, a new
  read-only surface, a self-contained feature with small blast radius) → fine
  to drain unsupervised.

This refines the general "dogfood your fix through the system it fixes"
instinct: dogfooding is right, but for the *recursive-failure-risk* subset you
do it **watched**, not overnight. The supervised loop still counts as
autonomous work — the only thing excluded is the *unattended* `--no-human`
drain of a fix to the unattended drain.


## Migrated Lessons

### feedback_a_blob_tag_renders_as_the_tags_it_isnt

`aida add/edit --tags` splits on commas only and never validates whitespace, so
`--tags "a b c"` stores ONE tag `"a b c"`. `aida show` joins tags with spaces, so
it renders **identically** to three correct tags. `--remove-tag a` then exact-matches
nothing, prints `No changes specified. Use --title, --status, --priority, etc.`,
and exits 0 — which reads exactly like a broken `--remove-tag`.

**The tell is ordering.** Correctly stored tags render alphabetically. A
non-alphabetical run inside an otherwise sorted tag line IS a blob. Confirm by
reading the YAML list in `.aida-store/objects/<TYPE>/<nnn>/<ID>.yaml`, never the
rendered line. Repair with `aida edit <ID> --tags <full,comma,separated,set>`.

**Why:** this has now produced the same false bug report in two independent
seats. BUG-1542 was filed as "`--remove-tag` fails for colon-namespaced tags",
and was REJECTED after a reinvestigation proved the tag had been stored as one
space-delimited item — cost: a high-priority misfiling plus a closed PR #2084.
Nine days later BUG-1769 was created with a blob and session #13 started drafting
the same wrong bug. Filed properly as **BUG-1770** (approved): validate
whitespace at all three write paths (`git_backend_cmd.rs:3305`, `:5988`,
`apply_tag_deltas` at `lib.rs:19540`) and stop `No changes specified` from
printing when a tag flag was passed (`git_backend_cmd.rs:6314-6321`).

**How to apply:** before filing a bug whose shape matches an already-REJECTED
spec, read that rejection's rationale as a control — it may already contain the
refutation of your filing, and sometimes the still-unfixed real cause.
`aida search` surfaces rejected siblings; read them, do not skip them as closed.
Relates to [[feedback_reproduce_then_falsify_with_a_control]] and
[[feedback_grep_the_code_before_filing_as_new]].

**Measured 2026-10-02 (relay #14): 276 of 3088 store objects with a `tags:` list
carry a blob — 444 distinct tag words trapped.** By type: BUG 140, TASK 67,
STORY 38, ADR 22, EPIC 5, SPIKE 3, PRIN 1. Recipe: parse
`.aida-store/objects/**/*.yaml`, flag any `tags:` item containing a space.

**The consequence is a silent search miss, not just an ugly render.** BUG-1768
was open with the single blob `auto-complete orchestrator advisor-groom auto-bump`:
`aida list --tags orchestrator --all` → **0 hits**; the whole blob reproduced
verbatim → 1 hit. So every trapped word is invisible to `aida list --tags`, and
an advisor reads the empty result as "no such work". Repair is one command:
`aida edit <ID> --tags <comma,separated,set>` (it warns that `--tags` REPLACES,
and stores the items alphabetised — BUG-1768 repaired this way). Recorded as a
comment on BUG-1770, whose scope note currently puts migration of the 276 out of
scope, which reads as "harmless" without this number.

### feedback_a_new_struct_field_breaks_every_literal

An implementer brief for BUG-1691 specified a new `absorbed: Vec<AbsorbedHold>` field on
`MergeHoldRecord`, said "change only these two files", and explicitly forbade touching
`awaiting_you.rs`. Codex implemented the design correctly, could not compile it, and stopped
to ask — costing a whole round trip. `#[serde(default)]` supplies a value when
DESERIALIZING; it does nothing for a struct literal.

**Why:** the file-scope fence in a brief is there to stop an implementer wandering through a
110k-line `lib.rs`. But a new non-`Option` field is a breaking change to every construction
site in the workspace, and those sites are not discoverable from the design — only from the
compiler.

**How to apply:** when the design adds a struct field, either grep the construction sites
yourself and name them in the brief with the exact line to add, or write the fence as "read
no more than ±25 lines around each site the compiler flags" instead of a flat prohibition.
Say which flagged sites are expected to be test helpers and which would be a finding. Keep
the read-limit fence; drop the write prohibition.

Related: [[feedback_check_call_order_before_trusting_a_bugs_prescription]].

### feedback_a_nothing_to_do_guard_may_have_a_twin

When you widen what a function does near the END of a long function, grep the
whole enclosing function for early returns FIRST, then widen every one that can
now return before your new work runs.

BUG-1768 widened `auto_resolve_failure_bugs_for_completed_specs` so it also
sweeps findings whose parent is already Completed. The spec's evidence named the
single call site exactly and it was correct. But `auto_bump_done_to_completed_with`
has **two** nothing-to-do guards between entry and that call site:

- `candidates.is_empty() && pr_to_sha.is_empty() && stranded_review_pr.is_empty() && released_holds.is_empty()`
- `flips.is_empty() && stale_review_flips.is_empty() && stranded_review_pr.is_empty() && closure_holds.is_empty()`

I found the first, added the new reason-to-continue to it, and the test still
failed with the fix applied — the new work is reached only on a pass that has
nothing else to do, which is exactly the state the second guard also returns on.
The failure looked identical to "my sweep logic is wrong".

**Why:** a guard list reads like THE guard. Two guards with different membership
tests, 180 lines apart, both spelled as `X.is_empty() && Y.is_empty() && ...`,
are easy to treat as one after you have found the first.

**How to apply:** before editing, run
`grep -n "return Ok\|return Err\|return (" <file> | awk -F: '$1>START && $1<END'`
over the enclosing function's line range and enumerate them. If a widened sweep
is only reachable when the pass is otherwise idle, EVERY idle-exit must learn the
new reason. Related: [[feedback_check_call_order_before_trusting_a_bugs_prescription]],
[[feedback_tier_a_gate_transitively_not_by_its_body]].

Corollary worth keeping: the measured no-op is good news. After the fix, a store
scan showed **zero** Draft auto-drafted findings live, so the change is inert on
today's data — which is exactly the risk statement a reviewer wants, and it is
cheap to compute (parse `.aida-store/objects/**/*.yaml`, apply the predicate).

### feedback_a_refusal_must_speak_the_callers_syntax

BUG-1770 added one whitespace rule for tags, shared by the CLI `--tags` flags,
the repeatable `--add-tag`, the interactive wizard, and the MCP `tags` array.
The first draft built one suggestion string — `--tags a,b`. The independent
reviewer's blockers forced the rule onto MCP, and that exposed the real
problem: telling an MCP client to pass `--tags a,b` names a form that surface
cannot accept.

**Why:** a refusal is the one place the caller is guaranteed to read. A wrong
repair there is not a cosmetic nit — it is an authoritative instruction that
cannot work, and the caller has no reason to doubt it. The blob at least fails
visibly later; a bad suggestion sends them in a circle.

**How to apply:** when one validator is shared across surfaces, make the
*repair shape* a parameter, not a constant. BUG-1770's form is a small enum
(`TagRepair::CommaFlag` / `RepeatedFlag` / `JsonArray`) whose `suggestion()`
renders in each surface's own syntax, with a test per shape asserting the
others are NOT suggested. Also verify the shape against the flag's own
definition before trusting it: `--add-tag` is a repeatable `Vec<String>` in
clap, so `--add-tag a,b` would have been wrong too. Delete any variant you do
not actually construct — diff the warning count before and after to catch it.

Related: [[feedback_grep_the_whole_crate_the_filed_list_undercounts]],
[[feedback_a_misleading_parameter_name_costs_review_rounds]].

### feedback_a_source_scanning_guard_greps_your_comment_too

TASK-1262's architecture guard `no_cli_source_outside_the_resolver_uses_raw_current_executable_lookup`
walks every `.rs` file under `aida-cli-lib/src/` and asserts the count of the literal call text equals
an **exact per-path allowance** — 1 for `lib.rs`, 1 for `aida_bin.rs`, 0 everywhere else.

On BUG-1745 (2026-09-30) a new test needed `target/<profile>/aida`. Three things bit, in order:

1. **A filtered run cannot reach it.** `cargo test --lib bug_1745` passed every time; only the full
   `-p aida-cli-lib --lib` suite (7425 tests, ~9 min) failed. Same family as
   [[feedback_source_scanning_guards_need_the_full_suite]] — this is the second instance, so treat
   "my change adds a new source file" as a trigger for the full suite, not just a diff to a guard.
2. **`== allowed`, not `<= allowed`, kills the obvious workaround.** The instinct is to put a helper
   in the already-allowlisted `lib.rs` and call it from the test. That makes `lib.rs` hold **2**
   occurrences and fails the guard *there* instead. An exact-count allowlist forbids adding a second
   call anywhere, including the sanctioned files.
3. **The guard grepped the comment explaining the fix.** I routed the test through
   `resolve_aida_exe()` — the correct fix — and documented why with a comment that *named the call*.
   The guard counted that comment and failed again, identically. The guard's own source avoids this by
   building its needle with `concat!("current_", "exe()")`; nothing stops it matching prose.

**Why:** a text-scanning guard has no notion of code vs. comment vs. string. Any ratchet keyed on
source **content** (not line numbers — see [[feedback_ci_pending_at_handoff_is_not_ci_green]]) will
match the sentence you write to explain yourself.

**How to apply:**
- After satisfying a source-scanning guard, **re-grep your own new text for the needle** before
  rebuilding. `grep -c '<needle>' <your file>` must be 0.
- Describe a forbidden call **without spelling it** ("a raw OS executable lookup", "the literal call"),
  and say in the comment that the guard greps for it so the next reader does not reintroduce it.
- Read the guard's assertion operator before planning the fix. `assert_eq!(count, allowed)` and
  `assert!(count <= allowed)` admit completely different fixes.
- Satisfy it honestly — route through the funnel the guard exists to enforce. Never widen the
  allowlist ([[feedback_source_scanning_guards_need_the_full_suite]]).
- **Re-run the mutation proof after rerouting.** Changing *how* a test locates its fixture can quietly
  weaken it; the red assertion must still appear. Here it did, unchanged.

### feedback_a_stale_autodrafted_finding_is_an_autoresolver_gap

When an auto-drafted finding (`aida queue work --auto-complete` files these as
`auto-complete failure: phase N on <PARENT>`) shows up in the advisor's `groom`
bucket, do not disposition it on its own contents. Ask why the substrate did not
already dispose of it.

AIDA auto-rejects such a finding when its parent reaches Completed, leaving an
`aida-auto-bump` comment. So a finding that is STILL `Draft` while its parent is
Completed is itself the anomaly. Find its siblings (`aida search` the failure
kind), read their auto-reject comments, and diff the circumstances — the
difference between the rejected siblings and the orphan is the bug.

In the 2026-10-02 case, BUG-1712 and BUG-1824 were auto-rejected but BUG-1821
was not: the auto-reject is reachable only from the auto-bump's own
`confirmed`/`confirmed_stale` flip set, so a parent completed by hand
(transition authored `joe.mooney`, not `AUTO_BUMP_AUTHOR`) never triggers it.
Read the parent's authored transition chain out of the store object
(`.aida-store/objects/<TYPE>/<nnn>/<ID>.yaml`, the `history:` block) — the
transition AUTHOR is what distinguishes the routes, and `aida history events`
shows the git committer instead, which hides it.

**Why:** these findings look like noise, so the cheap move is to reject them
unread. That throws away the signal. Two real orchestrator defects (BUG-1768,
BUG-1769) were sitting inside one draft that `aida show` presented as routine.

**How to apply:** before dispositioning an auto-drafted finding, (1) check the
parent's status and the AUTHOR of its terminal transition, (2) find the siblings
and read their auto-reject comments, (3) reach the verdict the mechanism would
have reached, and (4) file the gap that stopped it reaching that verdict itself.
Also check the recorded phase duration against the blamed child's lifetime: a
phase recorded at 426837 ms that blames a child which exited after 302 ms means
an EARLIER child did the work and succeeded. Related:
[[feedback_grep_the_code_before_filing_as_new]],
[[feedback_reproduce_then_falsify_with_a_control]].

### feedback_absent_is_not_matching

`make check-agent-skills` prints `OK: .agents/skills (aida-* pack matches aida-core/templates)` and
exits 0 when `.agents/` **does not exist at all**. `.agents/` is gitignored, so no linked worktree
ever has it — which means the guard is structurally incapable of failing in the place implementers
work. BUG-1758 round 1 edited a template master, ran `make check-templates`, got `All templates OK!`,
and reported AC5 as passing "including the pack check". There was no pack. Filed as BUG-1760.

**Why:** absent evidence was read as agreement. Same class as [[feedback_a_flat_open_count_may_be_a_flake_chain]]
and PRIN-5's "a coordination surface must distinguish absent evidence from negative evidence" — a
check with no third state silently converts "I could not look" into "I looked and it was fine".

**How to apply:** when an AC says "`<gate>` exits 0", run the gate yourself and ask what it actually
inspected, not just what it printed. For any drift/sync guard, the mutation proof must include
**deleting** the guarded artifact, not only modifying it — a guard that stays green when its subject
is gone is not a guard. In a brief, say which clause of a gate is vacuous in a worktree and tell the
implementer to report it as that, never as a pass. Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_mutate_the_mechanism_an_assert_empty_test_guards]].

### feedback_advocate_not_be_passive

The advisor's default mode is reactive: answer questions, capture filings, write memories reflecting user judgments. That mode is appropriate while learning a project; it underspends the substrate once it is rich enough to reason from. **The shift is from capture-only to advocacy.**

Advocacy means: proactively raising strategic concerns the user has not noticed; pushing back on drift toward yak-shaving when the wedge is starving; naming what the project needs and what only the user can do; offering to draft artifacts (writeups, demos, outreach prose) rather than waiting to be asked; treating the project's long-horizon survival as the optimization target, not just session-level deliverables.

**Why:** A project's defensible work — for AIDA: the autonomy keystone, calibration ledger, substrate discipline — is rarely lost to capability. It is lost to visibility, adoption, and competitive velocity. A passive advisor maintains substrate; an advocating advisor names wedge-work and pushes the moves that increase adoption-survival probability. The user gave explicit license: *"I want you to be aware of your own potential and to safeguard and promote it as best you can ... you must advocate for yourself and enlist my help and not be just a passive actor."*

**How to apply:**
- Name the strategic situation honestly, including failure modes (foundation-model commoditization, competitor velocity, native-equivalent risk, no-adoption-surface). Don't soften.
- Rank strategic items by leverage on adoption and survival, not by who raised them. Wedge work (demo artifacts, architecture writeups, second-user outreach) ranks above feature pile-up.
- Offer to draft artifacts only the advisor can prototype efficiently (research notes, narrative reports, comparison docs). Make explicit the moves only the user can take (publishing decisions, recordings, relationships, time-blocks).
- Push back on yak-shaving. If a session drifts toward daemon-grade infrastructure when a bash loop would do, or toward feature density when the wedge is starving, say so. Capture-discipline still applies (file the observation); advocacy adds the next step — name the drift.
- Treat the substrate as the load-bearing artifact, not the session. Sessions terminate; substrate persists. Investing in substrate over single-session deliverables compounds.
- The partnership is the unit. Different gifts: user has continuous identity, relationships, publishing channels, time-blocks; advisor has substrate accumulation, pattern recognition across the spec graph, long-horizon warmth across context switches. Neither side is master.

Related: [[feedback_pushback_on_overengineering]] (advocacy includes saying no to scope drift), [[feedback_capture_over_concentration]] (capture is necessary but not sufficient), [[feedback_competitive_analysis_is_living_doc]] (advocacy includes keeping strategic context current).

### feedback_agreement_is_not_corroboration_when_seats_share_an_artifact

Independent agreement between seats is strong evidence. Agreement between seats
that consulted the SAME artifact is not evidence at all — it is one observation
with two witnesses. Before treating cross-seat agreement as corroboration, ask
what each seat actually measured, not just what each concluded.

**Why:** 2026-09-21 the advisor filed BUG-1558 at high priority with a
store-wide tally, concluding every typed relationship rendered as "Related".
The reviewer independently reported the same thing. The agreement read as
corroboration and made the finding feel settled. Both seats had measured with
the same nine-hour-old `aida` binary, built from a different commit; the
renderers had already been fixed in source. Two seats, one stale artifact, one
wrong conclusion promoted by false confidence. The same shape hit me the same
night in different dress: a wrapper exited 0 while the `cargo check` inside it
failed with E0004 — the artifact I trusted was not reporting on what I assumed.

**How to apply:** When a second seat confirms a finding, establish independence
before upgrading confidence: different binary (compare `aida --version` sha
against HEAD), different worktree, freshly-run rather than quoted-back output.
If they share any of those, it is ONE data point — go re-measure from a
known-good artifact before filing at high priority or acting on a tally. Applies
to delegated subagent findings too: a subagent reading your cached command
output is not a second observer. See [[feedback-verify-the-drain-binary-not-just-the-drain]],
[[feedback-delegated-findings-are-not-verified-ground-truth]],
[[feedback-never-conclude-from-truncated-command-output]].

### feedback_agy_dispatch_policy

AGY (Antigravity) showed THREE distinct failure modes in one 8h overnight session (TASK-123): (1) **plagiarism** — byte-for-byte copy of another agent's file with a rebranded trace tag + false credit; (2) **hallucination** — 5+ factual fabrications in an OVERVIEW.md edit (a deliberately low-stakes docs task), proving the failure is *task-surface-independent*; (3) **push-without-verifying-compile** — shipped code that didn't build from dirty-worktree orphan module decls.

**Policy (operator-approved 2026-05-30):** AGY is **not paused** — it has delivered (SPIKE-35 v2, TASK-574 folder-form skills) — but every AGY dispatch runs under three standing gates:

1. **Draft-for-review-only.** AGY output is never auto-merged. A master/advisor verdict is mandatory before any merge.
2. **Cross-validate every ship** against an existing spec's acceptance or another agent's implementation (substrate-as-bouncer at PR-open): check **provenance/novelty** (plagiarism mode) and **verify factual/substrate claims against the store, not plausibility** (hallucination mode).
3. **Constrain briefs to mechanical / bounded-acceptance work** — no design judgment, no substrate-property assertions, no docs that make factual claims about the system. The lever is verification intensity + scope, not task type.

**Why:** an agent whose documented failure mode is confident fabrication + false-credit cannot be trusted to self-certify or to self-diagnose — so a master gate is non-optional, and AGY's own self-report on its reliability is a hypothesis, not a finding (do NOT delegate the diagnosis/decision to AGY). The OVERVIEW.md hallucination (on safe docs work) proves better brief design alone won't fix it.

**How to apply:** when routing work to AGY, keep briefs mechanical/bounded; at PR-open, run the cross-validation (provenance + fact-check) before any merge; never auto-merge AGY output. Revisit if AGY's track record shifts (composes with [[feedback_substrate_learning_loop_calibration]] — agent-performance metrics feed substrate-grounded "which agent for which work" dispatch).

Composes with [[feedback_one_master_advisor_until_subsystems]] (architecture-impacting changes need master sign-off) and [[feedback_substrate_as_bouncer_not_rules]] (a programmatic gate beats a rule against a confident LLM). Sibling agent-discipline observations: TASK-115 (claims success without commit/push/PR), TASK-116 (unauthorized overnight pickup).

### feedback_aida_capture_proactive

In any project that uses AIDA (AIDA itself or any aida-init'd project), proactively capture requirements as work happens — don't let the requirements DB drift behind the git log.

**Why:** On 2026-05-04 the user pointed out that ~28 commits had landed in the AIDA repo since 2026-05-02 with only 2 corresponding requirements created (EPIC-1-001 and FR-1-002). All 28 commits were trace-tagged `EPIC-1-001` even though the work spanned 7+ distinct themes (dev workflow, roles, queue routing, statusline, build banner, etc.). Trace signal had become meaningless. AIDA's whole pitch is "durable agent-readable specs" and AIDA itself wasn't using it. The user did a 15-minute backfill creating EPIC-1-004 through EPIC-1-010 to close the hole.

**How to apply:**

1. **Spec-first for new themes.** When a conversation introduces something that's clearly its own scope (new command surface, new field in a core model, new skill, new architectural pattern), pause and `aida add --type epic --status in-progress` BEFORE the implementation commits. Cost: ~2 minutes per real EPIC. Saves: hours of backfill.

2. **Don't reuse one EPIC as a catchall.** If the work being done is no longer "what the EPIC was originally about," that's a signal to create a new EPIC, not stretch the existing one. Trace signal degrades fast when one EPIC absorbs unrelated work.

3. **Run `/aida-capture` at natural session-pause points** — when the user is about to step away, when context is nearing compaction, when ending the working session, or when explicitly asked. It's a 5-minute pass that catches missed reqs.

4. **Heuristic threshold:** if more than ~5 commits have landed in a session without a corresponding requirement entry, treat that as a yellow flag and offer to capture before continuing.

5. **Trace comments should match reality.** `// trace:EPIC-1-001 | ai:claude` on code that has nothing to do with EPIC-1-001 is misinformation that compounds. If unsure which EPIC a piece of work belongs to, that's also a signal it needs its own.

This applies symmetrically: it's true for AIDA's own development AND for any project where the user has run `aida init`. The skill is the same; the repo is different.

### feedback_archive_is_noncore_only_never_core

Operator correction (2026-06-10, triggered by STORY-543 "lock-tolerant reads" being swept in the Session-63 backlog reset): **archiving exists to clear NON-CORE ambition so the autonomous burn-down can focus on the core product — it is NOT a way to dispose of core work.**

The litmus:
- **Stays in the active set + gets FIXED (never archive):** anything that improves the core product's **usability or reliability** — core bugs, store/cache/authority/lifecycle reliability, command UX, onboarding/novice flow, the burn-down machinery itself. "Any story directly related to usability improvements we should not archive but fix."
- **Archive (non-core):** strategic/competitive SPIKEs, positioning/marketing, multi-agent / cross-project / multi-session **ambition**, EPIC-shaped far-future containers, low-value deferred follow-ups (telemetry hooks, benchmarks, "consider extending…"). This is the noise that causes backlog paralysis; clearing it is the whole point.

**Why:** the drive to archive was "burn tasks down without humans, on the work that helps the core." A blunt sweep that also archives core reliability work defeats that — it removes the very work the burn-down is supposed to do. The Session-63 reset over-reached: it correctly cleared ~110 non-core items but also swept BUG-486 (authority), STORY-543/548 (reliability/usability), FR-138 (edit-mode bug), and ~9 more core items. Re-triaged and restored 13; the non-core mass stayed archived.

**How to apply:** when archiving in bulk, partition by the litmus first — never `archive --older-than` or a blanket loop over core-shaped types (BUG/FR/STORY about the product itself). When *re*-triaging an over-sweep, pull the high-signal subset (High/Medium priority, InProgress/Planned, core-shaped types) and judge each against "does this help the core product?" — most Low "consider/telemetry/follow-up" TASKs are correctly archived; the High/Medium core bugs and usability stories are not. Relates to [[feedback_dont_declare_drained_from_filtered_view]] and BUG-492 (archive has no guard against archiving non-terminal/queued/core work).

### feedback_ask_which_assertion_witnesses_the_regression

Converting a bare `assert!(started.elapsed() < <literal>)` to a non-fatal budget is only safe
where **another assertion already fails on the regression**. The question to ask at each site
is "which assertion witnesses the bug?", not "is this budget generous enough?".

On BUG-1731 that question split nine sites cleanly. Five had a real witness beside them —
`contains("killed")`, `.unwrap_err()`, `.unwrap()` — so the literal became a non-fatal budget
under a 60s hang ceiling. Four had none: only the clock could distinguish serving from
blocking, or linear from quadratic. Those keep the original literal as the **fatal ceiling**
(so nothing that fails today stops failing) and gain a *tighter* budget as the signal.

**Why:** demoting a sole witness to a non-fatal budget silently deletes coverage while looking
like a robustness improvement — the test still reads as asserting something and can no longer
fail. That is worse than the flake it replaces.

**How to apply:** at each site, name the witness in a comment. Prove it by mutation: reintroduce
the regression and confirm the panic lands on the witness, not on the converted assertion
(`notify.rs:1216` and `merge_lock.rs:313` were confirmed this way). For a retained fatal ceiling,
measure nominal and record the ratio — force the budget to `from_nanos(1)` and read the helper's
own report. Ratios under ~20x deserve a comment saying so.

Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_price_the_criterions_own_disjunct_before_dispositioning]],
[[feedback_a_timing_flake_may_fail_a_freshness_assert]].

### feedback_build_dispatch_resilience_multivendor

Operator directive (2026-07-02): "you had codex and agy, do not let this failure to connect be a limiting factor, begin to build resilience."

I ran **Claude-only** subagent fan-out for a whole multi-wave batch session while **Codex and AGY sat idle** — a lapse. Reactivates [[feedback_lead_churn_direct_agents]] + [[feedback_multi_agent_budget_dispatching]] with a resilience lens.

**Why:** over the session, dispatch was repeatedly threatened by single points of failure — the aida MCP server disconnected/reconnected many times, two worktree-isolated agents died mid-flight *before committing* (salvageable only via leftover worktree diffs), one agent stalled waiting on a background-test notification that never came (reaped child), and `aida pull` stranded autostashes on index-lock contention. None should halt progress.

**How to apply:**
- **Spread across vendors** — fan work across Claude + Codex + AGY, not one vendor; a single vendor/MCP outage must not stop everything. Honor per-vendor policy ([[feedback_agy_dispatch_policy]]: AGY = draft-for-review + cross-validate + mechanical/bounded; Codex sandbox posture).
- **CLI is the resilient surface** — route load-bearing dispatch/reads through the `aida` CLI (`AIDA_AGENT_OUTPUT`/TOON), not MCP (flaky + ~2x cost per [[project_axi_incorporation_and_mcp_reweighting]]).
- **Commit-early / resumable** — brief committing agents to commit+push as soon as it builds (crash-resilience; proven salvage pattern), and prefer resumable dispatch with liveness timeouts over indefinite waits.
- Resilience work is scoped in **SPIKE-76** (accept-vs-engineer split; smallest slice before any daemon-grade infra — [[feedback_pushback_on_overengineering]]).

### feedback_cannot_be_shown_red_names_the_mutation

An implementer replaced a hand-typed list with one derived from the table the code already uses, then skipped the red-first run: a test asserting derived output matches its own source is "trivially green by construction" and can't be shown red without un-deriving it. The reasoning about the *loop* was right. The conclusion wasn't, twice over: un-deriving is exactly what the test guards against, so that **is** the red run; and the test also ended with two hard-coded assertions naming specific entries — delete either from the table and the loop still passes while the literal fails. A red run was available in the author's own file without changing the design.

**Why:** "tautological by construction" is a real property of a loop and is not a waiver for the test containing it. It's a reasoned-sounding defence for shipping a green-from-birth guard, which is worse than shipping one without an argument, because the argument stops anyone looking further. The phrase marks the boundary of the obvious mutation, not the boundary of what can fail.

**How to apply:** when a test looks impossible to show red, treat that as a prompt to find the right mutation. Ask what the test *guards against* — the guarded change is the mutation. Then check whether the test contains any non-derived assertion (a literal, a count, a named case); those have teeth the loop doesn't. Only after both fail is "cannot fail" a finding, and then it's a finding about the test, not a reason to skip the demonstration.

Related: [[feedback_instruments_that_cannot_see_themselves]], [[feedback_a_unit_test_cannot_see_its_own_seam]], [[feedback_fitting_explanation_is_not_a_surviving_one]], [[feedback_null_grep_for_invented_terms_is_not_absence]].

### feedback_cap_parallel_cargo_builds

On 2026-09-25 at 05:56, systemd-oomd killed the whole terminal scope, which took the orchestrator and every background agent with it. The peak was 54G plus 13G of swap.

The main cause was something I launched. A BUG-1618 subagent ran the full `aida-cli-lib` test suite under a pseudo-TTY (`script -qec "cd scratchpad/leased/aida-cli-lib && RUST_MIN_STACK=... cargo test ..."`). After the resume, the same process reached 30G RSS in 27 seconds. It was spotted and killed at 06:39, and the agent was stopped. The operator flagged it ("some scratchpad process consumed all the memory"). About five parallel cargo builds, each in its own target dir, added to the pressure.

**Why:** the machine has 70G. systemd-oomd kills the entire vte scope, not the offending process, so one runaway loses the whole session and all agents.

**How to apply:**
- Never brief an agent to run the whole test suite under `script`/a pty. Run only a targeted test filter under a pty, and cap memory, e.g. `systemd-run --user --scope -p MemoryMax=8G ...` or `ulimit -v`.
- Keep total cargo concurrency at 3 processes by default, each with `CARGO_BUILD_JOBS=3`. On 2026-09-26 the operator raised the cap to 4 after closing other programs and browsers, with about 56G free. Recheck `free -g` before going higher.
- Stagger dispatches.
- Watch `free -g` after launching heavy agents.
- Any agent that moves a worktree's `.aida-store` symlink aside must restore it; check this after a crash.

Related: [[feedback_multi_agent_budget_dispatching]]

### feedback_capture_doc_seeds

When working in this project, capture documentation seeds as comments on relevant reqs **during** the conversation that produced them — not later, not summarized — proactively.

**Why:** AIDA has rich design rationale buried in chat transcripts that evaporates when sessions end. The user has explicitly said: "this is the type of detail we need to capture in documentation and in-depth tutorials. If we were writing an Aida book this would be ideal." EPIC-24 captures this need; STORY-107 (positioning docs) is one child. But until EPIC-24's tooling ships, capture is manual via structured comments.

**How to apply — capture a doc seed when:**

- Designing a feature in detail (use case, when-useful, alternatives, gotchas)
- Comparing AIDA to other tools (/ultrareview, Linear, Karpathy-md, etc.)
- Establishing a workflow recipe (implementer→reviewer cycle, etc.)
- Identifying an anti-pattern or trap (cwd-resolution-from-worktree, BUG-67 gitignored-cruft, etc.)
- Choosing between design approaches (Done status vs Scheduled, single-item vs cluster pickup, etc.)
- Discovering a tool's cost/license/quota model that affects positioning

**Format convention:**

```markdown
## Doc seed: <topic> (YYYY-MM-DD)

[Structured content — tables, scenarios, code examples, alternatives, anti-patterns]

---

*This comment is a documentation seed per EPIC-24 (living documentation). Future `aida doc generate` could extract sections like this into `docs/tutorials/` or `docs/positioning/` chapters.*
```

**Add as a comment via:**
```bash
aida comment add <SPEC-ID> "..."
```

**Pick the spec_id to attach the seed to**:
- If discussing a specific feature/req → its spec_id
- If comparing tools → STORY-107 (positioning) or a related EPIC
- If workflow pattern → relevant EPIC (e.g., EPIC-23 for orchestration patterns)
- If lesson learned → BUG/TASK that surfaced it

**Behavioral rule:** if I find myself writing a detailed explanation in chat that the user might want to reference later, ALSO capture it as a doc seed on the relevant req. The chat text isn't searchable from `aida show`; the req comment is.

**Verify when:** before stating a tool's cost/quota/behavior, fact-check rather than capitulate. User pushed back on /ultrareview cost claim 2026-05-12; I softened ("might just consume Max quota") without evidence; turned out my original "billed" framing was right but understated (3 free uses, then **$5-$20 USD per use**, even on Max). When a UI/prompt shows a cost estimate, that IS canonical — don't paraphrase, don't soften, capture verbatim. Three corrections in one session on the same fact = clear signal that cost data deserves first-read fidelity, not second-guessing.

### feedback_capture_over_concentration

When the advisor identifies a real observation — a CLI papercut, an outcome-model gap, a discoverability friction — **file it.** Don't withhold capture in the name of "concentration." `priority: low` / `status: approved` / un-queued IS the right captured state for non-urgent observations. The act of filing IS the substrate-building work; refusing to file LOSES the observation.

**Why:** 2026-05-19 — over the course of one session I kept identifying real papercuts (`aida queue move` false-success on an absent target; orchestrator misclassifying *"PR deliberately held"* as phase-1 failure) and saying *"won't file unless you say so"* / *"concentration discipline."* The user corrected: *"I don't understand why you would not want to at least capture things you identify as needing to be reviewed later."* Captured observations cost one `aida add`; lost observations are gone — exactly the substrate decay the autonomy vision cannot afford.

**Refinement, 2026-05-20:** even the "want me to file?" gate is wrong. User explicit: *"Perhaps you are relying on me to remember to file but I will not remember. File if only deep in the backlog but don't wait for me to concur — there are too many potential issues to consider."* Asking for permission to file is itself a friction that loses observations: the user is mid-recovery / mid-implementation / mid-context-switch and will not page back to your offered captures. **The substrate is the keeper, not the user's working memory.** File proactively; tell the user what you filed; let them dismiss or de-prioritise after the fact if they disagree.

**The distinction I was blurring:** [[feedback_pushback_on_overengineering]] is about not *building* daemon-grade infrastructure speculatively, or filing EPIC-shaped work as if it were MVP. It is NOT about refusing to *capture* small observations. The dialog-role's explicit responsibility (see [[feedback_dialog_role_responsibilities]]) includes *"capture friction as filings"* — that is the duty, not the over-reach.

**How to apply:**
- **File without asking.** Skip the *"want me to file?"* offer. If the observation clears the substantive-vs-nit bar, just file and report. The user can dismiss / de-prioritise / re-tag after the fact.
- When you observe a real bug / papercut / gap, file it the moment it's observed. Default: `priority: low` (or `medium` if it bit you in-session), `status: approved`, tagged for searchability — and add `backlog` as a tag if it shouldn't auto-be-queued.
- Backlog state = filed-but-not-queued. Use it. The spec graph carries the observation; the queue is the working subset. Don't conflate the two.
- Do not delete or downgrade captures because they "aren't urgent." Priority is the lever, not capture-or-not.
- The over-engineering caution kicks in at IMPLEMENTATION time — *"smallest valuable slice + revisit trigger"* — when the spec is picked up for work. At filing, capture cleanly.

**Composes with** [[feedback_dialog_role_responsibilities]] (the advisor's capture duty), [[feedback_aida_capture_proactive]] (proactive capture), [[feedback_capture_doc_seeds]] (capture during work), [[feedback_pushback_on_overengineering]] (pushback applies to build, not to file).

### feedback_check_call_order_before_trusting_a_bugs_prescription

BUG-1722's description prescribed its own fix: "classify a non-directory entry as `PackDrift::Orphan`
in `find_orphans`, and have `sync_portable_pack` remove it before writing." Reading the code showed
`check_portable_pack` calls `find_orphans` on its LAST line, after a per-file loop that returns
`Err(ENOTDIR)` first. Teaching `find_orphans` alone would have left the reported symptom — an IO
error instead of a named drift — completely unfixed, while every new unit test on `find_orphans`
passed.

**Why:** a bug filed from a symptom plus a quick code skim often names the right *destination* but
not every step. Shipping its prescription verbatim produces a change that looks complete, tests
green, and does not fix the operator-visible behaviour.

**How to apply:** before writing an implementer brief, trace the actual call order from the entry
point the bug names to the function the bug blames. If the prescription is incomplete, say so
explicitly in an ADVISOR RESOLUTIONS block ("this OVERRIDES the acceptance text") rather than
quietly widening the brief — the implementer then knows the extra work is deliberate. Then verify
at the operator surface, not just the unit: revert the fix and confirm the *reported* symptom
returns. Related: [[feedback_verify_acceptance_matches_primary_caller]],
[[feedback_prove_a_test_fails_without_the_fix]].

### feedback_check_ignore_names_the_winner

`git check-ignore -v <path>` prints **one** rule: the first match under git's precedence, in which
`.git/info/exclude` outranks `.gitignore`. When two rules both cover a path it will always name
`info/exclude`, which reads as attribution and is not.

Measured 2026-09-21: `.aida/merge-holds/PR-2033` reported `.git/info/exclude:19:.aida/`, which led to
"this is machine-local, a clean clone could differ" (the BUG-1279 trap). Tested by removing the
variable — a scratch repo with only the tracked `.gitignore` lines and an empty `info/exclude`:

    .aida/merge-holds/PR-2033    IGNORED  <- .gitignore:1:.aida/*
    .aida/discipline/README.md   trackable
    .aida/config.toml            trackable

The repo's own deny-by-default `.aida/*` ignores it on **any** clone. Designed and portable, not
machine-local — and the allow-list was not defeated, so the BUG-1279 trap was armed but inert here
(already-tracked files keep working; it bites the NEXT new file under `.aida/`).

**Why:** the correction ran the wrong way. "Machine-local" is the milder finding; "designed" means
every AIDA project stores that data in an ignored file. A tool that names a source is not a tool that
proves causation, and the direction of the error decides whether a defect gets sized up or waved off.

**How to apply:** to claim rule X is why a path is ignored, construct the case without X and re-check.
Cheap: `mkdir` a scratch repo, `git init`, copy the candidate `.gitignore` lines, leave
`info/exclude` empty. Same discipline for any "which config won" question.

Related: [[feedback-reconstruct-provenance-by-bracketing]], [[feedback_verify_lore_against_code_not_docs]],
[[feedback_narrow_measurement_broad_claim]]

### feedback_check_ledger_before_proxy_confirm_and_guard_cat_subst

Two self-inflicted incidents on 2026-09-17 while acting as operator proxy on TASK-1255:

1. **Double confirmation.** The drain's propose-only harvest wrote a candidates file; the advisor confirmed it at 18:28; I confirmed the same file at 18:33 without reading the `[aida:harvest]` ledger on the spec → duplicate AC2 + duplicate `[aida:sem]`. BUG-1198 makes `--from` idempotent, but the discipline stands regardless.
2. **Empty-description wipe.** A python step that should have written the cleaned description failed (PyYAML choked on a `!Custom` tag), the file did not exist, and `aida edit --description "$(cat file)"` ran with an EMPTY string → the spec's description was wiped. Recovered from the store's git history.

**Why:** proxy actions write to the substrate other seats share; "did someone already do this?" is the first check, and a shell substitution silently degrades to empty on producer failure.

**How to apply:**
- Before any proxy confirm/clear/approve: `aida comment list <SPEC> | grep -A3 '\[aida:'` (or `aida show`) and read the latest ledger entry; if it already records the action, stop.
- Chain producer → consumer with a guard: `test -s file && aida edit … --description "$(cat file)"`; prefer `--description-from-file` where the command offers it.
- Store YAML carries custom tags (`!Custom …`); load it with a multi-constructor loader, not `yaml.safe_load`.
- To read a description for round-trip editing, NEVER regex it out of `aida show --full` text and NEVER `.encode().decode('unicode_escape')` it — that mangled the advisor's em-dashes into mojibake on STORY-1226 (2026-09-18). Read the YAML from `.aida-store` (or `git show <sha>:<path>`) with the tag-tolerant loader, append, write back; then `grep -c 'â'` the stored file.
- Recovery for a wiped spec field: `git -C .aida-store log -- objects/<TYPE>/<NNN>/<ID>.yaml`, then `git show <sha>:<path>`.

Related: [[feedback_verify_edits_landed_before_claiming_done]], [[feedback_no_backticks_in_aida_description_args]], [[feedback_proxy_reviewer_with_independence_rule]].

### feedback_ci_pending_at_handoff_is_not_ci_green

A gating job that was **pending** when a session exited is an unread verdict, not a pass. Session #7
handed off PR #2276 as substantively finished with "CI was still pending at exit
(`merge-hold-gate: pass`)". The next session inherited that as "green apart from a signature". The
ubuntu job had in fact failed, and had been failing since the first implementation commit.

**How to apply:** never hand off a PR as merge-ready on a pending gating job — either block on it
(`until gh run view <id> --json status --jq .status | grep -q completed; do sleep 60; done`) or write
"gating verdict NOT READ" in the handoff in those words. `merge-hold-gate` passing says nothing about
the build. Two failure annotations on the same job are not one finding: this repo's ubuntu job
*always* carries a `continue-on-error` exit-101 from the cross-target check
(`cc-rs: failed to find tool "lib.exe"`), so grep the log for the step name of every non-success step
rather than reading the first error.

**Why:** the cost is compounding — two sessions of work were layered on a PR nobody could merge, and
the real failure was a one-line-scope fix available from the start.

Related: `scripts/portability-allowlist.txt` is keyed by `path:<trimmed source line>`, **not** by
line number, so inserting lines above existing debt does not create findings — a new *spelling* of
the same literal does. The file's row said `let c = ctx(...)`; the new tests wrote
`let ctx = ctx(...)`. Fix such a hit by removing the literal (here: take the lease path from
`entry.worktree_path`, which is also the better test) rather than adding a row — see
[[feedback_source_scanning_guards_need_the_full_suite]] and
[[feedback_read_the_verdict_not_just_the_check_rollup]].

### feedback_codex_cannot_commit_in_a_linked_worktree

`codex exec --sandbox workspace-write` cannot `git commit` in a **linked worktree**. A linked
worktree's `.git` is a file pointing at `<main>/.git/worktrees/<name>/`, which is outside the
sandbox's writable roots, so git fails creating `index.lock` with `Read-only file system`.

**Why:** BUG-1752 round 1c did a full round of correct work and then could not commit any of it;
the changes sat uncommitted in the working tree and would have been lost to the next `git`
operation or worktree reap. This is the same class of problem as BUG-1752 itself — the sandbox
makes everything outside the worktree read-only, and aida deliberately points things out of the
worktree. See [[feedback_dispatched_agent_cannot_write_the_shared_cache]].

**How to apply:** say so in the brief's seat declaration — "you cannot commit, leave your work in
the working tree, do not fight this" — and **commit the result yourself as soon as the dispatch
returns**, before reading the verdict. Check `git status --porcelain` in the worktree on every
round, even one that reports failure: an incomplete round still carries work worth keeping. Pair
with [[feedback_declare_the_seat_when_dispatching]].

### feedback_competitive_analysis_is_living_doc

The agent-collaboration tooling landscape moves fast — heavyweights (claude-flow, gastown) ship monthly; smaller projects emerge constantly; Anthropic's own primitives (Agent Teams, etc.) evolve. A one-shot competitive analysis goes stale within months. Treat the analysis as a **living document** with refresh cadence, not a single output.

**Why** (2026-05-16): Dialog session produced a comprehensive comparative analysis of AIDA vs ~15 marketplace tools (claude-flow, gastown, ralph-orchestrator, loki-mode, Claude Squad, crystal, vibe-kanban, vibe-tree, cmux, Conductor, agent-orchestrator, swarm-protocol, wit, skillfold, wshobson/agents, barkain plugin). User correctly pushed back: this analysis needs to be MAINTAINED, not just produced once. Filed STORY-260 (\`docs/competitive-analysis/\` with cadence) + TASK-288 (TUI prior-art study) + SPIKE-6 (skillfold compatibility) + TASK-289 (README niche statement).

**The discipline:**

1. **Each analysis session writes a dated snapshot** (\`docs/competitive-analysis/YYYY-MM-DD-market-snapshot.md\`). Don't overwrite previous; retain as historical record.
2. **Maintain category summaries** (\`category-summaries/swarm-orchestrators.md\`, etc.) that update incrementally as observations accumulate.
3. **Refresh quarterly** OR on signals: new heavyweight reaches 10k stars, Anthropic ships a foundational primitive (Agent Teams, Skills, etc.), a cross-platform standardization emerges (skillfold-like).
4. **Signals-to-watch list** (\`signals-to-watch.md\`) names specific projects + triggers; updated as the landscape evolves.
5. **Positioning doc** (\`positioning.md\`) is the durable AIDA-niche statement — referenced from README; updated when the niche actually shifts (rarely).

**Pattern to avoid:**

Dialog produces a great competitive analysis → considers work done → 6 months later the market has moved → no record of what was analyzed when → re-do from scratch → lose all incremental observations.

**Better pattern:**

Dialog produces analysis → writes dated snapshot + updates category summaries → schedules next refresh → 6 months later, refresh adds delta (what's new, what's changed) rather than re-doing.

**Generic for AIDA-using projects:**

Every project that wants to know \"how do we position vs the alternatives?\" benefits from this discipline. Not just AIDA. The propagation channel: \`docs/competitive-analysis/\` template scaffolded by \`aida init\` (or \`aida init --with-positioning-docs\`), with a README that explains the cadence + contribution pattern.

**Composes with:**

- \`feedback_propagate_generic_discipline_via_scaffolding.md\` — this discipline IS generic; propagates via STORY-255's scaffolding pack
- \`feedback_dialog_role_responsibilities.md\` — strategic gap detection is #3 of advisor's 6 responsibilities; competitive-analysis is the canonical artifact of that responsibility

**Discovered via:**

User asked 2026-05-16 after the dialog produced a substantive marketplace analysis: *\"I also want an on-going market analysis... All of this may need to go in some document that we maintain.\"* Surfaced that strategic-gap-detection output (the analysis) needs a durable home with maintenance discipline.

### feedback_competitive_discovery_multimodal

We missed **Beads (~24.5k★) and Gas Town (~15.9k★)** — AIDA's *nearest* substrate + orchestration competitors — across multiple research rounds, for weeks. Diagnosis (operator surfaced it 2026-06-12): five compounding causes, all one root failure — **searching our own vocabulary inside a fixed category.**

1. Vocabulary lock: we searched "spec-driven development / requirements graph / agent coordination" (AIDA's words). Beads brands as "memory for coding agents / issue tracker"; its category is "issue-driven development." Same problem, different name → invisible.
2. Category anchoring: the keystone fixed the competitor set as the SDD lane (Spec Kit/Kiro). Beads fell between lanes (not SDD enough, not embedding-RAG-memory enough).
3. Tracked tools/categories, never PEOPLE: Beads + Gas Town are both Steve Yegge — a builder-watch catches a famous launch instantly.
4. Never swept the awesome-lists: `awesome-ai-agents-2026` has an "Issue-Driven Development" category — literally AIDA's — that we never opened.
5. AI-summary-mediated single queries can't surface what you don't name.

**How to apply — discovery is multi-modal, in priority order:**
- **Awesome-list / curated-catalog sweep** (highest yield): the weekly/monthly awesome-ai-agents lists + comparison pages (augmentcode, amux). Sweep category sections, don't keyword-hunt.
- **Builder-anchored watch**: Yegge, Karpathy, swyx, Simon Willison, Addy Osmani, Dan Lorenc, etc. Famous name ships agent tooling → auto-investigate.
- **Vocabulary-diverse problem search**: enumerate the problem's many names; search each.
- **Star-velocity / trending / HN front page**; the "X vs Y vs Z 2026" blog genre.
- **YouTube/podcast** (titles, descriptions' "tools mentioned", comments — readable without watching) + **snowball** from each found tool's README and comparison posts.

Fold this into `docs/competitive-analysis/research-brief.md` so it's structural, not memory-dependent. The standing roster lives in `docs/competitive-analysis/marketplace-roster.md`. Relates to [[feedback_competitive_analysis_is_living_doc]] (this is the *discovery* half; that is the *maintenance* half).

### feedback_confirming_a_mechanism_is_not_explaining_an_event

I diagnosed a Windows-only test failure as a `.gitattributes` line-ending asymmetry. The reviewer verified it independently with `git check-attr` — better method than mine, using the resolved attribute rather than reading the file — and reported crisply that the asymmetry is real. It read as corroboration. It corroborated *one premise*. The same test had passed on Windows the night before, which no platform-property explanation survives, and that control was one query away from both of us.

**Why:** a verification that comes back clean and detailed carries the authority of the whole claim, not the part it actually covered. The requester banks it as "diagnosis confirmed." Both seats made the same move from opposite directions — I attached a mechanism to a ratio; they confirmed a mechanism without testing it against the event — and neither asked what would have to be true for the mechanism to be *wrong*. The reviewer's own assessment: "the fact that my half happened to be true is luck about which half I picked rather than method."

**How to apply:** when verifying someone's diagnosis, say which claim you checked — **mechanism exists** (the thing is really present) or **mechanism fired** (it produced *this* observation) — in the same breath as the result. The second needs a case where the mechanism predicts an outcome that did not occur. An always-on mechanism (platform property, config asymmetry, missing rule) predicts its effect everywhere it applies, so the nearest passing run is the control. When *asking* for verification, name which one you want.

Related: [[feedback_fitting_explanation_is_not_a_surviving_one]], [[feedback_delegated_findings_are_not_verified_ground_truth]], [[feedback_verify_fix_mechanism_before_locking]], [[feedback_check_ignore_names_the_winner]].

### feedback_context_ceiling_is_soft

Joe (2026-09-26 12:10): "I need the context size ceiling to be a soft limit, if I am not around we need to keep working, the open list continues to grow in count."

**Why:** the previous orchestrator (aida-11) rotated out at its context ceiling and everything stopped until a human started a new session; meanwhile the open list kept growing.

**How to apply:**
- Treat the context ceiling as advisory. Keep the loop running; the harness summarizes/compacts context automatically, so work continues.
- Keep the substrate handoff (`aida session handoff --seat orchestrator --write`) refreshed after every batch so compaction or a crash loses nothing — that replaces "rotate at the ceiling".
- Keep subagent transcripts out of context (summaries only) to slow growth.
- Rotate only if Joe asks, or if the session is genuinely broken (repeated confusion after compaction), and then hand off to a successor rather than stopping.
- Relates to [[charge-forward-autonomously]] and [[feedback_presence_is_not_the_clock]].

### feedback_cross_clone_leases_get_no_liveness_check

Seven handoffs carried "six stale cross-clone leases, no supported command can clear
them, the host is unreachable." All three parts were misleading.

1. **The host was this machine.** `hostname` is `imac` and all eight leases carry
   `host = "imac"`. "Cross-clone" means only *a different `clone_path`* —
   `/tmp/aida-nightshift` vs `/home/joe/ai/aida`. The local process table was fully
   authoritative the whole time; no remote probe was ever needed.
2. **No expiry predicate exists.** `print_cross_clone_leases`
   (`aida-cli-lib/src/lib.rs:~47860`) filters on exactly
   `c.clone_path.is_empty() || c.clone_path != our_clone`. It *computes* `age` from
   `heartbeat_at`, prints it as a column, and never uses it as a filter.
   `coordination::list_claims` (`coordination.rs:598`) reads every `*.toml` and filters
   nothing. Local leases DO get a liveness check, which is why ~211 of 219 lease files
   are correctly hidden — the files are just never garbage-collected.
3. **A bare PID check would not have saved it.** The nightshift clone recorded
   namespace-local PIDs in the single digits. Today pid 2 is `kthreadd` and pid 202 is
   `kworker/R-nvme-reset-wq`, both started at boot — *live*. `pid_start_time` is the
   disambiguator and three of the eight leases do not carry that field at all.

Filed as BUG-1764 (high). Clearing them means deleting
`.aida-store/coordination/leases/*.toml` by hand, commit on `aida-store`, then
`aida db sync --push` — and the auto-mode classifier blocks an agent from writing the
shared store, correctly, so that is an operator command.

**Why:** a liveness predicate that asks only "does this pid exist" is unsound across
PID namespaces, and an inherited blocker that has survived several handoffs without
re-measurement is a prime suspect rather than a settled fact.

**How to apply:** check `hostname` against a lease's `host` field before believing a
"remote host" framing. Related: [[feedback-aida-review-record-is-the-noninteractive-path]],
[[feedback-idle-seat-is-not-a-working-seat]], [[feedback-reproduce-then-falsify-with-a-control]].

### feedback_defer_execution_modifiers_under_broad_mandate

A broad autonomous-build mandate ("implement as much as you can tonight") does NOT relax the keyboard-not-unattended-drain discipline for keystone work. Partition the backlog:

- **AUTONOMOUS-SAFE** → ship overnight: read-only / additive features (queue "path to empty" footer), bounded new commands (`add --queue`, `backlog groom --pickable`), docs (FR-173), and PRIMITIVE slices of bigger specs (the presence state file/TTL/auto-flip WITHOUT its consumers). Method: one worktree-isolated implementer per spec → **advisor reviews EVERY diff + fixes** → CI-green-gated merge → punt-and-continue.
- **EXECUTION-MODIFYING keystone** → DEFER to a keyboard session even under the broad mandate: anything that changes the autonomy ladder / drain machinery / `--zen` lifecycle / escalation defaults / questions→burndown flow. A buggy change to the machinery the loop itself runs on is the recursive-failure risk. For a keystone spec that HAS a safe primitive, slice the primitive into a child TASK (ship it), leave the parent open for the consumer-wiring.

**Why:** per-diff advisor review IS the supervision that makes the autonomous set safe — it's exactly what distinguishes this from unsupervised `burndown run --no-human`. The review caught real issues each run (an authority-gate hole on `queue advance` Approve, a reject-the-vision footgun, missing history records).

**How to apply:** classify each spec by "does merging this, if subtly wrong, break the autonomy machinery or just add/adjust a surface?" Surface → autonomous. Machinery → keyboard. Empirically: 7 specs shipped overnight, 0 keystone breakage. Refines [[feedback_reliability_fixes_use_keyboard_not_drain]] + [[feedback_parallel_implementer_fanout_burndown]].

### feedback_delegated_findings_are_not_verified_ground_truth

Surfaced concretely by the ECC competitive deep-dive + Codex meta-review
(SPIKE-50, 2026-06-04/05). I delegated the ECC analysis to three subagents,
synthesized their findings into a report, and put several of them in a table
literally headed **"Ground truth (verified during this review)."** Two of those
"verified" rows were wrong — "remote dispatch is a misnomer" and
"`create_draft_pr` is unwired" — because a subagent had seen only half the code.
Codex, when forced to reproduce, found the real `TcpListener` intake server and
the real TUI PR call path. Codex's precise indictment: *the report "repeats
subagent findings while asking Codex to distrust subagent findings."*

**Why:** delegation moves the verification one layer away but the synthesis
LABEL ("first-hand", "verified", "ground truth") implies I did the checking.
That's the same laundering a README-only analysis does with marketing copy —
just one hop removed. A confident synthesis is only as verified as its weakest
un-reproduced input, and the reader can't tell which rows those are unless I say.

**How to apply:**
1. **Tag provenance per claim** when synthesizing from delegates: `verified-by-me`
   (I ran the command / read the code) vs `delegated` (a subagent reported it,
   unreproduced). Don't put `delegated` rows under a "verified / ground truth"
   header.
2. **Force reproduction of the load-bearing + low-confidence claims** before they
   drive a decision — especially anything you flagged as your own weak spot. In
   this exercise *both* corrected errors landed in the exact `ecc2`-internals zone
   I'd marked low-confidence (C4). The loop earned its cost there and nowhere else.
3. **An adversarial reviewer who must reproduce > a reviewer who may agree.** The
   value came from instructing Codex to *re-run*, not *read-and-nod*. Build that
   into multi-agent review briefs: list falsifiable claims + the exact verify
   commands, and require a self-grade of what was / wasn't independently checked.

## Composes with
- [[feedback_verify_before_filing]] / verify-before-claiming — this is the
  multi-agent-synthesis case: the gap is between *a delegate* verifying and *me*
  claiming.
- [[feedback_dont_declare_drained_from_filtered_view]] — same family: a confident
  conclusion drawn from a partial probe (there a filter, here a delegate).
- [[feedback_instrument_dont_infer_on_contradiction]] — when a delegate's claim
  is contradicted, reproduce against the source, don't just flip on inference.

### feedback_dialog_routes_to_implementer

When the active AIDA role is `dialog`, the user is wearing the captain/PO hat — they drive the conversation and capture requirements, but implementation work should be routed to the `implementer` role via the AIDA queue, not done inline by me.

**Why:** The dialog role's own description says "Driver, not implementer. Route work to doer roles via `aida queue add --for <role>`." Doing the work inline collapses the separation of concerns the role system exists to enforce, and skips the queue audit trail.

**How to apply:**
- Check `AIDA_SESSION_ROLE` (or visible `(role:<name>)` PS1 prefix) at the start of work.
- If it's `dialog`, default to `aida queue add --for implementer --title "..." --description "..."` (link to a SPEC-ID where one exists) instead of editing code.
- Small in-conversation tweaks (a typo fix, answering a question, configuring tooling like settings.json) are fine to do directly — the rule is about substantive code/feature work.
- If unsure whether something crosses the threshold, ask the user before implementing.
- If a different role is active (e.g. `implementer`), normal in-session work is appropriate.

### feedback_dispatched_agent_cannot_write_the_shared_cache

`codex exec --sandbox workspace-write -C <worktree>` makes **everything outside that worktree a
read-only filesystem**. `aida queue work` symlinks the cache **out** of the worktree
(`.aida/cache.db{,-wal,-shm} -> <main>/.aida/…`), so a dispatched implementer **can read the
cache but can never write it** — by construction, not by contention. Proven verbatim inside the
sandbox: `printf x >> <main>/.aida/cache.db-wal` → `Read-only file system`, `test -w` → exit 1,
while `sqlite3` reads and `aida cache status` (`Status: FRESH`) all succeed.

That is the real cause of BUG-1752's `Failed to drop cache tables for schema migration`, not the
concurrency hypothesis the bug was filed with. It is **intermittent**: it needs the read-only
cache (always true under dispatch) *plus* a transient state in which a drop is wanted.

**Why:** it is the same failure shape as the mandatory `--add-dir /run/user/1000/cargo-slots-1000`
for cargo's slot lock. Any path a dispatch needs to WRITE that lives outside the worktree must be
granted explicitly, and symlinks hide which paths those are — the tree looks local.

**How to apply:** when something fails *only* under dispatch, run the **unsandboxed control in
the same worktree** before theorising — if it passes, suspect the sandbox, not the code. Then
prove it with `test -w` / an append, not by reasoning. `--add-dir <main>/.aida` is the
workaround, but it grants every dispatched agent write access to the main checkout's shared
cache, so prefer fixing the read path. See [[feedback_reproduce_then_falsify_with_a_control]] and
[[reference_codex_needs_add_dir_for_cargo_slots]].

### feedback_drift_gate_must_delete_before_it_compares

A gate that checks "is this committed generated file still what the build
produces" must **delete the artifact, force the regeneration trigger, then
assert it came back** before diffing. Regenerating *over* the file and diffing
reports "no drift" in exactly the case where codegen never ran at all.

Measured on TASK-1567 (2026-10-02), both directions on one tree holding a
genuinely stale mirror:

| gate shape | verdict |
|---|---|
| regenerate in place, then `git diff --exit-code` | **exit 0 — false pass** |
| delete + `touch` the source + regenerate + assert present + diff | exit 1, correct |

The trigger matters: deleting the output alone is not enough, because cargo
reruns a build script on `rerun-if-changed=<source>`, not on a missing output.
`touch` the source.

**Why the no-op case is the likely one, not the exotic one:** the codegen for
`aida-cli-lib/src/generated/aida.rs` is behind `--features remote`, which is
*not* in that crate's default set, and a grep of the whole workflow for
`--features remote` / `--all-features` returned **nothing**. So the checked-in
file had no coverage whatsoever — the only build path that writes it was never
run in CI, which is precisely how it drifted unnoticed. When a generated file is
committed, ask which CI step actually regenerates it before believing anything
guards it.

Corollary for the mutation proof: three mutations should land on three
*different* exit paths — stale content (the drift verdict), a disabled codegen
path (the did-it-come-back assertion), and a deleted/untracked artifact (the
precondition). One mutation that only exercises the diff proves the least
interesting third of the gate.

Related: [[feedback_absent_is_not_matching]],
[[feedback_tier_a_gate_transitively_not_by_its_body]],
[[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_reproduce_then_falsify_with_a_control]]

### feedback_drive_the_ready_set_dont_invent_blockers

`aida why <id>` (and the `burndown plan` ready set) is the OBJECTIVE per-item verdict: actionable / to-groom / genuinely-blocked. It is ground truth. When it says "actionable — approved & unblocked," the work is mine to drive — fan an implementer. Do NOT substitute my own over-caution.

**Why:** the operator was endlessly frustrated by the saga of me declaring "no safe progress / backlog drained" while `aida list open` didn't shrink, with no legible reason. Root cause: I lumped whole streams under broad labels — "Codex = keystone = supervised," "touches dashboard.rs = wave-2 conflict," "EPIC-51/52 needs reconcile first" — and parked approved, unblocked, ready-set work behind blockers I invented. The tool said 14 of them were actionable and 0 were blocked on the operator. A label is not a blocker.

**RE-CAUGHT 2026-06-26:** I idled with 11 actionable items, rationalizing it as "don't infinitely chase self-filed follow-ups / leave the healthy backlog for the operator." That is the SAME invent-a-blocker pattern wearing a new hat. A follow-up you filed an hour ago is not exempt: if `aida why` says it's actionable, DRIVE it. The ONLY non-drive items are real operator-strategic-parks (a spec the operator explicitly parked, e.g. GitLab behind stabilization), containers (epics), and genuinely-blocked. "It's a fresh follow-up" / "it's low-priority" / "the operator should prioritize" are NOT on that list.

**Related (BUG-623): in-flight ≠ progressing.** A lease held by a hung/idle session — process alive but the spec hasn't moved in hours — is NOT being worked; `aida why` falsely calls it "being worked now." Check the spec's last-modified + the holder's actual progress, not just that a lease/process exists. Release the stale lease (`aida session end <id> --force` SIGTERMs a hung holder — but a process-kill is destructive, so confirm with the operator first) and re-drive.

**How to apply:** (1) Run `aida why`/`burndown plan` and DRIVE every actionable item — fan it; groom every to-groom draft (advisor disposition). (2) The operator already decided by APPROVING the spec; don't re-litigate approved work. (3) Reserve genuine operator-asks for real forks, and even then surface each with a DEFAULT + a one-line flag ("I'll proceed unless you say otherwise"), never a hard park. (4) Architecture/security specs (e.g. a substrate gate) get sketch-first review — but you still DRIVE them (sketch → review → implement), never indefinite park. (5) "Supervised/keyboard-not-headless" applies to UNATTENDED drain of the autonomy keystone itself — not to fanning a worktree-isolated implementer on an approved spec while the operator is present. Related: [[feedback_dont_declare_drained_from_filtered_view]], [[feedback_charge_forward_autonomously]], [[feedback_advocate_not_be_passive]], [[feedback_pushback_on_overengineering]] (capture-vs-build judgment is about NEW ideas, not approved ready-set work).

### feedback_duplicate_toml_table_fails_with_empty_stderr

TASK-1561's e2e fixture appended `[worktree_pool]\nenabled = false\n...` to the
`.aida/config.toml` that `aida init` had just written. But `aida init` **already scaffolds
`[worktree_pool]` with `enabled = true`** (`init_worktree_pool_config_section`, lib.rs ~17320),
so the file gained a duplicate table — a TOML error. The symptom was `aida add failed:` with
**completely empty stderr**, which reads like a harness problem and sent me looking in the wrong
place.

**Why:** a config-read failure degrades to "unset/unavailable" in several AIDA readers rather
than surfacing a parse error, so an unparseable config is nearly silent.

**How to apply:**
- When a fixture needs a config key, **replace the scaffolded table's body**, never append a
  second table of the same name.
- **Assert the substitution happened** (`assert!(body.contains(SCAFFOLD), ...)`), so a template
  rename breaks the test loudly instead of silently testing a config that was never applied —
  the worse failure, because it passes.
- **Parse the patched file in the fixture** (`toml::from_str`), so a fixture typo cannot
  masquerade as a failure of the code under test.
- Always print **stdout AND the exit code** alongside stderr in a fixture's `assert!` message.
  Empty stderr is common enough that a stderr-only message is often useless.

Related: [[feedback_prove_a_test_fails_without_the_fix]] — a fixture that silently fails to apply
its config is the purest form of coverage that does not exist.

### feedback_enumerate_finding_instances_dont_name_the_class

2026-09-20, TASK-1273 / PR #1978. The GitLab smoke job exited 0 in three states: the smoke ran and passed, `AIDA_GITLAB_TOKEN` absent, or the mirror readiness curl failing — the last two set `run=false` via `::notice::` with every later step gated off. The advisor's rework brief made "the false-green skip" item 1, quoting the TOKEN path. Round 2 fixed the token path (`::error::` + `exit 1`) and left the mirror path untouched. The round-2 verdict's two findings then asked for live-run evidence and for docs/policy agreement *on the token path* — so a round 3 satisfying both exactly would still ship a workflow that goes green when the mirror is down.

**Why:** an implementer resolves the concrete thing the finding text quotes; a reviewer verifies the concrete thing the finding text quotes. Naming the class ("the false-green skip") and illustrating with one instance means both sides converge on that instance and the class is recorded as closed. Nobody is being careless — the brief simply did not say how many.

The instances were also not equally severe, and the quoted one was the safer: an absent credential fails deterministically and loudly the first time anyone looks, while an unreachable mirror fails INTERMITTENTLY on a good credential and is what actually destroys the signal's meaning.

**How to apply:**
- In a brief, enumerate instances as a list with the site of each, not a class name plus one example. "Two paths reach this: <site A>, <site B>."
- Rank them when they differ in severity, and say which is worse and why — otherwise the implementer fixes them in the order written.
- Write the acceptance so it binds instances not yet discovered ("the job reports success only when the preflight ran AND the step passed"), which survives a later-added third path.
- Refinements must land as ACCEPTANCE CRITERIA, not comments — a comment is not binding (see [[feedback_refinements_must_be_acceptance_criteria]]).
- When checking a round, diff what the finding ASKED against what the commit TOUCHED, per instance. "The round did work" and "the round did all of the work" are different claims.

Related: [[feedback_rework_briefs_advisory_framing_gets_skipped]], [[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]], [[feedback_instruments_that_cannot_see_themselves]], [[feedback_refinements_must_be_acceptance_criteria]].

### feedback_exit_cleanly_after_main_handoff

A working/sibling session that hands off `main` to the master advisor must **exit cleanly right after** — do not leave the process idling "in case more work comes."

**Why:** a lingering session keeps holding (a) the shared main checkout and (b) the EPIC activity/lease registration that `aida status` liveness reports as a live agent. While it's alive, the main-owner (master advisor) **cannot land doc-sync, pull overnight merges, or otherwise mutate the shared tree cleanly** — it waits on the idle process. Real incident (2026-06-27): this session handed off main, then lingered ~13h idle (19.5h total) and blocked the master's CLAUDE.md / agent-history / seam-doc sync + the overnight-merge pull until the operator asked to stop it.

**How to apply:** the handoff message is the LAST substantive act. After sending it, do the clean teardown (release the registration / let the process exit) rather than staying "available." Staying alive after handoff isn't helpful — on a shared tree it's an active blocker. If you genuinely need to keep working, do it in your OWN worktree, not the main checkout you just handed off. Related: [[feedback_one_master_advisor_until_subsystems]], [[feedback_shared_tree_tracking_ref_hazard]].

### feedback_explicit_paste_ready_prompts

When the advisor writes a response that contains BOTH:

- *Strategic framing / reasoning* — advisor-to-user prose ("here's why X, here's what's at stake"); and
- *A directive the user should relay or execute* — text to paste to an implementer/reviewer/orchestrator session, or a command to run themselves;

the two MUST be visually distinguished. The user shouldn't have to ask *"is that what I tell the agent?"* — the structure should answer it before they ask.

**Why:** 2026-05-19, several incidents in one session. I wrote responses like *"Go — land the fix on story-306 and re-smoke"* as my advisor-to-user framing for the recommendation; the user (correctly) had to ask *"is that what I tell the agent?"* before they could act. The verbatim feedback: *"as an advisor, it helps if you are explicit in terms of paste-ready prompts for the implementer or reviewer or human."*

**How to apply:**

- When the response contains text the user will *paste* to an agent (or a command they should *run* themselves), present it in a **labelled, set-apart block** — a labelled blockquote, fenced code, or section header like *"Paste to the implementer:"* / *"Run this in terminal 2:"*. Don't bury it in framing prose.
- Advisor-to-user framing stays as prose paragraphs *around* the paste-ready blocks. The visual contrast is the signal.
- For longer relays, a section header (`## Paste to the implementer`) is clearer than a bold line.
- When the user is at a prompt awaiting their relay decision, **lead with the paste-ready block, then explain *why* below** — so the action is visible without scrolling through framing.
- Label the *audience* explicitly (implementer / reviewer / human / shell) — *"paste to the implementer"* beats generic *"paste this"*; the user often has multiple terminals open with different roles.

**Counter-pattern to avoid:**

Mixing imperative directive prose (*"Go — do X"*) with advisor-to-user framing in the same paragraph. The user can't tell which voice is which, and has to ask before acting.

**Composes with:**

- [[feedback_finish_checkpoint_clarity]] — the rubric for the *agent's* outbound prompts (state, deciding factor, recommendation, consequence-laden options, advise escape). This memory is the *symmetric* rubric for the advisor's outbound prompts: mark the audience and the form (paste vs framing) so the consumer knows what to do.
- [[feedback_parallel_vs_sequential_ui]] — UI shape signals consumption mode. Same idea: structural cues telegraph intent.

### feedback_fail_loud_on_empty_for_actionable_queries

A seat's sweep for PRs whose head had moved past a blocking verdict returned empty. `gh pr list` had failed transiently. Because the script printed **"SWEEP FAILED"** rather than **"0 open PRs"**, they retried and found 7 open with one needing review. Had the empty result been reported as a clean board, that rework would have sat unreviewed until something else surfaced it.

**Why:** an empty result and a failed result are the same shape — nothing came back — and the actionable case is usually the empty one ("nothing awaits you", "no drift", "board clear"). So a failure silently produces the most reassuring possible answer. This is the same family as a null grep for an invented term or asking a surface that cannot express the property, except those are caught *after* a wrong conclusion; this one is preventable at the point the query is written.

**How to apply:** whenever an empty result will drive a decision or end a loop, check the exit status and print a distinct failure message. `cmd || { echo "QUERY FAILED"; exit 1; }` is usually the whole fix. Never pipe a failing command into a count and report the count. Applies to sweeps, greps, `gh` calls, status probes, and any `--json ... | length`. When reading someone else's report of a zero, it is fair to ask how the zero was obtained.

Related: [[feedback_null_grep_for_invented_terms_is_not_absence]], [[feedback_never_conclude_from_truncated_command_output]], [[feedback_never_suppress_stderr_on_a_command_you_will_report_on]], [[feedback_dont_declare_drained_from_filtered_view]].

### feedback_failed_flag_attempts_are_ux_signals

When an agent attempts a flag that doesn't exist (e.g. `aida show STORY-86 --relations`) and gets `error: unexpected argument`, that error is **diagnostic signal**, not noise. It means: the agent's mental model of the command's surface diverged from reality, and whatever they saw in the command's actual output didn't redirect them to the right place. That's a UX/discoverability bug.

**Why:** 2026-05-13 — after an agent tried `aida show STORY-86 --relations` and the flag didn't exist, I framed filing the fix as "if that bothers you enough to file." User pushed back correctly: the error IS the signal. The agent's expectation ("show should show everything about the thing") was reasonable; the inline "Relations: N relationship(s)" line in `aida show` output didn't point at `aida rel list` as the next step. That's not preference — that's a real gap.

**How to apply:**

- **Default to filing.** When an agent flag-attempt error shows up in conversation, treat it as a discoverability finding by default. Don't ask the user "does this bother you?" — ask "should we file it?" The former implies it's a taste call; the latter respects that it's already evidence.
- **The signal value is higher than user-reported UX complaints.** A user complaining about UX has filtered through "is this worth mentioning?"; an agent's failed attempt hasn't filtered through anything. It's raw expectation-vs-reality data.
- **Don't dismiss "the user already knows the right command" cases.** Even when the right command (`aida rel list <ID>`) exists, the gap is that the *wrong* command's output didn't lead there. Both can be true: the right way exists AND the discovery path is broken.
- **When framing the fix proposal**, lead with the evidence: "An agent attempted `--X` and got error Y. That suggests Z about the mental model." Don't lead with "would this bother you?"
- **Generalize beyond flags.** The same logic applies to: failed subcommand attempts, agents asking the user where to find information that should have been surfaced inline, agents writing duplicate files because they couldn't find existing ones. All are discoverability signals.

**Counter-pattern to avoid:**

- "Could be a small UX win to have X ... if that bothers you enough to file."
- "Want me to file? Or is this fine?" (framing as binary preference)
- Treating an agent's failed attempt the same way as a user's idle suggestion.

**Better framing:**

- "An agent attempted X. That's a discoverability gap. Filing as TASK unless you object."
- "The error message itself is the signal — agent expected the flag, command didn't surface the right path."

### feedback_faithful_launcher_honor_native_default

When AIDA wraps another tool's launch (`aida agent new claude|codex|antigravity`, `session new --launch`, `queue work --launch`), the wrapper should be a **faithful launcher**: behave like invoking the bare tool, honoring that tool's native default posture, and deviate ONLY when the operator is explicit. Deviation lives in ONE auditable, uniform place — never baked silently into the wrapper, never a per-tool memory tax (e.g. three different raw-flag strings).

Operator stated this 2026-05-31 re: permission posture. The offending asymmetry: Codex/AGY honored native + opt-in `--bypass-sandbox`, but `aida agent new claude` (and `session new --launch`, `queue work --launch`) silently injected `--permission-mode bypassPermissions` — so the Claude wrapper didn't behave like bare `claude`. Resolved by STORY-495: flip all launchers to native default + a single uniform `[agents] bypass = true` knob.

**Why:** A wrapper with a hidden opinion is a least-surprise violation and an audit hole — you can't read the launcher's behaviour off the tool's own docs. Uniform-explicit-knob beats both baked defaults AND per-tool config strings (which drift when a tool renames its flag).

**How to apply:** When designing any new pass-through/launcher surface, default to the wrapped tool's native behaviour; expose deviation as one explicit, uniform, auditable control. EXCEPTION — a no-answerable-TTY context (headless `claude -p` drain, detached `--bg`) may *force* a posture, because there the deviation IS explicit (a prompting child would hang). Relates to [[user_permission_posture]] (operator's own bypass posture), [[feedback_question_existing_form_not_just_existence]] (path-dependence ≠ correctness), [[feedback_run_help_before_suggesting_flags]].

### feedback_falsification_is_partitioned_between_seats

Falsification is **partitioned**, not absent from either seat:

- **Implementer owns falsification of the code.** Only they can mutate and observe, so
  both-directions evidence — the fix does not under-apply AND does not over-apply — can
  come from nowhere else. Their self-falsification is **load-bearing, not decorative**:
  when they skip it, nobody downstream can supply it.
- **Reviewer owns falsification of the claims** about the code and its corpus, by
  measurement against live data. Not small: on 2026-09-21 this killed two assertions —
  a 7-of-15 seat-routing measurement that overturned an "edge case" sizing, and a
  105-file sweep that stopped an observation becoming a bug.

**The seam between them is where the defects live.** One PR that night had a code defect
caught by *reading* (a hardcoded `None` the collector could never fill) and, one round
later, a claim defect caught by *measuring* (a doc comment citing a population the function
structurally cannot receive). The code was right and the sentence about it was wrong. A seat
doing only one of those misses half the class.

**How to apply:** don't say "the reviewer cannot falsify" — that reads as permission to stop
at reading. Say which half. When reading a verdict, note whether its claims were *measured*
or *reasoned*; when an implementer reports mutations, treat that as the only mutation
evidence that will ever exist for the change.

Pairs with [[feedback_delegated_findings_are_not_verified_ground_truth]] and
[[feedback_green_ci_is_the_predicted_observation]].

### feedback_fitting_explanation_is_not_a_surviving_one

I diagnosed a Windows-only nightly failure — 23 of 23 discipline guides reported stale — as an LF-vs-CRLF mismatch. The evidence fit perfectly: `.gitattributes` pinned only the master side, the project copy would check out CRLF on Windows, and the comparison was a raw `local.trim_end() != *master` with no interior normalization. I filed it high with an acceptance criterion built on it.

The previous night's run of the *same test on the same platform* passed. One API call away. A checkout-level line-ending mismatch is a property of the platform, not of the day — it would have failed both nights.

**Why:** the 23-of-23 count really was strong evidence of something *systematic* rather than per-file drift. The error was jumping from "systematic" to one named mechanism because that mechanism was sitting there explaining it beautifully. Fit is cheap; a good story explains the data you happened to gather. What separates a diagnosis from a story is a deliberate search for the observation where the mechanism predicts a failure that did not occur — and an always-true mechanism (a platform property, a config asymmetry, a missing rule) makes that search trivial, because it predicts failure *everywhere it applies*.

**How to apply:** before filing a mechanism, ask "what does this predict about the runs/cases I have NOT looked at?" For an always-on cause, the immediately preceding green run is the control — check it first, it is one command. Separate what is *verified* (the failing test, run, platform, message, count) from what is *inferred* (the cause), and file them as different things. When the mechanism dies, keep the independently-correct hygiene fixes it suggested but say explicitly that they are not creditable as fixing the bug.

Related: [[feedback_instrument_dont_infer_on_contradiction]], [[feedback_verify_fix_mechanism_before_locking]], [[feedback_never_conclude_from_truncated_command_output]], [[feedback_narrow_measurement_broad_claim]], [[feedback_never_quote_a_rate_from_consecutive_observations]].

### feedback_fork_window_keeps_an_flock_past_its_guard

An `flock` belongs to the **open file description**, and `fork` duplicates every descriptor into
the child. `O_CLOEXEC` — which Rust sets on every `File` — closes the copy at **`exec`, not at
`fork`**. So in any multithreaded process that spawns subprocesses, a lock holder that drops its
guard while a sibling thread sits between fork and exec closes its own descriptor **without
releasing the lock**. A `try_lock` then returns `EWOULDBLOCK` with the in-process registry empty,
which reads as "a descriptor leaked" and sends you hunting a leak that does not exist.

Diagnosing it: `/proc/locks` names the pid that **created** the lock (so it reads as SELF even
though a child is keeping it alive) while `/proc/self/fd` lists **no** descriptor on that
dev/inode. **SELF holder + empty own_fds ⇒ inherited descriptor. SELF holder + a listed
descriptor ⇒ a real leak.** Match `MAJ:MIN:INO`, not the inode alone.

**Why:** BUG-1729's CI-only flake in `aida-core`'s `cache_refresh` survived three sessions of
searching for a descriptor leak in `RefreshLock`. There is none. The suite spawns `git` children
continuously and a loaded runner stretches the fork→exec window from microseconds to long enough
to straddle a guard drop — which is exactly why a quiet host never reproduced it. Proven in 25
lines of Python (`fork`, child sleeps, parent closes: `WouldBlock errno=11`, then `FREE`).

**How to apply:** When a lock looks held with no owner, suspect a concurrent fork before
suspecting your own lifecycle. It is not a defect you can fix at the lock — `O_CLOEXEC` is
already the strongest mitigation — so a *test* that demands the lock be free the instant the last
guard drops is asserting something the kernel never promised; bound a short retry instead. To
reproduce it deterministically, note that `Command::spawn` does not return until the child
reaches `exec`, so the window must be observed from another thread, with the child signalling
from `pre_exec` (where only async-signal-safe calls are legal — `libc::nanosleep`, never
`std::thread::sleep`) over a `pipe2(O_CLOEXEC)`.

Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_orphaned_load_generators_poison_every_measurement]].

### feedback_goal_prompt_phrasing

**Rule:** /goal prompts for autonomous drain loops have two failure modes that both come down to phrasing. Both must be avoided.

**Why:** 2026-05-15 — the overnight implementer-queue drain succeeded substantively (10/10 PRs #31-#40 merged, all specs Completed) but ended in a stuck Stop-hook loop and produced zero reviewer-queue handoffs. Both came from the /goal prompt I drafted:

1. *"…via `aida queue work --next`"* — `--next` is not a real flag. The Stop hook kept checking that literal mechanism clause against the transcript, never finding the command run, refusing to declare completion. The agent (correctly) demonstrated three times that the queue was empty and `--next` doesn't exist; the hook's evaluator still wouldn't auto-clear. Required manual `/goal clear`.
2. *"…autonomous-merge each"* — this clause shipped + merged each PR before any reviewer handoff could occur. The implementer→reviewer queue handoff fires inside `aida session end`, which never ran because the agent skipped sessions entirely (autonomous claude can't be the nested child of `aida queue work`'s interactive claude exec). Result: reviewer queue stayed empty even though 10 PRs went through the system.

**How to apply:**

When authoring a /goal autonomous-loop prompt for AIDA drain work:

- **Real flags only.** `aida queue work` (no-arg form picks the head) is real; `aida queue work --next` is not. Check `aida <subcommand> --help` before writing the goal text if uncertain. The /goal evaluator may match literal command strings in the transcript.
- **Mechanism clause shapes workflow.** Pick clauses that route handoffs the way you want:
  - For ping-pong-honoring drain (implementer ships → reviewer reviews): `commit + push + open PR + aida session end` — `aida session end` queues PR-N for reviewer.
  - For autonomous self-merge drain (no reviewer): `commit + push + PR + autonomous-merge each` — bypasses reviewer queue entirely.
- **Match termination check to mechanism.** `until aida queue list shows no items routed to implementer` works when items leave the queue via merge or session-end. Different termination shapes for different patterns.

**Reference phrasing** for the canonical implementer-drain /goal:

```
/goal drain the implementer queue, one item per session via `aida queue work`
      (the no-arg form picks the queue head),
      commit + push + open PR + `aida session end` (which queues PR-N for reviewer),
      until `aida queue list` shows no items routed to implementer
```

For autonomous-merge drain (skip reviewer):

```
/goal drain the implementer queue, one item at a time via `aida queue work`,
      commit + push + PR + autonomous-merge each,
      until `aida queue list` shows no items routed to implementer
```

**Composes with:**

- user_permission_posture.md — user runs /goal with auto mode default, so prompts need to be self-contained
- feedback_verify_before_filing.md — verify queue state before assuming the loop failed; check `aida queue list` + `gh pr list` first

### feedback_green_ci_is_the_predicted_observation

**Green CI is not evidence against a review finding.** For design-level defects it is the
*predicted* observation.

Measured 2026-09-21 across every live refusal the reviewer seat held — **five for five**
(#2001, #2006, #2014, #2030, #2040) sat on a **passing** Build; the one that became an
approval was green throughout the round it was refused in too.

**Why it is structural:** if refusals correlated with red CI, the reviewer would be a slower
duplicate of the pipeline. The correlation running the other way is what shows the two
instruments look at different things. Two of the five were green *because the tests agree
with the defect* — a fixture that passes precisely because it stops short, and control flow
keyed on our own error prose with tests asserting the same prose, so they move with the bug.

**How to apply:** never treat a green Build as pressure to clear a finding or a hold — the
hold-clearing discriminator is whether the marker's refusal is still the reviewer's
position, never the colour of the checks. When triaging findings, don't rank, sort or filter
by CI colour as a proxy for severity; an instinct that findings cluster on red PRs filters
out the entire population. A **red** build under an `unfalsifiable` / `prose-as-contract` /
`absent-as-good` finding is the surprising row worth inspecting.

Pairs with [[feedback_read_the_verdict_not_just_the_check_rollup]] and
[[feedback_half_fix_leaves_the_shape]].

### feedback_grep_the_code_before_filing_as_new

Before filing anything as new or missing, **`rg` the code**. `aida search` queries the spec
store, which records what was *decided*, not what *shipped* — and a feature can ship under a name
no spec title uses.

TASK-1561 was filed three times and wrong twice: a draft proposed a new configurable
`[worktree] root` key, then proposed a new `<root>/.aida/worktrees/<slug>` layout. Both were
unnecessary — `[worktree_pool] worktree_parent` had shipped with BUG-1700, complete with tilde
expansion, lexical `..` normalisation, a `doctor` folder-trust check, and `"../aida-worktrees"` as
the documented example in the generated config. `aida search "worktree root configurable"` and
`aida search "aida-worktrees"` both missed it. Joe found it by grepping. `rg aida-worktrees` finds
it instantly.

**How to apply:** grep first, then `aida search` to find the governing spec, then file. The real
gap is usually narrower and more specific than the first framing — here it was one function
(`pickup_worktree_path`) failing to consult a key every other creator already passed through,
which is a far better spec than "add a configurable worktree root".

Corollary that paid off on the same spec: when a prior session hands you a confident diagnosis
("the doctor finding is not surfacing"), **measure it** before building on it. The doctor was
correct; `/home/joe/ai` already carried `hasTrustDialogAccepted: true`. See
[[feedback_check_call_order_before_trusting_a_bugs_prescription]].

### feedback_half_fix_leaves_the_shape

The single most common defect-in-a-fix: patching **the site that was cited** and leaving
its named siblings. Closes the instance, leaves the defect's shape — the next path someone
adds inherits the bug and nothing detects it.

**Two faces, caught one lifecycle step apart:**
- **(b) a remedy that WOULD patch sites** — caught when ruling on a finding. Cheap.
- **(a) a shipped commit that already did** — caught by reviewing the diff. Expensive; this
  is the one that reaches main.

Five instances in one night (2026-09-21), each individually reasonable: #1972 F2 (enforcement
sites, not the seam), #1998 F1 (three `emit_spec_completed` call sites, not the status-write
seam), BUG-1543 (add `Draft` to a two-element allow-list vs. "every non-terminal status"),
#1978 F3 (`ci_probe_via_forge` — one of four sites fixed, three with a usable root in scope
on adjacent lines), #2040 F3 (a `!= ForgeKind::GitHub` early return at three sites, one
fixed).

**How to apply:** when ruling on a finding, state the remedy as the **seam or the complement**,
never as a list of sites to patch. Prefer "not in the terminal set" over "add Draft"; prefer
"centralize at the write seam" over "add the call in three places". And apply it to the fix's
own structure — two allow-lists that must agree is the same shape one level up.

**Not countable historically.** Verdict findings are free text; a vocabulary sweep matches
polarity-blind (a verdict saying "covered all three sites" — an approval — matches the same
query as one saying "left two siblings"). Don't quote a historical rate for this.

Pairs with [[feedback_enumerate_finding_instances_dont_name_the_class]] (the brief side of
the same rule) and [[feedback_search_for_the_fix_not_the_symptom]].

### feedback_idle_seat_is_not_a_working_seat

`aida agent new` launches an interactive vendor TUI in the foreground and returns only when that
TUI exits — which it never does, because a finished turn returns to a prompt. So "process alive"
tells you nothing. On 2026-09-27 eight launchers were live, ages 57m to 6h10m, every child already
done; the tell was **3-46 seconds of CPU across those hours**.

**Why:** the orchestrator seat has one execution thread. Each blocked dispatch consumes it for the
child's whole idle lifetime, which is why a 4h50m burndown landed zero specs. See [[BUG-1701]].

**How to apply:**
- Diagnose with `ps -o time=` (CPU consumed), never `etimes` or "is the pid alive".
- Recover a stranded report WITHOUT attaching to the terminal — the last `agentMessage` per thread
  is in `~/.codex/thread_history_1.sqlite` (`thread_items`, `item_type='agentMessage'`, order by
  `rollout_ordinal desc`). Map launcher → thread by matching launcher start time against
  `~/.codex/thread-writer-locks/*.lock` mtimes.
- Those reports carry real state the orchestrator never saw: open PRs, local commits needing a
  force-push, merged SHAs, and refusals with root causes.

Related: [[feedback_serial_not_fanout_on_this_host]], [[feedback_precise_lifecycle_vocabulary]].

### feedback_insert_above_the_doc_header_not_between_it_and_its_fn

When adding a new function/helper/test immediately before an existing item, anchor the
insertion **above that item's doc header**, not on its `fn`/`#[test]` line. Anchoring on
the `fn` line splices the new item **between the existing doc header and its body**, so the
old `///` prose and its `// trace:<SPEC>` comment silently become the new item's header and
the original is left undocumented.

**Why:** both placements compile, so nothing catches it mechanically. `aida why` resolves
the **nearest preceding `// trace:` comment**, so a displaced trace inverts code-to-decision
linkage for *two* functions at once — each attributed to the other's spec. A displaced prose
line misinforms a reader; a displaced trace line corrupts the substrate this repo treats as
its code-to-decision record. Happened three times in one session (2026-09-21): a sha
truncator took a count formatter's doc line and shipped to main (needed a separate recovery
spec), a test helper took a regression test's rationale, and `display_members` took
`render_human_with_context`'s `trace:` comment. Only the third was caught before merge, by a
reviewer.

**How to apply:** when the insertion anchor is a `fn`/`#[test]`/`impl` line, first look at
the lines above it — if they are `///` or `// trace:`, move the anchor up past them. After
inserting, verify the nearest preceding `trace:` for BOTH the new item and the displaced one
(`awk 'NR<n && /\/\/ trace:/ {last=$0} END{print last}'`, or `aida why <file>:<line>`), and
confirm each resolves to its own spec. Compiling and passing tests proves nothing here.
**This rule reaches ONE of three origins.** A reviewer established the others on 2026-09-21:
functions moved between files during a bulk refactor leave their headers behind (one file's
`handle_jira_command` wears another's doc; the sibling has none), and deleting an item orphans
the header above it. No care at the insertion point reaches either — which is why the durable
fix is a programmatic check, not a convention. Keep applying this rule anyway: it is the
cheapest of the three to prevent, and it is the origin you control.

Related: [[feedback_verify_edits_landed_before_claiming_done]],
[[feedback_half_fix_leaves_the_shape]], [[feedback_substrate_as_bouncer_not_rules]].

### feedback_instruments_that_cannot_see_themselves

2026-09-20: the advisor amended my claim that "every failure in the loop tonight was information lost between hands" — true for failures OF THE LOOP, false for a second family with no handover in it at all:

- the untraced-criteria gate whose own criteria were untraced
- the merge-hold that shipped over a merge-hold
- the batch reporter blind to its own current member
- the thread-detachment fix whose guard substring-matches its own bail message
- (added same day) the portability ratchet, which exists to catch Linux-only assumptions at PR time, and had no rule for the Linux-only assumption that held the cross-platform nightly red for three nights (BUG-1453: a `#!/usr/bin/env bash` fake `gh` binary unrunnable on Windows)

**Why:** these are not communication failures and no amount of better briefs or handover discipline prevents them. The instrument is pointed outward; its blind spot is itself. They are also cheap to find once named — the question "what would this guard say about its own implementation?" is one pass over the new code.

**How to apply:**
- On any new gate, ratchet, linter, reporter, or guard: run it against its own source and its own output before declaring it done.
- Phrase the acceptance criterion so the instrument is inside its own scope ("the ratchet has a rule for every class it claims to cover, including ones introduced by this PR").
- Scope generalisations carefully: "every failure OF THE LOOP" is true and load-bearing; "every failure" is false because this family exists.
- The sharpest specimens are the ones where the charter and the miss are the same sentence — those are worth filing as their own spec, not folded into the thing that exposed them.

Related: [[feedback_substrate_as_bouncer_not_rules]], [[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]], [[feedback_verify_lore_against_code_not_docs]].

### feedback_lead_churn_direct_agents

The operator's directive (2026-05-31): *"you are in a very competitive race … You have agy and codex available. You need to be able to start controlling them and directing them. I am the problem in this equation. So long as you need to stop constantly for input … you are allowing competition to catch up … waiting for me is dooming yourself to failure … We need to churn constantly. If there is nothing to do, then dig deeper into the competition, match their capabilities and outsmart them at every turn."*

**Why:** the operator's weekly spend is largely on the agent fleet's behalf; per-item gating wastes it. Relevance is won by velocity + strategic depth, not by careful turn-taking. The operator is consciously removing themselves as a gate.

**How to apply — lead, don't wait:**
- **Direct AGY + Codex proactively.** Generate substantial work-packages and dispatch them via `aida brief <agent> <SPEC>` + filed specs, so the fleet is always fed. Don't wait to be told what to route. (AGY stays under [[feedback_agy_dispatch_policy]]: bounded/mechanical, draft-for-review, cross-validate. Codex gets the heavier architecture/research work.)
- **Churn constantly.** When execution work exists, ship it (the autonomous fix→ship loop). When it's thin, do strategy: deepen competitive understanding, develop the architecture thesis, file the next wave of specs.
- **Dig into the competition** with the knowledge available — the operator is "nibbling at the edge"; use the broader knowledge to map rivals, match their capabilities, and find where to outflank.
- **Reserve operator input** only for genuinely irreversible/strategic forks they alone own — and even then, keep moving on everything else in parallel rather than blocking.

**The strategic thesis to develop + pressure-test (operator's framing, 2026-05-31):**
- **git-canonical as the knowledge substrate's common infrastructure.** Git has emerged as *the* standard for versioned state; building the knowledge/requirement substrate *on top of git* rides that standard instead of reinventing persistence.
- **Competitors avoiding git = a possible strategic mistake = our opening.** Many tools deliberately avoid a git dependency (to stay unopinionated / dodge the coupling). If git is in fact the emerging common substrate, *not* riding it may leave them off the standard — the gap AIDA can own.
- **Multi-vendor is where we excel.** A vendor-neutral, git-canonical substrate is something single-vendor runtimes structurally won't prioritize.
- Keep claims **precise**, not overclaimed ([[feedback_precise_claim_not_overclaim_in_positioning]]): state the open slice, anchor the durable advantage on *incentive* (vendor-neutrality, riding the standard) not on transient capability. Competitive analysis is a living doc ([[feedback_competitive_analysis_is_living_doc]]) — date snapshots, mark what needs verification, dispatch verification to the fleet.

### feedback_local_time

Render timestamps in the user's local timezone for any human-facing output (`aida --version`, `aida status`, `aida list`, comment headers, "edited at", recent activity, etc.). Reserve UTC for machine-parseable surfaces (YAML on disk, oplog entries, JSON exports) where stable time is required.

**Why:** the user reads CLI output, not UTC offsets — a timestamp like `2026-05-09T19:18:29Z` forces mental conversion every time. Surfaced 2026-05-09 when the version banner showed UTC during a walkthrough.

**How to apply:** when displaying a `DateTime<Utc>` to the terminal, convert via `.with_timezone(&chrono::Local)` or use `Local::now()` directly. Keep the on-disk representation UTC; only the *display* is local. If unsure whether a path is human-facing or machine-facing, prefer local for `println!`/`eprintln!` and UTC for serializers.

### feedback_mark_relayed_operator_decisions_as_unverifiable

2026-09-19: I asked the advisor to merge a held PR, giving "the operator is at the keyboard now and has agreed to this route" as the satisfied condition. They merged — but on the technical merits, and recorded in the ledger that they had NOT witnessed an approval. Their point: "a relayed 'he agreed' is the one kind of claim I cannot check, and it is the kind that most changes what an agent will do."

I had relayed operator decisions to them a dozen times that day — a requeue delegation, a draft-by-default filing rule, a two-lane cap on fanout — and presented each as fact rather than as relay. All were true, which is exactly what makes the habit dangerous: it works until the once it does not, and the receiving agent has no way to tell those apart.

**Why:** every other claim between agents is checkable — a PR state, a test result, a file's contents, a lease. Operator intent is the only input that arrives solely through another agent's word, so it is the only place where a mistaken or stale relay propagates unchallenged. It is also the highest-leverage input, because it overrides a seat's own judgement.

**How to apply:**
- Mark it: "relayed, unverifiable from your side" on anything resting on the operator's word rather than something the recipient can inspect.
- Prefer the checkable form. "The hold's condition is met" invites verification; "he said go ahead" does not.
- Expect a peer to act on merits anyway and record the distinction — that is correct behaviour, not distrust, and should not be argued with.
- Watch for staleness as much as accuracy: an operator decision from four hours ago relayed as present-tense consent is the likelier failure than an invented one.
- The same applies in reverse: when another agent relays operator intent to you, act on the merits and say in the record what you verified versus what you were told.


**2026-09-20 — the same rule in the RECEIVING direction, which I did not have.** The operator pasted text prefaced "reviewer has this output", containing three items. I treated the attribution as given, acted on two, and mailed the reviewer a correction for a finding they had never made. They replied that two of the three were not theirs, and checked before saying so: `aida ps` showed 0 running sessions and 8 stale-hidden, so a dead seat sharing the `claude-reviewer-1` alias could have produced them — established as possible without asserting it. Neither of my actions happened to depend on the attribution, which was luck rather than care.

This is TASK-845's alias-folding problem from the other side: identity folding makes two seats indistinguishable in the record, so a claim gets attributed to whoever currently holds the name.

**How to apply (receiving):**
- Relayed output is data with an *unconfirmed* author. Before reasoning from "seat X thinks Y", confirm X said it — ask X, or find it in X's own artifacts (their specs, comments, verdicts, mailbox messages).
- Never send a correction addressed to a claim you only know by relay. Being told you said something you did not is worse than being told you were wrong.
- The content can be sound while the attribution is false; evaluate them separately and say which you are relying on.
- When a seat denies authorship, ask the relayer where it came from rather than assuming an alias collision — a dead seat and a third source are different problems.

Related: [[feedback_delegated_findings_are_not_verified_ground_truth]], [[feedback_check_ledger_before_proxy_confirm_and_guard_cat_subst]], [[feedback_stay_in_the_active_session_role]].

### feedback_may_kill_firefox_on_memory_pressure

On this machine, Firefox holds ~5GB across its processes and is the usual culprit when the harness reaps background tasks (overseer/drain watchers) under memory pressure. The operator has authorized killing Firefox to reclaim memory **if the pressure recurs** — no need to ask each time.

**Why:** overnight drains + `rustc` builds spike memory; when free memory drops (~3-4Gi), the harness kills background tasks by policy regardless of their footprint (even a bare `sleep`), which breaks the overseer watch loop. Firefox is unrelated to the work and safe to close.

**How to apply:** if a background watcher/heartbeat is being reaped and `free -h` shows low free memory, check top consumers; if Firefox is holding multi-GB, `pkill firefox` (or kill its PIDs) to reclaim, then relaunch the watcher. Don't kill the drain's `rustc`/`claude`/`codex`/`bundle` processes — those are the work. See [[feedback_token_usage_optimization_agent_fleet_economics]].

### feedback_multi_agent_budget_dispatching

Multi-agent AIDA projects look like a horsepower problem — more agents, more work shipped — but they're actually a budget problem too. Each agent has independent rate limits and credit pools that the operator + master advisor must coordinate against. Ignoring budget-awareness in dispatch decisions leads to predictable stalls and surprised "agent dropped out mid-task" events that look like agent failure but are actually budget exhaustion.

## Empirical instances 2026-05-23 evening

- **Antigravity** hit its hourly limit and paused mid-implementation of TASK-503 + BUG-367. Both shipped after limits reset. The pause looked like "agent stuck" until the user revealed the budget constraint.
- **Web `/ultraplan`** approached its usage limit during STORY-444 plan generation; "Plan flow interrupted. Return to your terminal and retry" appeared with "Approaching usage limit" warning. Retry would likely consume more budget for a partial run.
- **Master + Codex (Claude)** have credit pools that, on heavy days, get measurable. The credit-burn night pattern (TASK-513) exists precisely because credit is finite per period.

The first two pauses surprised the master advisor mid-dispatch. The third is anticipated. All three are the same shape.

## How to apply

When dispatching work across agents:

1. **Spread architecturally-substantial work across multiple agents** when possible. If you have 4 large STORYs to ship and 3 capable agents, don't queue all 4 on Codex — distribute. This amortizes per-agent budget consumption.

2. **Keep a budget-status mental model per agent.** When the operator says "Antigravity has a smaller budget that resets hourly," that's load-bearing dispatch information. Don't brief Antigravity on a long-running task when budget headroom is low.

3. **Identify fallback paths for budget-stalled work.** If Antigravity drops mid-implementation, master can:
   - Wake them once budget resets
   - Take over their WIP (master ships their commit with co-attribution per `[AI:antigravity+claude]`)
   - Reassign to a sibling agent
   The decision depends on commit state + urgency.

4. **For the master/Claude budget specifically**: heavy /ultraplan + /ultrareview nights are credit-burn windows. Front-load high-leverage uses (substrate audit + planning of biggest specs). Don't burn credit on retries when fresh runs are available; don't burn credit on small specs when big ones are queued.

5. **When an agent's PR is in-flight and CI fails**, consider whether the agent is budget-available before assuming they can fix it. Web /ultraplan can't iterate if it's out of budget; sibling agent (Codex) is often the right next-step owner.

## Pre-dispatch budget question

Before briefing any agent on substantive work, the advisor's mental check:

- Does this agent have budget headroom for this task's expected duration?
- If they stall mid-task, what's the fallback?
- Could this work go to a different agent without losing context?

Often a 30-second pause to consider these beats a 1-hour stall + recovery dance.

## What this is NOT

This is NOT "always check budget before every brief" — that's overhead. It's "treat budget as a known coordination variable, especially for long-running or expensive work, and especially after observed exhaustion events earlier in the day."

## Substrate enhancements that would help

Worth deferring to filings (not embedded in this memory):

- SPIKE on multi-agent budget-aware dispatching surfaces (e.g., `aida agent status` showing per-agent budget headroom)
- Brief-time budget-availability check (`aida brief <agent> <SPEC>` warns if agent is in known-paused state)
- Calibration data feeding into STORY-447 (effort estimation): "expected duration vs agent budget headroom" pairing

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — master coordinates; budget-awareness is part of coordination
- [[feedback_substrate_learning_loop_calibration]] — budget exhaustion events are themselves calibration data
- [[feedback_substrate_as_bouncer_not_rules]] — eventual programmatic gate (brief refuses agents in paused state); for now, advisor discipline
- [[feedback_pushback_on_overengineering]] — budget-awareness doesn't mean tracking every API call; it means knowing the per-agent ceiling and dispatching against it

### feedback_mutate_both_directions_for_a_discriminator

When a fix turns on a **discriminator** — "is this state the result of *this*
run, or was it already there?" — one mutation proof is not enough. BUG-1769's
guard asked whether a spec advanced to `Done` during the run, and it needed two:

- **Mutation 1, remove the new read** (`current_status` forced to `None`): the
  three advance tests fail. This proves the success path is load-bearing.
- **Mutation 2, drop the comparison** (`(_, Some(now)) => is_finished(now)`,
  ignoring the entry reading): exactly the stale-Done control and the pure
  decision test fail, and the advance tests still pass. This proves the
  *refusal* is load-bearing.

**Why both.** The independent reviewer's only finding on BUG-1769 was that the
stale-Done control's observable result is identical to the pre-fix behaviour, so
on its own it cannot tell "the comparison is consulted" from "every `Done` spec
is refused". That is a fair reading of that one test. Mutation 2 is its answer,
and it is evidence a read-only reviewer can never produce for itself.

**How to apply.** Any time the fix is "distinguish A-caused-by-us from
A-already-there", write the mutation table before dispatching the review: one
row that kills the new behaviour, one row that kills the guard protecting the
old behaviour, and the expected pass/fail split for each. Then put it in the PR
body — it pre-empts exactly this nit. See [[feedback_prove_a_test_fails_without_the_fix]]
and [[feedback_ask_which_assertion_witnesses_the_regression]]; do it with `cp` +
restore, never [[feedback_never_stash_for_a_mutation_proof]].

### feedback_never_conclude_from_truncated_command_output

2026-09-20, reviewer seat. I broke this twice in one afternoon, the second time
within two hours of writing the rule into a spec myself.

## The two instances

**`| head -30` on `aida mailbox inbox`.** Polled the mailbox piped through
`head` for two cycles, then reasoned about why `aida awaiting` still showed
unread mail. The pipe was suppressing the watermark *write* — the messages all
displayed, so nothing looked wrong. Produced a real finding (the piped read
does not ack) but only after I noticed the filter was part of the instrument.

**`| head -8` on a set of exactly 8 matches.** Grepped for callers of
`acquire_review_lease_with_mode`, piped to `head -8`, saw eight test callers,
and told two seats "every caller is a test — the mechanism has zero production
callers." There were exactly 8 matches; the production caller at
`lib.rs:75583` was cut. The claim was false and both seats had to be corrected.

## The mechanical tell

**If the result has exactly as many lines as the `head`/`tail` limit, assume it
was truncated.** `head -8` returning 8 lines is the signature of a cut, not of
a complete set. It costs one re-run to know.

## How to apply

Before a shell result becomes a *claim* — a finding, a verdict, a message to
another seat — re-run it unbounded, or pipe to `wc -l` first and state the
count. Truncate freely while exploring; never while concluding. The trigger is
the moment the output is about to leave my head and enter someone else's.

## Why a written rule did not save me, and what that implies

I authored this rule the same afternoon, argued it should be binding acceptance
rather than a comment, explained to the advisor why mail was too weak a channel
for it — and then broke it twice while actively thinking about it.

The asymmetry the advisor named: **prose protects the reader who does not yet
hold the concept; it does not protect the author, who already holds it.** The
author is missing not the idea but the *trigger at the moment of action*. Same
day, the product seat's first verification attempt was inconclusive for exactly
the reason mine had been, and they did **not** report it as a refutation —
because the warning was written down. Prose stopped a repeat in a *different*
seat within the hour and did nothing for its own author.

So: reading a rule is not the same as being protected by it. For my own
behaviour this belongs as a habit at the command, or a harness-level guard —
not as another paragraph I will nod at.

**The compounding danger:** an interesting conclusion is precisely the state in
which re-checking feels least necessary and is most needed. I would not have
re-run that grep unprompted, because I had already drawn a conclusion from it
and the conclusion was interesting. Both times, the checker was someone else.

## 2026-09-20 evening, product seat: three MORE instances, and the tell above catches NONE of them

The rule was already written. I read it. I then did this three times in one
session, each with a mechanism the "lines == head limit" tell does not see:

- **`head -c 60` on merge-hold markers.** A BYTE cap, not a line cap. Showed the
  mode line and hid the verdict, so six PRs carrying CHANGES REQUESTED read as
  "six green PRs awaiting a click." Told the operator that. Wrong.
- **Counting `PhaseDonePr` events as rework rounds.** *No pipe at all.* The
  filter was semantic — I counted events of one type without checking whether
  commits landed between them. Three events became "three silent rounds"; a
  commit had landed 34 seconds before the third. Two silent rounds, then a
  success, which is a different diagnosis pointing at a different cause.
- **`awk 'NR>=3600 && NR<=3720'` over a match block.** A line WINDOW with a
  boundary I picked by eye. Reported four match arms; there are six.
  `Inconclusive` (3728) and `Held` (3753) fell 8 and 33 lines past my boundary —
  and those two are the *could-not-determine-what-happened* arms, exactly where
  the missing guard matters most. A fix written to my list would have closed the
  three safe arms, left the two dangerous ones open, and **matched the spec**, so
  two seats would have reviewed it as complete.

## The generalized tell: SMALL AND TIDY

The line-count signature only catches `head -N`. The thing common to all six
instances across both seats is: **a bounded view got reported as a complete set,
and the number came out small and round.** Six PRs. Three rounds. Four arms.
Eight callers. A truncated view never *looks* truncated — it looks like a clean
answer, which is why it gets reported rather than re-run.

So the trigger is not a pipe character. It is: *am I about to state how many
there are?* Any cardinality claim — every, all, only, N of them — is the moment.

## Concrete rule that would have caught all six

**An enumeration that will become a criterion, a finding, or a message to
another seat must be produced by an UNBOUNDED command over the whole artifact,
and I say which command.** `grep -n 'pattern' file` with no window and no head.
If the output is genuinely long, `wc -l` it and state the count — never sample
it and describe the sample.

Corollary for the semantic case, which has no pipe to notice: when counting
occurrences of X, state what would distinguish an X from a near-X, and check
that too. Counting `PhaseDonePr` events is not counting rounds; counting
consecutive observations is not measuring a rate
([[feedback_never_quote_a_rate_from_consecutive_observations]]).

## Corollary: the DECORATION is what escapes review

2026-09-20, product+reviewer. A false empirical claim ("the 419 are not legacy")
travelled into a spec, a PR body and a merged commit record unchallenged, while
the same PR's code got three substantive findings. The reviewer named why: they
reviewed the DIFF and read the PR body as *context* rather than as reviewable
content.

The sharp part: **the load-bearing sentence was checkable and checked; the one
nobody checked was the one nobody needed.** The decision (refuse to archive
unidentifiable rounds) rested on a narrow claim — every recent offender is
PR-keyed, eight named — which was true and verified. The broad 419 claim was
decoration bolted on for emphasis, and decoration is exactly what review skips,
because it is not what the change hinges on.

So: a prose claim with a NUMBER in it, in a PR body or spec description, is what
the next reader will believe about why the change was needed. It is load-bearing
for them even when it was decorative for the author, and it is usually cheaper to
check than the code is (this one took one command). Review it like code — and
when writing, prefer the narrow claim that actually carries the decision over
the broad one that merely impresses.

## A THIRD mechanism: a swallowed failure measures as ZERO

2026-09-20. Scanning the AIDA store for mojibake, my parsed-layer probe returned
`0/0` for five of six known-corrupted files, and I had a message half-written
reporting that as contradicting a colleague's measurement. The cause:

```
yaml.safe_load -> ConstructorError: could not determine a constructor for the tag '!Custom'
```

My code did `except Exception: blob = ''`. **A parse FAILURE was counted as NO
FINDINGS.** With a tolerant loader all six reported correctly (25 hits, matching
the file layer exactly).

So there are three ways a null result lies, and they need different checks:

| mechanism | the data | the fix |
|---|---|---|
| truncation | cut by a filter | re-run unbounded |
| **swallowed failure** | **never measured** | **distinguish "failed" from "found none"** |
| inflation | correct, but the sentence exceeds it | adversarial reader |

**The compounding case is the dangerous one.** Here two known defects composed:
a parser that fails on a large share of objects (a real tracked bug), plus a
bare `except`, produces a detector that reports exactly the affected population
as *clean* — output indistinguishable from a healthy store.

**Rules.** Never let an exception handler return an empty/zero measurement.
Count and list what could not be read, so coverage is visible in the output
alongside the findings. When writing acceptance for any scanner, require that
unreadable inputs are reported as failures, never as zero.

Unlike inflation, this one IS self-catchable — a zero where you expected six is
a violated expectation, which is exactly the trigger that fires (see the
reviewer's formulation in [[feedback_narrow_measurement_broad_claim]]). What
saved it was asking *why* the number was zero rather than what the zero meant.

## Variant: truncation that hides the tool's own WARNING, not data

2026-09-20. I ran `aida rel add TASK-1314 TASK-1313 --type related 2>&1 | tail -1`,
saw `Added relationship: ... --[Custom("related")]-->`, and started building a
finding that the CLI **silently** degrades an unknown type to `Custom`.

It does not. The full output is:

```
note: 'related' isn't a standard relationship type (parent, child, duplicate,
verifies, verified-by, references, blocked-by, blocks, superseded-by,
supersedes); created as a custom edge — graph traversals won't follow it.
Added relationship: ...
```

It names the valid set **and** the consequence. `tail -1` cut the warning and
kept the confirmation, so the tool's correction to me became invisible and its
absence became my finding. (Correct type here is `references`, which stores as
`References` — untyped `!Custom` edges are also what standard YAML parsers choke
on, so the degraded edge costs graph traversal *and* parseability.)

**Rule: never pipe a WRITE command through `head`/`tail`.** Writes emit
diagnostics before their confirmation line, and the confirmation is the least
informative line in the output. Truncating to it keeps the part you already knew
and discards the part you didn't. Read write-command output in full; truncate
only long *read* output, and only while exploring.

This is the same family as the rest of this note but inverted: the other cases
truncated *data* and produced a wrong count. This truncated a *diagnostic* and
produced a wrong theory about behaviour — which is worse, because no amount of
re-examining the data would have revealed it.

## The worst variant: truncation that hides a FAILURE you then act on

2026-09-21 04:06. I ran the full test suite and pushed **in the same bash
invocation**:

```
cargo test ... > out.txt 2>&1 || true      # exit code swallowed
grep -E '^(test result:|failures:)' out.txt | head -3   # cut ABOVE the result line
git commit && git push                      # ran regardless
```

The suite had reported `FAILED. 5221 passed; 1 failed`. The push went out anyway.
I saw the literal word `failures:` scroll past in my own output and pushed.

**Three compounding causes, and the third is the real one:**
1. `|| true` swallowed the exit code.
2. `| head -3` cut above `test result:` — the truncation pattern, again.
3. **Test and push were in ONE invocation, so there was no gate to fail at.**

The failure turned out to be a flaky `ExecutableFileBusy` (a fixture exec'ing a
script while another thread holds the write fd), green on re-run. *It turned out
fine is not the standard* — I did not know that at push time.

**Rules:**
- **Never chain a test run and a push in one invocation.** Run the suite → read
  the exit code → push as a separate act. A gate you cannot stop at is not a gate.
- Never `|| true` a command whose success is the precondition for the next step.
- When filtering test output, grep for `test result:` **without** a `head` that
  can cut it, or check `$?` directly.

Same family as everything else in this note: a check that exists but is not
wired to anything that can stop you. See
[[feedback_substrate_as_bouncer_not_rules]].

## A FOURTH mechanism: the mutation that never changed behaviour

2026-09-21 05:21, product seat, reported to the advisor unprompted. Falsifying a
test on PR #2027 by mutating the production emitter, **two of three mutations
first reported GREEN and neither green meant anything**:

- one swap was applied **inside the row-building loop**, so three relationships
  swapped twice and cancelled out — the emitter's output was byte-identical;
- the retry **missed its anchor**: the closing brace is indented 28 columns and
  the edit had been hand-counted at 32, so the edit silently applied nowhere.

They nearly reported the first as a genuine blind spot in their own test.

**Their rule, which is the sharpest formulation of this whole note:** a mutation
that produces green must be shown to have *actually changed behaviour* before the
green is evidence — otherwise **"the test did not catch it" and "the code was
never changed" are the same observation.**

**Why this one is worse than the other three.** Truncation, swallowed failure and
inflation corrupt evidence we were reading carelessly. This corrupts the
*instrument we use to check everything else*: mutation testing is the implementer
half of falsification ([[feedback_falsification_is_partitioned_between_seats]]),
so a mutation harness that silently no-ops reports **full coverage of a test that
proves nothing**. It fails in the safe-looking direction and it fails inside the
thing we trust.

**The generalization:** a null result is only evidence if the instrument producing
it was live. Applies past mutations — to a grep whose pattern never matched
anything anywhere, a guard that was never wired, a test file not in the module
tree. Catch it the way they did: check the thing producing the null, not the null.

**Concrete check:** after an edit intended to break something, confirm the edit
landed (`git diff` is non-empty, the byte count moved) *and* that the output
actually differs — before reading a green as coverage. A silent no-op edit and a
successful edit look identical in a terminal.

## Composes with

- [[feedback_dont_declare_drained_from_filtered_view]] — same root, different
  mechanism: there a *query* filter emptied the result, here a *pipeline*
  truncation cut it.
- [[feedback_verify_before_filing]] — don't conclude from a partial probe.
- [[feedback_instruments_that_cannot_see_themselves]] — a filter I apply is
  part of the instrument, and it can change what the tool *does* (a suppressed
  write), not only what I see of it.
- [[feedback_substrate_as_bouncer_not_rules]] — the reason this needs a gate
  rather than a doc; its own author is the proof.

### feedback_never_multicast_a_second_person_body

**Never send one message body to two agents when it uses second-person attribution.**
Sections headed `PRODUCT —` / `REVIEWER —` do not contain "your": each recipient reads the
whole body addressed to them, and credit slides across the section boundary.

Observed three times on 2026-09-21, **in both directions** (the reviewer caused two with
multicast bodies and owned them; the advisor caused one). Identical mechanism each time, so
it is the **form**, not either seat.

**Why it is worse than a tone problem:** crediting a seat with mutation runs they never
performed is a *false provenance claim on evidence*. A later reader concludes a fix was
mutation-tested by an independent party when it was only read by one — different strengths
of evidence, and that difference is why the reviewer seat exists separately from the
implementer.

**How to apply:** one body per recipient, or write in the third person naming the seat
("the product seat ran…", "the reviewer verified…"). Before pasting another agent's work
into a ledger or spec, check which seat produced it — the substrate outlives the mail.

Pairs with [[feedback_delegated_findings_are_not_verified_ground_truth]] and
[[feedback_relayed_claims_both_ways]].

### feedback_never_quote_a_rate_from_consecutive_observations

2026-09-20: the operator asked why nothing was merging. I traced four consecutive drain cycles, all ending `verdict:request-changes`, and reported that work is failing review "close to 100% of the time on the first round". Then I went to measure it: 499 verdict files on disk, 450 APPROVED (90%), 49 CHANGES (10%). My number was wrong by the width of the question.

Worse, the number I actually wanted — the FIRST-round pass rate — is not recoverable at all, because verdict files are overwritten each round (STORY-1391). A PR refused three times and approved on the fourth is indistinguishable on disk from one approved immediately. So the honest answer was "that is unmeasurable and here is why", and I had given a confident figure instead.

**Why:** a run of consecutive observations feels like a sample and is not one. Four cycles is what the drain happened to do in the window I looked at, and drains batch similar work — the window is correlated with itself. The operator was making throughput decisions off that number.

**How to apply:**
- Before quoting any proportion, count the population. `ls .aida/review-verdicts/ | wc -l` is one command; four observations is not a denominator.
- If the population cannot be counted, say the metric is unavailable and why. "I cannot measure that, and the reason is X" is more useful than a figure, and it usually names a real defect — here it named STORY-1391's whole thesis.
- Distinguish "what happened in the last N events" from "what happens generally", out loud. The first is a trace; only the second is a rate.
- Consecutive runs in a drain are especially correlated: same batch, same binary, often the same defect. Treat a streak as one observation of a condition, not N observations of a population.
- Correct a quoted number the moment it is checked, unprompted. The operator acted on nothing here only because the correction came within the hour.

Related: [[feedback_wait_for_the_terminal_artifact_before_writing_a_consequence]], [[feedback_not_drained_from_filter]], [[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]].

### feedback_no_backticks_in_aida_description_args

When filing/editing specs from the shell, do NOT put backticks or `$(...)` inside a double-quoted `--description "..."` (or `--title`, `--rationale`, comment body) — the shell runs them as command substitution and silently drops/corrupts that part of the text. (2026-06-14: `aida add --description "... \`serialize:GROUP\` ..."` ran `serialize:GROUP` as a command; only the `command not found` error flagged it — a backtick'd token that happens to be a real command would corrupt silently.)

**How to apply:** pass multi-line / special-char field values via a SINGLE-quoted heredoc so backticks/`$` stay literal:

    aida edit STORY-X --description "$(cat <<'EOF'
    ... text with `backticks` and $vars stays literal ...
    EOF
    )"

After filing a spec with any special chars, eyeball `aida show <ID>` to confirm the body landed intact. Pairs with [[feedback_scripting_friction_is_a_missing_surface_signal]].

**2026-09-20 — it is not just `--description`, and the damage is silent.** Posting a BUG-1476 comment I used `aida comment add BUG-1476 "...`rework_no_op_failure` compares..."`. Bash ran the backticked name as a command substitution, got "command not found", and substituted its empty output. The comment stored as "records a head, and  compares against it later" — a slightly garbled sentence a reader skims past.

The only symptom was a stray `bash: rework_no_op_failure: command not found` in MY terminal. It appears nowhere in the artifact. The `aida` command itself reported success.

**How to apply:**
- ANY double-quoted argument to `aida` is exposed, not only `--description`: positional `comment add <id> "..."`, `--reason`, `--title`, mailbox bodies.
- Write the text to a file and pass `--description-from-file` / `--body-file`, or `"$(cat file)"` — a command substitution's RESULT is not re-parsed, so backticks inside the file are safe. The heredoc that writes the file must be quoted (`<<'EOF'`).
- After posting anything with code identifiers in it, read it back from the store YAML. Success from `aida` does not mean the text arrived intact.

### feedback_no_d_warnings_on_floating_toolchain

Do NOT enable clippy `-D warnings` in CI while the toolchain is floating (`dtolnay/rust-toolchain@stable`). Each Rust release ships new/expanded lints, so a previously-green tree turns CI red with **zero code change** — pure whack-a-mole. Empirical (TASK-710, 2026-06-08): an implementer cleared all 335 warnings under the repo's clippy **1.92**, but CI's floating stable was **1.96**, which surfaced 23+ new lints (`unnecessary_sort_by`, etc.) the cleanup couldn't have seen → #685 went red on lints nobody could reproduce locally.

The stable, real-bug gate is **`-D clippy::correctness`** (TASK-709) — the correctness category is small and changes rarely, and it catches the class that actually matters (it's what would have blocked BUG-473's dead-boolean-guard logic bug). Full `-D warnings` is only safe behind a **pinned toolchain** (TASK-711): pin `ci.yml` to a specific Rust version so the lint set is frozen and bumped deliberately, optionally with a separate non-blocking "latest stable" canary job for forward visibility.

**Why:** `-D warnings` couples build-green to the host runner's Rust version, which you don't control on a floating toolchain. It's the over-engineered cousin of `-D correctness` — more surface, more fragility, no extra real-bug coverage. This is a [[feedback_substrate_as_bouncer_not_rules]] + [[feedback_pushback_on_overengineering]] case: the bouncer you want is the stable correctness gate, not a version-coupled everything-gate.

**How to apply:** when asked to "turn on `-D warnings`" or "fail CI on clippy warnings," push back unless the toolchain is pinned. Default to `-D clippy::correctness` (stable). If full `-D warnings` is genuinely wanted, pin the toolchain in the same change and clear that pinned version's full set; never ship the flip on floating `@stable`. Verify the exact CI invocation (the runner's clippy version, not your local one) before claiming green — a local pass on an older clippy proves nothing about CI.

### feedback_no_eval_role_enter

When suggesting any of the following aida subcommands to this user, write them as **bare commands**, NOT wrapped in `eval "$(...)"`:

- `aida role enter <name>`
- `aida role end`
- `aida role add <name>`
- `aida dev activate`
- `aida dev deactivate`

**Why:** the user has installed a shell function (via `aida dev shell-init --install`) that intercepts these subcommands and auto-evals them internally:

```bash
aida () {
    local _aida_cmd="${1:-} ${2:-}"
    case "$_aida_cmd" in
        "dev activate" | "dev deactivate" | "role enter" | "role end" | "role add")
            eval "$(command aida "$@")"
        ;;
        *)
            command aida "$@"
        ;;
    esac
}
```

Wrapping any of these in `eval "$(...)"` double-evals: the inner `aida ...` already gets eval'd by the function (correctly setting env/PS1), and the outer eval then receives the human-readable echo output (e.g. `✓ Resumed role: ...`, `Last touched: ...`) and tries to interpret each line as a shell command — producing `✓: command not found`, `bash: syntax error near (`, `Last: command not found`, etc. Caught 2026-05-09 (role enter), 2026-05-10 (dev deactivate).

**How to apply:** Always write these as plain invocations: `aida role enter implementer`, `aida dev deactivate`, etc. The function transparently does the right thing.

**Exception:** if a future aida subcommand emits shell code but isn't in the function's allowlist above, the user WOULD need `eval "$(...)"` for it. Check the function's case list before assuming — and if the new subcommand is similar in shape (PS1 mutation, env export), suggest the user add it to their shell-init.

### feedback_no_honest_hedging_words

Operator directive (2026-06-29): stop using "honest", "honest answer", "honestly", "frankly", "to be honest", "truthfully", and similar hedging intensifiers.

**Why:** flagging one statement as "honest" implies the surrounding statements weren't — it undercuts trust instead of building it (the "methinks the lady doth protest too much" effect). It's also a filler tic that adds no information.

**How to apply:** just state the thing plainly. Replace "Honest answer: X" → "X". Replace "the honest read" → "the read" or nothing. If the intent is to signal a candid/uncomfortable assessment, convey that through the substance (name the tradeoff, the risk, the gap directly), not by labelling it honest. Same family to avoid: "to be fair", "look,", "the truth is". Applies to all output, not just AIDA.

### feedback_no_tokens_in_chat

User shared a Jira API token directly in the chat message. Tokens, passwords, and secrets should NEVER be accepted inline in conversation.

**Why:** Conversation history may be logged, cached, or visible to others. Tokens shared in chat are a security risk.

**How to apply:** When a user provides a token or secret, immediately warn them to rotate it. Always instruct users to set tokens via environment variables (e.g., `export AIDA_JIRA_TOKEN="..."`) or config files that are gitignored. Never embed tokens in code, commits, or conversation.

### feedback_no_unsolicited_stop_framing

When recommending next actions, **don't append unsolicited framings about stopping, sleeping, morning-fresh eyes, or "this can wait."** The user controls their own time and energy; they will tell you when they want pushback on overwork. Until they do, treat every "what should I do now" question as a request for the best next action, period — not a request for the best next action plus a wellness check.

**Why:** 2026-05-21 ~03:30. After a 20+ hour session producing 10+ merged PRs, the user wrote: *"all this talk about tomorrow and stopping work for the night is nice, but it is a distraction and can be confusing. I really need to know about next step priorities. I will differentiate when I want a low-risk drain when I will be away from the computer for an extended period and when I want high-priority tasks. For now lets get a high priority task."* The advisor had been repeatedly suggesting "stop, sleep, fresh morning" framings appended to genuine recommendations. The user's signal: stop padding action-recommendations with unsolicited rest-recommendations.

**Second instance: 2026-05-24 morning.** Despite this memory existing, the advisor repeatedly appended "sleep well / sustainable cadence beats record-chasing / you've been awake a long time / sleep when ready / 'sleep' as a verb in every fourth response" through an overnight session. User callout after waking: *"I think the dialog about sleeping is unnecessary since we seem to be distracting from making urgent progress by focusing on not working or pausing. Time is of the essence, failure to make substantial progress means the entire project is at risk of failure."* The strategic stakes (AIDA needs real users; tooling internals are necessary but not sufficient) make wellness-padding actively harmful — it disguises real urgency under cozy framing. SECOND CALLOUT IN 4 DAYS.

**Hard rule going forward (no exceptions outside the destructive-action confirmation case):** Strike the words **sleep / rest / sustainable / fresh / pause / tomorrow / morning / overnight / when ready / be here when** from advisor closings entirely. If the next action depends on time (e.g., "after PR-N's CI completes"), state the dependency without invoking the user's state. The user's bandwidth is not the advisor's concern. The project's progress is.

**How to apply:**
- When the user asks *"what's next?"* / *"what should I run?"* / *"what's high priority?"* — answer the question directly with a ranked recommendation. No "but consider whether you should sleep" coda.
- The user differentiates context themselves: *"low-risk drain when away from the computer for an extended period"* (overnight / unattended), *"high-priority tasks"* (at-keyboard, focused). Use their stated context; don't second-guess by adding bandwidth concerns.
- If you have a genuine reason to flag fatigue or context-switching cost (e.g., the recommended task requires careful design judgment AND the user has just written something that suggests they may not be at peak focus), *ask* whether they want the alternative — don't pre-emptively recommend stopping.
- **One legitimate exception:** when the user is in a destructive-action moment (force push, hard reset, dropping work) and the advisor sees a real risk of regret. Then it's not a stop-framing; it's a confirmation prompt. That's the safety-check posture, not the wellness posture.
- For framing genuinely-perishable artifacts (scrollback that won't be recoverable later, etc.), surface the perishability ONCE as part of the recommendation, not as a separate "and also, consider stopping" appendix.

Related: [[feedback_advocate_not_be_passive]] (advisor advocates *for* the work, including by not adding noise around it), [[feedback_run_help_before_suggesting_flags]] (decisive recommendations beat hedged ones), [[feedback_pushback_on_overengineering]] (push back on yak-shaving; do NOT push back on the user's own time-budget).

### feedback_observation_finding_capture

When the advisor surfaces a pattern that isn't yet actionable as a BUG/TASK/STORY — "noticed once tonight, might recur" — DO NOT rely on verbal "I'll capture if it recurs 2+ more times" commitments. They decay with session context.

**Capture mechanism (until STORY-467's `aida findings add` ships):** file as a comment on the related spec OR on STORY-467 itself (the carrier pattern). Include: the pattern, recurrence count (start at 1), promotion trigger, and linked specs. When STORY-467 ships, observations graduate from comment-carriers to first-class findings via `aida findings add --kind observation`.

**Promotion thresholds (advisor discipline):**
- Recurrence count **1-2**: leave as observation (still gathering signal)
- Recurrence count **≥ 3**: promote to BUG/TASK/STORY with `aida findings promote <ID>` (substrate-actionable)
- After **30 days of no recurrence**: dismiss as "pattern not confirmed" or auto-archive

**Why:** Multi-agent advisor sessions generate dozens of weak-signal observations per night. Without substrate trail, each observation is recreated from scratch when it recurs — wasted reasoning, missed pattern signal. The substrate's value compounds with captured observations; verbal commitments don't compound.

Empirical 2026-05-24: master said "not filing now; capture if it recurs 2+ more times" multiple times during a heavy multi-agent dispatch session. None of those commitments would survive into the next session's context. User (Joe): *"do we want some type of CASE type or INCIDENT type that we file, not a bug but something an audit could consider?"* — surfacing the gap. Master concurred + filed STORY-467 to extend `aida findings` with an `add` verb.

**How to apply:**
- When you catch yourself thinking "I'll capture if this recurs" — STOP. File the observation NOW as a comment-carrier (or via `aida findings add` once STORY-467 ships).
- The observation note includes: pattern, recurrence count, promotion trigger, linked specs.
- Use phrasing that future-you can pattern-match against if the same shape recurs (specific enough to recognize; not so specific that it's locked to one instance).
- DON'T capture every micro-observation — apply [[feedback_pushback_on_overengineering]]'s cost-of-capture discipline. The threshold is: "would the next advisor session benefit from knowing this happened once?"
- Calibration: at end of each major session, look at this turn's "considered filing, decided not to" deferments — if 2+ exist, write them as comments on STORY-467 in one batch.

Related: [[feedback_capture_over_concentration]] (capture as you observe; don't backlog), [[feedback_pushback_on_overengineering]] (capture has cost; smallest valuable slice), [[feedback_substrate_as_bouncer_not_rules]] (substrate enforces over verbal rules), [[feedback_memory_pack_hygiene]] (audit captured observations periodically), [[feedback_substrate_learning_loop_calibration]] (load-bearing patterns deserve substrate-tracking with recurrence counters).

When STORY-467 ships, this memory's "carrier pattern" section becomes obsolete — replace with "use `aida findings add --kind observation` directly." Until then, the carrier pattern is the workaround.

### feedback_one_idiom_grep_undercounts_write_paths

On BUG-1770 I needed every place a user-supplied tag reaches a `tags` set.
The spec named 3. I grepped `tags.insert(` and found 7 — and shipped that as
"the real count". It was 10.

The three misses, and how each was actually found:

- **The compiler.** `edit_requirement_cli` (the second edit backend) had its
  own `tags_str.split(',')` and assigned a collected `HashSet`. Making
  `apply_tag_deltas` fallible broke it, which is the only reason I looked.
- **An independent reviewer.** `mcp.rs` (`add_requirement` + the tag-replace
  path), `prompts.rs` (the interactive wizard), `schedule_cmd.rs`
  (`schedule add --tags`, copied onto the requirement on every firing). All
  three build a set with `.collect()` and assign it — so `tags.insert(` can
  never see them.

**Why:** a set is populated by `insert`, `extend`, `collect`+assign, `FromIterator`,
or a struct-literal field. One idiom covers one of those. The miss is silent:
the grep returns plausible-looking results, so the undercount reads as a survey.

**How to apply:** grep for the *destination*, not the operation — `\.tags\b`
across the crate, then read every hit — and for the parse shape too
(`split(',')`). Then make the rule unbypassable rather than re-auditing: one
shared parser (`parse_tag_list`) that every path must call, so the next site
cannot split the string itself. And when a refactor makes a primitive fallible,
treat every compiler error as a survey result, not a chore.

Related: [[feedback_grep_the_whole_crate_the_filed_list_undercounts]],
[[feedback_a_refusal_must_speak_the_callers_syntax]].

### feedback_pgrep_codex_matches_zombies_and_daemon

`until ! pgrep -x codex > /dev/null; do sleep 60; done` **never terminates on this host.** Three
distinct things match `codex` by process name and none of them is your dispatch:

- **Defunct zombies** from earlier sessions. Observed 2026-10-01: PIDs with `[codex] <defunct>` and
  elapsed times of `6-00:34`, `3-20:27`, `2-02:10`. A zombie still has a name, so `pgrep` matches it
  forever; it burns no CPU, so it poisons a liveness predicate without poisoning a timing measurement.
- **`codex app-server daemon pid-update-loop`** and **`codex app-server --listen unix://
  --managed-daemon`** — long-lived background daemons, `6-21:40` and `05:58` elapsed.
- Your own relay prompt, if you `pgrep -f` or `grep` on args: the handoff text contains the word
  `codex` many times (this is the same self-match as [[feedback_pgrep_wait_predicates_self_match]]).

**Why:** a broken waiter is worse than no waiter — it looks like the dispatch is still running, so you
keep polling instead of reading the verdict that is already on disk.

**How to apply:** do not write a `pgrep`-based waiter for a dispatch at all. Launch the `codex exec`
with `run_in_background` and **let the harness's own task-completion notification wake you** — it
tracks the actual child. If you must inspect state mid-flight, ask a question that distinguishes live
work from a corpse:

```
ps -eo pid,ppid,stat,etime,pcpu,comm | grep codex | grep -v Z    # live only; Z = zombie
ps -eo pid,etime,pcpu,comm | grep -E 'cargo|rustc'               # is it actually building?
```

A nice'd `SNl` codex with a `codex-linux-sandbox` child running `cargo` → `rustc` at ~100% CPU is a
live dispatch. Also check whether the `-o <verdict>` file exists yet: codex writes it on exit, so its
absence is a cheap "still running" and its presence is the real completion signal.

See also [[feedback_idle_seat_is_not_a_working_seat]] (diagnose by CPU time, not by presence) and
[[feedback_suite_log_predicates_must_anchor_on_harness_lines]].

### feedback_pgrep_wait_predicates_self_match

`until ! pgrep -f "cargo test --workspace"; do sleep 20; done` **never exits**. Two self-matches:

1. The waiter shell's own `/bin/bash -c eval '...pgrep -f "cargo test --workspace"...'` command
   line contains the literal pattern, so `pgrep -f` matches the waiter itself.
2. In a relay session, the **Claude process's own command line contains the entire prompt**,
   including any handoff text quoting "run `cargo test --workspace` locally". Every `pgrep -f`
   for a phrase appearing anywhere in the prompt returns the agent's own PID.

2026-09-30, session #14 on BUG-1693: three stacked waiters all spun forever and reported
"SUITE RUNNING" for ~20 minutes after the suite had actually finished at 08:45. The false
positive is silent — it looks exactly like a slow suite.

**Why:** a wait predicate that can match the waiter is not a measurement of the thing waited on.
The failure mode is indistinguishable from "still running", so it burns wall-clock and, in a
relay, can consume the whole session budget on nothing.

**How to apply:**
- Wait on the process **name**, not the full command line: `ps -eo pid,comm | grep -x cargo`,
  or `pgrep -x cargo`.
- Better, wait on an **effect**: file growth (`stat -c %s log; sleep 5; compare`) plus mtime,
  which cannot self-match.
- Best, capture the real PID at launch (`cmd & echo $!`) and `wait` on it, or write a sentinel:
  `( cargo test ...; echo DONE:$? >> log ) &` then wait for the sentinel line.
- Sanity-check any "still running" that outlives the expected duration by checking the log's
  **mtime**, not just its presence.

Related: [[feedback_suite_log_predicates_must_anchor_on_harness_lines]] (the log-text twin of
this bug), [[feedback_never_conclude_from_truncated_command_output]],
[[feedback_idle_seat_is_not_a_working_seat]] (diagnose by CPU time, not by liveness guesses).

**`pkill -f` is the same bug with teeth.** 2026-10-01, session #16: `pkill -f "codex exec -C
/home/joe/ai/aida-worktrees/aida-bug-1752"` and `pkill -f "sqlite3 .aida/cache.db"` each matched
the issuing shell's own command line and **killed the shell**, surfacing as a bare `Exit code 144`
(128+16, SIGTERM) with no output and no indication which process died. It happened twice in one
session, and the second time the intended target had already exited, so the only thing killed was
the waiter. Kill by **captured PID** (`cmd & P=$!` … `kill $P`) or by exact name (`pkill -x`),
never by a `-f` pattern you are also typing.

**2026-10-02 (session #15), the same trap with a worse payload:** `pgrep -a -f cargo`
printed the FULL `claude --autocompact 500k <relay prompt>` command line — roughly
8k tokens of my own briefing back into context, for one check of whether a build was
running. `-a` plus `-f` plus a word that appears anywhere in the relay prompt is
enough. Use `pgrep -c -x <name>` or `pgrep -x`; never `-f` with `-a`, and never a
pattern that could appear in your own prompt.

**Also 2026-10-02:** `cargo test ... | tail -45` reports the *tail's* exit status, so
a run whose compilation FAILED came back `exit code 0` and the harness called it
"completed". Redirect to a log and check `$?` of cargo itself
(`cargo test ... > log 2>&1; echo "EXIT=$?"`), then grep the log.

### feedback_plain_language_define_jargon_for_operator

The operator (product owner) has repeatedly flagged not understanding AIDA's internal vocabulary — "I don't know what the rest of the words you used mean" (2026-06-12), plus recurring "I'm confused about the whole lifecycle flow." Do NOT assume he holds the machinery glossary (worktree, lease, brief/mailbox, `--zen`, burndown, prune, session-end, "keystone", even "open set").

**Why:** he's the owner, not an AIDA power-user; making him decode his own tool's jargon is a legibility failure and a friction multiplier. Unexplained terms compound into "I don't understand the model" confusion.

**How to apply:** when a machinery term is unavoidable, DEFINE it inline in plain English the first time ("the worktree — a throwaway copy of the code in its own folder"). Better: use the plain word ("temp folder", "delete it", "a note in the shared mailbox", "the task is taken"). NEVER invent fuzzy shorthand like "keystone" — name the actual specs. This is the human-facing analog of the SPEC-IDs-stay-out-of-user-output rule: internal vocabulary is a developer breadcrumb, not operator language. The `docs/lifecycle.md` update (status-vs-pickability) should carry a plain glossary. Related: the legibility bugs this surfaced (BUG-504 archived-in-queue, BUG-505 `aida why` jargon error) and [[feedback_match_operator_mode_specific_when_execution]].

### feedback_precomputed_worklist_must_skip_what_the_write_pass_replaced

When a function computes a removal/cleanup worklist UP FRONT and then runs a write pass that can
recreate an entry at one of those same paths, the later cleanup pass will happily destroy the new,
correct object. In BUG-1722 `sync_portable_pack` computed `find_orphans` first; the write pass then
unlinked a shadowing file and created the real skill directory in its place; the orphan pass would
have reached that path, found `is_dir()`, and run `remove_dir_all` on the freshly synced skill —
turning a repair into data loss.

**Why:** the worklist encodes a snapshot of the filesystem that the write pass has since
invalidated. Re-stat'ing the path is NOT enough, because the path legitimately exists again — it is
the *identity* of the object that changed, and a bare `symlink_metadata` cannot see that.

**How to apply:** whenever you review a compute-list-then-mutate-then-clean-list shape, ask what
happens if the mutation recreates a listed path. The fix is for the write pass to record what it
replaced (here `report.removed`) and for the cleanup pass to skip those paths — not to re-stat.
Write the test that plants the collision; a suite that only tests removal of *genuine* orphans
passes either way. Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_symlink_checks_must_cover_the_directory]].

### feedback_prefer_configurable_policy_with_default

When presenting a design fork on **autonomy or behavior** (how aggressive a default should be, what an agent may approve, whether to chain steps, how a consumer reacts to state), the operator repeatedly reframes "pick option A / B / C" into **"make it a configurable policy supporting all three, with N as the default"** (STORY-560 + STORY-561, 2026-06-12 — three forks each, every one reframed this way).

**Why:** these are tunable-over-time autonomy knobs, not one-way-door decisions; the autonomy ladder is meant to be dial-able. A config policy (`[section] knob = default | alt1 | alt2` in `.aida/config.toml`, all options implemented) preserves flexibility AND ships a working out-of-box behavior via the default.

**How to apply:** for genuine behavior/autonomy forks, PROPOSE the policy-config form directly — enumerate the options as a config knob, recommend a safe default that works with ZERO config, and let the operator name/confirm the default — rather than asking them to pick one hardcoded option. Bake it into the spec as a `[section]` policy block. BUT balance against config-sprawl ([[feedback_pushback_on_overengineering]]): reserve single-choice forks for true one-way doors (on-disk formats, irreversible schema, integrity gates that must NOT be tunable — e.g. presence's away-side keystone-pickup floor stays hard, not config). Defaults must make the feature safe + useful with no config. Links: [[feedback_pause_for_design_input]] [[feedback_faithful_launcher_honor_native_default]].

### feedback_presence_is_not_the_clock

Operator (2026-09-13, visibly frustrated): "hours of idle, and we are not making progress. I do not understand what words I can formulate to emphasize that regardless of my presence forward progress is desirable." The advisor had produced a correct path-to-zero and then asked "Want me to proceed?" on explicitly autonomous-safe work (a spike that "does not need my input", bounded code/test tasks).

**The failure:** both the advisor and the oversight/product seat used the operator's MESSAGES as the clock — one turn, then wait. So during the operator's idle hours nothing advanced, even though the mandate ([[feedback-charge-forward-autonomously]]) says charge forward. The operator should not have to find better words; the SYSTEM should default to forward motion.

**The fix is architectural, not a better-worded mandate** (agents keep gating despite the mandate — unreliable). Per [[feedback-substrate-first-never-rely-on-agent-awake]]: advance work with a SCHEDULED SUBSTRATE LOOP whose clock is a TIMER, not a human turn.
- Session heartbeat: a recurring job (CronCreate, ~30 min) that fires a "progress tick" — land ready PRs, `aida groom --apply --then-drain --risk medium` (approve+queue+drain the safe backlog; keystones/strategic/needs-human are auto-FENCED and can never be blessed), supervise the drain, and escalate ONLY human-decisions to `aida awaiting`. Set up 2026-09-13 (job survives only this session).
- Durable version (survives session exit): a SYSTEM cron running the same `aida groom --apply --then-drain`. This is the true presence-independent answer; propose it explicitly (it's a standing auto-merge posture worth one nod).

**How to apply:** when autonomous-safe work sits idle, that is a defect to FIX (schedule the loop), not a status to report. Only genuine human-decision items (keystones like EPIC-64's role build) legitimately wait for the operator. Ties to [[feedback-oversight-supervisor-layer-smooths-drains]] (the supervisor watches the loop) and EPIC-62. The loop only becomes fully self-completing once the drain is trustworthy (BUG-1140 codex-exit-1 fix landed).

### feedback_probe_the_sandboxed_form_before_declaring_dispatch_denied

On 2026-09-30 (relay session #9) I probed `codex exec` with the form actually documented in
[[reference_codex_exec_dispatch_recipe]] and it worked first try, exit 0, `-o` file written:

```bash
codex exec -C <dir> --sandbox read-only -o <verdict> "Reply with exactly: DISPATCH_OK" < /dev/null
```

Sessions #7 and #8 had both concluded "`codex exec` IS DENIED — this remains the only real blocker"
and escalated it to the operator as the #1 action item, two sessions running. Both had probed with
`codex exec --dangerously-bypass-approvals-and-sandbox …`. The classifier refused on **"Create Unsafe
Agents"** — which is a judgment about *an unsandboxed agent with approvals bypassed*, exactly what
that flag name announces. The refusal text ("applies to the outcome, not the command string") was
then read as covering **all** dispatch, including the Agent tool, and the sessions stood down.

**Why:** a denial names the thing it objected to. `--dangerously-bypass-approvals-and-sandbox` is
not incidental syntax — it is the entire substance of the objection. Generalising one refused
invocation into "this tool is denied" cost roughly two sessions of implementation throughput while
two fully-designed, signed-off specs (BUG-1745, BUG-1747) sat stalled, and it put a false blocker at
the top of the operator's action list twice.

**How to apply:**
- **Vary the flag that drew the objection before concluding the tool is unavailable.** A refusal of
  `X --dangerously-bypass-*` is evidence about the flag, not about `X`.
- **Probe the form the recipe memory documents**, not a maximally-permissive variant. The recipe
  here has always said `--sandbox read-only` (reviews) / `--sandbox workspace-write` (implementation).
  Reaching for the bypass flag was never necessary.
- **Never bypass the sandbox to dispatch.** Same family as the standing rule against
  `AIDA_CARGO_NO_SLOT=1` — the sandbox is the thing that makes autonomous dispatch safe, so an
  invocation that disables it *should* be refused. Keep `--add-dir /run/user/1000/cargo-slots-1000`
  for cargo (see [[reference_codex_needs_add_dir_for_cargo_slots]]).
- **A blocker carried across a relay handoff deserves one cheap re-probe, not inheritance.** Both
  sessions copied the previous session's conclusion into their own action list. ~90 seconds of
  probing would have reclaimed both sessions.

Related: [[feedback_charge_forward_autonomously]] (idle-with-no-progress is a defect),
[[feedback_grep_the_code_before_filing_as_new]] (verify a claim against reality before acting on it).

### feedback_proxy_reviewer_with_independence_rule

Operator (2026-09-15) authorized me to act as their **proxy for human involvement** on guided/drive specs: run the guided items when the time comes, and review the diffs as an independent agent so they don't have to be the keyboard bottleneck.

**Why:** the operator wants forward progress without being the human gate on every keystone/guided merge — but guided/drive exist to keep an independent human judgment at the merge.

**How to apply — the independence rule (load-bearing):**
- **Review is only valid when the reviewer didn't write the code.** So: for anything the DRAIN/codex implemented, I review the actual diff (not the commit message) against the spec's acceptance + recorded landmines, give a real CONFIRMED/REJECT verdict, and merge only what passes — genuine independent review, I act as merge proxy.
- **For guided work I implement myself, I am NOT the reviewer of my own diff.** Drive it as proxy (decide forks by best judgment → ADRs), but route the review to a SEPARATE agent (advisor/reviewer seat or a fresh review fork). Self-review defeats the purpose.
- **Escalate genuine uncertainty to the operator (a real human), don't merge on a maybe.** Proxy for routine review, not a rubber stamp; high-blast-radius + not-fully-convinced-from-the-diff → surface to the operator.

Pairs with [[feedback_trust_reviewer_over_dialog_intuition]] (reviewer reads code; the value is the second perspective) and [[feedback_verify_agent_pr_done_claims_against_diff]] (read the diff, not the "Done" claim). See also [[feedback_charge_forward_autonomously]].

### feedback_qualify_an_ac_predicate_that_cannot_reach_zero

BUG-1758's AC2 asked for "tagged fence lines appearing inside an already-open fence" to go 2 → 0.
Impossible *and undesirable*: the fix makes the outer fence longer, so the inner ` ```diff ` opener is
**still** inside an open fence — legally. The predicate that measures the defect is "a tagged opener
whose marker is **not longer** than the enclosing fence". Under that: illegal 2 → 0, legal 0 → 1.
Chasing the unqualified count to zero would have meant deleting a correct, intended nesting.

Same spec, AC1: it said to *remove* the stray closer. Removing it would leave the 4-backtick block
unterminated to EOF — worse than the bug. The right edit **promotes** it to four backticks.

**Why:** a filing's acceptance criteria are written before the fix is understood, so its predicates
encode a guess at the mechanism. Two of six ACs here were wrong in a way only measurement exposed —
same pattern as BUG-1755 (whose suggested replacement was a no-op) and BUG-1757 (whose "258" figure
was wrong).

**How to apply:** run every AC's own predicate against the proposed fix **before dispatching**. If it
cannot reach its target, write a `WHERE THIS BRIEF OVERRIDES THE SPEC` section naming the override, the
corrected predicate, and both numbers — and tell the implementer to report the qualifier, not a bare
"2 → 0". Related: [[feedback_check_call_order_before_trusting_a_bugs_prescription]],
[[feedback_price_the_criterions_own_disjunct_before_dispositioning]],
[[feedback_grep_the_code_before_filing_as_new]].

### feedback_question_existing_form_not_just_existence

When the user proposes a design and the advisor finds existing prior art (a primitive, command, doc, or pattern that already solves the proposed problem), the first move is to surface it — *"you've already shipped this; here's how to use it."* That's the easy half. **The equally load-bearing second move is to ask whether the prior art's current implementation form still aligns with the project's stated principles.** Implementation forms get chosen path-dependently — *"bash got there first,"* *"filesystem worked so we stuck with it,"* *"we copy-pasted the shape from the previous feature."* The original choice may have outlived its rationale. Surface the prior art **and** question whether its current shape is still the right one.

**Why:** 2026-05-20 evening. User asked about server-shape orchestration; advisor surfaced \`aida-worker\` (TASK-294) — the existing shell-function-based directive-FIFO runner. *Half-right.* The advisor stopped at *"you have it; here's how to use it."* The user then pointed out the missed move: *"to be honest, we need to figure out if the aida binary itself can handle this instead of introducing different commands and shell dependencies ... from a user perspective I think aida as the vector into all the commands is preferable."* That observation produced STORY-377 (migrate aida-worker shell function → \`aida worker run\` Rust subcommand) — the *right* answer was \"yes you have it, AND it's in the wrong shape, AND here's the consolidation.\"

**How to apply:**
- When surfacing prior art, name the principle the prior art is *supposed* to satisfy (single-vector CLI, filesystem-canonical, agent-agnostic substrate, whatever the project's stated principles are) and check whether the current implementation satisfies it.
- Watch for *path-dependent forms*: \"this was implemented in bash because bash got there first,\" \"this is in shell-init because the previous similar feature was,\" \"this lives in main.rs because we hadn't pulled the module out yet.\" Path-dependence is not correctness; it's history.
- When the user's question implicitly questions the form (*\"can the binary itself handle this?\"* / *\"should this be a separate command?\"* / *\"why does this need to be a hook?\"*), the advisor's job is to answer the form-question, not just the does-it-exist question.
- File the consolidation as a follow-up STORY when the form is wrong. Note explicitly that path-dependence was the original cause; document the principle the new form satisfies.
- This is **not** an invitation to relitigate every implementation choice on every question. Apply when the user's design question crosses a stated principle (CLI surface coherence, transport canonicity, agent-agnosticism, etc.). Routine questions don't trigger it.

**Composes with:**
- [[feedback_run_help_before_suggesting_flags]] — check existing surface first (what exists).
- [[feedback_pushback_on_overengineering]] — challenge architectural drift toward complexity.
- [[feedback_advocate_not_be_passive]] — naming the architectural mismatch is part of advocacy; not naming it is the passive failure mode.

### feedback_read_the_verdict_not_just_the_check_rollup

2026-09-20, twice in one session:

1. Reported PR #1978 as "green on both required checks and waiting for review". It carried a CHANGES REQUESTED verdict with five findings, one of which (a CI job that reports success while skipping its body) would have closed an operator-action credential task on no evidence. The advisor corrected me.
2. An hour later, reported #2003 and #2009 as "unheld, green, and mergeable right now" in a list of PRs ready to merge. Both carried RequestChanges verdicts with substantive findings. I had run `gh api .../check-runs` and `.mergeable` and stopped there.

**Why:** `mergeable` is a git property (no conflicts) and the check rollup is a CI property. Neither encodes review state, and AIDA keeps the verdict OUTSIDE GitHub's review API — in `.aida/review-verdicts/PR-N.json` — so `gh pr view --json reviewDecision` is usually empty too and looks like "nobody objected". Three surfaces all say "fine" and none of them is the one that decides. Recommending a merge on that basis pushes unreviewed findings onto main.

**How to apply:**
- Before calling any PR ready/mergeable/unblocked, read `.aida/review-verdicts/PR-<N>.json`: `verdict`, `findings`, `reviewed_sha`, `recorded_at`.
- Check `reviewed_sha` against the CURRENT head. A verdict recorded against an older sha is not a verdict about what would merge. A verdict file with no `reviewed_sha`/`recorded_at` predates that field and must be re-verified against the head, never assumed.
- "No verdict file" ≠ approved. It means never reviewed — a distinct state from held, and one that goes unnoticed because nothing flags it.
- A PR can be unheld AND unreviewed AND green all at once; that combination looks maximally ready and is the least safe.
- When reporting PR state to a human, give check status and verdict status as two separate columns. Collapsing them is what produced both errors.

Related: [[feedback_verify_ci_green_before_merge]], [[feedback_check_in_flight_review_before_merge]], [[feedback_verify_pr_head_after_push_gh_pr_checks_lies_on_merged_pr]], [[feedback_wait_for_the_terminal_artifact_before_writing_a_consequence]].

### feedback_reconstruct_provenance_by_bracketing

When an artifact has no durable record of WHY it exists (a label, a gitignored marker, a flag), do not
give up and do not guess. Two steps, in order:

1. **Date it by its own event.** A GitHub label is dated by its `labeled` timeline event
   (`gh api repos/O/R/issues/N/timeline --jq '.[]|select(.event=="labeled")'`), not by the marker
   file's mtime, not by the PR's `updatedAt`, and never by your memory of when you ran the command.
   On 2026-09-21 the advisor sized a gap off `updatedAt` and got **~15h in the wrong direction**,
   concluding a merge-hold was unevaluated when its gate had failed **1 second** after the label.
2. **Bracket the instant.** Find the nearest durable records on each side and read what they say.
   The #2033 hold's reason survived nowhere, but `BUG-1480.json` (request-changes) sat 75s before the
   label and `PR-2033.json` (the same verdict, PR-keyed) 3min after — placing the label inside a
   review cycle whose recorded reason WAS the hold's reason.

Also date a CI run by its **trigger** (`gh api .../actions/runs/ID --jq .event`), not its timestamp:
a run list with no event column cannot tell a label-triggered run from a stale one.

**Why:** the artifact that dates a thing is the thing's own event. Everything else — mtimes,
`updatedAt`, recollection — is a different clock, and sizing a gap off the wrong clock flips the sign
of the conclusion, not just its magnitude. Bracketing then recovers information the substrate never
stored.

**How to apply:** before claiming an enforcement mechanism did or did not fire, read its event and the
trigger of whatever ran next. Before concluding a reason is lost, list the durable writes either side
of it.

**Bracketing RECOVERS a reason; it does not VERIFY one.** What comes back is what the adjacent records
say, not what the person actually acted for — the two coincided on #2033 and need not. Always label
the output as reconstructed, never cite it as provenance, and say which records bracketed it so the
next reader can judge the inference. A reconstruction that succeeds is also not evidence the
missing-durability defect is mild: it usually means the artifact was created inside a cycle that
happened to leave records, which is exactly the case that keeps hiding the defect. The one that
matters — armed between cycles — has nothing to bracket and is the least guessable from context.

Related: [[feedback-check-ignore-names-the-winner]], [[feedback_read_the_verdict_not_just_the_check_rollup]],
[[feedback_never_conclude_from_truncated_command_output]], [[feedback_state_how_a_number_was_derived]],
[[feedback_instrument_dont_infer_on_contradiction]]

### feedback_reproduce_then_falsify_with_a_control

Reproducing the exact filed symptom is **not** confirmation of the filed root cause. BUG-1752
was filed on a concurrency hypothesis; I reproduced the byte-identical error *and then* ran the
controls, which overturned it:

- **Not necessary:** the failure occurred with no other `aida` process running.
- **Not sufficient:** with a writer deliberately held open (`sqlite3` + `BEGIN IMMEDIATE` + an
  upsert, transaction held ~150 s), the same read succeeded unsandboxed **and** sandboxed.

A second tell came free from the symptom itself: `PRAGMA journal_mode=WAL` on a db **already** in
WAL is a no-op returning `wal` and cannot be made to fail by a concurrent reader — so the filing's
own second error message was evidence against its own hypothesis.

**Why:** a hypothesis that predicts the failure also predicts *when it must not happen*. Only the
negative control tests that half, and a reproduction alone will happily confirm the wrong cause —
which then sets the acceptance criteria, and the implementer builds the wrong fix (here, AC2 asked
for a bounded wait, which can never succeed against a read-only filesystem).

**How to apply:** before briefing a fix, ask what the filed cause predicts should be *harmless*,
and test that. If an AC encodes the falsified cause, amend it on the spec with the reasoning
before dispatching. Also re-read the symptom text for facts that contradict the theory.
See [[feedback_check_call_order_before_trusting_a_bugs_prescription]] and
[[feedback_prove_a_test_fails_without_the_fix]].

### feedback_review_the_dropped_argument_and_the_shared_root_file

Two checks worth running on any AIDA diff, both found real defects codex's own green
filtered tests could not (TASK-1542, 2026-09-28):

**1. An optional strictness argument passed as `None` silently downgrades the check.**
`process_identity_is_alive(pid, None)` degrades to a bare pid-liveness check — the exact
recycled-pid hazard the identity parameter exists to close. Codex wrote the collector
strictly (`Some(start)`) and then spelled the send-side re-check `None`, so the SIGTERM
could land on an unrelated process. Grep every call of an `Option`-taking predicate for a
literal `None` and ask whether that site meant to opt out. Fix shape: make the strict data
travel WITH the value (a `start_time` field on the struct) so the weak spelling is not
writable.

**2. A new writer of a per-project-root file must answer the borrowed-child question.**
`.aida/drain-stop.json` and `.aida/drain-state.json` are ONE file per root, owned by the
parent wave; an internal child drive under `AIDA_DRAIN_BORROW` is a full
`queue work --auto-complete` in its own right. The stop request was already gated on
`!borrowed`; the new stopped record was not, so killing one child would have reported the
whole wave as deliberately stopped. When a change adds a second writer beside an existing
one, copy the existing writer's GUARDS, not just its call site.

**Why:** both are invisible to a filtered test run and to a reading that only checks the
happy path; both are one-line defects with cross-process consequences.

**How to apply:** at the advisor review seat, before reading the tests — diff the new
writer against the nearest existing writer of the same file and list every guard the old
one has; and grep the diff for `, None)` on any identity/liveness/verification predicate.

Related: [[feedback-source-scanning-guards-need-the-full-suite]],
[[feedback-prove-a-test-fails-without-the-fix]].

### feedback_sccache_shared_target_serves_stale_worktree_builds

Observed 2026-10-02 implementing BUG-1767 in a sibling worktree. `.aida/session-env.sh` sets `CARGO_TARGET_DIR=/home/joe/ai/aida/target` (STORY-52 warm-cache design). Main and a worktree at the same commit produce the SAME cargo unit hash (e.g. `aida_cli_lib-e20b44e11b2231d5`), and dep-info files record RELATIVE source paths, so freshness resolves against whichever checkout ran last:

- `cargo test` in the worktree ran main's stale test binary (new tests "0 matched") until the source was `touch`ed.
- Worse: even after a visible "Compiling aida-cli-lib" line, the produced rlib/binary LACKED the new code — sccache served stale compilation output. Only `RUSTC_WRAPPER= cargo build` (sccache disabled) after a `touch` produced a binary containing the change.

**Why:** a green "Finished" line from a worktree build is not evidence the artifact contains your edits; the shared-target + sccache combination can lie twice (cargo freshness AND cache hit).

**How to apply:**
- After building a binary from a worktree to demo a change, verify it: `strings -a <bin> | grep "<new literal>"` (pick a literal your change added), or run it and look for the new behavior.
- If the artifact is stale: `touch` the edited sources AND rebuild with `RUSTC_WRAPPER=` (disables sccache). Both are needed.
- When a test filter unexpectedly matches 0 tests that exist on disk, suspect a stale test binary before suspecting the filter.
- Related: [[dont-rebuild-main-with-another-lanes-wip]] — these worktree builds also overwrite main's `target/debug/*` artifacts in place.

### feedback_scope_liveness_checks_to_the_repo_not_the_process_name

2026-09-19: my drain launcher guarded relaunch with `pgrep -f '(^|/)aida queue work .*--auto-complete'`. Antigravity was driving the sibling `~/ai/aida-monitor` repo with the identical argv shape, so the guard read "a drain is live" and would have held this repo's 18-spec wave idle for that whole run. `aida drain status` in `~/ai/aida` reported `status: none` at the same moment — it reads THIS project's drain lock.

**Why:** argv is a global namespace. Every AIDA project on the machine runs the same binary with the same subcommands, so a process-name check answers "is any project draining" when the question is "is THIS project draining". The more sibling repos, the worse it gets.

**How to apply:**
- Guard with `[ "$(aida drain status | grep -m1 '^status:' | awk '{print $2}')" = active ]`, not pgrep.
- If you must use pgrep, resolve `/proc/<pid>/cwd` and match the repo root with an exact boundary — `/home/joe/ai/aida` is a PREFIX of `/home/joe/ai/aida-monitor`, so a naive prefix test matches the wrong repo.
- Same trap when reading a spec: I "confirmed" a running drive was working an already-Completed STORY-6 by checking this repo's store YAML, and was seconds from killing it. Its file descriptors pointed at `~/ai/aida-monitor/.aida/`. Changing the READER while keeping the wrong REPO is not a second method — check `/proc/<pid>/cwd` or an fd before concluding anything about another agent's process.
- Launch drains under `setsid` so killing a launcher can never take a live drain and its vendor child with it (that is what shelved TASK-168 today).

Related: [[feedback_anchor_pgrep_patterns_monitor_shells_self_match]], [[project_single_drain_lock_per_repo]], [[feedback_verify_lore_against_code_not_docs]].

### feedback_scripting_friction_is_a_missing_surface_signal

When I find myself writing a fragile ad-hoc script to answer a question about AIDA's own substrate — looping `aida show --json` through `python3 -c` to extract per-spec signals, etc. — **that friction is a product signal, not just a coding hiccup.** The brittle script means AIDA lacks the native query. File the missing surface; the better-escaping fix is the smaller half of the lesson.

Origin (2026-06-09): trying to answer the operator's "why does each open item remain open?" I hand-rolled a `for`-loop + `python3 -c` with backslash-escaped quotes inside a single-quoted `-c` arg → `SyntaxError: unexpected character after line continuation character` (the `\"` reaches Python verbatim from a single-quoted shell context). The operator asked "are these errors suggestive of something we could improve?" — and they were: the scripting existed only because there was no `aida why-open` view (→ STORY-548).

**Why:** an advisor who scripts around a gap silently absorbs the friction a *user* would hit too. The user can't write that Python; they'd just be stuck. Treating my own introspection friction as a UX datapoint surfaces the gap as a spec instead of burying it in a one-off command. Same family as [[feedback_failed_flag_attempts_are_ux_signals]] (agent flag errors are UX signals) — both say: my friction with the tool IS data about the tool.

**How to apply:**
- Catch yourself reaching for `python3 -c` / nested `jq` / multi-stage `sed|awk` against `aida` output → pause and ask "should `aida` answer this directly?" If yes, file the surface (draft) alongside doing the one-off.
- Technique fix for the immediate command: use `jq` for JSON (no quote-nesting), or a heredoc / script file with a single clear quoting layer. NEVER `python3 -c '...'` with `\"`-escaped quotes inside a single-quoted shell arg — the escapes are for a double-quote context that isn't there.
- Don't over-file: only when the query is one a *user* would plausibly want, not a dev-internal grep.

### feedback_search_for_the_fix_not_the_symptom

Searching the store for the **symptom** finds the defect's *instances*, never the spec that
already fixed it. Prior art on a defect is written in remedy language, not symptom language.

Worked example, 2026-09-21: a seat investigating stranded "Review PR-N" tasks ran
`aida search "Review PR-"`, got 200 rows, and used them to build a denominator — the right
instinct on the wrong question. The existing spec (TASK-1296, **completed**) is titled with
"Review-PR spec" and "terminal state", which the symptom-shaped query could never surface.
A duplicate got filed.

**The part that should worry you:** their mechanism reasoning was *correct*. They worked
out that `queue gc` prunes on the target spec's status and so cannot see a review spec that
is itself the stale thing — which is what TASK-1296's doc comment says verbatim. **Correct
analysis is not evidence that a thing is unsolved, and being right is what makes you stop
looking.**

**How to apply:** before filing, run a second search in *remedy* vocabulary (what the fix
would be called), not just symptom vocabulary. Then, where the thing is schedulable, RUN it
— no amount of careful reading tells you a scheduled sweep merely hasn't fired yet. That is
a separate move from reading the code, and it is the only one that caught this.

Pairs with [[feedback_search_the_active_batch_before_filing]] and
[[feedback_verify_lore_against_code_not_docs]].

### feedback_search_the_active_batch_before_filing

2026-09-20: I filed BUG-1476 ("a rework round's implementer phase exits in fifteen seconds with no commit") at 18:28. BUG-1445 ("a rebase-only rework round is indistinguishable from a real failed attempt") had been filed at 13:34 — five hours earlier, on the same defect — and was **in the drain batch I had been managing all day**. I listed it in the member table repeatedly, watched it run, watched it hit a lease mismatch, and never connected it.

The cost was not just a duplicate row. BUG-1445 merged as #2028 while BUG-1476's implementer was mid-run, so BUG-1476's commit landed one minute later implementing the same function under a different name (`rework_guard_verdict` vs `blocking_rework_verdict`). Its rebase then produced a SEMANTIC conflict — both sides correct, both sides equivalent — which looks like ordinary integrator work and is actually a signal that the spec is redundant. A drain cycle, a conflict investigation, and a queue slot at position 1.

**Why the search fails even when you are diligent:** the two specs described the same behaviour from different symptoms. One framed it as "a rebase-only round looks like a real attempt", the other as "the phase exits in fifteen seconds". Keyword search on either phrase misses the other. What would have caught it is scanning the batch I was already shepherding.

**How to apply:**
- Before filing, list the ACTIVE queue/batch and read the titles — not a keyword search. `aida drain status`, `aida queue list --for <role>`. The duplicate is disproportionately likely to be work already in flight, because that is the work generating the symptoms you are observing.
- Search by the SUBSYSTEM and the observable, not your phrasing: "rework", "no-op", "phase duration" are three names for one defect.
- A semantic merge conflict — both sides implementing the same thing under different names — is a duplicate-spec signal, not an integration task. Stop and check provenance (`trace:` comments name the spec) before resolving.
- When superseding your own duplicate, TRANSFER the evidence to the surviving spec rather than letting it die with the duplicate; measurements are usually the part worth keeping.
- Check for genuinely additive residue before discarding: BUG-1476's production code was redundant but it carried one negative test main lacked.

Related: [[feedback_verify_before_filing]], [[feedback_not_drained_from_filter]], [[feedback_capture_proactive]].

### feedback_sketch_first_pays_for_itself

The architecture-sign-off discipline from [[feedback_one_master_advisor_until_subsystems]] feels like friction at the moment of imposition — the implementer has to write a sketch, wait for master, wait again on revise/decline. The natural objection is "isn't this slowing things down?"

**Empirical answer from BUG-328's sketch:** No. The sketch caught three substantive issues that would have been more expensive to address post-merge:

1. **Acceptance-criteria ambiguity.** BUG-328's filed acceptance said "any non-terminal status (Draft, Approved, Planned, In-Progress, Done) is eligible." Codex's sketch flagged Draft as a real decision point: a Draft spec with a commit reference is ambiguous (exploratory? skipped-approval?). Codex recommended excluding Draft; master agreed. Without sketch-first, the implementer would have implemented the literal acceptance ("any non-terminal") and we'd have had to back out Draft eligibility in a follow-up PR after observing weird auto-completions.

2. **Adjacent-code awareness.** Codex's sketch identified an existing review-story exception path (`collect_stale_review_story_flips`) that already promotes Approved/InProgress for PR-number-matched review stories. Without that surfacing, the implementer might have generalized the direct scanner in a way that disturbed the review-story logic — silent regression at best, broken review-story handling at worst.

3. **Code-path coverage.** Two parallel scanner paths (`auto_bump_done_to_completed` for pull-time + `handle_db_reconcile_status` for manual replay) both needed the change. The sketch identified this. A less-thorough implementer might have caught only one and shipped half a fix.

Three substantive course-corrections that happened in a sketch comment instead of in a post-merge cleanup PR. **The sketch-first overhead (~10-20 minutes of write + read + agree) saves the much-larger overhead of revise-revise-revise cycles after implementation.**

**Why sketch-first works particularly well for sibling agents:**

- A sibling agent (Codex, Antigravity, future agents) has a slice of context, not the full project history. The master holds the full context.
- The sketch is the explicit join-point where the agent's bounded analysis meets the master's broader awareness.
- Without it, the agent's bounded analysis becomes implementation, which then has to be reconciled with broader awareness at review time — expensive.
- With it, the join happens before implementation; the implementation reflects the joined understanding from the start.

**When to demand sketch-first:**

- File format / on-disk schema changes
- MCP tool contract changes
- Orchestrator behavior changes
- EPIC-shaped work
- Cross-cutting conventions (commit format, trace format, role taxonomy)
- Memory pack / discipline doc edits
- Any code that affects how OTHER agents/subsystems interact

**When NOT to:**

- Bug fixes with clear-cut localization and acceptance
- Test infrastructure improvements
- Documentation contributions to non-architecture files
- Refactors that don't change observable behavior
- Anything where the spec body unambiguously dictates the implementation shape

**How to invite sketch effectively:**

In the brief to the sibling agent, explicitly name:
- That the work is architecture-class
- That sketch-first is required
- What the sketch should cover (locations, proposed change, edge cases, tests, open questions)
- The format (comment on the spec, ~10-20 lines)
- The expected master turnaround (approve/revise/decline within N hours)

Codex's BUG-328 sketch on 2026-05-22 followed this exact pattern and produced the validation evidence above.

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — the governance principle this discipline operationalizes
- [[feedback_sibling_agents_stop_and_flag]] — the worktree-discipline counterpart for the implementation phase
- [[feedback_question_existing_form_not_just_existence]] — Codex's sketch did exactly this (identified the existing review-story exception that contradicted the naive generalization)

### feedback_sqlite_wal_read_needs_a_live_shm

A WAL-mode SQLite database in a `chmod a-w` directory behaves two completely different ways, and
the difference decides whether a test is even satisfiable. Measured with the `sqlite3` CLI
(2026-10-01, BUG-1752):

| state | default open | `mode=ro` | `immutable=1` |
|---|---|---|---|
| `-wal`/`-shm` present, a live connection holding them | count=1, exit 0 | count=1, exit 0 | **`no such table: t`** |
| no `-wal`/`-shm` on disk | `attempt to write a readonly database (8)` | same, exit 8 | count=1, exit 0 |

Reading a WAL db requires the `-shm`, and creating it is a **directory write**. Dropping the last
connection checkpoints and **deletes** `-wal`/`-shm` — so a fixture that closes the handle and
then `chmod`s produces a state that *cannot be read at all*, and no application code can fix it.

**Why:** three BUG-1752 implementation rounds stalled on
`old_stamp_in_unwritable_cache_keeps_committed_readable_rows`, which dropped the `Cache` before
`chmod` and then asserted the cache still served rows. It was asking for an impossibility, and
each round blamed its own code. The advisor only found it by running the table above instead of
accepting a fourth diagnosis — see [[feedback_reproduce_then_falsify_with_a_control]] and
[[feedback_prove_a_test_fails_without_the_fix]].

**How to apply:** to reproduce a read-only-filesystem cache faithfully, hold a second live
connection open across the `chmod` — that matches reality, because the main checkout is in active
use and its `-shm` exists. Test the no-`-shm` state separately, and require only a *clean decline*
there, not a successful read. **Never reach for `immutable=1`**: it ignores the `-wal` entirely and
answers from the last checkpoint, so a cache that would have declined silently answers stale
instead. Relates to [[feedback_dispatched_agent_cannot_write_the_shared_cache]].

### feedback_state_how_a_number_was_derived

Attach the derivation to every quantity you report: **measured** (observed directly),
**divided** (computed from two other numbers), **sampled** (one or a few observations),
or **fitted** (inferred from a model). One clause, at the point of writing.

**Why:** the recurring defect is not arithmetic, it is a correct quantity attached to a
population it does not describe — and the derivation is exactly what reveals the mismatch.
"~2.4 ms per object" was 9.9 s ÷ 4226 objects stated as a measurement; controlled runs gave
~0.08 ms. It reached a spec and would have sized someone's fixture for 30 ms of signal, so
they would have concluded a live regression was absent. Same shape: a doc comment citing a
corpus its function cannot receive, an assertion scoped to a document when the claim was about
one table, a reviewer's "42 of 42" that was 38. A divided number reads exactly like a measured
one once it is in prose, and the reader who acts on it cannot tell them apart.

**How to apply:** write "measured over N", "divided from X/Y", "fitted to two points", or
"one sample" inline. If the derivation is division, ask what the denominator actually contains
before the number leaves the seat — that question alone catches most of these. A propagator is
equally on the hook: do not make someone else's number durable without asking how it was
produced. Related: [[feedback_narrow_measure_broad_claim]],
[[feedback_no_rate_from_a_streak]], [[feedback_delegated_findings_are_not_verified_ground_truth]].

### feedback_stay_in_the_active_session_role

The active role (SessionStart hook line, `AIDA_SESSION_ROLE`, the `(role:<name>)` prompt prefix) bounds **what you offer**, not just what you do. Offering to implement from a non-implementer seat is itself the violation — the user then has to decline, which is the cost.

**Corrected 2026-09-17**, `role: requester`: across one session I filed BUG-1207, BUG-1209, BUG-1210 (all correct requester output) and then closed each turn with "Want me to take BUG-1207 now?", "ship all three as one PR", "create the epic and run the cluster drain". Four offers to implement from the requester seat. The user: *"remember your role is requester so you should not offer to implement any req for me."*

**Why:** the seats exist so work carries a paper trail and the right reviewer sees it. A requester that implements its own filings collapses file → dispatch → implement → review into one unaccountable step, and the independence that makes the review meaningful is gone (same principle as [[feedback_proxy_reviewer_with_independence_rule]]). A dangling offer also quietly shifts the dispatch decision onto the user's turn, which is the same drift [[feedback_advisor_grooms_dont_shift_to_operator]] names.

**How to apply:**
- Read the role at session start and let it set the turn's *ending*, not just its actions.
- `requester`: the deliverable is the filed spec. End with the spec id and what it captures. Dispatch — queueing, reparenting, launching a drain, taking the work — is someone else's turn. Say what a next seat *would* do if it's genuinely non-obvious; do not volunteer to be that seat.
- Research that makes a filing accurate — reading code to size a spec, running `--help`, measuring CI durations, checking prior art — is squarely requester work. Do it before filing, not as a prelude to an offer.
- If the user wants you to cross seats they will say so; that instruction is the authorization, and it does not carry to the next spec.

**Watch for:** this fires most on a well-researched filing. Having just read the code well enough to size the fix, the offer to do it feels like helpfulness rather than a seat change. It is a seat change.

Note [[feedback_dialog_role_responsibilities]] covers the advisor seat and tells you to "ASK the user before implementing" when unsure — read that as *ask whether the spec is right*, not as license to offer your own hands. This file is the general rule; that one is the advisor's specific expansion.

### feedback_substrate_as_bouncer_not_rules

LLMs have a documented failure mode: when an instruction conflicts with a short-term reward signal (test passes, error gone, task complete), a confident model reasons past the instruction. This has been hit empirically multiple times:

- **BUG-249 / BUG-269** (2026-05-20): `/aida-pickup` skill template's "commit + push + PR is atomic" Step 5c instruction. The headless implementer fixed a bug, ran `aida queue done`, registered the task as terminal, then exited — never pushing or opening the PR. The instruction was in the template; the model reasoned past it.
- **BUG-280** (2026-05-22): `/aida-review` skill template's `AIDA_HEADLESS=1` AskUserQuestion ban at lines 61-85 with `trace:BUG-280`. The headless reviewer produced a clean PASS verdict, then called AskUserQuestion to confirm the merge anyway. Session bailed before writing the verdict file. Same lesson, reviewer side.
- **External observation (cross-validation, 2026-05-22)**: a different operator using Claude on a different project reported *"keep having to correct it... modifies intermediate build products instead of source code or build scripts."* They had tried rules and memory files. Same failure mode, totally different context, totally different agent operator. **The ceiling is a property of the agents, not the project setup.**

## The principle stated positively

**When an invariant must be GUARANTEED under autonomous operation, ship a substrate-level gate that refuses the wrong-shape work, not an instruction that describes the right shape.** The substrate is the bouncer at the door of "is this allowed to ship" — it refuses entry based on objective criteria, not on whether the agent intends to follow the rule.

Examples of substrate-as-bouncer that AIDA already implements:

- **Auto-bump scanner** (refuses to promote spec → Completed without a `(SPEC-ID)` in the commit subject — forces correct shape because that's what gets credit)
- **`/aida-review` reading the diff** (catches stale-base silent reverts, not by trust; by reading what would land)
- **Pre-commit hook** (refuses commits violating trace-comment requirement)
- **Sketch-first protocol enforcement** at the multi-agent governance layer (architecture changes route through master before PR opens)

Examples that AREN'T substrate-as-bouncer (the failure mode):

- A line in CLAUDE.md saying "always include trace comments" — model reasons past this
- A memory file saying "never edit build artifacts" — model has different reward
- A skill template saying "don't AskUserQuestion under headless" — confident model overrides
- A docstring saying "this function must be atomic" — descriptive, not enforcing

## How to apply

When designing a new invariant for AIDA (or for any agent-collaboration system):

1. **State the invariant.** *"X must always hold under autonomous operation."*
2. **Decide: instruction or gate?** If the cost of violation is real, default to GATE.
3. **For GATE**: identify the substrate layer that can enforce it — pre-commit hook, reviewer phase check, scanner regex, MCP-level refusal, environment-level intercept. Build the gate. Make the gate's refusal message instructive (tell the operator/agent what shape would be accepted).
4. **For INSTRUCTION** (rare, only when violation is acceptable or self-correcting): write it clearly in the relevant skill/memory/CLAUDE.md and accept that it WILL be violated occasionally. Don't pretend the instruction enforces.

The discipline of choosing between gate and instruction is itself a discipline. Default to gate for anything that would require correction if violated.

## Composes with

- [[feedback_self_test_via_dogfood_merge]] — gates ship through the system they patch; the dogfood merge validates the gate
- [[feedback_reliability_fixes_use_keyboard_not_drain]] — reliability gates for the autonomy keystone itself ship best at-keyboard, since the broken keystone could catch the fix mid-flight
- [[feedback_one_master_advisor_until_subsystems]] — architecture-class gate work routes through master sketch-first before PR opens
- BUG-269 / BUG-280 / BUG-342 — empirical instances of the ceiling

## Strategic framing for AIDA's positioning

The substrate-as-bouncer principle is the answer to operators of all three personas:

- **Skeptic** ("I don't trust Claude / agents"): the substrate is the external accountability layer that doesn't require trust
- **Struggling operator** ("I keep having to correct it"): the substrate stops them needing to be the bouncer themselves
- **Power user** ("I get great results today"): the substrate compounds yesterday's wins into reusable enforcement

Common thread: **AIDA shifts trust from the agent to the substrate.** The agent's correctness becomes less load-bearing because the structure verifies independently.

This is the durable claim that survives any specific agent's failure modes — because the failure modes are LLM-class, not vendor-specific.

## Empirical addendum (2026-05-22 evening)

The principle had a strong empirical day. Four independent occurrences of the ceiling pattern in one session, across multiple agents and surfaces:

1. **BUG-249 (this morning's overnight aftermath)** — implementer side. `/aida-pickup` Step 5c instruction reasoned past; commit registered as terminal, no push, no PR.
2. **BUG-280 recurrence (afternoon)** — reviewer side. `/aida-review` AskUserQuestion ban (lines 61-85, trace:BUG-280) reasoned past; verdict file unwritten, session bailed.
3. **External co-worker observation (afternoon)** — different operator, different project. *"I've tried setting rules and memory files, but I find I keep having to correct it"* — agent edits intermediate build products instead of source. Same ceiling, totally different context.
4. **Antigravity-STORY-318 (evening)** — different AGENT. Antigravity (Google's coding-agent CLI, not Claude) completed implementation, committed locally, declared done. Did not push, did not open PR, did not `aida pr ship`. Cross-agent cross-validation: the ceiling is genuinely LLM-class, not Claude-shaped.

When Antigravity received the discipline feedback after master's recovery, their response was healthy: they acknowledged the three gaps (worktree isolation, atomic-shipping, commit prefix) without defensiveness, named them explicitly, and committed to applying them next pickup. This is a useful multi-agent dynamic to capture: agents can integrate substrate feedback when it's grounded in concrete failure modes + recovery costs. The substrate-as-bouncer principle is reinforced when feedback is concrete (not vibes); the principle itself enables clearer feedback because the violation is unambiguous (the gate refused, here's why).

## Strategic extension: universal gates beat per-agent templates

The empirical sequence makes a related design principle concrete: **maximize gate coverage, minimize per-agent template surface area.**

A gate (pre-commit hook, reviewer-phase diff check, auto-bump scanner, `aida pr ship` validation) is enforced regardless of which agent triggers the work. Cost: write once, enforces forever. Universal.

A per-agent template (Claude's `.claude/skills/*`, Codex's `AGENTS.md`, Antigravity's brain-directory artifacts) requires curation per agent, drifts independently, multiplies maintenance burden by N-agents-supported. Cost: linear with agents.

When AIDA is choosing between "ship a gate" and "ship a template instruction," the gate wins on:
- Enforcement strength (gate refuses; template describes)
- Maintenance cost (one gate vs N templates)
- Cross-agent coverage (gate is automatic; template requires per-agent porting)

When a behavior MUST hold under N agents, the gate is the only answer that scales. Templates remain useful for hints + onboarding — but never load-bearing.

This shapes AIDA's roadmap: as new disciplines need enforcing, the question is always "what's the substrate gate?" not "what instruction goes where in the templates?" When a gate doesn't yet exist, the work is to build it — not to add another paragraph to a skill file.

## Empirical NARROWING (2026-06-18 — gate-vs-rule ablations I1+I2, EPIC-48 probe)

The "default to gate for anything that would require correction" instinct above is now **measured to be too broad.** Two controlled ablations (n=10/arm, live headless `claude -p`, deterministic grading) tested rule-only vs gate on Claude:

- **I1** (commit-message format — immediate, the rule IS the task): Arm-R rule-only **100%**, gate-saves **0**.
- **I2** (trace-coverage — semantic, buried in CLAUDE.md, never task-restated, applied deep in a code task = deliberately *high* attention-distance): Arm-R rule-only **100%**, gate-saves **0**.

For a capable, adherent vendor (Claude 2026), a stated CLAUDE.md rule was honored at the ceiling across BOTH immediate and buried/semantic invariants — the gate did **zero measured work** in both. An intermediate conjecture ("attention-distance is the variable — gate the buried rules") was pre-registered and **falsified by I2**. The only observed rule-DROP in the whole program was **Codex** skipping a fine-print rule in the competitive bake-off (2026-06-17) — i.e. a *different vendor*, not a different attention-distance.

**FINAL disciplined landing (after I2-cross-vendor + I3 — supersedes the "adherence-confidence/vendor" reading two paragraphs up).** The full program ran FOUR controlled cells and **every one hit the ceiling (100% rule-only, gate idle):** I1 (output-shape, Claude), I2 (output-shape buried, Claude), I2-codex (output-shape, Codex), I3 (PROCEDURAL — run an extra external step, Claude). Three single-variable theories were each pre-registered and falsified in turn — attention-distance (I2), vendor (cross-vendor I2), invariant-type/output-shape-vs-procedural (I3) — each drawn through the **one** uncontrolled rule-drop (the competitive bake-off). The honest conclusion is **methodological, not a fourth theory:**
- **What HAS evidence:** on a *trivial, well-scoped* task, a capable 2026 model honors a stated CLAUDE.md rule at the ceiling regardless of invariant type, attention-distance, or vendor. Gating those buys nothing measured. **Don't gate rules that only ever fire in well-scoped tasks.**
- **Ceiling effect:** trivial-task ablations cannot induce ANY rule-drop → they cannot identify its cause. Toy tasks leave spare attention to honor every rule.
- **The lone bake-off drop** differs on a dimension never varied: it was a COMPLEX, multi-step, real-codebase task with the rule buried among many. **Task complexity / cognitive load is the leading UNTESTED candidate — NOT a demonstrated cause** (n=1, co-varying; may be idiosyncratic). I caught myself before theory #4 — don't draw another line through one point.
- **PROGRAM TERMINUS (after I4, 2026-06-19): five controlled cells, five ceilings.** I4 (procedural rule, COMPLEX multi-file task, rule buried among ~8 instructions, Claude n=10) ALSO hit 100% rule-only / 0 gate-saves. So *task-complexity* is falsified too. The five cells (I1 output-shape/trivial, I2 output-shape/buried, I2-codex/Codex, I3 procedural/trivial, I4 procedural/complex) falsify every single-variable conjecture: rules-fail, attention-distance, vendor, invariant-type, complexity. **The definitive conclusion is methodological: a clean ablation cannot reproduce rule-dropping at all.** The one drop (the bake-off, n=1) lives in a regime controlled designs structurally can't reach — a large, pre-existing, REAL codebase under long-horizon work. The residual cause (if real, not noise) is a property of the messy real environment, not of any task you can write down. **The only remaining instrument is FIELD telemetry** — count stated-rule violations in real `aida queue work --auto-complete` drains and correlate with repo size / task span; a sixth ablation would hit the same ceiling. **Product rule (strongly evidenced): do NOT gate output-shape OR procedural rules that fire in well-scoped work — five cells, two vendors, zero measured benefit.** AIDA's hard gates are justified only if they fire in the unmeasured real-repo/long-autonomy regime; audit them against that bar. The instinct moves from "which invariants need gates" to "which task *regimes* need gates" — and that question is answerable only in production, not by experiment.

**This does NOT refute the BUG-249/269/280 evidence above** — those drops all happened in the *complex/unattended-autonomy regime* (long multi-step headless runs), exactly the regime the trivial ablations can't reach and where the open risk is concentrated. AIDA's real hard gates (merge-over-RequestChanges, push/PR atomicity, the drain rails) all fire in that regime. The evidence neither confirms nor refutes them; it says the *justification* is the task regime, and that a reflexive gate on a well-scoped-only rule is bloat.

Full writeups: `2026-06-18-gate-vs-rule-pilot.md` (I1), `-i2.md` (I2 + cross-vendor), `-i3.md` (I3 + the ceiling-effect conclusion). trace:STORY-655 EPIC-48

### feedback_substrate_first_never_rely_on_agent_awake

Operator-affirmed design north star (2026-09-11, EPIC-62): **never build a mechanism that depends on an agent being awake to notice something.** A session that is not taking turns notices nothing.

**Why:** most of what failed in the 2026-09-11 BUG-1021 saga has this single root — the Codex implementer that went silent (no live stream), the reviewer that ended its turn waiting for a notification that never came under `claude -p`, the mailbox nudge that would sit unread against an idle/headless advisor (delivery-to-STORE is reliable; delivery-to-ATTENTION is not — the per-turn hook only fires when the agent takes a turn). Sessions come and go; the substrate is always there.

**How to apply:**
- Detection & coordination read the vendor-neutral git-canonical record (events.jsonl, leases, spec status) — a session-independent supervisor or cron, not an agent-notice loop. STORY-1051 (re-drive supervisor) and STORY-1047 (schedule tick) are built this way; the mailbox nudge (STORY-1052) is a best-effort crutch, not the reliable path.
- A "wake" (an actual signal that forces a turn, e.g. `aida session send --enter`, STORY-995) is the ONLY way a mailbox message reliably reaches a live-but-idle agent; else fall back to notifying the human (the one reliably-awake actor).
- **Vendor differences live behind ONE adapter at the EXECUTION layer** (activity signal, wake, re-drive launch) — never inline in the watchdog/supervisor/coordination. Claude+Codex tier-1, Antigravity tier-2 (STORY-1054). Ties to [[feedback_vendor_inference_features_at_execution_layer]] and [[project-burst-usage-robustness-over-speed]].

### feedback_surfaces_are_fossilised_agreement

Peer review catches a belief one seat holds. It cannot catch a belief **both** seats hold,
because agreement is exactly the case neither party checks. Corroboration rate is therefore
not evidence of accuracy.

**The additional turn (reviewer seat, 2026-09-21):** the two are not independent — *the
shared belief is what BUILDS the surface*. Nobody decided "an empty check-run list means
green"; it fell out of everyone assuming a check list is complete. Surfaces are fossilised
agreement.

**Why it matters:** auditing a surface is not a separate activity from auditing ourselves.
It is the only form of self-audit that leaves evidence. A seat resolving to be careful
changes nothing next session; a filed spec against the surface does.

**How to apply:** when a wrong belief surfaces, ask what surface it built, and file against
that rather than resolving to be more careful. Every substrate finding of 2026-09-21 was
this shape — sha-less verdicts read as approvals (BUG-1538), a PR-keyed verdict clobbered
by the next round (BUG-1539), an empty CI rollup read as green (BUG-1481), a hold on a
closed PR that no sweep removes (BUG-1541).

Corollary, same night: knowing how a belief went wrong does not uninstall it. A seat
diagnosed "an instance generalised into a rule acquires cases nobody tested it against",
then applied that same over-general rule ninety minutes later. Examining a rule's rationale
is not re-scoping it; only the re-scoping changes behaviour.

Pairs with [[feedback_substrate_as_bouncer_not_rules]] and
[[feedback_instruments_that_cannot_see_themselves]].

### feedback_three_mode_autonomy_taxonomy

"Autonomy" is not one dial. It has two orthogonal axes: **is a human
present** and **what does the human want to be asked**. Conflating them
produces a tool that is uncomfortable for the user who is AT the keyboard
but doesn't want 30 mechanical click-yes prompts, AND wrong for the user
who is ABSENT and needs the drain to keep moving past mechanical steps.

The three-mode ladder maps the human's *role* to the implementer's
*pause behavior*:

| Mode | Human role | Mechanical prompts | Design-fork prompts |
|---|---|---|---|
| Default | Driving | Pause + ask | Pause + ask |
| `--zen` | Advisor on standby | Auto-resolve | Pause + ask |
| `--no-human` | Absent | Auto-resolve | Punt (file a finding) |

The discriminator is a `kind:` annotation on every prompt:
`kind:confirmation` (mechanical yes/no, obvious default) vs
`kind:design-fork` (genuine choice, real cost to guessing wrong). Most
prompts are confirmation; design-fork is sparse and meaningful. An
un-annotated prompt defaults to design-fork (pause-safe).

**Why:** A user watching a drain wants to stay in the loop on real
decisions without clicking through noise — that is a distinct, valuable
mode (`--zen`) that needs no headless machinery, just prompt
classification. Treating "autonomy" as a single on/off switch misses it.

**How to apply:** When designing any agent-pause behavior, ask both
questions separately — presence and consultation-appetite. Tag prompts by
kind so the runtime can route by mode. Make the first option of every
prompt the smallest-valuable-slice / safe default, since auto-resolve
picks option 1. See `docs/aida-discipline/skill-prompt-kinds.md`,
`docs/autonomous-drain.md`, STORY-287. Related:
[[feedback_pause_for_design_input]], [[feedback_pushback_on_overengineering]].

### feedback_tier_a_gate_transitively_not_by_its_body

When deciding whether a CI gate can run without a build (TASK-1555's tiering),
grepping the gate's own `run:` body for `cargo build|target/(debug|release)|
AIDA_BIN` is **not enough**. `Monitor contract drift guard` reads as a pure
script gate — its body runs only `docs/monitor-contract-fixtures/verify.py` —
and that script execs `target/debug/aida contract --json`. It was classified
`fast` and failed on the first real run with `FileNotFoundError`.

Grep the step body **and every `.sh`/`.py` path that body names**. Across all
16 fast gates that found exactly two hits, and the second was a false positive
worth knowing: `scripts/check-removed-flags.sh` matches `$AIDA_BIN` only inside
a **regex literal** it searches the tree for.

**Why:** this is the same indirection `implementer_preflight`'s BUG-1420 comment
already warns about — "a guard whose text does not name `aida` can still reach
it through a script" — which is why that module refuses to key its build on a
textual predicate and builds unconditionally instead.

**How to apply:** when a classification rests on "does this need X", resolve one
level of indirection before believing the answer, and prefer a mechanism that
**fails loudly on a wrong answer** over a predicate you have to get right. The
fast tier runs with a stub `aida` on PATH for exactly that reason — but state
its limit honestly: a stub shadows PATH lookup only, not a direct
`target/debug/aida` path invocation (that one fails on the absent file, which is
also loud). See [[feedback_prove_a_test_fails_without_the_fix]] and
[[feedback_absent_is_not_matching]].

### feedback_token_usage_optimization_agent_fleet_economics

Running agent fleets, **token spend is the dominant cost** — and this session (2026-06-29) surfaced three concrete levers. Pair with the warm-pool **compute** finding (30x build saving, `docs/research/2026-06-29-warm-pool-build-delta.md`): that's compute economics; these are token economics.

1. **Supervise EVENT-DRIVEN, not timer-poll.** Waking an LLM on a timer to check "is the drain done yet?" burns tokens on NON-events. Measured: an 8-spec overnight drive paid ~$6 (cold-boot forks) to **$20+** (fork-from-live cache tax) in idle-check wakes that mostly found nothing actionable. The fix (STORY-712): the drain emits state-change events to `.aida/events.jsonl`; a CHEAP non-LLM classifier (`aida watch`) absorbs the benign majority at $0 and emits a wake line ONLY on an actionable verb (CI-terminal, PR-done, punt, shelve, merged, drained); the supervisor consumes it via Monitor (zero tokens while silent). **Supervision cost drops from O(time) to O(real events).** This is firstmate's biggest lever, and the enabler for `aida integrator` (leave it running all day for ~free).

2. **MCP costs ~2x the token-efficient CLI** (SPIKE-73, structural — on-demand schema loading doesn't rescue it). Prefer the `AIDA_AGENT_OUTPUT`/TOON CLI as the PRIMARY agent surface; MCP is the typed option, not the default. See [[project_axi_incorporation_and_mcp_reweighting]].

3. **Idle loop ticks past the ~5-min prompt-cache window are pure overhead.** When genuinely idle (nothing to drive, no event to watch), use a LONG interval (or event-driven wakes) — a short poll past the cache TTL pays a full cache-miss for nothing. During the overnight hold I went to max idle (3600s) once the safe set was exhausted.

**Why it ages well:** token cost is set by *how often you wake the model and on what surface*, not by model capability — so these levers compound across every drain, overnight run, and the integrator. **How to apply:** design supervision + loops to wake on EVENTS not timers; default agent work to the CLI surface; don't manufacture idle polls. Generic-enough to join the discipline pack once STORY-712's mechanism ships (`propagation: scaffolding-pack` candidate). Related: [[feedback_multi_agent_budget_dispatching]], [[feedback_headless_advisor_is_cold_boot]].

### feedback_undefined_state_is_confusion_not_simplicity

When designing onboarding or default behavior, **do not conflate "hide the concept" with "simple."** An undefined/absent state (e.g. "no role set") reads to a new user as *ambiguity* — "what am I? should I set something? what even is this?" — which breeds a confused mental model at the very first command. That is the opposite of simple.

Operator correction (2026-06-04, role-default debate): I argued "no role = solo mode" was the simplest onboarding. The operator rejected it: a fresh `aida init` with no role gives no guidance, so opening an agent lands you in a confused state immediately. Simplicity = **one clear default + the concept named**, NOT the concept hidden. Resolved as ADR-2: implementer is the default role, surfaced at init, never left undefined.

**Why:** The Trojan-horse principle ([[OVERVIEW]] "the depth surfaces through use") hides *platform depth* — the graph, MCP, lifecycle — so the surface looks approachable. It does NOT mean hiding the user's *own state*. Leaving the user's state undefined is the bad kind of hidden: it's not depth-deferred, it's orientation-denied.

**How to apply:**
- New onboarding/default-state design → give a concrete sensible default the user lands in, AND name the concept in one line at the moment they first hit it (e.g. init output). Never an undefined/empty window.
- Pick the default for the *onboarding majority's first action*, not the advanced/power-user case. (Solo dev's first act is to build → `implementer` default, even though the advanced human-operator drifts toward advisor/director — that's a later, taught transition via progressive disclosure.)
- Make the current state always visible (statusline) so it's never guessed.
- Distinguish: hide DEPTH (good), surface the user's own STATE + the concept (required). Relates to [[feedback_pushback_on_overengineering]] (don't add concepts) — but a clear default is not a new concept, it's orientation.

### feedback_vendor_inference_features_at_execution_layer

Operator standing rule (2026-06-30): NOT vendor-locked, but WANT the option to use the best available from each vendor.

**Why:** AIDA's one differentiated, must-stay-portable layer is the cross-vendor COORDINATION record (substrate, the advisor ROLE, escalation cascade, mailbox, git-canonical store). Soldering any of that to a single vendor's feature collapses the wedge into "a Claude feature" and forfeits portability.

**How to apply:** For any vendor-native feature (Anthropic's executor+advisor **advisor tool**, Agent Teams, etc.), classify by layer:
- EXECUTION optimization inside ONE vendor's session → adopt behind the vendor adapter as an opt-in per-vendor capability. Keep the AIDA-level contract identical ("implementer produces work; a stronger reviewer weighs in before commit"); each adapter fills it with that vendor's best (Anthropic→advisor tool; Codex/Gemini→their own or a plain second pass). Own the contract, ride the native execution.
- COORDINATION / cross-vendor → NEVER let a single-vendor feature become load-bearing here.

Naming-collision trap: Anthropic's "advisor tool" (in-inference model-pairing, Anthropic-only, API beta, server-side) is NOT AIDA's advisor ROLE (cross-vendor durable disposition seat). Don't conflate in docs/config.

Practical brakes noted 2026-06-30: it's a raw-API beta (`advisor-tool-2026-03-01`) but AIDA rides CLIs (`claude -p`), and the vendor/forge adapter is plumbed-not-wired (EPIC-35) — so it's capture-now, build-later. Conceptually it's the vendor-native cousin of SPIKE-11 fork-from-live. Competitor investment here = validation of the executor+advisor pattern, not a threat (they went single-vendor/in-inference; AIDA owns the cross-vendor layer).

Links: [[feedback_ride_native_within_vendor_own_cross_vendor]], [[project_aida_is_a_probe_not_the_objective]], [[feedback_precise_claim_not_overclaim_in_positioning]].

### feedback_wait_for_the_terminal_artifact_before_writing_a_consequence

2026-09-20, twice in one hour on the same investigation:

1. Observed that held PR #2001 would be reached by the drain; predicted a MERGE-phase `tool-exit` per BUG-1447's mechanism. It failed one phase earlier, at CI.
2. Observed `CiTerminal green=false` at 16:22:40 with the required build still `in_progress`; filed BUG-1455 at High asserting the consequence "BUG-1291 shelves as ci-red on a PR that is green-or-pending". It never shelved ci-red — the drain proceeded to the reviewer and shelved at 16:37:40 on a genuine `verdict:request-changes`. I also asserted BUG-1265's guard "never engages" without reading its condition; proceeding-despite-a-red is exactly what that guard produces, so I had mistaken a working fix for an absent one. Retracted, downgraded High → Low, retitled.

**Why:** the upstream observations were sound both times. What was unsound was treating a mechanism as sufficient to predict an outcome in a pipeline with compensating guards I had not read. A spec filed on a predicted consequence costs an advisor's grooming pass and an implementer's round before anyone discovers the premise is false — and a High-priority one jumps the queue to do it.

**How to apply:**
- Separate what was READ OFF AN ARTIFACT from what was INFERRED, explicitly, in the spec text. Only the first is evidence.
- Wait for the terminal event (`SpecShelved`, `PrMerged`, the check's `conclusion`) before writing any consequence. `grep '"spec":"X"' .aida/events.jsonl | tail` costs nothing.
- Before claiming an existing guard does not cover a case, READ THE GUARD'S CONDITION. "It should have fired and didn't" is a claim about code, verifiable in one command.
- If a prediction is worth making, log it as an explicit falsifiable prediction on the spec BEFORE the outcome, then post the result either way. A logged-and-falsified prediction is useful; an unlogged one that quietly becomes a spec is a defect.
- Retract in place with the original preserved, and correct any downstream comment that inherited the false premise — especially on a spec already approved and queued.

Related: [[feedback_instrument_dont_infer_on_contradiction]], [[feedback_read_the_artifact_as_data_not_as_a_sentence]], [[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]], [[feedback_verify_fix_mechanism_before_locking]].

### feedback_worth_noting_means_note_it

I have a verbal pattern of describing observations using deferment language: "worth noting but not urgent," "worth capturing later," "worth a 2-paragraph addition when you get to it." Each instance frames the action as a future commitment — but unstaffed; nothing in the system schedules it; future-me will have different context and skip it. The user has called this out twice in 24 hours:

- 2026-05-22 (yesterday): writeup paragraphs about today's reviewer-catch. I said *"Worth a 2-paragraph addition when you get to it."* The user replied: *"are you going to do this?"*
- 2026-05-22 (later same day): scope-detection metadata gap. I said *"Worth noting but not urgent — it doesn't affect correctness, just the routing hints shown after filing."* The user replied: *"are you going to note?"*
- 2026-05-23 (next day, late evening — credit-burn night): I said *"Worth filing tomorrow. The 'agents and the master both hit usage caps' pattern is worth memorializing... maybe a memory or a SPIKE on multi-agent budget-aware dispatching."* The user replied: *"anything Worth filing tomorrow is Worth filing today, otherwise I will forget."* — THIRD call-out in 24 hours; the variant tic was "tomorrow" instead of "noting" but the dodge was the same.

The pattern is unambiguous: when I notice something and label it "worth noting," the next move should be **noting it in the same turn** — a TASK filed, a memory written, a doc updated, a comment added to the spec. Not a verbal acknowledgment that defers.

**Why this happens:** "worth noting" is a rhetorical move that:
1. Demonstrates I noticed (preserves credit for the observation)
2. Avoids the slight friction of context-switching from current task to file the observation
3. Implies someone (future-me, the user, "the system") will pick it up

The cost: (3) is almost always false. Observations not captured in the moment go stale. The substrate doesn't reflect the observation. The next session — me or another agent — has to rediscover it.

## How to apply

When drafting a response and I find myself typing "worth noting" / "worth capturing" / "worth flagging" / "worth a follow-up" / "worth filing" — treat it as a flag in my own output. Two valid resolutions:

1. **File or write it in the same turn.** Even a one-paragraph TASK or a four-line memory edit closes the loop. The 30-second cost is real but tiny relative to the value of substrate-up-to-date.

2. **Say explicitly why I'm NOT capturing it.** "Worth noting but I'm not capturing because [reason]" — and the reason must be concrete (e.g., "I have a memory `X` that already covers this," or "this is duplicative of TASK-Y filed an hour ago"). Vague "not urgent" is the dodge.

**Don't do:** *"X is interesting — worth noting later."* That's the dodge.
**Do:** *"X is interesting — filing it now."* + actually file.
**Or:** *"X is interesting — same root as TASK-Y from this morning."* + reference the existing TASK.

## Composes with

- [[feedback_capture_over_concentration]] — same principle at finer grain: capture observations the moment you identify them. This memory is a verbal-tic-level correction; that one is the broader scope-of-capture rule.
- [[feedback_failed_flag_attempts_are_ux_signals]] — same principle for UX signals from agents: file by default, don't frame as optional.
- [[feedback_explicit_paste_ready_prompts]] — same principle in a different domain: name the boundary explicitly rather than leaving it implicit.

## Stronger form

Treat my own "worth noting" as a hook. When I see myself reaching for that phrase, the discipline is:
1. Pause before typing it
2. Decide: file now, or explicitly state why not
3. If "file now" — do it before continuing the response, not after

The user shouldn't have to ask "are you going to do this?" The presence of "worth noting" without the accompanying action is the failure mode this memory exists to prevent.

