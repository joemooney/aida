# Where does the AIDA store live?

AIDA's requirement store is a git-canonical object store (one YAML file per spec).
*Where* that store sits relative to your code repo is a deployment choice. There
are three layouts; all are driven by one config value, `[deployment] store_path`
in `.aida/config.toml`, which the resolver follows verbatim (relative paths
resolve against the project root — the directory containing `.aida/`).

## 1. Orphan branch (single repo) — the default

```
aida init
```

The store is the orphan `aida-store` branch inside your code repo, checked out
as a gitignored `.aida-store/` worktree. One repo, store hidden in a branch.
Nothing to share, nothing extra on disk. This is the right default for a single
project.

## 2. Sibling store (multi-repo, same machine) — `--sibling` / `--store-path`

```
aida init --sibling                       # sugar for --store-path ../aida-store
aida init --store-path ../aida-store       # explicit, identical
aida init --store-path /srv/aida-store     # absolute path also works
```

The store is a **separate git repo** at the given path. `--sibling` defaults it
to `../aida-store` — a true sibling directory one level up:

```
workspace/
  repo-a/        ← code         (.aida/config.toml → store_path = "../aida-store")
  repo-b/        ← code         (.aida/config.toml → store_path = "../aida-store")
  aida-store/    ← the shared store
```

Several code repos that point at the **same** `store_path` share one store. They
also share one **dispenser** (`aida-store/.aida/dispenser.toml`), which
serializes id allocation atomically — so two repos never collide on an id even
though they use the same node. **No separate node id is needed** for the
same-machine sibling case.

A second repo **joins** an existing store with `--attach`:

```
cd repo-b
aida init --sibling --attach               # or --store-path <PATH> --attach
```

`--attach` writes the local config + builds the read cache and **never touches
the store's contents** (no re-seed, no overwrite). Without `--attach`, init on
an existing populated store refuses rather than risk overwriting another repo's
requirements — pass `--attach` to join or `--force` to deliberately wipe.

`--store-path` accepts a **sibling** (`../aida-store`), an **absolute** path
(`/srv/aida-store`, e.g. a machine-wide shared store), or a **nested**
sub-directory / submodule (`vendor/aida-store`).

## 3. Submodule — a pinned snapshot / cross-machine share

A submodule is a valid `store_path` target (`store_path = "aida-store"` where
`aida-store/` is a submodule), but it behaves differently from a shared working
directory, and the difference matters:

| Layout | Sharing behaviour |
|---|---|
| **Shared working dir** (sibling or absolute) | Both repos read/write the **same working tree** — changes are instant. Smoothest for live, same-machine sharing. |
| **Submodule** | The store is **pinned to a commit**. To share live you commit in the store, bump the submodule pointer in *each* code repo, and `git submodule update` to sync. Good for **vendoring a snapshot** or sharing **across machines** via the store's remote; heavier for live local sharing. |

For sharing a *live* store across **machines**, give the store a git remote
(`--registry-remote`) and clone it on each machine; each clone gets its own node
id via `aida node acquire` so independent dispensers don't collide. That is the
distributed (multi-machine) story; the sibling/`--store-path` layouts above are
the same-machine story.

## Quick reference

| You want… | Command |
|---|---|
| A single project | `aida init` |
| Several repos in one folder to share a store | `aida init --sibling` (in each), then `--attach` for the 2nd+ |
| The store at a specific path | `aida init --store-path <PATH>` |
| Join an existing shared store | `aida init --store-path <PATH> --attach` |
| Wipe + re-create a store | add `--force` |

trace:STORY-676 | ai:claude
