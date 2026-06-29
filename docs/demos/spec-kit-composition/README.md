# Composition-seam demo: scaffold with Spec Kit, keep the graph in AIDA

*TASK-875 · parent EPIC-50 · 2026-06-25*

This is the **shown, not described** version of the composability claim in
[`docs/positioning/vs-spec-kit.md`](../../positioning/vs-spec-kit.md): you can
scaffold a feature with [GitHub Spec Kit](https://github.com/github/spec-kit)
and let AIDA hold the **cross-feature graph + code-to-spec traces + lifecycle**
that Spec Kit structurally drops once a feature is `/implement`ed.

It answers *"why not just Spec Kit?"* with an artifact: the **same** feature,
**before** (a bare Spec Kit `specs/` dir) and **after** (filed into AIDA's
graph, queryable / traced / lifecycle-tracked six features later).

The framing is **composable, not competing** and **factual, not advocacy**.
The demo states plainly where Spec Kit alone is enough and a substrate is not.

---

## What's here

| Path | What it is |
|---|---|
| `speckit-feature/` | A faithful, hand-authored representative of `/speckit.specify` + `/plan` + `/tasks` output — a 3-feature auth service. (`specify` CLI not assumed installed; structure mirrors the documented format.) |
| `demo-spec-kit-composition.sh` | The runnable end-to-end demo. Creates a throwaway project, shows BEFORE, files the features into AIDA, traces code, then runs the AFTER queries. No network / GitHub / Spec Kit CLI needed. |
| `README.md` | This artifact: the before/after, the exact commands, the honest takeaway, and a real gap the demo surfaced. |

### The representative Spec Kit feature

Three features under `speckit-feature/specs/`, deliberately cross-cutting so
the *"what's blocked across this epic?"* question is real:

- `001-user-accounts` — implemented. Exposes `find_account_by_email`.
- `002-session-tokens` — login; **depends on 001** (calls that lookup).
- `003-password-reset` — **blocked by BOTH 001 and 002**; not yet started.

In Spec Kit those dependencies live only in **prose** inside each `spec.md`'s
`## Dependencies` section, and each feature has its **own** `FR-###` namespace
(`FR-001` means three different things across the three dirs). That is by
design — Spec Kit is a per-feature convention, not a project-wide graph.

---

## Run it

```bash
# aida on PATH (run `aida dev activate` for the dev build); git user.name/email set.
bash docs/demos/spec-kit-composition/demo-spec-kit-composition.sh
bash docs/demos/spec-kit-composition/demo-spec-kit-composition.sh --no-pause   # CI / non-interactive
```

Everything runs in a throwaway temp dir that is removed on exit — your real
store is untouched.

---

## BEFORE — the bare Spec Kit dir

The question that matters six features in:
**"What is `003-password-reset` blocked by, and is that blocker done?"**

From inside the Spec Kit dir, the only answer is grep over markdown:

```text
$ grep -A2 '## Dependencies' specs/003-password-reset/spec.md
## Dependencies
> Blocked by BOTH `001-user-accounts` and `002-session-tokens`. Implementation
> has NOT started pending those. Again: prose only — no graph to walk.
```

You get **text, not a queryable relationship.** There is no
`aida graph --blocked-by` equivalent — the dependency is not a record. Rename a
feature, and every cross-reference is a string match you have to chase by hand.

---

## THE SEAM — file the same features into AIDA

```bash
aida init --no-skills --no-hooks --no-agent-config

EPIC=$(aida add --type epic  --title 'Auth service' --status approved | ...)
S1=$(aida add   --type story --title 'User accounts (speckit 001)' --parent $EPIC ...)   # -> completed
S2=$(aida add   --type story --title 'Session tokens (speckit 002)' --parent $EPIC ...)
aida edit $S2 --blocked-by $S1                       # typed BlockedBy + inverse Blocks
S3=$(aida add   --type story --title 'Password reset (speckit 003)' --parent $EPIC ...)
aida edit $S3 --blocked-by $S1 --blocked-by $S2      # blocked by BOTH
```

The cross-feature dependencies that were prose in Spec Kit become **typed,
queryable edges**. Then link the implemented code back to its spec:

```rust
// trace:STORY-2 | ai:human
pub fn find_account_by_email(_email: &str) -> Option<()> { None }
```

```bash
aida trace scan src/        # discovers the inline // trace: comment
git commit -m "implement accounts (STORY-2)"   # (SPEC-ID) trailer = lifecycle linkage
```

> **Honest note on filing order.** The demo files `--parent` first, then adds
> `--blocked-by` via `aida edit`, because of a real `aida add` bug the demo
> surfaced — see [Honest findings](#honest-findings) below. The one-shot
> `aida add --parent X --blocked-by Y` form silently drops the blocked-by edge.

---

## AFTER — the queries AIDA can now answer (Spec Kit can't)

**Q1 — "What is `003-password-reset` blocked by?"** *(unanswerable from the Spec Kit dir)*

```text
$ aida graph STORY-4 --blocked-by
Graph (blocked-by) STORY-4 — Password reset (speckit 003)
  STORY-2  User accounts (speckit 001)
  STORY-3  Session tokens (speckit 002)
```

**Q2 — "What is at risk across the epic if 001 slips?"** *(reverse impact)*

```text
$ aida graph STORY-2 --impact
Graph (impact) STORY-2 — User accounts (speckit 001)
  STORY-3  Session tokens (speckit 002)
  STORY-4  Password reset (speckit 003)
```

**Q3 — "What's the status of every feature in this epic?"** *(lifecycle rollup)*

```text
$ aida graph EPIC-1 --tree
Graph (tree) EPIC-1 — Auth service
STORY-2  User accounts (speckit 001)
STORY-3  Session tokens (speckit 002)
STORY-4  Password reset (speckit 003)

Rollup: 3 total · 1 completed · 1 in progress · 1 remaining
```

**Q4 — "Is the code still traced to its spec?"**

```text
$ aida show STORY-2
Git linkage:
  Commits (1)     4a6ab62 implement accounts (STORY-2)
  Files traced (1) src/accounts/mod.rs — find_account_by_email
```

**The other half of the seam — `aida plan scan` grounds the *next* feature.**
Before you hand the next feature to Spec Kit's `/specify`, run a read-only pass
that summarizes the current API surface (from the trace graph) and flags any
code path the spec names that no longer exists. You feed *that* summary to Spec
Kit as grounding context, then `--attach` the provenance to the spec:

```text
$ aida plan scan STORY-4
## Pre-plan scan
### Current API surface (trace-graph derived)
- `src/accounts/mod.rs` — `find_account_by_email`
### Architectural constraints
- Parent EPIC-1 — Auth service (Approved)
- blocked-by: STORY-2 — User accounts (speckit 001)
- blocked-by: STORY-3 — Session tokens (speckit 002)
```

That is the literal composition command from `vs-spec-kit.md`: **scan first,
hand the summary to the external generator, then attach the provenance so the
imported spec records what the tree actually looked like at plan time.**

---

## Honest takeaway

- **Spec Kit produced the three feature scaffolds, and that work is real and
  good.** If you only ever ship one feature at a time, loosely cross-referenced,
  **Spec Kit alone is enough** and AIDA's machinery (orphan branch, cache, MCP)
  would not earn its keep. This demo does not pretend otherwise.
- The moment *"what's blocked across this epic?"* / *"what breaks if 001
  slips?"* / *"is this code still traced?"* became **live** questions, the
  per-feature dirs could not answer them. AIDA's typed graph + traces + lifecycle
  could. That is the seam — and the cost (the substrate) only pays for itself
  once the project is big and cross-linked enough that the relationships between
  specs matter more than producing any single one.
- **They compose.** Spec Kit standardizes how an agent *produces* a feature's
  specs; AIDA is the graph *underneath* that keeps every feature stable,
  related, traced, and queryable for the life of the project.

There is no "import the Spec Kit tree wholesale" command, and the positioning
doc never claimed one — filing the features into AIDA is a deliberate, explicit
step (here, a handful of `aida add`/`aida edit` calls). `aida plan scan` is the
*grounding* half of the seam, not a bulk importer.

---

## Honest findings

Surfaced while building this demo (the task explicitly asked to report real
gaps rather than fake around them):

### BUG-615 — `aida add --parent X --blocked-by Y` silently drops the blocked-by edge

When `aida add` is given **both** `--parent` and `--blocked-by` in one
invocation, the command prints both `Blocked by: …` and `Linked: … → parent
of …`, but only the **parent** edge survives — the BlockedBy edge is lost.

Reproduce:

```bash
aida add --type story --title D --parent EPIC-1 --blocked-by STORY-2 --status approved
aida show D   # Relations: shows ONLY "is child of EPIC-1" — no "is blocked by"
```

Root cause (`aida-cli/src/main.rs`): the `--blocked-by` loop runs first and
writes the BlockedBy edge via `add_blocked_by_edge` (which re-fetches fresh).
The `--parent` block then runs and does `let mut child = last.clone()` on the
**pre-blocked-by** snapshot captured at `add_requirement` time, pushes the
Child edge onto that stale copy, and calls `backend.update_requirement(&child)`
— **clobbering** the just-written BlockedBy edge. `--blocked-by` alone (any
count) works; `--parent` alone works; only the combination loses data.

Fix shape: re-fetch the requirement fresh before adding the parent edge (mirror
what `add_blocked_by_edge` already does), or merge both edge-adds into one
read-modify-write. Filed as **BUG-615**.

Workaround (used by this demo): `aida add --parent …` then a separate
`aida edit <id> --blocked-by …`.
