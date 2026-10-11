# Chapter 11 — Working on AIDA itself (contributor-only)

> This chapter is for people **developing AIDA** — building the binary from source, switching between in-repo builds, cutting releases — not for people *using* AIDA on their own projects. If you installed `aida` and are tracking your project's requirements with it, you never need anything here. Skip to [Chapter 1](01-getting-started.md).

These commands exist because AIDA dogfoods itself: the people building AIDA run AIDA *on the AIDA repo*, which means the same binary needs a "switch to the build I just compiled" affordance and a "ship a new release" affordance. That's the whole chapter.

> Manual contract reminder: rationale, not flag tables. `aida <command> --help` is the source of truth for exact flags and defaults.

---

### `aida dev`

**One line** — the AIDA-developer toolbelt: build and promote the single live dev binary, activate an in-repo build, run the dev servers, install shell helpers, cut a release.

**Mental model.** When you're hacking on AIDA, you've got an installed `aida` on PATH *and* a freshly-compiled `target/{release,debug}/aida` in your worktree, and you constantly need to make the *latter* win without losing the former. `aida dev` is that switchboard, pyenv-style: `activate` prepends the in-repo build dir to PATH (so `aida` resolves to your build), `deactivate` undoes it, `status` tells you which build is live and whether it matches HEAD, `shell-init` installs the `aida()` shell wrapper (which auto-evals `dev activate` / `dev deactivate`), and `serve` runs the REST/gRPC server + React dev server together. It also carries `release`/`patch` for cutting versions. Bare `activate` pins the **release** build — the daily-driver profile, so a stale debug binary can't silently end up driving your shell. Opting into `auto` mode makes `activate` **SHA-aware freshest-wins** — it prefers the newest build whose embedded git SHA matches (or is an ancestor of) the current branch HEAD, so switching branches between builds doesn't silently leave you running the *other* branch's binary.

**The live binary.** There is exactly one dev `aida` that every session, hook, MCP server and relay runs: `~/.local/bin/aida` and `~/.local/bin/aida-server` are links into a live directory (`~/.local/share/aida/live/`), which sits *outside* `target/`. `aida dev build` is the only command that changes it, and only after the new build has proved itself: it runs an incremental `--profile agent` build of the workspace, smoke-tests the new binaries (`aida --version`, `aida list --limit 1` against the current store, `aida-server --version`), and then atomically renames them into the live directory, keeping the outgoing pair in `live/prev/`. Because the live pair is a copy, `cargo clean`, a stray `cargo build`, or `make build-fast` (still there, build-only) can never break or replace what everyone is running.

**Reach for it when** — you've merged to `main` and want everyone running it (`aida dev build` in the primary checkout); you're developing AIDA and want `aida` to mean "the binary I just built" (`aida dev activate`, auto-eval'd by the `aida()` wrapper); you want to run the server + dashboard locally (`aida dev serve`); or you're checking which build is actually live (`aida dev status`).

**Don't reach for it when** — you're a *user*, not a contributor. None of this applies to using AIDA on your own project. And don't `eval "$(aida dev activate)"` *inside* the `aida()` shell wrapper — the wrapper auto-evals it; wrapping it again double-evals and breaks the shell.

**Key options (rationale only).**
- `build` — promotes only from the primary checkout, on `main`, with no tracked modifications and HEAD equal to `origin/main`; untracked files only warn. Anything else refuses before building anything, naming why. In a worktree it builds and smoke-tests but never promotes, and prints how to try the build in your shell (`aida dev activate auto` from that worktree). A failed build or smoke test exits non-zero, names the step, and leaves the live binaries untouched.
- `build --no-promote` — build and smoke-test anywhere without promoting. `build --allow-dirty` — promote anyway from a dirty or off-`main` checkout; `aida dev status` then shows the `+dirty` SHA so nobody mistakes it for `main`.
- `build --clean` / `--clean=full` — the escape hatch for incremental-cache trouble (baffling compile errors after an unrelated edit). Bare `--clean` deletes only `target/agent/incremental` and keeps the compiled dependencies; `--clean=full` runs `cargo clean --profile agent`. Either way the normal build, smoke test and promote follow. Build and smoke-test failures suggest it.
- `build --install-links` — one-time setup: points `~/.local/bin/aida` and `aida-server` at the live directory. It lists any real file or stale link it would replace and asks first (`--yes` skips the question; a replaced real file is kept as `<name>.pre-live`).
- `build --after-wave` — wait for a live drain wave pinned to the current binary to finish instead of refusing.
- `rollback` — swaps `live/prev/` back in, in one step; run it again to roll forward. With no previous build it says so and changes nothing.
- `activate [debug|release|auto]` (and `--debug`/`--release`/`--auto`) — the positional/flag profile is a **sticky pin**: once pinned, plain `aida dev activate` keeps honoring it across re-activations. The default (no pin) is the **release** build — the daily driver. `auto` is the explicit opt-in to freshest-wins (newest SHA-matched of debug vs release, re-picked on every activate), and it's sticky too until you pin `debug` or `release`. Pin `debug` only when you're deliberately testing that profile.
- `activate --repo <path>` — override the AIDA-repo location (defaults to CWD if it looks like the repo, else `$AIDA_DEV_REPO`). For activating an in-repo build from *outside* the repo.
- `serve --no-web` / `--rest-port` / `--grpc-port` / `--web-port` — the React dev server only starts when run from the AIDA repo (vite lives there); `--no-web` skips it even then, and the port overrides exist for when 8080/50051/5173 are taken.
- `shell-init --install` — appends the `aida()` shell wrapper to your rc file directly instead of printing it to paste. The one-time setup that makes `aida dev activate` a single word (the wrapper auto-evals it). It also installs the managed prompt hook so `(aida-debug↻)` / `(aida-release⇄)` reflect build staleness without hand-editing your profile. The wrapper auto-evals **only on exit 0**: a failing `role enter` / `session start` / `session end` / `worktree enter` / `worktree exit` has its real message printed to stderr and its status returned, instead of its error text being eval'd as shell. After upgrading, re-run `aida dev shell-init --install` (or open a new shell) to pick up the current wrapper.
- `ps1` — prompt-only probe. Prints nothing when the active build matches HEAD, `⇄` when a HEAD-matching build exists in the other profile and `aida dev activate` can flip to it, or `↻` when no built binary matches HEAD and you need to rebuild.
- `release [BUMP]` / `patch` — `dev release` is the full release sequence (bump + tag + push via `scripts/release.sh` → wait for published tarballs → upgrade sibling installs); `patch` is the alias for its most common case. See the top-level `aida release` below for the richer flag surface.

**Gotchas.** `aida dev status`'s SHA verdict (`exact match` / `ancestor of HEAD` / `DIVERGED from HEAD`) is the thing to read before trusting a test result — a `DIVERGED` verdict means your live binary was built from a different branch's source than HEAD, so your "fix" might be running against stale code. The PS1 marker has two stale remedies: `⇄` means re-run `aida dev activate` to switch to the already-built HEAD match; `↻` means rebuild with `cargo build` or `cargo build --release`. The cross-worktree cargo-cache trap also bites here: if a `session end` removed a worktree, a sibling worktree's `cargo build` can fail on the deleted worktree's absolute paths — recover with `cargo clean -p <crate>`.

Live drain waves pin their launching binary SHA and mtime. `aida dev build` (and `make build-fast`) refuse to replace that binary while the wave is live; `aida dev build --after-wave` (or `make build-fast AFTER_WAVE=1`) waits for the wave's `QueueDrained` event before continuing. `aida dev status` lists the live, previous and latest-built SHAs and flags when the latest build hasn't been promoted. `aida dev activate` only updates the calling shell's PATH, so it remains available during a live wave. Note that activating in the primary checkout puts its `target/agent/` first on that shell's PATH, which bypasses the live directory: that shell runs whatever cargo last built there, smoke-tested or not, until `aida dev deactivate`.

The promote holds an exclusive lock on `live/.lock` from staging to the final rename (and a rollback holds it too), so two promotes never interleave; the second one refuses. The smoke test runs against the staged copies in `live/.incoming/`, so the pair that goes live is exactly the pair that was tested. The two binaries are renamed one after the other, so a crash in between can leave a mixed pair; re-run `aida dev build` (or `aida dev rollback`) to repair it. The next promote or rollback also tidies any other leftovers from an interrupted run. `aida dev build --clean=full` refuses while `~/.local/bin/aida` (or the `aida` on your PATH) still points into the build's own `target/`, because the clean would delete the binary everyone is running; run `aida dev build --install-links` first. The promotion gate compares HEAD with your local `origin/main` ref and does not fetch, so run `git fetch` first if that ref might be stale. `aida dev rollback` respects the live-wave guard too (`--after-wave` waits, `--force` skips it).

**Chains with** — `aida dev build --install-links` once in the primary checkout, then `aida dev build` after each merge; `aida dev shell-init --install` once, then `aida dev activate` per shell; `aida dev status` to confirm; `aida dev serve` to run the stack; `aida dev release` (or the top-level `aida release`) to ship.

---

### `aida release`

**One line** — the one-verb release: sync the store, bump+tag+push, wait for the published tarballs, upgrade sibling installs.

**Mental model.** Cutting an AIDA release is a multi-step ritual (version bump, changelog regen, tag, push, wait for the GitHub Actions build, propagate to your other installs), and `aida release` wraps the whole sequence so you don't have to remember it — it's the top-level counterpart to `aida dev release`, with a richer command surface. The `aida release check` preview is the safety rail: it shows current→target version, the step sequence, and the repo/branch/tree state *without acting*, so you confirm before anything is tagged.

**Reach for it when** — you're a maintainer cutting a version after a merge spree, and you want the full sequence run for you rather than driving `scripts/release.sh` by hand. Start with `aida release check`.

**Don't reach for it when** — you're not the releaser. AIDA is Linux-only, so there is no cross-platform pre-release gate to wait on.

**Key options (rationale only).**
- `aida release check` — preview the planned release without acting. Always run this first; it surfaces a dirty tree or wrong-branch state before you've tagged anything irreversible.
- `aida release patch` / `aida release minor` / `aida release major` — the semver level (default `patch`). The single most consequential choice; pick deliberately.
- `--after-pr <N>` — land an in-flight PR *first* (wait for its checks, squash-merge, sync main) and *then* release. The way to fold a last-minute fix into the release without a separate manual merge round-trip; it refuses if the PR's checks fail, so it can't ship a red PR.

**Gotchas.** A clean `git status` is *not* "no work to release" — committed-but-unpushed work is on the branch; read what `aida release check` reports about tree/branch state before assuming.

**Chains with** — runs after the merge that finishes the last spec; the version tag flips merged-since-last-tag specs to **Released** (the final lifecycle state, Ch.4). `aida dev release`/`patch` is the dev-toolbelt entry point into the same machinery.

---

### `aida upgrade`

**One line** — upgrade the installed `aida` binary to the latest (or a specified) release.

**Mental model.** `upgrade` detects *how* aida was installed (cargo vs pre-built binary) and uses the matching strategy. From a developer build, with no `--target`, it also scans common install locations and offers to upgrade any **stale sibling installs** — handy when aida is on PATH in several places.

**Reach for it when** — a new release is out and you want this machine on it; `--check` first to compare current/sibling versions against the latest *without* installing.

**Don't reach for it when** — you're on an in-repo dev build you're actively changing (don't overwrite your own build — use the `aida dev` activation flow). `upgrade` is for *installed* aida, not the one you're hacking on.

**Chains with** — the counterpart to `aida release`; `--check` pairs with the statusline's version-staleness hint.

---

## Where to go next

That's the contributor surface. If you came here by accident and you're a *user*, not a developer of AIDA:
- **[Chapter 1 — Getting started](01-getting-started.md)**: where you actually want to be.
- **[Chapter 4 — Git & lifecycle](04-git-lifecycle.md)**: the **Released** state that `aida release` produces, in the context of the full lifecycle.
- For the deep developer workflow (CI split, cross-platform cadence, the cargo-cache gotcha), the AIDA repo's own `CLAUDE.md` is the authority — this manual covers the *commands*, not the contribution process.
