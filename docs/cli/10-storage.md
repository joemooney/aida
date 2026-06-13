# Chapter 10 — Storage & data

This is the chapter you can mostly *not read* and still use AIDA every day — which is the point. The store and the cache that hold every spec are designed to manage themselves: `init` creates them, ordinary `add`/`edit`/`list` keep them in sync, and `pull`/`push` move them with your code. The commands here are the **plumbing and the recovery kit** — the things you reach for when you want to *look at* the storage layer, or when something needs un-sticking. A healthy project rarely touches them by hand.

> Manual contract reminder: rationale, not flag tables. `aida <cmd> --help` is the source of truth for exact flags.

---

## The storage model: where your specs actually live

AIDA is **git-canonical by default**. The writer of record is an orphan branch called **`aida-store`** — a branch with no shared history with your code, holding **one YAML file per spec** under `objects/TYPE/000/SPEC-ID.yaml`. That file is the truth: every field, every relationship edge, and the spec's full transition `history:` array live inside it. The store lives on a branch (versioned, pushable, mergeable across machines) but checks out into a gitignored worktree at `.aida-store/`, so it's *alongside* your code without cluttering your working tree. When you `aida add` or `aida edit`, AIDA writes that YAML and commits it to the `aida-store` branch — git is the database.

Reading hundreds of YAML files for every `aida list` would be slow, so AIDA keeps a **rebuildable SQLite cache** at `.aida/cache.db` (gitignored). The cache is a pure **read projection** — a fast index over the store, never a second source of truth. Writes are **write-through**: a write goes to git *first*, then updates the cache, so the two can't disagree about a change you just made. `list`, `search`, `findings`, and the statusline all read the cache, which is why they're sub-millisecond even on a large store. Because it's derived, the cache is disposable: delete it and it rebuilds on the next read.

The glue is **stale-detection by HEAD SHA**. The cache records the `aida-store` HEAD it was built from; on each read AIDA compares that recorded SHA against the branch's *actual* HEAD. A mismatch — because a `pull` brought new specs down, or a sibling worktree wrote — triggers a rebuild before the read returns, so you never see a stale list. (One thing the cache does *not* project today: the per-spec `history:` arrays. For substrate-grounded time series — burn-down charts, status-flow analysis — read the YAML or the orphan-branch git log directly, not the cache.) On a **fresh clone** the first store-reading command auto-attaches the `.aida-store/` worktree and rebuilds the cache, so `list`/`queue`/`findings` work with no manual setup step.

What this means in practice: **a normal user needs almost none of the commands below.** The two you might genuinely reach for are `aida cache rebuild` (when the cache somehow drifts — rare) and `aida db reconcile-status` (when a merge's Done→Completed auto-bump missed). Everything else is for multi-repo workspaces, distributed-ID housekeeping at merge-to-trunk, or auditing the store's integrity — operations the lifecycle normally performs for you.

> `aida fetch`/`pull`/`push`/`rebase` *also* move the store (it's the second of AIDA's two git legs), but they're the everyday sync verbs and live in **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**. This chapter is the store's *direct* management surface; reach for Chapter 4 for routine syncing.

---

### `aida db`

**One line** — direct management of the git-backed store: inspect it, sync it, and run the ID/integrity housekeeping the lifecycle usually automates.

**Mental model.** `aida db` is a *family* of subcommands operating on the `aida-store` branch beneath the spec-level commands. Picture two tiers: **inspection** (`path`, `info`, `status` — pure sight) and **maintenance** (`sync`, `merge-gate`, `reconcile-status`, `check`, `block`, plus the rare `workspace-init` / `retire-legacy-ids`). Most of the maintenance tier runs *for* you — `aida pull`/`push` call `sync`, the merge flow calls `merge-gate` — so reaching for them by hand means either you're scripting, or you're recovering from a miss.

**Reach for it when** — you want to *see* the store's state (`db status`, `db info`, `db path`), recover a stranded spec (`db reconcile-status`), or set up a multi-repo workspace (`db workspace-init`). The recovery and audit subcommands are the ones a regular user touches; the rest are plumbing.

**Don't reach for it when** — you just want to sync with origin in normal work: that's `aida pull` / `aida push` (Chapter 4), which wrap `db sync` *and* move the code leg too. Using `db sync` directly moves only the store leg — correct when you mean exactly that, a footgun when you forgot the code.

**Key subcommands (rationale only).**
- `path` / `info` / `status` — the **read** trio. `path` prints where the store worktree lives (useful for scripts and "where is this actually?"); `info` is store statistics; `status` shows pending changes, sync state, and conflicts — the "is my store clean?" check before a push.
- `sync` — the store-leg sync primitive (`--pull` before, `--push` after, `-m` to commit pending changes). You rarely call it directly because `aida pull`/`push` call it *and* handle the code leg; reach for it only when you deliberately want store-only.
- `merge-gate` — collapses a distributed node-aware id (`<spec-id>-001`) down to its agreed short form (`<spec-id>`) at merge-to-trunk. It runs automatically inside `aida pull` (the merge-gate step); call it by hand only when you've disabled the auto-gate or are reconciling IDs manually.
- `reconcile-status` — **the recovery tool you'll actually use.** Replays the Done→Completed auto-bump over a wider commit window than the `pull` that missed it. See its own entry below.
- `check` — store integrity audit (currently `--collisions`: two specs claiming the same short id). The recovery-side counterpart to the gate-time prevention.
- `block` — pre-allocates blocks of agreed IDs to a node so offline `trace:` comments can cite a final id. Distributed-multi-node housekeeping; single-machine projects never need it.
- `workspace-init` / `retire-legacy-ids` — the rare ones. `workspace-init` sets up multiple code repos sharing one store; `retire-legacy-ids` is a one-time migration collapsing old zero-padded ids onto their short agreed ids. Both are setup/migration events, not daily verbs.

**Gotchas.** `db sync` moves *only the store branch* — it is **not** a substitute for `aida push`/`pull`, which move both legs. Pushing your store but forgetting your code (or vice versa) is exactly the split that two-leg `pull`/`push` exist to prevent; don't reintroduce it by reaching for `db sync` out of habit. And the destructive-shaped maintenance verbs (`retire-legacy-ids`, `check --repair`) take `--dry-run` for a reason — preview first.

**Chains with** — `db status` before a push; `db reconcile-status` after a merge whose auto-bump missed; `db merge-gate` is invoked by `aida pull`. For routine two-leg syncing, the wrappers in Chapter 4.

---

### `aida db reconcile-status`

**One line** — the recovery verb for specs stranded at **Done** when a merge's auto-bump should have moved them to **Completed**.

**Mental model.** When a PR merges, `aida pull` scans the commits *it just brought in* for `(SPEC-ID)` references and bumps each referenced Done spec to Completed. That scan is narrow by design — only the new commits. So the bump **silently misses** in two cases: the spec's YAML was unreadable at pull time, or the spec flipped to Done *after* its referencing commit had already landed on main (so no future pull will ever re-see that commit). `reconcile-status` is the **manual replay** of the same scan over a *wider* window, sweeping up whatever the pull-time scan couldn't see. It's idempotent — only Done specs flip — so an over-broad window is safe.

**Reach for it when** — a spec you know merged is still showing **Done**, not Completed. This is the canonical "the auto-bump missed" recovery. (Don't hand-set `--status completed`; re-run the bump so it's earned by the same logic, not asserted.)

**Don't reach for it when** — the spec legitimately *isn't* merged yet (it's still on a branch — that's Done-and-correct, not a miss); or the spec is already Completed (the replay is a no-op, but you're solving a non-problem).

**Key options (rationale only).**
- `--spec <SPEC-ID>` — limit the replay to the one spec you know is stuck. Much faster than the full scan when you've already identified the culprit.
- `--since <REF>` — bound the scan range to a SHA/tag/ref. Without it, the default window is the recent default-branch history — wide enough to catch most strands, safe because the bump is idempotent. Narrow it when you know roughly when the merge happened.
- `--dry-run` — preview which specs *would* flip without writing. The right first move; pairs with `--spec`/`--since` to confirm before applying.

**Gotchas.** It only flips specs that are currently **Done** and whose referencing commit is on the **default branch** — those guards are what make it safe to run broadly. If a spec stays Done after a reconcile, the cause is usually that no commit on the default branch actually carries its `(SPEC-ID)` trailer (check the merge's commit message), not that reconcile failed.

**Chains with** — the natural follow-up to a merge where `aida pull`'s auto-bump didn't promote the spec. Verify with `aida show <ID>` afterward.

---

### `aida cache`

**One line** — view and rebuild the SQLite read-cache that makes `list`/`search` instant.

**Mental model.** The cache (`.aida/cache.db`) is a **derived, disposable** index over the git store — never a source of truth. `aida cache` has exactly two subcommands because there are only two honest operations on a derived projection: **look at its state** (`status`) and **throw it away and recompute** (`rebuild`). You never *edit* the cache; writes flow through the spec commands and the cache updates itself write-through.

**Reach for it when** — you suspect the cache drifted from the store (a `list` that disagrees with `show`, or after a manual poke at the `.aida-store/` worktree): `cache status` to confirm, `cache rebuild` to fix. In normal operation the HEAD-SHA stale-check rebuilds automatically, so reaching for `rebuild` by hand means the auto-detect somehow didn't fire — rare, and worth a mental note if it recurs.

**Don't reach for it when** — `list`/`search` are simply *empty* and you assume the cache is broken. Empty results are far more often a *filter* (a role scope, an archive flag, the wrong `$USER` for the queue) than a stale cache. Diagnose the query before rebuilding.

**Key subcommands (rationale only).**
- `status` — shows the cache's recorded HEAD vs the store's actual HEAD, the requirement count, and last build time. The diagnostic: a HEAD mismatch that *isn't* auto-clearing is the signal to rebuild.
- `rebuild` — drops and recomputes the whole cache from the git store. The repair. It's cheap and side-effect-free (the store is untouched), so it's the safe thing to try when the cache is the suspect.

**Gotchas.** Rebuilding the cache **cannot lose data** — the store is canonical and the cache is regenerated from it — so it's a no-risk operation, unlike most "rebuild the database" commands. But it also *fixes nothing in the store itself*: if a spec is wrong in the YAML, rebuilding the cache faithfully re-projects the wrong value. The cache is only ever as right as the store it mirrors. (Also: `cache` is git-canonical-mode only; legacy `--centralized` projects have no cache to rebuild.)

**Chains with** — `cache status` to diagnose, `cache rebuild` to fix; both sit *beneath* `list`/`search`/`findings`, which read the cache without you thinking about it.

---

### `aida export`

**One line** — dump requirements out to a file (a tree, a flat JSON, or an ID mapping).

**Mental model.** `export` is the **read-only egress** from the store — it serializes specs into a shareable artifact without changing anything. Its headline use is the **tree export**: pick a root spec with `--id` and it emits that spec *and all its descendants* as a JSON tree you can carry to another project and `import`. The other formats (`mapping`, `json`) are flatter dumps for tooling and ID cross-reference.

**Reach for it when** — you want to share a requirement hierarchy between projects (export a `FOLDER`/`EPIC` subtree here, import it there), feed specs to an external tool, or snapshot the graph in a portable form.

**Don't reach for it when** — you want the *canonical backup* of your specs: that's already the `aida-store` branch (push it). `export` is for *moving* or *reshaping* data, not for safekeeping — the git store is the durable copy.

**Key options (rationale only).**
- `--id <ID>` — the pivot for **tree** export: exports this spec and everything beneath it. Without it, a tree export has no root to walk from; this is what makes export → import a subtree-transplant tool.
- `--format <FORMAT>` — `tree` for the export/import round-trip, `json`/`mapping` for flatter tooling/ID-reference consumption. Pick `tree` whenever the destination is another AIDA project.
- `--output <OUTPUT>` — write to a file instead of stdout. Pipe-friendly either way; use a file when the consumer is `aida import`.

**Chains with** — the egress half of the cross-project transplant: `aida export --format tree --id <ID> -o file.json` here, then `aida import file.json` there.

---

### `aida import`

**One line** — pull a previously-exported requirement tree into this project.

**Mental model.** `import` is the **ingress** counterpart to `export`'s tree format: it reads a tree JSON and grafts those specs into the current store, optionally hanging the whole subtree under a `--parent` you name. The interesting decision is **what to do when an incoming spec collides** with one you already have — that's the `--on-conflict` strategy, and it's the difference between a clean graft and a silent clobber.

**Reach for it when** — you have a tree JSON (from `aida export`, a shared template, another project) and want its specs in *this* graph — seeding a new project from a template hierarchy, or copying a feature's spec subtree between repos.

**Don't reach for it when** — you want to sync with collaborators on the *same* project: that's `aida pull` (the store branch), not import/export. `import` is for moving specs *between distinct graphs*, not for keeping one graph in sync across machines.

**Key options (rationale only).**
- `--parent <ID>` — attach the imported tree *under* an existing spec, so it slots into your hierarchy instead of landing rootless. The grafting point; choose it deliberately.
- `--on-conflict <skip|rename|replace>` — the collision policy, and the option to get right. `skip` (default) leaves your existing spec untouched — the safe default; `rename` keeps both (incoming gets a fresh id); `replace` overwrites yours with the incoming version — the destructive one, use only when you mean it.

**Gotchas.** The default `skip` is conservative on purpose — an import won't silently overwrite work you already have. If an import seems to "do nothing," it likely *skipped* colliding specs; check whether you wanted `rename` (keep both) before reaching for the destructive `replace`.

**Chains with** — the ingress half of the transplant pattern with `aida export`; often run right after `aida init` to seed a fresh project from a template tree.

---

## Where to go next

You've seen the floor the whole system stands on — the git-canonical store and its self-managing cache — plus the recovery kit for the rare day it needs a hand. Where to go from here:
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the everyday two-leg sync verbs (`fetch`/`pull`/`push`/`rebase`) that move this store *with* your code — and where the Done→Completed auto-bump (that `db reconcile-status` recovers) actually happens.
- **[Chapter 1 — Getting started](01-getting-started.md)**: `aida init`, which creates the store, cache, and worktree described here in the first place.
- **[Chapter 7 — Project setup](07-project-setup.md)**: `node` identity, remotes, and the rest of the per-clone setup the distributed store relies on.
