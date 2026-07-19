# Chapter 11 — Working on AIDA itself (contributor-only)

> This chapter is for people **developing AIDA** — building the binary from source, switching between in-repo builds, cutting releases — not for people *using* AIDA on their own projects. If you installed `aida` and are tracking your project's requirements with it, you never need anything here. Skip to [Chapter 1](01-getting-started.md).

These commands exist because AIDA dogfoods itself: the people building AIDA run AIDA *on the AIDA repo*, which means the same binary needs a "switch to the build I just compiled" affordance and a "ship a new release" affordance. That's the whole chapter.

> Manual contract reminder: rationale, not flag tables. `aida <command> --help` is the source of truth for exact flags and defaults.

---

### `aida dev`

**One line** — the AIDA-developer toolbelt: activate the in-repo build, run the dev servers, install shell helpers, cut a release.

**Mental model.** When you're hacking on AIDA, you've got an installed `aida` on PATH *and* a freshly-compiled `target/{release,debug}/aida` in your worktree, and you constantly need to make the *latter* win without losing the former. `aida dev` is that switchboard, pyenv-style: `activate` prepends the in-repo build dir to PATH (so `aida` resolves to your build), `deactivate` undoes it, `status` tells you which build is live and whether it matches HEAD, `shell-init` installs the `aida()` shell wrapper (which auto-evals `dev activate` / `dev deactivate`), and `serve` runs the REST/gRPC server + React dev server together. It also carries `release`/`patch` for cutting versions. Bare `activate` pins the **release** build — the daily-driver profile, so a stale debug binary can't silently end up driving your shell. Opting into `auto` mode makes `activate` **SHA-aware freshest-wins** — it prefers the newest build whose embedded git SHA matches (or is an ancestor of) the current branch HEAD, so switching branches between builds doesn't silently leave you running the *other* branch's binary.

**Reach for it when** — you're developing AIDA and want `aida` to mean "the binary I just built" (`aida dev activate`, auto-eval'd by the `aida()` wrapper); you want to run the server + dashboard locally (`aida dev serve`); or you're checking which build is actually live (`aida dev status`).

**Don't reach for it when** — you're a *user*, not a contributor. None of this applies to using AIDA on your own project. And don't `eval "$(aida dev activate)"` *inside* the `aida()` shell wrapper — the wrapper auto-evals it; wrapping it again double-evals and breaks the shell.

**Key options (rationale only).**
- `activate [debug|release|auto]` (and `--debug`/`--release`/`--auto`) — the positional/flag profile is a **sticky pin**: once pinned, plain `aida dev activate` keeps honoring it across re-activations. The default (no pin) is the **release** build — the daily driver. `auto` is the explicit opt-in to freshest-wins (newest SHA-matched of debug vs release, re-picked on every activate), and it's sticky too until you pin `debug` or `release`. Pin `debug` only when you're deliberately testing that profile.
- `activate --repo <path>` — override the AIDA-repo location (defaults to CWD if it looks like the repo, else `$AIDA_DEV_REPO`). For activating an in-repo build from *outside* the repo.
- `serve --no-web` / `--rest-port` / `--grpc-port` / `--web-port` — the React dev server only starts when run from the AIDA repo (vite lives there); `--no-web` skips it even then, and the port overrides exist for when 8080/50051/5173 are taken.
- `shell-init --install` — appends the `aida()` shell wrapper to your rc file directly instead of printing it to paste. The one-time setup that makes `aida dev activate` a single word (the wrapper auto-evals it). It also installs the managed prompt hook so `(aida-debug↻)` / `(aida-release⇄)` reflect build staleness without hand-editing your profile.
- `ps1` — prompt-only probe. Prints nothing when the active build matches HEAD, `⇄` when a HEAD-matching build exists in the other profile and `aida dev activate` can flip to it, or `↻` when no built binary matches HEAD and you need to rebuild.
- `release [BUMP]` / `patch` — `dev release` is the full release sequence (bump + tag + push via `scripts/release.sh` → wait for published tarballs → upgrade sibling installs); `patch` is the alias for its most common case. See the top-level `aida release` below for the richer flag surface.

**Gotchas.** `aida dev status`'s SHA verdict (`exact match` / `ancestor of HEAD` / `DIVERGED from HEAD`) is the thing to read before trusting a test result — a `DIVERGED` verdict means your live binary was built from a different branch's source than HEAD, so your "fix" might be running against stale code. The PS1 marker has two stale remedies: `⇄` means re-run `aida dev activate` to switch to the already-built HEAD match; `↻` means rebuild with `cargo build` or `cargo build --release`. The cross-worktree cargo-cache trap also bites here: if a `session end` removed a worktree, a sibling worktree's `cargo build` can fail on the deleted worktree's absolute paths — recover with `cargo clean -p <crate>`.

**Chains with** — `aida dev shell-init --install` once, then `aida dev activate` per shell; `aida dev status` to confirm; `aida dev serve` to run the stack; `aida dev release` (or the top-level `aida release`) to ship.

---

### `aida release`

**One line** — the one-verb release: sync the store, bump+tag+push, wait for the published tarballs, upgrade sibling installs.

**Mental model.** Cutting an AIDA release is a multi-step ritual (version bump, changelog regen, tag, push, wait for the GitHub Actions build, propagate to your other installs), and `aida release` wraps the whole sequence so you don't have to remember it — it's the top-level counterpart to `aida dev release`, with a richer flag surface. The `--check` preview is the safety rail: it shows current→target version, the step sequence, and the repo/branch/tree state *without acting*, so you confirm before anything is tagged.

**Reach for it when** — you're a maintainer cutting a version after a merge spree, and you want the full sequence run for you rather than driving `scripts/release.sh` by hand. Start with `--check`.

**Don't reach for it when** — you're not the releaser, or cross-platform CI isn't green. A published release requires the cross-platform matrix green within 24h of tagging; `release` runs that pre-release gate for you, and you should **not** `--skip-xplat-check` a real release just to move faster — that's how Windows debt ships to users.

**Key options (rationale only).**
- `--check` — preview the planned release without acting. Always run this first; it surfaces a dirty tree or wrong-branch state before you've tagged anything irreversible.
- `--patch` / `--minor` / `--major` — the semver level (default `patch`). The single most consequential choice; pick deliberately.
- `--after-pr <N>` — land an in-flight PR *first* (wait for its checks, squash-merge, sync main) and *then* release. The way to fold a last-minute fix into the release without a separate manual merge round-trip; it refuses if the PR's checks fail, so it can't ship a red PR.
- `--skip-xplat-check` — bypass the cross-platform pre-release gate. Exists for emergencies, explicitly *not recommended for a published release* — the gate is there because PR CI is Linux-only and cross-platform runs nightly-only, so this is the only thing standing between you and untested Windows/macOS behavior.
- `--skip-docs-check` — bypass the release-time documentation gate. AIDA gates documentation currency at *release* (not on every PR), so PRs ship fast and docs are verified once, where staleness reaches users. The gate (`scripts/docs-gate.sh`, run before the tag) blocks if the regenerated `CHANGELOG.md` has no section for this version or isn't deterministic, and it *advises* (non-blocking) on any plan files touched since the previous tag that fail `aida plan verify`. Skip it (`--skip-docs-check` / `AIDA_SKIP_DOCS_CHECK=1`) only in a pinch — a published release should ship a current changelog.

**Gotchas.** A clean `git status` is *not* "no work to release" — committed-but-unpushed work is on the branch; read what `--check` reports about tree/branch state before assuming. And the cross-platform gate's 24h freshness window means an old green run can go stale mid-release — if `--check` says the gate needs a fresh run, let it dispatch one rather than skipping.

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
