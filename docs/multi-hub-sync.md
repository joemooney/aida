# Multi-hub sync: keeping github and gitlab from drifting

AIDA stores two things in git: the **code** (on `main`) and the **requirement store**
(the orphan `aida-store` branch). When a project has more than one hub — e.g. a public
github `origin` plus a personal gitlab mirror — both branches must stay identical on every
hub, or the shared substrate forks.

## How drift happens

AIDA's store sync (`aida db sync`, and the auto-push paths) historically pushed the store
to a **single** remote named `origin`. A clone whose canonical hub is a *different* remote
(e.g. a work machine that pushes the store to gitlab, not github) drifts silently: each hub
accumulates commits the other never sees, with no fan-out and no warning. Reconciling after
the fact means a careful CRDT union-merge of the diverged tips (see `docs/git-verb-surface.md`).

The same happens to **code** whenever a push targets one hub only (`git push origin main`
without also pushing gitlab).

## The three legs of prevention

### 1. Detect — `aida remote status`

Read-only. Shows, per configured remote, how far the shared branches (`main` + `aida-store`)
are ahead/behind local, and flags any remote-vs-remote divergence. Exits non-zero on
divergence, so it can gate CI or a pre-push hook.

```
aida remote status            # best-effort fetch, then compare
aida remote status --no-fetch # offline: compare existing tracking refs
aida remote status --json     # machine-readable
```

The same check is a health finding: `aida doctor --category remote-drift`.

### 2. Store fan-out — `[store.sync] mirror_remotes`

Make every store push go to **all** hubs, not just `origin`:

```toml
[store.sync]
mirror_remotes = ["gitlab"]
```

After a successful `origin` push, `aida db sync --push` pushes `aida-store` to each mirror.
**Best-effort**: a non-fast-forward / unreachable / unconfigured mirror leg WARNS (with a
reconcile hint) and is skipped — it never fails the sync, because a mirror may be
intentionally behind (e.g. mid-reconcile). `aida init` scaffolds this as a commented stub.

### 3. Code fan-out

The code leg is pushed by plain `git`, which AIDA doesn't wrap, so pick one:

- **Native git multi-pushurl** (the clean end-state):
  `git remote set-url --add --push origin <mirror-url>` makes `git push origin` push `main`
  to both hubs natively.
  **Caveat:** this also fans out the *store* push, so while `aida-store` is intentionally
  diverged (mid-reconcile) the store leg to the mirror will be rejected and confuse
  `aida db sync`. Enable native multi-pushurl only once both hubs' `aida-store` are
  reconciled. Until then, use `mirror_remotes` (leg 2, best-effort) for the store and
  re-sync code manually or via a code-only pre-push hook (tracked follow-up).
- **Manual, until reconciled:** `git push origin/main:main` to each mirror after a code push,
  or `aida remote status` in a pre-push hook to at least *catch* drift before it lands.

## Recommended setup

```bash
# 1. add the mirror remote (once)
git remote add gitlab <mirror-url>

# 2. fan out the store leg
#    edit .aida/config.toml:
#    [store.sync]
#    mirror_remotes = ["gitlab"]

# 3. sanity-check anytime
aida remote status
```

Once every hub's `aida-store` is reconciled (no BUG-700-style held divergence), add the
native multi-pushurl (leg 3) so the *code* leg fans out automatically too.

## When drift has already happened

Don't force-push a shared branch to "fix" it — that destroys the other hub's commits.
Reconcile the divergent tips into a superset (a merge commit lets **both** hubs fast-forward
to it) and push that to every hub. For the store, the CRDT union in `conflict.rs` field-merges
concurrent spec edits; for a hub carrying unique node data (ID-range reservations, new specs),
merge it in rather than overwrite. See `docs/git-verb-surface.md` for the verb-surface rules.

## Related

- `docs/git-verb-surface.md` — the two-leg git-mirror verb conventions.
- `aida remote status`, `aida doctor --category remote-drift` — detection.
- STORY-760 — the drift-prevention epic; BUG-700 — the employer-content history scrub that
  currently blocks this repo's `aida-store` from reconciling across hubs.
