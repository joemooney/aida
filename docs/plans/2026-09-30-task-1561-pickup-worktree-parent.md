# TASK-1561 — the warm-pool fallback ignored `worktree_parent`

*2026-09-30 · advisor relay session #19*

## The defect

`[worktree_pool] worktree_parent` (BUG-1700) exists so an operator running agents
*without* `--dangerously-skip-permissions` can accept the Claude Code folder-trust
modal **once**, for one directory, and have every later worktree inherit that trust.

Every worktree creator honoured it — `acquire_session_pool_worktree`,
`integrate_checkout`, and the orchestrator path all pass
`worktree_pool_config_worktree_parent(project_root)` as `AcquireOptions.parent_dir`.
One did not: `pickup_worktree_path` hardcoded `project_root.parent().join(...)` and
never read the key.

That function is the **warm-pool fallback** — the path taken whenever the pool cannot
serve a tree. Reproduced live while claiming BUG-1743:

```
Warning: warm-pool acquire failed (reset pooled worktree /home/joe/ai/aida-pool-aida-1);
         falling back to a fresh worktree
worktree: /home/joe/ai/aida-bug-1743
```

So a correctly configured project still scattered worktrees into the project root's
parent the moment the pool missed — silently, with a warning about the pool and nothing
about the layout. The guarantee failed exactly when it mattered: the operator got a fresh
untrusted path with its own modal, and the only alternative was to trust the shared
parent `/home/joe/ai` and every unrelated project under it.

## The fix

**One placement rule, not two.** `pool_path_for` became
`aida_core::worktree_pool::worktree_placement_path` — public, documented as the single
placement rule for *any* AIDA-minted worktree. `pickup_worktree_path` now reads the key
and calls it. The name stays local to pickup; only the placement is shared.

All three pickup callers inherit the fix without signature churn: `queue_cmd.rs:11215`
(the `--dry-run` preview), `lib.rs:36873` (`resolve_pickup_workspace`, the auto-complete
phase-1 fallback) and `lib.rs:37369` (`session_start`).

### The second half, which the first test run forced

With placement fixed, the e2e test still failed — and on the right thing:

```
`--dry-run` previewed /tmp/.tmpAZM2ht/repo/../aida-worktrees/repo-task-1
but the real run created /tmp/.tmpAZM2ht/aida-worktrees/repo-task-1
```

The shipped example value is **relative** (`"../aida-worktrees"`), so the raw join spells
the parent `<root>/../aida-worktrees` while `git worktree add` registers
`<root-parent>/aida-worktrees`. Same directory, different string — preview and reality
disagreeing, which is precisely the divergence BUG-1628 consolidated this resolver to
prevent. BUG-1700 had already needed its own normaliser in `doctor_cmd` for the same
reason: its folder-trust check compares with a **lexical** `starts_with`.

So `normalize_path_lexically` moved into `aida-core` beside the placement rule, and
`worktree_placement_path` normalises the **configured** branch. `doctor_cmd`'s copy now
delegates — three places needing one path rule was how the key came to be honoured by the
pool and by nothing else.

Deliberate scope notes:

- The **unset** branch is **not** normalised. AC2 requires byte-identical output, and a
  project root containing `..` would otherwise come back respelled. Nothing moves under a
  live fleet.
- The pool's own relative-parent spelling changes (`/work/myrepo/../wt/...` →
  `/work/wt/...`); its test expectation was updated deliberately. git normalises anyway,
  and a `..` surviving into a lease file is a latent instance of the same family as
  BUG-1700's trust-comparison bug.
- **Symlink caveat, stated rather than hidden:** a `..` crossing a *symlinked* project
  root resolves differently lexically than the kernel would. The lexical reading is chosen
  because it is what the operator wrote and what the folder-trust check already compares,
  so the single-grant guarantee holds. This is the same tradeoff `doctor_cmd`'s normaliser
  documented when it was written.

## AC5 — is `aida doctor` silent here, and why?

**Yes, and it is correct. The previous session's framing ("the doctor finding is not
surfacing… that is part of the defect") is wrong — do not chase it.**

`worktree_trust_findings` returns early when the resolved parent, or an ancestor, carries
a trust record. This repository sets no `worktree_parent`, so the resolved parent is
`/home/joe/ai` — and `~/.claude.json` records:

```
/home/joe/ai   hasTrustDialogAccepted: true     (27 of 150 projects are trusted)
```

The check is therefore satisfied: no modal will appear, so there is nothing to warn about.
The doctor is behaving as designed.

**But that is exactly the operator's complaint, and the check cannot see it.** Trust was
satisfied by the *broad* grant on the shared parent — covering every unrelated project on
the machine — which is the outcome `worktree_parent` exists to let the operator avoid.
`worktree_trust_findings` treats "trusted narrowly" and "trusted because a 150-project
shared parent was granted" as the same clean result. A finding that observed *breadth*
(the resolved parent is trusted, but it is also the parent of N unrelated repos — consider
`worktree_parent` to narrow it) would have surfaced this. That is a **separate** spec, not
this one; it is recorded as a comment on TASK-1561.

## Verification

| check | result |
|---|---|
| `aida-cli-lib --lib task_1561 bug_1700` | **28 passed, 0 failed** (includes every BUG-1700 doctor-trust test through the delegated normaliser) |
| `aida-cli --test task_1561_worktree_parent_placement` | **2 passed, 0 failed** |
| `aida-core --lib worktree_pool` | **53 passed, 0 failed** |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy` (3 changed crates, `--all-targets`) | no errors; no warning cites changed code |

**Mutation proof (AC6).** Restoring the pre-fix body of `pickup_worktree_path`:

- unit: **4 of 6 fail**. The 2 that pass are exactly the two pinning the *no-change*
  invariant (unset key byte-identical; parentless-root error preserved).
- e2e: `fallback_worktree_lands_under_the_configured_parent_and_the_preview_agrees` fails
  with `the worktree escaped the configured parent /tmp/.tmpO5AU1t/aida-worktrees:
  /tmp/.tmpO5AU1t/repo-task-1`, and `unset_key_still_lands_at_the_historical_sibling_path`
  still passes.

### A fixture trap worth remembering

The first e2e run failed with `aida add failed:` and **empty stderr**. Cause: the fixture
appended a `[worktree_pool]` table, but `aida init` already scaffolds one — a duplicate
TOML table made the whole config unparseable, and the config-read failure surfaced with no
message. The fixture now *replaces* the scaffolded table body, asserts the substitution
happened (so a template rename breaks the test loudly instead of silently testing a config
that was never applied), and parses the result.
