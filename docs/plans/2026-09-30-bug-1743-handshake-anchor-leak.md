# BUG-1743 — the phase-3 handshake escapes its test's tempdir, 40 times out of 40

Date: 2026-09-30 · spec: BUG-1743 · branch: `bug-1743`

## Summary

`bug_1452_refusal_aftermath_tests::refusal_aftermath_is_parked_held_and_awaiting_visible`
was not flaking because something deleted a file it owned. It was writing a file it
**never owned**, into a directory belonging to somebody else, on **every single run**.
The CI flake was the subset of those runs where "somebody else" was a sibling test's
`tempfile` root that then dropped.

Measured, not inferred: **40 escapes in 40 consecutive runs** before the fix, **0 in 30**
after.

## The mechanism

`handle_review_record_at(project_root, …, pr)` writes two artifacts. The verdict and the
merge-hold go to `project_root`, the parameter. The **phase-3 handshake** does not — it
resolves its own anchor through `review_pr_handshake_path`, whose order is:

1. `AIDA_REVIEW_VERDICT_FILE` (verbatim path)
2. `AIDA_PROJECT_ROOT`, if it `is_dir()`
3. the project enclosing the **current working directory**
4. `AIDA_DRIVE_ROOT`, if it `is_dir()`
5. only now, the `project_root` that was passed in

That order is deliberate and correct in production: it is BUG-912's orchestrator anchor,
and in production those variables name a real repository. Under `cargo test` it is a
hazard, because `std::env` is process-global and step 3 resolves the real checkout.

The failing test set none of those variables, so it never reached step 5. It landed in
one of two places, depending on what a parallel test happened to be doing:

| branch taken | anchor | consequence |
|---|---|---|
| 2 — a sibling held `AIDA_PROJECT_ROOT` at its own tempdir | `/tmp/.tmpXXXXXX/.aida/review-verdicts/PR-1452.json` | sibling's `TempDir` drops → **the CI flake** |
| 3 — no sibling pin, so the cwd's enclosing project | `/home/joe/ai/aida/.aida/review-verdicts/PR-1452.json` | **writes into the real repository** |

The sibling is not hypothetical and not remote: it is
`recording_never_escapes_the_explicit_root_even_when_ambient_env_points_elsewhere`, the
**next test in the same module**, which pins `AIDA_PROJECT_ROOT` and `AIDA_DRIVE_ROOT` to
a tempdir on purpose (and documents that the handshake follows it, which is the correct
behaviour for *that* test). `libtest` orders tests by name — `recording_…` sorts before
`refusal_…` — and runs them concurrently in one process, so one test's deliberate pin is
the other test's ambient environment.

`EnvVarsGuard` holds `ENV_LOCK` for its whole lifetime, so the two *setters* can never
overlap. It cannot help a **reader** that takes no guard at all. `test_env`'s own
doc comment on `apply` already states the rule this test broke:

> A test that merely *reads* env-derived state through the code under test must hold one
> of these guards too, or it races the setters.

### Both observed failure faces, explained

- **run 36285986900** — `could not write …: No such file or directory`. The sibling's
  root passed `is_dir()` at step 2 and was gone by the time `create_dir_all` ran.
- **run 36717478815** — the write succeeded, `write_verdict_object` read the bytes back
  and they matched, then the BUG-1571 `is_file()` check found nothing. The sibling's
  `TempDir` dropped in between.

Same channel, two points in the same few-syscall window. No deletion code anywhere in
the codebase is responsible, which is why the 14-module "resolves a root and removes"
worklist on the spec had no hits worth chasing.

## Reproduction (AC1)

Deterministic, no timing:

```bash
cargo test -p aida-cli-lib --lib --no-run
FOREIGN=$(mktemp -d /tmp/.tmpXXXXXX); mkdir -p "$FOREIGN/.aida"
RUST_MIN_STACK=16777216 AIDA_PROJECT_ROOT="$FOREIGN" \
  target/debug/deps/aida_cli_lib-<hash> --exact \
  bug_1452_refusal_aftermath_tests::refusal_aftermath_is_parked_held_and_awaiting_visible
```

Before the fix this writes the handshake to `$FOREIGN`, not to the test's own tempdir.

Natural rate, no env manipulation at all — run the module's two tests in a loop and
count escapes: **40/40 before, 0/30 after.**

## The fix

Two halves, because one alone would be a fix for one test rather than for the class.

1. **The test pins its anchor.** `refusal_aftermath_…` now holds one `EnvVarsGuard`
   setting `AIDA_PROJECT_ROOT` and `AIDA_DRIVE_ROOT` to its own tempdir and unsetting
   `AIDA_REVIEW_VERDICT_FILE`. That makes the anchor resolve at step 2 to the root it
   owns, and — because the guard holds `ENV_LOCK` — stops any sibling moving it
   mid-test. A new assertion checks the handshake landed *inside* `root`; asserting only
   that the file exists would not have caught this defect, because the file did exist,
   elsewhere, with correct bytes already verified.

2. **The class is netted, in test builds only.** `assert_handshake_anchor_is_pinned`
   fires whenever the resolved handshake falls outside the call's `project_root` *and*
   the thread does not hold `ENV_LOCK`. The discriminator has to be the lock, not the
   path: a foreign tempdir and an owned one are both `/tmp/.tmpXXXXXX`. A test that
   pointed the anchor elsewhere on purpose did so under a guard and is serialised; a
   test that inherited a sibling's pin holds nothing, and is the bug. The panic names
   both paths and the remedy.

`assert_review_write_root_is_isolated` (BUG-1536) could never have caught this. Its
doc claimed to cover "every filesystem write the recording path makes … the phase-3
handshake"; that was wrong, and the comment is corrected. Even had it looked at the
handshake, a root-under-`/tmp` test would have passed — the leak target *was* under
`/tmp`.

## The sweep: the guard found a second offender

Running the whole `aida-cli-lib` lib suite with the net in place (7399 tests) produced
exactly one failure, and it was the net doing its job:
`story_1405_review_marker_tests::recording_the_verdict_clears_the_marker_and_unblocks_merge`
was writing `PR-14051.json` to the real repository by the same route. It is pinned the
same way. This is why the fix is a guard and not one `EnvVarsGuard`: the filed symptom
named one test, and the crate held two.

Both were the only offenders — every other `--pr` caller already pins. The pre-existing
`bug_775_commits_ahead_tests` caller was already correct, and
`recording_never_escapes_the_explicit_root_even_when_ambient_env_points_elsewhere` points
the anchor elsewhere deliberately, under a guard, which is exactly the case the lock
check is designed to permit.

## Collateral finding: fabricated verdicts against a real merged PR

`/home/joe/ai/aida/.aida/review-verdicts/PR-1452.json` exists in the operator's checkout
and carries this test's fixture payload — spec `BUG-14520`, sha `abc123`, branch
`bug-14520-work`, summary "the regression is still open", verdict **`request-changes`** —
with three accumulated `rounds` dated **2026-09-25, 09-27 and 09-30**. Different
worktrees, same destination, because step 3 resolves the *main* clone even when the
tests run from a linked worktree.

**PR 1452 is real and merged** ("refactor(cli): extract goal command handler into
goal_cmd.rs (SPIKE-78)", merged 2026-07-16). This is the BUG-1536 incident class
recurring through a different channel: that incident was a merge-hold written outside
its root against a coincidentally-real PR number, and BUG-1536 fixed the *protection*
root while the *handshake* anchor kept its ambient order.

Blast radius is limited to that one file — no merge-hold marker leaked, because the hold
does use `project_root`. It went unnoticed for weeks because `.aida/` is in
`.git/info/exclude`, so it never appeared in `git status`.

The second offender leaked the same way: `.aida/review-verdicts/PR-14051.json`, an
`approved` verdict for `BUG-14051` against `abc1234`, last written 2026-09-30 10:30.

**Recommended, not done here:** delete
`/home/joe/ai/aida/.aida/review-verdicts/PR-1452.json` and
`/home/joe/ai/aida/.aida/review-verdicts/PR-14051.json`. They are junk in the operator's
live checkout and are left for the operator to remove deliberately.

## Relationship to BUG-1695 (AC3)

**Different channel, same family — and the evidence points away from a shared cause.**

BUG-1695's `sandbox_create_and_seed_round_trip` (`aida-cli-lib/src/sandbox_cmd.rs`)
never publishes its `TempDir` to `AIDA_PROJECT_ROOT`, `AIDA_DRIVE_ROOT` or
`AIDA_REVIEW_VERDICT_FILE` — it sets no environment variable at all. Every consumer of
the BUG-1743 channel finds its target *by reading one of those three variables*, so no
such consumer can address BUG-1695's temp root. The mechanism proven here therefore
cannot be the writer BUG-1695's AC2 was unable to name.

What they share is the family: one test's temp tree being written into by another test
in the same process. BUG-1743 confirms that family is real and gives it one fully
characterised channel. BUG-1695's channel, if it has one, must reach its root some other
way — inherited cwd, or a `std::env::temp_dir()`-rooted sweep — and remains unnamed.
This is a claim with evidence, per the criterion, not a default "unrelated".

## Mutation proof (AC4)

The fix is the pin; reverting it is deleting the `EnvVarsGuard`. With the guard removed
the module's own loop returns to escaping on every run (40/40 measured), the new
`handshake.starts_with(root.path())` assertion fails outright, and
`assert_handshake_anchor_is_pinned` panics naming both paths. The test cannot pass
either way.

## Files changed

| file | what |
|---|---|
| `aida-cli-lib/src/test_env.rs` | `holds_env_lock()` — the deliberate-vs-inherited discriminator |
| `aida-cli-lib/src/lib.rs` | `assert_handshake_anchor_is_pinned` + its call site; the pin, the location assertion and corrected comments on `refusal_aftermath_…`; the pin on `recording_the_verdict_clears_the_marker_and_unblocks_merge`; corrected BUG-1536 doc claim |
| `docs/plans/2026-09-30-bug-1743-handshake-anchor-leak.md` | this report |
