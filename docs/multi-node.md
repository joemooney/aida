# Multi-node AIDA: keeping the queue and store consistent across machines

*Last updated: 2026-05-22 — trace:BUG-220*

You have AIDA on more than one machine — typically a home box and a work box
sharing the same git remote. `aida queue list` on machine B comes back empty
even though you queued work on machine A. This doc names the two ways that
happens and the durable workflow that prevents it.

For the single-node identity rule (queue keyed off shell `$USER`, not node
identity), see `CLAUDE.md` § *Queue identity (BUG-89)*. For the storage model
(orphan branch + cache) see `OVERVIEW.md`.

## TL;DR

```bash
# On machine A — before you walk away from the keyboard:
aida db sync --push       # push any orphan-store commits

# On machine B — at the start of a session:
aida pull                 # fetches code + orphan store + runs merge-gate
```

That is the whole pattern. Everything below explains *why* it works and how to
diagnose when it doesn't.

## The two failure modes

The queue is one YAML file per shell user, stored in the orphan `aida-store`
branch at `registry/queues/<user_id>.yaml`. `<user_id>` resolves in this order:
`--user <id>` flag → `AIDA_USER` env → `USER` env → `USERNAME` env → `"default"`
(see `current_user_id` in `aida-cli/src/main.rs`).<!-- trace:TASK-476 | ai:claude --> Queue reads and writes
go through the git backend directly — **not** through the SQLite cache (which
holds `aida list` / `aida search` projections only). So a stale cache cannot
make the queue look empty; only two things can:

### A · Shell `$USER` differs across machines

If machine A's shell sees `USER=joe` and machine B's shell sees `USER=joseph`
(or `jmooney`, or anything else), the two are writing and reading **different**
YAML files in the same orphan store. Queue items are there — just under the
other name.

Verify:

```bash
echo "USER=$USER  AIDA_USER=$AIDA_USER"
ls .aida-store/registry/queues/
```

Fix: pin `AIDA_USER` per-shell so both nodes resolve to the same id, regardless
of the shell's `$USER`:

```bash
# Add to ~/.bashrc / ~/.zshrc on every machine you work from:
export AIDA_USER=joe
```

The `--user <id>` flag works for one-off invocations.

### B · Orphan store on machine A is unpushed (the usual culprit)

This is what catches everyone. The orphan `aida-store` branch is committed
locally on every `aida add` / `aida edit` / `aida queue add`, but push to
`origin/aida-store` is **manual by default**. Machine A's local branch can sit
arbitrarily far ahead of origin; machine B's `aida pull` faithfully pulls the
stale origin and sees nothing new.

The 2026-05-18 incident this doc was filed against: machine A was nine commits
ahead of `origin/aida-store`. Machine B saw an empty queue not because the
queue was empty — because the queued items had never left machine A.

Verify on the machine you suspect is the writer:

```bash
git -C .aida-store fetch origin aida-store --quiet
git -C .aida-store log --oneline origin/aida-store..aida-store
```

Non-empty output → the writer hasn't pushed. Fix:

```bash
aida db sync --push
```

On the reader machine after the push:

```bash
aida pull        # code + orphan store
# or
aida db sync --pull
```

## Auto-push modes (so you don't have to remember)

`.aida/config.toml`:

```toml
[store.sync]
auto_push = "session-end"   # push when `aida session end` runs
# auto_push = "per-write"   # push after every queue/spec mutation
# auto_push = "manual"      # default — explicit `aida db sync --push` required
# auto_push = "periodic"    # reserved for aida-worker (EPIC-30) — falls back to manual today
```

`session-end` is the recommended setting on a multi-node workstation: every
time you wrap up an AIDA session, the store gets pushed automatically. The
cost is one network call per session-end; the value is never being the writer
who left commits stranded.

`per-write` is the strict mode — every mutation pushes. Useful if you actively
move between machines mid-session.

## Diagnostic checklist

Run on the node that sees an empty / stale queue:

```bash
echo "USER=$USER  AIDA_USER=$AIDA_USER"     # (A) — does the user id match?
aida cache status                            # is the cache fresh? (auto-rebuilt anyway)
ls .aida-store/registry/queues/              # whose queue files actually exist?
aida pull                                    # fetch+merge code + orphan store
git -C .aida-store log --oneline -5          # what was the last orphan-store commit?
aida queue list --sync                       # one-shot pull-then-list
```

Then run on the *other* node (the suspected writer):

```bash
git -C .aida-store fetch origin aida-store --quiet
git -C .aida-store log --oneline origin/aida-store..aida-store
# non-empty → you're the unpushed writer; run `aida db sync --push`
```

## Reproducing the two-node setup on one machine

You can simulate two nodes against the same origin with two clones in
different directories — useful for testing changes to the sync surface:

```bash
# Clone A (the writer)
git clone git@github.com:joemooney/aida.git /tmp/aida-node-a
cd /tmp/aida-node-a
git worktree add -B aida-store .aida-store origin/aida-store
# … queue some work …
aida queue add SPEC-123 --for implementer
# (Without `aida db sync --push`, node B will not see this.)

# Clone B (the reader)
git clone git@github.com:joemooney/aida.git /tmp/aida-node-b
cd /tmp/aida-node-b
git worktree add -B aida-store .aida-store origin/aida-store
aida queue list                # empty — node A hasn't pushed yet

# Now push from A:
cd /tmp/aida-node-a
aida db sync --push

# And pull from B:
cd /tmp/aida-node-b
aida pull
aida queue list                # populated
```

The same recipe applies across actual machines — `/tmp/aida-node-a` is just
another path that pretends to be a different node.

## What doesn't matter (so you don't chase it)

- **`~/.aida/node.toml`** — the per-machine node identity. Used for short-ID
  assignment in the merge gate and to say WHICH machine measured a published
  timing span; **not** used in queue routing. Different nodes can write queue
  items routed to the same shell user without any node-id conflict. (It is not
  used for hybrid-logical-clock timestamps: there is no HLC in the codebase —
  the module was removed rather than wired in, BUG-578.)
- **`.aida/cache.db`** — silently auto-rebuilt by `ensure_cache_fresh()` on
  every list/search/load (`aida-core/src/db/cached_git_backend.rs:85`). Stale
  cache cannot cause an empty queue because queue reads bypass it entirely.
- **Local `aida-store` worktree state** — a fresh `git worktree add` against
  `origin/aida-store` reconstructs everything. The orphan branch on origin is
  the source of truth; the local worktree is convenience.

## Cycle-time evidence across machines

A spec's phase events live in `.aida/events.jsonl`, which is **per-clone and
local-only**. Work done on one machine is not in another machine's stream, and
never will be — the live log is not synced (ADR-60 rejected making it a store
branch: that would put a git commit on the drain's hot path).

What crosses machines is the **per-spec timing record**. When a spec reaches a
terminal state, the machine that completed it writes
`.aida-store/timings/<SPEC-ID>.yaml` — the finished span list, lifecycle
activities only, no mail/cron/shift noise — and commits it **locally**. The next
`aida push` / `aida db sync` carries it, and any other clone reads it after
`aida pull`:

```bash
aida history STORY-1234 --timing          # the published span list
aida history STORY-1234 --timing --format json
```

### What the two clocks mean

The store has **no causal clock**. Two things follow, and the record is built
around them rather than hiding them:

- Each span records the node that measured it. A span's `duration_s` is that ONE
  host's start-to-end measurement, so it survives skew.
- The interval **between** two different nodes' spans is not a measurement — it
  mixes two clocks and can even be negative. It is reported as unknown time with
  both nodes named, never clamped to a plausible-looking wait.

Two machines that both publish for the same spec keep BOTH span sets: the
record merges by **union on each span's content-derived id**, not
last-writer-wins, so a machine whose clock is behind cannot delete another
machine's measurements. (Compare a spec's scalar fields, which ARE wall-clock
last-write-wins with a content-hash tie-break — under skew the wall-clock-later
write wins there even when it is causally earlier.)

### What is still not measured

- **In-flight work on another machine.** Only terminal specs publish. A spec
  being worked on host X is invisible on host Y until it completes.
- **A spec completed before this instrumentation existed**, or one whose local
  event log was rotated away first, publishes nothing. An absent record means
  "not measured", never "took no time".
- **Compile time inside an agent's own session.** AIDA measures the preflight
  build/guard step it runs itself; what the agent compiles inside its session is
  not observable from outside it.

## See also

- `CLAUDE.md` § *Queue identity (BUG-89)* — the single-node identity rule
- `CLAUDE.md` § *Storage model (EPIC-1-001)* — orphan branch + cache layout
- `CLAUDE.md` § *Divergent-branch recovery* — when `aida pull` refuses
- `docs/session-lifecycle.md` — where `auto_push = "session-end"` fits in
- BUG-89, STORY-284, BUG-220
