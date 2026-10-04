# Spec authoring: separate shipping proof from outcome measurement

Acceptance on the spec that ships a change must be verifiable before that
change merges. Keep the mechanism, its tests, and observable fixture behavior
on the shipping spec.

An outcome that can only be measured after shipping belongs on a follow-up
measurement spec. Make that spec blocked by the shipping spec and give it:

- the outcome to measure;
- the post-deployment window (for example, the next 20 specs); and
- the threshold that would falsify the change.

Do not weaken or delete the outcome. Split it so shipping cannot deadlock on
evidence that cannot exist yet.

## Worked example

TASK-1291 originally required: “Measured on the next 20 specs: median
rounds-to-merge drops; recorded on this spec as the acceptance evidence.” A
reviewer could never verify that before merging the change, while merging was
blocked on verification. The shipping spec should instead verify the round
tracking mechanism and its behavior on a fixture. A blocked follow-up spec
should measure the next 20 specs and record whether the median fell by the
stated threshold.

`aida criteria` flags common post-deployment shapes as advisory and recommends
this split. The heuristic is intentionally shallow; a human confirms the flag.

<!-- trace:TASK-1293 | ai:codex -->


## Migrated Lessons

### feedback_file_forks_as_decisionrequests_not_chat

When the advisor hits a fork it can't resolve and needs the operator's judgment, **file it as a structured DecisionRequest** with `aida questions ask <spec> --question … --choice 'label|consequence|resolution' … --recommend N --rationale …`. Do NOT relay the decision in chat, hold it as a held/draft PR, or bury it in a spec comment.

**Why:** `aida questions` IS the designed "decide" drain (the third drain: implement=`burndown run`, decide=`questions answer`, review=`aida review`). Relaying a fork in chat bypasses that surface — the decision becomes invisible to `aida questions list` / `aida human`, un-drainable as a pure pick, and dependent on the operator remembering the chat. Filing it makes it a first-class, enumerable, answerable item (ask-ahead / answer-async).

**How to apply:** the moment you'd say "your call: A or B" in prose, instead `aida questions ask` it (≥2 choices, deterministic `resolution` tokens — `noop` when the choice only records which approach to implement). Then the operator drains it with `aida questions answer <spec> <choice>`. Surface it in the decision inbox, not the conversation.

2026-06-13: held BUG-530 (A/B fix approach) and STORY-607 (#861 Book nesting) in chat/PR-drafts for many turns; the operator asked "should this be in `aida questions`?" — exposing that I'd bypassed the DecisionRequest system entirely. Filed both retroactively.

Pairs with [[feedback_precise_lifecycle_vocabulary]] (review vs decide vs merge are distinct asks) and the three-drains symmetry.

### feedback_file_spec_before_tracing

When implementing a new piece of work that needs `// trace:<SPEC-ID>` comments, **file the spec first** (`aida add …`), read the **returned** id, and only then write the trace comments with that real id.

**Why:** guessing "the next id will be TASK-N+1" is unreliable — id allocation consumes numbers between filings (other specs, blocks, concurrent sessions), so the guess is routinely off by one. Twice in the novice-first loop (TASK-727→guessed 728, TASK-728→guessed 729) the trace comments pointed at a non-existent spec, which (a) broke the very `aida show <id>` → linked-code feature the work was about, and (b) cost a separate correction commit each time. The commit `(SPEC-ID)` trailer guessed wrong too, so the auto-bump missed and the spec needed a manual `--status completed`.

**How to apply:** the order is **file → capture id → trace → commit with the real `(id)` trailer**. If you've already written code with a guessed id, fix it before committing — `grep -rn "trace:TASK-<guess>"` and rewrite to the real id. The five-second reorder beats the correction-commit churn every time.

### feedback_grep_the_whole_crate_the_filed_list_undercounts

When a spec enumerates sites (call sites, assertions, files), **re-run its survey command before
implementing**. BUG-1731 listed six bare wall-clock assertions in `aida-cli-lib`; the grep in its
own acceptance criterion returned **nine**. The three missed ones included the most load-fragile
assertion in the tree (`merge_lock.rs:315`, 150ms against a 200ms deadline) and a second site in
a file the spec already cited.

**Why:** a single-line grep misses multi-line `assert!` forms, and a survey run against part of a
crate reads as a survey of the crate. Implementing only the filed list leaves the defect class
alive while the spec closes as complete.

**How to apply:** run the criterion's grep first, diff it against the filed table, and post the
correction as a comment on the spec (not a silent description rewrite) so the filed-vs-found gap
stays auditable. Also check what the grep returns that is *not* a defect — production code and
bounded-wait loop conditions legitimately match, and converting those would be wrong; state the
precise post-change predicate instead of claiming the raw grep returns nothing.

Related: [[feedback_verify_acceptance_matches_primary_caller]],
[[feedback_source_scanning_guards_need_the_full_suite]].

### feedback_no_e2e_drain_test_on_real_spec

Manually verifying a presence consumer (STORY-561), I ran `aida queue work STORY-561 --auto-complete` against the real in-progress spec to check behavior. The `--no-human=None` (interactive) code path launched a real auto-complete drain; phase 1 failed (scope owned by my own lease) and the resilient-drain **parked STORY-561 → NeedsAttention** and filed a `failure:implementer` finding. No commits/worktrees were damaged, but I had to restore status to in-progress (which cleared the derived finding).

**Why:** drain/orchestrator commands mutate the substrate as a side effect of *running*, not just on success. A failed drain parks the spec and files findings. Testing against a real spec pollutes its state + the findings inbox.

**How to apply:** to verify a drain/auto-complete/escalation behavior by hand, prefer paths that **bail before phase 1** — e.g. away + `--no-human=both` hits the kickoff scope-ack gate and bails non-TTY (safe, proves the wiring). For the fall-through (interactive) path, rely on the **pure unit test** (`resolve_drain_mode` etc.), not an E2E run. If an E2E run is unavoidable, use a **throwaway spec**, never a real in-flight one. Pairs with [[feedback_verify_acceptance_matches_primary_caller]] and [[feedback_reliability_fixes_use_keyboard_not_drain]].

### feedback_refinements_must_be_acceptance_criteria

After filing a TASK/BUG/STORY, refinements often arise — clearer wording, updated examples, tweaked glyphs, corrected vocabulary. These refinements are typically captured as **comments on the spec** rather than edits to the original acceptance list. **Comments aren't binding on implementers.** The implementer's contract is the spec's `## Acceptance` checkbox list; comments are background context.

If a refinement needs to ship, it has to become an acceptance bullet — either:
- **Edit the original acceptance list** to reflect the refinement, OR
- **File a follow-up TASK** that explicitly supersedes the original behavior

**Why** (2026-05-16): I refined TASK-260's Path/Action/Why table design via a comment on TASK-260 specifying the new glyph set (`▶ ⇒ ⏸` replacing `▶ ⏵ 🚪`). The implementer of TASK-260 shipped using the ORIGINAL glyphs because:
1. The original acceptance criteria didn't mention specific glyphs
2. The skill templates already used `▶ ⏵ 🚪`
3. The refinement comment was descriptive prose, not an acceptance bullet
4. The implementer correctly followed the spec they were given

User caught the discrepancy after the menu rendered the old glyphs in the wild. Filed BUG-116 for the propagation.

**The trap**:

Dialog role files a spec → starts iterating with the user → refinements emerge → captures them as comments → considers the work done. The implementer never sees the comments as binding; they ship the original spec.

**How to apply:**

When iterating with the user on a spec that's already filed:

1. **For substantive refinements** (changes the implementer's output): edit the original acceptance list. Use `aida edit <ID> --description "..."` if AIDA supports description rewrites, OR file a follow-up TASK that supersedes the original.
2. **For background context** (rationale, examples, design forks considered): comments are fine.
3. **Discriminator**: would the implementer's output differ based on this refinement? If yes, it belongs in acceptance. If no (rationale only), comment is fine.

**Even better — verify after shipping**:

When a refinement-touched spec ships, **run the resulting code/output and confirm the refinement landed**. If it didn't, that's a BUG worth filing immediately (like BUG-116 here). Don't assume the comment did its job.

**Composes with:**

- `feedback_pause_for_design_input.md` — implementer pauses for input on design forks; ALSO the input needs to be captured in a binding form
- `feedback_trust_reviewer_over_dialog_intuition.md` — same family: don't assume your written description matches reality; verify the artifact
- BUG-116 — the propagation BUG that surfaced this lesson

**Discovered via:**

User caught 2026-05-16: *"For that table displayed, the glyphs look like the ones we have previously, I thought we updated those or was that a different table?"* — surfaced that TASK-260's refinement comment didn't ship with the implementation; the implementer correctly followed the original acceptance criteria.

### feedback_rerun_the_acceptance_measurement_yourself

When a brief makes a **number** the acceptance ("0 `/proc` opens when the var is unset, at
most 499 when set"), re-run that measurement yourself before accepting. Do not read the
number out of the implementer's report.

2026-09-29, TASK-1499: codex reported "0 opens with `AIDA_BIN` unset, **4** with it set" and
called both a pass. Re-running it in the advisor seat gave **489**, not 4 — two orders of
magnitude off, and it changed the verdict, because 489 meant the common fleet path was still
~80 ms instead of the 10 ms baseline. The implementer had not fabricated anything: the brief's
recipe put `env -u AIDA_BIN` on the **grep** line instead of on the traced run, so the traced
process never had the variable set. It followed the recipe literally and got a number that
looked like a pass.

**Why:** a measurement recipe is part of the brief, and a defective one fails *toward* looking
green. The implementer cannot catch it — the recipe is the spec of what "correct" means, so
following it faithfully is exactly what produces the wrong number. Only re-running it against
the built artifact catches it. This is the execution-verification discipline applied to the
advisor's own instrument rather than to the implementer's claim.

**How to apply:** (1) In the brief, put the env var on the **process being traced**, and show
the full command line rather than describing it. (2) Ask for the *before* number as well as the
after — "it was 992, get it under 40" makes a bogus 4 visibly wrong. (3) Re-run the measurement
in the advisor seat before the verdict; it is one command. (4) When the number is wrong because
the recipe was wrong, say so plainly in the next brief and credit the implementer — it keeps the
honest-reporting behaviour you want. Related:
[[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_never_conclude_from_truncated_command_output]],
[[feedback_declare_the_seat_when_dispatching]].

### feedback_set_spec_inprogress_at_dispatch

When you dispatch an agent (subagent, brief, or queue-work) to implement a spec, move the spec's status to **In Progress at dispatch time** (approve it first if still Draft). Leaving it Draft/Approved while an agent is actively building it makes the substrate lie about what's happening — the exact opacity AIDA exists to prevent.

**Why:** 2026-09-13 — I delegated a worktree-isolated implementer to fix BUG-1135 but never moved the spec; the operator saw `aida list` still showing it `Draft` and asked "if you're actively working it, why this status?" A fair hit.

**How to apply:** the moment you launch delegated work on a spec: `aida edit <ID> --status approved` (if needed) then `--status in-progress`, plus a one-line `aida comment add` noting who/what is working it. On merge, the `(SPEC-ID)` trailer + `aida pull` auto-completes it. Ties to [[feedback_substrate_first_never_rely_on_agent_awake]] — reliable coordination reads the substrate, so keep the substrate true.

